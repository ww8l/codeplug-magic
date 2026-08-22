//! The `radio-backups/` folder: what is in it, and how to make it smaller.
//!
//! Every read from a radio and every write to one drops a raw memory image
//! here first, and nothing has ever taken one away — 144 files and 37 MB on
//! this machine after two months, growing with every session (#77). Each of
//! those images is the radio's flash: the operator's call sign, DMR ID,
//! contacts, and any position they have stored.
//!
//! Nothing here deletes on its own. A backup is the recovery path after a bad
//! write, and the oldest one is often the closest thing to a factory image the
//! operator has; the app that silently ate it would be worse than the one that
//! grows. So this reports what is there, groups it by radio, and offers a prune
//! the operator asks for — after seeing the file count and the byte count it
//! would take.
//!
//! Files are grouped by the leading token of their name, which is what every
//! writer here puts first: the driver key on newer paths (`baofeng_uv5r-…`) and
//! a short radio name on the ones that predate them (`uv5r-…`, `tdh3-…`,
//! `anytone-…`). A name whose token is unrecognised still gets grouped and
//! counted under that token — reporting a file this app cannot name beats
//! hiding it — but see `plan_prune` for what that means for deleting.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::Serialize;
use tauri::{AppHandle, Manager};

use crate::error::MapErrString;

/// Radio backups worth keeping per radio when the operator prunes. Offered in
/// the UI rather than applied automatically.
pub const DEFAULT_KEEP: usize = 5;

/// One radio's worth of backups.
#[derive(Serialize)]
pub struct BackupGroup {
    /// The filename token these share (`uv5r`, `anytone`, `baofeng_uv5r`, …).
    pub key: String,
    /// What to call it on screen.
    pub label: String,
    pub files: usize,
    pub bytes: u64,
    /// Newest backup in the group, as `YYYY-MM-DD`.
    pub newest: Option<String>,
    /// What a prune to `keep` would remove from this group.
    pub prunable_files: usize,
    pub prunable_bytes: u64,
}

#[derive(Serialize)]
pub struct BackupsSummary {
    pub dir: String,
    pub files: usize,
    pub bytes: u64,
    pub groups: Vec<BackupGroup>,
    /// How many are kept per radio in the prunable counts above.
    pub keep: usize,
}

#[derive(Serialize)]
pub struct PruneResult {
    pub files_deleted: usize,
    pub bytes_freed: u64,
}

/// One backup as far as pruning is concerned: usually a single file, but a
/// write that leaves an `.expected.bin` beside its `.bin` is one backup in two
/// files and has to be kept or dropped as a unit.
struct Backup {
    stem: String,
    paths: Vec<PathBuf>,
    bytes: u64,
    /// Milliseconds since the epoch; the file's own time, not its name, so a
    /// file whose name this app does not parse still sorts correctly.
    modified: u64,
}

/// The label to show for a group key. Unknown keys are shown as themselves —
/// a folder holding something unexpected should say so, not hide it.
fn label_for(key: &str) -> String {
    match key {
        "uv5r" | "baofeng_uv5r" => "Baofeng UV-5R",
        "tdh3" | "tidradio_tdh3" => "TIDRADIO TD-H3",
        "anytone" | "anytone_atd890uv" => "AnyTone AT-D890UV",
        "ft5d" | "yaesu_ft5d" => "Yaesu FT5D",
        "id52" | "icom_id52" => "Icom ID-52",
        "thd75" | "kenwood_thd75" => "Kenwood TH-D75",
        other => return other.to_string(),
    }
    .to_string()
}

/// Read the folder into one entry per backup. Missing folder = nothing to
/// report, which is the normal state before the first radio session.
fn scan(dir: &Path) -> Vec<Backup> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    // Keyed by stem so the two halves of a write pair land in one entry.
    let mut by_stem: BTreeMap<String, Backup> = BTreeMap::new();
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(meta) = entry.metadata() else { continue };
        if !meta.is_file() {
            continue;
        }
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        // Hidden files are not ours (.DS_Store, and whatever else Finder
        // leaves), and deleting somebody else's file is not on the table.
        if name.starts_with('.') {
            continue;
        }
        let stem = name
            .rsplit_once('.')
            .map(|(base, _)| base.trim_end_matches(".expected").to_string())
            .unwrap_or_else(|| name.to_string());
        let modified = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        let slot = by_stem.entry(stem.clone()).or_insert_with(|| Backup {
            stem,
            paths: Vec::new(),
            bytes: 0,
            modified: 0,
        });
        slot.paths.push(path);
        slot.bytes += meta.len();
        slot.modified = slot.modified.max(modified);
    }
    by_stem.into_values().collect()
}

/// The group key for a backup: everything before the first `-`.
fn group_key(stem: &str) -> &str {
    stem.split_once('-').map(|(head, _)| head).unwrap_or(stem)
}

/// What a prune keeping `keep` backups per radio would delete, newest first
/// within each group.
///
/// `keep` of 0 is treated as 1: an operator asking to tidy up is not asking to
/// be left with no way back onto a radio they have just written.
fn doomed_stems(backups: &[Backup], keep: usize) -> Vec<&str> {
    let keep = keep.max(1);
    let mut by_group: BTreeMap<&str, Vec<&Backup>> = BTreeMap::new();
    for b in backups {
        by_group.entry(group_key(&b.stem)).or_default().push(b);
    }
    let mut doomed: Vec<&str> = Vec::new();
    for (_, mut group) in by_group {
        // Newest first, and on a filesystem too coarse to separate two rapid
        // writes, by name — so the same folder always prunes the same way.
        group.sort_by(|a, b| b.modified.cmp(&a.modified).then(b.stem.cmp(&a.stem)));
        doomed.extend(group.into_iter().skip(keep).map(|b| b.stem.as_str()));
    }
    doomed
}

/// The same plan, as the backups themselves.
fn plan_prune(backups: Vec<Backup>, keep: usize) -> Vec<Backup> {
    let doomed: Vec<String> = doomed_stems(&backups, keep)
        .into_iter()
        .map(str::to_string)
        .collect();
    backups
        .into_iter()
        .filter(|b| doomed.contains(&b.stem))
        .collect()
}

fn backups_path(app: &AppHandle) -> Result<PathBuf, String> {
    Ok(app.path().app_data_dir().estr()?.join("radio-backups"))
}

/// Absolute path of the radio-backups directory, so the UI can default the
/// "Restore backup…" file picker there and open it in the file manager (it
/// lives under the app data dir, which is otherwise awkward to find).
#[tauri::command]
pub async fn backups_dir(app: AppHandle) -> Result<String, String> {
    Ok(backups_path(&app)?.to_string_lossy().to_string())
}

/// What is in `radio-backups/`, by radio, and what pruning to `keep` per radio
/// would take. Reports only — see `prune_radio_backups` for the half that
/// deletes.
#[tauri::command]
pub async fn radio_backups_summary(
    app: AppHandle,
    keep: Option<usize>,
) -> Result<BackupsSummary, String> {
    let dir = backups_path(&app)?;
    let keep = keep.unwrap_or(DEFAULT_KEEP);
    let backups = scan(&dir);

    let files = backups.iter().map(|b| b.paths.len()).sum();
    let bytes = backups.iter().map(|b| b.bytes).sum();

    let doomed = doomed_stems(&backups, keep);
    let mut prunable: BTreeMap<String, (usize, u64)> = BTreeMap::new();
    for b in backups.iter().filter(|b| doomed.contains(&b.stem.as_str())) {
        let e = prunable
            .entry(group_key(&b.stem).to_string())
            .or_insert((0, 0));
        e.0 += b.paths.len();
        e.1 += b.bytes;
    }

    let mut grouped: BTreeMap<String, (usize, u64, u64)> = BTreeMap::new();
    for b in &backups {
        let e = grouped
            .entry(group_key(&b.stem).to_string())
            .or_insert((0, 0, 0));
        e.0 += b.paths.len();
        e.1 += b.bytes;
        e.2 = e.2.max(b.modified);
    }

    let groups = grouped
        .into_iter()
        .map(|(key, (files, bytes, newest))| {
            let (prunable_files, prunable_bytes) =
                prunable.get(&key).copied().unwrap_or((0, 0));
            BackupGroup {
                label: label_for(&key),
                key,
                files,
                bytes,
                newest: day(newest),
                prunable_files,
                prunable_bytes,
            }
        })
        .collect();

    Ok(BackupsSummary {
        dir: dir.to_string_lossy().to_string(),
        files,
        bytes,
        groups,
        keep,
    })
}

/// `YYYY-MM-DD` in local time for a unix timestamp in milliseconds, or None for
/// the 0 a file with no readable time gets.
fn day(millis: u64) -> Option<String> {
    if millis == 0 {
        return None;
    }
    use chrono::TimeZone;
    chrono::Local
        .timestamp_opt((millis / 1000) as i64, 0)
        .single()
        .map(|t| t.format("%Y-%m-%d").to_string())
}

/// Delete all but the `keep` most recent backups per radio. Only ever runs when
/// the operator asks for it, from a confirm that named these numbers.
#[tauri::command]
pub async fn prune_radio_backups(
    app: AppHandle,
    keep: Option<usize>,
) -> Result<PruneResult, String> {
    prune_dir(&backups_path(&app)?, keep.unwrap_or(DEFAULT_KEEP))
}

/// The prune itself, against a folder — the half worth testing, since it is the
/// only code in this app that deletes a radio backup.
fn prune_dir(dir: &Path, keep: usize) -> Result<PruneResult, String> {
    let mut result = PruneResult {
        files_deleted: 0,
        bytes_freed: 0,
    };
    for b in plan_prune(scan(dir), keep) {
        for path in &b.paths {
            std::fs::remove_file(path)
                .map_err(|e| format!("could not delete {}: {e}", path.display()))?;
            result.files_deleted += 1;
        }
        result.bytes_freed += b.bytes;
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Write `name` with `len` bytes. Files are written oldest-first, so their
    /// own modified times give the ordering `plan_prune` sorts on — no fixture
    /// clock, and it exercises the same `metadata().modified()` the app reads.
    /// Names are chosen so the tie-break (descending stem) agrees with that
    /// order on a filesystem too coarse to separate two rapid writes.
    fn write(dir: &Path, name: &str, len: usize) {
        std::fs::write(dir.join(name), vec![0u8; len]).unwrap();
    }

    fn tempdir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "cpm-backups-test-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn backups_group_by_the_radio_their_name_starts_with() {
        assert_eq!(group_key("uv5r-20260701-120000"), "uv5r");
        assert_eq!(group_key("baofeng_uv5r-20260701-120000"), "baofeng_uv5r");
        assert_eq!(group_key("tdh3-prewrite-laramie-td-h3-20260701-120000"), "tdh3");
        // Nothing to split on: the whole name is its own group rather than
        // being lumped in with somebody else's.
        assert_eq!(group_key("stray"), "stray");
    }

    #[test]
    fn a_write_and_its_expected_image_are_one_backup() {
        let dir = tempdir();
        write(&dir, "anytone-program-20260705-092635.bin", 100);
        write(&dir, "anytone-program-20260705-092635.expected.bin", 100);
        let scanned = scan(&dir);
        assert_eq!(scanned.len(), 1, "the pair is one backup");
        assert_eq!(scanned[0].paths.len(), 2);
        assert_eq!(scanned[0].bytes, 200);
    }

    #[test]
    fn a_prune_keeps_the_newest_per_radio_and_counts_the_rest() {
        let dir = tempdir();
        write(&dir, "tdh3-a.img", 10);
        for name in ["uv5r-a.img", "uv5r-b.img", "uv5r-c.img"] {
            write(&dir, name, 10);
        }

        // Keeping two leaves the oldest UV-5R backup and nothing else.
        let doomed = plan_prune(scan(&dir), 2);
        assert_eq!(doomed.len(), 1);
        assert_eq!(doomed[0].stem, "uv5r-a");

        // The TD-H3 has one backup, so no amount of pruning touches it — a
        // per-radio cap is not a per-folder cap.
        assert!(plan_prune(scan(&dir), 1)
            .iter()
            .all(|b| b.stem.starts_with("uv5r")));
        assert_eq!(plan_prune(scan(&dir), 1).len(), 2);

        // Nothing to do when every radio is already under the cap.
        assert!(plan_prune(scan(&dir), 5).is_empty());
    }

    /// "Tidy up" is never "leave me with nothing to restore from".
    #[test]
    fn keeping_zero_still_keeps_one() {
        let dir = tempdir();
        write(&dir, "uv5r-a.img", 10);
        write(&dir, "uv5r-b.img", 10);
        let doomed = plan_prune(scan(&dir), 0);
        assert_eq!(doomed.len(), 1);
        assert_eq!(doomed[0].stem, "uv5r-a", "the older one goes");
    }

    /// The only code in the app that deletes a radio backup, run for real
    /// against a folder: the newest survive, the pair goes together, and the
    /// radio with one backup is untouched.
    #[test]
    fn pruning_deletes_exactly_what_the_plan_named() {
        let dir = tempdir();
        write(&dir, "tdh3-a.img", 10);
        write(&dir, "uv5r-a.img", 10);
        write(&dir, "uv5r-b.img", 10);
        write(&dir, "anytone-program-a.bin", 30);
        write(&dir, "anytone-program-a.expected.bin", 30);
        write(&dir, "anytone-program-b.bin", 30);

        let res = prune_dir(&dir, 1).unwrap();
        assert_eq!(res.files_deleted, 3, "uv5r-a plus both halves of program-a");
        assert_eq!(res.bytes_freed, 70);

        let mut left: Vec<String> = std::fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        left.sort();
        assert_eq!(left, ["anytone-program-b.bin", "tdh3-a.img", "uv5r-b.img"]);

        // Running it again has nothing left to do.
        let again = prune_dir(&dir, 1).unwrap();
        assert_eq!(again.files_deleted, 0);
    }

    #[test]
    fn hidden_files_and_folders_are_not_ours_to_count_or_delete() {
        let dir = tempdir();
        write(&dir, ".DS_Store", 10);
        std::fs::create_dir_all(dir.join("subfolder")).unwrap();
        write(&dir, "uv5r-a.img", 10);
        let scanned = scan(&dir);
        assert_eq!(scanned.len(), 1);
        assert_eq!(scanned[0].stem, "uv5r-a");
    }

    #[test]
    fn a_folder_that_does_not_exist_yet_is_simply_empty() {
        assert!(scan(&std::env::temp_dir().join("cpm-no-such-backups-dir")).is_empty());
    }

    #[test]
    fn a_radio_this_app_knows_is_named_and_one_it_does_not_is_shown_as_itself() {
        assert_eq!(label_for("uv5r"), "Baofeng UV-5R");
        assert_eq!(label_for("kenwood_thd75"), "Kenwood TH-D75");
        assert_eq!(label_for("something-else"), "something-else");
    }
}
