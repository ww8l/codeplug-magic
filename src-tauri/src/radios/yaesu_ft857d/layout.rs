//! FT-857D / FT-897 clone-image layout. Constants only.
//!
//! Every address here is CHIRP's (`ft857.py`, which drives both radios from one
//! class) and has been checked against three clone images read off a real
//! **FT-897** (US) on 2026-10-17 — not an FT-857D, whose CAT port is dead (see
//! `scratchpad/yaesu_ft857d/FINDINGS.md`). What that check proved, and what it
//! did not:
//!
//! - the block table and image lengths: all 53 blocks of three reads, every
//!   checksum good;
//! - the memory table, its 28-byte stride and both bitmaps: 17 memories decode
//!   to a coherent band-plan set and the bitmaps mark exactly those 17;
//! - the 60 m block's position (US image only): five channels at 5.332-5.405 MHz.
//!
//! The FT-857D is the same layout **by CHIRP's say-so** until an image comes off
//! one.

/// A non-US image: everything up to the 60 m channels.
pub(crate) const IMAGE_LEN_EXPORT: usize = 7341;
/// A US image: the radio sends one more 140-byte block, the five 60 m channels.
pub(crate) const IMAGE_LEN_US: usize = 7481;

/// The clone's block lengths in the order the radio sends them, for an export
/// radio. A US radio appends [`US_EXTRA_BLOCK`]. The repeated 140 is the memory
/// area, which CHIRP describes as "block 8 × 40" — 42 blocks really, the last
/// two belonging to the same run.
pub(crate) fn block_lengths(us: bool) -> Vec<usize> {
    let mut v = vec![2, 82, 252, 196, 252, 196, 212, 55];
    v.extend(std::iter::repeat_n(140, 42));
    v.extend([38, 176]);
    if us {
        v.push(US_EXTRA_BLOCK);
    }
    v
}

/// The 60 m block a US radio sends last (block 0x34).
pub(crate) const US_EXTRA_BLOCK: usize = 140;

/// One memory record. Same stride for regular memories, PMS, VFOs, HOME, QMB
/// and the 60 m channels.
pub(crate) const ENTRY_LEN: usize = 28;
/// Bytes of tag (name) at the end of a record.
pub(crate) const NAME_LEN: usize = 8;
/// Offset of the name within a record.
pub(crate) const NAME_OFFSET: usize = 20;

/// Regular memories 1-200.
pub(crate) const CHANNEL_COUNT: usize = 200;
pub(crate) const MEMORY_BASE: usize = 0x04DF;

/// One bit per regular memory, LSB first: bit `(n-1) % 8` of byte `(n-1) / 8`.
/// "Visible" is what the radio shows; "filled" is whether the record holds
/// data. An erased memory keeps its record and its filled bit and loses only
/// visible — CHIRP's `_set_normal` relies on that, and on real images the two
/// agree for every memory in use.
pub(crate) const VISIBLE_BASE: usize = 0x04A9;
pub(crate) const FILLED_BASE: usize = 0x04C4;
pub(crate) const BITMAP_LEN: usize = 25;

/// The ten PMS edges (P-1L/U … P-5L/U) follow the 200 memories. Their bitmaps are
/// a little-endian u16 after each regular bitmap. Not programmed by the app.
pub(crate) const PMS_COUNT: usize = 10;
pub(crate) const PMS_BASE: usize = MEMORY_BASE + CHANNEL_COUNT * ENTRY_LEN;

/// The five fixed US 60 m channels. Present only in a [`IMAGE_LEN_US`] image,
/// and never written by the app — CHIRP makes them immutable.
pub(crate) const SIXTY_M_BASE: usize = 0x1CAD;
pub(crate) const SIXTY_M_COUNT: usize = 5;

/// Offset of regular memory `slot` (0-based).
pub(crate) fn memory_offset(slot: usize) -> usize {
    MEMORY_BASE + slot * ENTRY_LEN
}

// The layout's own arithmetic, checked at compile time: no region overlaps
// the next, and the 60 m block is exactly the tail a US image adds.
const _: () = {
    assert!(VISIBLE_BASE + BITMAP_LEN + 2 <= FILLED_BASE);
    assert!(FILLED_BASE + BITMAP_LEN + 2 <= MEMORY_BASE);
    assert!(PMS_BASE + PMS_COUNT * ENTRY_LEN <= SIXTY_M_BASE);
    assert!(SIXTY_M_BASE == IMAGE_LEN_EXPORT);
    assert!(SIXTY_M_BASE + SIXTY_M_COUNT * ENTRY_LEN == IMAGE_LEN_US);
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn block_tables_sum_to_the_two_image_lengths() {
        assert_eq!(block_lengths(false).iter().sum::<usize>(), IMAGE_LEN_EXPORT);
        assert_eq!(block_lengths(true).iter().sum::<usize>(), IMAGE_LEN_US);
        // 53 blocks on the US radio — the count the FT-897 actually sent.
        assert_eq!(block_lengths(true).len(), 53);
    }

}
