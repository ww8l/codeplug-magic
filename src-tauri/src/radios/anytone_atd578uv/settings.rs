//! AT-D578UV radio settings: read off the radio into the profile form, and
//! written back read-modify-write, over the same cable session as a program.
//!
//! The field table is generated (`settings_table.rs`) and the per-field codec is
//! the family's (`anytone_atd890uv::settings`), so this module is the D578's
//! regions and its own radio-ID record and nothing else.
//!
//! Every field region lives in the one 0x4000 window at [`SETTINGS_WINDOW`];
//! the radio's own DMR ID and name are radio-ID record 0 at [`RADIO_ID_BASE`],
//! shown by bit 0 of the radio-ID present bitmap.

use std::collections::BTreeMap;

use serde_json::{Map, Value};

use crate::radios::anytone_atd890uv::settings::{
    as_u32, bcd_be_decode, bcd_be_encode, decode_field, encode_field_into, SF, SK,
};
use crate::radios::anytone_atd890uv::{
    end_session, open_port, read_block, serialize_backup, BankPlan,
    WRITE_WINDOW,
};
use crate::radios::driver::{SettingsCapture, SettingsReader, SettingsWriteReport, SettingsWriter};

use super::{decode_ascii_field, encode_ascii_field, enter, AnytoneAtd578uv};

include!("settings_table.rs");

/// The window holding every generated field (general, boot, APRS, messages,
/// extension — 0x02500000..0x02501600).
pub(crate) const SETTINGS_WINDOW: u32 = 0x0250_0000;
/// Radio-ID records: BCD-BE DMR ID @+0, pad @+4, 16-char ASCII name @+5.
pub(crate) const RADIO_ID_BASE: u32 = 0x0258_0000;
/// The window holding the radio-ID present bitmap (0x024C1320).
pub(crate) const BITMAP_WINDOW: u32 = 0x024C_0000;
pub(crate) const RADIO_ID_BITMAP: u32 = 0x024C_1320;
pub(crate) const RADIO_ID_NAME: usize = 0x05;

pub(crate) const DMR_ID_KEY: &str = "dmr-id";
pub(crate) const RADIO_NAME_KEY: &str = "radio-name";

/// File-name prefix of a settings write's backup and expected image. The
/// program dialog's Verify and Restore find them by it.
pub(crate) const SETTINGS_PREFIX: &str = "atd578uv-settings-write-";

/// The windows a settings write reads and may rewrite.
pub(crate) const WRITE_WINDOWS: [u32; 3] = [SETTINGS_WINDOW, RADIO_ID_BASE, BITMAP_WINDOW];

fn slice_at(windows: &BTreeMap<u32, Vec<u8>>, addr: u32) -> &[u8] {
    let base = addr & !(WRITE_WINDOW as u32 - 1);
    windows
        .get(&base)
        .map(|w| &w[(addr - base) as usize..])
        .unwrap_or(&[])
}

/// Decode the settings windows into the profile form's values.
pub(crate) fn decode_settings(windows: &BTreeMap<u32, Vec<u8>>) -> Value {
    let mut map = Map::new();
    for f in D578_SETTINGS_FIELDS {
        map.insert(f.key.to_string(), decode_field(slice_at(windows, f.base), f.byte as usize, &f.kind));
    }
    let bitmap = slice_at(windows, RADIO_ID_BITMAP);
    let rec = slice_at(windows, RADIO_ID_BASE);
    if bitmap.first().is_some_and(|b| b & 1 == 1) && rec.len() >= RADIO_ID_NAME + 16 {
        if let Some(id) = bcd_be_decode(&rec[0..4]) {
            map.insert(DMR_ID_KEY.to_string(), Value::from(id));
            map.insert(
                RADIO_NAME_KEY.to_string(),
                Value::from(decode_ascii_field(&rec[RADIO_ID_NAME..RADIO_ID_NAME + 16])),
            );
        }
    }
    Value::Object(map)
}

/// Apply profile values onto copies of the windows (read-modify-write: every
/// byte and bit no field names is kept) and return a plan for each window that
/// changed. A value that does not parse leaves its bytes alone.
pub(crate) fn plan_settings_write(
    windows: &BTreeMap<u32, Vec<u8>>,
    values: &Value,
) -> Result<Vec<BankPlan>, String> {
    let obj = values.as_object().ok_or("settings values is not a JSON object")?;
    let mut patched = windows.clone();
    for f in D578_SETTINGS_FIELDS {
        let Some(v) = obj.get(f.key) else { continue };
        let base = f.base & !(WRITE_WINDOW as u32 - 1);
        let Some(w) = patched.get_mut(&base) else { continue };
        encode_field_into(&mut w[(f.base - base) as usize..], f.byte as usize, &f.kind, v);
    }

    // The radio's own DMR ID: record 0 and its present bit, only when the
    // profile carries a non-zero ID — a profile without one leaves the radio's
    // ID alone. A blank name likewise keeps the radio's own (#128).
    if let Some(id) = obj.get(DMR_ID_KEY).and_then(as_u32).filter(|&id| id != 0) {
        if id > 16_777_215 {
            return Err(format!("DMR ID {id} is out of range"));
        }
        if let Some(w) = patched.get_mut(&RADIO_ID_BASE) {
            w[0..4].copy_from_slice(&bcd_be_encode(id));
            w[4] = 0;
            // Rewrite the name only when it changed: the radio leaves stale
            // bytes after a shortened name's NUL ("My Radio\0…R" on the anchor
            // radio), and re-padding an unchanged name would rewrite them.
            let field = RADIO_ID_NAME..RADIO_ID_NAME + 16;
            if let Some(name) = obj
                .get(RADIO_NAME_KEY)
                .and_then(Value::as_str)
                .filter(|n| !n.is_empty() && *n != decode_ascii_field(&w[field.clone()]))
            {
                w[field].copy_from_slice(&encode_ascii_field(name, 16));
            }
        }
        if let Some(w) = patched.get_mut(&BITMAP_WINDOW) {
            w[(RADIO_ID_BITMAP - BITMAP_WINDOW) as usize] |= 1;
        }
    }

    Ok(patched
        .into_iter()
        .filter(|(base, data)| windows.get(base) != Some(data))
        .map(|(base, data)| BankPlan { base, original: windows[&base].clone(), patched: data })
        .collect())
}

fn read_windows(p: &mut dyn serialport::SerialPort) -> Result<BTreeMap<u32, Vec<u8>>, String> {
    let mut out = BTreeMap::new();
    for w in WRITE_WINDOWS {
        out.insert(w, read_block(p, w, WRITE_WINDOW)?);
    }
    Ok(out)
}

impl SettingsReader for AnytoneAtd578uv {
    /// Read the settings windows and decode them. The table is built in, so the
    /// schema argument is unused; the raw windows come back as the backup.
    fn read_settings(&self, port: &str, _schema_json: &str) -> Result<SettingsCapture, String> {
        let mut p = open_port(port)?;
        let _ident = enter(&mut *p)?;
        let windows = read_windows(&mut *p);
        let _ = end_session(&mut *p);
        let windows = windows?;
        let blocks: Vec<(u32, Vec<u8>)> = windows.iter().map(|(a, d)| (*a, d.clone())).collect();
        Ok(SettingsCapture {
            settings: decode_settings(&windows),
            backup: serialize_backup(&blocks),
            backup_ext: "bin",
        })
    }
}

impl SettingsWriter for AnytoneAtd578uv {
    /// Read the windows fresh, apply the profile, back up the originals and save
    /// the expected image BEFORE any write, write the changed windows, END once.
    fn write_settings(
        &self,
        port: &str,
        settings: &Value,
        _schema_json: &str,
        backup_dir: &std::path::Path,
    ) -> Result<SettingsWriteReport, String> {
        std::fs::create_dir_all(backup_dir).map_err(|e| e.to_string())?;
        let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
        let backup_path = backup_dir.join(format!("{SETTINGS_PREFIX}{stamp}.bin"));
        let expected_path = backup_dir.join(format!("{SETTINGS_PREFIX}{stamp}.expected.bin"));

        let mut p = open_port(port)?;
        let _ident = enter(&mut *p)?;
        let planned = read_windows(&mut *p).and_then(|w| {
            let plans = plan_settings_write(&w, settings)?;
            // The rest of every half a write lands in, carried unchanged.
            let mut reads = w;
            for a in super::windows_of_touched_halves(&plans) {
                if let std::collections::btree_map::Entry::Vacant(e) = reads.entry(a) {
                    e.insert(read_block(&mut *p, a, WRITE_WINDOW)?);
                }
            }
            super::with_carried_windows(plans, &reads)
        });
        let plans = match planned {
            Ok(plans) => plans,
            Err(e) => {
                let _ = end_session(&mut *p);
                return Err(e);
            }
        };
        if plans.is_empty() {
            let _ = end_session(&mut *p);
            return Err("the radio already matches this profile's settings — nothing to write".into());
        }
        let fields_written = D578_SETTINGS_FIELDS
            .iter()
            .filter(|f| {
                let base = f.base & !(WRITE_WINDOW as u32 - 1);
                plans.iter().find(|pl| pl.base == base).is_some_and(|pl| {
                    let o = (f.base - base) as usize + f.byte as usize;
                    field_changed(&f.kind, &pl.original[o..], &pl.patched[o..])
                })
            })
            .count();

        for (path, bytes) in [
            (&backup_path, serialize_backup(&super::plan_blocks(&plans, false))),
            (&expected_path, serialize_backup(&super::plan_blocks(&plans, true))),
        ] {
            if let Err(e) = std::fs::write(path, bytes) {
                let _ = end_session(&mut *p);
                return Err(format!("could not write {}: {e}", path.display()));
            }
        }
        let written = super::commit_windows(&mut *p, &plans).map_err(|e| {
            crate::radios::driver::with_restore_hint(
                e,
                &backup_path,
                "Power-cycle the radio, rescan the port, then use \"Restore backup\" in the \
                 Program dialog.",
            )
        })?;
        let _ = end_session(&mut *p);
        Ok(SettingsWriteReport {
            fields_written,
            // END commits and reboots the radio; verification is the dialog's
            // fresh-session Verify against the expected image.
            verified: None,
            note: Some(
                "Written and committed — the radio reboots and drops off USB. Reconnect it, \
                 rescan, then run Verify against the expected image."
                    .to_string(),
            ),
            backup_path: backup_path.to_string_lossy().to_string(),
            expected_path: Some(expected_path.to_string_lossy().to_string()),
            windows_written: written.iter().map(|a| format!("0x{a:08X}")).collect(),
        })
    }
}

/// Did this field's own value change? Bit fields compare only their bits, so a
/// change to one flag in a shared byte counts once, not once per neighbour.
fn field_changed(kind: &SK, before: &[u8], after: &[u8]) -> bool {
    match kind {
        SK::BoolBit { lsb } => (before[0] ^ after[0]) >> lsb & 1 == 1,
        SK::Bits { lsb, bits } => (before[0] ^ after[0]) >> lsb & ((1u8 << bits) - 1) != 0,
        _ => {
            let n = field_len(kind);
            before[..n] != after[..n]
        }
    }
}

/// Bytes a field occupies (bit fields: the one byte they live in).
fn field_len(kind: &SK) -> usize {
    match kind {
        SK::Uint { width } | SK::Enum { width, .. } => *width as usize,
        SK::Text { len } => *len as usize,
        SK::Utf16 { chars } => *chars as usize * 2,
        SK::FreqBcd | SK::FreqLe | SK::BcdId => 4,
        SK::Bool | SK::BoolBit { .. } | SK::Bits { .. } => 1,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The anchor radio's settings window (fw 1.21), the fields' regions only,
    /// with the boot text — which carries the owner's phone number — blanked.
    const SETTINGS: &[u8] = include_bytes!("testdata/d578_settings.bin");

    fn windows() -> BTreeMap<u32, Vec<u8>> {
        let mut w: BTreeMap<u32, Vec<u8>> = WRITE_WINDOWS.iter().map(|&a| (a, vec![0xFF; WRITE_WINDOW])).collect();
        for (addr, data) in crate::radios::anytone_atd890uv::parse_dump(SETTINGS).unwrap() {
            let base = addr & !(WRITE_WINDOW as u32 - 1);
            let o = (addr - base) as usize;
            w.get_mut(&base).unwrap()[o..o + data.len()].copy_from_slice(&data);
        }
        w
    }

    /// Values the anchor radio holds that are independently known: its owner is
    /// in Colorado (UTC-7) and uses imperial units, talk-permit is on for DMR,
    /// the P1 key is set to Scan, and APRS was never set up (factory call sign).
    #[test]
    fn decodes_known_values_off_the_real_radio() {
        let v = decode_settings(&windows());
        assert_eq!(v["time-zone"], "UTC-07:00h");
        assert_eq!(v["units"], "Archaic");
        assert_eq!(v["talk-permit-tone"], "DMR");
        assert_eq!(v["p1-short-press"], "Scan");
        assert_eq!(v["source-call"], "BG6LKT");
        assert_eq!(v["destination-call"], "APAT51");
        assert_eq!(v["aprs-path"], "WIDE1-1,WIDE2-1");
        assert_eq!(v["aprs-symbol-table"], "/");
        assert_eq!(v["fm-aprs-frequency-1"], "145.000");
        assert_eq!(v["fm-aprs-frequency-2"], "");
        assert_eq!(v["vfo-scan-vhf-min-frequency"], "136.000");
        assert_eq!(v["uhf-auto-repeater-max-frequency"], "480.000");
        assert_eq!(v[DMR_ID_KEY], 3_108_176);
        assert_eq!(v[RADIO_NAME_KEY], "My Radio");
    }

    /// Writing back exactly what was read changes nothing.
    #[test]
    fn writing_the_decoded_values_back_is_a_no_op() {
        let w = windows();
        let plans = plan_settings_write(&w, &decode_settings(&w)).unwrap();
        assert!(plans.is_empty(), "{} windows changed", plans.len());
    }

    /// One field changes one field's bytes, and nothing else in the window.
    #[test]
    fn one_change_touches_only_its_bytes() {
        let w = windows();
        let mut v = decode_settings(&w);
        v["fm-aprs-frequency-1"] = Value::from("144.390");
        v["lock-knob"] = Value::from(true);
        let plans = plan_settings_write(&w, &v).unwrap();
        assert_eq!(plans.len(), 1);
        let diff: Vec<usize> = (0..WRITE_WINDOW).filter(|&i| plans[0].original[i] != plans[0].patched[i]).collect();
        // 145.000 -> 144.390 is 14 50 00 00 -> 14 43 90 00: bytes +1 and +2.
        assert_eq!(diff, vec![0x0B0, 0x10AD, 0x10AE]);
        assert_eq!(&plans[0].patched[0x10AC..0x10B0], &[0x14, 0x43, 0x90, 0x00]);
        assert_eq!(plans[0].patched[0x0B0] & 1, 1);
        let back = decode_settings(&plans.iter().map(|p| (p.base, p.patched.clone())).chain(
            w.iter().filter(|(b, _)| **b != plans[0].base).map(|(b, d)| (*b, d.clone())),
        ).collect());
        assert_eq!(back["fm-aprs-frequency-1"], "144.390");
        assert_eq!(back["lock-knob"], true);
    }

    #[test]
    fn one_flag_in_a_shared_byte_counts_as_one_field() {
        let kind = |lsb| SK::BoolBit { lsb };
        assert!(field_changed(&kind(0), &[0b0000], &[0b0001]));
        assert!(!field_changed(&kind(1), &[0b0000], &[0b0001]), "a neighbour bit is not a change");
        assert!(field_changed(&SK::Bits { lsb: 2, bits: 2 }, &[0], &[0b1000]));
    }

    #[test]
    fn a_new_dmr_id_sets_record_zero_and_its_present_bit() {
        let mut w = windows();
        w.get_mut(&BITMAP_WINDOW).unwrap()[0x1320] = 0;
        let mut v = Map::new();
        v.insert(DMR_ID_KEY.into(), Value::from(3_100_001));
        v.insert(RADIO_NAME_KEY.into(), Value::from("Shack"));
        let plans = plan_settings_write(&w, &Value::Object(v)).unwrap();
        let rid = plans.iter().find(|p| p.base == RADIO_ID_BASE).unwrap();
        assert_eq!(&rid.patched[0..5], &[0x03, 0x10, 0x00, 0x01, 0x00]);
        assert_eq!(&rid.patched[5..10], b"Shack");
        let bm = plans.iter().find(|p| p.base == BITMAP_WINDOW).unwrap();
        assert_eq!(bm.patched[0x1320] & 1, 1);
    }

    #[test]
    fn an_unparseable_frequency_leaves_the_radio_alone() {
        let w = windows();
        let mut v = Map::new();
        v.insert("fm-aprs-frequency-1".into(), Value::from("not a frequency"));
        assert!(plan_settings_write(&w, &Value::Object(v)).unwrap().is_empty());
    }

    /// The generated table and the profile schema name the same keys, so no form
    /// field is dead and no table row is unreachable.
    #[test]
    fn table_and_schema_agree() {
        let schema: Vec<Value> =
            serde_json::from_str(include_str!("../../anytone_d578_settings_schema.json")).unwrap();
        let mut in_schema: Vec<&str> = schema
            .iter()
            .filter(|f| f["type"] != "section")
            .map(|f| f["key"].as_str().unwrap())
            .filter(|k| *k != DMR_ID_KEY && *k != RADIO_NAME_KEY)
            .collect();
        let mut in_table: Vec<&str> = D578_SETTINGS_FIELDS.iter().map(|f| f.key).collect();
        in_schema.sort_unstable();
        in_table.sort_unstable();
        assert_eq!(in_schema, in_table);
        assert_eq!(in_table.len(), 264);
    }

    /// Every field sits inside the one window a settings session reads.
    #[test]
    fn every_field_is_inside_the_settings_window() {
        for f in D578_SETTINGS_FIELDS {
            let end = f.base + f.byte as u32 + field_len(&f.kind) as u32;
            assert!(f.base >= SETTINGS_WINDOW && end <= SETTINGS_WINDOW + WRITE_WINDOW as u32, "{}", f.key);
        }
    }

    #[test]
    fn mhz_text_keeps_three_decimals_and_drops_the_rest() {
        use crate::radios::anytone_atd890uv::settings::{mhz_text, parse_mhz};
        assert_eq!(mhz_text(14_500_000), "145.000");
        assert_eq!(mhz_text(14_439_000), "144.390");
        assert_eq!(mhz_text(14_439_125), "144.39125");
        assert_eq!(parse_mhz("144.390"), Some(14_439_000));
        assert_eq!(parse_mhz("x"), None);
        assert_eq!(parse_mhz("-1"), None);
    }
}
