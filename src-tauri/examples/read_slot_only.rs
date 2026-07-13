//! Read-ONLY: fresh-read one 128-byte record for <slot> and print the time-slot
//! byte. Optionally diff against a backup .img to confirm a full restore.
//!
//! Usage: cargo run --example read_slot_only -- <serial-port> <slot> [backup.img]

use ww8l_codeplug_magic_lib::commands::anytone::read_record_for_slot;

const CH_TIME_SLOT: usize = 0x21;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 3 || args.len() > 4 {
        eprintln!("usage: cargo run --example read_slot_only -- <serial-port> <slot> [backup.img]");
        std::process::exit(2);
    }
    let port = &args[1];
    let slot: usize = args[2].parse().expect("slot must be a positive integer");

    let rec = read_record_for_slot(port, slot).expect("read failed");
    let ts = rec[CH_TIME_SLOT];
    println!("slot {slot}: time-slot byte 0x{ts:02X} (TS{})", ts + 1);

    if let Some(backup_path) = args.get(3) {
        let original = std::fs::read(backup_path).expect("could not read backup file");
        if rec == original {
            println!("✅ full 128-byte record BYTE-IDENTICAL to backup — restore confirmed.");
        } else {
            let diffs: Vec<usize> = original
                .iter()
                .zip(rec.iter())
                .enumerate()
                .filter(|(_, (o, a))| o != a)
                .map(|(i, _)| i)
                .collect();
            println!("❌ differs from backup at offsets: {diffs:02X?}");
            std::process::exit(1);
        }
    }
}
