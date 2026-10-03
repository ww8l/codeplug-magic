//! Memory records, flag bitmaps and banks inside an ID-5100 clone image.
//!
//! ## The record, 49 bytes
//!
//! | offset | bits | field |
//! |---|---|---|
//! | `+0x00` | 3·3·18 | `mult1`, `mult2`, frequency in `mult` units (u24 BE) |
//! | `+0x03` | 16 | offset in `mult1` units (u16 BE) |
//! | `+0x05` | 6·6·1·3 | repeater tone, TSQL tone, unknown, mode (u16 BE) |
//! | `+0x07` | 8 | DTCS code index |
//! | `+0x08` | 4·4 | tuning step, unknown |
//! | `+0x09` | 8 | unknown — see [`BYTE9_DEFAULT`] |
//! | `+0x0A` | 4·2·2 | squelch type, duplex, DTCS polarity |
//! | `+0x0B` | 16 chars | name, space padded |
//! | `+0x1B` | 8 | DV code |
//! | `+0x1C` | 3 x 7 | UR, RPT1, RPT2 — 8 chars at 7 bits each |
//!
//! Measured against the radio's own reads, `scratchpad/id5100/FINDINGS.md`:
//! all three multipliers (5 kHz, 6.25 kHz D-STAR repeaters, 8.33 kHz airband),
//! both duplex directions, TONE / TSQL / DTCS, FM / FM-N / AM / DV, the packed
//! call signs, and both skip flags. Every field value this module *writes* for
//! a new channel is one the radio itself stored somewhere in those reads — with
//! two exceptions named where they are made (DTCS polarity order, and the cross
//! squelch types, which are degraded rather than guessed).
//!
//! ## Frequency multiplier
//!
//! `mult` is 0 = 5 kHz, 1 = 6.25 kHz, 2 = 8.33 kHz (25/3 kHz), and the same
//! value is stored three times: `mult1`, `mult2`, and bits 5-6 of the memory's
//! bank byte. The radio chose `mult 0` for every 5 kHz-divisible
//! frequency it was given, including ones also divisible by 6.25 kHz (146.625,
//! 447.275), and `mult 2` for an airband memory on 118.400 even though that is a
//! 5 kHz multiple too. [`multiplier_for`] reproduces both choices.

use crate::commands::export::ExpandedChannel;
use crate::radios::icom_id52::memory::{
    ascii_field, dtcs_index, hz, pack_call, tone_index, DTCS_CODES, TONES_DHZ,
};
use crate::radios::icom_id52::memory_csv::{
    call_signs, duplex_and_offset, mode_of, tone_columns, tune_step,
};

use super::layout::{
    record_offset, BANK_CAPACITY, BANK_COUNT, BANK_NAMES, BANK_NAME_LEN, BANK_TABLE, BITMAP_LEN,
    CHANNEL_COUNT, EMPTY_BITMAP, NAME_LEN, NO_BANK, PSKIP_BITMAP, REC_LEN, SKIP_BITMAP,
};

/// Frequency multipliers in Hz, indexed by the stored `mult` value.
const MULT_HZ: [f64; 3] = [5_000.0, 6_250.0, 25_000.0 / 3.0];

/// Squelch type (`+0x0A` high nibble). Only the values measured on this radio:
/// OFF, TONE and TSQL from its own memories, DTCS from the front-panel probe.
/// CHIRP lists more (reverse and cross types, 6-11); none of them has been
/// stored by this radio in front of us, so none of them is written.
const SQL_OFF: u8 = 0;
const SQL_TONE: u8 = 1;
const SQL_TSQL: u8 = 3;
const SQL_DTCS: u8 = 5;

/// Mode (`+0x05` low 3 bits). FM, AM and DV from the radio's own memories, FM-N
/// from the probe flipped FM → FM-N on the front panel (`80 → 81`).
const MODE_FM: u8 = 0;
const MODE_FM_N: u8 = 1;
const MODE_AM: u8 = 3;
const MODE_DV: u8 = 5;

/// Tuning step (`+0x08` high nibble). `0` is what the radio stored for every
/// 2 m / 70 cm memory it had, including the 6.25 kHz ones. `14` is what it
/// stored on its airband memory — CHIRP does not know that value (its table
/// fills 11-14 with placeholders), and the radio's tuning-step window would not
/// open on a memory to say which of 8.33k / 25k / Auto it is. Reproduced
/// because the radio chose it for exactly this kind of channel, not because its
/// meaning is known.
const STEP_DEFAULT: u8 = 0;
const STEP_AIRBAND: u8 = 14;

/// Byte `+0x09`. Unknown. `0xE4` in 41 of the radio's 42 used memories, in its
/// empty-slot default record and in both call channels; `0x00` only in the
/// memories stored from the front panel during the probe session. CHIRP calls it
/// `unknown3` and writes zero. `0xE4` is written because it is what 41 memories
/// this radio demonstrably works with carry — a decoded record keeps its own
/// value regardless.
pub(crate) const BYTE9_DEFAULT: u8 = 0xE4;

/// One memory record, field by field and **lossless**: `encode(decode(b)) == b`
/// for any 49 bytes, including fields nobody understands. That property is the
/// Phase 2 gate, and it is also what lets a record the operator made on the
/// radio pass through a re-program untouched.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Record {
    pub mult1: u8,
    pub mult2: u8,
    pub freq_units: u32,
    pub offset_units: u16,
    pub rtone: u8,
    pub ctone: u8,
    pub unknown1: u8,
    pub mode: u8,
    pub dtcs: u8,
    pub step: u8,
    pub unknown2: u8,
    pub byte9: u8,
    pub squelch: u8,
    pub duplex: u8,
    pub polarity: u8,
    pub name: [u8; NAME_LEN],
    pub dv_code: u8,
    pub ur: [u8; 7],
    pub rpt1: [u8; 7],
    pub rpt2: [u8; 7],
}

impl Record {
    pub(crate) fn decode(b: &[u8]) -> Record {
        assert_eq!(b.len(), REC_LEN, "an ID-5100 record is {REC_LEN} bytes");
        let u24 = u32::from_be_bytes([0, b[0], b[1], b[2]]);
        let tones = u16::from_be_bytes([b[5], b[6]]);
        let arr7 = |at: usize| -> [u8; 7] { b[at..at + 7].try_into().expect("7 bytes") };
        Record {
            mult1: (u24 >> 21) as u8 & 0x07,
            mult2: (u24 >> 18) as u8 & 0x07,
            freq_units: u24 & 0x3_FFFF,
            offset_units: u16::from_be_bytes([b[3], b[4]]),
            rtone: (tones >> 10) as u8 & 0x3F,
            ctone: (tones >> 4) as u8 & 0x3F,
            unknown1: (tones >> 3) as u8 & 0x01,
            mode: tones as u8 & 0x07,
            dtcs: b[7],
            step: b[8] >> 4,
            unknown2: b[8] & 0x0F,
            byte9: b[9],
            squelch: b[10] >> 4,
            duplex: (b[10] >> 2) & 0x03,
            polarity: b[10] & 0x03,
            name: b[11..27].try_into().expect("16 bytes"),
            dv_code: b[27],
            ur: arr7(28),
            rpt1: arr7(35),
            rpt2: arr7(42),
        }
    }

    pub(crate) fn encode(&self) -> [u8; REC_LEN] {
        let mut b = [0u8; REC_LEN];
        let u24 = (u32::from(self.mult1 & 7) << 21)
            | (u32::from(self.mult2 & 7) << 18)
            | (self.freq_units & 0x3_FFFF);
        b[0..3].copy_from_slice(&u24.to_be_bytes()[1..]);
        b[3..5].copy_from_slice(&self.offset_units.to_be_bytes());
        let tones = (u16::from(self.rtone & 0x3F) << 10)
            | (u16::from(self.ctone & 0x3F) << 4)
            | (u16::from(self.unknown1 & 1) << 3)
            | u16::from(self.mode & 7);
        b[5..7].copy_from_slice(&tones.to_be_bytes());
        b[7] = self.dtcs;
        b[8] = (self.step << 4) | (self.unknown2 & 0x0F);
        b[9] = self.byte9;
        b[10] = (self.squelch << 4) | ((self.duplex & 3) << 2) | (self.polarity & 3);
        b[11..27].copy_from_slice(&self.name);
        b[27] = self.dv_code;
        b[28..35].copy_from_slice(&self.ur);
        b[35..42].copy_from_slice(&self.rpt1);
        b[42..49].copy_from_slice(&self.rpt2);
        b
    }

    pub(crate) fn rx_hz(&self) -> f64 {
        f64::from(self.freq_units) * MULT_HZ.get(usize::from(self.mult1)).copied().unwrap_or(5_000.0)
    }

    pub(crate) fn offset_hz(&self) -> f64 {
        f64::from(self.offset_units)
            * MULT_HZ.get(usize::from(self.mult1)).copied().unwrap_or(5_000.0)
    }

    pub(crate) fn name(&self) -> String {
        String::from_utf8_lossy(&self.name).trim_end().to_string()
    }
}

/// Which multiplier a frequency is stored in, and how many of them. `None` when
/// the frequency is on none of the radio's three grids.
///
/// Order matters, and follows what the radio itself chose (module header):
/// airband prefers 8.33 kHz, everything else prefers 5 kHz, then 6.25 kHz.
fn multiplier_for(freq_hz: u32, airband: bool) -> Option<(u8, u32)> {
    let order: &[u8] = if airband { &[2, 0, 1] } else { &[0, 1, 2] };
    order.iter().find_map(|&m| {
        let step = MULT_HZ[usize::from(m)];
        let units = (f64::from(freq_hz) / step).round();
        // Within 2 Hz: an 8.33 kHz channel arrives as a decimal MHz value that
        // cannot be exact (118.008333…), and must still land on its grid point.
        ((units * step - f64::from(freq_hz)).abs() < 2.0 && units < f64::from(1u32 << 18))
            .then_some((m, units as u32))
    })
}

/// Build a record for an app channel.
///
/// Errors rather than approximates when the frequency or offset is on none of
/// the radio's grids: a memory written at a frequency the operator did not ask
/// for is worse than a codeplug that refuses to build and says why.
pub(crate) fn encode_channel(ec: &ExpandedChannel, name: &str) -> Result<Record, String> {
    let c = &ec.channel;
    let rx = hz(c.rx_freq);
    let airband = tune_step(ec) == "8.33kHz";
    let (mult, freq_units) = multiplier_for(rx, airband).ok_or_else(|| {
        format!("{:.5} MHz is not on the radio's 5, 6.25 or 8.33 kHz channel grid", c.rx_freq)
    })?;

    let (dup, offset_mhz) = duplex_and_offset(ec);
    let step = MULT_HZ[usize::from(mult)];
    let offset_hz = f64::from(hz(offset_mhz));
    let offset_units = (offset_hz / step).round();
    if (offset_units * step - offset_hz).abs() >= 2.0 || offset_units > f64::from(u16::MAX) {
        return Err(format!(
            "a {offset_mhz:.4} MHz offset cannot be stored beside a frequency on the {} kHz grid",
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
        // WFM and anything else the radio cannot store in a memory is FM, the
        // same choice the ID-52 writer makes.
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
        step: if airband { STEP_AIRBAND } else { STEP_DEFAULT },
        unknown2: 0,
        byte9: BYTE9_DEFAULT,
        squelch: SQL_OFF,
        duplex,
        polarity: 0,
        name: name_bytes,
        dv_code: 0,
        // An analog memory on this radio carries `CQCQCQ` and blank repeaters —
        // every one of the 38 analog memories read does.
        ur: pack_call("CQCQCQ"),
        rpt1: pack_call(""),
        rpt2: pack_call(""),
    };

    if mode == MODE_DV {
        let (ur, rpt1, rpt2) = call_signs(ec, true);
        rec.ur = pack_call(&ur);
        rec.rpt1 = pack_call(&rpt1);
        rec.rpt2 = pack_call(&rpt2);
        return Ok(rec);
    }

    let t = tone_columns(ec);
    rec.squelch = match t.tone {
        "TONE" => SQL_TONE,
        "TSQL" => SQL_TSQL,
        "DTCS" | "DTCS(T)" | "DTCS(T)/TSQL(R)" => SQL_DTCS,
        "OFF" => SQL_OFF,
        // `TONE(T)/TSQL(R)` and `TONE(T)/DTCS(R)`: the cross types are not
        // measured on this radio. Keep the TRANSMIT side right — a memory that
        // does not key its repeater is the failure that matters.
        _ => SQL_TONE,
    };
    rec.rtone = tone_index(t.rpt_hz);
    rec.ctone = tone_index(t.tsql_hz);
    rec.dtcs = dtcs_index(t.dtcs.as_deref().or(c.dcs_code.as_deref()));
    // CHIRP's two-letter polarity, in CHIRP's order (NN, NR, RN, RR). Only `NN`
    // = 0 is measured here; the rest is CHIRP's table and the ID-52's order.
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

/// Whether memory `slot` holds a channel. The empty bitmap is authoritative:
/// the radio leaves old record bytes behind in a cleared slot — the probe
/// session wrote near-copies into two slots it left marked empty.
pub(crate) fn is_used(image: &[u8], slot: usize) -> bool {
    !bit(image, EMPTY_BITMAP, slot)
}

pub(crate) fn read_record(image: &[u8], slot: usize) -> Record {
    let at = record_offset(slot);
    Record::decode(&image[at..at + REC_LEN])
}

/// Skip state as the radio shows it: `SKIP` (passed over by the memory scan)
/// or `PSKIP` (passed over by the program scan too). The two bits were never
/// both set in any read; P wins if they ever are, as it does in CHIRP.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ScanSkip {
    None,
    Memory,
    Program,
}

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
pub(crate) fn read_bank(image: &[u8], slot: usize) -> Option<(u8, u8)> {
    let at = BANK_TABLE + slot * 2;
    let bank = image[at] & 0x1F;
    (bank != NO_BANK && usize::from(bank) < BANK_COUNT).then_some((bank, image[at + 1]))
}

/// Store a channel in `slot`: record, empty/skip bits, and its bank entry.
///
/// The bank byte is `[bit 7][mult: bits 5-6][bank: bits 0-4]`. The multiplier
/// bits are measured twice over: 4 of 4 non-5 kHz memories and 0 of 37 others
/// on Tim's configured radio, and all 1000 entries of a factory-reset one
/// (`9F`/`BF`/`DF` against records of mult 0/1/2). CHIRP writes the bare bank
/// number; a 6.25 kHz D-STAR memory written that way would disagree with
/// itself.
///
/// Bit 7 is unexplained. It is SET, with index `FF`, in every entry straight
/// after a factory reset, and CLEAR, with index `00` for an unbanked memory, in
/// every entry of a radio that has been configured — which is the form written
/// here, and the form the radio accepted on every ladder write.
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
    image[entry] = (rec.mult1 << 5) | b;
    image[entry + 1] = idx;
}

/// Empty `slot` the way the radio does: set its empty bit, clear its skips, and
/// take it out of any bank (`1F 00`, what every empty slot on the radio holds).
/// The record bytes are left alone — the radio leaves them too, and the empty
/// bit is what it reads.
pub(crate) fn clear(image: &mut [u8], slot: usize) {
    set_bit(image, EMPTY_BITMAP, slot, true);
    set_bit(image, SKIP_BITMAP, slot, false);
    set_bit(image, PSKIP_BITMAP, slot, false);
    let entry = BANK_TABLE + slot * 2;
    image[entry] = NO_BANK;
    image[entry + 1] = 0;
}

// Read back only by tests today: the download sample shows a memory's bank
// letter, and has no column for a bank's name.
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
// Human-readable decode, for the download sample
// ============================================================

pub(crate) struct DecodedMemory {
    pub slot: usize,
    pub name: String,
    pub rx_mhz: f64,
    pub shift: String,
    pub tone: String,
    pub mode: String,
    pub bank: Option<(char, u8)>,
    pub skip: ScanSkip,
}

fn tone_hz(idx: u8) -> String {
    TONES_DHZ
        .get(usize::from(idx))
        .map(|t| format!("{}.{}", t / 10, t % 10))
        .unwrap_or_else(|| format!("#{idx}"))
}

pub(crate) fn describe(rec: &Record) -> (String, String, String) {
    let shift = match rec.duplex {
        1 => format!("−{:.3}", rec.offset_hz() / 1e6),
        2 => format!("+{:.3}", rec.offset_hz() / 1e6),
        _ => String::new(),
    };
    let mode = match rec.mode {
        MODE_FM => "FM",
        MODE_FM_N => "NFM",
        MODE_AM => "AM",
        4 => "AM-N",
        MODE_DV => "DV",
        _ => "?",
    }
    .to_string();
    let tone = if rec.mode == MODE_DV {
        "—".to_string()
    } else {
        match rec.squelch {
            SQL_OFF => "—".to_string(),
            SQL_TONE => format!("T {}", tone_hz(rec.rtone)),
            SQL_TSQL => format!("TSQL {}", tone_hz(rec.ctone)),
            SQL_DTCS => {
                let code = DTCS_CODES.get(usize::from(rec.dtcs)).copied().unwrap_or(0);
                let pol = ["N", "NR", "RN", "R"][usize::from(rec.polarity & 3)];
                format!("DTCS {code:03} {pol}")
            }
            other => format!("sql {other}"),
        }
    };
    (shift, tone, mode)
}

pub(crate) fn decode_memories(image: &[u8]) -> Vec<DecodedMemory> {
    (0..CHANNEL_COUNT)
        .filter(|&s| is_used(image, s))
        .map(|s| {
            let rec = read_record(image, s);
            let (shift, tone, mode) = describe(&rec);
            DecodedMemory {
                slot: s,
                name: rec.name(),
                rx_mhz: rec.rx_hz() / 1e6,
                shift,
                tone,
                mode,
                bank: read_bank(image, s).map(|(b, i)| ((b'A' + b) as char, i)),
                skip: read_skip(image, s),
            }
        })
        .collect()
}

const _: () = assert!(BITMAP_LEN * 8 >= CHANNEL_COUNT);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::Channel;

    fn hex(s: &str) -> Vec<u8> {
        (0..s.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
            .collect()
    }

    /// Records copied verbatim from the radio's own clone reads, 2026-09-25
    /// (`scratchpad/id5100/id5100_01_base.img`, and `_06_` for the probe).
    /// Memories 7 and 46 and the DV memory's routing were also read back off
    /// the radio's screen.
    ///
    /// 446.8125 DV, DUP− 5.000, 6.25 kHz multiplier, RPT1 `W0QEY  B`.
    const REAL_1_DV: &str = "251742032028a50000e404435355204453544152202020202020200087461d18745020aec28c5b281042aec28c5b281047";
    /// 447.700 FM, DUP− 5.000, TSQL 100.0 (P-skip on the radio).
    const REAL_7_TSQL: &str = "015dc403e830c00000e4344255434b48524e2020202020202020200087461d187450204081020408102040810204081020";
    /// 147.000 FM, DUP+ 0.600, TSQL 100.0.
    const REAL_12_PLUS: &str = "0072d8007830c00000e438574152532020202020202020202020200087461d187450204081020408102040810204081020";
    /// 145.310 FM, DUP− 0.600, TONE 88.5.
    const REAL_16_TONE: &str = "007186007820800000e414434320424f554c4420202020202020200087461d187450204081020408102040810204081020";
    /// 118.400 AM, 8.33 kHz multiplier, step nibble 14.
    const REAL_46_AIR: &str = "483780000030c300e0e400464e4c204154432020202020202020200087461d187450204081020408102040810204081020";
    /// The front-panel probe after FM → FM-N: DTCS 023, DUP− 0.600 (the
    /// radio's auto-repeater added it), byte 9 = 00.
    const REAL_47_PROBE: &str = "007186007820810000005450524f424520445443532020202020200087461d187450204081020408102040810204081020";

    fn real(h: &str) -> Record {
        Record::decode(&hex(h))
    }

    #[test]
    fn decode_encode_is_lossless_for_any_bytes() {
        // Every byte value in every position, 49 records' worth.
        for fill in 0..=255u8 {
            let mut b = [0u8; REC_LEN];
            for (i, x) in b.iter_mut().enumerate() {
                *x = fill.wrapping_add((i as u8).wrapping_mul(37));
            }
            assert_eq!(Record::decode(&b).encode(), b, "fill {fill:#04x}");
        }
    }

    #[test]
    fn multiplier_matches_what_the_radio_chose() {
        // 5 kHz-divisible frequencies, even ones 6.25 kHz also divides: mult 0.
        assert_eq!(multiplier_for(146_625_000, false), Some((0, 29_325)));
        assert_eq!(multiplier_for(447_275_000, false), Some((0, 89_455)));
        // 6.25-only: the D-STAR repeaters.
        assert_eq!(multiplier_for(446_812_500, false), Some((1, 71_490)));
        assert_eq!(multiplier_for(145_387_500, false), Some((1, 23_262)));
        // Airband prefers 8.33 kHz even on a 5 kHz multiple — 118.400 is stored
        // as `m22`, units 14208, in the radio's own read.
        assert_eq!(multiplier_for(118_400_000, true), Some((2, 14_208)));
        // An 8.33 kHz channel arrives inexact and still lands on its grid point.
        assert_eq!(multiplier_for(118_008_333, true), Some((2, 14_161)));
        // On no grid at all.
        assert_eq!(multiplier_for(146_521_000, false), None);
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

    fn channel(rx: f64) -> Channel {
        Channel {
            rx_freq: rx,
            dcs_polarity: "NN".into(),
            ..Default::default()
        }
    }

    /// Byte-for-byte against memory 7 of the radio's own read: an app channel
    /// describing the same repeater encodes to the same record, except byte 9's
    /// provenance-dependent value, which both carry as E4.
    #[test]
    fn a_tsql_repeater_encodes_like_the_radios_own_record() {
        let mut c = channel(447.700);
        c.duplex = Some("-".into());
        c.offset = Some(5.0);
        c.tone_mode = Some("TSQL".into());
        c.ctcss_uplink = Some(100.0);
        c.ctcss_downlink = Some(100.0);
        let rec = encode_channel(&ec(c), "BUCKHRN").unwrap();
        assert_eq!(rec, real(REAL_7_TSQL));
    }

    #[test]
    fn plus_shift_and_tone_repeaters_encode_like_the_radios_own() {
        let mut c = channel(147.000);
        c.duplex = Some("+".into());
        c.offset = Some(0.6);
        c.tone_mode = Some("TSQL".into());
        c.ctcss_uplink = Some(100.0);
        c.ctcss_downlink = Some(100.0);
        assert_eq!(encode_channel(&ec(c), "WARS").unwrap(), real(REAL_12_PLUS));

        let mut c = channel(145.310);
        c.duplex = Some("-".into());
        c.offset = Some(0.6);
        c.tone_mode = Some("Tone".into());
        c.ctcss_uplink = Some(88.5);
        assert_eq!(encode_channel(&ec(c), "CC BOULD").unwrap(), real(REAL_16_TONE));
    }

    #[test]
    fn a_dv_repeater_carries_its_routing() {
        let mut c = channel(446.8125);
        c.mode = Some("DV".into());
        c.duplex = Some("-".into());
        c.offset = Some(5.0);
        c.dstar_ur_call = Some("CQCQCQ".into());
        c.dstar_rpt1 = Some("W0QEY  B".into());
        c.dstar_rpt2 = Some("W0QEY  G".into());
        let rec = encode_channel(&ec(c), "CSU DSTAR").unwrap();
        // Identical to the radio's own record except the two tone indices,
        // which a DV memory does not use: the radio left 94.8 in them, the
        // encoder writes its 88.5 default.
        let want = Record {
            rtone: rec.rtone,
            ctone: rec.ctone,
            ..real(REAL_1_DV)
        };
        assert_eq!(rec, want);
    }

    #[test]
    fn airband_am_uses_the_radios_airband_encoding() {
        let mut c = channel(118.400);
        c.mode = Some("AM".into());
        let rec = encode_channel(&ec(c), "FNL ATC").unwrap();
        // Identical except the unused tone indices (the radio left 100.0).
        let want = Record {
            rtone: rec.rtone,
            ctone: rec.ctone,
            ..real(REAL_46_AIR)
        };
        assert_eq!(rec, want);
    }

    #[test]
    fn dtcs_and_narrow_fm_match_the_probe() {
        let mut c = channel(145.310);
        c.mode = Some("NFM".into());
        c.duplex = Some("-".into());
        c.offset = Some(0.6);
        c.tone_mode = Some("DTCS".into());
        c.dcs_code = Some("023".into());
        let rec = encode_channel(&ec(c), "PROBE DTCS").unwrap();
        // Identical except byte 9: the front panel stored 00 where the encoder
        // writes the E4 that 41 of the radio's memories carry.
        let want = Record {
            byte9: BYTE9_DEFAULT,
            ..real(REAL_47_PROBE)
        };
        assert_eq!(rec, want);
    }

    #[test]
    fn a_frequency_off_every_grid_is_refused_not_rounded() {
        let e = encode_channel(&ec(channel(146.521)), "ODD").unwrap_err();
        assert!(e.contains("146.52100"), "{e}");
    }

    #[test]
    fn store_writes_the_multiplier_into_the_bank_byte() {
        let mut image = vec![0xFFu8; super::super::layout::IMAGE_LEN];
        let mut c = channel(446.8125);
        c.mode = Some("DV".into());
        let rec = encode_channel(&ec(c), "X").unwrap();
        store(&mut image, 1, &rec, Some((0, 2)));
        assert_eq!(&image[BANK_TABLE + 2..BANK_TABLE + 4], &[0x20, 0x02]);
        assert!(is_used(&image, 1));
        assert_eq!(read_bank(&image, 1), Some((0, 2)));
        clear(&mut image, 1);
        assert!(!is_used(&image, 1));
        assert_eq!(&image[BANK_TABLE + 2..BANK_TABLE + 4], &[NO_BANK, 0]);
    }
}
