//! Where things live in the MD-380's 256 KiB codeplug image.
//!
//! Offsets are image offsets (an `.rdt` adds a 0x225-byte header in front).
//! Farnsworth's `codeplugs.json`, dmrconfig's `md380.c` and qdmr agree on every
//! one of them; decoding Tim's radio with them gave the values his own screen
//! shows (s136: intro lines, radio ID and name, 24 zones, a channel's
//! frequencies, colour code and slot).

pub(crate) const IMAGE_LEN: usize = 0x40000;

/// General settings (radio ID, name, intro lines, timers…), 0x90 bytes. The
/// settings blocks after it — menu flags 0x20F0, side buttons 0x2102, text
/// messages 0x2180 — arrive with the settings schema.
#[cfg(test)]
pub(crate) const GENERAL_SETTINGS: usize = 0x2040;

/// One Touch Access, 6 × 4 bytes: byte 0 = mode (bits 7..2: 48 None, 52
/// Digital, 58 Analog) and call type (1..0), byte 1 a text message, bytes 2-3
/// the contact (u16 LE, 1-based). Number Keys, 10 × u16 LE contact indices.
/// Both point at contacts BY INDEX, so a program that rebuilds the contact
/// table must remap them (editcp `OneTouch` / `NumberKey`).
pub(crate) const ONE_TOUCH: usize = 0x2114;
pub(crate) const ONE_TOUCH_COUNT: usize = 6;
pub(crate) const NUMBER_KEYS: usize = 0x212C;
pub(crate) const NUMBER_KEY_COUNT: usize = 10;

/// The radio's current zone, 1-based, written by the radio itself (qdmr's
/// "boot settings" area); 0xFF on a CPS-written file. Measured s136 at three
/// zones Tim selected — 4, 2 and 23 (`RMH MISC`) — each read back as that
/// number. A program that left it at 23 over a 15-zone codeplug was clamped to
/// 1, and the radio came up on zone 1 (`NOCO DMR-GMRS 1`) normally.
pub(crate) const CURRENT_ZONE: usize = 0x2F003;

pub(crate) const CONTACTS: usize = 0x5F80;
pub(crate) const CONTACT_LEN: usize = 36;
pub(crate) const CONTACT_COUNT: usize = 1000;

pub(crate) const RX_GROUPS: usize = 0xEC20;
pub(crate) const RX_GROUP_LEN: usize = 96;
pub(crate) const RX_GROUP_COUNT: usize = 250;

pub(crate) const ZONES: usize = 0x149E0;
pub(crate) const ZONE_LEN: usize = 64;
pub(crate) const ZONE_COUNT: usize = 250;
pub(crate) const ZONE_MEMBERS: usize = 16;

pub(crate) const SCAN_LISTS: usize = 0x18860;
pub(crate) const SCAN_LIST_LEN: usize = 104;
pub(crate) const SCAN_LIST_COUNT: usize = 250;
pub(crate) const SCAN_LIST_MEMBERS: usize = 31;

pub(crate) const CHANNELS: usize = 0x1EE00;
pub(crate) const CHANNEL_LEN: usize = 64;
pub(crate) const CHANNEL_COUNT: usize = 1000;

/// Names are UTF-16LE, 16 code units, NUL-padded.
pub(crate) const NAME_UNITS: usize = 16;

/// Offset of record `i` (0-based) in a table.
pub(crate) const fn at(base: usize, len: usize, i: usize) -> usize {
    base + i * len
}

const _: () = assert!(CHANNELS + CHANNEL_COUNT * CHANNEL_LEN <= IMAGE_LEN);
const _: () = assert!(ZONES + ZONE_COUNT * ZONE_LEN <= SCAN_LISTS);
const _: () = assert!(SCAN_LISTS + SCAN_LIST_COUNT * SCAN_LIST_LEN <= CHANNELS);
const _: () = assert!(RX_GROUPS + RX_GROUP_COUNT * RX_GROUP_LEN <= ZONES);
const _: () = assert!(CONTACTS + CONTACT_COUNT * CONTACT_LEN <= RX_GROUPS);
