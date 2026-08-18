use std::path::Path;
use std::str::FromStr;

use sqlx::sqlite::{SqliteConnectOptions, SqlitePool, SqlitePoolOptions};

/// Shared application state held by Tauri and injected into every command.
pub struct AppState {
    pub pool: SqlitePool,
}

/// Filename of the live master database inside the app-data directory.
pub const DB_FILENAME: &str = "codeplug_manager.sqlite3";

/// Open (creating if necessary) the SQLite database at `db_path`, enabling
/// foreign keys, then run migrations and seed the built-in radio model
/// library.
pub async fn init_pool(db_path: &Path) -> Result<SqlitePool, sqlx::Error> {
    if let Some(parent) = db_path.parent() {
        std::fs::create_dir_all(parent).ok();
    }

    let url = format!("sqlite://{}", db_path.to_string_lossy());
    let options = SqliteConnectOptions::from_str(&url)?
        .create_if_missing(true)
        .foreign_keys(true)
        .busy_timeout(std::time::Duration::from_secs(10));

    let pool = SqlitePoolOptions::new()
        .max_connections(5)
        .connect_with(options)
        .await?;

    sqlx::migrate!("./migrations").run(&pool).await?;
    crate::seed::seed_radio_models(&pool).await?;
    crate::seed::seed_talkgroups(&pool).await?;

    Ok(pool)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn migrations_and_seed_run() {
        let dir = std::env::temp_dir().join(format!("cpm_test_{}", std::process::id()));
        let db_path = dir.join("test.sqlite3");
        let _ = std::fs::remove_file(&db_path);

        let pool = init_pool(&db_path).await.expect("init_pool failed");

        // Models are reintroduced one at a time (migration 0005 trimmed the
        // original set): currently the Baofeng UV-5R, TIDRADIO TD-H3, AnyTone
        // AT-D890UV, Yaesu FT5D, Icom ID-52 and Kenwood TH-D75. (0015 removed
        // the Vero VR-N76 placeholder.) None of the last three has a migration
        // of its own — seeding INSERTs new (manufacturer, model) rows, so a new
        // model reaches existing databases on the next startup without one.
        let count: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM radio_models")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(
            count.0, 6,
            "expected the UV-5R, TD-H3, AT-D890UV, FT5D, ID-52 and TH-D75 seeded models"
        );

        let models: Vec<(String,)> =
            sqlx::query_as("SELECT model FROM radio_models ORDER BY model")
                .fetch_all(&pool)
                .await
                .unwrap();
        let names: Vec<&str> = models.iter().map(|m| m.0.as_str()).collect();
        assert_eq!(
            names,
            vec!["AT-D890UV", "FT5D", "ID-52", "TD-H3", "TH-D75", "UV-5R"]
        );

        // Seeding twice must remain idempotent.
        crate::seed::seed_radio_models(&pool).await.unwrap();
        let count2: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM radio_models")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(count2.0, 6, "seeding should be idempotent");

        // The full Brandmeister talkgroup list should be seeded (~1,750 rows),
        // and re-seeding is idempotent.
        let tg_count: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM talkgroups")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert!(
            tg_count.0 > 1000,
            "expected the full Brandmeister TG list (>1000), got {}",
            tg_count.0
        );
        crate::seed::seed_talkgroups(&pool).await.unwrap();
        let tg_count2: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM talkgroups")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(
            tg_count2.0, tg_count.0,
            "talkgroup seeding should be idempotent"
        );

        // The UV-5R schema JSON (derived from the CHIRP driver) should be valid
        // and contain known CHIRP setting keys.
        let schema: (String,) = sqlx::query_as(
            "SELECT non_channel_settings_schema FROM radio_models WHERE model = 'UV-5R'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&schema.0).unwrap();
        let fields = parsed.as_array().unwrap();
        assert!(fields.iter().any(|f| f["key"] == "squelch"));
        assert!(fields.iter().any(|f| f["key"] == "poweron_msg.line1"));

        let _ = std::fs::remove_file(&db_path);
    }
}
