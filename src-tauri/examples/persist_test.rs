//! Real single-field persistence test: flip the DMR time-slot byte on <slot>,
//! commit (reboot), fresh-read to confirm it STUCK and neighbours are intact, then
//! restore the original bytes and confirm. Fully reversible; backs up first.
//!
//! Usage: cargo run --example persist_test -- <serial-port> <slot>

use std::path::Path;
use std::thread::sleep;
use std::time::Duration;

use ww8l_codeplug_magic_lib::commands::anytone::{
    flip_timeslot_commit, read_record_for_slot, write_record_to_slot,
};

const CH_TIME_SLOT: usize = 0x21; // record offset of the DMR time-slot byte

/// Ride out a commit-reboot: the radio drops off USB, reboots, and re-enumerates.
/// Wait for the port to disappear, then come back, then settle before reopening.
fn wait_for_port(port: &str) {
    use std::io::Write;
    print!("  waiting for radio to reboot");
    let _ = std::io::stdout().flush();
    // Phase 1: let it start rebooting / drop the port.
    for _ in 0..8 {
        if !Path::new(port).exists() {
            break;
        }
        print!(".");
        let _ = std::io::stdout().flush();
        sleep(Duration::from_secs(1));
    }
    // Phase 2: wait for it to come back.
    for _ in 0..40 {
        if Path::new(port).exists() {
            sleep(Duration::from_secs(3)); // settle so the CDC-ACM node is openable
            println!(" — back.");
            return;
        }
        print!(".");
        let _ = std::io::stdout().flush();
        sleep(Duration::from_secs(1));
    }
    println!(" — timed out; trying anyway.");
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() != 3 {
        eprintln!("usage: cargo run --example persist_test -- <serial-port> <slot>");
        std::process::exit(2);
    }
    let port = &args[1];
    let slot: usize = args[2].parse().expect("slot must be a positive integer");

    // 1+2. Read + flip + write + commit, all in ONE session (a mid-flow reboot
    // would break a separate read then write).
    println!("[write] read + flip time slot + commit…");
    let (original, old_ts, new_ts) =
        flip_timeslot_commit(port, slot).expect("flip write failed");
    let backup = std::env::temp_dir().join(format!("anytone-persist-slot{slot}.img"));
    std::fs::write(&backup, &original).expect("backup write failed");
    println!(
        "slot {slot}: time slot 0x{old_ts:02X} (TS{}) -> 0x{new_ts:02X} (TS{})",
        old_ts + 1,
        new_ts + 1
    );
    println!("backup: {}", backup.display());
    wait_for_port(port);

    // 3. Fresh read: did it stick, and is everything else unchanged?
    let after = read_record_for_slot(port, slot).expect("verify read failed");
    let ts_stuck = after[CH_TIME_SLOT] == new_ts;
    let only_ts_changed = original
        .iter()
        .zip(after.iter())
        .enumerate()
        .all(|(i, (o, a))| i == CH_TIME_SLOT || o == a);
    println!(
        "  time-slot byte now 0x{:02X} — {}",
        after[CH_TIME_SLOT],
        if ts_stuck { "STUCK ✅" } else { "did NOT stick ❌" }
    );
    println!(
        "  rest of record: {}",
        if only_ts_changed { "unchanged ✅" } else { "OTHER BYTES CHANGED ❌" }
    );

    // 4. Restore the original bytes and commit.
    println!("\n[restore] writing original bytes back + commit…");
    write_record_to_slot(port, slot, &original).expect("restore write failed");
    wait_for_port(port);

    // 5. Confirm the restore.
    let restored = read_record_for_slot(port, slot).expect("restore-verify read failed");
    let restored_ok = restored == original;
    println!(
        "  restored to original: {}",
        if restored_ok { "YES ✅" } else { "NO ❌ (restore from backup file!)" }
    );

    println!("\n== SUMMARY ==");
    println!("  change persisted across reboot : {}", if ts_stuck { "YES" } else { "no" });
    println!("  neighbours/other bytes intact  : {}", if only_ts_changed { "YES" } else { "NO" });
    println!("  restored cleanly               : {}", if restored_ok { "YES" } else { "NO" });
    if ts_stuck && only_ts_changed && restored_ok {
        println!("\n🎉 Real single-field write PERSISTS and is safe. Programming is unblocked.");
    } else {
        println!("\n⚠️  Something's off — see above; backup at {}", backup.display());
        std::process::exit(1);
    }
}
