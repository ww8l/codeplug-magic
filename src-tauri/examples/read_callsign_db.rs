//! DIAGNOSTIC (read-only): dump the AT-D890UV Call-sign Database as it actually
//! sits on the radio, so we can compare the real on-wire record format against
//! what `encode_callsign_db` produces. Session 36 only ever validated 2 tiny
//! all-ASCII records; a 300k real-world write misaligns after ~16 records, so
//! this reads the bytes back and decodes them to reveal the true layout.
//!
//! Reads three regions in ONE PC-mode session (one END/reboot):
//!   * Limits   @ 0x07000000 (16 bytes): entry count + end-of-DB address.
//!   * Map bank @ 0x07080000 (first N 8-byte entries): key + contact_index.
//!   * DB bank  @ 0x07900000 (first few KB): the variable-length records.
//!
//! Usage:
//!   cargo run --example read_callsign_db -- <serial-port> [db_bytes] [map_entries]
//!   e.g. ... -- /dev/cu.usbmodem... 2048 48

use ww8l_codeplug_magic_lib::commands::anytone::read_windows_raw;

const LIMITS_BASE: u32 = 0x0700_0000;
const MAP_BASE: u32 = 0x0708_0000;
const DB_BASE: u32 = 0x0790_0000;

fn hexdump(base: u32, data: &[u8]) {
    for (i, chunk) in data.chunks(16).enumerate() {
        let addr = base + (i * 16) as u32;
        // Group bytes in pairs so UTF-16LE code units are easy to read.
        let mut grouped = String::new();
        for (j, b) in chunk.iter().enumerate() {
            grouped.push_str(&format!("{b:02X}"));
            if j % 2 == 1 {
                grouped.push(' ');
            }
        }
        let ascii: String = chunk
            .iter()
            .map(|&b| if (0x20..=0x7E).contains(&b) { b as char } else { '.' })
            .collect();
        println!("  {addr:08X}  {:<45}{ascii}", grouped.trim_end());
    }
}

/// Read one UTF-16LE NUL-terminated string starting at `pos`; returns the string
/// and the index just past its 0x0000 terminator. `None` if it runs off the end.
fn read_u16_string(data: &[u8], pos: usize) -> Option<(String, usize)> {
    let mut units = Vec::new();
    let mut i = pos;
    loop {
        if i + 1 >= data.len() {
            return None;
        }
        let u = u16::from_le_bytes([data[i], data[i + 1]]);
        i += 2;
        if u == 0 {
            break;
        }
        units.push(u);
    }
    Some((String::from_utf16_lossy(&units), i))
}

/// Best-effort walk of the DB assuming session 36's model: call_type(1) +
/// flags(1) + dmr_id(4 BE BCD) + 6 NUL-terminated UTF-16LE fields, no pad.
/// Prints each record so we can see exactly where the real format diverges.
fn decode_records(data: &[u8], max_records: usize) {
    let mut pos = 0usize;
    for n in 0..max_records {
        if pos + 6 > data.len() {
            println!("  [record {n}] ran off the end at offset {pos}");
            return;
        }
        let call_type = data[pos];
        let flags = data[pos + 1];
        let bcd = &data[pos + 2..pos + 6];
        // BE BCD -> decimal string.
        let dmr: String = bcd.iter().flat_map(|b| [b >> 4, b & 0x0F]).map(|d| (b'0' + d) as char).collect();
        let mut fpos = pos + 6;
        let labels = ["name", "city", "call", "state", "country", "comment"];
        let mut fields = Vec::new();
        let mut ok = true;
        for _ in 0..6 {
            match read_u16_string(data, fpos) {
                Some((s, next)) => {
                    fields.push(s);
                    fpos = next;
                }
                None => {
                    ok = false;
                    break;
                }
            }
        }
        if !ok {
            println!("  [record {n} @ off {pos}] incomplete (ran off end)");
            return;
        }
        let flen = fpos - pos;
        println!(
            "  [rec {n:>2} @off {pos:>5} len {flen:>3}] type={call_type} flags={flags:02X} dmr={dmr}  {}",
            labels
                .iter()
                .zip(&fields)
                .map(|(l, v)| format!("{l}={v:?}"))
                .collect::<Vec<_>>()
                .join(" ")
        );
        pos = fpos;
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 2 {
        eprintln!("usage: cargo run --example read_callsign_db -- <serial-port> [db_bytes] [map_entries]");
        std::process::exit(2);
    }
    let port = &args[1];
    let db_bytes: u32 = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(2048);
    let map_entries: u32 = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(48);

    const BANK_STEP: u32 = 0x0008_0000;
    let mut windows = vec![(LIMITS_BASE, 16u32), (MAP_BASE, map_entries * 8)];
    // Probe the head of every map bank 0..=11 to map out which banks persisted.
    for k in 0..12u32 {
        windows.push((MAP_BASE + k * BANK_STEP, 8));
    }
    // Probe the head of several DB banks (0,10,20,30,42) to confirm interior DB
    // banks committed, not just the first/last.
    let db_probe_banks = [0u32, 10, 20, 30, 42];
    for &b in &db_probe_banks {
        windows.push((DB_BASE + b * BANK_STEP, 32));
    }
    windows.push((DB_BASE, db_bytes));
    let regions = read_windows_raw(port, &windows).expect("read failed");

    let limits = &regions[0].1;
    let map = &regions[1].1;
    let db = &regions[19].1;

    println!("=== Limits @ 0x{LIMITS_BASE:08X} ===");
    let count = u32::from_le_bytes([limits[0], limits[1], limits[2], limits[3]]);
    let end = u32::from_le_bytes([limits[4], limits[5], limits[6], limits[7]]);
    println!("  entry count      = {count}");
    println!("  end-of-db addr   = 0x{end:08X}");
    hexdump(LIMITS_BASE, limits);

    println!("\n=== Contact Map @ 0x{MAP_BASE:08X} (first {map_entries} entries) ===");
    for i in 0..(map.len() / 8) {
        let key = u32::from_le_bytes([map[i * 8], map[i * 8 + 1], map[i * 8 + 2], map[i * 8 + 3]]);
        let idx = u32::from_le_bytes([map[i * 8 + 4], map[i * 8 + 5], map[i * 8 + 6], map[i * 8 + 7]]);
        // key = (bcd8be(dmr) << 1) | gcf  -> recover dmr for readability
        let bcd8be = key >> 1;
        let dmr: String = bcd8be
            .to_be_bytes()
            .iter()
            .flat_map(|b| [b >> 4, b & 0x0F])
            .map(|d| (b'0' + d) as char)
            .collect();
        if key == 0xFFFF_FFFF && idx == 0xFFFF_FFFF {
            println!("  [map {i:>2}] <empty 0xFFFFFFFF>");
            break;
        }
        println!("  [map {i:>2}] key=0x{key:08X} (dmr {dmr}) contact_index={idx}");
    }

    // Per-bank persistence map: heads of banks 0..=11 (regions index 2..=13).
    println!("\n=== Map bank heads (banks 0..11) ===");
    for k in 0..12usize {
        let region = &regions[2 + k].1;
        let base = u32::from_le_bytes([region[0], region[1], region[2], region[3]]);
        let idx = u32::from_le_bytes([region[4], region[5], region[6], region[7]]);
        let addr = MAP_BASE + (k as u32) * BANK_STEP;
        if base == 0xFFFF_FFFF {
            println!("  bank {k:>2} @0x{addr:08X}: <EMPTY 0xFFFFFFFF>");
        } else {
            let dmr: String = (base >> 1)
                .to_be_bytes()
                .iter()
                .flat_map(|b| [b >> 4, b & 0x0F])
                .map(|d| (b'0' + d) as char)
                .collect();
            println!("  bank {k:>2} @0x{addr:08X}: key=0x{base:08X} (dmr {dmr}) contact_index={idx}");
        }
    }

    // DB bank head probes (regions index 14..=18): first record of banks 0/10/20/30/42.
    println!("\n=== DB bank heads (banks 0/10/20/30/42) ===");
    for (i, &b) in db_probe_banks.iter().enumerate() {
        let region = &regions[14 + i].1;
        let addr = DB_BASE + b * BANK_STEP;
        if region.iter().all(|&x| x == 0xFF) {
            println!("  DB bank {b:>2} @0x{addr:08X}: <EMPTY 0xFF>");
        } else {
            let ct = region[0];
            let bcd = &region[2..6];
            let dmr: String = bcd.iter().flat_map(|b| [b >> 4, b & 0x0F]).map(|d| (b'0' + d) as char).collect();
            let (name, _) = read_u16_string(region, 6).unwrap_or((String::from("?"), 0));
            println!("  DB bank {b:>2} @0x{addr:08X}: type={ct} dmr={dmr} name={name:?}");
        }
    }

    // Hexdump a window around byte offset `hexoff` (arg 4, default 0) so we can
    // inspect a specific record boundary.
    let hexoff: usize = std::env::args().nth(4).and_then(|s| s.parse().ok()).unwrap_or(0);
    let end = (hexoff + 512).min(db.len());
    println!("\n=== DB bank @ 0x{DB_BASE:08X} bytes [{hexoff}..{end}] hexdump ===");
    hexdump(DB_BASE + hexoff as u32, &db[hexoff..end]);

    println!("\n=== DB records decoded (session-36 model: 6 NUL-terminated fields, no pad) ===");
    decode_records(db, 60);
}
