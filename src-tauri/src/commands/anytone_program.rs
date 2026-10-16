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
    diff_dumps, end_session, enter_program_and_ident_as, open_port, parse_dump, read_block,
    write_block, AnytoneDumpDiff, AnytonePatchWriteResult, IdentExpect, D890_IDENT,
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
// Family dispatch
// ============================================================

/// One AnyTone model the program dialog serves. The dialog's Verify, Restore
/// and "latest program" actions are shared across the family; what differs is
/// the ident a session must find, which windows a restore may write, and the
/// file-name prefixes that model's backups carry.
/// A model's restore guard: refuses a backup holding windows it never writes.
type RestoreCheck = fn(&[(u32, Vec<u8>)]) -> Result<(), String>;

/// How a model writes one restored block.
type BlockWrite = fn(&mut dyn serialport::SerialPort, u32, &[u8]) -> Result<(), String>;

struct AnytoneFamily {
    ident: IdentExpect,
    check_restore: RestoreCheck,
    /// The D578 never sends a flash half's marker frame, even from a backup.
    write: BlockWrite,
    /// Backups this model writes (newest-first search in `latest_anytone_program`).
    prefixes: &'static [&'static str],
}

const D890_FAMILY: AnytoneFamily = AnytoneFamily {
    ident: D890_IDENT,
    check_restore: super::anytone::check_restore_blocks,
    write: write_block,
    prefixes: &["anytone-program-", "anytone-settings-write-"],
};

const D578_FAMILY: AnytoneFamily = AnytoneFamily {
    ident: crate::radios::anytone_atd578uv::D578_IDENT,
    check_restore: crate::radios::anytone_atd578uv::check_restore_blocks,
    write: crate::radios::anytone_atd578uv::write_sendable,
    prefixes: &[
        crate::radios::anytone_atd578uv::program::PROGRAM_PREFIX,
        crate::radios::anytone_atd578uv::settings::SETTINGS_PREFIX,
    ],
};

/// Every family member, for the cross-model file check.
const FAMILIES: [(&str, &AnytoneFamily); 2] =
    [("anytone_atd890uv", &D890_FAMILY), ("anytone_atd578uv", &D578_FAMILY)];

fn family(driver_key: &str) -> Result<&'static AnytoneFamily, String> {
    FAMILIES
        .iter()
        .find(|(k, _)| *k == driver_key)
        .map(|(_, f)| *f)
        .ok_or_else(|| format!("'{driver_key}' is not an AnyTone radio this dialog programs"))
}

/// Refuse a backup or expected image written for a DIFFERENT AnyTone model. The
/// maps overlap — the D578's zone lists sit at the D890's first channel bank —
/// so a window check alone would let one radio's backup through onto the other.
/// A file carrying no family prefix is left to the window check, which keeps
/// the D890's older backups restorable exactly as before.
fn check_file_belongs(driver_key: &str, path: &str) -> Result<(), String> {
    let name = std::path::Path::new(path)
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    for (key, fam) in FAMILIES {
        if key != driver_key && fam.prefixes.iter().any(|p| name.starts_with(p)) {
            return Err(format!(
                "{name} was written for a different radio ({key}) — refusing to use it here"
            ));
        }
    }
    let own = family(driver_key)?;
    if driver_key != "anytone_atd890uv" && !own.prefixes.iter().any(|p| name.starts_with(p)) {
        return Err(format!("{name} is not a backup this app wrote for this radio"));
    }
    Ok(())
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
    driver_key: String,
    port: String,
    expected_path: String,
) -> Result<AnytoneVerifyResult, String> {
    let fam = family(&driver_key)?;
    check_file_belongs(&driver_key, &expected_path)?;
    let bytes = std::fs::read(&expected_path)
        .map_err(|e| format!("could not read {expected_path}: {e}"))?;
    let expected = parse_dump(&bytes).map_err(|e| format!("{expected_path}: {e}"))?;
    tauri::async_runtime::spawn_blocking(move || {
        // One radio operation per port: this guard lives for the whole blocking
        // session and releases on the way out, panic included. (#67)
        let _port_guard = crate::radios::port_lock::claim(&port)?;
        let mut p = open_port(&port)?;
        let _ident = enter_program_and_ident_as(&mut *p, fam.ident)?;
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
    driver_key: String,
) -> Result<Option<AnytoneLatestProgram>, String> {
    let dir = app.path().app_data_dir().estr()?.join("radio-backups");
    // Both the channel-set program and the settings push write a
    // `{prefix}{stamp}.expected.bin` / `{prefix}{stamp}.bin` pair; either is a
    // valid Verify/Restore target. Track the newest by full base filename —
    // among THIS model's prefixes only, so a D578 dialog never offers a D890
    // backup or the reverse.
    Ok(newest_program(&dir, family(&driver_key)?.prefixes))
}

/// The newest `{prefix}{stamp}.expected.bin` / `.bin` pair in `dir` among
/// `prefixes`, or `None`. Pure file listing; no radio.
fn newest_program(dir: &std::path::Path, prefixes: &[&str]) -> Option<AnytoneLatestProgram> {
    let mut newest: Option<(String, String)> = None; // (stamp, base filename)
    let entries = std::fs::read_dir(dir).ok()?; // the dir may not exist yet
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        let Some(base) = name.strip_suffix(".expected.bin") else {
            continue;
        };
        // The stamp (YYYYMMDD-HHMMSS) sorts lexicographically, so max = newest.
        let Some(stamp) = prefixes.iter().find_map(|p| base.strip_prefix(p)) else {
            continue;
        };
        if newest.as_ref().is_none_or(|(cur, _)| stamp > cur.as_str()) {
            newest = Some((stamp.to_string(), base.to_string()));
        }
    }
    newest.map(|(stamp, base)| {
        let expected = dir.join(format!("{base}.expected.bin"));
        let backup = dir.join(format!("{base}.bin"));
        AnytoneLatestProgram {
            backup_path: backup.to_string_lossy().to_string(),
            expected_path: expected.to_string_lossy().to_string(),
            stamp,
        }
    })
}


/// Safety net: write a program backup (the `[addr][len][data]` file from
/// `program::program_radio`) back to the radio, restoring every window it
/// captured, then END once. Brick-capable; gated behind the dialog.
#[tauri::command]
pub async fn restore_anytone_backup(
    driver_key: String,
    port: String,
    backup_path: String,
) -> Result<AnytonePatchWriteResult, String> {
    let fam = family(&driver_key)?;
    check_file_belongs(&driver_key, &backup_path)?;
    let bytes = std::fs::read(&backup_path)
        .map_err(|e| format!("could not read {backup_path}: {e}"))?;
    let blocks = parse_dump(&bytes).map_err(|e| format!("{backup_path}: {e}"))?;
    if blocks.is_empty() {
        return Err(format!("{backup_path} contains no data blocks"));
    }
    (fam.check_restore)(&blocks).map_err(|e| format!("{backup_path}: {e}"))?;
    tauri::async_runtime::spawn_blocking(move || {
        // One radio operation per port: this guard lives for the whole blocking
        // session and releases on the way out, panic included. (#67)
        let _port_guard = crate::radios::port_lock::claim(&port)?;
        let mut p = open_port(&port)?;
        let _ident = enter_program_and_ident_as(&mut *p, fam.ident)?;
        let mut written = Vec::new();
        for (addr, data) in &blocks {
            (fam.write)(&mut *p, *addr, data)?;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_dialog_model_finds_only_its_own_backups() {
        let dir = std::env::temp_dir().join(format!("cpm-anytone-latest-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        for name in [
            "anytone-program-20261001-090000.expected.bin",
            "anytone-settings-write-20261002-090000.expected.bin",
            "atd578uv-program-20260930-090000.expected.bin",
            "atd578uv-settings-write-20260929-090000.expected.bin",
        ] {
            std::fs::write(dir.join(name), b"").unwrap();
        }
        let d890 = newest_program(&dir, D890_FAMILY.prefixes).unwrap();
        assert!(d890.expected_path.ends_with("anytone-settings-write-20261002-090000.expected.bin"));
        let d578 = newest_program(&dir, D578_FAMILY.prefixes).unwrap();
        assert!(d578.expected_path.ends_with("atd578uv-program-20260930-090000.expected.bin"));
        assert!(d578.backup_path.ends_with("atd578uv-program-20260930-090000.bin"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_backup_from_the_other_model_is_refused() {
        assert!(check_file_belongs("anytone_atd890uv", "/x/anytone-program-20261001-090000.bin").is_ok());
        assert!(check_file_belongs("anytone_atd890uv", "/x/atd578uv-program-20261001-090000.bin").is_err());
        assert!(check_file_belongs("anytone_atd578uv", "/x/atd578uv-settings-write-1.bin").is_ok());
        assert!(check_file_belongs("anytone_atd578uv", "/x/anytone-program-20261001-090000.bin").is_err());
        assert!(check_file_belongs("anytone_atd578uv", "/x/something-else.bin").is_err());
        assert!(family("tyt_md380").is_err());
    }

    /// The gate against the real files: every AnyTone backup on this machine's
    /// prod and dev app data stays usable by the D890 dialog, and none of them is
    /// accepted by the D578's. Skips when the directories are absent (CI).
    #[test]
    fn real_backups_on_this_machine_keep_their_dialog() {
        let Some(home) = std::env::var_os("HOME") else { return };
        let mut seen = 0;
        for app in ["com.ww8l.codeplugmagic", "com.ww8l.codeplugmagic.dev"] {
            let dir = std::path::Path::new(&home)
                .join("Library/Application Support")
                .join(app)
                .join("radio-backups");
            let Ok(entries) = std::fs::read_dir(&dir) else { continue };
            for e in entries.flatten() {
                let name = e.file_name().to_string_lossy().to_string();
                if !D890_FAMILY.prefixes.iter().any(|p| name.starts_with(p)) {
                    continue;
                }
                seen += 1;
                let path = e.path().to_string_lossy().to_string();
                assert!(check_file_belongs("anytone_atd890uv", &path).is_ok(), "{name}");
                assert!(check_file_belongs("anytone_atd578uv", &path).is_err(), "{name}");
            }
        }
        eprintln!("checked {seen} real D890 backups");
    }
}
