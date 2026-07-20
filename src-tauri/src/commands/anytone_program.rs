//! "Program radio" for the AnyTone AT-D890UV: push a codeplug from the app
//! database to the radio as a FULL REPLACE of its channel set — channels,
//! zones (from the codeplug's channel lists), scan lists, and DMR contacts
//! (talkgroups) — in ONE PC-mode session with a single END/commit/reboot.
//!
//! Two halves:
//!   1. `build_anytone_program` — pure DB → plan assembly (no hardware). The
//!      inverse of the radio-download import mapping: CHIRP tone scheme back
//!      to TX/RX sub-tones, power levels back to the mode-byte bits, DMR
//!      channels expanded per (talkgroup × timeslot) with contact indices
//!      assigned in first-use order. Drives the preview.
//!   2. `program_anytone_codeplug` — the hardware session. Reads every region
//!      it will touch (whole-bank/0x4000-window RMW, the HW-proven safe write
//!      unit), computes the patched windows (slots/zones/scan lists/contacts
//!      beyond the plan are cleared to each region's empty pattern), writes a
//!      mandatory backup of the ORIGINAL bytes plus an `.expected.bin` image
//!      of the patched bytes BEFORE any write, then commits and ENDs once.
//!      `verify_anytone_program` byte-checks the expected image in a fresh
//!      session after the reboot; `restore_anytone_backup` writes the backup
//!      back if anything looks wrong.

use serde::Serialize;
use tauri::{AppHandle, Manager, State};

use crate::db::AppState;
use crate::error::MapErrString;
use crate::models::RadioModel;

use super::anytone::{
    diff_dumps, end_session, enter_program_and_ident, open_port, parse_dump, read_block,
    run_patch_writes_direct, write_block, AnytoneDumpDiff, AnytonePatchWriteResult, WRITE_WINDOW,
};
use super::anytone_callsign_db::{encode_callsign_db, CallsignDbEntry};
// The plan/session half moved into the driver in 3.6b; the DTOs it returns are
// the generic driver-level ones, so the wire shape (and the frontend) is
// unchanged.
use crate::radios::driver::{CodeplugPreview, ProgramReport};
use crate::radios::registry::driver_for_model;
// run_settings_program + its DTO moved into the driver's settings module in 3.5;
// re-exported here so `commands::anytone_program::run_settings_program` (the
// settings-write RE example) keeps resolving until 3.6.
pub use super::anytone_settings::{
    encode_anytone_settings, run_settings_program, AnytoneSettingsProgramResult, SETTINGS_WINDOWS,
};
use super::dmr_users::select_prioritized_dmr_users;
use super::export::resolve_codeplug_payload;

/// Result of a fresh-session byte-verify against an `.expected.bin` image.
#[derive(Serialize)]
pub struct AnytoneVerifyResult {
    pub ok: bool,
    pub bytes_checked: usize,
    pub mismatches: Vec<AnytoneDumpDiff>,
}

// ============================================================
// Settings (radio profile) write path
// ============================================================

/// Push the radio profile's non-channel settings (the "radio profile") to the
/// AT-D890UV. Model-gated to the AT-D890UV; loads the codeplug's profile
/// settings JSON (the same shape "Download from radio" produces + saves), then
/// runs the single-session backup→write→commit path. Brick-capable — UI-gated
/// with an explicit ack, like `program_anytone_codeplug`.
#[tauri::command]
pub async fn program_anytone_settings(
    app: AppHandle,
    state: State<'_, AppState>,
    codeplug_id: i64,
    port: String,
) -> Result<AnytoneSettingsProgramResult, String> {
    let row: Option<(String, Option<String>)> = sqlx::query_as(
        "SELECT rm.model, rp.non_channel_settings \
         FROM codeplugs cp \
         JOIN radio_profiles rp ON rp.id = cp.radio_profile_id \
         JOIN radio_models rm ON rm.id = rp.radio_model_id \
         WHERE cp.id = ?1",
    )
    .bind(codeplug_id)
    .fetch_optional(&state.pool)
    .await
    .estr()?;
    let (model, settings_json) = row.ok_or("codeplug or its radio profile not found")?;
    if model != "AT-D890UV" {
        return Err(format!(
            "Writing settings over the AnyTone cable supports the AT-D890UV only (this profile is a {model})."
        ));
    }
    let settings_json = settings_json.ok_or(
        "this profile has no saved settings — open the profile editor and run \
         \"Download from radio\" first, then save.",
    )?;
    let values: serde_json::Value = serde_json::from_str(&settings_json)
        .map_err(|e| format!("saved profile settings are not valid JSON: {e}"))?;

    let backup_dir = app.path().app_data_dir().estr()?.join("radio-backups");
    std::fs::create_dir_all(&backup_dir).estr()?;
    let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
    let backup_path = backup_dir.join(format!("anytone-settings-write-{stamp}.bin"));
    let expected_path = backup_dir.join(format!("anytone-settings-write-{stamp}.expected.bin"));

    tauri::async_runtime::spawn_blocking(move || {
        run_settings_program(&port, &values, &backup_path, &expected_path)
    })
    .await
    .estr()?
}

/// Push the DMR **Call-sign Database** (caller-ID / "UserDB") to the AT-D890UV:
/// pull a capacity-capped, country/continent-prioritized batch from the local
/// `dmr_users` library (session 34's selection logic), encode it with
/// [`encode_callsign_db`], and write it in one PC-mode session via
/// [`run_patch_writes_direct`] — a DIRECT write-only path (no read-modify-write).
/// The RMW path drops interior flash banks on large writes (HW-proven), so this
/// fully-overwritten region is written without the corrupting read phase.
///
/// Model-gated to the AT-D890UV (the region layout is model-specific). This is a
/// radio-wide database, not codeplug-scoped, but `codeplug_id` gives us the
/// model to gate on and keeps the call consistent with the other Program paths.
/// Verification is FUNCTIONAL (the radio's own caller-ID screen) — this region
/// has no golden image to byte-diff, and no backup is taken (the read to make
/// one is the very thing that corrupts the commit). Brick-capable; UI-gated.
#[tauri::command]
pub async fn program_anytone_callsign_db(
    state: State<'_, AppState>,
    codeplug_id: i64,
    port: String,
    max_count: Option<i64>,
    priority_countries: Vec<String>,
) -> Result<AnytonePatchWriteResult, String> {
    let model: Option<(String,)> = sqlx::query_as(
        "SELECT rm.model \
         FROM codeplugs cp \
         JOIN radio_profiles rp ON rp.id = cp.radio_profile_id \
         JOIN radio_models rm ON rm.id = rp.radio_model_id \
         WHERE cp.id = ?1",
    )
    .bind(codeplug_id)
    .fetch_optional(&state.pool)
    .await
    .estr()?;
    let model = model.ok_or("codeplug or its radio profile not found")?.0;
    if model != "AT-D890UV" {
        return Err(format!(
            "Writing the call-sign database over the AnyTone cable supports the AT-D890UV only (this profile is a {model})."
        ));
    }

    let users = select_prioritized_dmr_users(&state.pool, max_count, &priority_countries).await?;
    if users.is_empty() {
        return Err(
            "No DMR contacts to write — open DMR Contacts and Refresh the library first \
             (or widen the country selection)."
                .to_string(),
        );
    }
    let entries: Vec<CallsignDbEntry> = users
        .into_iter()
        .filter(|u| u.dmr_id > 0 && u.dmr_id <= 99_999_999)
        .map(|u| CallsignDbEntry {
            dmr_id: u.dmr_id as u32,
            name: u.display_name(),
            city: u.city.clone().unwrap_or_default(),
            call: u.callsign.clone(),
            state: u.state.clone().unwrap_or_default(),
            country: u.country.clone().unwrap_or_default(),
            comment: u.remarks.clone().unwrap_or_default(),
        })
        .collect();
    if entries.is_empty() {
        return Err("No valid DMR IDs in the selected batch.".to_string());
    }

    let patches = encode_callsign_db(&entries)?;

    // Direct write-only (no read-modify-write): the caller-ID DB region is fully
    // overwritten, so nothing needs preserving — and crucially the RMW read
    // phase corrupts large flash commits (HW-proven: interior map/DB banks come
    // back empty via RMW, fully intact via the direct path). See
    // `run_patch_writes_direct`. No backup is taken (the read to make one is what
    // breaks the commit); the region is self-contained and regenerable.
    tauri::async_runtime::spawn_blocking(move || run_patch_writes_direct(&port, &patches))
        .await
        .estr()?
}

// ============================================================
// Tauri commands
// ============================================================

/// Resolve the codeplug's driver and confirm it can program from the database.
/// The model→driver hop is the registry's job; this command layer only owns the
/// DB read and the backup directory.
fn codeplug_programmer(
    model: &RadioModel,
) -> Result<&'static dyn crate::radios::driver::CodeplugProgrammer, String> {
    let driver = driver_for_model(model).ok_or_else(|| {
        format!(
            "no live-radio driver for {} — it is export-only",
            model.display_name
        )
    })?;
    driver.as_codeplug_programmer().ok_or_else(|| {
        format!(
            "{} cannot be programmed directly from the database",
            driver.display_name()
        )
    })
}

/// Preview what `program_anytone_codeplug` would write: counts, skipped
/// channels with reasons, and warnings. Pure DB — no radio needed.
#[tauri::command]
pub async fn program_anytone_preview(
    state: State<'_, AppState>,
    codeplug_id: i64,
) -> Result<CodeplugPreview, String> {
    let resolved = resolve_codeplug_payload(&state.pool, codeplug_id).await?;
    codeplug_programmer(&resolved.model)?.preview(&resolved.payload())
}

/// FULL-REPLACE program: assemble the codeplug and write it to the radio as
/// its entire channel/zone/scan-list/contact set, in one session with a single
/// END (one reboot). Mandatory backup + expected image are written before any
/// byte goes to the radio. Brick-capable — UI-gated with an explicit ack.
#[tauri::command]
pub async fn program_anytone_codeplug(
    app: AppHandle,
    state: State<'_, AppState>,
    codeplug_id: i64,
    port: String,
) -> Result<ProgramReport, String> {
    let resolved = resolve_codeplug_payload(&state.pool, codeplug_id).await?;
    let programmer = codeplug_programmer(&resolved.model)?;
    let backup_dir = app.path().app_data_dir().estr()?.join("radio-backups");

    // `resolved` owns every row the planner needs, so it moves into the
    // blocking task wholesale — the payload borrows from it in there.
    let result = tauri::async_runtime::spawn_blocking(move || {
        programmer.program(&port, &resolved.payload(), &backup_dir)
    })
    .await
    .estr()??;

    sqlx::query(
        "UPDATE codeplugs SET last_exported = CURRENT_TIMESTAMP, last_export_kind = 'radio' WHERE id = ?1",
    )
    .bind(codeplug_id)
    .execute(&state.pool)
    .await
    .estr()?;

    Ok(result)
}

/// Fresh-session byte-verify: strict-read every range in an `.expected.bin`
/// image (from `program_anytone_codeplug`) and diff against what the radio
/// actually holds after its commit+reboot. Read-only.
#[tauri::command]
pub async fn verify_anytone_program(
    port: String,
    expected_path: String,
) -> Result<AnytoneVerifyResult, String> {
    let bytes = std::fs::read(&expected_path)
        .map_err(|e| format!("could not read {expected_path}: {e}"))?;
    let expected = parse_dump(&bytes).map_err(|e| format!("{expected_path}: {e}"))?;
    tauri::async_runtime::spawn_blocking(move || {
        let mut p = open_port(&port)?;
        let _ident = enter_program_and_ident(&mut *p)?;
        let mut actual = Vec::with_capacity(expected.len());
        for (addr, data) in &expected {
            match read_block(&mut *p, *addr, data.len()) {
                Ok(read) => actual.push((*addr, read)),
                Err(e) => {
                    let _ = end_session(&mut *p);
                    return Err(format!("read 0x{addr:08X}+0x{:X} failed: {e}", data.len()));
                }
            }
        }
        let _ = end_session(&mut *p);
        let mismatches = diff_dumps(&expected, &actual);
        Ok(AnytoneVerifyResult {
            ok: mismatches.is_empty(),
            bytes_checked: expected.iter().map(|(_, d)| d.len()).sum(),
            mismatches,
        })
    })
    .await
    .estr()?
}

/// The newest program image pair on disk (`anytone-program-{stamp}.bin`
/// backup + its `.expected.bin`), so Verify/Restore stay reachable even when
/// the `program_anytone_codeplug` result never made it back to the UI (the
/// commit reboot drops USB, which can disrupt the IPC round-trip).
#[derive(serde::Serialize)]
pub struct AnytoneLatestProgram {
    pub backup_path: String,
    pub expected_path: String,
    pub stamp: String,
}

/// Find the most recent program image pair in the app's `radio-backups` dir.
/// Returns `None` if no program has ever been written. Read-only; no radio.
#[tauri::command]
pub async fn latest_anytone_program(
    app: AppHandle,
) -> Result<Option<AnytoneLatestProgram>, String> {
    let dir = app.path().app_data_dir().estr()?.join("radio-backups");
    // Both the channel-set program and the settings push write a
    // `{prefix}{stamp}.expected.bin` / `{prefix}{stamp}.bin` pair; either is a
    // valid Verify/Restore target. Track the newest by full base filename.
    const PREFIXES: &[&str] = &["anytone-program-", "anytone-settings-write-"];
    let mut newest: Option<(String, String)> = None; // (stamp, base filename)
    let entries = match std::fs::read_dir(&dir) {
        Ok(e) => e,
        Err(_) => return Ok(None), // dir may not exist yet
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        let Some(base) = name.strip_suffix(".expected.bin") else {
            continue;
        };
        // The stamp (YYYYMMDD-HHMMSS) sorts lexicographically, so max = newest.
        let Some(stamp) = PREFIXES.iter().find_map(|p| base.strip_prefix(p)) else {
            continue;
        };
        if newest.as_ref().is_none_or(|(cur, _)| stamp > cur.as_str()) {
            newest = Some((stamp.to_string(), base.to_string()));
        }
    }
    Ok(newest.map(|(stamp, base)| {
        let expected = dir.join(format!("{base}.expected.bin"));
        let backup = dir.join(format!("{base}.bin"));
        AnytoneLatestProgram {
            backup_path: backup.to_string_lossy().to_string(),
            expected_path: expected.to_string_lossy().to_string(),
            stamp,
        }
    }))
}


/// Safety net: write a program backup (the `[addr][len][data]` file from
/// `program_anytone_codeplug`) back to the radio, restoring every window it
/// captured, then END once. Brick-capable; gated behind the dialog.
#[tauri::command]
pub async fn restore_anytone_backup(
    port: String,
    backup_path: String,
) -> Result<AnytonePatchWriteResult, String> {
    let bytes = std::fs::read(&backup_path)
        .map_err(|e| format!("could not read {backup_path}: {e}"))?;
    let blocks = parse_dump(&bytes).map_err(|e| format!("{backup_path}: {e}"))?;
    if blocks.is_empty() {
        return Err(format!("{backup_path} contains no data blocks"));
    }
    for (addr, data) in &blocks {
        if !(*addr as usize).is_multiple_of(WRITE_WINDOW) || data.len() % WRITE_WINDOW != 0 {
            return Err(format!(
                "{backup_path}: block 0x{addr:08X}+0x{:X} is not 0x4000-aligned — not a \
                 program backup",
                data.len()
            ));
        }
    }
    tauri::async_runtime::spawn_blocking(move || {
        let mut p = open_port(&port)?;
        let _ident = enter_program_and_ident(&mut *p)?;
        let mut written = Vec::new();
        for (addr, data) in &blocks {
            write_block(&mut *p, *addr, data)?;
            written.push(format!("0x{addr:08X}"));
        }
        let _ = end_session(&mut *p);
        Ok(AnytonePatchWriteResult {
            windows_written: written,
            backup_path,
            note: "Backup restored and committed (radio rebooted). Verify with a fresh \
                   download once USB re-enumerates."
                .to_string(),
        })
    })
    .await
    .estr()?
}
