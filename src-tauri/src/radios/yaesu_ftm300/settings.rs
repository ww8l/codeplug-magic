//! FTM-300 Set Mode settings: decode from, and apply to, `CLNFTM300D.dat`.
//!
//! The field table and the profile form's schema are both generated from one
//! measurement (`scratchpad/ftm300/rts/gen_settings.py`, fed by the
//! bit-signature campaign `plan.py` / `runpass.py` / `solve.py`), so the decoder
//! and the form cannot drift. Every address was measured on RT Systems' own
//! writes of this radio's file; none is inferred from CHIRP or another radio.
//!
//! This radio packs settings several to a byte (the solver found booleans and
//! small enums sharing bytes all over the `0x80-0xFF` block), so every kind is a
//! bit field and every write is read-modify-write: the other bits in the byte
//! are someone else's setting.

use serde_json::{Map, Value};

pub(crate) struct SF {
    pub key: &'static str,
    /// File offset into `CLNFTM300D.dat`.
    pub byte: u32,
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
    /// Fixed-length ASCII, padded with `pad` (0xFF for the radio call sign,
    /// 0xCA for the APRS call sign — both read off Tim's backup). `punct`
    /// allows `-` and `/`, which the radio call sign takes and APRS does not.
    Text { len: u32, pad: u8, punct: bool },
    /// One byte holding the number itself (the SmartBeaconing parameters).
    /// The schema carries the manual's range; `settings_bounds` enforces it.
    Uint,
}

include!("ftm300_settings_table.rs");

fn mask(width: u8) -> u32 {
    (1u32 << width) - 1
}

fn label_for(labels: &[(u32, &str)], raw: u32) -> Value {
    match labels.iter().find(|(v, _)| *v == raw) {
        Some((_, l)) => Value::String((*l).to_string()),
        // An unknown stored value comes back as its number rather than a wrong
        // label, so a write of an untouched profile cannot normalise it away.
        None => Value::String(raw.to_string()),
    }
}

fn field_len(f: &SF) -> usize {
    match &f.kind {
        SK::Enum16 { .. } => 2,
        SK::Text { len, .. } => *len as usize,
        _ => 1,
    }
}

fn decode_field(img: &[u8], f: &SF) -> Value {
    let at = f.byte as usize;
    match &f.kind {
        SK::Bool { shift, active_low } => Value::Bool(((img[at] >> shift) & 1 == 1) != *active_low),
        SK::Enum { shift, width, labels } => {
            label_for(labels, (img[at] as u32 >> shift) & mask(*width))
        }
        SK::Enum16 { labels } => label_for(labels, u16::from_be_bytes([img[at], img[at + 1]]) as u32),
        SK::Uint => Value::from(img[at]),
        SK::Text { len, pad, .. } => {
            let raw = &img[at..at + *len as usize];
            let cut = raw.iter().position(|&b| b == *pad || b == 0).unwrap_or(raw.len());
            Value::String(String::from_utf8_lossy(&raw[..cut]).trim_end().to_string())
        }
    }
}

/// Decode every known setting, shaped like the profile form.
pub(crate) fn decode_settings(img: &[u8]) -> Value {
    let mut out = Map::new();
    for f in FTM300_SETTINGS_FIELDS {
        if f.byte as usize + field_len(f) > img.len() {
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
        // A raw number handed back by `label_for` round-trips as itself.
        .or_else(|| s.parse().ok())
}

/// Write a profile's settings into the image, returning how many fields were
/// applied. Unknown keys and wrongly-shaped values are skipped, never guessed;
/// a field already holding the value is left untouched (no bytes rewritten).
pub(crate) fn apply_settings(img: &mut [u8], settings: &Map<String, Value>) -> usize {
    let mut written = 0;
    for f in FTM300_SETTINGS_FIELDS {
        let Some(v) = settings.get(f.key) else {
            continue;
        };
        let at = f.byte as usize;
        if at + field_len(f) > img.len() {
            continue;
        }
        if decode_field(img, f) == *v {
            written += 1;
            continue;
        }
        let ok = match &f.kind {
            SK::Bool { shift, active_low } => match v.as_bool() {
                Some(b) => {
                    let bit = b != *active_low;
                    img[at] = (img[at] & !(1 << shift)) | ((bit as u8) << shift);
                    true
                }
                None => false,
            },
            SK::Enum { shift, width, labels } => match raw_for(labels, v) {
                Some(raw) if raw <= mask(*width) => {
                    let m = (mask(*width) << shift) as u8;
                    img[at] = (img[at] & !m) | ((raw << shift) as u8 & m);
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
            SK::Uint => match v.as_u64() {
                Some(n) if n <= 0xFF => {
                    img[at] = n as u8;
                    true
                }
                _ => false,
            },
            SK::Text { len, pad, punct } => match v.as_str() {
                Some(s) => {
                    let s: Vec<u8> = s
                        .to_ascii_uppercase()
                        .bytes()
                        .filter(|b| b.is_ascii_alphanumeric() || (*punct && (*b == b'-' || *b == b'/')))
                        .take(*len as usize)
                        .collect();
                    let field = &mut img[at..at + *len as usize];
                    field.fill(*pad);
                    field[..s.len()].copy_from_slice(&s);
                    true
                }
                None => false,
            },
        };
        written += usize::from(ok);
    }
    written
}

#[cfg(test)]
mod tests {
    use super::*;

    fn schema() -> Vec<Value> {
        serde_json::from_str(include_str!("../../ftm300_settings_schema.json")).unwrap()
    }

    /// The generator emits both halves; this is the guard that they still agree.
    #[test]
    fn table_and_schema_agree() {
        let fields: Vec<Value> = schema().into_iter().filter(|f| f["type"] != "section").collect();
        assert_eq!(fields.len(), FTM300_SETTINGS_FIELDS.len());
        for (s, f) in fields.iter().zip(FTM300_SETTINGS_FIELDS) {
            assert_eq!(s["key"], f.key);
            match &f.kind {
                SK::Bool { .. } => assert_eq!(s["type"], "boolean", "{}", f.key),
                SK::Enum { labels, .. } | SK::Enum16 { labels } => {
                    let opts: Vec<&str> = s["options"].as_array().unwrap().iter().map(|o| o.as_str().unwrap()).collect();
                    let want: Vec<&str> = labels.iter().map(|(_, l)| *l).collect();
                    assert_eq!(opts, want, "{}", f.key);
                }
                SK::Text { .. } => assert_eq!(s["type"], "text", "{}", f.key),
                SK::Uint => assert_eq!(s["type"], "integer", "{}", f.key),
            }
        }
    }

    /// Every value of every field written and read back, and nothing else in
    /// the image moved — the shared-byte fields must not trample each other.
    #[test]
    fn every_value_round_trips_alone() {
        let base: Vec<u8> = (0..super::super::sd_image::IMAGE_LEN).map(|i| (i * 37 % 251) as u8).collect();
        for f in FTM300_SETTINGS_FIELDS {
            let values: Vec<Value> = match &f.kind {
                SK::Bool { .. } => vec![Value::Bool(true), Value::Bool(false)],
                SK::Enum { labels, .. } | SK::Enum16 { labels } => {
                    labels.iter().map(|(_, l)| Value::String((*l).into())).collect()
                }
                SK::Text { .. } => vec![Value::String("WW8L".into())],
                SK::Uint => vec![Value::from(1), Value::from(255)],
            };
            for v in values {
                let mut img = base.clone();
                let mut m = Map::new();
                m.insert(f.key.into(), v.clone());
                assert_eq!(apply_settings(&mut img, &m), 1, "{} {v}", f.key);
                assert_eq!(decode_settings(&img)[f.key], v, "{}", f.key);
                let moved: Vec<usize> = (0..img.len()).filter(|&i| img[i] != base[i]).collect();
                let at = f.byte as usize;
                assert!(moved.iter().all(|&i| i >= at && i < at + field_len(f)), "{} moved {moved:x?}", f.key);
            }
        }
    }

    /// Unchanged values touch no bytes: decoding an image and writing the
    /// result straight back must be a no-op.
    #[test]
    fn writing_back_what_was_read_changes_nothing() {
        let img: Vec<u8> = (0..super::super::sd_image::IMAGE_LEN).map(|i| (i * 91 % 253) as u8).collect();
        let decoded = decode_settings(&img);
        let mut out = img.clone();
        apply_settings(&mut out, decoded.as_object().unwrap());
        assert_eq!(out, img);
    }

    #[test]
    fn an_aprs_radio_gets_aprs_settings() {
        assert!(schema().iter().filter(|f| f["group"] == "APRS").count() >= 1);
    }
}
