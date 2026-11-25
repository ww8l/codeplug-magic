//! THROWAWAY hardware ladder for the AT-D868UV (issue #51, new-radio step 7),
//! the D578's ladder with the D868's ident, map and backup names. Each test is
//! ONE radio operation in one process. Each write saves its backup and its
//! expected image BEFORE anything is sent, and says what the follow-up verify
//! must find.
//!
//!   CPM_PORT=/dev/cu.usbmodemXXX CPM_DIR=<scratch dir> \
//!     cargo test --lib hw_d868_<op> -- --ignored --nocapture
//!
//! The whole-half safety copy (`scratchpad/anytone_atd868uv/ladder/
//! halves_before_ladder.bin`, read-only) restores with `hw_d868_restore`.

use std::collections::BTreeMap;
use std::path::PathBuf;

use crate::radios::anytone_atd578uv::{
    channel_addr, commit_windows, encode_channel_into, encode_zone_list,
    encode_zone_name, plan_blocks, windows_of_touched_halves, with_carried_windows, write_sendable,
    CHANNEL_BASE, CHANNEL_BITMAP_BASE, CH_NAME, CH_REC_LEN, NAME_CHARS, ZONE_BITMAP_BASE, ZONE_LIST_BASE,
    ZONE_LIST_STEP, ZONE_NAME_BASE, ZONE_NAME_STEP,
};
use crate::radios::anytone_atd890uv::{
    diff_dumps, end_session, open_port, parse_dump, read_block, serialize_backup, BankPlan, WRITE_WINDOW,
};

use super::{channel_record_template, check_restore_blocks, enter, DRIVER};

fn env(key: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| panic!("{key} not set"))
}

fn dir() -> PathBuf {
    PathBuf::from(env("CPM_DIR"))
}

/// Read `windows`, let `edit` patch copies, carry the rest of every touched
/// half, back up + save expected, write, END.
fn write_windows(label: &str, windows: &[u32], edit: impl FnOnce(&mut BTreeMap<u32, Vec<u8>>)) {
    let mut p = open_port(&env("CPM_PORT")).unwrap();
    let _ = enter(&mut *p).unwrap();
    let mut original = BTreeMap::new();
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
    let mut reads = original.clone();
    for w in windows_of_touched_halves(&plans) {
        reads.entry(w).or_insert_with(|| read_block(&mut *p, w, WRITE_WINDOW).unwrap());
    }
    let plans = with_carried_windows(plans, &reads).unwrap();
    eprintln!(
        "writing {:?}",
        plans
            .iter()
            .map(|p| format!("0x{:08X}{}", p.base, if p.original == p.patched { " (carried)" } else { "" }))
            .collect::<Vec<_>>()
    );
    let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
    let backup = dir().join(format!("ladder-{label}-{stamp}.bin"));
    let expected = dir().join(format!("ladder-{label}-{stamp}.expected.bin"));
    std::fs::write(&backup, serialize_backup(&plan_blocks(&plans, false))).unwrap();
    std::fs::write(&expected, serialize_backup(&plan_blocks(&plans, true))).unwrap();
    eprintln!("{} windows; backup {}", plans.len(), backup.display());
    commit_windows(&mut *p, &plans).unwrap_or_else(|e| panic!("write failed (NOT ended): {e}"));
    end_session(&mut *p).unwrap();
    eprintln!("written and ended; expected {}", expected.display());
}

/// Rung 1: rewrite bank 0 byte for byte, the rest of its half carried. Written
/// directly, because `write_windows` drops a window nothing changed. The verify
/// must find zero differences.
#[test]
#[ignore = "writes a real radio"]
fn hw_d868_identity() {
    let mut p = open_port(&env("CPM_PORT")).unwrap();
    let _ = enter(&mut *p).unwrap();
    let bank0 = read_block(&mut *p, CHANNEL_BASE, WRITE_WINDOW).unwrap();
    let plans = vec![BankPlan { base: CHANNEL_BASE, original: bank0.clone(), patched: bank0 }];
    let mut reads = BTreeMap::new();
    for w in windows_of_touched_halves(&plans) {
        reads.insert(w, read_block(&mut *p, w, WRITE_WINDOW).unwrap());
    }
    let plans = with_carried_windows(plans, &reads).unwrap();
    let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
    let backup = dir().join(format!("ladder-identity-{stamp}.bin"));
    let expected = dir().join(format!("ladder-identity-{stamp}.expected.bin"));
    std::fs::write(&backup, serialize_backup(&plan_blocks(&plans, false))).unwrap();
    std::fs::write(&expected, serialize_backup(&plan_blocks(&plans, true))).unwrap();
    eprintln!("writing {} windows unchanged; expected {}", plans.len(), expected.display());
    commit_windows(&mut *p, &plans).unwrap_or_else(|e| panic!("write failed (NOT ended): {e}"));
    end_session(&mut *p).unwrap();
    eprintln!("written and ended");
}

/// Rung 2: channel 1's name, last character 'S' -> 'Z' ("***SCAN CHANNELZ").
/// The verify must find exactly that byte changed, and the radio's channel list
/// must show the new name.
#[test]
#[ignore = "writes a real radio"]
fn hw_d868_rename() {
    write_windows("rename", &[CHANNEL_BASE], |w| {
        let b = w.get_mut(&CHANNEL_BASE).unwrap();
        let o = CH_NAME + NAME_CHARS - 1;
        assert_eq!(b[o], b'S', "channel 1 is not the anchor radio's \"***SCAN CHANNELS\"");
        b[o] = b'Z';
    });
}

/// Rung 2, observable: channel 1 is in no zone, so its rename cannot be seen on
/// the radio. This puts channel 1's name back ("***SCAN CHANNELS") and changes
/// channel 2 — the first channel of zone 1 "1 - NOCO DMRGMRS" — from
/// "SCAN NC DMRGMRS" to "SCAN NC DMRGMRZ". The verify must find exactly those
/// two bytes changed; the radio's screen must
/// show the new name at zone 1, channel 1.
#[test]
#[ignore = "writes a real radio"]
fn hw_d868_rename_zone1() {
    write_windows("rename-zone1", &[CHANNEL_BASE], |w| {
        let b = w.get_mut(&CHANNEL_BASE).unwrap();
        let o1 = CH_NAME + NAME_CHARS - 1;
        assert_eq!(b[o1], b'Z', "channel 1 should carry rung 2's 'Z'");
        b[o1] = b'S';
        let o2 = CH_REC_LEN + CH_NAME + 14;
        assert_eq!(&b[CH_REC_LEN + CH_NAME..CH_REC_LEN + CH_NAME + 16], b"SCAN NC DMRGMRS\0");
        b[o2] = b'Z';
    });
}

/// Rung 2, visible: the radio's channel screen shows 14 characters, so the
/// 15th-character change above could not be seen. First letter instead:
/// "SCAN NC DMRGMRZ" -> "ZCAN NC DMRGMRS" (zone 1, first channel).
#[test]
#[ignore = "writes a real radio"]
fn hw_d868_rename_first_letter() {
    write_windows("rename-first-letter", &[CHANNEL_BASE], |w| {
        let b = w.get_mut(&CHANNEL_BASE).unwrap();
        let n = CH_REC_LEN + CH_NAME;
        assert_eq!(&b[n..n + 16], b"SCAN NC DMRGMRZ\0");
        b[n] = b'Z';
        b[n + 14] = b'S';
    });
}

/// Read back every block of an expected image (CPM_EXPECTED, else the newest
/// `.expected.bin` in CPM_DIR) and byte-compare. Read-only.
#[test]
#[ignore = "reads a real radio"]
fn hw_d868_verify() {
    let expected = std::env::var("CPM_EXPECTED").map(PathBuf::from).unwrap_or_else(|_| {
        let mut all: Vec<PathBuf> = std::fs::read_dir(dir())
            .unwrap()
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.to_string_lossy().ends_with(".expected.bin"))
            .collect();
        all.sort_by_key(|p| std::fs::metadata(p).and_then(|m| m.modified()).ok());
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

/// Write a backup (CPM_BACKUP) back through the D868's restore guard, then END.
/// Works for the whole-half safety copy and for any ladder or program backup.
#[test]
#[ignore = "writes a real radio"]
fn hw_d868_restore() {
    let backup = PathBuf::from(env("CPM_BACKUP"));
    eprintln!("restoring {}", backup.display());
    let blocks = parse_dump(&std::fs::read(&backup).unwrap()).unwrap();
    check_restore_blocks(&blocks).unwrap();
    let mut p = open_port(&env("CPM_PORT")).unwrap();
    let _ = enter(&mut *p).unwrap();
    for (addr, data) in &blocks {
        write_sendable(&mut *p, *addr, data).unwrap_or_else(|e| panic!("write 0x{addr:08X} failed (NOT ended): {e}"));
    }
    end_session(&mut *p).unwrap();
    eprintln!("restored {} blocks and ended", blocks.len());
}

/// Rung 3: a full codeplug through the driver's real program path, from a COPY
/// of the app database: every channel list and scan list any codeplug uses.
///   CPM_DB=<copy of the app db> CPM_PORT=… CPM_DIR=… cargo test --lib hw_d868_program -- --ignored --nocapture
#[tokio::test]
#[ignore = "writes a real radio"]
async fn hw_d868_program() {
    use crate::radios::driver::CodeplugProgrammer;
    let pool = crate::db::init_pool(std::path::Path::new(&env("CPM_DB"))).await.unwrap();
    let (model_id,): (i64,) = sqlx::query_as("SELECT id FROM radio_models WHERE model = 'AT-D868UV'")
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
         {} contacts ({} cleared); {} windows; backup {}; expected {}",
        report.channels_written,
        report.slots_cleared,
        report.zones_written,
        report.zones_cleared,
        report.scan_lists_written,
        report.scan_lists_cleared,
        report.contacts_written,
        report.contacts_cleared,
        report.windows_written.len(),
        report.backup_path,
        report.expected_path
    );
}

/// Rung 4: four receive-only probe channels at the edges of the claimed bands
/// (136.000, 173.995, 400.000, 479.995) in empty slots 241-244 (bank 1, after
/// rung 3's 237 channels), gathered into the empty zone 17 as "BAND PROBE".
/// The radio must list all four at these frequencies.
#[test]
#[ignore = "writes a real radio"]
fn hw_d868_band_probe() {
    const SLOTS: [usize; 4] = [240, 241, 242, 243];
    const FREQS: [f64; 4] = [136.0, 173.995, 400.0, 479.995];
    const ZONE: usize = 16;
    let bank = channel_addr(SLOTS[0]) & !(WRITE_WINDOW as u32 - 1);
    assert!(SLOTS.iter().all(|&s| channel_addr(s) & !(WRITE_WINDOW as u32 - 1) == bank));
    let bitmap = CHANNEL_BITMAP_BASE & !(WRITE_WINDOW as u32 - 1);
    // Check emptiness on a read of its own first: a panic inside the write
    // session would leave the radio in PC mode without an END.
    {
        let mut p = open_port(&env("CPM_PORT")).unwrap();
        let _ = enter(&mut *p).unwrap();
        let bm = read_block(&mut *p, CHANNEL_BITMAP_BASE, 0x200);
        let zb = read_block(&mut *p, ZONE_BITMAP_BASE, 0x20);
        end_session(&mut *p).unwrap();
        let (bm, zb) = (bm.unwrap(), zb.unwrap());
        for &slot in &SLOTS {
            assert_eq!(bm[slot / 8] >> (slot % 8) & 1, 0, "slot {} is not empty", slot + 1);
        }
        assert_eq!(zb[ZONE / 8] >> (ZONE % 8) & 1, 0, "zone 17 is not empty");
        eprintln!("slots and zone are empty; reconnect wait, then the write");
    }
    // END reboots the radio; wait for its port to come back (gone, then back).
    let port = env("CPM_PORT");
    std::thread::sleep(std::time::Duration::from_secs(3));
    for _ in 0..60 {
        if std::path::Path::new(&port).exists() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_secs(1));
    }
    std::thread::sleep(std::time::Duration::from_secs(4));
    write_windows("band-probe", &[bank, bitmap, ZONE_LIST_BASE, ZONE_NAME_BASE], |w| {
        for (&slot, &mhz) in SLOTS.iter().zip(&FREQS) {
            let bm = &mut w.get_mut(&bitmap).unwrap()[(CHANNEL_BITMAP_BASE - bitmap) as usize..];
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
            let o = (channel_addr(slot) - bank) as usize;
            w.get_mut(&bank).unwrap()[o..o + CH_REC_LEN].copy_from_slice(&rec);
        }
        let zb = &mut w.get_mut(&bitmap).unwrap()[(ZONE_BITMAP_BASE - bitmap) as usize..];
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

/// Rung 5: settings through the SHIPPING writer. CPM_VALUES is a JSON object of
/// profile values.
#[test]
#[ignore = "writes a real radio"]
fn hw_d868_settings_driver() {
    use crate::radios::driver::SettingsWriter;
    let values: serde_json::Value = serde_json::from_str(&env("CPM_VALUES")).unwrap();
    let r = DRIVER.write_settings(&env("CPM_PORT"), &values, "", &dir()).unwrap();
    eprintln!(
        "fields {} windows {:?} backup {} expected {:?}",
        r.fields_written, r.windows_written, r.backup_path, r.expected_path
    );
}

/// Rung 6: the call-sign DB through the SHIPPING writer, from a copy of the app
/// database. The expected image is saved first as `csdb-<stamp>.expected.bin`.
#[tokio::test]
#[ignore = "writes a real radio"]
async fn hw_d868_csdb_program() {
    use crate::radios::anytone_atd578uv::callsign_db::{encode_with, entries_from_records};
    use crate::radios::driver::CallsignDbWriter;
    let pool = crate::db::init_pool(std::path::Path::new(&env("CPM_DB"))).await.unwrap();
    let records = crate::commands::program::callsign_records(&pool, None, &[]).await.unwrap();
    let enc = encode_with(&super::CALLSIGN, &entries_from_records(&records)).unwrap();
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
fn hw_d868_csdb_restore() {
    let blocks = parse_dump(&std::fs::read(env("CPM_BACKUP")).unwrap()).unwrap();
    let mut p = open_port(&env("CPM_PORT")).unwrap();
    let _ = enter(&mut *p).unwrap();
    for (addr, data) in &blocks {
        write_sendable(&mut *p, *addr, data).unwrap_or_else(|e| panic!("0x{addr:08X} failed (NOT ended): {e}"));
    }
    end_session(&mut *p).unwrap();
    eprintln!("restored {} blocks", blocks.len());
}

/// Desk check, no radio: the restore guard accepts a backup (CPM_BACKUP).
#[test]
#[ignore = "needs CPM_BACKUP"]
fn hw_d868_check_backup() {
    let blocks = parse_dump(&std::fs::read(env("CPM_BACKUP")).unwrap()).unwrap();
    check_restore_blocks(&blocks).unwrap();
    eprintln!("{} blocks accepted by the D868 restore guard", blocks.len());
}

/// Settings through the SHIPPING reader: print the named keys (CPM_KEYS,
/// comma-separated) as the profile form would receive them. Read-only.
#[test]
#[ignore = "reads a real radio"]
fn hw_d868_settings_read() {
    use crate::radios::driver::SettingsReader;
    let cap = DRIVER.read_settings(&env("CPM_PORT"), "").unwrap();
    for k in env("CPM_KEYS").split(',') {
        eprintln!("{k} = {}", cap.settings[k]);
    }
}
