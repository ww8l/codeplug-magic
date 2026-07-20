//! Registry-dispatched radio commands + the UV-5R's remaining per-radio ones.
//!
//! Two kinds of command live here. The generic ones resolve a driver through
//! `radios/registry.rs` and serve every radio: `identify_radio` and
//! `download_image` (Chunk 3.6c), then `read_radio_settings`,
//! `write_radio_settings`, and `write_callsign_db` (Chunk 3.6d). The rest are
//! still UV-5R-specific (`program_codeplug`, `restore_image`) and fold into the
//! generic surface in 3.6e; the UV-5R clone protocol they call has lived in
//! `radios/baofeng_uv5r` since Chunk 3.3.
//!
//! Either way this module is only the Tauri layer — DB lookups, backup-file
//! naming, and the result DTOs the frontend renders. `list_serial_ports` is
//! model-agnostic and shared by every radio's dialog.
//!
//! Note which commands take a `driver_key` and which do not: the 3.6c pair is
//! handed a bare port name, which carries no model hint, so the caller must say
//! what it expects to find. The 3.6d trio is anchored to a radio profile, whose
//! model row already carries the key.

use serde::Serialize;
use tauri::{AppHandle, Manager, State};

use crate::commands::dmr_users::select_prioritized_dmr_users;
use crate::commands::export;
use crate::db::AppState;
use crate::error::MapErrString;
use crate::radios::baofeng_uv5r as uv5r;
use crate::radios::baofeng_uv5r::settings as uv5r_settings;
use crate::radios::baofeng_uv5r::DecodedChannel;
use crate::radios::driver::{
    CallsignDbReport, CallsignRecord, DecodedChannelSample, RadioDriver, SettingsWriteReport,
};
use crate::radios::registry;

// ============================================================
// Tauri commands
// ============================================================

#[derive(Serialize)]
pub struct PortInfo {
    pub name: String,
    /// "usb" | "bluetooth" | "pci" | "unknown" — lets the UI highlight the
    /// likely programming cable (usb).
    pub kind: String,
    /// "Vendor Product" for USB ports, when the OS exposes it.
    pub product: Option<String>,
}

/// List serial ports the OS can see. The UV-5R cable shows up as a `usb` port
/// (e.g. `/dev/cu.usbserial-*` on macOS). The UI also allows typing a path.
#[tauri::command]
pub async fn list_serial_ports() -> Result<Vec<PortInfo>, String> {
    let ports = serialport::available_ports().estr()?;
    let mut out: Vec<PortInfo> = ports
        .into_iter()
        .map(|p| {
            let (kind, product) = match &p.port_type {
                serialport::SerialPortType::UsbPort(info) => {
                    let product = match (&info.manufacturer, &info.product) {
                        (Some(m), Some(p)) => Some(format!("{m} {p}")),
                        (None, Some(p)) => Some(p.clone()),
                        (Some(m), None) => Some(m.clone()),
                        _ => None,
                    };
                    ("usb", product)
                }
                serialport::SerialPortType::BluetoothPort => ("bluetooth", None),
                serialport::SerialPortType::PciPort => ("pci", None),
                serialport::SerialPortType::Unknown => ("unknown", None),
            };
            PortInfo {
                name: p.port_name,
                kind: kind.to_string(),
                product,
            }
        })
        .collect();

    // macOS: the serialport crate's IOKit enumeration misses some USB CDC-ACM
    // ("usbmodem") callout devices — notably the TIDRADIO TD-H3's built-in
    // USB-C port. Supplement the list from /dev/cu.* so the radio still shows up
    // in the picker. Dedupe by name so anything already enumerated is untouched.
    #[cfg(target_os = "macos")]
    supplement_macos_usb_ports(&mut out);

    Ok(out)
}

/// Scan `/dev` for USB serial callout devices the crate may have skipped and add
/// any that aren't already listed (kind = "usb").
#[cfg(target_os = "macos")]
fn supplement_macos_usb_ports(out: &mut Vec<PortInfo>) {
    let Ok(entries) = std::fs::read_dir("/dev") else {
        return;
    };
    for entry in entries.flatten() {
        let fname = entry.file_name();
        let fname = fname.to_string_lossy();
        let is_usb_callout = fname.starts_with("cu.usbmodem")
            || fname.starts_with("cu.usbserial")
            || fname.starts_with("cu.wchusbserial")
            || fname.starts_with("cu.SLAB_USBtoUART");
        if !is_usb_callout {
            continue;
        }
        let path = format!("/dev/{fname}");
        if out.iter().any(|p| p.name == path) {
            continue;
        }
        out.push(PortInfo {
            name: path,
            kind: "usb".to_string(),
            product: None,
        });
    }
}

/// Resolve a `radio_models.driver_key` to its compiled-in driver, with an error
/// message the UI can show as-is. Shared by every registry-dispatched command.
fn driver(driver_key: &str) -> Result<&'static dyn RadioDriver, String> {
    registry::driver_for_key(driver_key)
        .ok_or_else(|| format!("no driver is compiled in for '{driver_key}'"))
}

#[derive(Serialize)]
pub struct RadioIdent {
    /// Which handshake matched: a magic key like "UV5R_ORIG" on radios with a
    /// magic table, otherwise the model token the radio reported.
    pub matched_magic: String,
    /// The raw ident bytes the radio returned, as hex.
    pub ident_hex: String,
    /// The same bytes as printable ASCII (e.g. "P31183", "ID890UV"), or null on
    /// radios whose ident is binary.
    pub ident_ascii: Option<String>,
}

/// Harmless handshake: confirm the expected radio is connected and in clone/PC
/// mode. Reads no memory, so it cannot affect the radio's contents.
///
/// Registry-dispatched (Chunk 3.6c) — replaces the former per-radio
/// `identify_radio` / `identify_tdh3` / `identify_anytone` trio. `driver_key`
/// comes from `radio_models.driver_key`; the port alone carries no model hint,
/// so the caller must say which radio it expects.
#[tauri::command]
pub async fn identify_radio(driver_key: String, port: String) -> Result<RadioIdent, String> {
    let driver = driver(&driver_key)?;
    tauri::async_runtime::spawn_blocking(move || {
        let ident = driver.identify(&port)?;
        Ok(RadioIdent {
            matched_magic: ident.matched,
            ident_hex: ident.ident_hex,
            ident_ascii: ident.ident_ascii,
        })
    })
    .await
    .estr()?
}

#[derive(Serialize)]
pub struct DownloadResult {
    pub matched_magic: String,
    pub ident_hex: String,
    pub ident_ascii: Option<String>,
    pub image_bytes: usize,
    /// Absolute path of the saved CHIRP-compatible backup `.img`.
    pub backup_path: String,
    /// Number of programmed (non-empty) channels found in the image.
    pub channel_count: usize,
    /// A sanity sample of decoded channels (first programmed ones) so the user
    /// can eyeball that the read is real before we ever build the write path.
    pub channels: Vec<DecodedChannelSample>,
}

/// Read a clone-mode radio's full memory image and save it as a timestamped
/// backup. This is the non-destructive proof that the protocol port is correct,
/// and it produces the safety backup the write path always takes first.
///
/// Registry-dispatched (Chunk 3.6c) — replaces `download_image` (UV-5R) and
/// `download_tdh3_image`. Only [`ImageProgrammer`] radios are image-clonable;
/// the AnyTone is programmed from DB state rather than a memory image, so its
/// read path stays `download_anytone_image` (a region probe that feeds the
/// importer, not an image download).
#[tauri::command]
pub async fn download_image(
    app: AppHandle,
    driver_key: String,
    port: String,
) -> Result<DownloadResult, String> {
    let driver = driver(&driver_key)?;
    let imager = driver
        .as_image_programmer()
        .ok_or_else(|| format!("{} is not programmed as a memory image", driver.display_name()))?;

    // Resolve the backup directory on the main thread (AppHandle path API).
    let backup_dir = app
        .path()
        .app_data_dir()
        .estr()?
        .join("radio-backups");
    std::fs::create_dir_all(&backup_dir).estr()?;
    let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
    // Backup names stay driver-keyed so a folder of .img files is still
    // self-identifying (previously hardcoded "uv5r-"/"tdh3-").
    let backup_path = backup_dir.join(format!("{driver_key}-{stamp}.img"));

    tauri::async_runtime::spawn_blocking(move || {
        let (ident, image) = imager.download_image(&port)?;

        std::fs::write(&backup_path, &image)
            .map_err(|e| format!("could not write backup {}: {e}", backup_path.display()))?;

        let channels = imager.decode_sample(&image);
        Ok(DownloadResult {
            matched_magic: ident.matched,
            ident_hex: ident.ident_hex,
            ident_ascii: ident.ident_ascii,
            image_bytes: image.len(),
            backup_path: backup_path.to_string_lossy().to_string(),
            channel_count: channels.len(),
            channels: channels.into_iter().take(20).collect(),
        })
    })
    .await
    .estr()?
}

#[derive(Serialize)]
pub struct RadioSettingsRead {
    /// Decoded non-channel settings, keyed by CHIRP setting name with values
    /// shaped like the profile form (booleans, numbers, select labels, text).
    pub settings: serde_json::Value,
    /// How many settings were decoded.
    pub count: usize,
    /// Absolute path of the backup `.img` saved during the read.
    pub backup_path: String,
}

/// What a radio profile resolves to for the settings commands: the driver that
/// speaks to its radio, the schema its values are keyed by, and the profile's
/// own name (for backup filenames).
struct ProfileTarget {
    driver: &'static dyn RadioDriver,
    schema: String,
    profile_name: String,
}

/// Resolve `profile_id` to its driver + settings schema.
///
/// Unlike the 3.6c commands, the settings commands take NO `driver_key`
/// argument: they are anchored to a profile, and a profile's model row already
/// carries the key. (`identify`/`download_image` needed one because a bare port
/// name carries no model hint.)
async fn profile_target(
    pool: &sqlx::SqlitePool,
    profile_id: i64,
) -> Result<ProfileTarget, String> {
    let row: Option<(Option<String>, String, Option<String>, String)> = sqlx::query_as(
        "SELECT rm.driver_key, rm.display_name, rm.non_channel_settings_schema, rp.display_name \
         FROM radio_profiles rp JOIN radio_models rm ON rm.id = rp.radio_model_id \
         WHERE rp.id = ?1",
    )
    .bind(profile_id)
    .fetch_optional(pool)
    .await
    .estr()?;
    let (driver_key, model_name, schema, profile_name) = row.ok_or("radio profile not found")?;
    let driver_key = driver_key
        .ok_or_else(|| format!("{model_name} has no live-radio driver — it is export-only"))?;
    let driver = driver(&driver_key)?;
    let schema = schema
        .ok_or_else(|| format!("{model_name} has no settings schema to decode into"))?;
    Ok(ProfileTarget {
        driver,
        schema,
        profile_name,
    })
}

/// Slugify a profile name for a backup filename: lowercase alphanumerics, runs
/// of anything else collapsed to a single dash, capped at 40 chars.
fn slug(label: &str) -> String {
    let mut out = String::new();
    let mut prev_dash = false;
    for ch in label.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
            prev_dash = false;
        } else if !prev_dash && !out.is_empty() {
            out.push('-');
            prev_dash = true;
        }
    }
    out.trim_end_matches('-').chars().take(40).collect()
}

/// Read the radio's current non-channel settings into a profile's shape. Lets
/// the user pull configuration changes they made on the radio back into the
/// profile (the inverse of writing settings during programming). Whatever the
/// session read is also saved as a backup, exactly like a normal download.
///
/// Registry-dispatched (Chunk 3.6d) — replaces the former
/// `read_radio_settings` (UV-5R) / `read_tdh3_settings_for_profile` /
/// `read_anytone_settings_for_profile` trio, which already returned this same
/// DTO. The backup bytes come back from the driver alongside the decoded
/// values so the read stays ONE hardware session.
#[tauri::command]
pub async fn read_radio_settings(
    app: AppHandle,
    state: State<'_, AppState>,
    profile_id: i64,
    port: String,
) -> Result<RadioSettingsRead, String> {
    let target = profile_target(&state.pool, profile_id).await?;
    let reader = target.driver.as_settings_reader().ok_or_else(|| {
        format!(
            "{} cannot report its settings over the cable",
            target.driver.display_name()
        )
    })?;

    let backup_dir = app.path().app_data_dir().estr()?.join("radio-backups");
    std::fs::create_dir_all(&backup_dir).estr()?;
    let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
    let key = target.driver.key();
    let slug = slug(&target.profile_name);

    tauri::async_runtime::spawn_blocking(move || {
        let capture = reader.read_settings(&port, &target.schema)?;

        // Named after the driver key, matching `download_image`'s convention, so
        // a folder of backups stays self-identifying.
        let ext = capture.backup_ext;
        let backup_path = backup_dir.join(if slug.is_empty() {
            format!("{key}-settings-{stamp}.{ext}")
        } else {
            format!("{key}-settings-{slug}-{stamp}.{ext}")
        });
        std::fs::write(&backup_path, &capture.backup)
            .map_err(|e| format!("could not write backup {}: {e}", backup_path.display()))?;

        let count = capture.settings.as_object().map(|o| o.len()).unwrap_or(0);
        Ok(RadioSettingsRead {
            settings: capture.settings,
            count,
            backup_path: backup_path.to_string_lossy().to_string(),
        })
    })
    .await
    .estr()?
}

/// Push a saved radio profile's non-channel settings to the connected radio,
/// leaving channels untouched. Every driver takes a mandatory backup before the
/// first byte goes out; the returned report says how to verify (in-session
/// read-back on the clone radios, a fresh-session byte-diff against
/// `expected_path` on the AnyTone). Brick-capable — UI-gated with an ack.
///
/// Registry-dispatched (Chunk 3.6d) — replaces `apply_tdh3_profile` and
/// `program_anytone_settings`. The AnyTone command took a `codeplug_id` purely
/// to reach its profile's model; settings belong to the profile, so this one is
/// profile-anchored like the TD-H3's always was.
#[tauri::command]
pub async fn write_radio_settings(
    app: AppHandle,
    state: State<'_, AppState>,
    profile_id: i64,
    port: String,
) -> Result<SettingsWriteReport, String> {
    let target = profile_target(&state.pool, profile_id).await?;
    let writer = target.driver.as_settings_writer().ok_or_else(|| {
        format!(
            "{} cannot have settings written on their own — they go out with the \
             channels when you program a codeplug.",
            target.driver.display_name()
        )
    })?;

    let saved: Option<String> =
        sqlx::query_scalar("SELECT non_channel_settings FROM radio_profiles WHERE id = ?1")
            .bind(profile_id)
            .fetch_optional(&state.pool)
            .await
            .estr()?
            .flatten();
    let saved = saved.ok_or(
        "This profile has no saved settings to apply. Open it under Radios, run \
         \"Download from radio\", and save first.",
    )?;
    let settings: serde_json::Value = serde_json::from_str(&saved)
        .map_err(|e| format!("saved profile settings are not valid JSON: {e}"))?;

    let backup_dir = app.path().app_data_dir().estr()?.join("radio-backups");
    std::fs::create_dir_all(&backup_dir).estr()?;

    tauri::async_runtime::spawn_blocking(move || {
        writer.write_settings(&port, &settings, &target.schema, &backup_dir)
    })
    .await
    .estr()?
}

/// Push the DMR **call-sign database** (caller-ID / "UserDB") to a radio that
/// has one: pull a capacity-capped, country/continent-prioritized batch from
/// the local `dmr_users` library and hand it to the driver, which owns the
/// on-flash layout and the write discipline.
///
/// Radio-wide rather than codeplug-scoped, but profile-anchored like the other
/// settings commands so the driver resolves the same way. Verification is
/// FUNCTIONAL (the radio's own caller-ID screen) — this region has no golden
/// image to byte-diff. Brick-capable; UI-gated.
///
/// Registry-dispatched (Chunk 3.6d) — replaces `program_anytone_callsign_db`.
#[tauri::command]
pub async fn write_callsign_db(
    state: State<'_, AppState>,
    profile_id: i64,
    port: String,
    max_count: Option<i64>,
    priority_countries: Vec<String>,
) -> Result<CallsignDbReport, String> {
    let target = profile_target(&state.pool, profile_id).await?;
    let db_writer = target.driver.as_callsign_db_writer().ok_or_else(|| {
        format!(
            "{} has no on-board call-sign database",
            target.driver.display_name()
        )
    })?;

    let users = select_prioritized_dmr_users(&state.pool, max_count, &priority_countries).await?;
    let records: Vec<CallsignRecord> = users
        .into_iter()
        .map(|u| CallsignRecord {
            dmr_id: u.dmr_id as u32,
            name: u.display_name(),
            callsign: u.callsign,
            city: u.city,
            state: u.state,
            country: u.country,
            comment: u.remarks,
        })
        .collect();
    if records.is_empty() {
        return Err(
            "No DMR contacts to write — open DMR Contacts and Refresh the library first \
             (or widen the country selection)."
                .to_string(),
        );
    }

    tauri::async_runtime::spawn_blocking(move || db_writer.write_callsign_db(&port, &records))
        .await
        .estr()?
}

// ============================================================
// Write / upload
// ============================================================

#[derive(Serialize)]
pub struct ProgramResult {
    /// Channels written to slots 0..written.
    pub written: usize,
    /// Slots cleared (written..128) so the radio matches the codeplug exactly.
    pub cleared: usize,
    /// Number of non-channel settings written from the radio profile, or `None`
    /// if the profile had no settings and only channels/names were written.
    pub settings_written: Option<usize>,
    /// Whether a post-write read-back matched what we intended to write.
    pub verified: bool,
    /// Set when verification could not be completed or found differences.
    pub verify_note: Option<String>,
    /// Absolute path of the pre-write backup `.img`.
    pub backup_path: String,
    /// Channels actually present on the radio after writing (read back).
    pub channels: Vec<DecodedChannel>,
}

/// Program a codeplug directly into a connected UV-5R.
///
/// Safety model: download the full image and save it as a backup FIRST, then
/// patch the channel + name regions and (when the radio profile carries them)
/// the non-channel settings into that downloaded image, upload the affected
/// regions, and finally read the radio back to confirm the result. Because we
/// start from the radio's own image, only the bytes we explicitly encode change;
/// every untouched byte is written back exactly as it was read.
#[tauri::command]
pub async fn program_codeplug(
    app: AppHandle,
    state: State<'_, AppState>,
    codeplug_id: i64,
    port: String,
) -> Result<ProgramResult, String> {
    // Resolve the channels (DB work) before entering the blocking serial task.
    // Included channels are packed contiguously from slot 0; excluded (e.g.
    // digital-mode) channels are dropped and the rest close up behind them.
    let (model, slots) = export::resolve_codeplug_slots(&state.pool, codeplug_id).await?;
    if model.model != "UV-5R" {
        return Err(format!(
            "Direct programming supports the Baofeng UV-5R only (this codeplug targets {}).",
            model.display_name
        ));
    }
    if slots.len() > uv5r::CHANNEL_COUNT {
        return Err(format!(
            "Codeplug has {} programmable channels, but the UV-5R holds only {}.",
            slots.len(),
            uv5r::CHANNEL_COUNT
        ));
    }

    // The radio profile's non-channel settings (CHIRP-keyed JSON) + the model's
    // settings schema (for select-option indices). When both are present we
    // also write the settings memory so profile changes flow through to the
    // radio; otherwise we fall back to a channels+names-only write.
    let profile_settings: Option<String> = sqlx::query_scalar(
        "SELECT rp.non_channel_settings FROM codeplugs cp \
         JOIN radio_profiles rp ON rp.id = cp.radio_profile_id WHERE cp.id = ?1",
    )
    .bind(codeplug_id)
    .fetch_optional(&state.pool)
    .await
    .estr()?
    .flatten();
    let settings_schema = model.non_channel_settings_schema.clone();

    // Label the pre-write backup with the codeplug name so multiple
    // profiles/codeplugs for the same radio stay distinguishable when restoring.
    let codeplug_name: String = sqlx::query_scalar("SELECT name FROM codeplugs WHERE id = ?1")
        .bind(codeplug_id)
        .fetch_one(&state.pool)
        .await
        .estr()?;

    let backup_dir = app.path().app_data_dir().estr()?.join("radio-backups");
    std::fs::create_dir_all(&backup_dir).estr()?;
    let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
    let backup_path = backup_dir.join(uv5r::backup_filename("prewrite", &codeplug_name, &stamp));

    let result = tauri::async_runtime::spawn_blocking(move || -> Result<ProgramResult, String> {
        let mut p = uv5r::open_port(&port)?;

        // 1. Download + back up the current radio contents.
        let (magic, ident) = uv5r::ident_radio(&mut *p)?;
        if !magic.starts_with("UV5R") {
            return Err("the connected radio did not identify as a UV-5R".into());
        }
        let mut image = uv5r::download(&mut *p, &ident)?;
        std::fs::write(&backup_path, &image)
            .map_err(|e| format!("could not write backup {}: {e}", backup_path.display()))?;

        // 2. Patch the channel + name regions in the downloaded image.
        let written = slots.len();
        uv5r::patch_image(&mut image, &slots);

        // 2b. Patch the radio profile's non-channel settings into the image, if
        //     the profile carries them. This makes the profile authoritative:
        //     every editable setting is written from the profile.
        let settings_written = match (&settings_schema, &profile_settings) {
            (Some(schema), Some(settings)) => Some(uv5r_settings::apply_profile_settings(
                &mut image, schema, settings,
            )?),
            _ => None,
        };
        let write_settings = settings_written.is_some();

        // 3. Upload the patched regions (re-identify to start a clone session).
        //    The radio may take a moment to be ready to talk again after the
        //    full read, so settle first, then retry the identify. When settings
        //    are written we upload CHIRP's full main ranges + the aux ranges
        //    (poweron message, band limits); otherwise just channels + names.
        std::thread::sleep(std::time::Duration::from_secs(1));
        uv5r::reident_with_retry(&mut *p)?;
        if write_settings {
            for &(start, end) in uv5r_settings::SETTINGS_MAIN_RANGES {
                uv5r::write_region(&mut *p, &image, start, end)?;
            }
            for &(start, end) in uv5r_settings::SETTINGS_AUX_RANGES {
                uv5r::write_aux_region(&mut *p, &image, start, end)?;
            }
        } else {
            uv5r::write_region(&mut *p, &image, uv5r::CHANNEL_ADDR.0, uv5r::CHANNEL_ADDR.1)?;
            uv5r::write_region(&mut *p, &image, uv5r::NAME_ADDR.0, uv5r::NAME_ADDR.1)?;
        }

        // 4. Read back and verify (non-fatal: a write that ack'd every block
        //    succeeded; verification is a best-effort confirmation).
        let (verified, verify_note, channels) = match uv5r::verify_after_write(&mut *p, &image) {
            Ok((ok, note, ch)) => (ok, note, ch),
            Err(e) => (
                false,
                Some(format!(
                    "Write completed, but read-back verification could not run ({e}). \
                     Power-cycle the radio and use Download to confirm."
                )),
                uv5r::decode_channels(&image),
            ),
        };

        Ok(ProgramResult {
            written,
            cleared: uv5r::CHANNEL_COUNT - written,
            settings_written,
            verified,
            verify_note,
            backup_path: backup_path.to_string_lossy().to_string(),
            channels: channels.into_iter().take(20).collect(),
        })
    })
    .await
    .estr()??;

    // The write ack'd every block, so record this as the codeplug's last
    // program time (the Codeplugs screen shows "Programmed <date>").
    // Verification is best-effort and doesn't gate the stamp.
    sqlx::query(
        "UPDATE codeplugs SET last_exported = CURRENT_TIMESTAMP, last_export_kind = 'radio' WHERE id = ?1",
    )
    .bind(codeplug_id)
    .execute(&state.pool)
    .await
    .estr()?;

    Ok(result)
}

#[derive(Serialize)]
pub struct RestoreResult {
    pub bytes: usize,
    pub source: String,
}

/// Absolute path of the radio-backups directory, so the UI can default the
/// "Restore backup…" file picker there (it lives under the app data dir, which
/// is otherwise awkward to find in Finder).
#[tauri::command]
pub async fn backups_dir(app: AppHandle) -> Result<String, String> {
    let dir = app.path().app_data_dir().estr()?.join("radio-backups");
    Ok(dir.to_string_lossy().to_string())
}

/// Restore a previously-saved backup `.img` to the radio. Writes the full main
/// block (channels, names, DTMF, and all settings) plus the aux ranges we ever
/// touch, straight from the file — the recovery path for a bad write. The file
/// must be a CHIRP-compatible UV-5R image (it carries the 8-byte ident prefix,
/// so radio address 0x0000 is at image offset 0x0008).
#[tauri::command]
pub async fn restore_image(port: String, path: String) -> Result<RestoreResult, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let image = std::fs::read(&path).map_err(|e| format!("could not read {path}: {e}"))?;
        // Need at least the main block + names (image 0x1808) to be a real image.
        if image.len() < uv5r::MIN_IMAGE_LEN {
            return Err(format!(
                "{path} is only {} bytes — not a UV-5R backup image.",
                image.len()
            ));
        }

        let mut p = uv5r::open_port(&port)?;
        let (magic, _ident) = uv5r::ident_radio(&mut *p)?;
        if !magic.starts_with("UV5R") {
            return Err("the connected radio did not identify as a UV-5R".into());
        }

        uv5r::upload_full_image(&mut *p, &image)?;

        Ok(RestoreResult {
            bytes: image.len(),
            source: path,
        })
    })
    .await
    .estr()?
}
