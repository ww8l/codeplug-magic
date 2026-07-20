//! Live-USB radio driver trait and its capability sub-traits (Chunk 3.1).
//!
//! A driver bundles everything needed to talk to one radio over a cable. The
//! base [`RadioDriver`] trait carries identity (`key`, `display_name`, `baud`)
//! plus a set of `as_*` accessors, one per capability. Each accessor defaults
//! to `None`; a concrete driver overrides only the ones it supports. Frontend
//! capability flags are therefore *derived* from the trait (see
//! [`DriverCapabilities`]) and never hand-maintained alongside it.
//!
//! ## Why the trait is synchronous
//!
//! Serial I/O in this codebase is blocking: the async Tauri commands resolve
//! DB state with `.await`, then hand the work to `spawn_blocking` around
//! synchronous `serialport` calls (see `commands/program.rs`). Keeping these
//! trait methods synchronous makes the trait object-safe, so the registry can
//! store `&'static dyn RadioDriver` without pulling in `async_trait` or boxed
//! futures. The async + DB responsibility stays in the command layer; the
//! driver only sees already-resolved data and a port name.
//!
//! ## Status
//!
//! All three drivers live under `radios/<key>/`: 3.3 (UV-5R), 3.4 (TD-H3), 3.5
//! (AnyTone D890UV). The command layer dispatches through the registry for
//! `identify`/`download_image` (3.6c); settings, call-sign DB, and the
//! remaining per-radio commands follow in 3.6d/3.6e.
//!
//! `dead_code` is still allowed here: [`ChannelWriter`] has no implementor yet
//! (no shipping radio is programmed channels-only), and a few trait methods are
//! reached only from the command paths that 3.6d/3.6e will rewire.
#![allow(dead_code)]

use std::path::Path;

use crate::commands::export::{
    ChannelScanListOverride, CodeplugGroup, CodeplugScanList, ExpandedChannel, SlotChannel,
};
use crate::models::RadioModel;

/// A radio's self-reported identity, read during the connect handshake.
pub(crate) struct RadioIdentity {
    /// Which handshake matched: the UV-5R magic key `"UV5R_ORIG"`, or the model
    /// token the radio returned (`"P31183"`, `"ID890UV"`) for radios whose
    /// handshake has no magic table.
    pub matched: String,
    /// The raw ident bytes the radio returned, as hex.
    pub ident_hex: String,
    /// The same bytes rendered as printable ASCII, when the radio returns a
    /// readable token. `None` for radios whose ident is not text (UV-5R).
    pub ident_ascii: Option<String>,
}

/// One channel decoded out of a freshly-read image for the download sanity
/// sample — the "is this read real?" table the program dialogs show. A superset
/// of what the image-clone radios decode: `shift` and `mode` are `None` on
/// radios that do not report them (UV-5R).
///
/// Deliberately separate from each driver's own richer `*DecodedChannel` (which
/// still backs the per-radio program/verify results): this one exists only to
/// give the generic `download_image` command a single wire shape.
#[derive(Debug, Clone, serde::Serialize)]
pub struct DecodedChannelSample {
    pub index: usize,
    pub name: String,
    pub rx_mhz: f64,
    /// TX shift as the radio applies it, e.g. `"−0.600"`, `"+5.000"`,
    /// `"RX-only"`, or `""` for simplex. `None` if the driver doesn't decode it.
    pub shift: Option<String>,
    /// Human-readable tone summary, e.g. `"T 88.5"`, `"DTCS 023 N"`, `"—"`.
    pub tone: String,
    /// The radio's stored TX power level, e.g. `"High"` / `"Low"`.
    pub power: String,
    /// `"FM"` (wide) or `"NFM"` (narrow). `None` if the driver doesn't decode it.
    pub mode: Option<String>,
}

/// One DMR contacts / call-sign DB entry, as the command layer hands it to a
/// driver. Radios encode this into their own on-flash layout; the DB→DTO
/// mapping lives in the command layer so drivers stay storage-agnostic.
pub(crate) struct CallsignRecord {
    pub dmr_id: u32,
    pub callsign: String,
    pub name: String,
    pub city: Option<String>,
    pub state: Option<String>,
    pub country: Option<String>,
}

/// Base trait every live-USB radio driver implements. Object-safe: stored as
/// `&'static dyn RadioDriver` in the registry (see `radios/registry.rs`, 3.2).
pub(crate) trait RadioDriver: Send + Sync {
    /// Stable key. Matches `radio_models.driver_key` and the `radios/<key>/`
    /// folder name. Never renamed once shipped (it's a persisted foreign key).
    fn key(&self) -> &'static str;

    /// Human-readable label for logs and error messages.
    fn display_name(&self) -> &'static str;

    /// Serial baud rate for the handshake and bulk transfer.
    fn baud(&self) -> u32;

    /// Harmless connect handshake: confirm the right radio is on `port`. Reads
    /// no memory, so it can never affect the radio's contents.
    ///
    /// This sits on the base trait rather than on a capability sub-trait because
    /// *every* driver can identify — it's how the user proves the cable works
    /// before committing to anything. Notably the AnyTone has no
    /// [`ImageProgrammer`] (it is programmed as a whole codeplug from the DB,
    /// not as a memory image), yet still handshakes.
    fn identify(&self, port: &str) -> Result<RadioIdentity, String>;

    // --- Capability accessors -------------------------------------------------
    // Default `None`; a driver overrides the ones it supports. The derived
    // `DriverCapabilities` (below) reads these, so a capability is advertised to
    // the frontend if and only if the accessor returns `Some`.

    fn as_image_programmer(&self) -> Option<&dyn ImageProgrammer> {
        None
    }
    fn as_settings_programmer(&self) -> Option<&dyn SettingsProgrammer> {
        None
    }
    fn as_channel_writer(&self) -> Option<&dyn ChannelWriter> {
        None
    }
    fn as_codeplug_programmer(&self) -> Option<&dyn CodeplugProgrammer> {
        None
    }
    fn as_callsign_db_writer(&self) -> Option<&dyn CallsignDbWriter> {
        None
    }
    fn as_codeplug_exporter(&self) -> Option<&dyn CodeplugExporter> {
        None
    }
    fn as_diagnostics(&self) -> Option<&dyn DriverDiagnostics> {
        None
    }
}

/// Clone-mode radios: the whole memory image is read, patched, and written back
/// as one blob (Baofeng UV-5R, TIDRADIO TD-H3).
/// `Send + Sync` because the command layer resolves the driver on the async
/// side and then moves the `&'static dyn` reference into `spawn_blocking`.
pub(crate) trait ImageProgrammer: Send + Sync {
    /// Read the radio's full memory image (raw codeplug bytes), returning the
    /// handshake identity alongside it.
    ///
    /// The identity comes back from *this* session rather than the caller
    /// running [`RadioDriver::identify`] first: the clone protocols hand the
    /// ident bytes straight to the block reader, and opening the port twice
    /// would mean two handshakes where the hardware-verified flow has one.
    fn download_image(&self, port: &str) -> Result<(RadioIdentity, Vec<u8>), String>;

    /// Decode the programmed channels out of an image for the download sanity
    /// sample. Pure — no radio required, so it's unit-testable.
    fn decode_sample(&self, image: &[u8]) -> Vec<DecodedChannelSample>;

    /// Write a prepared memory image back to the radio.
    fn upload_image(&self, port: &str, image: &[u8]) -> Result<(), String>;

    /// Patch `channels` (slot-resolved, see `export::resolve_codeplug_slots`)
    /// into `base` (a freshly-read image), returning the image to write. Kept
    /// separate from I/O so it's unit-testable without hardware.
    fn build_image(
        &self,
        model: &RadioModel,
        channels: &[SlotChannel],
        base: &[u8],
    ) -> Result<Vec<u8>, String>;

    /// Full clone-mode program: read the current image, patch the channels in,
    /// write it back. Returns the image that was written. Default composition of
    /// the three primitives above — most drivers won't override it.
    fn program_codeplug(
        &self,
        port: &str,
        model: &RadioModel,
        channels: &[SlotChannel],
    ) -> Result<Vec<u8>, String> {
        let (_ident, base) = self.download_image(port)?;
        let image = self.build_image(model, channels, &base)?;
        self.upload_image(port, &image)?;
        Ok(image)
    }
}

/// Radios whose non-channel settings (General Settings, keys, display, …) are
/// read and written against a JSON schema (TD-H3, AnyTone D890UV).
pub(crate) trait SettingsProgrammer {
    /// Read current settings off the radio, decoded into the schema's shape.
    fn read_settings(&self, port: &str, schema_json: &str) -> Result<serde_json::Value, String>;

    /// Encode `settings` per the schema and write them to the radio.
    fn write_settings(
        &self,
        port: &str,
        settings: &serde_json::Value,
        schema_json: &str,
    ) -> Result<(), String>;
}

/// Everything a driver needs to program a whole codeplug, resolved from the
/// database by the command layer. Drivers are synchronous and storage-agnostic,
/// so every row a planner might need is gathered up front — a driver never
/// issues a query. See `export::resolve_codeplug_payload`, which builds this.
///
/// Contacts/talkgroups are deliberately absent: they are derived from
/// `channels` (first-use order of `tg_number`), not stored separately.
pub(crate) struct CodeplugPayload<'a> {
    pub model: &'a RadioModel,
    /// The codeplug's channel lists in assignment order. One group becomes one
    /// zone on zone-capable radios, one bank on bank-capable ones.
    pub groups: &'a [CodeplugGroup],
    /// DMR-expanded channel rows in memory-slot order (a repeater carrying N
    /// talkgroups occupies N entries).
    pub channels: &'a [ExpandedChannel],
    /// Scan lists assigned to this codeplug, with their member channel ids.
    pub scan_lists: &'a [CodeplugScanList],
    /// Explicit per-channel scan-list assignments (`codeplug_channel_scan_lists`).
    /// A list named here is *manually managed* — see `scan-list-per-channel-field`.
    pub scan_list_overrides: &'a [ChannelScanListOverride],
}

/// A channel that could not be programmed, and why. Surfaced in the preview so
/// the user can fix the codeplug before touching the radio.
#[derive(Debug, Clone, serde::Serialize)]
pub struct SkippedChannel {
    pub name: String,
    pub reason: String,
}

/// Dry-run summary of what [`CodeplugProgrammer::program`] would write. Pure —
/// no radio required, so the UI can show it before the user commits.
#[derive(Debug, Clone, serde::Serialize)]
pub struct CodeplugPreview {
    pub radio: String,
    pub channels: usize,
    pub zones: usize,
    pub scan_lists: usize,
    pub contacts: usize,
    pub zone_names: Vec<String>,
    pub scan_list_names: Vec<String>,
    pub skipped: Vec<SkippedChannel>,
    pub warnings: Vec<String>,
}

/// Outcome of a committed full-replace program.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ProgramReport {
    pub channels_written: usize,
    /// Previously-programmed slots beyond the new channel set that were blanked.
    pub slots_cleared: usize,
    pub zones_written: usize,
    pub zones_cleared: usize,
    pub scan_lists_written: usize,
    pub scan_lists_cleared: usize,
    pub contacts_written: usize,
    pub contacts_cleared: usize,
    /// Flash regions actually rewritten, as hex addresses — driver-defined
    /// granularity (0x4000 windows/banks on the AnyTone).
    pub windows_written: Vec<String>,
    pub backup_path: String,
    /// Image to hand back for a post-commit byte-verify.
    pub expected_path: String,
    pub warnings: Vec<String>,
    pub note: String,
}

/// Radios programmed as a whole codeplug — channels *plus* zones, scan lists,
/// and contacts — in one session, from database state rather than a memory
/// image (AnyTone D890UV).
///
/// This is distinct from [`ImageProgrammer`] (whole-blob clone radios) and from
/// [`ChannelWriter`] (channels only). It exists because the AnyTone flow is
/// DB-shaped: it needs list membership and scan-list rows that a flat channel
/// slice cannot carry, and it writes a mandatory backup before any byte goes
/// out. The command layer resolves the payload and owns the backup directory;
/// the driver owns planning and the hardware session.
/// `Send + Sync` because the command layer resolves the driver on the async
/// side and then moves the `&'static dyn` reference into `spawn_blocking` to
/// run the serial session.
pub(crate) trait CodeplugProgrammer: Send + Sync {
    /// Summarize what [`program`](Self::program) would write. Pure — must not
    /// open the port. Errors only on structural problems (wrong model, over
    /// capacity); per-channel issues become `skipped` or `warnings`.
    fn preview(&self, payload: &CodeplugPayload) -> Result<CodeplugPreview, String>;

    /// Full-replace program: write the payload as the radio's entire
    /// channel/zone/scan-list/contact set. Writes a backup and an expected
    /// image under `backup_dir` *before* the first byte goes to the radio.
    fn program(
        &self,
        port: &str,
        payload: &CodeplugPayload,
        backup_dir: &Path,
    ) -> Result<ProgramReport, String>;
}

/// Radios programmed by targeted per-channel/bank writes rather than a full
/// image clone. Channels only — see [`CodeplugProgrammer`] when zones, scan
/// lists, or contacts are also involved.
pub(crate) trait ChannelWriter {
    /// Write `channels` to the radio, returning the number of channels written.
    fn program_channels(
        &self,
        port: &str,
        model: &RadioModel,
        channels: &[ExpandedChannel],
    ) -> Result<usize, String>;
}

/// Radios with an on-board DMR contacts / call-sign database (AnyTone D890UV).
pub(crate) trait CallsignDbWriter {
    /// Encode and write `records` to the radio's call-sign DB region, returning
    /// the number of entries written.
    fn write_callsign_db(&self, port: &str, records: &[CallsignRecord])
        -> Result<usize, String>;
}

/// File-format exporters (CHIRP-style CSV, AnyTone dual-CSV bundle, …). The
/// `export_format` key matches `radio_models.export_format`.
pub(crate) trait CodeplugExporter {
    /// Format key this exporter handles, e.g. `"anytone_csv"`.
    fn export_format(&self) -> &'static str;

    /// Write the codeplug file(s) rooted at `path`, returning channels written.
    fn export(
        &self,
        path: &str,
        channels: &[ExpandedChannel],
        model: &RadioModel,
    ) -> Result<usize, String>;
}

/// Optional low-level diagnostics used during protocol reverse-engineering
/// (raw memory dumps, dump diffing). Not surfaced in the normal UI.
pub(crate) trait DriverDiagnostics {
    /// Deterministic raw dump of a memory region, for byte-level comparison.
    fn dump_raw(&self, port: &str, start: u32, len: u32) -> Result<Vec<u8>, String>;
}

/// Frontend-facing capability flags, derived from a driver's `as_*` accessors.
/// Serialized to the UI so dialogs enable actions from ground truth (the trait
/// impls) rather than a hand-maintained list.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub(crate) struct DriverCapabilities {
    pub program_image: bool,
    pub program_settings: bool,
    pub write_channels: bool,
    pub program_codeplug: bool,
    pub write_callsign_db: bool,
    pub export: bool,
    pub diagnostics: bool,
}

impl DriverCapabilities {
    /// Compute the capability set for a driver from its accessors.
    pub fn of(driver: &dyn RadioDriver) -> Self {
        Self {
            program_image: driver.as_image_programmer().is_some(),
            program_settings: driver.as_settings_programmer().is_some(),
            write_channels: driver.as_channel_writer().is_some(),
            program_codeplug: driver.as_codeplug_programmer().is_some(),
            write_callsign_db: driver.as_callsign_db_writer().is_some(),
            export: driver.as_codeplug_exporter().is_some(),
            diagnostics: driver.as_diagnostics().is_some(),
        }
    }
}
