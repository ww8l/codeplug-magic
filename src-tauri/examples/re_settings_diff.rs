//! Reverse-engineering helper for pinning DOWN unknown D890UV settings offsets
//! (e.g. the GPS/Ranging flags qdmr never modeled: Get GPS Positioning, GPS
//! Roaming, GPS Mode, GPS Template Information).
//!
//! Workflow (the proven differential-dump method): snapshot the radio, change
//! ONE setting on its front panel, snapshot again, diff. The byte that moved IS
//! that setting's field, and its new value IS that menu choice's raw encoding.
//!
//! ```text
//! cargo run --example re_settings_diff -- dump <port> base.bin
//!     ... change EXACTLY ONE setting on the radio's front panel ...
//! cargo run --example re_settings_diff -- dump <port> after.bin
//! cargo run --example re_settings_diff -- diff base.bin after.bin
//! ```
//!
//! `diff` prints every changed byte: region, absolute address, offset within the
//! region (the `byte:` value for the SF table), and old -> new.
//!
//! The front panel is deliberately the only source and oracle here: this project
//! never depends on the vendor's CPS, not even as a one-time RE oracle.
//!
//! Regions other than the settings struct are probed by passing explicit
//! `<base>:<len>` specs -- e.g. the scan-list records (base 0x02100000, stride
//! 0x200 per list, so record N's field at offset X is at N*0x200 + X):
//!
//! ```text
//! cargo run --example re_settings_diff -- dump <port> base.bin 0x02100000:0x800
//! ```
//!
//! NB each run enters PC mode and ends with END, which REBOOTS the radio and
//! re-enumerates USB -- so one dump per process, and let the port settle between
//! runs.
//!
//! Snapshot file format (self-describing, multi-window): per window, a u32 BE
//! base, then a u32 BE length, then that many bytes of data. This matches the
//! golden-settings.bin convention but allows more than one window.

use std::io::{Read, Write};

use ww8l_codeplug_magic_lib::commands::anytone::read_windows_raw;
use ww8l_codeplug_magic_lib::commands::anytone_settings::SETTINGS_WINDOWS;

fn usage() -> ! {
    eprintln!(
        "usage:\n  \
         re_settings_diff dump <serial-port> <out.bin> [<base>:<len> ...]\n  \
         re_settings_diff diff <a.bin> <b.bin>\n  \
         re_settings_diff show <snap.bin> [<offset> [<len>]]\n\
         \n\
         With no region given, dump reads SETTINGS_WINDOWS (the 0x03500000\n\
         settings struct + radio-ID regions). Pass explicit regions to probe\n\
         anything else, e.g. the scan lists:\n  \
         re_settings_diff dump /dev/cu.usbmodem1101 base.bin 0x02100000:0x800"
    );
    std::process::exit(2);
}

/// Parse a `<base>:<len>` region spec; both accept 0x-prefixed hex or decimal.
fn parse_region(spec: &str) -> Result<(u32, u32), String> {
    let (b, l) = spec
        .split_once(':')
        .ok_or_else(|| format!("region {spec:?} is not <base>:<len>"))?;
    let num = |s: &str| -> Result<u32, String> {
        let s = s.trim();
        match s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
            Some(h) => u32::from_str_radix(h, 16),
            None => s.parse(),
        }
        .map_err(|e| format!("bad number {s:?}: {e}"))
    };
    Ok((num(b)?, num(l)?))
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

fn cmd_dump(port: &str, out: &str, regions: &[String]) {
    let windows: Vec<(u32, u32)> = if regions.is_empty() {
        SETTINGS_WINDOWS.to_vec()
    } else {
        regions
            .iter()
            .map(|s| parse_region(s).unwrap_or_else(|e| { eprintln!("{e}"); usage() }))
            .collect()
    };
    for &(base, len) in &windows {
        println!("reading region 0x{base:08X}+0x{len:X}");
    }
    let blocks = read_windows_raw(port, &windows).expect("read failed");
    for (base, data) in &blocks {
        println!("read region 0x{base:08X} ({} bytes)", data.len());
    }
    write_snapshot(out, &blocks).expect("write snapshot failed");
    println!("wrote snapshot -> {out}");
}

/// Hex-dump a snapshot, or just the `len` bytes at `off` within each region.
/// Reading a known byte here and comparing it to what the radio's screen shows
/// pins that field's encoding without changing anything.
fn cmd_show(path: &str, off: Option<usize>, len: usize) {
    let snap = read_snapshot(path).expect("read snapshot failed");
    for (base, data) in &snap {
        match off {
            None => {
                println!("region 0x{base:08X} ({} bytes)", data.len());
                hex_dump(*base, data, 0, data.len());
            }
            Some(o) if o < data.len() => {
                let end = (o + len).min(data.len());
                println!(
                    "region 0x{base:08X}  offset 0x{o:03X}..0x{:03X}",
                    end.saturating_sub(1)
                );
                hex_dump(*base, data, o, end - o);
            }
            Some(o) => println!("region 0x{base:08X}: offset 0x{o:03X} past end ({} bytes)", data.len()),
        }
    }
}

/// 16-bytes-per-line hex, labelled with both absolute address and region offset.
fn hex_dump(base: u32, data: &[u8], from: usize, count: usize) {
    let end = (from + count).min(data.len());
    let mut i = from - (from % 16);
    while i < end {
        let row: Vec<String> = (i..i + 16)
            .map(|j| {
                if j >= from && j < end && j < data.len() {
                    format!("{:02X}", data[j])
                } else {
                    "  ".to_string()
                }
            })
            .collect();
        println!(
            "  0x{:08X}  +0x{i:03X}  {}",
            base + i as u32,
            row.join(" ")
        );
        i += 16;
    }
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
        Some("dump") if args.len() >= 4 => cmd_dump(&args[2], &args[3], &args[4..]),
        Some("diff") if args.len() == 4 => cmd_diff(&args[2], &args[3]),
        Some("show") if args.len() == 3 => cmd_show(&args[2], None, 0),
        Some("show") if args.len() == 4 || args.len() == 5 => {
            let num = |s: &String| -> usize {
                let s = s.trim();
                match s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
                    Some(h) => usize::from_str_radix(h, 16).ok(),
                    None => s.parse().ok(),
                }
                .unwrap_or_else(|| { eprintln!("bad number {s:?}"); usage() })
            };
            let off = num(&args[3]);
            let len = args.get(4).map(num).unwrap_or(1);
            cmd_show(&args[2], Some(off), len);
        }
        _ => usage(),
    }
}
