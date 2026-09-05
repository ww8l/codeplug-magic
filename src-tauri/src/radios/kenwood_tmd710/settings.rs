//! The TM-D710's menu settings, read and written as the `MU` command (#113).
//!
//! One ASCII line carries all **42** menu parameters, and setting any of them
//! means sending all 42 back. There is no image and no card file on this radio,
//! so this is the only place its settings live.
//!
//! ## Every range here was measured on the radio
//!
//! `d710_menu_bounds` swept each parameter and read the line back. The TM-D710
//! answers an out-of-range menu value with an explicit `?`, so the first refused
//! value is the size of the enum behind that menu — all 42 in 131 seconds, with
//! the line restored exactly afterwards.
//!
//! That is stronger evidence than the sheet the last two Kenwoods were built
//! from, and it caught **five errors** in the published table. The two that
//! would have shipped wrong values:
//!
//! - **Beep volume and Voice volume are 7 levels, not 8.** The manual says "a
//!   level from 1 to 7"; the radio takes `0..=6` and refuses `7`. So the display
//!   is the stored value **plus one**, and a driver mapping them directly would
//!   have been off by one across the whole range.
//! - **The panel PF keys accept a non-contiguous set** — `0x00`–`0x0A` and then
//!   `0x16`. A contiguous `0..=16` enum, which is what the published table
//!   implies, would offer six values the radio refuses and still miss `0x16`.
//!
//! ## What is deliberately missing
//!
//! Seven of the 42 are **not** exposed: the six PF-key assignments and p25,
//! which no source names. Their sizes are measured and their meanings are not,
//! and an enum whose labels are guesses is the failure mode that writes a wrong
//! value to a real radio. `scratchpad/kenwood_tmd710/MEASURED.md` grades every
//! row and says which are still owed a look at the radio's own screen.
//!
//! ## Grading
//!
//! Sizes are measured. **Orders are mostly inferred** — from the manual and from
//! LA3QMA's table, which agree with each other and now with the radio on 37 of
//! 42 counts. A printed option list is display order, not necessarily the stored
//! index; that distinction cost the TH-D75 a shipped wrong meaning. Two rows are
//! better than inferred: p1 (key beep) and p26 (brightness) were each pinned by
//! a single-change diff on the radio in session 120.

use serde_json::{json, Map, Value};
use std::path::Path;

use super::memory::Menu;
use super::{ask_settling, open_port, write_menu};
use crate::radios::driver::{SettingsCapture, SettingsReader, SettingsWriteReport, SettingsWriter};

/// One menu parameter, as the generated table states it.
pub(crate) struct TF {
    pub key: &'static str,
    pub label: &'static str,
    /// 0-based index into the 42 `MU` parameters.
    pub mu: usize,
    /// The radio's own menu number, for the form's label. Documentation only —
    /// a wrong one mislabels a control, it does not write a wrong value.
    pub menu: Option<&'static str>,
    pub kind: TK,
}

impl TF {
    /// How this field should be named to an operator — the form's label, with
    /// the radio's own menu number when there is one, so a rejected value points
    /// at the menu to go and look at. This is also the only non-test reader of
    /// `label` and `menu`; a `never used` warning on either would mean the
    /// generated table had drifted out of use.
    fn display(&self) -> String {
        match self.menu {
            Some(m) => format!("{} (Menu {})", self.label, m.trim_end_matches('?')),
            None => self.label.to_string(),
        }
    }
}

pub(crate) enum TK {
    Bool,
    Enum { labels: &'static [(u8, &'static str)] },
    Uint { min: u8, max: u8 },
}

include!("tmd710_settings_table.rs");

/// Decode a menu line into the profile form's shape.
fn decode(menu: &Menu) -> Value {
    let mut out = Map::new();
    for f in TMD710_SETTINGS_FIELDS {
        let Ok(text) = menu.field(f.mu + 1) else { continue };
        let Ok(v) = text.parse::<u8>() else { continue };
        let value = match &f.kind {
            TK::Bool => json!(v != 0),
            TK::Uint { .. } => json!(v),
            TK::Enum { labels } => match labels.iter().find(|(raw, _)| *raw == v) {
                Some((_, label)) => json!(label),
                // Reported as the number rather than dropped or clamped: an
                // honest "your radio holds something this table cannot name",
                // which is a measurement gap and not a corrupt radio.
                None => json!(v),
            },
        };
        out.insert(f.key.to_string(), value);
    }
    Value::Object(out)
}

/// One form value as the number the radio stores.
fn encode_one(f: &TF, v: &Value) -> Result<u8, String> {
    Ok(match &f.kind {
        TK::Bool => match v.as_bool() {
            Some(b) => u8::from(b),
            None => return Err(format!("{} expects true or false, got {v}", f.display())),
        },
        TK::Uint { min, max } => {
            let n = v
                .as_u64()
                .ok_or_else(|| format!("{} expects a number, got {v}", f.display()))?;
            if n < u64::from(*min) || n > u64::from(*max) {
                return Err(format!("{} is {n}, outside the radio's {min}..={max}", f.display()));
            }
            n as u8
        }
        TK::Enum { labels } => match v {
            // ⚠ A raw NUMBER is valid, and refusing it bricks the driver.
            // `decode` hands back the number for a stored value this table
            // cannot label; that number is saved into the profile, and if only
            // a label were accepted every later settings write and every
            // program run carrying settings would fail with "has no option 64".
            // The TH-D72 shipped exactly that bug and it was found in review.
            Value::Number(n) => n
                .as_u64()
                .filter(|n| *n <= u64::from(u8::MAX))
                .ok_or_else(|| format!("{} cannot store {v}", f.display()))? as u8,
            _ => {
                let s = v
                    .as_str()
                    .ok_or_else(|| format!("{} expects one of its options, got {v}", f.display()))?;
                labels
                    .iter()
                    .find(|(_, label)| *label == s)
                    .map(|(raw, _)| *raw)
                    // A string that is not an option is a stale label, not a
                    // measurement gap, so it stays an error.
                    .ok_or_else(|| format!("{} has no option {s:?}", f.display()))?
            }
        },
    })
}

/// Patch the profile's fields over the line the radio currently holds.
///
/// A **patch, never a build from defaults.** `MU` sets all 42 parameters at
/// once, so any parameter the profile does not carry — including all seven this
/// table deliberately does not expose — has to go back exactly as it came.
/// Building the line from scratch would silently rewrite the operator's PF key
/// assignments every time they changed the beep volume.
fn patch(base: &Menu, settings: &Value) -> Result<(Menu, usize), String> {
    let mut out = base.clone();
    let mut written = 0usize;
    for f in TMD710_SETTINGS_FIELDS {
        let Some(v) = settings.get(f.key) else { continue };
        if v.is_null() {
            continue;
        }
        let encoded = encode_one(f, v)?;
        let text = encoded.to_string();
        if base.field(f.mu + 1)? != format!("{text:0>width$}", width = base.field(f.mu + 1)?.len())
        {
            written += 1;
        }
        out = out.with_field(f.mu + 1, &text)?;
    }
    Ok((out, written))
}

impl SettingsReader for super::KenwoodTmD710 {
    fn read_settings(&self, port: &str, _schema_json: &str) -> Result<SettingsCapture, String> {
        let mut p = open_port(port)?;
        let line = ask_settling(&mut *p, "MU")?;
        let menu = Menu::parse(&line)?;
        Ok(SettingsCapture {
            settings: decode(&menu),
            // The backup for a live-mode radio is a TRANSCRIPT. This one is the
            // menu line itself, which is exactly what a settings write can
            // clobber — `d710_restore` puts it back.
            backup: line.into_bytes(),
            backup_ext: "txt",
        })
    }
}

impl SettingsWriter for super::KenwoodTmD710 {
    /// Read the current line, back it up, patch the profile's fields over it,
    /// write, and read back to verify — one session, no clone mode.
    ///
    /// ⚠ **Not yet run on a real radio.** Reading `MU` is proven; writing one
    /// field at a time is proven by `d710_set_menu` and by the 42-parameter
    /// sweep, which wrote and restored every parameter. This path — a profile's
    /// worth of fields patched in one go — has not been. In this repo a working
    /// read path has twice hidden a dead write path, so it is stated rather
    /// than assumed.
    fn write_settings(
        &self,
        port: &str,
        settings: &Value,
        _schema_json: &str,
        backup_dir: &Path,
    ) -> Result<SettingsWriteReport, String> {
        let mut p = open_port(port)?;
        let before = ask_settling(&mut *p, "MU")?;
        let base = Menu::parse(&before)?;

        std::fs::create_dir_all(backup_dir).map_err(|e| e.to_string())?;
        let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
        let backup_path = backup_dir.join(format!("kenwood_tmd710-menu-{stamp}.txt"));
        std::fs::write(&backup_path, &before).map_err(|e| e.to_string())?;

        let (wanted, fields_written) = patch(&base, settings)?;
        let failed = write_menu(&mut *p, &wanted)?;

        Ok(SettingsWriteReport {
            fields_written,
            // `write_menu` re-reads the line and diffs it, so this is a real
            // read-back and not the same buffer compared with itself — the
            // mistake found in the TH-D72's review.
            verified: Some(failed.is_empty()),
            note: (!failed.is_empty()).then(|| {
                let names: Vec<String> = failed
                    .iter()
                    .map(|(p, mine, theirs)| format!("p{p}: sent {mine}, radio kept {theirs}"))
                    .collect();
                format!(
                    "{} menu parameter(s) did not take: {}",
                    failed.len(),
                    names.join("; ")
                )
            }),
            backup_path: backup_path.to_string_lossy().into_owned(),
            expected_path: None,
            windows_written: Vec::new(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::radios::kenwood_tmd710::memory::MENU_FIELDS;

    /// The real line off Tim's radio, as first read in session 120.
    const REAL_MU: &str = "MU 0,4,0,1,0,4,1,0,10,0,0,0,0,0,0,2,0,0,0,0,2,0,1,0,0,8,0,0,00,02,14,15,0C,0E,0,1,0,1,0,4,1,1";

    /// ★ Every emitted option list must be exactly as long as the range the
    /// radio accepted. The sizes are measured (`d710_menu_bounds`), so a list
    /// that has grown or shrunk is offering an operator a value the radio
    /// refuses — or hiding one it has. The generator asserts this too; this is
    /// the half that runs in CI.
    #[test]
    fn every_option_list_matches_the_range_the_radio_accepted() {
        // p (1-based) -> values accepted, measured 2026-09-01.
        const MEASURED: [(usize, usize); 42] = [
            (1, 2), (2, 7), (3, 2), (4, 3), (5, 2), (6, 7), (7, 5), (8, 2), (9, 61), (10, 2),
            (11, 2), (12, 2), (13, 4), (14, 6), (15, 2), (16, 3), (17, 2), (18, 2), (19, 2),
            (20, 2), (21, 7), (22, 2), (23, 2), (24, 2), (25, 3), (26, 9), (27, 2), (28, 2),
            (29, 12), (30, 12), (31, 32), (32, 32), (33, 32), (34, 32), (35, 2), (36, 3),
            (37, 6), (38, 4), (39, 2), (40, 6), (41, 2), (42, 2),
        ];
        for f in TMD710_SETTINGS_FIELDS {
            let p = f.mu + 1;
            let (_, size) = MEASURED
                .iter()
                .find(|(mp, _)| *mp == p)
                .unwrap_or_else(|| panic!("p{p} is not in the measured set"));
            let emitted = match &f.kind {
                TK::Bool => 2,
                TK::Enum { labels } => labels.len(),
                TK::Uint { min, max } => (max - min) as usize + 1,
            };
            assert_eq!(
                emitted, *size,
                "{}: emits {emitted} options, the radio accepted {size}",
                f.key
            );
        }
    }

    /// ★ The pairing the skill requires: **one sheet, both halves.** A table
    /// entry with no form field is a setting nobody can reach; a form field with
    /// no table entry silently does nothing when saved. Both are generated from
    /// `MEASURED.md` by one script, and this is what stops them drifting after.
    ///
    /// It also checks the labels, which is the only thing that reads `TF::label`
    /// and `TF::menu` — the schema is what the form renders, so a table label
    /// that disagrees with it means the two were regenerated from different
    /// sheets.
    #[test]
    fn the_table_and_the_profile_schema_describe_the_same_fields() {
        let schema: Vec<serde_json::Value> =
            serde_json::from_str(crate::seed::TMD710_SETTINGS_SCHEMA).expect("schema parses");
        assert_eq!(schema.len(), TMD710_SETTINGS_FIELDS.len());

        for f in TMD710_SETTINGS_FIELDS {
            let entry = schema
                .iter()
                .find(|e| e["key"] == f.key)
                .unwrap_or_else(|| panic!("{} has no form field", f.key));

            assert_eq!(entry["label"], serde_json::json!(f.display()), "{}", f.key);

            match &f.kind {
                TK::Bool => assert_eq!(entry["type"], "boolean", "{}", f.key),
                TK::Uint { min, max } => {
                    assert_eq!(entry["type"], "integer", "{}", f.key);
                    assert_eq!(entry["min"], serde_json::json!(min), "{}", f.key);
                    assert_eq!(entry["max"], serde_json::json!(max), "{}", f.key);
                }
                TK::Enum { labels } => {
                    assert_eq!(entry["type"], "enum", "{}", f.key);
                    let opts: Vec<&str> = entry["options"]
                        .as_array()
                        .expect("options")
                        .iter()
                        .map(|o| o.as_str().expect("option string"))
                        .collect();
                    let mine: Vec<&str> = labels.iter().map(|(_, l)| *l).collect();
                    assert_eq!(opts, mine, "{} options disagree", f.key);
                }
            }
        }

        for e in &schema {
            let key = e["key"].as_str().expect("key");
            assert!(
                TMD710_SETTINGS_FIELDS.iter().any(|f| f.key == key),
                "the form offers {key:?}, which no table entry writes — saving it \
                 would do nothing"
            );
        }
    }

    /// The seven that must stay out. Their sizes are known and their meanings
    /// are not, and this is the assertion that stops someone filling them in
    /// from a published table — the same table that was wrong about their
    /// ranges in the first place.
    #[test]
    fn the_undetermined_parameters_are_not_exposed() {
        for p in [25, 29, 30, 31, 32, 33, 34] {
            assert!(
                !TMD710_SETTINGS_FIELDS.iter().any(|f| f.mu + 1 == p),
                "p{p}'s encoding has not been measured and must not be offered"
            );
        }
        assert_eq!(TMD710_SETTINGS_FIELDS.len(), 35);
    }

    /// Keys are what a saved profile stores, so a duplicate would make one field
    /// silently overwrite another on load.
    #[test]
    fn keys_and_indices_are_unique_and_inside_the_line() {
        let mut keys: Vec<&str> = TMD710_SETTINGS_FIELDS.iter().map(|f| f.key).collect();
        let n = keys.len();
        keys.sort_unstable();
        keys.dedup();
        assert_eq!(keys.len(), n, "duplicate settings key");

        let mut idx: Vec<usize> = TMD710_SETTINGS_FIELDS.iter().map(|f| f.mu).collect();
        idx.sort_unstable();
        idx.dedup();
        assert_eq!(idx.len(), n, "two fields claim one MU parameter");
        assert!(
            TMD710_SETTINGS_FIELDS.iter().all(|f| f.mu < MENU_FIELDS),
            "a field points past the {MENU_FIELDS} parameters an MU line has"
        );
    }

    /// A real line decodes, and the two parameters that were pinned on the radio
    /// itself decode to what the radio was showing.
    #[test]
    fn the_real_menu_line_decodes() {
        let menu = Menu::parse(REAL_MU).unwrap();
        let v = decode(&menu);
        assert_eq!(v["key-beep"], json!(false), "p1 was 0 and KEY BEEP was off");
        assert_eq!(v["display-brightness"], json!("Level 8"), "p26 was 8");
        // p2 = 4 and the display is stored + 1 — the off-by-one the sweep found.
        assert_eq!(v["beep-volume"], json!("5"));
    }

    /// ★ A settings write is a PATCH. The seven unexposed parameters — the PF
    /// keys among them — must come back byte-identical, because `MU` writes all
    /// 42 and an operator's key assignments are not this form's to touch.
    #[test]
    fn patching_leaves_every_unexposed_parameter_exactly_as_found() {
        let base = Menu::parse(REAL_MU).unwrap();
        let (patched, written) = patch(&base, &json!({ "key-beep": true })).unwrap();
        assert_eq!(written, 1);
        for p in [25, 29, 30, 31, 32, 33, 34] {
            assert_eq!(
                patched.field(p).unwrap(),
                base.field(p).unwrap(),
                "p{p} was rewritten by a patch that only set the key beep"
            );
        }
        // And the one field asked for did move, with the radio's own width.
        assert_eq!(patched.field(1).unwrap(), "1");
        assert_eq!(base.diff(&patched).len(), 1);
    }

    /// Widths are part of the line: p9 is two characters on this radio, so a
    /// patched `0` has to go back as `00` or every field after it shifts.
    #[test]
    fn a_patched_value_keeps_the_radios_own_width() {
        let base = Menu::parse(REAL_MU).unwrap();
        assert_eq!(base.field(9).unwrap(), "10");
        let (patched, _) = patch(&base, &json!({ "playback-repeat-interval": 0 })).unwrap();
        assert_eq!(patched.field(9).unwrap(), "00");
        assert_eq!(Menu::parse(&patched.to_line()).unwrap().to_line(), patched.to_line());
    }

    /// The numeric fallback. An unlabelled value decodes to a number, is saved
    /// into the profile, and must survive the round trip — otherwise every later
    /// write fails and the radio becomes unprogrammable from the app.
    #[test]
    fn an_unlabelled_value_round_trips_as_a_number() {
        let f = TMD710_SETTINGS_FIELDS
            .iter()
            .find(|f| matches!(f.kind, TK::Enum { .. }))
            .unwrap();
        assert_eq!(encode_one(f, &json!(64)).unwrap(), 64);
        assert!(encode_one(f, &json!("not an option")).is_err());
    }

    /// Out-of-range numbers are refused rather than clamped — the radio would
    /// answer `?` and the whole line would be rejected, so catching it here
    /// names the field instead of failing the write.
    #[test]
    fn a_value_outside_the_measured_range_is_refused() {
        let interval = TMD710_SETTINGS_FIELDS
            .iter()
            .find(|f| f.key == "playback-repeat-interval")
            .unwrap();
        let err = encode_one(interval, &json!(61)).unwrap_err();
        assert!(err.contains("0..=60") && err.contains("Menu 008"), "{err}");
    }
}
