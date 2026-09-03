//! Phase 1 capture harness for the Kenwood TM-D710A (issue #113).
//!
//! This is a **measuring instrument, not driver code**. It exists to answer the
//! four questions the plan in `scratchpad/kenwood_tmd710/PLAN.md` says must be
//! answered before a line of driver is written, and it is `#[cfg(test)]` +
//! `#[ignore]`d so `cargo test` stays hardware-free.
//!
//! ## Why this radio needs a different harness from every other one here
//!
//! The TM-D710 is a **live-mode** radio. There is no clone image and no card
//! file: the PC sends one ASCII command per memory, terminated by `\r`, and the
//! radio answers in kind. So the thing to capture is a **transcript**, and the
//! Phase 2 gate is re-emitting these lines character-identically — the same gate
//! as a byte-identical re-encode, on a different substrate.
//!
//! ## Everything sent here is a query. Nothing can change the radio.
//!
//! On this protocol a command **with no parameter list** reads, and the same
//! command **with** one writes. Every command in `QUERIES` below is the bare
//! form. That is the whole safety argument, so the list is explicit and short
//! rather than assembled at runtime:
//!
//! - `ID` — model string the radio calls itself
//! - `TY` — type/variant
//! - `FV 0` — firmware version of unit 0
//! - `MU` — **all 42 menu parameters in one line**, per LA3QMA's `MU.md`
//! - `ME nnn` — memory channel `nnn`, 16 comma-separated fields
//! - `MN nnn` — memory channel `nnn`'s name
//!
//! ⚠ `TX` is a command on this radio and it **keys the transmitter**. It is not
//! in the list and must never be. Nor is `MC`, which moves the radio's current
//! channel — harmless but it changes state under the operator.
//!
//! ## Running it
//!
//! Radio on, cable into the COM port on the rear of the **operation panel**
//! (Kenwood manual §5.1.2). ⚠ Not the main unit — AG7GN's README says main
//! unit, but that is the TM-D710**G**; measured on this radio in session 120.
//! From `src-tauri/`:
//!
//! ```text
//! D710_PORT=/dev/cu.usbserial-XXXX cargo test --lib d710_find_the_radio -- --ignored --nocapture
//! D710_PORT=/dev/cu.usbserial-XXXX D710_BAUD=9600 cargo test --lib d710_capture -- --ignored --nocapture
//! ```
//!
//! One radio operation per process, per the `hw-test-harness-pattern` note.

use serialport::SerialPort;
use std::time::{Duration, Instant};

/// The bare, parameter-less forms. See the module doc: this list *is* the
/// safety argument, so it is written out rather than built.
const QUERIES: &[&str] = &["ID", "TY", "AI", "MU", "MS", "FV"];

/// Rates the PC port offers (menu 519 on this family). CHIRP's driver assumes
/// 9600; AG7GN's CLI defaults to 57600. Neither is evidence about *this* radio,
/// so all four get tried.
const RATES: &[u32] = &[9600, 19200, 38400, 57600];

fn port_path() -> String {
    std::env::var("D710_PORT")
        .expect("set D710_PORT to the cable's /dev/cu.* path (ls /dev/cu.*)")
}

/// Open with no flow control.
///
/// ⚠ The rate is **not** verified by reading it back: `baud_rate()` echoes the
/// value that was set, on some adapters even when the hardware ignored it, so it
/// proves nothing (see the `verify-hardware-claims-not-reports` note). Here that
/// does not matter — the reply is ASCII, so a wrong rate produces visible
/// garbage rather than a plausible-looking answer. That is the check.
fn open(port: &str, rate: u32) -> Result<Box<dyn SerialPort>, String> {
    serialport::new(port, rate)
        .data_bits(serialport::DataBits::Eight)
        .parity(serialport::Parity::None)
        .stop_bits(serialport::StopBits::One)
        .flow_control(serialport::FlowControl::None)
        .timeout(Duration::from_millis(700))
        .open()
        .map_err(|e| format!("could not open {port} at {rate}: {e}"))
}

/// Send one command and read the reply up to its `\r`.
///
/// Returns the raw bytes as well as the lossy string: at a wrong baud rate the
/// bytes are the interesting half, and a reply that is not valid UTF-8 is itself
/// the finding.
fn ask(p: &mut dyn SerialPort, cmd: &str) -> Result<(String, Vec<u8>), String> {
    let _ = p.clear(serialport::ClearBuffer::All);
    p.write_all(format!("{cmd}\r").as_bytes())
        .map_err(|e| format!("write {cmd}: {e}"))?;
    p.flush().map_err(|e| format!("flush {cmd}: {e}"))?;

    let mut raw = Vec::new();
    let deadline = Instant::now() + Duration::from_millis(1500);
    let mut byte = [0u8; 1];
    while Instant::now() < deadline {
        match p.read(&mut byte) {
            Ok(0) => continue,
            Ok(_) => {
                if byte[0] == b'\r' {
                    break;
                }
                raw.push(byte[0]);
            }
            Err(ref e) if e.kind() == std::io::ErrorKind::TimedOut => break,
            Err(e) => return Err(format!("read after {cmd}: {e}")),
        }
    }
    Ok((String::from_utf8_lossy(&raw).into_owned(), raw))
}

/// Sweep the four PC-port rates asking `ID`, and print what comes back.
///
/// A reply containing `TM-D710` at exactly one rate settles both the rate and
/// the model in one pass. **Silence at every rate is the RT Systems cable
/// question**, not a protocol question: those cables carry FTDI chips programmed
/// with RT Systems' own USB VID/PID. If nothing enumerated as `/dev/cu.*` at
/// all, this test cannot even start, which is the same answer arriving earlier.
#[test]
#[ignore = "requires a TM-D710 on the cable"]
fn d710_find_the_radio() {
    let path = port_path();
    println!("\n=== TM-D710 rate sweep on {path} ===\n");
    let mut found = Vec::new();
    for &rate in RATES {
        match open(&path, rate) {
            Err(e) => println!("{rate:>6}: {e}"),
            Ok(mut p) => match ask(&mut *p, "ID") {
                Err(e) => println!("{rate:>6}: {e}"),
                Ok((_, raw)) if raw.is_empty() => println!("{rate:>6}: (silence)"),
                Ok((text, raw)) => {
                    println!("{rate:>6}: {text:?}  raw={raw:02x?}");
                    if text.contains("TM-D") || text.contains("TM-V") {
                        found.push((rate, text));
                    }
                }
            },
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    println!("\n--- radio answered at: {found:?}\n");
    assert!(
        !found.is_empty(),
        "no rate produced an ID reply naming a Kenwood. Before reading anything into this: is \
         the cable in the COM port on the rear of the OPERATION PANEL — not the main unit, \
         which is where the G's is — and does the port enumerate at all? An RT Systems cable's \
         FTDI carries their own VID/PID and may not bind a driver here."
    );
}

/// Capture the transcript Phase 2 will be built against: identity, the whole
/// menu line, and the first memories the radio already holds.
///
/// Writes `scratchpad/kenwood_tmd710/capture-<stamp>.txt` — gitignored, and the
/// anchor every later claim gets checked against.
#[test]
#[ignore = "requires a TM-D710 on the cable"]
fn d710_capture() {
    let path = port_path();
    let rate: u32 = std::env::var("D710_BAUD")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(9600);
    let mut p = open(&path, rate).expect("open");

    let mut log = String::new();
    log.push_str(&format!("# TM-D710 capture — {path} @ {rate} baud\n"));

    for cmd in QUERIES {
        let (text, raw) = ask(&mut *p, cmd).expect("query");
        println!("{cmd:>6} -> {text}");
        log.push_str(&format!("{cmd}\t{text}\traw={raw:02x?}\n"));
    }

    // The first ten memories, both record and name. Ten is enough to see the
    // field shape and to spot an empty slot's encoding without a long session.
    for ch in 0..10 {
        for cmd in [format!("ME {ch:03}"), format!("MN {ch:03}")] {
            let (text, _) = ask(&mut *p, &cmd).expect("memory query");
            println!("{cmd:>7} -> {text}");
            log.push_str(&format!("{cmd}\t{text}\n"));
        }
    }

    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let out = format!("../scratchpad/kenwood_tmd710/capture-{stamp}.txt");
    std::fs::write(&out, &log).expect("write transcript");
    println!("\n--- wrote {out}\n");
}

/// Read `MU` alone and append it to `scratchpad/kenwood_tmd710/mu-log.txt`,
/// labelled with `D710_LABEL`.
///
/// The unit of work for Phase 4: **one** menu item changed on the front panel
/// between two runs, so every field that moves can be attributed to it. Two
/// controls that both go `0 -> 1` in the same pass cannot be told apart, and
/// attributing them by position is how a previous radio shipped two exactly
/// swapped fields.
///
/// The first run of all is the noise floor — read twice with nothing changed.
/// If any field moves on its own, every later attribution is worthless, so this
/// gets established before a single value is read into.
#[test]
#[ignore = "requires a TM-D710 on the cable"]
fn d710_mu() {
    let path = port_path();
    let rate: u32 = std::env::var("D710_BAUD")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(57600);
    let label = std::env::var("D710_LABEL").unwrap_or_else(|_| "unlabelled".into());
    let mut p = open(&path, rate).expect("open");

    // ⚠ The first command after opening can draw a bare `?`: the rate sweep left
    // the radio's parser mid-garbage and it answered the next line with an
    // error. Ask twice and keep the second — and note that a real driver will
    // need the same retry rather than treating one `?` as a refusal.
    let _ = ask(&mut *p, "ID");
    let (text, _) = ask(&mut *p, "MU").expect("MU");

    let fields: Vec<&str> = text.trim_start_matches("MU ").split(',').collect();
    println!("\n{label}: {} fields\n{text}\n", fields.len());
    for (i, f) in fields.iter().enumerate() {
        print!("p{}={} ", i + 1, f);
    }
    println!();

    let log = "../scratchpad/kenwood_tmd710/mu-log.txt";
    let mut all = std::fs::read_to_string(log).unwrap_or_default();
    all.push_str(&format!("{label}\t{text}\n"));
    std::fs::write(log, all).expect("write mu log");
}

/// Read every memory slot and record three things Phase 2 cannot be written
/// without: the **full transcript** (its re-emit is the gate), how an **empty**
/// slot answers, and how long 1000 round trips actually take.
///
/// Needs nobody at the radio — just the cable — so it costs no operator time.
#[test]
#[ignore = "requires a TM-D710 on the cable"]
fn d710_dump_memories() {
    let path = port_path();
    let rate: u32 = std::env::var("D710_BAUD")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(57600);
    let mut p = open(&path, rate).expect("open");
    let _ = ask(&mut *p, "ID");

    let started = Instant::now();
    let mut log = String::new();
    let (mut populated, mut empty, mut other) = (0usize, 0usize, Vec::new());

    for ch in 0..1000 {
        let (text, _) = ask(&mut *p, &format!("ME {ch:03}")).expect("ME");
        if text.starts_with("ME ") {
            populated += 1;
            let (name, _) = ask(&mut *p, &format!("MN {ch:03}")).expect("MN");
            log.push_str(&format!("{text}\n{name}\n"));
        } else if text == "N" {
            empty += 1;
        } else {
            other.push((ch, text.clone()));
            log.push_str(&format!("# ch {ch}: unexpected reply {text:?}\n"));
        }
    }

    let elapsed = started.elapsed();
    println!("\n=== {populated} populated, {empty} empty, {} other", other.len());
    for (ch, t) in other.iter().take(10) {
        println!("    ch {ch}: {t:?}");
    }
    println!(
        "=== {:.1}s for {} round trips ({:.0} ms each)\n",
        elapsed.as_secs_f64(),
        1000 + populated,
        elapsed.as_millis() as f64 / (1000 + populated) as f64
    );

    std::fs::write("../scratchpad/kenwood_tmd710/memories.txt", &log).expect("write");
}

/// Read a named list of slots and print the `ME` and `MN` lines verbatim.
///
/// Read-only, and deliberately **not** `d710_dump_memories`: that one rewrites
/// `scratchpad/kenwood_tmd710/memories.txt`, which is the restore file holding
/// the radio's as-found state. Running it while a campaign has test values in
/// the radio would overwrite the only copy of what to put back.
///
/// `D710_SLOTS=500,501,502,503`
#[test]
#[ignore = "requires a TM-D710 on the cable"]
fn d710_read_slots() {
    let slots: Vec<u16> = std::env::var("D710_SLOTS")
        .expect("set D710_SLOTS to a comma-separated list, e.g. 500,501,502,503")
        .split(',')
        .map(|s| s.trim().parse().expect("slot number"))
        .collect();

    let path = port_path();
    let mut p = open(&path, 57600).expect("open");
    let _ = ask(&mut *p, "ID");

    println!();
    for slot in slots {
        let (me, _) = ask(&mut *p, &format!("ME {slot:03}")).expect("ME");
        if me == crate::radios::kenwood_tmd710::memory::EMPTY_REPLY {
            println!("{slot:03}  (empty)");
            continue;
        }
        let (mn, _) = ask(&mut *p, &format!("MN {slot:03}")).expect("MN");
        println!("{me}\n{mn}");
    }
    println!();
}

/// ★ **`0M PROGRAM` mode — the radio's OTHER transport, and where APRS lives.**
///
/// `MU` carries 42 menu parameters and stops at the 500-series. The TM-D710's
/// APRS and TNC settings are the **600-series menus**, and there is no `MU`
/// parameter for any of them — which is why a settings read built on `MU` alone
/// comes back with no APRS at all. On an APRS radio that is most of the point of
/// the thing missing.
///
/// MCP-2A does not use `MU`. It puts the radio into a block-transfer mode and
/// reads a **memory image**, so this radio is not purely live-mode after all:
/// it has a second transport, and everything `MU` cannot reach lives in there.
///
/// ```text
/// "0M PROGRAM\r"          -> "0M\r"        the display shows PROG MCP
/// R <addr:2 BE> <len:1>   -> W <addr:2> <len:1> <data...>   (len 0 = 256)
///                            then the host sends 06 and the radio answers 06
/// "E"                     -> 06 0D 00      back to normal
/// ```
///
/// ## ★ The handshake is the whole trick
///
/// The first three attempts at this all showed the same shape — the first `R`
/// after entering the mode returned a block and every one after it timed out —
/// which read like a refusal and was not. **The host must acknowledge each
/// block with `0x06`, and the radio acknowledges that back**, so a reader that
/// skips it is left holding a stream one byte out of step. The giveaway was a
/// header that came back `06 57 00 00`: a status byte, then `W`, then the
/// address. Published notes for this mode do not mention it.
///
/// ## This one is read-only and it still changes the radio's state
///
/// Nothing here writes a byte of configuration. But entering the mode puts the
/// radio into `PROG MCP` on its own display, and **leaving it there strands the
/// operator** until they power-cycle. So the exit is not on the happy path: the
/// dump runs inside a closure and `E` is sent afterwards either way.
#[test]
#[ignore = "requires a TM-D710 on the cable"]
fn d710_program_mode_dump() {
    let path = port_path();
    let mut p = open(&path, 57600).expect("open");
    let _ = ask(&mut *p, "ID");
    let id = ask(&mut *p, "ID").expect("ID").0;
    assert!(id.contains("TM-D710"), "not a TM-D710: {id:?}");

    let entered = ask(&mut *p, "0M PROGRAM").expect("enter program mode").0;
    println!("\n0M PROGRAM -> {entered:?}");
    assert!(entered.starts_with("0M"), "the radio refused program mode: {entered:?}");

    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let mut image: Vec<u8> = Vec::new();
        let mut addr: u32 = 0;
        while addr < 0x1_0000 {
            match read_block(&mut *p, addr as u16, 0) {
                Ok(data) => {
                    let n = data.len();
                    image.extend_from_slice(&data);
                    addr += n as u32;
                }
                Err(e) => {
                    println!("stopped at 0x{addr:04X}: {e}");
                    break;
                }
            }
        }
        image
    }));

    // ⚠ Always. See the doc comment.
    let _ = p.write_all(b"E");
    let _ = p.flush();
    std::thread::sleep(Duration::from_millis(300));
    let mut ack = [0u8; 3];
    let _ = read_exact_timeout(&mut *p, &mut ack);
    println!("E -> {ack:02X?}");

    let image = result.expect("the dump panicked; the radio was still taken out of program mode");
    println!("=== {} bytes ({:.1} KiB)", image.len(), image.len() as f64 / 1024.0);
    assert!(image.len() > 256, "program mode gave back only {} bytes", image.len());

    let out = format!("../scratchpad/kenwood_tmd710/progmode-{}.bin", std::process::id());
    std::fs::write(&out, &image).expect("write");
    println!("--- saved {out}\n");
}

/// Raw stream capture — no framing, no interpretation.
///
/// The first full dump came back drifting **one byte per block**: the same
/// content, sliding. That is a reader bug, not radio data, and guessing at it
/// costs more than looking. This sends three small requests and prints every
/// byte that comes back with a gap-based split, so the actual framing is
/// visible rather than inferred.
#[test]
#[ignore = "requires a TM-D710 on the cable"]
fn d710_program_mode_raw() {
    let path = port_path();
    let mut p = open(&path, 57600).expect("open");
    let _ = ask(&mut *p, "ID");
    assert!(ask(&mut *p, "ID").expect("ID").0.contains("TM-D710"));
    assert!(ask(&mut *p, "0M PROGRAM").expect("enter").0.starts_with("0M"));

    // Drain whatever is in flight, then send one request and read everything
    // that arrives until the line goes quiet.
    let drain = |p: &mut dyn SerialPort, label: &str, req: &[u8]| {
        let _ = p.clear(serialport::ClearBuffer::Input);
        let _ = p.write_all(req);
        let _ = p.flush();
        let mut got = Vec::new();
        let deadline = Instant::now() + Duration::from_millis(900);
        let mut b = [0u8; 1];
        while Instant::now() < deadline {
            match p.read(&mut b) {
                Ok(1) => got.push(b[0]),
                _ => {
                    if !got.is_empty() {
                        break;
                    }
                }
            }
        }
        println!("  {label}: sent {req:02X?}\n      got {got:02X?}");
        got
    };

    // ★ Address byte order. The 256-byte block at 0x0000 has `00 00 30 30` at
    // offset 0x10, so whichever request returns those is the right way round —
    // and 0x0000 itself cannot answer it, which is exactly what let the first
    // dump walk one byte per block instead of 256.
    drain(&mut *p, "hi-first 0x0010", &[b'R', 0x00, 0x10, 0x10]);
    drain(&mut *p, "  ack          ", &[0x06]);
    drain(&mut *p, "lo-first 0x0010", &[b'R', 0x10, 0x00, 0x10]);
    drain(&mut *p, "  ack          ", &[0x06]);

    let _ = p.write_all(b"E");
    let _ = p.flush();
    std::thread::sleep(Duration::from_millis(300));
    let mut ack = [0u8; 3];
    let _ = read_exact_timeout(&mut *p, &mut ack);
    println!("  E -> {ack:02X?}\n");
}

/// One block, with the acknowledgement the radio waits for.
///
/// `len` of 0 means 256 bytes, which is what the radio's own header uses.
fn read_block(p: &mut dyn SerialPort, addr: u16, len: u8) -> Result<Vec<u8>, String> {
    // ⚠ BIG-endian, high byte first. The published note for this mode says
    // little-endian, and 0x0000 — the only address anyone checks first — reads
    // the same either way, so the error survives. It cost this driver a 64 KiB
    // dump that drifted exactly one byte per block: stepping to 0x0100 sent
    // `00 01`, which the radio read as 0x0001.
    let req = [b'R', (addr >> 8) as u8, (addr & 0xFF) as u8, len];
    p.write_all(&req).map_err(|e| e.to_string())?;
    p.flush().map_err(|e| e.to_string())?;

    let mut head = [0u8; 4];
    read_exact_timeout(p, &mut head)?;
    if head[0] != b'W' {
        return Err(format!("expected a W header, got {head:02X?}"));
    }
    let n = if head[3] == 0 { 256 } else { head[3] as usize };
    let mut data = vec![0u8; n];
    read_exact_timeout(p, &mut data)?;

    // ★ The handshake. Without it the next request is never answered.
    p.write_all(&[0x06]).map_err(|e| e.to_string())?;
    p.flush().map_err(|e| e.to_string())?;
    let mut status = [0u8; 1];
    read_exact_timeout(p, &mut status)?;
    if status[0] != 0x06 {
        return Err(format!("the radio answered the ack with {:02X}", status[0]));
    }
    Ok(data)
}

/// Why the second block read in a session never answers.
///
/// Both earlier probes show the same shape: the **first** `R` after entering
/// program mode returns a block, and every one after it times out. That is not
/// an addressing problem — it happened at four different addresses — so the
/// question is what the radio is waiting for between blocks. The obvious
/// candidate is the `0x06` the radio itself sends to acknowledge a write: a
/// host that never acknowledges a block may simply be left holding one.
///
/// Also settles the address byte order as a side effect, which the first dump
/// could not: it read `0x0000`, where both orders are the same two bytes.
/// Offset `0x10` of that block is `00 00 30 30`, so whichever request returns
/// those is the right way round.
#[test]
#[ignore = "requires a TM-D710 on the cable"]
fn d710_program_mode_handshake() {
    let path = port_path();
    let mut p = open(&path, 57600).expect("open");
    let _ = ask(&mut *p, "ID");
    assert!(ask(&mut *p, "ID").expect("ID").0.contains("TM-D710"));
    assert!(ask(&mut *p, "0M PROGRAM").expect("enter").0.starts_with("0M"));

    fn one(p: &mut dyn SerialPort, req: [u8; 4], ack_after: bool) -> String {
        let _ = p.write_all(&req);
        let _ = p.flush();
        let mut head = [0u8; 4];
        if read_exact_timeout(p, &mut head).is_err() {
            return "no reply".into();
        }
        let len = if head[3] == 0 { 256 } else { head[3] as usize };
        let mut data = vec![0u8; len];
        let body = match read_exact_timeout(p, &mut data) {
            Ok(()) => format!("{:02X?}", &data[..len.min(4)]),
            Err(e) => format!("(short: {e})"),
        };
        if ack_after {
            let _ = p.write_all(&[0x06]);
            let _ = p.flush();
        }
        format!("head {head:02X?} data {body}")
    }

    // Four in a row, acknowledging each. If the ACK is what was missing, all
    // four answer where previously only the first did.
    println!();
    for (i, addr) in [0x0000u16, 0x0000, 0x0010, 0x0020].iter().enumerate() {
        let req = [b'R', (addr & 0xFF) as u8, (addr >> 8) as u8, 0x04];
        println!("  {i}: LE 0x{addr:04X} (sent {req:02X?}) -> {}", one(&mut *p, req, true));
    }
    let req = [b'R', 0x00, 0x10, 0x04];
    println!("  BE 0x0010 (sent {req:02X?}) -> {}", one(&mut *p, req, true));

    let _ = p.write_all(b"E");
    let _ = p.flush();
    std::thread::sleep(Duration::from_millis(300));
    let mut ack = [0u8; 3];
    let _ = read_exact_timeout(&mut *p, &mut ack);
    println!("E -> {ack:02X?}\n");
}

/// Read exactly `buf.len()` bytes, or give up. Block transfers are binary and
/// fixed-length, so the `\r`-terminated [`ask`] cannot be used for them.
fn read_exact_timeout(p: &mut dyn SerialPort, buf: &mut [u8]) -> Result<(), String> {
    let deadline = Instant::now() + Duration::from_millis(2000);
    let mut got = 0;
    while got < buf.len() {
        if Instant::now() > deadline {
            return Err(format!("timed out after {got} of {} bytes", buf.len()));
        }
        match p.read(&mut buf[got..]) {
            Ok(0) => continue,
            Ok(n) => got += n,
            Err(ref e) if e.kind() == std::io::ErrorKind::TimedOut => continue,
            Err(e) => return Err(e.to_string()),
        }
    }
    Ok(())
}

// ⚠ Everything below WRITES to the radio. Above this line nothing does.
// The safety net is that `memories.txt` and `mu-log.txt` hold the radio's
// entire state as it was found, so `d710_restore` can put any of it back.

/// **Hardware ladder step 1 — identity write.** Read a memory, write the
/// identical line back, read it again, and require that nothing moved.
///
/// Proves the write path with nothing at risk: the radio ends holding exactly
/// what it already held. It does **not** prove there is no checksum — an
/// identical line carries any digest along unchanged — but on an ASCII protocol
/// with no commit step there is nothing for a checksum to live in. Step 2 is
/// the real test.
#[test]
#[ignore = "requires a TM-D710 on the cable"]
fn d710_identity_write() {
    use crate::radios::kenwood_tmd710::{memory::Memory, write_memory};
    let path = port_path();
    let mut p = open(&path, 57600).expect("open");
    let _ = ask(&mut *p, "ID");

    let slot: u16 = std::env::var("D710_SLOT")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    let before = ask(&mut *p, &format!("ME {slot:03}")).expect("read").0;
    println!("\nbefore: {before}");
    let m = Memory::parse(&before).expect("parse");

    write_memory(&mut *p, &m).expect("identity write");
    let after = ask(&mut *p, &format!("ME {slot:03}")).expect("re-read").0;
    println!("after:  {after}\n");
    assert_eq!(after, before, "an identity write changed the slot");
    println!("--- identity write clean on slot {slot:03}\n");
}

/// **Ladder step 2, and the measurement instrument.** Write one memory built
/// from `D710_LINE`, verified by read-back.
///
/// Used to put a known tone index into an empty slot so the operator can read
/// the tone off the radio's own screen — the half no cable can answer.
#[test]
#[ignore = "requires a TM-D710 on the cable"]
fn d710_write_memory() {
    use crate::radios::kenwood_tmd710::{memory::Memory, write_memory, write_name};
    let line = std::env::var("D710_LINE").expect("set D710_LINE to a full ME line");
    let m = Memory::parse(&line).expect("D710_LINE does not parse");

    let path = port_path();
    let mut p = open(&path, 57600).expect("open");
    let _ = ask(&mut *p, "ID");

    let was = ask(&mut *p, &format!("ME {:03}", m.slot)).expect("read").0;
    println!("\nslot {:03} was: {was}", m.slot);
    write_memory(&mut *p, &m).expect("write");
    println!("slot {:03} now: {}", m.slot, m.to_line());

    if let Ok(name) = std::env::var("D710_NAME") {
        let n = crate::radios::kenwood_tmd710::memory::MemoryName {
            slot: m.slot,
            text: name,
        };
        write_name(&mut *p, &n).expect("name");
        println!("name: {}", n.to_line());
    }
    println!();
}

/// Change **one** menu parameter and prove only that one moved.
///
/// `D710_P` is 1-based (`p1`…`p42`), `D710_VALUE` the new value. The line is
/// built from a `MU` read taken moments earlier, never from a remembered one:
/// this command writes all 42 at once.
#[test]
#[ignore = "requires a TM-D710 on the cable"]
fn d710_set_menu() {
    use crate::radios::kenwood_tmd710::{memory::Menu, write_menu};
    let field: usize = std::env::var("D710_P")
        .expect("set D710_P to the 1-based menu parameter")
        .parse()
        .expect("D710_P");
    let value = std::env::var("D710_VALUE").expect("set D710_VALUE");

    let path = port_path();
    let mut p = open(&path, 57600).expect("open");
    let _ = ask(&mut *p, "ID");

    let before = Menu::parse(&ask(&mut *p, "MU").expect("MU").0).expect("parse");
    let wanted = before.with_field(field, &value).expect("with_field");
    println!("\np{field}: {:?} -> {:?}", before.field(field).unwrap(), value);

    let failed = write_menu(&mut *p, &wanted).expect("write");
    let after = Menu::parse(&ask(&mut *p, "MU").expect("MU").0).expect("parse");
    let moved = before.diff(&after);

    println!("moved: {moved:?}");
    if !failed.is_empty() {
        println!("⚠ did not take: {failed:?}");
    }
    assert_eq!(
        moved.len(),
        1,
        "expected exactly one field to move; a second means the line shifted"
    );
    assert_eq!(moved[0].0, field, "the wrong field moved");
    println!();
}

/// Sweep one `ME` field through every value its width allows and record which
/// ones the radio takes.
///
/// ## Why this works, and why it is the cheapest instrument here
///
/// The TM-D710 **validates a write and refuses it whole** — a rejected line
/// leaves the slot exactly as it was. So acceptance is a measurement, and the
/// first refused value is the size of the enum behind the field. That is how
/// fields 9-11 were settled as indices with lengths 42 and 104 (see
/// `kenwood_tmd710::tone`) without anyone reading the radio's screen.
///
/// It measures a **range**, never a meaning. Knowing field 13 accepts `0`, `1`
/// and `2` does not say which is AM; that still takes the manual, a cross-check
/// against real memories, or the radio's own display.
///
/// `D710_SLOT=504 D710_FIELDS=3,4,13,16` — 1-based, counting the slot number as
/// field 1, the way the module doc numbers them. Text in, text out: the base
/// line is substituted as **characters**, so a value `Memory::parse` would
/// refuse (an unknown shift, say) still reaches the radio, which is the whole
/// point.
#[test]
#[ignore = "requires a TM-D710 on the cable"]
fn d710_field_bounds() {
    let slot: u16 = std::env::var("D710_SLOT")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(504);
    let fields: Vec<usize> = std::env::var("D710_FIELDS")
        .unwrap_or_else(|_| "3,4,5,6,7,8,13,15,16".into())
        .split(',')
        .map(|s| s.trim().parse().expect("field number"))
        .collect();

    let captured = std::fs::read_to_string("../scratchpad/kenwood_tmd710/memories.txt")
        .expect("no captured memories — refusing to probe without the as-found copy");
    assert!(
        !captured.contains(&format!("ME {slot:03},")),
        "slot {slot:03} held a memory when the radio was first read; probe an empty one"
    );

    // Everything off and zero, so a refusal is the field under test and not a
    // combination. Widths are the radio's — see the `memory` module doc.
    //
    // ⚠ The base is not neutral for every field, and the first sweep proved it:
    // field 3 accepted only the tuning steps that divide **this** frequency
    // evenly, and fields 4 and 15 are constrained by the TX frequency in field
    // 14. So `D710_BASE` overrides the whole line (minus the slot) — measuring
    // a field means choosing a base that lets it move.
    let base: Vec<String> = format!(
        "{slot:03},{}",
        std::env::var("D710_BASE")
            .unwrap_or_else(|_| "0146520000,0,0,0,0,0,0,00,00,000,00000000,0,0000000000,0,0".into())
    )
    .split(',')
    .map(str::to_string)
    .collect();
    assert_eq!(base.len(), 16, "D710_BASE must be the 15 fields after the slot");

    let path = port_path();
    let mut p = open(&path, 57600).expect("open");
    let _ = ask(&mut *p, "ID");

    println!();
    for f in fields {
        let width = base[f - 1].len();
        let limit = 10usize.pow(width as u32);
        // A wide field cannot be swept — field 12 is eight digits — so an
        // explicit candidate list stands in for the range.
        let candidates: Vec<usize> = match std::env::var("D710_VALUES") {
            Ok(list) => list
                .split(',')
                .map(|v| v.trim().parse().expect("D710_VALUES"))
                .collect(),
            Err(_) => (0..limit.min(120)).collect(),
        };
        let mut taken = Vec::new();
        let mut first_refused = None;
        for v in candidates {
            let mut line = base.clone();
            line[f - 1] = format!("{v:0width$}");
            let sent = format!("ME {}", line.join(","));
            // A refused write is not an error reply — the radio acknowledges and
            // simply does not apply it — so the read-back is what decides.
            let _ = ask(&mut *p, &sent);
            let back = ask(&mut *p, &format!("ME {slot:03}")).expect("re-read").0;
            if back == sent {
                taken.push(v);
            } else if first_refused.is_none() {
                first_refused = Some(v);
            }
            // Leave nothing behind between candidates.
            let _ = ask(&mut *p, &format!("ME {slot:03},C"));
        }
        let contiguous =
            std::env::var("D710_VALUES").is_err() && taken.iter().enumerate().all(|(i, &v)| i == v);
        println!(
            "field {f:>2} (width {width}): accepted {} value(s){}{}",
            taken.len(),
            if contiguous {
                format!(" — 0..={}", taken.len().saturating_sub(1))
            } else {
                format!(" — {taken:?} ⚠ NOT contiguous")
            },
            match first_refused {
                Some(v) => format!(", first refused {v}"),
                None => ", nothing refused in range".into(),
            }
        );
    }
    println!();
}

/// Which characters survive a memory name, one character at a time.
///
/// A name is a **separate command** (`MN nnn,TEXT`) whose text runs to the end
/// of the line, so the failure this guards against is not cosmetic: the app's
/// channel names come from a database that has never been constrained to what a
/// 1990s Kenwood accepts, and a character the radio silently drops or rewrites
/// produces a memory labelled something other than what the operator asked for.
/// Same instrument as everywhere else here — write, read back, compare.
#[test]
#[ignore = "requires a TM-D710 on the cable"]
fn d710_name_charset() {
    use crate::radios::kenwood_tmd710::{memory::Memory, write_memory};
    let slot: u16 = std::env::var("D710_SLOT")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(504);

    let captured = std::fs::read_to_string("../scratchpad/kenwood_tmd710/memories.txt")
        .expect("no captured memories — refusing to probe without the as-found copy");
    assert!(
        !captured.contains(&format!("ME {slot:03},")),
        "slot {slot:03} held a memory when the radio was first read; probe an empty one"
    );

    let path = port_path();
    let mut p = open(&path, 57600).expect("open");
    let _ = ask(&mut *p, "ID");

    // A name needs a memory to hang on, so put one there first.
    let line = format!("ME {slot:03},0146520000,0,0,0,0,0,0,00,00,000,00000000,0,0000000000,0,0");
    write_memory(&mut *p, &Memory::parse(&line).expect("parse")).expect("seed the slot");

    let (mut kept, mut changed) = (String::new(), Vec::new());
    for byte in 0x20u8..0x7F {
        let ch = byte as char;
        // Padded so a dropped character shows as a length change rather than
        // shifting into a neighbour and reading as a match.
        let wanted = format!("A{ch}B");
        let sent = format!("MN {slot:03},{wanted}");
        let _ = ask(&mut *p, &sent);
        let back = ask(&mut *p, &format!("MN {slot:03}")).expect("re-read").0;
        match back.strip_prefix(&format!("MN {slot:03},")) {
            Some(got) if got == wanted => kept.push(ch),
            other => changed.push((ch, other.unwrap_or(&back).to_string())),
        }
    }

    println!("\n=== kept verbatim ({}): {kept}", kept.len());
    println!("=== altered or refused ({}):", changed.len());
    for (ch, got) in &changed {
        println!("    {ch:?} (0x{:02X}) -> {got:?}", *ch as u8);
    }
    println!();
    let _ = ask(&mut *p, &format!("ME {slot:03},C"));
}

/// ★ **The Phase 2 hardware gate: does the encoder emit lines this radio takes?**
///
/// Every unit test in `encode.rs` compares the encoder against text. None of
/// them can catch the failure that actually matters here, because a value the
/// TM-D710 dislikes is **not** an error — the radio acknowledges the line and
/// leaves the slot alone. A driver can therefore be entirely self-consistent
/// and still write nothing.
///
/// So: take each of the 38 memories the radio itself holds, decode it into app
/// terms, re-encode it into a **spare slot**, write it, and read it back. It
/// covers every channel shape Tim actually has — VHF, UHF, 220, the AM air-band
/// memory, the two on a 25 kHz step, split tone and CTCSS indices — without
/// touching one of his memories.
#[test]
#[ignore = "requires a TM-D710 on the cable"]
fn d710_encoder_acceptance() {
    use crate::radios::kenwood_tmd710::{
        encode::{decode_channel, encode_channel},
        memory::Memory,
        write_memory,
    };
    let slot: u16 = std::env::var("D710_SLOT")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(504);

    let captured = std::fs::read_to_string("../scratchpad/kenwood_tmd710/memories.txt")
        .expect("no captured memories to encode from");
    assert!(
        !captured.contains(&format!("ME {slot:03},")),
        "slot {slot:03} held a memory when the radio was first read; use an empty one"
    );

    let path = port_path();
    let mut p = open(&path, 57600).expect("open");
    let _ = ask(&mut *p, "ID");

    let (mut accepted, mut refused) = (0usize, Vec::new());
    for line in captured.lines().filter(|l| l.starts_with("ME ")) {
        let original = Memory::parse(line).expect("parse");
        let built = encode_channel(slot, &decode_channel(&original)).expect("encode");
        match write_memory(&mut *p, &built) {
            Ok(()) => accepted += 1,
            Err(e) => refused.push(format!("from {line}\n    {e}")),
        }
        let _ = ask(&mut *p, &format!("ME {slot:03},C"));
    }

    println!("\n=== {accepted} encoded memories accepted by the radio");
    if !refused.is_empty() {
        println!("=== {} REFUSED:", refused.len());
        for r in &refused {
            println!("  {r}");
        }
    }
    println!();
    assert!(refused.is_empty(), "{} encoded lines were refused", refused.len());
}

/// ★ **What the memory will actually hold, swept off the radio.**
///
/// `rx_bands` is the seed field with the worst failure mode in this project: an
/// out-of-coverage frequency does not error, it becomes a **silently empty
/// memory** while the app reports the channel written (three repeaters were lost
/// that way on the ID-52). The usual defence is to copy the band table out of
/// the manual and hope it matches the variant in front of you — and the manual
/// on hand covers the TM-D710**G**, not Tim's non-G.
///
/// So measure it. The radio refuses an `ME` line it cannot hold, which turns
/// coverage into the same accept/refuse question every other field answered:
/// sweep at 1 MHz, then bisect each edge down to 5 kHz.
///
/// `D710_SWEEP_LO=50 D710_SWEEP_HI=1400` (MHz). Reads out as a table of ranges
/// ready to become `rx_bands`.
///
/// ⚠ This measures what the **memory** accepts. It says nothing about transmit:
/// the radio stores an out-of-band memory happily and refuses at `[PTT]`
/// (manual, REPEATER-1 note), which is exactly the receive-only case the app
/// already models. `tx_bands` cannot be measured this way and must not be
/// guessed from this output.
#[test]
#[ignore = "requires a TM-D710 on the cable"]
fn d710_rx_band_sweep() {
    use crate::radios::kenwood_tmd710::encode::step_field;
    let slot: u16 = std::env::var("D710_SLOT")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(504);
    let lo: u64 = std::env::var("D710_SWEEP_LO")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(50);
    let hi: u64 = std::env::var("D710_SWEEP_HI")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(1400);

    let captured = std::fs::read_to_string("../scratchpad/kenwood_tmd710/memories.txt")
        .expect("no captured memories — refusing to sweep without the as-found copy");
    assert!(
        !captured.contains(&format!("ME {slot:03},")),
        "slot {slot:03} held a memory when the radio was first read; sweep into an empty one"
    );

    let path = port_path();
    let mut p = open(&path, 57600).expect("open");
    let _ = ask(&mut *p, "ID");

    // One closure, used by both the coarse sweep and the bisection, so an edge
    // is never decided by a different test than the sweep that found it.
    let mut holds = |p: &mut dyn SerialPort, hz: u64| -> bool {
        let Ok(step) = step_field(hz) else { return false };
        let sent = format!(
            "ME {slot:03},{hz:010},{step},0,0,0,0,0,08,08,000,00000000,0,0000000000,0,0"
        );
        let _ = ask(p, &sent);
        let back = ask(p, &format!("ME {slot:03}")).map(|(t, _)| t).unwrap_or_default();
        let _ = ask(p, &format!("ME {slot:03},C"));
        back == sent
    };

    let started = Instant::now();
    let mut coarse = Vec::new();
    for mhz in lo..=hi {
        coarse.push((mhz, holds(&mut *p, mhz * 1_000_000)));
    }

    // Bisect every accepted/refused transition to the 5 kHz the field can express.
    let refine = |p: &mut dyn SerialPort,
                  holds: &mut dyn FnMut(&mut dyn SerialPort, u64) -> bool,
                  mut good: u64,
                  mut bad: u64| {
        while good.abs_diff(bad) > 5_000 {
            let mid = (good + bad) / 2 / 5_000 * 5_000;
            if mid == good || mid == bad {
                break;
            }
            if holds(p, mid) {
                good = mid;
            } else {
                bad = mid;
            }
        }
        good
    };

    let mut ranges: Vec<(u64, u64)> = Vec::new();
    let mut open_at: Option<u64> = None;
    for i in 0..coarse.len() {
        let (mhz, ok) = coarse[i];
        let prev_ok = i > 0 && coarse[i - 1].1;
        if ok && !prev_ok {
            let start = if i == 0 {
                mhz * 1_000_000
            } else {
                refine(&mut *p, &mut holds, mhz * 1_000_000, (mhz - 1) * 1_000_000)
            };
            open_at = Some(start);
        }
        if !ok && prev_ok {
            let end = refine(&mut *p, &mut holds, (mhz - 1) * 1_000_000, mhz * 1_000_000);
            if let Some(start) = open_at.take() {
                ranges.push((start, end));
            }
        }
    }
    if let (Some(start), Some(&(mhz, true))) = (open_at, coarse.last()) {
        ranges.push((start, mhz * 1_000_000));
    }

    println!("\n=== what the TM-D710's memory accepts, {lo}-{hi} MHz");
    for (a, b) in &ranges {
        println!(
            "    {:>11.5} .. {:>11.5} MHz",
            *a as f64 / 1e6,
            *b as f64 / 1e6
        );
    }
    println!(
        "=== {} range(s) in {:.0}s\n",
        ranges.len(),
        started.elapsed().as_secs_f64()
    );
    assert!(!ranges.is_empty(), "the radio accepted no frequency at all");
}

/// ★ **Every `MU` menu parameter's range, swept off the radio (Phase 4).**
///
/// The same instrument as `d710_field_bounds`, pointed at the menu instead of a
/// memory. `MU` sets all 42 parameters in one line and the radio refuses a
/// value it does not have, so the accepted count *is* the size of the enum
/// behind that menu — which is what turns the manual's menu list from a
/// suggestion into a match: a menu with eight options can only be a field that
/// takes `0..=7`.
///
/// It measures a **size, never a meaning.** Which option is which still takes
/// the radio's own screen. That half is Tim's, and it is the cheap half once
/// the sizes have narrowed the candidates.
///
/// ## Safety
///
/// - **Nothing here changes the port speed.** Menu 920 (PC PORT SPEED) and the
///   COM port speed are not `MU` parameters — the line reaches menu 507 at p29
///   and the published field list has no port speed in it. That was checked
///   before a byte was written, because sweeping a baud-rate field would drop
///   the connection mid-write with the radio on an unknown rate.
/// - **The original line is restored after every field**, not once at the end,
///   so an abort leaves at most one parameter moved.
/// - p37 is APO on the published list. Restoring per-field means it never
///   stays on a timeout long enough to power the radio down mid-sweep.
#[test]
#[ignore = "requires a TM-D710 on the cable"]
fn d710_menu_bounds() {
    use crate::radios::kenwood_tmd710::{memory::Menu, write_menu};

    // p29-p34 are two-digit HEX (the capture holds `0C` and `0E`), so their
    // candidates have to be hex or the sweep measures the wrong alphabet.
    const HEX_FIELDS: [usize; 6] = [29, 30, 31, 32, 33, 34];

    let fields: Vec<usize> = match std::env::var("D710_FIELDS") {
        Ok(list) => list.split(',').map(|v| v.trim().parse().expect("field")).collect(),
        Err(_) => (1..=42).collect(),
    };

    let path = port_path();
    let mut p = open(&path, 57600).expect("open");
    let _ = ask(&mut *p, "ID");

    let original = Menu::parse(&ask(&mut *p, "MU").expect("MU").0).expect("parse");
    println!("\noriginal: {}\n", original.to_line());

    for f in fields {
        let width = original.field(f).expect("field").len();
        let hex = HEX_FIELDS.contains(&f);
        let limit = match (width, hex) {
            (1, _) => 10,
            (_, true) => 0x40,
            _ => 64,
        };

        let mut taken = Vec::new();
        let mut first_refused = None;
        for v in 0..limit {
            let text = if hex {
                format!("{v:02X}")
            } else {
                format!("{v:0width$}")
            };
            if text.len() > width {
                break;
            }
            let wanted = original.with_field(f, &text).expect("with_field");
            // ★ `MU` refuses differently from `ME`. A memory the radio dislikes
            // is acknowledged and quietly not stored; an out-of-range MENU value
            // draws an explicit `?`, which `ask` turns into an error. Both are
            // the same measurement — the value is not one this radio has — so
            // an Err here is a refusal, not a failure of the probe.
            let accepted = match write_menu(&mut *p, &wanted) {
                Ok(failed) => failed.is_empty(),
                Err(_) => false,
            };
            if accepted {
                taken.push(v);
            } else if first_refused.is_none() {
                first_refused = Some(v);
            }
            // Put it back before moving on — see the safety note. A `?` can
            // leave the parser mid-line, so the restore gets the same one
            // retry `ask_settling` gives every session.
            let back = match write_menu(&mut *p, &original) {
                Ok(diff) => diff,
                Err(_) => write_menu(&mut *p, &original).expect("restore"),
            };
            assert!(back.is_empty(), "p{f}: could not restore the menu line: {back:?}");
        }

        let contiguous = taken.iter().enumerate().all(|(i, &v)| i == v);
        println!(
            "p{f:<2} (width {width}{}) accepted {:>2}{}",
            if hex { ", hex" } else { "" },
            taken.len(),
            if contiguous {
                format!("  0..={}", taken.len().saturating_sub(1))
            } else {
                format!("  {taken:?} ⚠ NOT contiguous")
            }
        );
        let _ = first_refused;
    }

    let after = Menu::parse(&ask(&mut *p, "MU").expect("MU").0).expect("parse");
    assert_eq!(
        after.to_line(),
        original.to_line(),
        "the sweep did not leave the menu as it found it"
    );
    println!("\n--- menu restored exactly\n");
}

/// ★ **The settings path end to end, through the traits the app actually calls.**
///
/// `d710_menu_bounds` proved the radio takes a menu write. This proves the
/// *driver* does — `SettingsReader::read_settings` and
/// `SettingsWriter::write_settings`, the same two methods the profile editor
/// reaches, rather than the raw command underneath them.
///
/// That distinction is the whole point: in this repo a working read path has
/// twice hidden a dead write path, most expensively on the ID-52, where the
/// form filled correctly and the values simply never reached the radio.
///
/// Changes one field, reads it back through the decoder, and puts it back —
/// asserting the whole 42-parameter line is byte-identical to how it started,
/// which is also what proves the write is a PATCH and not a rebuild.
#[test]
#[ignore = "requires a TM-D710 on the cable"]
fn d710_settings_roundtrip() {
    use crate::radios::driver::{SettingsReader, SettingsWriter};
    use crate::radios::kenwood_tmd710::DRIVER;
    use serde_json::json;

    let path = port_path();
    let dir = std::env::temp_dir().join("d710-settings-probe");

    // Straight through the trait, exactly as `read_radio_settings` does.
    let before = DRIVER.read_settings(&path, "[]").expect("read settings");
    let line_before = String::from_utf8(before.backup.clone()).expect("the backup is the MU line");
    println!("\nbefore: {line_before}");
    println!("beep volume reads {}", before.settings["beep-volume"]);

    // Something audible and harmless, and pick a value it is NOT already on so
    // the write cannot pass by doing nothing — the trap that made a TH-D72
    // settings write look verified against its own unread buffer.
    let was = before.settings["beep-volume"].as_str().expect("an option label");
    let target = if was == "2" { "6" } else { "2" };

    let report = DRIVER
        .write_settings(&path, &json!({ "beep-volume": target }), "[]", &dir)
        .expect("write settings");
    println!(
        "wrote {} field(s), verified={:?}{}",
        report.fields_written,
        report.verified,
        report.note.as_deref().unwrap_or("")
    );
    assert_eq!(report.fields_written, 1, "expected exactly one field to change");
    assert_eq!(report.verified, Some(true), "the radio did not take the write");

    let mid = DRIVER.read_settings(&path, "[]").expect("re-read");
    assert_eq!(
        mid.settings["beep-volume"],
        json!(target),
        "the decoder does not see the value the write claimed to make"
    );

    // Put it back, and require the WHOLE line to match — that is what says the
    // write patched one parameter instead of rebuilding all 42.
    DRIVER
        .write_settings(&path, &json!({ "beep-volume": was }), "[]", &dir)
        .expect("restore");
    let after = DRIVER.read_settings(&path, "[]").expect("final read");
    let line_after = String::from_utf8(after.backup).expect("utf8");
    println!("after:  {line_after}");
    assert_eq!(
        line_after, line_before,
        "the settings round trip did not leave the menu as it found it"
    );
    println!("\n--- settings read + write proven through the driver traits\n");
}

/// Clear slots back to empty — `ME nnn,C`, the documented form, tested here.
///
/// `d710_restore` can overwrite a memory but cannot **un-write** one, so every
/// slot a campaign creates in previously-empty space stays created. This is the
/// other half of giving the radio back as found.
///
/// `D710_SLOTS=505` — refuses to touch a slot that was populated before this
/// campaign, because those are the operator's and `memories.txt` is the only
/// copy of them.
#[test]
#[ignore = "requires a TM-D710 on the cable"]
fn d710_clear_slots() {
    let slots: Vec<u16> = std::env::var("D710_SLOTS")
        .expect("set D710_SLOTS to a comma-separated list")
        .split(',')
        .map(|s| s.trim().parse().expect("slot number"))
        .collect();

    let captured = std::fs::read_to_string("../scratchpad/kenwood_tmd710/memories.txt")
        .expect("no captured memories — refusing to clear anything without the as-found copy");

    let path = port_path();
    let mut p = open(&path, 57600).expect("open");
    let _ = ask(&mut *p, "ID");

    println!();
    for slot in slots {
        let was_populated = captured.contains(&format!("ME {slot:03},"));
        assert!(
            !was_populated,
            "slot {slot:03} held a memory when the radio was first read. Clearing it would \
             destroy the operator's own channel; only slots this campaign created may be cleared."
        );
        ask(&mut *p, &format!("ME {slot:03},C")).expect("clear");
        let after = ask(&mut *p, &format!("ME {slot:03}")).expect("re-read").0;
        let empty = after == crate::radios::kenwood_tmd710::memory::EMPTY_REPLY;
        println!("{slot:03}  {}  (read back: {after})", if empty { "cleared" } else { "⚠ NOT CLEARED" });
        assert!(empty, "slot {slot:03} did not clear");
    }
    println!();
}

/// Put the radio back exactly as it was found, from the captured transcript.
///
/// The reason writing to Tim's radio is a reasonable thing to do at all.
#[test]
#[ignore = "requires a TM-D710 on the cable"]
fn d710_restore() {
    use crate::radios::kenwood_tmd710::{
        memory::{Memory, MemoryName, Menu},
        write_memory, write_menu, write_name,
    };
    let path = port_path();
    let mut p = open(&path, 57600).expect("open");
    let _ = ask(&mut *p, "ID");

    let text = std::fs::read_to_string("../scratchpad/kenwood_tmd710/memories.txt")
        .expect("no captured memories to restore from");
    let mut restored = 0;
    for line in text.lines().filter(|l| !l.starts_with('#')) {
        if line.starts_with("ME ") {
            write_memory(&mut *p, &Memory::parse(line).expect("parse")).expect("write");
            restored += 1;
        } else if line.starts_with("MN ") {
            write_name(&mut *p, &MemoryName::parse(line).expect("parse")).expect("write");
        }
    }

    // The menu line as first read, before anything in this campaign touched it.
    let log = std::fs::read_to_string("../scratchpad/kenwood_tmd710/mu-log.txt").expect("mu log");
    let first = log
        .lines()
        .find(|l| l.starts_with("noise-floor-1\t"))
        .and_then(|l| l.split_once('\t'))
        .map(|(_, line)| line)
        .expect("no noise-floor-1 row to restore the menu from");
    let failed = write_menu(&mut *p, &Menu::parse(first).expect("parse")).expect("write");

    println!("\n--- restored {restored} memories and the menu line");
    if failed.is_empty() {
        println!("--- menu clean\n");
    } else {
        println!("⚠ menu fields that did not take: {failed:?}\n");
    }
}

/// ★★★ The whole image — the two thirds `d710_program_mode_dump` never saw.
///
/// That dump walked forward until a read failed and stopped at `0x7F00`,
/// reporting 32 512 bytes as "the image". It is not. **`0x7F00` is a hole in
/// the middle, not the end.** CHIRP's clone-mode driver
/// (`chirp/drivers/tmd710.py`, `KenwoodTMD710Radio._read_mem`) reads blocks
/// `0x00`-`0x9B` and skips exactly one of them with the comment
/// `# Skip block 7f !!??`, then reads two odd tails at `0xFEF0` and `0xFF00`.
/// A reader that treats the hole as an end loses everything above it.
///
/// What is up there matters: CHIRP maps SkyCommand around `0x8660`, and
/// **nothing anywhere in `0x0000`-`0x7EFF` looks like an APRS setting** — no
/// call sign but the power-on message, no path, no beacon text — on a radio
/// whose 600-series holds 32 APRS and TNC menus. This is where they have to be.
///
/// ## Addresses here are RADIO addresses
///
/// The output is a `0x1_0000`-byte file with `FF` for every address never read,
/// so a file offset *is* the address the radio answers to. CHIRP's own mmap
/// concatenates blocks instead, which shifts everything above the skipped block
/// down by 0x100 — that is why its `#seekto 0x08660` is really `0x8760` on the
/// wire. Not a convention to inherit while measuring.
///
/// Read-only, and it still leaves `PROG MCP` on the display, so `E` is sent
/// outside the happy path exactly as in `d710_program_mode_dump`.
#[test]
#[ignore = "requires a TM-D710 on the cable"]
fn d710_program_mode_dump_full() {
    let path = port_path();
    let mut p = open(&path, 57600).expect("open");
    let _ = ask(&mut *p, "ID");
    let id = ask(&mut *p, "ID").expect("ID").0;
    assert!(id.contains("TM-D710"), "not a TM-D710: {id:?}");

    let entered = ask(&mut *p, "0M PROGRAM").expect("enter program mode").0;
    println!("\n0M PROGRAM -> {entered:?}");
    assert!(entered.starts_with("0M"), "the radio refused program mode: {entered:?}");

    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let mut image = vec![0xFFu8; 0x1_0000];
        let mut read: Vec<(u16, usize)> = Vec::new();
        let mut holes: Vec<(u16, String)> = Vec::new();

        // Every request CHIRP makes, in its order. `len` 0 means 256.
        //
        // ⚠ 0x7F00 is left out, and that IS measured rather than inherited: the
        // first run of this probe asked for it and the radio answered with
        // nothing at all — `timed out after 0 of 4 bytes`. CHIRP's `!!??` is a
        // real hole in the address space. Asking costs the rest of the dump,
        // because the recovery from a dead request desynchronises the stream.
        let mut plan: Vec<(u16, u8)> =
            (0u16..0x9C).filter(|b| *b != 0x7F).map(|b| (b << 8, 0u8)).collect();
        plan.push((0xFEF0, 0x10));
        plan.push((0xFF00, 0x90));

        for (addr, len) in plan {
            match read_block(&mut *p, addr, len) {
                Ok(data) => {
                    let end = addr as usize + data.len();
                    assert!(end <= image.len(), "block at 0x{addr:04X} overruns 64 KiB");
                    image[addr as usize..end].copy_from_slice(&data);
                    read.push((addr, data.len()));
                }
                Err(e) => {
                    // A failed read leaves the stream mid-block, and every
                    // address after it would then be measuring the desync
                    // rather than the radio. Stop and report where.
                    println!("  0x{addr:04X}: {e}");
                    holes.push((addr, e));
                    break;
                }
            }
        }
        (image, read, holes)
    }));

    // ⚠ Always. See `d710_program_mode_dump`.
    let _ = p.write_all(b"E");
    let _ = p.flush();
    std::thread::sleep(Duration::from_millis(300));
    let mut ack = [0u8; 3];
    let _ = read_exact_timeout(&mut *p, &mut ack);
    println!("E -> {ack:02X?}");

    let (image, read, holes) =
        result.expect("the dump panicked; the radio was still taken out of program mode");
    let bytes: usize = read.iter().map(|(_, n)| n).sum();
    println!("=== {} of {} requests answered, {bytes} bytes", read.len(), read.len() + holes.len());
    for (addr, e) in &holes {
        println!("    hole 0x{addr:04X}: {e}");
    }
    assert!(bytes > 0x8000, "only {bytes} bytes came back — this is not the full image");

    let out = format!("../scratchpad/kenwood_tmd710/progfull-{}.bin", std::process::id());
    std::fs::write(&out, &image).expect("write");
    println!("--- saved {out} (0x10000 bytes, FF where nothing was read)\n");
}

/// Get the radio out of `PROG MCP` when a dump left it there.
///
/// A probe that fails mid-block never reaches its own `E`, and the radio then
/// answers nothing at all — `ID` comes back empty, which looks exactly like a
/// dead cable. It is not: the radio is in program mode and only speaks the
/// binary protocol. Sending `E` on its own is the whole fix, and it is worth a
/// named instrument because the failure mode is indistinguishable from
/// hardware trouble at the point where someone would start unplugging things.
#[test]
#[ignore = "requires a TM-D710 on the cable"]
fn d710_leave_program_mode() {
    let path = port_path();
    let mut p = open(&path, 57600).expect("open");
    let _ = p.clear(serialport::ClearBuffer::All);
    p.write_all(b"E").expect("write E");
    p.flush().expect("flush");
    std::thread::sleep(Duration::from_millis(400));
    let mut ack = [0u8; 3];
    let _ = read_exact_timeout(&mut *p, &mut ack);
    println!("E -> {ack:02X?}");
    let _ = p.clear(serialport::ClearBuffer::All);

    let _ = ask(&mut *p, "ID");
    let id = ask(&mut *p, "ID").expect("ID after E").0;
    println!("ID -> {id:?}");
    assert!(id.contains("TM-D710"), "the radio is still not answering: {id:?}");
}

/// One block written, and the acknowledgement the radio sends back.
///
/// `W <addr:2 BE> <len:1> <data…>` -> one status byte. Note the asymmetry with
/// [`read_block`]: a read is acknowledged by the **host** and the radio answers
/// that acknowledgement, while a write is acknowledged by the **radio** and
/// there is nothing to send back. Getting that the wrong way round leaves the
/// stream one byte out of step for every request that follows.
fn write_block(p: &mut dyn SerialPort, addr: u16, data: &[u8]) -> Result<(), String> {
    assert!(!data.is_empty() && data.len() <= 256, "a block is 1..=256 bytes");
    let len = if data.len() == 256 { 0u8 } else { data.len() as u8 };
    let mut req = vec![b'W', (addr >> 8) as u8, (addr & 0xFF) as u8, len];
    req.extend_from_slice(data);
    p.write_all(&req).map_err(|e| e.to_string())?;
    p.flush().map_err(|e| e.to_string())?;

    let mut status = [0u8; 1];
    read_exact_timeout(p, &mut status)?;
    match status[0] {
        0x06 => Ok(()),
        // Published: 0x0F is "the radio is in an error state", which it enters
        // when the host leaves it idle in program mode for too long. It is not
        // a refusal of this particular write, and treating it as one sends you
        // looking for a validation rule that does not exist.
        0x0F => Err("0x0F — the radio is in the program-mode error state (PROG ERR)".into()),
        other => Err(format!("the radio answered a write with {other:02X}")),
    }
}

/// ★★★ The image WRITE path, climbed one rung at a time on the narrowest,
/// least destructive field this radio has.
///
/// Nothing had ever been written to this radio's image before this test. The
/// two questions it answers, in order, are the ladder from the `new-radio`
/// skill adapted to a transport rather than a container:
///
/// 1. **Identity write** — write a field back byte-for-byte and read it back.
///    Proves the verb, the framing and the acknowledgement with **nothing at
///    risk**: if every byte is the one already there, a total success and a
///    total no-op are the same outcome.
/// 2. **One field** — change it, read it back, leave the mode, come back and
///    read it again. The second read is the one that matters: this protocol has
///    no checksum and no commit step, so a value that survives a re-entry is
///    the only evidence that anything was *stored* rather than echoed.
///
/// ## Why status text 3
///
/// It is **empty on Tim's radio** (`FF` x 42), so there is no operator data to
/// lose, and it is directly visible on the radio's own screen under the status
/// text menu — which is the half a read-back cannot supply. See
/// [[an-ack-is-not-a-commit]]: on the BT-9000 an APRS block answered `0x06`
/// four times and never changed a byte.
///
/// ## ⚠ This is a NARROW write and that is the thing being tested
///
/// CHIRP writes the whole 156-block image and wraps it in an invalidate /
/// revalidate dance — `FF` over the first byte of the headers at `0x0000` and
/// `0x8000`, all blocks, then the saved headers back. This writes **42 bytes**
/// and touches no header at all. That may simply not commit: the BT-9000 has a
/// segment that acknowledges a partial write and silently keeps the old
/// contents. If the re-entry read shows the old value, the narrow write is the
/// thing that failed, not the transport.
#[test]
#[ignore = "requires a TM-D710 on the cable — WRITES to the radio"]
fn d710_status_text_write_ladder() {
    // Status text 3 of 5. The block base is the live APRS config; the entry
    // stride is 44 bytes and the text starts one byte into each entry.
    const APRS_LIVE: u16 = 0x8100;
    const TEXT_LEN: usize = 42;
    let addr = APRS_LIVE + 0x089 + 2 * 44 + 1;
    let probe: Vec<u8> = {
        let mut v = b"CPMAGIC TEST 129".to_vec();
        v.resize(TEXT_LEN, 0x00);
        v
    };

    let path = port_path();
    let mut p = open(&path, 57600).expect("open");
    let _ = ask(&mut *p, "ID");
    let id = ask(&mut *p, "ID").expect("ID").0;
    assert!(id.contains("TM-D710"), "not a TM-D710: {id:?}");

    let enter = |p: &mut dyn SerialPort| -> bool {
        ask(p, "0M PROGRAM").map(|r| r.0.starts_with("0M")).unwrap_or(false)
    };
    let leave = |p: &mut dyn SerialPort| {
        let _ = p.write_all(b"E");
        let _ = p.flush();
        std::thread::sleep(Duration::from_millis(300));
        let mut ack = [0u8; 3];
        let _ = read_exact_timeout(p, &mut ack);
        println!("  E -> {ack:02X?}");
    };

    assert!(enter(&mut *p), "the radio refused program mode");
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let before = read_block(&mut *p, addr, TEXT_LEN as u8).expect("read status text 3");
        println!("\nbefore      {}", show(&before));

        // Rung 1 — identity.
        write_block(&mut *p, addr, &before).expect("identity write");
        let same = read_block(&mut *p, addr, TEXT_LEN as u8).expect("read back after identity");
        assert_eq!(same, before, "an identity write changed the field");
        println!("identity ok, unchanged");

        // Rung 2 — one field.
        write_block(&mut *p, addr, &probe).expect("write the probe text");
        let after = read_block(&mut *p, addr, TEXT_LEN as u8).expect("read back after write");
        println!("after write {}", show(&after));
        assert_eq!(after, probe, "the read-back does not match what was written");
        before
    }));
    leave(&mut *p);
    let before = result.expect("the ladder panicked; the radio was taken out of program mode");

    // Rung 2b — the one that separates a stored value from an echoed one.
    std::thread::sleep(Duration::from_millis(500));
    assert!(enter(&mut *p), "could not re-enter program mode");
    let persisted = read_block(&mut *p, addr, TEXT_LEN as u8);
    leave(&mut *p);
    let persisted = persisted.expect("read after re-entering");
    println!("after re-entry {}", show(&persisted));

    assert_ne!(
        persisted, before,
        "the field is back to what it was: the narrow write was acknowledged and NOT committed"
    );
    assert_eq!(persisted, probe, "the field changed, but not to what was written");
    println!(
        "\n★ narrow image write PROVEN over a re-entry.\n  \
         Status text 3 now reads {:?} — check it on the radio's own screen,\n  \
         then run d710_restore_status_text_3 to put it back.",
        String::from_utf8_lossy(&probe[..16])
    );
}

/// Put status text 3 back to the `FF`-filled empty it was before the ladder.
#[test]
#[ignore = "requires a TM-D710 on the cable — WRITES to the radio"]
fn d710_restore_status_text_3() {
    const APRS_LIVE: u16 = 0x8100;
    const TEXT_LEN: usize = 42;
    let addr = APRS_LIVE + 0x089 + 2 * 44 + 1;
    let empty = vec![0xFFu8; TEXT_LEN];

    let path = port_path();
    let mut p = open(&path, 57600).expect("open");
    let _ = ask(&mut *p, "ID");
    assert!(ask(&mut *p, "ID").expect("ID").0.contains("TM-D710"));
    assert!(ask(&mut *p, "0M PROGRAM").expect("enter").0.starts_with("0M"));

    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        write_block(&mut *p, addr, &empty).expect("restore");
        read_block(&mut *p, addr, TEXT_LEN as u8).expect("read back")
    }));
    let _ = p.write_all(b"E");
    let _ = p.flush();
    std::thread::sleep(Duration::from_millis(300));
    let mut ack = [0u8; 3];
    let _ = read_exact_timeout(&mut *p, &mut ack);

    let back = result.expect("the restore panicked; the radio was taken out of program mode");
    println!("restored to {}", show(&back));
    assert_eq!(back, empty, "status text 3 is not back to empty");
}

/// Bytes as hex plus their printable reading, which is how every field in this
/// block has to be looked at: half of them are text and half are not.
fn show(b: &[u8]) -> String {
    let t: String = b.iter().map(|c| if (0x20..0x7F).contains(c) { *c as char } else { '.' }).collect();
    format!("{}  |{}|", b.iter().map(|c| format!("{c:02X}")).collect::<Vec<_>>().join(""), t)
}

/// Put the whole live APRS block back from a saved dump.
///
/// This is what makes a front-panel measurement pass reversible, and it is why
/// the write path was built before the campaign rather than after it: without
/// it, every setting Tim changes to let a diff name an offset is a setting he
/// has to re-enter by hand from memory.
///
/// `D710_IMAGE=<path>` names a `d710_program_mode_dump_full` file — 64 KiB with
/// `FF` for anything unread, so a file offset is a radio address. Only the
/// 1152 bytes of the **live** block are written; PM1-5 are the operator's saved
/// profiles and nothing here has any business touching them.
///
/// Writes in 256-byte blocks and reads each one back before moving on, because
/// an acknowledged write that did not commit is a failure this protocol can
/// produce and would otherwise report as success.
#[test]
#[ignore = "requires a TM-D710 on the cable — WRITES to the radio"]
fn d710_restore_aprs_block() {
    const APRS_LIVE: u16 = 0x8100;
    const BLOCK_LEN: usize = 0x480;

    let src = std::env::var("D710_IMAGE").expect("set D710_IMAGE to a full-dump file");
    let image = std::fs::read(&src).expect("read the dump");
    assert_eq!(image.len(), 0x1_0000, "{src} is not a 64 KiB full dump");
    let want = &image[APRS_LIVE as usize..APRS_LIVE as usize + BLOCK_LEN];
    assert!(
        want.iter().any(|b| *b != 0xFF),
        "the APRS block in {src} is all FF — that dump never read this region"
    );

    let path = port_path();
    let mut p = open(&path, 57600).expect("open");
    let _ = ask(&mut *p, "ID");
    assert!(ask(&mut *p, "ID").expect("ID").0.contains("TM-D710"));
    assert!(ask(&mut *p, "0M PROGRAM").expect("enter").0.starts_with("0M"));

    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let mut written = 0usize;
        for off in (0..BLOCK_LEN).step_by(256) {
            let n = 256.min(BLOCK_LEN - off);
            let addr = APRS_LIVE + off as u16;
            let chunk = &want[off..off + n];
            write_block(&mut *p, addr, chunk).unwrap_or_else(|e| panic!("write 0x{addr:04X}: {e}"));
            let back = read_block(&mut *p, addr, if n == 256 { 0 } else { n as u8 })
                .unwrap_or_else(|e| panic!("read back 0x{addr:04X}: {e}"));
            assert_eq!(back, chunk, "0x{addr:04X} did not take the write");
            written += n;
        }
        written
    }));

    let _ = p.write_all(b"E");
    let _ = p.flush();
    std::thread::sleep(Duration::from_millis(300));
    let mut ack = [0u8; 3];
    let _ = read_exact_timeout(&mut *p, &mut ack);
    println!("E -> {ack:02X?}");

    let written = result.expect("the restore panicked; the radio was taken out of program mode");
    println!("restored {written} bytes of the live APRS block from {src}");
}
