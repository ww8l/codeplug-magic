//! TH-D72 image layout: where everything lives, and the one rule that decides
//! whether a channel can transmit.
//!
//! Offsets come from CHIRP's `chirp/drivers/thd72.py` memory map and were
//! checked against eight real 65536-byte clone images before any of this was
//! written (`scratchpad/kenwood_thd72/FINDINGS.md`). Three of those images are a
//! controlled series off one radio — factory reset, then one channel entered
//! from the front panel, then a second — so the channel record, the name cell
//! and the flag record are confirmed against bytes a TH-D72 wrote itself.
//!
//! ## The programmable-VFO trap
//!
//! Every memory carries a band index in the low nibble of its flag record. If it
//! disagrees with the channel's own frequency, the radio stores the channel,
//! displays it, receives on it — and **refuses to transmit**. This is not
//! hypothetical: a user codeplug in CHIRP's bug tracker has all 187 of its
//! memories claiming the 410-470 MHz band, which is why its 2 m repeaters were
//! dead while 70 cm worked.
//!
//! CHIRP matches frequencies against a hard-coded default table. That is wrong
//! for any radio whose operator has used Menu 130, which edits these very
//! ranges — so [`read_prog_vfo_table`] takes them out of the image being
//! patched, and [`prog_vfo_index`] is the only way a flag nibble is chosen.

/// A TH-D72 clone image is exactly 64 KiB. Nothing shorter is one.
pub(crate) const IMAGE_LEN: usize = 0x1_0000;

/// The clone protocol moves 256-byte blocks, and an upload can write any subset
/// of them — which is how this driver avoids touching regions it does not own.
pub(crate) const BLOCK_LEN: usize = 0x100;
pub(crate) const BLOCK_COUNT: usize = IMAGE_LEN / BLOCK_LEN;

/// MCP-4A `.mc4` files are this header followed by the same 64 KiB image.
pub(crate) const MC4_HEADER_LEN: usize = 0x100;

/// Six `(start, end)` little-endian `u32` Hz pairs — the programmable VFO
/// ranges, as *this* radio holds them.
pub(crate) const PROG_VFO_BASE: usize = 0x02C0;
pub(crate) const PROG_VFO_COUNT: usize = 6;

/// Menu settings block (`power_on_msg`, lamp, contrast, battery saver, APO,
/// key beep, balance). Decoded in Phase 4, not here.
pub(crate) const SETTINGS_BASE: usize = 0x0300;

/// Two bytes per memory: byte 0 is `disabled` (high nibble) + `prog_vfo` (low
/// nibble), byte 1 is the scan lockout.
pub(crate) const FLAG_BASE: usize = 0x0C00;
pub(crate) const FLAG_LEN: usize = 2;

/// 16 bytes per memory. See `memory.rs` for the field breakdown.
pub(crate) const MEMORY_BASE: usize = 0x1500;
pub(crate) const ENTRY_LEN: usize = 16;

/// 8-char memory names, `0xFF`-padded.
pub(crate) const NAME_BASE: usize = 0x5E00;
pub(crate) const NAME_LEN: usize = 8;

/// Ten 8-char memory-group names. Groups are fixed positional blocks of 100
/// memories — group *n* is memories `n * 100 ..= n * 100 + 99` — so a group is
/// named, never populated by a membership flag.
pub(crate) const GROUP_NAME_BASE: usize = 0x7ED0;
pub(crate) const GROUP_COUNT: usize = 10;
pub(crate) const GROUP_SIZE: usize = 100;

/// Programmable memories: 0-999. Above them sit the scan limit pairs
/// (L0/U0-L9/U9 at 1000-1019), the weather channels (1020-1029) and the two
/// call channels (1030-1031), none of which this driver writes.
pub(crate) const CHANNEL_COUNT: usize = GROUP_COUNT * GROUP_SIZE;

/// The last two blocks are **per-radio data** — they are not `0xFF` fill and
/// they differ between every image examined, which is almost certainly
/// calibration. CHIRP never writes them and neither does this driver.
pub(crate) const CALIBRATION_BASE: usize = 0xFE00;

/// `disabled` nibble value for a memory the radio considers empty. A
/// radio-authored empty flag record is `FF FF`.
pub(crate) const FLAG_EMPTY: u8 = 0xFF;

/// One programmable-VFO band, as stored at [`PROG_VFO_BASE`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ProgVfoBand {
    pub start_hz: u32,
    pub end_hz: u32,
}

pub(crate) type ProgVfoTable = [ProgVfoBand; PROG_VFO_COUNT];

/// The factory-default table, decoded out of `000-factory-reset.img`. Index
/// order is band A (0-2) then band B (3-5), which independently matches
/// LA3QMA's `PV.md`.
///
/// ⚠ Reference and test material only. Never encode against this — Menu 130
/// lets the operator move these edges, and the image carries what they chose.
/// Reaching for this constant instead of [`read_prog_vfo_table`] reintroduces
/// exactly the bug this module exists to avoid.
#[cfg(test)]
pub(crate) const DEFAULT_PROG_VFO: ProgVfoTable = [
    ProgVfoBand { start_hz: 136_000_000, end_hz: 174_000_000 },
    ProgVfoBand { start_hz: 410_000_000, end_hz: 470_000_000 },
    ProgVfoBand { start_hz: 118_000_000, end_hz: 136_000_000 },
    ProgVfoBand { start_hz: 136_000_000, end_hz: 174_000_000 },
    ProgVfoBand { start_hz: 320_000_000, end_hz: 400_000_000 },
    ProgVfoBand { start_hz: 400_000_000, end_hz: 524_000_000 },
];

/// Read the six programmable-VFO ranges out of the image being patched.
pub(crate) fn read_prog_vfo_table(image: &[u8]) -> Result<ProgVfoTable, String> {
    let end = PROG_VFO_BASE + PROG_VFO_COUNT * 8;
    if image.len() < end {
        return Err(format!(
            "image is {} bytes — too short to hold the programmable-VFO table at {PROG_VFO_BASE:#06X}",
            image.len()
        ));
    }
    let mut table = [ProgVfoBand { start_hz: 0, end_hz: 0 }; PROG_VFO_COUNT];
    for (i, band) in table.iter_mut().enumerate() {
        let off = PROG_VFO_BASE + i * 8;
        band.start_hz = u32::from_le_bytes(image[off..off + 4].try_into().unwrap());
        band.end_hz = u32::from_le_bytes(image[off + 4..off + 8].try_into().unwrap());
    }
    Ok(table)
}

/// Pick the flag nibble for a frequency: the first band that contains it.
///
/// "First" is not arbitrary. Bands 0 and 3 are both 136-174 MHz by default (band
/// A and band B), and the radio itself writes **0** for a 2 m memory entered from
/// the front panel — confirmed in `002-set-channel-1-from-radio.img`, where
/// 144.000 MHz carries flag byte `0x00`. Matching in index order reproduces what
/// the radio does.
///
/// `None` means no band covers the frequency, and the caller must refuse to
/// write the channel rather than guess a nibble. A guessed one is the silent
/// no-TX failure this module is named for.
pub(crate) fn prog_vfo_index(table: &ProgVfoTable, hz: u32) -> Option<u8> {
    table
        .iter()
        .position(|b| hz >= b.start_hz && hz < b.end_hz)
        .map(|i| i as u8)
}

/// Which memory group a channel number falls in. Positional, not a flag.
pub(crate) fn group_of(channel: usize) -> usize {
    channel / GROUP_SIZE
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The 48 bytes at 0x02C0 of `000-factory-reset.img`, verbatim.
    const REAL_PROG_VFO_BYTES: [u8; 48] = [
        0x00, 0x32, 0x1b, 0x08, 0x80, 0x07, 0x5f, 0x0a, 0x80, 0x1a, 0x70, 0x18, 0x80, 0xa1, 0x03,
        0x1c, 0x80, 0x89, 0x08, 0x07, 0x00, 0x32, 0x1b, 0x08, 0x00, 0x32, 0x1b, 0x08, 0x80, 0x07,
        0x5f, 0x0a, 0x00, 0xd0, 0x12, 0x13, 0x00, 0x84, 0xd7, 0x17, 0x00, 0x84, 0xd7, 0x17, 0x00,
        0x9b, 0x3b, 0x1f,
    ];

    fn image_with_real_table() -> Vec<u8> {
        let mut image = vec![0xFFu8; IMAGE_LEN];
        image[PROG_VFO_BASE..PROG_VFO_BASE + REAL_PROG_VFO_BYTES.len()]
            .copy_from_slice(&REAL_PROG_VFO_BYTES);
        image
    }

    #[test]
    fn a_real_factory_image_decodes_to_the_documented_ranges() {
        let table = read_prog_vfo_table(&image_with_real_table()).unwrap();
        assert_eq!(table, DEFAULT_PROG_VFO);
    }

    #[test]
    fn two_metres_takes_band_a_the_way_the_radio_writes_it() {
        // 002-set-channel-1-from-radio.img: 144.000 MHz, flag byte 0x00.
        let table = read_prog_vfo_table(&image_with_real_table()).unwrap();
        assert_eq!(prog_vfo_index(&table, 144_000_000), Some(0));
        assert_eq!(prog_vfo_index(&table, 144_025_000), Some(0));
    }

    #[test]
    fn seventy_centimetres_and_the_air_band_land_where_they_should() {
        let table = read_prog_vfo_table(&image_with_real_table()).unwrap();
        assert_eq!(prog_vfo_index(&table, 442_575_000), Some(1));
        assert_eq!(prog_vfo_index(&table, 121_500_000), Some(2));
        assert_eq!(prog_vfo_index(&table, 350_000_000), Some(4));
    }

    /// The gap between 174 and 320 MHz, and everything past 524, belong to no
    /// band. A caller that invents a nibble here ships a channel that cannot
    /// transmit, which is the whole reason this returns an Option.
    #[test]
    fn a_frequency_in_no_band_has_no_index() {
        let table = read_prog_vfo_table(&image_with_real_table()).unwrap();
        assert_eq!(prog_vfo_index(&table, 220_000_000), None);
        assert_eq!(prog_vfo_index(&table, 530_000_000), None);
        assert_eq!(prog_vfo_index(&table, 100_000_000), None);
    }

    /// An operator who narrows Menu 130 changes the answer. This is the case
    /// CHIRP's hard-coded table gets wrong, so it is asserted rather than
    /// described.
    #[test]
    fn an_edited_table_moves_the_index() {
        let mut image = image_with_real_table();
        // Narrow band A's first range to 144-146 MHz.
        image[PROG_VFO_BASE..PROG_VFO_BASE + 4].copy_from_slice(&144_000_000u32.to_le_bytes());
        image[PROG_VFO_BASE + 4..PROG_VFO_BASE + 8].copy_from_slice(&146_000_000u32.to_le_bytes());
        let table = read_prog_vfo_table(&image).unwrap();
        // 145 still fits band A; 147 has fallen out of it into band B's 136-174.
        assert_eq!(prog_vfo_index(&table, 145_000_000), Some(0));
        assert_eq!(prog_vfo_index(&table, 147_015_000), Some(3));
    }

    #[test]
    fn a_short_image_is_refused_rather_than_indexed() {
        assert!(read_prog_vfo_table(&[0u8; 16]).is_err());
    }

    #[test]
    fn groups_are_hundred_channel_blocks() {
        assert_eq!(group_of(0), 0);
        assert_eq!(group_of(99), 0);
        assert_eq!(group_of(100), 1);
        assert_eq!(group_of(CHANNEL_COUNT - 1), GROUP_COUNT - 1);
    }
}
