//! The phase-2 gate: re-encode a radio's own memories out of its own image and
//! get the same bytes back.
//!
//! The images are real clone reads and are not committed — they are the
//! operator's codeplug. They live in the gitignored scratchpad:
//!
//! ```sh
//! CPM_FT857_IMAGES=../scratchpad/yaesu_ft857d/captures \
//!   cargo test --lib yaesu_ft857d::real_images -- --ignored --nocapture
//! ```
//!
//! ⚠ As of 2026-10-17 every image there came off an **FT-897**. A pass is
//! evidence for the shared layout on that radio, not for the FT-857D.

use std::path::PathBuf;

use super::layout::{
    memory_offset, CHANNEL_COUNT, ENTRY_LEN, IMAGE_LEN_EXPORT, IMAGE_LEN_US, MEMORY_BASE,
    PMS_COUNT,
};
use super::memory::{
    decode_channel, decode_memory, encode_channel, encode_memory, existing_memory, is_visible,
    store_memory, tag_text,
};

fn images() -> Vec<(PathBuf, Vec<u8>)> {
    let dir = std::env::var("CPM_FT857_IMAGES").expect("set CPM_FT857_IMAGES to a folder of .img");
    let mut out: Vec<_> = std::fs::read_dir(&dir)
        .expect("read the image folder")
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "img"))
        .map(|p| {
            let b = std::fs::read(&p).unwrap();
            (p, b)
        })
        .collect();
    out.sort();
    assert!(!out.is_empty(), "no .img files in {dir}");
    out
}

#[test]
#[ignore]
fn every_record_of_every_real_image_re_encodes_byte_identically() {
    for (path, img) in images() {
        assert!(
            img.len() == IMAGE_LEN_US || img.len() == IMAGE_LEN_EXPORT,
            "{}: {} bytes",
            path.display(),
            img.len()
        );
        // Memories and PMS alike, empty `FF` slots included.
        for slot in 0..CHANNEL_COUNT + PMS_COUNT {
            let at = MEMORY_BASE + slot * ENTRY_LEN;
            let raw: [u8; ENTRY_LEN] = img[at..at + ENTRY_LEN].try_into().unwrap();
            assert_eq!(encode_memory(&decode_memory(&raw)), raw, "{} slot {slot}", path.display());
        }
        println!("{}: lossless", path.display());
    }
}

#[test]
#[ignore]
fn writing_a_radios_own_channels_back_changes_nothing() {
    for (path, img) in images() {
        let mut out = img.clone();
        let mut n = 0;
        for slot in 0..CHANNEL_COUNT {
            if !is_visible(&img, slot) {
                continue;
            }
            let base = existing_memory(&img, slot).expect("a visible memory is filled");
            let channel = decode_channel(&base);
            let m = encode_channel(&channel, &tag_text(&base), Some(&base))
                .unwrap_or_else(|e| panic!("{} memory {}: {e}", path.display(), slot + 1));
            store_memory(&mut out, slot, &m);
            n += 1;
        }
        if out != img {
            let first = out.iter().zip(&img).position(|(a, b)| a != b).unwrap();
            let slot = first.saturating_sub(MEMORY_BASE) / ENTRY_LEN;
            panic!(
                "{}: first difference at 0x{first:04X} (memory {} at 0x{:04X})",
                path.display(),
                slot + 1,
                memory_offset(slot)
            );
        }
        println!("{}: {n} memories re-encoded through the channel model, identical", path.display());
    }
}

#[test]
#[ignore]
fn every_real_images_settings_round_trip_byte_identically() {
    use super::settings::{apply_profile_settings, decode_settings};
    for (path, img) in images() {
        let decoded = decode_settings(&img);
        let mut out = img.clone();
        let (n, notes) = apply_profile_settings(&mut out, &decoded)
            .unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        assert!(notes.is_empty(), "{}: {notes:?}", path.display());
        assert_eq!(out, img, "{}", path.display());
        println!("{}: {n} settings, beep-vol {}, identical", path.display(), decoded["beep-vol"]);
    }
}
