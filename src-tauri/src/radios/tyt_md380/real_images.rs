//! Phase 3's gate: every record of every real MD-380 read decodes and
//! re-encodes **byte-identically**, and every free slot is in the form the
//! encoder writes for one.
//!
//! `#[ignore]`d and env-gated because the reads live in gitignored
//! `scratchpad/` — they are Tim's radio. `memory.rs` carries a real record as a
//! constant, which CI does run.
//!
//! ```sh
//! CPM_MD380_IMAGES=../scratchpad/tyt_md380/captures \
//!   cargo test --lib tyt_md380::real_images -- --ignored --nocapture
//! ```

use super::layout::*;
use super::memory::{Channel, Contact, RxGroup, ScanList, Zone};

/// The free-record spellings seen in real images: zeros (Tim's radio, every
/// zone and RX group table), the CPS/factory spelling where one exists, and
/// erased flash.
fn is_free_form(raw: &[u8], cps: Option<&[u8]>) -> bool {
    raw.iter().all(|&b| b == 0) || raw.iter().all(|&b| b == 0xFF) || cps == Some(raw)
}

fn images() -> Vec<(String, Vec<u8>)> {
    let dir = std::env::var("CPM_MD380_IMAGES").expect("CPM_MD380_IMAGES");
    let mut out = Vec::new();
    for entry in std::fs::read_dir(&dir).expect("read image dir") {
        let path = entry.expect("dir entry").path();
        let ext = path.extension().and_then(|e| e.to_str());
        let bytes = std::fs::read(&path).expect("read image");
        let image = match ext {
            Some("img") => bytes,
            Some("rdt") => bytes[0x225..0x225 + IMAGE_LEN].to_vec(),
            _ => continue,
        };
        out.push((path.file_name().unwrap().to_string_lossy().into_owned(), image));
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    assert!(!out.is_empty(), "no .img/.rdt files in {dir}");
    out
}

#[test]
#[ignore = "needs the real reads in scratchpad/tyt_md380"]
fn every_record_of_every_real_read_re_encodes_byte_identically() {
    for (name, image) in images() {
        assert_eq!(image.len(), IMAGE_LEN, "{name}");
        let mut used = [0usize; 5];
        for i in 0..CHANNEL_COUNT {
            let raw = &image[at(CHANNELS, CHANNEL_LEN, i)..][..CHANNEL_LEN];
            if !Channel::is_used(raw) {
                assert_eq!(raw, Channel::UNUSED.as_slice(), "{name} free channel {i}");
                continue;
            }
            used[0] += 1;
            let c = Channel::decode(raw).unwrap_or_else(|| panic!("{name} channel {i} decodes"));
            assert_eq!(c.encode().as_slice(), raw, "{name} channel {i}");
        }
        for i in 0..ZONE_COUNT {
            let raw = &image[at(ZONES, ZONE_LEN, i)..][..ZONE_LEN];
            if !Zone::is_used(raw) {
                assert!(is_free_form(raw, None), "{name} free zone {i}");
                continue;
            }
            used[1] += 1;
            assert_eq!(Zone::decode(raw).encode().as_slice(), raw, "{name} zone {i}");
        }
        for i in 0..CONTACT_COUNT {
            let raw = &image[at(CONTACTS, CONTACT_LEN, i)..][..CONTACT_LEN];
            if !Contact::is_used(raw) {
                assert_eq!(raw, Contact::UNUSED.as_slice(), "{name} free contact {i}");
                continue;
            }
            used[2] += 1;
            let c = Contact::decode(raw).unwrap();
            assert_eq!(c.encode().as_slice(), raw, "{name} contact {i}");
        }
        for i in 0..RX_GROUP_COUNT {
            let raw = &image[at(RX_GROUPS, RX_GROUP_LEN, i)..][..RX_GROUP_LEN];
            if !RxGroup::is_used(raw) {
                assert!(is_free_form(raw, None), "{name} free RX group {i}");
                continue;
            }
            used[3] += 1;
            assert_eq!(RxGroup::decode(raw).encode().as_slice(), raw, "{name} RX group {i}");
        }
        for i in 0..SCAN_LIST_COUNT {
            let raw = &image[at(SCAN_LISTS, SCAN_LIST_LEN, i)..][..SCAN_LIST_LEN];
            if !ScanList::is_used(raw) {
                assert!(
                    is_free_form(raw, Some(ScanList::UNUSED.as_slice()))
                        || raw.iter().all(|&b| b == 0 || b == 0xFF),
                    "{name} free scan list {i}"
                );
                continue;
            }
            used[4] += 1;
            assert_eq!(ScanList::decode(raw).encode().as_slice(), raw, "{name} scan list {i}");
        }
        println!(
            "{name}: byte-identical — {} channels, {} zones, {} contacts, {} RX groups, {} scan lists",
            used[0], used[1], used[2], used[3], used[4]
        );
    }
}

/// Settings decoded out of every real image and applied straight back change
/// nothing: the decoder and encoder agree on every field of every real radio
/// state we have, and a stored value the table cannot name is left alone.
#[test]
#[ignore = "needs the real reads in scratchpad/tyt_md380"]
fn every_real_images_settings_apply_back_unchanged() {
    use super::settings::{apply, decode};
    for (name, image) in images() {
        let s = decode(&image);
        let mut copy = image.clone();
        let (n, notes) = apply(&mut copy, s.as_object().unwrap());
        assert!(notes.is_empty(), "{name}: {notes:?}");
        let diff: Vec<usize> = (0..IMAGE_LEN).filter(|&i| copy[i] != image[i]).collect();
        assert!(diff.is_empty(), "{name}: {diff:x?}");
        let blank: Vec<&String> = s.as_object().unwrap().iter().filter(|(_, v)| v == &&serde_json::json!("")).map(|(k, _)| k).collect();
        println!("{name}: {n} fields round-trip; blank (unnameable) {blank:?}");
    }
}

#[test]
#[ignore = "prints one image's decoded settings"]
fn print_settings() {
    let p = std::env::var("CPM_MD380_ONE").expect("CPM_MD380_ONE");
    let img = std::fs::read(p).unwrap();
    for (k, v) in super::settings::decode(&img).as_object().unwrap() {
        println!("{k:45} {v}");
    }
}
