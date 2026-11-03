//! Yaesu FTM-510DR driver (`driver_key = "yaesu_ftm510"`, #151) — microSD
//! programming, memories and Setup Menu settings both.
//!
//! The operator backs the radio up with Setup Menu **107 BACKUP → WRITE TO SD →
//! ALL**, which writes `FTM510D/BACKUP/CLNFTM510D.dat`; [`sd_image`] patches the
//! memories into that file and **READ FROM SD → ALL** loads it back.
//!
//! ## A sibling's format, re-measured
//!
//! No published map exists for the FTM-510 (CHIRP has none; fulgidus/channel-forge
//! has a rough FTM-500 note). The file turned out to be the FTM-300's layout —
//! same 98,304 bytes, 16-byte records at `0x0800`, a name-length table at
//! `0x4CB0`, names at `0x10000` — so the record encoder is the FTM-300's,
//! reused rather than copied. That is an inference by analogy until proven, and
//! it was proven against the 510's own bytes, not the FTM-300's:
//!
//! - the 510's file magic is its own (`AH089$`; FTM-300 `AH071$`, FTM-500 `AH081$`);
//! - Tim's radio-written file: both factory memories and all 20 VFO / Home records
//!   decode and re-encode under this encoder (`gate.rs`);
//! - Yaesu's ADMS-18 writing a memory (146.520, `TEST`) put every byte where the
//!   FTM-300 encoder puts it — except the name-length entry (below).
//!
//! What differs from the FTM-300: receive coverage stops at 550 MHz (the FTM-300
//! goes to 999), and the 8.33 kHz airband code — measured on the FTM-300's screen —
//! has not been seen on a 510, so every off-grid airband point is refused here.
//!
//! ## Hardware-proven (2026-11-03, Tim's radio, `ladder.rs`)
//!
//! All five ladder steps passed, each loaded with 107 BACKUP → READ FROM SD → ALL
//! from a file placed beside the radio's own as `CLNFTM510D01`-`05` — the radio
//! lists and loads numbered files it did not write itself.
//!
//! 1. Identity copy: loaded, call sign and memories intact.
//! 2. One renamed memory: loaded and showed `LADDER 2` — **no checksum is
//!    checked on load**, and the radio reads names with the FTM-300's
//!    `(1 << (16 - len)) - 1` length rule (ADMS-18 writes `0x0000`; both load).
//! 3. Full codeplug, 61 memories from the app's pipeline: names, shifts, DN,
//!    narrow FM and the count all as written.
//! 4. Band probe: all 11 edges of 108-550 MHz landed, both sides of each
//!    internal boundary.
//! 5. Settings: LCD MID, beep HIGH, **Sub Band Mute ON = 3** (ADMS-18 was right;
//!    CHIRP's FTM-300 reading is inverted) and **S-DX AUTO** across its three
//!    scattered bits (the screen showed AUTO-A, SPU-1 fitted).
//!
//! ## Still open
//!
//! - **Memory-group flag.** The FTM-300's M-GRP is byte 0 bit 4; the 510 has
//!   PMG instead, and an FTM-500 file carries an unexplained byte 0 bit 3. Neither
//!   is set here.
//!
//! Working notes: `scratchpad/yaesu_ftm510/` (gitignored) — `PLAN.md`,
//! `FINDINGS.md`, `adms/MEASURED.md`.

pub(crate) mod sd_image;
pub(crate) mod settings;

use crate::radios::driver::{
    CardSettingsWriter, CodeplugExporter, ExportRequest, RadioDriver, RadioIdentity,
};

pub(crate) struct YaesuFtm510;

pub(crate) static DRIVER: YaesuFtm510 = YaesuFtm510;

impl RadioDriver for YaesuFtm510 {
    fn key(&self) -> &'static str {
        "yaesu_ftm510"
    }

    fn display_name(&self) -> &'static str {
        "Yaesu FTM-510DR"
    }

    /// Unused: nothing here opens a port. ADMS-18 also reaches the radio over
    /// an SCU-20 cable, but the card needs no cable and no driver.
    fn baud(&self) -> u32 {
        38400
    }

    /// There is no handshake, because there is no cable path.
    fn identify(&self, _port: &str) -> Result<RadioIdentity, String> {
        Err("The FTM-510 is programmed from its microSD card, not over a cable. On the radio, \
             Setup Menu 107 BACKUP → WRITE TO SD → ALL, put the card in this computer, and use \
             the card actions instead."
            .into())
    }

    fn as_codeplug_exporter(&self) -> Option<&dyn CodeplugExporter> {
        Some(self)
    }
}

impl CodeplugExporter for YaesuFtm510 {
    fn export_format(&self) -> &'static str {
        "yaesu_ftm510_sd"
    }

    /// The radio's microSD `CLNFTM510D.dat`.
    fn target_extensions(&self) -> &'static [&'static str] {
        &["dat"]
    }

    /// `path` is the operator's own `FTM510D/BACKUP/CLNFTM510D.dat`: read as the
    /// template and written back in place, because READ FROM SD lists files by
    /// the names the radio gave them. The untouched original is kept beside it
    /// as `CLNFTM510D.dat.orig`, written once and never overwritten.
    fn export(&self, path: &str, req: &ExportRequest) -> Result<usize, String> {
        patch_in_place(path, |template| {
            let mut image = sd_image::patch_backup(template, req.channels, req.model)?;
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

    fn as_card_settings_writer(&self) -> Option<&dyn CardSettingsWriter> {
        Some(self)
    }
}

impl CardSettingsWriter for YaesuFtm510 {
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
        "Pick FTM510D/BACKUP/CLNFTM510D.dat on the radio's microSD card — back the radio \
         up first (Setup Menu 107 BACKUP → WRITE TO SD → ALL) if it is not there.",
        patch,
    )
}

#[cfg(test)]
mod dev_export;
#[cfg(test)]
mod gate;
#[cfg(test)]
mod ladder;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::radios::driver::DriverCapabilities;

    /// Step 6's gate: one export carries the memories AND the profile's
    /// settings into the same file, and keeps the untouched original.
    #[test]
    fn an_export_carries_memories_and_settings_together() {
        use crate::commands::export::ExpandedChannel;
        use crate::models::Channel;
        let dir = std::env::temp_dir().join(format!("ftm510-export-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("CLNFTM510D.dat");
        let mut template = vec![0x5Au8; sd_image::IMAGE_LEN];
        template[..6].copy_from_slice(b"AH089$");
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
        let refs = [&ec];
        let model = crate::seed::test_model("FTM-510DR");
        let req = ExportRequest {
            channels: &refs,
            groups: &[],
            model: &model,
            profile_settings: Some(r#"{"lcd-brightness":"MID","aprs-modem":true,"aprs-callsign":"WW8L"}"#),
            memory_group: &[],
        };
        assert_eq!(DRIVER.export(path.to_str().unwrap(), &req).unwrap(), 1);
        let out = std::fs::read(&path).unwrap();
        assert_eq!(&out[0x800..0x805], &[0x83, 0x02, 0x84, 0x47, 0x27]);
        let s = settings::decode_settings(&out);
        assert_eq!(s["lcd-brightness"], "MID");
        assert_eq!(s["aprs-modem"], true);
        assert_eq!(s["aprs-callsign"], "WW8L");
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
