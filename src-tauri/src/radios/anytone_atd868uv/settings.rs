//! AT-D868UV radio settings: read off the radio into the profile form, and
//! written back read-modify-write over the same cable session as a program.
//!
//! The D868's settings live in the same three 0x4000 windows as the D578's —
//! the general block at 0x02500000, the selective-calling and alarm blocks in
//! the bitmap window at 0x024C0000, and the radio-ID record at 0x02580000 — so
//! the D578's reader and writer serve it with this model's own field table
//! (`settings_table.rs`, generated) and its own backup prefix.

use serde_json::Value;

use crate::radios::anytone_atd578uv::settings::{read_settings_model, write_settings_model};
use crate::radios::anytone_atd890uv::settings::{SF, SK};
use crate::radios::driver::{SettingsCapture, SettingsReader, SettingsWriteReport, SettingsWriter};

use super::{AnytoneAtd868uv, D868_MODEL};

include!("settings_table.rs");

/// File-name prefix of a settings write's backup and expected image, distinct
/// from the D578's so the program dialog's Verify and Restore never mix them.
pub(crate) const SETTINGS_PREFIX: &str = "atd868uv-settings-write-";

impl SettingsReader for AnytoneAtd868uv {
    fn read_settings(&self, port: &str, _schema_json: &str) -> Result<SettingsCapture, String> {
        read_settings_model(&D868_MODEL, D868_SETTINGS_FIELDS, port)
    }
}

impl SettingsWriter for AnytoneAtd868uv {
    fn write_settings(
        &self,
        port: &str,
        settings: &Value,
        _schema_json: &str,
        backup_dir: &std::path::Path,
    ) -> Result<SettingsWriteReport, String> {
        write_settings_model(&D868_MODEL, D868_SETTINGS_FIELDS, SETTINGS_PREFIX, port, settings, backup_dir)
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use serde_json::{Map, Value};

    use super::*;
    use crate::radios::anytone_atd578uv::settings::{
        decode_settings_with, plan_settings_write_with, BITMAP_WINDOW, DMR_ID_KEY, RADIO_ID_BASE,
        RADIO_NAME_KEY, SETTINGS_WINDOW, WRITE_WINDOWS,
    };
    use crate::radios::anytone_atd890uv::WRITE_WINDOW;

    /// The anchor radio's settings regions (general, power-on password,
    /// 5-Tone/DTMF, 2-Tone, bitmaps, alarm, radio ID 1). The boot text, which
    /// carries the owner's phone number, is not in it.
    const SETTINGS: &[u8] = include_bytes!("testdata/d868_settings.bin");

    fn windows() -> BTreeMap<u32, Vec<u8>> {
        let mut w: BTreeMap<u32, Vec<u8>> = WRITE_WINDOWS.iter().map(|&a| (a, vec![0xFF; WRITE_WINDOW])).collect();
        for (addr, data) in crate::radios::anytone_atd890uv::parse_dump(SETTINGS).unwrap() {
            let base = addr & !(WRITE_WINDOW as u32 - 1);
            let o = (addr - base) as usize;
            w.get_mut(&base).unwrap()[o..o + data.len()].copy_from_slice(&data);
        }
        w
    }

    fn decode(w: &BTreeMap<u32, Vec<u8>>) -> Value {
        decode_settings_with(D868_SETTINGS_FIELDS, w)
    }

    /// Values independently known about the anchor radio: the VFO scan limits
    /// it reports are its band (136-174 / 400-480), the CPS's 5-Tone defaults
    /// (70 ms decode tone, the 0x0D the CPS never moves is not a field), the
    /// alarm extension's 12345678 the CPS ships, and the radio's own ID.
    #[test]
    fn decodes_known_values_off_the_real_radio() {
        let v = decode(&windows());
        assert_eq!(v["vfo-scan-start-freq-uhf"], "400.000");
        assert_eq!(v["vfo-scan-end-freq-uhf"], "480.000");
        assert_eq!(v["vfo-scan-start-freq-vhf"], "136.000");
        assert_eq!(v["vfo-scan-end-freq-vhf"], "174.000");
        assert_eq!(v["five-tone-decode-tone-time"], "70");
        assert_eq!(v["five-tone-bot-tone-time"], "70");
        assert_eq!(v["five-tone-eot-tone-time"], "70");
        assert_eq!(v["two-tone-first-tone-duration"], 5, "0.5 s in 0.1 s units");
        assert_eq!(v["dtmf-interval-character"], "*");
        assert_eq!(v["digital-alarm-tg-dmr-id"], 12_345_678);
        assert_eq!(v["digital-receive-alarm"], true);
        assert_eq!(v["power-on-password-char"], "12345678");
        // DMR APRS: auto interval raw 20 = 345 s (qdmr: 45 + 15 x raw), the
        // talk group BCD 00 42 47 36.
        assert_eq!(v["dmr-aprs-auto-tx-interval"], "345");
        assert_eq!(v["dmr-aprs-talk-group"], 424_736);
        assert_eq!(v["dmr-aprs-transmit-power"], "Low");
        // Alarm durations in seconds: 10 / 10 / 60 (analog), 10 / 10 / 60 (digital).
        assert_eq!(v["analog-alarm-time"], 10);
        assert_eq!(v["analog-alarm-rx-duration"], 60);
        assert_eq!(v["digital-alarm-rx-duration"], 60);
        // DTMF IDs: one digit code per byte, 0xFF padded.
        assert_eq!(v["dtmf-self-id"], "000");
        assert_eq!(v["dtmf-bot-id"], "");
        // Placed by front-panel changes on this radio (before them: 50 ms, Off, M-SMS).
        assert_eq!(v["dtmf-transmitting-time"], "50 ms");
        assert_eq!(v["auto-repeater-b"], "Off");
        assert_eq!(v["sms-format"], "M-SMS");
        assert_eq!(v[DMR_ID_KEY], 3_108_176);
        assert_eq!(v[RADIO_NAME_KEY], "My Radio");
    }

    /// Writing back exactly what was read changes nothing — including the two
    /// values the CPS could not have produced, which pass through.
    #[test]
    fn writing_the_decoded_values_back_is_a_no_op() {
        let w = windows();
        let v = decode(&w);
        assert_eq!(v["dtmf-group-code"], "9", "an off-list raw value passes through");
        let plans = plan_settings_write_with(D868_SETTINGS_FIELDS, &w, &v).unwrap();
        assert!(plans.is_empty(), "{} windows changed", plans.len());
    }

    /// One field in each window changes that field's byte and nothing else.
    #[test]
    fn one_change_touches_only_its_bytes() {
        let w = windows();
        let mut v = Map::new();
        v.insert("brightness".into(), Value::from("2"));
        v.insert("five-tone-stop-code".into(), Value::from("D"));
        let plans = plan_settings_write_with(D868_SETTINGS_FIELDS, &w, &Value::Object(v)).unwrap();
        let diff = |base: u32| -> Vec<usize> {
            let p = plans.iter().find(|p| p.base == base).unwrap();
            (0..WRITE_WINDOW).filter(|&i| p.original[i] != p.patched[i]).collect()
        };
        assert_eq!(plans.len(), 2);
        assert_eq!(diff(SETTINGS_WINDOW), vec![0x26]);
        assert_eq!(diff(BITMAP_WINDOW), vec![0x1032]);
        let p = plans.iter().find(|p| p.base == SETTINGS_WINDOW).unwrap();
        assert_eq!(p.patched[0x26], 1, "brightness '2' is index 1");
        let p = plans.iter().find(|p| p.base == BITMAP_WINDOW).unwrap();
        assert_eq!(p.patched[0x1032], 0x0D, "stop code D is the hex digit");
    }

    /// The key functions store the function CODE, not the position in that
    /// key's list: P1's list omits Call and Sub PTT, so its "Scan" is code 0x0A.
    #[test]
    fn key_functions_encode_by_name_to_the_code() {
        let w = windows();
        let mut v = Map::new();
        v.insert("p1-short-key".into(), Value::from("Scan"));
        v.insert("pf2-long-key".into(), Value::from("Hot Key 1"));
        let plans = plan_settings_write_with(D868_SETTINGS_FIELDS, &w, &Value::Object(v)).unwrap();
        let p = plans.iter().find(|p| p.base == SETTINGS_WINDOW).unwrap();
        assert_eq!(p.patched[0x13], 0x0A);
        assert_eq!(p.patched[0x42], 0x14);
    }

    /// A DTMF ID is one digit code per byte, 0xFF after it; a string with a
    /// non-DTMF character, or one too long, leaves the radio's bytes alone.
    #[test]
    fn dtmf_ids_are_digit_codes_padded_with_ff() {
        let w = windows();
        let mut v = Map::new();
        v.insert("dtmf-bot-id".into(), Value::from("12a*#"));
        let plans = plan_settings_write_with(D868_SETTINGS_FIELDS, &w, &Value::Object(v)).unwrap();
        let p = plans.iter().find(|p| p.base == BITMAP_WINDOW).unwrap();
        assert_eq!(&p.patched[0x1090..0x10A0], &[1, 2, 0x0A, 0x0E, 0x0F, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF]);
        for bad in ["12G", "12345678901234567"] {
            let mut v = Map::new();
            v.insert("dtmf-bot-id".into(), Value::from(bad));
            assert!(plan_settings_write_with(D868_SETTINGS_FIELDS, &w, &Value::Object(v)).unwrap().is_empty(), "{bad}");
        }
    }

    /// The four key locks are bits of one byte (0xB5): one lock, one bit.
    #[test]
    fn key_locks_are_bits_of_one_byte() {
        let w = windows();
        let mut v = Map::new();
        v.insert("side-key-lock".into(), Value::from(true));
        let plans = plan_settings_write_with(D868_SETTINGS_FIELDS, &w, &Value::Object(v)).unwrap();
        let p = plans.iter().find(|p| p.base == SETTINGS_WINDOW).unwrap();
        assert_eq!(p.patched[0xB5] ^ p.original[0xB5], 0b1000);
    }

    #[test]
    fn a_new_dmr_id_sets_record_zero() {
        let w = windows();
        let mut v = Map::new();
        v.insert(DMR_ID_KEY.into(), Value::from(3_100_001));
        let plans = plan_settings_write_with(D868_SETTINGS_FIELDS, &w, &Value::Object(v)).unwrap();
        let rid = plans.iter().find(|p| p.base == RADIO_ID_BASE).unwrap();
        assert_eq!(&rid.patched[0..5], &[0x03, 0x10, 0x00, 0x01, 0x00]);
    }

    /// The generated table and the profile schema name the same keys, so no form
    /// field is dead and no table row is unreachable.
    #[test]
    fn table_and_schema_agree() {
        let schema: Vec<Value> =
            serde_json::from_str(include_str!("../../anytone_d868_settings_schema.json")).unwrap();
        let mut in_schema: Vec<&str> = schema
            .iter()
            .filter(|f| f["type"] != "section")
            .map(|f| f["key"].as_str().unwrap())
            .filter(|k| *k != DMR_ID_KEY && *k != RADIO_NAME_KEY)
            .collect();
        let mut in_table: Vec<&str> = D868_SETTINGS_FIELDS.iter().map(|f| f.key).collect();
        in_schema.sort_unstable();
        in_table.sort_unstable();
        assert_eq!(in_schema, in_table);
        assert_eq!(in_table.len(), 156);
    }

    /// Every field sits inside a window a settings session reads.
    #[test]
    fn every_field_is_inside_a_settings_window() {
        for f in D868_SETTINGS_FIELDS {
            let len = match f.kind {
                SK::Text { len } => len as u32,
                SK::Digits { len } => len as u32,
                SK::FreqLe | SK::BcdId => 4,
                _ => 1,
            };
            let end = f.base + f.byte as u32 + len;
            assert!(
                WRITE_WINDOWS.iter().any(|&w| f.base >= w && end <= w + WRITE_WINDOW as u32),
                "{}",
                f.key
            );
        }
    }
}
