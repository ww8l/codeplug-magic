//! Read-only verification: fresh-read the channel bank for <slot> and diff it
//! against a saved backup file (byte-for-byte).
//!
//! Usage: cargo run --example verify_bank -- <serial-port> <slot> <backup-file>

use ww8l_codeplug_magic_lib::commands::anytone::read_bank_for_slot;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() != 4 {
        eprintln!("usage: cargo run --example verify_bank -- <serial-port> <slot> <backup-file>");
        std::process::exit(2);
    }
    let port = &args[1];
    let slot: usize = args[2].parse().expect("slot must be a positive integer");
    let backup_file = &args[3];

    let expected = std::fs::read(backup_file).expect("could not read backup file");
    let actual = match read_bank_for_slot(port, slot) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("READ FAILED: {e}");
            std::process::exit(1);
        }
    };

    println!("read {} bytes (expected {} from backup)", actual.len(), expected.len());
    if actual == expected {
        println!("\n✅ BYTE-IDENTICAL — the committed no-op left the bank exactly as it was.");
        return;
    }

    // Report the differing byte-runs at their real radio addresses.
    let base: u32 = 0x0100_0000 + ((slot as u32 - 1) / 128) * 0x0008_0000;
    let n = actual.len().min(expected.len());
    let mut diffs = 0;
    let mut i = 0;
    println!("\n❌ DIFFERENCES:");
    while i < n {
        if actual[i] == expected[i] {
            i += 1;
            continue;
        }
        let start = i;
        while i < n && actual[i] != expected[i] {
            i += 1;
        }
        let a: Vec<String> = expected[start..i].iter().map(|b| format!("{b:02X}")).collect();
        let b: Vec<String> = actual[start..i].iter().map(|b| format!("{b:02X}")).collect();
        println!("  0x{:08X} ({} bytes)  was [{}]  now [{}]", base + start as u32, i - start, a.join(" "), b.join(" "));
        diffs += 1;
        if diffs >= 40 {
            println!("  … (truncated)");
            break;
        }
    }
    std::process::exit(1);
}
