//! HW runner for the committing whole-bank NO-OP test.
//!
//! Usage: cargo run --example noop_bank -- <serial-port> <slot>
//!
//! Drives the SHIPPING `run_noop_bank_test` (same code path as the
//! `commit_noop_bank_anytone` Tauri command) so there is no duplicated
//! brick-capable write logic. Reads the whole channel bank containing <slot>,
//! backs it up, writes the identical bytes back, and ends the session (the radio
//! reboots to commit). Nothing should change — this proves the bank-granularity
//! write transport is safe.

use ww8l_codeplug_magic_lib::commands::anytone::run_noop_bank_test;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() != 3 {
        eprintln!("usage: cargo run --example noop_bank -- <serial-port> <slot>");
        std::process::exit(2);
    }
    let port = &args[1];
    let slot: usize = args[2].parse().expect("slot must be a positive integer");

    let backup_path =
        std::env::temp_dir().join(format!("anytone-noop-bank-slot{slot}.img"));

    println!("== AnyTone whole-bank NO-OP test ==");
    println!("port : {port}");
    println!("slot : {slot}");
    println!("backup -> {}", backup_path.display());
    println!("(radio will reboot to commit when this finishes)\n");

    match run_noop_bank_test(port, slot, &backup_path) {
        Ok(r) => {
            println!("OK — bank {} ({} bytes) rewritten with its own bytes.", r.bank_addr, r.bank_len);
            println!("bank preview: {}", r.preview);
            println!("backup saved: {}", r.backup_path);
            println!(
                "\nNext: power-cycle/rescan the radio, then do a full download and\n\
                 diff it against the pre-test backup — expect BYTE-IDENTICAL and NO\n\
                 \"please initialize\" error."
            );
        }
        Err(e) => {
            eprintln!("FAILED: {e}");
            std::process::exit(1);
        }
    }
}
