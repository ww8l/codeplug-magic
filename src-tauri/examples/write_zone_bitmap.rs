//! Write the D890UV zone-present bitmap at 0x03482C00 (32 bytes, 1 bit/zone,
//! LSB = zone 1). Sets bits 0..N for N zones, clears the rest — activating the
//! real zones and clearing any stale ones. Uses the shipping patch-write path
//! (backup-before-write, whole-window RMW, single END/commit/reboot).
//!
//! Usage: cargo run --example write_zone_bitmap -- <serial-port> <num-zones> <backup.bin>

use ww8l_codeplug_magic_lib::commands::anytone::{run_patch_writes, RegionPatch};

const ZONE_BITMAP_BASE: u32 = 0x0348_2C00;
const ZONE_BITMAP_BYTES: usize = 32; // 250 zones → ceil(250/8) = 32 bytes

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() != 4 {
        eprintln!("usage: cargo run --example write_zone_bitmap -- <serial-port> <num-zones> <backup.bin>");
        std::process::exit(2);
    }
    let port = &args[1];
    let n: usize = args[2].parse().expect("num-zones must be a positive integer");
    let backup = std::path::Path::new(&args[3]);

    let mut bitmap = vec![0u8; ZONE_BITMAP_BYTES];
    for z in 0..n {
        bitmap[z / 8] |= 1 << (z % 8);
    }
    let hex: Vec<String> = bitmap.iter().take(4).map(|b| format!("{b:02X}")).collect();
    println!("[write] zone bitmap @ 0x{ZONE_BITMAP_BASE:08X} for {n} zone(s) → {} ...", hex.join(" "));

    let patches = vec![RegionPatch { addr: ZONE_BITMAP_BASE, data: bitmap }];
    let res = run_patch_writes(port, &patches, backup).expect("patch write failed");
    println!(
        "windows written: {:?}\nbackup: {}\n{}",
        res.windows_written, res.backup_path, res.note
    );
}
