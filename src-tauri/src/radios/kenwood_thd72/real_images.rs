//! Phase 2's gate, at image scale: read a real TH-D72's own memories out of its
//! own clone image and put them back **byte-identically**.
//!
//! The unit tests in `memory.rs` prove this one record at a time, against real
//! bytes pasted in as constants. This harness proves it across whole images —
//! every memory of every image on hand, through the real container, exercising
//! the two modules together rather than separately. On the eight images
//! recovered from CHIRP's bug tracker that is roughly 200 memories, including
//! one file whose 187 channels all carry another tool's damage.
//!
//! It is `#[ignore]`d and env-gated because the images live in gitignored
//! `scratchpad/` — they are other people's codeplugs, so they are not committed
//! and CI cannot see them. Same shape as `kenwood_thd75::dev_export`.
//!
//! ```sh
//! CPM_THD72_IMAGES=scratchpad/kenwood_thd72/images \
//!   cargo test --lib kenwood_thd72::real_images -- --ignored --nocapture
//! ```
//!
//! ⚠ These are **other people's radios**, of unknown variant and firmware. They
//! prove the layout; they cannot stand in for Phase 1 on the radio this app will
//! actually program. What they cannot settle is listed at the end of
//! `scratchpad/kenwood_thd72/FINDINGS.md`.

use super::container::Thd72Image;
use super::layout::{prog_vfo_index, CHANNEL_COUNT};
use super::memory::{apply_record, decode_memory, encode_memory, read_record};

fn images() -> Vec<(String, Vec<u8>)> {
    let dir = std::env::var("CPM_THD72_IMAGES").expect("CPM_THD72_IMAGES");
    let mut out = Vec::new();
    for entry in std::fs::read_dir(&dir).expect("read image dir") {
        let path = entry.expect("dir entry").path();
        if path.extension().and_then(|e| e.to_str()) != Some("img") {
            continue;
        }
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        out.push((name, std::fs::read(&path).expect("read image")));
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    assert!(!out.is_empty(), "no .img files in {dir}");
    out
}

/// The gate. Every memory of every image decodes into its fields and re-encodes
/// to the same 16 bytes it came from.
///
/// This is deliberately *lossless*, not *correct*: a record carrying another
/// tool's damage must come back damaged, because a driver that quietly repairs
/// bytes it was only asked to preserve cannot be trusted with the ones it was
/// asked to keep. `2455-fcv.img` is the case that matters — all 187 of its
/// memories have an invalid step nibble, and all 187 must survive the round trip
/// still invalid.
#[test]
#[ignore = "needs the real images in scratchpad/kenwood_thd72/images"]
fn every_memory_of_every_real_image_re_encodes_byte_identically() {
    let mut total = 0usize;
    for (name, bytes) in images() {
        let image = Thd72Image::parse(&bytes)
            .unwrap_or_else(|e| panic!("{name} is a real TH-D72 image and the guard refused it: {e}"));
        let mut count = 0usize;
        for slot in 0..CHANNEL_COUNT {
            let Some(record) = read_record(image.body(), slot) else {
                continue;
            };
            let round_tripped = encode_memory(&decode_memory(&record.memory));
            assert_eq!(
                round_tripped, record.memory,
                "{name} memory {slot}: re-encode differs from the radio's own bytes"
            );
            count += 1;
        }
        println!("  {name}: {count} memories re-encoded byte-identically");
        total += count;
    }
    println!("total: {total} memories");
    assert!(total > 0, "no memories found — the images parsed but decoded empty");
}

/// The same gate through the container's own write path: reading every memory
/// and applying it straight back must leave the image untouched *and* leave
/// every block clean.
///
/// The clean-block half is the one that matters. Partial uploads are this
/// driver's whole safety argument — it can put a codeplug on the radio without
/// touching the APRS, GPS, TNC or calibration regions — and that argument only
/// holds if the dirty set is exactly what changed. A no-op write that dirtied
/// blocks would send bytes to a radio for no reason.
#[test]
#[ignore = "needs the real images in scratchpad/kenwood_thd72/images"]
fn writing_a_radios_own_memories_back_changes_nothing() {
    for (name, bytes) in images() {
        let mut image = Thd72Image::parse(&bytes).expect("parse");
        for slot in 0..CHANNEL_COUNT {
            let Some(record) = read_record(image.body(), slot) else {
                continue;
            };
            for (off, cell) in apply_record(slot, &record) {
                image.patch(off, &cell).expect("patch a memory back where it came from");
            }
        }
        assert!(
            image.dirty_blocks().is_empty(),
            "{name}: writing memories back marked blocks {:?} dirty",
            image.dirty_blocks()
        );
        assert_eq!(image.into_raw(), bytes, "{name}: image changed after a no-op rewrite");
        println!("  {name}: unchanged, no blocks dirtied");
    }
}

/// Every real image must pass the model guard. A guard that refuses a legitimate
/// file is worse than no guard: it turns a working radio into a bug report, and
/// this project has shipped one that did.
#[test]
#[ignore = "needs the real images in scratchpad/kenwood_thd72/images"]
fn the_guard_accepts_every_legitimate_file() {
    for (name, bytes) in images() {
        Thd72Image::parse(&bytes).unwrap_or_else(|e| panic!("{name} refused: {e}"));
    }
}

/// Survey the flag nibbles across the real images and print what the radios
/// actually claim, alongside what the frequency says they should.
///
/// Not an assertion — `2455-fcv.img` is *expected* to be wrong on all 187, which
/// is why it exists. This prints the evidence so a human can see the trap rather
/// than take it on trust, and it is how a future regression would be spotted.
#[test]
#[ignore = "needs the real images in scratchpad/kenwood_thd72/images"]
fn survey_the_band_index_against_the_frequency() {
    for (name, bytes) in images() {
        let image = Thd72Image::parse(&bytes).expect("parse");
        let table = image.prog_vfo_table().expect("prog vfo table");
        let (mut agree, mut disagree, mut uncovered) = (0usize, 0usize, 0usize);
        for slot in 0..CHANNEL_COUNT {
            let Some(record) = read_record(image.body(), slot) else {
                continue;
            };
            let freq = decode_memory(&record.memory).freq_hz;
            match prog_vfo_index(&table, freq) {
                Some(expected) if expected == record.prog_vfo() => agree += 1,
                Some(_) => disagree += 1,
                None => uncovered += 1,
            }
        }
        println!(
            "  {name}: {agree} agree, {disagree} MIS-BANDED (cannot transmit), \
             {uncovered} outside every band"
        );
    }
}
