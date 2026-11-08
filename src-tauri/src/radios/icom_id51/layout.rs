//! Where things are in an ID-51 Plus `.icf` image.
//!
//! Offsets are CHIRP's `id51plus.py` (`ID51PLUSRadio`, "Icom ID-51 Plus/50th
//! Anniversary"), **checked against a file the radio wrote** before a line of
//! the encoder existed: all 87 memories on Tim's radio decode to plausible
//! repeaters, and Icom's own CS-51PLUS shows the same values for every one of
//! them (`scratchpad/icom_id51/FINDINGS.md`, `MEASURED.md`). The record itself is
//! the ID-31 lineage's 49 bytes, the same codec as the ID-5100's.
//!
//! | offset | shape | what |
//! |---|---|---|
//! | `0x0000` | 500 x 49 | memory records |
//! | `0x6A40` | 63 bytes used | "empty" bitmap, LSB-first — bit SET = slot empty |
//! | `0x6A86` | 63 bytes used | skip bitmap |
//! | `0x6ACB` | 63 bytes used | P-skip bitmap |
//! | `0x6B40` | 500 x 2 | `[mult: bits 5-6][bank: bits 0-4][index in bank]` |
//! | `0x6FD0` | 26 x 16 | bank names A-Z, space padded |
//! | `0x1A000`-`0x1AF00` | | menu settings ([`super::settings`]) and the radio's working state |

/// The decoded image, `0x1FB40` bytes — CHIRP's `_memsize`, and exactly what
/// the radio's own file decodes to.
pub(crate) const IMAGE_LEN: usize = 0x1FB40;

pub(crate) const REC_LEN: usize = 49;
/// Programmable memories 0-499. The radio's 50 scan edges and 4 call channels
/// live elsewhere and are not touched.
pub(crate) const CHANNEL_COUNT: usize = 500;

pub(crate) const EMPTY_BITMAP: usize = 0x6A40;
pub(crate) const SKIP_BITMAP: usize = 0x6A86;
pub(crate) const PSKIP_BITMAP: usize = 0x6ACB;
/// Bytes of each bitmap that carry a memory's bit (500 / 8, rounded up).
pub(crate) const BITMAP_LEN: usize = 63;

pub(crate) const BANK_TABLE: usize = 0x6B40;
pub(crate) const BANK_NAMES: usize = 0x6FD0;
pub(crate) const BANK_NAME_LEN: usize = 16;
pub(crate) const BANK_COUNT: usize = 26;
/// Memories one bank can index (`00`-`99`).
pub(crate) const BANK_CAPACITY: usize = 100;
/// The bank number that means "in no bank". Every empty slot on Tim's radio
/// holds `1F 00`.
pub(crate) const NO_BANK: u8 = 0x1F;

pub(crate) const NAME_LEN: usize = 16;

/// What the receiver covers, in MHz: the A/B-band ranges of the USA version
/// (Advanced Manual p. 19-1, "RX 108~174, 380~479"), which CHIRP's
/// `valid_bands` repeats. The broadcast receiver (AM/FM BC radio) is a separate
/// receiver with its own memories, not a memory band.
///
/// A memory outside this is refused by the codeplug build rather than written:
/// the ID-52 dropped such memories silently while the app reported them
/// written, and the ID-5100 refused a whole clone over one and factory-reset.
/// What the ID-51 does with one in a loaded file is unmeasured, and does not
/// need to be.
pub(crate) const RX_COVERAGE: [(f64, f64); 2] = [(108.0, 174.0), (380.0, 479.0)];

pub(crate) const fn record_offset(slot: usize) -> usize {
    slot * REC_LEN
}

// The tables sit where CHIRP puts them and the radio's file shows them, with
// the record array ending before the first bitmap.
const _: () = assert!(CHANNEL_COUNT * REC_LEN <= EMPTY_BITMAP);
const _: () = assert!(BITMAP_LEN * 8 >= CHANNEL_COUNT);
const _: () = assert!(EMPTY_BITMAP + BITMAP_LEN <= SKIP_BITMAP);
const _: () = assert!(SKIP_BITMAP + BITMAP_LEN <= PSKIP_BITMAP);
const _: () = assert!(PSKIP_BITMAP + BITMAP_LEN <= BANK_TABLE);
const _: () = assert!(BANK_TABLE + CHANNEL_COUNT * 2 <= BANK_NAMES);
const _: () = assert!(REC_LEN == crate::radios::icom_id5100::layout::REC_LEN);
