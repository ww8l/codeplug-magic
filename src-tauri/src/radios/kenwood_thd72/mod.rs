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
pub(crate) mod settings;
pub(crate) mod protocol;
#[cfg(test)]
mod hw_phase1;
#[cfg(test)]
mod hw_phase1b;
#[cfg(test)]
mod hw_phase5;
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

    fn as_settings_reader(&self) -> Option<&dyn crate::radios::driver::SettingsReader> {
        Some(self)
    }

    fn as_settings_writer(&self) -> Option<&dyn crate::radios::driver::SettingsWriter> {
        Some(self)
    }

    // `CodeplugProgrammer` is deliberately absent: that trait is for radios
    // programmed by targeted record writes (the AnyTone). A clone radio patches
    // and re-uploads an image, which is `ImageProgrammer::program_codeplug`.
    //
    // Settings are read and written over the `MU` command rather than through
    // the clone image — `settings.rs` has the evidence and the reasoning. They
    // are a separate operation from a codeplug write, which deliberately leaves
    // the radio's own settings exactly as read.
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

/// Which of the blocks we wrote did not come back the way we sent them.
///
/// ★ Verification must be scoped to what we WROTE. Measured on a real TH-D72A,
/// 2026-08-26: after a codeplug write the radio rewrites 18 bytes of its own in
/// `0x0200-0x0400` — the current-channel and UI state — because the memories
/// underneath it changed. Those bytes are not ours, we never sent them, and a
/// whole-image `after == built` comparison would have reported `verified: false`
/// on every successful program the app ever ran.
///
/// Comparing only the uploaded blocks is also self-maintaining: it stays correct
/// if the set of regions this driver writes ever changes.
/// Bytes the radio rewrites on its OWN account after a write, measured on a real
/// TH-D72A on 2026-08-26 and again by Phase 1's noise floor: the last-used
/// channel at `0x0246`, the byte RT Systems blanks at `0x02BF`, and the block at
/// `0xA890`.
///
/// ⚠ Scoping the comparison to the blocks we wrote used to be enough, because
/// nothing this driver wrote lived in `0x0200-0x0400`. Since the settings work,
/// 31 image-backed settings DO live there — six in block 2 and twenty-five in
/// block 3 — so those blocks are now in `changed_blocks`, and the radio's own
/// rewrite would make them mismatch and report a successful write as failed.
const SELF_REWRITTEN: [std::ops::Range<usize>; 3] =
    [0x0246..0x0247, 0x02BF..0x02C0, 0xA890..0xA8C0];

fn mismatched_blocks(built: &[u8], after: &[u8], blocks: &[usize]) -> Vec<usize> {
    blocks
        .iter()
        .copied()
        .filter(|i| {
            let r = i * BLOCK_LEN..(i + 1) * BLOCK_LEN;
            let (Some(a), Some(b)) = (built.get(r.clone()), after.get(r.clone())) else {
                return true;
            };
            r.clone().zip(a.iter().zip(b.iter())).any(|(addr, (x, y))| {
                x != y && !SELF_REWRITTEN.iter().any(|w| w.contains(&addr))
            })
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
    /// `req.settings`, when the profile carries them, is applied to the image
    /// alongside the channels.
    ///
    /// ⚠⚠ This used to say `req.settings` is IGNORED, which was correct while
    /// the only transport was the `MU` command — a channel program writes an
    /// image and `MU` is not in it. Since s125 there are 103 settings that ARE
    /// image bytes, so ignoring them here would mean a profile whose settings
    /// the operator filled in, a form that reads them back correctly, and a
    /// program run that silently writes none of them. That is the dead write
    /// path this codebase has shipped twice, and it is the fourth box on the
    /// new-radio wiring checklist for exactly that reason.
    ///
    /// The 19 `MU` parameters are still NOT written here: they are not in the
    /// image, so a channel program leaves them as the radio holds them, and the
    /// profile editor's own write is what sets them.
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
        drop(p);

        // 2. Patch the codeplug into THAT image, so every byte we are not
        //    responsible for goes back exactly as it came.
        let mut built = program::build_image(req.model, req.channels, &base)?;
        // ⚠ Both return values are REPORTED. See `apply_image_settings`: this
        // used to drop them, so a run that wrote 103 settings said it wrote
        // none, and a value the radio could not take vanished without a word.
        let (settings_written, settings_notes) = match req.settings {
            Some((settings, schema_json)) => {
                let (n, notes) = settings::apply_image_settings(&mut built, settings, schema_json)?;
                (Some(n), notes)
            }
            None => (None, Vec::new()),
        };
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
        // ★ The reconnect is not optional and not politeness. Measured on a real
        //   TH-D72A on 2026-08-26: a clone session leaves the radio unreachable
        //   for several seconds after `E`, and a write attempted inside that
        //   window dies in `enter_program` with a bare "Broken pipe". This
        //   sequence — download, upload, read back — is three clone sessions and
        //   needs a reconnect between each. The ladder's step 1 failed three
        //   times on exactly this before it wrote a single byte.
        let mut p = protocol::reconnect_after_clone(port).map_err(restore_hint)?;
        protocol::upload(&mut *p, &built, &blocks).map_err(restore_hint)?;
        drop(p);

        // 4. Read back and verify. Non-fatal: every block was ack'd, so a failed
        //    read-back is a reporting problem, not a write problem.
        let reread = protocol::reconnect_after_clone(port)
            .and_then(|mut p| protocol::download(&mut *p));
        let (verified, note) = match reread {
            Ok(after) => match mismatched_blocks(&built, &after, &blocks) {
                bad if bad.is_empty() => (true, None),
                bad => (
                    false,
                    Some(format!(
                        "Write completed and every block was acknowledged, but {} of the {} \
                         blocks written did not read back the same. Power-cycle the radio and \
                         use Download to confirm what it is holding.",
                        bad.len(),
                        blocks.len()
                    )),
                ),
            },
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
            settings_written,
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
            warnings: settings_notes,
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
