//! THROWAWAY hardware ladder for the AT-D578UV (issue #47, new-radio step 7).
//! Each test is ONE radio operation in one process — the radio reboots and drops
//! off USB on END, so it is reattached between runs. Each write saves its backup
//! and its expected image BEFORE anything is sent, and says what the follow-up
//! verify must find.
//!
//!   CPM_PORT=/dev/cu.usbmodemXXX CPM_DIR=<scratch dir> \
//!     cargo test --lib hw_d578_<op> -- --ignored --nocapture
//!
//! Ops: `identity` (rewrite bank 0 unchanged), `rename` (change one character of
//! channel 1's name), `verify` (read back the last `.expected.bin` in CPM_DIR,
//! or CPM_EXPECTED, and byte-compare).

use std::path::PathBuf;

use crate::radios::anytone_atd890uv::{
    diff_dumps, end_session, open_port, parse_dump, read_block,
    serialize_backup, BankPlan, WRITE_WINDOW,
};

use super::*;

fn env(key: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| panic!("{key} not set"))
}

fn dir() -> PathBuf {
    PathBuf::from(env("CPM_DIR"))
}

/// Read bank 0, apply `edit`, and write it the way the driver does: the rest of
/// its flash half carried, the marker frame never sent ([`write_windows`]).
fn write_bank0(label: &str, edit: impl FnOnce(&mut Vec<u8>)) {
    write_windows(label, &[CHANNEL_BASE], |w| edit(w.get_mut(&CHANNEL_BASE).unwrap()));
}

/// Rung 1: rewrite bank 0 byte-for-byte. Proves the write path and the commit
/// with nothing at risk; the verify must find zero differences.
#[test]
#[ignore = "writes a real radio"]
fn hw_d578_identity() {
    write_bank0("identity", |_| {});
}

/// Rung 2: channel 1's name, last character 'S' -> 'Z' ("***SCAN CHANNELZ").
/// The verify must find exactly that one byte changed, and the radio's channel
/// list must show the new name.
#[test]
#[ignore = "writes a real radio"]
fn hw_d578_rename() {
    write_bank0("rename", |w| {
        let o = CH_NAME + NAME_CHARS - 1;
        assert_eq!(w[o], b'S', "channel 1 is not the anchor radio's \"***SCAN CHANNELS\"");
        w[o] = b'Z';
    });
}

/// Read back every block of an expected image and byte-compare. Read-only.
#[test]
#[ignore = "reads a real radio"]
fn hw_d578_verify() {
    let expected = std::env::var("CPM_EXPECTED").map(PathBuf::from).unwrap_or_else(|_| {
        let mut all: Vec<PathBuf> = std::fs::read_dir(dir())
            .unwrap()
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.to_string_lossy().ends_with(".expected.bin"))
            .collect();
        all.sort();
        all.pop().expect("no .expected.bin in CPM_DIR")
    });
    eprintln!("verifying {}", expected.display());
    let blocks = parse_dump(&std::fs::read(&expected).unwrap()).unwrap();
    let mut p = open_port(&env("CPM_PORT")).unwrap();
    let _ = enter(&mut *p).unwrap();
    let mut actual = Vec::new();
    for (addr, data) in &blocks {
        match read_block(&mut *p, *addr, data.len()) {
            Ok(d) => actual.push((*addr, d)),
            Err(e) => {
                let _ = end_session(&mut *p);
                panic!("read 0x{addr:08X} failed: {e}");
            }
        }
    }
    let _ = end_session(&mut *p);
    let diffs = diff_dumps(&blocks, &actual);
    eprintln!("{} bytes checked, {} differences", blocks.iter().map(|(_, d)| d.len()).sum::<usize>(), diffs.len());
    for d in diffs.iter().take(20) {
        eprintln!("  {d:?}");
    }
    assert!(diffs.is_empty(), "radio does not match the expected image");
}

/// Rung 3: a full codeplug through the driver's real program path
/// (`CodeplugProgrammer::program`): every channel list and scan list any codeplug
/// in a COPY of the app database uses, assembled the way the app assembles one.
/// The backup and expected image land in CPM_DIR as `atd578uv-program-*`.
///   CPM_DB=<copy of the app db> CPM_PORT=… CPM_DIR=… cargo test --lib hw_d578_program -- --ignored --nocapture
#[tokio::test]
#[ignore = "writes a real radio"]
async fn hw_d578_program() {
    use crate::radios::driver::CodeplugProgrammer;
    let pool = crate::db::init_pool(std::path::Path::new(&env("CPM_DB"))).await.unwrap();
    let (model_id,): (i64,) = sqlx::query_as("SELECT id FROM radio_models WHERE model = 'AT-D578UV'")
        .fetch_one(&pool)
        .await
        .unwrap();
    let profile_id = sqlx::query("INSERT INTO radio_profiles (display_name, radio_model_id) VALUES ('ladder', ?1)")
        .bind(model_id)
        .execute(&pool)
        .await
        .unwrap()
        .last_insert_rowid();
    let cp_id = sqlx::query("INSERT INTO codeplugs (name, radio_profile_id) VALUES ('ladder', ?1)")
        .bind(profile_id)
        .execute(&pool)
        .await
        .unwrap()
        .last_insert_rowid();
    sqlx::query(
        "INSERT INTO codeplug_channel_lists (codeplug_id, channel_list_id, position)
         SELECT ?1, channel_list_id, ROW_NUMBER() OVER (ORDER BY MIN(codeplug_id), MIN(position))
         FROM codeplug_channel_lists WHERE codeplug_id != ?1 GROUP BY channel_list_id",
    )
    .bind(cp_id)
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO codeplug_scan_lists (codeplug_id, scan_list_id)
         SELECT DISTINCT ?1, scan_list_id FROM codeplug_scan_lists WHERE codeplug_id != ?1",
    )
    .bind(cp_id)
    .execute(&pool)
    .await
    .unwrap();
    let resolved = crate::commands::export::resolve_codeplug_payload(&pool, cp_id).await.unwrap();
    let report = DRIVER.program(&env("CPM_PORT"), &resolved.payload(), &dir()).unwrap();
    eprintln!(
        "programmed {} channels ({} cleared), {} zones ({} cleared), {} scan lists ({} cleared), \
         {} contacts ({} cleared); {} windows; backup {}",
        report.channels_written,
        report.slots_cleared,
        report.zones_written,
        report.zones_cleared,
        report.scan_lists_written,
        report.scan_lists_cleared,
        report.contacts_written,
        report.contacts_cleared,
        report.windows_written.len(),
        report.backup_path
    );
    for w in report.warnings.iter().take(10) {
        eprintln!("  warning {w}");
    }
}

/// Restore the newest `atd578uv-program-*.bin` backup in CPM_DIR (or
/// CPM_BACKUP) through the same guard and write loop as the dialog's Restore,
/// then END. Verify afterwards with CPM_EXPECTED pointing at the backup itself.
#[test]
#[ignore = "writes a real radio"]
fn hw_d578_restore() {
    let backup = std::env::var("CPM_BACKUP").map(PathBuf::from).unwrap_or_else(|_| {
        let mut all: Vec<PathBuf> = std::fs::read_dir(dir())
            .unwrap()
            .flatten()
            .map(|e| e.path())
            .filter(|p| {
                let n = p.file_name().unwrap().to_string_lossy().to_string();
                n.starts_with(program::PROGRAM_PREFIX) && !n.ends_with(".expected.bin")
            })
            .collect();
        all.sort();
        all.pop().expect("no program backup in CPM_DIR")
    });
    eprintln!("restoring {}", backup.display());
    let blocks = parse_dump(&std::fs::read(&backup).unwrap()).unwrap();
    check_restore_blocks(&blocks).unwrap();
    let mut p = open_port(&env("CPM_PORT")).unwrap();
    let _ = enter(&mut *p).unwrap();
    for (addr, data) in &blocks {
        write_sendable(&mut *p, *addr, data)
            .unwrap_or_else(|e| panic!("write 0x{addr:08X} failed (NOT ended): {e}"));
    }
    end_session(&mut *p).unwrap();
    eprintln!("restored {} windows and ended", blocks.len());
}

/// Read `windows`, let `edit` patch copies, back up + save expected, write the
/// changed windows, END.
fn write_windows(label: &str, windows: &[u32], edit: impl FnOnce(&mut std::collections::BTreeMap<u32, Vec<u8>>)) {
    let mut p = open_port(&env("CPM_PORT")).unwrap();
    let _ = enter(&mut *p).unwrap();
    let mut original = std::collections::BTreeMap::new();
    for &w in windows {
        match read_block(&mut *p, w, WRITE_WINDOW) {
            Ok(d) => {
                original.insert(w, d);
            }
            Err(e) => {
                let _ = end_session(&mut *p);
                panic!("read 0x{w:08X} failed: {e}");
            }
        }
    }
    let mut patched = original.clone();
    edit(&mut patched);
    let plans: Vec<BankPlan> = patched
        .into_iter()
        .filter(|(b, d)| original[b] != *d)
        .map(|(b, d)| BankPlan { base: b, original: original[&b].clone(), patched: d })
        .collect();
    // The radio rebuilds every half it is written into: carry the rest of it.
    let mut reads = original.clone();
    for w in windows_of_touched_halves(&plans) {
        reads.entry(w).or_insert_with(|| read_block(&mut *p, w, WRITE_WINDOW).unwrap());
    }
    let plans = with_carried_windows(plans, &reads).unwrap();
    eprintln!("writing {:?}", plans.iter().map(|p| format!("0x{:08X}{}", p.base, if p.original == p.patched { " (carried)" } else { "" })).collect::<Vec<_>>());
    let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
    let backup = dir().join(format!("ladder-{label}-{stamp}.bin"));
    let expected = dir().join(format!("ladder-{label}-{stamp}.expected.bin"));
    std::fs::write(&backup, serialize_backup(&plan_blocks(&plans, false))).unwrap();
    std::fs::write(&expected, serialize_backup(&plan_blocks(&plans, true))).unwrap();
    eprintln!("{} windows change; backup {}", plans.len(), backup.display());
    commit_windows(&mut *p, &plans).unwrap_or_else(|e| panic!("write failed (NOT ended): {e}"));
    end_session(&mut *p).unwrap();
    eprintln!("written and ended; expected {}", expected.display());
}

/// Rung 4: six receive-only probe channels at the edges of the claimed coverage,
/// in the anchor radio's empty slots of bank 0, gathered into its empty zone 11
/// as "BAND PROBE". The radio must list all six at these frequencies.
#[test]
#[ignore = "writes a real radio"]
fn hw_d578_band_probe() {
    const SLOTS: [usize; 6] = [21, 37, 38, 70, 85, 125];
    const FREQS: [f64; 6] = [136.0, 173.995, 220.0, 224.995, 400.0, 519.995];
    const ZONE: usize = 10;
    let bitmap = CHANNEL_BITMAP_BASE & !(WRITE_WINDOW as u32 - 1);
    write_windows("band-probe", &[CHANNEL_BASE, bitmap, ZONE_LIST_BASE, ZONE_NAME_BASE], |w| {
        for (&slot, &mhz) in SLOTS.iter().zip(&FREQS) {
            let bm = &mut w.get_mut(&bitmap).unwrap()[(CHANNEL_BITMAP_BASE - bitmap) as usize..];
            assert_eq!(bm[slot / 8] >> (slot % 8) & 1, 0, "slot {} is not empty", slot + 1);
            bm[slot / 8] |= 1 << (slot % 8);
            let mut rec = channel_record_template().to_vec();
            let edit = crate::radios::anytone_atd890uv::AnytoneChannelEdit {
                name: format!("PROBE {mhz:.3}"),
                rx_mhz: mhz,
                tx_mhz: mhz,
                power: 0,
                wide: true,
                mode: 0,
                tone: Default::default(),
                color_code: 1,
                time_slot: 1,
                contact_index: 0,
                scan_list_index: Some(0xFF),
            };
            encode_channel_into(&mut rec, &edit).unwrap();
            let o = slot * CH_REC_LEN;
            w.get_mut(&CHANNEL_BASE).unwrap()[o..o + CH_REC_LEN].copy_from_slice(&rec);
        }
        let zb = &mut w.get_mut(&bitmap).unwrap()[(ZONE_BITMAP_BASE - bitmap) as usize..];
        assert_eq!(zb[ZONE / 8] >> (ZONE % 8) & 1, 0, "zone 11 is not empty");
        zb[ZONE / 8] |= 1 << (ZONE % 8);
        let members: Vec<u16> = SLOTS.iter().map(|&s| s as u16).collect();
        let o = ZONE * ZONE_LIST_STEP as usize;
        w.get_mut(&ZONE_LIST_BASE).unwrap()[o..o + ZONE_LIST_STEP as usize]
            .copy_from_slice(&encode_zone_list(&members).unwrap());
        let o = ZONE * ZONE_NAME_STEP as usize;
        w.get_mut(&ZONE_NAME_BASE).unwrap()[o..o + ZONE_NAME_STEP as usize]
            .copy_from_slice(&encode_zone_name("BAND PROBE"));
    });
}

/// Rung 5: two settings that stay visible, through the real settings encoder:
/// display brightness 4 -> 2, Channel A name colour Yellow -> Green.
#[test]
#[ignore = "writes a real radio"]
fn hw_d578_settings() {
    let values = serde_json::json!({ "display-brightness": 2, "channel-a-name-color": "Green" });
    write_windows("settings", &settings::WRITE_WINDOWS, |w| {
        let plans = settings::plan_settings_write(w, &values).unwrap();
        assert_eq!(plans.len(), 1, "only the settings window should change");
        let pl = &plans[0];
        let diff: Vec<usize> = (0..WRITE_WINDOW).filter(|&i| pl.original[i] != pl.patched[i]).collect();
        assert_eq!(diff, vec![0x01D, 0x0C5], "exactly the two fields' bytes");
        w.insert(pl.base, pl.patched.clone());
    });
}

/// Rung 5, any values: CPM_VALUES is a JSON object of profile settings, written
/// through the real settings encoder.
#[test]
#[ignore = "writes a real radio"]
fn hw_d578_settings_values() {
    let values: serde_json::Value = serde_json::from_str(&env("CPM_VALUES")).unwrap();
    write_windows("settings", &settings::WRITE_WINDOWS, |w| {
        for pl in settings::plan_settings_write(w, &values).unwrap() {
            let diff: Vec<String> = (0..WRITE_WINDOW)
                .filter(|&i| pl.original[i] != pl.patched[i])
                .map(|i| format!("0x{:X}", pl.base + i as u32))
                .collect();
            eprintln!("changing {diff:?}");
            w.insert(pl.base, pl.patched.clone());
        }
    });
}

/// Repair: put the anchor radio's GPS-roaming table (0x02504000, 0x400 bytes of
/// zeros in the first dump) back after a settings write erased it. The settings
/// window shares the half and is carried unchanged.
#[test]
#[ignore = "writes a real radio"]
fn hw_d578_repair_gps_roaming() {
    write_windows("repair-gps-roaming", &[0x0250_4000], |w| {
        let win = w.get_mut(&0x0250_4000).unwrap();
        assert!(win.iter().all(|&b| b == 0xFF), "the table is not in its erased state");
        win[..0x400].fill(0);
    });
}

/// Settings through the SHIPPING `SettingsWriter::write_settings` (half carrying
/// included). CPM_VALUES is a JSON object of profile values.
#[test]
#[ignore = "writes a real radio"]
fn hw_d578_settings_driver() {
    use crate::radios::driver::SettingsWriter;
    let values: serde_json::Value = serde_json::from_str(&env("CPM_VALUES")).unwrap();
    let r = DRIVER.write_settings(&env("CPM_PORT"), &values, "", &dir()).unwrap();
    eprintln!("fields {} windows {:?} backup {}", r.fields_written, r.windows_written, r.backup_path);
}

/// The AnyTone dialog's own Verify command, as the dialog calls it (driver key
/// first). CPM_EXPECTED is the image to verify against.
#[tokio::test]
#[ignore = "reads a real radio"]
async fn hw_d578_dialog_verify() {
    use crate::commands::anytone_program::verify_anytone_program;
    let path = env("CPM_EXPECTED");
    let wrong = verify_anytone_program("anytone_atd890uv".into(), env("CPM_PORT"), path.clone()).await;
    eprintln!("D890 key on a D578 file: {:?}", wrong.as_ref().err());
    assert!(wrong.is_err());
    let r = verify_anytone_program("anytone_atd578uv".into(), env("CPM_PORT"), path).await.unwrap();
    eprintln!("ok {} bytes {} mismatches {}", r.ok, r.bytes_checked, r.mismatches.len());
    assert!(r.ok);
}

/// The AnyTone dialog's own Restore command. CPM_BACKUP is the file.
#[tokio::test]
#[ignore = "writes a real radio"]
async fn hw_d578_dialog_restore() {
    use crate::commands::anytone_program::restore_anytone_backup;
    let r = restore_anytone_backup("anytone_atd578uv".into(), env("CPM_PORT"), env("CPM_BACKUP")).await.unwrap();
    eprintln!("restored {:?}", r.windows_written);
}

/// Call-sign DB through the SHIPPING writer, from a copy of the app database
/// (users + talkgroups exactly as the command assembles them). The expected
/// image is saved first as `csdb-<stamp>.expected.bin` in CPM_DIR.
#[tokio::test]
#[ignore = "writes a real radio"]
async fn hw_d578_csdb_program() {
    use crate::radios::driver::CallsignDbWriter;
    let pool = crate::db::init_pool(std::path::Path::new(&env("CPM_DB"))).await.unwrap();
    let records = crate::commands::program::callsign_records(&pool, None, &[]).await.unwrap();
    let enc = callsign_db::encode(&callsign_db::entries_from_records(&records)).unwrap();
    let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
    let expected = dir().join(format!("csdb-{stamp}.expected.bin"));
    std::fs::write(&expected, serialize_backup(&enc.blocks)).unwrap();
    eprintln!("{} records -> {} entries, {} blocks; expected {}", records.len(), enc.entries, enc.blocks.len(), expected.display());
    let r = DRIVER.write_callsign_db(&env("CPM_PORT"), &records).unwrap();
    eprintln!("wrote {} entries, {} blocks", r.entries_written, r.windows_written.len());
}

/// Put a full call-sign DB dump (CPM_BACKUP) back, direct, in one session.
#[test]
#[ignore = "writes a real radio"]
fn hw_d578_csdb_restore() {
    let blocks = parse_dump(&std::fs::read(env("CPM_BACKUP")).unwrap()).unwrap();
    let mut p = open_port(&env("CPM_PORT")).unwrap();
    let _ = enter(&mut *p).unwrap();
    for (addr, data) in &blocks {
        write_sendable(&mut *p, *addr, data).unwrap_or_else(|e| panic!("0x{addr:08X} failed (NOT ended): {e}"));
    }
    end_session(&mut *p).unwrap();
    eprintln!("restored {} blocks", blocks.len());
}

/// Air band: three channels at the edges and middle of 108-136 MHz in the
/// first three air slots, present and scanned; everything else as it was.
#[test]
#[ignore = "writes a real radio"]
fn hw_d578_air_probe() {
    write_windows("air-probe", &[AIR_BASE], |w| {
        let win = w.get_mut(&AIR_BASE).unwrap();
        for (i, (mhz, name)) in [(108.0, "AIR 108.000"), (121.5, "AIR 121.500"), (135.975, "AIR 135.975")].iter().enumerate() {
            win[i * AIR_REC_LEN..(i + 1) * AIR_REC_LEN].copy_from_slice(&encode_air_channel(*mhz, name));
        }
        let bm = (AIR_BITMAP - AIR_BASE) as usize;
        let scan = (AIR_SCAN_BITMAP - AIR_BASE) as usize;
        win[bm] |= 0x07;
        win[scan] |= 0x07;
    });
}
