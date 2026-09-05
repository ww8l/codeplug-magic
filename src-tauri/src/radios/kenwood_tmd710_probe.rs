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
//!
//! ## The APRS/TNC settings block, as measured on the radio
//!
//! `scratchpad/` is gitignored, so the working sheet
//! (`scratchpad/kenwood_tmd710/APRS-BLOCK.md`) does not survive this machine.
//! This is the committed copy of what the radio itself has confirmed.
//!
//! The 600-series menus are **not** reachable by `MU` — they live in the image
//! behind `0M PROGRAM`, in six `0x480`-byte blocks at `0x8100 + n * 0x480`
//! (live, then PM1-5). Offsets below are relative to the **live** block base.
//!
//! | offset | field | menu | values measured |
//! |---|---|---|---|
//! | `+0x000` | My call sign, 10 bytes | 600 | |
//! | `+0x00A` | Beacon type | 600 | `00` APRS, `01` NAVITRA |
//! | `+0x00C` | Data band | 601 | `01` = `B Band` |
//! | `+0x00D` | Packet transfer rate | 601 | `01` = `9600 bps` |
//! | `+0x00E` | DCD sense | 601 | `02` = `IGNORE DCD` |
//! | `+0x00F` | TX delay | 601 | `00` = `100ms`, `03` = `300ms` |
//! | `+0x01C` | My position channels 1-5, 20 bytes each | 605 | |
//! | `+0x083` | Speed information | 606 | `00` off, `01` on |
//! | `+0x084` | Altitude information | 606 | `00` off, `01` on |
//! | `+0x085` | Position ambiguity | 606 | `04` = `4-DIGIT` |
//! | `+0x087` | Position comment | 607 | `09` = `CUSTOM 2` |
//! | `+0x089` | Status text 1-5, 44 bytes each | 608 | |
//! | `+0x0B5` | Status text TX rate | 608 | `01`=`1/1`, `02`=`1/2`, `05`=`1/5` |
//! | `+0x169` | Station icon, 2 raw ASCII bytes | 610 | `/-` shows as a house |
//! | `+0x16D` | Packet transmit method | 611 | `02` = `AUTO` |
//! | `+0x16E` | Beacon TX interval | 611 | `04`=`3 min`, `05`=`5 min`, `09`=`60 min` |
//! | `+0x16F` | Decay algorithm | 611 | `00` = off |
//! | `+0x460` | SkyCommand commander / transporter call signs | 700/701 | |
//! | `+0x474` | SkyCommand tone | 702 | `08` = `88.5 Hz`, the manual's default |
//!
//! ⚠⚠ **The D710A has no SmartBeaconing.** The 7 bytes at `+0x3D7` do match the
//! published SmartBeaconing defaults, but "SmartBeaconing" appears **zero**
//! times in the TM-D710**A** manual and there is no menu 630/631/632 on this
//! radio. They were graded against the **G**'s manual — a grade that was never
//! valid here. No menu reaches them; they must not ship as settings.
//!
//! ## The menu census (`new-radio` step 1, for this transport)
//!
//! `MU` reaches 42 of the radio's ~115 menus and **none** of the 6xx group,
//! which is the feature the radio is named for. The 6xx/7xx range is
//! **34 menu numbers and 66 individual settings**; ~25 are located and 15 are
//! confirmed on the radio.
//!
//! ★★★ **The factory-default cross-check.** The PM blocks are untouched
//! defaults and the A manual states the default of every menu, so a candidate
//! offset whose PM byte contradicts the manual is refuted with no radio time —
//! and since most defaults are `0`, a **non-zero** default is a rare anchor. It
//! is 5-for-5 on already-measured fields (`+0x00F`=`02`=`200 ms`, `+0x083` and
//! `+0x084`=`01`=`ON`, `+0x169`=`\K`=the KENWOOD icon, `+0x474`=`08`=`88.5 Hz`).
//!
//! ## The image map, closed at the desk (s131)
//!
//! CHIRP's **non-G** class (`TM-D710_CloneMode`) declares a structural map that
//! this project had written off wholesale. Its APRS claims are indeed useless,
//! but its **structure** is right and was never tested:
//!
//! | region | what | check |
//! |---|---|---|
//! | `0x0200`+`0x0400`..`0x0C00` | config block 0 and PM1-5, 394 bytes each | |
//! | `0x0E00`-`0x160B` | channel map | |
//! | `0x1700`-`0x575F` | memory channels, 16 bytes each | ch 39 is all `FF`; the radio holds 39 |
//! | `0x5800`-`0x77DF` | channel names, 8 bytes each | `W0UPS`/`W0LRA`/`W0TX`, matching `MN` |
//! | `0x77E0`-`0x782F` | weather channel names | **`WX   1 … WX   8`** ✓ |
//! | `0x7DA0` / `0x7DF0` | PM names / MCP comment | zeros here, untouched |
//! | `0x8100`-`0x9BFF` | the six APRS blocks | |
//!
//! ★★ `0x5800 + 1020*8 = 0x77E0` exactly, and `0x8100 + 6*0x480 = 0x9C00`
//! exactly — the last structure ends on the last address [`read_plan`] asks
//! for. Four regions previously logged as "unidentified" were simply **unused
//! memory and name slots**: the array extents had been read off the *populated*
//! part instead of the structure.
//!
//! ★ CHIRP's offsets **above** the hole are `true + 0x100` (`skycmd` declared at
//! `0x8660`/`0x8674`, measured here at `0x8560`/`0x8574`). Use the rule.
//!
//! ⚠ Do not re-search for menu 612's path or menu 613's `APK102`: neither
//! appears anywhere in the 39 840 bytes, in plain **or** AX.25 bit-shifted form.
//! That is not evidence they are absent — 612's TYPE is an enum whose default
//! *renders* as `WIDE1-1, WIDE2-1`, and 613's `APK102` is almost certainly enum
//! index 0. Both are small enums at their default, which is the same blind spot
//! that hid the first four fields. Find them with a front-panel change.
//!
//! ⚠ `+0x00C`, `+0x00E`, `+0x085`, `+0x087`, `+0x16D` and `+0x16F` are each
//! pinned at **one** value and are owed a second before they go in a table:
//! one index does not settle an enum.
//!
//! ★ **TX rate stores the denominator, not a list position** — `05` reads
//! `1/5`, not the sixth entry. A table built on display order would write this
//! field wrong for every value but one.
//!
//! ⚠ `+0x0B5` is `+0x089 + 44`, the lead byte of status text **record 2**. The
//! obvious reading — that each record carries its own TX rate, and menu 608
//! edited record 2 because record 2 is selected — is **one measurement on one
//! record and is not established.**
//!
//! ⚠ `+0x165` is unidentified and has been misnamed twice: position comment in
//! session 129, status text TX rate in session 131. Both times it was a value
//! that merely fit.
//!
//! ⚠ Use `TM-D710A_manual.pdf` (the A's own, with the full menu table, option
//! lists and factory defaults), **not** the G's. `APRS LOCK` is in the G manual
//! and is **not on this radio's screen**; the radio says `ID TM-D710`,
//! `TY K,0,3,1,0`.
//!
//! ★ And the A manual is not sufficient either: it prints menu 611's interval
//! list with **8** entries and the radio has at least **10**. Three readings
//! (`04`=`3 min`, `05`=`5 min`, `09`=`60 min`) fit only
//! `0.2/0.5/1/2/3/5/10/20/30/60`. Take **defaults** from the manual — those
//! cross-check perfectly — and **lists** from the radio.
//!
//! ## ★★★ Menu 625 is not in the image (s132, desk work, no radio time)
//!
//! Search a menu's whole **default vector**, not one byte. Every menu measured
//! so far lays its settings out contiguously in the manual's printed order —
//! menu 601's four at `+0x00C`..`+0x00F`, menu 606's three at `+0x083`..`+0x085`
//! — so a menu's defaults are a byte *string*, and a string is far rarer than
//! any byte in it.
//!
//! Menu **625 INTERRUPT DISPLAY** defaults to `DISPLAY AREA` = `ENTIRE` (index
//! `02` of `OFF/HALF/ENTIRE`), `AUTO BRIGHTNESS` = `ON` (`01`), `CHANGE COLOR`
//! = `ON` (`01`). The pattern `02 01 01` occurs **zero times in the entire
//! 39 840 bytes** — live copy, all five factory PM copies, APRS block, config
//! blocks, everywhere. So do `04 01 02 01 01` (624's tail plus 625) and the
//! full ten-byte 624-627 default vector.
//!
//! The APRS block is independently ruled out by counting anchors in a factory
//! copy: it holds exactly **one** byte equal to `02` (`+0x00F`, TX delay) and
//! exactly **one** equal to `04` (`+0x16E`, beacon interval), both already
//! spoken for. `DISPLAY AREA` = `02` and `RX BEEP` = `04` have nowhere to live.
//!
//! ⚠ This names **no** offset and must not be read as one. `0x0377` = `04`
//! followed by `01 01` is the only unclaimed `04` in a config block outside the
//! VFO table — and `0x0355` has the identical shape with CHIRP naming its
//! neighbour `beepvol`. A byte that merely fits is not evidence.
//!
//! ★ What it changes: the "invisible enum at its default" argument that rescued
//! menus 612 and 613 **cannot** rescue 624/625, whose defaults are non-zero and
//! would therefore show. Either those settings are non-contiguous, or they live
//! in `0x9C00`-`0xFEEF` — the 25 328 addresses [`read_plan`] has never
//! requested. That puts the two-minute probe of `0x9C00` back near the top of
//! the list, ahead of hunting 624-627 from the front panel.
//!
//! ⚠ The refutation rests on the contiguity rule, which stands on **two**
//! menus. Best rule available; not a law.

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
    use super::kenwood_tmd710::image::{ProgramMode, HOLE};

    let path = port_path();
    let mut p = open(&path, 57600).expect("open");
    let _ = ask(&mut *p, "ID");
    let id = ask(&mut *p, "ID").expect("ID").0;
    assert!(id.contains("TM-D710"), "not a TM-D710: {id:?}");

    // `ProgramMode` sends `E` from Drop, so a panic in here still leaves the
    // radio usable — which is the whole reason the transport owns the session
    // rather than the harness.
    let image = {
        let mut prog = ProgramMode::enter(&mut *p).expect("enter program mode");
        let image = prog.read_image().expect("read the image");
        prog.leave().expect("leave program mode");
        image
    };

    println!("=== {} bytes read, hole at 0x{HOLE:04X} skipped", image.bytes_read());
    assert_eq!(image.bytes_read(), 39_840, "the ritual did not return the whole image");

    let out = format!("../scratchpad/kenwood_tmd710/progfull-{}.bin", std::process::id());
    std::fs::write(&out, image.as_addressed_bytes()).expect("write");
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

/// ★★★ The image WRITE path, climbed one rung at a time on the narrowest,
/// least destructive field this radio has.
///
/// Nothing had ever been written to this radio's image before this test. The
/// rungs are the `new-radio` ladder adapted from a container to a transport:
///
/// 1. **Identity write** — write a field back byte-for-byte and read it back.
///    Proves the verb, the framing and the acknowledgement with **nothing at
///    risk**: if every byte is the one already there, a total success and a
///    total no-op are the same outcome.
/// 2. **One field** — change it, read it back, leave the mode, come back and
///    read it again. The second read is the one that matters. This protocol has
///    no checksum and no commit step, so a value that survives a re-entry is
///    the only evidence anything was *stored* rather than echoed.
/// 3. **The screen** — and that one is not in here. ✅ Done on 2026-09-02: Tim
///    read `CPMAGIC TEST 129` off menu 608. A read-back proves the bytes are in
///    the image; only the radio's own display proves the image is what the
///    radio's settings are.
///
/// ## Why status text 3
///
/// It is **empty on Tim's radio** (`FF` x 42), so there is no operator data to
/// lose, and it is directly visible on the radio's own screen.
///
/// ## ⚠ This is a NARROW write and that is the thing being tested
///
/// CHIRP writes the whole 156-block image wrapped in an invalidate/revalidate
/// dance. This writes **42 bytes** and touches no header at all. That could
/// simply not commit: the BT-9000 has a segment that acknowledges a partial
/// write and silently keeps the old contents. It does commit — measured — and
/// that is the difference between "to change a status text, rewrite the
/// operator's entire radio" and not.
#[test]
#[ignore = "requires a TM-D710 on the cable — WRITES to the radio"]
fn d710_status_text_write_ladder() {
    use super::kenwood_tmd710::image::{ProgramMode, APRS_LIVE};

    const TEXT_LEN: usize = 42;
    let addr = APRS_LIVE + STATUS_TEXT_3;
    let probe: Vec<u8> = {
        let mut v = b"CPMAGIC TEST 129".to_vec();
        v.resize(TEXT_LEN, 0x00);
        v
    };

    let path = port_path();
    let mut p = open(&path, 57600).expect("open");
    let _ = ask(&mut *p, "ID");
    assert!(ask(&mut *p, "ID").expect("ID").0.contains("TM-D710"));

    let before = {
        let mut prog = ProgramMode::enter(&mut *p).expect("enter");
        let before = prog.read(addr, TEXT_LEN as u8).expect("read status text 3");
        println!("\nbefore      {}", show(&before));

        prog.write(addr, &before).expect("identity write");
        let same = prog.read(addr, TEXT_LEN as u8).expect("read back after identity");
        assert_eq!(same, before, "an identity write changed the field");
        println!("identity ok, unchanged");

        prog.write(addr, &probe).expect("write the probe text");
        let after = prog.read(addr, TEXT_LEN as u8).expect("read back after write");
        println!("after write {}", show(&after));
        assert_eq!(after, probe, "the read-back does not match what was written");
        prog.leave().expect("leave");
        before
    };

    // Rung 2b — the one that separates a stored value from an echoed one.
    std::thread::sleep(Duration::from_millis(500));
    let persisted = {
        let mut prog = ProgramMode::enter(&mut *p).expect("re-enter");
        let got = prog.read(addr, TEXT_LEN as u8).expect("read after re-entering");
        prog.leave().expect("leave");
        got
    };
    println!("after re-entry {}", show(&persisted));

    assert_ne!(
        persisted, before,
        "the field is back to what it was: the narrow write was acknowledged and NOT committed"
    );
    assert_eq!(persisted, probe, "the field changed, but not to what was written");
    println!("\n★ narrow image write PROVEN over a re-entry. Check menu 608 on the radio.");
}

/// Status text 3 of 5, as an offset into the APRS block: `[1 flag][42 text]`
/// entries from `+0x089`.
const STATUS_TEXT_3: u16 = 0x089 + 2 * 44 + 1;

/// Put status text 3 back to the `FF`-filled empty it was before the ladder.
#[test]
#[ignore = "requires a TM-D710 on the cable — WRITES to the radio"]
fn d710_restore_status_text_3() {
    use super::kenwood_tmd710::image::{ProgramMode, APRS_LIVE};

    const TEXT_LEN: usize = 42;
    let addr = APRS_LIVE + STATUS_TEXT_3;
    let empty = vec![0xFFu8; TEXT_LEN];

    let path = port_path();
    let mut p = open(&path, 57600).expect("open");
    let _ = ask(&mut *p, "ID");
    assert!(ask(&mut *p, "ID").expect("ID").0.contains("TM-D710"));

    let mut prog = ProgramMode::enter(&mut *p).expect("enter");
    prog.write(addr, &empty).expect("restore");
    let back = prog.read(addr, TEXT_LEN as u8).expect("read back");
    prog.leave().expect("leave");

    println!("restored to {}", show(&back));
    assert_eq!(back, empty, "status text 3 is not back to empty");
}

/// Put the whole live APRS block back from a saved dump.
///
/// This is what makes a front-panel measurement pass reversible, and it is why
/// the write path was built before the campaign rather than after it: without
/// it, every setting changed to let a diff name an offset is a setting the
/// operator has to re-enter by hand from memory.
///
/// `D710_IMAGE=<path>` names a `d710_program_mode_dump_full` file — 64 KiB with
/// `FF` for anything unread, so a file offset is a radio address. Only the
/// 1152 bytes of the **live** block are written; PM1-5 are the operator's saved
/// profiles and nothing here has any business touching them.
#[test]
#[ignore = "requires a TM-D710 on the cable — WRITES to the radio"]
fn d710_restore_aprs_block() {
    use super::kenwood_tmd710::image::{ProgramMode, APRS_BLOCK_LEN, APRS_LIVE, IMAGE_SPAN};

    let src = std::env::var("D710_IMAGE").expect("set D710_IMAGE to a full-dump file");
    let image = std::fs::read(&src).expect("read the dump");
    assert_eq!(image.len(), IMAGE_SPAN, "{src} is not a 64 KiB full dump");
    let want = &image[APRS_LIVE as usize..APRS_LIVE as usize + APRS_BLOCK_LEN];
    assert!(
        want.iter().any(|b| *b != 0xFF),
        "the APRS block in {src} is all FF — that dump never read this region"
    );

    let path = port_path();
    let mut p = open(&path, 57600).expect("open");
    let _ = ask(&mut *p, "ID");
    assert!(ask(&mut *p, "ID").expect("ID").0.contains("TM-D710"));

    let mut prog = ProgramMode::enter(&mut *p).expect("enter");
    let mut written = 0usize;
    for off in (0..APRS_BLOCK_LEN).step_by(256) {
        let n = 256.min(APRS_BLOCK_LEN - off);
        let addr = APRS_LIVE + off as u16;
        let chunk = &want[off..off + n];
        prog.write(addr, chunk).unwrap_or_else(|e| panic!("write 0x{addr:04X}: {e}"));
        // ⚠ Read back every block before moving on: an acknowledged write that did
        // not commit is a failure this protocol can produce and would otherwise
        // report as success.
        let back = prog
            .read(addr, if n == 256 { 0 } else { n as u8 })
            .unwrap_or_else(|e| panic!("read back 0x{addr:04X}: {e}"));
        assert_eq!(back, chunk, "0x{addr:04X} did not take the write");
        written += n;
    }
    prog.leave().expect("leave");
    println!("restored {written} bytes of the live APRS block from {src}");
}

/// Poke individual bytes of the **live** APRS block and read them back.
///
/// The inverse of a front-panel measurement pass. `PASS-A.md` asks the operator
/// to move a menu so a diff can name the byte; that only works for fields whose
/// byte actually moves, and the four this campaign still wants — `DATA SPEED`,
/// `DCD SENSE`, `POSITION AMBIGUITY`, `POSITION COMMENT` — all sit on the first
/// entry of their list, so they read `00` in the live block *and* `00` in the PM
/// defaults and no differential can see them.
///
/// So run it the other way: write a distinct value into each candidate byte and
/// let the operator read the menus. A menu that comes back showing entry `N`
/// names its own offset, because exactly one byte was given the value `N`.
///
/// `D710_POKE="160=01,161=02"` — offsets are hex and **relative to the live
/// block base**, matching `APRS-BLOCK.md`; values are hex. Only the 256-byte
/// pages that actually contain a poke are written, and every one is read back
/// before the next: this protocol acknowledges writes that did not commit.
///
/// Reverse it with `d710_restore_aprs_block`.
#[test]
#[ignore = "requires a TM-D710 on the cable — WRITES to the radio"]
fn d710_poke_aprs() {
    use super::kenwood_tmd710::image::{ProgramMode, APRS_BLOCK_LEN, APRS_LIVE};

    let spec = std::env::var("D710_POKE")
        .expect("set D710_POKE to off=val[,off=val...] — hex, offsets relative to the live APRS block");
    let pokes: Vec<(usize, u8)> = spec
        .split(',')
        .filter(|s| !s.trim().is_empty())
        .map(|kv| {
            let (o, v) = kv.split_once('=').unwrap_or_else(|| panic!("{kv:?} is not off=val"));
            let off = usize::from_str_radix(o.trim(), 16)
                .unwrap_or_else(|_| panic!("offset {o:?} is not hex"));
            let val = u8::from_str_radix(v.trim(), 16)
                .unwrap_or_else(|_| panic!("value {v:?} is not hex"));
            assert!(off < APRS_BLOCK_LEN, "+0x{off:03X} is outside the {APRS_BLOCK_LEN}-byte live block");
            (off, val)
        })
        .collect();
    assert!(!pokes.is_empty(), "D710_POKE named no bytes");
    // Two pokes at one offset would make the read-back check pass while the
    // second value silently won, and the whole method rests on one value per byte.
    let mut seen: Vec<usize> = pokes.iter().map(|(o, _)| *o).collect();
    seen.sort_unstable();
    seen.dedup();
    assert_eq!(seen.len(), pokes.len(), "D710_POKE names the same offset twice");

    let path = port_path();
    let mut p = open(&path, 57600).expect("open");
    let _ = ask(&mut *p, "ID");
    assert!(ask(&mut *p, "ID").expect("ID").0.contains("TM-D710"));

    let mut prog = ProgramMode::enter(&mut *p).expect("enter");

    // Read every page that a poke touches, so the bytes around it go back untouched.
    let mut pages: Vec<usize> = pokes.iter().map(|(o, _)| o / 256 * 256).collect();
    pages.sort_unstable();
    pages.dedup();

    let mut before: Vec<(usize, u8, u8)> = vec![];
    for base in pages {
        let n = 256.min(APRS_BLOCK_LEN - base);
        let addr = APRS_LIVE + base as u16;
        let mut page = prog
            .read(addr, if n == 256 { 0 } else { n as u8 })
            .unwrap_or_else(|e| panic!("read 0x{addr:04X}: {e}"));
        for (off, val) in pokes.iter().filter(|(o, _)| o / 256 * 256 == base) {
            before.push((*off, page[off - base], *val));
            page[off - base] = *val;
        }
        prog.write(addr, &page).unwrap_or_else(|e| panic!("write 0x{addr:04X}: {e}"));
        let back = prog
            .read(addr, if n == 256 { 0 } else { n as u8 })
            .unwrap_or_else(|e| panic!("read back 0x{addr:04X}: {e}"));
        assert_eq!(back, page, "0x{addr:04X} did not take the write");
    }
    prog.leave().expect("leave");

    before.sort_unstable_by_key(|(o, _, _)| *o);
    println!("\n  offset   was -> now   (decimal value the radio should show as its list index)");
    for (off, was, now) in &before {
        println!("  +0x{off:03X}    {was:02X} -> {now:02X}     {now}");
    }
    println!("\n{} bytes poked; read back clean. Restore with d710_restore_aprs_block.", before.len());
}

/// Bytes as hex plus their printable reading, which is how every field in this
/// block has to be looked at: half of them are text and half are not.
fn show(b: &[u8]) -> String {
    let t: String = b.iter().map(|c| if (0x20..0x7F).contains(c) { *c as char } else { '.' }).collect();
    format!("{}  |{}|", b.iter().map(|c| format!("{c:02X}")).collect::<Vec<_>>().join(""), t)
}

