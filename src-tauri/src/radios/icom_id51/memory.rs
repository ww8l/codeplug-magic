//! Memories, flags and banks inside an ID-51 Plus `.icf` image.
//!
//! The 49-byte record is the ID-31 lineage's, field for field the ID-5100's —
//! measured here against Tim's own file: TONE 1 / TSQL 3 / DTCS 5, FM 0 / FM-N 1
//! / AM 3 / DV 5, byte `+0x09` = `E4` in all 87 memories, and the bank byte's
//! bits 5-6 equal to the record's multiplier in all 87. So the codec is the
//! ID-5100's [`Record`], reused rather than copied; what differs is decided
//! here.
//!
//! ## What differs from the ID-5100
//!
//! - **Which multiplier** is the ID-5100's rule: 5 kHz units first, 6.25 kHz
//!   only for a frequency 5 kHz cannot reach. That is what the two Icom sources
//!   write — the radio's own front panel (memories 58 and 82 in Tim's file,
//!   448.400 and 438.900, both 6.25-divisible and both stored in 5 kHz units)
//!   and CS-51PLUS, which re-encodes every memory that way on save. 49 of Tim's
//!   87 memories are stored in 6.25 kHz units instead; they came from some other
//!   tool, and decode to the same frequencies either way.
//! - **No 8.33 kHz multiplier.** The ID-5100 stores airband on an 8.33 kHz grid
//!   (`mult` 2). Nothing on the ID-51 has been seen to, and CHIRP's ID-51 code
//!   only knows 5 and 6.25 kHz, so a channel on neither grid is refused with a
//!   reason rather than written with an unmeasured encoding.
//! - **The tuning-step table.** Index 2 is 8.33 kHz on this radio (measured in
//!   CS-51PLUS, s002); the ID-5100 stores airband as 14. Every memory written
//!   here takes index 0 (5 kHz), which is what the radio stored for every memory
//!   it has, airband included.
//! - **The layout**: 500 memories, 26 banks, different table offsets
//!   ([`super::layout`]).

use std::collections::HashMap;

use crate::commands::export::{expanded_names, CodeplugGroup, ExpandedChannel};
use crate::models::RadioModel;
use crate::radios::icom_id52::memory::{ascii_field, dtcs_index, hz, pack_call, tone_index};
use crate::radios::icom_id52::memory_csv::{call_signs, duplex_and_offset, mode_of, tone_columns};
pub(crate) use crate::radios::icom_id5100::memory::Record;

use super::layout::{
    record_offset, BANK_CAPACITY, BANK_COUNT, BANK_NAMES, BANK_NAME_LEN, BANK_TABLE,
    CHANNEL_COUNT, EMPTY_BITMAP, NAME_LEN, NO_BANK, PSKIP_BITMAP, REC_LEN, RX_COVERAGE,
    SKIP_BITMAP,
};

/// Multipliers in Hz, indexed by the stored `mult` value. Only 0 and 1 are
/// written; 2 is the ID-5100's 8.33 kHz grid, kept so a record carrying it still
/// decodes to the right frequency.
const MULT_HZ: [f64; 3] = [5_000.0, 6_250.0, 25_000.0 / 3.0];

const SQL_OFF: u8 = 0;
const SQL_TONE: u8 = 1;
const SQL_TSQL: u8 = 3;
const SQL_DTCS: u8 = 5;

const MODE_FM: u8 = 0;
const MODE_FM_N: u8 = 1;
const MODE_AM: u8 = 3;
const MODE_DV: u8 = 5;

/// Tuning step index 0 = 5 kHz: what the radio stored for every memory in
/// Tim's file except two the operator set to 25 kHz by hand.
const STEP_DEFAULT: u8 = 0;

/// Byte `+0x09`, `E4` in every one of the radio's 87 memories.
const BYTE9: u8 = 0xE4;

/// Which multiplier a frequency is stored in, and how many of them: 5 kHz
/// when it divides evenly, else 6.25 kHz. `None` when it is on neither grid.
fn multiplier_for(freq_hz: u32) -> Option<(u8, u32)> {
    [0u8, 1].iter().find_map(|&m| {
        let step = MULT_HZ[usize::from(m)];
        let units = f64::from(freq_hz) / step;
        (units.fract() == 0.0 && units < f64::from(1u32 << 18)).then_some((m, units as u32))
    })
}

/// Whether a memory can hold `rx_mhz` at all — the driver's
/// `unstorable_frequency`, so `channel_fit` excludes what [`encode_channel`]
/// would refuse.
pub(crate) fn storable(rx_mhz: f64) -> Option<(u8, u32)> {
    multiplier_for(hz(rx_mhz))
}

/// Build a record for an app channel. Errors rather than approximates when a
/// frequency or offset is on none of the radio's grids.
pub(crate) fn encode_channel(ec: &ExpandedChannel, name: &str) -> Result<Record, String> {
    let c = &ec.channel;
    let rx = hz(c.rx_freq);
    let (dup, offset_mhz) = duplex_and_offset(ec);
    let offset_hz = hz(offset_mhz);
    let (mult, freq_units) = multiplier_for(rx).ok_or_else(|| {
        format!(
            "{:.5} MHz is not on the radio's 5 or 6.25 kHz channel grid",
            c.rx_freq
        )
    })?;
    let step = MULT_HZ[usize::from(mult)];
    let offset_units = f64::from(offset_hz) / step;
    if offset_units.fract() != 0.0 || offset_units > f64::from(u16::MAX) {
        return Err(format!(
            "a {offset_mhz:.4} MHz offset is not on the radio's {} kHz grid",
            step / 1000.0
        ));
    }
    let duplex = match dup {
        "DUP-" => 1,
        "DUP+" => 2,
        _ => 0,
    };
    let mode = match mode_of(ec) {
        "FM-N" => MODE_FM_N,
        "AM" => MODE_AM,
        "DV" => MODE_DV,
        // WFM and anything else a memory cannot hold is FM, as on the ID-52 and
        // ID-5100.
        _ => MODE_FM,
    };

    let mut name_bytes = [b' '; NAME_LEN];
    ascii_field(&mut name_bytes, name);

    let mut rec = Record {
        mult1: mult,
        mult2: mult,
        freq_units,
        offset_units: offset_units as u16,
        rtone: tone_index(88.5),
        ctone: tone_index(88.5),
        unknown1: 0,
        mode,
        dtcs: 0,
        step: STEP_DEFAULT,
        unknown2: 0,
        byte9: BYTE9,
        squelch: SQL_OFF,
        duplex,
        polarity: 0,
        name: name_bytes,
        dv_code: 0,
        // Every analog memory on Tim's radio carries `CQCQCQ` and blank repeaters.
        ur: pack_call("CQCQCQ"),
        rpt1: pack_call(""),
        rpt2: pack_call(""),
    };

    if mode == MODE_DV {
        let (ur, rpt1, rpt2) = call_signs(ec, true);
        rec.ur = pack_call(&ur);
        rec.rpt1 = pack_call(&rpt1);
        rec.rpt2 = pack_call(&rpt2);
        // A DV memory does not read its tone fields. They keep the 88.5 Hz
        // default here; the radio's own DV memories carry 94.8 (Tim's 0-2).
        return Ok(rec);
    }

    let t = tone_columns(ec);
    rec.squelch = match t.tone {
        "TONE" => SQL_TONE,
        "TSQL" => SQL_TSQL,
        "DTCS" | "DTCS(T)" | "DTCS(T)/TSQL(R)" => SQL_DTCS,
        "OFF" => SQL_OFF,
        // The cross types are not measured on this radio: keep the TRANSMIT side
        // right, which is what keys the repeater (the ID-5100's choice).
        _ => SQL_TONE,
    };
    rec.rtone = tone_index(t.rpt_hz);
    rec.ctone = tone_index(t.tsql_hz);
    rec.dtcs = dtcs_index(t.dtcs.as_deref().or(c.dcs_code.as_deref()));
    // CHIRP's polarity order (NN, NR, RN, RR). Only NN = 0 is measured.
    rec.polarity = match c.dcs_polarity.to_uppercase().as_str() {
        "NR" => 1,
        "RN" => 2,
        "RR" => 3,
        _ => 0,
    };
    Ok(rec)
}

// ============================================================
// Image-level: flags and banks
// ============================================================

#[cfg(test)]
fn bit(image: &[u8], base: usize, slot: usize) -> bool {
    image[base + slot / 8] & (1 << (slot % 8)) != 0
}

fn set_bit(image: &mut [u8], base: usize, slot: usize, on: bool) {
    let mask = 1u8 << (slot % 8);
    if on {
        image[base + slot / 8] |= mask;
    } else {
        image[base + slot / 8] &= !mask;
    }
}

/// Whether memory `slot` holds a channel. The empty bitmap is what the radio
/// reads; a cleared slot keeps its old record bytes.
#[cfg(test)]
pub(crate) fn is_used(image: &[u8], slot: usize) -> bool {
    !bit(image, EMPTY_BITMAP, slot)
}

#[cfg(test)]
pub(crate) fn read_record(image: &[u8], slot: usize) -> Record {
    let at = record_offset(slot);
    Record::decode(&image[at..at + REC_LEN])
}

#[cfg(test)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ScanSkip {
    None,
    Memory,
    Program,
}

/// `Skip` or `PSkip`, as CS-51PLUS shows them. A PSkip memory has both bits set
/// on this radio (memories 15/19/20/22/24 in Tim's file).
#[cfg(test)]
pub(crate) fn read_skip(image: &[u8], slot: usize) -> ScanSkip {
    if bit(image, PSKIP_BITMAP, slot) {
        ScanSkip::Program
    } else if bit(image, SKIP_BITMAP, slot) {
        ScanSkip::Memory
    } else {
        ScanSkip::None
    }
}

/// `(bank, index)` of a memory, or `None` when it is in no bank.
#[cfg(test)]
pub(crate) fn read_bank(image: &[u8], slot: usize) -> Option<(u8, u8)> {
    let at = BANK_TABLE + slot * 2;
    let bank = image[at] & 0x1F;
    (bank != NO_BANK && usize::from(bank) < BANK_COUNT).then_some((bank, image[at + 1]))
}

/// Store a channel in `slot`: record, empty/skip bits, and its bank entry.
///
/// The bank byte is `[mult: bits 5-6][bank: bits 0-4]` — the multiplier bits
/// match the record in all 87 of Tim's memories, and CS-51PLUS clears them when
/// it re-encodes a record to 5 kHz. CHIRP writes the bare bank number.
pub(crate) fn store(image: &mut [u8], slot: usize, rec: &Record, bank: Option<(u8, u8)>) {
    assert!(slot < CHANNEL_COUNT);
    let at = record_offset(slot);
    image[at..at + REC_LEN].copy_from_slice(&rec.encode());
    set_bit(image, EMPTY_BITMAP, slot, false);
    set_bit(image, SKIP_BITMAP, slot, false);
    set_bit(image, PSKIP_BITMAP, slot, false);
    let (b, idx) = match bank {
        Some((b, idx)) => {
            assert!(usize::from(b) < BANK_COUNT && usize::from(idx) < BANK_CAPACITY);
            (b, idx)
        }
        None => (NO_BANK, 0),
    };
    let entry = BANK_TABLE + slot * 2;
    image[entry] = ((rec.mult1 & 3) << 5) | b;
    image[entry + 1] = idx;
}

/// Empty `slot` the way the radio does: empty bit set, skips cleared, `1F 00`
/// bank entry (what all 413 empty slots in Tim's file hold). The record bytes
/// stay, as the radio leaves them.
pub(crate) fn clear(image: &mut [u8], slot: usize) {
    set_bit(image, EMPTY_BITMAP, slot, true);
    set_bit(image, SKIP_BITMAP, slot, false);
    set_bit(image, PSKIP_BITMAP, slot, false);
    let entry = BANK_TABLE + slot * 2;
    image[entry] = NO_BANK;
    image[entry + 1] = 0;
}

#[cfg(test)]
pub(crate) fn bank_name(image: &[u8], bank: usize) -> String {
    let at = BANK_NAMES + bank * BANK_NAME_LEN;
    String::from_utf8_lossy(&image[at..at + BANK_NAME_LEN]).trim_end().to_string()
}

pub(crate) fn set_bank_name(image: &mut [u8], bank: usize, name: &str) {
    let at = BANK_NAMES + bank * BANK_NAME_LEN;
    ascii_field(&mut image[at..at + BANK_NAME_LEN], name);
}

// ============================================================
// The codeplug write
// ============================================================

/// Overwrite the memory area — records, the three bitmaps, the bank table and
/// all 26 bank names — with `channels`, in order, from memory 0.
///
/// Lists become banks A, B, C … in codeplug order, only for lists that received
/// a memory. A channel in two lists is in the first one's bank (a memory carries
/// one bank byte). More than 26 such lists, or more than 100 channels in one, is
/// REFUSED with a reason, as the ID-52 does: a card export has no way to show a
/// warning, and a channel silently left out of its bank is the failure a
/// warning nobody reads produces. Nothing outside those tables is
/// touched, so the call channels, scan edges, repeater list, call signs, GPS
/// memories and every MENU setting stay exactly as the radio wrote them.
///
/// Every channel is encoded before the image is touched, so one bad channel
/// leaves nothing half-written.
pub(crate) fn write_memories(
    image: &mut [u8],
    channels: &[&ExpandedChannel],
    groups: &[CodeplugGroup],
    model: &RadioModel,
) -> Result<usize, String> {
    if image.len() != super::layout::IMAGE_LEN {
        return Err(format!(
            "That is not an ID-51 Plus settings image: expected {} bytes, got {}.",
            super::layout::IMAGE_LEN,
            image.len()
        ));
    }
    let capacity = model
        .memory_channels
        .map(|n| (n as usize).min(CHANNEL_COUNT))
        .unwrap_or(CHANNEL_COUNT);
    if channels.len() > capacity {
        return Err(format!(
            "This codeplug has {} channels; the ID-51 holds {capacity}. Remove {} before \
             programming.",
            channels.len(),
            channels.len() - capacity
        ));
    }

    let names = expanded_names(channels.iter().copied(), model);
    let mut records = Vec::with_capacity(channels.len());
    for (ec, name) in channels.iter().zip(&names) {
        let f = ec.channel.rx_freq;
        if !RX_COVERAGE.iter().any(|&(lo, hi)| f >= lo && f <= hi) {
            return Err(format!(
                "Channel \"{name}\" ({f:.4} MHz) is outside the ID-51's receive coverage \
                 (108–174 and 380–479 MHz). Nothing was written."
            ));
        }
        records.push(encode_channel(ec, name).map_err(|e| {
            format!("Channel \"{name}\" ({f:.4} MHz) cannot be programmed: {e}")
        })?);
    }

    // Channel id -> the first list that names it.
    let mut first_list: HashMap<i64, usize> = HashMap::new();
    for (gi, g) in groups.iter().enumerate() {
        for c in &g.channels {
            first_list.entry(c.id).or_insert(gi);
        }
    }
    let mut bank_names: Vec<String> = Vec::new();
    let mut letter_of_list: HashMap<usize, u8> = HashMap::new();
    let mut counts = [0usize; BANK_COUNT];
    let mut bank_of: Vec<Option<(u8, u8)>> = vec![None; channels.len()];
    for (slot, ec) in channels.iter().enumerate() {
        let Some(&li) = first_list.get(&ec.channel.id) else {
            continue;
        };
        let letter = match letter_of_list.get(&li) {
            Some(&l) => l,
            None => {
                if bank_names.len() == BANK_COUNT {
                    return Err(format!(
                        "This codeplug's channels come from more than {BANK_COUNT} channel lists, \
                         and the ID-51 has {BANK_COUNT} banks (A-Z). Combine or unassign lists \
                         — \"{}\" is the first that does not fit.",
                        groups[li].list_name
                    ));
                }
                bank_names.push(groups[li].list_name.clone());
                let l = (bank_names.len() - 1) as u8;
                letter_of_list.insert(li, l);
                l
            }
        };
        let idx = counts[usize::from(letter)];
        if idx == BANK_CAPACITY {
            return Err(format!(
                "List \"{}\" has more than {BANK_CAPACITY} channels, and an ID-51 bank holds \
                 {BANK_CAPACITY}. Split it into two lists.",
                groups[li].list_name
            ));
        }
        counts[usize::from(letter)] += 1;
        bank_of[slot] = Some((letter, idx as u8));
    }

    for (slot, rec) in records.iter().enumerate() {
        store(image, slot, rec, bank_of[slot]);
    }
    for slot in channels.len()..CHANNEL_COUNT {
        clear(image, slot);
    }
    for bank in 0..BANK_COUNT {
        set_bank_name(image, bank, bank_names.get(bank).map(String::as_str).unwrap_or(""));
    }
    Ok(records.len())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::Channel;

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

    fn ch(id: i64, rx: f64) -> Channel {
        Channel {
            id,
            rx_freq: rx,
            mode: Some("FM".into()),
            dcs_polarity: "NN".into(),
            ..Default::default()
        }
    }

    /// 5 kHz first (the radio's front panel and CS-51PLUS), 6.25 kHz only when
    /// 5 kHz cannot reach the frequency, and nothing off both grids.
    #[test]
    fn multiplier_follows_the_radio() {
        assert_eq!(multiplier_for(448_400_000), Some((0, 89_680)), "memory 58, front panel");
        assert_eq!(multiplier_for(146_625_000), Some((0, 29_325)));
        assert_eq!(multiplier_for(145_115_000), Some((0, 29_023)));
        assert_eq!(multiplier_for(446_812_500), Some((1, 71_490)), "CSU DSTAR, 6.25 kHz only");
        assert_eq!(multiplier_for(118_400_000), Some((0, 23_680)));
        assert_eq!(multiplier_for(118_008_333), None, "an 8.33 kHz channel");
    }

    /// Memory 1 on Tim's radio, `CSU DSTAR`: 446.8125 DV (6.25 kHz units — 5 kHz
    /// cannot reach it), DUP− 5 MHz, UR CQCQCQ, R1 `W0QEY  B`, R2 `W0QEY  G`.
    /// Re-encoded from the channel the app would hold, it is the radio's own 49
    /// bytes but for the two tone fields a DV memory does not read.
    #[test]
    fn a_dv_memory_re_encodes_to_the_radios_bytes() {
        let radio = hex("251742032028a50000e404435355204453544152202020202020200087461d18745020aec28c5b281042aec28c5b281047");
        let mut c = ch(1, 446.8125);
        c.offset = Some(5.0);
        c.duplex = Some("-".into());
        c.mode = Some("DV".into());
        c.dstar_capable = true;
        c.dstar_ur_call = Some("CQCQCQ".into());
        c.dstar_rpt1 = Some("W0QEY  B".into());
        c.dstar_rpt2 = Some("W0QEY  G".into());
        let mut rec = encode_channel(&ec(c), "CSU DSTAR").unwrap();
        // Bytes 5-6: the radio left 94.8 Hz in this DV memory's unused tone
        // fields; a new one gets the 88.5 Hz default.
        let theirs = Record::decode(&radio);
        assert_eq!((rec.rtone, rec.ctone), (tone_index(88.5), tone_index(88.5)));
        rec.rtone = theirs.rtone;
        rec.ctone = theirs.ctone;
        assert_eq!(rec.encode().to_vec(), radio);
    }

    fn hex(s: &str) -> Vec<u8> {
        (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap()).collect()
    }

    /// Lists become banks in codeplug order, a channel in two lists lands in the
    /// first, unused slots are emptied, and the bank byte carries the
    /// multiplier.
    #[test]
    fn write_memories_places_banks_and_clears_the_rest() {
        let mut image = vec![0u8; super::super::layout::IMAGE_LEN];
        let a = ch(1, 446.8125);
        let b = ch(2, 145.115);
        let groups = vec![
            CodeplugGroup { list_id: 10, list_name: "Local".into(), channels: vec![a.clone()] },
            CodeplugGroup { list_id: 11, list_name: "Both".into(), channels: vec![a.clone(), b.clone()] },
        ];
        let ea = ec(a);
        let eb = ec(b);
        let model = RadioModel { memory_channels: Some(500), max_name_length: Some(16), ..Default::default() };
        let n = write_memories(&mut image, &[&ea, &eb], &groups, &model).unwrap();
        assert_eq!(n, 2);
        assert_eq!(read_bank(&image, 0), Some((0, 0)));
        assert_eq!(read_bank(&image, 1), Some((1, 0)));
        assert_eq!(image[BANK_TABLE] >> 5, 1, "446.8125 is a 6.25 kHz record");
        assert_eq!(image[BANK_TABLE + 2] >> 5, 0, "145.115 is a 5 kHz record");
        assert!(is_used(&image, 0) && is_used(&image, 1) && !is_used(&image, 2));
        assert_eq!(bank_name(&image, 0), "Local");
        assert_eq!(bank_name(&image, 1), "Both");
        assert_eq!(bank_name(&image, 2), "");
        assert_eq!(&image[BANK_TABLE + 4..BANK_TABLE + 6], &[NO_BANK, 0]);
    }

    /// `channel_fit` excludes an 8.33 kHz channel with a reason, so it never
    /// reaches the encoder (which would fail the whole export on it).
    #[test]
    fn an_833_khz_channel_is_excluded_upstream_with_a_reason() {
        let model = crate::seed::test_model("ID-51 Plus");
        let mut c = ch(1, 118.008333);
        c.mode = Some("AM".into());
        let why = crate::commands::export::exclusion_reason(&c, &model).expect("excluded");
        assert!(why.contains("6.25 kHz"), "{why}");
        let mut ok = ch(2, 118.400);
        ok.mode = Some("AM".into());
        assert_eq!(crate::commands::export::exclusion_reason(&ok, &model), None);
    }

    /// Outside the receiver's coverage is refused before anything is written.
    #[test]
    fn a_channel_outside_coverage_is_refused_and_nothing_moves() {
        let mut image = vec![0u8; super::super::layout::IMAGE_LEN];
        let before = image.clone();
        let x = ec(ch(1, 222.10));
        let model = RadioModel { memory_channels: Some(500), ..Default::default() };
        let err = write_memories(&mut image, &[&x], &[], &model).unwrap_err();
        assert!(err.contains("outside"), "{err}");
        assert_eq!(image, before);
    }
}
