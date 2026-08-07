//! BRICK-CAPABLE write test for the AT-D890UV **Call-sign Database** (caller-ID)
//! path. Encodes a small batch of DMR-ID → callsign records with
//! `encode_callsign_db` and pushes them through the shipping `run_patch_writes`
//! machinery (plan windows → backup ORIGINAL windows BEFORE any write → whole-
//! 0x4000-window RMW → single END/commit + reboot).
//!
//! SINGLE PC-mode session (one op per process: END reboots + re-enumerates USB).
//!
//! Verification is FUNCTIONAL, not byte-level: this unit ships empty and we
//! never populate it via AnyTone's CPS, so there is no golden image to diff.
//! After the write + reboot, confirm on the RADIO'S OWN caller-ID screen that a
//! programmed DMR ID resolves to its callsign/name (e.g. receive/monitor that
//! ID, or look it up in the radio's digital-monitor / contact display). Choose
//! DMR IDs you can actually make appear on the radio (your hotspot/repeater ID).
//!
//! Usage:
//!   cargo run --example write_callsign_db_test -- <serial-port> <dmr_id:name:call> [more...]
//!   e.g. ... -- /dev/cu.usbmodem... 3112345:Tim:W0CPH 1234567:Bob:K0BOB

use std::path::PathBuf;

use ww8l_codeplug_magic_lib::commands::anytone::run_patch_writes;
use ww8l_codeplug_magic_lib::commands::anytone_callsign_db::{encode_callsign_db, CallsignDbEntry};

fn parse_entry(s: &str) -> CallsignDbEntry {
    let mut parts = s.splitn(3, ':');
    let id: u32 = parts
        .next()
        .and_then(|v| v.parse().ok())
        .unwrap_or_else(|| panic!("bad dmr_id in {s:?}"));
    let name = parts.next().unwrap_or("").to_string();
    let call = parts.next().unwrap_or("").to_string();
    CallsignDbEntry {
        dmr_id: id,
        group_call: false,
        name,
        call,
        city: String::new(),
        state: String::new(),
        country: String::new(),
        comment: String::new(),
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 3 {
        eprintln!(
            "usage: cargo run --example write_callsign_db_test -- <serial-port> <dmr_id:name:call> [more...]"
        );
        std::process::exit(2);
    }
    let port = &args[1];
    let entries: Vec<CallsignDbEntry> = args[2..].iter().map(|s| parse_entry(s)).collect();

    let patches = encode_callsign_db(&entries).expect("encode failed");
    println!("encoded {} entries into {} region patch(es):", entries.len(), patches.len());
    for p in &patches {
        println!("  0x{:08X}  {} bytes", p.addr, p.data.len());
    }

    let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
    let backup_path = PathBuf::from("/tmp").join(format!("anytone-callsigndb-test-{stamp}.bin"));

    let result = run_patch_writes(port, &patches, &backup_path).expect("call-sign DB write failed");

    println!("\nwindows_written: {:?}", result.windows_written);
    println!("backup: {}", result.backup_path);
    println!("\n{}", result.note);
    println!(
        "\nVERIFY FUNCTIONALLY on the radio's caller-ID screen (no byte round-trip \
         for this region). Restore by writing the empty region back from the backup."
    );
}
