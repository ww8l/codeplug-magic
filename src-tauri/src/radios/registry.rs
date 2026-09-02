//! Static registry of live-USB radio drivers (Chunk 3.2).
//!
//! One list, `all_drivers()`, holds every compiled-in driver as a
//! `&'static dyn RadioDriver`. Lookups resolve a `radio_models.driver_key`
//! (see migration 0014) to its driver. The command layer (3.6) dispatches
//! through here instead of matching on model names, so adding a radio means
//! adding its folder under `radios/<key>/` and one line to `all_drivers()` —
//! never a new arm in `lib.rs`, `Codeplugs.tsx`, or `export.rs`.
//!
//! Registered: `baofeng_uv5r` (3.3), `tidradio_tdh3` (3.4), `anytone_atd890uv`
//! (3.5), and `yaesu_ft5d` (issue #32, scaffolding — identify only, no
//! capabilities yet). `driver_for_key` is live as of 3.6c —
//! `identify_radio` and `download_image` dispatch through it, joined by the
//! settings and call-sign DB commands in 3.6d — so a lookup returning `None`
//! now means a bad `driver_key`, not an unmigrated radio.
//!
//! File export (3.8) looks up by `export_format` instead — see
//! [`exporter_for_format`] for why that is not the same lookup.

use super::driver::{CodeplugExporter, RadioDriver};
use crate::models::RadioModel;

/// Every driver compiled into the app. Order is not significant — lookups are
/// by `key()`, which is unique. (A static array rather than a slice literal:
/// references to statics aren't const-promotable inside a returned temporary.)
static DRIVERS: [&dyn RadioDriver; 8] = [
    &super::baofeng_uv5r::DRIVER,
    &super::binteradio_bt9000::DRIVER,
    &super::tidradio_tdh3::DRIVER,
    &super::anytone_atd890uv::DRIVER,
    &super::yaesu_ft5d::DRIVER,
    &super::icom_id52::DRIVER,
    &super::kenwood_thd75::DRIVER,
    &super::kenwood_thd72::DRIVER,
];

pub(crate) fn all_drivers() -> &'static [&'static dyn RadioDriver] {
    &DRIVERS
}

/// Resolve a `driver_key` to its driver, or `None` if the key is unknown or no
/// driver has been compiled in for it yet.
pub(crate) fn driver_for_key(key: &str) -> Option<&'static dyn RadioDriver> {
    all_drivers().iter().copied().find(|d| d.key() == key)
}

/// Resolve a radio model to its live-USB driver. `None` for export-only models
/// (NULL `driver_key`) or any key without a compiled-in driver.
pub(crate) fn driver_for_model(model: &RadioModel) -> Option<&'static dyn RadioDriver> {
    driver_for_key(model.driver_key.as_deref()?)
}

/// Resolve a `radio_models.export_format` key to the exporter that handles it.
///
/// Deliberately keyed on the FORMAT, not on the model's driver: an export-only
/// model (NULL `driver_key`) has no driver to route through, but it still names
/// a format, and any driver's exporter that claims that format can write it.
/// `None` means no exporter claims the key — the caller falls back to the
/// shared CHIRP-style CSV.
pub(crate) fn exporter_for_format(format: &str) -> Option<&'static dyn CodeplugExporter> {
    all_drivers()
        .iter()
        .copied()
        .filter_map(|d| d.as_codeplug_exporter())
        .find(|e| e.export_format() == format)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The generic `identify_radio` / `download_image` commands (3.6c) resolve a
    /// `driver_key` string straight from the DB, so an unknown or renamed key is
    /// a user-visible failure rather than a compile error. Lock the keys.
    #[test]
    fn every_driver_key_resolves_and_is_unique() {
        for key in [
            "baofeng_uv5r",
            "binteradio_bt9000",
            "tidradio_tdh3",
            "anytone_atd890uv",
            "yaesu_ft5d",
            "icom_id52",
            "kenwood_thd75",
        ] {
            let d = driver_for_key(key).unwrap_or_else(|| panic!("no driver for '{key}'"));
            assert_eq!(d.key(), key);
        }
        assert!(driver_for_key("nonesuch").is_none());

        let mut keys: Vec<&str> = all_drivers().iter().map(|d| d.key()).collect();
        let before = keys.len();
        keys.sort_unstable();
        keys.dedup();
        assert_eq!(keys.len(), before, "duplicate driver_key in the registry");
    }

    /// The generic settings commands (3.6d) dispatch on these two accessors, so
    /// the read/write split is what decides which radios each command accepts.
    /// The UV-5R reads settings but cannot write them on its own (its settings
    /// ride along in the image `program_codeplug` uploads), which is exactly why
    /// `SettingsReader` and `SettingsWriter` are separate traits. The FT5D does
    /// neither: it is scaffolding until a modality is proven on the radio
    /// (issue #32), so it must not be offered a settings action at all.
    #[test]
    fn settings_capabilities_match_each_radios_reality() {
        for d in all_drivers() {
            let (expect_read, expect_write) = match d.key() {
                "baofeng_uv5r" => (true, false),
                // All three card radios: no cable, so no cable settings session.
                // The FT5D's settings ride in its microSD backup, the ID-52's in
                // its `.icf` and the TH-D75's in its `.d75`, none of which goes
                // through these traits.
                "yaesu_ft5d" | "icom_id52" | "kenwood_thd75" => (false, false),
                // TH-D72: reads and writes its 19 menu parameters over the
                // `MU` ASCII command — no clone session involved, which is why
                // it claims both halves where the card radios claim neither.
                "kenwood_thd72" => (true, true),
                // BT-9000: both. Like the TH-D72 it reads and writes its own
                // settings, but through a clone session rather than an ASCII
                // command — and the WRITE is narrowed to the one 256-byte
                // function segment, because the same transport could rewrite
                // all 960 channel records to change a squelch level.
                //
                // ⚠ The schema behind this is deliberately SHORT. It carries
                // only the fields whose encoding was settled on the radio's own
                // screen; the measurement sheet grades ~43 candidates and the
                // generator withholds the rest. A wrong encoding here is not
                // caught anywhere downstream — this radio stored 127 in fields
                // whose maxima are 9, 2, 3 and 1 (issue #43).
                "binteradio_bt9000" => (true, true),
                _ => (true, true),
            };
            assert_eq!(
                d.as_settings_reader().is_some(),
                expect_read,
                "{} settings-read capability",
                d.key()
            );
            assert_eq!(
                d.as_settings_writer().is_some(),
                expect_write,
                "{} settings-write capability",
                d.key()
            );
        }
    }

    /// Only the AnyTone carries an on-board call-sign DB, so `write_callsign_db`
    /// must reject the others by name rather than opening their port.
    #[test]
    fn only_the_anytone_writes_a_callsign_db() {
        for d in all_drivers() {
            assert_eq!(
                d.as_callsign_db_writer().is_some(),
                d.key() == "anytone_atd890uv",
                "{} call-sign DB capability",
                d.key()
            );
        }
    }

    /// `generate_codeplug` (3.8) resolves an exporter from the model's
    /// `export_format` string, so a renamed key silently downgrades a DMR export
    /// to the CHIRP analog CSV instead of failing. Lock the key, and lock the
    /// fact that no exporter claims `chirp_csv` — that path is the fallback, not
    /// a driver's.
    #[test]
    fn export_formats_are_unique_and_chirp_stays_the_fallback() {
        let anytone = exporter_for_format("anytone_csv").expect("no exporter for 'anytone_csv'");
        assert_eq!(anytone.export_format(), "anytone_csv");
        assert!(exporter_for_format("chirp_csv").is_none());
        assert!(exporter_for_format("nonesuch").is_none());

        let mut formats: Vec<&str> = all_drivers()
            .iter()
            .copied()
            .filter_map(|d| d.as_codeplug_exporter())
            .map(|e| e.export_format())
            .collect();
        let before = formats.len();
        formats.sort_unstable();
        formats.dedup();
        assert_eq!(formats.len(), before, "two exporters claim one export_format");
    }

    /// `identify` lives on the base trait precisely so the AnyTone — which has no
    /// `ImageProgrammer` — is still reachable from the generic identify command.
    #[test]
    fn all_drivers_identify_but_only_clone_radios_download_images() {
        for d in all_drivers() {
            let expect_image = matches!(
                d.key(),
                "baofeng_uv5r" | "tidradio_tdh3" | "kenwood_thd72" | "binteradio_bt9000"
            );
            assert_eq!(
                d.as_image_programmer().is_some(),
                expect_image,
                "{} image-programmer capability",
                d.key()
            );
        }
    }
}
