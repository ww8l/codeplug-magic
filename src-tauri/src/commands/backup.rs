//! Whole-database master backup & restore.
//!
//! Unlike the per-radio and per-channel exporters, this captures 100% of the
//! app's data: every table, every row, in one file. It deliberately copies the
//! SQLite database itself rather than serialising tables to JSON, so it stays
//! complete and zero-maintenance as the schema grows — new tables and columns
//! are included automatically with no code change here.
//!
//! Export uses SQLite's online `VACUUM INTO`, producing a clean, consistent
//! point-in-time snapshot even while the app is running (no torn WAL). Restore
//! happens IN PLACE on the live connection: the backup is attached and every
//! shared table's contents are replaced inside one transaction — no file swap
//! and no process restart (which is unreliable under `tauri dev`). The frontend
//! reloads the webview afterwards so all views re-query the restored data.

use std::collections::HashSet;
use std::path::Path;
use std::str::FromStr;

use sqlx::sqlite::SqliteConnectOptions;
use sqlx::{Connection, SqliteConnection};
use tauri::State;

use crate::db::AppState;
use crate::error::MapErrString;

/// Highest applied migration version recorded in `_sqlx_migrations`, or `None`
/// if the ledger cannot be read or records nothing.
///
/// It used to answer `0` for both, which let a file with an unreadable or empty
/// ledger through the compatibility gate as "older than us, safe to restore"
/// — the one case where the gate most needs to say no (#74).
async fn migration_version(conn: &mut SqliteConnection) -> Option<i64> {
    sqlx::query_scalar::<_, i64>(
        "SELECT COALESCE(MAX(version), 0) FROM _sqlx_migrations WHERE success = 1",
    )
    .fetch_one(conn)
    .await
    .ok()
    .filter(|v| *v > 0)
}

/// [`migration_version`] for the live pool. Goes through an owned connection
/// because the generic executor form does not resolve behind a borrowed pool in
/// an async command.
async fn pool_migration_version(pool: &sqlx::SqlitePool) -> Option<i64> {
    sqlx::query_scalar::<_, i64>(
        "SELECT COALESCE(MAX(version), 0) FROM _sqlx_migrations WHERE success = 1",
    )
    .fetch_one(pool)
    .await
    .ok()
    .filter(|v| *v > 0)
}

/// Open a candidate file read-only and confirm it is a WW8L Codeplug Magic
/// database. Returns its highest applied migration version so callers can gate
/// on schema compatibility; `None` means "not one of our databases".
async fn inspect_backup(path: &str) -> Option<i64> {
    if !Path::new(path).is_file() {
        return None;
    }
    let opts = SqliteConnectOptions::from_str(&format!("sqlite://{path}"))
        .ok()?
        .read_only(true)
        .create_if_missing(false);
    let mut conn = SqliteConnection::connect_with(&opts).await.ok()?;

    // Must carry our schema: two signature tables plus the migration ledger.
    let hits: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM sqlite_master
         WHERE type = 'table' AND name IN ('codeplugs', 'channels', '_sqlx_migrations')",
    )
    .fetch_one(&mut conn)
    .await
    .unwrap_or(0);
    if hits < 3 {
        let _ = conn.close().await;
        return None;
    }

    let version = migration_version(&mut conn).await;
    let _ = conn.close().await;
    version
}

/// True if `path` is a restorable WW8L Codeplug Magic master database.
#[tauri::command]
pub async fn is_database_backup(path: String) -> Result<bool, String> {
    Ok(inspect_backup(&path).await.is_some())
}

/// True if `candidate` names the live database file or one of its sidecars.
/// Compared canonically where possible so `./db.sqlite3` and an absolute path
/// to the same file both match.
fn is_live_database(live: &Path, candidate: &Path) -> bool {
    let canon = |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    let target = canon(candidate);
    let base = live.to_string_lossy();
    [base.to_string(), format!("{base}-wal"), format!("{base}-shm")]
        .iter()
        .any(|p| canon(Path::new(p)) == target)
}

/// Where the vacuum writes before it is renamed into place: the target's own
/// directory, so the rename is atomic and cannot cross a filesystem boundary.
/// The pid keeps two exports from colliding.
fn temp_sibling(target: &Path) -> std::path::PathBuf {
    target.with_file_name(format!(
        "{}.{}.tmp",
        target
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("codeplug-backup"),
        std::process::id()
    ))
}

/// Export the ENTIRE database to `path` as a single consistent SQLite file.
///
/// The vacuum goes to a sibling temp file that is renamed over the target only
/// once it has succeeded. Writing straight to `path` meant deleting the user's
/// existing backup first (VACUUM INTO refuses to overwrite) — a full disk or
/// any other vacuum failure then left them with neither the old file nor a new
/// one, and SQLite removes its own partial output.
#[tauri::command]
pub async fn export_database(state: State<'_, AppState>, path: String) -> Result<String, String> {
    export_impl(&state.pool, path).await
}

async fn export_impl(pool: &sqlx::SqlitePool, path: String) -> Result<String, String> {
    let target = Path::new(&path);
    let live = pool.connect_options().get_filename().to_path_buf();
    if is_live_database(&live, target) {
        return Err(
            "That is the database this app is running from. Choose a different file.".to_string(),
        );
    }

    let tmp = temp_sibling(target);
    if tmp.exists() {
        let _ = std::fs::remove_file(&tmp);
    }

    // VACUUM INTO takes a string literal, not a bind parameter; escape quotes.
    let escaped = tmp.to_string_lossy().replace('\'', "''");
    let vacuumed = sqlx::query(&format!("VACUUM INTO '{escaped}'"))
        .execute(pool)
        .await;
    if let Err(e) = vacuumed {
        let _ = std::fs::remove_file(&tmp);
        return Err(format!("Could not write the backup: {e}"));
    }

    // Only now is the old backup at risk, and only for as long as a rename takes.
    if let Err(e) = std::fs::rename(&tmp, target) {
        let _ = std::fs::remove_file(&tmp);
        return Err(format!("Could not save to {path}: {e}"));
    }
    Ok(path)
}

/// Column names a table has in BOTH databases, in the live table's order. Any
/// column missing from the backup (e.g. added by a later migration) is skipped
/// so an older backup still restores cleanly; the new columns keep their schema
/// defaults.
async fn shared_columns(conn: &mut SqliteConnection, table: &str) -> Result<Vec<String>, String> {
    let t = table.replace('\'', "''");
    let main_cols: Vec<String> =
        sqlx::query_scalar(&format!("SELECT name FROM pragma_table_info('{t}')"))
            .fetch_all(&mut *conn)
            .await
            .estr()?;
    let src_cols: Vec<String> =
        sqlx::query_scalar(&format!("SELECT name FROM pragma_table_info('{t}', 'restore_src')"))
            .fetch_all(&mut *conn)
            .await
            .estr()?;
    let src: HashSet<String> = src_cols.into_iter().collect();
    Ok(main_cols.into_iter().filter(|c| src.contains(c)).collect())
}

/// Migrations that transform DATA rather than schema, by version.
///
/// A restore replaces table CONTENTS while the live schema stays at the current
/// version, so the rows that arrive are the backup's — carrying whatever
/// meaning they had at the backup's migration version. The ledger is
/// deliberately not restored, so these would never run again and the data would
/// keep the old meaning forever (#69):
///
///   * `0008` holds AT-D890UV DCS codes as the radio's raw decimal instead of
///     octal — **a wrong DCS code programmed to a radio**, invisible in the UI.
///   * `0011` leaves RepeaterBook rows on the 3-segment dedupe key, so the next
///     import matches none of them and re-inserts every repeater as a duplicate.
///   * `0005`/`0015` bring back radio models that were removed.
///
/// Each is re-run after a restore only when it is NEWER than the backup, which
/// is exactly the condition under which the backup's rows have not seen it.
/// The files themselves are used — they are immutable once applied, so this
/// cannot drift from what a fresh database gets.
///
/// `pure_data_migrations_are_all_registered` keeps this list complete.
const DATA_FIXUPS: &[(i64, &str)] = &[
    (3, include_str!("../../migrations/0003_anytone_export.sql")),
    (4, include_str!("../../migrations/0004_fix_thd74_dmr.sql")),
    (5, include_str!("../../migrations/0005_trim_radio_models.sql")),
    (8, include_str!("../../migrations/0008_dcs_octal_codes.sql")),
    (11, include_str!("../../migrations/0011_repeaterbook_id_city.sql")),
    (13, include_str!("../../migrations/0013_scan_list_revert_last_called.sql")),
    (15, include_str!("../../migrations/0015_remove_vero_vrn76.sql")),
];

/// Replace the contents of every user table present in both databases from the
/// attached `restore_src`, all inside one transaction. Rolls back on any error.
///
/// `backup_version` is the migration version the backup's ROWS were written at;
/// every data fixup newer than it is replayed before the commit, so a failure
/// there takes the whole restore back with it.
async fn replace_all_tables(
    conn: &mut SqliteConnection,
    backup_version: i64,
) -> Result<(), String> {
    let tables: Vec<String> = sqlx::query_scalar(
        "SELECT m.name FROM main.sqlite_master m
         JOIN restore_src.sqlite_master s ON s.name = m.name AND s.type = 'table'
         WHERE m.type = 'table'
           AND m.name NOT LIKE 'sqlite_%'
           AND m.name <> '_sqlx_migrations'
         ORDER BY m.name",
    )
    .fetch_all(&mut *conn)
    .await
    .estr()?;

    sqlx::query("BEGIN IMMEDIATE")
        .execute(&mut *conn)
        .await
        .estr()?;

    // Everything from here to the COMMIT must leave via `rollback`, or the
    // connection goes back to the pool inside an open transaction with foreign
    // keys still off, for the next command to inherit (#74).
    async fn rollback(conn: &mut SqliteConnection, e: String) -> Result<(), String> {
        let _ = sqlx::query("ROLLBACK").execute(&mut *conn).await;
        Err(e)
    }

    for table in &tables {
        if let Err(e) = replace_one_table(conn, table).await {
            return rollback(conn, e).await;
        }
    }

    for (version, sql) in DATA_FIXUPS.iter().filter(|(v, _)| *v > backup_version) {
        if let Err(e) = run_script(conn, sql).await {
            return rollback(
                conn,
                format!("Could not bring the restored data up to date (migration {version:04}): {e}"),
            )
            .await;
        }
    }

    // Foreign keys were off for the restore, so nothing checked the backup's
    // references on the way in. A backup with dangling rows used to restore
    // "successfully" and be discovered later by whichever query tripped over it.
    // Columns are (table, rowid, parent, fkid); rowid is NULL on a WITHOUT ROWID
    // table, so it is decoded as optional.
    let dangling: Vec<(String, Option<i64>, String, i64)> =
        match sqlx::query_as("PRAGMA main.foreign_key_check").fetch_all(&mut *conn).await {
            Ok(rows) => rows,
            Err(e) => return rollback(conn, format!("Could not check the restored data: {e}")).await,
        };
    if let Some((table, _rowid, parent, _fkid)) = dangling.first() {
        return rollback(
            conn,
            format!(
                "This backup has {} row(s) that point at records it does not contain                  (first: {table} → {parent}). Nothing was changed.",
                dangling.len()
            ),
        )
        .await;
    }

    if let Err(e) = sqlx::query("COMMIT").execute(&mut *conn).await {
        return rollback(conn, format!("Could not finish the restore: {e}")).await;
    }
    Ok(())
}

/// Execute a multi-statement SQL script one statement at a time.
///
/// `Query` runs exactly one statement, and `raw_sql` — which runs several —
/// does not satisfy the `Send` bound a Tauri command's future needs.
///
/// Line comments are stripped BEFORE splitting on `;`, because the migration
/// files have prose comments containing semicolons and splitting first cuts one
/// in half. Neither the statement separator nor `--` is tracked through string
/// literals; the migrations this runs are fixed, comment-heavy, and contain no
/// literal carrying either.
async fn run_script(conn: &mut SqliteConnection, sql: &str) -> Result<(), sqlx::Error> {
    use sqlx::Executor;
    let code: String = sql
        .lines()
        .map(|l| match l.find("--") {
            Some(i) => &l[..i],
            None => l,
        })
        .collect::<Vec<_>>()
        .join("\n");
    for stmt in code.split(';') {
        let trimmed = stmt.trim();
        if trimmed.is_empty() {
            continue;
        }
        conn.execute(trimmed).await?;
    }
    Ok(())
}

/// Clear one table and refill it from the backup, matching columns by name.
async fn replace_one_table(conn: &mut SqliteConnection, table: &str) -> Result<(), String> {
    let cols = shared_columns(conn, table).await?;
    if cols.is_empty() {
        // Skipping left this table holding its OLD rows while every other table
        // was replaced — a silent half-restore. It means the backup's table of
        // the same name shares no column with ours, which is not something to
        // paper over (#74).
        return Err(format!(
            "The backup's \"{table}\" table has no column in common with this version's. \
             Nothing was changed."
        ));
    }
    let list = cols
        .iter()
        .map(|c| format!("\"{}\"", c.replace('"', "\"\"")))
        .collect::<Vec<_>>()
        .join(", ");
    let qt = format!("\"{}\"", table.replace('"', "\"\""));
    sqlx::query(&format!("DELETE FROM main.{qt}"))
        .execute(&mut *conn)
        .await
        .estr()?;
    sqlx::query(&format!(
        "INSERT INTO main.{qt} ({list}) SELECT {list} FROM restore_src.{qt}"
    ))
    .execute(&mut *conn)
    .await
    .estr()?;
    Ok(())
}

/// Restore the whole database from a backup file, in place. The frontend should
/// reload the webview on success so every view re-queries the restored data.
///
/// Returns the path of the snapshot taken before anything was touched.
#[tauri::command]
pub async fn import_database(state: State<'_, AppState>, path: String) -> Result<String, String> {
    import_impl(&state.pool, path).await
}

/// One table's contribution to a master backup, for the inventory shown before
/// the file is written (#81).
#[derive(serde::Serialize)]
pub struct BackupTable {
    /// The SQLite table name. Also the fallback label, so a table added by a
    /// later migration still appears in the inventory with no code change here
    /// — the same bargain the exporter itself makes.
    pub table: String,
    /// What the user calls it.
    pub label: String,
    pub rows: i64,
    /// Why this line deserves a second look before the file leaves the machine.
    /// `None` for the user's own data, which is what anyone expects a backup to
    /// hold; `Some` for the parts nobody would predict from "back up my
    /// codeplugs".
    pub caution: Option<String>,
}

/// Human labels and cautions for the tables we know about. Anything not listed
/// falls back to its table name with no caution — an unrecognised table is
/// still disclosed, just not described.
const TABLE_FACTS: &[(&str, &str, Option<&str>)] = &[
    ("channels", "Channels", None),
    ("channel_lists", "Channel lists", None),
    ("channel_list_entries", "Channel list members", None),
    ("codeplugs", "Codeplugs", None),
    ("codeplug_channel_lists", "Codeplug channel lists", None),
    ("codeplug_scan_lists", "Codeplug scan lists", None),
    ("codeplug_channel_scan_lists", "Codeplug scan-list members", None),
    ("scan_lists", "Scan lists", None),
    ("scan_list_entries", "Scan list members", None),
    ("talkgroups", "Talkgroups", None),
    ("repeater_talkgroups", "Repeater talkgroups", None),
    ("radio_models", "Radio models", None),
    (
        "radio_profiles",
        "Radio profiles",
        Some("your call sign, DMR ID and any APRS beacon position, exactly as programmed into each radio"),
    ),
    (
        "dmr_users",
        "DMR contacts",
        Some("names, call signs and cities of third-party operators, imported from radioid.net — other people's data, not yours"),
    ),
];

/// Everything a master backup would contain, table by table, so the user can
/// see it before the file exists rather than after they have mailed it to
/// someone (#81).
///
/// It counts the LIVE tables rather than a hardcoded list, for the same reason
/// the exporter copies the database file rather than serialising known tables:
/// a table added by a later migration lands in the backup automatically, and
/// must therefore land in this inventory automatically too.
#[tauri::command]
pub async fn backup_contents(state: State<'_, AppState>) -> Result<Vec<BackupTable>, String> {
    contents_impl(&state.pool).await
}

async fn contents_impl(pool: &sqlx::SqlitePool) -> Result<Vec<BackupTable>, String> {
    let tables: Vec<String> = sqlx::query_scalar(
        "SELECT name FROM sqlite_master
         WHERE type = 'table' AND name NOT LIKE 'sqlite_%' AND name <> '_sqlx_migrations'
         ORDER BY name",
    )
    .fetch_all(pool)
    .await
    .estr()?;

    let mut out = Vec::with_capacity(tables.len());
    for table in tables {
        let quoted = table.replace('"', "\"\"");
        let rows: i64 = sqlx::query_scalar(&format!("SELECT COUNT(*) FROM \"{quoted}\""))
            .fetch_one(pool)
            .await
            .estr()?;
        let facts = TABLE_FACTS.iter().find(|(t, _, _)| *t == table);
        out.push(BackupTable {
            label: facts.map(|(_, l, _)| l.to_string()).unwrap_or_else(|| table.clone()),
            caution: facts.and_then(|(_, _, c)| *c).map(str::to_string),
            table,
            rows,
        });
    }
    Ok(out)
}

/// Where the pre-restore snapshot lives: beside the live database, one file,
/// overwritten by each restore. Bounded on purpose — it is a way back from
/// restoring the wrong file, not a backup history.
fn snapshot_path(live: &Path) -> std::path::PathBuf {
    let name = live
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("codeplug_manager.sqlite3");
    live.with_file_name(format!("{name}.before-restore"))
}

async fn import_impl(pool: &sqlx::SqlitePool, path: String) -> Result<String, String> {
    let backup_version = inspect_backup(&path)
        .await
        .ok_or("That file is not a WW8L Codeplug Magic database backup.")?;

    // A backup newer than this build's schema could carry columns/tables we
    // can't map. Reject it rather than restore a partial, confusing result.
    let current = pool_migration_version(pool)
        .await
        .ok_or("This app's own database has no migration record — restoring could not be checked for compatibility.")?;
    if backup_version > current {
        return Err("This backup was made by a newer version of WW8L Codeplug Magic. \
             Update the app before restoring it."
            .into());
    }

    // Take a snapshot BEFORE touching anything. A restore replaces every row in
    // the database; without this, picking the wrong file in the dialog — or a
    // restore that fails partway and rolls back to data the user had already
    // decided to discard — has no way back (#74).
    let live = pool.connect_options().get_filename().to_path_buf();
    let snapshot = snapshot_path(&live);
    let snapshot_str = snapshot.to_string_lossy().to_string();
    export_impl(pool, snapshot_str.clone())
        .await
        .map_err(|e| format!("Could not snapshot the current database before restoring: {e}"))?;

    // One dedicated connection: foreign_keys must toggle outside a transaction,
    // and ATTACH cannot run inside one.
    let mut conn = pool.acquire().await.estr()?;
    let escaped = path.replace('\'', "''");
    sqlx::query("PRAGMA foreign_keys = OFF")
        .execute(&mut *conn)
        .await
        .estr()?;
    sqlx::query(&format!("ATTACH DATABASE '{escaped}' AS restore_src"))
        .execute(&mut *conn)
        .await
        .estr()?;

    let result = replace_all_tables(&mut conn, backup_version).await;

    // Always detach and restore FK enforcement before the connection returns to
    // the pool, even if the restore failed.
    let _ = sqlx::query("DETACH DATABASE restore_src")
        .execute(&mut *conn)
        .await;
    let _ = sqlx::query("PRAGMA foreign_keys = ON")
        .execute(&mut *conn)
        .await;
    drop(conn);

    result.map_err(|e| format!("{e} Your data before the restore is saved at {snapshot_str}."))?;

    // The backup's radio catalogue and talkgroup library replaced ours, so a
    // model added since the backup is gone until the next launch re-seeds it —
    // and there is no launch here, only a webview reload. Seeding is an
    // idempotent upsert, so running it now is the same thing, sooner.
    crate::seed::seed_radio_models(pool)
        .await
        .map_err(|e| format!("Restored, but the radio library could not be refreshed: {e}"))?;
    crate::seed::seed_talkgroups(pool)
        .await
        .map_err(|e| format!("Restored, but the talkgroup library could not be refreshed: {e}"))?;

    Ok(snapshot_str)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The whole export → mutate → restore round-trip at the SQL level: a
    /// `VACUUM INTO` snapshot is captured, the live DB is then mutated, and an
    /// in-place restore must reproduce the snapshot exactly (mutations gone,
    /// snapshot rows back) — the mechanism that replaced the flaky app-restart.
    #[tokio::test]
    async fn restore_reproduces_snapshot_in_place() {
        let dir = std::env::temp_dir().join(format!("cpm_backup_{}", std::process::id()));
        let db_path = dir.join("live.sqlite3");
        let backup_path = dir.join("snap.sqlite3");
        let _ = std::fs::remove_file(&db_path);
        let _ = std::fs::remove_file(&backup_path);

        let pool = crate::db::init_pool(&db_path).await.expect("init_pool");

        // State at snapshot time: one distinctive list.
        sqlx::query("INSERT INTO channel_lists (name) VALUES ('BEFORE')")
            .execute(&pool)
            .await
            .unwrap();

        // Snapshot via the same online backup the export command uses.
        let escaped = backup_path.to_string_lossy().replace('\'', "''");
        sqlx::query(&format!("VACUUM INTO '{escaped}'"))
            .execute(&pool)
            .await
            .unwrap();

        // Mutate AFTER the snapshot: drop the old list, add a new one.
        sqlx::query("DELETE FROM channel_lists WHERE name = 'BEFORE'")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO channel_lists (name) VALUES ('AFTER')")
            .execute(&pool)
            .await
            .unwrap();

        // The backup file is recognised as one of ours.
        let ver = inspect_backup(&backup_path.to_string_lossy())
            .await
            .expect("backup recognised");
        assert!(ver > 0, "migration version should be recorded");

        // Restore in place, exactly as import_database does.
        let mut conn = pool.acquire().await.unwrap();
        sqlx::query("PRAGMA foreign_keys = OFF")
            .execute(&mut *conn)
            .await
            .unwrap();
        sqlx::query(&format!("ATTACH DATABASE '{escaped}' AS restore_src"))
            .execute(&mut *conn)
            .await
            .unwrap();
        replace_all_tables(&mut conn, ver).await.expect("restore");
        sqlx::query("DETACH DATABASE restore_src")
            .execute(&mut *conn)
            .await
            .unwrap();
        drop(conn);

        // Post-restore the DB matches the snapshot: BEFORE is back, AFTER is gone.
        let names: Vec<String> = sqlx::query_scalar("SELECT name FROM channel_lists")
            .fetch_all(&pool)
            .await
            .unwrap();
        assert_eq!(names, vec!["BEFORE".to_string()]);

        pool.close().await;
        let _ = std::fs::remove_file(&db_path);
        let _ = std::fs::remove_file(&backup_path);
    }

    /// Build a live DB, snapshot it, mutate it, and restore through the real
    /// command path. Returns (pool, live path, backup path, dir).
    async fn live_and_backup(
        tag: &str,
    ) -> (sqlx::SqlitePool, std::path::PathBuf, std::path::PathBuf, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!("cpm_{tag}_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let db_path = dir.join("live.sqlite3");
        let backup_path = dir.join("snap.sqlite3");
        let _ = std::fs::remove_file(&db_path);
        let _ = std::fs::remove_file(&backup_path);
        let pool = crate::db::init_pool(&db_path).await.expect("init_pool");
        (pool, db_path, backup_path, dir)
    }

    /// Issue #74: a restore must leave a way back. It replaces every row in the
    /// database, so picking the wrong file in the dialog was unrecoverable.
    #[tokio::test]
    async fn a_restore_snapshots_the_current_database_first() {
        let (pool, db_path, backup_path, dir) = live_and_backup("restore_snap").await;

        sqlx::query("INSERT INTO channel_lists (name) VALUES ('IN THE BACKUP')")
            .execute(&pool)
            .await
            .unwrap();
        export_impl(&pool, backup_path.to_string_lossy().to_string())
            .await
            .expect("backup");

        // Data that exists only now — what a careless restore would destroy.
        sqlx::query("DELETE FROM channel_lists")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO channel_lists (name) VALUES ('ONLY LIVE')")
            .execute(&pool)
            .await
            .unwrap();

        let snapshot = import_impl(&pool, backup_path.to_string_lossy().to_string())
            .await
            .expect("restore");

        // The restore did what it says.
        let names: Vec<String> = sqlx::query_scalar("SELECT name FROM channel_lists")
            .fetch_all(&pool)
            .await
            .unwrap();
        assert_eq!(names, vec!["IN THE BACKUP".to_string()]);

        // And the snapshot still holds what was there a moment ago.
        assert_eq!(snapshot, snapshot_path(&db_path).to_string_lossy());
        assert!(inspect_backup(&snapshot).await.is_some(), "snapshot is a real backup");
        let opts = SqliteConnectOptions::from_str(&format!("sqlite://{snapshot}"))
            .unwrap()
            .read_only(true);
        let mut snap = SqliteConnection::connect_with(&opts).await.unwrap();
        let saved: Vec<String> = sqlx::query_scalar("SELECT name FROM channel_lists")
            .fetch_all(&mut snap)
            .await
            .unwrap();
        assert_eq!(saved, vec!["ONLY LIVE".to_string()], "the snapshot must hold the pre-restore data");
        snap.close().await.unwrap();

        pool.close().await;
        std::fs::remove_dir_all(&dir).ok();
    }

    /// Issue #74: foreign keys are off for the restore, so nothing checks the
    /// backup's references on the way in. A backup with dangling rows used to
    /// restore "successfully" and be found later by whichever query tripped
    /// over it — now it is refused and NOTHING is changed.
    #[tokio::test]
    async fn a_backup_with_dangling_references_is_refused_whole() {
        let (pool, _db_path, backup_path, dir) = live_and_backup("restore_fk").await;

        sqlx::query("INSERT INTO channel_lists (id, name) VALUES (1, 'LIST')")
            .execute(&pool)
            .await
            .unwrap();
        export_impl(&pool, backup_path.to_string_lossy().to_string())
            .await
            .expect("backup");

        // Corrupt the BACKUP: an entry pointing at a channel that isn't there.
        let opts = SqliteConnectOptions::from_str(&format!(
            "sqlite://{}",
            backup_path.to_string_lossy()
        ))
        .unwrap();
        let mut b = SqliteConnection::connect_with(&opts).await.unwrap();
        sqlx::query("PRAGMA foreign_keys = OFF").execute(&mut b).await.unwrap();
        sqlx::query(
            "INSERT INTO channel_list_entries (channel_list_id, channel_id, position)
             VALUES (1, 987654, 0)",
        )
        .execute(&mut b)
        .await
        .unwrap();
        b.close().await.unwrap();

        // The live database has its own state, which must survive intact.
        sqlx::query("INSERT INTO channel_lists (id, name) VALUES (2, 'UNTOUCHED')")
            .execute(&pool)
            .await
            .unwrap();

        let err = import_impl(&pool, backup_path.to_string_lossy().to_string())
            .await
            .expect_err("a backup with dangling references must be refused");
        assert!(err.contains("does not contain"), "unexpected error: {err}");

        let names: Vec<String> =
            sqlx::query_scalar("SELECT name FROM channel_lists ORDER BY id")
                .fetch_all(&pool)
                .await
                .unwrap();
        assert_eq!(names, vec!["LIST".to_string(), "UNTOUCHED".to_string()]);

        // The connection must not have gone back to the pool inside an open
        // transaction with foreign keys off: the next write has to just work.
        sqlx::query("INSERT INTO channel_lists (id, name) VALUES (3, 'AFTER')")
            .execute(&pool)
            .await
            .expect("the pool must not be poisoned by a failed restore");
        let fk_on: i64 = sqlx::query_scalar("PRAGMA foreign_keys")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(fk_on, 1, "foreign keys must be back on");

        pool.close().await;
        std::fs::remove_dir_all(&dir).ok();
    }

    /// Issue #69: the migration ledger is deliberately not restored, so the
    /// data fixups would never run again and the restored rows would keep their
    /// old meaning forever — for 0008, a WRONG DCS CODE programmed to a radio.
    #[tokio::test]
    async fn an_older_backup_gets_the_data_fixups_it_never_saw() {
        let (pool, _db_path, backup_path, dir) = live_and_backup("restore_fixups").await;

        // A channel exactly as the pre-0008 AT-D890UV import wrote it: the
        // radio's raw binary DCS value in decimal, where the app means octal.
        sqlx::query(
            "INSERT INTO channels (id, name_long, rx_freq, mode, source, notes, dcs_code)
             VALUES (1, 'RAW DCS', 146.94, 'FM', 'anytone',
                     'Imported from AT-D890UV slot 1', '265')",
        )
        .execute(&pool)
        .await
        .unwrap();
        // And a RepeaterBook row on the pre-0011 three-segment dedupe key.
        sqlx::query(
            "INSERT INTO channels (id, name_long, callsign, rx_freq, mode, source, state, city,
                                   repeaterbook_id)
             VALUES (2, 'RB', 'N2SKY', 448.4, 'FM', 'repeaterbook', 'CO', 'Fort Collins',
                     'N2SKY|448.4000|CO')",
        )
        .execute(&pool)
        .await
        .unwrap();
        export_impl(&pool, backup_path.to_string_lossy().to_string())
            .await
            .expect("backup");

        sqlx::query("DELETE FROM channels").execute(&pool).await.unwrap();

        // Restore claiming the backup predates both fixups.
        let mut conn = pool.acquire().await.unwrap();
        sqlx::query("PRAGMA foreign_keys = OFF").execute(&mut *conn).await.unwrap();
        sqlx::query(&format!(
            "ATTACH DATABASE '{}' AS restore_src",
            backup_path.to_string_lossy()
        ))
        .execute(&mut *conn)
        .await
        .unwrap();
        replace_all_tables(&mut conn, 7).await.expect("restore");
        sqlx::query("DETACH DATABASE restore_src").execute(&mut *conn).await.unwrap();
        drop(conn);

        let dcs: String = sqlx::query_scalar("SELECT dcs_code FROM channels WHERE id = 1")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(dcs, "411", "0008 must convert the raw radio value to octal");
        let rb: String = sqlx::query_scalar("SELECT repeaterbook_id FROM channels WHERE id = 2")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(
            rb, "N2SKY|448.4000|CO|FORT COLLINS",
            "0011 must recompute the dedupe key, or the next import duplicates every repeater"
        );

        pool.close().await;
        std::fs::remove_dir_all(&dir).ok();
    }

    /// A fixup must NOT run against data that already had it: 0008 is not
    /// idempotent — re-converting "411" would give "633".
    #[tokio::test]
    async fn a_current_backup_is_left_alone() {
        let (pool, _db_path, backup_path, dir) = live_and_backup("restore_current").await;

        sqlx::query(
            "INSERT INTO channels (id, name_long, rx_freq, mode, source, notes, dcs_code)
             VALUES (1, 'OCTAL DCS', 146.94, 'FM', 'anytone',
                     'Imported from AT-D890UV slot 1', '411')",
        )
        .execute(&pool)
        .await
        .unwrap();
        export_impl(&pool, backup_path.to_string_lossy().to_string())
            .await
            .expect("backup");
        sqlx::query("DELETE FROM channels").execute(&pool).await.unwrap();

        import_impl(&pool, backup_path.to_string_lossy().to_string())
            .await
            .expect("restore");

        let dcs: String = sqlx::query_scalar("SELECT dcs_code FROM channels WHERE id = 1")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(dcs, "411", "a fixup the backup already had must not run again");

        pool.close().await;
        std::fs::remove_dir_all(&dir).ok();
    }

    /// The registry in [`DATA_FIXUPS`] is hand-maintained, so this asserts it is
    /// complete: every migration that only moves DATA has to be replayable
    /// after a restore, or it silently never runs again for that database.
    ///
    /// A migration carrying any DDL is a schema change — the live schema is
    /// already current after a restore, so replaying it would fail.
    #[test]
    fn pure_data_migrations_are_all_registered() {
        let registered: HashSet<i64> = DATA_FIXUPS.iter().map(|(v, _)| *v).collect();
        let mut seen = 0;
        for entry in std::fs::read_dir("migrations").expect("migrations dir") {
            let path = entry.unwrap().path();
            if path.extension().and_then(|e| e.to_str()) != Some("sql") {
                continue;
            }
            let name = path.file_name().unwrap().to_string_lossy().to_string();
            let version: i64 = name[..4].parse().expect("migration files start with a version");
            let body = std::fs::read_to_string(&path).unwrap();
            let code: String = body
                .lines()
                .map(str::trim)
                .filter(|l| !l.starts_with("--"))
                .collect::<Vec<_>>()
                .join("\n")
                .to_uppercase();
            let has_ddl = ["CREATE ", "ALTER ", "DROP "].iter().any(|k| code.contains(k));
            seen += 1;
            if has_ddl {
                assert!(
                    !registered.contains(&version),
                    "{name} changes the schema and must not be replayed after a restore"
                );
            } else {
                assert!(
                    registered.contains(&version),
                    "{name} only moves data, so it must be in DATA_FIXUPS — after a restore of \
                     an older backup it would otherwise never run again"
                );
            }
        }
        assert!(seen >= 17, "only found {seen} migrations — is the path right?");
    }

    /// A failed export must not cost the user the backup it was replacing.
    /// The command used to unlink the target first (VACUUM INTO refuses to
    /// overwrite), so a vacuum that then failed left them with neither file.
    #[tokio::test]
    async fn a_failed_export_leaves_the_previous_backup_intact() {
        let dir = std::env::temp_dir().join(format!("cpm_export_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let db_path = dir.join("live.sqlite3");
        let target = dir.join("codeplug-backup.sqlite3");
        let _ = std::fs::remove_file(&db_path);
        let _ = std::fs::remove_file(&target);

        let pool = crate::db::init_pool(&db_path).await.expect("init_pool");

        // A first export succeeds and lands a real, recognisable backup.
        let out = export_impl(&pool, target.to_string_lossy().to_string())
            .await
            .expect("first export");
        assert_eq!(out, target.to_string_lossy());
        assert!(inspect_backup(&out).await.is_some(), "export is one of ours");
        let previous = std::fs::read(&target).unwrap();
        assert!(!previous.is_empty());

        // Now force the vacuum to fail: occupy its scratch path with a
        // directory, which it cannot open as a file (and which the pre-clean
        // `remove_file` cannot clear either).
        let tmp = temp_sibling(&target);
        std::fs::create_dir_all(&tmp).unwrap();

        let err = export_impl(&pool, target.to_string_lossy().to_string())
            .await
            .expect_err("the vacuum must fail with its scratch path blocked");
        assert!(err.contains("Could not write the backup"), "unexpected error: {err}");

        // The whole point: the user still has the backup they were replacing.
        assert!(target.exists(), "the previous backup must survive a failed export");
        assert_eq!(
            std::fs::read(&target).unwrap(),
            previous,
            "the previous backup must be untouched, not truncated"
        );

        std::fs::remove_dir_all(&tmp).ok();
        pool.close().await;
        std::fs::remove_dir_all(&dir).ok();
    }

    /// Picking the live database in the save dialog used to unlink it out from
    /// under the open pool.
    #[tokio::test]
    async fn refuses_to_export_over_the_live_database() {
        let dir = std::env::temp_dir().join(format!("cpm_export_live_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let db_path = dir.join("live.sqlite3");
        let _ = std::fs::remove_file(&db_path);
        let pool = crate::db::init_pool(&db_path).await.expect("init_pool");

        for candidate in [
            db_path.to_string_lossy().to_string(),
            format!("{}-wal", db_path.to_string_lossy()),
            // The same file by a non-canonical path.
            dir.join(".").join("live.sqlite3").to_string_lossy().to_string(),
        ] {
            let err = export_impl(&pool, candidate.clone())
                .await
                .expect_err("must refuse the live database");
            assert!(err.contains("running from"), "unexpected error for {candidate}: {err}");
        }
        assert!(db_path.exists(), "the live database must still be there");

        pool.close().await;
        std::fs::remove_dir_all(&dir).ok();
    }

    /// The inventory shown before a backup is written must be derived from the
    /// live schema, not from a list someone has to remember to update. A table
    /// added by a future migration lands in the backup file automatically — so
    /// it has to land in the disclosure automatically too, even with nothing
    /// known about it beyond its name (#81).
    #[tokio::test]
    async fn a_table_nobody_described_is_still_disclosed() {
        let dir = std::env::temp_dir().join(format!("cpm_manifest_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let db_path = dir.join("live.sqlite3");
        let _ = std::fs::remove_file(&db_path);
        let pool = crate::db::init_pool(&db_path).await.expect("init_pool");

        sqlx::query("CREATE TABLE aprs_beacons (id INTEGER PRIMARY KEY, note TEXT)")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO aprs_beacons (note) VALUES ('home'), ('shack')")
            .execute(&pool)
            .await
            .unwrap();

        let inventory = contents_impl(&pool).await.expect("inventory");

        // Every real table is accounted for, including the one TABLE_FACTS has
        // never heard of, which falls back to its own name.
        let live: Vec<String> = sqlx::query_scalar(
            "SELECT name FROM sqlite_master
             WHERE type = 'table' AND name NOT LIKE 'sqlite_%' AND name <> '_sqlx_migrations'",
        )
        .fetch_all(&pool)
        .await
        .unwrap();
        for table in &live {
            assert!(
                inventory.iter().any(|e| &e.table == table),
                "{table} would ride along in the backup undisclosed"
            );
        }

        let stranger = inventory
            .iter()
            .find(|e| e.table == "aprs_beacons")
            .expect("the undescribed table");
        assert_eq!(stranger.label, "aprs_beacons", "unknown tables fall back to their name");
        assert_eq!(stranger.rows, 2);

        // And the part nobody predicts from "back up my codeplugs" says so.
        let dmr = inventory
            .iter()
            .find(|e| e.table == "dmr_users")
            .expect("dmr_users");
        let caution = dmr.caution.as_deref().unwrap_or("");
        assert!(
            caution.contains("radioid.net"),
            "the third-party operator dump must name where it came from: {caution:?}"
        );

        pool.close().await;
        std::fs::remove_dir_all(&dir).ok();
    }
}
