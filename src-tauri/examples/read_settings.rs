//! Read-ONLY: read the AT-D890UV General Settings region from a connected radio
//! and decode it with the SHIPPING decoder (`decode_anytone_settings`), printing
//! every setting. This is the headless equivalent of the profile editor's
//! "Download from radio" for the D890UV — a ground-truth check that the field
//! table + decoder match the real radio, for spot-checking against the radio's
//! on-screen menu / RT Systems.
//!
//! Usage: cargo run --example read_settings -- <serial-port>

use ww8l_codeplug_magic_lib::commands::anytone::read_windows_raw;
use ww8l_codeplug_magic_lib::commands::anytone_settings::{decode_anytone_settings, SETTINGS_WINDOWS};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 2 {
        eprintln!("usage: cargo run --example read_settings -- <serial-port>");
        std::process::exit(2);
    }
    let port = &args[1];

    let blocks = read_windows_raw(port, SETTINGS_WINDOWS).expect("read failed");
    let settings = decode_anytone_settings(&blocks);
    let obj = settings.as_object().expect("object");
    println!("decoded {} settings:", obj.len());
    for (k, v) in obj {
        println!("  {k:<44} {v}");
    }
}
