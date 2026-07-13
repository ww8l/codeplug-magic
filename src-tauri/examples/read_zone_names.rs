//! Read-ONLY: dump the D890UV zone-name array and decode each slot, printing
//! only the NON-EMPTY zones with their 1-based zone number. Lets us see exactly
//! how many stale zones exist before a full Program run clears them.
//!
//! Usage: cargo run --example read_zone_names -- <serial-port>

use ww8l_codeplug_magic_lib::commands::anytone::read_windows_raw;

// From the d890uv map (anytone.rs): names at 0x03600000, step 0x40, 16 UTF-16
// chars each, 250 zones. One 0x4000 window covers all of them.
const ZONE_NAME_BASE: u32 = 0x0360_0000;
const ZONE_NAME_STEP: usize = 0x40;
const ZONE_NAME_CHARS: usize = 16;
const MAX_ZONES: usize = 250;
const WINDOW: u32 = 0x4000;

fn decode_utf16le(slot: &[u8], chars: usize) -> String {
    let units: Vec<u16> = slot
        .chunks_exact(2)
        .take(chars)
        .map(|c| u16::from_le_bytes([c[0], c[1]]))
        .take_while(|&u| u != 0x0000 && u != 0xFFFF)
        .collect();
    String::from_utf16_lossy(&units).trim_end().to_string()
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 2 {
        eprintln!("usage: cargo run --example read_zone_names -- <serial-port>");
        std::process::exit(2);
    }
    let port = &args[1];

    let blocks = read_windows_raw(port, &[(ZONE_NAME_BASE, WINDOW)]).expect("read failed");
    let (_, data) = &blocks[0];

    let mut names: Vec<(usize, String)> = Vec::new();
    let mut highest = 0usize; // highest 1-based slot with a non-empty name
    for i in 0..MAX_ZONES {
        let off = i * ZONE_NAME_STEP;
        let name = decode_utf16le(&data[off..off + ZONE_NAME_STEP], ZONE_NAME_CHARS);
        if !name.is_empty() {
            names.push((i + 1, name));
            highest = i + 1;
        }
    }

    println!("Zone-name array @ 0x{ZONE_NAME_BASE:08X} ({} bytes):", data.len());
    println!("  {} non-empty zone(s); highest occupied slot = #{highest}\n", names.len());
    for (num, name) in &names {
        println!("  zone #{num:<3}  {name:?}");
    }
    if names.is_empty() {
        println!("  (all slots empty)");
    }
}
