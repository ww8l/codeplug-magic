//! AnyTone AT-D890UV **non-channel settings** (the "radio profile") —
//! re-export shim.
//!
//! Everything real lives in the driver (`radios/anytone_atd890uv/settings.rs`):
//! the generated field table, the General/Boot settings decode + encode, and
//! the `SettingsReader` / `SettingsWriter` device I/O. The Tauri command that
//! used to live here (`read_anytone_settings_for_profile`) was replaced in
//! Chunk 3.6d by the registry-dispatched `program::read_radio_settings`, which
//! serves every radio through one command.
//!
//! The module stays as a `pub use` so the reverse-engineering examples and
//! `anytone_program.rs` keep resolving `commands::anytone_settings::*`; it folds
//! away with the rest of the per-radio command surface in 3.6e.

pub use crate::radios::anytone_atd890uv::settings::*;
