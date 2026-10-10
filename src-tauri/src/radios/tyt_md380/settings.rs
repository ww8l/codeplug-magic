//! MD-380 non-channel settings: General Settings, the side buttons and the
//! menu switches, read out of the codeplug image and written back into it.
//!
//! The settings live in the same 256 KiB image as the channels, so a read is a
//! download, and a write reads the image, patches these bytes and rewrites
//! only the 64 KiB sector holding them (all in 0x2040..0x2120, sector 0),
//! then verifies the whole image after the restart. It is a separate,
//! explicitly-acknowledged operation, as on the AnyTone: a codeplug program
//! carries the radio's own settings through untouched.
//!
//! ## Where the table came from
//!
//! GENERATED (`md380_settings_table.rs`) by `scratchpad/tyt_md380/
//! gen_md380_settings.py` from that folder's `MEASURED.md`, which was seeded
//! from Farnsworth's editcp field map (dmrconfig and qdmr agree on the
//! offsets; see RESEARCH.md §1.8 for where dmrconfig's bit order does not).
//! Each row's grade rides along in its comment. Four fields are deliberately
//! absent — the three passwords and the switch that enables one — because a
//! wrong value locks the operator, or this app, out of the radio.
//!
//! ## Rules
//!
//! - A blank or missing value means LEAVE ALONE, never "write a default".
//! - A value whose stored form already matches is not rewritten.
//! - A value the table cannot encode (a label it does not know, a number off
//!   the field's step) is not written, and is named in the report.
//! - A stored value the table cannot name decodes as blank, so the form cannot
//!   send it back as something else.

use serde_json::{Map, Value};

use crate::radios::driver::{SettingsCapture, SettingsReader, SettingsWriteReport, SettingsWriter};

use super::memory::{decode_name, encode_name};
use super::{check_band, protocol, restore_hint, session, verify_after_write, TytMd380};

pub(crate) struct SF {
    pub key: &'static str,
    /// Image offset of the field's first byte.
    pub at: usize,
    pub kind: K,
}

pub(crate) enum K {
    /// One bit; `on` is the bit value that means On (many are inverted).
    Flag { bit: u8, on: u8 },
    /// An enumerated bit field inside one byte.
    Bits { shift: u8, width: u8, labels: &'static [(u8, &'static str)] },
    /// An enumerated whole byte.
    Byte { labels: &'static [(u8, &'static str)] },
    /// A byte holding `raw` in `min..=max` (a multiple of `step`), shown as
    /// `raw × scale`, or as `min_label` at `min`.
    Span { min: u8, max: u8, step: u8, scale: u32, min_label: Option<&'static str> },
    /// UTF-16LE text, NUL-padded, `units` code units.
    Text16 { units: usize },
    /// A 24-bit little-endian DMR ID.
    Id24,
}

include!("md380_settings_table.rs");

#[cfg(test)]
fn len(k: &K) -> usize {
    match k {
        K::Text16 { units } => units * 2,
        K::Id24 => 3,
        _ => 1,
    }
}

fn decode_field(img: &[u8], f: &SF) -> Value {
    let b = img[f.at];
    match &f.kind {
        K::Flag { bit, on } => Value::Bool((b >> bit) & 1 == *on),
        K::Bits { shift, width, labels } => {
            let raw = (b >> shift) & ((1 << width) - 1);
            label_of(labels, raw)
        }
        K::Byte { labels } => label_of(labels, b),
        K::Span { min, max, step, scale, min_label } => {
            if b == *min && min_label.is_some() {
                Value::String(min_label.unwrap().into())
            } else if b < *min || b > *max || !b.is_multiple_of(*step) {
                Value::String(String::new())
            } else if *scale == 1 && *step == 1 && min_label.is_none() {
                Value::from(b)
            } else {
                Value::String((b as u32 * scale).to_string())
            }
        }
        K::Text16 { units } => Value::String(decode_name(&img[f.at..f.at + units * 2])),
        K::Id24 => Value::from(u32::from_le_bytes([img[f.at], img[f.at + 1], img[f.at + 2], 0])),
    }
}

fn label_of(labels: &[(u8, &str)], raw: u8) -> Value {
    Value::String(labels.iter().find(|(v, _)| *v == raw).map(|(_, l)| *l).unwrap_or("").into())
}

/// Every setting the table knows, decoded and shaped like the profile form.
pub(crate) fn decode(img: &[u8]) -> Value {
    let mut out = Map::new();
    for f in MD380_SETTINGS_FIELDS {
        out.insert(f.key.into(), decode_field(img, f));
    }
    Value::Object(out)
}

fn is_blank(v: &Value) -> bool {
    v.is_null() || v.as_str().is_some_and(|s| s.trim().is_empty())
}

/// Encode one value into its bytes, or say why it cannot be. Every check runs
/// before the first byte is written.
fn encode_field(img: &mut [u8], f: &SF, v: &Value) -> Result<(), String> {
    let text = |v: &Value| v.as_str().map(str::trim).map(str::to_string).unwrap_or_else(|| v.to_string());
    let by_label = |labels: &[(u8, &str)]| -> Result<u8, String> {
        let want = text(v);
        labels
            .iter()
            .find(|(_, l)| *l == want)
            .map(|(r, _)| *r)
            .ok_or_else(|| format!("'{want}' is not one of its options"))
    };
    match &f.kind {
        K::Flag { bit, on } => {
            let set = v.as_bool().ok_or("expected on/off")?;
            let bitval = if set { *on } else { 1 - on };
            img[f.at] = (img[f.at] & !(1 << bit)) | (bitval << bit);
        }
        K::Bits { shift, width, labels } => {
            let raw = by_label(labels)?;
            let mask = ((1u8 << width) - 1) << shift;
            img[f.at] = (img[f.at] & !mask) | ((raw << shift) & mask);
        }
        K::Byte { labels } => img[f.at] = by_label(labels)?,
        K::Span { min, max, step, scale, min_label } => {
            let s = text(v);
            let raw = if min_label.is_some_and(|l| l == s) {
                *min
            } else {
                let n: u32 = s.parse().map_err(|_| format!("'{s}' is not a number"))?;
                if !n.is_multiple_of(*scale) {
                    return Err(format!("{n} is not a multiple of {scale}"));
                }
                let r = n / scale;
                if r < *min as u32 || r > *max as u32 || !r.is_multiple_of(*step as u32) {
                    return Err(format!("{n} is outside what the radio stores"));
                }
                r as u8
            };
            img[f.at] = raw;
        }
        K::Text16 { units } => {
            // NOT trimmed: operators pad intro lines with spaces to centre
            // them. Only an all-blank value is special, and `apply` has
            // already skipped that as "leave alone".
            let s = v.as_str().map(str::to_string).unwrap_or_else(|| v.to_string());
            if s.encode_utf16().count() > *units {
                return Err(format!("longer than {units} characters"));
            }
            encode_name(&s, &mut img[f.at..f.at + units * 2]);
        }
        K::Id24 => {
            let n = v.as_u64().or_else(|| text(v).parse().ok()).ok_or("expected a number")?;
            if !(1..=16_776_415).contains(&n) {
                return Err(format!("{n} is not a DMR ID"));
            }
            img[f.at..f.at + 3].copy_from_slice(&(n as u32).to_le_bytes()[..3]);
        }
    }
    Ok(())
}

/// Write a profile's settings into an image. Returns (fields that now hold the
/// profile's value, notes on values that could not be written).
pub(crate) fn apply(img: &mut [u8], settings: &Map<String, Value>) -> (usize, Vec<String>) {
    let mut applied = 0;
    let mut notes = Vec::new();
    for f in MD380_SETTINGS_FIELDS {
        let Some(v) = settings.get(f.key) else { continue };
        if is_blank(v) {
            continue;
        }
        if decode_field(img, f) == *v {
            applied += 1;
            continue;
        }
        // `encode_field` validates before it touches a byte, so a refused
        // value leaves the image as it was.
        match encode_field(img, f, v) {
            Ok(()) => applied += 1,
            Err(e) => notes.push(format!("{}: {e} — left as the radio has it", f.key)),
        }
    }
    (applied, notes)
}

impl SettingsReader for TytMd380 {
    fn read_settings(&self, _port: &str, _schema_json: &str) -> Result<SettingsCapture, String> {
        let image = session(|link, _| protocol::read_image(link))?;
        Ok(SettingsCapture { settings: decode(&image), backup: image, backup_ext: "img" })
    }
}

impl SettingsWriter for TytMd380 {
    /// Read and back up the radio, patch only the profile's settings, and
    /// rewrite only the 64 KiB sector(s) that changed — in one session — then
    /// verify the whole image from a fresh one after the restart. Nothing is
    /// written when every setting already matches.
    fn write_settings(
        &self,
        _port: &str,
        settings: &Value,
        _schema_json: &str,
        backup_dir: &std::path::Path,
    ) -> Result<SettingsWriteReport, String> {
        let settings = settings.as_object().ok_or("the profile's settings are not an object")?;
        std::fs::create_dir_all(backup_dir).map_err(|e| e.to_string())?;
        let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
        let backup_path = backup_dir.join(format!("tyt_md380-presettings-{stamp}.img"));

        let (applied, notes, image, changed) = session(|link, id| {
            check_band(id)?;
            let base = protocol::read_image(link)?;
            std::fs::write(&backup_path, &base)
                .map_err(|e| format!("could not write backup {}: {e}", backup_path.display()))?;
            let mut image = base.clone();
            let (applied, notes) = apply(&mut image, settings);
            // Only the sectors that changed — sector 0 for every field here —
            // so a failed settings write cannot take the channel tables with it.
            let sectors = protocol::changed_sectors(&base, &image);
            let changed = !sectors.is_empty();
            if changed {
                protocol::write_sectors(link, &image, &sectors)
                    .map_err(|e| restore_hint(e, &backup_path))?;
            }
            Ok((applied, notes, image, changed))
        })?;

        let mut note = notes.join("; ");
        let verified = if changed {
            let (ok, _, log) = verify_after_write(&image)?;
            if !ok {
                note = format!("the read-back did not match ({}). {note}", log.join("; "));
            }
            ok
        } else {
            if note.is_empty() {
                note = "every setting already matched the radio — nothing was written".into();
            }
            true
        };
        Ok(SettingsWriteReport {
            fields_written: applied,
            verified: Some(verified),
            note: (!note.is_empty()).then_some(note),
            backup_path: backup_path.to_string_lossy().into_owned(),
            expected_path: None,
            windows_written: Vec::new(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::radios::tyt_md380::layout::IMAGE_LEN;

    fn schema() -> Vec<Value> {
        serde_json::from_str(include_str!("../../md380_settings_schema.json")).unwrap()
    }

    /// The form and the encoder are one parse; this is what keeps them one.
    #[test]
    fn the_table_and_the_schema_describe_the_same_fields() {
        let schema = schema();
        let fields: Vec<&str> = schema
            .iter()
            .filter(|f| f["type"] != "section")
            .map(|f| f["key"].as_str().unwrap())
            .collect();
        let table: Vec<&str> = MD380_SETTINGS_FIELDS.iter().map(|f| f.key).collect();
        assert_eq!(fields, table);
        for (f, s) in MD380_SETTINGS_FIELDS.iter().zip(schema.iter().filter(|f| f["type"] != "section")) {
            match (&f.kind, s["type"].as_str().unwrap()) {
                (K::Flag { .. }, "boolean") | (K::Text16 { .. }, "text") | (K::Id24, "integer") => {}
                (K::Bits { labels, .. } | K::Byte { labels }, "select") => {
                    let opts: Vec<&str> = s["options"].as_array().unwrap().iter().map(|o| o.as_str().unwrap()).collect();
                    let labs: Vec<&str> = labels.iter().map(|(_, l)| *l).collect();
                    assert_eq!(opts, labs, "{}", f.key);
                }
                (K::Span { .. }, "select" | "integer") => {}
                (k, t) => panic!("{}: table kind and schema type '{t}' disagree ({})", f.key, len(k)),
            }
        }
    }

    /// Every option the form offers encodes, and decodes back to itself.
    #[test]
    fn every_offered_value_round_trips() {
        let schema = schema();
        let mut img = vec![0u8; IMAGE_LEN];
        for s in schema.iter().filter(|f| f["type"] != "section") {
            let key = s["key"].as_str().unwrap();
            let f = MD380_SETTINGS_FIELDS.iter().find(|f| f.key == key).unwrap();
            let values: Vec<Value> = match s["type"].as_str().unwrap() {
                "boolean" => vec![Value::Bool(true), Value::Bool(false)],
                "select" => s["options"].as_array().unwrap().clone(),
                "integer" => vec![s["min"].clone(), s["max"].clone()],
                "text" => vec![Value::String("AB12".into())],
                t => panic!("{t}"),
            };
            for v in values {
                encode_field(&mut img, f, &v).unwrap_or_else(|e| panic!("{key} {v}: {e}"));
                assert_eq!(decode_field(&img, f), v, "{key}");
            }
        }
    }

    #[test]
    fn a_blank_value_leaves_the_radio_alone_and_a_bad_one_is_named() {
        let mut img = vec![0x5Au8; IMAGE_LEN];
        let before = img.clone();
        let mut s = Map::new();
        s.insert("general-radio-name".into(), Value::String("".into()));
        s.insert("general-backlight-time-s".into(), Value::String("".into()));
        s.insert("general-vox-sensitivity".into(), Value::String("eleven".into()));
        let (n, notes) = apply(&mut img, &s);
        assert_eq!(n, 0);
        assert_eq!(img, before);
        assert!(notes[0].contains("general-vox-sensitivity"), "{notes:?}");
    }

    /// No two settings claim the same bit. A mis-transcribed row shows up here
    /// as a collision rather than as two controls that move each other.
    #[test]
    fn no_two_settings_claim_the_same_bit() {
        let mut seen = std::collections::HashMap::new();
        for f in MD380_SETTINGS_FIELDS {
            let bits: Vec<(usize, u8)> = match &f.kind {
                K::Flag { bit, .. } => vec![(f.at, *bit)],
                K::Bits { shift, width, .. } => (*shift..shift + width).map(|b| (f.at, b)).collect(),
                k => (f.at..f.at + len(k)).flat_map(|a| (0..8).map(move |b| (a, b))).collect(),
            };
            for b in bits {
                if let Some(other) = seen.insert(b, f.key) {
                    panic!("{} and {other} both claim byte {:#x} bit {}", f.key, b.0, b.1);
                }
            }
        }
    }

    #[test]
    fn a_padded_intro_line_keeps_its_spaces() {
        let mut img = vec![0u8; IMAGE_LEN];
        let mut s = Map::new();
        s.insert("general-intro-screen-line-2".into(), Value::String("  673-7744".into()));
        let (n, notes) = apply(&mut img, &s);
        assert_eq!((n, notes.len()), (1, 0));
        assert_eq!(decode(&img)["general-intro-screen-line-2"], "  673-7744");
    }

    #[test]
    fn a_flag_shares_its_byte_without_disturbing_its_neighbours() {
        let mut img = vec![0u8; IMAGE_LEN];
        img[0x2081] = 0b1010_0101;
        let mut s = Map::new();
        s.insert("general-save-mode-receive".into(), Value::Bool(true)); // bit 1
        apply(&mut img, &s);
        assert_eq!(img[0x2081], 0b1010_0111);
    }
}
