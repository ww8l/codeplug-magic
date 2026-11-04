//! Yaesu FT-857D and FT-897 (issue #145): one driver, because the two radios
//! share one clone protocol and one memory map — CHIRP drives both from a single
//! `FT857Radio` class, and three FT-897 reads matched it block for block.
//!
//! The first radio here whose clone is driven from its own front panel: the
//! operator presses [C] SEND for every read and [A] RCV before every write, and
//! the app tells them when through [`crate::radios::operator`]. A program run is
//! therefore three operator steps — SEND (backup), RCV + Continue (write), SEND
//! (read-back) — inside one session under one port lock.
//!
//! ⚠ Hardware status:
//! - every image this was built against came off an **FT-897** (US). The
//!   FT-857D on hand has a dead CAT/LINEAR TXD line (0 V; it sends nothing in
//!   CLONE or CAT) — `scratchpad/yaesu_ft857d/FINDINGS.md`. Its layout is the
//!   same by CHIRP's account and unmeasured here;
//! - the clone **read** is proven on the FT-897; the **write** has never run
//!   against a radio (ladder steps 1-2);
//! - FM tone/DCS/shift encoding is CHIRP's and unseen in a radio-written record.

#[cfg(test)]
mod hw_ladder;
mod ft857_settings_table;
pub(crate) mod layout;
pub(crate) mod memory;
pub(crate) mod program;
pub(crate) mod protocol;
pub(crate) mod settings;
#[cfg(test)]
mod real_images;

use crate::commands::export::SlotChannel;
use crate::models::RadioModel;
use crate::radios::driver::{
    with_restore_hint, CodeplugProgramReport, DecodedChannelSample, ImageProgramRequest,
    ImageProgrammer, ImageReader, ImageRestorer, RadioDriver, RadioIdentity,
};
use crate::radios::operator;
use layout::{CHANNEL_COUNT, IMAGE_LEN_US};
use protocol::{CLONE_MODE_STEP, PATIENCE, RCV_STEP, SEND_STEP};

pub(crate) struct YaesuFt857d;

/// Registry entry (see `radios/registry.rs`).
pub(crate) static DRIVER: YaesuFt857d = YaesuFt857d;

impl RadioDriver for YaesuFt857d {
    fn key(&self) -> &'static str {
        "yaesu_ft857d"
    }

    fn display_name(&self) -> &'static str {
        "Yaesu FT-857D / FT-897"
    }

    fn baud(&self) -> u32 {
        protocol::BAUD
    }

    /// Over CAT, with the radio switched on normally — the clone has no
    /// handshake, and this reads nothing but the displayed frequency.
    fn identify(&self, port: &str) -> Result<RadioIdentity, String> {
        let (baud, reply) = protocol::cat_identify(port)?;
        Ok(RadioIdentity {
            matched: format!("FT-857/897 CAT at {baud}"),
            ident_hex: reply.iter().map(|b| format!("{b:02X}")).collect::<Vec<_>>().join(" "),
            ident_ascii: Some(protocol::cat_reply_text(&reply)),
        })
    }

    /// `memory.rs` writes every mode the channel library has an analog name for.
    fn analog_modes(&self) -> &'static [&'static str] {
        &["FM", "NFM", "AM", "USB", "LSB", "CW"]
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

    /// Settings are their own write — read, patch, write, read back — and a
    /// codeplug program leaves the radio's settings exactly as it read them.
    fn as_settings_writer(&self) -> Option<&dyn crate::radios::driver::SettingsWriter> {
        Some(self)
    }
}

/// Listen, ask for SEND, and take the image.
fn read_from_radio(port: &str, purpose: &str) -> Result<Vec<u8>, String> {
    let mut p = protocol::open_port(port)?;
    operator::say(&format!(
        "{purpose}: if the radio is not showing CLONE MODE — {CLONE_MODE_STEP}. Then \
         {SEND_STEP}. The app is listening."
    ));
    protocol::read_clone(&mut *p, PATIENCE, &mut |_, _| {})
}

/// Ask for RCV, wait for Continue, and send the image.
fn write_to_radio(port: &str, image: &[u8], purpose: &str) -> Result<(), String> {
    operator::wait(
        &format!(
            "{purpose}: if the radio is not showing CLONE MODE — {CLONE_MODE_STEP}. Then \
             {RCV_STEP} and click Continue."
        ),
        operator::WAIT_LIMIT,
    )?;
    let mut p = protocol::open_port(port)?;
    protocol::write_clone(&mut *p, image, PATIENCE, &mut |_, _| {})
}

fn identity_of(image: &[u8]) -> RadioIdentity {
    RadioIdentity {
        matched: "clone image".to_string(),
        ident_hex: image[..2].iter().map(|b| format!("{b:02X}")).collect::<Vec<_>>().join(" "),
        ident_ascii: Some(if image.len() == IMAGE_LEN_US { "US model" } else { "export model" }.into()),
    }
}

impl ImageReader for YaesuFt857d {
    fn download_image(&self, port: &str) -> Result<(RadioIdentity, Vec<u8>), String> {
        let image = read_from_radio(port, "Backup")?;
        Ok((identity_of(&image), image))
    }

    fn decode_sample(&self, image: &[u8]) -> Vec<DecodedChannelSample> {
        program::decode_sample(image)
    }
}

impl ImageProgrammer for YaesuFt857d {
    fn upload_image(&self, port: &str, image: &[u8]) -> Result<(), String> {
        write_to_radio(port, image, "Write")
    }

    fn build_image(
        &self,
        model: &RadioModel,
        channels: &[SlotChannel],
        base: &[u8],
    ) -> Result<Vec<u8>, String> {
        program::build_image(model, channels, base)
    }

    /// Back up, patch, write, read back — three trips to the radio's front
    /// panel, one session.
    ///
    /// ⚠ The write half has never run against a radio. Ladder steps 1-2.
    fn program_codeplug(
        &self,
        port: &str,
        req: &ImageProgramRequest,
    ) -> Result<CodeplugProgramReport, String> {
        let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
        let slug: String = req
            .label
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_lowercase() } else { '-' })
            .collect::<String>()
            .trim_matches('-')
            .to_string();
        let backup_path = req.backup_dir.join(if slug.is_empty() {
            format!("ft857d-prewrite-{stamp}.img")
        } else {
            format!("ft857d-prewrite-{slug}-{stamp}.img")
        });

        // 1. Back up what the radio holds now.
        let base = read_from_radio(port, "Programming, step 1 of 3 — first, a backup of the radio")?;
        std::fs::write(&backup_path, &base)
            .map_err(|e| format!("could not write backup {}: {e}", backup_path.display()))?;

        // 2. Patch the codeplug into THAT image — refused whole before the
        //    operator is asked for anything if a channel cannot be held.
        let built = program::build_image(req.model, req.channels, &base)?;

        // 3. Write.
        write_to_radio(port, &built, "Programming, step 2 of 3 — the write").map_err(|e| {
            with_restore_hint(
                e,
                &backup_path,
                "Keep that file. \"Restore backup…\" in this dialog sends it back over the same \
                 cable — it is the only copy of what was on the radio before this write.",
            )
        })?;

        // 4. Read back. Non-fatal: every block was acknowledged.
        let (verified, note) = match read_from_radio(port, "Programming, step 3 of 3 — checking the write") {
            Ok(after) if after == built => (true, None),
            Ok(after) => {
                let differ = after.iter().zip(&built).filter(|(a, b)| a != b).count();
                (
                    false,
                    Some(format!(
                        "Every block was acknowledged, but the read-back differs from what was \
                         written in {differ} bytes. Download again to see what the radio holds."
                    )),
                )
            }
            Err(e) => (
                false,
                Some(format!("Written, but the read-back check could not run ({e}).")),
            ),
        };

        Ok(CodeplugProgramReport {
            channels_written: req.channels.len(),
            slots_cleared: CHANNEL_COUNT - req.channels.len(),
            settings_written: None,
            verified: Some(verified),
            note,
            backup_path: backup_path.to_string_lossy().to_string(),
            channels: program::decode_sample(&built),
            zones_written: 0,
            zones_cleared: 0,
            banks_written: 0,
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

impl ImageRestorer for YaesuFt857d {
    /// Shape only — a backup from another FT-857D/897 is a legitimate restore.
    /// ⚠ What an export radio does with a US image's extra 60 m block, or a US
    /// radio with an image that lacks it, is unmeasured — nothing here can tell
    /// the radio's kind before the write starts.
    fn check_restore_image(&self, image: &[u8]) -> Result<(), String> {
        protocol::check_image(image)
    }

    fn restore_image(&self, port: &str, image: &[u8]) -> Result<(), String> {
        write_to_radio(port, image, "Restore")
    }
}
