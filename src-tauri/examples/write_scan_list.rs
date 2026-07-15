//! Write one scan-list record through the shipping encoder + patch-write path
//! (`encode_scan_list_record` + `run_patch_writes`): backup-before-write,
//! whole-window RMW, single END/commit/reboot. Non-revert settings stay at the
//! CPS defaults (priority scanning off). Verify afterwards with re_settings_diff
//! in a fresh session.
//!
//! Usage: cargo run --example write_scan_list -- <serial-port> <slot> <name> <revert 0-7> <members-csv> <backup.bin>
//! e.g.:  ... -- /dev/cu.usbmodemXXX 0 "NOCO HT" 4 0,1,2,3 backup.bin

use ww8l_codeplug_magic_lib::commands::anytone::{
    encode_scan_list_record, run_patch_writes, RegionPatch, ScanListRecordSettings,
};

// Scan-list region: 250 records of 0x200 bytes (see SCAN_LIST_BASE in anytone.rs).
const SCAN_LIST_BASE: u32 = 0x0210_0000;
const SCAN_LIST_STEP: u32 = 0x0000_0200;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() != 7 {
        eprintln!(
            "usage: cargo run --example write_scan_list -- <serial-port> <slot> <name> <revert 0-7> <members-csv> <backup.bin>"
        );
        std::process::exit(2);
    }
    let port = &args[1];
    let slot: u32 = args[2].parse().expect("slot must be a non-negative integer");
    let name = &args[3];
    let revert: u8 = args[4].parse().expect("revert must be 0-7");
    let members: Vec<u16> = args[5]
        .split(',')
        .filter(|s| !s.is_empty())
        .map(|s| s.trim().parse().expect("members must be 0-based channel slots"))
        .collect();
    let backup = std::path::Path::new(&args[6]);

    let settings = ScanListRecordSettings {
        revert_channel: revert,
        ..Default::default()
    };
    let record = encode_scan_list_record(name, &members, &settings).expect("bad scan list");
    let patches = vec![RegionPatch {
        addr: SCAN_LIST_BASE + slot * SCAN_LIST_STEP,
        data: record,
    }];

    println!(
        "[write] scan list slot {slot} → {name:?}, revert={revert}, {} members (window RMW + backup)…",
        members.len()
    );
    let res = run_patch_writes(port, &patches, backup).expect("patch write failed");
    println!(
        "windows written: {:?}\nbackup: {}\n{}",
        res.windows_written, res.backup_path, res.note
    );
}
