//! UV-5R programming commands + model-agnostic serial-port enumeration.
//!
//! The UV-5R clone protocol itself (handshake, block reads/writes, channel
//! encode/decode) lives in `radios/baofeng_uv5r` since Chunk 3.3; this module
//! is the Tauri command layer on top of it — DB lookups, backup-file naming,
//! and the result DTOs the frontend renders. `list_serial_ports` is
//! model-agnostic and shared by every radio's dialog.

use serde::Serialize;
use tauri::{AppHandle, Manager, State};

use crate::commands::export;
use crate::db::AppState;
use crate::error::MapErrString;
use crate::radios::baofeng_uv5r as uv5r;
use crate::radios::baofeng_uv5r::settings as uv5r_settings;
use crate::radios::baofeng_uv5r::DecodedChannel;

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

#[derive(Serialize)]
pub struct RadioIdent {
    /// Which magic matched, e.g. "UV5R_ORIG".
    pub matched_magic: String,
    /// The 8-byte ident the radio returned, as hex.
    pub ident_hex: String,
}

/// Harmless handshake: confirm a UV-5R is connected and in clone mode. Performs
/// no reads of memory, so it cannot affect the radio's contents.
#[tauri::command]
pub async fn identify_radio(port: String) -> Result<RadioIdent, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let mut p = uv5r::open_port(&port)?;
        let (magic, ident) = uv5r::ident_radio(&mut *p)?;
        Ok(RadioIdent {
            matched_magic: magic,
            ident_hex: uv5r::hex(&ident),
        })
    })
    .await
    .estr()?
}

#[derive(Serialize)]
pub struct DownloadResult {
    pub matched_magic: String,
    pub ident_hex: String,
    pub image_bytes: usize,
    /// Absolute path of the saved CHIRP-compatible backup `.img`.
    pub backup_path: String,
    /// Number of programmed (non-empty) channels found in the image.
    pub channel_count: usize,
    /// A sanity sample of decoded channels (first programmed ones) so the user
    /// can eyeball that the read is real before we ever build the write path.
    pub channels: Vec<DecodedChannel>,
}

/// Read the full radio image and save it as a timestamped backup. This is the
/// non-destructive proof that our protocol port is correct, and it produces the
/// safety backup that the write path always takes first.
#[tauri::command]
pub async fn download_image(app: AppHandle, port: String) -> Result<DownloadResult, String> {
    // Resolve the backup directory on the main thread (AppHandle path API).
    let backup_dir = app
        .path()
        .app_data_dir()
        .estr()?
        .join("radio-backups");
    std::fs::create_dir_all(&backup_dir).estr()?;
    let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
    let backup_path = backup_dir.join(format!("uv5r-{stamp}.img"));

    tauri::async_runtime::spawn_blocking(move || {
        let mut p = uv5r::open_port(&port)?;
        let (magic, ident) = uv5r::ident_radio(&mut *p)?;
        let image = uv5r::download(&mut *p, &ident)?;

        std::fs::write(&backup_path, &image)
            .map_err(|e| format!("could not write backup {}: {e}", backup_path.display()))?;

        let channels = uv5r::decode_channels(&image);
        Ok(DownloadResult {
            matched_magic: magic,
            ident_hex: uv5r::hex(&ident),
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

/// Read the radio's current non-channel settings into a profile's shape. Lets
/// the user pull configuration changes they made on the radio back into the
/// profile (the inverse of writing settings during programming). The full
/// image is also saved as a backup, exactly like a normal download.
#[tauri::command]
pub async fn read_radio_settings(
    app: AppHandle,
    state: State<'_, AppState>,
    profile_id: i64,
    port: String,
) -> Result<RadioSettingsRead, String> {
    // The profile's model + settings schema decide what we can decode.
    let row: Option<(String, Option<String>, String)> = sqlx::query_as(
        "SELECT rm.model, rm.non_channel_settings_schema, rp.name \
         FROM radio_profiles rp JOIN radio_models rm ON rm.id = rp.radio_model_id \
         WHERE rp.id = ?1",
    )
    .bind(profile_id)
    .fetch_optional(&state.pool)
    .await
    .estr()?;
    let (model, schema, profile_name) = row.ok_or("radio profile not found")?;
    if model != "UV-5R" {
        return Err(format!(
            "Reading settings from the radio supports the Baofeng UV-5R only (this profile is a {model})."
        ));
    }
    let schema = schema.ok_or("this radio model has no settings schema to decode into")?;

    let backup_dir = app.path().app_data_dir().estr()?.join("radio-backups");
    std::fs::create_dir_all(&backup_dir).estr()?;
    let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
    let backup_path = backup_dir.join(uv5r::backup_filename("settings", &profile_name, &stamp));

    tauri::async_runtime::spawn_blocking(move || {
        let mut p = uv5r::open_port(&port)?;
        let (magic, ident) = uv5r::ident_radio(&mut *p)?;
        if !magic.starts_with("UV5R") {
            return Err("the connected radio did not identify as a UV-5R".into());
        }
        let image = uv5r::download(&mut *p, &ident)?;
        std::fs::write(&backup_path, &image)
            .map_err(|e| format!("could not write backup {}: {e}", backup_path.display()))?;

        let settings = uv5r_settings::decode_settings_from_image(&image, &schema)?;
        let count = settings.as_object().map(|o| o.len()).unwrap_or(0);
        Ok(RadioSettingsRead {
            settings,
            count,
            backup_path: backup_path.to_string_lossy().to_string(),
        })
    })
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
