//! Registry-dispatched radio commands + the UV-5R's remaining per-radio ones.
//!
//! Two kinds of command live here. The generic ones resolve a driver through
//! `radios/registry.rs` and serve every radio: `identify_radio` and
//! `download_image` (Chunk 3.6c), then `read_radio_settings`,
//! `write_radio_settings`, and `write_callsign_db` (Chunk 3.6d), and
//! `program_radio` (Chunk 3.6e). `restore_image` is the last genuinely
//! UV-5R-specific one — it writes a CHIRP-format `.img` straight back, which
//! only means anything on a clone radio; the protocol it calls has lived in
//! `radios/baofeng_uv5r` since 3.3.
//!
//! `program_radio` dispatches on CAPABILITY, not on a driver key: clone radios
//! (`ImageProgrammer`) are programmed from a patched memory image, the AnyTone
//! (`CodeplugProgrammer`) from DB state as a full replace.
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
use crate::radios::driver::{
    CallsignDbReport, CallsignRecord, CodeplugProgramReport, DecodedChannelSample,
    DriverCapabilities, ImageProgramRequest, ProgramReport, RadioDriver, SettingsWriteReport,
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

/// What a driver can actually do, derived from its trait impls.
///
/// The frontend gates UI on this instead of comparing model names (Chunk 3.7):
/// which buttons a program dialog offers, and whether the profile editor shows
/// its read-from-radio bar, are ground truth from the driver rather than a
/// hand-maintained list that silently rots when a radio is added.
///
/// Static per driver, so the UI may fetch it once and cache it.
#[tauri::command]
pub async fn driver_capabilities(driver_key: String) -> Result<DriverCapabilities, String> {
    Ok(DriverCapabilities::of(driver(&driver_key)?))
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
            channels,
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

/// Read an FT5D's non-channel settings out of a microSD `BACKUP.dat`.
///
/// Removable cards that look like a radio's own backup card.
///
/// The radio always writes to the same place on the card, so there is nothing
/// for the operator to navigate to — asking them to find
/// `FT5D/BACKUP/BACKUP.dat` themselves is how you end up patching the wrong
/// file. Only cards that actually hold a readable, valid backup are offered;
/// anything doubtful is left to the manual picker.
#[derive(serde::Serialize)]
pub struct MemoryCard {
    /// Full path to the backup file, ready to hand to the exporter.
    pub path: String,
    /// The mounted volume, for showing the user which card this is.
    pub volume: String,
    /// True once a previous write has left a pristine copy beside it.
    pub has_original: bool,
}

/// Mounted removable media, wherever this OS puts it. macOS mounts under
/// /Volumes; Windows gives each card a drive letter. Both are cheap to
/// enumerate and neither needs any permission the app does not already have.
fn mounted_roots() -> Vec<std::path::PathBuf> {
    if cfg!(target_os = "windows") {
        ('D'..='Z')
            .map(|d| std::path::PathBuf::from(format!("{d}:\\")))
            .collect()
    } else {
        std::fs::read_dir("/Volumes")
            .map(|rd| rd.flatten().map(|e| e.path()).collect())
            .unwrap_or_default()
    }
}

fn volume_name(root: &std::path::Path) -> String {
    root.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| root.to_string_lossy().into_owned())
}

/// Find mounted cards holding a file this codeplug format can be written into.
///
/// Keyed on `export_format` rather than on a model or a driver, the same way
/// [`crate::commands::export::generate_codeplug`] picks its exporter — a format
/// with no card path simply finds nothing. Never errors: a card that cannot be
/// read is not offered, and an empty list means "use the picker".
#[tauri::command]
pub async fn find_memory_cards(format: String) -> Vec<MemoryCard> {
    tauri::async_runtime::spawn_blocking(move || match format.as_str() {
        "yaesu_ft5d_sd" => ft5d_cards(),
        "icom_id52_icf" => id52_cards(),
        "kenwood_thd75_sd" => thd75_cards(),
        _ => Vec::new(),
    })
    .await
    .unwrap_or_default()
}

/// The FT5D writes to one fixed path, so there is nothing for the operator to
/// navigate to.
fn ft5d_cards() -> Vec<MemoryCard> {
    let mut out = Vec::new();
    for root in mounted_roots() {
        let path = root.join("FT5D").join("BACKUP").join("BACKUP.dat");
        let Ok(image) = std::fs::read(&path) else {
            continue;
        };
        // Validate rather than trust the name — a stale file from another
        // model would otherwise be offered as a safe target.
        if crate::radios::yaesu_ft5d::sd_image::validate_backup(&image).is_err() {
            continue;
        }
        out.push(MemoryCard {
            has_original: path.with_extension("dat.orig").exists(),
            volume: volume_name(&root),
            path: path.to_string_lossy().into_owned(),
        });
    }
    out
}

/// The ID-52 keeps its settings files in one fixed folder but names them
/// itself, so what gets offered is the **folder**, not a file: the write creates
/// a new `SetYYYYMMDD_NN.icf` in it rather than overwriting anything the
/// operator saved. One entry per card, so the "write straight to the card"
/// action appears exactly as it does for the FT5D.
///
/// A folder is only offered if it holds at least one file this driver can
/// actually use as a template — a file from another Icom, or one saved to an
/// older firmware layout, is no more use here than no file at all.
fn id52_cards() -> Vec<MemoryCard> {
    let mut out = Vec::new();
    for root in mounted_roots() {
        let dir = root.join("ID-52").join("Setting");
        let usable = std::fs::read_dir(&dir).is_ok_and(|entries| {
            entries.flatten().any(|e| {
                let p = e.path();
                p.extension().is_some_and(|x| x.eq_ignore_ascii_case("icf"))
                    && std::fs::read_to_string(&p).is_ok_and(|t| {
                        crate::radios::icom_id52::icf::IcfFile::parse(&t)
                            .is_ok_and(|f| {
                                crate::radios::icom_id52::check_is_an_id52_file(&f).is_ok()
                            })
                    })
            })
        });
        if !usable {
            continue;
        }
        out.push(MemoryCard {
            // Nothing of the operator's is modified, so there is no pristine
            // copy to keep and nothing for the UI to reassure them about.
            has_original: false,
            volume: volume_name(&root),
            path: dir.to_string_lossy().into_owned(),
        });
    }
    out
}

/// The TH-D75 names its own config files too, so — like the ID-52 — what gets
/// offered is the **folder**, `KENWOOD/TH-D75/SETTINGS/DATA`. The write builds a
/// new `MMDDYYYY_HHMMSS.d75` in it from the newest save there, so every file the
/// operator has taken off the radio survives untouched.
///
/// A folder counts only if it holds a file this driver can actually use as a
/// template. That is a stricter test than "there is a `.d75` in here": Kenwood's
/// own MCP-D75 software writes the full clone image into the same folder with
/// the same extension, and a partial save declares the same header as a good
/// one. Both parse-fail by name, and neither should make the card look ready.
fn thd75_cards() -> Vec<MemoryCard> {
    let mut out = Vec::new();
    for root in mounted_roots() {
        let dir = root
            .join("KENWOOD")
            .join("TH-D75")
            .join("SETTINGS")
            .join("DATA");
        let usable = std::fs::read_dir(&dir).is_ok_and(|entries| {
            entries.flatten().any(|e| {
                let p = e.path();
                p.extension().is_some_and(|x| x.eq_ignore_ascii_case("d75"))
                    && std::fs::read(&p).is_ok_and(|b| {
                        crate::radios::kenwood_thd75::d75::D75File::parse(&b).is_ok()
                    })
            })
        });
        if !usable {
            continue;
        }
        out.push(MemoryCard {
            // A new file every time, so there is nothing of the operator's to
            // keep a pristine copy of.
            has_original: false,
            volume: volume_name(&root),
            path: dir.to_string_lossy().into_owned(),
        });
    }
    out
}

/// The file-based sibling of [`read_radio_settings`]. The FT5D has no proven
/// cable modality, so its "radio" for settings purposes is the backup file its
/// own Back Up menu writes — the same file the codeplug export patches. Nothing
/// is written here, and no backup copy is taken, because the source file is
/// itself the operator's backup and is left untouched.
///
/// Returns the decoded settings for the profile form to merge; saving them is
/// the form's job, exactly as with the cable path.
#[tauri::command]
pub async fn read_ft5d_settings_from_backup(path: String) -> Result<RadioSettingsRead, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let image = std::fs::read(&path).map_err(|e| {
            format!(
                "Could not read {path}: {e}. Pick FT5D/BACKUP/BACKUP.dat from the \
                 radio's microSD card."
            )
        })?;
        crate::radios::yaesu_ft5d::sd_image::validate_backup(&image)?;
        let settings = crate::radios::yaesu_ft5d::settings::decode_settings(&image);
        let count = settings.as_object().map(|o| o.len()).unwrap_or(0);
        Ok(RadioSettingsRead {
            settings,
            count,
            backup_path: path,
        })
    })
    .await
    .estr()?
}

/// The ID-52's version of [`read_ft5d_settings_from_backup`], reading the `.icf`
/// the radio writes with `SET > SD Card > Save Setting`.
///
/// Nothing is written here. The values land in the profile form, and they go
/// back out into an `.icf` when the codeplug is written to the card — one file
/// carries the memories and the settings together, because `Load Setting > ALL`
/// restores both in one operation.
#[tauri::command]
pub async fn read_id52_settings_from_card(path: String) -> Result<RadioSettingsRead, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let text = std::fs::read_to_string(&path).map_err(|e| {
            format!(
                "Could not read {path}: {e}. Pick a settings file from ID-52/Setting/ on \
                 the radio's microSD card — save one with SET > SD Card > Save Setting if \
                 it is not there."
            )
        })?;
        let icf = crate::radios::icom_id52::icf::IcfFile::parse(&text)?;
        crate::radios::icom_id52::check_is_an_id52_file(&icf)?;
        let settings = crate::radios::icom_id52::settings::decode_settings(icf.image());
        let count = settings.as_object().map(|o| o.len()).unwrap_or(0);
        Ok(RadioSettingsRead {
            settings,
            count,
            backup_path: path,
        })
    })
    .await
    .estr()?
}

/// The TH-D75's version of the same, reading the `.d75` the radio writes to its
/// microSD card.
///
/// Nothing is written here. The values land in the profile form and go back out
/// into a `.d75` when the codeplug is written, the way the ID-52's do — this
/// radio also carries memories and settings in one file.
///
/// Offsets in the settings table are BODY offsets, so the decoder gets
/// `d75.body()` rather than the whole file.
#[tauri::command]
pub async fn read_thd75_settings_from_card(path: String) -> Result<RadioSettingsRead, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let bytes = std::fs::read(&path).map_err(|e| {
            format!(
                "Could not read {path}: {e}. Pick a config file from \
                 KENWOOD/TH-D75/SETTINGS/DATA/ on the radio's microSD card — save one \
                 with Menu > Configuration > SD Card > Save Setting if it is not there."
            )
        })?;
        let d75 = crate::radios::kenwood_thd75::d75::D75File::parse(&bytes)?;
        let settings = crate::radios::kenwood_thd75::settings::decode_settings(d75.body());
        let count = settings.as_object().map(|o| o.len()).unwrap_or(0);
        Ok(RadioSettingsRead {
            settings,
            count,
            backup_path: path,
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
    let mut records: Vec<CallsignRecord> = users
        .into_iter()
        .map(|u| CallsignRecord {
            dmr_id: u.dmr_id as u32,
            group_call: false,
            name: u.display_name(),
            callsign: u.callsign,
            city: u.city,
            state: u.state,
            country: u.country,
            comment: u.remarks,
        })
        .collect();

    // Append the talkgroup library as GROUP-call entries (issue #30). The
    // caller-ID DB is what the radio's TX screen resolves a talkgroup ID
    // through, so a DB holding only individual users leaves TX showing a bare
    // number. `talkgroups` is a global reusable library, not codeplug-scoped,
    // which matches this command being radio-wide.
    //
    // Appended AFTER the users so that on a number collision the hand-curated
    // talkgroup wins the encoder's last-wins dedup. `max_count` deliberately
    // does not apply: it caps the bulk user library, and the talkgroups are the
    // few dozen entries the user actually cares about naming.
    let talkgroups: Vec<(i64, String, String)> =
        sqlx::query_as("SELECT tg_number, name, call_type FROM talkgroups ORDER BY tg_number")
            .fetch_all(&state.pool)
            .await
            .estr()?;
    records.extend(talkgroups.into_iter().map(|(tg_number, name, call_type)| {
        CallsignRecord {
            dmr_id: tg_number.max(0) as u32,
            // The library models a "talkgroup" that is really a private contact
            // (call_type 'Private'), so honour the column rather than assuming.
            group_call: !call_type.eq_ignore_ascii_case("private"),
            // Description field 1 ("name") is what the TX screen renders for a
            // group call — HW-proven (s76): TG 31088 displayed "Colorado HD" in
            // full, not the 8-char "Colorado" that field 3 ("call") would have
            // been truncated to. A talkgroup has no callsign, so field 3 stays
            // empty; do not put the name there too.
            name,
            callsign: String::new(),
            city: None,
            state: None,
            country: None,
            comment: None,
        }
    }));

    // Checked on the COMBINED set, not on the users alone: a codeplug with
    // talkgroups but no downloaded user library is still worth writing — it is
    // exactly what gives the TX screen its names.
    if records.is_empty() {
        return Err(
            "No DMR contacts or talkgroups to write — open DMR Contacts and Refresh the \
             library first (or widen the country selection)."
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

/// Program a codeplug to whichever radio it targets.
///
/// Registry-dispatched (Chunk 3.6e). Two programming models coexist behind one
/// command, which is why the dispatch is on capability rather than a driver key:
///
///   * [`CodeplugProgrammer`] radios (AnyTone D890UV) are programmed from DB
///     state as a full replace — channels *plus* zones, scan lists, and contacts
///     — so they get the whole resolved payload.
///   * [`ImageProgrammer`] radios (UV-5R, TD-H3) are clone radios: the codeplug's
///     channels are patched into a freshly-read image, alongside the profile's
///     settings on radios that write them during a program.
///
/// `CodeplugProgrammer` is tried first: a radio advertising both would be one
/// that can do the richer DB-driven program, and that is the better write.
///
/// Brick-capable — every driver takes a mandatory backup before the first byte,
/// and the UI gates this behind an explicit acknowledgement.
#[tauri::command]
pub async fn program_radio(
    app: AppHandle,
    state: State<'_, AppState>,
    codeplug_id: i64,
    port: String,
) -> Result<CodeplugProgramReport, String> {
    let model = export::codeplug_model(&state.pool, codeplug_id).await?;
    let driver = registry::driver_for_model(&model).ok_or_else(|| {
        format!(
            "no live-radio driver for {} — it is export-only",
            model.display_name
        )
    })?;

    let backup_dir = app.path().app_data_dir().estr()?.join("radio-backups");
    std::fs::create_dir_all(&backup_dir).estr()?;

    let report = if let Some(programmer) = driver.as_codeplug_programmer() {
        // `resolved` owns every row the planner needs, so it moves into the
        // blocking task wholesale — the payload borrows from it in there.
        let resolved = export::resolve_codeplug_payload(&state.pool, codeplug_id).await?;
        let report = tauri::async_runtime::spawn_blocking(move || {
            programmer.program(&port, &resolved.payload(), &backup_dir)
        })
        .await
        .estr()??;
        program_report_to_generic(report)
    } else if let Some(imager) = driver.as_image_programmer() {
        // Included channels pack contiguously from slot 0; excluded (e.g.
        // digital-mode) channels drop out and the rest close up behind them.
        let (_model, slots) = export::resolve_codeplug_slots(&state.pool, codeplug_id).await?;

        // The radio profile's settings + the model's schema. Present on the
        // UV-5R (which makes the profile authoritative over every editable
        // setting during a program); the TD-H3 ignores them and pushes settings
        // through its separate, explicitly-acknowledged settings write.
        let profile_settings: Option<String> = sqlx::query_scalar(
            "SELECT rp.non_channel_settings FROM codeplugs cp \
             JOIN radio_profiles rp ON rp.id = cp.radio_profile_id WHERE cp.id = ?1",
        )
        .bind(codeplug_id)
        .fetch_optional(&state.pool)
        .await
        .estr()?
        .flatten();
        let schema = model.non_channel_settings_schema.clone();

        // Label the pre-write backup with the codeplug name so several
        // codeplugs for one radio stay distinguishable when restoring.
        let label: String = sqlx::query_scalar("SELECT name FROM codeplugs WHERE id = ?1")
            .bind(codeplug_id)
            .fetch_one(&state.pool)
            .await
            .estr()?;

        tauri::async_runtime::spawn_blocking(move || {
            let settings = match (&profile_settings, &schema) {
                (Some(s), Some(schema)) => {
                    let value: serde_json::Value = serde_json::from_str(s)
                        .map_err(|e| format!("saved profile settings are not valid JSON: {e}"))?;
                    Some((value, schema.as_str()))
                }
                _ => None,
            };
            let req = ImageProgramRequest {
                model: &model,
                channels: &slots,
                settings: settings.as_ref().map(|(v, s)| (v, *s)),
                backup_dir: &backup_dir,
                label: &label,
            };
            imager.program_codeplug(&port, &req)
        })
        .await
        .estr()??
    } else {
        return Err(format!(
            "{} cannot be programmed over the cable",
            driver.display_name()
        ));
    };

    // Every block ack'd, so record this as the codeplug's last program time (the
    // Codeplugs screen shows "Programmed <date>"). Verification is best-effort
    // and deliberately does not gate the stamp.
    sqlx::query(
        "UPDATE codeplugs SET last_exported = CURRENT_TIMESTAMP, last_export_kind = 'radio' WHERE id = ?1",
    )
    .bind(codeplug_id)
    .execute(&state.pool)
    .await
    .estr()?;

    Ok(report)
}

/// Widen a [`CodeplugProgrammer`] report into the unified shape. The DB-driven
/// path reports zones/scan lists/contacts and an `.expected.bin` for a
/// fresh-session byte-diff, but cannot read back in-session (the commit reboots
/// the radio), so `verified` stays `None`.
fn program_report_to_generic(r: ProgramReport) -> CodeplugProgramReport {
    CodeplugProgramReport {
        channels_written: r.channels_written,
        slots_cleared: r.slots_cleared,
        settings_written: None,
        zones_written: r.zones_written,
        zones_cleared: r.zones_cleared,
        scan_lists_written: r.scan_lists_written,
        scan_lists_cleared: r.scan_lists_cleared,
        contacts_written: r.contacts_written,
        contacts_cleared: r.contacts_cleared,
        verified: None,
        note: Some(r.note),
        backup_path: r.backup_path,
        expected_path: Some(r.expected_path),
        windows_written: r.windows_written,
        channels: Vec::new(),
        skipped: Vec::new(),
        warnings: r.warnings,
    }
}

#[derive(Serialize)]
pub struct RestoreResult {
    pub bytes: usize,
    pub source: String,
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
