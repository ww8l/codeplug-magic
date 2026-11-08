//! The gates, against the one file Tim's radio wrote
//! (`scratchpad/icom_id51/id51_01_base.icf`, gitignored — it is his radio).
//! `#[ignore]`d for CI; running one without the file FAILS rather than passing
//! having asserted nothing (#89).
//!
//! ```sh
//! cargo test --lib icom_id51::real_file -- --ignored --nocapture
//! ```

use crate::commands::export::ExpandedChannel;
use crate::models::Channel;
use crate::radios::icom_id52::icf::IcfFile;
use crate::radios::icom_id52::memory::{DTCS_CODES, TONES_DHZ};

use super::layout::{record_offset, BANK_TABLE, CHANNEL_COUNT, IMAGE_LEN, REC_LEN};
use super::memory::{encode_channel, is_used, read_bank, read_record, read_skip, store, Record, ScanSkip};

fn file() -> (String, IcfFile) {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../scratchpad/icom_id51/id51_01_base.icf");
    let text = std::fs::read_to_string(path).unwrap_or_else(|e| {
        panic!("this test needs the radio's own file at {path} ({e}); scratchpad/ is gitignored")
    });
    let icf = IcfFile::parse(&text).expect("the radio's file parses and its #CD verifies");
    (text, icf)
}

/// The envelope: the radio's model, revision and length, its checksum
/// reproduced, and an untouched file re-rendered byte for byte.
#[test]
#[ignore = "needs scratchpad/icom_id51/id51_01_base.icf"]
fn the_radios_file_round_trips_byte_for_byte() {
    let (text, icf) = file();
    super::check_is_an_id51_file(&icf).unwrap();
    assert_eq!(icf.image().len(), IMAGE_LEN);
    assert_eq!(icf.render(), text);
}

/// Every one of the 500 records — used or not — decodes and re-encodes to the
/// same 49 bytes.
#[test]
#[ignore = "needs scratchpad/icom_id51/id51_01_base.icf"]
fn every_record_re_encodes_byte_identically() {
    let (_, icf) = file();
    let image = icf.image();
    for slot in 0..CHANNEL_COUNT {
        let at = record_offset(slot);
        let raw = &image[at..at + REC_LEN];
        assert_eq!(Record::decode(raw).encode(), raw, "record {slot}");
    }
}

/// Storing each used memory back through `store` reproduces the radio's bank
/// entries, multiplier bits included. A skipped memory differs in its skip bits
/// only (`store` is for new channels), and memory 58's `1F FF` — the radio's
/// own form for a memory it was never asked to bank — comes back `1F 00`.
#[test]
#[ignore = "needs scratchpad/icom_id51/id51_01_base.icf"]
fn storing_the_radios_own_memories_reproduces_its_tables() {
    let (_, icf) = file();
    let image = icf.image().to_vec();
    let mut rebuilt = image.clone();
    let mut n = 0;
    for slot in (0..CHANNEL_COUNT).filter(|&s| is_used(&image, s)) {
        store(&mut rebuilt, slot, &read_record(&image, slot), read_bank(&image, slot));
        let e = BANK_TABLE + 2 * slot;
        if image[e + 1] == 0xFF && image[e] & 0x1F == 0x1F {
            assert_eq!(rebuilt[e..e + 2], [image[e], 0], "memory {slot}");
        } else {
            assert_eq!(rebuilt[e..e + 2], image[e..e + 2], "memory {slot}: bank entry");
        }
        n += 1;
    }
    let skipped = (0..CHANNEL_COUNT)
        .filter(|&s| is_used(&image, s) && read_skip(&image, s) != ScanSkip::None)
        .count();
    let differing: Vec<usize> = (0..IMAGE_LEN).filter(|&i| image[i] != rebuilt[i]).collect();
    // Skip bits share bitmap bytes, so a count of bytes is at most the number of
    // skip bits cleared, plus memory 58's index byte.
    assert!(differing.iter().all(|&i| (0x6A86..0x6B40).contains(&i) || (BANK_TABLE..BANK_TABLE + 1000).contains(&i)));
    assert_eq!(n, 87);
    println!("{n} memories re-stored; {} bytes differ ({skipped} skipped memories)", differing.len());
}

/// Eight 7-bit characters packed into 7 bytes, MSB first — the inverse of the
/// ID-52's `pack_call`.
fn unpack_call(p: &[u8; 7]) -> String {
    let v = p.iter().fold(0u64, |acc, &b| (acc << 8) | u64::from(b));
    (0..8).map(|i| char::from(((v >> (7 * (7 - i))) & 0x7F) as u8)).collect()
}

fn tone_hz(idx: u8) -> f64 {
    f64::from(TONES_DHZ[usize::from(idx)]) / 10.0
}

/// The app channel this memory would be, as an operator would have entered it.
fn channel_of(rec: &Record, id: i64) -> Channel {
    let rx = rec.rx_hz() / 1e6;
    let off = rec.offset_hz() / 1e6;
    let mut c = Channel {
        id,
        rx_freq: rx,
        offset: Some(off),
        duplex: Some(match rec.duplex { 1 => "-", 2 => "+", _ => "" }.into()),
        dcs_polarity: ["NN", "NR", "RN", "RR"][usize::from(rec.polarity & 3)].into(),
        ..Default::default()
    };
    c.mode = Some(match rec.mode { 1 => "NFM", 3 => "AM", 5 => "DV", _ => "FM" }.into());
    match rec.squelch {
        1 => {
            c.tone_mode = Some("Tone".into());
            c.ctcss_uplink = Some(tone_hz(rec.rtone));
        }
        3 => {
            c.tone_mode = Some("TSQL".into());
            c.ctcss_uplink = Some(tone_hz(rec.ctone));
            c.ctcss_downlink = Some(tone_hz(rec.ctone));
        }
        5 => {
            c.tone_mode = Some("DTCS".into());
            c.dcs_code = Some(format!("{:03}", DTCS_CODES[usize::from(rec.dtcs)]));
        }
        _ => {}
    }
    if rec.mode == 5 {
        let un = |p: &[u8; 7]| unpack_call(p);
        c.dstar_capable = true;
        c.dstar_ur_call = Some(un(&rec.ur));
        c.dstar_rpt1 = Some(un(&rec.rpt1));
        c.dstar_rpt2 = Some(un(&rec.rpt2));
    }
    c
}

/// ★ The step-3 gate: each of the radio's 87 memories, turned into the channel
/// the app would hold and encoded, gives back the radio's own bytes in every
/// field the radio reads.
///
/// The frequency is compared as a VALUE, and as bytes only where the memory was
/// made on the radio's front panel (bank entry `1F FF`: memories 58 and 82) —
/// those are the radio's own encoding choice, which the encoder must match. The
/// other memories came from a tool that preferred 6.25 kHz units; CS-51PLUS and
/// the front panel prefer 5 kHz, and both decode the same.
///
/// Not compared, because the radio ignores them or the operator chose them:
/// the tuning step (two memories set to 25 kHz by hand; new ones get 5 kHz),
/// the tone field the squelch type does not use, and the tone fields of a DV
/// memory (the radio leaves its 94.8 there; a new one gets 88.5).
#[test]
#[ignore = "needs scratchpad/icom_id51/id51_01_base.icf"]
fn the_radios_memories_re_encode_from_the_channels_they_describe() {
    let (_, icf) = file();
    let image = icf.image();
    let mut checked = 0;
    let mut front_panel = 0;
    for slot in (0..CHANNEL_COUNT).filter(|&s| is_used(image, s)) {
        let radio = read_record(image, slot);
        let ec = ExpandedChannel {
            channel: channel_of(&radio, slot as i64),
            tg_label: None,
            timeslot: None,
            tg_number: None,
            tg_call_type: None,
            tg_inline: false,
        };
        let ours = encode_channel(&ec, &radio.name()).unwrap_or_else(|e| panic!("memory {slot}: {e}"));
        let ctx = format!("memory {slot} {:?} {:.5}", radio.name(), radio.rx_hz() / 1e6);
        assert!((ours.rx_hz() - radio.rx_hz()).abs() < 1.0, "{ctx}: frequency");
        assert!((ours.offset_hz() - radio.offset_hz()).abs() < 1.0, "{ctx}: offset");
        assert_eq!(ours.mult1, ours.mult2, "{ctx}: one multiplier");
        let entry = BANK_TABLE + 2 * slot;
        if image[entry + 1] == 0xFF {
            assert_eq!((ours.mult1, ours.freq_units, ours.offset_units), (radio.mult1, radio.freq_units, radio.offset_units), "{ctx}: the front panel's own encoding");
            front_panel += 1;
        }
        assert_eq!((ours.mode, ours.duplex, ours.squelch), (radio.mode, radio.duplex, radio.squelch), "{ctx}: mode/duplex/squelch");
        assert_eq!(ours.name, radio.name, "{ctx}: name");
        assert_eq!((ours.byte9, ours.unknown1, ours.unknown2), (radio.byte9, radio.unknown1, radio.unknown2), "{ctx}: unknowns");
        match radio.squelch {
            1 => assert_eq!(ours.rtone, radio.rtone, "{ctx}: repeater tone"),
            3 => assert_eq!(ours.ctone, radio.ctone, "{ctx}: TSQL tone"),
            5 => assert_eq!((ours.dtcs, ours.polarity), (radio.dtcs, radio.polarity), "{ctx}: DTCS"),
            _ => {}
        }
        assert_eq!((ours.ur, ours.rpt1, ours.rpt2, ours.dv_code), (radio.ur, radio.rpt1, radio.rpt2, radio.dv_code), "{ctx}: D-STAR");
        checked += 1;
    }
    assert_eq!(checked, 87);
    assert!(front_panel >= 1, "no front-panel memory to hold the multiplier rule against");
    println!("{checked} memories re-encoded from their channels, every live field equal; {front_panel} front-panel memories byte-equal in frequency");
}

/// Programming the radio with the settings just read off it changes nothing:
/// all 241 fields plus the manual position decode and apply back with zero
/// bytes moved.
#[test]
#[ignore = "needs scratchpad/icom_id51/id51_01_base.icf"]
fn applying_the_settings_read_off_the_radio_changes_nothing() {
    let (_, icf) = file();
    let image = icf.image().to_vec();
    let decoded = super::settings::decode_settings(&image);
    let mut again = image.clone();
    let (n, notes) = super::settings::apply_settings(&mut again, decoded.as_object().unwrap());
    let moved: Vec<String> = (0..IMAGE_LEN)
        .filter(|&i| image[i] != again[i])
        .map(|i| format!("{i:#x}:{:02x}->{:02x}", image[i], again[i]))
        .collect();
    assert!(moved.is_empty(), "applying its own settings moved {moved:?} (notes {notes:?})");
    assert!(notes.is_empty(), "{notes:?}");
    println!("{n} settings re-applied, zero bytes moved");
}

/// Print every decoded setting — the sheet Tim reads his radio's menus against.
#[test]
#[ignore = "needs scratchpad/icom_id51/id51_01_base.icf"]
fn dump_decoded_settings() {
    let (_, icf) = file();
    let v = super::settings::decode_settings(icf.image());
    for (k, val) in v.as_object().unwrap() {
        println!("{k} = {val}");
    }
}
