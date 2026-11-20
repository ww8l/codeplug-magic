//! FTM-400 microSD codeplug writer: patch memories into the radio's own
//! `FTM400D/BACKUP/CLONE/CLNFTM400D.dat` (SD CARD → BACKUP → Write to SD → ALL).
//!
//! ## Layout
//!
//! Every address and encoding here is Yaesu's own: ADMS-7 v1.1's
//! `DataConverter` (decompiled in featherfeet/ftm400xdrprogrammer) reads and
//! writes exactly this file, and Tim's radio-written backup decodes under it
//! with no errors across all 392 of his memories.
//!
//! | region | address | what |
//! |---|---|---|
//! | header | `0x0000` | magic `AH034$`, `[0x7F]` = byte sum of `0x00..0x7F` |
//! | Band A memories | `0x0800` | 500 + 18 PMS records, 16 bytes each |
//! | Band B memories | `0x2860` | the same for Band B |
//! | Band A names | `0x48C0` | 500 + 18 names, 8 bytes each |
//! | Band B names | `0x58F0` | the same for Band B |
//!
//! Only the 500 regular memories of each band are rewritten — plus, when the
//! codeplug needs it, each band's RX COVERAGE bit (below). The header — and so
//! its checksum — PMS, VFO, Home, every other setting and the APRS logs are the
//! operator's own, byte for byte.
//!
//! ## Both bands get the same list — and may need RX COVERAGE WIDE
//!
//! The radio keeps two independent 500-memory lists, one per band. Tim's own
//! codeplug holds the same channels on both, so the operator can recall any of
//! them on either band; the writer does the same.
//!
//! CONFIG 11 RX COVERAGE is a per-band setting, and at NORMAL (the factory
//! default) a band shows only 137-480 MHz. Measured on Tim's radio (2026-11-20):
//! with both bands at NORMAL, a file's air-band (AM) and 480 MHz-and-up memories
//! were holes on both bands; switching Band B to WIDE made them appear there.
//! So a codeplug holding such a channel also sets both bands to WIDE
//! ([`needs_wide`], [`set_wide`]) — otherwise those channels would export
//! "successfully" and never appear.

//! ## The record
//!
//! The FTM-300/510 family's 16-byte record, with two differences ADMS-7 shows:
//! byte 11 bit 7 is a "has a name" flag (there is no digital-mode field — the
//! FTM-400's AMS is per band, not per memory), and bytes 14-15 are unused.
//!
//! ## Names
//!
//! Eight characters in Yaesu's own character set (`0x00`-`0x09` digits,
//! `0x0A` A…, `0x64` space), padded with `0xCA`. ⚠ The radio writes the digit
//! zero as **`0xBF`** — all 290 zeros in Tim's backup — where ADMS-7 v1.1
//! writes `0x00`. This writer follows the radio — and the radio showed the
//! zeros in names this writer wrote (2026-11-20).

use crate::commands::export::{expanded_names, tx_frequency, ExpandedChannel};
use crate::models::RadioModel;

pub(crate) const IMAGE_LEN: usize = 76_800;
const MAGIC: &[u8] = b"AH034$";

pub(crate) const MEM_STRIDE: usize = 16;
pub(crate) const NAME_LEN: usize = 8;
/// Regular memories per band. Each table runs on for 18 more (9 PMS pairs).
pub(crate) const MEM_SLOTS: usize = 500;

/// One band's two tables.
pub(crate) struct Bank {
    pub(crate) mem: usize,
    pub(crate) name: usize,
    /// What the radio itself holds in an empty slot of this band — all 320
    /// empty A slots and all 324 empty B slots in Tim's backup.
    empty: [u8; MEM_STRIDE],
}

pub(crate) const BANKS: [Bank; 2] = [
    // A: 144.000, 5 kHz step, 100.0 Hz, 600 kHz.
    Bank {
        mem: 0x0800,
        name: 0x48C0,
        empty: [0x01, 0x00, 0x01, 0x44, 0x00, 0x00, 0, 0, 0, 0x0C, 0x00, 0x0F, 0x00, 0x0C, 0, 0],
    },
    // B: 430.000, 25 kHz step, 100.0 Hz, 5 MHz.
    Bank {
        mem: 0x2860,
        name: 0x58F0,
        empty: [0x03, 0x00, 0x04, 0x30, 0x00, 0x07, 0, 0, 0, 0x0C, 0x00, 0x0F, 0x00, 0x64, 0, 0],
    },
];

/// Whether the bands must be at RX COVERAGE WIDE to show every channel: at
/// NORMAL a band hides everything outside 137-480 MHz (measured on Tim's radio).
pub(crate) fn needs_wide(channels: &[&ExpandedChannel]) -> bool {
    channels.iter().any(|ec| !(137_000_000..480_000_000).contains(&hz(ec.channel.rx_freq)))
}

/// Each band's RX COVERAGE (CONFIG 11) bit — the `rx-coverage-a` / `-b`
/// settings' WIDE value, as ADMS-7 writes it; WIDE on Band B was honoured by
/// the radio.
const RX_COVERAGE: [(usize, u8); 2] = [(0x00E3, 0x20), (0x00F3, 0x20)];

/// Set both bands to RX COVERAGE WIDE; returns whether either band was NORMAL.
pub(crate) fn set_wide(img: &mut [u8]) -> bool {
    let mut changed = false;
    for (at, bit) in RX_COVERAGE {
        changed |= img[at] & bit == 0;
        img[at] |= bit;
    }
    changed
}

/// How many of the memories already in `img` (either band) lie outside
/// 137-480 MHz, which a band at RX COVERAGE NORMAL hides. A settings-only write
/// uses this: it has no channel list, only the file the last export wrote.
pub(crate) fn memories_needing_wide(img: &[u8]) -> usize {
    BANKS
        .iter()
        .flat_map(|bank| (0..MEM_SLOTS).map(move |i| bank.mem + i * MEM_STRIDE))
        .filter(|&at| img[at] & 0x80 != 0)
        .filter(|&at| !(137_000_000..480_000_000).contains(&decode_freq(&img[at + 2..at + 5])))
        .count()
}

/// The operator-facing note for a coverage change, or None if nothing changed.
pub(crate) fn wide_note(changed: bool, outside: usize) -> Option<String> {
    changed.then(|| {
        format!(
            "RX COVERAGE (CONFIG 11) was set to WIDE on Band A and Band B: {outside} memor{} \
             outside 137–480 MHz, which a band at NORMAL does not show.",
            if outside == 1 { "y lies" } else { "ies lie" }
        )
    })
}

const NAME_FILL: u8 = 0xCA;
/// The digit zero as the radio writes it (see the module doc).
const ZERO: u8 = 0xBF;
const SPACE: u8 = 0x64;
/// The printable characters in code order from `0x00`: ADMS-7's
/// `Moji.tbl_memoryName`, English set. `0x00` ('0') is never written — see
/// [`ZERO`].
const CHARSET: &str = "0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz!\"#$%&'()*+,-./:;<=>?@[\\]^_`{|}~";

/// The standard 50-tone CTCSS table, in tenths of a hertz (ADMS-7 `tbl_ToneFreq`).
const TONES_DHZ: [u16; 50] = [
    670, 693, 719, 744, 770, 797, 825, 854, 885, 915, 948, 974, 1000, 1035, 1072, 1109, 1148,
    1188, 1230, 1273, 1318, 1365, 1413, 1462, 1514, 1567, 1598, 1622, 1655, 1679, 1713, 1738,
    1773, 1799, 1835, 1862, 1899, 1928, 1966, 1995, 2035, 2065, 2107, 2181, 2257, 2291, 2336,
    2418, 2503, 2541,
];
/// 100.0 Hz — the tone an empty slot and an untoned channel carry.
const DEFAULT_TONE: u8 = 12;

/// The 104 DCS codes; the stored index is 0-based (ADMS-7 `tbl_DcsCode`).
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
// Byte 11.
const NAMED: u8 = 0x80;
/// Low five bits: the PR FREQ tone in Hz/100. 1500 Hz is what the radio keeps
/// in an empty slot and in the memories its operator entered by hand.
const PR_TONE_1500: u8 = 15;

/// Largest repeater shift the radio offers (0.00-99.95 MHz in 50 kHz steps —
/// ADMS-7 `checkShiftFreq`). A wider or off-grid pair is stored as split.
const MAX_SHIFT_HZ: i64 = 99_950_000;
const SHIFT_UNIT_HZ: i64 = 50_000;

/// The receiver: 108-999.99 MHz with the US cellular bands blocked
/// (Operating Manual specifications). A frequency outside it would become a
/// slot the radio shows empty while the export reported success.
const RX_RANGES_HZ: [(i64, i64); 3] =
    [(108_000_000, 824_000_000), (849_000_000, 869_000_000), (894_000_000, 999_990_000)];

/// Refuse a file that is not an FTM-400 ALL backup, naming what it is.
pub(crate) fn validate_backup(image: &[u8]) -> Result<(), String> {
    if image.len() != IMAGE_LEN {
        let hint = if image.len() == 25_600 {
            " That size is the MEMORY-only backup (MEMFTM400D.dat); this needs the ALL backup."
        } else {
            ""
        };
        return Err(format!(
            "This is not an FTM-400 backup: it is {} bytes, and the radio's \
             FTM400D/BACKUP/CLONE/CLNFTM400D.dat is always {IMAGE_LEN}.{hint} On the radio, \
             hold [DISP] → SD CARD → BACKUP → Write to SD → ALL, then pick that file.",
            image.len()
        ));
    }
    if &image[..MAGIC.len()] != MAGIC {
        let other = match &image[..MAGIC.len()] {
            b"AH071$" => " (that is an FTM-300 backup)",
            b"AH089$" => " (that is an FTM-510 backup)",
            b"AH081$" => " (that is an FTM-500 backup)",
            _ => "",
        };
        return Err(format!(
            "This backup is from a different radio: it starts {:?}{other}, and an FTM-400 \
             backup starts \"AH034$\". The FTM-400 writer only patches FTM-400 backups.",
            String::from_utf8_lossy(&image[..MAGIC.len()])
        ));
    }
    Ok(())
}

/// The sub-10 kHz part of a frequency, as the high nibble of its first byte
/// (ADMS-7 `tbl_1KHz_freq`, in Hz). The 1.25 kHz multiples are the bit sum
/// the FTM-300 measured (8 = 5, 4 = 2.5, 2 = 1.25 kHz); the four 8.33 kHz
/// airband points have codes of their own, and Tim's radio displayed all four
/// as written (2026-11-20). `None` = not representable.
fn fine_code(rem: i64) -> Option<u8> {
    match rem {
        0 => Some(0x0),
        1250 => Some(0x2),
        1660..=1670 => Some(0x3),
        2500 => Some(0x4),
        3330..=3340 => Some(0x5),
        3750 => Some(0x6),
        5000 => Some(0x8),
        6250 => Some(0xA),
        6660..=6670 => Some(0xB),
        7500 => Some(0xC),
        8330..=8340 => Some(0xD),
        8750 => Some(0xE),
        _ => None,
    }
}

const FINE_HZ: [i64; 16] = [0, 0, 1250, 1666, 2500, 3333, 3750, 0, 5000, 0, 6250, 6666, 7500, 8333, 8750, 0];

fn encode_freq(hz: i64) -> [u8; 3] {
    let hz = hz.max(0);
    let tens = (hz / 10_000) as u32; // 10 kHz units, e.g. 14652 for 146.52 MHz
    let fine = fine_code(hz % 10_000).unwrap_or(0) << 4;
    let d = |n: u32| (n % 10) as u8;
    [
        fine | d(tens / 10_000),
        (d(tens / 1000) << 4) | d(tens / 100),
        (d(tens / 10) << 4) | d(tens),
    ]
}

/// The inverse of [`encode_freq`]; an 8.33 kHz point comes back to the Hz.
pub(crate) fn decode_freq(b: &[u8]) -> i64 {
    let bcd = |x: u8| ((x >> 4) * 10 + (x & 0x0F)) as i64;
    let tens = (b[0] & 0x0F) as i64 * 10_000 + bcd(b[1]) * 100 + bcd(b[2]);
    tens * 10_000 + FINE_HZ[(b[0] >> 4) as usize]
}

/// Whether the radio can hold `hz` as written: inside its receiver and on a
/// grid the record can express.
pub(crate) fn writable(hz: i64) -> bool {
    RX_RANGES_HZ.iter().any(|&(lo, hi)| (lo..=hi).contains(&hz)) && fine_code(hz % 10_000).is_some()
}

/// Byte 0's low bits: ADMS-7's band index (`tbl_RecvFreq`) — air, 137-174,
/// 174-400, 400-480, 480 and up. Tim's backup carries 0, 1, 2 and 3 exactly
/// where these boundaries put them.
fn band_code(hz: i64) -> u8 {
    match hz {
        h if h < 137_000_000 => 0,
        h if h < 174_000_000 => 1,
        h if h < 400_000_000 => 2,
        h if h < 480_000_000 => 3,
        _ => 4,
    }
}

/// The shift the radio keeps in a record that has none: the band's default
/// (600 kHz on 2 m, 1.6 MHz at 222, 5 MHz on 70 cm, nothing on airband or above
/// 480 MHz) — what Tim's simplex memories carry, so ours match the radio's own.
fn default_shift_units(hz: i64) -> u16 {
    match band_code(hz) {
        1 => 12,
        2 => 32,
        3 => 100,
        _ => 0,
    }
}

/// Byte 5's low nibble (ADMS-7 `tbl_Step_all`): 0 = 5, 1 = 6.25, 2 = 8.33,
/// 4 = 12.5 kHz. A memory is a fixed frequency, so this matters only when the
/// operator dials away from it — but it must agree with the frequency's grid.
fn step_code(hz: i64) -> u8 {
    let rem = hz % 10_000;
    if rem % 5000 == 0 {
        0
    } else if rem % 1250 != 0 {
        2 // only the airband's 8.33 kHz channels fall off the 1.25 kHz grid
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
/// The radio's cross modes (DCS ENC, TONE DCS, DCS TSQL) need Set Mode SQL
/// EXPANSION on, which nothing here controls — so Cross falls back the
/// project-wide way (`cross-tone-unsupported-fallback`): keep the TX tone,
/// drop the RX side. Both indexes are carried whatever the mode, as the
/// radio's own records do (Tim's W0TX is TSQL with a DCS code kept beside it):
/// the channel's tone, else 100.0 Hz; its DCS code, else 023.
fn encode_tone(c: &crate::models::Channel) -> (u8, u8, u8) {
    let mode = c.tone_mode.as_deref().unwrap_or("off");
    let tone = c.ctcss_uplink.or(c.ctcss_downlink).map(tone_index).unwrap_or(DEFAULT_TONE);
    let dcs = dcs_index(c.dcs_code.as_deref()).unwrap_or(0);
    let is = |m: &str| mode.eq_ignore_ascii_case(m);
    let tmode = if is("Tone") {
        TMODE_TONE
    } else if is("TSQL") {
        TMODE_TSQL
    } else if is("DTCS") {
        TMODE_DCS
    } else if is("Cross") {
        match c.cross_mode.split_once("->").map(|(tx, _)| tx) {
            Some(tx) if tx.eq_ignore_ascii_case("DTCS") => TMODE_DCS,
            Some(tx) if tx.eq_ignore_ascii_case("Tone") => TMODE_TONE,
            _ => TMODE_OFF,
        }
    } else {
        TMODE_OFF
    };
    (tmode, tone, dcs)
}

/// Byte 9's power field (ADMS-7 `tbl_SendOut`): 0 HIGH (50 W), 1 MID (20 W),
/// 2 LOW (5 W).
fn power_code(power: Option<&str>) -> u8 {
    match power {
        Some(p) if p.eq_ignore_ascii_case("Low") => 2,
        Some(p) if p.eq_ignore_ascii_case("Med") || p.eq_ignore_ascii_case("Medium") => 1,
        _ => 0,
    }
}

/// Encode one channel into its 16-byte record. `named` sets byte 11's name
/// flag, which ADMS-7 sets exactly when the memory has a tag.
///
/// A C4FM channel is programmed as FM: the FTM-400 has no per-memory digital
/// mode, and its AMS picks digital up from the signal.
pub(crate) fn encode_record(ec: &ExpandedChannel, named: bool) -> [u8; MEM_STRIDE] {
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
    r[1] = duplex | if is("AM") { AM } else { 0 } | if is("NFM") { NARROW } else { 0 };
    r[2..5].copy_from_slice(&encode_freq(rx));
    let (tmode, tone, dcs) = encode_tone(c);
    r[5] = (tmode << 4) | step_code(rx);
    r[6..9].copy_from_slice(&tx_field);
    r[9] = (power_code(c.power.as_deref()) << 6) | tone;
    r[10] = dcs; // clock shift (0x80) stays off
    r[11] = if named { NAMED } else { 0 } | PR_TONE_1500;
    r[12..14].copy_from_slice(&shift_units.to_be_bytes());
    r
}

fn char_code(ch: char) -> Option<u8> {
    match ch {
        '0' => Some(ZERO),
        ' ' => Some(SPACE),
        _ => CHARSET.find(ch).map(|i| i as u8),
    }
}

/// The tag as the radio will hold it: characters outside its set dropped,
/// at most eight, trailing spaces trimmed.
pub(crate) fn tag(name: &str) -> String {
    let s: String = name.chars().filter(|&c| char_code(c).is_some()).take(NAME_LEN).collect();
    s.trim_end().to_string()
}

/// A cleaned tag's eight stored bytes.
pub(crate) fn name_field(tag: &str) -> [u8; NAME_LEN] {
    let mut field = [NAME_FILL; NAME_LEN];
    for (slot, code) in field.iter_mut().zip(tag.chars().filter_map(char_code)) {
        *slot = code;
    }
    field
}

/// The inverse of [`name_field`], `0x00` read as a zero too (ADMS-7 writes it).
#[cfg(test)]
pub(crate) fn decode_name(field: &[u8]) -> String {
    field
        .iter()
        .take_while(|&&b| b != NAME_FILL)
        .map(|&b| match b {
            ZERO => '0',
            SPACE => ' ',
            _ => CHARSET.as_bytes().get(b as usize).map(|&c| c as char).unwrap_or('?'),
        })
        .collect()
}

fn write_slot(img: &mut [u8], bank: &Bank, slot: usize, record: &[u8; MEM_STRIDE], name: &[u8; NAME_LEN]) {
    let at = bank.mem + slot * MEM_STRIDE;
    img[at..at + MEM_STRIDE].copy_from_slice(record);
    let at = bank.name + slot * NAME_LEN;
    img[at..at + NAME_LEN].copy_from_slice(name);
}

/// Patch `template` with `channels` in memory-slot order (slot 1 first), the
/// same list into Band A and Band B.
///
/// Every one of the 500 regular slots in each band is rewritten — a filled one
/// with its channel, the rest with the radio's own empty record — so nothing
/// from the operator's old memory lists survives. The 18 PMS records of each
/// band, VFO, Home and every setting are the template's, except RX COVERAGE,
/// set WIDE on both bands when [`needs_wide`].
///
/// Overflow and frequencies the radio cannot hold as written are errors, never
/// a silent truncation or rounding.
pub(crate) fn patch_backup(
    template: &[u8],
    channels: &[&ExpandedChannel],
    model: &RadioModel,
) -> Result<Vec<u8>, String> {
    validate_backup(template)?;
    if channels.len() > MEM_SLOTS {
        return Err(format!(
            "This codeplug has {} channels; each band of the FTM-400 holds {MEM_SLOTS}. \
             Remove {} before exporting.",
            channels.len(),
            channels.len() - MEM_SLOTS
        ));
    }
    let bad: Vec<String> = channels
        .iter()
        .filter(|ec| !writable(hz(ec.channel.rx_freq)) || !writable(hz(tx_frequency(&ec.channel))))
        .map(|ec| format!("{:.5} MHz", ec.channel.rx_freq))
        .collect();
    if !bad.is_empty() {
        return Err(format!(
            "The FTM-400 cannot hold these as written — outside its 108–999.99 MHz receiver \
             (US cellular blocked), or off every channel step it has: {}. Remove them from \
             this codeplug.",
            bad.join(", ")
        ));
    }
    crate::radios::yaesu_ftm300::sd_image::refuse_inverted_dcs("FTM-400", channels)?;
    let mut img = template.to_vec();
    let names = radio_names(channels, model);
    for bank in &BANKS {
        for (slot, (ec, name)) in channels.iter().zip(&names).enumerate() {
            let tag = tag(name);
            write_slot(&mut img, bank, slot, &encode_record(ec, !tag.is_empty()), &name_field(&tag));
        }
        for slot in channels.len()..MEM_SLOTS {
            write_slot(&mut img, bank, slot, &bank.empty, &[NAME_FILL; NAME_LEN]);
        }
    }
    if needs_wide(channels) {
        set_wide(&mut img);
    }
    debug_assert!((0..BANKS[0].mem).all(|i| img[i] == template[i] || RX_COVERAGE.iter().any(|(at, _)| *at == i)));
    Ok(img)
}

fn hz(mhz: f64) -> i64 {
    (mhz * 1_000_000.0).round() as i64
}

/// The names as the radio will store them, unique AFTER cleaning: characters
/// outside the radio's set are dropped first, so two names that differed only
/// in such a character are told apart again rather than written identically.
fn radio_names(channels: &[&ExpandedChannel], model: &RadioModel) -> Vec<String> {
    let mut rows: Vec<(String, f64)> = expanded_names(channels.iter().copied(), model)
        .iter()
        .zip(channels)
        .map(|(n, ec)| (tag(n), ec.channel.rx_freq))
        .collect();
    // Unnamed channels stay unnamed — never numbered against each other.
    let named: Vec<usize> = (0..rows.len()).filter(|&i| !rows[i].0.is_empty()).collect();
    let mut sub: Vec<(String, f64)> = named.iter().map(|&i| rows[i].clone()).collect();
    crate::commands::export::disambiguate_names(&mut sub, NAME_LEN);
    for (&i, row) in named.iter().zip(sub) {
        rows[i].0 = tag(&row.0);
    }
    rows.into_iter().map(|(n, _)| n).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::Channel;

    pub(super) fn chan(rx: f64, tx: Option<f64>) -> Channel {
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

    pub(super) fn ec(c: Channel) -> ExpandedChannel {
        ExpandedChannel {
            channel: c,
            tg_label: None,
            timeslot: None,
            tg_number: None,
            tg_call_type: None,
            tg_inline: false,
        }
    }

    fn template() -> Vec<u8> {
        let mut t = vec![0x5Au8; IMAGE_LEN];
        t[..6].copy_from_slice(MAGIC);
        t
    }

    fn model() -> RadioModel {
        crate::seed::test_model("FTM-400")
    }

    #[test]
    fn frequency_encoding_matches_the_radio() {
        // Pairs read off Tim's backup, then ADMS-7's sub-10 kHz table.
        let cases: &[(i64, [u8; 3])] = &[
            (447_275_000, [0x84, 0x47, 0x27]),
            (447_700_000, [0x04, 0x47, 0x70]),
            (145_115_000, [0x81, 0x45, 0x11]),
            (118_400_000, [0x01, 0x18, 0x40]),
            (224_960_000, [0x02, 0x24, 0x96]),
            (446_106_250, [0xA4, 0x46, 0x10]),
            (446_112_500, [0x44, 0x46, 0x11]),
            (127_808_330, [0xD1, 0x27, 0x80]),
            (127_801_670, [0x31, 0x27, 0x80]),
            (127_803_330, [0x51, 0x27, 0x80]),
            (127_806_670, [0xB1, 0x27, 0x80]),
            (999_987_500, [0xC9, 0x99, 0x98]),
        ];
        for &(hz, bytes) in cases {
            assert_eq!(encode_freq(hz), bytes, "{hz}");
            assert!((decode_freq(&bytes) - hz).abs() <= 4, "{hz}");
        }
    }

    #[test]
    fn names_use_the_radios_own_zero() {
        assert_eq!(name_field("W0UPS"), [0x20, 0xBF, 0x1E, 0x19, 0x1C, 0xCA, 0xCA, 0xCA]);
        assert_eq!(name_field("FNL TOWE"), [0x0F, 0x17, 0x15, 0x64, 0x1D, 0x18, 0x20, 0x0E]);
        assert_eq!(tag("Café 146.52 long"), "Caf 146.");
        assert_eq!(decode_name(&name_field("ab-9 Z")), "ab-9 Z");
        assert_eq!(decode_name(&[0x00, 0x01, 0xCA, 0, 0, 0, 0, 0]), "01");
    }

    #[test]
    fn repeater_and_split_records() {
        // W0UPS 447.275 −5 MHz TSQL 100.0, Tim's memory A1 but for byte 11
        // (his carries PR 300 Hz, which a channel cannot say).
        let mut c = chan(447.275, Some(442.275));
        c.tone_mode = Some("TSQL".into());
        c.ctcss_uplink = Some(100.0);
        assert_eq!(
            encode_record(&ec(c), true),
            [0x83, 0x02, 0x84, 0x47, 0x27, 0x20, 0, 0, 0, 0x0C, 0, 0x8F, 0, 0x64, 0, 0]
        );
        // ISS 437.800 / 145.990, Tone 67.0 — too wide for a shift, so split.
        let mut c = chan(437.8, Some(145.99));
        c.tone_mode = Some("Tone".into());
        c.ctcss_uplink = Some(67.0);
        assert_eq!(
            encode_record(&ec(c), false),
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
        let r = encode_record(&ec(c), true);
        assert_eq!((r[1], r[5] >> 4, r[9], r[10]), (0x22, 4, 0x08, 0x0E));
        let mut c = chan(118.4, None);
        c.mode = Some("AM".into());
        c.power = Some("Low".into());
        let r = encode_record(&ec(c), true);
        assert_eq!((r[0], r[1], r[9] >> 6, r[13]), (0x80, 0x10, 2, 0));
        let mut c = chan(147.0, Some(147.6));
        c.mode = Some("YSF".into());
        c.power = Some("Med".into());
        let r = encode_record(&ec(c), true);
        assert_eq!((r[1], r[9] >> 6, r[11]), (0x03, 1, 0x8F));
    }

    #[test]
    fn rejects_other_files() {
        assert!(validate_backup(&template()).is_ok());
        assert!(validate_backup(&[0u8; 25_600]).unwrap_err().contains("MEMORY-only"));
        let mut t = template();
        t[..6].copy_from_slice(b"AH071$");
        assert!(validate_backup(&t).unwrap_err().contains("FTM-300"));
    }

    #[test]
    fn refuses_what_the_radio_cannot_hold() {
        for (rx, tx) in [(107.9, None), (880.0, None), (146.52, Some(830.0)), (1000.0, None), (146.5212, None)] {
            let e = ec(chan(rx, tx));
            let err = patch_backup(&template(), &[&e], &model()).unwrap_err();
            assert!(err.contains(&format!("{rx:.5}")), "{rx}: {err}");
        }
        for rx in [108.0, 127.80833, 823.9875, 851.0125, 999.99] {
            let e = ec(chan(rx, None));
            assert!(patch_backup(&template(), &[&e], &model()).is_ok(), "{rx}");
        }
    }

    #[test]
    fn overflow_is_an_error() {
        let e = ec(chan(146.52, None));
        let many: Vec<&ExpandedChannel> = std::iter::repeat_n(&e, MEM_SLOTS + 1).collect();
        assert!(patch_backup(&template(), &many, &model()).unwrap_err().contains("500"));
    }

    /// Only the regular memories of the two bands change — the header and
    /// its checksum, PMS, VFO, Home, settings and logs are the template's.
    #[test]
    fn patches_both_bands_and_nothing_else() {
        let t = template();
        let mut c = chan(447.275, Some(442.275));
        c.tone_mode = Some("TSQL".into());
        c.ctcss_uplink = Some(100.0);
        c.name_short = Some("W0UPS".into());
        let e = ec(c);
        let out = patch_backup(&t, &[&e], &model()).unwrap();
        for bank in &BANKS {
            assert_eq!(&out[bank.mem..bank.mem + 6], &[0x83, 0x02, 0x84, 0x47, 0x27, 0x20]);
            assert_eq!(decode_name(&out[bank.name..bank.name + 8]), "W0UPS");
            assert_eq!(&out[bank.mem + 16..bank.mem + 32], &bank.empty);
            assert_eq!(&out[bank.name + 8..bank.name + 16], &[0xCA; 8]);
        }
        let in_tables = |i: usize| {
            BANKS.iter().any(|b| {
                (b.mem..b.mem + MEM_SLOTS * MEM_STRIDE).contains(&i)
                    || (b.name..b.name + MEM_SLOTS * NAME_LEN).contains(&i)
            })
        };
        assert!((0..IMAGE_LEN).filter(|&i| !in_tables(i)).all(|i| out[i] == t[i]));
        // PMS records sit right after the 500 and are untouched.
        assert_eq!(out[BANKS[0].mem + MEM_SLOTS * MEM_STRIDE], 0x5A);
        assert_eq!(out[BANKS[1].name + MEM_SLOTS * NAME_LEN], 0x5A);
    }

    /// An air-band or 800 MHz channel puts both bands at RX COVERAGE WIDE — a
    /// band at NORMAL hides it — and touches no other bit of those bytes; an
    /// all-ham codeplug leaves the coverage exactly as the operator had it.
    #[test]
    fn out_of_ham_channels_widen_both_bands() {
        let mut air = chan(118.4, None);
        air.mode = Some("AM".into());
        let (a, b, c) = (ec(air), ec(chan(146.52, None)), ec(chan(851.0125, None)));
        let mut t = template();
        t[0xE3] = 0x5A & !0x20;
        t[0xF3] = 0x5A & !0x20;
        for list in [vec![&a, &b], vec![&b, &c]] {
            let out = patch_backup(&t, &list, &model()).unwrap();
            assert_eq!((out[0xE3], out[0xF3]), (0x7A, 0x7A));
            for bank in &BANKS {
                assert!((0..2).all(|i| out[bank.mem + i * MEM_STRIDE] & 0x80 != 0), "both bands hold both");
            }
        }
        let out = patch_backup(&t, &[&b], &model()).unwrap();
        assert_eq!((out[0xE3], out[0xF3]), (0x5A & !0x20, 0x5A & !0x20));
    }

    #[test]
    fn inverted_dcs_is_refused_by_name() {
        let mut c = chan(447.825, Some(442.825));
        c.tone_mode = Some("DTCS".into());
        c.dcs_code = Some("023".into());
        c.dcs_polarity = "NR".into();
        let e = ec(c.clone());
        let err = patch_backup(&template(), &[&e], &model()).unwrap_err();
        assert!(err.contains("447.82500"), "{err}");
        c.dcs_polarity = "NN".into();
        let e = ec(c.clone());
        assert!(patch_backup(&template(), &[&e], &model()).is_ok());
        // Cross keeps only the TX side, so an inverted RX side is not refused.
        c.tone_mode = Some("Cross".into());
        c.cross_mode = "DTCS->DTCS".into();
        c.dcs_polarity = "NR".into();
        let e = ec(c.clone());
        assert!(patch_backup(&template(), &[&e], &model()).is_ok());
        c.dcs_polarity = "RN".into();
        let e = ec(c);
        assert!(patch_backup(&template(), &[&e], &model()).is_err());
    }

    /// Names differing only in a character the radio cannot store stay distinct.
    #[test]
    fn names_stay_unique_after_cleaning() {
        let mut a = chan(146.52, None);
        a.name_short = Some("Café".into());
        let mut b = chan(446.0, None);
        b.name_short = Some("Caf".into());
        let (a, b) = (ec(a), ec(b));
        let names = radio_names(&[&a, &b], &model());
        assert_ne!(names[0], names[1], "{names:?}");
        assert!(names.iter().all(|n| n.starts_with("Caf") && n.len() <= NAME_LEN), "{names:?}");
    }

    #[test]
    fn coverage_is_read_back_from_the_memories() {
        let mut air = chan(118.4, None);
        air.mode = Some("AM".into());
        let (a, b) = (ec(air), ec(chan(146.52, None)));
        let out = patch_backup(&template(), &[&a, &b], &model()).unwrap();
        assert_eq!(memories_needing_wide(&out), 2, "the air channel, on both bands");
        let out = patch_backup(&template(), &[&b], &model()).unwrap();
        assert_eq!(memories_needing_wide(&out), 0);
        assert!(wide_note(false, 3).is_none());
        assert!(wide_note(true, 1).unwrap().contains("1 memory lies"));
    }

    #[test]
    fn frequencies_round_trip_across_the_coverage() {
        for mhz in [108.0, 136.99375, 137.0, 146.52, 173.99375, 174.0, 222.1, 399.99375, 400.0, 446.10625, 479.99375, 480.0, 823.9875, 894.0, 999.99] {
            let r = encode_record(&ec(chan(mhz, None)), false);
            assert_eq!(decode_freq(&r[2..5]), hz(mhz), "{mhz}");
            let want = [137.0, 174.0, 400.0, 480.0].iter().filter(|&&b| mhz >= b).count() as u8;
            assert_eq!(r[0] & 0x07, want, "{mhz}");
        }
    }
}
