//! SAFE first write test for a new region: read one 0x4000-aligned window, back
//! it up, write the IDENTICAL bytes back, END (commit + reboot). Nothing changes
//! even if the flash sector erases — proves window writes in this region are
//! collateral-free before any real edit. Verify afterwards with read_windows
//! (fresh session) against the saved backup.
//!
//! Usage: cargo run --example noop_window -- <serial-port> <base-addr-hex> <backup.img>

use ww8l_codeplug_magic_lib::commands::anytone::run_noop_window;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() != 4 {
        eprintln!("usage: cargo run --example noop_window -- <serial-port> <base-addr-hex> <backup.img>");
        std::process::exit(2);
    }
    let port = &args[1];
    let base = u32::from_str_radix(args[2].trim_start_matches("0x"), 16).expect("bad hex addr");
    let backup = &args[3];

    println!("[noop] window 0x{base:08X}: read → backup → write same bytes → END…");
    let original = run_noop_window(port, base).expect("noop window failed");
    std::fs::write(backup, &original).expect("backup write failed");
    println!(
        "wrote {} bytes back unchanged; backup: {backup}\nradio is rebooting to commit — verify with read_windows in a fresh session.",
        original.len()
    );
}
