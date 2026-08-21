use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use sqlx::Acquire;
use tauri::State;

use crate::commands::channel_io::now_iso8601;
use crate::db::AppState;
use crate::error::MapErrString;
use crate::models::{ImportSummary, RepeaterTalkgroup, Talkgroup, TalkgroupInput};

// ============================================================
// BrandMeister list (downloaded, never bundled)
// ============================================================

/// The BrandMeister talkgroup list: `{ "<tg_number>": "<name>" }`.
const BRANDMEISTER_URL: &str = "https://api.brandmeister.network/v2/talkgroup/";

const FETCH_CONNECT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(20);
const FETCH_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(120);

/// The list is ~45 KB and about 1,800 entries. Ten megabytes is far past any
/// plausible growth and far short of anything that would trouble memory.
const MAX_LIST_BYTES: usize = 10 * 1024 * 1024;

#[derive(Debug, Clone, Serialize)]
pub struct TalkgroupRefreshSummary {
    /// Entries the service returned.
    pub fetched: usize,
    /// Rows inserted. Entries already in the library are left alone, so this is
    /// smaller than `fetched` on every run after the first.
    pub added: usize,
    /// Talkgroups in the library afterwards, from every source.
    pub total: i64,
}

/// Download the BrandMeister talkgroup list and add the ones the library does
/// not already hold.
///
/// This list used to ship inside the binary — `include_str!` of a June 2026
/// snapshot, seeded into every new database on first launch. That made this app
/// a redistributor of somebody else's directory, which is the same objection
/// that took the radioid.net records out of the repository (#56, #57). Now the
/// operator asks for it and it comes from BrandMeister, exactly like the DMR
/// user database does from radioid.net.
///
/// Existing rows are never touched: a talkgroup the operator renamed, or one
/// that came from a CSV import, survives a refresh. That is why the summary
/// reports `added` rather than "updated" — this fills gaps, it does not
/// overwrite the operator's library.
#[tauri::command]
pub async fn refresh_brandmeister_talkgroups(
    state: State<'_, AppState>,
) -> Result<TalkgroupRefreshSummary, String> {
    let mut list = fetch_brandmeister_list().await?;
    let fetched = list.len();

    // 9990 is BrandMeister's echo/parrot test — the one talkgroup every new
    // DMR operator is told to key up first. The bundled snapshot carried it
    // with BrandMeister's own unhelpful name ("None") and the seed renamed it;
    // the live endpoint does not serve it at all today (verified 2026-08-21 via
    // `cargo run --example brandmeister_dryrun`). Either way the library ends
    // up with a usable entry, and this is the app's own one row rather than
    // anybody's directory.
    list.insert("9990".to_string(), "Parrot (Echo Test)".to_string());

    let mut conn = state.pool.acquire().await.estr()?;
    let mut tx = conn.begin().await.estr()?;
    let mut added = 0usize;
    for (number, name) in &list {
        let Ok(tg_number) = number.parse::<i64>() else {
            continue;
        };
        let name = name.as_str();
        let res = sqlx::query(
            "INSERT INTO talkgroups (tg_number, name, network, call_type, source)
             VALUES (?1, ?2, 'Brandmeister', 'Group', 'brandmeister')
             ON CONFLICT(network, tg_number) DO NOTHING",
        )
        .bind(tg_number)
        .bind(name)
        .execute(&mut *tx)
        .await
        .estr()?;
        added += res.rows_affected() as usize;
    }
    tx.commit().await.estr()?;

    let total: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM talkgroups")
        .fetch_one(&state.pool)
        .await
        .estr()?;

    Ok(TalkgroupRefreshSummary {
        fetched,
        added,
        total: total.0,
    })
}

/// The fetch on its own, for `examples/brandmeister_dryrun.rs` — the live
/// service exercised without a database or a GUI session.
pub async fn fetch_brandmeister_list_for_diagnostics() -> Result<HashMap<String, String>, String> {
    fetch_brandmeister_list().await
}

/// Fetch and parse the list, bounded in time and in bytes.
async fn fetch_brandmeister_list() -> Result<HashMap<String, String>, String> {
    let client = crate::commands::http::client(FETCH_CONNECT_TIMEOUT, FETCH_TIMEOUT)?;
    let resp = client
        .get(BRANDMEISTER_URL)
        .send()
        .await
        .map_err(|e| format!("Could not reach BrandMeister: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("BrandMeister returned HTTP {}", resp.status()));
    }
    if let Some(len) = resp.content_length() {
        if len > MAX_LIST_BYTES as u64 {
            return Err(format!(
                "BrandMeister offered a {} MB download, far larger than the talkgroup list. \
                 Refusing it.",
                len / (1024 * 1024)
            ));
        }
    }
    let body = resp
        .bytes()
        .await
        .map_err(|e| format!("The download from BrandMeister was interrupted: {e}"))?;
    if body.len() > MAX_LIST_BYTES {
        return Err("The download from BrandMeister was far larger than the talkgroup list.".into());
    }
    parse_brandmeister_list(&body)
}

/// Parse the `{ "<tg>": "<name>" }` map. Separate from the I/O so the shape the
/// service actually returns is testable without the network.
fn parse_brandmeister_list(body: &[u8]) -> Result<HashMap<String, String>, String> {
    let list: HashMap<String, String> = serde_json::from_slice(body).map_err(|e| {
        format!("BrandMeister returned something this app could not read as a talkgroup list: {e}")
    })?;
    if list.is_empty() {
        return Err("BrandMeister returned an empty talkgroup list.".into());
    }
    Ok(list)
}

// ============================================================
// Talkgroup library
// ============================================================

/// List talkgroups, optionally filtered by a search string matched against the
/// name or the talkgroup number.
#[tauri::command]
pub async fn list_talkgroups(
    state: State<'_, AppState>,
    search: Option<String>,
) -> Result<Vec<Talkgroup>, String> {
    let term = search.unwrap_or_default();
    let term = term.trim();
    if term.is_empty() {
        sqlx::query_as::<_, Talkgroup>(
            "SELECT id, tg_number, name, network, call_type, notes, source, created_at, updated_at
             FROM talkgroups ORDER BY name",
        )
        .fetch_all(&state.pool)
        .await
        .estr()
    } else {
        let like = format!("%{term}%");
        sqlx::query_as::<_, Talkgroup>(
            "SELECT id, tg_number, name, network, call_type, notes, source, created_at, updated_at
             FROM talkgroups
             WHERE name LIKE ?1 OR CAST(tg_number AS TEXT) LIKE ?1
             ORDER BY name",
        )
        .bind(&like)
        .fetch_all(&state.pool)
        .await
        .estr()
    }
}

#[tauri::command]
pub async fn create_talkgroup(
    state: State<'_, AppState>,
    input: TalkgroupInput,
) -> Result<Talkgroup, String> {
    let id = sqlx::query(
        "INSERT INTO talkgroups (tg_number, name, network, call_type, notes, source)
         VALUES (?1, ?2, ?3, ?4, ?5, 'manual')",
    )
    .bind(input.tg_number)
    .bind(&input.name)
    .bind(&input.network)
    .bind(&input.call_type)
    .bind(&input.notes)
    .execute(&state.pool)
    .await
    .estr()?
    .last_insert_rowid();

    get_talkgroup(&state, id).await
}

#[tauri::command]
pub async fn update_talkgroup(
    state: State<'_, AppState>,
    id: i64,
    input: TalkgroupInput,
) -> Result<Talkgroup, String> {
    sqlx::query(
        "UPDATE talkgroups
         SET tg_number = ?2, name = ?3, network = ?4, call_type = ?5, notes = ?6,
             updated_at = CURRENT_TIMESTAMP
         WHERE id = ?1",
    )
    .bind(id)
    .bind(input.tg_number)
    .bind(&input.name)
    .bind(&input.network)
    .bind(&input.call_type)
    .bind(&input.notes)
    .execute(&state.pool)
    .await
    .estr()?;

    get_talkgroup(&state, id).await
}

#[tauri::command]
pub async fn delete_talkgroup(state: State<'_, AppState>, id: i64) -> Result<(), String> {
    // Cascades to repeater_talkgroups via the FK.
    sqlx::query("DELETE FROM talkgroups WHERE id = ?1")
        .bind(id)
        .execute(&state.pool)
        .await
        .estr()?;
    Ok(())
}

async fn get_talkgroup(state: &AppState, id: i64) -> Result<Talkgroup, String> {
    sqlx::query_as::<_, Talkgroup>(
        "SELECT id, tg_number, name, network, call_type, notes, source, created_at, updated_at
         FROM talkgroups WHERE id = ?1",
    )
    .bind(id)
    .fetch_one(&state.pool)
    .await
    .estr()
}

// ============================================================
// Per-repeater talkgroup assignments
// ============================================================

/// The talkgroups assigned to a repeater channel, ordered, with the library
/// fields joined in for display.
#[tauri::command]
pub async fn get_repeater_talkgroups(
    state: State<'_, AppState>,
    channel_id: i64,
) -> Result<Vec<RepeaterTalkgroup>, String> {
    sqlx::query_as::<_, RepeaterTalkgroup>(
        r#"
        SELECT rtg.id, rtg.channel_id, rtg.talkgroup_id, rtg.timeslot, rtg.position,
               rtg.name_override, tg.tg_number, tg.name, tg.network, tg.call_type
        FROM repeater_talkgroups rtg
        JOIN talkgroups tg ON tg.id = rtg.talkgroup_id
        WHERE rtg.channel_id = ?1
        ORDER BY rtg.position
        "#,
    )
    .bind(channel_id)
    .fetch_all(&state.pool)
    .await
    .estr()
}

/// Assign a talkgroup to a repeater channel on a given timeslot. Idempotent:
/// the UNIQUE(channel, talkgroup, timeslot) constraint means re-assigning the
/// same triple is a no-op rather than an error.
#[tauri::command]
pub async fn assign_talkgroup(
    state: State<'_, AppState>,
    channel_id: i64,
    talkgroup_id: i64,
    timeslot: i64,
) -> Result<(), String> {
    let exists: Option<(i64,)> = sqlx::query_as(
        "SELECT id FROM repeater_talkgroups
         WHERE channel_id = ?1 AND talkgroup_id = ?2 AND timeslot = ?3",
    )
    .bind(channel_id)
    .bind(talkgroup_id)
    .bind(timeslot)
    .fetch_optional(&state.pool)
    .await
    .estr()?;
    if exists.is_some() {
        return Ok(());
    }

    let next_pos: (i64,) = sqlx::query_as(
        "SELECT COALESCE(MAX(position), -1) + 1 FROM repeater_talkgroups WHERE channel_id = ?1",
    )
    .bind(channel_id)
    .fetch_one(&state.pool)
    .await
    .estr()?;

    sqlx::query(
        "INSERT INTO repeater_talkgroups (channel_id, talkgroup_id, timeslot, position)
         VALUES (?1, ?2, ?3, ?4)",
    )
    .bind(channel_id)
    .bind(talkgroup_id)
    .bind(timeslot)
    .bind(next_pos.0)
    .execute(&state.pool)
    .await
    .estr()?;
    Ok(())
}

/// Remove a single assignment row by its id.
#[tauri::command]
pub async fn remove_talkgroup_assignment(
    state: State<'_, AppState>,
    assignment_id: i64,
) -> Result<(), String> {
    sqlx::query("DELETE FROM repeater_talkgroups WHERE id = ?1")
        .bind(assignment_id)
        .execute(&state.pool)
        .await
        .estr()?;
    Ok(())
}

/// Replace the ordering of a repeater's talkgroup assignments. Done in a
/// transaction; ids not belonging to this channel are ignored.
#[tauri::command]
pub async fn reorder_repeater_talkgroups(
    state: State<'_, AppState>,
    channel_id: i64,
    ordered_assignment_ids: Vec<i64>,
) -> Result<(), String> {
    let mut conn = state.pool.acquire().await.estr()?;
    let mut tx = conn.begin().await.estr()?;

    for (pos, assignment_id) in ordered_assignment_ids.iter().enumerate() {
        sqlx::query(
            "UPDATE repeater_talkgroups SET position = ?1 WHERE id = ?2 AND channel_id = ?3",
        )
        .bind(pos as i64)
        .bind(assignment_id)
        .bind(channel_id)
        .execute(&mut *tx)
        .await
        .estr()?;
    }

    tx.commit().await.estr()?;
    Ok(())
}

// ============================================================
// CSV import
// ============================================================

/// A talkgroup parsed from an import file, ready to preview or insert.
#[derive(Debug, Clone, Serialize)]
pub struct ParsedTalkgroup {
    pub tg_number: i64,
    pub name: String,
    pub network: String,
    pub call_type: String,
    pub notes: Option<String>,
}

/// Result of parsing a talkgroup import file. `rows` is capped for very large
/// files; `total` is the true count that will be imported.
#[derive(Debug, Clone, Serialize)]
pub struct TalkgroupImportPreview {
    pub total: usize,
    pub rows: Vec<ParsedTalkgroup>,
}

/// Cap on preview rows sent to the UI.
const PREVIEW_CAP: usize = 1000;

#[tauri::command]
pub async fn preview_talkgroup_import(
    path: String,
    network: String,
) -> Result<TalkgroupImportPreview, String> {
    let parsed = parse_talkgroup_csv(&path, &network)?;
    let rows = parsed.iter().take(PREVIEW_CAP).cloned().collect();
    Ok(TalkgroupImportPreview {
        total: parsed.len(),
        rows,
    })
}

#[tauri::command]
pub async fn import_talkgroups(
    state: State<'_, AppState>,
    path: String,
    network: String,
) -> Result<ImportSummary, String> {
    let parsed = parse_talkgroup_csv(&path, &network)?;

    let mut conn = state.pool.acquire().await.estr()?;
    let mut tx = conn.begin().await.estr()?;

    let mut added = 0usize;
    let mut skipped = 0usize;
    for tg in &parsed {
        // Skip (don't clobber) talkgroups that already exist on this network so
        // user edits and seeds survive a re-import.
        let res = sqlx::query(
            "INSERT INTO talkgroups (tg_number, name, network, call_type, notes, source)
             VALUES (?1, ?2, ?3, ?4, ?5, 'import')
             ON CONFLICT(network, tg_number) DO NOTHING",
        )
        .bind(tg.tg_number)
        .bind(&tg.name)
        .bind(&tg.network)
        .bind(&tg.call_type)
        .bind(&tg.notes)
        .execute(&mut *tx)
        .await
        .estr()?;
        if res.rows_affected() > 0 {
            added += 1;
        } else {
            skipped += 1;
        }
    }

    tx.commit().await.estr()?;
    Ok(ImportSummary { added, skipped, updated: 0 })
}

/// Parse a talkgroup CSV. Header-driven and forgiving about column names:
/// the number can be `tg_number`/`number`/`id`/`tgid`/`talkgroup`, the name
/// `name`/`alias`, plus optional `network`/`call_type`(or `type`)/`notes`
/// columns. Rows without a positive number or a name are skipped. A row's own
/// `network` column wins over `default_network`.
fn parse_talkgroup_csv(path: &str, default_network: &str) -> Result<Vec<ParsedTalkgroup>, String> {
    let mut reader = csv::ReaderBuilder::new()
        .flexible(true)
        .trim(csv::Trim::All)
        .from_path(path)
        .map_err(|e| format!("Could not open CSV: {e}"))?;

    let headers = reader.headers().estr()?.clone();
    let mut col: HashMap<String, usize> = HashMap::new();
    for (i, h) in headers.iter().enumerate() {
        col.insert(h.trim().to_lowercase(), i);
    }

    // First matching alias wins.
    let get = |rec: &csv::StringRecord, names: &[&str]| -> Option<String> {
        names
            .iter()
            .find_map(|n| col.get(*n))
            .and_then(|&i| rec.get(i))
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
    };

    let default_network = default_network.trim();
    let default_network = if default_network.is_empty() {
        "Brandmeister"
    } else {
        default_network
    };

    let mut out = Vec::new();
    for result in reader.records() {
        let rec = result.estr()?;

        let tg_number = match get(&rec, &["tg_number", "number", "id", "tgid", "tg", "talkgroup id"])
            .and_then(|s| s.parse::<i64>().ok())
        {
            Some(n) if n > 0 => n,
            _ => continue,
        };
        let name = match get(&rec, &["name", "alias", "talkgroup name", "talkgroup"]) {
            Some(n) => n,
            None => continue,
        };
        let network = get(&rec, &["network"]).unwrap_or_else(|| default_network.to_string());
        let call_type = match get(&rec, &["call_type", "type"]).map(|s| s.to_lowercase()) {
            Some(t) if t.starts_with("priv") => "Private".to_string(),
            _ => "Group".to_string(),
        };
        let notes = get(&rec, &["notes", "description", "comment"]);

        out.push(ParsedTalkgroup {
            tg_number,
            name,
            network,
            call_type,
            notes,
        });
    }

    Ok(out)
}

// ============================================================
// Full-library JSON backup (lossless)
// ============================================================
//
// The CSV importer above only round-trips the four display fields and always
// forces `source = 'import'`. This backup captures the ENTIRE talkgroup library
// — including orphan talkgroups that aren't assigned to any channel and would
// therefore never appear in a channel backup — with `notes` and the original
// `source` preserved, so it survives a full database flush. Mirrors the channel
// backup format in `channel_io.rs`.

/// Magic value stamped into the export so import can recognise our own files.
const TALKGROUP_BACKUP_FORMAT: &str = "codeplug-magic-talkgroups";
/// Legacy format id from before the product was renamed; still accepted on import.
const LEGACY_TALKGROUP_BACKUP_FORMAT: &str = "73plug-talkgroups";
/// Current backup schema version (bump if the on-disk shape changes).
const TALKGROUP_BACKUP_VERSION: u32 = 1;

/// One talkgroup as stored in a backup file — every column except the surrogate
/// `id` and the DB-managed timestamps, so restore is lossless.
#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct BackupTalkgroupRow {
    pub tg_number: i64,
    pub name: String,
    pub network: String,
    pub call_type: String,
    #[serde(default)]
    pub notes: Option<String>,
    #[serde(default)]
    pub source: Option<String>,
}

/// The whole talkgroup backup file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TalkgroupBackup {
    pub format: String,
    pub version: u32,
    pub exported_at: String,
    pub count: usize,
    pub talkgroups: Vec<BackupTalkgroupRow>,
}

/// Write every talkgroup in the library to a Codeplug Magic talkgroup backup JSON file.
/// Returns how many were written.
#[tauri::command]
pub async fn export_talkgroups(state: State<'_, AppState>, path: String) -> Result<usize, String> {
    let talkgroups = sqlx::query_as::<_, BackupTalkgroupRow>(
        "SELECT tg_number, name, network, call_type, notes, source
         FROM talkgroups ORDER BY network, tg_number",
    )
    .fetch_all(&state.pool)
    .await
    .estr()?;

    let backup = TalkgroupBackup {
        format: TALKGROUP_BACKUP_FORMAT.to_string(),
        version: TALKGROUP_BACKUP_VERSION,
        exported_at: now_iso8601(),
        count: talkgroups.len(),
        talkgroups,
    };

    let json = serde_json::to_string_pretty(&backup)
        .map_err(|e| format!("Could not serialize talkgroups: {e}"))?;
    std::fs::write(&path, json).map_err(|e| format!("Could not write file: {e}"))?;

    Ok(backup.count)
}

/// Read and validate a talkgroup backup file, rejecting anything not one of ours.
fn read_talkgroup_backup(path: &str) -> Result<TalkgroupBackup, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("Could not open file: {e}"))?;
    let backup: TalkgroupBackup = serde_json::from_str(&text)
        .map_err(|e| format!("This file is not a Codeplug Magic talkgroup backup: {e}"))?;
    if backup.format != TALKGROUP_BACKUP_FORMAT && backup.format != LEGACY_TALKGROUP_BACKUP_FORMAT {
        return Err("This file is not a Codeplug Magic talkgroup backup.".to_string());
    }
    Ok(backup)
}

/// Cheap probe used by the import dialog to route a `.json` file to the talkgroup
/// backup importer. Never errors on a parse mismatch.
#[tauri::command]
pub async fn is_talkgroup_backup(path: String) -> Result<bool, String> {
    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(_) => return Ok(false),
    };
    let v: serde_json::Value = match serde_json::from_str(&text) {
        Ok(v) => v,
        Err(_) => return Ok(false),
    };
    Ok(v.get("format").and_then(|f| f.as_str()) == Some(TALKGROUP_BACKUP_FORMAT))
}

#[tauri::command]
pub async fn preview_talkgroup_backup(path: String) -> Result<TalkgroupImportPreview, String> {
    let backup = read_talkgroup_backup(&path)?;
    let rows = backup
        .talkgroups
        .iter()
        .take(PREVIEW_CAP)
        .map(|t| ParsedTalkgroup {
            tg_number: t.tg_number,
            name: t.name.clone(),
            network: t.network.clone(),
            call_type: t.call_type.clone(),
            notes: t.notes.clone(),
        })
        .collect();
    Ok(TalkgroupImportPreview {
        total: backup.talkgroups.len(),
        rows,
    })
}

/// Restore a talkgroup backup. Talkgroups that already exist on the same network
/// are skipped (never clobbered), so a restore into a non-empty library is safe;
/// after a full flush everything comes back. `notes` and the original `source`
/// are preserved (falling back to `import` for older files without a source).
#[tauri::command]
pub async fn import_talkgroup_backup(
    state: State<'_, AppState>,
    path: String,
) -> Result<ImportSummary, String> {
    let backup = read_talkgroup_backup(&path)?;

    let mut conn = state.pool.acquire().await.estr()?;
    let mut tx = conn.begin().await.estr()?;

    let mut added = 0usize;
    let mut skipped = 0usize;
    for tg in &backup.talkgroups {
        let source = tg.source.as_deref().unwrap_or("import");
        let res = sqlx::query(
            "INSERT INTO talkgroups (tg_number, name, network, call_type, notes, source)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(network, tg_number) DO NOTHING",
        )
        .bind(tg.tg_number)
        .bind(&tg.name)
        .bind(&tg.network)
        .bind(&tg.call_type)
        .bind(&tg.notes)
        .bind(source)
        .execute(&mut *tx)
        .await
        .estr()?;
        if res.rows_affected() > 0 {
            added += 1;
        } else {
            skipped += 1;
        }
    }

    tx.commit().await.estr()?;
    Ok(ImportSummary { added, skipped, updated: 0 })
}

#[cfg(test)]
mod tests {

    /// 9990 (the echo/parrot test) must end up in the library whatever the
    /// service does with it — it is absent from the live endpoint today and was
    /// named "None" in the snapshot that used to ship.
    #[test]
    fn the_parrot_test_survives_whatever_brandmeister_serves() {
        for served in [None, Some("None"), Some("Parrot")] {
            let mut list: HashMap<String, String> = HashMap::new();
            list.insert("91".into(), "World-wide".into());
            if let Some(name) = served {
                list.insert("9990".into(), name.into());
            }
            // Same line `refresh_brandmeister_talkgroups` applies before the upsert.
            list.insert("9990".to_string(), "Parrot (Echo Test)".to_string());
            assert_eq!(list["9990"], "Parrot (Echo Test)", "served as {served:?}");
            assert_eq!(list["91"], "World-wide", "nothing else is touched");
        }
    }

    /// The exact shape `api.brandmeister.network/v2/talkgroup/` returns —
    /// checked against a live response while this was written: a flat
    /// `{"<number>": "<name>"}` map, 1,833 entries, with non-ASCII names.
    #[test]
    fn the_brandmeister_response_shape_parses() {
        let body = br#"{"1":"Local","91":"World-wide","202":"\u0394\u03b9\u03b5\u03b8\u03bd\u03ad\u03c2","9990":"None"}"#;
        let list = parse_brandmeister_list(body).expect("parses");
        assert_eq!(list.len(), 4);
        assert_eq!(list["91"], "World-wide");
        assert_eq!(list["202"], "Διεθνές", "non-ASCII names survive");
        // 9990 comes back as "None"; the refresh renames it, not the parser.
        assert_eq!(list["9990"], "None");
    }

    #[test]
    fn a_response_that_is_not_a_talkgroup_list_is_refused() {
        assert!(parse_brandmeister_list(b"<html>maintenance</html>").is_err());
        assert!(parse_brandmeister_list(b"[1,2,3]").is_err());
        // An empty map is a service problem, not a library to import.
        let err = parse_brandmeister_list(b"{}").unwrap_err();
        assert!(err.contains("empty"), "{err}");
    }
    use super::*;

    #[test]
    fn parses_sample_talkgroup_csv() {
        let parsed = parse_talkgroup_csv("../sample-data/talkgroups-sample.csv", "Brandmeister")
            .expect("parse failed");
        // 5 valid rows; the missing-number and non-numeric rows are skipped.
        assert_eq!(parsed.len(), 5);
        assert_eq!(parsed[0].tg_number, 3108);
        assert_eq!(parsed[0].name, "Colorado");
        assert_eq!(parsed[0].network, "Brandmeister");
        assert_eq!(parsed[0].call_type, "Group");
        assert_eq!(parsed[0].notes.as_deref(), Some("Statewide CO"));

        let parrot = parsed.iter().find(|t| t.tg_number == 9990).unwrap();
        assert_eq!(parrot.call_type, "Private");

        // The "TAC 311" row has an empty Notes cell -> None.
        let tac = parsed.iter().find(|t| t.tg_number == 311).unwrap();
        assert_eq!(tac.notes, None);
    }
}
