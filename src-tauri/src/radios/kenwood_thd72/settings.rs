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
//! ⚠ `MU` is **not** everything the radio holds, and the gap is large. `MU`
//! carries the 1xx menus only; the radio has ~144 menu items in total, including
//! 72 APRS ones (the 3xx menus) and 15 packet ones (2xx), all of which live in
//! the image at `0x0400`-`0x0C00` and none of which are here. Menu 102, Lamp
//! Control, is a 1xx item with no `MU` field either. Measuring those is its own
//! campaign; this module covers the 19 `MU` parameters and claims nothing more.
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
    pub src: MSrc,
    pub menu: Option<&'static str>,
    pub kind: MK,
}

/// Where a field's value lives. The TH-D72 has two settings transports and
/// needs both: `MU` carries 19 menu parameters as one ASCII line, and the other
/// 103 exist ONLY as bytes in the clone image — there is no `MU` parameter for
/// any of them, which is why the s125 ten-pass campaign had to measure them.
///
/// ⚠ Where the two overlap they AGREE: `MU`'s battery saver, APO and key beep
/// are `0x314`, `0x315` and `0x317`, measured independently by the campaign and
/// then confirmed on the radio's own menus. The generator drops the image copy
/// of a duplicated parameter so no value exists twice under two names.
pub(crate) enum MSrc {
    /// 0-based index into the 19 comma-separated parameters of an `MU` line.
    Mu(usize),
    /// A byte in the 64 KiB clone image.
    ///
    /// `mask` is the bits the campaign PROVED move with this field. For a
    /// checkbox that is its single bit, and only that bit is written, because
    /// booleans are packed several to a byte. For a combo the whole byte is the
    /// value — `byte == index` held in all ten passes for every emitted enum,
    /// and no combo shares a byte with another field.
    Image {
        addr: usize,
        mask: u8,
        /// The stored bit is the complement of the control's state.
        active_low: bool,
    },
}

impl MSrc {
    /// The field's raw value, as the table's enum labels number it.
    fn read(&self, mu: &[u8], image: &[u8], kind: &MK) -> u8 {
        match *self {
            MSrc::Mu(i) => mu[i],
            MSrc::Image { addr, mask, active_low } => match kind {
                MK::Bool => u8::from(((image[addr] & mask) != 0) != active_low),
                _ => image[addr],
            },
        }
    }

    /// Patch the field in place. A checkbox touches only its own bit.
    fn write(&self, mu: &mut [u8], image: &mut [u8], kind: &MK, v: u8) {
        match *self {
            MSrc::Mu(i) => mu[i] = v,
            MSrc::Image { addr, mask, active_low } => match kind {
                MK::Bool => {
                    let on = (v != 0) != active_low;
                    image[addr] = if on { image[addr] | mask } else { image[addr] & !mask };
                }
                _ => image[addr] = v,
            },
        }
    }
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
fn decode(raw: &[u8], image: &[u8]) -> Value {
    let mut out = serde_json::Map::new();
    for f in THD72_SETTINGS_FIELDS {
        let v = f.src.read(raw, image, &f.kind);
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
/// from nothing: `MU` sets all 19 parameters at once, so a parameter the profile
/// does not carry must go back exactly as it came. Building the line from
/// defaults would quietly rewrite every field the operator did not touch — and
/// on this radio that is eighteen of them.
fn encode_over(
    base: &[u8],
    base_image: &[u8],
    settings: &Value,
) -> Result<(Vec<u8>, Vec<u8>, usize), String> {
    let mut raw = base.to_vec();
    let mut image = base_image.to_vec();
    let mut written = 0usize;
    for f in THD72_SETTINGS_FIELDS {
        let Some(v) = settings.get(f.key) else { continue };
        if v.is_null() {
            continue;
        }
        let encoded = encode_one(f, v)?;
        if f.src.read(&raw, &image, &f.kind) != encoded {
            written += 1;
        }
        f.src.write(&mut raw, &mut image, &f.kind, encoded);
    }
    Ok((raw, image, written))
}

/// One field's form value as the byte the radio stores.
///
/// Shared by the settings write and the program path so the two cannot drift:
/// a value encoded one way here and another way there is a wrong value on a
/// real radio, and the two paths write the same bytes.
fn encode_one(f: &MF, v: &Value) -> Result<u8, String> {
    Ok(match &f.kind {
        MK::Bool => match v.as_bool() {
            Some(b) => u8::from(b),
            None => return Err(format!("{} expects true or false, got {v}", f.key)),
        },
        MK::Uint { min, max } => {
            let n = v
                .as_u64()
                .ok_or_else(|| format!("{} expects a number, got {v}", f.key))?;
            if n < *min as u64 || n > *max as u64 {
                return Err(format!("{} is {n}, outside the radio's {min}..={max}", f.key));
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
    })
}

/// Patch a codeplug image with the profile's image-backed settings.
///
/// Used by the PROGRAM path, which writes an image and therefore cannot carry
/// the 19 `MU` parameters — those stay as the radio holds them and are the
/// profile editor's own write to make. Everything else the profile carries is
/// applied here, so a program run does not silently drop five sixths of the
/// operator's settings.
///
/// Out-of-range values are stripped and reported the same way a settings write
/// strips them, rather than being clamped into something the operator never
/// chose.
/// Returns how many fields were written, and any the radio could not take.
///
/// ⚠ BOTH halves of that are reported to the operator, and neither used to be.
/// `settings_written` was hard-coded `None` — whose documented meaning is "the
/// profile carried none" — so a program run that wrote 103 settings told the
/// operator it had written zero. And the out-of-range notes were discarded at
/// the call site, so a value the radio cannot take was dropped silently. Both
/// are the same defect: a report saying something untrue about what reached the
/// radio. The dialog renders both fields, so they are read, not decorative.
pub(crate) fn apply_image_settings(
    image: &mut [u8],
    settings: &Value,
    schema_json: &str,
) -> Result<(usize, Vec<String>), String> {
    let mut settings = settings.clone();
    let notes = settings_bounds::strip_out_of_range(schema_json, &mut settings);
    let mut mu = [0u8; MU_FIELDS];
    let mut written = 0usize;
    for f in THD72_SETTINGS_FIELDS {
        let MSrc::Image { addr, .. } = f.src else { continue };
        let Some(v) = settings.get(f.key) else { continue };
        if v.is_null() {
            continue;
        }
        // An address past the end of the image would panic mid-program, which
        // on this path means a half-built codeplug and a Tauri command that
        // dies rather than reports. The table is asserted in range by a unit
        // test; this covers the image being the wrong thing.
        if addr >= image.len() {
            return Err(format!(
                "{} is at 0x{addr:04X}, past the end of a {}-byte image",
                f.key,
                image.len()
            ));
        }
        let encoded = encode_one(f, v)?;
        f.src.write(&mut mu, image, &f.kind, encoded);
        written += 1;
    }
    Ok((written, notes))
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

/// Refuse an image that is not a whole one before anything indexes into it.
///
/// ⚠ `MSrc::read`/`write` index `image[addr]` directly, so a short buffer is a
/// PANIC inside a Tauri command — the app dies rather than reporting. Every
/// caller today passes a `protocol::download`, which is always 64 KiB, but that
/// is an assumption about another module and this is the boundary where it can
/// be checked cheaply and said out loud.
fn whole_image(image: &[u8]) -> Result<(), String> {
    if image.len() != super::layout::IMAGE_LEN {
        return Err(format!(
            "the radio returned {} bytes, not the {} a TH-D72 image is — settings \
             were not read",
            image.len(),
            super::layout::IMAGE_LEN
        ));
    }
    Ok(())
}

/// Read the menu line, with the identity check in front of it.
fn read_line(p: &mut dyn serialport::SerialPort) -> Result<Vec<u8>, String> {
    protocol::identify(p)?;
    let reply = protocol::command(p, "MU")?;
    parse_mu(&reply)
}

impl SettingsReader for super::KenwoodThd72 {
    /// `MU` first, then a clone download — BOTH transports, because the radio
    /// keeps its settings in two places and 103 of the 122 fields exist only in
    /// the image. `MU` is one ASCII round trip; the clone costs the 16 s read
    /// and the settle afterwards, and there is no way around it: no `MU`
    /// parameter exists for any of the image fields.
    ///
    /// The backup is the whole 64 KiB image, not the `MU` line. It has to be:
    /// this path can now write image bytes, so an `MU`-only backup would be a
    /// partial restore of a write that was not partial.
    fn read_settings(&self, port: &str, _schema_json: &str) -> Result<SettingsCapture, String> {
        let mut p = protocol::open_port(port)?;
        let raw = read_line(&mut *p)?;
        let image = protocol::download(&mut *p)?;
        whole_image(&image)?;
        Ok(SettingsCapture {
            settings: decode(&raw, &image),
            backup: image,
            backup_ext: "img",
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
        let base_image = protocol::download(&mut *p)?;
        whole_image(&base_image)?;
        drop(p);

        let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
        let backup_path = backup_dir.join(format!("thd72-preprofile-{stamp}.img"));
        std::fs::write(&backup_path, &base_image)
            .map_err(|e| format!("could not write backup {}: {e}", backup_path.display()))?;

        let (raw, image, fields_written) = encode_over(&base, &base_image, &settings)?;
        if raw == base && image == base_image {
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

        // ⚠⚠ THE IMAGE HAS TO BE WRITTEN, not just computed. 103 of the 122
        // fields live only here, so skipping this would leave the form filling
        // correctly, the MU line landing, and five sixths of the settings never
        // reaching the radio — a working read path hiding a dead write path,
        // which this codebase has shipped twice.
        if image != base_image {
            protocol::reconnect_after_clone(port).map(drop)?;
            crate::radios::driver::ImageProgrammer::upload_image(
                &super::KenwoodThd72,
                port,
                &image,
            )?;
        }

        // The clone session ended the ASCII session with it, so the radio has to
        // be picked back up before it will answer `MU` again.
        let mut p = protocol::reconnect_after_clone(port)?;
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
                    .filter(|f| {
                        f.src.read(&after, &image, &f.kind)
                            != f.src.read(&raw, &image, &f.kind)
                    })
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

    /// A whole image with every image-backed field set to a value the table
    /// knows about.
    ///
    /// Not a zeroed buffer: twelve fields store `index + 1`, so zero is not a
    /// value any of their enums carries, and decode would hand back a bare
    /// number that encode then refuses. Seeding each field with its own lowest
    /// labelled value keeps the round trip exercising real encodings instead of
    /// a coincidence. Real radio images live in gitignored `scratchpad/`, so CI
    /// cannot use one.
    fn sample_image() -> Vec<u8> {
        let mut img = vec![0u8; super::super::layout::IMAGE_LEN];
        for f in THD72_SETTINGS_FIELDS {
            let MSrc::Image { .. } = f.src else { continue };
            let v = match &f.kind {
                MK::Bool => 1,
                MK::Uint { min, .. } => *min,
                MK::Enum { labels } => labels.iter().map(|(v, _)| *v).min().unwrap_or(0),
            };
            let mut mu = [0u8; MU_FIELDS];
            f.src.write(&mut mu, &mut img, &f.kind, v);
        }
        img
    }

    #[test]
    fn the_radios_own_values_decode_to_what_the_screen_showed() {
        let raw = parse_mu(REAL).unwrap();
        let v = decode(&raw, &sample_image());
        // Both confirmed on the radio's own menus: APO on 111, balance on 120.
        assert_eq!(v["apo"], json!("Off"));
        assert_eq!(v["balance"], json!("Center"));
    }

    #[test]
    fn every_field_round_trips_through_encode_and_decode() {
        let base = parse_mu(REAL).unwrap();
        let base_image = sample_image();
        let decoded = decode(&base, &base_image);
        let (raw, image, _) =
            encode_over(&base, &base_image, &decoded).expect("re-encode what we just decoded");
        assert_eq!(raw, base, "decode -> encode must be lossless for the MU line");
        // ★ This is the check that matters now: 103 of the 122 fields are image
        // bytes, and a decode that loses one would come back as a CHANGED byte
        // in an image the operator never asked to modify.
        assert!(
            image == base_image,
            "decode -> encode changed the image; it must be lossless there too"
        );
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
        let (raw, image, written) =
            encode_over(&base, &sample_image(), &json!({"apo": "60 minutes"})).unwrap();
        assert_eq!(image, sample_image(), "an MU-only write must not touch the image");
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
        assert!(matches!(f.src, MSrc::Mu(1)));
        // Raw 0..15, but the radio DISPLAYS raw + 1 — its bar graph carries no
        // numbers, and RT Systems shows the stored index 7 as "8". Shipping the
        // raw value would have put a control on screen reading one below what
        // the operator sees, so the labels carry the offset.
        match &f.kind {
            MK::Enum { labels } => {
                assert_eq!(labels.len(), 16, "sixteen levels, per the radio and RT Systems");
                assert_eq!(labels[0], (0, "1"), "raw 0 is level 1, not level 0");
                assert_eq!(labels[15], (15, "16"), "raw 15 is level 16");
            }
            _ => panic!("contrast labels carry a +1 display offset, so it cannot be a bare integer"),
        }
    }

    #[test]
    fn a_value_the_radio_has_no_option_for_is_refused_by_name() {
        let base = parse_mu(REAL).unwrap();
        let err = encode_over(&base, &sample_image(), &json!({"apo": "90 minutes"}))
            .expect_err("no such option");
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
        let mut mus = std::collections::HashSet::new();
        // Bytes are shared on purpose — booleans pack several to a byte — so
        // the thing that must be unique for an image field is its (address,
        // mask) PAIR. Two fields claiming the same bit would silently overwrite
        // each other on every write, exactly as two claiming one MU parameter
        // would.
        let mut bits = std::collections::HashSet::new();
        for f in THD72_SETTINGS_FIELDS {
            match f.src {
                MSrc::Mu(i) => {
                    assert!(i < MU_FIELDS, "{} indexes parameter {i}", f.key);
                    assert!(mus.insert(i), "two fields claim parameter {i}");
                }
                MSrc::Image { addr, mask, .. } => {
                    assert!(
                        addr < super::super::layout::IMAGE_LEN,
                        "{} addresses 0x{addr:04X}, past the end of the image",
                        f.key
                    );
                    assert!(mask != 0, "{} has an empty mask", f.key);
                    assert!(
                        bits.insert((addr, mask)),
                        "{} claims 0x{addr:04X}/{mask:#04x}, which another field already owns",
                        f.key
                    );
                }
            }
        }
    }

    /// The step-6 gate: a program run must carry memories AND settings.
    ///
    /// ⚠ The failure this guards is not hypothetical. `program_codeplug` used to
    /// document `req.settings` as IGNORED — correct when the only transport was
    /// `MU`, and silently wrong the moment 103 settings became image bytes. The
    /// operator would fill the profile, the form would read it back, and the
    /// program run would write none of it.
    #[test]
    fn a_program_run_carries_settings_into_the_image_without_touching_memories() {
        let schema = include_str!("../../thd72_settings_schema.json");
        let mut img = sample_image();
        // A byte inside the memory region, to prove settings stay out of it.
        img[super::super::layout::MEMORY_BASE + 3] = 0xAB;

        // Two fields confirmed on the radio's own menus in s125. Keyed by
        // CONTROL ID, not by label: a key is what a value is saved under in an
        // operator's profile, so deriving it from a label meant that improving
        // the label silently renamed the setting. That is why this test names
        // `common1-1152` rather than `common1-battery-type`.
        let want = json!({ "common1-1152": "Alkaline", "gps-1311": "Tokyo" });
        let (written, notes) = apply_image_settings(&mut img, &want, schema).expect("apply");
        assert!(notes.is_empty(), "nothing should have been out of range: {notes:?}");
        // The count is REPORTED to the operator as "N settings", so it has to be
        // the number actually written, not the number asked for.
        assert_eq!(written, 2, "both fields should be counted as written");

        assert_eq!(img[0x0316], 1, "battery type must land at its measured address");
        assert_eq!(img[0x0A03], 1, "GPS datum must land at its measured address");
        assert_eq!(
            img[super::super::layout::MEMORY_BASE + 3],
            0xAB,
            "a settings patch must not reach into the memory region"
        );

        // And the values come back out as the operator set them.
        let round = decode(&[0u8; MU_FIELDS], &img);
        assert_eq!(round["common1-1152"], json!("Alkaline"));
        assert_eq!(round["gps-1311"], json!("Tokyo"));
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
