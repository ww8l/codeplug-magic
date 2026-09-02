//! BT-9000 non-channel settings, read from and written to the function block.
//!
//! ## The write is deliberately narrow
//!
//! Settings live in one 256-byte segment at radio `0x9000`. The driver *can*
//! write the whole 33 KB clone image, and doing so to change a squelch level
//! would rewrite all 960 channel records — four minutes instead of a third of a
//! second, with the operator's memories in the write path for a change that
//! never touched them. So this module uses [`super::SETTINGS_SEGMENTS`] and
//! addresses nothing else. A transport that gains reach has to be narrowed on
//! purpose.
//!
//! ## An ACK is not a commit
//!
//! This radio answers `0x06` to blocks it does not always commit — the APRS
//! block acknowledges every write and never changes, verified four times. So a
//! settings write is followed by a read-back in the same session, and the
//! read-back is the only evidence reported. It costs 0.35 s here.
//!
//! ## The radio validates nothing
//!
//! It stored `127` in four function fields whose real maxima are 9, 2, 3 and 1,
//! and read them back unchanged. There is no hardware backstop: every bound has
//! to be enforced here, because a value written out of range is *stored*, not
//! rejected. [`encode_field`] is that backstop and refuses rather than clamps —
//! a silently adjusted value is how an operator ends up transmitting on a
//! setting they did not choose.
//!
//! ## What is NOT here
//!
//! Three fields of the ~43 the sheet names. That is not the radio's limit, it
//! is the measurement's: `scratchpad/binteradio_bt9000/MEASURED.md` grades every
//! row, and only rows settled on the radio's own screen are emitted. The rest
//! are measured but not settled — an option list confirmed at a single index
//! cannot tell the printed order from a reversed one, which is exactly how the
//! TH-D75 shipped a control that wrote Volume Link when the operator picked
//! Level 1. `SCREEN-CHECK.md` is the runbook that closes the gap.

use std::path::Path;

use serde_json::{json, Value};

use crate::radios::driver::{SettingsCapture, SettingsReader, SettingsWriteReport, SettingsWriter};

use super::bt9000_settings_table::{Enc, Kind, FIELDS, SF};
use super::{
    download_segments, handshake, open_port, upload_segments, FUNCTION_LEN, FUNCTION_OFFSET,
    FUNCTION_LIVE_LEN, READ_SEGMENTS, SETTINGS_READ_SEGMENTS, SETTINGS_SEGMENTS, SETTLE,
};

// ============================================================
// Encode / decode one field
// ============================================================

/// The field's value as the form carries it: a number for `Int`, the option
/// label for `Enum`, a bool for `Bool`.
fn decode_field(f: &SF, block: &[u8]) -> Value {
    let raw = block[f.addr];
    match f.kind {
        Kind::Bool => json!(raw != 0),
        Kind::Enum => match f.options.get(raw as usize) {
            Some(label) => json!(label),
            // The radio stores whatever it was handed, including by some other
            // tool. Surface the raw byte rather than inventing a label or
            // failing the whole read for one field.
            None => json!(format!("(unknown value {raw})")),
        },
        Kind::Int { lo, hi } => {
            let shown = match f.enc {
                Enc::Direct => i32::from(raw),
                Enc::Minus1 => i32::from(raw) + 1,
            };
            if (i32::from(lo)..=i32::from(hi)).contains(&shown) {
                json!(shown)
            } else {
                json!(null)
            }
        }
    }
}

/// Why a value could not be written.
#[derive(Debug)]
pub(crate) enum Reject {
    /// A `select` label this app's option list does not carry. That is not an
    /// error: it is how a value read off a radio this app cannot name survives
    /// a round trip (see `settings_bounds`' own note on selects). The field is
    /// left exactly as the radio has it, and the caller gets a note.
    Unnameable(String),
    /// The wrong shape, or outside the field's range. This radio would store
    /// either one — it stored 127 in a field whose maximum is 1 — so the write
    /// stops here rather than being clamped into something plausible.
    Invalid(String),
}

/// The byte to store.
///
/// Refuses out-of-range rather than clamping: this radio stores a clamped value
/// just as happily as the right one, and the operator would never see the
/// difference.
fn encode_field(f: &SF, v: &Value) -> Result<u8, Reject> {
    match f.kind {
        Kind::Bool => v
            .as_bool()
            .map(u8::from)
            .ok_or_else(|| Reject::Invalid(format!("{}: expected true or false, got {v}", f.key))),
        Kind::Enum => {
            let s = v.as_str().ok_or_else(|| {
                Reject::Invalid(format!("{}: expected one of its options, got {v}", f.key))
            })?;
            f.options
                .iter()
                .position(|o| *o == s)
                .map(|i| i as u8)
                .ok_or_else(|| {
                    Reject::Unnameable(format!(
                        "{} [{}] holds {s:?}, which this app cannot name; left as the radio had it",
                        f.label, f.menu
                    ))
                })
        }
        Kind::Int { lo, hi } => {
            let n = v
                .as_i64()
                .ok_or_else(|| Reject::Invalid(format!("{}: expected a number, got {v}", f.key)))?;
            if !(i64::from(lo)..=i64::from(hi)).contains(&n) {
                return Err(Reject::Invalid(format!(
                    "{}: {n} is outside {lo}..={hi}",
                    f.key
                )));
            }
            Ok(match f.enc {
                Enc::Direct => n as u8,
                Enc::Minus1 => (n - 1) as u8,
            })
        }
    }
}

/// Decode every field out of a full clone image.
pub(crate) fn decode_settings(image: &[u8]) -> Value {
    let block = &image[FUNCTION_OFFSET..FUNCTION_OFFSET + FUNCTION_LEN];
    let mut out = serde_json::Map::new();
    for f in &FIELDS {
        out.insert(f.key.to_string(), decode_field(f, block));
    }
    Value::Object(out)
}

/// Patch the profile's settings into `image`'s function block. Returns how many
/// fields were written, and a note for each one that was deliberately left
/// alone.
///
/// Named `apply_profile_settings` rather than `apply_settings` to sit in the
/// right architectural bucket, and `radios/wiring.rs` keys on the difference.
/// `apply_settings` belongs to the CARD radios, whose settings ride out inside
/// an exported file — there the export path has to call the encoder, and twice
/// it did not, which is the guard that test exists to be. This radio is shaped
/// like the TD-H3 instead: settings go over the cable through `SettingsWriter`
/// as their own acknowledged operation, so the caller below IS the write path.
///
/// A key the profile does not carry is left exactly as the radio had it — this
/// is a patch, not a replace. On a radio that stores anything, writing a default
/// over a field the operator set and this app never measured would be a silent
/// change to a working radio.
pub(crate) fn apply_profile_settings(
    image: &mut [u8],
    settings: &Value,
) -> Result<(usize, Vec<String>), String> {
    let obj = settings
        .as_object()
        .ok_or_else(|| "profile settings are not a JSON object".to_string())?;
    let (mut written, mut notes) = (0, Vec::new());
    for f in &FIELDS {
        let Some(v) = obj.get(f.key) else { continue };
        // ⚠ A CLEARED number input is stored as `""`, not null — the form's
        // number field maps an empty box to the empty string. Treated as
        // "leave it alone" like a missing key, because the alternative is what
        // it used to do: reject the whole write, so clearing one field stopped
        // every OTHER field from reaching the radio too.
        if v.is_null() || v.as_str() == Some("") {
            continue;
        }
        match encode_field(f, v) {
            Ok(byte) => {
                image[FUNCTION_OFFSET + f.addr] = byte;
                written += 1;
            }
            Err(Reject::Unnameable(note)) => notes.push(note),
            Err(Reject::Invalid(e)) => return Err(e),
        }
    }
    Ok((written, notes))
}

// ============================================================
// SettingsReader / SettingsWriter
// ============================================================

impl SettingsReader for super::BinteradioBt9000 {
    /// Read the whole clone image and decode the settings out of it.
    ///
    /// The *read* is the full image rather than the one segment, because the
    /// command layer saves the capture as the session's backup and a
    /// function-block-only file is not a backup of anything. Reading everything
    /// is harmless; it is the write that has to be narrow.
    fn read_settings(&self, port: &str, _schema_json: &str) -> Result<SettingsCapture, String> {
        let mut p = open_port(port)?;
        let hs = handshake(&mut *p)?;
        let image = download_segments(&mut *p, &hs, &READ_SEGMENTS)?;
        Ok(SettingsCapture {
            settings: decode_settings(&image),
            backup: image,
            backup_ext: "img",
        })
    }
}

impl SettingsWriter for super::BinteradioBt9000 {
    /// Read + back up the whole image, patch the settings into its function
    /// block, write **only that block**, then read it back and compare.
    fn write_settings(
        &self,
        port: &str,
        settings: &Value,
        schema_json: &str,
        backup_dir: &Path,
    ) -> Result<SettingsWriteReport, String> {
        let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
        let backup_path = backup_dir.join(format!("bt9000-presettings-{stamp}.img"));

        let mut p = open_port(port)?;

        // 1. Read and back up everything, so the file beside the write is a
        //    whole radio and not just the part being changed.
        let hs = handshake(&mut *p)?;
        let mut image = download_segments(&mut *p, &hs, &READ_SEGMENTS)?;
        std::fs::write(&backup_path, &image)
            .map_err(|e| format!("could not write backup {}: {e}", backup_path.display()))?;

        // 2. Patch. The shared range check runs first so a stale profile value
        //    is dropped with a note rather than blocking the write; this
        //    driver's own encoder is then the backstop, because the radio is
        //    not one.
        let mut settings = settings.clone();
        let mut notes = crate::radios::settings_bounds::strip_out_of_range(
            schema_json,
            &mut settings,
        );
        let (fields_written, skipped) = apply_profile_settings(&mut image, &settings)?;
        notes.extend(skipped);
        let expected: Vec<u8> =
            image[FUNCTION_OFFSET..FUNCTION_OFFSET + FUNCTION_LEN].to_vec();

        // 3. Write the function segment alone. Fresh session: this radio needs
        //    a moment after a full read before it answers again.
        std::thread::sleep(SETTLE);
        let hs = handshake(&mut *p).map_err(|e| {
            crate::radios::driver::with_restore_hint(
                e,
                &backup_path,
                "Nothing was written. That file is the radio as it was read.",
            )
            .to_string()
        })?;
        upload_segments(&mut *p, &hs, &image, &SETTINGS_SEGMENTS).map_err(|e| {
            crate::radios::driver::with_restore_hint(
                e,
                &backup_path,
                "Keep that file. It is the only copy of what was on the radio \
                 before this write, and it can be uploaded back over the same cable.",
            )
        })?;

        // 4. Read the block back, and RETRY ONCE if it disagrees.
        //
        //    Measured on the radio (s128): a settings write acknowledged every
        //    block and did not commit, and an identical second write landed
        //    perfectly. The read-back is what caught it, so acting on the
        //    result is the point of having one — reporting "it did not take,
        //    try again" while holding a proven-idempotent 0.35 s write in hand
        //    is worse for the operator and no safer.
        //
        //    Exactly one retry. A radio that fails twice is not flaky, and
        //    hammering a write path on this platform is how radios have been
        //    damaged.
        std::thread::sleep(SETTLE);
        // ⚠ Verify ONCE and keep the answer. Calling it again for the report
        // meant a proven-committed write could still be announced as
        // unverified, because the second call is a fresh handshake and download
        // on a radio this driver documents as needing seconds to settle and as
        // wedging its handshake. Only an actual retry earns a second look.
        let mut outcome = verify(&mut *p, &expected);
        if matches!(outcome, Ok((false, _))) {
            let hs = handshake(&mut *p).map_err(|e| {
                crate::radios::driver::with_restore_hint(
                    e,
                    &backup_path,
                    "The first write did not commit and the radio would not answer for \
                     a retry. Keep that file — it is the radio as it was read.",
                )
                .to_string()
            })?;
            upload_segments(&mut *p, &hs, &image, &SETTINGS_SEGMENTS).map_err(|e| {
                crate::radios::driver::with_restore_hint(
                    e,
                    &backup_path,
                    "The first write did not commit and the retry failed. Keep that \
                     file — it is the radio as it was before either attempt.",
                )
            })?;
            notes.push(
                "The radio acknowledged the first write without committing it, so it \
                 was written a second time."
                    .to_string(),
            );
            std::thread::sleep(SETTLE);
            outcome = verify(&mut *p, &expected);
        }
        let (verified, verify_note) = match outcome {
            Ok(v) => v,
            Err(e) => (
                false,
                Some(format!(
                    "Settings written, but read-back verification could not run ({e}). \
                     This radio acknowledges blocks it does not always commit, so \
                     power-cycle it and use Read to confirm before trusting it."
                )),
            ),
        };
        notes.extend(verify_note);
        let note = (!notes.is_empty()).then(|| notes.join(" "));

        Ok(SettingsWriteReport {
            fields_written,
            verified: Some(verified),
            note,
            backup_path: backup_path.to_string_lossy().to_string(),
            expected_path: None,
            windows_written: Vec::new(),
        })
    }
}

/// Re-read the function block and compare it with what was sent.
fn verify(
    p: &mut dyn serialport::SerialPort,
    expected: &[u8],
) -> Result<(bool, Option<String>), String> {
    let hs = handshake(p)?;
    let back = download_segments(p, &hs, &SETTINGS_READ_SEGMENTS)?;
    // ⚠ Compare the LIVE area only. `0x80-0xFF` of this segment is a
    // firmware-maintained shadow of the settings, and it moves on its own —
    // comparing the whole 256 bytes reported a mismatch on a restore that had
    // in fact landed perfectly (measured on the radio, s128). Everything this
    // driver writes lives below `0x46`.
    let live = super::comparable(SETTINGS_READ_SEGMENTS[0]);
    let got = &back[live];
    let expected = &expected[..FUNCTION_LIVE_LEN];
    if got == expected {
        return Ok((true, None));
    }
    let bad: Vec<String> = FIELDS
        .iter()
        .filter(|f| got[f.addr] != expected[f.addr])
        .map(|f| {
            format!(
                "{} [{}] (wrote {}, read {})",
                f.label, f.menu, expected[f.addr], got[f.addr]
            )
        })
        .collect();
    let detail = if bad.is_empty() {
        "the differences are outside the fields this app writes".to_string()
    } else {
        bad.join(", ")
    };
    Ok((
        false,
        Some(format!(
            "The radio acknowledged the write but read back differently: {detail}. \
             The settings on the radio are NOT what was sent."
        )),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The table and the form schema come from one parse of one sheet, and a
    /// drift between them is invisible at runtime: a form field with no table
    /// entry silently does nothing, and a table entry with no form field is a
    /// setting nobody can reach.
    #[test]
    fn table_and_schema_describe_the_same_fields() {
        let schema: Vec<Value> = serde_json::from_str(crate::seed::BT9000_SETTINGS_SCHEMA).unwrap();
        let form: Vec<&str> = schema
            .iter()
            .filter(|f| f["type"] != "section")
            .map(|f| f["key"].as_str().unwrap())
            .collect();
        let table: Vec<&str> = FIELDS.iter().map(|f| f.key).collect();
        assert_eq!(form, table, "regenerate both with gen_bt9000_settings.py");
    }

    /// Every enum option in the schema must exist in the table in the same
    /// order, because the table's index IS the byte written to the radio.
    #[test]
    fn schema_options_match_the_stored_order() {
        let schema: Vec<Value> = serde_json::from_str(crate::seed::BT9000_SETTINGS_SCHEMA).unwrap();
        for f in &FIELDS {
            if f.kind != Kind::Enum {
                continue;
            }
            let entry = schema
                .iter()
                .find(|e| e["key"] == f.key)
                .unwrap_or_else(|| panic!("{} missing from the schema", f.key));
            let opts: Vec<&str> = entry["options"]
                .as_array()
                .unwrap()
                .iter()
                .map(|o| o.as_str().unwrap())
                .collect();
            assert_eq!(opts, f.options, "{}: schema and table disagree", f.key);
        }
    }

    /// The narrowed write must address the function segment and nothing else.
    /// Both constants are INDEXES into their tables, so a reordering of either
    /// would silently retarget every settings write at the channel records.
    #[test]
    fn settings_segments_address_only_the_function_block() {
        for (segs, cmd, what) in [
            (&SETTINGS_SEGMENTS[..], 0x57u8, "write"),
            (&SETTINGS_READ_SEGMENTS[..], 0x52u8, "read"),
        ] {
            assert_eq!(segs.len(), 1, "{what}");
            let seg = segs[0];
            assert_eq!(seg.name, "function", "{what}");
            assert_eq!(seg.address, 0x9000, "{what}");
            assert_eq!(seg.file_offset, FUNCTION_OFFSET, "{what}");
            assert_eq!(seg.length, FUNCTION_LEN, "{what}");
            // ⚠ The read and write tables describe the SAME block with
            // different opcodes. Reading with the write segment would put a
            // write command on the wire with nothing behind it, on a platform
            // where a desynchronised write stream has permanently degraded a
            // radio's transmit.
            assert_eq!(seg.command, cmd, "{what} segment carries the wrong opcode");
        }
    }

    /// And each transport refuses a segment from the other table outright, so a
    /// future caller reaching for the wrong constant gets an error instead of
    /// the wire. Checked before any I/O, which is why it needs no port.
    #[test]
    fn a_transport_refuses_a_segment_from_the_other_table() {
        let err = super::super::check_commands(&SETTINGS_SEGMENTS, &[0x52, 0x54], "read")
            .expect_err("reading with the write segment must be refused");
        assert!(err.contains("not a read command"), "{err}");

        let err = super::super::check_commands(&SETTINGS_READ_SEGMENTS, &[0x57], "write")
            .expect_err("writing with the read segment must be refused");
        assert!(err.contains("not a write command"), "{err}");

        // The pairings the driver actually uses are accepted.
        super::super::check_commands(&SETTINGS_READ_SEGMENTS, &[0x52, 0x54], "read").unwrap();
        super::super::check_commands(&SETTINGS_SEGMENTS, &[0x57], "write").unwrap();
    }

    /// ⚠ Every field must live inside the area the read-back compares.
    ///
    /// `verify` truncates to `FUNCTION_LIVE_LEN` (0x46) and then indexes
    /// `FIELDS` addresses into that slice. The highest address today is 0x44 —
    /// one byte of headroom — and the sheet still has rows owed. A field at 0x46
    /// or above would never be verified AND would panic inside the
    /// mismatch-reporting loop, which only runs when a write failed to commit.
    #[test]
    fn every_field_lives_inside_the_verified_area() {
        for f in &FIELDS {
            assert!(
                f.addr < FUNCTION_LIVE_LEN,
                "{} is at 0x{:02X}, outside the 0x{:02X} bytes `verify` compares",
                f.key,
                f.addr,
                FUNCTION_LIVE_LEN
            );
        }
    }

    /// Round-trip every field through the form's own representation.
    #[test]
    fn every_field_round_trips() {
        let mut image = vec![0u8; super::super::IMAGE_LEN];
        for f in &FIELDS {
            let probes: Vec<Value> = match f.kind {
                Kind::Bool => vec![json!(false), json!(true)],
                Kind::Enum => f.options.iter().map(|o| json!(o)).collect(),
                Kind::Int { lo, hi } => (lo..=hi).map(|n| json!(n)).collect(),
            };
            for v in probes {
                let raw = encode_field(f, &v).unwrap();
                image[FUNCTION_OFFSET + f.addr] = raw;
                let back = decode_field(f, &image[FUNCTION_OFFSET..FUNCTION_OFFSET + FUNCTION_LEN]);
                assert_eq!(back, v, "{} did not round-trip through byte {raw}", f.key);
            }
        }
    }

    /// ⚠ The radio stores whatever it is handed — 127 into a field whose
    /// maximum is 1 — so a value the form should never produce has to be
    /// refused HERE. Nothing downstream will catch it.
    #[test]
    fn out_of_range_is_refused_not_clamped() {
        for f in &FIELDS {
            let bad = match f.kind {
                Kind::Bool => json!("yes"),
                Kind::Enum => json!(42),
                Kind::Int { hi, .. } => json!(i64::from(hi) + 1),
            };
            match encode_field(f, &bad) {
                Err(Reject::Invalid(e)) => assert!(e.contains(f.key), "{}: {e}", f.key),
                other => panic!("{} accepted {bad}: {other:?}", f.key),
            }
        }
    }

    /// A select label this app cannot name must SKIP, not fail. That is how a
    /// value some other tool put on the radio survives being read into a
    /// profile and written back — the byte keeps whatever the radio had.
    #[test]
    fn an_unnameable_select_leaves_the_radio_alone() {
        let f = FIELDS
            .iter()
            .find(|f| f.kind == Kind::Enum)
            .expect("a settled enum field");
        assert!(matches!(
            encode_field(f, &json!("something this app has never heard of")),
            Err(Reject::Unnameable(_))
        ));

        let mut image = vec![0x77u8; super::super::IMAGE_LEN];
        let (written, notes) =
            apply_profile_settings(&mut image, &json!({f.key: "something else"})).unwrap();
        assert_eq!(written, 0);
        assert_eq!(notes.len(), 1);
        assert_eq!(image[FUNCTION_OFFSET + f.addr], 0x77, "the byte was touched");
    }

    /// An index the radio holds that this app has no label for must survive
    /// being decoded, or the round trip above has nothing to carry.
    #[test]
    fn an_unmapped_stored_value_decodes_to_something_writable_back() {
        let f = FIELDS.iter().find(|f| f.kind == Kind::Enum).unwrap();
        let mut image = vec![0u8; super::super::IMAGE_LEN];
        image[FUNCTION_OFFSET + f.addr] = 200;
        let decoded = decode_settings(&image);
        assert!(matches!(
            encode_field(f, &decoded[f.key]),
            Err(Reject::Unnameable(_))
        ));
    }

    /// The two "Level 1-9" fields two bytes apart do NOT share a convention:
    /// SQL stores the level, VOX Level stores the level minus one. Both were
    /// read off the radio's screen; a generator that assumed one rule for both
    /// would ship one of them silently off by one.
    #[test]
    fn the_two_level_fields_keep_their_different_conventions() {
        let sql = FIELDS.iter().find(|f| f.key == "squelch").unwrap();
        let vox = FIELDS.iter().find(|f| f.key == "vox-level").unwrap();
        assert_eq!(encode_field(sql, &json!(9)).unwrap(), 9);
        assert_eq!(encode_field(vox, &json!(7)).unwrap(), 6);
    }

    /// Clearing one field in the form must not stop the others reaching the
    /// radio. The form stores an empty number box as `""`, and rejecting that
    /// as "expected a number" used to fail the entire write.
    #[test]
    fn a_cleared_field_is_skipped_not_fatal() {
        let mut image = vec![0u8; super::super::IMAGE_LEN];
        let (n, notes) = apply_profile_settings(
            &mut image,
            &json!({"squelch": "", "vox-level": 5, "power-on-display": null}),
        )
        .expect("a cleared field must not fail the write");
        assert_eq!(n, 1, "vox-level should still have been written");
        assert!(notes.is_empty());
        assert_eq!(image[FUNCTION_OFFSET + 0x02], 4, "vox-level 5 stores as 4");
    }

    /// A key the profile does not carry must come back off the radio untouched.
    #[test]
    fn unknown_keys_are_left_alone() {
        let mut image = vec![0x5Au8; super::super::IMAGE_LEN];
        let (n, notes) = apply_profile_settings(&mut image, &json!({"squelch": 4})).unwrap();
        assert_eq!(n, 1);
        assert!(notes.is_empty());
        assert_eq!(image[FUNCTION_OFFSET], 4);
        // Every other byte of the block is as it was read.
        for i in 1..FUNCTION_LEN {
            assert_eq!(image[FUNCTION_OFFSET + i], 0x5A, "byte 0x{i:02X} was touched");
        }
    }
}
