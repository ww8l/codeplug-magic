//! Reverse-engineering harness for the FT5D clone port (issue #32).
//!
//! This is a measuring instrument, not driver code — `#[ignore]`d so
//! `cargo test` stays hardware-free, one radio operation per process (see the
//! `hw-test-harness-pattern` note). It exists to settle the clone protocol; the
//! driver implementation that grows out of it lives in `mod.rs`.
//!
//! ## What we are trying to learn, and why this is safe
//!
//! The Yaesu clone protocol is **radio-initiated**: the PC opens the port and
//! waits, and the radio streams when the operator presses the send key. Per
//! CHIRP's `yaesu_clone.__clone_in`, the PC transmits exactly **one** byte for
//! the whole transfer — a `0x06` ack after the ident block — and the radio
//! sends everything else. So a clone-in cannot write to the radio no matter how
//! wrong our guesses are.
//!
//! The deliverable is the **10-byte ident header**. `BACKUP.dat` does not
//! contain it, and CHIRP's `match_model` says the clone image opens with the
//! model token, so a clone-*out* image cannot be assembled without capturing
//! these bytes first. Everything else — the `AH82M` token, all 130496 body
//! bytes, both checksum families — we already have from the microSD decode.
//!
//! ## The expected shape (derived, to be confirmed by this capture)
//!
//! ```text
//! clone image (130507) = ident header (10) || BACKUP.dat (130496) || sum byte (1)
//! ```
//!
//! CHIRP's `FT1Radio._block_lengths = [10, 130497]` and `_memsize = 130507`,
//! and its last checksum `YaesuChecksum(0x0000, 0x1FDC9)` stores an 8-bit sum at
//! clone `0x1FDCA` = SD `0x1FDC0`, one past the end of the SD file. That is the
//! trailing byte. The success criterion for this probe is therefore exact:
//! **strip 10 from the head and 1 from the tail and the result must equal a
//! `BACKUP.dat` taken from the radio in the same state, byte for byte.**
//!
//! ## Running it
//!
//! Radio off → SCU cable into the DATA jack → hold [DISP] while powering on
//! until "CLONE" appears. Then, from `src-tauri/`:
//!
//! ```text
//! cargo test --lib ft5d_clone_probe -- --ignored --nocapture
//! ```
//!
//! Press the radio's *send* key once the harness prints "waiting". Overrides,
//! so a retry never needs a recompile:
//!
//! - `FT5D_PORT=/dev/cu.usbserial-XXXX` — pick the port explicitly.
//! - `FT5D_BAUD=9600` — the 38400 in `mod.rs` is assumed from CHIRP's FT1D
//!   family and has never been confirmed on an FT5D. A wrong rate shows up as a
//!   garbage ident block, which this prints *before* it acks.
//! - `FT5D_IDENT_ONLY=1` — stop after the ident block without acking, so no
//!   image transfer ever starts.
//!
//! Every byte received is written to `scratchpad/ft5d/` before any assertion
//! runs, so a capture can never be lost to a panic or a scrolled-off terminal.

use std::time::{Duration, Instant};

use serialport::{ClearBuffer, SerialPort};

/// The only byte this harness ever transmits: CHIRP's `CMD_ACK`, sent once to
/// release the image transfer after the ident block.
const ACK: u8 = 0x06;

/// Silence that marks the end of the ident block. The radio sends its ident and
/// then waits for the ack, so any real gap here is that wait. At 38400 baud
/// consecutive bytes are ~0.26 ms apart, making a full second unambiguous.
const IDENT_GAP: Duration = Duration::from_secs(1);

/// Silence that means the image transfer is over. CHIRP's `_chunk_read` gives
/// up after exactly this long, so the radio is known not to pause longer
/// mid-stream.
const STREAM_IDLE: Duration = Duration::from_secs(2);

/// How long to wait for the operator to press the send key.
const SEND_WAIT: Duration = Duration::from_secs(90);

/// Hard ceiling on a capture, so a protocol misread cannot spin forever.
const MAX_CAPTURE: usize = 8 * 1024 * 1024;

/// What the FT1D/FT2D/FT3D family sends, and what we expect the FT5D to send.
const EXPECT_IDENT: usize = 10;
const EXPECT_TOTAL: usize = 130_507;

/// Pick the USB serial port, preferring an explicit `FT5D_PORT`.
///
/// Scans `/dev` directly rather than using `serialport::available_ports`: per
/// the `serialport-macos-usbmodem-gap` note the crate misses USB-C `usbmodem`
/// devices on macOS. The prefix list matches `commands::program`'s port scan —
/// SCU-19/39/57 cables appear under all three depending on the chipset.
fn pick_port() -> Result<String, String> {
    if let Ok(p) = std::env::var("FT5D_PORT") {
        return Ok(p);
    }
    let mut found: Vec<String> = std::fs::read_dir("/dev")
        .map_err(|e| format!("cannot scan /dev: {e}"))?
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| {
            n.starts_with("cu.usbserial")
                || n.starts_with("cu.usbmodem")
                || n.starts_with("cu.wchusbserial")
        })
        .map(|n| format!("/dev/{n}"))
        .collect();
    found.sort();
    match found.len() {
        0 => Err("no /dev/cu.usbserial*, cu.usbmodem* or cu.wchusbserial* port found. Is the \
                  SCU cable plugged in and the radio powered on in clone mode?"
            .into()),
        1 => Ok(found.remove(0)),
        _ => Err(format!(
            "several candidate ports: {}. Re-run with FT5D_PORT=<one of them>.",
            found.join(", ")
        )),
    }
}

fn baud() -> u32 {
    std::env::var("FT5D_BAUD")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(super::BAUD)
}

fn open(port: &str, rate: u32, timeout: Duration) -> Result<Box<dyn SerialPort>, String> {
    serialport::new(port, rate)
        .data_bits(serialport::DataBits::Eight)
        .parity(serialport::Parity::None)
        .stop_bits(serialport::StopBits::One)
        .flow_control(serialport::FlowControl::None)
        .timeout(timeout)
        .open()
        .map_err(|e| format!("could not open {port}: {e}"))
}

/// Read whatever is available, or `None` once the port timeout elapses with an
/// empty wire. Chunked rather than byte-at-a-time: the image is 130 KB and the
/// stream phase has no per-byte decisions to make.
fn read_some(p: &mut dyn SerialPort, buf: &mut [u8]) -> Result<Option<usize>, String> {
    match std::io::Read::read(p, buf) {
        Ok(0) => Ok(None),
        Ok(n) => Ok(Some(n)),
        Err(ref e) if e.kind() == std::io::ErrorKind::TimedOut => Ok(None),
        Err(e) => Err(e.to_string()),
    }
}

/// Block until the radio sends its first byte, or [`SEND_WAIT`] expires.
fn await_first(p: &mut dyn SerialPort, buf: &mut [u8]) -> Result<usize, String> {
    let deadline = Instant::now() + SEND_WAIT;
    loop {
        if let Some(n) = read_some(p, buf)? {
            return Ok(n);
        }
        if Instant::now() >= deadline {
            return Err("the radio never sent anything. Check that \"CLONE\" is on the display, \
                        that you pressed the send key, and that the cable is in the DATA jack."
                .into());
        }
    }
}

/// Collect bytes until the wire has been quiet for the port's timeout.
///
/// Returns how much was appended. Used for both phases; the phases differ only
/// in how long "quiet" is, which is the port timeout set by the caller.
fn drain(p: &mut dyn SerialPort, into: &mut Vec<u8>) -> Result<usize, String> {
    let start = into.len();
    let mut buf = [0u8; 4096];
    while into.len() < MAX_CAPTURE {
        match read_some(p, &mut buf)? {
            Some(n) => into.extend_from_slice(&buf[..n]),
            None => break,
        }
    }
    Ok(into.len() - start)
}

fn hexdump(bytes: &[u8], base: usize) -> String {
    let mut out = String::new();
    for (i, chunk) in bytes.chunks(16).enumerate() {
        let hex: Vec<String> = chunk.iter().map(|b| format!("{b:02X}")).collect();
        let txt: String = chunk
            .iter()
            .map(|&b| if (0x20..0x7F).contains(&b) { b as char } else { '.' })
            .collect();
        out.push_str(&format!("{:08X}  {:-48}  |{}|\n", base + i * 16, hex.join(" "), txt));
    }
    out
}

/// Write a capture next to the SD-card dumps so the two can be diffed.
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
    let rate = baud();
    let ident_only = std::env::var("FT5D_IDENT_ONLY").is_ok();

    println!("\n=== FT5D clone-in probe ===");
    println!("port: {port}   baud: {rate}");
    println!(
        "mode: {}",
        if ident_only { "IDENT ONLY (never acks)" } else { "FULL IMAGE (one ack)" }
    );
    println!("\nwaiting up to {}s -- press the SEND key on the radio now...\n", SEND_WAIT.as_secs());

    // Phase 1: the ident block, delimited by the radio's wait for our ack.
    let mut p = open(&port, rate, IDENT_GAP).expect("open");
    let _ = p.clear(ClearBuffer::All);
    let mut buf = [0u8; 4096];
    let n = await_first(&mut *p, &mut buf).expect("first byte");
    let mut ident = buf[..n].to_vec();
    drain(&mut *p, &mut ident).expect("ident block");

    println!("--- ident block: {} bytes ---", ident.len());
    print!("{}", hexdump(&ident, 0));
    println!("saved: {}", save("ft5d_ident.bin", &ident));
    if ident.len() != EXPECT_IDENT {
        println!(
            "\n!! expected {EXPECT_IDENT} bytes (CHIRP FT1D _block_lengths[0]). A garbage or \
             wrong-length block usually means the wrong baud -- retry with FT5D_BAUD=9600 or \
             19200. If it looks like sane ASCII, the FT5D's header is simply a different \
             length and that is a finding worth keeping."
        );
    }

    if ident_only {
        println!("\nStopping without acking, so no image transfer starts.");
        return;
    }

    // Phase 2: one ack, then the radio streams the rest without pausing. CHIRP
    // acks exactly once here and then never writes again -- a stray ack partway
    // through a stream we do not yet understand is the one way this could go
    // wrong, so the loop below is deliberately ack-free.
    println!("\nacking (0x06) -- the image takes ~{}s at {rate} baud...", EXPECT_TOTAL / (rate as usize / 10));
    let mut p = open(&port, rate, STREAM_IDLE).expect("reopen");
    std::io::Write::write_all(&mut *p, &[ACK]).expect("ack");
    std::io::Write::flush(&mut *p).expect("flush");

    let mut all = ident.clone();
    let body = drain(&mut *p, &mut all).expect("image");

    // Save before asserting anything: a capture is expensive (a radio cycle and
    // the operator's hands) and must survive whatever we conclude about it.
    let path = save("ft5d_clone_image.bin", &all);
    println!("\n--- captured {} bytes total ({body} after the ident) ---", all.len());
    println!("saved: {path}");
    println!("\nhead:\n{}", hexdump(&all[..64.min(all.len())], 0));
    if all.len() > 64 {
        let tail = all.len().saturating_sub(32);
        println!("tail:\n{}", hexdump(&all[tail..], tail));
    }

    if all.len() == EXPECT_TOTAL {
        println!("size matches CHIRP's _memsize ({EXPECT_TOTAL}).");
    } else {
        println!(
            "!! size is {} not {EXPECT_TOTAL}. Under- means the stream was cut short (idle \
             timeout too tight, or the radio aborted); over- usually means an echoed ack from a \
             2-pin cable, which CHIRP chews at the head of each block.",
            all.len()
        );
    }

    // The whole point: does stripping the header and trailing byte reproduce the
    // microSD image we already decoded? Reported, never asserted -- an
    // unexpected answer is data, not a failed test.
    if all.len() >= EXPECT_IDENT + 1 {
        let sum: u32 = all[..all.len() - 1].iter().map(|&b| b as u32).sum();
        let want = (sum & 0xFF) as u8;
        let got = all[all.len() - 1];
        println!(
            "trailing 8-bit sum over [0..len-1]: calculated {want:02X}, stored {got:02X} -- {}",
            if want == got { "OK" } else { "MISMATCH" }
        );
        println!(
            "\nNext, offline: compare bytes 10..{} of that file against a BACKUP.dat taken from \
             the radio in this same state. Byte-identical proves the +10 rule and the trailing \
             checksum together, and makes the clone-out payload a solved problem.",
            all.len() - 1
        );
    }
}
