//! BRICK-CAPABLE write test for the AT-D890UV settings path. Changes ONE field
//! to a given value and pushes it through the EXACT shipping write path
//! (`run_settings_program`: reads the settings window fresh in-session, encodes,
//! writes backup + expected image BEFORE any write, whole-0x4000-window RMW,
//! single END/commit).
//!
//! SINGLE PC-mode session: it passes only the one key (the encoder skips every
//! absent field), so it does NOT pre-read — that would reboot the radio and
//! break the write session (one op per process). The radio reboots +
//! re-enumerates USB on commit — verify in a FRESH process (`verify_program`
//! against the printed .expected.bin), and restore by re-running with the
//! original value.
//!
//! Usage: cargo run --example write_settings_test -- <serial-port> <key> <value>
//!   e.g. cargo run --example write_settings_test -- /dev/cu.usbmodem... squelch-level-vfo-a 3

use std::path::PathBuf;

use ww8l_codeplug_magic_lib::commands::anytone_program::run_settings_program;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 4 {
        eprintln!("usage: cargo run --example write_settings_test -- <serial-port> <key> <value>");
        std::process::exit(2);
    }
    let (port, key, raw) = (&args[1], &args[2], &args[3]);

    // Coerce the CLI string into decode's value shape (bool / number / string).
    let value = match raw.as_str() {
        "true" => serde_json::Value::Bool(true),
        "false" => serde_json::Value::Bool(false),
        s => match s.parse::<u64>() {
            Ok(n) => serde_json::Value::from(n),
            Err(_) => serde_json::Value::from(s.to_string()),
        },
    };
    // Minimal values object — only this one field is written; every other
    // setting is left exactly as the radio holds it (encoder skips absent keys).
    let mut values = serde_json::Map::new();
    values.insert(key.clone(), value.clone());
    let values = serde_json::Value::Object(values);
    println!("writing {key} = {value} (single PC-mode session)");

    let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
    let dir = PathBuf::from("/tmp");
    let backup_path = dir.join(format!("anytone-settings-test-{stamp}.bin"));
    let expected_path = dir.join(format!("anytone-settings-test-{stamp}.expected.bin"));

    let result = run_settings_program(port, &values, &backup_path, &expected_path)
        .expect("settings write failed");

    println!("\nfields_changed: {}", result.fields_changed);
    println!("windows_written: {:?}", result.windows_written);
    println!("backup:   {}", result.backup_path);
    println!("expected: {}", result.expected_path);
    println!("\n{}", result.note);
    println!(
        "\nVERIFY (fresh process, after reboot + rescan):\n  cargo run --example verify_program -- <port> {}",
        result.expected_path
    );
}
