use tauri::State;

use crate::commands::{export, rb_api};
use crate::db::AppState;
use crate::radios::registry::driver_for_model;
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

/// A channel's display label for a warning list: its best name and RX freq.
const CHANNEL_LABEL: &str = "COALESCE(NULLIF(c.name_long, ''), NULLIF(c.name_short, ''), \
     NULLIF(c.rb_name, ''), NULLIF(c.callsign, ''), 'Unnamed') \
     || ' · ' || printf('%.4f', c.rx_freq)";

/// True when channel `c` is in a zone/group that some codeplug includes, i.e.
/// it will reach a radio.
const IN_A_CODEPLUG: &str = "EXISTS (SELECT 1 FROM channel_list_entries e \
     JOIN codeplug_channel_lists ccl ON ccl.channel_list_id = e.channel_list_id \
     WHERE e.channel_id = c.id)";

/// Matches `expand_for_export`: a DMR channel expands to nothing when it has
/// neither curated talkgroups nor an inline one.
const DMR_WITHOUT_TALKGROUP: &str = "c.mode = 'DMR' AND c.dmr_talkgroup IS NULL \
     AND NOT EXISTS (SELECT 1 FROM repeater_talkgroups r WHERE r.channel_id = c.id)";

async fn warning_items(
    pool: &sqlx::SqlitePool,
    sql: &str,
) -> Result<Vec<DashboardWarningItem>, String> {
    sqlx::query_as::<_, DashboardWarningItem>(sql).fetch_all(pool).await.estr()
}

async fn channel_items(
    pool: &sqlx::SqlitePool,
    filter: &str,
) -> Result<Vec<DashboardWarningItem>, String> {
    warning_items(
        pool,
        &format!("SELECT c.id, {CHANNEL_LABEL} AS label FROM channels c WHERE {filter} ORDER BY label"),
    )
    .await
}

/// Driver preview warnings that describe what the write will do rather than
/// something lost: a long MD-380 list split into numbered zones, the TM-D710's
/// one-memory-at-a-time write, AM channels routed to the air-band memory.
/// Every other preview warning drops, truncates, or substitutes something.
const INFORMATIONAL_PROGRAM_NOTES: &[&str] = &[
    "so it becomes",
    "this write is not atomic",
    "separate air-band memory",
];

/// What programming this codeplug would complain about, found without a radio.
///
/// A radio with a codeplug programmer answers for itself: its `preview` is the
/// same planner `program` runs, so the dashboard says exactly what the write
/// would warn about, skip, or refuse — the MD-380 splits a long list into more
/// zones where an AnyTone truncates it, and only the driver knows which. Other
/// radios are checked against what the resolver decides: the memory count, and
/// a fixed-zone layout's refusals.
async fn codeplug_program_problems(
    pool: &sqlx::SqlitePool,
    codeplug_id: i64,
) -> Result<Vec<String>, String> {
    let resolved = export::resolve_codeplug_payload(pool, codeplug_id).await?;
    let model = &resolved.model;

    if let Some(programmer) =
        driver_for_model(model).and_then(|d| d.as_codeplug_programmer())
    {
        // Skipped channels are left out: they are the per-model exclusions
        // (band, mode) of a radio-agnostic library, by design, and the
        // codeplug's own preview lists them.
        return Ok(match programmer.preview(&resolved.payload()) {
            Err(e) => vec![e],
            Ok(p) => p
                .warnings
                .into_iter()
                .filter(|w| !INFORMATIONAL_PROGRAM_NOTES.iter().any(|n| w.contains(n)))
                .collect(),
        });
    }

    let mut problems = Vec::new();
    if let Some(layout) = export::fixed_zone_layout(model) {
        let zones = layout.zones;
        let (_, zoned) = export::resolve_codeplug_zone_slots(pool, codeplug_id, layout).await?;
        if !zoned.refused.is_empty() {
            problems.push(format!(
                "{} channels do not fit in the radio's {zones} zones",
                zoned.refused.len()
            ));
        }
        return Ok(problems);
    }
    let included = resolved
        .channels
        .iter()
        .filter(|ec| export::exclusion_reason(&ec.channel, model).is_none())
        .count();
    if let Some(max) = model.memory_channels {
        if included as i64 > max {
            problems.push(format!("{included} channels; the radio holds {max}"));
        }
    }
    Ok(problems)
}

#[tauri::command]
pub async fn get_dashboard_warnings(
    state: State<'_, AppState>,
) -> Result<DashboardWarnings, String> {
    dashboard_warnings(&state.pool).await
}

async fn dashboard_warnings(pool: &sqlx::SqlitePool) -> Result<DashboardWarnings, String> {

    let dmr_without_talkgroup_in_codeplug =
        channel_items(pool, &format!("{DMR_WITHOUT_TALKGROUP} AND {IN_A_CODEPLUG}")).await?;
    let dmr_without_talkgroup_unused =
        channel_items(pool, &format!("{DMR_WITHOUT_TALKGROUP} AND NOT {IN_A_CODEPLUG}")).await?;
    let dmr_without_color_code = channel_items(
        pool,
        &format!("c.mode = 'DMR' AND c.dmr_color_code IS NULL AND {IN_A_CODEPLUG}"),
    )
    .await?;
    let off_air_in_codeplug =
        channel_items(pool, &format!("c.operational_status = 'Off Air' AND {IN_A_CODEPLUG}"))
            .await?;
    let stale_repeaterbook = channel_items(pool, rb_api::STALE_WHERE).await?;

    let unused_channel_lists = warning_items(
        pool,
        "SELECT l.id, l.name AS label FROM channel_lists l \
         WHERE NOT EXISTS (SELECT 1 FROM codeplug_channel_lists x WHERE x.channel_list_id = l.id) \
         ORDER BY l.name",
    )
    .await?;
    let empty_channel_lists = warning_items(
        pool,
        "SELECT l.id, l.name AS label FROM channel_lists l \
         WHERE NOT EXISTS (SELECT 1 FROM channel_list_entries e WHERE e.channel_list_id = l.id) \
         ORDER BY l.name",
    )
    .await?;

    let unused_scan_lists = warning_items(
        pool,
        "SELECT s.id, s.name AS label FROM scan_lists s \
         WHERE NOT EXISTS (SELECT 1 FROM codeplug_scan_lists x WHERE x.scan_list_id = s.id) \
           AND NOT EXISTS (SELECT 1 FROM codeplug_channel_scan_lists y WHERE y.scan_list_id = s.id) \
         ORDER BY s.name",
    )
    .await?;
    let empty_scan_lists = warning_items(
        pool,
        "SELECT s.id, s.name AS label FROM scan_lists s \
         WHERE NOT EXISTS (SELECT 1 FROM scan_list_entries e WHERE e.scan_list_id = s.id) \
         ORDER BY s.name",
    )
    .await?;
    // A scan list member the codeplug does not program has nothing to scan.
    let scan_list_channels_outside_zones = warning_items(
        pool,
        r#"
        SELECT s.id,
               s.name || ' in ' || cp.name || ': ' || COUNT(*) || ' channel'
                 || CASE COUNT(*) WHEN 1 THEN '' ELSE 's' END
                 || ' not in its zones' AS label
        FROM codeplug_scan_lists x
        JOIN scan_lists s ON s.id = x.scan_list_id
        JOIN codeplugs cp ON cp.id = x.codeplug_id
        JOIN scan_list_entries se ON se.scan_list_id = s.id
        WHERE NOT EXISTS (
            SELECT 1 FROM codeplug_channel_lists ccl
            JOIN channel_list_entries ce ON ce.channel_list_id = ccl.channel_list_id
            WHERE ccl.codeplug_id = x.codeplug_id AND ce.channel_id = se.channel_id)
        GROUP BY x.codeplug_id, s.id
        ORDER BY label
        "#,
    )
    .await?;

    let codeplugs_without_profile = warning_items(
        pool,
        "SELECT cp.id, cp.name AS label FROM codeplugs cp \
         WHERE cp.radio_profile_id IS NULL \
            OR NOT EXISTS (SELECT 1 FROM radio_profiles rp WHERE rp.id = cp.radio_profile_id) \
         ORDER BY cp.name",
    )
    .await?;
    let codeplugs_without_zones = warning_items(
        pool,
        "SELECT cp.id, cp.name AS label FROM codeplugs cp \
         WHERE NOT EXISTS (SELECT 1 FROM codeplug_channel_lists x WHERE x.codeplug_id = cp.id) \
         ORDER BY cp.name",
    )
    .await?;

    // The per-codeplug checks resolve each codeplug the way programming does.
    // A codeplug that cannot resolve (no profile) is already warned about.
    let resolvable: Vec<(i64, String, Option<String>)> = sqlx::query_as(
        "SELECT cp.id, cp.name, cp.programmed_fingerprint FROM codeplugs cp \
         JOIN radio_profiles rp ON rp.id = cp.radio_profile_id ORDER BY cp.name",
    )
    .fetch_all(pool)
    .await
    .estr()?;
    let mut codeplugs_changed_since_programmed = Vec::new();
    let mut codeplugs_program_problems = Vec::new();
    for (id, name, stamped) in resolvable {
        if let Some(stamped) = stamped {
            if export::codeplug_fingerprint(pool, id).await.ok().as_ref() != Some(&stamped) {
                codeplugs_changed_since_programmed.push(DashboardWarningItem {
                    id,
                    label: name.clone(),
                });
            }
        }
        if let Ok(problems) = codeplug_program_problems(pool, id).await {
            for p in problems {
                codeplugs_program_problems.push(DashboardWarningItem {
                    id,
                    label: format!("{name}: {p}"),
                });
            }
        }
    }

    Ok(DashboardWarnings {
        dmr_without_talkgroup_in_codeplug,
        dmr_without_talkgroup_unused,
        dmr_without_color_code,
        off_air_in_codeplug,
        stale_repeaterbook,
        unused_channel_lists,
        empty_channel_lists,
        unused_scan_lists,
        empty_scan_lists,
        scan_list_channels_outside_zones,
        codeplugs_without_profile,
        codeplugs_without_zones,
        codeplugs_changed_since_programmed,
        codeplugs_program_problems,
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

