//! AnyTone AT-D578UV driver (`driver_key = "anytone_atd578uv"`, issue #47).
//!
//! The D578UV speaks the same clone protocol as the D890UV — `PROGRAM`, the
//! `0x02` ident, 16-byte `R`/`W` frames with the `addr+len+data` checksum, `END`
//! to commit and reboot — so the transport, the window read-modify-write engine
//! and the codeplug planner are shared with `anytone_atd890uv`. The MEMORY MAP is
//! not shared: the D578 uses the D868/D878 family layout with its own overrides,
//! and every base address and record size below differs from the D890's.
//!
//! Sources, both for firmware 1.21 (the radio this was built against reads
//! `ID578UV\0` band 0x0E rev `V111`, Device Info firmware 1.21):
//!   - qdmr (GPLv3) `lib/d578uv_codeplug.*`, `d878uv_codeplug.*`, `d868uv_codeplug.*`;
//!   - the dmr-tools/codeplugs `d578uv` v1.21 definition (machine-generated from
//!     the CPS by anytone-emu).
//!
//! Every offset this module writes was anchored on a read-only dump of a real
//! D578UV first (`scratchpad/anytone_atd578uv/FINDINGS.md`): 392 channels, 21
//! zones and 21 scan lists decoded at these addresses, and the channel fields
//! agree with the operator's own channel library on 272 independently joined
//! rows. The fixture in `testdata/` is that dump's record regions only.
//!
//! Two traps, each measured on the radio rather than taken from a source:
//!   - the CTCSS table has NO 150.0 Hz entry (index 25 = 151.4). The published
//!     D578 maps agree; the D890UV's hardware-proven table does have one, so the
//!     two drivers' tables are NOT interchangeable ([`CTCSS`]);
//!   - dmr-tools gives channel byte 0x3D bit 0 a default of 1; the radio writes 0
//!     on 391 of 392 channels ([`channel_record_template`]).

use serialport::SerialPort;

use std::collections::{BTreeMap, BTreeSet};


use crate::radios::anytone_atd890uv::{
    ascii, bcd_be_encode, encode_mode_byte, encode_shift, end_session,
    enter_program_and_ident_as, freq_encode, hex, open_port, read_block, write_block,
    AnytoneChannelEdit,
    AnytoneSubTone, BankPlan, IdentExpect, BAUD, WRITE_WINDOW,
};
#[cfg(test)]
use crate::radios::anytone_atd890uv::{bcd_be, freq_mhz, AnytoneToneEdit};
use crate::radios::driver::{
    CallsignDbWriter, CodeplugProgrammer, DriverDiagnostics, RadioDriver, RadioIdentity,
    SettingsReader, SettingsWriter,
};

pub(crate) mod callsign_db;
pub(crate) mod program;
pub(crate) mod settings;

// ============================================================
// Driver registration
// ============================================================

pub(crate) struct AnytoneAtd578uv;

pub(crate) static DRIVER: AnytoneAtd578uv = AnytoneAtd578uv;

/// The ident every D578 session requires. A D890UV (or a D878UV, whose map is
/// close but whose channel tail is laid out differently) is refused before a
/// single address is read — and so is a D578 of another revision. The map is
/// firmware 1.21's; field positions moved between 1.17 and 1.21, and the PLUS
/// runs separate firmware. No readable region carries the firmware version
/// (0x02FA0000 holds a serial and build date), so the ident's revision is the
/// gate: qdmr's 1.21 map is for `V111`, which the anchor radio reports.
pub(crate) const D578_IDENT: IdentExpect = IdentExpect {
    token: "578",
    name: "D578UV",
    revision: Some("V111"),
};

impl RadioDriver for AnytoneAtd578uv {
    fn key(&self) -> &'static str {
        "anytone_atd578uv"
    }

    fn display_name(&self) -> &'static str {
        "AnyTone AT-D578UV"
    }

    fn baud(&self) -> u32 {
        BAUD
    }

    fn identify(&self, port: &str) -> Result<RadioIdentity, String> {
        let mut p = open_port(port)?;
        let ident = enter(&mut *p)?;
        // END reboots the radio and drops it off USB; it is always the last
        // thing a session does.
        let _ = end_session(&mut *p);
        Ok(RadioIdentity {
            matched: ascii(&ident),
            ident_hex: hex(&ident),
            ident_ascii: Some(ascii(&ident)),
        })
    }

    // The programming unit is the whole codeplug (channels + zones + scan lists
    // + contacts in one session), as on the D890UV.
    fn as_codeplug_programmer(&self) -> Option<&dyn CodeplugProgrammer> {
        Some(self)
    }

    // Settings are a separate, explicitly chosen write (the dialog's Profile
    // payload); a codeplug program leaves the radio's own alone.
    fn as_settings_reader(&self) -> Option<&dyn SettingsReader> {
        Some(self)
    }

    fn as_settings_writer(&self) -> Option<&dyn SettingsWriter> {
        Some(self)
    }

    fn as_callsign_db_writer(&self) -> Option<&dyn CallsignDbWriter> {
        Some(self)
    }

    fn as_diagnostics(&self) -> Option<&dyn DriverDiagnostics> {
        Some(self)
    }
}

impl DriverDiagnostics for AnytoneAtd578uv {
    fn dump_raw(&self, port: &str, start: u32, len: u32) -> Result<Vec<u8>, String> {
        let mut p = open_port(port)?;
        let _ident = enter(&mut *p)?;
        let data = read_block(&mut *p, start, len as usize);
        let _ = end_session(&mut *p);
        data
    }
}

/// Enter PC mode and identify, refusing anything that is not a D578UV.
pub(crate) fn enter(p: &mut dyn SerialPort) -> Result<Vec<u8>, String> {
    enter_program_and_ident_as(p, D578_IDENT)
}

// ============================================================
// Memory map (firmware 1.21; anchored on a real dump — see the module doc)
// ============================================================

/// Channels: 32 banks × 128 records of 0x40 bytes. Bank N is at
/// `CHANNEL_BASE + N * BANK_STEP`; its records fill the first 0x2000 bytes and a
/// parallel 0x40-byte extension record per channel fills the next 0x2000, so one
/// 0x4000 write window covers a bank's records AND their extensions.
pub(crate) const CHANNEL_BASE: u32 = 0x0080_0000;
pub(crate) const BANK_STEP: u32 = 0x0004_0000;
pub(crate) const NUM_BANKS: usize = 32;
pub(crate) const CH_PER_BANK: usize = 128;
pub(crate) const CH_REC_LEN: usize = 0x40;
pub(crate) const MAX_CHANNELS: usize = 4000;
/// Channel-present bitmap: 1 bit per channel, LSB-first, SET = present. 392 bits
/// set on the anchor radio, exactly the 392 records that decode.
pub(crate) const CHANNEL_BITMAP_BASE: u32 = 0x024C_1500;
pub(crate) const CHANNEL_BITMAP_BYTES: usize = 500;

/// Zones: 250 member lists of 250 u16-LE 0-based channel indices (0xFFFF fill)
/// at `ZONE_LIST_BASE` step 0x200, names 16 ASCII chars in a 0x20 record at
/// `ZONE_NAME_BASE`, and a present bitmap (SET = present).
pub(crate) const ZONE_LIST_BASE: u32 = 0x0100_0000;
pub(crate) const ZONE_LIST_STEP: u32 = 0x200;
pub(crate) const ZONE_LIST_SLOTS: usize = 250;
pub(crate) const ZONE_NAME_BASE: u32 = 0x0254_0000;
pub(crate) const ZONE_NAME_STEP: u32 = 0x20;
pub(crate) const MAX_ZONES: usize = 250;
pub(crate) const ZONE_BITMAP_BASE: u32 = 0x024C_1300;
pub(crate) const ZONE_BITMAP_BYTES: usize = 32;
/// Channels per zone. The record holds 250 and qdmr, dmr-tools and the radio's
/// own 75-channel zone agree on more than the 2019 manual's 160; 250 is the
/// record's capacity and the published limit.
pub(crate) const ZONE_MAX_CHANNELS: usize = 250;

/// Scan lists: 16 records of 0x200 per 0x40000 bank (list 17 is at 0x010C0000 —
/// confirmed on the anchor radio, which carries 21). Record layout (dmr-tools,
/// all 21 radio lists decode with it): +0x01 priority select, +0x02/+0x04
/// priority channel u16 LE (0xFFFF none), +0x06/+0x08 look-back A/B, +0x0A
/// dropout, +0x0C dwell (u16 LE, 0.1 s), +0x0E revert enum, +0x0F name (16
/// ASCII), +0x20 50 × u16 LE 0-based channel indices (0xFFFF fill).
pub(crate) const SCAN_LIST_BASE: u32 = 0x0108_0000;
pub(crate) const SCAN_LISTS_PER_BANK: usize = 16;
pub(crate) const SCAN_LIST_STEP: u32 = 0x200;
pub(crate) const MAX_SCAN_LISTS: usize = 250;
pub(crate) const SCAN_BITMAP_BASE: u32 = 0x024C_1340;
pub(crate) const SCAN_BITMAP_BYTES: usize = 32;
pub(crate) const SCAN_MAX_CHANNELS: usize = 50;
pub(crate) const SCAN_NAME: usize = 0x0F;
pub(crate) const SCAN_MEMBERS: usize = 0x20;
/// End of the bytes this driver owns in a scan-list record. 0x84..0x90 carry
/// values that match no channel field on the anchor radio (dmr-tools calls them
/// unused); a read-modify-write leaves them, and everything after, alone.
pub(crate) const SCAN_OWNED_END: usize = SCAN_MEMBERS + SCAN_MAX_CHANNELS * 2;

/// DMR contacts: 10 banks of 1000 records of 100 bytes, bank step 0x40000.
/// +0x00 call type (0 private, 1 group, 2 all), +0x01 name (16 ASCII), +0x23
/// DMR ID (BCD BE, 4 bytes), +0x27 call alert.
pub(crate) const CONTACT_BASE: u32 = 0x0268_0000;
pub(crate) const CONTACT_BANK_STEP: u32 = 0x0004_0000;
pub(crate) const CONTACTS_PER_BANK: usize = 1000;
pub(crate) const CONTACT_REC_LEN: usize = 0x64;
pub(crate) const MAX_CONTACTS: usize = 10_000;
pub(crate) const CONTACT_NAME: usize = 0x01;
pub(crate) const CONTACT_DMR_ID: usize = 0x23;
/// Contact index list: u32 LE per listed contact, 0xFFFFFFFF fill.
pub(crate) const CONTACT_INDEX_BASE: u32 = 0x0260_0000;
/// Contact present bitmap — INVERTED (a clear bit = present), as on the D890.
pub(crate) const CONTACT_BITMAP_BASE: u32 = 0x0264_0000;
pub(crate) const CONTACT_BITMAP_BYTES: usize = 1250;

/// Contact ID table: one 8-byte entry per contact — a u32 LE key of the DMR ID
/// in BCD shifted left one bit with the group-call flag in bit 0, then the u32
/// LE contact index — sorted by key, then one 0xFF entry. Because the ID is BCD
/// the key order is numeric ID order. The radio looks talkgroups up here;
/// rebuilding it from the anchor radio's 38 contacts reproduces its own table
/// byte for byte.
pub(crate) const CONTACT_ID_TABLE: u32 = 0x0480_0000;
pub(crate) const CONTACT_ID_ENTRY: usize = 8;

/// Air band: a separate memory of 100 AM receive-only channels, 108-136 MHz
/// (qdmr's limits). Each 0x20 record: frequency BCD BE in 10 Hz units (4),
/// name 16 ASCII NUL-padded, 12 zero bytes; an empty slot is all 0xFF. A present
/// bitmap and a scan-enable bitmap follow at 0x02BC1020 / 0x02BC1040, and the
/// air-band VFO at 0x02BC1000 is left alone. The anchor radio holds one: slot
/// 1, 108.000 MHz, no name, present and scanned.
pub(crate) const AIR_BASE: u32 = 0x02BC_0000;
pub(crate) const AIR_REC_LEN: usize = 0x20;
pub(crate) const MAX_AIR: usize = 100;
pub(crate) const AIR_BITMAP: u32 = 0x02BC_1020;
pub(crate) const AIR_SCAN_BITMAP: u32 = 0x02BC_1040;
pub(crate) const AIR_BITMAP_BYTES: usize = 13;
pub(crate) const AIR_MIN_MHZ: f64 = 108.0;
pub(crate) const AIR_MAX_MHZ: f64 = 136.0;

/// One air-band record.
pub(crate) fn encode_air_channel(mhz: f64, name: &str) -> Vec<u8> {
    let mut rec = vec![0u8; AIR_REC_LEN];
    rec[0..4].copy_from_slice(&freq_encode(mhz));
    rec[4..4 + NAME_CHARS].copy_from_slice(&encode_ascii_field(name, NAME_CHARS));
    rec
}

/// Name width shared by channels, zones, scan lists and contacts.
pub(crate) const NAME_CHARS: usize = 16;

// ============================================================
// Channel record (0x40 bytes)
// ============================================================
//
//   0x00  RX freq, BCD BE, 10 Hz units      0x14  contact index, u32 LE
//   0x04  TX offset, BCD BE (= RX, simplex) 0x18  radio-ID index
//   0x08  shift(7-6) bw(5-4) pwr(3-2) mode  0x19  squelch mode, bit 4 = CTCSS/DCS
//   0x09  tone enables, low nibble          0x1B  scan list (0xFF none)
//   0x0A  TX CTCSS index  0x0B  RX index    0x1C  RX group list (0xFF none)
//   0x0C  TX DCS u16 LE   0x0E  RX DCS      0x20  colour code
//   0x10  custom CTCSS                      0x21  bit 0 = time slot (0 = TS1)
//   0x23  name, 16 ASCII, NUL padded        0x34..0x3F  D578-specific tail
//
// Bytes 0x00-0x22 are laid out as on the D890UV; the name and everything after
// it are not.

pub(crate) const CH_RX: usize = 0x00;
pub(crate) const CH_TX_OFFSET: usize = 0x04;
pub(crate) const CH_MODE: usize = 0x08;
pub(crate) const CH_TONE_EN: usize = 0x09;
pub(crate) const CH_TX_CTCSS: usize = 0x0A;
pub(crate) const CH_RX_CTCSS: usize = 0x0B;
pub(crate) const CH_TX_DCS: usize = 0x0C;
pub(crate) const CH_RX_DCS: usize = 0x0E;
pub(crate) const CH_CONTACT: usize = 0x14;
pub(crate) const CH_SQUELCH_MODE: usize = 0x19;
pub(crate) const CH_SQ_CTCSS_DCS: u8 = 0x10;
pub(crate) const CH_SCAN_LIST: usize = 0x1B;
pub(crate) const CH_COLOR_CODE: usize = 0x20;
pub(crate) const CH_TIME_SLOT: usize = 0x21;
pub(crate) const CH_NAME: usize = 0x23;

/// CTCSS tone table in the radio's index order. 51 tones; 51 = custom.
///
/// MEASURED, and different from the D890UV's table: this radio has no 150.0 Hz
/// entry, so index 25 is 151.4. Eleven channels on the anchor radio use indices
/// 25 and up, and every one matches the operator's library only under this
/// table (`scratchpad/anytone_atd578uv/FINDINGS.md`). Reusing the D890's table
/// would shift every tone from 151.4 Hz up by one step.
pub(crate) const CTCSS: &[f64] = &[
    62.5, 67.0, 69.3, 71.9, 74.4, 77.0, 79.7, 82.5, 85.4, 88.5, 91.5, 94.8, 97.4, 100.0, 103.5,
    107.2, 110.9, 114.8, 118.8, 123.0, 127.3, 131.8, 136.5, 141.3, 146.2, 151.4, 156.7, 159.8,
    162.2, 165.5, 167.9, 171.3, 173.8, 177.3, 179.9, 183.5, 186.2, 189.9, 192.8, 196.6, 199.5,
    203.5, 206.5, 210.7, 218.1, 225.7, 229.1, 233.6, 241.8, 250.3, 254.1,
];

/// The CTCSS table index for a tone in Hz, if the radio has it.
pub(crate) fn ctcss_index(hz: f64) -> Option<u8> {
    CTCSS.iter().position(|&t| (t - hz).abs() < 0.05).map(|i| i as u8)
}

/// Absolute address of a 0-based channel slot's record.
pub(crate) fn channel_addr(slot0: usize) -> u32 {
    CHANNEL_BASE
        + (slot0 / CH_PER_BANK) as u32 * BANK_STEP
        + ((slot0 % CH_PER_BANK) * CH_REC_LEN) as u32
}

/// The record a never-programmed slot starts from: every byte this driver does
/// not encode takes the value the radio itself wrote on the most channels of the
/// anchor dump (the DCS fields' 0x0011, custom CTCSS 0x09CF, "no group list"
/// 0xFF, zeros elsewhere — including 0x3D, whose published default is wrong).
pub(crate) fn channel_record_template() -> [u8; CH_REC_LEN] {
    let mut rec = [0u8; CH_REC_LEN];
    rec[CH_TX_CTCSS] = 0x0D; // 100.0 Hz, inert until a tone-enable bit is set
    rec[CH_RX_CTCSS] = 0x0D;
    rec[CH_TX_DCS] = 0x11;
    rec[CH_RX_DCS] = 0x11;
    rec[0x10] = 0xCF;
    rec[0x11] = 0x09;
    rec[CH_SCAN_LIST] = 0xFF;
    rec[0x1C] = 0xFF;
    rec
}

/// Fixed-width ASCII name: up to `chars` bytes, NUL padded, folded the same way
/// as the call-sign database (`util::fold_ascii`: Zürich -> Zurich).
pub(crate) fn encode_ascii_field(name: &str, chars: usize) -> Vec<u8> {
    let mut out = crate::util::fold_ascii(name);
    out.resize(chars, 0);
    out
}

/// Inverse of [`encode_ascii_field`]: stop at the first NUL, trim trailing spaces.
pub(crate) fn decode_ascii_field(data: &[u8]) -> String {
    data.iter()
        .take_while(|&&b| b != 0 && b != 0xFF)
        .map(|&b| if (0x20..0x7F).contains(&b) { b as char } else { '?' })
        .collect::<String>()
        .trim_end()
        .to_string()
}

/// True for a record nothing has been written to.
#[cfg(test)]
pub(crate) fn is_blank(rec: &[u8]) -> bool {
    rec.iter().all(|&b| b == 0xFF) || rec.iter().all(|&b| b == 0x00)
}

/// Decode a channel record into the family's channel edit — the exact inverse of
/// [`encode_channel_into`] for every field the encoder owns. `None` for a blank
/// record.
#[cfg(test)]
pub(crate) fn decode_channel(rec: &[u8]) -> Option<AnytoneChannelEdit> {
    if rec.len() < CH_REC_LEN || is_blank(rec) {
        return None;
    }
    let rx_mhz = freq_mhz(&rec[CH_RX..CH_RX + 4]);
    let offset = freq_mhz(&rec[CH_TX_OFFSET..CH_TX_OFFSET + 4]);
    let m = rec[CH_MODE];
    let tx_mhz = match (m >> 6) & 0x03 {
        1 => rx_mhz + offset,
        2 => rx_mhz - offset,
        _ => rx_mhz,
    };
    let en = rec[CH_TONE_EN];
    let side = |ctcss_on: bool, dcs_on: bool, c: usize, d: usize| {
        if ctcss_on {
            CTCSS.get(rec[c] as usize).map(|&hz| AnytoneSubTone::Ctcss(hz))
        } else if dcs_on {
            Some(AnytoneSubTone::Dcs(u16::from_le_bytes([rec[d], rec[d + 1]])))
        } else {
            None
        }
    };
    Some(AnytoneChannelEdit {
        name: decode_ascii_field(&rec[CH_NAME..CH_NAME + NAME_CHARS]),
        rx_mhz,
        tx_mhz: (tx_mhz * 100_000.0).round() / 100_000.0,
        power: (m >> 2) & 0x03,
        wide: (m >> 4) & 0x03 == 1,
        mode: m & 0x03,
        tone: AnytoneToneEdit {
            tx: side(en & 0x04 != 0, en & 0x08 != 0, CH_TX_CTCSS, CH_TX_DCS),
            rx: side(en & 0x01 != 0, en & 0x02 != 0, CH_RX_CTCSS, CH_RX_DCS),
        },
        color_code: rec[CH_COLOR_CODE],
        time_slot: (rec[CH_TIME_SLOT] & 0x01) + 1,
        contact_index: u32::from_le_bytes([
            rec[CH_CONTACT],
            rec[CH_CONTACT + 1],
            rec[CH_CONTACT + 2],
            rec[CH_CONTACT + 3],
        ]) as u16,
        scan_list_index: Some(rec[CH_SCAN_LIST]),
    })
}

/// Patch a channel edit onto a record (read-modify-write). Only the fields the
/// edit models are written; every other byte — the D578 tail, APRS, encryption,
/// radio-ID and group-list bytes — is left as it was.
///
/// Unlike the D890 encoder this writes BOTH field groups whatever the mode: the
/// D578 keeps a DMR channel's tone bytes and an FM channel's colour code in the
/// same record, and writing both is what lets a decoded record re-encode to
/// itself byte for byte. Squelch mode is derived from the RX tone, as on the
/// D890: an RX tone with carrier squelch never actually squelches.
pub(crate) fn encode_channel_into(rec: &mut [u8], edit: &AnytoneChannelEdit) -> Result<(), String> {
    if rec.len() != CH_REC_LEN {
        return Err(format!("channel record is {} bytes, expected {CH_REC_LEN}", rec.len()));
    }
    rec[CH_RX..CH_RX + 4].copy_from_slice(&freq_encode(edit.rx_mhz));
    let (shift, off) = encode_shift(edit.rx_mhz, edit.tx_mhz);
    // A simplex channel carries its own RX frequency in the offset field: all
    // 57 simplex channels on the anchor radio do, and none hold zero. The
    // direction bits say simplex, so the field is not used to transmit; writing
    // the radio's own convention keeps a re-encode byte-identical.
    let off = if shift == 0 { freq_encode(edit.rx_mhz) } else { off };
    rec[CH_TX_OFFSET..CH_TX_OFFSET + 4].copy_from_slice(&off);
    rec[CH_MODE] = encode_mode_byte(shift, edit.wide, edit.power, edit.mode);
    rec[CH_NAME..CH_NAME + NAME_CHARS].copy_from_slice(&encode_ascii_field(&edit.name, NAME_CHARS));
    if let Some(idx) = edit.scan_list_index {
        rec[CH_SCAN_LIST] = idx;
    }

    let mut en = rec[CH_TONE_EN] & 0xF0;
    match &edit.tone.tx {
        Some(AnytoneSubTone::Ctcss(hz)) => {
            en |= 0x04;
            rec[CH_TX_CTCSS] =
                ctcss_index(*hz).ok_or_else(|| format!("no CTCSS table entry for {hz} Hz (TX)"))?;
        }
        Some(AnytoneSubTone::Dcs(code)) => {
            en |= 0x08;
            rec[CH_TX_DCS..CH_TX_DCS + 2].copy_from_slice(&code.to_le_bytes());
        }
        None => {}
    }
    match &edit.tone.rx {
        Some(AnytoneSubTone::Ctcss(hz)) => {
            en |= 0x01;
            rec[CH_RX_CTCSS] =
                ctcss_index(*hz).ok_or_else(|| format!("no CTCSS table entry for {hz} Hz (RX)"))?;
        }
        Some(AnytoneSubTone::Dcs(code)) => {
            en |= 0x02;
            rec[CH_RX_DCS..CH_RX_DCS + 2].copy_from_slice(&code.to_le_bytes());
        }
        None => {}
    }
    rec[CH_TONE_EN] = en;

    rec[CH_COLOR_CODE] = edit.color_code.min(15);
    rec[CH_TIME_SLOT] = (rec[CH_TIME_SLOT] & !0x01) | (edit.time_slot.saturating_sub(1) & 0x01);
    rec[CH_CONTACT..CH_CONTACT + 4].copy_from_slice(&(edit.contact_index as u32).to_le_bytes());
    Ok(())
}

/// Set the squelch-mode bit from the channel's RX tone. Kept out of
/// [`encode_channel_into`] so the byte-identical gate can re-encode the radio's
/// own records — most of which carry an RX tone with carrier squelch, as their
/// last programmer left them — without this policy rewriting them.
pub(crate) fn apply_tone_squelch(rec: &mut [u8], edit: &AnytoneChannelEdit) {
    let sq = rec[CH_SQUELCH_MODE] & !CH_SQ_CTCSS_DCS;
    rec[CH_SQUELCH_MODE] = if edit.tone.rx.is_some() { sq | CH_SQ_CTCSS_DCS } else { sq };
}

// ============================================================
// Zone / scan-list / contact records
// ============================================================

/// One zone's member list: 0-based channel indices, 0xFFFF fill to 250 slots.
pub(crate) fn encode_zone_list(members: &[u16]) -> Result<Vec<u8>, String> {
    if members.len() > ZONE_LIST_SLOTS {
        return Err(format!("zone has {} channels (record holds {ZONE_LIST_SLOTS})", members.len()));
    }
    let mut out = vec![0xFFu8; ZONE_LIST_STEP as usize];
    for (i, m) in members.iter().enumerate() {
        out[i * 2..i * 2 + 2].copy_from_slice(&m.to_le_bytes());
    }
    Ok(out)
}

/// One zone's name record: 16 ASCII + 16 zero bytes.
pub(crate) fn encode_zone_name(name: &str) -> Vec<u8> {
    let mut out = encode_ascii_field(name, NAME_CHARS);
    out.resize(ZONE_NAME_STEP as usize, 0);
    out
}

/// Absolute address of a 0-based scan list's record.
pub(crate) fn scan_list_addr(i: usize) -> u32 {
    SCAN_LIST_BASE
        + (i / SCAN_LISTS_PER_BANK) as u32 * BANK_STEP
        + (i % SCAN_LISTS_PER_BANK) as u32 * SCAN_LIST_STEP
}

/// A scan list's owned fields, encoded as the record's first [`SCAN_OWNED_END`]
/// bytes. Priority channels are 0-based slots (`None` = 0xFFFF).
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ScanListFields {
    pub name: String,
    pub members: Vec<u16>,
    pub priority_select: u8,
    pub priority_1: Option<u16>,
    pub priority_2: Option<u16>,
    pub look_back_a: u16,
    pub look_back_b: u16,
    pub dropout_delay: u16,
    pub dwell_time: u16,
    pub revert_channel: u8,
}

/// Write a scan list's owned bytes onto `rec` (0x200), leaving 0x84.. alone.
pub(crate) fn encode_scan_list_into(rec: &mut [u8], s: &ScanListFields) -> Result<(), String> {
    if s.members.len() > SCAN_MAX_CHANNELS {
        return Err(format!(
            "scan list '{}' has {} channels (radio holds {SCAN_MAX_CHANNELS})",
            s.name,
            s.members.len()
        ));
    }
    rec[0x00] = 0x00;
    rec[0x01] = s.priority_select;
    rec[0x02..0x04].copy_from_slice(&s.priority_1.unwrap_or(0xFFFF).to_le_bytes());
    rec[0x04..0x06].copy_from_slice(&s.priority_2.unwrap_or(0xFFFF).to_le_bytes());
    rec[0x06..0x08].copy_from_slice(&s.look_back_a.to_le_bytes());
    rec[0x08..0x0A].copy_from_slice(&s.look_back_b.to_le_bytes());
    rec[0x0A..0x0C].copy_from_slice(&s.dropout_delay.to_le_bytes());
    rec[0x0C..0x0E].copy_from_slice(&s.dwell_time.to_le_bytes());
    rec[0x0E] = s.revert_channel;
    rec[SCAN_NAME..SCAN_NAME + NAME_CHARS].copy_from_slice(&encode_ascii_field(&s.name, NAME_CHARS));
    rec[SCAN_NAME + NAME_CHARS] = 0x00;
    rec[SCAN_MEMBERS..SCAN_OWNED_END].fill(0xFF);
    for (i, m) in s.members.iter().enumerate() {
        let o = SCAN_MEMBERS + i * 2;
        rec[o..o + 2].copy_from_slice(&m.to_le_bytes());
    }
    Ok(())
}

/// Decode a scan list's owned fields.
#[cfg(test)]
pub(crate) fn decode_scan_list(rec: &[u8]) -> ScanListFields {
    let u16_at = |o: usize| u16::from_le_bytes([rec[o], rec[o + 1]]);
    let opt = |v: u16| (v != 0xFFFF).then_some(v);
    ScanListFields {
        name: decode_ascii_field(&rec[SCAN_NAME..SCAN_NAME + NAME_CHARS]),
        members: (0..SCAN_MAX_CHANNELS)
            .map(|i| u16_at(SCAN_MEMBERS + i * 2))
            .filter(|&m| m != 0xFFFF)
            .collect(),
        priority_select: rec[0x01],
        priority_1: opt(u16_at(0x02)),
        priority_2: opt(u16_at(0x04)),
        look_back_a: u16_at(0x06),
        look_back_b: u16_at(0x08),
        dropout_delay: u16_at(0x0A),
        dwell_time: u16_at(0x0C),
        revert_channel: rec[0x0E],
    }
}

/// Absolute address of a 0-based contact record.
pub(crate) fn contact_addr(i: usize) -> u32 {
    CONTACT_BASE
        + (i / CONTACTS_PER_BANK) as u32 * CONTACT_BANK_STEP
        + ((i % CONTACTS_PER_BANK) * CONTACT_REC_LEN) as u32
}

/// One contact record. `call_type` 0 = private, 1 = group.
pub(crate) fn encode_contact(call_type: u8, dmr_id: u32, name: &str) -> Result<Vec<u8>, String> {
    if dmr_id > 16_777_215 {
        return Err(format!("DMR ID {dmr_id} is out of range"));
    }
    let mut rec = vec![0u8; CONTACT_REC_LEN];
    rec[0] = call_type;
    rec[CONTACT_NAME..CONTACT_NAME + NAME_CHARS].copy_from_slice(&encode_ascii_field(name, NAME_CHARS));
    rec[CONTACT_DMR_ID..CONTACT_DMR_ID + 4].copy_from_slice(&bcd_be_encode(dmr_id as u64, 4));
    Ok(rec)
}

/// `(call type, DMR ID, name)` of a contact record.
#[cfg(test)]
pub(crate) fn decode_contact(rec: &[u8]) -> (u8, u32, String) {
    (
        rec[0],
        bcd_be(&rec[CONTACT_DMR_ID..CONTACT_DMR_ID + 4]) as u32,
        decode_ascii_field(&rec[CONTACT_NAME..CONTACT_NAME + NAME_CHARS]),
    )
}

/// The contact ID table for `(call_type, dmr_id, index)` contacts, sorted, with
/// its 0xFF terminator.
pub(crate) fn encode_contact_id_table(contacts: &[(u8, u32, u32)]) -> Vec<u8> {
    let mut keyed: Vec<(u32, u32)> = contacts
        .iter()
        .map(|&(call_type, id, index)| {
            let bcd = u32::from_be_bytes(
                bcd_be_encode(id as u64, 4).try_into().expect("four BCD bytes"),
            );
            ((bcd << 1) | (call_type == 1) as u32, index)
        })
        .collect();
    keyed.sort_unstable();
    let mut out = Vec::with_capacity((keyed.len() + 1) * CONTACT_ID_ENTRY);
    for (key, index) in keyed {
        out.extend_from_slice(&key.to_le_bytes());
        out.extend_from_slice(&index.to_le_bytes());
    }
    out.extend_from_slice(&[0xFF; CONTACT_ID_ENTRY]);
    out
}

/// The family's bitmap builders: set-means-present, and the contact bitmap's
/// inverted form (a clear bit = present).
pub(crate) use crate::radios::anytone_atd890uv::program::{contact_present_bitmap, present_bitmap};

/// Indices of the set bits in a present bitmap.
pub(crate) fn bitmap_indices(bm: &[u8], n: usize) -> Vec<usize> {
    (0..n.min(bm.len() * 8)).filter(|&i| bm[i / 8] >> (i % 8) & 1 == 1).collect()
}

/// The contact bitmap for `n` contacts (inverted: the first `n` bits clear).
pub(crate) fn contact_bitmap(n: usize) -> Vec<u8> {
    contact_present_bitmap(n, CONTACT_BITMAP_BYTES)
}

// ============================================================
// Flash halves — the radio's real write unit
// ============================================================
//
// Every 256 KB block of the codeplug is two 128 KB halves, each ending in the
// marker `22 33 44 55 FF FF FF FF 55 55 AA AA` at +0x1FFF4. The codeplug lives
// in the first; the second holds the first's PREVIOUS contents — on a write the
// radio copies the old half there, then rebuilds the first half from what the
// session sent and nothing else. MEASURED on the anchor radio (2026-10-14): a
// settings write of the 16 KB window at 0x02500000 erased the GPS-roaming table
// at 0x02504000, which no write addressed; a repair write then left the second
// half byte-identical to the first half as it stood before the repair. The
// second half is the radio's; nothing here reads or writes it.
//
// So a session that writes a window must also write every other window of the
// same half that holds data, unchanged. [`with_carried_windows`] adds those.
//
// The marker frame is the radio's, and it rewrites it itself (it cleared the
// settings half's `22334455` on its own while saving the operator's zone
// browsing). A session that carried the marker windows unchanged left two of
// its thirteen halves' new data in the INACTIVE half — the radio kept showing
// the old channels — where the same program without them had committed every
// half live, twice. So the final 16-byte frame of a half is never sent, and a
// window whose only data is that marker is not carried ([`sendable`]).

/// A flash half: the unit the radio rebuilds on a write.
pub(crate) const FLASH_HALF: u32 = 0x0002_0000;
/// The final frame of every half, where the radio keeps its marker.
pub(crate) const MARKER_FRAME: usize = 0x10;

/// The bytes of a block starting at `base` this driver may send: all of them,
/// except a half's final frame when the block reaches the end of its half.
pub(crate) fn sendable(base: u32, data: &[u8]) -> &[u8] {
    let half_end = half_of(base) + FLASH_HALF;
    if base + data.len() as u32 == half_end && data.len() >= MARKER_FRAME {
        &data[..data.len() - MARKER_FRAME]
    } else {
        data
    }
}

/// Write one block, never its half's marker frame.
pub(crate) fn write_sendable(p: &mut dyn SerialPort, addr: u32, data: &[u8]) -> Result<(), String> {
    write_block(p, addr, sendable(addr, data))
}

/// Write every planned window (marker frames left out); the bases written.
pub(crate) fn commit_windows(p: &mut dyn SerialPort, plans: &[BankPlan]) -> Result<Vec<u32>, String> {
    for plan in plans {
        write_sendable(p, plan.base, &plan.patched)?;
    }
    Ok(plans.iter().map(|p| p.base).collect())
}

/// The plans as backup / expected-image blocks, marker frames left out, so a
/// Verify never compares the radio's own housekeeping.
pub(crate) fn plan_blocks(plans: &[BankPlan], patched: bool) -> Vec<(u32, Vec<u8>)> {
    plans
        .iter()
        .map(|p| {
            let data = if patched { &p.patched } else { &p.original };
            (p.base, sendable(p.base, data).to_vec())
        })
        .collect()
}

pub(crate) fn half_of(addr: u32) -> u32 {
    addr & !(FLASH_HALF - 1)
}

/// Every window of the halves `plans` touch, for the session to read.
pub(crate) fn windows_of_touched_halves(plans: &[BankPlan]) -> BTreeSet<u32> {
    plans
        .iter()
        .map(|p| half_of(p.base))
        .flat_map(|h| (0..FLASH_HALF / WRITE_WINDOW as u32).map(move |i| h + i * WRITE_WINDOW as u32))
        .collect()
}

/// `plans` plus, for every half they touch, each other window of that half that
/// holds data (anything but erased 0xFF), to be rewritten unchanged so the
/// radio's rebuild of the half keeps it. `reads` must hold every window of
/// every touched half ([`windows_of_touched_halves`]).
pub(crate) fn with_carried_windows(
    mut plans: Vec<BankPlan>,
    reads: &BTreeMap<u32, Vec<u8>>,
) -> Result<Vec<BankPlan>, String> {
    let planned: BTreeSet<u32> = plans.iter().map(|p| p.base).collect();
    for w in windows_of_touched_halves(&plans) {
        if planned.contains(&w) {
            continue;
        }
        let data = reads
            .get(&w)
            .ok_or_else(|| format!("internal: window 0x{w:08X} of a written half was not read"))?;
        if sendable(w, data).iter().any(|&b| b != 0xFF) {
            plans.push(BankPlan { base: w, original: data.clone(), patched: data.clone() });
        }
    }
    plans.sort_by_key(|p| p.base);
    Ok(plans)
}

// ============================================================
// Write-window guard
// ============================================================

/// The 0x4000 windows a D578 program, settings write or restore owns, by name.
/// Anything else is refused unless it is carried unchanged beside an owned
/// window ([`with_carried_windows`]) — a backup from another radio, or a bug,
/// cannot reach the call-sign database or the VFOs' own regions this way.
pub(crate) fn writable_window_name(addr: u32) -> Option<&'static str> {
    let win = WRITE_WINDOW as u32;
    if !addr.is_multiple_of(win) {
        return None;
    }
    let run = |base: u32, n: u32| addr >= base && addr < base + n * win;
    if addr >= CHANNEL_BASE
        && (addr - CHANNEL_BASE).is_multiple_of(BANK_STEP)
        && (addr - CHANNEL_BASE) / BANK_STEP < NUM_BANKS as u32
    {
        return Some("channel bank");
    }
    if run(ZONE_LIST_BASE, (MAX_ZONES * ZONE_LIST_STEP as usize).div_ceil(WRITE_WINDOW) as u32) {
        return Some("zone member lists");
    }
    if addr >= SCAN_LIST_BASE && (addr - SCAN_LIST_BASE).is_multiple_of(BANK_STEP) {
        let bank = (addr - SCAN_LIST_BASE) / BANK_STEP;
        if (bank as usize) < MAX_SCAN_LISTS.div_ceil(SCAN_LISTS_PER_BANK) {
            return Some("scan lists");
        }
    }
    if addr == ZONE_NAME_BASE {
        return Some("zone names");
    }
    if addr == CHANNEL_BITMAP_BASE & !(win - 1) {
        return Some("present bitmaps");
    }
    if addr == settings::SETTINGS_WINDOW {
        return Some("settings");
    }
    if addr == AIR_BASE {
        return Some("air band");
    }
    if addr == settings::RADIO_ID_BASE {
        return Some("radio IDs");
    }
    for b in 0..MAX_CONTACTS.div_ceil(CONTACTS_PER_BANK) as u32 {
        let base = CONTACT_BASE + b * CONTACT_BANK_STEP;
        let windows = (CONTACTS_PER_BANK * CONTACT_REC_LEN).div_ceil(WRITE_WINDOW) as u32;
        if run(base, windows) {
            return Some("contacts");
        }
    }
    if run(CONTACT_INDEX_BASE, (MAX_CONTACTS * 4).div_ceil(WRITE_WINDOW) as u32) {
        return Some("contact index");
    }
    if addr == CONTACT_BITMAP_BASE {
        return Some("contact bitmap");
    }
    if run(CONTACT_ID_TABLE, ((MAX_CONTACTS + 1) * CONTACT_ID_ENTRY).div_ceil(WRITE_WINDOW) as u32) {
        return Some("contact ID table");
    }
    None
}

/// Refuse a restore image unless every block is whole aligned windows in flash
/// halves this driver writes: a window it owns, or a window carried unchanged
/// because it shares a half with one ([`with_carried_windows`]).
pub(crate) fn check_restore_blocks(blocks: &[(u32, Vec<u8>)]) -> Result<(), String> {
    let mut owned_halves = BTreeSet::new();
    for (addr, data) in blocks {
        for w in 0..data.len().div_ceil(WRITE_WINDOW) {
            let a = addr + (w * WRITE_WINDOW) as u32;
            if writable_window_name(a).is_some() {
                owned_halves.insert(half_of(a));
            }
        }
    }
    for (addr, data) in blocks {
        // A window that ends its half is stored without the marker frame.
        let len = if data.len() % WRITE_WINDOW == WRITE_WINDOW - MARKER_FRAME
            && addr + data.len() as u32 + MARKER_FRAME as u32 == half_of(*addr) + FLASH_HALF
        {
            data.len() + MARKER_FRAME
        } else {
            data.len()
        };
        if !addr.is_multiple_of(WRITE_WINDOW as u32) || len % WRITE_WINDOW != 0 {
            return Err(format!(
                "block 0x{addr:08X}+0x{:X} is not 0x4000-aligned — not a program backup",
                data.len()
            ));
        }
        for w in 0..len / WRITE_WINDOW {
            let a = addr + (w * WRITE_WINDOW) as u32;
            if writable_window_name(a).is_none() && !owned_halves.contains(&half_of(a)) {
                return Err(format!(
                    "block 0x{a:08X} is not a region this app writes on a D578UV — refusing to \
                     restore it"
                ));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod hw_ladder;
