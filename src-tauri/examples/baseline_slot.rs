//! Read-ONLY: fresh-read one 128-byte record for <slot>, hexdump it, and save the
//! raw bytes to <out.img> as a ground-truth baseline for later byte-identical
//! comparison (e.g. around a UI-driven encoder write + restore).
//!
//! Usage: cargo run --example baseline_slot -- <serial-port> <slot> <out.img>

use ww8l_codeplug_magic_lib::commands::anytone::read_record_for_slot;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() != 4 {
        eprintln!("usage: cargo run --example baseline_slot -- <serial-port> <slot> <out.img>");
        std::process::exit(2);
    }
    let port = &args[1];
    let slot: usize = args[2].parse().expect("slot must be a positive integer");
    let out = &args[3];

    let rec = read_record_for_slot(port, slot).expect("read failed");
    std::fs::write(out, &rec).expect("could not save baseline");

    println!("slot {slot} — {} bytes saved to {out}", rec.len());
    for (i, chunk) in rec.chunks(16).enumerate() {
        let hex: Vec<String> = chunk.iter().map(|b| format!("{b:02X}")).collect();
        let ascii: String = chunk
            .iter()
            .map(|&b| if (0x20..0x7F).contains(&b) { b as char } else { '.' })
            .collect();
        println!("  {:02X}: {}  |{}|", i * 16, hex.join(" "), ascii);
    }
}
