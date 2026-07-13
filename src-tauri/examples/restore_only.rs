//! Write-ONLY restore: write a backed-up 128-byte record back to <slot> and commit.
//! A single op per process — the D890UV reboots on END, so restore must not share a
//! process with a prior read (the post-END re-enumerate breaks the next open).
//!
//! Usage: cargo run --example restore_only -- <serial-port> <slot> <backup.img>

use ww8l_codeplug_magic_lib::commands::anytone::write_record_to_slot;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() != 4 {
        eprintln!("usage: cargo run --example restore_only -- <serial-port> <slot> <backup.img>");
        std::process::exit(2);
    }
    let port = &args[1];
    let slot: usize = args[2].parse().expect("slot must be a positive integer");
    let backup_path = &args[3];

    let original = std::fs::read(backup_path).expect("could not read backup file");
    assert_eq!(original.len(), 128, "backup must be a 128-byte record");

    println!("[restore] writing original bytes back to slot {slot} + commit…");
    write_record_to_slot(port, slot, &original).expect("restore write failed");
    println!("  restore write sent + committed (radio reboots). Power-cycle, then");
    println!("  run `verify_bank`/a fresh read to CONFIRM slot {slot} is back to TS2.");
}
