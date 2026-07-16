//! Tauri command layer for direct TIDRADIO TD-H3 programming.
//!
//! The clone protocol, channel encode/decode, and non-channel settings live in
//! `radios/tidradio_tdh3` since Chunk 3.4 (`ImageProgrammer` in `mod.rs`,
//! `SettingsProgrammer` in `settings.rs`); this module is the thin Tauri layer —
//! DB access, backup-file paths, and the result DTOs. Command signatures are
//! unchanged from before the extraction, so `lib.rs` and the frontend are
//! untouched (that rewiring is Chunk 3.6/3.7). Port enumeration reuses
//! `program::list_serial_ports` (it is model-agnostic).

use std::time::Duration;

use serde::Serialize;
use tauri::{AppHandle, Manager, State};

use crate::commands::export;
use crate::commands::program::RadioSettingsRead;
use crate::db::AppState;
use crate::error::MapErrString;
use crate::models::RadioProfile;
use crate::radios::tidradio_tdh3 as tdh3;
use crate::radios::tidradio_tdh3::settings::{self as tdh3_settings, Tdh3Settings};
use tdh3::Tdh3DecodedChannel;

// ============================================================
// Identify
// ============================================================

#[derive(Serialize)]
pub struct Tdh3Ident {
    /// The raw ident bytes the radio returned, as hex.
    pub ident_hex: String,
    /// The same ident rendered as ASCII (printable bytes), e.g. "P31183" — the
    /// HAM/GMRS variants report different trailing digits.
    pub ident_ascii: String,
}

/// Harmless handshake: confirm a TD-H3 is connected and in clone mode. Reads no
/// memory, so it cannot affect the radio's contents.
#[tauri::command]
pub async fn identify_tdh3(port: String) -> Result<Tdh3Ident, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let mut p = tdh3::open_port(&port)?;
        let ident = tdh3::do_ident(&mut *p)?;
        Ok(Tdh3Ident {
            ident_hex: tdh3::hex(&ident),
            ident_ascii: tdh3::ascii(&ident),
        })
    })
    .await
    .estr()?
}

// ============================================================
// Download (read-only backup + sanity sample)
// ============================================================

#[derive(Serialize)]
pub struct Tdh3DownloadResult {
    pub ident_hex: String,
    pub ident_ascii: String,
    pub image_bytes: usize,
    /// Absolute path of the saved CHIRP-compatible backup `.img`.
    pub backup_path: String,
    /// Number of programmed (non-empty) channels found in the image.
    pub channel_count: usize,
    /// A sanity sample of the first programmed channels so the user can eyeball
    /// that the read is real before we ever build the write path.
    pub channels: Vec<Tdh3DecodedChannel>,
}

/// Read the full radio image and save it as a timestamped backup. This is the
/// non-destructive proof that the protocol port is correct, and it produces the
/// safety backup the write path always takes first.
#[tauri::command]
pub async fn download_tdh3_image(
    app: AppHandle,
    port: String,
) -> Result<Tdh3DownloadResult, String> {
    let backup_dir = app.path().app_data_dir().estr()?.join("radio-backups");
    std::fs::create_dir_all(&backup_dir).estr()?;
    let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
    let backup_path = backup_dir.join(format!("tdh3-{stamp}.img"));

    tauri::async_runtime::spawn_blocking(move || {
        let mut p = tdh3::open_port(&port)?;
        let ident = tdh3::do_ident(&mut *p)?;
        let image = tdh3::download(&mut *p, &ident)?;

        std::fs::write(&backup_path, &image)
            .map_err(|e| format!("could not write backup {}: {e}", backup_path.display()))?;

        let channels = tdh3::decode_channels(&image);
        Ok(Tdh3DownloadResult {
            ident_hex: tdh3::hex(&ident),
            ident_ascii: tdh3::ascii(&ident),
            image_bytes: image.len(),
            backup_path: backup_path.to_string_lossy().to_string(),
            channel_count: channels.len(),
            channels: channels.into_iter().take(20).collect(),
        })
    })
    .await
    .estr()?
}

// ============================================================
// Program channels
// ============================================================

#[derive(Serialize)]
pub struct Tdh3ProgramResult {
    /// Channels written (to channel numbers 1..=written).
    pub written: usize,
    /// Channel slots cleared (so the radio matches the codeplug exactly).
    pub cleared: usize,
    /// Whether the post-write read-back matched what we intended to write.
    pub verified: bool,
    /// Set when verification could not run or found differences.
    pub verify_note: Option<String>,
    /// Absolute path of the pre-write backup `.img`.
    pub backup_path: String,
    /// Channels present on the radio after writing (read back), sample.
    pub channels: Vec<Tdh3DecodedChannel>,
}

/// Program a codeplug's channels directly into a connected TD-H3.
///
/// Safety model (mirrors the UV-5R path): download the full image and save it as
/// a backup FIRST, patch only the channel/name regions and the used/scan bitmaps
/// into that downloaded image, upload the whole main range (so every untouched
/// byte — including all radio settings — is written back exactly as read), then
/// read the radio back to confirm. This writes channels only; the radio's
/// non-channel settings are preserved as-is (see the settings commands).
#[tauri::command]
pub async fn program_tdh3_codeplug(
    app: AppHandle,
    state: State<'_, AppState>,
    codeplug_id: i64,
    port: String,
) -> Result<Tdh3ProgramResult, String> {
    let (model, slots) = export::resolve_codeplug_slots(&state.pool, codeplug_id).await?;
    if model.model != "TD-H3" {
        return Err(format!(
            "This programmer is for the TIDRADIO TD-H3 (this codeplug targets {}).",
            model.display_name
        ));
    }
    if slots.len() > tdh3::MAX_CHANNEL {
        return Err(format!(
            "Codeplug has {} programmable channels, but the TD-H3 holds only {}.",
            slots.len(),
            tdh3::MAX_CHANNEL
        ));
    }

    let codeplug_name: String = sqlx::query_scalar("SELECT name FROM codeplugs WHERE id = ?1")
        .bind(codeplug_id)
        .fetch_one(&state.pool)
        .await
        .estr()?;

    let backup_dir = app.path().app_data_dir().estr()?.join("radio-backups");
    std::fs::create_dir_all(&backup_dir).estr()?;
    let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
    // Slug the codeplug name into the backup filename so multiple codeplugs for
    // the radio stay distinguishable when restoring.
    let slug = tdh3::slug_label(&codeplug_name);
    let backup_path = backup_dir.join(if slug.is_empty() {
        format!("tdh3-prewrite-{stamp}.img")
    } else {
        format!("tdh3-prewrite-{slug}-{stamp}.img")
    });

    let result = tauri::async_runtime::spawn_blocking(move || -> Result<Tdh3ProgramResult, String> {
        let mut p = tdh3::open_port(&port)?;

        // 1. Download + back up the current radio contents.
        let ident = tdh3::do_ident(&mut *p)?;
        let mut image = tdh3::download(&mut *p, &ident)?;
        std::fs::write(&backup_path, &image)
            .map_err(|e| format!("could not write backup {}: {e}", backup_path.display()))?;

        // 2. Patch channels/names + used/scan bitmaps into the downloaded image.
        let written = slots.len();
        tdh3::patch_image(&mut image, &slots);

        // 3. Re-identify (the radio settles after a full read) and upload the
        //    whole main range, then exit programming mode.
        std::thread::sleep(Duration::from_secs(1));
        tdh3::reident(&mut *p)?;
        tdh3::upload(&mut *p, &image)?;

        // 4. Read back and verify (non-fatal — every block was ack'd).
        let (verified, verify_note, channels) = match tdh3::verify_after_write(&mut *p, &image) {
            Ok((ok, note, ch)) => (ok, note, ch),
            Err(e) => (
                false,
                Some(format!(
                    "Write completed, but read-back verification could not run ({e}). \
                     Power-cycle the radio and use Download to confirm."
                )),
                tdh3::decode_channels(&image),
            ),
        };

        Ok(Tdh3ProgramResult {
            written,
            cleared: tdh3::MAX_CHANNEL - written,
            verified,
            verify_note,
            backup_path: backup_path.to_string_lossy().to_string(),
            channels: channels.into_iter().take(20).collect(),
        })
    })
    .await
    .estr()??;

    // The write ack'd every block, so stamp the codeplug's last program time
    // (the Codeplugs screen shows "Programmed <date>"). Verification is
    // best-effort and doesn't gate the stamp.
    sqlx::query(
        "UPDATE codeplugs SET last_exported = CURRENT_TIMESTAMP, last_export_kind = 'radio' WHERE id = ?1",
    )
    .bind(codeplug_id)
    .execute(&state.pool)
    .await
    .estr()?;

    Ok(result)
}

// ============================================================
// Settings: read radio → profile editor form
// ============================================================

/// Read the radio's current settings into a profile's editor form (ident →
/// download → decode into the schema-keyed JSON the profile stores). The TD-H3
/// mirror of the UV-5R `read_radio_settings`: non-destructive (reads memory
/// only), backs up the downloaded image first, and decodes only the keys we can
/// locate so the editor can merge them over the profile's current values.
#[tauri::command]
pub async fn read_tdh3_settings_for_profile(
    app: AppHandle,
    state: State<'_, AppState>,
    profile_id: i64,
    port: String,
) -> Result<RadioSettingsRead, String> {
    // The profile's model + settings schema decide what we can decode.
    let row: Option<(String, Option<String>, String)> = sqlx::query_as(
        "SELECT rm.model, rm.non_channel_settings_schema, rp.display_name \
         FROM radio_profiles rp JOIN radio_models rm ON rm.id = rp.radio_model_id \
         WHERE rp.id = ?1",
    )
    .bind(profile_id)
    .fetch_optional(&state.pool)
    .await
    .estr()?;
    let (model, schema, profile_name) = row.ok_or("radio profile not found")?;
    if model != "TD-H3" {
        return Err(format!(
            "This radio profile is for {model}, not the TD-H3."
        ));
    }
    let schema = schema.ok_or("the TD-H3 model has no settings schema to decode into")?;

    let backup_dir = app.path().app_data_dir().estr()?.join("radio-backups");
    std::fs::create_dir_all(&backup_dir).estr()?;
    let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
    let slug = tdh3::slug_label(&profile_name);
    let backup_path = backup_dir.join(if slug.is_empty() {
        format!("tdh3-settings-{stamp}.img")
    } else {
        format!("tdh3-settings-{slug}-{stamp}.img")
    });

    tauri::async_runtime::spawn_blocking(move || {
        let mut p = tdh3::open_port(&port)?;
        let ident = tdh3::do_ident(&mut *p)?;
        let image = tdh3::download(&mut *p, &ident)?;
        std::fs::write(&backup_path, &image)
            .map_err(|e| format!("could not write backup {}: {e}", backup_path.display()))?;

        let settings = tdh3_settings::decode_profile_settings(&image, &schema)?;
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
// Settings: write edited settings → radio
// ============================================================

#[derive(Serialize)]
pub struct Tdh3SettingsWriteResult {
    /// Whether the post-write read-back matched what we intended to write.
    pub verified: bool,
    /// Set when verification could not run or found differences.
    pub verify_note: Option<String>,
    /// Absolute path of the pre-write backup `.img`.
    pub backup_path: String,
    /// The settings present on the radio after writing (read back).
    pub settings: Tdh3Settings,
}

/// Write edited settings back to a connected TD-H3.
///
/// Same safety model as the channel write: download the full image and back it
/// up FIRST, patch only the settings bits into that image (every other byte —
/// channels included — is preserved exactly as read), upload the whole main
/// range, then read the radio back and confirm the settings took.
#[tauri::command]
pub async fn write_tdh3_settings(
    app: AppHandle,
    port: String,
    settings: Tdh3Settings,
) -> Result<Tdh3SettingsWriteResult, String> {
    let backup_dir = app.path().app_data_dir().estr()?.join("radio-backups");
    std::fs::create_dir_all(&backup_dir).estr()?;
    let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
    let backup_path = backup_dir.join(format!("tdh3-presettings-{stamp}.img"));

    tauri::async_runtime::spawn_blocking(move || {
        let mut p = tdh3::open_port(&port)?;

        // 1. Download + back up the current radio contents.
        let ident = tdh3::do_ident(&mut *p)?;
        let mut image = tdh3::download(&mut *p, &ident)?;
        std::fs::write(&backup_path, &image)
            .map_err(|e| format!("could not write backup {}: {e}", backup_path.display()))?;

        // 2. Patch the settings bits into the downloaded image.
        tdh3_settings::encode_settings(&mut image, &settings);

        // 3. Re-identify and upload the whole main range, then leave clone mode.
        std::thread::sleep(Duration::from_secs(1));
        tdh3::reident(&mut *p)?;
        tdh3::upload(&mut *p, &image)?;

        // 4. Read back and verify against what we intended (non-fatal).
        let (verified, verify_note, result) =
            match tdh3_settings::verify_settings_after_write(&mut *p, &image) {
                Ok((ok, note, st)) => (ok, note, st),
                Err(e) => (
                    false,
                    Some(format!(
                        "Write completed, but read-back verification could not run ({e}). \
                         Power-cycle the radio and use Read to confirm."
                    )),
                    tdh3_settings::decode_settings(&image),
                ),
            };

        Ok(Tdh3SettingsWriteResult {
            verified,
            verify_note,
            backup_path: backup_path.to_string_lossy().to_string(),
            settings: result,
        })
    })
    .await
    .estr()?
}

// ============================================================
// Settings: apply a saved radio profile → radio
// ============================================================

#[derive(Serialize)]
pub struct Tdh3ProfileApplyResult {
    /// Number of profile fields actually written to the radio.
    pub applied: usize,
    /// Whether the post-write read-back matched what we intended to write.
    pub verified: bool,
    /// Set when verification could not run or found differences.
    pub verify_note: Option<String>,
    /// Absolute path of the pre-write backup `.img`.
    pub backup_path: String,
    /// The settings present on the radio after applying (read back) — lets the
    /// Radio Options form refresh to show what the profile set.
    pub settings: Tdh3Settings,
}

/// Apply a saved radio profile's non-channel settings to a connected TD-H3,
/// leaving channels untouched. Same safety model as the other writes: download +
/// back up the full image first, patch only the profile's settings bits, upload
/// the whole main range (channels and every unsupported setting preserved as
/// read), then read back and verify.
#[tauri::command]
pub async fn apply_tdh3_profile(
    app: AppHandle,
    state: State<'_, AppState>,
    port: String,
    profile_id: i64,
) -> Result<Tdh3ProfileApplyResult, String> {
    // Pull the profile's model, schema, and saved settings up front (before the
    // blocking serial work, which can't hold the DB connection).
    let row: Option<(String, Option<String>, Option<String>, String)> = sqlx::query_as(
        "SELECT rm.model, rm.non_channel_settings_schema, rp.non_channel_settings, rp.display_name \
         FROM radio_profiles rp JOIN radio_models rm ON rm.id = rp.radio_model_id WHERE rp.id = ?1",
    )
    .bind(profile_id)
    .fetch_optional(&state.pool)
    .await
    .estr()?;
    let (model, schema, settings, profile_name) = row.ok_or("radio profile not found")?;
    if model != "TD-H3" {
        return Err(format!(
            "This radio profile is for {model}, not the TD-H3."
        ));
    }
    let schema = schema.ok_or("the TD-H3 model has no settings schema")?;
    let settings = settings
        .ok_or("This profile has no saved settings to apply. Edit it under Radios first.")?;

    let backup_dir = app.path().app_data_dir().estr()?.join("radio-backups");
    std::fs::create_dir_all(&backup_dir).estr()?;
    let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
    let slug = tdh3::slug_label(&profile_name);
    let backup_path = backup_dir.join(if slug.is_empty() {
        format!("tdh3-preprofile-{stamp}.img")
    } else {
        format!("tdh3-preprofile-{slug}-{stamp}.img")
    });

    tauri::async_runtime::spawn_blocking(move || {
        let mut p = tdh3::open_port(&port)?;

        // 1. Download + back up the current radio contents.
        let ident = tdh3::do_ident(&mut *p)?;
        let mut image = tdh3::download(&mut *p, &ident)?;
        std::fs::write(&backup_path, &image)
            .map_err(|e| format!("could not write backup {}: {e}", backup_path.display()))?;

        // 2. Patch the profile's settings bits into the downloaded image.
        let applied = tdh3_settings::apply_profile_settings(&mut image, &schema, &settings)?;

        // 3. Re-identify and upload the whole main range, then leave clone mode.
        std::thread::sleep(Duration::from_secs(1));
        tdh3::reident(&mut *p)?;
        tdh3::upload(&mut *p, &image)?;

        // 4. Read back and verify (non-fatal).
        let (verified, verify_note, result) =
            match tdh3_settings::verify_settings_after_write(&mut *p, &image) {
                Ok((ok, note, st)) => (ok, note, st),
                Err(e) => (
                    false,
                    Some(format!(
                        "Profile written, but read-back verification could not run ({e}). \
                         Power-cycle the radio and use Read to confirm."
                    )),
                    tdh3_settings::decode_settings(&image),
                ),
            };

        Ok(Tdh3ProfileApplyResult {
            applied,
            verified,
            verify_note,
            backup_path: backup_path.to_string_lossy().to_string(),
            settings: result,
        })
    })
    .await
    .estr()?
}

// ============================================================
// Settings: import radio settings INTO a saved profile
// ============================================================
//
// The inverse of `apply_profile_settings`: read the radio's current settings and
// store them in a radio profile (update an existing one, merging so the deferred
// DTMF/key keys it already holds are kept, or create a new profile). The
// schema-keyed JSON shape matches what the profile's Settings tab edits.

/// Save the (possibly edited) settings currently in the Radio Options form into a
/// radio profile — either updating an existing profile (merging over its stored
/// settings so keys we don't manage are preserved) or creating a new one. Pure
/// DB + CPU work; the radio was already read to populate the form.
#[tauri::command]
pub async fn save_tdh3_settings_to_profile(
    state: State<'_, AppState>,
    settings: Tdh3Settings,
    model_id: i64,
    profile_id: Option<i64>,
    new_name: Option<String>,
) -> Result<RadioProfile, String> {
    let schema: Option<String> =
        sqlx::query_scalar("SELECT non_channel_settings_schema FROM radio_models WHERE id = ?1")
            .bind(model_id)
            .fetch_optional(&state.pool)
            .await
            .estr()?
            .flatten();
    let schema = schema.ok_or("this radio model has no settings schema")?;

    // Render the form's settings to the schema-keyed JSON shape via a scratch
    // image (encode → decode), reusing the single source-of-truth bit map.
    let mut scratch = vec![0u8; tdh3::MEMSIZE as usize + 0x20];
    tdh3_settings::encode_settings(&mut scratch, &settings);
    let decoded = tdh3_settings::decode_profile_settings(&scratch, &schema)?;
    let decoded_obj = decoded.as_object().cloned().unwrap_or_default();

    let id = if let Some(pid) = profile_id {
        // Merge the read keys over the profile's existing settings.
        let existing: Option<String> =
            sqlx::query_scalar("SELECT non_channel_settings FROM radio_profiles WHERE id = ?1")
                .bind(pid)
                .fetch_optional(&state.pool)
                .await
                .estr()?
                .flatten();
        let mut base = existing
            .as_deref()
            .and_then(|s| serde_json::from_str::<serde_json::Value>(s).ok())
            .and_then(|v| v.as_object().cloned())
            .unwrap_or_default();
        for (k, v) in decoded_obj {
            base.insert(k, v);
        }
        let merged = serde_json::to_string(&serde_json::Value::Object(base)).estr()?;
        sqlx::query(
            "UPDATE radio_profiles SET non_channel_settings = ?2, updated_at = CURRENT_TIMESTAMP WHERE id = ?1",
        )
        .bind(pid)
        .bind(&merged)
        .execute(&state.pool)
        .await
        .estr()?;
        pid
    } else {
        let name = new_name
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .ok_or("Enter a name for the new profile.")?;
        let json = serde_json::to_string(&serde_json::Value::Object(decoded_obj)).estr()?;
        sqlx::query(
            "INSERT INTO radio_profiles (display_name, radio_model_id, non_channel_settings, notes, updated_at) \
             VALUES (?1, ?2, ?3, NULL, CURRENT_TIMESTAMP)",
        )
        .bind(&name)
        .bind(model_id)
        .bind(&json)
        .execute(&state.pool)
        .await
        .estr()?
        .last_insert_rowid()
    };

    sqlx::query_as::<_, RadioProfile>(
        "SELECT id, display_name, radio_model_id, non_channel_settings, notes, created_at, updated_at FROM radio_profiles WHERE id = ?1",
    )
    .bind(id)
    .fetch_one(&state.pool)
    .await
    .estr()
}
