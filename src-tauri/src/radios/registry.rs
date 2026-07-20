//! Static registry of live-USB radio drivers (Chunk 3.2).
//!
//! One list, `all_drivers()`, holds every compiled-in driver as a
//! `&'static dyn RadioDriver`. Lookups resolve a `radio_models.driver_key`
//! (see migration 0014) to its driver. The command layer (3.6) dispatches
//! through here instead of matching on model names, so adding a radio means
//! adding its folder under `radios/<key>/` and one line to `all_drivers()` —
//! never a new arm in `lib.rs`, `Codeplugs.tsx`, or `export.rs`.
//!
//! All three drivers are registered: `baofeng_uv5r` (3.3), `tidradio_tdh3`
//! (3.4), `anytone_atd890uv` (3.5). `driver_for_key` is live as of 3.6c —
//! `identify_radio` and `download_image` dispatch through it — so a lookup
//! returning `None` now means a bad `driver_key`, not an unmigrated radio.

use super::driver::RadioDriver;
use crate::models::RadioModel;

/// Every driver compiled into the app. Order is not significant — lookups are
/// by `key()`, which is unique. (A static array rather than a slice literal:
/// references to statics aren't const-promotable inside a returned temporary.)
static DRIVERS: [&dyn RadioDriver; 3] = [
    &super::baofeng_uv5r::DRIVER,
    &super::tidradio_tdh3::DRIVER,
    &super::anytone_atd890uv::DRIVER,
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

#[cfg(test)]
mod tests {
    use super::*;

    /// The generic `identify_radio` / `download_image` commands (3.6c) resolve a
    /// `driver_key` string straight from the DB, so an unknown or renamed key is
    /// a user-visible failure rather than a compile error. Lock the keys.
    #[test]
    fn every_driver_key_resolves_and_is_unique() {
        for key in ["baofeng_uv5r", "tidradio_tdh3", "anytone_atd890uv"] {
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

    /// `identify` lives on the base trait precisely so the AnyTone — which has no
    /// `ImageProgrammer` — is still reachable from the generic identify command.
    #[test]
    fn all_drivers_identify_but_only_clone_radios_download_images() {
        for d in all_drivers() {
            let expect_image = matches!(d.key(), "baofeng_uv5r" | "tidradio_tdh3");
            assert_eq!(
                d.as_image_programmer().is_some(),
                expect_image,
                "{} image-programmer capability",
                d.key()
            );
        }
    }
}
