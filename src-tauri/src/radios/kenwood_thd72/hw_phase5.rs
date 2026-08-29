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

/// **Ladder step 3 — the full codeplug.**
///
/// Writes the image `program::dev_export` built from the real dev database, so
/// what reaches the radio is the app's own output and not a second
/// implementation that happens to agree.
///
/// Reports the memory list **per group**, because the D72's groups are fixed
/// positional blocks of 100 and a group that did not get written is invisible
/// from the screen the radio powers up on.
#[test]
#[ignore = "OVERWRITES the radio's memories"]
fn step3_full_codeplug() {
    let dir = out_dir();
    let built = std::fs::read(dir.join("ww8l-step3-built.img")).expect("built image");
    let base = std::fs::read(dir.join("ww8l-step3-base.img")).expect("base image");
    assert_eq!(built.len(), layout::IMAGE_LEN);

    let blocks: Vec<usize> = (0..layout::BLOCK_COUNT)
        .filter(|i| {
            let r = i * layout::BLOCK_LEN..(i + 1) * layout::BLOCK_LEN;
            base[r.clone()] != built[r]
        })
        .collect();
    let intended = (0..layout::CHANNEL_COUNT)
        .filter(|&s| memory::read_record(&built, s).is_some())
        .count();
    println!("{intended} memories in the built image; {} blocks differ from the radio", blocks.len());
    assert!(!blocks.is_empty(), "nothing to write");

    let mut p = protocol::open_port(&port()).expect("open");
    println!("radio: {}", protocol::identify(&mut *p).expect("identify").matched);
    protocol::upload(&mut *p, &built, &blocks).expect("upload the codeplug");
    println!("uploaded {} blocks", blocks.len());
    drop(p);

    println!("reconnecting…");
    let mut p = protocol::reconnect_after_clone(&port()).expect("reconnect");
    let after = protocol::download(&mut *p).expect("download after");
    std::fs::write(dir.join("ww8l-step3-after.img"), &after).expect("save");

    // ★ Compare only the blocks we WROTE. The radio rewrites its own
    // current-channel state in 0x0200-0x0400 when the memories underneath it
    // change — 18 bytes on this run — and those are not ours to verify.
    let bad: Vec<usize> = blocks
        .iter()
        .copied()
        .filter(|i| {
            let r = i * layout::BLOCK_LEN..(i + 1) * layout::BLOCK_LEN;
            built[r.clone()] != after[r]
        })
        .collect();
    let elsewhere = (0..built.len()).filter(|&i| built[i] != after[i]).count();
    println!(
        "read back: {} of {} written blocks differ; {elsewhere} bytes differ outside what we wrote",
        bad.len(),
        blocks.len()
    );

    // Per group, because the radio only shows one at a time.
    let table = container::Thd72Image::parse(&after).unwrap().prog_vfo_table().unwrap();
    let mut per_group = [0usize; layout::GROUP_COUNT];
    let mut misbanded = Vec::new();
    let mut on_radio = 0usize;
    for slot in 0..layout::CHANNEL_COUNT {
        let Some(rec) = memory::read_record(&after, slot) else { continue };
        on_radio += 1;
        per_group[layout::group_of(slot)] += 1;
        let m = memory::decode_memory(&rec.memory);
        if layout::prog_vfo_index(&table, m.freq_hz) != Some(rec.prog_vfo()) {
            misbanded.push((slot, m.freq_hz));
        }
    }
    println!("\n{on_radio} memories on the radio, by group:");
    for (g, n) in per_group.iter().enumerate() {
        if *n > 0 {
            println!("  group {g} (memories {}-{}): {n}", g * 100, g * 100 + 99);
        }
    }
    println!("\nmis-banded (would not transmit): {}", misbanded.len());
    for (slot, hz) in misbanded.iter().take(10) {
        println!("  memory {slot} at {:.4} MHz", *hz as f64 / 1e6);
    }

    // A sample for the screen check, first and last of each occupied group.
    println!("\nCheck these on the radio:");
    for (g, n) in per_group.iter().enumerate() {
        if *n == 0 { continue }
        for slot in [g * 100, g * 100 + n - 1] {
            if let Some(rec) = memory::read_record(&after, slot) {
                let name: String = rec.name.iter().take_while(|&&b| b != 0xFF).map(|&b| b as char).collect();
                let m = memory::decode_memory(&rec.memory);
                println!("  memory {slot}: {name:<9} {:9.4} MHz", m.freq_hz as f64 / 1e6);
            }
        }
    }

    assert!(bad.is_empty(), "the radio did not store {} of the blocks we wrote", bad.len());
    assert!(misbanded.is_empty(), "{} memories cannot transmit", misbanded.len());
    assert_eq!(on_radio, intended, "the radio holds a different number of memories than we built");
}

/// Put the radio back the way it was found, through the driver's own
/// `ImageRestorer` — the same path a user reaches for after a bad write.
///
/// This is the one that matters most to the person whose radio it is, and it is
/// deliberately verified by CONTENT rather than by the restore reporting
/// success: a restore that wrote nothing at all would also "succeed".
#[test]
#[ignore = "writes to a real TH-D72"]
fn restore_the_radio_as_found() {
    let dir = out_dir();
    let asfound = std::fs::read(dir.join("ww8l-asfound.img")).expect("the as-found image");

    // Whatever the radio is holding right now, read fresh — not a stale image
    // from an earlier session. On 2026-08-28 a botched MCP-mode abort left this
    // radio FACTORY DEFAULT with 0 memories, and a hard-coded "49" would have
    // printed a comforting number that was simply untrue.
    //
    // ⚠ This used to read a FILE, defaulting to `ww8l-step3-after.img` — an
    // image a different test had written days earlier. The comment above said
    // "read fresh" and the code did the opposite, so on 2026-08-29 the restore
    // announced "radio holds 49 memories and 1077 bytes that differ" about a
    // radio that in fact held 53. The verification afterwards was sound (it
    // re-reads the radio), but the line describing the BEFORE state was a claim
    // about hardware nobody had asked the hardware about — exactly what this
    // project's rule on hardware claims forbids. Now it downloads.
    // ⚠ `reconnect_after_clone`, not `open_port`. This test may well be run
    // straight after another clone session, and the radio re-enumerates on the
    // USB bus afterwards -- a bare open fails instantly with the port simply
    // not there. The helper waits and retries, which is what every other step
    // in this file already does.
    let current = {
        let mut p = protocol::reconnect_after_clone(&port()).expect("connect before");
        let img = protocol::download(&mut *p).expect("download before");
        drop(p);
        std::fs::write(dir.join("ww8l-before-restore.img"), &img).expect("save");
        img
    };
    let before_count = (0..layout::CHANNEL_COUNT)
        .filter(|&s| memory::read_record(&current, s).is_some())
        .count();
    let target_count = (0..layout::CHANNEL_COUNT)
        .filter(|&s| memory::read_record(&asfound, s).is_some())
        .count();
    let to_undo = real_differences(&current, &asfound);
    println!(
        "radio holds {before_count} memories and {} bytes that differ from as-found; \
         restoring to {target_count} memories",
        to_undo.len()
    );
    // The guard used to compare MEMORY COUNTS, which only fires for the case it
    // was written for -- the factory reset, 0 memories against 53. A restore
    // that undoes a settings campaign leaves the count untouched, so that guard
    // refused to run the very restore its owner asked for. Bytes are the
    // general statement of "there is something to undo", and they cover the
    // factory reset too.
    assert!(!to_undo.is_empty(), "the restore must have something to undo");

    // The download above was a clone session, so the radio needs its few
    // seconds and a re-enumeration before it will answer again. Wait for it the
    // same way every other step here does, and drop the port so `restore_image`
    // can open its own.
    drop(protocol::reconnect_after_clone(&port()).expect("reconnect before restore"));
    super::DRIVER.restore_image(&port(), &asfound).expect("restore");
    println!("restored; reconnecting…");

    let mut p = protocol::reconnect_after_clone(&port()).expect("reconnect");
    let after = protocol::download(&mut *p).expect("download after");
    std::fs::write(dir.join("ww8l-restored.img"), &after).expect("save");

    let diffs = real_differences(&asfound, &after);
    let count = (0..layout::CHANNEL_COUNT)
        .filter(|&s| memory::read_record(&after, s).is_some())
        .count();
    println!("{count} memories on the radio; {} bytes differ from the as-found image", diffs.len());
    for &i in diffs.iter().take(12) {
        println!("  0x{i:04X}: {:02x} -> {:02x}", asfound[i], after[i]);
    }

    let m59 = memory::read_record(&after, 59).expect("memory 59");
    let name: String = m59.name.iter().take_while(|&&b| b != 0xFF).map(|&b| b as char).collect();
    println!("memory 59 reads {name:?}");

    assert_eq!(count, target_count, "the radio should hold its original memories again");
    assert_eq!(name, "PO101-LO", "memory 59's original name must be back");
    // Memories and one name are not the whole codeplug. A settings restore is
    // exactly the case where they can all be right and the SETTINGS still wrong,
    // so require the image itself to match -- s123 measured precisely 0 bytes of
    // difference after a restore, so 0 is the achievable standard, not a hope.
    assert!(
        diffs.is_empty(),
        "{} bytes still differ from the as-found image after the restore",
        diffs.len()
    );
}

/// **Ladder step 5b — the settings WRITE, the last unproven path in the driver.**
///
/// Restores the contrast the measurement session left at maximum, through the
/// driver's own `SettingsWriter`. Contrast is the right field to prove this on:
/// it is the most harmless setting on the radio, its range was measured on this
/// radio rather than inherited, and the change is visible from across the desk.
///
/// A working read path has twice hidden a dead write path in this codebase, so
/// this asserts three separate things: that the radio *accepted* the line, that
/// the read-back carries the new value, and — most importantly — that **nothing
/// else moved**. `MU` sets all 19 parameters at once, so a formatting bug here
/// would rewrite eighteen settings the operator never touched.
#[test]
#[ignore = "writes settings to a real TH-D72"]
fn step5b_settings_write_restores_contrast() {
    use crate::radios::driver::{SettingsReader, SettingsWriter};

    let dir = out_dir();
    let schema = crate::seed::THD72_SETTINGS_SCHEMA;

    let before = super::DRIVER.read_settings(&port(), schema).expect("read settings");
    let before_line = String::from_utf8(before.backup.clone()).expect("the MU line");
    println!("before: {before_line}");
    println!("  contrast reads {}", before.settings["contrast"]);
    assert_ne!(
        before.settings["contrast"],
        serde_json::json!(7),
        "the radio should still be at the value the measurement left it at"
    );

    let want = serde_json::json!({ "contrast": 7 });
    let report = super::DRIVER
        .write_settings(&port(), &want, schema, &dir)
        .expect("write settings");
    println!(
        "wrote {} field(s); verified={:?} note={:?}",
        report.fields_written, report.verified, report.note
    );
    println!("backup: {}", report.backup_path);

    let after = super::DRIVER.read_settings(&port(), schema).expect("read back");
    let after_line = String::from_utf8(after.backup.clone()).expect("the MU line");
    println!("after:  {after_line}");

    // Nothing but contrast may have moved. This is the assertion that matters —
    // the other two would both pass on a driver that rewrote every parameter.
    let b: Vec<&str> = before_line.trim_start_matches("MU ").split(',').collect();
    let a: Vec<&str> = after_line.trim_start_matches("MU ").split(',').collect();
    let moved: Vec<usize> = (0..b.len()).filter(|&i| a[i] != b[i]).collect();
    println!("menu parameters that changed: {:?}", moved.iter().map(|i| i + 1).collect::<Vec<_>>());

    assert_eq!(report.fields_written, 1);
    assert_eq!(after.settings["contrast"], serde_json::json!(7), "contrast did not land");
    assert_eq!(moved, vec![1], "a write must touch only p2 — it moved {moved:?}");
    assert_eq!(report.verified, Some(true), "the driver's own read-back must agree");
    println!("\n>>> The display should have dimmed. Contrast is back to 7, as found.");
}

/// The screen check for the s125 address campaign — the half a diff cannot do.
///
/// ⚠ Ten passes of bit signatures measured 116 addresses against the radio's own
/// IMAGE. That proves where a byte moved when RT Systems moved a control; it
/// does NOT prove what the byte MEANS to the radio. The TH-D75 shipped
/// `voice-guidance-volume` writing "Volume Link" when the operator picked
/// "Level 1" for exactly this reason: the tool's numbers agreed with themselves
/// and nobody looked at the radio until afterwards.
///
/// So this writes values at the addresses THIS campaign measured — not through
/// RT Systems — and every one is chosen to differ from what the radio already
/// holds, because a check that cannot fail proves nothing. The expected reading
/// AND the reading that would mean we are wrong are both printed before the
/// operator is asked to look.
///
/// `CPM_THD72_UNDO=1` puts the as-found image back instead.
#[test]
#[ignore = "writes to a real TH-D72"]
fn screen_check_the_measured_addresses() {
    let dir = out_dir();
    let asfound = std::fs::read(dir.join("ww8l-asfound.img")).expect("the as-found image");

    // (address, menu, setting, byte to write, what the radio should show,
    //  what the radio holds now, what a WRONG address/encoding would show)
    let checks: [(usize, &str, &str, u8, &str, &str, &str); 6] = [
        (0x0314, "110", "Battery Saver",      0x02, "0.2 sec",  "1.0 sec", "still 1.0 sec"),
        (0x0315, "111", "Auto Power Off",     0x02, "30 min",   "Off",     "still Off"),
        (0x0316, "112", "Battery Type",       0x01, "Alkaline", "Lithium", "still Lithium"),
        (0x0317, "121", "Key Beep",           0x03, "GPS Only", "Off",     "still Off, or Radio Only"),
        // ★ The one that can distinguish two hypotheses rather than merely
        // confirm one. Every other field here stores its option INDEX, but
        // 0x0325 held 0x05 while the programmer displayed "5 sec" — index 4.
        // So this byte stores SECONDS, not the index, the same +1 that display
        // contrast has. Writing 9: "9 sec" means seconds, "10 sec" means the
        // index reading is right and this row of the table is off by one.
        (0x0325, "151", "Time-Operate Resume", 0x09, "9 sec",   "5 sec",   "10 sec => stored is the INDEX, not seconds"),
        (0x032F, "182", "Mic Key Lock",       0x01, "On",       "Off",     "still Off"),
    ];

    let undo = std::env::var("CPM_THD72_UNDO").is_ok();
    let mut img = asfound.clone();
    if !undo {
        for &(addr, _, _, val, _, _, _) in &checks {
            assert_ne!(
                asfound[addr], val,
                "0x{addr:04X} already holds {val:#04x} — this check could not fail"
            );
            img[addr] = val;
        }
    }

    protocol::reconnect_after_clone(&port()).map(drop).ok();
    super::DRIVER.restore_image(&port(), &img).expect("write");
    println!("written; reconnecting…");
    let mut p = protocol::reconnect_after_clone(&port()).expect("reconnect");
    let after = protocol::download(&mut *p).expect("download after");

    for &(addr, menu, name, val, _, _, _) in &checks {
        let want = if undo { asfound[addr] } else { val };
        assert_eq!(
            after[addr], want,
            "menu {menu} {name}: 0x{addr:04X} reads {:#04x}, wanted {want:#04x}",
            after[addr]
        );
    }
    println!("all {} bytes read back as written", checks.len());

    if undo {
        let diffs = real_differences(&asfound, &after);
        assert!(diffs.is_empty(), "{} bytes still differ from as-found", diffs.len());
        println!("\n>>> The radio is back to the as-found image; 0 bytes differ.");
        return;
    }

    println!("\n>>> ON THE RADIO: press [MENU], then type the number.\n");
    println!("Menu   Setting                SHOULD READ  (was)");
    for &(_, menu, name, _, expect, was, _) in &checks {
        println!("{menu:<6} {name:<22} {expect:<12} ({was})");
    }
    println!("\n>>> WHAT A FAILURE LOOKS LIKE — any of these means the table is wrong:");
    for &(addr, menu, name, _, _, _, fail) in &checks {
        println!("  menu {menu} {name} (0x{addr:04X}): {fail}");
    }
}

/// Screen check, round two — other tabs, and the region the exclusion nearly ate.
///
/// Round one confirmed six Common 1 fields, three of which had been settled by
/// inference. This one goes after what round one could not reach:
///
///   * three different tabs, at address regions far from `0x03xx` — `gps` at
///     `0x0A0x` and `dtmf`/`txrx` at `0x032x`-`0x033x`;
///   * `0x0227`, which is inside `0x0200-0x02FF`. Excluding that window as "the
///     radio's own state" cut this campaign's ambiguities from 12 to 3 and was
///     WRONG — the measured noise floor is six bytes and none of them are
///     there. If 0x0227 reads back on the radio's display, that settles it.
///
/// ⚠ Every value written here is the whole byte, taken as the option index.
/// The campaign only ever proved the bits it happened to exercise, so this also
/// tests the assumption that the rest of the byte belongs to the same field —
/// which is why a wrong reading is worth as much as a right one.
#[test]
#[ignore = "writes to a real TH-D72"]
fn screen_check_round_two() {
    let dir = out_dir();
    let asfound = std::fs::read(dir.join("ww8l-asfound.img")).expect("the as-found image");

    let checks: [(usize, &str, &str, u8, &str, &str, &str); 5] = [
        (0x0A03, "210", "GPS Datum",        0x01, "Tokyo",         "WGS-84",   "still WGS-84"),
        (0x0A01, "201", "GPS Battery Saver",0x04, "8 min",         "Auto",     "still Auto"),
        (0x032A, "171", "DTMF Tx speed",    0x02, "150 ms",        "100 ms",   "still 100 ms, or 50 ms"),
        (0x033A, "13A", "Time-out Timer",   0x03, "2.0 Minutes",   "10.0 Minutes", "still 10.0 Minutes"),
        // No menu number: output power is [F],[MENU], and it shows on the main
        // display as H / L / EL. Only ONE of the two bands is changed, so which
        // band shows EL also tells us whether 0x0227 is the A-band or the
        // B-band copy — the labels in the programmer call both of them "High".
        (0x0227, "—",   "Output power",     0x02, "EL on one band", "H on both", "H on both bands"),
    ];

    let undo = std::env::var("CPM_THD72_UNDO").is_ok();
    let mut img = asfound.clone();
    if !undo {
        for &(addr, _, _, val, _, _, _) in &checks {
            assert_ne!(asfound[addr], val,
                "0x{addr:04X} already holds {val:#04x} — this check could not fail");
            img[addr] = val;
        }
    }

    protocol::reconnect_after_clone(&port()).map(drop).ok();
    super::DRIVER.restore_image(&port(), &img).expect("write");
    println!("written; reconnecting…");
    let mut p = protocol::reconnect_after_clone(&port()).expect("reconnect");
    let after = protocol::download(&mut *p).expect("download after");
    for &(addr, menu, name, val, _, _, _) in &checks {
        let want = if undo { asfound[addr] } else { val };
        assert_eq!(after[addr], want,
            "menu {menu} {name}: 0x{addr:04X} reads {:#04x}, wanted {want:#04x}", after[addr]);
    }
    println!("all {} bytes read back as written", checks.len());

    if undo {
        let diffs = real_differences(&asfound, &after);
        assert!(diffs.is_empty(), "{} bytes still differ from as-found", diffs.len());
        println!("\n>>> The radio is back to the as-found image; 0 bytes differ.");
        return;
    }
    println!("\n>>> ON THE RADIO: press [MENU], then type the number.\n");
    println!("Menu   Setting                SHOULD READ      (was)");
    for &(_, menu, name, _, expect, was, _) in &checks {
        println!("{menu:<6} {name:<22} {expect:<16} ({was})");
    }
    println!("\n>>> WHAT A FAILURE LOOKS LIKE:");
    for &(addr, menu, name, _, _, _, fail) in &checks {
        println!("  menu {menu} {name} (0x{addr:04X}): {fail}");
    }
}

/// Ladder step 5 for the s125 settings transport — the read half.
///
/// ⚠ The 11 screen checks proved the ADDRESSES. They did not prove this: the
/// new `read_settings` issues a clone `download` immediately after the `MU`
/// command IN THE SAME SESSION, and nothing has shown the radio will do that.
/// If it refuses, the fix is two sessions with `reconnect_after_clone` between
/// them — so a failure here is a design answer, not a dead end.
///
/// Read-only. It writes nothing to the radio.
#[test]
#[ignore = "reads a real TH-D72"]
fn settings_read_over_both_transports() {
    use crate::radios::driver::SettingsReader;
    let schema = include_str!("../../thd72_settings_schema.json");
    let cap = super::KenwoodThd72
        .read_settings(&port(), schema)
        .expect("read settings");

    let obj = cap.settings.as_object().expect("an object");
    println!("{} fields read; backup is {} bytes (.{})",
             obj.len(), cap.backup.len(), cap.backup_ext);
    assert_eq!(cap.backup.len(), layout::IMAGE_LEN, "the backup must be the whole image");

    // Both transports have to have produced something, or one of them is dead.
    let mu_field = &obj["apo"];              // MU parameter 4
    let img_field = &obj["common1-1152"];  // image 0x0316, screen-confirmed
    println!("  MU    apo                  = {mu_field}");
    println!("  image common1-battery-type = {img_field}");
    println!("  image gps-datum            = {}", obj["gps-1311"]);
    println!("  image txrx-time_out_timer? = {}", obj.get("txrx-1311")
             .map(|v| v.to_string()).unwrap_or_else(|| "(key not in schema)".into()));
    assert!(!mu_field.is_null(), "the MU half read nothing");
    assert!(!img_field.is_null(), "the image half read nothing");

    // The backup must be a real image, not a buffer of zeroes.
    let nonzero = cap.backup.iter().filter(|&&b| b != 0).count();
    assert!(nonzero > 1000, "the backup looks empty: {nonzero} non-zero bytes");
    println!("\n>>> Both transports answered in one session.");
}

/// Ladder step 5 — the WRITE half, both transports in one call.
///
/// Deliberately changes ONE field of each kind so a failure says which half
/// broke: `apo` is an `MU` parameter, `common1-battery-type` is image-only at
/// `0x0316`. Both are readable on the radio's own menus (111 and 112), and both
/// values differ from what the radio holds, so the check can fail.
///
/// `CPM_THD72_UNDO=1` puts them back.
#[test]
#[ignore = "writes to a real TH-D72"]
fn settings_write_over_both_transports() {
    use crate::radios::driver::{SettingsReader, SettingsWriter};
    let schema = include_str!("../../thd72_settings_schema.json");
    let dir = out_dir();
    let undo = std::env::var("CPM_THD72_UNDO").is_ok();

    let before = super::KenwoodThd72
        .read_settings(&port(), schema)
        .expect("read before");
    let obj = before.settings.as_object().unwrap();
    println!("before: apo={} battery-type={}", obj["apo"], obj["common1-1152"]);

    let want = if undo {
        serde_json::json!({ "apo": "Off", "common1-1152": "Lithium" })
    } else {
        serde_json::json!({ "apo": "30 minutes", "common1-1152": "Alkaline" })
    };
    for (k, v) in want.as_object().unwrap() {
        assert_ne!(&obj[k.as_str()], v, "{k} already holds {v} — this could not fail");
    }

    protocol::reconnect_after_clone(&port()).map(drop).ok();
    let report = super::KenwoodThd72
        .write_settings(&port(), &want, schema, &dir)
        .expect("write settings");
    println!("wrote {} field(s); verified={:?}; backup {}",
             report.fields_written, report.verified, report.backup_path);
    if let Some(n) = &report.note {
        println!("note: {n}");
    }

    protocol::reconnect_after_clone(&port()).map(drop).ok();
    let after = super::KenwoodThd72
        .read_settings(&port(), schema)
        .expect("read back");
    let got = after.settings.as_object().unwrap();
    for (k, v) in want.as_object().unwrap() {
        assert_eq!(&got[k.as_str()], v, "{k} did not land");
    }
    println!("read back: apo={} battery-type={}", got["apo"], got["common1-1152"]);

    if undo {
        println!("\n>>> Both fields are back as found.");
    } else {
        println!("\n>>> ON THE RADIO:");
        println!("  Menu 111 Auto Power Off should read  30 min    (was Off)      [MU half]");
        println!("  Menu 112 Battery Type   should read  Alkaline  (was Lithium)  [image half]");
        println!(">>> A FAILURE looks like: 111 still Off, or 112 still Lithium.");
    }
}

/// Ladder step 4 — the band probe, against what the app said it wrote.
///
/// ⚠ The step this project has been burned by. An out-of-coverage frequency
/// does NOT error: it becomes a silently EMPTY memory slot while the app
/// reports success, and three repeaters were lost that way with a clean report.
/// So this counts what actually LANDED and buckets it against the model's own
/// declared bands, rather than trusting the write report.
///
/// Reads only. `CPM_THD72_EXPECT=49` asserts the count the app claimed.
#[test]
#[ignore = "reads a real TH-D72"]
fn step4_band_probe() {
    let mut p = protocol::reconnect_after_clone(&port()).expect("connect");
    let image = protocol::download(&mut *p).expect("download");

    // The model's own declared coverage — kept here as literals on purpose, so
    // this test fails if the seed is edited without re-running the probe.
    const TX: [(f64, f64); 2] = [(144.0, 148.0), (430.0, 450.0)];
    const RX: [(f64, f64); 2] = [(118.0, 174.0), (320.0, 524.0)];
    let inside = |mhz: f64, bands: &[(f64, f64)]| bands.iter().any(|&(a, b)| mhz >= a && mhz <= b);

    let mut total = 0usize;
    let (mut tx_ok, mut rx_only, mut outside) = (0usize, 0usize, Vec::new());
    let mut highest = 0usize;
    for slot in 0..layout::CHANNEL_COUNT {
        let Some(rec) = memory::read_record(&image, slot) else { continue };
        total += 1;
        highest = slot;
        let mhz = memory::decode_memory(&rec.memory).freq_hz as f64 / 1e6;
        let name: String = rec.name.iter().take_while(|&&b| b != 0xFF).map(|&b| b as char).collect();
        if inside(mhz, &TX) {
            tx_ok += 1;
        } else if inside(mhz, &RX) {
            rx_only += 1;
        } else {
            outside.push(format!("slot {slot} {name:?} {mhz:.4} MHz"));
        }
    }

    println!("{total} memories on the radio, highest slot {highest}");
    println!("  {tx_ok} inside a TX band, {rx_only} receive-only, {} outside all bands",
             outside.len());

    // ★ The failure mode is a HOLE, not a bad value: a dropped channel leaves
    // the slots below it populated and one empty in the middle, or the run
    // short. Count the gaps rather than eyeballing the list.
    let holes: Vec<usize> = (0..=highest)
        .filter(|&s| memory::read_record(&image, s).is_none())
        .collect();
    if holes.is_empty() {
        println!("  no empty slots below the highest — nothing was silently dropped");
    } else {
        println!("  ⚠ {} EMPTY slot(s) below the highest: {:?}", holes.len(),
                 &holes[..holes.len().min(12)]);
    }

    for o in &outside {
        println!("  ⚠ outside every declared band: {o}");
    }
    assert!(outside.is_empty(), "{} memories sit outside the declared bands", outside.len());
    assert!(holes.is_empty(), "{} slots were silently dropped", holes.len());

    if let Ok(want) = std::env::var("CPM_THD72_EXPECT") {
        let want: usize = want.parse().expect("CPM_THD72_EXPECT must be a number");
        assert_eq!(total, want, "the app said it wrote {want} channels");
        println!("  ✓ matches the {want} the app reported");
    }
}
