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
//!   2. the hardware session (now `CodeplugProgrammer::program` on the driver,
//!      reached via `program::program_radio`). Reads every region
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
    check_restore_blocks, diff_dumps, end_session, enter_program_and_ident, open_port, parse_dump,
    read_block, write_block, AnytoneDumpDiff, AnytonePatchWriteResult,
};
// The plan/session half moved into the driver in 3.6b; the DTOs it returns are
// the generic driver-level ones, so the wire shape (and the frontend) is
// unchanged.
use crate::radios::driver::CodeplugPreview;
use crate::radios::registry::driver_for_model;
// run_settings_program + its DTO moved into the driver's settings module in 3.5;
// re-exported here so `commands::anytone_program::run_settings_program` (the
// settings-write RE example) keeps resolving until 3.6.
pub use super::anytone_settings::{
    encode_anytone_settings, run_settings_program, AnytoneSettingsProgramResult, SETTINGS_WINDOWS,
};
use super::export::resolve_codeplug_payload;

/// Result of a fresh-session byte-verify against an `.expected.bin` image.
#[derive(Serialize)]
pub struct AnytoneVerifyResult {
    pub ok: bool,
    pub bytes_checked: usize,
    pub mismatches: Vec<AnytoneDumpDiff>,
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

/// Preview what `program::program_radio` would write to this radio: counts, skipped
/// channels with reasons, and warnings. Pure DB — no radio needed.
#[tauri::command]
pub async fn program_anytone_preview(
    state: State<'_, AppState>,
    codeplug_id: i64,
) -> Result<CodeplugPreview, String> {
    let resolved = resolve_codeplug_payload(&state.pool, codeplug_id).await?;
    codeplug_programmer(&resolved.model)?.preview(&resolved.payload())
}

/// Fresh-session byte-verify: strict-read every range in an `.expected.bin`
/// image (from `program::program_radio`) and diff against what the radio
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
        // One radio operation per port: this guard lives for the whole blocking
        // session and releases on the way out, panic included. (#67)
        let _port_guard = crate::radios::port_lock::claim(&port)?;
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
/// the program result never made it back to the UI (the
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
/// `program::program_radio`) back to the radio, restoring every window it
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
    check_restore_blocks(&blocks).map_err(|e| format!("{backup_path}: {e}"))?;
    tauri::async_runtime::spawn_blocking(move || {
        // One radio operation per port: this guard lives for the whole blocking
        // session and releases on the way out, panic included. (#67)
        let _port_guard = crate::radios::port_lock::claim(&port)?;
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
