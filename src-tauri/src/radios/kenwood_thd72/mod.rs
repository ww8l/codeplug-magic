//! Kenwood TH-D72 driver (`driver_key = "kenwood_thd72"`, issue #55).
//!
//! **Phase 2 only: the container and the memory encoder.** Nothing here speaks
//! to a radio yet, and the driver is deliberately not in `registry.rs` — seeding
//! the model and claiming capabilities is Phase 3, after the byte-identical
//! re-encode gate passes.
//!
//! The TH-D72 is a 2010 dual-band HT with a built-in mini-USB port. It is
//! programmed by cloning a **64 KiB image** over that port — the same modality
//! as the UV-5R and TD-H3, not the microSD patching the TH-D75 uses, and not the
//! live ASCII commands the TM-D710 needs. The radio *also* answers live commands
//! (`ME`, `MN`, `MU`, `PV`, `TY`), which makes it the only radio here that can be
//! asked what a write actually landed; that is a Phase 5 instrument, not a
//! programming path.
//!
//! - `layout`    — offsets, and the programmable-VFO rule that decides whether a
//!   channel can transmit. Read its header before touching either module below.
//! - `container` — the image itself: parsing, the MCP-4A `.mc4` wrapper, the
//!   model guard, and which 256-byte blocks an upload is allowed to write.
//! - `memory`    — channel records, name cells, flag records and group names.
//!
//! Everything was checked against eight real clone images pulled out of CHIRP's
//! bug tracker before it was written; `scratchpad/kenwood_thd72/FINDINGS.md` has
//! the decode and the grading. Phase 2's gate runs over those images in
//! `real_images.rs`: 1164 memories from three radios, every one re-encoding
//! byte-identically and writing back without dirtying a block.
//!
//! ⚠ They are other people's radios. What they cannot settle — this radio's
//! variant, its firmware, its own Menu 130 edits, and whether anything
//! checksums the regions we do *not* write — is listed at the end of that file
//! and waits for the hardware ladder.

// Phase 2 builds the pieces; Phase 3 is what calls them. Until the export path
// is wired, every public item here is reached only from tests. Remove this the
// moment the driver is registered — an "unused encoder" warning is a bug report
// in this codebase, and one was how a dead write path got caught before.
#![allow(dead_code)]

pub(crate) mod container;
pub(crate) mod layout;
pub(crate) mod memory;
#[cfg(test)]
mod real_images;
