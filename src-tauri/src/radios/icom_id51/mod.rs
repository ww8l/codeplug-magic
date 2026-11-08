//! Icom ID-51 Plus driver (`driver_key = "icom_id51"`, issue #50) — microSD
//! programming.
//!
//! The radio saves and loads everything through one file on its card:
//! `MENU > SD Card > Save Setting` writes `ID-51/Setting/Setyyyymmdd_xx.icf`
//! ("memory channels, item settings in the menu screen, and Repeater List",
//! Advanced Manual p. 2-13), and `Load Setting > ALL` reads one back. So the
//! codeplug export patches a copy of the radio's own file — memories, banks and
//! the profile's MENU settings together — and writes it beside the original as
//! a NEW file named the way the radio names its own. Nothing the operator saved
//! is modified.
//!
//! The ID-51 Plus can also clone over a cable, but Tim chose the card for this
//! radio (2026-11-04), and there is no cable path here.
//!
//! - [`layout`] — offsets (CHIRP's `id51plus.py`, checked against Tim's file)
//! - [`memory`] — records, flags, banks; the ID-5100's 49-byte codec, this
//!   radio's rules
//! - [`settings`] — 241 MENU settings + the manual position, every one measured
//!   in CS-51PLUS
//! - the `.icf` envelope and its `#CD` MD5 are the ID-52's ([`IcfFile`]) — the
//!   radio's file reproduces its checksum under that algorithm
//!
//! Working notes: `scratchpad/icom_id51/` (PLAN, FINDINGS, MEASURED; gitignored).
//!
//! ## Hardware-proven (2026-11-07, Tim's ID-51 Plus 50th Anniversary)
//!
//! The ladder in [`ladder`] passed every rung, each file loaded with
//! `Load Setting > ALL`: an identity copy; a one-field rename (proves the
//! `#CD`); a full 50-memory codeplug in two named banks through the real
//! exporter; a band probe at 108.000 AM / 173.995 / 380.000 / 478.995 plus 2 m
//! and 70 cm (all six landed); and two settings (Backlight, Beep Level) shown on
//! the radio's own menus.

pub(crate) mod layout;
pub(crate) mod memory;
pub(crate) mod settings;
#[cfg(test)]
mod dev_export;
#[cfg(test)]
mod ladder;
#[cfg(test)]
mod real_file;

use crate::radios::driver::{
    CardSettingsWriter, CodeplugExporter, ExportRequest, RadioDriver, RadioIdentity,
};
use crate::radios::icom_id52::icf::IcfFile;

/// Line 1 of the radio's `.icf`: the ID-51 **Plus** (and the 50th Anniversary
/// edition, which CHIRP's driver names alongside it). The original ID-51 is
/// `33900001` and the Plus2 `33900003`; neither has been measured.
pub(crate) const ID51_MODEL_ID: &str = "33900002";

/// `#MapRev` of the file the radio writes, and the only one these offsets were
/// measured against.
pub(crate) const ID51_MAP_REV: u32 = 1;

/// Refuse a file this driver's measured offsets do not describe, saying which
/// radio it came from.
pub(crate) fn check_is_an_id51_file(icf: &IcfFile) -> Result<(), String> {
    match icf.model_id() {
        ID51_MODEL_ID => {}
        "33900001" => {
            return Err("That settings file is from an original ID-51 (not the Plus). This app \
                        has only been measured against the ID-51 Plus, and the two lay their \
                        memories out differently."
                .into())
        }
        "33900003" => {
            return Err("That settings file is from an ID-51 Plus2. This app has only been \
                        measured against the ID-51 Plus."
                .into())
        }
        other => {
            return Err(format!(
                "That settings file belongs to a different Icom (model {other}, not the ID-51 \
                 Plus's {ID51_MODEL_ID}). Use a file saved by this radio."
            ))
        }
    }
    if icf.map_rev() != Some(ID51_MAP_REV) {
        return Err(format!(
            "This settings file is layout revision {}, and this app only writes revision \
             {ID51_MAP_REV} — the one the radio saves for itself. Save a fresh file on the radio \
             with MENU > SD Card > Save Setting and use that.",
            icf.map_rev().map_or_else(|| "unknown".to_string(), |r| r.to_string())
        ));
    }
    if icf.image().len() != layout::IMAGE_LEN {
        return Err(format!(
            "This settings file holds {} bytes; an ID-51 Plus file holds {}.",
            icf.image().len(),
            layout::IMAGE_LEN
        ));
    }
    Ok(())
}

pub(crate) struct IcomId51;

pub(crate) static DRIVER: IcomId51 = IcomId51;

impl RadioDriver for IcomId51 {
    /// `memory.rs`: FM, FM-N and AM memories.
    fn analog_modes(&self) -> &'static [&'static str] {
        &["FM", "NFM", "AM"]
    }

    fn key(&self) -> &'static str {
        "icom_id51"
    }

    /// Off the 5 and 6.25 kHz grids — in practice an 8.33 kHz airband channel,
    /// which no ID-51 memory has been seen to store.
    fn unstorable_frequency(&self, rx_mhz: f64) -> Option<String> {
        memory::storable(rx_mhz)
            .is_none()
            .then(|| "is not on the 5 or 6.25 kHz channel grid a memory can store".to_string())
    }

    fn display_name(&self) -> &'static str {
        "Icom ID-51 Plus"
    }

    /// Unused: there is no cable session. The family's CI-V default is quoted
    /// so the number is not invented.
    fn baud(&self) -> u32 {
        9600
    }

    fn identify(&self, _port: &str) -> Result<RadioIdentity, String> {
        Err("The ID-51 Plus is programmed from its microSD card here, not over a cable. Save \
             its settings to the card (MENU > SD Card > Save Setting), put the card in this \
             computer, and use the card actions instead."
            .into())
    }

    fn as_codeplug_exporter(&self) -> Option<&dyn CodeplugExporter> {
        Some(self)
    }
}

/// The ID-51 Plus's card folder, for the shared `.icf` file rules
/// ([`crate::radios::icom_card`]).
pub(crate) const ID51_CARD: crate::radios::icom_card::IcfCard = crate::radios::icom_card::IcfCard {
    radio: "ID-51 Plus",
    folder: "ID-51/Setting/",
    save_menu: "MENU > SD Card > Save Setting",
    is_ours: |f| check_is_an_id51_file(f).is_ok(),
};

impl CodeplugExporter for IcomId51 {
    fn export_format(&self) -> &'static str {
        "icom_id51_icf"
    }

    fn target_extensions(&self) -> &'static [&'static str] {
        &["icf"]
    }

    fn export(&self, path: &str, req: &ExportRequest) -> Result<usize, String> {
        self.export_noting(path, req).map(|(n, _)| n)
    }

    /// Memories, banks and the profile's settings, patched into one `.icf`
    /// (`Load Setting > ALL` restores all of it in one operation). Every
    /// setting the encoder declined comes back as a note for the report.
    fn export_noting(&self, path: &str, req: &ExportRequest) -> Result<(usize, Vec<String>), String> {
        ID51_CARD.patch_icf(path, |icf| {
            check_is_an_id51_file(icf)?;
            let written = memory::write_memories(icf.image_mut(), req.channels, req.groups, req.model)?;
            // Settings ride in the same file. An empty profile leaves the radio's
            // settings exactly as its file has them.
            let mut notes = Vec::new();
            if let Some(json) = req.profile_settings {
                let parsed: serde_json::Value = serde_json::from_str(json)
                    .map_err(|e| format!("This profile's settings are not valid JSON: {e}"))?;
                if let Some(map) = parsed.as_object() {
                    notes = settings::apply_settings(icf.image_mut(), map).1;
                }
            }
            Ok((written, notes))
        })
    }

    fn as_card_settings_writer(&self) -> Option<&dyn CardSettingsWriter> {
        Some(self)
    }

    /// A folder means "make me a new file in here", named the way the radio
    /// names its own so it sorts into place on the Load Setting screen.
    fn resolve_target(&self, path: &str) -> Result<String, String> {
        ID51_CARD.resolve_target(path)
    }
}

impl CardSettingsWriter for IcomId51 {
    fn export_settings(
        &self,
        path: &str,
        settings: &serde_json::Map<String, serde_json::Value>,
    ) -> Result<usize, String> {
        self.export_settings_noting(path, settings).map(|(n, _)| n)
    }

    /// The export's file rules, with only the settings applied.
    fn export_settings_noting(
        &self,
        path: &str,
        settings: &serde_json::Map<String, serde_json::Value>,
    ) -> Result<(usize, Vec<String>), String> {
        ID51_CARD.patch_icf(path, |icf| {
            check_is_an_id51_file(icf)?;
            Ok(settings::apply_settings(icf.image_mut(), settings))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::radios::driver::DriverCapabilities;

    /// The card file and nothing else: no cable session is claimed.
    #[test]
    fn advertises_the_card_file_only() {
        let caps = DriverCapabilities::of(&DRIVER);
        assert!(caps.export && caps.write_card_settings);
        assert!(!caps.download_image && !caps.program_image && !caps.read_settings && !caps.write_settings);
    }

    #[test]
    fn identify_explains_the_card_path_instead_of_opening_a_port() {
        let Err(err) = DRIVER.identify("/dev/cu.nonesuch") else {
            panic!("identify claimed to reach a radio over a cable");
        };
        assert!(err.contains("microSD"), "{err}");
    }
}
