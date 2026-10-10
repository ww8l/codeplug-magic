//! Icom ID-5100 driver (`driver_key = "icom_id5100"`, issue #49).
//!
//! A D-STAR dual-band mobile programmed over the data cable by Icom's clone
//! protocol: the whole 0x2A380-byte image is read, patched, and written back.
//! (The radio can also load an `.icf` from its SD card, the ID-52's path; the
//! cable was chosen instead.)
//!
//! - `layout`   — offsets and the one firmware layout this driver accepts.
//! - `memory`   — records, flag bitmaps, banks.
//! - `program`  — the codeplug build: channels + banks into a read image.
//! - `protocol` — the clone protocol. Download measured on the radio; upload
//!   is CHIRP's sequence, checked against a fake, and NOT yet run on hardware.
//!
//! Its memory layout is the ID-4100's lineage (CHIRP `id5100.py`), **not** the
//! ID-52's — 26 banks A-Z and 49-byte records, where the ID-52 has 100 groups
//! and 51. What it shares with the ID-52 is Icom's channel semantics, so the
//! channel normalisation (`duplex_and_offset`, `tone_columns`, `call_signs`, …)
//! is the ID-52's own, reused rather than copied.
//!
//! - `settings` — the menu settings: decoded from a download, and written by
//!   riding out inside the codeplug program's upload (the UV-5R pattern).

pub(crate) mod layout;
pub(crate) mod memory;
pub(crate) mod program;
pub(crate) mod protocol;
pub(crate) mod settings;
#[cfg(test)]
mod hw_read;
#[cfg(test)]
mod real_images;
#[cfg(test)]
mod dev_export;
#[cfg(test)]
mod hw_ladder;

use std::time::Duration;

use serialport::SerialPort;

use crate::commands::export::SlotChannel;
use crate::models::RadioModel;
use crate::radios::driver::{
    with_restore_hint, CodeplugProgramReport, DecodedChannelSample, ImageProgramRequest,
    ImageProgrammer, ImageReader, ImageRestorer, RadioDriver, RadioIdentity,
};

use layout::{CHANNEL_COUNT, IMAGE_LEN, VOLATILE_BASE};

pub(crate) struct IcomId5100;

/// Registry entry (see `radios/registry.rs`).
pub(crate) static DRIVER: IcomId5100 = IcomId5100;

impl RadioDriver for IcomId5100 {
    fn key(&self) -> &'static str {
        "icom_id5100"
    }

    fn display_name(&self) -> &'static str {
        "Icom ID-5100"
    }

    /// The rate the radio answers its ID query on. The clone itself switches
    /// to [`protocol::BAUD_CLONE`] mid-session.
    fn baud(&self) -> u32 {
        protocol::BAUD_INITIAL
    }

    fn identify(&self, port: &str) -> Result<RadioIdentity, String> {
        open_identified(port).map(|(_, id)| id)
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

    // No `SettingsWriter`: settings live in the clone image, so they are written
    // by the codeplug program (`carries_profile_settings`). A standalone settings
    // write would be the same full clone and the same press-POWER restart.
}

/// A fresh session for the next clone. Each clone is its own session, as in
/// CHIRP.
///
/// ★ Measured on Tim's radio, 2026-09-25: after a clone read ends the radio
/// ignores ID queries for **6.8 s**. The ladder's first write died on exactly
/// that — it opened the write session the instant the backup read finished and
/// got silence. Retried rather than delayed by a fixed amount: five attempts of
/// ~3.25 s each plus the pauses between them is ~22 s of patience.
///
/// Used for EVERY session, the first included: an operator who presses
/// Download and then Program (or Read settings) within those seconds would
/// otherwise get "the radio did not answer — check the cable" about a cable
/// that is fine.
pub(super) fn reopen(port: &str) -> Result<Box<dyn SerialPort>, String> {
    open_identified(port).map(|(p, _)| p)
}

/// [`reopen`], keeping the identity the radio answered with.
fn open_identified(port: &str) -> Result<(Box<dyn SerialPort>, RadioIdentity), String> {
    let mut last = String::new();
    for attempt in 0..5 {
        if attempt > 0 {
            std::thread::sleep(Duration::from_millis(1500));
        }
        let opened = protocol::open_port(port)
            .and_then(|mut p| protocol::identify(&mut *p).map(|id| (p, id)));
        match opened {
            Ok(pair) => return Ok(pair),
            // Only silence is worth waiting out. A radio that ANSWERED and is
            // the wrong model or layout will not become right in 1.5 s.
            Err(e) if !e.contains("did not answer") => return Err(e),
            Err(e) => last = e,
        }
    }
    Err(last)
}

/// Read the radio again after a write.
///
/// ★ Measured, ladder steps 1-2 (2026-09-25): after a clone-in the radio shows
/// a message and waits for its POWER button. Until it is pressed it still
/// answers ID queries, but a read starts and then STALLS mid-stream. So a
/// stalled read is retried within a bound while the operator — told by
/// [`ImageProgrammer::after_write_instruction`] — restarts it.
fn read_back(port: &str) -> Result<Vec<u8>, String> {
    let deadline = std::time::Instant::now() + Duration::from_secs(120);
    loop {
        match reopen(port).and_then(|mut p| protocol::download(&mut *p)) {
            Ok(image) => return Ok(image),
            Err(e) if std::time::Instant::now() >= deadline => return Err(e),
            Err(_) => std::thread::sleep(Duration::from_secs(2)),
        }
    }
}

/// Addresses that did not read back as written.
///
/// Everything below [`VOLATILE_BASE`] must match — that is the whole memory
/// area. Above it the radio rewrites its own RX history and state between any
/// two reads, so only the bytes this write CHANGED are held to it.
fn mismatches(base: &[u8], built: &[u8], after: &[u8]) -> usize {
    (0..IMAGE_LEN)
        .filter(|&i| {
            let ours = i < VOLATILE_BASE || base[i] != built[i];
            ours && built[i] != after[i]
        })
        .count()
}

/// Refuse an image the radio would refuse — and answer by factory resetting.
///
/// Length first, then every USED memory's frequency against
/// [`layout::RX_COVERAGE`]: an upload is all-or-nothing, and a single memory
/// the radio cannot hold makes it reject the clone and wipe itself (measured
/// 2026-10-01). A backup read off this radio passes by construction; this is
/// for a file from anywhere else.
fn check_image(image: &[u8]) -> Result<(), String> {
    if image.len() != IMAGE_LEN {
        return Err(format!(
            "this file is {} bytes; an ID-5100 image (current firmware layout) is {IMAGE_LEN}",
            image.len()
        ));
    }
    for slot in (0..CHANNEL_COUNT).filter(|&s| memory::is_used(image, s)) {
        let mhz = memory::read_record(image, slot).rx_hz() / 1e6;
        if !layout::RX_COVERAGE.iter().any(|&(lo, hi)| mhz >= lo - 1e-6 && mhz <= hi + 1e-6) {
            return Err(format!(
                "memory {slot} in this file is {mhz:.4} MHz, outside what the ID-5100 can \
                 receive. Writing it would make the radio reject the whole image and reset \
                 itself to factory defaults, so nothing was sent."
            ));
        }
    }
    Ok(())
}

impl ImageReader for IcomId5100 {
    fn download_image(&self, port: &str) -> Result<(RadioIdentity, Vec<u8>), String> {
        let (mut p, ident) = open_identified(port)?;
        let image = protocol::download(&mut *p)?;
        Ok((ident, image))
    }
    fn decode_sample(&self, image: &[u8]) -> Vec<DecodedChannelSample> {
        program::decode_sample(image)
    }
}

impl ImageProgrammer for IcomId5100 {
    /// The profile's settings are in the image the program uploads, so they go
    /// out with the channels.
    fn carries_profile_settings(&self) -> bool {
        true
    }

    fn after_write_instruction(&self) -> Option<&'static str> {
        Some(
            "When the write finishes, the radio asks to be restarted — press its POWER \
             button. The app reads it back to verify once it is up again.",
        )
    }



    fn upload_image(&self, port: &str, image: &[u8]) -> Result<(), String> {
        check_image(image)?;
        let mut p = reopen(port)?;
        protocol::upload(&mut *p, image)
    }

    fn build_image(
        &self,
        model: &RadioModel,
        channels: &[SlotChannel],
        base: &[u8],
    ) -> Result<Vec<u8>, String> {
        program::build_codeplug(model, channels, &[], base).map(|b| b.image)
    }

    /// Download and back up, build the codeplug into THAT image, upload it, and
    /// read it back — three clone sessions, each on a freshly opened port.
    ///
    /// ⚠ The upload half has not run against a radio; the hardware ladder is
    /// what proves it.
    fn program_codeplug(
        &self,
        port: &str,
        req: &ImageProgramRequest,
    ) -> Result<CodeplugProgramReport, String> {
        if req.channels.len() > CHANNEL_COUNT {
            return Err(format!(
                "Codeplug has {} programmable channels, but the ID-5100 holds only {CHANNEL_COUNT}.",
                req.channels.len()
            ));
        }
        let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
        let slug = slug_label(req.label);
        let backup_path = req.backup_dir.join(if slug.is_empty() {
            format!("id5100-prewrite-{stamp}.img")
        } else {
            format!("id5100-prewrite-{slug}-{stamp}.img")
        });

        // 1. Download + back up.
        let mut p = reopen(port)?;
        let base = protocol::download(&mut *p)?;
        std::fs::write(&backup_path, &base)
            .map_err(|e| format!("could not write backup {}: {e}", backup_path.display()))?;
        drop(p);

        // 2. Build into that image. Before the port is touched again, so a
        //    codeplug that cannot be built never starts a write.
        let mut built = program::build_codeplug(req.model, req.channels, req.banks, &base)?;
        // ⚠ Both halves of the result are REPORTED — the count and the notes.
        // A settings write that said nothing is the dead write path this
        // codebase has shipped twice.
        let (settings_written, settings_notes) = match req.settings {
            Some((values, _schema)) => {
                let (n, notes) = settings::apply(&mut built.image, values);
                (Some(n), notes)
            }
            None => (None, Vec::new()),
        };
        built.warnings.extend(settings_notes);

        // 3. Upload.
        let restore_hint = |e: String| {
            with_restore_hint(
                e,
                &backup_path,
                "Keep that file. Put it back with \"Restore backup…\" in this dialog, \
                 which uploads it over the same cable — it is the only copy of what was \
                 on the radio before this write.",
            )
        };
        let mut p = reopen(port).map_err(restore_hint)?;
        protocol::upload(&mut *p, &built.image).map_err(restore_hint)?;
        drop(p);

        // 4. Read back and verify. Non-fatal: the radio confirmed the clone, so
        //    a failed read-back is a reporting problem, not a write problem.
        let reread = read_back(port);
        let (verified, note) = match reread {
            Ok(after) => match mismatches(&base, &built.image, &after) {
                0 => (true, None),
                n => (
                    false,
                    Some(format!(
                        "The radio confirmed the write, but {n} bytes of what was written did \
                         not read back the same. Power-cycle the radio and use Download to \
                         confirm what it is holding."
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

        let channels_written = req.channels.len();
        Ok(CodeplugProgramReport {
            channels_written,
            slots_cleared: CHANNEL_COUNT - channels_written,
            settings_written,
            verified: Some(verified),
            note,
            backup_path: backup_path.to_string_lossy().to_string(),
            channels: program::decode_sample(&built.image),
            zones_written: 0,
            zones_cleared: 0,
            banks_written: built.banks_written,
            scan_lists_written: 0,
            scan_lists_cleared: 0,
            contacts_written: 0,
            contacts_cleared: 0,
            expected_path: None,
            windows_written: Vec::new(),
            skipped: Vec::new(),
            warnings: built.warnings,
        })
    }
}

impl ImageRestorer for IcomId5100 {
    /// Shape only: a backup from another ID-5100 on the same layout is a normal
    /// thing to restore.
    fn check_restore_image(&self, image: &[u8]) -> Result<(), String> {
        check_image(image)
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
    fn verification_holds_the_memory_area_and_only_what_we_changed_above_it() {
        let base = vec![0u8; IMAGE_LEN];
        let mut built = base.clone();
        built[0x100] = 1; // a memory we wrote
        built[0x29AFC] = 3; // a setting we wrote
        let mut after = built.clone();
        // The radio's own RX-history churn: not ours, not a mismatch.
        after[0x24000] = 0x55;
        assert_eq!(mismatches(&base, &built, &after), 0);
        // A setting we wrote that did not stick is.
        after[0x29AFC] = 5;
        assert_eq!(mismatches(&base, &built, &after), 1);
        // Anything in the memory area is, written by us or not.
        after[0x29AFC] = 3;
        after[0x200] = 9;
        assert_eq!(mismatches(&base, &built, &after), 1);
    }

    /// The program path's own sequence — build the codeplug into the read
    /// image, then apply the profile's settings to that same image — yields one
    /// upload carrying both. The settings half is the one this codebase has
    /// twice wired for read and left dead for write.
    #[test]
    fn a_program_carries_memories_and_settings_together() {
        use crate::commands::export::{SlotBank, SlotChannel};
        let mut base = vec![0u8; IMAGE_LEN];
        base[layout::EMPTY_BITMAP..layout::EMPTY_BITMAP + layout::BITMAP_LEN].fill(0xFF);
        let model = RadioModel { memory_channels: Some(1000), ..Default::default() };
        let slots = vec![SlotChannel {
            slot: 0,
            name: "W0UPS 275".into(),
            channel: crate::models::Channel {
                id: 1,
                rx_freq: 447.275,
                dcs_polarity: "NN".into(),
                ..Default::default()
            },
        }];
        let banks = vec![SlotBank { name: "HOME".into(), slots: vec![0] }];
        let mut built = program::build_codeplug(&model, &slots, &banks, &base).unwrap();
        let profile = serde_json::json!({
            "sounds-beep-level": "3",
            "my-station-my-call-sign-m01": "WW8L",
            "gps-tx-gps-tx-mode": "D-PRS",
        });
        let (n, notes) = settings::apply(&mut built.image, &profile);
        assert_eq!((n, notes.len()), (3, 0), "{notes:?}");
        assert!(memory::is_used(&built.image, 0));
        assert_eq!(memory::read_record(&built.image, 0).name(), "W0UPS 275");
        assert_eq!(built.image[0x29AFC], 3);
        assert_eq!(&built.image[0x2215C..0x22164], b"WW8L    ");
        assert_eq!(built.image[0x29ACD], 1);
    }

    #[test]
    fn a_wrong_length_file_is_refused_for_restore() {
        assert!(DRIVER.check_restore_image(&[0u8; 1000]).is_err());
    }

    /// The probe image that reset the radio on 2026-10-01 is refused here, and
    /// the backup that restored it is not. Real bytes: the probe's X1 record
    /// (117.975 AM, 8.33 kHz multiplier) as the driver built it.
    #[test]
    fn a_file_with_an_out_of_coverage_memory_is_refused_for_restore() {
        let mut image = vec![0u8; IMAGE_LEN];
        image[layout::EMPTY_BITMAP..layout::EMPTY_BITMAP + layout::BITMAP_LEN].fill(0xFF);
        assert!(DRIVER.check_restore_image(&image).is_ok());
        let ec = crate::commands::export::ExpandedChannel {
            channel: crate::models::Channel {
                rx_freq: 117.975,
                mode: Some("AM".into()),
                dcs_polarity: "NN".into(),
                ..Default::default()
            },
            tg_label: None,
            timeslot: None,
            tg_number: None,
            tg_call_type: None,
            tg_inline: false,
        };
        let rec = memory::encode_channel(&ec, "X1").unwrap();
        memory::store(&mut image, 996, &rec, None);
        let e = DRIVER.check_restore_image(&image).unwrap_err();
        assert!(e.contains("memory 996") && e.contains("factory defaults"), "{e}");
    }
}
