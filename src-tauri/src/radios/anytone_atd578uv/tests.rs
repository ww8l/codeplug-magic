//! The D578UV's encoders, held against the radio's own records.
//!
//! `testdata/d578_codeplug_records.bin` is the record regions of a read-only dump
//! of a real D578UV (fw 1.21): the channel and zone/scan present bitmaps, every
//! present channel record, every zone list and name, and every scan list. Its
//! settings block, boot text and radio IDs were left out on purpose.

use std::collections::BTreeMap;

use super::*;
use crate::radios::anytone_atd890uv::{BankPlan, WRITE_WINDOW};
use crate::radios::anytone_atd890uv::parse_dump;

const FIXTURE: &[u8] = include_bytes!("testdata/d578_codeplug_records.bin");

fn fixture() -> BTreeMap<u32, Vec<u8>> {
    parse_dump(FIXTURE).unwrap().into_iter().collect()
}

fn present(fx: &BTreeMap<u32, Vec<u8>>, base: u32, off: usize, bytes: usize, n: usize) -> Vec<usize> {
    let window = &fx[&base];
    bitmap_indices(&window[off..off + bytes], n)
}

fn channel_slots(fx: &BTreeMap<u32, Vec<u8>>) -> Vec<usize> {
    present(fx, CHANNEL_BITMAP_BASE, 0, CHANNEL_BITMAP_BYTES, MAX_CHANNELS)
}

/// ★ The gate: every channel the radio wrote decodes and re-encodes onto its own
/// bytes with nothing changed. A field read from one offset and written to
/// another, a dropped bit, or a lossy conversion fails this on a real record.
#[test]
fn every_radio_channel_reencodes_byte_identically() {
    let fx = fixture();
    let slots = channel_slots(&fx);
    assert_eq!(slots.len(), 392, "fixture should carry the anchor radio's 392 channels");
    for slot in slots {
        let original = &fx[&channel_addr(slot)];
        let edit = decode_channel(original).unwrap_or_else(|| panic!("slot {slot} blank"));
        let mut rec = original.clone();
        encode_channel_into(&mut rec, &edit).unwrap();
        assert_eq!(&rec, original, "slot {} ({}) re-encoded differently", slot + 1, edit.name);
    }
}

/// The stronger half: encode each decoded channel onto the BLANK template and
/// check every byte the encoder owns lands where the radio put it. The bytes it
/// does not own come from the template, so they are compared only where the
/// template and the radio agree by construction.
#[test]
fn every_radio_channel_encodes_onto_the_template_at_the_radios_offsets() {
    let fx = fixture();
    let owned: Vec<usize> = (0x00..0x10)
        .chain(0x14..0x18)
        .chain([CH_SCAN_LIST, CH_COLOR_CODE])
        .chain(CH_NAME..CH_NAME + NAME_CHARS)
        .collect();
    for slot in channel_slots(&fx) {
        let original = &fx[&channel_addr(slot)];
        let edit = decode_channel(original).unwrap();
        let mut rec = channel_record_template().to_vec();
        encode_channel_into(&mut rec, &edit).unwrap();
        for &o in &owned {
            // A side with no tone leaves that side's code bytes at the template,
            // which is what an unset field holds; the radio's stale value there
            // is unreachable through the enable bits.
            let tone_byte = (CH_TX_CTCSS..CH_RX_DCS + 2).contains(&o);
            if tone_byte && rec[o] != original[o] {
                continue;
            }
            assert_eq!(rec[o], original[o], "slot {} byte 0x{o:02X}", slot + 1);
        }
        assert_eq!(rec[CH_TIME_SLOT] & 1, original[CH_TIME_SLOT] & 1, "slot {} TS", slot + 1);
        assert_eq!(decode_channel(&rec), Some(edit), "slot {} round trip", slot + 1);
    }
}

/// The template is what the radio wrote on most channels, byte for byte, outside
/// the fields the encoder sets.
#[test]
fn the_template_matches_the_radios_majority_record() {
    let fx = fixture();
    let t = channel_record_template();
    let recs: Vec<&Vec<u8>> = channel_slots(&fx).iter().map(|&s| &fx[&channel_addr(s)]).collect();
    for o in [0x10, 0x11, 0x12, 0x13, 0x15, 0x16, 0x17, 0x18, 0x1A, 0x1C, 0x1D, 0x1E, 0x1F, 0x22]
        .into_iter()
        .chain(0x33..0x40)
    {
        let agree = recs.iter().filter(|r| r[o] == t[o]).count();
        assert!(agree * 2 > recs.len(), "template byte 0x{o:02X} disagrees with the radio");
    }
}

#[test]
fn every_radio_zone_reencodes_byte_identically() {
    let fx = fixture();
    let zones = present(&fx, ZONE_BITMAP_BASE, 0, ZONE_BITMAP_BYTES, MAX_ZONES);
    assert_eq!(zones.len(), 21);
    for z in zones {
        let list = &fx[&(ZONE_LIST_BASE + z as u32 * ZONE_LIST_STEP)];
        let members: Vec<u16> = list
            .chunks(2)
            .map(|p| u16::from_le_bytes([p[0], p[1]]))
            .take_while(|&m| m != 0xFFFF)
            .collect();
        assert_eq!(&encode_zone_list(&members).unwrap(), list, "zone {} list", z + 1);
        // The radio leaves stale bytes after a shortened name's NUL ("2 - NOCO
        // MIXED\0S"); the encoder zero-fills the field, so the name is held to
        // the bytes up to and including its terminator.
        let name = &fx[&(ZONE_NAME_BASE + z as u32 * ZONE_NAME_STEP)];
        let enc = encode_zone_name(&decode_ascii_field(&name[..16]));
        let end = name.iter().position(|&b| b == 0).unwrap_or(16);
        assert_eq!(&enc[..end], &name[..end], "zone {} name", z + 1);
        assert!(enc[end..].iter().all(|&b| b == 0), "zone {} name padding", z + 1);
    }
}

/// Scan lists: the owned fields re-encode onto the radio's own record unchanged.
/// (The radio leaves stale bytes after a shortened name's NUL; the encoder
/// zero-fills the field, so names are compared through the decoder.)
#[test]
fn every_radio_scan_list_reencodes_its_owned_fields() {
    let fx = fixture();
    let lists = present(&fx, ZONE_BITMAP_BASE, 0x40, SCAN_BITMAP_BYTES, MAX_SCAN_LISTS);
    assert_eq!(lists.len(), 21);
    for i in lists {
        let original = &fx[&scan_list_addr(i)];
        let fields = decode_scan_list(original);
        let mut rec = original.clone();
        encode_scan_list_into(&mut rec, &fields).unwrap();
        assert_eq!(&rec[..SCAN_NAME], &original[..SCAN_NAME], "list {} header", i + 1);
        assert_eq!(&rec[SCAN_MEMBERS..], &original[SCAN_MEMBERS..], "list {} members+tail", i + 1);
        assert_eq!(decode_scan_list(&rec), fields, "list {} round trip", i + 1);
    }
}

/// The anchor radio's list 17 sits in the SECOND scan bank — the 16-per-bank
/// layout is measured, not assumed.
#[test]
fn scan_list_17_starts_the_second_bank() {
    assert_eq!(scan_list_addr(15), 0x0108_1E00);
    assert_eq!(scan_list_addr(16), 0x010C_0000);
    let fx = fixture();
    assert_eq!(decode_scan_list(&fx[&scan_list_addr(16)]).name, "RMH719 SOUTH");
}

/// Channel addresses: 128 per 0x40000 bank, 0x40 apart.
#[test]
fn channel_addresses_follow_the_bank_layout() {
    assert_eq!(channel_addr(0), 0x0080_0000);
    assert_eq!(channel_addr(127), 0x0080_1FC0);
    assert_eq!(channel_addr(128), 0x0084_0000);
    assert_eq!(channel_addr(3999), 0x0080_0000 + 31 * 0x40000 + 31 * 0x40);
}

#[test]
fn ctcss_table_has_no_150_hz() {
    assert_eq!(CTCSS.len(), 51);
    assert_eq!(ctcss_index(146.2), Some(24));
    assert_eq!(ctcss_index(151.4), Some(25));
    assert_eq!(ctcss_index(150.0), None);
    assert_eq!(ctcss_index(254.1), Some(50));
}

#[test]
fn contact_records_round_trip_and_bank() {
    let rec = encode_contact(1, 3_100, "NA Wide").unwrap();
    assert_eq!(rec.len(), CONTACT_REC_LEN);
    assert_eq!(&rec[CONTACT_DMR_ID..CONTACT_DMR_ID + 4], &[0x00, 0x00, 0x31, 0x00]);
    assert_eq!(decode_contact(&rec), (1, 3_100, "NA Wide".to_string()));
    assert_eq!(contact_addr(999), CONTACT_BASE + 999 * 0x64);
    assert_eq!(contact_addr(1000), CONTACT_BASE + 0x40000);
    assert!(encode_contact(1, 16_777_216, "x").is_err());
}

#[test]
fn bitmaps() {
    assert_eq!(present_bitmap(10, 4), vec![0xFF, 0x03, 0x00, 0x00]);
    let c = contact_bitmap(10);
    assert_eq!(c.len(), CONTACT_BITMAP_BYTES);
    assert_eq!(&c[..3], &[0x00, 0xFC, 0xFF]);
    assert_eq!(bitmap_indices(&[0x05], 8), vec![0, 2]);
}

#[test]
fn names_are_ascii_and_padded() {
    assert_eq!(encode_ascii_field("AB", 4), b"AB\0\0");
    assert_eq!(encode_ascii_field("Zürich 12345678901", 4), b"Zuri");
    assert_eq!(encode_ascii_field("東京", 4), b"??\0\0");
    assert_eq!(decode_ascii_field(b"AB \0\x41"), "AB");
}

/// Every region this driver writes is a named window; nothing in the general
/// settings block or the call-sign database is.
#[test]
fn write_guard_names_only_codeplug_regions() {
    for a in [
        CHANNEL_BASE,
        CHANNEL_BASE + 31 * BANK_STEP,
        ZONE_LIST_BASE,
        ZONE_LIST_BASE + 0x1C000,
        ZONE_NAME_BASE,
        0x024C_0000,
        SCAN_LIST_BASE,
        SCAN_LIST_BASE + 15 * BANK_STEP,
        CONTACT_BASE,
        CONTACT_BASE + 9 * CONTACT_BANK_STEP + 0x14000,
        CONTACT_INDEX_BASE + 0x8000,
        CONTACT_BITMAP_BASE,
        CONTACT_ID_TABLE,
        CONTACT_ID_TABLE + 0x10000,
    ] {
        assert!(writable_window_name(a).is_some(), "0x{a:08X} should be writable");
    }
    assert_eq!(writable_window_name(0x0250_0000), Some("settings"));
    assert_eq!(writable_window_name(0x0258_0000), Some("radio IDs"));
    for a in [
        0x0250_4000, // GPS roaming zones, next to the settings window
        0x0258_4000, // past radio ID 250
        0x0550_0000, // call-sign DB
        CHANNEL_BASE + 0x4000, // inside a channel bank, past the records
        CHANNEL_BASE + 31 * BANK_STEP + 0x4000,
        SCAN_LIST_BASE + 16 * BANK_STEP,
        ZONE_LIST_BASE + 0x20000,
    ] {
        assert!(writable_window_name(a).is_none(), "0x{a:08X} must not be writable");
    }
}

/// Bank 31 holds only channels 3969-4000 (0x800 bytes), and the two VFO records
/// start exactly where channel 4000 ends — inside the same write window. A
/// program that touches bank 31 rewrites that window read-modify-write, so the
/// VFOs pass through untouched; nothing may ever address a slot past 4000.
#[test]
fn the_vfos_share_bank_31s_window() {
    assert_eq!(channel_addr(MAX_CHANNELS - 1) + CH_REC_LEN as u32, 0x00FC_0800);
    assert_eq!(channel_addr(MAX_CHANNELS - 1) & !(WRITE_WINDOW as u32 - 1), 0x00FC_0000);
}

/// The anchor radio's contact regions (a second read-only dump): 38 contacts,
/// the index list, the inverted present bitmap and the contact ID table.
const CONTACTS: &[u8] = include_bytes!("testdata/d578_contacts.bin");

fn contacts_fixture() -> BTreeMap<u32, Vec<u8>> {
    parse_dump(CONTACTS).unwrap().into_iter().collect()
}

/// ★ Every contact the radio wrote re-encodes to its own 100 bytes, and the
/// index list and present bitmap the program writes for 38 contacts are the
/// radio's own.
#[test]
fn every_radio_contact_reencodes_byte_identically() {
    let fx = contacts_fixture();
    let recs = &fx[&CONTACT_BASE];
    let n = 38;
    for i in 0..n {
        let original = &recs[i * CONTACT_REC_LEN..(i + 1) * CONTACT_REC_LEN];
        let (call_type, id, name) = decode_contact(original);
        assert_eq!(encode_contact(call_type, id, &name).unwrap(), original, "contact {i} ({name})");
    }
    let index = &fx[&CONTACT_INDEX_BASE];
    for i in 0..n {
        assert_eq!(&index[i * 4..i * 4 + 4], &(i as u32).to_le_bytes());
    }
    assert_eq!(&index[n * 4..n * 4 + 4], &[0xFF; 4]);
    assert_eq!(&fx[&CONTACT_BITMAP_BASE][..CONTACT_BITMAP_BYTES], &contact_bitmap(n)[..]);
}

/// ★ Rebuilding the contact ID table from the radio's own contacts reproduces
/// the radio's table byte for byte — key encoding, sort order and terminator.
#[test]
fn the_contact_id_table_rebuilds_byte_identically() {
    let fx = contacts_fixture();
    let recs = &fx[&CONTACT_BASE];
    let contacts: Vec<(u8, u32, u32)> = (0..38)
        .map(|i| {
            let (t, id, _) = decode_contact(&recs[i * CONTACT_REC_LEN..(i + 1) * CONTACT_REC_LEN]);
            (t, id, i as u32)
        })
        .collect();
    let table = encode_contact_id_table(&contacts);
    assert_eq!(table.len(), 39 * CONTACT_ID_ENTRY);
    assert_eq!(&fx[&CONTACT_ID_TABLE][..table.len()], &table[..]);
}

/// ★ The half rule, as measured: a write into a 128 KB half carries every other
/// window of that half that holds data, unchanged — the GPS-roaming table that
/// a lone settings-window write erased on the anchor radio is carried — while
/// erased windows and untouched halves are left out.
#[test]
fn a_write_carries_the_rest_of_its_flash_half() {
    let settings = 0x0250_0000;
    let gps_roaming = 0x0250_4000;
    let mut reads: BTreeMap<u32, Vec<u8>> = (0..8)
        .map(|i| (settings + i * WRITE_WINDOW as u32, vec![0xFF; WRITE_WINDOW]))
        .collect();
    reads.get_mut(&settings).unwrap()[0] = 1;
    reads.get_mut(&gps_roaming).unwrap()[..0x400].fill(0);
    let mut patched = reads[&settings].clone();
    patched[0x1D] = 2;
    let plan = BankPlan { base: settings, original: reads[&settings].clone(), patched };
    let plans = with_carried_windows(vec![plan], &reads).unwrap();
    let bases: Vec<u32> = plans.iter().map(|p| p.base).collect();
    assert_eq!(bases, vec![settings, gps_roaming], "the data window is carried, erased ones are not");
    assert_eq!(plans[1].original, plans[1].patched, "a carried window is rewritten unchanged");

    // A window whose only data is the radio's marker is not carried, and the
    // marker frame of a half's last window is never sent.
    let last = settings + 0x1_C000;
    reads.get_mut(&last).unwrap()[0x3FF0..].copy_from_slice(&[
        0xFF, 0xFF, 0xFF, 0xFF, 0x22, 0x33, 0x44, 0x55, 0xFF, 0xFF, 0xFF, 0xFF, 0x55, 0x55, 0xAA, 0xAA,
    ]);
    let plan = BankPlan { base: settings, original: reads[&settings].clone(), patched: plans[0].patched.clone() };
    let again = with_carried_windows(vec![plan], &reads).unwrap();
    assert!(again.iter().all(|p| p.base != last), "a marker-only window is not carried");
    assert_eq!(sendable(last, &reads[&last]).len(), WRITE_WINDOW - MARKER_FRAME);
    assert_eq!(sendable(settings, &reads[&settings]).len(), WRITE_WINDOW);
    assert_eq!(plan_blocks(&again, true).iter().map(|(_, d)| d.len()).max(), Some(WRITE_WINDOW));

    // A half nothing is written to is not touched at all.
    assert!(with_carried_windows(Vec::new(), &reads).unwrap().is_empty());
    // A half that was not fully read is an error, not a silent skip.
    reads.remove(&(settings + 0x1_C000));
    let plan = BankPlan { base: settings, original: vec![0; 1], patched: vec![1; 1] };
    assert!(with_carried_windows(vec![plan], &reads).is_err());
}

/// Halves are 128 KB; the second half of each block (the radio's copy of the
/// previous contents) is a half of its own and nothing here writes it.
#[test]
fn halves_are_128k_and_the_settings_half_holds_gps_roaming() {
    assert_eq!(half_of(0x0250_4000), 0x0250_0000);
    assert_eq!(half_of(0x0252_0000), 0x0252_0000);
    assert_eq!(half_of(0x024C_4000), 0x024C_0000, "AES keys share the bitmaps' half");
}

/// A restore may carry windows this app never owns, but only inside a half it
/// does own.
#[test]
fn restore_guard_accepts_carried_windows_only_beside_an_owned_one() {
    let w = vec![0u8; WRITE_WINDOW];
    assert!(check_restore_blocks(&[(0x0250_0000, w.clone()), (0x0250_4000, w.clone())]).is_ok());
    assert!(check_restore_blocks(&[(0x0250_4000, w.clone())]).is_err());
    assert!(check_restore_blocks(&[(0x0480_0000, w.clone()), (0x0484_0000, w)]).is_err());
}

/// A backup stores a half's last window without its marker frame; the restore
/// guard accepts that shape, and only at the end of a half.
#[test]
fn restore_guard_accepts_a_trimmed_last_window() {
    let trimmed = vec![0u8; WRITE_WINDOW - MARKER_FRAME];
    assert!(check_restore_blocks(&[(0x0250_0000, vec![0; WRITE_WINDOW]), (0x0251_C000, trimmed.clone())]).is_ok());
    assert!(check_restore_blocks(&[(0x0250_0000, trimmed)]).is_err());
}
