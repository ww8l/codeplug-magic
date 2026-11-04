//! FT-857D / FT-897 settings: decoded out of, and patched into, the clone image.
//!
//! There is no narrower transport — the clone always carries the whole image —
//! so a settings write sends every byte back. What keeps it narrow is the
//! patch: only the bits of the fields the profile names change, everything else
//! (memories included) goes back exactly as it was read, and the read-back
//! compares the whole image.
//!
//! The field set is short on purpose: [`super::ft857_settings_table`] carries
//! only fields measured on the radio or anchored by the factory-default
//! cross-check with a settled encoding. See its header for what is withheld and
//! why.
//!
//! A key the profile does not carry, or carries blank, is left exactly as the
//! radio has it — a profile is a patch, not a replace.

use std::path::Path;

use serde_json::{json, Value};

use super::ft857_settings_table::{At, Charset, Kind, FIELDS, SF};
use super::{read_from_radio, write_to_radio};
use crate::radios::driver::{
    with_restore_hint, SettingsCapture, SettingsReader, SettingsWriteReport, SettingsWriter,
};

/// The radio's alphabets, as CHIRP `ft857.py` defines them. The byte is the
/// index; these are not ASCII.
fn alphabet(set: Charset) -> Vec<char> {
    let mut v: Vec<char> = ('0'..='9').chain('A'..='Z').collect();
    v.extend([' ', '/']);
    if set == Charset::Beacon {
        v.extend(['+', '.']);
    }
    v
}

fn bits(image: &[u8], byte: usize, shift: u8, width: u8) -> u16 {
    u16::from(image[byte] >> shift) & ((1u16 << width) - 1)
}

fn set_bits(image: &mut [u8], byte: usize, shift: u8, width: u8, value: u16) {
    let mask = (((1u16 << width) - 1) as u8) << shift;
    image[byte] = (image[byte] & !mask) | (((value as u8) << shift) & mask);
}

/// The field's value as the form carries it: a number, an option label, or text.
/// A stored value outside what the field can show reads as `null` (or a named
/// unknown, for an enum) rather than failing the whole read.
fn decode_field(f: &SF, image: &[u8]) -> Value {
    match (f.at, f.kind) {
        (At::Text { off, len, set }, _) => {
            let abc = alphabet(set);
            let text: Option<String> =
                image[off..off + len].iter().map(|&b| abc.get(b as usize).copied()).collect();
            match text {
                Some(t) => json!(t.trim_end()),
                None => Value::Null,
            }
        }
        (At::Bits { byte, shift, width }, Kind::Int { lo, hi, bias }) => {
            let shown = i32::from(bits(image, byte, shift, width)) - i32::from(bias);
            if (i32::from(lo)..=i32::from(hi)).contains(&shown) {
                json!(shown)
            } else {
                Value::Null
            }
        }
        (At::Bits { byte, shift, width }, Kind::Bool) => json!(bits(image, byte, shift, width) != 0),
        (At::Bits { byte, shift, width }, _) => {
            let v = bits(image, byte, shift, width) as usize;
            match f.options.get(v) {
                Some(label) => json!(label),
                None => json!(format!("(unknown value {v})")),
            }
        }
    }
}

/// Why a value could not be written.
#[derive(Debug)]
enum Reject {
    /// A select label this app cannot name — how a value read off a radio
    /// survives a round trip. Left as the radio has it, with a note.
    Unnameable(String),
    /// Wrong shape or out of range: refused, never clamped.
    Invalid(String),
}

fn encode_field(f: &SF, v: &Value, image: &mut [u8]) -> Result<(), Reject> {
    match (f.at, f.kind) {
        (At::Text { off, len, set }, _) => {
            let s = v
                .as_str()
                .ok_or_else(|| Reject::Invalid(format!("{}: expected text, got {v}", f.key)))?
                .to_ascii_uppercase();
            if s.chars().count() > len {
                return Err(Reject::Invalid(format!(
                    "{} ({}): at most {len} characters",
                    f.label, f.menu
                )));
            }
            let abc = alphabet(set);
            let mut cells = vec![abc.iter().position(|&c| c == ' ').unwrap() as u8; len];
            for (i, ch) in s.chars().enumerate() {
                cells[i] = abc.iter().position(|&c| c == ch).ok_or_else(|| {
                    Reject::Invalid(format!(
                        "{} ({}): the radio cannot store '{ch}' — it has {}",
                        f.label,
                        f.menu,
                        if set == Charset::Beacon { "0-9, A-Z, space, / + ." } else { "0-9, A-Z, space and /" }
                    ))
                })? as u8;
            }
            image[off..off + len].copy_from_slice(&cells);
            Ok(())
        }
        (At::Bits { byte, shift, width }, Kind::Int { lo, hi, bias }) => {
            let n = v
                .as_i64()
                .ok_or_else(|| Reject::Invalid(format!("{}: expected a number, got {v}", f.key)))?;
            if !(i64::from(lo)..=i64::from(hi)).contains(&n) {
                return Err(Reject::Invalid(format!("{}: {n} is outside {lo}..={hi}", f.key)));
            }
            set_bits(image, byte, shift, width, (n + i64::from(bias)) as u16);
            Ok(())
        }
        (At::Bits { byte, shift, width }, Kind::Bool) => {
            let b = v
                .as_bool()
                .ok_or_else(|| Reject::Invalid(format!("{}: expected true or false, got {v}", f.key)))?;
            set_bits(image, byte, shift, width, u16::from(b));
            Ok(())
        }
        (At::Bits { byte, shift, width }, _) => {
            let s = v.as_str().ok_or_else(|| {
                Reject::Invalid(format!("{}: expected one of its options, got {v}", f.key))
            })?;
            let i = f.options.iter().position(|o| *o == s).ok_or_else(|| {
                Reject::Unnameable(format!(
                    "{} ({}) holds {s:?}, which this app cannot name; left as the radio had it",
                    f.label, f.menu
                ))
            })?;
            set_bits(image, byte, shift, width, i as u16);
            Ok(())
        }
    }
}

/// Every emitted field, decoded out of a clone image.
pub(crate) fn decode_settings(image: &[u8]) -> Value {
    Value::Object(FIELDS.iter().map(|f| (f.key.to_string(), decode_field(f, image))).collect())
}

/// Patch the profile's settings into `image`. Returns how many fields were
/// written and a note for each deliberately left alone. Named as the cable
/// radios' encoder is (see the BT-9000's): the caller is `write_settings`
/// itself, not an export path.
pub(crate) fn apply_profile_settings(
    image: &mut [u8],
    settings: &Value,
) -> Result<(usize, Vec<String>), String> {
    let obj = settings.as_object().ok_or("profile settings are not a JSON object")?;
    let (mut written, mut notes) = (0, Vec::new());
    for f in &FIELDS {
        let Some(v) = obj.get(f.key) else { continue };
        // Blank means "leave it alone": a cleared number box arrives as "".
        // Text is the exception — an empty beacon text is a real value.
        if v.is_null() || (v.as_str() == Some("") && f.kind != Kind::Text) {
            continue;
        }
        match encode_field(f, v, image) {
            Ok(()) => written += 1,
            Err(Reject::Unnameable(n)) => notes.push(n),
            Err(Reject::Invalid(e)) => return Err(e),
        }
    }
    Ok((written, notes))
}

impl SettingsReader for super::YaesuFt857d {
    /// A clone read ([C] SEND); the image is the session's backup.
    fn read_settings(&self, port: &str, _schema_json: &str) -> Result<SettingsCapture, String> {
        let image = read_from_radio(port, "Read settings")?;
        Ok(SettingsCapture { settings: decode_settings(&image), backup: image, backup_ext: "img" })
    }
}

impl SettingsWriter for super::YaesuFt857d {
    /// Read and back up (SEND), patch, write (RCV + Continue), read back (SEND).
    /// A profile that changes nothing on this radio skips the write and its
    /// front-panel step altogether.
    fn write_settings(
        &self,
        port: &str,
        settings: &Value,
        schema_json: &str,
        backup_dir: &Path,
    ) -> Result<SettingsWriteReport, String> {
        let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
        let backup_path = backup_dir.join(format!("ft857d-presettings-{stamp}.img"));

        let base = read_from_radio(port, "Writing settings, step 1 of 3 — first, a backup of the radio")?;
        std::fs::write(&backup_path, &base)
            .map_err(|e| format!("could not write backup {}: {e}", backup_path.display()))?;

        let mut settings = settings.clone();
        let mut notes =
            crate::radios::settings_bounds::strip_out_of_range(schema_json, &mut settings);
        let mut image = base.clone();
        let (fields_written, skipped) = apply_profile_settings(&mut image, &settings)?;
        notes.extend(skipped);

        if image == base {
            notes.push("The radio already holds every value in the profile; nothing was written.".into());
            return Ok(SettingsWriteReport {
                fields_written,
                verified: Some(true),
                note: Some(notes.join(" ")),
                backup_path: backup_path.to_string_lossy().to_string(),
                expected_path: None,
                windows_written: Vec::new(),
            });
        }

        write_to_radio(port, &image, "Writing settings, step 2 of 3 — the write").map_err(|e| {
            with_restore_hint(
                e,
                &backup_path,
                "Keep that file — it is the radio as it was read, and \"Restore backup…\" \
                 sends it back over the same cable.",
            )
        })?;

        let verified = match read_from_radio(port, "Writing settings, step 3 of 3 — checking the write") {
            Ok(after) if after == image => true,
            Ok(after) => {
                let n = after.iter().zip(&image).filter(|(a, b)| a != b).count();
                notes.push(format!("The read-back differs from what was written in {n} bytes."));
                false
            }
            Err(e) => {
                notes.push(format!("Written, but the read-back check could not run ({e})."));
                false
            }
        };

        Ok(SettingsWriteReport {
            fields_written,
            verified: Some(verified),
            note: (!notes.is_empty()).then(|| notes.join(" ")),
            backup_path: backup_path.to_string_lossy().to_string(),
            expected_path: None,
            windows_written: Vec::new(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::radios::yaesu_ft857d::layout::IMAGE_LEN_US;

    const SCHEMA: &str = include_str!("../../ft857_settings_schema.json");

    /// A real FT-897 settings block, bytes 0..71 of the image read 2026-11-03
    /// with EXT MENU switched on — the image whose values were read off the
    /// radio's own screen — and its ARTS IDW text.
    fn real() -> Vec<u8> {
        let mut img = vec![0u8; IMAGE_LEN_US];
        let head: [u8; 71] = [
            0x1F, 0x06, 0x80, 0x00, 0xC0, 0x20, 0x13, 0x54, 0x87, 0x09, 0x40, 0x03, 0x65, 0x19,
            0x32, 0x48, 0xB2, 0x05, 0x00, 0x00, 0xB2, 0x32, 0x32, 0x32, 0x32, 0x32, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x1D, 0x00, 0x00, 0x00, 0x00, 0x00, 0x40, 0x32,
            0x00, 0xC5, 0x00, 0x78, 0x1F, 0x00, 0x32, 0x00, 0x00, 0x32, 0x32, 0xE4, 0x02, 0x85,
            0x86, 0x88, 0x85, 0x86, 0x87, 0x60, 0x00, 0x60, 0x00, 0x00, 0x20, 0x00, 0xE4, 0xB2,
            0x94,
        ];
        img[..71].copy_from_slice(&head);
        // ARTS IDW "YAESU" + 5 spaces, in the call-sign alphabet.
        let abc = alphabet(Charset::Callsign);
        for (i, ch) in "YAESU     ".chars().enumerate() {
            img[0x1BF3 + i] = abc.iter().position(|&c| c == ch).unwrap() as u8;
        }
        img
    }

    #[test]
    fn the_table_and_the_schema_describe_the_same_fields() {
        let schema: Vec<Value> = serde_json::from_str(SCHEMA).unwrap();
        let fields: Vec<&Value> = schema.iter().filter(|f| f["type"] != "section").collect();
        assert_eq!(fields.len(), FIELDS.len());
        for (s, f) in fields.iter().zip(FIELDS.iter()) {
            assert_eq!(s["key"], f.key);
            match f.kind {
                Kind::Bool => assert_eq!(s["type"], "boolean", "{}", f.key),
                Kind::Int { lo, hi, .. } => {
                    assert_eq!((s["type"].as_str(), s["min"].as_u64(), s["max"].as_u64()),
                               (Some("integer"), Some(lo.into()), Some(hi.into())), "{}", f.key);
                }
                Kind::Enum => {
                    let opts: Vec<&str> =
                        s["options"].as_array().unwrap().iter().map(|o| o.as_str().unwrap()).collect();
                    assert_eq!(opts, f.options, "{}", f.key);
                }
                Kind::Text => assert_eq!(s["type"], "text", "{}", f.key),
            }
        }
    }

    #[test]
    fn the_ft897s_own_values_decode_to_what_its_screen_showed() {
        let s = decode_settings(&real());
        // Read off the radio's own screen, 2026-11-03.
        assert_eq!(s["ext-menu"], true, "001: switched ON, one-bit differential");
        assert_eq!(s["clar-dial-sel"], "CLAR", "021");
        assert_eq!(s["cw-auto-mode"], false, "022");
        assert_eq!(s["cw-qsk"], "10 ms", "028 — CHIRP's list would say 5 ms");
        assert_eq!(s["cw-sidetone"], 50, "029");
        assert_eq!(s["cw-speed"], "12 wpm", "030");
        assert_eq!(s["disp-color"], "FIX", "041");
        assert_eq!(s["disp-color-fix"], 1, "041 FIX:1 at raw 0");
        assert_eq!(s["disp-contrast"], 5, "042 at raw 7 — CHIRP's +2 would say 9");
        assert_eq!(s["beep-vol"], 50, "014");
        // The factory-default cross-check (the radio had been reset).
        assert_eq!(s["ssb-mic-gain"], 50);
        assert_eq!(s["dsp-nr-level"], 8, "raw 7");
        assert_eq!(s["dsp-lpf-cutoff"], "6000 Hz");
        assert_eq!(s["dig-mode"], "RTTY-L");
        assert_eq!(s["cat-rate"], "4800");
        assert_eq!(s["cw-pitch"], "700 Hz");
        assert_eq!(s["cw-weight"], "1:3.0");
        assert_eq!(s["disp-mode"], "AUTO2");
        assert_eq!(
            [&s["pg-a"], &s["pg-b"], &s["pg-c"], &s["pg-acc"], &s["pg-p1"], &s["pg-p2"]],
            ["MONI", "Q.SPL", "ATC", "MONI", "Q.SPL", "TCALL"]
        );
        assert_eq!(
            [&s["rf-power-hf"], &s["rf-power-6m"], &s["rf-power-vhf"], &s["rf-power-uhf"]],
            [100, 100, 50, 20]
        );
        assert_eq!(s["arts-idw"], "YAESU");
    }

    #[test]
    fn reading_then_writing_back_changes_nothing() {
        let img = real();
        let mut out = img.clone();
        let (n, notes) = apply_profile_settings(&mut out, &decode_settings(&img)).unwrap();
        assert_eq!(n, FIELDS.len());
        assert!(notes.is_empty(), "{notes:?}");
        assert_eq!(out, img);
    }

    #[test]
    fn a_change_moves_only_its_own_bits() {
        let img = real();
        let mut out = img.clone();
        apply_profile_settings(
            &mut out,
            &json!({"beep-vol": 37, "cat-rate": "38400", "arts-idw": "ww8l/p", "vox-gain": ""}),
        )
        .unwrap();
        let changed: Vec<usize> = (0..img.len()).filter(|&i| img[i] != out[i]).collect();
        // BEEP VOL 50 -> 37 (0x32 -> 0x25), top bit kept.
        assert_eq!(out[0x29], 0x25);
        // CAT RATE: bits 7-6 of byte 17 to 2; VOX DELAY (low 5 bits) untouched.
        assert_eq!(out[17], (2 << 6) | (img[17] & 0x3F));
        assert_eq!(decode_settings(&out)["arts-idw"], "WW8L/P");
        assert!(changed.iter().all(|&i| i == 0x29 || i == 17 || (0x1BF3..0x1BFD).contains(&i)));
    }

    #[test]
    fn refusals() {
        let mut img = real();
        let e = apply_profile_settings(&mut img, &json!({"rf-power-uhf": 50})).unwrap_err();
        assert!(e.contains("2..=20"), "{e}");
        let e = apply_profile_settings(&mut img, &json!({"arts-idw": "W8@B"})).unwrap_err();
        assert!(e.contains("'@'"), "{e}");
        let e = apply_profile_settings(&mut img, &json!({"beacon-text-1": "x".repeat(41)})).unwrap_err();
        assert!(e.contains("40 characters"), "{e}");
        let (_, notes) = apply_profile_settings(&mut img, &json!({"cw-pitch": "750 Hz"})).unwrap();
        assert_eq!(notes.len(), 1);
    }
}
