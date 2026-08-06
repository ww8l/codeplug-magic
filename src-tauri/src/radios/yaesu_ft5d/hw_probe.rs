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
//! Radio off → SCU cable into the DATA jack → hold [F] while powering on. The
//! screen shows CLONE above two touch buttons, RECEIVE and SEND. Then, from
//! `src-tauri/`:
//!
//! ```text
//! cargo test --lib ft5d_clone_probe -- --ignored --nocapture
//! ```
//!
//! Tap **SEND** once the harness prints "waiting" — the radio sends, we listen.
//! A clone-*out* later is the **RECEIVE** button.
//!
//! Those labels are photographed off the radio (s85), not carried over from
//! CHIRP: `FT1Radio::get_prompts` says [BAND] to send and [Dx] to receive, but
//! the FT5D is touchscreen and has no [Dx] key. Only the [F]-while-powering-on
//! entry survived the check.
//!
//! Overrides, so a retry never needs a recompile:
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

/// How long to wait for the operator to tap SEND. Generous by default and
/// `FT5D_WAIT`-overridable: the harness is usually started by one person and
/// tapped by another, and a timeout that races the hand at the radio just
/// burns a clone-mode cycle.
fn send_wait() -> Duration {
    Duration::from_secs(
        std::env::var("FT5D_WAIT").ok().and_then(|v| v.parse().ok()).unwrap_or(300),
    )
}

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
pub(super) fn pick_port() -> Result<String, String> {
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

/// Open the port and **verify the baud rate actually took**.
///
/// Do not trust the builder's rate argument. Three captures returned the
/// byte-identical `35 52 FE` at nominally 38400 and 115200 — impossible for live
/// UART data, which cannot survive a 3x change in sampling rate unchanged — and
/// `stty` showed the device sitting at 9600. A rate that is silently ignored
/// looks exactly like a protocol mystery: 10 bytes sent at 38400 and sampled at
/// 9600 arrive as 10 * 9600/38400 = 2.5, i.e. the three bytes we kept getting.
///
/// So set it explicitly after opening as well, read it back, and say so.
fn open(port: &str, rate: u32, timeout: Duration) -> Result<Box<dyn SerialPort>, String> {
    let mut p = serialport::new(port, rate)
        .data_bits(serialport::DataBits::Eight)
        .parity(serialport::Parity::None)
        .stop_bits(serialport::StopBits::One)
        .flow_control(serialport::FlowControl::None)
        .timeout(timeout)
        .open()
        .map_err(|e| format!("could not open {port}: {e}"))?;

    if let Err(e) = p.set_baud_rate(rate) {
        return Err(format!("could not set {rate} baud on {port}: {e}"));
    }
    match p.baud_rate() {
        Ok(actual) if actual == rate => Ok(p),
        Ok(actual) => Err(format!(
            "asked {port} for {rate} baud but it reports {actual}. The adapter is ignoring the \
             rate, which silently corrupts every capture -- fix this before reading anything \
             into the bytes."
        )),
        Err(e) => Err(format!("could not read back the baud rate on {port}: {e}")),
    }
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

/// Discard anything already queued, and return how much there was.
///
/// `clear(ClearBuffer::All)` is not sufficient on macOS: bytes from an earlier
/// aborted transfer survive in the driver queue and get handed to the next
/// opener. That is not a harmless quirk here — the probe treats the first bytes
/// it sees as the ident block and a gap as end-of-block, so stale leftovers make
/// it "capture" an ident, ack into the void and exit seconds after starting,
/// long before the operator has touched the radio. Two runs at different baud
/// rates returning byte-identical garbage is what exposed it: real serial data
/// cannot be independent of the sampling rate, but already-framed bytes sitting
/// in a queue are.
///
/// So drain by *reading* until the wire is genuinely quiet, and only then start
/// listening for the radio.
fn purge_stale(p: &mut dyn SerialPort) -> Result<usize, String> {
    let _ = p.clear(ClearBuffer::All);
    let mut junk = Vec::new();
    drain(p, &mut junk)?;
    if !junk.is_empty() {
        println!(
            "discarded {} stale byte(s) left over from an earlier transfer: {}",
            junk.len(),
            junk.iter().map(|b| format!("{b:02X}")).collect::<Vec<_>>().join(" ")
        );
    }
    Ok(junk.len())
}

/// Block until the radio sends its first byte, or [`send_wait`] expires.
fn await_first(p: &mut dyn SerialPort, buf: &mut [u8]) -> Result<usize, String> {
    let deadline = Instant::now() + send_wait();
    loop {
        if let Some(n) = read_some(p, buf)? {
            return Ok(n);
        }
        if Instant::now() >= deadline {
            return Err("the radio never sent anything. Check that \"CLONE\" is on the display, \
                        that you tapped SEND on the screen, and that the cable is in the DATA \
                        jack."
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

/// Log every arriving chunk with a timestamp, for one long window, never acking.
///
/// Distinguishes two very different worlds that both look like "3 bytes":
///
/// - **3 bytes in a millisecond, then silence** — the radio really does send a
///   short message and stop, and we are looking at a protocol we do not know.
/// - **3 bytes dribbled out over the seconds the radio shows TX** — data is
///   flowing on the wire but almost none of it survives framing, which points
///   at the electrical path (wrong cable, wrong jack, inverted levels), not at
///   the protocol or the baud rate.
///
/// Worth measuring because the baud sweep proved the adapter honest, so the
/// remaining explanations are about the wire itself.
fn trace(port: &str, rate: u32) {
    let window = Duration::from_secs(
        std::env::var("FT5D_TRACE_SECS").ok().and_then(|v| v.parse().ok()).unwrap_or(60),
    );
    println!("\n=== FT5D wire trace ===");
    println!("port: {port}   baud: {rate}   window: {}s", window.as_secs());
    println!("never acks. Tap SEND and let the radio do whatever it does.\n");

    let mut p = match open(port, rate, Duration::from_millis(50)) {
        Ok(p) => p,
        Err(e) => {
            println!("could not open: {e}");
            return;
        }
    };
    let _ = purge_stale(&mut *p);

    let start = Instant::now();
    let deadline = start + window;
    let mut buf = [0u8; 4096];
    let mut total = Vec::new();
    let mut chunks = 0usize;
    let mut first_at: Option<f64> = None;
    let mut last_at = 0.0;

    while Instant::now() < deadline {
        if let Ok(Some(n)) = read_some(&mut *p, &mut buf) {
            let at = start.elapsed().as_secs_f64();
            first_at.get_or_insert(at);
            last_at = at;
            chunks += 1;
            println!(
                "  t={at:>7.3}s  +{n:>4} bytes  {}",
                buf[..n.min(16)].iter().map(|b| format!("{b:02X} ")).collect::<String>()
            );
            total.extend_from_slice(&buf[..n]);
        }
    }

    println!("\n--- {} bytes in {chunks} chunk(s) over {}s ---", total.len(), window.as_secs());
    if total.is_empty() {
        println!("Nothing arrived at all -- no tap landed, or nothing is reaching the RX line.");
        return;
    }
    println!("saved: {}", save(&format!("trace_{rate}.bin"), &total));
    let span = last_at - first_at.unwrap_or(0.0);
    println!("activity spanned {span:.3}s, from t={:.3}s", first_at.unwrap_or(0.0));
    if span > 0.5 {
        println!(
            "\n>> Data trickled in over {span:.1}s. The radio is transmitting for a long time and\n\
             almost none of it is framing into bytes -- that is an ELECTRICAL problem (cable\n\
             wiring, wrong jack, or inverted levels), not a baud or protocol one."
        );
    } else {
        println!(
            "\n>> One short burst. The radio really does send just these bytes and stop, so this\n\
             is a protocol we do not recognise rather than a broken wire."
        );
    }
}

/// Rates worth trying, low to high. The FT1D family uses 38400; the FT5D is a
/// later radio and nothing has confirmed it inherits that.
const SWEEP_RATES: &[u32] = &[4800, 9600, 19200, 38400, 57600, 115200, 230400];

/// Listen at every rate in turn and report what arrives at each.
///
/// The question this answers is not "which rate is right" but something more
/// basic: **do the received bytes change with the rate at all?** Four captures
/// returned an identical `35 52 FE` at nominally 38400 and 115200. Live UART
/// data cannot do that — misframing is rate-dependent by construction — so
/// either the adapter is pinned to one physical speed while reporting whatever
/// we ask, or those bytes are not framed data. A sweep separates the two:
/// values that vary mean the rate is really changing and one of them is right;
/// values that never vary mean the adapter is lying and no protocol conclusion
/// drawn from these captures is worth anything.
///
/// Never acks, so no image transfer is ever started and every phase stays short.
/// The operator just keeps tapping SEND; each rate gets its own window.
fn sweep(port: &str) {
    let window = Duration::from_secs(
        std::env::var("FT5D_SWEEP_WINDOW").ok().and_then(|v| v.parse().ok()).unwrap_or(45),
    );
    println!("\n=== FT5D baud sweep ===");
    println!("port: {port}");
    println!(
        "{} rates x {}s each (~{} min). Keep tapping SEND every few seconds --\n\
         each rate gets its own window and a missed tap only costs that one rate.\n",
        SWEEP_RATES.len(),
        window.as_secs(),
        (SWEEP_RATES.len() as u64 * window.as_secs()).div_ceil(60)
    );

    let mut results: Vec<(u32, Vec<u8>)> = Vec::new();
    for &rate in SWEEP_RATES {
        println!("--- {rate} baud: listening {}s ---", window.as_secs());
        let mut p = match open(port, rate, IDENT_GAP) {
            Ok(p) => p,
            Err(e) => {
                println!("  skipped: {e}\n");
                results.push((rate, Vec::new()));
                continue;
            }
        };
        let _ = purge_stale(&mut *p);

        let deadline = Instant::now() + window;
        let mut buf = [0u8; 4096];
        let mut got = Vec::new();
        while Instant::now() < deadline {
            if let Ok(Some(n)) = read_some(&mut *p, &mut buf) {
                got.extend_from_slice(&buf[..n]);
                let _ = drain(&mut *p, &mut got);
                break;
            }
        }
        if got.is_empty() {
            println!("  nothing arrived (no tap landed in this window)\n");
        } else {
            println!("  {} byte(s):", got.len());
            print!("{}", hexdump(&got[..got.len().min(64)], 0));
            println!("  saved: {}\n", save(&format!("sweep_{rate}.bin"), &got));
        }
        results.push((rate, got));
    }

    println!("\n=== sweep summary ===");
    for (rate, got) in &results {
        let hex: String =
            got.iter().take(12).map(|b| format!("{b:02X} ")).collect::<String>();
        println!("  {rate:>7} baud  {:>5} bytes  {hex}", got.len());
    }

    let seen: Vec<&Vec<u8>> = results.iter().map(|(_, g)| g).filter(|g| !g.is_empty()).collect();
    println!();
    match seen.len() {
        0 => println!(
            "Nothing at any rate. Either no tap landed, or the radio is not transmitting on \
             this cable at all."
        ),
        1 => println!("Only one rate produced data -- rerun to confirm it is repeatable."),
        _ if seen.windows(2).all(|w| w[0] == w[1]) => println!(
            "!! IDENTICAL BYTES AT EVERY RATE THAT RESPONDED.\n\
             The adapter is not actually changing speed, so no capture so far means anything \
             about the FT5D's protocol. Fix the adapter (different cable, or set the rate with \
             stty before opening) before drawing any further conclusion."
        ),
        _ => println!(
            "Bytes differ by rate, so the rate really is changing. The right one is whichever \
             gives 10 bytes opening with 41 48 38 32 4D (AH82M)."
        ),
    }
}

#[test]
#[ignore = "requires an FT5D in clone mode on a USB cable"]
fn ft5d_clone_probe() {
    let port = pick_port().expect("port");
    let rate = baud();
    let ident_only = std::env::var("FT5D_IDENT_ONLY").is_ok();

    if std::env::var("FT5D_SWEEP").is_ok() {
        sweep(&port);
        return;
    }
    if std::env::var("FT5D_TRACE").is_ok() {
        trace(&port, rate);
        return;
    }

    println!("\n=== FT5D clone-in probe ===");
    println!("port: {port}   baud: {rate}");
    println!(
        "mode: {}",
        if ident_only { "IDENT ONLY (never acks)" } else { "FULL IMAGE (one ack)" }
    );
    println!(
        "\nwaiting up to {}s -- tap SEND on the radio now...\n",
        send_wait().as_secs()
    );

    // Phase 1: the ident block, delimited by the radio's wait for our ack.
    let mut p = open(&port, rate, IDENT_GAP).expect("open");
    purge_stale(&mut *p).expect("purge");
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
    // Widen the read timeout on the handle we already hold. Closing and
    // reopening loses the run: macOS does not release the device immediately
    // ("Device or resource busy"), and even when it does, the reopen drops
    // DTR/RTS and any bytes the radio sends in the gap. The two phases differ
    // only in how long "quiet" means finished.
    p.set_timeout(STREAM_IDLE).expect("set stream timeout");
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
