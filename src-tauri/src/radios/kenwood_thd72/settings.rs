//! TH-D72 settings, read and written over the `MU` command.
//!
//! ## Why `MU` and not the image
//!
//! The radio exposes its menu two ways and they carry the **same values** — six
//! of six agree between `MU` and the clone image's `0x0300` block on a real
//! radio, three of them on values that differ from the factory image, so the
//! match is on distinctive numbers rather than a row of zeroes. The measurement
//! is in `scratchpad/kenwood_thd72/MEASURED.md`.
//!
//! Given they agree, `MU` wins on every other count:
//!
//! - It covers **all 19** parameters. CHIRP's struct names offsets for only six;
//!   the other thirteen sit in bytes it calls `unknown`, and guessing an address
//!   is the one failure mode that writes a wrong value to a real radio.
//! - It is one ASCII command. Going through the image would mean a whole clone
//!   session each way — 16 s, plus the several-second settle this radio needs
//!   after one — and would rewrite bytes nobody here understands.
//! - There is no partial-write state. A clone write that fails halfway leaves a
//!   mixed codeplug; `MU` either lands or it does not.
//!
//! ⚠ `MU` is **not** everything the radio holds. CHIRP reads `lamp_control`
//! (Manual/Auto) at `0x339`, which has no `MU` field. This module covers the 19
//! `MU` parameters and does not claim to be the radio's complete settings set.
//!
//! ## The hex trap
//!
//! `MU` renders several parameters as a **hex digit**: `A` is 10 for the lamp
//! timer, the battery saver and both scan restart timers. A decimal parse would
//! reject a radio whose backlight is set to 10 seconds — and reject it at read
//! time, which looks like a broken cable rather than a parsing bug.

use std::path::Path;

use serde_json::{json, Value};

use crate::radios::driver::{SettingsCapture, SettingsReader, SettingsWriteReport, SettingsWriter};
use crate::radios::settings_bounds;

use super::protocol;

/// One settings field, generated from the measurement sheet.
pub(crate) struct MF {
    pub key: &'static str,
    pub label: &'static str,
    /// 0-based index into the 19 comma-separated parameters of an `MU` line.
    pub mu: usize,
    pub menu: Option<&'static str>,
    pub kind: MK,
}

pub(crate) enum MK {
    Bool,
    Enum { labels: &'static [(u8, &'static str)] },
    Uint { min: u8, max: u8 },
}

include!("thd72_settings_table.rs");

/// How many parameters an `MU` line carries. Confirmed on a real TH-D72A
/// (firmware 1.08) — the published sheet said 19 and the radio agreed. The
/// TM-D710's `MU` has 42, so a driver that accepted any count would happily
/// mis-parse a sibling radio.
const MU_FIELDS: usize = 19;

/// Split an `MU` reply into its raw parameters.
///
/// Rejects a line of the wrong width rather than reading what it can: a short
/// line means this is not the radio we think it is, and every index after the
/// missing field would be silently wrong.
fn parse_mu(reply: &str) -> Result<Vec<u8>, String> {
    let body = reply.strip_prefix("MU ").unwrap_or(reply).trim();
    let parts: Vec<&str> = body.split(',').collect();
    if parts.len() != MU_FIELDS {
        return Err(format!(
            "the radio returned {} menu parameters, not {MU_FIELDS} — this is not a TH-D72 \
             menu line (a TM-D710 returns 42).",
            parts.len()
        ));
    }
    parts
        .iter()
        .enumerate()
        .map(|(i, p)| {
            // Hex, not decimal — see the module header.
            u8::from_str_radix(p.trim(), 16)
                .map_err(|_| format!("menu parameter {} is {p:?}, which is not a hex digit", i + 1))
        })
        .collect()
}

/// Render the 19 parameters back into an `MU` set command.
fn format_mu(raw: &[u8]) -> String {
    let body: Vec<String> = raw.iter().map(|v| format!("{v:X}")).collect();
    format!("MU {}", body.join(","))
}

/// Decode the raw parameters into the shape the profile form expects.
fn decode(raw: &[u8]) -> Value {
    let mut out = serde_json::Map::new();
    for f in THD72_SETTINGS_FIELDS {
        let v = raw[f.mu];
        let value = match &f.kind {
            MK::Bool => json!(v != 0),
            MK::Uint { .. } => json!(v),
            MK::Enum { labels } => match labels.iter().find(|(raw_v, _)| *raw_v == v) {
                Some((_, label)) => json!(label),
                // An unlabelled value is reported as the number rather than
                // dropped or clamped. The operator can then see that their radio
                // holds something this table does not know about, which is a
                // measurement gap, not a corrupt radio.
                None => json!(v),
            },
        };
        out.insert(f.key.to_string(), value);
    }
    Value::Object(out)
}

/// Encode form values over a base line read from the radio.
///
/// Deliberately a **patch over what the radio currently holds**, not a build
/// from nothing: `MU` sets all 19 parameters at once, so a field the profile
/// does not carry — p2 contrast, which is omitted on purpose because three
/// sources give three different ranges — must go back exactly as it came.
/// Building the line from defaults would quietly rewrite it.
fn encode_over(base: &[u8], settings: &Value) -> Result<(Vec<u8>, usize), String> {
    let mut raw = base.to_vec();
    let mut written = 0usize;
    for f in THD72_SETTINGS_FIELDS {
        let Some(v) = settings.get(f.key) else { continue };
        if v.is_null() {
            continue;
        }
        let encoded = match &f.kind {
            MK::Bool => match v.as_bool() {
                Some(b) => u8::from(b),
                None => return Err(format!("{} expects true or false, got {v}", f.key)),
            },
            MK::Uint { min, max } => {
                let n = v
                    .as_u64()
                    .ok_or_else(|| format!("{} expects a number, got {v}", f.key))?;
                if n < *min as u64 || n > *max as u64 {
                    return Err(format!(
                        "{} is {n}, outside the radio's {min}..={max}",
                        f.key
                    ));
                }
                n as u8
            }
            MK::Enum { labels } => {
                let s = v
                    .as_str()
                    .ok_or_else(|| format!("{} expects one of its options, got {v}", f.key))?;
                labels
                    .iter()
                    .find(|(_, label)| *label == s)
                    .map(|(raw_v, _)| *raw_v)
                    .ok_or_else(|| format!("{} has no option {s:?}", f.key))?
            }
        };
        if raw[f.mu] != encoded {
            written += 1;
        }
        raw[f.mu] = encoded;
    }
    Ok((raw, written))
}

/// Fold the out-of-range report in with whatever the write itself has to say,
/// so a dropped field is never lost behind a verification message.
fn merge_notes(bounds: &[String], own: Option<String>) -> Option<String> {
    match (settings_bounds::note_line(bounds), own) {
        (Some(a), Some(b)) => Some(format!("{a} {b}")),
        (Some(a), None) => Some(a),
        (None, b) => b,
    }
}

/// Read the menu line, with the identity check in front of it.
fn read_line(p: &mut dyn serialport::SerialPort) -> Result<Vec<u8>, String> {
    protocol::identify(p)?;
    let reply = protocol::command(p, "MU")?;
    parse_mu(&reply)
}

impl SettingsReader for super::KenwoodThd72 {
    /// One ASCII round trip — no clone session, so this costs neither the 16 s
    /// read nor the settle the radio needs afterwards.
    ///
    /// The backup is the `MU` line itself. That is the honest thing to save
    /// here: it is exactly what this path can write back, so a restore from it
    /// is a real restore rather than a partial one. The whole-image backup is
    /// what `ImageProgrammer` takes, and it is a different operation.
    fn read_settings(&self, port: &str, _schema_json: &str) -> Result<SettingsCapture, String> {
        let mut p = protocol::open_port(port)?;
        let raw = read_line(&mut *p)?;
        Ok(SettingsCapture {
            settings: decode(&raw),
            backup: format_mu(&raw).into_bytes(),
            backup_ext: "mu",
        })
    }
}

impl SettingsWriter for super::KenwoodThd72 {
    /// Read the current line, back it up, patch the profile's fields over it,
    /// write, and read back to verify — all in one session, because none of it
    /// needs clone mode.
    ///
    /// ⚠ Untried on hardware as of 2026-08-26. Reading is proven on a real
    /// TH-D72A; writing is not, and in this codebase a working read path has
    /// twice hidden a dead write path. Ladder step 5 is where this gets its
    /// answer.
    fn write_settings(
        &self,
        port: &str,
        settings: &Value,
        schema_json: &str,
        backup_dir: &Path,
    ) -> Result<SettingsWriteReport, String> {
        // The range check every settings write in this app runs first. It does
        // not refuse the write — it removes values the radio cannot take and
        // says which, so an out-of-range field is reported rather than either
        // silently truncated or silently blocked.
        let mut settings = settings.clone();
        let bounds_notes = settings_bounds::strip_out_of_range(schema_json, &mut settings);

        let mut p = protocol::open_port(port)?;
        let base = read_line(&mut *p)?;

        let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
        let backup_path = backup_dir.join(format!("thd72-preprofile-{stamp}.mu"));
        std::fs::write(&backup_path, format_mu(&base))
            .map_err(|e| format!("could not write backup {}: {e}", backup_path.display()))?;

        let (raw, fields_written) = encode_over(&base, &settings)?;
        if raw == base {
            return Ok(SettingsWriteReport {
                fields_written: 0,
                verified: Some(true),
                note: merge_notes(
                    &bounds_notes,
                    Some("The radio already holds these settings; nothing was written.".into()),
                ),
                backup_path: backup_path.to_string_lossy().to_string(),
                expected_path: None,
                windows_written: Vec::new(),
            });
        }

        let reply = protocol::command(&mut *p, &format_mu(&raw))?;
        if reply.trim() == "N" {
            return Err(format!(
                "the radio refused the menu line ({reply:?}). Nothing was written; the \
                 settings it had are saved at {}.",
                backup_path.display()
            ));
        }

        // Read back in the same session. Non-fatal: the radio accepted the line,
        // so a failed read-back is a reporting problem.
        let (verified, note) = match read_line(&mut *p) {
            Ok(after) if after == raw => (true, None),
            Ok(after) => {
                let differing: Vec<&str> = THD72_SETTINGS_FIELDS
                    .iter()
                    .filter(|f| after[f.mu] != raw[f.mu])
                    .map(|f| f.key)
                    .collect();
                (
                    false,
                    Some(format!(
                        "The radio accepted the write but read back differently in: {}. \
                         Check those on the radio's own menus.",
                        differing.join(", ")
                    )),
                )
            }
            Err(e) => (
                false,
                Some(format!("Write accepted, but the read-back could not run ({e}).")),
            ),
        };

        Ok(SettingsWriteReport {
            fields_written,
            verified: Some(verified),
            note: merge_notes(&bounds_notes, note),
            backup_path: backup_path.to_string_lossy().to_string(),
            expected_path: None,
            windows_written: Vec::new(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The line this radio actually returned on 2026-08-26.
    const REAL: &str = "MU 7,7,6,0,0,0,0,0,4,1,0,0,0,2,0,1,5,2,1";

    #[test]
    fn the_real_menu_line_parses_and_re_renders_identically() {
        let raw = parse_mu(REAL).expect("parse");
        assert_eq!(raw.len(), MU_FIELDS);
        assert_eq!(format_mu(&raw), REAL);
    }

    /// The trap in the module header, asserted rather than described: `A` is 10.
    #[test]
    fn a_hex_digit_parses_as_ten_not_rejected() {
        let line = "MU A,7,A,0,0,0,0,0,4,1,0,0,0,2,0,1,A,A,1";
        let raw = parse_mu(line).expect("a radio set to 10 seconds must parse");
        assert_eq!(raw[0], 10);
        assert_eq!(raw[2], 10);
        assert_eq!(format_mu(&raw), line);
    }

    /// A TM-D710 returns 42 parameters. Reading what we can would silently
    /// misattribute every field after the first missing one.
    #[test]
    fn a_line_of_the_wrong_width_is_refused() {
        let err = parse_mu("MU 0,1,2").expect_err("3 fields is not a D72 menu line");
        assert!(err.contains("not a TH-D72"), "{err}");
        assert!(parse_mu(&format!("MU {}", vec!["0"; 42].join(","))).is_err());
    }

    #[test]
    fn the_radios_own_values_decode_to_what_the_screen_showed() {
        let raw = parse_mu(REAL).unwrap();
        let v = decode(&raw);
        // Both confirmed on the radio's own menus: APO on 111, balance on 120.
        assert_eq!(v["apo"], json!("Off"));
        assert_eq!(v["balance"], json!("Center"));
    }

    #[test]
    fn every_field_round_trips_through_encode_and_decode() {
        let base = parse_mu(REAL).unwrap();
        let decoded = decode(&base);
        let (raw, _) = encode_over(&base, &decoded).expect("re-encode what we just decoded");
        assert_eq!(raw, base, "decode -> encode must be lossless");
    }

    /// A write must change **only** the parameters the caller asked for.
    ///
    /// `MU` sets all 19 at once, so the encoder patches over the line the radio
    /// currently holds rather than building one from defaults. A profile that
    /// carries a single field must leave the other eighteen exactly as the radio
    /// had them — that is what makes a partial settings write safe, and a
    /// build-from-defaults encoder would quietly rewrite every one of them.
    ///
    /// ★ This test used to assert that p2 contrast was absent from the table,
    /// because three sources gave three different ranges for it. The radio
    /// settled that on 2026-08-26 (0..15, all three sources wrong), so contrast
    /// is now modelled and every parameter has an owner. The property being
    /// guarded is unchanged; only the example had to go.
    #[test]
    fn a_write_touches_only_the_fields_it_was_given() {
        let base = parse_mu(REAL).unwrap();
        let (raw, written) = encode_over(&base, &json!({"apo": "60 minutes"})).unwrap();
        assert_eq!(written, 1, "one field asked for, one field written");
        for (i, (before, after)) in base.iter().zip(raw.iter()).enumerate() {
            if i == 3 {
                assert_ne!(before, after, "the field we did set must change");
            } else {
                assert_eq!(before, after, "menu parameter {} was not ours to touch", i + 1);
            }
        }
    }

    /// Contrast is the field the radio corrected all three published sources on.
    /// Its ceiling is asserted here so a later regeneration cannot quietly walk
    /// it back to the manual's "1 to 8" or CHIRP's 1..15.
    #[test]
    fn contrast_runs_the_range_the_radio_proved() {
        let f = THD72_SETTINGS_FIELDS
            .iter()
            .find(|f| f.key == "contrast")
            .expect("contrast is modelled");
        assert_eq!(f.mu, 1);
        match f.kind {
            MK::Uint { min, max } => assert_eq!(
                (min, max),
                (0, 15),
                "the radio's own control stopped at F; the manual's 1-8 and CHIRP's 1-15 \
                 are both wrong"
            ),
            _ => panic!("contrast is a bar graph with no labels — it must be an integer"),
        }
    }

    #[test]
    fn a_value_the_radio_has_no_option_for_is_refused_by_name() {
        let base = parse_mu(REAL).unwrap();
        let err = encode_over(&base, &json!({"apo": "90 minutes"})).expect_err("no such option");
        assert!(err.contains("apo") && err.contains("90 minutes"), "{err}");
    }

    /// ⚠ The TH-D75 shipped ten fields carrying only their endpoints, because
    /// the generator read `First=0 … Last=N` as a two-value enum. This is the
    /// guard, and it fires on the whole table rather than a sampled field.
    #[test]
    fn enum_labels_cover_their_whole_range() {
        for f in THD72_SETTINGS_FIELDS {
            if let MK::Enum { labels } = &f.kind {
                let lo = labels.iter().map(|(v, _)| *v).min().unwrap();
                let hi = labels.iter().map(|(v, _)| *v).max().unwrap();
                assert_eq!(
                    labels.len(),
                    (hi - lo) as usize + 1,
                    "{} labels {}..={} but carries only {} of them",
                    f.key,
                    lo,
                    hi,
                    labels.len()
                );
            }
        }
    }

    /// Every field must address a parameter that exists, and no two may address
    /// the same one — a duplicated index silently makes one field overwrite the
    /// other on every write.
    #[test]
    fn every_field_maps_to_its_own_menu_parameter() {
        let mut seen = std::collections::HashSet::new();
        for f in THD72_SETTINGS_FIELDS {
            assert!(f.mu < MU_FIELDS, "{} indexes parameter {}", f.key, f.mu);
            assert!(seen.insert(f.mu), "two fields claim parameter {}", f.mu);
        }
    }

    /// The table and the form schema are generated from one sheet by one script.
    /// A field in the table with no schema entry is a setting nobody can reach;
    /// a schema entry with no table row silently does nothing.
    #[test]
    fn the_table_and_the_form_schema_agree() {
        let schema: Vec<serde_json::Value> =
            serde_json::from_str(crate::seed::THD72_SETTINGS_SCHEMA).expect("schema parses");
        let form: std::collections::HashSet<&str> = schema
            .iter()
            .filter(|f| f["type"] != "section")
            .map(|f| f["key"].as_str().expect("key"))
            .collect();
        let table: std::collections::HashSet<&str> =
            THD72_SETTINGS_FIELDS.iter().map(|f| f.key).collect();
        assert_eq!(
            table, form,
            "the Rust table and the form schema list different fields"
        );
    }
}
