//! Phase 2's gate, at image scale: every record of every real ID-5100 clone
//! read decodes and re-encodes **byte-identically**, and every used memory's
//! flags and bank entry come back the same when stored through [`store`].
//!
//! `#[ignore]`d and env-gated because the reads live in gitignored
//! `scratchpad/` — they are Tim's radio, so they are not committed and CI
//! cannot see them. `memory.rs` carries six real records as constants, which
//! CI does run.
//!
//! ```sh
//! CPM_ID5100_IMAGES=../scratchpad/id5100 \
//!   cargo test --lib icom_id5100::real_images -- --ignored --nocapture
//! ```

use super::layout::{record_offset, BANK_TABLE, CHANNEL_COUNT, IMAGE_LEN, RECORD_COUNT, REC_LEN};
use super::memory::{is_used, read_bank, read_record, read_skip, store, Record, ScanSkip};

fn images() -> Vec<(String, Vec<u8>)> {
    let dir = std::env::var("CPM_ID5100_IMAGES").expect("CPM_ID5100_IMAGES");
    let mut out = Vec::new();
    for entry in std::fs::read_dir(&dir).expect("read image dir") {
        let path = entry.expect("dir entry").path();
        if path.extension().and_then(|e| e.to_str()) != Some("img") {
            continue;
        }
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        let mut bytes = std::fs::read(&path).expect("read image");
        // CHIRP appends a metadata trailer; our own reader does not.
        bytes.truncate(IMAGE_LEN);
        out.push((name, bytes));
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    assert!(!out.is_empty(), "no .img files in {dir}");
    out
}

#[test]
#[ignore = "needs the real reads in scratchpad/id5100"]
fn every_record_of_every_real_read_re_encodes_byte_identically() {
    for (name, image) in images() {
        assert_eq!(image.len(), IMAGE_LEN, "{name}");
        for slot in 0..RECORD_COUNT {
            let at = record_offset(slot);
            let raw = &image[at..at + REC_LEN];
            assert_eq!(Record::decode(raw).encode(), raw, "{name} record {slot}");
        }
        println!("{name}: {RECORD_COUNT} records byte-identical");
    }
}

/// Storing each used memory back through `store` — record, empty bit, skip
/// bits cleared, bank byte with its multiplier bits — reproduces the radio's
/// own bytes for every memory that is not skipped. (`store` is for NEW
/// channels, which carry no skip; a skipped memory differs in exactly its
/// skip bit, and that is checked too.)
#[test]
#[ignore = "needs the real reads in scratchpad/id5100"]
fn storing_the_radios_own_memories_reproduces_its_tables() {
    for (name, image) in images() {
        let mut rebuilt = image.clone();
        let mut n = 0;
        for slot in (0..CHANNEL_COUNT).filter(|&s| is_used(&image, s)) {
            let rec = read_record(&image, slot);
            let bank = read_bank(&image, slot);
            store(&mut rebuilt, slot, &rec, bank);
            let entry = BANK_TABLE + slot * 2;
            if image[entry] & 0x80 == 0 {
                assert_eq!(
                    rebuilt[entry..entry + 2],
                    image[entry..entry + 2],
                    "{name} memory {slot}: bank entry"
                );
            } else {
                // A factory-reset entry (`9F FF` and kin): `store` writes the
                // configured radio's form — bit 7 clear, index 00 — with the
                // same bank and multiplier bits.
                assert_eq!(rebuilt[entry], image[entry] & 0x7F, "{name} memory {slot}");
                assert_eq!(rebuilt[entry + 1], 0, "{name} memory {slot}");
            }
            n += 1;
            if read_skip(&image, slot) != ScanSkip::None {
                assert_eq!(read_skip(&rebuilt, slot), ScanSkip::None);
            }
        }
        let skipped: Vec<usize> = (0..CHANNEL_COUNT)
            .filter(|&s| is_used(&image, s) && read_skip(&image, s) != ScanSkip::None)
            .collect();
        let reset_entries = (0..CHANNEL_COUNT)
            .filter(|&s| is_used(&image, s) && image[BANK_TABLE + 2 * s] & 0x80 != 0)
            .count();
        let differing: usize = (0..IMAGE_LEN).filter(|&i| image[i] != rebuilt[i]).count();
        // Each skipped memory flips one bit in one byte; each factory-reset
        // bank entry differs in both of its bytes.
        assert!(
            differing <= skipped.len() + 2 * reset_entries,
            "{name}: {differing} bytes differ, {} skipped, {reset_entries} reset entries",
            skipped.len()
        );
        println!("{name}: {n} memories re-stored, {differing} bytes differ ({} skipped)", skipped.len());
    }
}

/// The restore gate against every real file on hand: each read of the radio,
/// and each image the ladder wrote and the radio ACCEPTED, passes; the band
/// probe image the radio REFUSED (and factory reset over) does not.
#[test]
#[ignore = "needs the real reads in scratchpad/id5100"]
fn the_restore_gate_passes_every_accepted_image_and_refuses_the_one_that_reset_the_radio() {
    use crate::radios::driver::ImageRestorer;
    let mut refused = 0;
    for (name, image) in images() {
        let verdict = super::DRIVER.check_restore_image(&image);
        if name.contains("bandprobe_written") {
            assert!(verdict.is_err(), "{name} reset the radio and was let through");
            println!("{name}: refused — {}", verdict.unwrap_err());
            refused += 1;
        } else {
            assert!(verdict.is_ok(), "{name}: {}", verdict.unwrap_err());
        }
    }
    assert_eq!(refused, 1, "the probe image is not in the folder");
}

/// Print every setting decoded from one real read (`CPM_ID5100_SETTINGS_IMG`)
/// — the form Tim reads his radio's menus against.
#[test]
#[ignore = "needs a real read in CPM_ID5100_SETTINGS_IMG"]
fn dump_decoded_settings() {
    let path = std::env::var("CPM_ID5100_SETTINGS_IMG").expect("CPM_ID5100_SETTINGS_IMG");
    let image = std::fs::read(path).unwrap();
    let v = super::settings::decode(&image[..IMAGE_LEN]);
    for (k, val) in v.as_object().unwrap() {
        println!("{k} = {val}");
    }
    if let Ok(out) = std::env::var("CPM_ID5100_SETTINGS_OUT") {
        std::fs::write(out, serde_json::to_string(&v).unwrap()).unwrap();
    }
}

/// Programming a radio with the settings just read off it must change nothing:
/// decode every one of the 291 fields from a real image, apply them back, and
/// compare byte for byte. A field whose decode -> encode is not neutral would
/// quietly rewrite the radio on every program — the factory 0°W longitude did,
/// as 0°E, until this was checked.
#[test]
#[ignore = "needs the real reads in scratchpad/id5100"]
fn applying_the_settings_read_off_a_radio_changes_nothing() {
    for (name, image) in images() {
        let decoded = super::settings::decode(&image);
        let mut again = image.clone();
        let (n, notes) = super::settings::apply(&mut again, &decoded);
        let changed: Vec<String> = (0..IMAGE_LEN)
            .filter(|&i| image[i] != again[i])
            .map(|i| format!("{i:#x}:{:02x}->{:02x}", image[i], again[i]))
            .collect();
        assert!(changed.is_empty(), "{name}: applying its own settings changed {changed:?} (notes {notes:?})");
        println!("{name}: {n} settings re-applied, 0 bytes changed, {} notes", notes.len());
    }
}
