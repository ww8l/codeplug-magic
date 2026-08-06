use tauri::State;

use crate::db::AppState;
use crate::error::MapErrString;
use crate::models::{RadioModel, RadioProfile, RadioProfileInput};

const MODEL_COLUMNS: &str = "id, manufacturer, model, display_name, analog_capable, dmr_capable, dstar_capable, ysf_capable, nxdn_capable, p25_capable, m17_capable, aprs_capable, covers_hf, covers_vhf, covers_uhf, covers_220, covers_900, freq_min, freq_max, tx_bands, rx_bands, memory_channels, zones_supported, max_zones, channels_per_zone, scan_lists_supported, max_scan_lists, banks_supported, max_name_length, export_format, connection_type, non_channel_settings_schema, driver_key, programming_ui";

// ============================================================
// Radio Model library (read-only)
// ============================================================
#[tauri::command]
pub async fn list_radio_models(state: State<'_, AppState>) -> Result<Vec<RadioModel>, String> {
    sqlx::query_as::<_, RadioModel>(&format!(
        "SELECT {MODEL_COLUMNS} FROM radio_models ORDER BY manufacturer, model"
    ))
    .fetch_all(&state.pool)
    .await
    .estr()
}

#[tauri::command]
pub async fn get_radio_model(state: State<'_, AppState>, id: i64) -> Result<RadioModel, String> {
    sqlx::query_as::<_, RadioModel>(&format!(
        "SELECT {MODEL_COLUMNS} FROM radio_models WHERE id = ?1"
    ))
    .bind(id)
    .fetch_one(&state.pool)
    .await
    .estr()
}

// ============================================================
// Radio Profiles
// ============================================================
#[tauri::command]
pub async fn list_radio_profiles(
    state: State<'_, AppState>,
) -> Result<Vec<RadioProfile>, String> {
    sqlx::query_as::<_, RadioProfile>(
        "SELECT id, display_name, radio_model_id, non_channel_settings, notes, created_at, updated_at FROM radio_profiles ORDER BY display_name",
    )
    .fetch_all(&state.pool)
    .await
    .estr()
}

#[tauri::command]
pub async fn get_radio_profile(
    state: State<'_, AppState>,
    id: i64,
) -> Result<RadioProfile, String> {
    sqlx::query_as::<_, RadioProfile>(
        "SELECT id, display_name, radio_model_id, non_channel_settings, notes, created_at, updated_at FROM radio_profiles WHERE id = ?1",
    )
    .bind(id)
    .fetch_one(&state.pool)
    .await
    .estr()
}

#[tauri::command]
pub async fn create_radio_profile(
    state: State<'_, AppState>,
    input: RadioProfileInput,
) -> Result<RadioProfile, String> {
    let id = sqlx::query(
        "INSERT INTO radio_profiles (display_name, radio_model_id, non_channel_settings, notes, updated_at) VALUES (?1, ?2, ?3, ?4, CURRENT_TIMESTAMP)",
    )
    .bind(&input.display_name)
    .bind(input.radio_model_id)
    .bind(&input.non_channel_settings)
    .bind(&input.notes)
    .execute(&state.pool)
    .await
    .estr()?
    .last_insert_rowid();

    get_radio_profile(state, id).await
}

#[tauri::command]
pub async fn update_radio_profile(
    state: State<'_, AppState>,
    id: i64,
    input: RadioProfileInput,
) -> Result<RadioProfile, String> {
    sqlx::query(
        "UPDATE radio_profiles SET display_name = ?2, radio_model_id = ?3, non_channel_settings = ?4, notes = ?5, updated_at = CURRENT_TIMESTAMP WHERE id = ?1",
    )
    .bind(id)
    .bind(&input.display_name)
    .bind(input.radio_model_id)
    .bind(&input.non_channel_settings)
    .bind(&input.notes)
    .execute(&state.pool)
    .await
    .estr()?;

    get_radio_profile(state, id).await
}

#[tauri::command]
pub async fn delete_radio_profile(state: State<'_, AppState>, id: i64) -> Result<(), String> {
    sqlx::query("DELETE FROM radio_profiles WHERE id = ?1")
        .bind(id)
        .execute(&state.pool)
        .await
        .estr()?;
    Ok(())
}
