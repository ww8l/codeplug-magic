use sqlx::Acquire;
use tauri::State;

use crate::db::AppState;
use crate::error::MapErrString;
use crate::models::{
    ChannelListSummary, Codeplug, CodeplugInput, CodeplugSummary, ScanListSummary,
};

const CODEPLUG_SUMMARY_SQL: &str = r#"
    SELECT cp.id, cp.name, cp.radio_profile_id,
           rp.display_name AS profile_name,
           rm.display_name AS model_display_name,
           cp.last_exported, cp.last_export_kind,
           (SELECT COUNT(DISTINCT e.channel_id)
              FROM codeplug_channel_lists ccl
              JOIN channel_list_entries e ON e.channel_list_id = ccl.channel_list_id
             WHERE ccl.codeplug_id = cp.id) AS channel_count
    FROM codeplugs cp
    LEFT JOIN radio_profiles rp ON rp.id = cp.radio_profile_id
    LEFT JOIN radio_models rm ON rm.id = rp.radio_model_id
"#;

#[tauri::command]
pub async fn list_codeplugs(state: State<'_, AppState>) -> Result<Vec<CodeplugSummary>, String> {
    sqlx::query_as::<_, CodeplugSummary>(&format!(
        "{CODEPLUG_SUMMARY_SQL} ORDER BY COALESCE(cp.updated_at, cp.created_at) DESC"
    ))
    .fetch_all(&state.pool)
    .await
    .estr()
}

#[tauri::command]
pub async fn get_codeplug(state: State<'_, AppState>, id: i64) -> Result<Codeplug, String> {
    sqlx::query_as::<_, Codeplug>(
        "SELECT id, name, radio_profile_id, notes, last_exported, last_export_kind, created_at, updated_at FROM codeplugs WHERE id = ?1",
    )
    .bind(id)
    .fetch_one(&state.pool)
    .await
    .estr()
}

#[tauri::command]
pub async fn create_codeplug(
    state: State<'_, AppState>,
    input: CodeplugInput,
) -> Result<Codeplug, String> {
    let id = sqlx::query(
        "INSERT INTO codeplugs (name, radio_profile_id, notes, updated_at) VALUES (?1, ?2, ?3, CURRENT_TIMESTAMP)",
    )
    .bind(&input.name)
    .bind(input.radio_profile_id)
    .bind(&input.notes)
    .execute(&state.pool)
    .await
    .estr()?
    .last_insert_rowid();

    get_codeplug(state, id).await
}

#[tauri::command]
pub async fn update_codeplug(
    state: State<'_, AppState>,
    id: i64,
    input: CodeplugInput,
) -> Result<Codeplug, String> {
    sqlx::query(
        "UPDATE codeplugs SET name = ?2, radio_profile_id = ?3, notes = ?4, updated_at = CURRENT_TIMESTAMP WHERE id = ?1",
    )
    .bind(id)
    .bind(&input.name)
    .bind(input.radio_profile_id)
    .bind(&input.notes)
    .execute(&state.pool)
    .await
    .estr()?;

    get_codeplug(state, id).await
}

#[tauri::command]
pub async fn delete_codeplug(state: State<'_, AppState>, id: i64) -> Result<(), String> {
    sqlx::query("DELETE FROM codeplugs WHERE id = ?1")
        .bind(id)
        .execute(&state.pool)
        .await
        .estr()?;
    Ok(())
}

// ---- Channel-list assignments ----
#[tauri::command]
pub async fn get_codeplug_channel_lists(
    state: State<'_, AppState>,
    codeplug_id: i64,
) -> Result<Vec<ChannelListSummary>, String> {
    sqlx::query_as::<_, ChannelListSummary>(
        r#"
        SELECT cl.id, cl.name, cl.description,
               (SELECT COUNT(*) FROM channel_list_entries e WHERE e.channel_list_id = cl.id) AS channel_count
        FROM codeplug_channel_lists ccl
        JOIN channel_lists cl ON cl.id = ccl.channel_list_id
        WHERE ccl.codeplug_id = ?1
        ORDER BY ccl.position
        "#,
    )
    .bind(codeplug_id)
    .fetch_all(&state.pool)
    .await
    .estr()
}

#[tauri::command]
pub async fn add_channel_list_to_codeplug(
    state: State<'_, AppState>,
    codeplug_id: i64,
    channel_list_id: i64,
) -> Result<(), String> {
    let next_pos: (i64,) = sqlx::query_as(
        "SELECT COALESCE(MAX(position), -1) + 1 FROM codeplug_channel_lists WHERE codeplug_id = ?1",
    )
    .bind(codeplug_id)
    .fetch_one(&state.pool)
    .await
    .estr()?;

    sqlx::query(
        "INSERT INTO codeplug_channel_lists (codeplug_id, channel_list_id, position) VALUES (?1, ?2, ?3) ON CONFLICT(codeplug_id, channel_list_id) DO NOTHING",
    )
    .bind(codeplug_id)
    .bind(channel_list_id)
    .bind(next_pos.0)
    .execute(&state.pool)
    .await
    .estr()?;
    Ok(())
}

#[tauri::command]
pub async fn remove_channel_list_from_codeplug(
    state: State<'_, AppState>,
    codeplug_id: i64,
    channel_list_id: i64,
) -> Result<(), String> {
    sqlx::query(
        "DELETE FROM codeplug_channel_lists WHERE codeplug_id = ?1 AND channel_list_id = ?2",
    )
    .bind(codeplug_id)
    .bind(channel_list_id)
    .execute(&state.pool)
    .await
    .estr()?;
    Ok(())
}

#[tauri::command]
pub async fn reorder_codeplug_channel_lists(
    state: State<'_, AppState>,
    codeplug_id: i64,
    ordered_list_ids: Vec<i64>,
) -> Result<(), String> {
    let mut conn = state.pool.acquire().await.estr()?;
    let mut tx = conn.begin().await.estr()?;

    for (pos, list_id) in ordered_list_ids.iter().enumerate() {
        sqlx::query(
            "UPDATE codeplug_channel_lists SET position = ?3 WHERE codeplug_id = ?1 AND channel_list_id = ?2",
        )
        .bind(codeplug_id)
        .bind(list_id)
        .bind(pos as i64)
        .execute(&mut *tx)
        .await
        .estr()?;
    }

    tx.commit().await.estr()?;
    Ok(())
}

// ---- Scan-list assignments ----
#[tauri::command]
pub async fn get_codeplug_scan_lists(
    state: State<'_, AppState>,
    codeplug_id: i64,
) -> Result<Vec<ScanListSummary>, String> {
    sqlx::query_as::<_, ScanListSummary>(
        r#"
        SELECT sl.id, sl.name, sl.description,
               sl.priority_channel_id, sl.priority_channel_2_id, sl.priority_select,
               sl.look_back_a, sl.look_back_b, sl.dropout_delay, sl.dwell_time, sl.revert_channel,
               (SELECT COUNT(*) FROM scan_list_entries e WHERE e.scan_list_id = sl.id) AS channel_count
        FROM codeplug_scan_lists csl
        JOIN scan_lists sl ON sl.id = csl.scan_list_id
        WHERE csl.codeplug_id = ?1
        ORDER BY sl.name
        "#,
    )
    .bind(codeplug_id)
    .fetch_all(&state.pool)
    .await
    .estr()
}

#[tauri::command]
pub async fn add_scan_list_to_codeplug(
    state: State<'_, AppState>,
    codeplug_id: i64,
    scan_list_id: i64,
) -> Result<(), String> {
    sqlx::query(
        "INSERT INTO codeplug_scan_lists (codeplug_id, scan_list_id) VALUES (?1, ?2) ON CONFLICT(codeplug_id, scan_list_id) DO NOTHING",
    )
    .bind(codeplug_id)
    .bind(scan_list_id)
    .execute(&state.pool)
    .await
    .estr()?;
    Ok(())
}

#[tauri::command]
pub async fn remove_scan_list_from_codeplug(
    state: State<'_, AppState>,
    codeplug_id: i64,
    scan_list_id: i64,
) -> Result<(), String> {
    sqlx::query("DELETE FROM codeplug_scan_lists WHERE codeplug_id = ?1 AND scan_list_id = ?2")
        .bind(codeplug_id)
        .bind(scan_list_id)
        .execute(&state.pool)
        .await
        .estr()?;
    Ok(())
}

// ---- Per-channel scan-list assignment (radio channel byte 0x1B) ----
// Each channel points at exactly one scan list to launch when Scan is pressed,
// an explicit per-channel setting kept per codeplug. Channels with no row here
// fall back to membership-derivation when programming.
#[derive(serde::Serialize, sqlx::FromRow)]
pub struct ChannelScanListAssignment {
    pub channel_id: i64,
    pub scan_list_id: i64,
}

#[tauri::command]
pub async fn get_codeplug_channel_scan_lists(
    state: State<'_, AppState>,
    codeplug_id: i64,
) -> Result<Vec<ChannelScanListAssignment>, String> {
    sqlx::query_as::<_, ChannelScanListAssignment>(
        "SELECT channel_id, scan_list_id FROM codeplug_channel_scan_lists WHERE codeplug_id = ?1",
    )
    .bind(codeplug_id)
    .fetch_all(&state.pool)
    .await
    .estr()
}

/// Point a channel at a scan list within this codeplug (upsert — a channel
/// resolves to at most one scan list here, so re-assigning moves it).
#[tauri::command]
pub async fn set_channel_scan_list(
    state: State<'_, AppState>,
    codeplug_id: i64,
    channel_id: i64,
    scan_list_id: i64,
) -> Result<(), String> {
    sqlx::query(
        "INSERT INTO codeplug_channel_scan_lists (codeplug_id, channel_id, scan_list_id)
         VALUES (?1, ?2, ?3)
         ON CONFLICT(codeplug_id, channel_id) DO UPDATE SET scan_list_id = excluded.scan_list_id",
    )
    .bind(codeplug_id)
    .bind(channel_id)
    .bind(scan_list_id)
    .execute(&state.pool)
    .await
    .estr()?;
    Ok(())
}

/// Clear a channel's explicit scan-list assignment; it reverts to
/// membership-derivation when programming.
#[tauri::command]
pub async fn clear_channel_scan_list(
    state: State<'_, AppState>,
    codeplug_id: i64,
    channel_id: i64,
) -> Result<(), String> {
    sqlx::query(
        "DELETE FROM codeplug_channel_scan_lists WHERE codeplug_id = ?1 AND channel_id = ?2",
    )
    .bind(codeplug_id)
    .bind(channel_id)
    .execute(&state.pool)
    .await
    .estr()?;
    Ok(())
}
