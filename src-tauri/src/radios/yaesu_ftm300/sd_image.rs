//! FTM-300DR microSD codeplug writer: patch memories into the radio's own
//! `FTM300D/BACKUP/CLNFTM300D.dat`.
//!
//! ## Why patch instead of generate
//!
//! The file is the radio's whole configuration — memories, Home and PMS
//! channels, every Set Mode item, APRS, the station list, the GM/WIRES-X
//! state — and a block at `0x400` that RT Systems does not even carry
//! (see below). Generating one from scratch would revert all of it, so this
//! takes the operator's own backup as the template and rewrites exactly the
//! 999 regular memories in three parallel tables:
//!
//! | region | address | what |
//! |---|---|---|
//! | `memory[999]` | `0x0800` | 16-byte records |
//! | `name_len[999]` | `0x4CB0` | u16 BE `(1 << (16 - len)) - 1`, `0xFFFF` = no name |
//! | `name[999]` | `0x10000` | 16 ASCII bytes, `0xFF`-padded |
//!
//! Each table continues for 100 more entries (`1099` in all) — the 50 PMS
//! pairs, by count — which are left exactly as the operator had them.
//!
//! ## Provenance
//!
//! The record layout is CHIRP's `ftm300d.py` (2026-09), corrected against
//! Tim's own radio: all 119 of his channels decode to RT Systems' reading of
//! the same file with zero mismatches across 15 columns, and two designed probe
//! passes (34 + 37 channels written by RT Systems) pinned the fields his
//! codeplug never exercised. Three of CHIRP's fields are wrong for this radio
//! and are corrected here: the offset is a **u16** at bytes 12-13 (CHIRP: u8 at
//! 13), byte 11's low five bits are the **user CTCSS** (CHIRP: "occupied"), and
//! the name-length table is a length code, not a 0/0xFFFF flag (CHIRP writes
//! `0x0000`, which says "16 characters"). The byte tables are in
//! `scratchpad/ftm300/FINDINGS.md`.
//!
//! ## No checksum
//!
//! None was found: a probe that added 34 channels changed nothing outside the
//! three tables above, and xmutantson/ftm300dr-tools reports the radio
//! restores an edited file as-is. This is the claim the hardware ladder's
//! one-byte write exists to test — an identity copy cannot.
//!
//! ## The `0x400` block
//!
//! RT Systems' re-save of an unchanged file blanks `0x400-0x45F` and
//! `0x478-0x47F` to `0xFF` — radio-specific data CHIRP's clone protocol also
//! never sends. Patching the operator's own file keeps it by construction, and
//! [`patch_backup`] asserts it.

use crate::commands::export::{expanded_names, tx_frequency, ExpandedChannel};
use crate::models::RadioModel;

pub(crate) const IMAGE_LEN: usize = 0x18000;
const MAGIC: &[u8] = b"AH071$";

const MEM_BASE: usize = 0x0800;
const MEM_STRIDE: usize = 16;
/// Regular memories. The tables run on for 100 more entries (PMS).
pub(crate) const MEM_SLOTS: usize = 999;
const NAME_LEN_BASE: usize = 0x4CB0;
const NAME_BASE: usize = 0x10000;
pub(crate) const NAME_LEN: usize = 16;

/// What the radio itself holds in an empty slot — 979 of the 980 empty slots in
/// Tim's backup are exactly this (144.000 MHz, 12.5 kHz step, 100.0 Hz,
/// 600 kHz). CHIRP and xmutantson each use a different record; the radio's own
/// is the one to copy.
const EMPTY_RECORD: [u8; MEM_STRIDE] = [
    0x01, 0x00, 0x01, 0x44, 0x00, 0x04, 0x00, 0x00, 0x00, 0x0C, 0x00, 0x0F, 0x00, 0x0C, 0x00, 0x00,
];

/// The standard 50-tone CTCSS table, in tenths of a hertz.
const TONES_DHZ: [u16; 50] = [
    670, 693, 719, 744, 770, 797, 825, 854, 885, 915, 948, 974, 1000, 1035, 1072, 1109, 1148,
    1188, 1230, 1273, 1318, 1365, 1413, 1462, 1514, 1567, 1598, 1622, 1655, 1679, 1713, 1738,
    1773, 1799, 1835, 1862, 1899, 1928, 1966, 1995, 2035, 2065, 2107, 2181, 2257, 2291, 2336,
    2418, 2503, 2541,
];
/// 100.0 Hz — the tone an empty slot and an untoned channel carry.
const DEFAULT_TONE: u8 = 12;

/// The 104 DCS codes; the stored index is 0-based (RT Systems `073` = 0x0F,
/// `754` = 0x67 — xmutantson's 1-based reading is wrong).
const DTCS_CODES: [u16; 104] = [
    23, 25, 26, 31, 32, 36, 43, 47, 51, 53, 54, 65, 71, 72, 73, 74, 114, 115, 116, 122, 125, 131,
    132, 134, 143, 145, 152, 155, 156, 162, 165, 172, 174, 205, 212, 223, 225, 226, 243, 244, 245,
    246, 251, 252, 255, 261, 263, 265, 266, 271, 274, 306, 311, 315, 325, 331, 332, 343, 346, 351,
    356, 364, 365, 371, 411, 412, 413, 423, 431, 432, 445, 446, 452, 454, 455, 462, 464, 465, 466,
    503, 506, 516, 523, 526, 532, 546, 565, 606, 612, 624, 627, 631, 632, 654, 662, 664, 703, 712,
    723, 731, 732, 734, 743, 754,
];

// Byte 0.
const USED: u8 = 0x80;
/// The one user memory group, M-GRP (RT Systems "Memory Group"; set on 5 of
/// Tim's channels and on probe 222).
const MGRP: u8 = 0x10;
// Byte 1.
const AM: u8 = 0x10;
const NARROW: u8 = 0x20;
const DUPLEX_SIMPLEX: u8 = 0;
const DUPLEX_MINUS: u8 = 2;
const DUPLEX_PLUS: u8 = 3;
const DUPLEX_SPLIT: u8 = 4;
// Byte 5, high nibble.
const TMODE_OFF: u8 = 0;
const TMODE_TONE: u8 = 1;
const TMODE_TSQL: u8 = 2;
const TMODE_DCS: u8 = 4;
// Byte 11: DN (0x40), VW (0x20 with 0x40), user CTCSS in Hz/100 (low 5 bits).
const DIGITAL_DN: u8 = 0x40;
const USER_CTCSS_1500: u8 = 15;

/// Largest repeater shift the radio offers (Set Mode RPT SHIFT FREQ, 0.00-99.95
/// MHz in 50 kHz steps). A wider or off-grid TX/RX pair is stored as Split.
const MAX_SHIFT_HZ: i64 = 99_950_000;
const SHIFT_UNIT_HZ: i64 = 50_000;

/// Refuse a file that is not an FTM-300 ALL backup, naming what it is.
pub(crate) fn validate_backup(image: &[u8]) -> Result<(), String> {
    if image.len() != IMAGE_LEN {
        return Err(format!(
            "This is not an FTM-300 backup: it is {} bytes, and the radio's \
             FTM300D/BACKUP/CLNFTM300D.dat is always {IMAGE_LEN}. On the radio, \
             SD CARD → BACKUP → WRITE TO SD → ALL, then pick that file.",
            image.len()
        ));
    }
    if &image[..MAGIC.len()] != MAGIC {
        return Err(format!(
            "This backup is from a different radio (it starts {:?}; an FTM-300 \
             backup starts \"AH071$\"). The FTM-300 writer only patches FTM-300 backups.",
            String::from_utf8_lossy(&image[..MAGIC.len()])
        ));
    }
    Ok(())
}

/// The radio's frequency encoding: the low nibble of the first byte and two
/// BCD bytes give the frequency in 10 kHz units (hundreds of MHz in the
/// nibble), and the first byte's HIGH nibble adds the sub-10 kHz part as flags
/// worth 5, 2.5, 1.25 and 0.625 kHz.
///
/// The remainder is truncated to the 625 Hz grid. That is what RT Systems does
/// with airband 8.33 kHz channels — 127.80833 stores as `0xD` (8.125 kHz) — and
/// it is exact for every 5, 6.25 and 12.5 kHz channel.
fn encode_freq(hz: i64) -> [u8; 3] {
    let hz = hz.max(0);
    let tens = (hz / 10_000) as u32; // 10 kHz units, e.g. 14652 for 146.52 MHz
    let mut rem = hz % 10_000;
    let mut fine = 0u8;
    for (bit, add) in [(0x80u8, 5000i64), (0x40, 2500), (0x20, 1250), (0x10, 625)] {
        if rem >= add {
            fine |= bit;
            rem -= add;
        }
    }
    let d = |n: u32| (n % 10) as u8;
    [
        fine | d(tens / 10_000),
        (d(tens / 1000) << 4) | d(tens / 100),
        (d(tens / 10) << 4) | d(tens),
    ]
}

#[cfg(test)]
fn decode_freq(b: &[u8]) -> i64 {
    let bcd = |x: u8| ((x >> 4) * 10 + (x & 0x0F)) as i64;
    let tens = (b[0] & 0x0F) as i64 * 10_000 + bcd(b[1]) * 100 + bcd(b[2]);
    let mut hz = tens * 10_000;
    for (bit, add) in [(0x80u8, 5000i64), (0x40, 2500), (0x20, 1250), (0x10, 625)] {
        if b[0] & bit != 0 {
            hz += add;
        }
    }
    hz
}

/// Byte 0's low nibble. Measured on probes RT Systems wrote at 118.4, 146.52,
/// 222.1, 446 and 851.0125 MHz — the radio's five receive ranges.
fn band_code(hz: i64) -> u8 {
    match hz {
        h if h < 137_000_000 => 0,
        h if h < 174_000_000 => 1,
        h if h < 400_000_000 => 2,
        h if h < 480_000_000 => 3,
        _ => 4,
    }
}

/// The shift the radio keeps in a record that has none — RT Systems and the
/// radio both leave the band's default there (600 kHz on 2 m, 5 MHz on 70 cm,
/// 1.6 MHz in the 222 range, nothing on airband or above 480 MHz). Matching it
/// keeps our records byte-identical with the radio's own.
fn default_shift_units(hz: i64) -> u16 {
    match band_code(hz) {
        1 => 12,
        2 => 32,
        3 => 100,
        _ => 0,
    }
}

/// Tuning step for the record's low nibble of byte 5: 0 is 5 kHz (RT Systems
/// shows it as Auto too), 1 is 6.25, 2 is 8.33 (airband), 4 is 12.5. A memory
/// is a fixed frequency either way, so this matters only when the operator dials
/// away from it — but it must agree with the frequency's own grid.
fn step_code(hz: i64) -> u8 {
    let rem = hz % 10_000;
    if rem % 5000 == 0 {
        0
    } else if rem % 625 != 0 {
        2 // only the airband's 8.33 kHz channels fall off the 625 Hz grid
    } else if rem % 2500 == 0 {
        4
    } else {
        1
    }
}

fn tone_index(hz: f64) -> u8 {
    let dhz = (hz * 10.0).round() as i32;
    let mut best = 0usize;
    for (i, &t) in TONES_DHZ.iter().enumerate() {
        if (t as i32 - dhz).abs() < (TONES_DHZ[best] as i32 - dhz).abs() {
            best = i;
        }
    }
    best as u8
}

fn dcs_index(code: Option<&str>) -> Option<u8> {
    let n: u16 = code?.trim().parse().ok()?;
    DTCS_CODES.iter().position(|&c| c == n).map(|i| i as u8)
}

/// (tone mode nibble, CTCSS index, DCS index).
///
/// The radio does have cross modes (T DCS = 8, D Tone = 9, D Code = 7), but
/// they only work with Set Mode SQL EXPANSION switched on, which nothing here
/// controls — so Cross falls back the project-wide way
/// (`cross-tone-unsupported-fallback`): keep the TX tone, drop the RX side.
/// An untoned channel still carries a CTCSS index, as the radio's own records
/// do: the channel's tone if it has one, else 100.0 Hz.
fn encode_tone(c: &crate::models::Channel) -> (u8, u8, u8) {
    let mode = c.tone_mode.as_deref().unwrap_or("off");
    let carried = c.ctcss_uplink.or(c.ctcss_downlink).map(tone_index).unwrap_or(DEFAULT_TONE);
    let dcs = || dcs_index(c.dcs_code.as_deref()).unwrap_or(0);
    if mode.eq_ignore_ascii_case("Tone") {
        (TMODE_TONE, carried, 0)
    } else if mode.eq_ignore_ascii_case("TSQL") {
        (TMODE_TSQL, carried, 0)
    } else if mode.eq_ignore_ascii_case("DTCS") {
        (TMODE_DCS, carried, dcs())
    } else if mode.eq_ignore_ascii_case("Cross") {
        let (tx, _) = c.cross_mode.split_once("->").unwrap_or(("", ""));
        if tx.eq_ignore_ascii_case("DTCS") {
            (TMODE_DCS, carried, dcs())
        } else if tx.eq_ignore_ascii_case("Tone") {
            (TMODE_TONE, carried, 0)
        } else {
            (TMODE_OFF, carried, 0)
        }
    } else {
        (TMODE_OFF, carried, 0)
    }
}

/// Byte 9's power field: 0 High (50 W), 1 Medium (25 W), 2 Low (5 W).
fn power_code(power: Option<&str>) -> u8 {
    match power {
        Some(p) if p.eq_ignore_ascii_case("Low") => 2,
        Some(p) if p.eq_ignore_ascii_case("Med") || p.eq_ignore_ascii_case("Medium") => 1,
        _ => 0,
    }
}

/// Encode one channel into its 16-byte record.
///
/// Receive-only channels (airband, NOAA, GMRS…) are encoded like any other —
/// the radio enforces its own TX bands, and the warning belongs in the program
/// dialog, not in a byte the radio ignores (the FT5D's rule, for the same
/// reason).
pub(crate) fn encode_record(ec: &ExpandedChannel) -> [u8; MEM_STRIDE] {
    let c = &ec.channel;
    let rx = hz(c.rx_freq);
    let tx = hz(tx_frequency(c));
    let diff = tx - rx;

    let (duplex, shift_units, tx_field) = if diff == 0 {
        (DUPLEX_SIMPLEX, default_shift_units(rx), [0u8; 3])
    } else if diff.abs() <= MAX_SHIFT_HZ && diff % SHIFT_UNIT_HZ == 0 {
        let dir = if diff > 0 { DUPLEX_PLUS } else { DUPLEX_MINUS };
        (dir, (diff.abs() / SHIFT_UNIT_HZ) as u16, [0u8; 3])
    } else {
        (DUPLEX_SPLIT, default_shift_units(rx), encode_freq(tx))
    };

    let mode = c.mode.as_deref().unwrap_or("FM");
    let is = |m: &str| mode.eq_ignore_ascii_case(m);

    let mut r = [0u8; MEM_STRIDE];
    r[0] = USED | band_code(rx);
    r[1] = duplex
        | if is("AM") { AM } else { 0 }
        | if is("NFM") { NARROW } else { 0 };
    r[2..5].copy_from_slice(&encode_freq(rx));
    let (tmode, tone, dcs) = encode_tone(c);
    r[5] = (tmode << 4) | step_code(rx);
    r[6..9].copy_from_slice(&tx_field);
    r[9] = (power_code(c.power.as_deref()) << 6) | tone;
    r[10] = dcs; // clock shift (0x80) stays off
    // A YSF channel is programmed DN-fixed, as the operator's own C4FM
    // repeater channels are; AMS (byte 1, 0x40) stays off.
    r[11] = USER_CTCSS_1500 | if is("YSF") { DIGITAL_DN } else { 0 };
    r[12..14].copy_from_slice(&shift_units.to_be_bytes());
    // 14/15: RX/TX DG-ID 00.
    r
}

/// The name-length code at `0x4CB0`: `(1 << (16 - len)) - 1`, so 1 character
/// is `0x7FFF`, 16 is `0x0000`, and no name at all is `0xFFFF`. The radio
/// reads THIS as the tag's length — a name with the code left at `0xFFFF`
/// shows blank. Confirmed on all 119 names in Tim's backup and on probes of 1,
/// 10 and 16 characters.
fn name_len_code(len: usize) -> u16 {
    let len = len.min(NAME_LEN) as u32;
    if len == 0 {
        0xFFFF
    } else {
        ((1u32 << (16 - len)) - 1) as u16
    }
}

/// Printable ASCII, trailing spaces dropped (they would count toward the
/// length code), at most 16 characters. Lower case is kept — the radio stores
/// and displays it.
pub(super) fn tag(name: &str) -> String {
    let s: String = name
        .chars()
        .filter(|c| c.is_ascii_graphic() || *c == ' ')
        .take(NAME_LEN)
        .collect();
    s.trim_end().to_string()
}

/// A cleaned tag's two stored forms: the 16-byte name field and its length code.
pub(super) fn name_fields(name: &str) -> ([u8; NAME_LEN], u16) {
    let mut field = [0xFFu8; NAME_LEN];
    field[..name.len()].copy_from_slice(name.as_bytes());
    (field, name_len_code(name.len()))
}

fn write_slot(img: &mut [u8], slot: usize, record: &[u8; MEM_STRIDE], name: &str) {
    let at = MEM_BASE + slot * MEM_STRIDE;
    img[at..at + MEM_STRIDE].copy_from_slice(record);
    let (field, code) = name_fields(name);
    img[NAME_BASE + slot * NAME_LEN..NAME_BASE + (slot + 1) * NAME_LEN].copy_from_slice(&field);
    img[NAME_LEN_BASE + slot * 2..NAME_LEN_BASE + slot * 2 + 2].copy_from_slice(&code.to_be_bytes());
}

/// Patch `template` with `channels` in memory-slot order (slot 1 first).
///
/// Every one of the 999 regular slots is rewritten — a filled one with its
/// channel, the rest with the radio's own empty record — so nothing from the
/// operator's old memory list survives into the new one. Home, PMS, VFO and
/// every setting are the template's. There are no banks on this radio, so
/// channel lists flatten; its one user group, M-GRP, is a flag on each memory,
/// set for every channel whose id is in `mgrp` (chosen per codeplug).
///
/// Overflow is an error rather than a silent truncation.
pub(crate) fn patch_backup(
    template: &[u8],
    channels: &[&ExpandedChannel],
    model: &RadioModel,
    mgrp: &std::collections::HashSet<i64>,
) -> Result<Vec<u8>, String> {
    validate_backup(template)?;
    if channels.len() > MEM_SLOTS {
        return Err(format!(
            "This codeplug has {} channels; the FTM-300 holds {MEM_SLOTS}. \
             Remove {} before exporting.",
            channels.len(),
            channels.len() - MEM_SLOTS
        ));
    }
    let mut img = template.to_vec();
    let names = expanded_names(channels.iter().copied(), model);
    for (slot, (ec, name)) in channels.iter().zip(&names).enumerate() {
        let mut record = encode_record(ec);
        if mgrp.contains(&ec.channel.id) {
            record[0] |= MGRP;
        }
        write_slot(&mut img, slot, &record, &tag(name));
    }
    for slot in channels.len()..MEM_SLOTS {
        write_slot(&mut img, slot, &EMPTY_RECORD, "");
    }
    debug_assert_eq!(&img[0x400..0x480], &template[0x400..0x480]);
    Ok(img)
}

fn hz(mhz: f64) -> i64 {
    (mhz * 1_000_000.0).round() as i64
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::Channel;

    fn chan(rx: f64, tx: Option<f64>) -> Channel {
        Channel {
            id: 1,
            name_short: Some("TEST".into()),
            rx_freq: rx,
            tx_freq: tx,
            dcs_polarity: "NN".into(),
            cross_mode: "Tone->Tone".into(),
            source: "test".into(),
            ..Default::default()
        }
    }

    fn ec(c: Channel) -> ExpandedChannel {
        ExpandedChannel {
            channel: c,
            tg_label: None,
            timeslot: None,
            tg_number: None,
            tg_call_type: None,
            tg_inline: false,
        }
    }

    #[test]
    fn frequency_encoding_matches_the_radio() {
        // Pairs read off Tim's backup and RT Systems' probe passes.
        let cases: &[(i64, [u8; 3])] = &[
            (438_550_000, [0x04, 0x38, 0x55]),
            (447_275_000, [0x84, 0x47, 0x27]),
            (146_625_000, [0x81, 0x46, 0x62]),
            (118_400_000, [0x01, 0x18, 0x40]),
            (446_106_250, [0xA4, 0x46, 0x10]),
            (446_112_500, [0x44, 0x46, 0x11]),
            (851_012_500, [0x48, 0x51, 0x01]),
            (127_808_330, [0xD1, 0x27, 0x80]),
        ];
        for &(hz, bytes) in cases {
            assert_eq!(encode_freq(hz), bytes, "{hz}");
        }
        for &(hz, bytes) in &cases[..7] {
            assert_eq!(decode_freq(&bytes), hz, "{hz}");
        }
    }

    #[test]
    fn name_length_codes_match_the_radio() {
        assert_eq!(name_len_code(0), 0xFFFF);
        assert_eq!(name_len_code(1), 0x7FFF);
        assert_eq!(name_len_code(3), 0x1FFF);
        assert_eq!(name_len_code(7), 0x01FF);
        assert_eq!(name_len_code(10), 0x003F);
        assert_eq!(name_len_code(12), 0x000F);
        assert_eq!(name_len_code(16), 0x0000);
    }

    #[test]
    fn repeater_and_split_records() {
        // NCARC 447.275 −5 MHz TSQL 100.0, exactly as the radio stores it.
        let mut c = chan(447.275, Some(442.275));
        c.tone_mode = Some("TSQL".into());
        c.ctcss_uplink = Some(100.0);
        c.ctcss_downlink = Some(100.0);
        assert_eq!(
            encode_record(&ec(c)),
            [0x83, 0x02, 0x84, 0x47, 0x27, 0x20, 0, 0, 0, 0x0C, 0, 0x0F, 0, 0x64, 0, 0]
        );
        // ISS 437.800 / 145.990, Tone 67.0 — too wide for a shift, so Split.
        let mut c = chan(437.8, Some(145.99));
        c.tone_mode = Some("Tone".into());
        c.ctcss_uplink = Some(67.0);
        assert_eq!(
            encode_record(&ec(c)),
            [0x83, 0x04, 0x04, 0x37, 0x80, 0x10, 0x01, 0x45, 0x99, 0x00, 0, 0x0F, 0, 0x64, 0, 0]
        );
    }

    #[test]
    fn modes_power_and_dcs() {
        let mut c = chan(447.825, Some(442.825));
        c.mode = Some("NFM".into());
        c.tone_mode = Some("DTCS".into());
        c.dcs_code = Some("073".into());
        c.ctcss_uplink = Some(88.5);
        let r = encode_record(&ec(c));
        assert_eq!(r[1], 0x22);
        assert_eq!(r[5] >> 4, 4);
        assert_eq!(r[10], 0x0E); // 073 is index 14 (0x0E); see DTCS_CODES
        let mut c = chan(118.4, None);
        c.mode = Some("AM".into());
        c.power = Some("Low".into());
        let r = encode_record(&ec(c));
        assert_eq!((r[0], r[1], r[9] >> 6, r[13]), (0x80, 0x10, 2, 0));
        let mut c = chan(147.0, Some(147.6));
        c.mode = Some("YSF".into());
        c.power = Some("Med".into());
        let r = encode_record(&ec(c));
        assert_eq!((r[1], r[9] >> 6, r[11]), (0x03, 1, 0x4F));
    }

    #[test]
    fn rejects_other_files() {
        assert!(validate_backup(&vec![0u8; IMAGE_LEN]).is_err());
        assert!(validate_backup(&[0u8; 100]).is_err());
        let mut ok = vec![0xFFu8; IMAGE_LEN];
        ok[..6].copy_from_slice(MAGIC);
        assert!(validate_backup(&ok).is_ok());
    }
}
