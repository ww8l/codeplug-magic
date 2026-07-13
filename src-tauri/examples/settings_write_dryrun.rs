//! Read-ONLY dry run for the AT-D890UV settings WRITE path: read the current
//! settings regions from a connected radio, then exercise the SHIPPING encoder
//! (`encode_anytone_settings`) against those real bytes WITHOUT writing anything.
//!
//! Two checks:
//!   1. Live idempotence — re-encoding the radio's own decoded settings must
//!      produce ZERO patches (the encoder is a faithful inverse of the decoder
//!      on THIS radio, not just the golden fixture).
//!   2. Edit preview — apply one field change to the decoded values and print the
//!      exact byte patch(es) the write path would send. This is the pre-flight
//!      you run before trusting the real (brick-capable) write.
//!
//! Usage: cargo run --example settings_write_dryrun -- <serial-port> [key value]
//!   e.g. cargo run --example settings_write_dryrun -- /dev/cu.usbmodem1234 enable-gps false

use ww8l_codeplug_magic_lib::commands::anytone::read_windows_raw;
use ww8l_codeplug_magic_lib::commands::anytone_settings::{
    decode_anytone_settings, encode_anytone_settings, SETTINGS_WINDOWS,
};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 2 {
        eprintln!(
            "usage: cargo run --example settings_write_dryrun -- <serial-port> [key value]"
        );
        std::process::exit(2);
    }
    let port = &args[1];

    let blocks = read_windows_raw(port, SETTINGS_WINDOWS).expect("read failed");
    let decoded = decode_anytone_settings(&blocks);

    // 1. Live idempotence: re-encode the radio's own values → expect no patches.
    let noop = encode_anytone_settings(&blocks, &decoded).expect("encode failed");
    if noop.is_empty() {
        println!("idempotence OK — re-encoding the radio's current settings changes 0 bytes");
    } else {
        println!(
            "WARNING: re-encoding current settings would change {} run(s) — encoder is NOT \
             a faithful inverse on this radio:",
            noop.len()
        );
        for p in &noop {
            println!("  0x{:08X}  {:02X?}", p.addr, p.data);
        }
    }

    // 2. Optional edit preview: apply one `key value` change and show the patch.
    if args.len() >= 4 {
        let (key, raw) = (&args[2], &args[3]);
        let mut edited = decoded.clone();
        let obj = edited.as_object_mut().expect("object");
        // Coerce the CLI string into the shape decode uses: bool / number / string.
        let value = match raw.as_str() {
            "true" => serde_json::Value::Bool(true),
            "false" => serde_json::Value::Bool(false),
            s => match s.parse::<u64>() {
                Ok(n) => serde_json::Value::from(n),
                Err(_) => serde_json::Value::from(s.to_string()),
            },
        };
        if !obj.contains_key(key) {
            eprintln!("note: '{key}' is not a known settings key — nothing will change");
        }
        obj.insert(key.clone(), value.clone());
        let patches = encode_anytone_settings(&blocks, &edited).expect("encode failed");
        println!("\nsetting {key} = {value} would write {} patch(es):", patches.len());
        for p in &patches {
            println!("  0x{:08X}  {:02X?}", p.addr, p.data);
        }
        println!("(dry run — nothing was written to the radio)");
    }
}
