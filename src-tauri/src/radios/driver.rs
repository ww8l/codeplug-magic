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
//! `identify`/`download_image` (3.6c) and for settings read/write + the
//! call-sign DB (3.6d); the remaining per-radio commands follow in 3.6e.
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
    /// Free-text remarks from the source library, for radios with a comment
    /// column in their DB entry.
    pub comment: Option<String>,
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
    fn as_settings_reader(&self) -> Option<&dyn SettingsReader> {
        None
    }
    fn as_settings_writer(&self) -> Option<&dyn SettingsWriter> {
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

    /// Full clone-mode program: back up, patch channels (and the profile's
    /// settings, when it carries them), write, and read back to verify — all in
    /// ONE session, which is why this is a single trait method rather than the
    /// caller composing the primitives above. Verification is best-effort: a
    /// write whose blocks all ack'd succeeded, so a failed read-back is reported
    /// in `note`, not raised as an error.
    fn program_codeplug(
        &self,
        port: &str,
        req: &ImageProgramRequest,
    ) -> Result<CodeplugProgramReport, String>;
}

/// Everything a clone radio needs to program a codeplug, resolved from the
/// database by the command layer (drivers never query). Mirrors
/// [`CodeplugPayload`]'s role for [`CodeplugProgrammer`] radios.
pub(crate) struct ImageProgramRequest<'a> {
    pub model: &'a RadioModel,
    /// Slot-resolved channels, packed from slot 0 (see
    /// `export::resolve_codeplug_slots`).
    pub channels: &'a [SlotChannel],
    /// The radio profile's non-channel settings and the model's schema, when the
    /// profile carries them. `None` writes channels + names only. Passing them
    /// makes the profile authoritative over every editable setting.
    pub settings: Option<(&'a serde_json::Value, &'a str)>,
    /// Where the mandatory pre-write backup goes; the driver names the file.
    pub backup_dir: &'a Path,
    /// Codeplug name, slugged into that filename so several codeplugs for one
    /// radio stay distinguishable when restoring.
    pub label: &'a str,
}

/// Unified outcome of a codeplug program, for every radio. The union of what
/// the clone-mode and DB-driven paths report; fields a given radio cannot
/// produce are `None`/zero/empty.
///
/// Clone radios fill `verified`/`channels` (they read back in the same session)
/// and leave the zone/scan-list/contact counts at zero — they program channels
/// and settings only. The AnyTone is the inverse: it reboots on commit, so it
/// reports `expected_path` for a fresh-session byte-diff instead of `verified`.
#[derive(Debug, Clone, serde::Serialize)]
pub struct CodeplugProgramReport {
    pub channels_written: usize,
    /// Previously-programmed slots beyond the new channel set that were blanked.
    pub slots_cleared: usize,
    /// Non-channel settings written from the radio profile, or `None` when the
    /// profile carried none and only channels/names went out.
    pub settings_written: Option<usize>,
    pub zones_written: usize,
    pub zones_cleared: usize,
    pub scan_lists_written: usize,
    pub scan_lists_cleared: usize,
    pub contacts_written: usize,
    pub contacts_cleared: usize,
    /// Whether an in-session read-back matched. `None` when the driver cannot
    /// read back in the same session (the AnyTone reboots on commit).
    pub verified: Option<bool>,
    /// Set when verification could not run, found differences, or the radio
    /// needs post-write handling.
    pub note: Option<String>,
    pub backup_path: String,
    /// Image for a fresh-session byte-verify, on drivers that write one.
    pub expected_path: Option<String>,
    /// Flash regions rewritten, as hex addresses — driver-defined granularity.
    /// Empty on clone radios, which rewrite whole ranges.
    pub windows_written: Vec<String>,
    /// A sample of the channels actually on the radio after writing, read back.
    pub channels: Vec<DecodedChannelSample>,
    /// Channels that could not be programmed, and why.
    pub skipped: Vec<SkippedChannel>,
    pub warnings: Vec<String>,
}

/// What one settings-read session captured.
///
/// The raw bytes come back alongside the decoded values because the read is a
/// *single* hardware session: the clone radios decode settings out of the very
/// image they just downloaded, and the AnyTone out of the windows it just read.
/// Handing the command layer only `settings` would force it to open the port a
/// second time to produce the safety backup — the same one-session rule that
/// shaped [`ImageProgrammer::download_image`].
pub(crate) struct SettingsCapture {
    /// Decoded settings, keyed and shaped like the profile form.
    pub settings: serde_json::Value,
    /// Everything the session read, for the command layer to save as a backup.
    /// Driver-defined format: the whole memory image on clone radios, a
    /// `[addr:BE][len:BE][data]` window dump on the AnyTone.
    pub backup: Vec<u8>,
    /// Extension for that backup file, without the dot (`"img"`, `"bin"`).
    pub backup_ext: &'static str,
}

/// Outcome of a committed settings write. The union of what the per-radio write
/// paths report; fields a given radio cannot produce are `None`/empty.
#[derive(Debug, Clone, serde::Serialize)]
pub struct SettingsWriteReport {
    /// Settings fields actually written to the radio.
    pub fields_written: usize,
    /// Whether an in-session read-back matched. `None` when the driver cannot
    /// read back in the same session (the AnyTone reboots on END/commit, so its
    /// verify is a separate fresh-session byte-diff against `expected_path`).
    pub verified: Option<bool>,
    /// How to interpret the result — set when verification could not run, found
    /// differences, or the radio needs post-write handling.
    pub note: Option<String>,
    /// Absolute path of the mandatory pre-write backup.
    pub backup_path: String,
    /// Absolute path of an `.expected.bin` image for a fresh-session byte
    /// verify, on drivers that write one.
    pub expected_path: Option<String>,
    /// Flash regions rewritten, as hex addresses — driver-defined granularity.
    /// Empty on whole-image clone radios, which rewrite the entire main range.
    pub windows_written: Vec<String>,
}

/// Radios whose non-channel settings (General Settings, keys, display, …) can be
/// READ off the radio and decoded into a profile's shape (UV-5R, TD-H3, AnyTone
/// D890UV).
///
/// Split from [`SettingsWriter`] because reading and writing settings are not
/// the same capability: the UV-5R has no standalone settings-write path (its
/// settings go out inside `program_codeplug`), so folding both halves into one
/// trait would force it to advertise a write it cannot perform.
/// `Send + Sync`: the command layer resolves the driver on the async side and
/// moves the `&'static dyn` into `spawn_blocking`.
pub(crate) trait SettingsReader: Send + Sync {
    /// Read current settings off the radio, decoded into the schema's shape,
    /// with the raw bytes the session captured. Drivers with a built-in field
    /// table may ignore `schema_json`.
    fn read_settings(&self, port: &str, schema_json: &str) -> Result<SettingsCapture, String>;
}

/// Radios that can have a saved profile's settings pushed to them on their own,
/// without rewriting channels (TD-H3, AnyTone D890UV).
///
/// The driver owns the backup filenames (they differ per radio and some are
/// scanned by name — see `latest_anytone_program`), so it is handed the
/// directory rather than finished paths.
pub(crate) trait SettingsWriter: Send + Sync {
    /// Encode `settings` per the schema and write them to the radio, taking the
    /// mandatory pre-write backup under `backup_dir` first.
    fn write_settings(
        &self,
        port: &str,
        settings: &serde_json::Value,
        schema_json: &str,
        backup_dir: &Path,
    ) -> Result<SettingsWriteReport, String>;
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

/// Outcome of a committed call-sign DB write.
#[derive(Debug, Clone, serde::Serialize)]
pub struct CallsignDbReport {
    /// Entries actually encoded and written.
    pub entries_written: usize,
    /// Flash regions rewritten, as hex addresses.
    pub windows_written: Vec<String>,
    /// How to verify — this region has no golden image to byte-diff, so
    /// verification is functional (the radio's own caller-ID screen).
    pub note: String,
}

/// Radios with an on-board DMR contacts / call-sign database (AnyTone D890UV).
/// `Send + Sync` for the same `spawn_blocking` reason as the other capabilities.
pub(crate) trait CallsignDbWriter: Send + Sync {
    /// Encode and write `records` to the radio's call-sign DB region.
    fn write_callsign_db(
        &self,
        port: &str,
        records: &[CallsignRecord],
    ) -> Result<CallsignDbReport, String>;
}

/// Everything an exporter gets to write a codeplug file. The file-side twin of
/// [`CodeplugPayload`], and it grows the same way: one struct rather than an
/// ever-widening argument list.
pub(crate) struct ExportRequest<'a> {
    /// Included rows in memory-slot order. By reference because the export
    /// command filters a larger expansion down without copying it.
    pub channels: &'a [&'a ExpandedChannel],
    /// The same channels grouped by the channel list they came from — the unit
    /// that becomes one bank (or zone) on radios that have them. Flat formats
    /// like the CHIRP CSV ignore it; the FT5D writer turns each into a bank.
    pub groups: &'a [CodeplugGroup],
    pub model: &'a RadioModel,
    /// The codeplug's radio-profile settings
    /// (`radio_profiles.non_channel_settings`), when it has a profile. Formats
    /// that carry non-channel settings apply them; the CSV formats cannot.
    pub profile_settings: Option<&'a str>,
}

/// File-format exporters (CHIRP-style CSV, AnyTone dual-CSV bundle, …). The
/// `export_format` key matches `radio_models.export_format`.
pub(crate) trait CodeplugExporter {
    /// Format key this exporter handles, e.g. `"anytone_csv"`.
    fn export_format(&self) -> &'static str;

    /// Write the codeplug file(s) rooted at `path`, returning channels written.
    fn export(&self, path: &str, req: &ExportRequest) -> Result<usize, String>;
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
pub struct DriverCapabilities {
    pub program_image: bool,
    pub read_settings: bool,
    pub write_settings: bool,
    pub write_channels: bool,
    pub program_codeplug: bool,
    pub write_callsign_db: bool,
    pub export: bool,
    pub diagnostics: bool,
}

impl DriverCapabilities {
    /// Compute the capability set for a driver from its accessors.
    /// `pub(crate)` while the struct itself is `pub`: the flags cross the Tauri
    /// boundary as a command result, but `RadioDriver` never leaves the crate.
    pub(crate) fn of(driver: &dyn RadioDriver) -> Self {
        Self {
            program_image: driver.as_image_programmer().is_some(),
            read_settings: driver.as_settings_reader().is_some(),
            write_settings: driver.as_settings_writer().is_some(),
            write_channels: driver.as_channel_writer().is_some(),
            program_codeplug: driver.as_codeplug_programmer().is_some(),
            write_callsign_db: driver.as_callsign_db_writer().is_some(),
            export: driver.as_codeplug_exporter().is_some(),
            diagnostics: driver.as_diagnostics().is_some(),
        }
    }
}
