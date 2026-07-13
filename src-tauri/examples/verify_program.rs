//! Read-ONLY: verify a programmed radio against an `.expected.bin` image
//! written by `program_anytone_codeplug`. Parses the expected image's
//! `[addr:u32 BE][len:u32 BE][data]` blocks, strict-reads the SAME ranges from
//! the radio in one PC-mode session, and reports every byte-run that differs.
//!
//! This is the CLI equivalent of the in-app `verify_anytone_program` command,
//! for when the UI never got its result panel (e.g. the program command threw
//! as USB dropped on the commit reboot, so the Verify button never rendered).
//!
//! Usage: cargo run --example verify_program -- <serial-port> <expected.bin>

use ww8l_codeplug_magic_lib::commands::anytone::read_windows_raw;

/// Parse a self-describing `[addr:u32 BE][len:u32 BE][data]` dump.
fn parse_dump(bytes: &[u8]) -> Vec<(u32, Vec<u8>)> {
    let mut out = Vec::new();
    let mut i = 0;
    while i + 8 <= bytes.len() {
        let addr = u32::from_be_bytes([bytes[i], bytes[i + 1], bytes[i + 2], bytes[i + 3]]);
        let len = u32::from_be_bytes([bytes[i + 4], bytes[i + 5], bytes[i + 6], bytes[i + 7]])
            as usize;
        i += 8;
        assert!(i + len <= bytes.len(), "truncated block at 0x{addr:08X}");
        out.push((addr, bytes[i..i + len].to_vec()));
        i += len;
    }
    out
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() != 3 {
        eprintln!("usage: cargo run --example verify_program -- <serial-port> <expected.bin>");
        std::process::exit(2);
    }
    let port = &args[1];
    let expected_bytes = std::fs::read(&args[2]).expect("could not read expected.bin");
    let expected = parse_dump(&expected_bytes);

    let windows: Vec<(u32, u32)> = expected.iter().map(|(a, d)| (*a, d.len() as u32)).collect();
    let actual = read_windows_raw(port, &windows).expect("read failed");

    let mut total = 0usize;
    let mut mismatch_runs = 0usize;
    let mut mismatch_bytes = 0usize;
    for ((addr, exp), (a2, act)) in expected.iter().zip(actual.iter()) {
        assert_eq!(addr, a2, "window order mismatch");
        total += exp.len();
        let n = exp.len().min(act.len());
        let mut j = 0;
        while j < n {
            if exp[j] != act[j] {
                let start = j;
                while j < n && exp[j] != act[j] {
                    j += 1;
                }
                mismatch_runs += 1;
                mismatch_bytes += j - start;
                let e: Vec<String> = exp[start..j].iter().map(|b| format!("{b:02X}")).collect();
                let g: Vec<String> = act[start..j].iter().map(|b| format!("{b:02X}")).collect();
                println!(
                    "  MISMATCH 0x{:08X} ({} b): expected {} → radio {}",
                    addr + start as u32,
                    j - start,
                    e.join(" "),
                    g.join(" ")
                );
            } else {
                j += 1;
            }
        }
        if exp.len() != act.len() {
            println!(
                "  LENGTH 0x{addr:08X}: expected {} bytes, read {}",
                exp.len(),
                act.len()
            );
        }
    }

    println!("\n{total} bytes checked across {} window(s)", expected.len());
    if mismatch_runs == 0 {
        println!("✓ VERIFIED — radio matches the expected image byte-for-byte");
    } else {
        println!("✗ {mismatch_runs} differing run(s), {mismatch_bytes} byte(s) — consider Restore");
        std::process::exit(1);
    }
}
