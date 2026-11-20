//! Yaesu FTM-400DR / FTM-400XDR driver (`driver_key = "yaesu_ftm400"`, #52) —
//! microSD programming, memories and set-up menu settings both.
//!
//! The operator backs the radio up with hold **[DISP]** → SD CARD → BACKUP →
//! Write to SD → **ALL**, which writes `FTM400D/BACKUP/CLONE/CLNFTM400D.dat`;
//! [`sd_image`] patches the memories into that file and Read from SD → ALL
//! loads it back. The DR and the XDR write the same file (ADMS-7 serves both).
//!
//! ## Provenance
//!
//! Not a sibling's map by analogy. Yaesu's own programmer, ADMS-7, is a .NET
//! program, and its decompiled `DataConverter` (published in
//! featherfeet/ftm400xdrprogrammer) is the codec for this exact file: regions,
//! the record, the name character set, the header checksum. Tim's radio-written
//! backup decodes under it with zero errors, and every one of his 392 memories
//! re-encodes byte for byte but for the fields a channel cannot say
//! (`gate.rs`). The one place the radio and ADMS-7 disagree — the digit zero
//! in a name — follows the radio.
//!
//! Settings: [`settings`] — 260 set-up menu items, each option exactly the
//! bytes Yaesu's ADMS-7 wrote for it (its own encoder, run headless).
//!
//! ## Hardware-proven (2026-11-20, Tim's FTM-400DR, DR firmware)
//!
//! One combined file — Tim's choice: the restore menu has no file list, so
//! every load is a card trip — carrying an 80-memory codeplug on both bands,
//! a 19-point band probe and five changed settings, loaded with Read from SD →
//! ALL:
//!
//! - loaded, no refusal and no reset: nothing past the header byte-sum is
//!   checked, and memories and settings may be patched;
//! - names with zeros displayed (`W0UPS 27`): the radio's own `0xBF` zero;
//! - all five settings as written (BACKGROUND COLOR, TIME/VDD, AUTO DIALER,
//!   12-hour time, BEEP), so ADMS-7's option order holds where checked;
//! - at CONFIG 11 RX COVERAGE NORMAL a band shows only 137-480 MHz: the
//!   file's air-band and 480 MHz-and-up memories were holes until a band was
//!   switched to WIDE; then that band showed all 80. RX COVERAGE is per band,
//!   and both bands showed everything at WIDE, AM included, so the writer sets
//!   both bands WIDE when a codeplug needs it ([`sd_image`]);
//! - band probe: all 19 memories landed on both bands at their written
//!   frequencies — 108 and 999.99 MHz, both sides of 137 / 174 / 400 / 480 MHz,
//!   222.1, the cellular-gap edges 823.9875 / 851.0125 / 868.9875 / 894 MHz, and
//!   the four 8.33 kHz airband points (127.80833, 127.81667, 127.83333,
//!   127.84167), each displayed as written.
//!
//! Working notes: `scratchpad/yaesu_ftm400/` (gitignored) — `PLAN.md`,
//! `FINDINGS.md`, `CENSUS.md`.

pub(crate) mod sd_image;
pub(crate) mod settings;

use crate::radios::driver::{
    CardSettingsWriter, CodeplugExporter, ExportRequest, RadioDriver, RadioIdentity,
};

pub(crate) struct YaesuFtm400;

pub(crate) static DRIVER: YaesuFtm400 = YaesuFtm400;

impl RadioDriver for YaesuFtm400 {
    /// Byte 1's mode field: FM, AM, narrow FM. C4FM channels go in as FM.
    fn analog_modes(&self) -> &'static [&'static str] {
        &["FM", "NFM", "AM"]
    }

    fn key(&self) -> &'static str {
        "yaesu_ftm400"
    }

    fn display_name(&self) -> &'static str {
        "Yaesu FTM-400DR / XDR"
    }

    /// Unused: nothing here opens a port. ADMS-7 also reaches the radio over
    /// a cable, but the card needs no cable and no driver.
    fn baud(&self) -> u32 {
        38400
    }

    /// There is no handshake, because there is no cable path.
    fn identify(&self, _port: &str) -> Result<RadioIdentity, String> {
        Err("The FTM-400 is programmed from its microSD card, not over a cable. On the radio, \
             hold [DISP] → SD CARD → BACKUP → Write to SD → ALL, put the card in this computer, \
             and use the card actions instead."
            .into())
    }

    fn as_codeplug_exporter(&self) -> Option<&dyn CodeplugExporter> {
        Some(self)
    }
}

impl CodeplugExporter for YaesuFtm400 {
    fn export_format(&self) -> &'static str {
        "yaesu_ftm400_sd"
    }

    /// The radio's microSD `CLNFTM400D.dat`.
    fn target_extensions(&self) -> &'static [&'static str] {
        &["dat"]
    }

    /// `path` is the operator's own `FTM400D/BACKUP/CLONE/CLNFTM400D.dat`: read
    /// as the template and written back in place, because Read from SD loads
    /// that path. The untouched original is kept beside it as
    /// `CLNFTM400D.dat.orig`, written once and never overwritten.
    fn export(&self, path: &str, req: &ExportRequest) -> Result<usize, String> {
        self.export_noting(path, req).map(|(n, _)| n)
    }

    /// Memories and the profile's settings in one file. If the codeplug needs
    /// RX COVERAGE WIDE and the file did not have it, a note says so — the
    /// export changes a radio setting the operator may not have asked for.
    fn export_noting(&self, path: &str, req: &ExportRequest) -> Result<(usize, Vec<String>), String> {
        let mut notes = Vec::new();
        let n = patch_in_place(path, |template| {
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
            // Last, so a profile holding NORMAL cannot hide the codeplug's own air-band
            // or 480 MHz-and-up channels (see `sd_image`'s module doc). Noted against
            // the operator's own file, not the intermediate image, which `patch_backup`
            // has already widened.
            if sd_image::needs_wide(req.channels) {
                // Noted if either the operator's file was at NORMAL or their profile
                // asked for NORMAL — both are a setting changed on their behalf.
                let profile_normal = sd_image::set_wide(&mut image);
                let file_normal = sd_image::set_wide(&mut template.to_vec());
                notes.extend(sd_image::wide_note(profile_normal || file_normal, sd_image::memories_needing_wide(&image)));
            }
            Ok((image, req.channels.len()))
        })?;
        Ok((n, notes))
    }

    fn as_card_settings_writer(&self) -> Option<&dyn CardSettingsWriter> {
        Some(self)
    }
}

impl CardSettingsWriter for YaesuFtm400 {
    /// The same file and rules as the export — in place, `.orig` kept once —
    /// with only the settings applied. Every memory is the template's.
    fn export_settings(
        &self,
        path: &str,
        settings: &serde_json::Map<String, serde_json::Value>,
    ) -> Result<usize, String> {
        self.export_settings_noting(path, settings).map(|(n, _)| n)
    }

    /// A profile read off a card at NORMAL must not hide the memories a
    /// codeplug export put there: if any lie outside 137-480 MHz, coverage is
    /// put back to WIDE after the profile is applied, and the note says so.
    fn export_settings_noting(
        &self,
        path: &str,
        settings: &serde_json::Map<String, serde_json::Value>,
    ) -> Result<(usize, Vec<String>), String> {
        let mut notes = Vec::new();
        let n = patch_in_place(path, |template| {
            sd_image::validate_backup(template)?;
            let mut image = template.to_vec();
            let n = settings::apply_settings(&mut image, settings);
            let outside = sd_image::memories_needing_wide(&image);
            if outside > 0 && sd_image::set_wide(&mut image) {
                notes.extend(sd_image::wide_note(true, outside));
            }
            Ok((image, n))
        })?;
        Ok((n, notes))
    }
}

fn patch_in_place(
    path: &str,
    patch: impl FnOnce(&[u8]) -> Result<(Vec<u8>, usize), String>,
) -> Result<usize, String> {
    crate::radios::card_file::patch_in_place(
        path,
        "Pick FTM400D/BACKUP/CLONE/CLNFTM400D.dat on the radio's microSD card — back the \
         radio up first (hold [DISP] → SD CARD → BACKUP → Write to SD → ALL) if it is not there.",
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
        let dir = std::env::temp_dir().join(format!("ftm400-export-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("CLNFTM400D.dat");
        let mut template = vec![0x5Au8; sd_image::IMAGE_LEN];
        template[..6].copy_from_slice(b"AH034$");
        std::fs::write(&path, &template).unwrap();

        let ec = ExpandedChannel {
            channel: Channel {
                id: 7,
                name_short: Some("W0UPS".into()),
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
        let model = crate::seed::test_model("FTM-400");
        let req = ExportRequest {
            channels: &refs,
            groups: &[],
            model: &model,
            profile_settings: Some(r#"{"apo":"1.0hour","aprs-modem":"ON","aprs-call-sign":"WW8L"}"#),
            memory_group: &[],
        };
        assert_eq!(DRIVER.export(path.to_str().unwrap(), &req).unwrap(), 1);
        let out = std::fs::read(&path).unwrap();
        assert_eq!(&out[0x800..0x805], &[0x83, 0x02, 0x84, 0x47, 0x27]);
        assert_eq!(&out[0x2860..0x2865], &[0x83, 0x02, 0x84, 0x47, 0x27]);
        let s = settings::decode_settings(&out);
        assert_eq!(s["apo"], "1.0hour");
        assert_eq!(s["aprs-modem"], "ON");
        assert_eq!(s["aprs-call-sign"], "WW8L");
        assert_eq!(std::fs::read(path.with_extension("dat.orig")).unwrap(), template);
        std::fs::remove_dir_all(&dir).ok();
    }

    /// A settings-only write of a profile holding NORMAL keeps the codeplug's
    /// air-band memory visible, and the export notes a coverage change.
    #[test]
    fn coverage_stays_wide_for_out_of_ham_memories() {
        use crate::commands::export::ExpandedChannel;
        use crate::models::Channel;
        let dir = std::env::temp_dir().join(format!("ftm400-cov-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("CLNFTM400D.dat");
        let mut template = vec![0x00u8; sd_image::IMAGE_LEN];
        template[..6].copy_from_slice(b"AH034$");
        std::fs::write(&path, &template).unwrap();
        let ec = ExpandedChannel {
            channel: Channel {
                id: 1,
                name_short: Some("TOWER".into()),
                rx_freq: 118.4,
                tx_freq: Some(118.4),
                mode: Some("AM".into()),
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
        let model = crate::seed::test_model("FTM-400");
        let req = ExportRequest {
            channels: &refs,
            groups: &[],
            model: &model,
            profile_settings: Some(r#"{"rx-coverage-a":"NORMAL","rx-coverage-b":"NORMAL"}"#),
            memory_group: &[],
        };
        let (_, notes) = DRIVER.export_noting(path.to_str().unwrap(), &req).unwrap();
        assert_eq!(notes.len(), 1, "{notes:?}");
        let s = settings::decode_settings(&std::fs::read(&path).unwrap());
        assert_eq!((&s["rx-coverage-a"], &s["rx-coverage-b"]), (&"WIDE".into(), &"WIDE".into()));
        // Over the now-WIDE file a profile asking NORMAL is still overridden — and noted;
        // with no coverage in the profile nothing changes, so nothing is said.
        assert_eq!(DRIVER.export_noting(path.to_str().unwrap(), &req).unwrap().1.len(), 1);
        let quiet = ExportRequest { profile_settings: None, ..req };
        assert!(DRIVER.export_noting(path.to_str().unwrap(), &quiet).unwrap().1.is_empty());

        let normal: serde_json::Value = serde_json::json!({"rx-coverage-a": "NORMAL", "rx-coverage-b": "NORMAL"});
        let (_, notes) = DRIVER.export_settings_noting(path.to_str().unwrap(), normal.as_object().unwrap()).unwrap();
        assert_eq!(notes.len(), 1, "{notes:?}");
        let s = settings::decode_settings(&std::fs::read(&path).unwrap());
        assert_eq!((&s["rx-coverage-a"], &s["rx-coverage-b"]), (&"WIDE".into(), &"WIDE".into()));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn advertises_export_only() {
        let caps = DriverCapabilities::of(&DRIVER);
        assert!(!caps.download_image && !caps.program_image && !caps.read_settings);
        assert!(DRIVER.as_codeplug_exporter().is_some());
    }
}
