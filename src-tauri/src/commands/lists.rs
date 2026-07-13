use sqlx::Acquire;
use tauri::State;

use crate::db::AppState;
use crate::error::MapErrString;
use crate::models::{
    Channel, ChannelList, ChannelListSummary, ScanList, ScanListSettings, ScanListSummary,
};

// ============================================================
// Channel Lists
// ============================================================
#[tauri::command]
pub async fn list_channel_lists(
    state: State<'_, AppState>,
) -> Result<Vec<ChannelListSummary>, String> {
    sqlx::query_as::<_, ChannelListSummary>(
        r#"
        SELECT cl.id, cl.name, cl.description,
               (SELECT COUNT(*) FROM channel_list_entries e WHERE e.channel_list_id = cl.id) AS channel_count
        FROM channel_lists cl
        ORDER BY cl.name
        "#,
    )
    .fetch_all(&state.pool)
    .await
    .estr()
}

#[tauri::command]
pub async fn create_channel_list(
    state: State<'_, AppState>,
    name: String,
    description: Option<String>,
) -> Result<ChannelList, String> {
    let id = sqlx::query("INSERT INTO channel_lists (name, description) VALUES (?1, ?2)")
        .bind(&name)
        .bind(&description)
        .execute(&state.pool)
        .await
        .estr()?
        .last_insert_rowid();

    sqlx::query_as::<_, ChannelList>(
        "SELECT id, name, description, created_at, updated_at FROM channel_lists WHERE id = ?1",
    )
    .bind(id)
    .fetch_one(&state.pool)
    .await
    .estr()
}

#[tauri::command]
pub async fn update_channel_list(
    state: State<'_, AppState>,
    id: i64,
    name: String,
    description: Option<String>,
) -> Result<(), String> {
    sqlx::query(
        "UPDATE channel_lists SET name = ?2, description = ?3, updated_at = CURRENT_TIMESTAMP WHERE id = ?1",
    )
    .bind(id)
    .bind(&name)
    .bind(&description)
    .execute(&state.pool)
    .await
    .estr()?;
    Ok(())
}

#[tauri::command]
pub async fn delete_channel_list(state: State<'_, AppState>, id: i64) -> Result<(), String> {
    sqlx::query("DELETE FROM channel_lists WHERE id = ?1")
        .bind(id)
        .execute(&state.pool)
        .await
        .estr()?;
    Ok(())
}

#[tauri::command]
pub async fn get_channel_list_channels(
    state: State<'_, AppState>,
    list_id: i64,
) -> Result<Vec<Channel>, String> {
    sqlx::query_as::<_, Channel>(
        r#"
        SELECT c.* FROM channels c
        JOIN channel_list_entries e ON e.channel_id = c.id
        WHERE e.channel_list_id = ?1
        ORDER BY e.position
        "#,
    )
    .bind(list_id)
    .fetch_all(&state.pool)
    .await
    .estr()
}

#[tauri::command]
pub async fn add_channel_to_list(
    state: State<'_, AppState>,
    list_id: i64,
    channel_id: i64,
) -> Result<(), String> {
    // Skip if the channel is already in this list.
    let exists: Option<(i64,)> = sqlx::query_as(
        "SELECT id FROM channel_list_entries WHERE channel_list_id = ?1 AND channel_id = ?2",
    )
    .bind(list_id)
    .bind(channel_id)
    .fetch_optional(&state.pool)
    .await
    .estr()?;
    if exists.is_some() {
        return Ok(());
    }

    let next_pos: (i64,) = sqlx::query_as(
        "SELECT COALESCE(MAX(position), -1) + 1 FROM channel_list_entries WHERE channel_list_id = ?1",
    )
    .bind(list_id)
    .fetch_one(&state.pool)
    .await
    .estr()?;

    sqlx::query(
        "INSERT INTO channel_list_entries (channel_list_id, channel_id, position) VALUES (?1, ?2, ?3)",
    )
    .bind(list_id)
    .bind(channel_id)
    .bind(next_pos.0)
    .execute(&state.pool)
    .await
    .estr()?;
    Ok(())
}

/// Add many channels to a list at once (e.g. from a multi-select on the
/// Repeaters screen). Skips channels already in the list; appends the rest in
/// the given order. Returns how many were newly added.
#[tauri::command]
pub async fn add_channels_to_list(
    state: State<'_, AppState>,
    list_id: i64,
    channel_ids: Vec<i64>,
) -> Result<u64, String> {
    let mut conn = state.pool.acquire().await.estr()?;
    let mut tx = conn.begin().await.estr()?;

    let mut next_pos: i64 = sqlx::query_as::<_, (i64,)>(
        "SELECT COALESCE(MAX(position), -1) + 1 FROM channel_list_entries WHERE channel_list_id = ?1",
    )
    .bind(list_id)
    .fetch_one(&mut *tx)
    .await
    .estr()?
    .0;

    let mut added = 0u64;
    for channel_id in &channel_ids {
        let exists: Option<(i64,)> = sqlx::query_as(
            "SELECT id FROM channel_list_entries WHERE channel_list_id = ?1 AND channel_id = ?2",
        )
        .bind(list_id)
        .bind(channel_id)
        .fetch_optional(&mut *tx)
        .await
        .estr()?;
        if exists.is_some() {
            continue;
        }
        sqlx::query(
            "INSERT INTO channel_list_entries (channel_list_id, channel_id, position) VALUES (?1, ?2, ?3)",
        )
        .bind(list_id)
        .bind(channel_id)
        .bind(next_pos)
        .execute(&mut *tx)
        .await
        .estr()?;
        next_pos += 1;
        added += 1;
    }

    tx.commit().await.estr()?;
    Ok(added)
}

#[tauri::command]
pub async fn remove_channel_from_list(
    state: State<'_, AppState>,
    list_id: i64,
    channel_id: i64,
) -> Result<(), String> {
    sqlx::query(
        "DELETE FROM channel_list_entries WHERE channel_list_id = ?1 AND channel_id = ?2",
    )
    .bind(list_id)
    .bind(channel_id)
    .execute(&state.pool)
    .await
    .estr()?;
    Ok(())
}

/// Replace the ordering of a channel list with the supplied channel id order.
/// Done in a transaction (delete + reinsert) to satisfy the
/// UNIQUE(channel_list_id, position) constraint cleanly.
#[tauri::command]
pub async fn reorder_channel_list(
    state: State<'_, AppState>,
    list_id: i64,
    ordered_channel_ids: Vec<i64>,
) -> Result<(), String> {
    let mut conn = state.pool.acquire().await.estr()?;
    let mut tx = conn.begin().await.estr()?;

    sqlx::query("DELETE FROM channel_list_entries WHERE channel_list_id = ?1")
        .bind(list_id)
        .execute(&mut *tx)
        .await
        .estr()?;

    for (pos, channel_id) in ordered_channel_ids.iter().enumerate() {
        sqlx::query(
            "INSERT INTO channel_list_entries (channel_list_id, channel_id, position) VALUES (?1, ?2, ?3)",
        )
        .bind(list_id)
        .bind(channel_id)
        .bind(pos as i64)
        .execute(&mut *tx)
        .await
        .estr()?;
    }

    tx.commit().await.estr()?;
    Ok(())
}

// ============================================================
// Scan Lists
// ============================================================
#[tauri::command]
pub async fn list_scan_lists(state: State<'_, AppState>) -> Result<Vec<ScanListSummary>, String> {
    sqlx::query_as::<_, ScanListSummary>(
        r#"
        SELECT sl.id, sl.name, sl.description,
               sl.priority_channel_id, sl.priority_channel_2_id, sl.priority_select,
               sl.look_back_a, sl.look_back_b, sl.dropout_delay, sl.dwell_time, sl.revert_channel,
               (SELECT COUNT(*) FROM scan_list_entries e WHERE e.scan_list_id = sl.id) AS channel_count
        FROM scan_lists sl
        ORDER BY sl.name
        "#,
    )
    .fetch_all(&state.pool)
    .await
    .estr()
}

#[tauri::command]
pub async fn create_scan_list(
    state: State<'_, AppState>,
    name: String,
    description: Option<String>,
) -> Result<ScanList, String> {
    // revert_channel defaults to 4 (Last Called) at the app level — the column
    // DEFAULT stays 0 for checksum reasons (see migration 0013).
    let id = sqlx::query(
        "INSERT INTO scan_lists (name, description, revert_channel) VALUES (?1, ?2, 4)",
    )
        .bind(&name)
        .bind(&description)
        .execute(&state.pool)
        .await
        .estr()?
        .last_insert_rowid();

    sqlx::query_as::<_, ScanList>(
        r#"SELECT id, name, description,
                  priority_channel_id, priority_channel_2_id, priority_select,
                  look_back_a, look_back_b, dropout_delay, dwell_time, revert_channel,
                  created_at
           FROM scan_lists WHERE id = ?1"#,
    )
    .bind(id)
    .fetch_one(&state.pool)
    .await
    .estr()
}

#[tauri::command]
pub async fn update_scan_list(
    state: State<'_, AppState>,
    id: i64,
    name: String,
    description: Option<String>,
    settings: ScanListSettings,
) -> Result<(), String> {
    // Validate enum/range bounds before writing (timers are u16 on the radio).
    if !(0..=3).contains(&settings.priority_select) {
        return Err(format!("priority_select {} out of range (0..=3)", settings.priority_select));
    }
    if !(0..=7).contains(&settings.revert_channel) {
        return Err(format!("revert_channel {} out of range (0..=7)", settings.revert_channel));
    }
    for (label, v) in [
        ("look_back_a", settings.look_back_a),
        ("look_back_b", settings.look_back_b),
        ("dropout_delay", settings.dropout_delay),
        ("dwell_time", settings.dwell_time),
    ] {
        if !(0..=65535).contains(&v) {
            return Err(format!("{label} {v} out of range (0..=65535)"));
        }
    }
    sqlx::query(
        r#"UPDATE scan_lists SET
             name = ?2, description = ?3,
             priority_channel_id = ?4, priority_channel_2_id = ?5, priority_select = ?6,
             look_back_a = ?7, look_back_b = ?8, dropout_delay = ?9, dwell_time = ?10,
             revert_channel = ?11
           WHERE id = ?1"#,
    )
    .bind(id)
    .bind(&name)
    .bind(&description)
    .bind(settings.priority_channel_id)
    .bind(settings.priority_channel_2_id)
    .bind(settings.priority_select)
    .bind(settings.look_back_a)
    .bind(settings.look_back_b)
    .bind(settings.dropout_delay)
    .bind(settings.dwell_time)
    .bind(settings.revert_channel)
    .execute(&state.pool)
    .await
    .estr()?;
    Ok(())
}

#[tauri::command]
pub async fn delete_scan_list(state: State<'_, AppState>, id: i64) -> Result<(), String> {
    sqlx::query("DELETE FROM scan_lists WHERE id = ?1")
        .bind(id)
        .execute(&state.pool)
        .await
        .estr()?;
    Ok(())
}

#[tauri::command]
pub async fn get_scan_list_channels(
    state: State<'_, AppState>,
    list_id: i64,
) -> Result<Vec<Channel>, String> {
    sqlx::query_as::<_, Channel>(
        r#"
        SELECT c.* FROM channels c
        JOIN scan_list_entries e ON e.channel_id = c.id
        WHERE e.scan_list_id = ?1
        ORDER BY e.position
        "#,
    )
    .bind(list_id)
    .fetch_all(&state.pool)
    .await
    .estr()
}

#[tauri::command]
pub async fn add_channel_to_scan_list(
    state: State<'_, AppState>,
    list_id: i64,
    channel_id: i64,
) -> Result<(), String> {
    let exists: Option<(i64,)> = sqlx::query_as(
        "SELECT id FROM scan_list_entries WHERE scan_list_id = ?1 AND channel_id = ?2",
    )
    .bind(list_id)
    .bind(channel_id)
    .fetch_optional(&state.pool)
    .await
    .estr()?;
    if exists.is_some() {
        return Ok(());
    }

    let next_pos: (i64,) = sqlx::query_as(
        "SELECT COALESCE(MAX(position), -1) + 1 FROM scan_list_entries WHERE scan_list_id = ?1",
    )
    .bind(list_id)
    .fetch_one(&state.pool)
    .await
    .estr()?;

    sqlx::query(
        "INSERT INTO scan_list_entries (scan_list_id, channel_id, position) VALUES (?1, ?2, ?3)",
    )
    .bind(list_id)
    .bind(channel_id)
    .bind(next_pos.0)
    .execute(&state.pool)
    .await
    .estr()?;
    Ok(())
}

/// Add many channels to a scan list at once. Mirrors [`add_channels_to_list`]:
/// skips channels already in the list, appends the rest in order, returns how
/// many were newly added. Used when seeding a scan list created alongside a
/// channel list from the same selection.
#[tauri::command]
pub async fn add_channels_to_scan_list(
    state: State<'_, AppState>,
    list_id: i64,
    channel_ids: Vec<i64>,
) -> Result<u64, String> {
    let mut conn = state.pool.acquire().await.estr()?;
    let mut tx = conn.begin().await.estr()?;

    let mut next_pos: i64 = sqlx::query_as::<_, (i64,)>(
        "SELECT COALESCE(MAX(position), -1) + 1 FROM scan_list_entries WHERE scan_list_id = ?1",
    )
    .bind(list_id)
    .fetch_one(&mut *tx)
    .await
    .estr()?
    .0;

    let mut added = 0u64;
    for channel_id in &channel_ids {
        let exists: Option<(i64,)> = sqlx::query_as(
            "SELECT id FROM scan_list_entries WHERE scan_list_id = ?1 AND channel_id = ?2",
        )
        .bind(list_id)
        .bind(channel_id)
        .fetch_optional(&mut *tx)
        .await
        .estr()?;
        if exists.is_some() {
            continue;
        }
        sqlx::query(
            "INSERT INTO scan_list_entries (scan_list_id, channel_id, position) VALUES (?1, ?2, ?3)",
        )
        .bind(list_id)
        .bind(channel_id)
        .bind(next_pos)
        .execute(&mut *tx)
        .await
        .estr()?;
        next_pos += 1;
        added += 1;
    }

    tx.commit().await.estr()?;
    Ok(added)
}

#[tauri::command]
pub async fn remove_channel_from_scan_list(
    state: State<'_, AppState>,
    list_id: i64,
    channel_id: i64,
) -> Result<(), String> {
    sqlx::query("DELETE FROM scan_list_entries WHERE scan_list_id = ?1 AND channel_id = ?2")
        .bind(list_id)
        .bind(channel_id)
        .execute(&state.pool)
        .await
        .estr()?;
    Ok(())
}

#[tauri::command]
pub async fn reorder_scan_list(
    state: State<'_, AppState>,
    list_id: i64,
    ordered_channel_ids: Vec<i64>,
) -> Result<(), String> {
    let mut conn = state.pool.acquire().await.estr()?;
    let mut tx = conn.begin().await.estr()?;

    sqlx::query("DELETE FROM scan_list_entries WHERE scan_list_id = ?1")
        .bind(list_id)
        .execute(&mut *tx)
        .await
        .estr()?;

    for (pos, channel_id) in ordered_channel_ids.iter().enumerate() {
        sqlx::query(
            "INSERT INTO scan_list_entries (scan_list_id, channel_id, position) VALUES (?1, ?2, ?3)",
        )
        .bind(list_id)
        .bind(channel_id)
        .bind(pos as i64)
        .execute(&mut *tx)
        .await
        .estr()?;
    }

    tx.commit().await.estr()?;
    Ok(())
}
