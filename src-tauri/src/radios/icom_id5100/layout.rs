//! Where things are in an ID-5100 clone image.
//!
//! Offsets come from CHIRP's `id5100.py` (which inherits its ID-4100 layout)
//! and were **checked against this radio's own clone reads** before a line of
//! the encoder existed — `scratchpad/id5100/FINDINGS.md`. 41 real memories in
//! two named banks decoded with every field plausible, three of them were read
//! back off the radio's screen, and a front-panel-programmed probe landed in the
//! slot, bitmap bit and fields predicted. Two things CHIRP does not know were
//! found on the way and are documented where they are used: the bank byte's
//! upper bits, and the record byte at `+0x09`.
//!
//! ## One image layout, by firmware
//!
//! The ID-5100 changed its memory layout twice with firmware (CHIRP's MapRev
//! 1/2/3, and the radio's own `SD Card > Save Form` offers "Now Ver", "Old Ver
//! (1.10)" and "Old Ver (1.00-1.05)"). This driver knows **MapRev 3 only** — the
//! current firmware, measured three ways on a radio running CPU 1.21: the ID
//! reply's revision byte is `3`, the clone ends with `Icom Inc.8E`, and the
//! image is exactly [`IMAGE_LEN`] bytes. An older radio is refused at identify
//! with a message saying so, rather than being written at the wrong offsets.
//!
//! ## Map
//!
//! | offset | shape | what |
//! |---|---|---|
//! | `0x0000` | 1004 x 49 | records: 1000 memories, then 4 call channels |
//! | `0xC040` | 125 bytes | "empty" bitmap, LSB-first — bit SET = slot empty |
//! | `0xC0BE` | 125 bytes | skip bitmap |
//! | `0xC13B` | 125 bytes | P-skip bitmap |
//! | `0xC1C0` | 1000 x 2 | `[bank byte][index in bank]` per memory |
//! | `0xC9C0` | 16 chars | an ICF comment field (spaces on this radio) |
//! | `0xC9D0` | 26 x 16 | bank names A-Z, space padded |
//! | `0x23E00+` | | RX history and other state the radio rewrites continuously |

/// The whole clone image, MapRev 3.
pub(crate) const IMAGE_LEN: usize = 0x2A380;
/// MapRev this driver was measured against.
pub(crate) const MAP_REV: u8 = 3;
/// Model code in the clone ID reply and the clone frames.
pub(crate) const MODEL: [u8; 4] = [0x34, 0x84, 0x00, 0x01];
/// What the radio sends in CLONE_END for a MapRev 3 image, and what a write has
/// to finish with. CHIRP's table, and what the radio sent on every read here.
pub(crate) const ENDFRAME: &[u8] = b"Icom Inc.8E";

pub(crate) const REC_LEN: usize = 49;
/// Programmable memories 0-999.
pub(crate) const CHANNEL_COUNT: usize = 1000;
/// Memories plus the four call channels (144-C0/C1, 430-C0/C1) at 1000-1003.
pub(crate) const RECORD_COUNT: usize = 1004;

pub(crate) const EMPTY_BITMAP: usize = 0xC040;
pub(crate) const SKIP_BITMAP: usize = 0xC0BE;
pub(crate) const PSKIP_BITMAP: usize = 0xC13B;
pub(crate) const BITMAP_LEN: usize = 125;

pub(crate) const BANK_TABLE: usize = 0xC1C0;
pub(crate) const BANK_NAMES: usize = 0xC9D0;
pub(crate) const BANK_NAME_LEN: usize = 16;
pub(crate) const BANK_COUNT: usize = 26;
/// Memories one bank can index (`00`-`99`).
pub(crate) const BANK_CAPACITY: usize = 100;
/// The bank number that means "in no bank".
pub(crate) const NO_BANK: u8 = 0x1F;

pub(crate) const NAME_LEN: usize = 16;

/// What the receiver covers, in MHz — the manual's USA specification, which
/// CHIRP's `valid_bands` repeats. The seed row's `rx_bands` must say the same
/// (`program::tests` holds them together).
///
/// ★★ This is a SAFETY limit, not a filter. Measured 2026-10-01: a clone
/// carrying memories at and just past these edges (117.975, 174.005, 374.995,
/// 550.005 among them) was REFUSED WHOLE by the radio — it showed an error and
/// came back FACTORY RESET. One bad memory does not get skipped; it costs the
/// operator every memory and setting they had. So the codeplug build refuses
/// anything outside this, whatever reached it.
pub(crate) const RX_COVERAGE: [(f64, f64); 2] = [(118.0, 174.0), (375.0, 550.0)];

/// Everything above this is state the radio rewrites on its own between two
/// reads with nothing touched — 282 bytes of it in one ten-minute noise floor,
/// all RX history and last-heard data. None of it is this driver's.
pub(crate) const VOLATILE_BASE: usize = 0x23E00;

pub(crate) const fn record_offset(slot: usize) -> usize {
    slot * REC_LEN
}

// The tables have to sit where CHIRP puts them and the reads showed them, with
// the record array ending before the first bitmap. A wrong REC_LEN or count
// would overrun into it.
const _: () = assert!(RECORD_COUNT * REC_LEN <= EMPTY_BITMAP);
const _: () = assert!(SKIP_BITMAP + BITMAP_LEN == PSKIP_BITMAP);
const _: () = assert!(BANK_TABLE + CHANNEL_COUNT * 2 <= 0xC9C0);
const _: () = assert!(BANK_NAMES + BANK_COUNT * BANK_NAME_LEN <= VOLATILE_BASE);
