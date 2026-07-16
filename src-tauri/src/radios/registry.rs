//! Static registry of live-USB radio drivers (Chunk 3.2).
//!
//! One list, `all_drivers()`, holds every compiled-in driver as a
//! `&'static dyn RadioDriver`. Lookups resolve a `radio_models.driver_key`
//! (see migration 0014) to its driver. The command layer (3.6) dispatches
//! through here instead of matching on model names, so adding a radio means
//! adding its folder under `radios/<key>/` and one line to `all_drivers()` —
//! never a new arm in `lib.rs`, `Codeplugs.tsx`, or `export.rs`.
//!
//! Drivers are registered here as they're migrated: 3.3 (UV-5R), 3.4 (TD-H3),
//! 3.5 (AnyTone D890UV). The list is empty until then, so the lookups return
//! `None` and callers fall back to the current per-command code paths.
//!
//! `dead_code` is allowed while nothing calls these yet — remove the attribute
//! once 3.6 routes commands through the registry.
#![allow(dead_code)]

use super::driver::RadioDriver;
use crate::models::RadioModel;

/// Every driver compiled into the app. Order is not significant — lookups are
/// by `key()`, which is unique. (A static array rather than a slice literal:
/// references to statics aren't const-promotable inside a returned temporary.)
static DRIVERS: [&dyn RadioDriver; 1] = [
    &super::baofeng_uv5r::DRIVER,
    // Populated in 3.4–3.5:
    //   &super::tidradio_tdh3::DRIVER,
    //   &super::anytone_atd890uv::DRIVER,
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
