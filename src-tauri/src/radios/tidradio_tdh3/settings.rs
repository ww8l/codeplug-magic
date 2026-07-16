//! TIDRADIO TD-H3 non-channel settings (the `SettingsProgrammer` half of the
//! driver). Moved here from `commands/tdh3.rs` in Chunk 3.4.
//!
//! Buffer offsets (image includes the 8-byte ident prefix), taken from CHIRP
//! `tdh8.py` `MEM_FORMAT_H3`'s `settings` struct `#seekto 0x0CA0` and
//! `poweron_msg` `#seekto 0x1c08`. Hardware-verified against real backup images.
//! CHIRP bitfields are MSB-first within each u8 (the first-declared field takes
//! the high bits), so e.g. `scanmode:2` at the top of its byte sits at bit
//! positions 7-6 (lsb 6, width 2).

use std::collections::HashMap;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use serialport::SerialPort;

use crate::error::MapErrString;
use crate::radios::driver::SettingsProgrammer;

use super::{do_ident, download, open_port, reident, upload, TidradioTdh3};

const SETTINGS_BASE: usize = 0x0CA0;
const POWERON_MSG_BASE: usize = 0x1C08;
const MSG_LEN: usize = 16;

// ============================================================
// SettingsProgrammer (schema/JSON path)
// ============================================================

impl SettingsProgrammer for TidradioTdh3 {
    fn read_settings(&self, port: &str, schema_json: &str) -> Result<Value, String> {
        let mut p = open_port(port)?;
        let ident = do_ident(&mut *p)?;
        let image = download(&mut *p, &ident)?;
        decode_profile_settings(&image, schema_json)
    }

    fn write_settings(
        &self,
        port: &str,
        settings: &Value,
        schema_json: &str,
    ) -> Result<(), String> {
        let settings_json = serde_json::to_string(settings).estr()?;
        let mut p = open_port(port)?;

        // Download the current image, patch only the profile's settings bits, and
        // write the whole main range back (channels + unsupported settings kept).
        let ident = do_ident(&mut *p)?;
        let mut image = download(&mut *p, &ident)?;
        apply_profile_settings(&mut image, schema_json, &settings_json)?;

        std::thread::sleep(Duration::from_secs(1));
        reident(&mut *p)?;
        upload(&mut *p, &image)?;
        Ok(())
    }
}

/// The radio-global "options" Codeplug Magic exposes for editing — the common-settings
/// subset (the DTMF group and the six side/top-key assignments are intentionally
/// left out for now). Multi-value fields are carried as a zero-based index into
/// the matching label list on the frontend (see `Tdh3SETTINGS_*` lists there);
/// switches are plain booleans. These are NOT tied to a codeplug: they are read
/// from and written straight back to the connected radio.
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
pub struct Tdh3Settings {
    /// Squelch level 0..=9.
    pub squelch: u8,
    /// Display brightness as a 0..=4 index (label "1".."5"). Stored inverted on
    /// the radio (stored = 4 - index), handled in decode/encode.
    pub brightness: u8,
    /// Backlight time-out `ligcon`, index 0..=4 (CONT/5s/10s/15s/30s).
    pub backlight: u8,
    /// VOX gain `voxgain`, index 0..=5 (Off/1..5).
    pub vox_gain: u8,
    /// VOX delay `voxdelay`, index 0..=2 (1.05s/2.0s/3.0s).
    pub vox_delay: u8,
    /// Transmit time-out timer `tot`, index 0..=7 (Off/30S..210S).
    pub tot: u8,
    /// Scan resume mode `scanmode`, index 0..=2 (TO/CO/SE).
    pub scan_mode: u8,
    /// Power-on display `ponmsg`, index 0..=2 (Off/Msg/Icon).
    pub power_on_msg_mode: u8,
    /// Battery save `save`, index 0..=5 (Off/1:1/1:2/1:3/1:4/1:8).
    pub power_save: u8,
    /// Breathing LED `breathled`, index 0..=4 (Off/5S/10S/15S/30S).
    pub breath_led: u8,
    /// Display A mode `mdfa`, index 0..=1 (Frequency/Name).
    pub display_a: u8,
    /// Display B mode `mdfb`, index 0..=1 (Frequency/Name).
    pub display_b: u8,
    /// Alarm mode `alarm`, index 0..=1 (On site/Alarm).
    pub alarm_mode: u8,
    /// Key beep `btnvoice`.
    pub beep: bool,
    /// Voice prompt `voiceprompt`.
    pub voice_prompt: bool,
    /// Keypad auto-lock `keyautolock`.
    pub auto_lock: bool,
    /// Dual watch `dbrx`.
    pub dual_watch: bool,
    /// Enable TX on the 220 MHz band `tx220`.
    pub tx_220: bool,
    /// Enable TX on the 350 MHz band `tx350`.
    pub tx_350: bool,
    /// Enable TX on the 500 MHz band `tx500`.
    pub tx_500: bool,
    /// AM band receive `amband`.
    pub am_band: bool,
    /// Three lines of power-on message text (16 chars each).
    pub power_on_msg1: String,
    pub power_on_msg2: String,
    pub power_on_msg3: String,
}

/// Extract `width` bits at LSB position `lsb` from `byte`.
fn bits(byte: u8, lsb: u8, width: u8) -> u8 {
    let mask = ((1u16 << width) - 1) as u8;
    (byte >> lsb) & mask
}

/// Replace `width` bits at LSB position `lsb` in `byte` with `val`'s low bits.
fn set_bits(byte: &mut u8, lsb: u8, width: u8, val: u8) {
    let field = ((1u16 << width) - 1) as u8;
    let mask = field << lsb;
    *byte = (*byte & !mask) | ((val & field) << lsb);
}

/// Decode the editable settings from a full radio image. Byte indices are
/// relative to `SETTINGS_BASE`; see the struct field docs for each meaning.
pub(crate) fn decode_settings(image: &[u8]) -> Tdh3Settings {
    let s = &image[SETTINGS_BASE..];
    let bit = |i: usize, b: u8| (s[i] >> b) & 1 == 1;
    Tdh3Settings {
        squelch: s[17],
        brightness: 4u8.saturating_sub(s[5]),
        backlight: s[21],
        vox_gain: bits(s[15], 0, 3),
        vox_delay: s[22],
        tot: s[18],
        scan_mode: bits(s[9], 6, 2),
        power_on_msg_mode: bits(s[11], 6, 2),
        power_save: s[20],
        breath_led: bits(s[23], 4, 3),
        display_a: bits(s[10], 2, 1),
        display_b: bits(s[11], 4, 1),
        alarm_mode: bits(s[23], 0, 1),
        beep: bit(9, 2),
        voice_prompt: bit(9, 0),
        auto_lock: bit(9, 4),
        dual_watch: bit(11, 2),
        tx_220: bit(19, 4),
        tx_350: bit(19, 3),
        tx_500: bit(19, 2),
        am_band: bit(23, 1),
        power_on_msg1: decode_msg(image, 0),
        power_on_msg2: decode_msg(image, 1),
        power_on_msg3: decode_msg(image, 2),
    }
}

/// Patch the editable settings into a downloaded image in place, touching only
/// the specific bits/bytes each field owns (every other byte — including the
/// unknown/reserved bits packed alongside — is preserved exactly as read).
pub(crate) fn encode_settings(image: &mut [u8], st: &Tdh3Settings) {
    let b = SETTINGS_BASE;
    image[b + 17] = st.squelch.min(9);
    image[b + 5] = 4u8.saturating_sub(st.brightness.min(4));
    image[b + 21] = st.backlight.min(4);
    set_bits(&mut image[b + 15], 0, 3, st.vox_gain.min(5));
    image[b + 22] = st.vox_delay.min(2);
    image[b + 18] = st.tot.min(7);
    set_bits(&mut image[b + 9], 6, 2, st.scan_mode.min(2));
    set_bits(&mut image[b + 11], 6, 2, st.power_on_msg_mode.min(2));
    image[b + 20] = st.power_save.min(5);
    set_bits(&mut image[b + 23], 4, 3, st.breath_led.min(4));
    set_bits(&mut image[b + 10], 2, 1, st.display_a & 1);
    set_bits(&mut image[b + 11], 4, 1, st.display_b & 1);
    set_bits(&mut image[b + 23], 0, 1, st.alarm_mode & 1);
    set_bits(&mut image[b + 9], 2, 1, st.beep as u8);
    set_bits(&mut image[b + 9], 0, 1, st.voice_prompt as u8);
    set_bits(&mut image[b + 9], 4, 1, st.auto_lock as u8);
    set_bits(&mut image[b + 11], 2, 1, st.dual_watch as u8);
    set_bits(&mut image[b + 19], 4, 1, st.tx_220 as u8);
    set_bits(&mut image[b + 19], 3, 1, st.tx_350 as u8);
    set_bits(&mut image[b + 19], 2, 1, st.tx_500 as u8);
    set_bits(&mut image[b + 23], 1, 1, st.am_band as u8);
    encode_msg(image, 0, &st.power_on_msg1);
    encode_msg(image, 1, &st.power_on_msg2);
    encode_msg(image, 2, &st.power_on_msg3);
}

/// Read one 16-byte power-on message line (left-aligned, null/0xFF-padded).
/// Non-printable bytes render as a space so a junk byte stays visible.
fn decode_msg(image: &[u8], i: usize) -> String {
    let off = POWERON_MSG_BASE + i * MSG_LEN;
    if off + MSG_LEN > image.len() {
        return String::new();
    }
    image[off..off + MSG_LEN]
        .iter()
        .take_while(|&&b| b != 0x00 && b != 0xFF)
        .map(|&b| if (0x20..0x7F).contains(&b) { b as char } else { ' ' })
        .collect::<String>()
        .trim_end()
        .to_string()
}

/// Write one 16-byte power-on message line: left-aligned ASCII, null-padded,
/// non-ASCII (or out-of-range) chars folded to a space.
fn encode_msg(image: &mut [u8], i: usize, text: &str) {
    let off = POWERON_MSG_BASE + i * MSG_LEN;
    let mut buf = [0u8; MSG_LEN];
    for (j, ch) in text.chars().take(MSG_LEN).enumerate() {
        let c = ch as u32;
        buf[j] = if (0x20..0x7F).contains(&c) { c as u8 } else { b' ' };
    }
    image[off..off + MSG_LEN].copy_from_slice(&buf);
}

/// Re-read the radio and compare decoded settings to what we intended to write.
pub(crate) fn verify_settings_after_write(
    p: &mut dyn SerialPort,
    intended: &[u8],
) -> Result<(bool, Option<String>, Tdh3Settings), String> {
    std::thread::sleep(Duration::from_secs(1));
    let ident = reident(p)?;
    let readback = download(p, &ident)?;

    let expected = decode_settings(intended);
    let actual = decode_settings(&readback);
    if expected == actual {
        Ok((true, None, actual))
    } else {
        Ok((
            false,
            Some(
                "Read-back settings differ from what was sent. The pre-write backup is saved \
                 if you need to revert."
                    .to_string(),
            ),
            actual,
        ))
    }
}

// ============================================================
// Apply a saved radio profile's settings
// ============================================================
//
// The TD-H3 radio profile stores its `non_channel_settings` as JSON keyed by the
// CHIRP `RadioSetting` names in the model's settings schema (squelch, ligcon,
// btnvoice, ponmsg, brightness, tx220…). This mirrors the UV-5R
// `radios::baofeng_uv5r::settings::apply_profile_settings`: patch each KNOWN key's bits into a
// downloaded image (unknown/unsupported keys — the deferred DTMF + key groups —
// are skipped, so they're preserved as read), then upload. Only the settings
// region changes; channels are untouched.

/// Where one TD-H3 schema key lives, as an index relative to `SETTINGS_BASE`.
enum SLoc {
    /// Whole byte = the resolved index.
    Byte(usize),
    /// Whole byte, but stored INVERTED as `4 - index` (brightness only).
    BrightnessByte(usize),
    /// `width` bits at LSB `lsb` within the byte (others preserved).
    Bits(usize, u8, u8),
    /// One of the three 16-byte power-on message lines (0..=2).
    Msg(usize),
}

/// Resolve a TD-H3 schema key to its storage location, or `None` for keys we
/// don't write (the deferred DTMF group + side/top-key assignments).
fn tdh3_loc(key: &str) -> Option<SLoc> {
    Some(match key {
        "squelch" => SLoc::Byte(17),
        "brightness" => SLoc::BrightnessByte(5),
        "ligcon" => SLoc::Byte(21),
        "voxgain" => SLoc::Bits(15, 0, 3),
        "voxdelay" => SLoc::Byte(22),
        "tot" => SLoc::Byte(18),
        "scanmode" => SLoc::Bits(9, 6, 2),
        "ponmsg" => SLoc::Bits(11, 6, 2),
        "save" => SLoc::Byte(20),
        "breathled" => SLoc::Bits(23, 4, 3),
        "mdfa" => SLoc::Bits(10, 2, 1),
        "mdfb" => SLoc::Bits(11, 4, 1),
        "alarm" => SLoc::Bits(23, 0, 1),
        "btnvoice" => SLoc::Bits(9, 2, 1),
        "voiceprompt" => SLoc::Bits(9, 0, 1),
        "keyautolock" => SLoc::Bits(9, 4, 1),
        "dbrx" => SLoc::Bits(11, 2, 1),
        "tx220" => SLoc::Bits(19, 4, 1),
        "tx350" => SLoc::Bits(19, 3, 1),
        "tx500" => SLoc::Bits(19, 2, 1),
        "amband" => SLoc::Bits(23, 1, 1),
        "poweron_msg.msg1" => SLoc::Msg(0),
        "poweron_msg.msg2" => SLoc::Msg(1),
        "poweron_msg.msg3" => SLoc::Msg(2),
        _ => return None,
    })
}

/// Build a `key -> option labels` map from the model schema, so a stored select
/// label (e.g. "15s", "Name", "Icon") can be turned back into its byte index.
fn schema_options(schema_json: &str) -> Result<HashMap<String, Vec<String>>, String> {
    let schema: Value = serde_json::from_str(schema_json)
        .map_err(|e| format!("invalid settings schema JSON: {e}"))?;
    let arr = schema.as_array().ok_or("settings schema is not a JSON array")?;
    let mut map = HashMap::new();
    for field in arr {
        if let (Some(key), Some(opts)) = (
            field.get("key").and_then(Value::as_str),
            field.get("options").and_then(Value::as_array),
        ) {
            let labels = opts
                .iter()
                .filter_map(|o| o.as_str().map(str::to_string))
                .collect();
            map.insert(key.to_string(), labels);
        }
    }
    Ok(map)
}

/// Resolve a profile value to a byte index: booleans → 0/1, numbers as-is,
/// select labels → their option index.
fn resolve_index(key: &str, value: &Value, options: &HashMap<String, Vec<String>>) -> Option<i64> {
    match value {
        Value::Bool(b) => Some(*b as i64),
        Value::Number(n) => n.as_i64(),
        Value::String(s) => options
            .get(key)
            .and_then(|opts| opts.iter().position(|o| o == s))
            .map(|i| i as i64),
        _ => None,
    }
}

/// Patch a profile's `non_channel_settings` into a downloaded image. Returns the
/// number of fields applied (unknown keys and unresolvable values are skipped so
/// a partial/odd profile can't corrupt the write). Touches only each field's own
/// bits; every other byte stays exactly as read.
pub fn apply_profile_settings(
    image: &mut [u8],
    schema_json: &str,
    settings_json: &str,
) -> Result<usize, String> {
    let options = schema_options(schema_json)?;
    let settings: Value = serde_json::from_str(settings_json)
        .map_err(|e| format!("invalid profile settings JSON: {e}"))?;
    let obj = settings
        .as_object()
        .ok_or("profile settings is not a JSON object")?;

    let mut applied = 0usize;
    for (key, value) in obj {
        let Some(loc) = tdh3_loc(key) else { continue };
        let ok = match loc {
            SLoc::Byte(n) => match resolve_index(key, value, &options) {
                Some(v) if SETTINGS_BASE + n < image.len() => {
                    image[SETTINGS_BASE + n] = v as u8;
                    true
                }
                _ => false,
            },
            SLoc::BrightnessByte(n) => match resolve_index(key, value, &options) {
                Some(v) if SETTINGS_BASE + n < image.len() => {
                    image[SETTINGS_BASE + n] = 4u8.saturating_sub((v as u8).min(4));
                    true
                }
                _ => false,
            },
            SLoc::Bits(n, lsb, w) => match resolve_index(key, value, &options) {
                Some(v) if SETTINGS_BASE + n < image.len() => {
                    set_bits(&mut image[SETTINGS_BASE + n], lsb, w, v as u8);
                    true
                }
                _ => false,
            },
            SLoc::Msg(i) => match value.as_str() {
                Some(s) if POWERON_MSG_BASE + i * MSG_LEN + MSG_LEN <= image.len() => {
                    encode_msg(image, i, s);
                    true
                }
                _ => false,
            },
        };
        if ok {
            applied += 1;
        }
    }
    Ok(applied)
}

/// Read the numeric byte index a non-`Msg` location holds.
fn read_index(image: &[u8], loc: &SLoc) -> i64 {
    match loc {
        SLoc::Byte(n) => *image.get(SETTINGS_BASE + n).unwrap_or(&0) as i64,
        SLoc::BrightnessByte(n) => {
            4i64 - (*image.get(SETTINGS_BASE + n).unwrap_or(&0)).min(4) as i64
        }
        SLoc::Bits(n, lsb, w) => bits(*image.get(SETTINGS_BASE + n).unwrap_or(&0), *lsb, *w) as i64,
        SLoc::Msg(_) => 0,
    }
}

/// Decode an image's settings into the schema-keyed JSON a profile stores
/// (booleans, select labels, integers, text) — the inverse of
/// [`apply_profile_settings`]. Only the keys we know how to locate are emitted.
pub fn decode_profile_settings(image: &[u8], schema_json: &str) -> Result<Value, String> {
    let schema: Value = serde_json::from_str(schema_json)
        .map_err(|e| format!("invalid settings schema JSON: {e}"))?;
    let arr = schema.as_array().ok_or("settings schema is not a JSON array")?;
    let mut map = serde_json::Map::new();
    for field in arr {
        let Some(key) = field.get("key").and_then(Value::as_str) else {
            continue;
        };
        let Some(loc) = tdh3_loc(key) else { continue };
        let ftype = field.get("type").and_then(Value::as_str).unwrap_or("text");
        let value = match loc {
            SLoc::Msg(i) => Value::String(decode_msg(image, i)),
            ref other => {
                let n = read_index(image, other);
                match ftype {
                    "boolean" => Value::Bool(n != 0),
                    "select" => {
                        let label = field
                            .get("options")
                            .and_then(Value::as_array)
                            .and_then(|opts| opts.get(n as usize))
                            .and_then(Value::as_str);
                        match label {
                            Some(s) => Value::String(s.to_string()),
                            None => continue, // out-of-range index; skip
                        }
                    }
                    _ => Value::Number(n.into()),
                }
            }
        };
        map.insert(key.to_string(), value);
    }
    Ok(Value::Object(map))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_settings() -> Tdh3Settings {
        Tdh3Settings {
            squelch: 3,
            brightness: 2, // index 2 → stored 4 - 2 = 2
            backlight: 1,
            vox_gain: 4,
            vox_delay: 1,
            tot: 5,
            scan_mode: 2,
            power_on_msg_mode: 1,
            power_save: 3,
            breath_led: 4,
            display_a: 1,
            display_b: 0,
            alarm_mode: 1,
            beep: true,
            voice_prompt: false,
            auto_lock: true,
            dual_watch: true,
            tx_220: true,
            tx_350: false,
            tx_500: true,
            am_band: true,
            power_on_msg1: "WW8L".into(),
            power_on_msg2: "TIM".into(),
            power_on_msg3: "73".into(),
        }
    }

    #[test]
    fn settings_round_trip_through_an_image() {
        // Start from a realistic image (0xFF fill) and confirm encode → decode
        // returns exactly what we put in for every field.
        let mut image = vec![0xFFu8; 0x2008];
        let st = sample_settings();
        encode_settings(&mut image, &st);
        assert_eq!(decode_settings(&image), st);
    }

    #[test]
    fn settings_encode_preserves_neighbouring_bits() {
        // The reserved/unknown bits packed alongside our fields must survive a
        // write untouched. Fill with 0xFF, encode all-zero-ish settings, and
        // check a byte we only partially own keeps its foreign bits.
        let mut image = vec![0xFFu8; 0x2008];
        let st = Tdh3Settings {
            scan_mode: 0,
            power_on_msg_mode: 0,
            beep: false,
            voice_prompt: false,
            auto_lock: false,
            dual_watch: false,
            display_b: 0,
            ..sample_settings()
        };
        encode_settings(&mut image, &st);
        // Byte 9 holds scanmode(7-6), keyautolock(4), btnvoice(2), voiceprompt(0)
        // plus unused16(5)/unused17(3)/unknown18(1) which started as 1s and must
        // remain set: 0b0010_1010 = 0x2A.
        assert_eq!(image[SETTINGS_BASE + 9], 0x2A);
    }

    #[test]
    fn brightness_inverts_between_stored_and_index() {
        let mut image = vec![0xFFu8; 0x2008];
        let mut st = sample_settings();
        st.brightness = 0; // brightest display → stored 4
        encode_settings(&mut image, &st);
        assert_eq!(image[SETTINGS_BASE + 5], 4);
        assert_eq!(decode_settings(&image).brightness, 0);
    }

    #[test]
    fn power_on_message_is_left_aligned_null_padded() {
        let mut image = vec![0xFFu8; 0x2008];
        let mut st = sample_settings();
        st.power_on_msg1 = "WW8L".into();
        encode_settings(&mut image, &st);
        assert_eq!(&image[POWERON_MSG_BASE..POWERON_MSG_BASE + 4], b"WW8L");
        assert_eq!(image[POWERON_MSG_BASE + 4], 0x00);
        assert_eq!(decode_settings(&image).power_on_msg1, "WW8L");
    }

    #[test]
    fn profile_settings_apply_by_schema_key() {
        // The TD-H3 schema keys (CHIRP names) with select labels + booleans +
        // text, applied to an image, must land at the right bits — and decode
        // back to the expected struct values (proves the schema→bits mapping).
        let schema = r#"[
            {"key":"squelch","type":"select","options":["0","1","2","3","4","5","6","7","8","9"]},
            {"key":"brightness","type":"select","options":["1","2","3","4","5"]},
            {"key":"ligcon","type":"select","options":["CONT","5s","10s","15s","30s"]},
            {"key":"voxgain","type":"select","options":["Off","1","2","3","4","5"]},
            {"key":"ponmsg","type":"select","options":["Off","Msg","Icon"]},
            {"key":"mdfa","type":"select","options":["Frequency","Name"]},
            {"key":"alarm","type":"select","options":["On site","Alarm"]},
            {"key":"btnvoice","type":"boolean"},
            {"key":"tx220","type":"boolean"},
            {"key":"poweron_msg.msg1","type":"text"},
            {"key":"ssidekey1","type":"select","options":["None","FM Radio"]}
        ]"#;
        let settings = r#"{
            "squelch":"7","brightness":"5","ligcon":"15s","voxgain":"3",
            "ponmsg":"Icon","mdfa":"Name","alarm":"Alarm","btnvoice":true,
            "tx220":true,"poweron_msg.msg1":"WW8L","ssidekey1":"FM Radio"
        }"#;
        let mut image = vec![0xFFu8; 0x2008];
        let applied = apply_profile_settings(&mut image, schema, settings).unwrap();
        // 10 known keys applied; the deferred key-assignment (ssidekey1) skipped.
        assert_eq!(applied, 10);

        let d = decode_settings(&image);
        assert_eq!(d.squelch, 7);
        assert_eq!(d.brightness, 4); // label "5" → index 4 (stored inverted as 0)
        assert_eq!(image[SETTINGS_BASE + 5], 0);
        assert_eq!(d.backlight, 3); // "15s" → index 3
        assert_eq!(d.vox_gain, 3);
        assert_eq!(d.power_on_msg_mode, 2); // "Icon"
        assert_eq!(d.display_a, 1); // "Name"
        assert_eq!(d.alarm_mode, 1); // "Alarm"
        assert!(d.beep);
        assert!(d.tx_220);
        assert_eq!(d.power_on_msg1, "WW8L");
    }

    #[test]
    fn decode_profile_settings_round_trips_apply() {
        // Apply a schema-keyed profile to an image, then decode it back out: the
        // emitted JSON must match the input for every supported key (the
        // radio→profile import is the faithful inverse of profile→radio apply).
        let schema = r#"[
            {"key":"squelch","type":"select","options":["0","1","2","3","4","5","6","7","8","9"]},
            {"key":"brightness","type":"select","options":["1","2","3","4","5"]},
            {"key":"ligcon","type":"select","options":["CONT","5s","10s","15s","30s"]},
            {"key":"ponmsg","type":"select","options":["Off","Msg","Icon"]},
            {"key":"mdfa","type":"select","options":["Frequency","Name"]},
            {"key":"btnvoice","type":"boolean"},
            {"key":"tx220","type":"boolean"},
            {"key":"poweron_msg.msg1","type":"text"},
            {"key":"ssidekey1","type":"select","options":["None","FM Radio"]}
        ]"#;
        let input = r#"{
            "squelch":"7","brightness":"5","ligcon":"15s","ponmsg":"Icon",
            "mdfa":"Name","btnvoice":true,"tx220":true,"poweron_msg.msg1":"WW8L"
        }"#;
        let mut image = vec![0xFFu8; 0x2008];
        apply_profile_settings(&mut image, schema, input).unwrap();

        let decoded = decode_profile_settings(&image, schema).unwrap();
        assert_eq!(decoded["squelch"], serde_json::json!("7"));
        assert_eq!(decoded["brightness"], serde_json::json!("5"));
        assert_eq!(decoded["ligcon"], serde_json::json!("15s"));
        assert_eq!(decoded["ponmsg"], serde_json::json!("Icon"));
        assert_eq!(decoded["mdfa"], serde_json::json!("Name"));
        assert_eq!(decoded["btnvoice"], serde_json::json!(true));
        assert_eq!(decoded["tx220"], serde_json::json!(true));
        assert_eq!(decoded["poweron_msg.msg1"], serde_json::json!("WW8L"));
        // The deferred key-assignment is not emitted (we don't locate it).
        assert!(decoded.get("ssidekey1").is_none());
    }

    #[test]
    fn profile_apply_skips_unknown_keys_and_preserves_neighbors() {
        // An unsupported key must be skipped, and applying one bitfield must not
        // disturb the reserved bits sharing its byte.
        let schema = r#"[{"key":"dbrx","type":"boolean"},{"key":"dtmfst","type":"boolean"}]"#;
        let mut image = vec![0xFFu8; 0x2008];
        let applied =
            apply_profile_settings(&mut image, schema, r#"{"dbrx":false,"dtmfst":true}"#).unwrap();
        assert_eq!(applied, 1); // only dbrx is mapped; dtmfst is deferred/skipped
        // byte 11: dbrx is bit 2 → cleared; the other bits stay 1 → 0xFB.
        assert_eq!(image[SETTINGS_BASE + 11], 0xFB);
    }

    #[test]
    fn set_bits_replaces_only_the_field() {
        let mut byte = 0xFFu8;
        set_bits(&mut byte, 6, 2, 0b01); // top two bits → 01
        assert_eq!(byte, 0b0111_1111);
        let mut byte = 0x00u8;
        set_bits(&mut byte, 2, 1, 1);
        assert_eq!(byte, 0b0000_0100);
    }
}
