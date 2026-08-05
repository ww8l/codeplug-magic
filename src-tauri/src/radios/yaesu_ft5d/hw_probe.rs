//! THROWAWAY reverse-engineering harness for the FT5D clone port (issue #32).
//!
//! Delete this file and its `mod` line in `mod.rs` once the clone protocol is
//! settled — it exists to *learn* the protocol, not to be part of the driver.
//! See the `hw-test-harness-pattern` note: one radio operation per process,
//! `#[ignore]`d so `cargo test` stays hardware-free.
//!
//! ## Running it
//!
//! Radio off → SCU cable into the DATA jack → hold [DISP] while powering on
//! until "CLONE" shows. Then, from `src-tauri/`:
//!
//! ```text
//! # Pass 1 — ident only. Never acks, so the radio sends its first block and
//! # stops. Nothing is read out of the radio beyond the ident itself.
//! cargo test --lib ft5d_clone_probe -- --ignored --nocapture
//!
//! # Pass 2 — full image. Acks each block the way CHIRP's yaesu_clone does and
//! # drains until the radio goes quiet. Still read-only: the radio is the
//! # sender throughout, and 0x06 is the only byte we ever transmit.
//! FT5D_ACK=1 cargo test --lib ft5d_clone_probe -- --ignored --nocapture
//! ```
//!
//! Press [Send] on the radio once the test prints "waiting". Override the port
//! with `FT5D_PORT=/dev/cu.usbserial-XXXX` if autodetect picks the wrong one.
//!
//! Every byte received is written to `scratchpad/ft5d/` so a capture is never
//! lost to a scrolled-off terminal.

use std::time::{Duration, Instant};

use serialport::{ClearBuffer, SerialPort};

/// The only byte this harness ever transmits. In CHIRP's `yaesu_clone` the PC
/// acks each received block with `0x06` to let the next one start.
const ACK: u8 = 0x06;

/// Quiet period that marks the end of a block. At 38400 baud consecutive bytes
/// are ~0.26 ms apart, so a full second of silence is unambiguous.
const GAP: Duration = Duration::from_secs(1);

/// Consecutive quiet periods that mean the radio is finished rather than
/// between blocks. Three seconds of nothing after an ack is the radio saying it
/// has no more to send.
const QUIET_ROUNDS_TO_STOP: u32 = 3;

/// How long to wait for the operator to press [Send].
const SEND_WAIT: Duration = Duration::from_secs(60);

/// Hard ceiling on a capture. The FT3D image is 130507 bytes; the FT5D's is
/// unknown and this is only here so a protocol misread cannot spin forever.
const MAX_CAPTURE: usize = 8 * 1024 * 1024;

/// Pick the USB serial port, preferring an explicit `FT5D_PORT`.
///
/// This scans `/dev` directly rather than using `serialport::available_ports`:
/// per the `serialport-macos-usbmodem-gap` note the crate misses USB-C
/// `usbmodem` devices on macOS, and the SCU-19/39 cables show up under both
/// `usbserial` (Prolific/FTDI) and `usbmodem` (CDC) names.
fn pick_port() -> Result<String, String> {
    if let Ok(p) = std::env::var("FT5D_PORT") {
        return Ok(p);
    }
    let mut found: Vec<String> = std::fs::read_dir("/dev")
        .map_err(|e| format!("cannot scan /dev: {e}"))?
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.starts_with("cu.usbserial") || n.starts_with("cu.usbmodem"))
        .map(|n| format!("/dev/{n}"))
        .collect();
    found.sort();
    match found.len() {
        0 => Err("no /dev/cu.usbserial* or /dev/cu.usbmodem* port found. Is the SCU cable \
                  plugged in and the radio powered on in clone mode?"
            .into()),
        1 => Ok(found.remove(0)),
        _ => Err(format!(
            "several candidate ports: {}. Re-run with FT5D_PORT=<one of them>.",
            found.join(", ")
        )),
    }
}

fn open(port: &str) -> Result<Box<dyn SerialPort>, String> {
    serialport::new(port, super::BAUD)
        .data_bits(serialport::DataBits::Eight)
        .parity(serialport::Parity::None)
        .stop_bits(serialport::StopBits::One)
        .flow_control(serialport::FlowControl::None)
        .timeout(GAP)
        .open()
        .map_err(|e| format!("could not open {port}: {e}"))
}

/// Read one byte, or `None` once [`GAP`] elapses with nothing on the wire.
fn read_byte(p: &mut dyn SerialPort) -> Result<Option<u8>, String> {
    let mut b = [0u8; 1];
    match std::io::Read::read(p, &mut b) {
        Ok(0) => Ok(None),
        Ok(_) => Ok(Some(b[0])),
        Err(ref e) if e.kind() == std::io::ErrorKind::TimedOut => Ok(None),
        Err(e) => Err(e.to_string()),
    }
}

/// Block until the radio sends its first byte, or [`SEND_WAIT`] expires.
fn await_first(p: &mut dyn SerialPort) -> Result<u8, String> {
    let deadline = Instant::now() + SEND_WAIT;
    loop {
        if let Some(b) = read_byte(p)? {
            return Ok(b);
        }
        if Instant::now() >= deadline {
            return Err("the radio never sent anything. Check that \"CLONE\" is on the \
                        display and that you pressed [Send]."
                .into());
        }
    }
}

/// Collect bytes until the wire has been quiet for [`GAP`].
fn read_block(p: &mut dyn SerialPort, into: &mut Vec<u8>) -> Result<usize, String> {
    let start = into.len();
    while into.len() < MAX_CAPTURE {
        match read_byte(p)? {
            Some(b) => into.push(b),
            None => break,
        }
    }
    Ok(into.len() - start)
}

fn hexdump(bytes: &[u8], limit: usize) -> String {
    let mut out = String::new();
    for (i, chunk) in bytes.iter().take(limit).collect::<Vec<_>>().chunks(16).enumerate() {
        let hex: Vec<String> = chunk.iter().map(|b| format!("{b:02X}")).collect();
        let txt: String = chunk
            .iter()
            .map(|&&b| if (0x20..0x7F).contains(&b) { b as char } else { '.' })
            .collect();
        out.push_str(&format!("{:08X}  {:-48}  |{}|\n", i * 16, hex.join(" "), txt));
    }
    if bytes.len() > limit {
        out.push_str(&format!("... {} more bytes\n", bytes.len() - limit));
    }
    out
}

/// Write a capture next to the SD-card dumps so the two can be diffed later.
fn save(name: &str, bytes: &[u8]) -> String {
    // The test runs with CWD = src-tauri/, so the repo's scratchpad is one up.
    let dir = std::path::Path::new("../scratchpad/ft5d");
    let _ = std::fs::create_dir_all(dir);
    let path = dir.join(name);
    match std::fs::write(&path, bytes) {
        Ok(()) => path.display().to_string(),
        Err(e) => format!("<failed to write {}: {e}>", path.display()),
    }
}

#[test]
#[ignore = "requires an FT5D in clone mode on a USB cable"]
fn ft5d_clone_probe() {
    let port = pick_port().expect("port");
    let ack_mode = std::env::var("FT5D_ACK").is_ok();
    let mut p = open(&port).expect("open");
    let _ = p.clear(ClearBuffer::All);

    println!("\n=== FT5D clone probe ===");
    println!("port: {port}   mode: {}", if ack_mode { "FULL IMAGE (acking)" } else { "IDENT ONLY (never acks)" });
    println!("waiting up to 60s -- press [Send] on the radio now...\n");

    let first = await_first(&mut *p).expect("first byte");
    let mut all = vec![first];
    let ident_len = 1 + read_block(&mut *p, &mut all).expect("ident block");

    println!("--- ident block: {ident_len} bytes ---");
    print!("{}", hexdump(&all[..ident_len], 256));
    println!("saved: {}", save("ft5d_ident.bin", &all[..ident_len]));

    if !ack_mode {
        println!(
            "\nStopping here without acking, so no image transfer starts.\n\
             Re-run with FT5D_ACK=1 to capture the whole image."
        );
        return;
    }

    // Ack-and-drain. The radio is the sender for the entire exchange; ACK is
    // the only byte we put on the wire, so this stays a read-only operation no
    // matter how wrong our guess about the block structure turns out to be.
    let mut quiet = 0;
    let mut blocks = 1;
    while quiet < QUIET_ROUNDS_TO_STOP && all.len() < MAX_CAPTURE {
        if std::io::Write::write_all(&mut *p, &[ACK]).is_err() {
            break;
        }
        let _ = std::io::Write::flush(&mut *p);
        let n = read_block(&mut *p, &mut all).expect("block");
        if n == 0 {
            quiet += 1;
        } else {
            quiet = 0;
            blocks += 1;
            println!("block {blocks}: +{n} bytes (total {})", all.len());
        }
    }

    println!("\n--- total captured: {} bytes in {blocks} block(s) ---", all.len());
    print!("{}", hexdump(&all, 512));
    println!("saved: {}", save("ft5d_clone_image.bin", &all));
}
