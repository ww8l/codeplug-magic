//! The memory pool inside a `.d75` — records, names, flags and the 30 groups.
//!
//! [`d75`](super::d75) is the envelope; this is the letter. It writes a whole
//! codeplug's worth of memories into a config file the radio itself saved, which
//! the radio reads back from **Menu > Configuration > SD Card > Load Setting**.
//!
//! ## Four structures, not one
//!
//! A record carries the frequency and the tones; nothing else about a memory.
//! Its name, its group and whether it exists at all live in separate arrays, and
//! writing a record without maintaining them is the AnyTone present-bitmap bug
//! again — the radio simply does not show the memory.
//!
//! | body address | shape | what |
//! |---|---|---|
//! | `0x2000` | 1200 × 4 | flags: `used` (band code, `FF` = empty), `lockout`, `group`, `FF` |
//! | `0x4000` | 192 × (6 × 40 + 16 pad) | the records; memory *i* is block `i/6`, record `i%6` |
//! | `0x10000` | 1200 × 16 | names, NUL-padded |
//! | `0x10000 + 1152×16` | 30 × 16 | the group names, sharing the name array |
//!
//! These are **CHIRP's**, from `chirp/drivers/thd74.py` (`THD75Radio` is a bare
//! subclass of `THD74Radio` — it overrides `MODEL` and nothing else), and they
//! were verified against a real TH-D75 save before a line of this was written:
//! 91 memories decoded out of Tim's own card, tones and shifts and all, and the
//! only occupied slots above 999 are 1101–1110 and 1131–1136 — exactly CHIRP's
//! `EXTD_NUMBERS` for WX 1–10 and the six Call channels. That numbering does not
//! line up by accident. `scratchpad/thd75/FINDINGS.md` §3 has the working.
//!
//! One structural check falls out for free: `0x4000 + 192 × 256` is exactly
//! `0x10000`, so the record pool ends where the name array begins with no slack.
//! [`POOL_ENDS_AT_THE_NAMES`] holds the compiler to it.
//!
//! ## Groups are the radio's banks, and a memory belongs to exactly one
//!
//! The group is a byte of the memory's own flags, not a membership table, so a
//! channel named by two of the codeplug's lists lands in the first — the ID-52's
//! rule, for the same structural reason ([[channel-list-zone-bank-mapping]]).
//! The radio has **30** groups where the ID-52 has 100.
//!
//! ## Patch, don't generate
//!
//! Everything outside the four arrays above is the operator's radio: APRS
//! beacons, D-STAR call sign history, the repeater list, program-scan limits and
//! every MENU setting. A real file goes in, the memory arrays are overwritten,
//! and the rest comes back untouched — which
//! [`only_the_memory_arrays_change`](tests::only_the_memory_arrays_change)
//! proves against a real radio's file.

use crate::commands::export::{expanded_names, tx_frequency, CodeplugGroup, ExpandedChannel};
use crate::models::RadioModel;
use crate::radios::driver::{CodeplugExporter, ExportRequest};
use crate::util::truncate;

use super::d75::D75File;
use super::KenwoodThd75;

/// User memories, and the arrays that describe them.
///
/// `SLOTS` is what the operator can program (`000`–`999`). The arrays run past
/// it — 1152 records, 1200 flags and names — because the radio keeps its
/// program-scan limits, the WX channels and the six Call channels in the same
/// tables. This writer touches none of that.
pub(crate) const SLOTS: usize = 1000;
const FLAGS: usize = 0x2000;
const FLAG_LEN: usize = 4;
const POOL: usize = 0x4000;
const REC_LEN: usize = 40;
const RECS_PER_BLOCK: usize = 6;
/// Six records then 16 bytes of `FF` padding — Kenwood aligns the block to 256.
const BLOCK_LEN: usize = RECS_PER_BLOCK * REC_LEN + 16;
const BLOCKS: usize = 192;
const NAMES: usize = 0x10000;
const NAME_LEN: usize = 16;

/// The 30 memory groups, whose names live in the name array past the last
/// record's. Index 1182 holds a 31st, `Weather`, for the WX channels — outside
/// [`GROUPS`] and left alone.
const GROUP_NAME_BASE: usize = 1152;
pub(crate) const GROUPS: usize = 30;

/// Memory and group names are 16 characters (TH-D75 manual).
pub(crate) const MAX_NAME: usize = 16;

/// Where a channel goes when the codeplug has no channel lists to make groups
/// out of. The radio always shows *some* group, so an unnamed one would read as
/// blank on the front panel.
const DEFAULT_GROUP_NAME: &str = "Memories";

/// The record pool ends exactly where the names begin. If a future capture moves
/// either base, or a block turns out not to be 256 bytes, this stops compiling
/// rather than quietly writing records over the first names.
#[allow(dead_code)]
const POOL_ENDS_AT_THE_NAMES: () = {
    assert!(POOL + BLOCKS * BLOCK_LEN == NAMES);
    assert!(BLOCKS * RECS_PER_BLOCK == GROUP_NAME_BASE);
};

/// `used`, byte 0 of a memory's flags: **which Band-A sub-band the memory is
/// in**, or `7` for one only Band B can hear.
///
/// This radio has two receivers with different coverage, and Kenwood's own
/// specification names them:
///
/// | | coverage |
/// |---|---|
/// | Band-A RX | 136–174, 216–260, 410–470 MHz |
/// | Band-B RX | 0.1–76, 76–108 (WFM), 108–524 MHz |
///
/// Those three Band-A ranges are exactly the three codes the radio wrote across
/// 149 memories — `0` from 144 to 163 MHz, `1` from 223 to 225, `2` from 438 to
/// 468 — and its two airband memories, 118.400 and 122.800, carry **`7`**.
/// Airband is Band-B-only, so `7` is "this memory is not on Band A".
///
/// The manual is what makes that a fact rather than a correlation. Both airband
/// samples are also the only AM ones in the file, so band and mode are
/// confounded in the data; the specification is not confounded, and its band
/// edges are the ranges below to the megahertz. ★★ [[research-before-reverse-engineering]]
///
/// It also means **CHIRP's `get_used_flag` is wrong here**: its `<150 / <400 /
/// else` rule writes `0` for airband, where this radio writes `7`. Getting it
/// wrong is the ID-52's failure mode — a memory the app reports written that the
/// radio quietly leaves empty. [[radio-tx-vs-rx-bands]]
const BAND_A_VHF: u8 = 0;
const BAND_A_220: u8 = 1;
const BAND_A_UHF: u8 = 2;
const BAND_B_ONLY: u8 = 7;
/// `used` on a slot with no memory in it.
const EMPTY: u8 = 0xFF;

fn used_flag(mhz: f64) -> u8 {
    match mhz {
        f if (136.0..174.0).contains(&f) => BAND_A_VHF,
        f if (216.0..260.0).contains(&f) => BAND_A_220,
        f if (410.0..470.0).contains(&f) => BAND_A_UHF,
        _ => BAND_B_ONLY,
    }
}

/// The band code for a whole memory. An **odd-split** memory is filed under the
/// band it transmits in, not the one it listens on — CHIRP's `get_used_flag`,
/// which reads `mem.offset` (the transmit frequency) when the duplex is split.
/// Unverified on hardware, and it can only bite on a cross-band memory.
fn band_code(ec: &ExpandedChannel) -> u8 {
    let rx = used_flag(ec.channel.rx_freq);
    let tx = used_flag(tx_frequency(&ec.channel));
    if rx == tx {
        rx
    } else {
        tx
    }
}

/// Record byte 9, bits 6–4. CHIRP's `MODES`, where index 7 is the same DV the
/// radio calls **DR** (repeater mode) on its own screens.
const MODE_FM: u8 = 0;
const MODE_DV: u8 = 1;
const MODE_AM: u8 = 2;
const MODE_NFM: u8 = 6;
const MODE_DR: u8 = 7;

/// Record byte 10, from bit 7 down: which squelch the memory uses, then the
/// odd-split flag and the shift direction.
const TONE_ON: u8 = 0b1000_0000;
const CTCSS_ON: u8 = 0b0100_0000;
const DTCS_ON: u8 = 0b0010_0000;
const CROSS_ON: u8 = 0b0001_0000;
const SPLIT_ON: u8 = 0b0000_0100;
const DUPLEX_NONE: u8 = 0;
const DUPLEX_PLUS: u8 = 1;
const DUPLEX_MINUS: u8 = 2;

/// Record byte 14, bits 5–4: which way a cross-tone memory encodes and decodes.
/// CHIRP's `CROSS_MODES`; index 3 (`Tone->Tone`) is the only one the channel
/// importer produces, and Tim's radio wrote exactly that for the two repeaters
/// in his list with different uplink and downlink tones.
const CROSS_TONE_TONE: u8 = 3;

/// Record byte 14, bits 3–2 — unexplained, and reproduced rather than reasoned
/// about, the way the ID-52's `ANALOG_TAIL` is.
///
/// Constant across all 149 memories in the sample and it is not noise: every
/// analog memory carries `0b11` and every DR memory `0b10`, with `0b00` only on
/// the factory Call channels. Writing zero here would make our file the one
/// shape the radio never produces for a memory that is in use.
const TAIL_ANALOG: u8 = 0b11 << 2;
const TAIL_DR: u8 = 0b10 << 2;

/// CTCSS tones in tenths of a hertz, as the radio indexes them — CHIRP's
/// `chirp_common.TONES`. Index 12 is 100.0 Hz, which is what most of Tim's
/// repeaters carry.
const TONES_DHZ: [u16; 50] = [
    670, 693, 719, 744, 770, 797, 825, 854, 885, 915, 948, 974, 1000, 1035, 1072, 1109, 1148, 1188,
    1230, 1273, 1318, 1365, 1413, 1462, 1514, 1567, 1598, 1622, 1655, 1679, 1713, 1738, 1773, 1799,
    1835, 1862, 1899, 1928, 1966, 1995, 2035, 2065, 2107, 2181, 2257, 2291, 2336, 2418, 2503, 2541,
];

/// DTCS codes as the radio indexes them — CHIRP's `chirp_common.DTCS_CODES`,
/// written in octal the way the front panel shows them, which is also how the
/// channel database stores them ([[dcs-octal-display]]).
const DTCS_CODES: [u16; 104] = [
    23, 25, 26, 31, 32, 36, 43, 47, 51, 53, 54, 65, 71, 72, 73, 74, 114, 115, 116, 122, 125, 131,
    132, 134, 143, 145, 152, 155, 156, 162, 165, 172, 174, 205, 212, 223, 225, 226, 243, 244, 245,
    246, 251, 252, 255, 261, 263, 265, 266, 271, 274, 306, 311, 315, 325, 331, 332, 343, 346, 351,
    356, 364, 365, 371, 411, 412, 413, 423, 431, 432, 445, 446, 452, 454, 455, 462, 464, 465, 466,
    503, 506, 516, 523, 526, 532, 546, 565, 606, 612, 624, 627, 631, 632, 654, 662, 664, 703, 712,
    723, 731, 732, 734, 743, 754,
];

/// The D-STAR fields an **analog** memory carries. Not blanks: the radio fills
/// every memory's call signs whether it uses them or not, and a file that left
/// them zeroed would be a shape the radio never writes.
const UR_DEFAULT: &str = "CQCQCQ";
const RPT_DEFAULT: &str = "DIRECT";

/// Patch a codeplug into a config file the radio saved for itself.
pub(crate) fn write_codeplug(
    file: &mut D75File,
    channels: &[&ExpandedChannel],
    groups: &[CodeplugGroup],
    model: &RadioModel,
) -> Result<usize, String> {
    write_memories(file.body_mut(), channels, groups, model)
}

/// Overwrite every memory, name, flag and group name.
///
/// Wholesale, not incremental: after this returns the radio's memories are
/// exactly `channels`, and nothing that was there before survives. Slots past
/// the codeplug are cleared the way the radio clears a deleted memory — record
/// all-`FF`, name all-zero, flags `FF 00 00 FF` — rather than left holding
/// whatever the operator had, which would be a codeplug we did not write.
pub(crate) fn write_memories(
    body: &mut [u8],
    channels: &[&ExpandedChannel],
    groups: &[CodeplugGroup],
    model: &RadioModel,
) -> Result<usize, String> {
    if body.len() != super::d75::RADIO_BODY_LEN {
        return Err(format!(
            "That is not a TH-D75 config image: expected {} bytes, got {}.",
            super::d75::RADIO_BODY_LEN,
            body.len()
        ));
    }
    if channels.len() > SLOTS {
        return Err(format!(
            "This codeplug has {} channels; the TH-D75 holds {SLOTS}. Remove {} before \
             programming.",
            channels.len(),
            channels.len() - SLOTS
        ));
    }

    let (placements, group_names) = assign_groups(channels, groups)?;
    let names = expanded_names(channels.iter().copied(), model);

    for slot in 0..SLOTS {
        let flags = FLAGS + slot * FLAG_LEN;
        let name = NAMES + slot * NAME_LEN;
        let rec = record_at(slot);
        match channels.get(slot) {
            Some(ec) => {
                body[flags] = band_code(ec);
                body[flags + 1] = 0;
                body[flags + 2] = placements[slot] as u8;
                body[flags + 3] = 0xFF;
                body[rec..rec + REC_LEN].copy_from_slice(&encode_record(ec));
                ascii_field(&mut body[name..name + NAME_LEN], &names[slot]);
            }
            None => {
                body[flags] = EMPTY;
                body[flags + 1] = 0;
                body[flags + 2] = 0;
                body[flags + 3] = 0xFF;
                body[rec..rec + REC_LEN].fill(0xFF);
                body[name..name + NAME_LEN].fill(0);
            }
        }
    }

    // Group names, all 30 of them. A group the codeplug does not use goes back
    // to the radio's own default rather than keeping a name from whatever was
    // programmed before — a stale "Cycle Oregon" on an empty group is a group
    // the operator scrolls past for nothing.
    for g in 0..GROUPS {
        let at = NAMES + (GROUP_NAME_BASE + g) * NAME_LEN;
        let name = group_names
            .get(g)
            .cloned()
            .unwrap_or_else(|| format!("GRP-{g}"));
        ascii_field(&mut body[at..at + NAME_LEN], &name);
    }

    Ok(channels.len())
}

/// Byte offset of memory `slot`'s record. Kenwood stores six records per
/// 256-byte block with 16 bytes of padding after them, so this is not a flat
/// stride — CHIRP's `_get_raw_memory`, whose comment on the subject is "Why
/// Kenwood ... WHY?".
fn record_at(slot: usize) -> usize {
    POOL + (slot / RECS_PER_BLOCK) * BLOCK_LEN + (slot % RECS_PER_BLOCK) * REC_LEN
}

/// Encode one channel into its 40-byte record.
///
/// ⚠ CHIRP's `MEM_FORMAT` packs bitfields **MSB-first** within each byte, so
/// `mode:3` after a 1-bit field means bits 6–4, not bits 3–1. Reading it the
/// other way decoded every D-STAR memory in the sample as AM and inverted every
/// shift, which is a mistake that survives right up until a radio reads the file.
fn encode_record(ec: &ExpandedChannel) -> [u8; REC_LEN] {
    let c = &ec.channel;
    let mut r = [0u8; REC_LEN];

    let rx_hz = hz(c.rx_freq);
    let tx_mhz = tx_frequency(c);
    let tx_hz = hz(tx_mhz);
    // A shift can only move a memory within one band. When transmit and receive
    // land in different Band-A ranges — a cross-band pair — the radio stores the
    // transmit *frequency* in the offset field and sets the odd-split flag
    // instead (CHIRP's `duplex == 'split'`). Deciding it from the band table
    // rather than from a maximum-offset figure keeps this sourced to Kenwood's
    // published specification instead of a guessed ceiling.
    let split = used_flag(c.rx_freq) != used_flag(tx_mhz);
    let (duplex, offset_hz) = if split {
        (DUPLEX_NONE, tx_hz)
    } else {
        match tx_hz as i64 - rx_hz as i64 {
            0 => (DUPLEX_NONE, standard_offset_hz(c.rx_freq)),
            d if d > 0 => (DUPLEX_PLUS, d as u32),
            d => (DUPLEX_MINUS, (-d) as u32),
        }
    };
    r[0..4].copy_from_slice(&rx_hz.to_le_bytes());
    r[4..8].copy_from_slice(&offset_hz.to_le_bytes());

    // Byte 8 is `tuning_step:4 | split_tuning_step:3 | unknown:1`. The channel
    // database has no per-channel step and the step only affects what happens
    // when you dial *off* a memory — the stored frequency is exact either way —
    // so both stay at index 0 (5 kHz), which is what the radio wrote for all 91
    // of Tim's own memories.
    r[8] = 0;

    // Byte 9: `unknown:1 | mode:3 | narrow:1 | fine_mode:1 | fine_step:2`.
    let (mode, narrow) = mode_of(ec);
    r[9] = (mode << 4) | (u8::from(narrow) << 3);

    // Byte 10: the squelch flags, then odd-split and the shift direction.
    let (squelch, rtone, ctone, dtcs) = tone_fields(c);
    r[10] = squelch | if split { SPLIT_ON } else { 0 } | duplex;
    r[11] = rtone;
    r[12] = ctone;
    r[13] = dtcs;

    // Byte 14: `unknown:2 | cross_mode:2 | unexplained:2 | dig_squelch:2`.
    let cross = if squelch & CROSS_ON != 0 {
        CROSS_TONE_TONE << 4
    } else {
        0
    };
    r[14] = cross
        | if mode == MODE_DR {
            TAIL_DR
        } else {
            TAIL_ANALOG
        };

    // Bytes 15-38: the three call signs, 8 characters each, NUL-padded.
    let (ur, rpt1, rpt2) = call_signs(ec, mode);
    ascii_field(&mut r[15..23], &ur);
    ascii_field(&mut r[23..31], &rpt1);
    ascii_field(&mut r[31..39], &rpt2);
    // Byte 39 is `unknown:1 | dv_code:7`; zero on every memory in the sample.

    r
}

/// Mode and the narrow flag. D-STAR splits two ways on this radio: a memory with
/// a repeater to work through is **DR**, the mode the radio's own screens name,
/// and a simplex one is plain DV — which is exactly how Tim's radio stored his
/// five hotspots against its factory DV Call channels.
fn mode_of(ec: &ExpandedChannel) -> (u8, bool) {
    let c = &ec.channel;
    if is_dv(ec) {
        let simplex = hz(tx_frequency(c)) == hz(c.rx_freq);
        return (if simplex { MODE_DV } else { MODE_DR }, false);
    }
    match c.mode.as_deref().unwrap_or("FM").to_uppercase().as_str() {
        "NFM" | "FM-N" | "FMN" => (MODE_NFM, true),
        "AM" => (MODE_AM, false),
        _ => (MODE_FM, false),
    }
}

fn is_dv(ec: &ExpandedChannel) -> bool {
    ec.channel.dstar_capable
        || ec.channel.mode.as_deref().is_some_and(|m| {
            m.eq_ignore_ascii_case("dstar")
                || m.eq_ignore_ascii_case("d-star")
                || m.eq_ignore_ascii_case("dv")
        })
}

/// The squelch flag bits and the three tone bytes together, because they are one
/// decision: the flags say which of the tone bytes the radio actually reads.
///
/// The TH-D75 has real cross-tone modes, so a repeater with different uplink and
/// downlink tones is programmed as stored rather than falling back to
/// [[cross-tone-unsupported-fallback]]. Every tone byte is written whatever the
/// mode, because the radio does the same — its own records carry live tone
/// indices on memories that squelch on neither.
///
/// The placement is measured, not chosen: byte 11 takes the **uplink** tone and
/// byte 12 the downlink, except under TSQL where the radio puts the one live
/// tone in both. Tim's two `Tone` repeaters carry 100.0 and 88.5 in those two
/// bytes respectively, his TSQL ones carry the same index twice, and his one
/// cross-tone pair reads 88.5 up / 123.0 down — which is what makes re-encoding
/// his memories reproduce the radio's bytes.
fn tone_fields(c: &crate::models::Channel) -> (u8, u8, u8, u8) {
    let up = c.ctcss_uplink.map(tone_index).unwrap_or(0);
    let down = c.ctcss_downlink.map(tone_index).unwrap_or(0);
    let dtcs = dtcs_index(c.dcs_code.as_deref()).unwrap_or(0);
    match c.tone_mode.as_deref().unwrap_or("off") {
        m if m.eq_ignore_ascii_case("tone") => (TONE_ON, up, down, dtcs),
        m if m.eq_ignore_ascii_case("tsql") => {
            let t = if down != 0 { down } else { up };
            (CTCSS_ON, t, t, dtcs)
        }
        m if m.eq_ignore_ascii_case("dtcs") => (DTCS_ON, up, down, dtcs),
        m if m.eq_ignore_ascii_case("cross") => (CROSS_ON, up, down, dtcs),
        _ => (0, up, down, dtcs),
    }
}

/// The three D-STAR call sign fields.
///
/// Kenwood packs a call sign into 8 characters with the **port letter last** —
/// `WW8L   B` — the port being the repeater's band, and the gateway the same
/// call with `G`. Measured off Tim's own hotspot memories. An analog memory is
/// not blank: it carries the radio's defaults.
fn call_signs(ec: &ExpandedChannel, mode: u8) -> (String, String, String) {
    if mode != MODE_DR {
        return (
            UR_DEFAULT.to_string(),
            RPT_DEFAULT.to_string(),
            RPT_DEFAULT.to_string(),
        );
    }
    let Some(call) = ec
        .channel
        .callsign
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    else {
        // A repeater memory with no call sign has nothing to route through.
        return (
            UR_DEFAULT.to_string(),
            RPT_DEFAULT.to_string(),
            RPT_DEFAULT.to_string(),
        );
    };
    let port = match ec.channel.rx_freq {
        f if f >= 1200.0 => 'A',
        f if f >= 400.0 => 'B',
        _ => 'C',
    };
    let call: String = call.chars().take(7).collect();
    (
        UR_DEFAULT.to_string(),
        format!("{call:<7}{port}"),
        format!("{call:<7}G"),
    )
}

/// The offset a simplex memory carries. The radio stores one it is not using and
/// fills it with the band's standard shift, so a memory the operator later flips
/// to DUP on the front panel lands somewhere sensible instead of on top of
/// itself — matching that keeps a file we wrote indistinguishable from one the
/// radio saved. Airband has no repeater convention and gets a flat zero.
fn standard_offset_hz(rx_mhz: f64) -> u32 {
    if (108.0..137.0).contains(&rx_mhz) {
        return 0;
    }
    hz(crate::util::standard_offsets(rx_mhz)
        .first()
        .copied()
        .unwrap_or(0.0))
}

/// MHz as the database stores it → Hz, the integer domain the record uses.
fn hz(mhz: f64) -> u32 {
    (mhz * 1_000_000.0).round().max(0.0) as u32
}

/// Nearest entry in the CTCSS table. An exact match is the norm; the fallback
/// keeps an oddball tone from silently becoming 67.0 Hz.
fn tone_index(hz: f64) -> u8 {
    let dhz = (hz * 10.0).round() as i32;
    let mut best = 0usize;
    for (i, t) in TONES_DHZ.iter().enumerate() {
        if (i32::from(*t) - dhz).abs() < (i32::from(TONES_DHZ[best]) - dhz).abs() {
            best = i;
        }
    }
    best as u8
}

fn dtcs_index(code: Option<&str>) -> Option<u8> {
    let n: u16 = code?.trim().parse().ok()?;
    DTCS_CODES.iter().position(|&c| c == n).map(|i| i as u8)
}

/// Write an ASCII string into a fixed field, NUL-padded — the radio's own
/// padding for names and call signs alike.
fn ascii_field(dst: &mut [u8], s: &str) {
    dst.fill(0);
    for (slot, ch) in dst.iter_mut().zip(s.chars()) {
        *slot = if (0x20..0x7F).contains(&(ch as u32)) {
            ch as u8
        } else {
            b' '
        };
    }
}

/// Decide which group each memory belongs to, and name the groups.
///
/// Channels arrive in memory order and groups in codeplug order. A channel named
/// by several lists takes the **first**: the group is a byte of the memory
/// itself, so there is no way to honour more than one, and duplicating the
/// memory into a second group would spend capacity the operator never asked for.
///
/// Channels in no list at all collect in one trailing group, so a codeplug with
/// no channel lists still exports as a usable single group rather than nothing.
fn assign_groups(
    channels: &[&ExpandedChannel],
    groups: &[CodeplugGroup],
) -> Result<(Vec<usize>, Vec<String>), String> {
    use std::collections::HashMap;

    let mut first_list: HashMap<i64, usize> = HashMap::new();
    for (gi, g) in groups.iter().enumerate() {
        for c in &g.channels {
            first_list.entry(c.id).or_insert(gi);
        }
    }

    // Only lists that actually receive a memory become groups, so an empty or
    // wholly-excluded list does not burn one of the 30.
    let mut group_names: Vec<String> = Vec::new();
    let mut group_no_of_list: HashMap<usize, usize> = HashMap::new();
    let mut ungrouped: Option<usize> = None;
    let mut placements = Vec::with_capacity(channels.len());

    for ec in channels {
        let group_no = match first_list.get(&ec.channel.id) {
            Some(&li) => *group_no_of_list.entry(li).or_insert_with(|| {
                group_names.push(truncate(&groups[li].list_name, MAX_NAME));
                group_names.len() - 1
            }),
            None => *ungrouped.get_or_insert_with(|| {
                group_names.push(DEFAULT_GROUP_NAME.to_string());
                group_names.len() - 1
            }),
        };
        placements.push(group_no);
    }

    if group_names.len() > GROUPS {
        return Err(format!(
            "This codeplug fills {} memory groups; the TH-D75 has {GROUPS}. Combine or unassign \
             {} channel lists before exporting.",
            group_names.len(),
            group_names.len() - GROUPS
        ));
    }

    Ok((placements, group_names))
}

impl CodeplugExporter for KenwoodThd75 {
    fn export_format(&self) -> &'static str {
        "kenwood_thd75_sd"
    }

    /// Patch a config file the radio saved, and write the result as a **new**
    /// file beside it.
    ///
    /// The operator's own saves are never modified: a `.d75` carries their whole
    /// radio, the radio holds many of them quite happily, and overwriting the
    /// one file that reflects the radio is the mistake there is no undo for. The
    /// template is whichever save is newest, so everything the codeplug does not
    /// describe is inherited from the most recent picture of the radio.
    fn export(&self, path: &str, req: &ExportRequest) -> Result<usize, String> {
        let dir = std::path::Path::new(path);
        let template = if dir.is_file() {
            path.to_string()
        } else {
            newest_config_file(dir.parent().ok_or("There is no folder to write into.")?)?
        };
        patch_d75(&template, path, req)
    }

    /// A folder means "make me a new file in here", named the way the radio
    /// names its own so it sorts into place on the Load Setting screen.
    fn resolve_target(&self, path: &str) -> Result<String, String> {
        let dir = std::path::Path::new(path);
        if !dir.is_dir() {
            return Ok(path.to_string());
        }
        // Prove there is something to patch before naming the output: a folder
        // with no readable config file cannot produce one, and finding that out
        // now beats reporting a file name and then failing.
        newest_config_file(dir)?;
        Ok(next_config_file(dir).to_string_lossy().into_owned())
    }
}

/// Every `.d75` in a folder that this driver can actually patch, oldest first.
/// The radio's `MMDDYYYY_HHMMSS` names do **not** sort chronologically as text,
/// so this sorts by the date they encode.
fn config_files(dir: &std::path::Path) -> Vec<std::path::PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out: Vec<(String, std::path::PathBuf)> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.extension().is_some_and(|e| e.eq_ignore_ascii_case("d75"))
                && std::fs::read(p).is_ok_and(|b| D75File::parse(&b).is_ok())
        })
        .map(|p| (sort_key(&p), p))
        .collect();
    out.sort();
    out.into_iter().map(|(_, p)| p).collect()
}

/// `MMDDYYYY_HHMMSS` rearranged to `YYYYMMDD_HHMMSS`, which does sort. A name
/// that is not in the radio's format sorts first, so a hand-named file never
/// displaces a real save as the template.
fn sort_key(path: &std::path::Path) -> String {
    let stem = path.file_stem().unwrap_or_default().to_string_lossy();
    let b = stem.as_bytes();
    if b.len() == 15 && b[8] == b'_' && b.iter().enumerate().all(|(i, c)| i == 8 || c.is_ascii_digit()) {
        format!("{}{}{}", &stem[4..8], &stem[0..4], &stem[8..])
    } else {
        format!(" {stem}")
    }
}

/// The template a new file is built from: the most recent config the radio wrote
/// to this card.
fn newest_config_file(dir: &std::path::Path) -> Result<String, String> {
    config_files(dir)
        .last()
        .map(|p| p.to_string_lossy().into_owned())
        .ok_or_else(|| {
            format!(
                "No TH-D75 config file to work from in {}. On the radio, save one with \
                 Menu > Configuration > SD Card > Save Setting, then try again.",
                dir.display()
            )
        })
}

/// The radio's own naming scheme, `MMDDYYYY_HHMMSS.d75`, with the current time.
fn next_config_file(dir: &std::path::Path) -> std::path::PathBuf {
    let now = chrono::Local::now();
    for n in 0..60 {
        let stamp = (now + chrono::Duration::seconds(n))
            .format("%m%d%Y_%H%M%S")
            .to_string();
        let candidate = dir.join(format!("{stamp}.d75"));
        if !candidate.exists() {
            return candidate;
        }
    }
    dir.join(format!("{}.d75", now.format("%m%d%Y_%H%M%S")))
}

/// Patch a config file and write the result to `path`.
fn patch_d75(template: &str, path: &str, req: &ExportRequest) -> Result<usize, String> {
    let raw = std::fs::read(template).map_err(|e| {
        format!(
            "Could not read {template}: {e}. Pick a config file the radio saved for itself with \
             Menu > Configuration > SD Card > Save Setting — they live in \
             KENWOOD/TH-D75/SETTINGS/DATA/."
        )
    })?;
    let mut file = D75File::parse(&raw)?;
    let written = write_codeplug(&mut file, req.channels, req.groups, req.model)?;
    std::fs::write(path, file.to_bytes()).map_err(|e| format!("Could not write {path}: {e}"))?;
    Ok(written)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::Channel;

    /// A real radio save, for the tests that need one. `#[ignore]`d along with
    /// them: the file is a dump of a personal radio and lives under
    /// `scratchpad/`, which is gitignored.
    const REAL_SAVE: &str = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../scratchpad/thd75/card/08142026_204448.d75"
    );

    fn model() -> RadioModel {
        RadioModel {
            display_name: "Kenwood TH-D75".into(),
            max_name_length: Some(MAX_NAME as i64),
            ..RadioModel::default()
        }
    }

    fn channel(name: &str, rx: f64) -> Channel {
        Channel {
            id: 1,
            name_short: Some(name.into()),
            rx_freq: rx,
            ..Channel::default()
        }
    }

    fn expanded(c: Channel) -> ExpandedChannel {
        ExpandedChannel {
            channel: c,
            tg_label: None,
            timeslot: None,
            tg_number: None,
            tg_call_type: None,
            tg_inline: false,
        }
    }

    fn empty_body() -> Vec<u8> {
        vec![0x5A; super::super::d75::RADIO_BODY_LEN]
    }

    fn decode_freq(body: &[u8], slot: usize) -> u32 {
        let at = record_at(slot);
        u32::from_le_bytes(body[at..at + 4].try_into().unwrap())
    }

    /// The stride is the thing most likely to be got wrong, because it is not a
    /// stride: six records, then padding, then the next six. Slot 5 and slot 6
    /// are 40 and 56 bytes apart respectively, and a flat `i * 40` would put
    /// every memory past the fifth in the wrong place — silently, since the
    /// bytes still land inside the pool.
    #[test]
    fn records_skip_the_padding_between_blocks() {
        assert_eq!(record_at(0), 0x4000);
        assert_eq!(record_at(5), 0x4000 + 5 * 40);
        assert_eq!(record_at(6), 0x4000 + 256);
        assert_eq!(record_at(999), 0x4000 + 166 * 256 + 3 * 40);
        assert!(record_at(SLOTS - 1) + REC_LEN <= NAMES);
    }

    /// The band code is the flag that decides whether the radio shows a memory
    /// at all, and airband is the case CHIRP gets wrong: its rule says `0` where
    /// Tim's radio wrote `7`.
    #[test]
    fn band_code_matches_what_the_radio_wrote() {
        assert_eq!(used_flag(144.390), BAND_A_VHF);
        assert_eq!(used_flag(163.275), BAND_A_VHF);
        assert_eq!(used_flag(224.520), BAND_A_220);
        assert_eq!(used_flag(438.450), BAND_A_UHF);
        assert_eq!(used_flag(467.6125), BAND_A_UHF);
        // Band-B-only, and the case CHIRP's rule gets wrong: it says 0.
        assert_eq!(used_flag(118.400), BAND_B_ONLY);
        assert_eq!(used_flag(122.800), BAND_B_ONLY);
        // Kenwood's published Band-A edges, to the megahertz.
        assert_eq!(used_flag(135.999), BAND_B_ONLY);
        assert_eq!(used_flag(136.0), BAND_A_VHF);
        assert_eq!(used_flag(174.0), BAND_B_ONLY);
        assert_eq!(used_flag(216.0), BAND_A_220);
        assert_eq!(used_flag(260.0), BAND_B_ONLY);
        assert_eq!(used_flag(410.0), BAND_A_UHF);
        assert_eq!(used_flag(470.0), BAND_B_ONLY);
        // Band B alone reaches 6 m and the FM broadcast band.
        assert_eq!(used_flag(52.525), BAND_B_ONLY);
        assert_eq!(used_flag(98.5), BAND_B_ONLY);
    }

    /// A cross-band pair is not a shift, and encoding it as one would put a
    /// 295 MHz "offset" in a field that means something else. The radio's own
    /// answer is odd split: the transmit frequency goes in the offset field and
    /// the shift direction goes to none.
    #[test]
    fn cross_band_memories_become_an_odd_split() {
        let mut c = channel("CROSSBAND", 145.000);
        c.tx_freq = Some(440.000);
        let ec = expanded(c);
        let r = encode_record(&ec);
        assert_eq!(r[10] & SPLIT_ON, SPLIT_ON);
        assert_eq!(r[10] & 0b11, DUPLEX_NONE);
        assert_eq!(u32::from_le_bytes(r[4..8].try_into().unwrap()), 440_000_000);
        // Filed under the band it transmits in.
        assert_eq!(band_code(&ec), BAND_A_UHF);

        // An ordinary repeater is not a split, however far the shift.
        let mut c = channel("W0UPS", 449.600);
        c.duplex = Some("-".into());
        c.offset = Some(5.0);
        let ec = expanded(c);
        assert_eq!(encode_record(&ec)[10] & SPLIT_ON, 0);
        assert_eq!(band_code(&ec), BAND_A_UHF);
    }

    /// A memory is four separate writes, and the record alone is not enough:
    /// the flags say it exists, the name array names it, and the group byte puts
    /// it somewhere the operator can find it.
    #[test]
    fn writes_the_record_flags_name_and_group_together() {
        let mut body = empty_body();
        let ec = expanded(channel("W0UPS", 146.850));
        let n = write_memories(&mut body, &[&ec], &[], &model()).expect("write");
        assert_eq!(n, 1);

        assert_eq!(body[FLAGS], BAND_A_VHF);
        assert_eq!(body[FLAGS + 1], 0);
        assert_eq!(body[FLAGS + 2], 0);
        assert_eq!(body[FLAGS + 3], 0xFF);
        assert_eq!(decode_freq(&body, 0), 146_850_000);
        assert_eq!(&body[NAMES..NAMES + 5], b"W0UPS");
        // The second slot is cleared the way the radio clears one.
        assert_eq!(body[FLAGS + FLAG_LEN], EMPTY);
        assert_eq!(&body[record_at(1)..record_at(1) + 4], &[0xFF; 4]);
        assert_eq!(&body[NAMES + NAME_LEN..NAMES + NAME_LEN + 4], &[0; 4]);
        // An unused group keeps the radio's own default name.
        let g0 = NAMES + GROUP_NAME_BASE * NAME_LEN;
        assert_eq!(&body[g0..g0 + 9], b"Memories\0");
        let g1 = g0 + NAME_LEN;
        assert_eq!(&body[g1..g1 + 6], b"GRP-1\0");
    }

    /// A repeater's shift is stored as a direction plus a magnitude, and the 220
    /// band is the one where the sign is not the obvious one — Tim's 224.520 is
    /// a *minus* 1.6 MHz repeater. Getting the direction wrong transmits on the
    /// wrong frequency, which is the failure a radio cannot warn about.
    #[test]
    fn encodes_shift_direction_and_magnitude() {
        let mut c = channel("W0UPS 220", 224.520);
        c.duplex = Some("-".into());
        c.offset = Some(1.6);
        let r = encode_record(&expanded(c));
        assert_eq!(r[10] & 0b11, DUPLEX_MINUS);
        assert_eq!(u32::from_le_bytes(r[4..8].try_into().unwrap()), 1_600_000);

        let mut c = channel("W0LRA", 147.195);
        c.duplex = Some("+".into());
        c.offset = Some(0.6);
        let r = encode_record(&expanded(c));
        assert_eq!(r[10] & 0b11, DUPLEX_PLUS);
        assert_eq!(u32::from_le_bytes(r[4..8].try_into().unwrap()), 600_000);
    }

    /// Cross tone is the case a lot of radios in this project cannot do, and
    /// this one can: a repeater with 88.5 up and 123.0 down keeps both.
    #[test]
    fn keeps_both_tones_on_a_cross_memory() {
        let mut c = channel("KB0VJJ", 145.310);
        c.tone_mode = Some("Cross".into());
        c.cross_mode = "Tone->Tone".into();
        c.ctcss_uplink = Some(88.5);
        c.ctcss_downlink = Some(123.0);
        let r = encode_record(&expanded(c));
        assert_eq!(r[10] & CROSS_ON, CROSS_ON);
        assert_eq!(r[11], 8);
        assert_eq!(r[12], 18);
        assert_eq!((r[14] >> 4) & 0b11, CROSS_TONE_TONE);
        assert_eq!(r[14] & 0b1100, TAIL_ANALOG);
    }

    /// An analog memory is not blank in the D-STAR fields: the radio fills them,
    /// and a file that zeroed them would be a shape the radio never writes.
    #[test]
    fn analog_memories_carry_the_radios_own_call_sign_defaults() {
        let r = encode_record(&expanded(channel("SIMPLEX", 146.520)));
        assert_eq!(&r[15..21], b"CQCQCQ");
        assert_eq!(&r[23..29], b"DIRECT");
        assert_eq!(&r[31..37], b"DIRECT");
        assert_eq!(r[39], 0);
    }

    /// D-STAR splits two ways, and the repeater form is the one that needs the
    /// port letter in the eighth character.
    #[test]
    fn dstar_repeaters_are_dr_and_simplex_is_dv() {
        let mut c = channel("CSU DSTAR", 446.8125);
        c.mode = Some("DSTAR".into());
        c.callsign = Some("WW8L".into());
        c.duplex = Some("-".into());
        c.offset = Some(5.0);
        let r = encode_record(&expanded(c));
        assert_eq!((r[9] >> 4) & 0b111, MODE_DR);
        assert_eq!(&r[23..31], b"WW8L   B");
        assert_eq!(&r[31..39], b"WW8L   G");
        assert_eq!(r[14] & 0b1100, TAIL_DR);

        let mut c = channel("DV SIMPLEX", 145.670);
        c.mode = Some("DSTAR".into());
        let r = encode_record(&expanded(c));
        assert_eq!((r[9] >> 4) & 0b111, MODE_DV);
        assert_eq!(&r[23..29], b"DIRECT");
    }

    /// Overflowing the radio is an error, not a silent truncation: a codeplug
    /// that does not fit is the operator's to fix, and quietly dropping the tail
    /// hands back a radio that looks programmed and is missing channels.
    #[test]
    fn refuses_more_than_the_radio_holds() {
        let mut body = empty_body();
        let ecs: Vec<ExpandedChannel> = (0..SLOTS + 1)
            .map(|i| expanded(channel("CH", 146.0 + i as f64 / 1000.0)))
            .collect();
        let refs: Vec<&ExpandedChannel> = ecs.iter().collect();
        let err = write_memories(&mut body, &refs, &[], &model()).unwrap_err();
        assert!(err.contains("1000"), "{err}");

        let err = write_memories(&mut [0u8; 16], &[], &[], &model()).unwrap_err();
        assert!(err.contains("config image"), "{err}");
    }

    /// The radio's own file names sort by month, not by year, so plain
    /// alphabetical order would call a January save newer than a December one.
    #[test]
    fn config_files_sort_by_the_date_their_names_encode() {
        assert!(sort_key(std::path::Path::new("12312025_235959.d75"))
            < sort_key(std::path::Path::new("01012026_000000.d75")));
        assert!(sort_key(std::path::Path::new("template.d75"))
            < sort_key(std::path::Path::new("01012026_000000.d75")));
    }

    /// Rebuild the channel the driver would have been handed for a record the
    /// radio wrote. Test-only, and deliberately naive — it exists to feed
    /// [`encode_record`] its own output back, so anything it gets wrong shows up
    /// as a mismatch rather than being hidden.
    fn channel_from_record(m: &[u8], name: &str) -> Channel {
        let freq = u32::from_le_bytes(m[0..4].try_into().unwrap()) as f64 / 1e6;
        let off = u32::from_le_bytes(m[4..8].try_into().unwrap()) as f64 / 1e6;
        let mode = (m[9] >> 4) & 0b111;
        let flags = m[10];
        let (duplex, offset) = if flags & SPLIT_ON != 0 {
            (None, None)
        } else {
            match flags & 0b11 {
                DUPLEX_PLUS => (Some("+".to_string()), Some(off)),
                DUPLEX_MINUS => (Some("-".to_string()), Some(off)),
                _ => (None, None),
            }
        };
        let tone = |i: u8| f64::from(TONES_DHZ[i as usize]) / 10.0;
        let tone_mode = match flags {
            f if f & TONE_ON != 0 => "Tone",
            f if f & CTCSS_ON != 0 => "TSQL",
            f if f & DTCS_ON != 0 => "DTCS",
            f if f & CROSS_ON != 0 => "Cross",
            _ => "off",
        };
        let rpt1 = String::from_utf8_lossy(&m[23..31])
            .trim_end_matches('\0')
            .to_string();
        Channel {
            id: 1,
            name_short: Some(name.to_string()),
            rx_freq: freq,
            tx_freq: if flags & SPLIT_ON != 0 { Some(off) } else { None },
            duplex,
            offset,
            mode: Some(
                match mode {
                    MODE_AM => "AM",
                    MODE_NFM => "NFM",
                    MODE_DV | MODE_DR => "DSTAR",
                    _ => "FM",
                }
                .into(),
            ),
            tone_mode: Some(tone_mode.into()),
            ctcss_uplink: Some(tone(m[11])),
            ctcss_downlink: Some(tone(m[12] & 0x3F)),
            dcs_code: Some(format!("{:03}", DTCS_CODES[(m[13] & 0x7F) as usize])),
            cross_mode: "Tone->Tone".into(),
            callsign: if mode == MODE_DR {
                Some(rpt1.chars().take(7).collect::<String>().trim_end().into())
            } else {
                None
            },
            ..Channel::default()
        }
    }

    /// ★★ The decisive one, borrowed from the ID-52: decode the radio's own
    /// memories and re-encode them with the production encoder, and the bytes
    /// have to come back the same.
    ///
    /// This is what separates "the layout parses" from "the writer is right".
    /// Every field the radio actually reads is exercised by real data — three
    /// squelch modes, a genuine cross-tone pair, D-STAR repeaters with call
    /// signs, GMRS, airband, all three band codes and both shift directions —
    /// and a rule invented at a desk fails it immediately. It is what caught the
    /// tone placement: byte 11 is the uplink tone and byte 12 the downlink,
    /// where a first draft wrote the uplink into both.
    ///
    /// Three allowances, each of them a field the channel database does not
    /// carry rather than a disagreement about encoding:
    ///
    /// - **byte 8**, the tuning step, and **byte 9's low three bits**, fine mode
    ///   and fine step — no per-channel step exists to write.
    /// - **the offset on a simplex memory**, which the radio fills with a band
    ///   default it is not using. 12 of Tim's 14 simplex memories match ours
    ///   anyway; the two that do not are GMRS channels carrying a leftover from
    ///   whatever they were edited from.
    #[test]
    #[ignore = "needs a real .d75 under scratchpad/thd75/card/"]
    fn re_encoding_the_radios_own_memories_reproduces_its_bytes() {
        let raw = std::fs::read(REAL_SAVE).expect("real save");
        let body = D75File::parse(&raw).expect("parse").body().to_vec();

        let (mut exact, mut allowed_only, mut wrong) = (0, 0, Vec::new());
        for slot in 0..SLOTS {
            if body[FLAGS + slot * FLAG_LEN] == EMPTY {
                continue;
            }
            let at = record_at(slot);
            let m = &body[at..at + REC_LEN];
            let name_at = NAMES + slot * NAME_LEN;
            let name = String::from_utf8_lossy(&body[name_at..name_at + NAME_LEN])
                .trim_end_matches('\0')
                .to_string();
            let ec = expanded(channel_from_record(m, &name));
            let ours = encode_record(&ec);
            let simplex = m[10] & 0b11 == DUPLEX_NONE && m[10] & SPLIT_ON == 0;

            let diffs: Vec<usize> = (0..REC_LEN)
                .filter(|&i| m[i] != ours[i])
                .filter(|&i| i != 8)
                .filter(|&i| !(i == 9 && m[9] & 0xF8 == ours[9] & 0xF8))
                .filter(|&i| !(simplex && (4..8).contains(&i)))
                .collect();
            if !diffs.is_empty() {
                wrong.push(format!(
                    "slot {slot} ({:.4} MHz) differs at {diffs:?}\n  radio {m:02X?}\n  ours  {ours:02X?}",
                    ec.channel.rx_freq
                ));
            } else if m[..] == ours[..] {
                exact += 1;
            } else {
                allowed_only += 1;
            }

            assert_eq!(
                body[FLAGS + slot * FLAG_LEN],
                band_code(&ec),
                "slot {slot} band code, {:.4} MHz",
                ec.channel.rx_freq
            );
        }

        assert!(
            wrong.is_empty(),
            "{} of the radio's own memories do not re-encode:\n{}",
            wrong.len(),
            wrong.join("\n")
        );
        assert!(
            exact + allowed_only >= 90,
            "only {} memories checked — is the fixture the right file?",
            exact + allowed_only
        );
        println!("{exact} exact, {allowed_only} differing only in fields the database has no value for");
    }

    /// ★ The one that matters, and it needs a real radio save: everything
    /// outside the four memory arrays has to come back byte-identical. That is
    /// the whole patch-don't-generate bargain — the file carries APRS beacons,
    /// call sign history, the repeater list and every MENU setting, none of
    /// which this codeplug describes.
    #[test]
    #[ignore = "needs a real .d75 under scratchpad/thd75/card/"]
    fn only_the_memory_arrays_change() {
        let raw = std::fs::read(REAL_SAVE).expect("real save");
        let before = D75File::parse(&raw).expect("parse");
        let mut after = before.clone();
        let ec = expanded(channel("W0UPS", 146.850));
        write_codeplug(&mut after, &[&ec], &[], &model()).expect("write");

        // Exactly the bytes this writer claims, slot by slot — not "the pool",
        // which would hide the worst version of getting the stride wrong:
        // clobbering the WX and Call channels that share these same arrays
        // above slot 999.
        let mut claimed = vec![false; before.body().len()];
        for slot in 0..SLOTS {
            for i in 0..FLAG_LEN {
                claimed[FLAGS + slot * FLAG_LEN + i] = true;
            }
            for i in 0..REC_LEN {
                claimed[record_at(slot) + i] = true;
            }
            for i in 0..NAME_LEN {
                claimed[NAMES + slot * NAME_LEN + i] = true;
            }
        }
        for g in 0..GROUPS {
            for i in 0..NAME_LEN {
                claimed[NAMES + (GROUP_NAME_BASE + g) * NAME_LEN + i] = true;
            }
        }

        let (a, b) = (before.body(), after.body());
        let touched = |i: usize| claimed[i];
        let strays: Vec<usize> = (0..a.len()).filter(|&i| a[i] != b[i] && !touched(i)).collect();
        assert!(
            strays.is_empty(),
            "{} bytes changed outside the memory arrays, first at {:#X}",
            strays.len(),
            strays.first().copied().unwrap_or(0)
        );
    }
}
