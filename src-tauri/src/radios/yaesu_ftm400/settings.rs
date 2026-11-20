//! FTM-400 set-up menu settings: decode from, and apply to, `CLNFTM400D.dat`.
//!
//! ## Where the table comes from
//!
//! Yaesu's own programmer, ADMS-7 (1.0.0.2, the build for the DR firmware), was
//! run headless: loaded by reflection, its `DataConverter` decoding and encoding
//! the radio's file with no window ever shown. For every setting and every
//! option the encoder was asked to write that option, and the bytes it moved
//! were recorded. Each option below is exactly those masked bytes, so no
//! encoding here is inferred — the odd ones (time zone's sign-magnitude byte,
//! auto power-off's gapped codes, BEEP across two bytes) are simply what ADMS-7
//! wrote. Option labels are ADMS-7's own list text, in stored order: every one
//! of its 181 drop-downs is data-bound `SelectedIndex` ↔ stored value with no
//! converter, and the five radio-button groups were mapped by loading the form.
//!
//! `scratchpad/yaesu_ftm400/adms/gen_settings.py` generates this table and the
//! profile form's schema from that one measurement, so the two cannot drift.
//! Self-check: the table decodes four real images — Tim's radio backup and
//! three others — to exactly what ADMS-7's own decoder reads, 260/260 each.
//!
//! ## Checked on the radio (2026-11-20)
//!
//! Five settings written by this table showed as written on Tim's radio:
//! BACKGROUND COLOR, TIME/VDD, AUTO DIALER (whose ADMS-7 list puts ON first —
//! right), 12-hour time and the two-byte BEEP. The other 255 are what Yaesu's
//! own programmer writes, not each seen on the radio — including RX COVERAGE,
//! which Tim switched to WIDE by hand during the check, not from a file.
//!
//! Settings share bytes all over `0x80-0x7FF`, so every write is
//! read-modify-write through the field's own mask: the other bits in a byte are
//! another setting's.

use serde_json::{Map, Value};

pub(crate) struct SF {
    pub key: &'static str,
    pub kind: SK,
}

pub(crate) enum SK {
    /// A choice: `cells` are the (offset, mask) pairs the setting owns, and each
    /// option is the masked value of every cell, in the same order.
    Opt {
        cells: &'static [(u32, u8)],
        opts: &'static [(&'static str, &'static [u8])],
    },
    /// Fixed-length ASCII at `at`, padded with `pad`, restricted to `set`.
    Text { at: u32, len: u32, pad: u8, set: TS },
    /// One byte holding the number itself (SmartBeaconing). The schema carries
    /// ADMS-7's range; `settings_bounds` enforces it before this runs.
    Uint { at: u32 },
}

/// What a text field may hold. Checked, never filtered: stripping a character
/// would write a different call sign or message than the one asked for.
#[derive(Clone, Copy)]
pub(crate) enum TS {
    /// A-Z and 0-9, stored upper case — ADMS-7's encoder refuses anything else
    /// in the APRS call sign, ringer calls, digipath hops and reply call.
    Call,
    /// The GM call sign also takes `-` and `/`.
    CallPunct,
    /// A message group or bulletin name: call-sign characters plus `*` and `?`
    /// (the radio's own defaults are `ALL******` and `BLN?*****`).
    Group,
    /// Free text — messages, reply and status texts: printable ASCII.
    Plain,
}

include!("ftm400_settings_table.rs");

/// Every byte a field may change.
fn touches(f: &SF) -> Vec<usize> {
    match &f.kind {
        SK::Opt { cells, .. } => cells.iter().map(|(a, _)| *a as usize).collect(),
        SK::Text { at, len, .. } => (*at as usize..(*at + *len) as usize).collect(),
        SK::Uint { at } => vec![*at as usize],
    }
}

fn decode_field(img: &[u8], f: &SF) -> Value {
    match &f.kind {
        SK::Opt { cells, opts } => {
            let got: Vec<u8> = cells.iter().map(|(a, m)| img[*a as usize] & m).collect();
            match opts.iter().find(|(_, v)| **v == got[..]) {
                Some((l, _)) => Value::String((*l).to_string()),
                // An unknown stored value comes back as `#hh[.hh]` rather than a
                // wrong label, so a write of an untouched profile cannot normalise
                // it away.
                None => Value::String(format!(
                    "#{}",
                    got.iter().map(|b| format!("{b:02x}")).collect::<Vec<_>>().join(".")
                )),
            }
        }
        SK::Uint { at } => Value::from(img[*at as usize]),
        SK::Text { at, len, pad, .. } => {
            let raw = &img[*at as usize..(*at + *len) as usize];
            let cut = raw.iter().position(|&b| b == *pad || b == 0).unwrap_or(raw.len());
            Value::String(String::from_utf8_lossy(&raw[..cut]).trim_end().to_string())
        }
    }
}

/// Decode every known setting, shaped like the profile form.
pub(crate) fn decode_settings(img: &[u8]) -> Value {
    let mut out = Map::new();
    for f in FTM400_SETTINGS_FIELDS {
        if touches(f).iter().any(|&i| i >= img.len()) {
            continue;
        }
        out.insert(f.key.to_string(), decode_field(img, f));
    }
    Value::Object(out)
}

/// The cell values a choice stands for: a label, or the `#hh[.hh]` form
/// `decode_field` hands back for a value it cannot name.
fn cells_for(opts: &[(&str, &[u8])], n: usize, v: &Value) -> Option<Vec<u8>> {
    let s = v.as_str()?;
    if let Some((_, bytes)) = opts.iter().find(|(l, _)| *l == s) {
        return Some(bytes.to_vec());
    }
    let raw: Vec<u8> = s
        .strip_prefix('#')?
        .split('.')
        .map(|h| u8::from_str_radix(h, 16).ok())
        .collect::<Option<_>>()?;
    (raw.len() == n).then_some(raw)
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

/// Write a profile's settings into the image, returning how many fields were
/// applied. Unknown keys and wrongly-shaped values are skipped, never guessed;
/// a field already holding the value is left untouched (no bytes rewritten).
pub(crate) fn apply_settings(img: &mut [u8], settings: &Map<String, Value>) -> usize {
    let mut written = 0;
    for f in FTM400_SETTINGS_FIELDS {
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
        let ok = match &f.kind {
            SK::Opt { cells, opts } => match cells_for(opts, cells.len(), v) {
                // A raw value with bits outside the field's mask is not this field's.
                Some(vals) if vals.iter().zip(cells.iter()).all(|(b, (_, m))| b & !m == 0) => {
                    for ((a, m), b) in cells.iter().zip(vals) {
                        let at = *a as usize;
                        img[at] = (img[at] & !m) | b;
                    }
                    true
                }
                _ => false,
            },
            SK::Uint { at } => match v.as_u64() {
                Some(n) if n <= 0xFF => {
                    img[*at as usize] = n as u8;
                    true
                }
                _ => false,
            },
            // A value the field cannot hold is skipped, not cut down to fit.
            SK::Text { at, len, pad, set } => match v.as_str() {
                Some(s) if text_fits(s, *len, *set) => {
                    let s = match set {
                        TS::Plain => s.to_string(),
                        _ => s.to_ascii_uppercase(),
                    };
                    let field = &mut img[*at as usize..(*at + *len) as usize];
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
    use crate::radios::yaesu_ftm400::sd_image::IMAGE_LEN;

    fn schema() -> Vec<Value> {
        serde_json::from_str(include_str!("../../ftm400_settings_schema.json")).unwrap()
    }

    /// The generator emits both halves; this is the guard that they still agree.
    #[test]
    fn table_and_schema_agree() {
        let fields: Vec<Value> = schema().into_iter().filter(|f| f["type"] != "section").collect();
        assert_eq!(fields.len(), FTM400_SETTINGS_FIELDS.len());
        for (s, f) in fields.iter().zip(FTM400_SETTINGS_FIELDS) {
            assert_eq!(s["key"], f.key);
            match &f.kind {
                SK::Opt { opts, .. } => {
                    let got: Vec<&str> = s["options"].as_array().unwrap().iter().map(|o| o.as_str().unwrap()).collect();
                    let want: Vec<&str> = opts.iter().map(|(l, _)| *l).collect();
                    assert_eq!(got, want, "{}", f.key);
                }
                SK::Text { len, .. } => {
                    assert_eq!(s["type"], "text", "{}", f.key);
                    assert_eq!(s["max_length"], *len, "{}", f.key);
                }
                SK::Uint { .. } => assert_eq!(s["type"], "integer", "{}", f.key),
            }
        }
        let mut keys: Vec<&str> = FTM400_SETTINGS_FIELDS.iter().map(|f| f.key).collect();
        keys.sort_unstable();
        let n = keys.len();
        keys.dedup();
        assert_eq!(keys.len(), n, "duplicate settings key");
    }

    /// No two settings own the same bit, and every option fits its own masks.
    #[test]
    fn fields_own_disjoint_bits() {
        let mut owner = std::collections::HashMap::new();
        for f in FTM400_SETTINGS_FIELDS {
            let cells: Vec<(u32, u8)> = match &f.kind {
                SK::Opt { cells, opts } => {
                    for (l, v) in opts.iter() {
                        assert_eq!(v.len(), cells.len(), "{} {l}", f.key);
                        assert!(v.iter().zip(cells.iter()).all(|(b, (_, m))| b & !m == 0), "{} {l}", f.key);
                    }
                    cells.to_vec()
                }
                SK::Text { at, len, .. } => (*at..*at + *len).map(|a| (a, 0xFF)).collect(),
                SK::Uint { at } => vec![(*at, 0xFF)],
            };
            for (a, m) in cells {
                for bit in 0..8 {
                    if m >> bit & 1 == 1 {
                        if let Some(prev) = owner.insert((a, bit), f.key) {
                            panic!("{a:#x} bit {bit}: {prev} and {}", f.key);
                        }
                    }
                }
            }
        }
    }

    /// Every value of every field written and read back, and nothing outside
    /// the field's own bytes moved.
    #[test]
    fn every_value_round_trips_alone() {
        let base: Vec<u8> = (0..IMAGE_LEN).map(|i| (i * 37 % 251) as u8).collect();
        for f in FTM400_SETTINGS_FIELDS {
            let values: Vec<Value> = match &f.kind {
                SK::Opt { opts, .. } => opts.iter().map(|(l, _)| Value::String((*l).into())).collect(),
                SK::Text { set: TS::Plain, .. } => vec![Value::String("Mobile 73".into())],
                SK::Text { set: TS::Group, .. } => vec![Value::String("ALL*".into())],
                SK::Text { .. } => vec![Value::String("WW8L".into())],
                SK::Uint { .. } => vec![Value::from(5), Value::from(180)],
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
            }
        }
    }

    /// Unchanged values touch no bytes: decoding an image and writing the
    /// result straight back must be a no-op, unknown `#` values included.
    #[test]
    fn writing_back_what_was_read_changes_nothing() {
        let img: Vec<u8> = (0..IMAGE_LEN).map(|i| (i * 91 % 253) as u8).collect();
        let decoded = decode_settings(&img);
        assert!(decoded.as_object().unwrap().values().any(|v| v.as_str().is_some_and(|s| s.starts_with('#'))));
        let mut out = img.clone();
        apply_settings(&mut out, decoded.as_object().unwrap());
        assert_eq!(out, img);
    }

    /// A value the field cannot hold is refused whole, never trimmed.
    #[test]
    fn a_value_that_does_not_fit_is_not_written() {
        let base = vec![0u8; IMAGE_LEN];
        for (key, bad) in [
            ("aprs-call-sign", "WW8L-9"),
            ("aprs-call-sign", "WW8LXYZ"),
            ("call-sign-gm", "WW8L 1"),
            ("message-group-1", "ALL-CALL"),
            ("apo", "13.0hour"),
            ("apo", "#ff"),
        ] {
            let mut img = base.clone();
            let mut m = Map::new();
            m.insert(key.into(), Value::String(bad.into()));
            assert_eq!(apply_settings(&mut img, &m), 0, "{key} {bad}");
            assert_eq!(img, base, "{key} {bad}");
        }
        let mut img = base.clone();
        let mut m = Map::new();
        m.insert("call-sign-gm".into(), Value::String("ww8l/m".into()));
        assert_eq!(apply_settings(&mut img, &m), 1);
        assert_eq!(decode_settings(&img)["call-sign-gm"], "WW8L/M");
    }

    /// Bytes from Tim's radio backup decode to what ADMS-7 showed for them —
    /// the encodings that are not one-byte-one-value.
    #[test]
    fn tims_backup_decodes_like_adms7() {
        let mut img = vec![0u8; IMAGE_LEN];
        for (at, b) in [(0xA4, 0x00), (0xA8, 0x8A), (0xAF, 0x15), (0xB4, 0x03), (0xEB, 0x88), (0xFA, 0x02), (0x52F, 0x1E)] {
            img[at] = b;
        }
        img[0x508..0x50E].copy_from_slice(b"WW8L\xCA\xCA");
        let s = decode_settings(&img);
        for (k, want) in [
            ("apo", "OFF"),
            ("time-zone", "UTC-05:00"),
            ("gps-datum", "WGS-84"),
            ("gps-log", "OFF"),
            ("beep", "LOW"),
            ("beacon-rate-limit", "30sec"),
            ("aprs-call-sign", "WW8L"),
        ] {
            assert_eq!(s[k], want, "{k}");
        }
    }

    #[test]
    fn an_aprs_radio_gets_aprs_settings() {
        assert!(schema().iter().filter(|f| f["group"] == "APRS").count() >= 1);
    }
}
