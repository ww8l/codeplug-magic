//! FTM-510 Setup Menu settings: decode from, and apply to, `CLNFTM510D.dat`.
//!
//! The field table and the profile form's schema are generated together from
//! one measurement (`scratchpad/yaesu_ftm510/adms/gen_settings.py`, fed by the
//! ADMS-18 campaign: 14 bit-signature passes, one-field isolation saves, and
//! token / radio-button / grid passes), so the decoder and the form cannot
//! drift. Every address was measured on files Yaesu's own ADMS-18 wrote for
//! this radio; none is copied from the FTM-300, though most turned out to match.
//!
//! The FTM-300's machinery (`yaesu_ftm300::settings`) covered most of this
//! radio, but not all of it: S-DX lives in three bits of three bytes, and a few
//! band-A/B settings are written to both bands' bytes at once. Hence two kinds
//! the FTM-300 never needed — `Bits` and a `mirror` byte — and this module of
//! its own rather than a change to a shipped radio's code.
//!
//! Settings share bytes all over `0x80-0xFF`, so every write is
//! read-modify-write: the other bits in the byte are someone else's setting.

use serde_json::{Map, Value};

pub(crate) struct SF {
    pub key: &'static str,
    /// File offset into `CLNFTM510D.dat`.
    pub byte: u32,
    /// A second byte the same bits are written to (0 = none): ADMS-18 writes
    /// Rear Out and SQL Expansion to both bands' bytes.
    pub mirror: u32,
    pub kind: SK,
}

pub(crate) enum SK {
    /// One bit. `active_low` stores ON as 0 — measured, never assumed.
    Bool { shift: u8, active_low: bool },
    /// `width` bits starting at `shift`, mapped to labels by stored value.
    Enum {
        shift: u8,
        width: u8,
        labels: &'static [(u32, &'static str)],
    },
    /// Big-endian u16 mapped to labels by stored value.
    Enum16 { labels: &'static [(u32, &'static str)] },
    /// A value assembled from single bits scattered over the file: bit i of
    /// the value is `bits[i]` = (offset, bit). `byte` is the first of them.
    Bits {
        bits: &'static [(u32, u8)],
        labels: &'static [(u32, &'static str)],
    },
    /// Fixed-length ASCII, padded with `pad`, restricted to `set`.
    Text { len: u32, pad: u8, set: TS },
    /// One byte holding the number itself (the SmartBeaconing parameters).
    /// The schema carries the manual's range; `settings_bounds` enforces it.
    Uint,
    /// A DTMF memory: one key per byte (0-9, A-D = 0x0A-0x0D, `*` 0x0E, `#`
    /// 0x0F), padded with 0xFF — measured with ADMS-18 on all sixteen keys.
    Dtmf { len: u32 },
}

/// What a text field may hold. Checked, never filtered: stripping a character
/// out would write a different call sign or message than the one asked for.
#[derive(Clone, Copy)]
pub(crate) enum TS {
    /// Call sign: A-Z and 0-9 (stored upper case).
    Call,
    /// The radio's own call sign also takes `-` and `/`.
    CallPunct,
    /// An APRS message group or bulletin name: call-sign characters plus `*`
    /// and `?` (the radio's defaults are `ALL******` and `BLN?`).
    Group,
    /// Free text — APRS messages and status texts: printable ASCII.
    Plain,
}

include!("ftm510_settings_table.rs");

fn mask(width: u8) -> u32 {
    (1u32 << width) - 1
}

fn label_for(labels: &[(u32, &str)], raw: u32) -> Value {
    match labels.iter().find(|(v, _)| *v == raw) {
        Some((_, l)) => Value::String((*l).to_string()),
        // An unknown stored value comes back as `#N` rather than a wrong label,
        // so a write of an untouched profile cannot normalise it away.
        None => Value::String(format!("#{raw}")),
    }
}

/// Every byte a field may change.
fn touches(f: &SF) -> Vec<usize> {
    let at = f.byte as usize;
    let mut out: Vec<usize> = match &f.kind {
        SK::Enum16 { .. } => vec![at, at + 1],
        SK::Text { len, .. } | SK::Dtmf { len } => (at..at + *len as usize).collect(),
        SK::Bits { bits, .. } => bits.iter().map(|(b, _)| *b as usize).collect(),
        _ => vec![at],
    };
    if f.mirror != 0 {
        out.push(f.mirror as usize);
    }
    out
}

fn decode_field(img: &[u8], f: &SF) -> Value {
    let at = f.byte as usize;
    match &f.kind {
        SK::Bool { shift, active_low } => Value::Bool(((img[at] >> shift) & 1 == 1) != *active_low),
        SK::Enum { shift, width, labels } => label_for(labels, (img[at] as u32 >> shift) & mask(*width)),
        SK::Enum16 { labels } => label_for(labels, u16::from_be_bytes([img[at], img[at + 1]]) as u32),
        SK::Bits { bits, labels } => {
            let raw = bits
                .iter()
                .enumerate()
                .fold(0u32, |v, (i, (b, bit))| v | ((((img[*b as usize] >> bit) & 1) as u32) << i));
            label_for(labels, raw)
        }
        SK::Uint => Value::from(img[at]),
        SK::Text { len, pad, .. } => {
            let raw = &img[at..at + *len as usize];
            let cut = raw.iter().position(|&b| b == *pad || b == 0).unwrap_or(raw.len());
            Value::String(String::from_utf8_lossy(&raw[..cut]).trim_end().to_string())
        }
        SK::Dtmf { len } => Value::String(
            img[at..at + *len as usize]
                .iter()
                .take_while(|&&b| b < 16)
                .map(|&b| DTMF_KEYS[b as usize] as char)
                .collect(),
        ),
    }
}

const DTMF_KEYS: &[u8; 16] = b"0123456789ABCD*#";

/// Decode every known setting, shaped like the profile form.
pub(crate) fn decode_settings(img: &[u8]) -> Value {
    let mut out = Map::new();
    for f in FTM510_SETTINGS_FIELDS {
        if touches(f).iter().any(|&i| i >= img.len()) {
            continue;
        }
        out.insert(f.key.to_string(), decode_field(img, f));
    }
    Value::Object(out)
}

fn raw_for(labels: &[(u32, &str)], v: &Value) -> Option<u32> {
    let s = v.as_str()?;
    labels
        .iter()
        .find(|(_, l)| *l == s)
        .map(|(raw, _)| *raw)
        // An unknown raw value handed back by `label_for` round-trips as itself.
        .or_else(|| s.strip_prefix('#')?.parse().ok())
}

fn text_fits(s: &str, len: u32, set: TS) -> bool {
    let ok = |c: char| match set {
        TS::Call => c.is_ascii_alphanumeric(),
        TS::CallPunct => c.is_ascii_alphanumeric() || c == '-' || c == '/',
        TS::Group => c.is_ascii_alphanumeric() || c == '*' || c == '?',
        TS::Plain => (' '..='~').contains(&c),
    };
    s.chars().count() <= len as usize && s.chars().all(ok)
}

fn put_bits(img: &mut [u8], at: usize, shift: u8, width: u8, raw: u32) {
    let m = (mask(width) << shift) as u8;
    img[at] = (img[at] & !m) | ((raw << shift) as u8 & m);
}

/// Write a profile's settings into the image, returning how many fields were
/// applied. Unknown keys and wrongly-shaped values are skipped, never guessed;
/// a field already holding the value is left untouched (no bytes rewritten).
pub(crate) fn apply_settings(img: &mut [u8], settings: &Map<String, Value>) -> usize {
    let mut written = 0;
    for f in FTM510_SETTINGS_FIELDS {
        let Some(v) = settings.get(f.key) else {
            continue;
        };
        if touches(f).iter().any(|&i| i >= img.len()) {
            continue;
        }
        if decode_field(img, f) == *v {
            written += 1;
            continue;
        }
        let at = f.byte as usize;
        let mirror = f.mirror as usize;
        let ok = match &f.kind {
            SK::Bool { shift, active_low } => match v.as_bool() {
                Some(b) => {
                    let bit = (b != *active_low) as u32;
                    put_bits(img, at, *shift, 1, bit);
                    if mirror != 0 {
                        put_bits(img, mirror, *shift, 1, bit);
                    }
                    true
                }
                None => false,
            },
            SK::Enum { shift, width, labels } => match raw_for(labels, v) {
                Some(raw) if raw <= mask(*width) => {
                    put_bits(img, at, *shift, *width, raw);
                    if mirror != 0 {
                        put_bits(img, mirror, *shift, *width, raw);
                    }
                    true
                }
                _ => false,
            },
            SK::Enum16 { labels } => match raw_for(labels, v) {
                Some(raw) if raw <= 0xFFFF => {
                    img[at..at + 2].copy_from_slice(&(raw as u16).to_be_bytes());
                    true
                }
                _ => false,
            },
            SK::Bits { bits, labels } => match raw_for(labels, v) {
                Some(raw) if raw < 1 << bits.len() => {
                    for (i, (b, bit)) in bits.iter().enumerate() {
                        put_bits(img, *b as usize, *bit, 1, (raw >> i) & 1);
                    }
                    true
                }
                _ => false,
            },
            SK::Uint => match v.as_u64() {
                Some(n) if n <= 0xFF => {
                    img[at] = n as u8;
                    true
                }
                _ => false,
            },
            SK::Dtmf { len } => match v.as_str().map(|s| s.to_ascii_uppercase()) {
                Some(s) if s.len() <= *len as usize && s.bytes().all(|c| DTMF_KEYS.contains(&c)) => {
                    let field = &mut img[at..at + *len as usize];
                    field.fill(0xFF);
                    for (d, c) in field.iter_mut().zip(s.bytes()) {
                        *d = DTMF_KEYS.iter().position(|&k| k == c).unwrap() as u8;
                    }
                    true
                }
                _ => false,
            },
            // A value the field cannot hold is skipped, not cut down to fit.
            SK::Text { len, pad, set } => match v.as_str() {
                Some(s) if text_fits(s, *len, *set) => {
                    let s = match set {
                        TS::Plain => s.to_string(),
                        _ => s.to_ascii_uppercase(),
                    };
                    let field = &mut img[at..at + *len as usize];
                    field.fill(*pad);
                    field[..s.len()].copy_from_slice(s.as_bytes());
                    true
                }
                _ => false,
            },
        };
        written += usize::from(ok);
    }
    written
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::radios::yaesu_ftm510::sd_image::IMAGE_LEN;

    fn schema() -> Vec<Value> {
        serde_json::from_str(include_str!("../../ftm510_settings_schema.json")).unwrap()
    }

    /// The generator emits both halves; this is the guard that they still agree.
    #[test]
    fn table_and_schema_agree() {
        let fields: Vec<Value> = schema().into_iter().filter(|f| f["type"] != "section").collect();
        assert_eq!(fields.len(), FTM510_SETTINGS_FIELDS.len());
        for (s, f) in fields.iter().zip(FTM510_SETTINGS_FIELDS) {
            assert_eq!(s["key"], f.key);
            match &f.kind {
                SK::Bool { .. } => assert_eq!(s["type"], "boolean", "{}", f.key),
                SK::Enum { labels, .. } | SK::Enum16 { labels } | SK::Bits { labels, .. } => {
                    let opts: Vec<&str> = s["options"].as_array().unwrap().iter().map(|o| o.as_str().unwrap()).collect();
                    let want: Vec<&str> = labels.iter().map(|(_, l)| *l).collect();
                    assert_eq!(opts, want, "{}", f.key);
                }
                SK::Text { len, .. } => {
                    assert_eq!(s["type"], "text", "{}", f.key);
                    assert_eq!(s["max_length"], *len, "{}", f.key);
                }
                SK::Uint => assert_eq!(s["type"], "integer", "{}", f.key),
                SK::Dtmf { len } => {
                    assert_eq!(s["type"], "text", "{}", f.key);
                    assert_eq!(s["max_length"], *len, "{}", f.key);
                }
            }
        }
        let mut keys: Vec<&str> = FTM510_SETTINGS_FIELDS.iter().map(|f| f.key).collect();
        keys.sort_unstable();
        let n = keys.len();
        keys.dedup();
        assert_eq!(keys.len(), n, "duplicate settings key");
    }

    /// Every value of every field written and read back, and nothing outside
    /// the field's own bytes moved — the shared-byte fields must not trample
    /// each other.
    #[test]
    fn every_value_round_trips_alone() {
        let base: Vec<u8> = (0..IMAGE_LEN).map(|i| (i * 37 % 251) as u8).collect();
        for f in FTM510_SETTINGS_FIELDS {
            let values: Vec<Value> = match &f.kind {
                SK::Bool { .. } => vec![Value::Bool(true), Value::Bool(false)],
                SK::Enum { labels, .. } | SK::Enum16 { labels } | SK::Bits { labels, .. } => {
                    labels.iter().map(|(_, l)| Value::String((*l).into())).collect()
                }
                SK::Text { set: TS::Plain, .. } => vec![Value::String("Mobile 73".into())],
                SK::Text { .. } => vec![Value::String("WW8L".into())],
                SK::Uint => vec![Value::from(1), Value::from(255)],
                SK::Dtmf { .. } => vec![Value::String("0123456789ABCD*#".into()), Value::String("73".into())],
            };
            for v in values {
                let mut img = base.clone();
                let mut m = Map::new();
                m.insert(f.key.into(), v.clone());
                assert_eq!(apply_settings(&mut img, &m), 1, "{} {v}", f.key);
                assert_eq!(decode_settings(&img)[f.key], v, "{}", f.key);
                let own = touches(f);
                let moved: Vec<usize> = (0..img.len()).filter(|&i| img[i] != base[i]).collect();
                assert!(moved.iter().all(|i| own.contains(i)), "{} moved {moved:x?}", f.key);
                if f.mirror != 0 {
                    assert_eq!(decode_field(&img, &SF { key: "", byte: f.mirror, mirror: 0, kind: clone_kind(&f.kind) }), v, "{} mirror", f.key);
                }
            }
        }
    }

    fn clone_kind(k: &SK) -> SK {
        match k {
            SK::Bool { shift, active_low } => SK::Bool { shift: *shift, active_low: *active_low },
            SK::Enum { shift, width, labels } => SK::Enum { shift: *shift, width: *width, labels },
            _ => unreachable!("only Bool and Enum fields are mirrored"),
        }
    }

    /// Unchanged values touch no bytes: decoding an image and writing the
    /// result straight back must be a no-op.
    #[test]
    fn writing_back_what_was_read_changes_nothing() {
        let img: Vec<u8> = (0..IMAGE_LEN).map(|i| (i * 91 % 253) as u8).collect();
        let decoded = decode_settings(&img);
        let mut out = img.clone();
        apply_settings(&mut out, decoded.as_object().unwrap());
        assert_eq!(out, img);
    }

    /// A value the field cannot hold is refused whole, never trimmed.
    #[test]
    fn a_text_value_that_does_not_fit_is_not_written() {
        let base = vec![0u8; IMAGE_LEN];
        for (key, bad) in [("aprs-callsign", "WW8L-9"), ("aprs-callsign", "WW8LXYZ"), ("callsign", "WW8L 1"), ("aprs-message-group-1", "ALL-CALL")] {
            let mut img = base.clone();
            let mut m = Map::new();
            m.insert(key.into(), Value::String(bad.into()));
            assert_eq!(apply_settings(&mut img, &m), 0, "{key} {bad}");
            assert_eq!(img, base, "{key} {bad}");
        }
        let mut img = base.clone();
        let mut m = Map::new();
        m.insert("callsign".into(), Value::String("ww8l/m".into()));
        assert_eq!(apply_settings(&mut img, &m), 1);
        assert_eq!(decode_settings(&img)["callsign"], "WW8L/M");
    }

    /// The factory-fresh radio's own file (bytes copied out of capture 01)
    /// decodes to the manual's printed defaults for the fields whose default is
    /// distinctive — the cross-check that refuted three FTM-300 addresses.
    #[test]
    fn the_new_radios_defaults_decode_to_the_manuals() {
        let mut img = vec![0u8; IMAGE_LEN];
        for (at, b) in [
            (0xA4, 0x00), (0xAA, 0x01), (0xAD, 0x00), (0xAF, 0x15), (0xB5, 0x00), (0xB6, 0x00),
            (0xB7, 0x00), (0xEB, 0x88), (0xFA, 0x22), (0xFB, 0x20), (0x28F, 0x06), (0x2D6, 0x01),
            (0x2DD, 0x02), (0x2DF, 0xDD), (0x2E0, 0xE1), (0x2E1, 0xE8), (0x2E2, 0xE9), (0x2C8, b'W'),
            (0x2C9, b'W'), (0x2CA, b'8'), (0x2CB, b'L'),
        ] {
            img[at] = b;
        }
        img[0x2CC..0x2D2].fill(0xFF);
        let s = decode_settings(&img);
        for (k, want) in [
            ("auto-power-off", "OFF"), ("time-out-timer", "5 min"), ("display-select", "BACKTRACK"),
            ("gps-datum", "WGS-84"), ("gps-log", "OFF"), ("vox", "OFF"), ("vox-delay", "0.5s"),
            ("date-format", "mmm/dd/yyyy"), ("time-format", "24 hour"), ("lcd-brightness", "MAX"),
            ("compass", "HEADING UP"), ("mic-gain", "NORMAL"), ("mic-key-p1", "2nd PTT"),
            ("mic-key-p2", "HOME CH"), ("mic-key-p3", "D_X"), ("mic-key-p4", "WX"), ("callsign", "WW8L"),
        ] {
            assert_eq!(s[k], want, "{k}");
        }
        assert_eq!(s["beep"], true);
        assert_eq!(s["beep-level"], "LOW");
    }

    #[test]
    fn an_aprs_radio_gets_aprs_settings() {
        assert!(schema().iter().filter(|f| f["group"] == "APRS").count() >= 1);
    }
}
