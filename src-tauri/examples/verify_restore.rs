//! Recovery/continuation for persist_test when the post-reboot re-open failed:
//! fresh-read <slot> to confirm whether a prior time-slot flip PERSISTED, then
//! restore the original 128-byte record from a backup .img and confirm.
//!
//! Usage: cargo run --example verify_restore -- <serial-port> <slot> <backup.img>

use ww8l_codeplug_magic_lib::commands::anytone::{read_record_for_slot, write_record_to_slot};

const CH_TIME_SLOT: usize = 0x21;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() != 4 {
        eprintln!("usage: cargo run --example verify_restore -- <serial-port> <slot> <backup.img>");
        std::process::exit(2);
    }
    let port = &args[1];
    let slot: usize = args[2].parse().expect("slot must be a positive integer");
    let backup_path = &args[3];

    let original = std::fs::read(backup_path).expect("could not read backup file");
    assert_eq!(original.len(), 128, "backup must be a 128-byte record");
    let orig_ts = original[CH_TIME_SLOT];

    // 1. Fresh read — did the flip stick, and are the other bytes intact?
    println!("[verify] fresh-reading slot {slot}…");
    let after = read_record_for_slot(port, slot).expect("verify read failed");
    let flipped_expected = 1 - orig_ts;
    let ts_stuck = after[CH_TIME_SLOT] == flipped_expected;
    let only_ts_changed = original
        .iter()
        .zip(after.iter())
        .enumerate()
        .all(|(i, (o, a))| i == CH_TIME_SLOT || o == a);
    println!(
        "  original time slot 0x{orig_ts:02X} (TS{}), now 0x{:02X} (TS{}) — {}",
        orig_ts + 1,
        after[CH_TIME_SLOT],
        after[CH_TIME_SLOT] + 1,
        if ts_stuck { "PERSISTED ✅" } else { "did NOT persist ❌" }
    );
    println!(
        "  rest of record: {}",
        if only_ts_changed { "unchanged ✅" } else { "OTHER BYTES CHANGED ❌" }
    );

    // 2. Restore the original bytes and commit.
    println!("\n[restore] writing original bytes back + commit (radio will reboot)…");
    write_record_to_slot(port, slot, &original).expect("restore write failed");
    println!("  restore write sent + committed. Power-cycle, then re-run this to CONFIRM the restore.");

    println!("\n== SUMMARY ==");
    println!("  flip persisted across reboot : {}", if ts_stuck { "YES" } else { "no" });
    println!("  neighbours/other bytes intact: {}", if only_ts_changed { "YES" } else { "NO" });
    if ts_stuck && only_ts_changed {
        println!("\n🎉 Real single-field write PERSISTS and is byte-safe. Direct programming is unblocked.");
    }
}
