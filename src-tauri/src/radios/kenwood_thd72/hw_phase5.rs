//! THROWAWAY (issue #55, Phase 5): the hardware write ladder, in order,
//! stopping at the first failure.
//!
//! Each step asserts its property **in the image, before the radio sees it**, so
//! a failure is the radio's answer rather than a bad file.
//!
//! The noise floor from Phase 1 is what makes these comparisons possible: two
//! downloads with nothing changed differ in exactly six bytes — `0x0246` (last
//! menu number) and four ASCII digits around `0xA89E` that the radio rewrites on
//! its own. Everything else is stable, so a difference anywhere else is ours.
//!
//! ```sh
//! CPM_THD72_PORT=/dev/cu.SLAB_USBtoUART \
//!   cargo test --lib kenwood_thd72::hw_phase5::step1 -- --ignored --nocapture
//! ```

use super::{container, layout, memory, protocol};
use crate::radios::driver::{ImageProgrammer, ImageRestorer};

/// Bytes the radio changes by itself between two reads (Phase 1, measured).
const VOLATILE: [std::ops::Range<usize>; 2] = [0x0246..0x0247, 0xA890..0xA8C0];

fn is_volatile(i: usize) -> bool {
    VOLATILE.iter().any(|r| r.contains(&i))
}

/// Compare two images, ignoring only the bytes Phase 1 proved the radio moves on
/// its own. Returns the offsets that differ for real.
fn real_differences(a: &[u8], b: &[u8]) -> Vec<usize> {
    (0..a.len().min(b.len()))
        .filter(|&i| a[i] != b[i] && !is_volatile(i))
        .collect()
}

fn port() -> String {
    std::env::var("CPM_THD72_PORT").expect("CPM_THD72_PORT")
}

fn out_dir() -> std::path::PathBuf {
    std::path::PathBuf::from(
        std::env::var("CPM_THD72_DIR")
            .expect("CPM_THD72_DIR — where to keep the before/after images"),
    )
}

/// **Ladder step 1 — identity write.** Put the radio's own bytes back and see
/// that it accepts them.
///
/// Proves the block-write protocol with nothing at risk: even a write that fails
/// halfway has written the radio its own data. It is deliberately NOT the
/// checksum test — an identical copy carries any internal digest along unchanged,
/// which is what step 2 exists for.
#[test]
#[ignore = "writes to a real TH-D72"]
fn step1_identity_write() {
    let dir = out_dir();
    let mut p = protocol::open_port(&port()).expect("open");
    let ident = protocol::identify(&mut *p).expect("identify");
    println!("radio: {}", ident.matched);

    let before = protocol::download(&mut *p).expect("download before");
    std::fs::write(dir.join("ww8l-step1-before.img"), &before).expect("save before");
    println!("downloaded {} bytes", before.len());

    // Assert the property in the file first: what we are about to send IS what
    // the radio just gave us.
    let payload = before.clone();
    assert_eq!(payload, before, "the identity write must send the radio's own bytes");
    super::DRIVER
        .check_restore_image(&payload)
        .expect("the driver's own guard must accept this radio's image");

    // Release the port before handing it to the driver: `upload_image` opens it
    // itself, in one session, exactly as the app does. Holding it here made the
    // driver's own open fail with "Device or resource busy" — a harness bug, but
    // the right shape to keep, because the thing under test is the driver's
    // whole one-session write and not a socket this test happens to own.
    drop(p);
    println!("uploading 254 blocks…");
    super::DRIVER.upload_image(&port(), &payload).expect("upload");

    let mut p = protocol::open_port(&port()).expect("reopen");
    protocol::identify(&mut *p).expect("re-identify");
    let after = protocol::download(&mut *p).expect("download after");
    std::fs::write(dir.join("ww8l-step1-after.img"), &after).expect("save after");

    let diffs = real_differences(&before, &after);
    let volatile_moved = (0..before.len()).filter(|&i| before[i] != after[i] && is_volatile(i)).count();
    println!("read back: {} real differences, {volatile_moved} volatile bytes moved", diffs.len());
    for &i in diffs.iter().take(20) {
        println!("  0x{i:04X}: {:02x} -> {:02x}", before[i], after[i]);
    }

    let count = (0..layout::CHANNEL_COUNT)
        .filter(|&s| memory::read_record(&after, s).is_some())
        .count();
    println!("{count} memories still on the radio");

    assert!(diffs.is_empty(), "the radio came back different in {} bytes", diffs.len());
    assert_eq!(count, 53, "the radio should still hold its 53 memories");
}

/// Recovery: end a clone session that was abandoned partway.
///
/// A failed upload leaves the radio in program mode at [`protocol::BAUD_CLONE`],
/// where it no longer answers ASCII — `ID` comes back empty and every other
/// command looks broken. `E` is the documented way out, and it is worth trying
/// before asking a human to power-cycle: see [[fix-the-instrument-not-the-human]].
#[test]
#[ignore = "recovery tool"]
fn end_an_abandoned_clone_session() {
    use std::io::Write;
    let mut p = serialport::new(port(), protocol::BAUD_CLONE)
        .timeout(std::time::Duration::from_millis(500))
        .open()
        .expect("open at clone rate");
    p.write_all(b"E").expect("send E");
    p.flush().ok();
    std::thread::sleep(std::time::Duration::from_millis(300));
    drop(p);

    let mut p = protocol::open_port(&port()).expect("reopen");
    match protocol::identify(&mut *p) {
        Ok(id) => println!("recovered — radio answers as {}", id.matched),
        Err(e) => println!("still not answering: {e}"),
    }
}

/// Diagnostic: how far does a write actually get?
///
/// Step 1 failed with a bare "Broken pipe" and no position, which means it died
/// before the block loop or very early in it. This walks the write path one
/// stage at a time, printing between stages, writing the radio's OWN bytes so
/// every outcome is harmless.
#[test]
#[ignore = "writes to a real TH-D72"]
fn diagnose_the_write_path() {
    let mut p = protocol::open_port(&port()).expect("open");
    protocol::identify(&mut *p).expect("identify");
    let image = protocol::download(&mut *p).expect("download");
    println!("downloaded ok");
    drop(p);
    std::thread::sleep(std::time::Duration::from_millis(500));

    for count in [1usize, 4, 16, 64, 254] {
        let blocks: Vec<usize> = (0..count).collect();
        let mut p = protocol::open_port(&port()).expect("reopen");
        protocol::identify(&mut *p).expect("re-identify");
        let started = std::time::Instant::now();
        match protocol::upload(&mut *p, &image, &blocks) {
            Ok(()) => println!(
                "  {count:3} blocks: OK in {:.1}s",
                started.elapsed().as_secs_f64()
            ),
            Err(e) => {
                println!("  {count:3} blocks: FAILED after {:.1}s — {e}", started.elapsed().as_secs_f64());
                drop(p);
                let _ = end_session_quietly();
                return;
            }
        }
        drop(p);
        std::thread::sleep(std::time::Duration::from_millis(500));
    }
}

fn end_session_quietly() -> Result<(), String> {
    use std::io::Write;
    let mut p = serialport::new(port(), protocol::BAUD_CLONE)
        .timeout(std::time::Duration::from_millis(500))
        .open()
        .map_err(|e| e.to_string())?;
    p.write_all(b"E").map_err(|e| e.to_string())?;
    p.flush().ok();
    Ok(())
}

/// Diagnostic: is hardware flow control what breaks the second session?
///
/// The recovery helper reopened the port with `FlowControl::None` and the radio
/// answered `ID` immediately. `open_port` uses `FlowControl::Hardware` on macOS,
/// following CHIRP's `HARDWARE_FLOW = sys.platform == "darwin"`. Those are the
/// two variables; this changes one at a time.
#[test]
#[ignore = "talks to a real TH-D72"]
fn does_flow_control_break_the_second_session() {
    let open = |flow: serialport::FlowControl| {
        serialport::new(port(), protocol::BAUD_INITIAL)
            .flow_control(flow)
            .timeout(std::time::Duration::from_secs(1))
            .open()
    };

    for (label, flow) in [
        ("Hardware", serialport::FlowControl::Hardware),
        ("None", serialport::FlowControl::None),
    ] {
        println!("\n=== flow control: {label} ===");
        let mut p = open(flow).expect("open");
        match protocol::identify(&mut *p) {
            Ok(id) => println!("  first identify:  OK ({})", id.matched),
            Err(e) => println!("  first identify:  FAILED - {e}"),
        }
        let downloaded = protocol::download(&mut *p).map(|i| i.len());
        println!("  download:        {downloaded:?}");
        drop(p);
        std::thread::sleep(std::time::Duration::from_millis(500));

        let mut p = open(flow).expect("reopen");
        let started = std::time::Instant::now();
        match protocol::identify(&mut *p) {
            Ok(id) => println!(
                "  second identify: OK ({}) in {:.1}s",
                id.matched,
                started.elapsed().as_secs_f64()
            ),
            Err(e) => println!(
                "  second identify: FAILED after {:.1}s - {e}",
                started.elapsed().as_secs_f64()
            ),
        }
        drop(p);
        std::thread::sleep(std::time::Duration::from_millis(500));
    }
}

/// **Ladder step 1, second attempt — identity write in ONE session.**
///
/// The first attempt closed the port between the download and the upload,
/// because `upload_image` opens its own. That fails: after a completed clone
/// read the reopened port is dead, and a write to it returns "Broken pipe"
/// several seconds later. Flow control is not the variable — it fails the same
/// way with and without.
///
/// This is the shape `ImageProgrammer::program_codeplug` already uses, and the
/// shape the rest of this codebase settled on for the AnyTone: **one radio
/// operation per port, and do not hand a second one a reopened port.**
#[test]
#[ignore = "writes to a real TH-D72"]
fn step1_identity_write_one_session() {
    let dir = out_dir();
    let mut p = protocol::open_port(&port()).expect("open");
    let ident = protocol::identify(&mut *p).expect("identify");
    println!("radio: {}", ident.matched);

    let before = protocol::download(&mut *p).expect("download");
    std::fs::write(dir.join("ww8l-step1-before.img"), &before).expect("save before");
    println!("downloaded {} bytes", before.len());

    // The property, asserted before the radio sees anything: we are sending back
    // exactly what it just gave us.
    let payload = before.clone();
    super::DRIVER
        .check_restore_image(&payload)
        .expect("the driver's own guard must accept this radio's image");

    let blocks: Vec<usize> = (0..254).collect();
    let started = std::time::Instant::now();
    protocol::upload(&mut *p, &payload, &blocks).expect("upload");
    println!("uploaded 254 blocks in {:.1}s", started.elapsed().as_secs_f64());

    let after = protocol::download(&mut *p).expect("download after");
    std::fs::write(dir.join("ww8l-step1-after.img"), &after).expect("save after");

    let diffs = real_differences(&before, &after);
    let volatile = (0..before.len()).filter(|&i| before[i] != after[i] && is_volatile(i)).count();
    println!("read back: {} real differences, {volatile} volatile bytes moved", diffs.len());
    for &i in diffs.iter().take(20) {
        println!("  0x{i:04X}: {:02x} -> {:02x}", before[i], after[i]);
    }
    let count = (0..layout::CHANNEL_COUNT)
        .filter(|&s| memory::read_record(&after, s).is_some())
        .count();
    println!("{count} memories still on the radio");

    assert!(diffs.is_empty(), "the radio came back different in {} bytes", diffs.len());
    assert_eq!(count, 53);
}

/// **Ladder step 1, third attempt — upload as the FIRST clone operation.**
///
/// What the two failures established: the upload dies in `enter_program`, not in
/// the block loop (the loop attaches a block number and the error has none), and
/// it dies the same way whether the port was reopened or held. The common factor
/// is a **completed clone session before it** — a download that ended with `E`.
/// Flow control is not the variable.
///
/// So this does the write with no download in front of it, reading the payload
/// from the image already saved off this radio. If it succeeds, the rule is one
/// clone-mode operation per USB connection, and `program_codeplug`'s
/// download-then-upload design has to change. That would match what this repo
/// already learned on the AnyTone.
#[test]
#[ignore = "writes to a real TH-D72"]
fn step1_upload_first_on_a_fresh_connection() {
    let dir = out_dir();
    let payload = std::fs::read(dir.join("ww8l-asfound.img")).expect("the saved image");
    assert_eq!(payload.len(), layout::IMAGE_LEN);
    super::DRIVER.check_restore_image(&payload).expect("guard");

    let mut p = protocol::open_port(&port()).expect("open");
    let ident = protocol::identify(&mut *p).expect("identify");
    println!("radio: {}", ident.matched);

    let blocks: Vec<usize> = (0..254).collect();
    let started = std::time::Instant::now();
    match protocol::upload(&mut *p, &payload, &blocks) {
        Ok(()) => println!("UPLOADED 254 blocks in {:.1}s", started.elapsed().as_secs_f64()),
        Err(e) => {
            println!("FAILED after {:.1}s - {e}", started.elapsed().as_secs_f64());
            panic!("upload failed");
        }
    }
}

/// Diagnostic: how long does the radio need after a clone session?
///
/// Revised reading of the failures. A reopen 500 ms after a completed download
/// dies with "Broken pipe"; a reopen after a whole process restart works. That
/// is a settling time, not "one session per connection" — and this measures it
/// instead of guessing, so the number in the driver is one the radio gave us.
#[test]
#[ignore = "talks to a real TH-D72"]
fn how_long_after_a_clone_session_before_the_radio_answers() {
    let mut p = protocol::open_port(&port()).expect("open");
    protocol::identify(&mut *p).expect("identify");
    let image = protocol::download(&mut *p).expect("download");
    println!("downloaded {} bytes; closing the port", image.len());
    drop(p);

    let started = std::time::Instant::now();
    for attempt in 1..=10 {
        std::thread::sleep(std::time::Duration::from_millis(1000));
        let elapsed = started.elapsed().as_secs_f64();
        match protocol::open_port(&port()) {
            Ok(mut p) => match protocol::identify(&mut *p) {
                Ok(id) => {
                    println!("  +{elapsed:.1}s attempt {attempt}: OK ({})", id.matched);
                    return;
                }
                Err(e) => println!("  +{elapsed:.1}s attempt {attempt}: {e}"),
            },
            Err(e) => println!("  +{elapsed:.1}s attempt {attempt}: could not open - {e}"),
        }
    }
    panic!("the radio never came back");
}

/// **Ladder step 1 — identity write, with the settling time the radio asked for.**
///
/// Download, reconnect, write the radio's own bytes back, reconnect, read them
/// again. Proves the block-write protocol with nothing at risk: even a write
/// that fails halfway has written the radio its own data.
///
/// It is deliberately NOT the checksum test — an identical copy carries any
/// internal digest along unchanged. That is step 2.
#[test]
#[ignore = "writes to a real TH-D72"]
fn step1_identity_write_final() {
    let dir = out_dir();
    let mut p = protocol::open_port(&port()).expect("open");
    println!("radio: {}", protocol::identify(&mut *p).expect("identify").matched);

    let before = protocol::download(&mut *p).expect("download");
    std::fs::write(dir.join("ww8l-step1-before.img"), &before).expect("save");
    println!("downloaded {} bytes", before.len());
    drop(p);

    // The property, asserted before the radio sees anything.
    let payload = before.clone();
    super::DRIVER.check_restore_image(&payload).expect("guard accepts this radio's image");

    println!("reconnecting…");
    let mut p = protocol::reconnect_after_clone(&port()).expect("reconnect after read");
    let started = std::time::Instant::now();
    protocol::upload(&mut *p, &payload, &(0..254).collect::<Vec<_>>()).expect("upload");
    println!("uploaded 254 blocks in {:.1}s", started.elapsed().as_secs_f64());
    drop(p);

    println!("reconnecting…");
    let mut p = protocol::reconnect_after_clone(&port()).expect("reconnect after write");
    let after = protocol::download(&mut *p).expect("download after");
    std::fs::write(dir.join("ww8l-step1-after.img"), &after).expect("save");

    let diffs = real_differences(&before, &after);
    let volatile = (0..before.len()).filter(|&i| before[i] != after[i] && is_volatile(i)).count();
    println!("read back: {} real differences, {volatile} volatile bytes moved", diffs.len());
    for &i in diffs.iter().take(20) {
        println!("  0x{i:04X}: {:02x} -> {:02x}", before[i], after[i]);
    }
    let count = (0..layout::CHANNEL_COUNT)
        .filter(|&s| memory::read_record(&after, s).is_some())
        .count();
    println!("{count} memories still on the radio");

    assert!(diffs.is_empty(), "the radio came back different in {} bytes", diffs.len());
    assert_eq!(count, 53);
}

/// **Ladder step 2 — one-byte write. THIS is the checksum test.**
///
/// Step 1 cannot be: a byte-identical copy carries any internal digest along
/// unchanged, so the radio would accept it even if one existed. Changing a
/// single memory name makes the stored bytes disagree with any digest over
/// them. If the radio accepted step 1 and rejects this, there is a checksum to
/// find — and this project has been here before, on the FT5D, where getting it
/// wrong cost a factory reset.
///
/// Phase 1's diff evidence says there is nothing to find: two radio-made
/// single-channel edits moved 25 bytes each and none outside the flag, channel
/// and name regions. This is the test that turns that into an answer.
///
/// It is also the **partial upload** test. One name lives in one 256-byte block,
/// so this writes exactly one block and leaves the other 253 untouched — the
/// property the whole program path depends on.
#[test]
#[ignore = "writes to a real TH-D72"]
fn step2_one_name_write() {
    let dir = out_dir();
    let mut p = protocol::open_port(&port()).expect("open");
    println!("radio: {}", protocol::identify(&mut *p).expect("identify").matched);
    let before = protocol::download(&mut *p).expect("download");
    std::fs::write(dir.join("ww8l-step2-before.img"), &before).expect("save");
    drop(p);

    // Pick the highest programmed memory, so the radio's first screen is left
    // alone and the change is somewhere Tim has to go and look.
    let slot = (0..layout::CHANNEL_COUNT)
        .rfind(|&s| memory::read_record(&before, s).is_some())
        .expect("the radio holds memories");
    let mut record = memory::read_record(&before, slot).expect("record");
    let old: String = record.name.iter().take_while(|&&b| b != 0xFF).map(|&b| b as char).collect();
    record.set_name("D72TEST");
    println!("memory {slot}: {old:?} -> \"D72TEST\"");

    let mut image = container::Thd72Image::parse(&before).expect("parse");
    for (off, cell) in memory::apply_record(slot, &record) {
        image.patch(off, &cell).expect("patch");
    }
    let blocks = image.dirty_blocks();
    let built = image.into_raw();

    // Assert the properties in the image, before the radio sees it.
    assert_eq!(blocks.len(), 1, "one name must touch exactly one block, not {blocks:?}");
    let changed: Vec<usize> = (0..built.len()).filter(|&i| built[i] != before[i]).collect();
    println!("changed {} bytes in block {:?}", changed.len(), blocks);
    assert!(changed.iter().all(|&i| (0x5E00..0x7D40).contains(&i)), "only the name cell may change");

    println!("reconnecting…");
    let mut p = protocol::reconnect_after_clone(&port()).expect("reconnect");
    protocol::upload(&mut *p, &built, &blocks).expect("upload one block");
    println!("uploaded block {:?}", blocks);
    drop(p);

    println!("reconnecting…");
    let mut p = protocol::reconnect_after_clone(&port()).expect("reconnect");
    let after = protocol::download(&mut *p).expect("download after");
    std::fs::write(dir.join("ww8l-step2-after.img"), &after).expect("save");

    let diffs = real_differences(&built, &after);
    println!("read back vs intended: {} real differences", diffs.len());
    for &i in diffs.iter().take(20) {
        println!("  0x{i:04X}: {:02x} -> {:02x}", built[i], after[i]);
    }

    let back = memory::read_record(&after, slot).expect("the memory must still exist");
    let name: String = back.name.iter().take_while(|&&b| b != 0xFF).map(|&b| b as char).collect();
    let count = (0..layout::CHANNEL_COUNT).filter(|&s| memory::read_record(&after, s).is_some()).count();
    println!("memory {slot} now reads {name:?}; {count} memories on the radio");

    assert!(diffs.is_empty(), "the radio did not store what we sent");
    assert_eq!(name, "D72TEST", "the name did not change on the radio");
    assert_eq!(count, 53, "no memory may be lost by a name write");
    println!("\n>>> Look at memory {slot} on the radio: it should read D72TEST (was {old:?}).");
}
