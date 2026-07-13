use tauri::State;

use crate::db::AppState;
use crate::error::MapErrString;
use crate::models::{CodeplugSummary, DashboardStats};

#[tauri::command]
pub async fn get_dashboard_stats(state: State<'_, AppState>) -> Result<DashboardStats, String> {
    let total_channels: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM channels")
        .fetch_one(&state.pool)
        .await
        .estr()?;
    let total_channel_lists: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM channel_lists")
        .fetch_one(&state.pool)
        .await
        .estr()?;
    let total_scan_lists: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM scan_lists")
        .fetch_one(&state.pool)
        .await
        .estr()?;
    let total_codeplugs: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM codeplugs")
        .fetch_one(&state.pool)
        .await
        .estr()?;
    let total_radio_profiles: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM radio_profiles")
        .fetch_one(&state.pool)
        .await
        .estr()?;
    let total_talkgroups: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM talkgroups")
        .fetch_one(&state.pool)
        .await
        .estr()?;

    Ok(DashboardStats {
        total_channels: total_channels.0,
        total_channel_lists: total_channel_lists.0,
        total_scan_lists: total_scan_lists.0,
        total_codeplugs: total_codeplugs.0,
        total_radio_profiles: total_radio_profiles.0,
        total_talkgroups: total_talkgroups.0,
    })
}

#[tauri::command]
pub async fn get_recent_codeplugs(
    state: State<'_, AppState>,
) -> Result<Vec<CodeplugSummary>, String> {
    sqlx::query_as::<_, CodeplugSummary>(
        r#"
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
        ORDER BY COALESCE(cp.updated_at, cp.created_at) DESC
        LIMIT 5
        "#,
    )
    .fetch_all(&state.pool)
    .await
    .estr()
}
