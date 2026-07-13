//! Read-ONLY: read several arbitrary address windows in ONE PC-mode session,
//! hexdump each, and save them all to one self-describing `[addr][len][data]`
//! dump (the same format `diff_anytone_dumps` / the write backups use).
//!
//! Usage: cargo run --example read_windows -- <serial-port> <out.bin> <addr:len> [addr:len ...]
//!        (addr/len in hex, e.g. 0x02000000:0x200)

use ww8l_codeplug_magic_lib::commands::anytone::read_windows_raw;

fn parse_hex(s: &str) -> u32 {
    let s = s.trim_start_matches("0x").trim_start_matches("0X");
    u32::from_str_radix(s, 16).expect("bad hex value")
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 4 {
        eprintln!(
            "usage: cargo run --example read_windows -- <serial-port> <out.bin> <addr:len> [addr:len ...]"
        );
        std::process::exit(2);
    }
    let port = &args[1];
    let out = &args[2];
    let windows: Vec<(u32, u32)> = args[3..]
        .iter()
        .map(|w| {
            let (a, l) = w.split_once(':').expect("window must be addr:len");
            (parse_hex(a), parse_hex(l))
        })
        .collect();

    let blocks = read_windows_raw(port, &windows).expect("read failed");

    let mut dump = Vec::new();
    for (addr, data) in &blocks {
        println!("== 0x{addr:08X} ({} bytes) ==", data.len());
        for (i, chunk) in data.chunks(16).enumerate() {
            let hex: Vec<String> = chunk.iter().map(|b| format!("{b:02X}")).collect();
            let ascii: String = chunk
                .iter()
                .map(|&b| if (0x20..0x7F).contains(&b) { b as char } else { '.' })
                .collect();
            println!("  {:04X}: {:<47}  |{}|", i * 16, hex.join(" "), ascii);
        }
        dump.extend_from_slice(&addr.to_be_bytes());
        dump.extend_from_slice(&(data.len() as u32).to_be_bytes());
        dump.extend_from_slice(data);
    }
    std::fs::write(out, &dump).expect("could not save dump");
    println!("saved {} window(s) to {out}", blocks.len());
}
