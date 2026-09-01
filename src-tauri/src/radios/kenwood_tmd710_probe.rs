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
