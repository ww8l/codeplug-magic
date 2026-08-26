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
pub(crate) mod program;
pub(crate) mod protocol;
#[cfg(test)]
mod hw_phase1;
#[cfg(test)]
mod hw_phase1b;
#[cfg(test)]
mod real_images;

use crate::commands::export::SlotChannel;
use crate::models::RadioModel;
use crate::radios::driver::{
    with_restore_hint, CodeplugProgramReport, DecodedChannelSample, ImageProgramRequest,
    ImageProgrammer, ImageRestorer, RadioDriver, RadioIdentity,
};

use container::check_thd72_image;
use layout::{BLOCK_LEN, CALIBRATION_BASE, CHANNEL_COUNT, IMAGE_LEN};

/// The TH-D72 driver unit type. All protocol state lives on the serial port, so
/// one static instance serves the registry.
pub(crate) struct KenwoodThd72;

/// Registry entry (see `radios/registry.rs`).
pub(crate) static DRIVER: KenwoodThd72 = KenwoodThd72;

impl RadioDriver for KenwoodThd72 {
    fn key(&self) -> &'static str {
        "kenwood_thd72"
    }

    fn display_name(&self) -> &'static str {
        "Kenwood TH-D72"
    }

    /// The rate the radio answers `ID` on. The clone itself runs at
    /// [`protocol::BAUD_CLONE`] — `0M PROGRAM` is acknowledged at this speed and
    /// the port is re-rated immediately afterwards, so the two are not
    /// interchangeable and only the opening rate belongs here.
    fn baud(&self) -> u32 {
        protocol::BAUD_INITIAL
    }

    fn identify(&self, port: &str) -> Result<RadioIdentity, String> {
        let mut p = protocol::open_port(port)?;
        protocol::identify(&mut *p)
    }

    fn as_image_programmer(&self) -> Option<&dyn ImageProgrammer> {
        Some(self)
    }

    fn as_image_restorer(&self) -> Option<&dyn ImageRestorer> {
        Some(self)
    }

    // `CodeplugProgrammer` is deliberately absent: that trait is for radios
    // programmed by targeted record writes (the AnyTone). A clone radio patches
    // and re-uploads an image, which is `ImageProgrammer::program_codeplug`.
    //
    // Settings stay absent until Phase 4. The schema is seeded empty on
    // purpose: the 19 `MU` parameters have published enums, one of which
    // (audio balance) is already known to be contradicted between two sources,
    // and a guessed encoding is the one failure mode that writes a wrong value
    // to a real radio.
}

/// Which 256-byte blocks differ between the image the radio gave us and the one
/// we intend to write.
///
/// Derived from the bytes rather than carried along as bookkeeping. `patch()`
/// tracks dirty blocks correctly, but that state does not survive
/// [`ImageProgrammer::build_image`]'s `Vec<u8>` return, and a diff of the two
/// images cannot drift from what actually changed — which is the property the
/// partial upload depends on.
fn changed_blocks(base: &[u8], built: &[u8]) -> Vec<usize> {
    (0..IMAGE_LEN / BLOCK_LEN)
        .filter(|i| {
            let r = i * BLOCK_LEN..(i + 1) * BLOCK_LEN;
            base[r.clone()] != built[r]
        })
        .collect()
}

/// Every block this driver is allowed to write: all of them except the two the
/// radio keeps its own data in. See [`layout::CALIBRATION_BASE`].
fn writable_blocks() -> Vec<usize> {
    (0..CALIBRATION_BASE / BLOCK_LEN).collect()
}

impl ImageProgrammer for KenwoodThd72 {
    fn download_image(&self, port: &str) -> Result<(RadioIdentity, Vec<u8>), String> {
        let mut p = protocol::open_port(port)?;
        let ident = protocol::identify(&mut *p)?;
        let image = protocol::download(&mut *p)?;
        Ok((ident, image))
    }

    fn decode_sample(&self, image: &[u8]) -> Vec<DecodedChannelSample> {
        program::decode_sample(image)
    }

    /// Write a whole image back, minus the calibration blocks.
    ///
    /// Deliberately not `writable_blocks()`-minus-nothing: those two blocks hold
    /// per-radio data, and an image taken from a *different* TH-D72 carries that
    /// radio's copy. Restoring one unit's backup onto another is a normal thing
    /// to do after a bad write, and it must not carry the wrong unit's data with
    /// it.
    fn upload_image(&self, port: &str, image: &[u8]) -> Result<(), String> {
        check_thd72_image(image)?;
        let mut p = protocol::open_port(port)?;
        protocol::identify(&mut *p)?;
        protocol::upload(&mut *p, image, &writable_blocks())
    }

    fn build_image(
        &self,
        model: &RadioModel,
        channels: &[SlotChannel],
        base: &[u8],
    ) -> Result<Vec<u8>, String> {
        program::build_image(model, channels, base)
    }

    /// Full clone-mode program in ONE session: download and back up, patch the
    /// codeplug into *that* image, upload only the blocks that actually changed,
    /// then read back to verify.
    ///
    /// ⚠ **Never run against a radio.** Every step below is transcribed from
    /// CHIRP and cross-read against LA3QMA's command reference; the sequencing is
    /// exercised only by a fake port. This is Phase 5's job, and until it runs
    /// nothing here should be described as proven.
    ///
    /// `req.settings` is ignored: the TH-D72 has no settings support yet
    /// (Phase 4), and a channel program deliberately leaves the radio's own
    /// settings exactly as read.
    fn program_codeplug(
        &self,
        port: &str,
        req: &ImageProgramRequest,
    ) -> Result<CodeplugProgramReport, String> {
        if req.channels.len() > CHANNEL_COUNT {
            return Err(format!(
                "Codeplug has {} programmable channels, but the TH-D72 holds only {CHANNEL_COUNT}.",
                req.channels.len()
            ));
        }
        let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
        let slug = slug_label(req.label);
        let backup_path = req.backup_dir.join(if slug.is_empty() {
            format!("thd72-prewrite-{stamp}.img")
        } else {
            format!("thd72-prewrite-{slug}-{stamp}.img")
        });

        let mut p = protocol::open_port(port)?;

        // 1. Download + back up what the radio is holding now.
        protocol::identify(&mut *p)?;
        let base = protocol::download(&mut *p)?;
        std::fs::write(&backup_path, &base)
            .map_err(|e| format!("could not write backup {}: {e}", backup_path.display()))?;

        // 2. Patch the codeplug into THAT image, so every byte we are not
        //    responsible for goes back exactly as it came.
        let built = program::build_image(req.model, req.channels, &base)?;
        let blocks = changed_blocks(&base, &built);
        let channels_written = req.channels.len();

        // 3. Upload only what changed. Name the backup on every failure from
        //    here on: the operator is being sent to the Restore button and
        //    `radio-backups/` is a folder of similarly-named files.
        let restore_hint = |e: String| {
            with_restore_hint(
                e,
                &backup_path,
                "Keep that file. Put it back with \"Restore backup…\" in this dialog, \
                 which uploads it over the same cable — it is the only copy of what was \
                 on the radio before this write.",
            )
        };
        protocol::upload(&mut *p, &built, &blocks).map_err(restore_hint)?;

        // 4. Read back and verify. Non-fatal: every block was ack'd, so a failed
        //    read-back is a reporting problem, not a write problem.
        let (verified, note) = match protocol::download(&mut *p) {
            Ok(after) if after == built => (true, None),
            Ok(_) => (
                false,
                Some(
                    "Write completed and every block was acknowledged, but the read-back \
                     did not match. Power-cycle the radio and use Download to confirm what \
                     it is holding."
                        .to_string(),
                ),
            ),
            Err(e) => (
                false,
                Some(format!(
                    "Write completed, but read-back verification could not run ({e}). \
                     Power-cycle the radio and use Download to confirm."
                )),
            ),
        };

        Ok(CodeplugProgramReport {
            channels_written,
            slots_cleared: CHANNEL_COUNT - channels_written,
            settings_written: None,
            verified: Some(verified),
            note,
            backup_path: backup_path.to_string_lossy().to_string(),
            channels: program::decode_sample(&built),
            zones_written: 0,
            zones_cleared: 0,
            scan_lists_written: 0,
            scan_lists_cleared: 0,
            contacts_written: 0,
            contacts_cleared: 0,
            expected_path: None,
            windows_written: Vec::new(),
            skipped: Vec::new(),
            warnings: Vec::new(),
        })
    }
}

impl ImageRestorer for KenwoodThd72 {
    /// Checks SHAPE, not which unit the image came from — restoring a backup
    /// taken from another TH-D72 is a normal thing to do, and this is the path
    /// reached for after a bad write.
    fn check_restore_image(&self, image: &[u8]) -> Result<(), String> {
        check_thd72_image(image)
    }

    fn restore_image(&self, port: &str, image: &[u8]) -> Result<(), String> {
        self.upload_image(port, image)
    }
}

/// Filesystem-safe slug of a codeplug name, for the backup filename.
fn slug_label(label: &str) -> String {
    label
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_lowercase() } else { '-' })
        .collect::<String>()
        .trim_matches('-')
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_blocks_that_differ_are_uploaded() {
        let mut base = vec![0u8; IMAGE_LEN];
        let mut built = base.clone();
        built[0x1500] = 0x42; // block 0x15
        built[0x5E00] = 0x43; // block 0x5E
        assert_eq!(changed_blocks(&base, &built), vec![0x15, 0x5E]);
        base[0x1500] = 0x42;
        assert_eq!(changed_blocks(&base, &built), vec![0x5E]);
    }

    #[test]
    fn an_unchanged_image_uploads_nothing() {
        let base = vec![0u8; IMAGE_LEN];
        assert!(changed_blocks(&base, &base.clone()).is_empty());
    }

    /// The two blocks the radio keeps its own data in are outside every write
    /// this driver makes, restore included.
    #[test]
    fn the_calibration_blocks_are_never_writable() {
        let blocks = writable_blocks();
        assert_eq!(blocks.len(), 254);
        assert_eq!(*blocks.last().unwrap(), 253);
        assert!(!blocks.contains(&254) && !blocks.contains(&255));
    }

    #[test]
    fn a_codeplug_name_becomes_a_safe_filename() {
        assert_eq!(slug_label("Dayton 2026!"), "dayton-2026");
        assert_eq!(slug_label("   "), "");
    }
}
