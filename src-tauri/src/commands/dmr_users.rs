//! DMR-ID -> callsign/name lookup library ("caller ID" database), sourced
//! from radioid.net's public dump (`https://database.radioid.net/static/users.json`).
//!
//! This is deliberately separate from the small per-codeplug talkgroup
//! "contacts" list (`talkgroups.rs`) and from any radio-specific write path:
//! this module only owns the local library, refreshing it, and building a
//! capacity-capped, country/continent-prioritized subset for export. Which
//! radio a subset ends up on, and how it gets there, is out of scope here.

use serde::Deserialize;
use sqlx::{Acquire, Sqlite, Transaction};
use tauri::State;

use crate::db::AppState;
use crate::error::MapErrString;
use crate::models::{
    DmrExportCountryBreakdown, DmrExportPreview, DmrExportSummary, DmrUser,
    DmrUsersRefreshSummary, DmrUsersStatus,
};

const USERS_JSON_URL: &str = "https://database.radioid.net/static/users.json";

/// Rows are upserted in batches inside one transaction so a ~350k-row refresh
/// doesn't hold a single enormous statement or a lock for the whole import.
const UPSERT_BATCH: usize = 2000;

/// Cap on rows sent to the UI for a single browse page.
const MAX_PAGE_SIZE: i64 = 500;

// ============================================================
// Refresh
// ============================================================

#[derive(Debug, Deserialize)]
struct RadioIdDump {
    users: Vec<RadioIdUser>,
}

#[derive(Debug, Deserialize)]
struct RadioIdUser {
    id: i64,
    callsign: String,
    #[serde(default)]
    fname: Option<String>,
    #[serde(default)]
    surname: Option<String>,
    #[serde(default)]
    city: Option<String>,
    #[serde(default)]
    state: Option<String>,
    #[serde(default)]
    country: Option<String>,
    #[serde(default)]
    remarks: Option<String>,
}

/// Blank strings coming out of the dump are noise, not data.
fn norm(s: Option<String>) -> Option<String> {
    s.and_then(|v| {
        let t = v.trim();
        if t.is_empty() {
            None
        } else {
            Some(t.to_string())
        }
    })
}

/// Upsert a batch of parsed dump rows into an open transaction, chunked so a
/// ~350k-row refresh doesn't hold one enormous statement. Returns
/// `(added, updated)`. Split out from `refresh_dmr_users` so it can be
/// exercised in tests without a network round-trip.
async fn upsert_dmr_users(
    tx: &mut Transaction<'_, Sqlite>,
    users: &[RadioIdUser],
) -> Result<(usize, usize), String> {
    let before: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM dmr_users")
        .fetch_one(&mut **tx)
        .await
        .estr()?;

    for chunk in users.chunks(UPSERT_BATCH) {
        for u in chunk {
            sqlx::query(
                "INSERT INTO dmr_users
                     (dmr_id, callsign, first_name, last_name, city, state, country, remarks, source, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 'radioid.net', CURRENT_TIMESTAMP)
                 ON CONFLICT(dmr_id) DO UPDATE SET
                     callsign = excluded.callsign,
                     first_name = excluded.first_name,
                     last_name = excluded.last_name,
                     city = excluded.city,
                     state = excluded.state,
                     country = excluded.country,
                     remarks = excluded.remarks,
                     updated_at = CURRENT_TIMESTAMP",
            )
            .bind(u.id)
            .bind(&u.callsign)
            .bind(norm(u.fname.clone()))
            .bind(norm(u.surname.clone()))
            .bind(norm(u.city.clone()))
            .bind(norm(u.state.clone()))
            .bind(norm(u.country.clone()))
            .bind(norm(u.remarks.clone()))
            .execute(&mut **tx)
            .await
            .estr()?;
        }
    }

    let after: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM dmr_users")
        .fetch_one(&mut **tx)
        .await
        .estr()?;

    let added = (after.0 - before.0).max(0) as usize;
    let updated = users.len().saturating_sub(added);
    Ok((added, updated))
}

#[tauri::command]
pub async fn refresh_dmr_users(
    state: State<'_, AppState>,
) -> Result<DmrUsersRefreshSummary, String> {
    let resp = reqwest::get(USERS_JSON_URL)
        .await
        .map_err(|e| format!("Could not reach radioid.net: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("radioid.net returned HTTP {}", resp.status()));
    }
    let dump: RadioIdDump = resp
        .json()
        .await
        .map_err(|e| format!("Could not parse radioid.net response: {e}"))?;
    let fetched = dump.users.len();

    let mut conn = state.pool.acquire().await.estr()?;
    let mut tx = conn.begin().await.estr()?;
    let (added, updated) = upsert_dmr_users(&mut tx, &dump.users).await?;
    tx.commit().await.estr()?;

    let total: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM dmr_users")
        .fetch_one(&state.pool)
        .await
        .estr()?;

    Ok(DmrUsersRefreshSummary {
        fetched,
        added,
        updated,
        total: total.0,
    })
}

// ============================================================
// Status / browse
// ============================================================

#[tauri::command]
pub async fn dmr_users_status(state: State<'_, AppState>) -> Result<DmrUsersStatus, String> {
    let row: (i64, Option<String>) =
        sqlx::query_as("SELECT COUNT(*), MAX(updated_at) FROM dmr_users")
            .fetch_one(&state.pool)
            .await
            .estr()?;
    Ok(DmrUsersStatus {
        total: row.0,
        last_refreshed_at: row.1,
    })
}

#[tauri::command]
pub async fn list_dmr_users(
    state: State<'_, AppState>,
    search: Option<String>,
    country: Option<String>,
    limit: i64,
    offset: i64,
) -> Result<Vec<DmrUser>, String> {
    let limit = limit.clamp(1, MAX_PAGE_SIZE);
    let offset = offset.max(0);
    let term = search.unwrap_or_default();
    let term = term.trim();
    let like = format!("%{term}%");
    let country = country.filter(|c| !c.trim().is_empty());

    let base = "SELECT id, dmr_id, callsign, first_name, last_name, city, state, country,
                        remarks, source, updated_at
                 FROM dmr_users";
    let mut clauses: Vec<&str> = Vec::new();
    if !term.is_empty() {
        clauses.push("(callsign LIKE ?1 OR first_name LIKE ?1 OR last_name LIKE ?1 OR CAST(dmr_id AS TEXT) LIKE ?1)");
    }
    if country.is_some() {
        clauses.push(if term.is_empty() { "country = ?1" } else { "country = ?2" });
    }
    let where_clause = if clauses.is_empty() {
        String::new()
    } else {
        format!(" WHERE {}", clauses.join(" AND "))
    };
    let sql = format!("{base}{where_clause} ORDER BY callsign LIMIT ?{} OFFSET ?{}",
        clauses.len() + 1, clauses.len() + 2);

    let mut q = sqlx::query_as::<_, DmrUser>(&sql);
    if !term.is_empty() {
        q = q.bind(like);
    }
    if let Some(c) = &country {
        q = q.bind(c);
    }
    q = q.bind(limit).bind(offset);
    q.fetch_all(&state.pool).await.estr()
}

#[tauri::command]
pub async fn list_dmr_user_countries(state: State<'_, AppState>) -> Result<Vec<String>, String> {
    let rows: Vec<(String,)> = sqlx::query_as(
        "SELECT DISTINCT country FROM dmr_users WHERE country IS NOT NULL ORDER BY country",
    )
    .fetch_all(&state.pool)
    .await
    .estr()?;
    Ok(rows.into_iter().map(|r| r.0).collect())
}

#[tauri::command]
pub fn list_dmr_continents() -> Vec<String> {
    let mut seen = std::collections::BTreeSet::new();
    for (_, continent) in COUNTRY_CONTINENTS {
        seen.insert((*continent).to_string());
    }
    seen.into_iter().collect()
}

// ============================================================
// Prioritized subset selection (shared by preview + export)
// ============================================================

/// Expand any continent names in `priority` into their member countries (in
/// table order), leaving plain country names as-is, and dedupe while
/// preserving the caller's chosen order. This is what lets the export dialog
/// accept a mix of countries and continents in one priority list.
fn expand_priority(priority: &[String]) -> Vec<String> {
    let mut out = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for item in priority {
        let item = item.trim();
        if item.is_empty() {
            continue;
        }
        let is_continent = COUNTRY_CONTINENTS
            .iter()
            .any(|(_, continent)| continent.eq_ignore_ascii_case(item));
        if is_continent {
            for (country, continent) in COUNTRY_CONTINENTS {
                if continent.eq_ignore_ascii_case(item) {
                    let key = country.to_lowercase();
                    if seen.insert(key) {
                        out.push((*country).to_string());
                    }
                }
            }
        } else {
            let key = item.to_lowercase();
            if seen.insert(key) {
                out.push(item.to_string());
            }
        }
    }
    out
}

/// Build the `CASE country WHEN ? THEN 0 WHEN ? THEN 1 ... ELSE n END`
/// ordering fragment for a priority list, ranking earlier entries first and
/// everything else after.
fn priority_case_sql(priority: &[String]) -> String {
    if priority.is_empty() {
        // NOT "0": a bare integer literal in ORDER BY is read as a column
        // ordinal (and 0 is out of range → a runtime error). NULL is a valid
        // constant sort key that leaves the secondary `dmr_id` ordering intact.
        return "NULL".to_string();
    }
    let arms: Vec<String> = priority
        .iter()
        .enumerate()
        .map(|(i, _)| format!("WHEN ? THEN {i}"))
        .collect();
    format!("CASE country {} ELSE {} END", arms.join(" "), priority.len())
}

#[tauri::command]
pub async fn preview_dmr_export(
    state: State<'_, AppState>,
    max_count: Option<i64>,
    priority_countries: Vec<String>,
) -> Result<DmrExportPreview, String> {
    let priority = expand_priority(&priority_countries);

    let total: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM dmr_users")
        .fetch_one(&state.pool)
        .await
        .estr()?;
    let will_export = match max_count {
        Some(n) => n.max(0).min(total.0),
        None => total.0,
    };

    let case_sql = priority_case_sql(&priority);
    let sql = format!(
        "SELECT country, COUNT(*) as n FROM (
            SELECT country FROM dmr_users ORDER BY {case_sql}, dmr_id LIMIT ?
         ) GROUP BY country ORDER BY n DESC"
    );
    let mut q = sqlx::query_as::<_, (Option<String>, i64)>(&sql);
    for c in &priority {
        q = q.bind(c);
    }
    q = q.bind(will_export);
    let rows = q.fetch_all(&state.pool).await.estr()?;

    Ok(DmrExportPreview {
        total_available: total.0,
        will_export,
        by_country: rows
            .into_iter()
            .map(|(country, count)| DmrExportCountryBreakdown {
                country: country.unwrap_or_else(|| "(unknown)".to_string()),
                count,
            })
            .collect(),
    })
}

/// One prioritized DMR-user row selected for a downstream consumer (CSV export
/// or a radio-specific caller-ID writer). Deliberately radio-agnostic — this
/// module owns *which* users make the cut, never *how* a radio stores them.
#[derive(Debug, Clone)]
pub struct SelectedDmrUser {
    pub dmr_id: i64,
    pub callsign: String,
    pub first_name: Option<String>,
    pub last_name: Option<String>,
    pub city: Option<String>,
    pub state: Option<String>,
    pub country: Option<String>,
    pub remarks: Option<String>,
}

impl SelectedDmrUser {
    /// Human display name = first + last (trimmed), the same shape the CSV
    /// export writes to its "Name" column.
    pub fn display_name(&self) -> String {
        [self.first_name.as_deref(), self.last_name.as_deref()]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
            .join(" ")
    }
}

/// Select the capacity-capped, priority-ordered batch of DMR users. Shared by
/// the CSV export and the radio caller-ID writer so they agree byte-for-byte on
/// *who* gets exported and in what order. `max_count` `None` means no cap.
pub async fn select_prioritized_dmr_users(
    pool: &sqlx::SqlitePool,
    max_count: Option<i64>,
    priority_countries: &[String],
) -> Result<Vec<SelectedDmrUser>, String> {
    let priority = expand_priority(priority_countries);

    let total: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM dmr_users")
        .fetch_one(pool)
        .await
        .estr()?;
    let limit = match max_count {
        Some(n) => n.max(0).min(total.0),
        None => total.0,
    };

    let case_sql = priority_case_sql(&priority);
    let sql = format!(
        "SELECT dmr_id, callsign, first_name, last_name, city, state, country, remarks
         FROM dmr_users ORDER BY {case_sql}, dmr_id LIMIT ?"
    );
    let mut q = sqlx::query_as::<_, (
        i64,
        String,
        Option<String>,
        Option<String>,
        Option<String>,
        Option<String>,
        Option<String>,
        Option<String>,
    )>(&sql);
    for c in &priority {
        q = q.bind(c);
    }
    q = q.bind(limit);
    let rows = q.fetch_all(pool).await.estr()?;

    Ok(rows
        .into_iter()
        .map(
            |(dmr_id, callsign, first_name, last_name, city, state, country, remarks)| {
                SelectedDmrUser {
                    dmr_id,
                    callsign,
                    first_name,
                    last_name,
                    city,
                    state,
                    country,
                    remarks,
                }
            },
        )
        .collect())
}

#[tauri::command]
pub async fn export_dmr_users(
    state: State<'_, AppState>,
    path: String,
    max_count: Option<i64>,
    priority_countries: Vec<String>,
) -> Result<DmrExportSummary, String> {
    let rows = select_prioritized_dmr_users(&state.pool, max_count, &priority_countries).await?;

    let mut wtr = csv::Writer::from_writer(vec![]);
    wtr.write_record(["DMR ID", "Callsign", "Name", "City", "State", "Country", "Remarks"])
        .estr()?;
    for u in &rows {
        wtr.write_record([
            u.dmr_id.to_string(),
            u.callsign.clone(),
            u.display_name(),
            u.city.clone().unwrap_or_default(),
            u.state.clone().unwrap_or_default(),
            u.country.clone().unwrap_or_default(),
            u.remarks.clone().unwrap_or_default(),
        ])
        .estr()?;
    }
    let csv_bytes = wtr.into_inner().map_err(|e| e.to_string())?;
    std::fs::write(&path, csv_bytes).map_err(|e| format!("Could not write file: {e}"))?;

    Ok(DmrExportSummary {
        written: rows.len() as i64,
    })
}

/// Country -> continent reference table used to expand a continent name into
/// its member countries when building a priority order. Country names match
/// the free-text values radioid.net's dump uses (standard English short
/// names); values that don't exactly match a row here simply won't expand,
/// they're still usable as plain country priorities.
const COUNTRY_CONTINENTS: &[(&str, &str)] = &[
    // ---- Africa ----
    ("Algeria", "Africa"),
    ("Angola", "Africa"),
    ("Benin", "Africa"),
    ("Botswana", "Africa"),
    ("Burkina Faso", "Africa"),
    ("Burundi", "Africa"),
    ("Cabo Verde", "Africa"),
    ("Cameroon", "Africa"),
    ("Central African Republic", "Africa"),
    ("Chad", "Africa"),
    ("Comoros", "Africa"),
    ("Congo", "Africa"),
    ("Democratic Republic of the Congo", "Africa"),
    ("Djibouti", "Africa"),
    ("Egypt", "Africa"),
    ("Equatorial Guinea", "Africa"),
    ("Eritrea", "Africa"),
    ("Eswatini", "Africa"),
    ("Ethiopia", "Africa"),
    ("Gabon", "Africa"),
    ("Gambia", "Africa"),
    ("Ghana", "Africa"),
    ("Guinea", "Africa"),
    ("Guinea-Bissau", "Africa"),
    ("Ivory Coast", "Africa"),
    ("Kenya", "Africa"),
    ("Lesotho", "Africa"),
    ("Liberia", "Africa"),
    ("Libya", "Africa"),
    ("Madagascar", "Africa"),
    ("Malawi", "Africa"),
    ("Mali", "Africa"),
    ("Mauritania", "Africa"),
    ("Mauritius", "Africa"),
    ("Morocco", "Africa"),
    ("Mozambique", "Africa"),
    ("Namibia", "Africa"),
    ("Niger", "Africa"),
    ("Nigeria", "Africa"),
    ("Rwanda", "Africa"),
    ("Sao Tome and Principe", "Africa"),
    ("Senegal", "Africa"),
    ("Seychelles", "Africa"),
    ("Sierra Leone", "Africa"),
    ("Somalia", "Africa"),
    ("South Africa", "Africa"),
    ("South Sudan", "Africa"),
    ("Sudan", "Africa"),
    ("Tanzania", "Africa"),
    ("Togo", "Africa"),
    ("Tunisia", "Africa"),
    ("Uganda", "Africa"),
    ("Zambia", "Africa"),
    ("Zimbabwe", "Africa"),
    // ---- Asia ----
    ("Afghanistan", "Asia"),
    ("Armenia", "Asia"),
    ("Azerbaijan", "Asia"),
    ("Bahrain", "Asia"),
    ("Bangladesh", "Asia"),
    ("Bhutan", "Asia"),
    ("Brunei", "Asia"),
    ("Cambodia", "Asia"),
    ("China", "Asia"),
    ("Cyprus", "Asia"),
    ("Georgia", "Asia"),
    ("Hong Kong", "Asia"),
    ("India", "Asia"),
    ("Indonesia", "Asia"),
    ("Iran", "Asia"),
    ("Iraq", "Asia"),
    ("Israel", "Asia"),
    ("Japan", "Asia"),
    ("Jordan", "Asia"),
    ("Kazakhstan", "Asia"),
    ("Kuwait", "Asia"),
    ("Kyrgyzstan", "Asia"),
    ("Laos", "Asia"),
    ("Lebanon", "Asia"),
    ("Macau", "Asia"),
    ("Malaysia", "Asia"),
    ("Maldives", "Asia"),
    ("Mongolia", "Asia"),
    ("Myanmar", "Asia"),
    ("Nepal", "Asia"),
    ("North Korea", "Asia"),
    ("Oman", "Asia"),
    ("Pakistan", "Asia"),
    ("Palestine", "Asia"),
    ("Philippines", "Asia"),
    ("Qatar", "Asia"),
    ("Saudi Arabia", "Asia"),
    ("Singapore", "Asia"),
    ("South Korea", "Asia"),
    ("Sri Lanka", "Asia"),
    ("Syria", "Asia"),
    ("Taiwan", "Asia"),
    ("Tajikistan", "Asia"),
    ("Thailand", "Asia"),
    ("Timor-Leste", "Asia"),
    ("Turkey", "Asia"),
    ("Turkmenistan", "Asia"),
    ("United Arab Emirates", "Asia"),
    ("Uzbekistan", "Asia"),
    ("Vietnam", "Asia"),
    ("Yemen", "Asia"),
    // ---- Europe ----
    ("Albania", "Europe"),
    ("Andorra", "Europe"),
    ("Austria", "Europe"),
    ("Belarus", "Europe"),
    ("Belgium", "Europe"),
    ("Bosnia and Herzegovina", "Europe"),
    ("Bulgaria", "Europe"),
    ("Croatia", "Europe"),
    ("Czechia", "Europe"),
    ("Czech Republic", "Europe"),
    ("Denmark", "Europe"),
    ("Estonia", "Europe"),
    ("Finland", "Europe"),
    ("France", "Europe"),
    ("Germany", "Europe"),
    ("Greece", "Europe"),
    ("Hungary", "Europe"),
    ("Iceland", "Europe"),
    ("Ireland", "Europe"),
    ("Italy", "Europe"),
    ("Kosovo", "Europe"),
    ("Latvia", "Europe"),
    ("Liechtenstein", "Europe"),
    ("Lithuania", "Europe"),
    ("Luxembourg", "Europe"),
    ("Malta", "Europe"),
    ("Moldova", "Europe"),
    ("Monaco", "Europe"),
    ("Montenegro", "Europe"),
    ("Netherlands", "Europe"),
    ("North Macedonia", "Europe"),
    ("Norway", "Europe"),
    ("Poland", "Europe"),
    ("Portugal", "Europe"),
    ("Romania", "Europe"),
    ("Russia", "Europe"),
    ("San Marino", "Europe"),
    ("Serbia", "Europe"),
    ("Slovakia", "Europe"),
    ("Slovenia", "Europe"),
    ("Spain", "Europe"),
    ("Sweden", "Europe"),
    ("Switzerland", "Europe"),
    ("Ukraine", "Europe"),
    ("United Kingdom", "Europe"),
    ("Vatican City", "Europe"),
    // ---- North America ----
    ("Antigua and Barbuda", "North America"),
    ("Bahamas", "North America"),
    ("Barbados", "North America"),
    ("Belize", "North America"),
    ("Canada", "North America"),
    ("Costa Rica", "North America"),
    ("Cuba", "North America"),
    ("Dominica", "North America"),
    ("Dominican Republic", "North America"),
    ("El Salvador", "North America"),
    ("Grenada", "North America"),
    ("Guatemala", "North America"),
    ("Haiti", "North America"),
    ("Honduras", "North America"),
    ("Jamaica", "North America"),
    ("Mexico", "North America"),
    ("Nicaragua", "North America"),
    ("Panama", "North America"),
    ("Saint Kitts and Nevis", "North America"),
    ("Saint Lucia", "North America"),
    ("Saint Vincent and the Grenadines", "North America"),
    ("Trinidad and Tobago", "North America"),
    ("United States", "North America"),
    // ---- South America ----
    ("Argentina", "South America"),
    ("Bolivia", "South America"),
    ("Brazil", "South America"),
    ("Chile", "South America"),
    ("Colombia", "South America"),
    ("Ecuador", "South America"),
    ("Guyana", "South America"),
    ("Paraguay", "South America"),
    ("Peru", "South America"),
    ("Suriname", "South America"),
    ("Uruguay", "South America"),
    ("Venezuela", "South America"),
    // ---- Oceania ----
    ("Australia", "Oceania"),
    ("Fiji", "Oceania"),
    ("Kiribati", "Oceania"),
    ("Marshall Islands", "Oceania"),
    ("Micronesia", "Oceania"),
    ("Nauru", "Oceania"),
    ("New Zealand", "Oceania"),
    ("Palau", "Oceania"),
    ("Papua New Guinea", "Oceania"),
    ("Samoa", "Oceania"),
    ("Solomon Islands", "Oceania"),
    ("Tonga", "Oceania"),
    ("Tuvalu", "Oceania"),
    ("Vanuatu", "Oceania"),
];

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::Acquire;

    async fn test_pool() -> sqlx::SqlitePool {
        use std::sync::atomic::{AtomicU32, Ordering};
        static SEQ: AtomicU32 = AtomicU32::new(0);
        // Unique per call so tests sharing this process don't collide on one file.
        let n = SEQ.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "cpm_dmr_users_test_{}_{n}",
            std::process::id()
        ));
        let db_path = dir.join("test.sqlite3");
        let _ = std::fs::remove_file(&db_path);
        crate::db::init_pool(&db_path).await.expect("init_pool")
    }

    fn user(id: i64, callsign: &str, country: &str) -> RadioIdUser {
        RadioIdUser {
            id,
            callsign: callsign.to_string(),
            fname: Some("Test".to_string()),
            surname: Some("Op".to_string()),
            city: Some("Anytown".to_string()),
            state: Some("".to_string()), // blank -> should normalize to None
            country: Some(country.to_string()),
            remarks: None,
        }
    }

    #[tokio::test]
    async fn upsert_adds_then_updates_on_reimport() {
        let pool = test_pool().await;
        let mut conn = pool.acquire().await.unwrap();

        let batch1 = vec![user(3112345, "W0CPH", "United States"), user(2345678, "G0ABC", "United Kingdom")];
        let mut tx = conn.begin().await.unwrap();
        let (added, updated) = upsert_dmr_users(&mut tx, &batch1).await.unwrap();
        tx.commit().await.unwrap();
        assert_eq!((added, updated), (2, 0));

        let count: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM dmr_users")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(count.0, 2);

        // Re-importing the same dmr_id with a changed callsign, plus one new
        // id, should update the first and add the second - never duplicate.
        let mut changed = user(3112345, "W0CPH-2", "United States");
        changed.state = Some("CO".to_string());
        let batch2 = vec![changed, user(9999999, "NEWCALL", "Canada")];
        let mut tx = conn.begin().await.unwrap();
        let (added2, updated2) = upsert_dmr_users(&mut tx, &batch2).await.unwrap();
        tx.commit().await.unwrap();
        assert_eq!((added2, updated2), (1, 1));

        let count2: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM dmr_users")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(count2.0, 3, "no duplicate row for the re-imported dmr_id");

        let row: (String, Option<String>) =
            sqlx::query_as("SELECT callsign, state FROM dmr_users WHERE dmr_id = 3112345")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(row.0, "W0CPH-2");
        assert_eq!(row.1.as_deref(), Some("CO"));
    }

    #[tokio::test]
    async fn select_prioritized_caps_and_orders_by_priority() {
        let pool = test_pool().await;
        let mut conn = pool.acquire().await.unwrap();
        let batch = vec![
            user(1000001, "US1", "United States"),
            user(1000002, "US2", "United States"),
            user(2000001, "GB1", "United Kingdom"),
            user(3000001, "CA1", "Canada"),
        ];
        let mut tx = conn.begin().await.unwrap();
        upsert_dmr_users(&mut tx, &batch).await.unwrap();
        tx.commit().await.unwrap();

        // Priority UK first, cap at 2: the single UK op then the lowest-dmr-id
        // filler (US1), never a third row.
        let picked = select_prioritized_dmr_users(&pool, Some(2), &["United Kingdom".to_string()])
            .await
            .unwrap();
        assert_eq!(picked.len(), 2);
        assert_eq!(picked[0].callsign, "GB1", "priority country ranks first");
        assert_eq!(picked[1].callsign, "US1", "then ascending dmr_id fills the cap");
        // Field mapping the radio writer relies on: display name = first+last.
        assert_eq!(picked[0].display_name(), "Test Op");

        // No cap, no priority: every row, ascending dmr_id.
        let all = select_prioritized_dmr_users(&pool, None, &[]).await.unwrap();
        assert_eq!(all.len(), 4);
        assert_eq!(all[0].dmr_id, 1000001);
    }

    #[test]
    fn norm_blanks_out_empty_strings() {
        assert_eq!(norm(Some("  ".to_string())), None);
        assert_eq!(norm(Some("".to_string())), None);
        assert_eq!(norm(Some(" Colorado ".to_string())), Some("Colorado".to_string()));
        assert_eq!(norm(None), None);
    }

    #[test]
    fn expand_priority_expands_continents_and_dedupes() {
        let expanded = expand_priority(&[
            "United States".to_string(),
            "North America".to_string(), // should not re-add United States
            "Germany".to_string(),
        ]);
        assert_eq!(expanded[0], "United States");
        assert!(expanded.contains(&"Canada".to_string()));
        assert!(expanded.contains(&"Germany".to_string()));
        // United States must appear exactly once even though both the plain
        // entry and the continent expansion could have produced it.
        assert_eq!(
            expanded.iter().filter(|c| *c == "United States").count(),
            1
        );
    }

    #[test]
    fn expand_priority_is_case_insensitive_on_continent_names() {
        let expanded = expand_priority(&["oceania".to_string()]);
        assert!(expanded.contains(&"Australia".to_string()));
        assert!(expanded.contains(&"New Zealand".to_string()));
    }

    #[test]
    fn priority_case_sql_ranks_in_order_with_fallback() {
        let sql = priority_case_sql(&["United States".to_string(), "Canada".to_string()]);
        assert_eq!(
            sql,
            "CASE country WHEN ? THEN 0 WHEN ? THEN 1 ELSE 2 END"
        );
        assert_eq!(priority_case_sql(&[]), "NULL");
    }

    #[test]
    fn radioid_dump_shape_deserializes() {
        let json = r#"{"count": 2, "users": [
            {"id": 9999001, "callsign": "QQ0AAA", "fname": "Example", "surname": "Operator",
             "city": "Anytown", "state": "ZZ", "country": "Example Country", "remarks": ""},
            {"id": 9999002, "callsign": "QQ0BBB", "fname": "", "surname": "",
             "city": "", "state": "", "country": "Second Country", "remarks": null}
        ]}"#;
        let dump: RadioIdDump = serde_json::from_str(json).unwrap();
        assert_eq!(dump.users.len(), 2);
        assert_eq!(dump.users[0].callsign, "QQ0AAA");
        assert_eq!(dump.users[1].country.as_deref(), Some("Second Country"));
    }

    /// The full field set the live `https://database.radioid.net/static/users.json`
    /// dump sends, so the shape is exercised end to end: it confirms the
    /// top-level key is `users` (not `results`, which an older third-party
    /// script implied) and that the extra fields (`name`, `radio_id`,
    /// `has_valid_callsign`, ...) are harmlessly ignored by serde's default
    /// (non-strict) deserialization.
    ///
    /// The VALUES are synthetic on purpose. A radioid.net record is a named
    /// person's licence record, and a call sign resolves through the national
    /// licence database to a mailing address, so no real record is checked in
    /// here — the call sign uses the Q prefix block (reserved for Q-codes, never
    /// issued to a station) and the DMR ID sits above the assigned user range.
    #[test]
    fn radioid_full_field_set_deserializes() {
        let json = r#"{"users":[{"name":"Example Operator","fname":"Example Operator","surname":"",
            "country":"Example Country","callsign":"QQ0CCC","city":"Anytown","radio_id":9999003,
            "id":9999003,"state":"Example Province","has_valid_callsign":"1","has_valid_email":0,
            "account_unvalidated":0,"account_validation_status":"validated"}]}"#;
        let dump: RadioIdDump = serde_json::from_str(json).unwrap();
        assert_eq!(dump.users.len(), 1);
        let u = &dump.users[0];
        assert_eq!(u.id, 9999003);
        assert_eq!(u.callsign, "QQ0CCC");
        assert_eq!(u.city.as_deref(), Some("Anytown"));
        assert_eq!(u.state.as_deref(), Some("Example Province"));
        assert_eq!(u.country.as_deref(), Some("Example Country"));
        assert_eq!(norm(u.surname.clone()), None, "blank surname normalizes to None");
    }
}
