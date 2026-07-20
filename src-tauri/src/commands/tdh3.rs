//! What is left of the TD-H3 command layer: ONE database-only command.
//!
//! Everything that talks to the radio now dispatches through the registry, so
//! nothing model-specific remains here. The protocol and encode/decode have
//! lived in `radios/tidradio_tdh3` since Chunk 3.4 (`ImageProgrammer` in
//! `mod.rs`, `SettingsReader`/`SettingsWriter` in `settings.rs`), and the
//! commands went generic across 3.6d/3.6e:
//!
//!   `read_tdh3_settings_for_profile` → `program::read_radio_settings`
//!   `apply_tdh3_profile`             → `program::write_radio_settings`
//!   `program_tdh3_codeplug`          → `program::program_radio`
//!
//! `save_tdh3_settings_to_profile` stays because it never touches a radio — it
//! renders the Radio Options form into a profile's JSON — so there is no driver
//! to dispatch to. It is TD-H3-shaped only because the form is; folding it into
//! a generic settings-save is a job for the 3.7 dialog registry.


use tauri::State;

use crate::db::AppState;
use crate::error::MapErrString;
use crate::models::RadioProfile;
use crate::radios::tidradio_tdh3 as tdh3;
use crate::radios::tidradio_tdh3::settings::{self as tdh3_settings, Tdh3Settings};

// ============================================================
// Program channels
// ============================================================

// ============================================================
// Settings: import radio settings INTO a saved profile
// ============================================================
//
// The inverse of `apply_profile_settings`: read the radio's current settings and
// store them in a radio profile (update an existing one, merging so the deferred
// DTMF/key keys it already holds are kept, or create a new profile). The
// schema-keyed JSON shape matches what the profile's Settings tab edits.

/// Save the (possibly edited) settings currently in the Radio Options form into a
/// radio profile — either updating an existing profile (merging over its stored
/// settings so keys we don't manage are preserved) or creating a new one. Pure
/// DB + CPU work; the radio was already read to populate the form.
#[tauri::command]
pub async fn save_tdh3_settings_to_profile(
    state: State<'_, AppState>,
    settings: Tdh3Settings,
    model_id: i64,
    profile_id: Option<i64>,
    new_name: Option<String>,
) -> Result<RadioProfile, String> {
    let schema: Option<String> =
        sqlx::query_scalar("SELECT non_channel_settings_schema FROM radio_models WHERE id = ?1")
            .bind(model_id)
            .fetch_optional(&state.pool)
            .await
            .estr()?
            .flatten();
    let schema = schema.ok_or("this radio model has no settings schema")?;

    // Render the form's settings to the schema-keyed JSON shape via a scratch
    // image (encode → decode), reusing the single source-of-truth bit map.
    let mut scratch = vec![0u8; tdh3::MEMSIZE as usize + 0x20];
    tdh3_settings::encode_settings(&mut scratch, &settings);
    let decoded = tdh3_settings::decode_profile_settings(&scratch, &schema)?;
    let decoded_obj = decoded.as_object().cloned().unwrap_or_default();

    let id = if let Some(pid) = profile_id {
        // Merge the read keys over the profile's existing settings.
        let existing: Option<String> =
            sqlx::query_scalar("SELECT non_channel_settings FROM radio_profiles WHERE id = ?1")
                .bind(pid)
                .fetch_optional(&state.pool)
                .await
                .estr()?
                .flatten();
        let mut base = existing
            .as_deref()
            .and_then(|s| serde_json::from_str::<serde_json::Value>(s).ok())
            .and_then(|v| v.as_object().cloned())
            .unwrap_or_default();
        for (k, v) in decoded_obj {
            base.insert(k, v);
        }
        let merged = serde_json::to_string(&serde_json::Value::Object(base)).estr()?;
        sqlx::query(
            "UPDATE radio_profiles SET non_channel_settings = ?2, updated_at = CURRENT_TIMESTAMP WHERE id = ?1",
        )
        .bind(pid)
        .bind(&merged)
        .execute(&state.pool)
        .await
        .estr()?;
        pid
    } else {
        let name = new_name
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .ok_or("Enter a name for the new profile.")?;
        let json = serde_json::to_string(&serde_json::Value::Object(decoded_obj)).estr()?;
        sqlx::query(
            "INSERT INTO radio_profiles (display_name, radio_model_id, non_channel_settings, notes, updated_at) \
             VALUES (?1, ?2, ?3, NULL, CURRENT_TIMESTAMP)",
        )
        .bind(&name)
        .bind(model_id)
        .bind(&json)
        .execute(&state.pool)
        .await
        .estr()?
        .last_insert_rowid()
    };

    sqlx::query_as::<_, RadioProfile>(
        "SELECT id, display_name, radio_model_id, non_channel_settings, notes, created_at, updated_at FROM radio_profiles WHERE id = ?1",
    )
    .bind(id)
    .fetch_one(&state.pool)
    .await
    .estr()
}
