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

/// Highest applied migration version recorded in `_sqlx_migrations`.
async fn migration_version<'e, E>(conn: E) -> i64
where
    E: sqlx::Executor<'e, Database = sqlx::Sqlite>,
{
    sqlx::query_scalar("SELECT COALESCE(MAX(version), 0) FROM _sqlx_migrations WHERE success = 1")
        .fetch_one(conn)
        .await
        .unwrap_or(0)
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
    Some(version)
}

/// True if `path` is a restorable WW8L Codeplug Magic master database.
#[tauri::command]
pub async fn is_database_backup(path: String) -> Result<bool, String> {
    Ok(inspect_backup(&path).await.is_some())
}

/// Export the ENTIRE database to `path` as a single consistent SQLite file.
#[tauri::command]
pub async fn export_database(state: State<'_, AppState>, path: String) -> Result<String, String> {
    // VACUUM INTO refuses to overwrite; the save dialog may hand us an existing
    // path the user chose to replace, so clear it first.
    if Path::new(&path).exists() {
        std::fs::remove_file(&path).map_err(|e| format!("Could not overwrite {path}: {e}"))?;
    }
    // VACUUM INTO takes a string literal, not a bind parameter; escape quotes.
    let escaped = path.replace('\'', "''");
    sqlx::query(&format!("VACUUM INTO '{escaped}'"))
        .execute(&state.pool)
        .await
        .estr()?;
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

/// Replace the contents of every user table present in both databases from the
/// attached `restore_src`, all inside one transaction. Rolls back on any error.
async fn replace_all_tables(conn: &mut SqliteConnection) -> Result<(), String> {
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
    for table in &tables {
        if let Err(e) = replace_one_table(conn, table).await {
            let _ = sqlx::query("ROLLBACK").execute(&mut *conn).await;
            return Err(e);
        }
    }
    sqlx::query("COMMIT").execute(&mut *conn).await.estr()?;
    Ok(())
}

/// Clear one table and refill it from the backup, matching columns by name.
async fn replace_one_table(conn: &mut SqliteConnection, table: &str) -> Result<(), String> {
    let cols = shared_columns(conn, table).await?;
    if cols.is_empty() {
        return Ok(());
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
#[tauri::command]
pub async fn import_database(state: State<'_, AppState>, path: String) -> Result<(), String> {
    let backup_version = inspect_backup(&path)
        .await
        .ok_or("That file is not a WW8L Codeplug Magic database backup.")?;

    // A backup newer than this build's schema could carry columns/tables we
    // can't map. Reject it rather than restore a partial, confusing result.
    let current = migration_version(&state.pool).await;
    if backup_version > current {
        return Err("This backup was made by a newer version of WW8L Codeplug Magic. \
             Update the app before restoring it."
            .into());
    }

    // One dedicated connection: foreign_keys must toggle outside a transaction,
    // and ATTACH cannot run inside one.
    let mut conn = state.pool.acquire().await.estr()?;
    let escaped = path.replace('\'', "''");
    sqlx::query("PRAGMA foreign_keys = OFF")
        .execute(&mut *conn)
        .await
        .estr()?;
    sqlx::query(&format!("ATTACH DATABASE '{escaped}' AS restore_src"))
        .execute(&mut *conn)
        .await
        .estr()?;

    let result = replace_all_tables(&mut conn).await;

    // Always detach and restore FK enforcement before the connection returns to
    // the pool, even if the restore failed.
    let _ = sqlx::query("DETACH DATABASE restore_src")
        .execute(&mut *conn)
        .await;
    let _ = sqlx::query("PRAGMA foreign_keys = ON")
        .execute(&mut *conn)
        .await;

    result
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
        replace_all_tables(&mut conn).await.expect("restore");
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
}
