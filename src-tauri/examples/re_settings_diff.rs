//! Reverse-engineering helper for pinning DOWN unknown D890UV settings offsets
//! (e.g. the GPS/Ranging flags qdmr never modeled: Get GPS Positioning, GPS
//! Roaming, GPS Mode, GPS Template Information).
//!
//! Workflow (the proven differential-dump method):
//!   1. cargo run --example re_settings_diff -- dump <port> base.bin
//!        - reads the settings windows from the radio, saves a raw snapshot.
//!   2. On the radio / in RT Systems, toggle EXACTLY ONE setting, write it back
//!      to the radio, then:
//!        cargo run --example re_settings_diff -- dump <port> after.bin
//!   3. cargo run --example re_settings_diff -- diff base.bin after.bin
//!        - prints every byte that changed: region, absolute address, offset
//!          within the region (the `byte:` value for the SF table), and old→new.
//!      With a single setting toggled, the changed byte IS that setting's field.
//!
//! Snapshot file format (self-describing, multi-window):
//!   repeat per window: [u32 BE base][u32 BE len][len bytes data]
//! This matches the golden-settings.bin convention (base, len, data) but allows
//! more than one window.

use std::io::{Read, Write};

use ww8l_codeplug_magic_lib::commands::anytone::read_windows_raw;
use ww8l_codeplug_magic_lib::commands::anytone_settings::SETTINGS_WINDOWS;

fn usage() -> ! {
    eprintln!(
        "usage:\n  \
         re_settings_diff dump <serial-port> <out.bin>\n  \
         re_settings_diff diff <a.bin> <b.bin>"
    );
    std::process::exit(2);
}

fn write_snapshot(path: &str, blocks: &[(u32, Vec<u8>)]) -> std::io::Result<()> {
    let mut f = std::fs::File::create(path)?;
    for (base, data) in blocks {
        f.write_all(&base.to_be_bytes())?;
        f.write_all(&(data.len() as u32).to_be_bytes())?;
        f.write_all(data)?;
    }
    Ok(())
}

fn read_snapshot(path: &str) -> std::io::Result<Vec<(u32, Vec<u8>)>> {
    let mut raw = Vec::new();
    std::fs::File::open(path)?.read_to_end(&mut raw)?;
    let mut out = Vec::new();
    let mut i = 0usize;
    while i + 8 <= raw.len() {
        let base = u32::from_be_bytes([raw[i], raw[i + 1], raw[i + 2], raw[i + 3]]);
        let len = u32::from_be_bytes([raw[i + 4], raw[i + 5], raw[i + 6], raw[i + 7]]) as usize;
        i += 8;
        if i + len > raw.len() {
            break;
        }
        out.push((base, raw[i..i + len].to_vec()));
        i += len;
    }
    out
        .is_empty()
        .then(|| eprintln!("warning: {path} contained no windows"));
    Ok(out)
}

fn cmd_dump(port: &str, out: &str) {
    let blocks = read_windows_raw(port, SETTINGS_WINDOWS).expect("read failed");
    for (base, data) in &blocks {
        println!("read region 0x{base:08X} ({} bytes)", data.len());
    }
    write_snapshot(out, &blocks).expect("write snapshot failed");
    println!("wrote snapshot -> {out}");
}

fn cmd_diff(a: &str, b: &str) {
    let sa = read_snapshot(a).expect("read a failed");
    let sb = read_snapshot(b).expect("read b failed");
    let mut changes = 0;
    for (base, da) in &sa {
        let Some((_, db)) = sb.iter().find(|(bb, _)| bb == base) else {
            println!("region 0x{base:08X}: present in {a} but not {b}");
            continue;
        };
        let n = da.len().min(db.len());
        for off in 0..n {
            if da[off] != db[off] {
                changes += 1;
                println!(
                    "region 0x{base:08X}  addr 0x{:08X}  byte 0x{off:03X}  {:3} (0x{:02X}) -> {:3} (0x{:02X})",
                    base + off as u32,
                    da[off],
                    da[off],
                    db[off],
                    db[off],
                );
            }
        }
        if da.len() != db.len() {
            println!(
                "region 0x{base:08X}: length differs ({} vs {})",
                da.len(),
                db.len()
            );
        }
    }
    if changes == 0 {
        println!("no byte differences — did the setting actually write back to the radio?");
    } else {
        println!("\n{changes} byte(s) changed. With one setting toggled, that IS the field.");
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(String::as_str) {
        Some("dump") if args.len() == 4 => cmd_dump(&args[2], &args[3]),
        Some("diff") if args.len() == 4 => cmd_diff(&args[2], &args[3]),
        _ => usage(),
    }
}
