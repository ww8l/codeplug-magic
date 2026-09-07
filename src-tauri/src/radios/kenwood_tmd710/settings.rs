//! The TM-D710's menu settings — **two transports, one form** (#113).
//!
//! One `MU` line carries all **42** menu parameters, and setting any of them
//! means sending all 42 back. That is this module.
//!
//! ⚠⚠ It is not the radio's settings. `MU` reaches 42 of the radio's ~115
//! menus and **none of the 600-series**, which is the APRS/TNC feature the
//! radio is named for. Those live in the image behind `0M PROGRAM` and are
//! [`super::image_settings`]'s half. A settings read here is both exchanges and a
//! settings write is both again, because an operator's profile is one thing.
//!
//! ★ This radio shipped a correct, fully measured 35-field schema with **no
//! APRS on an APRS radio** for a whole session, because one command's coverage
//! was taken for the radio's settings. The join below is the fix, and the
//! `carries_every_group_the_radio_advertises` test is what stops it recurring.
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
//! ## ★★★ s133: the menu NUMBERS were the G's, and ten were wrong
//!
//! A full audit against the **A** manual's own menu table, asked for after the
//! form went on screen. The values were right; the numbers beside them were not,
//! and a wrong number sends an operator to the wrong menu:
//!
//! | field | said | the A actually has | what that number IS on the A |
//! |---|---|---|---|
//! | VHF AIP | 100 | **103** | 100 is PROGRAMMABLE VFO |
//! | UHF AIP | 101 | **104** | 101 is STEP |
//! | Microphone key lock | 513? | **513** ✓ | the `?` was unearned doubt |
//! | Scan resume method | 907? | **514** | |
//! | Auto power off | 917? | **516** | |
//! | External data band | 918? | **517** | |
//! | External data speed | 919? | **518** | |
//! | SQC output source | 921? | **520** | |
//! | Auto PM store | 922? | **521** | |
//! | Display partition bar | 928 | **527** | |
//!
//! ★ The seven `9xx?` guesses came from a G-oriented source. The A puts all of
//! them in the 5xx AUX group, and the A manual states every one. This is the
//! same defect as the 6xx work — a source written for the **G** used on a
//! non-G radio — and it survived because a menu number is documentation and
//! nothing tests it. `MU`'s parameter order **is** the A's menu order, which is
//! what makes the corrected numbers self-consistent.
//!
//! ## ★ p25 is menu 403 or 406, and it matters which
//!
//! `MU` follows menu order, so p25 sits between p24 (402) and p26 (501). The A
//! manual leaves exactly two three-option menus in that gap, and p25's measured
//! size is 3:
//!
//! - **403 REPEATER MODE** — `CROSS BAND / LOCKED TX:A-BAND / LOCKED TX:B-BAND`
//! - **406 REPEATER ID TX** — `OFF / MORSE / VOICE`
//!
//! ⚠ One front-panel change to menu 403 settles it. Until then it stays
//! unexposed, and the reason is not tidiness: 403 is **cross-band repeat**, so
//! guessing wrong would make the radio transmit on a band the operator never
//! chose. This is the one omitted parameter whose identity is now nearly known
//! and still must not be shipped.
//!
//! ## What `MU` cannot reach at all
//!
//! Twelve menus the A has and this command has no parameter for: **105**
//! S-METER SQUELCH, **110** WEATHER ALERT, **203** GROUP LINK, **504** CONTRAST,
//! **505** display reverse, **515** VISUAL SCAN, **519** PC PORT BAUDRATE,
//! **522** REMOTE ID, **523** REMOTE ANSWER BACK, **524**-**526** DATE/TIME/TIME
//! ZONE, **528** COM PORT BAUDRATE. Plus the per-band and per-memory menus
//! (100-102, 200, 202, 204, 301, 400, 405) which are not profile settings.
//!
//! ★ Several of those **are** in the config window this driver now reads —
//! CHIRP names contrast, PC port baud, visual scan, group link, S-meter squelch,
//! WX alert and repeater mode inside the `0x0200` block. That is a real second
//! tranche and it is **not** shipped: CHIRP's field claims for this radio have
//! never been checked, and its APRS claims were useless while its structure was
//! right. Each would need the factory-default cross-check before it could ship.
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

use super::image_settings as imgset;
use super::image::ProgramMode;
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
    /// `MU`, then the APRS block out of program mode — one port session.
    ///
    /// ⚠ The image half is **not** best-effort. A read that quietly came back
    /// with 35 of 57 fields would look like a success and be exactly the failure
    /// this driver already shipped once, so a radio that will not enter program
    /// mode is an error naming what to do about it.
    fn read_settings(&self, port: &str, _schema_json: &str) -> Result<SettingsCapture, String> {
        let mut p = open_port(port)?;
        // ⚠ Identity first even on the READ. The read is harmless, but it feeds a
        // profile that `write_settings` later pushes back, so a menu line decoded
        // from the wrong Kenwood becomes 42 wrong values on the next write.
        super::confirm_model(&mut *p)?;
        let line = ask_settling(&mut *p, "MU")?;
        let menu = Menu::parse(&line)?;

        let mut pm = ProgramMode::enter(&mut *p)?;
        let wins = imgset::read_all(&mut pm)?;
        pm.leave()?;

        let Value::Object(mut settings) = decode(&menu) else {
            unreachable!("decode returns an object")
        };
        imgset::decode(&wins, &mut settings);

        Ok(SettingsCapture {
            settings: Value::Object(settings),
            // The backup for a live-mode radio is a TRANSCRIPT — and this radio
            // has two transports, so the file carries both halves: the menu line
            // and the APRS block as hex. Between them they are everything a
            // settings write on this radio can clobber.
            backup: backup_text(&line, &wins).into_bytes(),
            backup_ext: "txt",
        })
    }
}

/// The pre-write backup: one file holding both transports' state.
///
/// Written so `xxd -r` is not needed to read it and a person can see at a glance
/// which half is which — a backup nobody can interpret is not a backup.
fn backup_text(line: &str, wins: &imgset::Windows) -> String {
    let mut out = format!("{line}\n");
    for (w, buf) in wins {
        out.push_str(&format!("# {w:?} window, 16 bytes a line\n"));
        for (i, chunk) in buf.chunks(16).enumerate() {
            let addr = w.base() as usize + i * 16;
            let hex: Vec<String> = chunk.iter().map(|b| format!("{b:02X}")).collect();
            out.push_str(&format!("{addr:04X}  {}\n", hex.join(" ")));
        }
    }
    out
}

impl SettingsWriter for super::KenwoodTmD710 {
    /// Read both halves, back them up, patch the profile's fields over each,
    /// write, and read back to verify — one port session, two transports.
    ///
    /// ⚠ **Not yet run on a real radio, and now in two ways.** Reading `MU` is
    /// proven and writing one parameter at a time is proven by the 42-parameter
    /// sweep; reading the APRS block and writing differing runs into it are both
    /// proven by `d710_restore_diff`, which restored this radio to pristine.
    /// What is **not** proven is (a) a profile's worth of fields patched in one
    /// go and (b) **entering program mode after an `MU` exchange on the same
    /// open port**. Neither has been in front of the radio. In this repo a
    /// working read path has twice hidden a dead write path, so it is stated
    /// rather than assumed — this is a hardware-ladder step 5 item.
    fn write_settings(
        &self,
        port: &str,
        settings: &Value,
        _schema_json: &str,
        backup_dir: &Path,
    ) -> Result<SettingsWriteReport, String> {
        let mut p = open_port(port)?;
        // ⚠⚠ Identity first. This path writes 42 menu parameters AND raw bytes into
        // the image at `0x8100`/`0x0200`, and it had no model check of any kind —
        // the only accidental guard was `Menu::parse` counting 42 fields, which
        // says nothing about the image half. It is also the path that reaches the
        // APRS block, so it is the one that most needed the check.
        super::confirm_model(&mut *p)?;
        let before = ask_settling(&mut *p, "MU")?;
        let base = Menu::parse(&before)?;

        let mut pm = ProgramMode::enter(&mut *p)?;
        let img_before = imgset::read_all(&mut pm)?;
        pm.leave()?;

        // ⚠ The backup is written before a single byte is sent, and it holds
        // BOTH transports — a settings write on this radio can move the menu
        // line and the APRS block, so a backup of one half is not a backup.
        std::fs::create_dir_all(backup_dir).map_err(|e| e.to_string())?;
        let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
        let backup_path = backup_dir.join(format!("kenwood_tmd710-menu-{stamp}.txt"));
        std::fs::write(&backup_path, backup_text(&before, &img_before)).map_err(|e| e.to_string())?;

        let (wanted, mut fields_written) = patch(&base, settings)?;

        // ⚠ Both halves are encoded BEFORE either is written. A profile whose
        // APRS half is unencodable must not leave the radio with its menus
        // already changed — this is the cheap half of atomicity, and the only
        // half a two-transport radio can have.
        let (img_wanted, img_changed) = imgset::patch(&img_before, settings)?;

        let failed = write_menu(&mut *p, &wanted)?;

        let (windows_written, img_verified) = if img_changed == 0 {
            (Vec::new(), true)
        } else {
            let mut pm = ProgramMode::enter(&mut *p)?;
            let r = imgset::write_narrow(&mut pm, &img_before, &img_wanted);
            pm.leave()?;
            r?
        };
        // ⚠ Counted only once the read-back agrees. Adding this before the write
        // let a report say "27 fields written" while `verified` was `false` and the
        // note told the operator to treat the 600-series settings as NOT written.
        if img_verified {
            fields_written += img_changed;
        }

        let mut notes: Vec<String> = Vec::new();
        if !failed.is_empty() {
            let names: Vec<String> = failed
                .iter()
                .map(|(p, mine, theirs)| format!("p{p}: sent {mine}, radio kept {theirs}"))
                .collect();
            notes.push(format!(
                "{} menu parameter(s) did not take: {}",
                failed.len(),
                names.join("; ")
            ));
        }
        if !img_verified {
            notes.push(
                "an image window read back different from what was written. On this protocol \
                 the radio answers 0x06 whether or not it kept a write, so the read-back is \
                 the only evidence — treat the 600-series settings as NOT written."
                    .to_string(),
            );
        }

        Ok(SettingsWriteReport {
            fields_written,
            // `write_menu` re-reads the line and diffs it, and
            // `write_block_narrow` re-reads the block — both are real
            // read-backs and not a buffer compared with itself, the mistake
            // found in the TH-D72's review.
            verified: Some(failed.is_empty() && img_verified),
            note: (!notes.is_empty()).then(|| notes.join(" ")),
            backup_path: backup_path.to_string_lossy().into_owned(),
            expected_path: None,
            windows_written,
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
        // The form is both transports and `super::image_settings` asserts the
        // same pairing over its half.
        //
        // ⚠ Partition on the keys that table actually owns, **not on a name
        // prefix**. Menu 500's power-on message is an image field and is not
        // called `aprs-*`, so a prefix test leaves it on this side and both
        // halves claim it. The generator had the identical bug.
        let theirs: Vec<&str> = crate::radios::kenwood_tmd710::image_settings::TMD710_IMAGE_FIELDS
            .iter()
            .map(|f| f.key)
            .collect();
        let mine: Vec<&serde_json::Value> = schema
            .iter()
            .filter(|e| e["type"] != "section")
            .filter(|e| !theirs.contains(&e["key"].as_str().unwrap_or_default()))
            .collect();
        let extra: Vec<&str> = mine
            .iter()
            .map(|e| e["key"].as_str().unwrap_or_default())
            .filter(|k| !TMD710_SETTINGS_FIELDS.iter().any(|f| f.key == *k))
            .collect();
        assert_eq!(mine.len(), TMD710_SETTINGS_FIELDS.len(), "extra: {extra:?}");

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
                    assert_eq!(entry["type"], "select", "{}", f.key);
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

        for e in &mine {
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
