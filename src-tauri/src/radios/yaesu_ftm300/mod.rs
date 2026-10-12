//! Yaesu FTM-300DR driver (`driver_key = "yaesu_ftm300"`) — microSD programming.
//!
//! The fourth card radio. The operator backs the radio up to its card with
//! **SD CARD → BACKUP → WRITE TO SD → ALL**, which writes
//! `FTM300D/BACKUP/CLNFTM300D.dat`; [`sd_image`] patches the memories into that
//! file in place, and **READ FROM SD → ALL** loads it back. No cable, and no
//! third-party software in the loop.
//!
//! ## Research first
//!
//! Two published sources cover the file, and both were checked against Tim's
//! own radio rather than trusted ([[research-before-reverse-engineering]]):
//!
//! | source | what it gives | how it held up |
//! |---|---|---|
//! | CHIRP `chirp/drivers/ftm300d.py` (Sep 2026, experimental) | the 16-byte record, ~35 settings, the cable clone protocol | record right but for three fields: offset width, user CTCSS, the name-length table |
//! | xmutantson/ftm300dr-tools | the SD map, "no checksum", the name-length code | name-length code right; its 1-based DCS index wrong |
//!
//! The FT5D's driver contributed the workflow only — the layout is unrelated
//! (16-byte records and a parallel name table, against the FT5D's 32-byte
//! records), and the FT5D's 32-bit checksum has no counterpart found here.
//!
//! ## What is NOT here
//!
//! No cable path. CHIRP documents one (SCU-56/SCU-20 on the DATA jack, 131-byte
//! framed clone at 38400) and reports the clone image IS this file's layout, so
//! it is a possible later modality — but the card needs no cable at all.
//! `identify` fails by name rather than timing out on a port.
//!
//! Working notes: `scratchpad/ftm300/` (gitignored — raw dumps of a personal
//! radio), `FINDINGS.md` for the byte tables, `PLAN.md` for the menu census.

pub(crate) mod sd_image;
pub(crate) mod settings;

use crate::radios::driver::{
    CardSettingsWriter, CodeplugExporter, ExportRequest, RadioDriver, RadioIdentity,
};

pub(crate) struct YaesuFtm300;

pub(crate) static DRIVER: YaesuFtm300 = YaesuFtm300;

impl RadioDriver for YaesuFtm300 {
    fn key(&self) -> &'static str {
        "yaesu_ftm300"
    }

    fn display_name(&self) -> &'static str {
        "Yaesu FTM-300DR"
    }

    /// Unused: nothing here opens a port. CHIRP's figure for the radio's DATA
    /// jack clone is quoted so the number is not invented.
    fn baud(&self) -> u32 {
        38400
    }

    /// There is no handshake, because there is no cable path.
    fn identify(&self, _port: &str) -> Result<RadioIdentity, String> {
        Err("The FTM-300 is programmed from its microSD card, not over a cable. On the radio, \
             SD CARD → BACKUP → WRITE TO SD → ALL, put the card in this computer, and use the \
             card actions instead."
            .into())
    }

    fn as_codeplug_exporter(&self) -> Option<&dyn CodeplugExporter> {
        Some(self)
    }
}

impl CodeplugExporter for YaesuFtm300 {
    fn export_format(&self) -> &'static str {
        "yaesu_ftm300_sd"
    }

    /// The radio's microSD `CLNFTM300D.dat`.
    fn target_extensions(&self) -> &'static [&'static str] {
        &["dat"]
    }

    /// `path` is the operator's own `FTM300D/BACKUP/CLNFTM300D.dat`: read as the
    /// template and written back in place, because that fixed path is what READ
    /// FROM SD loads. The untouched original is kept beside it as
    /// `CLNFTM300D.dat.orig`, written once and never overwritten, so a second
    /// export cannot clobber the only pristine copy.
    fn export(&self, path: &str, req: &ExportRequest) -> Result<usize, String> {
        patch_in_place(path, |template| {
            let mut image = sd_image::patch_backup(template, req.channels, req.model, &req.memory_group.iter().copied().collect())?;
            // Non-channel settings from the codeplug's radio profile. A codeplug with
            // no profile leaves every one exactly as the operator's backup had it.
            if let Some(json) = req.profile_settings {
                let parsed: serde_json::Value = serde_json::from_str(json)
                    .map_err(|e| format!("This profile's settings are not valid JSON: {e}"))?;
                if let Some(map) = parsed.as_object() {
                    settings::apply_settings(&mut image, map);
                }
            }
            Ok((image, req.channels.len()))
        })
    }

    fn has_memory_group(&self) -> bool {
        true
    }

    fn as_card_settings_writer(&self) -> Option<&dyn CardSettingsWriter> {
        Some(self)
    }
}

impl CardSettingsWriter for YaesuFtm300 {
    /// The same file and rules as the export — in place, `.orig` kept once —
    /// with only the settings applied. Every memory is the template's.
    fn export_settings(
        &self,
        path: &str,
        settings: &serde_json::Map<String, serde_json::Value>,
    ) -> Result<usize, String> {
        patch_in_place(path, |template| {
            sd_image::validate_backup(template)?;
            let mut image = template.to_vec();
            let n = settings::apply_settings(&mut image, settings);
            Ok((image, n))
        })
    }
}

fn patch_in_place(
    path: &str,
    patch: impl FnOnce(&[u8]) -> Result<(Vec<u8>, usize), String>,
) -> Result<usize, String> {
    crate::radios::card_file::patch_in_place(
        path,
        "Pick FTM300D/BACKUP/CLNFTM300D.dat on the radio's microSD card — back the radio \
         up first (SD CARD → BACKUP → WRITE TO SD → ALL) if it is not there.",
        patch,
    )
}

#[cfg(test)]
mod gate;
#[cfg(test)]
mod dev_export;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::radios::driver::DriverCapabilities;

    /// Step 6's gate: one export carries the memories AND the profile's
    /// settings into the same file, and touches nothing else.
    #[test]
    fn an_export_carries_memories_and_settings_together() {
        use crate::commands::export::ExpandedChannel;
        use crate::models::Channel;
        let dir = std::env::temp_dir().join(format!("ftm300-export-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("CLNFTM300D.dat");
        let mut template = vec![0x5Au8; sd_image::IMAGE_LEN];
        template[..6].copy_from_slice(b"AH071$");
        std::fs::write(&path, &template).unwrap();

        let ec = ExpandedChannel {
            channel: Channel {
                id: 7,
                name_short: Some("NCARC".into()),
                rx_freq: 447.275,
                tx_freq: Some(442.275),
                dcs_polarity: "NN".into(),
                cross_mode: "Tone->Tone".into(),
                source: "test".into(),
                ..Default::default()
            },
            tg_label: None,
            timeslot: None,
            tg_number: None,
            tg_call_type: None,
            tg_inline: false,
        };
        let ec2 = ExpandedChannel {
            channel: Channel { id: 8, name_short: Some("OUTSIDE".into()), ..ec.channel.clone() },
            tg_label: None,
            timeslot: None,
            tg_number: None,
            tg_call_type: None,
            tg_inline: false,
        };
        let refs = [&ec, &ec2];
        let model = crate::seed::test_model("FTM-300DR");
        // Channel 7 is in the codeplug's M-GRP, so its memory gets the flag;
        // channel 8 is not.
        let req = ExportRequest {
            channels: &refs,
            groups: &[],
            model: &model,
            profile_settings: Some(r#"{"lcd-brightness":"Mid","aprs-modem":true}"#),
            memory_group: &[7],
        };
        assert_eq!(DRIVER.export(path.to_str().unwrap(), &req).unwrap(), 2);
        let out = std::fs::read(&path).unwrap();
        assert_eq!(&out[0x800..0x805], &[0x93, 0x02, 0x84, 0x47, 0x27]);
        assert_eq!(out[0x810], 0x83, "a channel not chosen for M-GRP stays out of it");
        let s = settings::decode_settings(&out);
        assert_eq!(s["lcd-brightness"], "Mid");
        assert_eq!(s["aprs-modem"], true);
        assert_eq!(std::fs::read(path.with_extension("dat.orig")).unwrap(), template);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn advertises_export_only() {
        let caps = DriverCapabilities::of(&DRIVER);
        assert!(!caps.download_image && !caps.program_image && !caps.read_settings);
        assert!(DRIVER.as_codeplug_exporter().is_some());
    }
}
