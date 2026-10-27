use tauri::State;

use crate::db::AppState;
use crate::error::MapErrString;
use crate::models::{CodeplugSummary, DashboardStats, DashboardWarningItem, DashboardWarnings};

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
pub async fn get_dashboard_warnings(
    state: State<'_, AppState>,
) -> Result<DashboardWarnings, String> {
    let items = |sql: &'static str| {
        sqlx::query_as::<_, DashboardWarningItem>(sql).fetch_all(&state.pool)
    };

    // Matches `expand_for_export`: a DMR channel expands to nothing when it has
    // neither curated talkgroups nor an inline one.
    let dmr_channels_without_talkgroup = items(
        r#"
        SELECT c.id,
               COALESCE(NULLIF(c.name_long, ''), NULLIF(c.name_short, ''),
                        NULLIF(c.rb_name, ''), NULLIF(c.callsign, ''), 'Unnamed')
                 || ' · ' || printf('%.4f', c.rx_freq) AS label
        FROM channels c
        WHERE c.mode = 'DMR'
          AND c.dmr_talkgroup IS NULL
          AND NOT EXISTS (SELECT 1 FROM repeater_talkgroups r WHERE r.channel_id = c.id)
        ORDER BY label
        "#,
    )
    .await
    .estr()?;

    let unused_channel_lists = items(
        r#"
        SELECT l.id, l.name AS label
        FROM channel_lists l
        WHERE NOT EXISTS (SELECT 1 FROM codeplug_channel_lists x WHERE x.channel_list_id = l.id)
        ORDER BY l.name
        "#,
    )
    .await
    .estr()?;

    let unused_scan_lists = items(
        r#"
        SELECT s.id, s.name AS label
        FROM scan_lists s
        WHERE NOT EXISTS (SELECT 1 FROM codeplug_scan_lists x WHERE x.scan_list_id = s.id)
          AND NOT EXISTS (SELECT 1 FROM codeplug_channel_scan_lists y WHERE y.scan_list_id = s.id)
        ORDER BY s.name
        "#,
    )
    .await
    .estr()?;

    let codeplugs_without_profile = items(
        r#"
        SELECT cp.id, cp.name AS label
        FROM codeplugs cp
        WHERE cp.radio_profile_id IS NULL
           OR NOT EXISTS (SELECT 1 FROM radio_profiles rp WHERE rp.id = cp.radio_profile_id)
        ORDER BY cp.name
        "#,
    )
    .await
    .estr()?;

    Ok(DashboardWarnings {
        dmr_channels_without_talkgroup,
        unused_channel_lists,
        unused_scan_lists,
        codeplugs_without_profile,
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
