use std::collections::HashMap;

use sqlx::{Acquire, QueryBuilder};
use tauri::State;

use crate::db::AppState;
use crate::error::MapErrString;
use crate::models::{Channel, ChannelFilter, ChannelInput, CityCentroid};
use crate::util::{
    derive_band, derive_duplex, derive_tone_mode, differs_from_rb_f64, differs_from_rb_str,
    gen_name_long, gen_name_short, keeps_dcs, normalize_dstar_call, truncate,
};

/// Normalize a search term for matching against frequency text. If the term
/// looks like a decimal number (e.g. "445.050"), strip trailing zeros and any
/// trailing decimal point so it matches the stored "445.05". Non-numeric terms
/// (text searches) are returned unchanged.
fn normalize_freq_term(term: &str) -> String {
    // Only normalize when the term is a plain decimal number with a fractional
    // part — that's the case where a trailing zero would break the match.
    let is_decimal =
        term.contains('.') && term.chars().all(|c| c.is_ascii_digit() || c == '.');
    if !is_decimal {
        return term.to_string();
    }
    let trimmed = term.trim_end_matches('0');
    trimmed.trim_end_matches('.').to_string()
}

const CHANNEL_COLUMNS: &str = "id, rb_name, name_long, name_short, callsign, rx_freq, tx_freq, offset, duplex, band, mode, tone_mode, ctcss_uplink, ctcss_downlink, dcs_code, dcs_rx_code, dcs_polarity, cross_mode, power, dmr_color_code, dmr_timeslot, dmr_talkgroup, dstar_capable, dstar_ur_call, dstar_rpt1, dstar_rpt2, ysf_capable, nxdn_capable, p25_capable, p25_nac, m17_capable, m17_can, tetra_capable, allstar_node, echolink_node, irlp_node, wires_node, ares, races, skywarn, canwarn, use_type, operational_status, service_type, city, county, state, country, latitude, longitude, notes, source, repeaterbook_id, rb_ctcss_uplink, rb_ctcss_downlink, rb_operational_status, rb_notes, ctcss_uplink_overridden, ctcss_downlink_overridden, operational_status_overridden, notes_overridden, has_overrides, last_rb_update, last_user_edit, created_at";

#[tauri::command]
pub async fn list_channels(
    state: State<'_, AppState>,
    filter: Option<ChannelFilter>,
) -> Result<Vec<Channel>, String> {
    let filter = filter.unwrap_or_default();
    let mut qb: QueryBuilder<sqlx::Sqlite> =
        QueryBuilder::new(format!("SELECT {CHANNEL_COLUMNS} FROM channels WHERE 1=1"));

    if let Some(band) = filter.band.filter(|s| !s.is_empty()) {
        qb.push(" AND band = ").push_bind(band);
    }
    if let Some(mode) = filter.mode.filter(|s| !s.is_empty()) {
        qb.push(" AND mode = ").push_bind(mode);
    }
    if let Some(st) = filter.state.filter(|s| !s.is_empty()) {
        qb.push(" AND state = ").push_bind(st);
    }
    if let Some(status) = filter.operational_status.filter(|s| !s.is_empty()) {
        qb.push(" AND operational_status = ").push_bind(status);
    }
    if let Some(src) = filter.source.filter(|s| !s.is_empty()) {
        qb.push(" AND source = ").push_bind(src);
    }
    if let Some(search) = filter.search.filter(|s| !s.trim().is_empty()) {
        let term = search.trim();
        let pat = format!("%{}%", term);
        // Frequencies are stored as f64 MHz, so CAST(445.05 AS TEXT) is "445.05"
        // with no trailing zeros. Users often type the full "445.050", so strip
        // trailing zeros from the typed term before matching against frequencies.
        let freq_pat = format!("%{}%", normalize_freq_term(term));
        // Match text fields plus the RX/TX frequency (cast to text so partial
        // numeric searches like "447.275" or "275" work).
        qb.push(" AND (name_long LIKE ")
            .push_bind(pat.clone())
            .push(" OR name_short LIKE ")
            .push_bind(pat.clone())
            .push(" OR callsign LIKE ")
            .push_bind(pat.clone())
            .push(" OR city LIKE ")
            .push_bind(pat.clone())
            .push(" OR CAST(rx_freq AS TEXT) LIKE ")
            .push_bind(freq_pat.clone())
            .push(" OR CAST(tx_freq AS TEXT) LIKE ")
            .push_bind(freq_pat)
            .push(")");
    }

    qb.push(" ORDER BY state, city, rx_freq");

    qb.build_query_as::<Channel>()
        .fetch_all(&state.pool)
        .await
        .estr()
}

/// Distinct cities (with representative coordinates) for the "distance from"
/// origin picker. Averages the coordinates of all repeaters in each city.
#[tauri::command]
pub async fn list_cities(state: State<'_, AppState>) -> Result<Vec<CityCentroid>, String> {
    fetch_centroids(&state.pool).await
}

/// City centroids (averaged coordinates per town) from repeaters that already
/// have coordinates. Shared by `list_cities` and the bulk backfill.
async fn fetch_centroids(pool: &sqlx::SqlitePool) -> Result<Vec<CityCentroid>, String> {
    sqlx::query_as::<_, CityCentroid>(
        r#"
        SELECT city,
               state,
               AVG(latitude) AS latitude,
               AVG(longitude) AS longitude,
               COUNT(*) AS count
        FROM channels
        WHERE city IS NOT NULL AND city <> ''
          AND latitude IS NOT NULL AND longitude IS NOT NULL
        GROUP BY LOWER(city), LOWER(COALESCE(state, ''))
        ORDER BY city
        "#,
    )
    .fetch_all(pool)
    .await
    .estr()
}

/// A single lat/lon pair returned to the frontend for auto-filling coordinates.
#[derive(serde::Serialize)]
pub struct GeoPoint {
    pub latitude: f64,
    pub longitude: f64,
}

#[derive(serde::Deserialize)]
struct NominatimResult {
    lat: String,
    lon: String,
}

/// Build an HTTP client whose User-Agent identifies us — Nominatim's usage
/// policy requires it. The version comes from the crate so a build OSM blocks
/// for misbehaving does not take every later build down with it.
///
/// Timeouts for the same reason the radioid.net client has them (#78):
/// `reqwest`'s default client has none at all, so a captive portal or a
/// half-open connection leaves a bulk backfill hung on one town with no cancel.
/// A backfill walks hundreds of rows, so the per-request budget is short.
fn geocoder_client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .user_agent(concat!(
            "WW8LCodeplugMagic/",
            env!("CARGO_PKG_VERSION"),
            " (amateur-radio codeplug editor)"
        ))
        .connect_timeout(std::time::Duration::from_secs(10))
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .map_err(|e| format!("HTTP client error: {e}"))
}

/// One Nominatim lookup. Returns the best match's coordinates, or `None` if the
/// service found nothing. Shared by the single-field geocoder and the bulk
/// backfill so both behave identically.
async fn geocode_nominatim(
    client: &reqwest::Client,
    city: &str,
    state: Option<&str>,
    country: Option<&str>,
) -> Result<Option<GeoPoint>, String> {
    // Structured query: city (+ optional state/country) narrows the match.
    let mut query: Vec<(&str, String)> = vec![
        ("format", "jsonv2".into()),
        ("limit", "1".into()),
        ("city", city.to_string()),
    ];
    if let Some(s) = state.map(str::trim).filter(|s| !s.is_empty()) {
        query.push(("state", s.to_string()));
    }
    if let Some(c) = country.map(str::trim).filter(|c| !c.is_empty()) {
        query.push(("country", c.to_string()));
    }

    let resp = client
        .get("https://nominatim.openstreetmap.org/search")
        .query(&query)
        .send()
        .await
        .map_err(|e| format!("Could not reach the geocoder: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("Geocoder returned HTTP {}", resp.status()));
    }
    let results: Vec<NominatimResult> = resp
        .json()
        .await
        .map_err(|e| format!("Could not parse geocoder response: {e}"))?;

    let Some(first) = results.first() else {
        return Ok(None);
    };
    let latitude = first
        .lat
        .parse::<f64>()
        .map_err(|_| "Geocoder returned a bad latitude".to_string())?;
    let longitude = first
        .lon
        .parse::<f64>()
        .map_err(|_| "Geocoder returned a bad longitude".to_string())?;
    Ok(Some(GeoPoint {
        latitude,
        longitude,
    }))
}

/// Geocode a town via OpenStreetMap's Nominatim service. Returns the best
/// match's coordinates, or `None` if nothing was found. This is the online
/// fallback used when a manually entered channel's city isn't already present
/// in the local data (whose centroids `list_cities` serves instantly offline).
#[tauri::command]
pub async fn geocode_city(
    city: String,
    state: Option<String>,
    country: Option<String>,
) -> Result<Option<GeoPoint>, String> {
    let city = city.trim();
    if city.is_empty() {
        return Ok(None);
    }
    let client = geocoder_client()?;
    geocode_nominatim(&client, city, state.as_deref(), country.as_deref()).await
}

/// Outcome of a bulk coordinate backfill, for a summary toast.
#[derive(serde::Serialize)]
pub struct BackfillResult {
    /// Channels that had a city but no coordinates — the work set.
    pub scanned: usize,
    /// Filled from a centroid of the user's own repeaters (offline).
    pub from_data: usize,
    /// Filled from OpenStreetMap.
    pub from_online: usize,
    /// City present but neither source could locate it.
    pub not_found: usize,
    /// A lookup errored (e.g. the geocoder was unreachable).
    pub failed: usize,
}

/// Find a matching centroid among the user's own repeaters: exact city, and
/// exact state when the channel has one. When several towns share a name, the
/// one backed by the most repeaters wins — mirrors the create-form auto-fill.
fn match_centroid<'a>(
    centroids: &'a [CityCentroid],
    city: &str,
    state: Option<&str>,
) -> Option<&'a CityCentroid> {
    let want_city = city.trim().to_lowercase();
    let want_state = state.unwrap_or("").trim().to_lowercase();
    centroids
        .iter()
        .filter(|c| {
            c.city.to_lowercase() == want_city
                && (want_state.is_empty()
                    || c.state.as_deref().unwrap_or("").to_lowercase() == want_state)
        })
        .max_by_key(|c| c.count)
}

/// Backfill coordinates for every channel that has a city but no lat/lon.
/// Prefers an offline centroid from the user's own repeaters; otherwise falls
/// back to the online geocoder (throttled to respect Nominatim's usage policy).
/// Never touches channels that already have coordinates.
#[tauri::command]
pub async fn backfill_channel_coordinates(
    state: State<'_, AppState>,
) -> Result<BackfillResult, String> {
    backfill_impl(&state.pool).await
}

async fn backfill_impl(pool: &sqlx::SqlitePool) -> Result<BackfillResult, String> {
    let client = geocoder_client()?;
    backfill_with(pool, |city, st, country| {
        let client = client.clone();
        async move {
            geocode_nominatim(&client, &city, st.as_deref(), country.as_deref()).await
        }
    })
    .await
}

/// The backfill itself, with the online lookup injected so a test can count how
/// many times it is actually called.
async fn backfill_with<L, F>(
    pool: &sqlx::SqlitePool,
    mut lookup: L,
) -> Result<BackfillResult, String>
where
    L: FnMut(String, Option<String>, Option<String>) -> F,
    F: std::future::Future<Output = Result<Option<GeoPoint>, String>>,
{
    // Offline centroids from repeaters that already have coordinates.
    let centroids = fetch_centroids(pool).await?;

    // The work set: a city to geocode, but no coordinates yet.
    let targets: Vec<(i64, String, Option<String>, Option<String>)> = sqlx::query_as(
        r#"
        SELECT id, city, state, country
        FROM channels
        WHERE city IS NOT NULL AND TRIM(city) <> ''
          AND (latitude IS NULL OR longitude IS NULL)
        ORDER BY id
        "#,
    )
    .fetch_all(pool)
    .await
    .estr()?;

    let mut result = BackfillResult {
        scanned: targets.len(),
        from_data: 0,
        from_online: 0,
        not_found: 0,
        failed: 0,
    };
    let mut did_online = false;

    // Every town this run has already resolved online, and every town it has
    // already been told does not exist. Without it a 60-repeater import for one
    // city sent Nominatim 60 identical queries: `centroids` is read once, before
    // the loop, so rows geocoded during the run could never feed the offline
    // short-circuit. Nominatim's usage policy names repeated identical queries
    // as grounds for blocking the User-Agent — which is shared by every user of
    // the app, not just the one who started the backfill.
    let mut seen: HashMap<(String, String), Option<(f64, f64)>> = HashMap::new();

    for (id, city, st, country) in targets {
        // 1) Offline centroid from the user's own data — instant, no network.
        if let Some(hit) = match_centroid(&centroids, &city, st.as_deref()) {
            let (lat, lon) = (hit.latitude, hit.longitude);
            if update_coords(pool, id, lat, lon).await.is_ok() {
                result.from_data += 1;
            } else {
                result.failed += 1;
            }
            continue;
        }

        // 2) A town another channel in this same run already looked up.
        let key = (
            city.trim().to_lowercase(),
            st.as_deref().unwrap_or("").trim().to_lowercase(),
        );
        if let Some(cached) = seen.get(&key) {
            match *cached {
                Some((lat, lon)) => {
                    if update_coords(pool, id, lat, lon).await.is_ok() {
                        result.from_online += 1;
                    } else {
                        result.failed += 1;
                    }
                }
                None => result.not_found += 1,
            }
            continue;
        }

        // 3) Online geocoder. Space requests ~1s apart per Nominatim's policy.
        if did_online {
            tokio::time::sleep(std::time::Duration::from_millis(1100)).await;
        }
        did_online = true;
        match lookup(city.clone(), st.clone(), country.clone()).await {
            Ok(Some(pt)) => {
                seen.insert(key, Some((pt.latitude, pt.longitude)));
                if update_coords(pool, id, pt.latitude, pt.longitude)
                    .await
                    .is_ok()
                {
                    result.from_online += 1;
                } else {
                    result.failed += 1;
                }
            }
            Ok(None) => {
                seen.insert(key, None);
                result.not_found += 1;
            }
            // A transport error is not an answer about the town — leave it
            // uncached so a later channel can retry it.
            Err(_) => result.failed += 1,
        }
    }

    Ok(result)
}

/// Write just the coordinates onto one channel.
async fn update_coords(
    pool: &sqlx::SqlitePool,
    id: i64,
    lat: f64,
    lon: f64,
) -> Result<(), String> {
    sqlx::query("UPDATE channels SET latitude = ?1, longitude = ?2 WHERE id = ?3")
        .bind(lat)
        .bind(lon)
        .bind(id)
        .execute(pool)
        .await
        .estr()
        .map(|_| ())
}

#[tauri::command]
pub async fn get_channel(state: State<'_, AppState>, id: i64) -> Result<Channel, String> {
    fetch_channel(&state.pool, id).await
}

/// `get_channel` without a Tauri handle, so the command implementations (and
/// their tests) can read a row back.
async fn fetch_channel(pool: &sqlx::SqlitePool, id: i64) -> Result<Channel, String> {
    sqlx::query_as::<_, Channel>(&format!(
        "SELECT {CHANNEL_COLUMNS} FROM channels WHERE id = ?1"
    ))
    .bind(id)
    .fetch_one(pool)
    .await
    .estr()
}

#[tauri::command]
pub async fn create_channel(
    state: State<'_, AppState>,
    input: ChannelInput,
) -> Result<Channel, String> {
    let band = derive_band(input.rx_freq);
    let (duplex, offset) = derive_duplex(input.rx_freq, input.tx_freq);

    // Auto-generate names if the user did not supply them.
    let callsign = input.callsign.clone().unwrap_or_default();
    let city = input.city.clone().unwrap_or_default();
    let name_long = input
        .name_long
        .clone()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| gen_name_long(&callsign, &city));
    let name_short = input
        .name_short
        .clone()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| gen_name_short(&callsign));

    // Manual channels carry whatever provenance the user typed; blank → "manual".
    let source = input
        .source
        .clone()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| "manual".to_string());

    let id = sqlx::query(
        r#"
        INSERT INTO channels (
            name_long, name_short, callsign, rx_freq, tx_freq, offset, duplex,
            band, mode, tone_mode, ctcss_uplink, ctcss_downlink, dcs_code,
            dmr_color_code, dmr_timeslot, dmr_talkgroup,
            dstar_capable, ysf_capable, nxdn_capable, p25_capable, p25_nac,
            m17_capable, m17_can, use_type, operational_status, service_type,
            city, county, state, country, latitude, longitude, notes,
            dcs_rx_code, dcs_polarity, cross_mode, power,
            dstar_ur_call, dstar_rpt1, dstar_rpt2,
            source, last_user_edit
        ) VALUES (
            ?1, ?2, ?3, ?4, ?5, ?6, ?7,
            ?8, ?9, ?10, ?11, ?12, ?13,
            ?14, ?15, ?16,
            ?17, ?18, ?19, ?20, ?21,
            ?22, ?23, ?24, ?25, ?26,
            ?27, ?28, ?29, ?30, ?31, ?32, ?33,
            ?34, ?35, ?36, ?37,
            ?38, ?39, ?40,
            ?41, CURRENT_TIMESTAMP
        )
        "#,
    )
    .bind(&name_long)
    .bind(&name_short)
    .bind(&input.callsign)
    .bind(input.rx_freq)
    .bind(input.tx_freq)
    .bind(offset)
    .bind(&duplex)
    .bind(band)
    .bind(&input.mode)
    .bind(&input.tone_mode)
    .bind(input.ctcss_uplink)
    .bind(input.ctcss_downlink)
    .bind(&input.dcs_code)
    .bind(input.dmr_color_code)
    .bind(input.dmr_timeslot)
    .bind(input.dmr_talkgroup)
    .bind(input.dstar_capable)
    .bind(input.ysf_capable)
    .bind(input.nxdn_capable)
    .bind(input.p25_capable)
    .bind(&input.p25_nac)
    .bind(input.m17_capable)
    .bind(input.m17_can)
    .bind(&input.use_type)
    .bind(&input.operational_status)
    .bind(&input.service_type)
    .bind(&input.city)
    .bind(&input.county)
    .bind(&input.state)
    .bind(input.country.clone().unwrap_or_else(|| "United States".to_string()))
    .bind(input.latitude)
    .bind(input.longitude)
    .bind(&input.notes)
    .bind(&input.dcs_rx_code)
    .bind(&input.dcs_polarity)
    .bind(&input.cross_mode)
    .bind(&input.power)
    .bind(normalize_dstar_call(input.dstar_ur_call.as_deref()))
    .bind(normalize_dstar_call(input.dstar_rpt1.as_deref()))
    .bind(normalize_dstar_call(input.dstar_rpt2.as_deref()))
    .bind(&source)
    .execute(&state.pool)
    .await
    .estr()?
    .last_insert_rowid();

    get_channel(state, id).await
}

#[tauri::command]
pub async fn update_channel(
    state: State<'_, AppState>,
    id: i64,
    input: ChannelInput,
) -> Result<Channel, String> {
    update_impl(&state.pool, id, input).await
}

/// The body of [`update_channel`], against a pool rather than a Tauri handle so
/// the override-flag rules can be tested the way the editor actually runs them.
pub(super) async fn update_impl(
    pool: &sqlx::SqlitePool,
    id: i64,
    input: ChannelInput,
) -> Result<Channel, String> {
    // Load the existing row so we can compute override flags against the
    // RepeaterBook-supplied baseline values.
    let existing = sqlx::query_as::<_, Channel>(&format!(
        "SELECT {CHANNEL_COLUMNS} FROM channels WHERE id = ?1"
    ))
    .bind(id)
    .fetch_one(pool)
    .await
    .estr()?;

    let band = derive_band(input.rx_freq);
    let (duplex, offset) = derive_duplex(input.rx_freq, input.tx_freq);

    // An override exists when the user value differs from the RepeaterBook
    // baseline. Only a RepeaterBook-sourced row can ever be re-imported, so the
    // flags are meaningless — and would show a spurious "overridden" badge — on
    // a manually created channel; those never override.
    //
    // "Differs" is deliberately symmetric (`differs_from_rb_*`): CLEARING a tone
    // RepeaterBook reports is an override just as much as changing it. It used
    // to count only when both sides had a value, so a deliberate clear recorded
    // no override and the next re-import put the tone straight back (issue #86).
    let rb_sourced = existing.repeaterbook_id.is_some();
    let ctcss_up_over =
        rb_sourced && differs_from_rb_f64(input.ctcss_uplink, existing.rb_ctcss_uplink);
    let ctcss_dn_over =
        rb_sourced && differs_from_rb_f64(input.ctcss_downlink, existing.rb_ctcss_downlink);
    let status_over = rb_sourced
        && differs_from_rb_str(&input.operational_status, &existing.rb_operational_status);
    let notes_over = rb_sourced && differs_from_rb_str(&input.notes, &existing.rb_notes);
    let has_overrides = ctcss_up_over || ctcss_dn_over || status_over || notes_over;

    // Keep the existing provenance unless the form supplied a (non-blank) value.
    let source = input
        .source
        .clone()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or(existing.source);

    sqlx::query(
        r#"
        UPDATE channels SET
            name_long = ?2, name_short = ?3, callsign = ?4, rx_freq = ?5,
            tx_freq = ?6, offset = ?7, duplex = ?8, band = ?9, mode = ?10,
            tone_mode = ?11, ctcss_uplink = ?12, ctcss_downlink = ?13,
            dcs_code = ?14, dmr_color_code = ?15, dmr_timeslot = ?16,
            dmr_talkgroup = ?17, dstar_capable = ?18, ysf_capable = ?19,
            nxdn_capable = ?20, p25_capable = ?21, p25_nac = ?22,
            m17_capable = ?23, m17_can = ?24, use_type = ?25,
            operational_status = ?26, service_type = ?27, city = ?28,
            county = ?29, state = ?30, country = ?31, latitude = ?32,
            longitude = ?33, notes = ?34,
            ctcss_uplink_overridden = ?35, ctcss_downlink_overridden = ?36,
            operational_status_overridden = ?37, notes_overridden = ?38,
            has_overrides = ?39,
            dcs_rx_code = ?40, dcs_polarity = ?41, cross_mode = ?42, power = ?43,
            dstar_ur_call = ?45, dstar_rpt1 = ?46, dstar_rpt2 = ?47,
            source = ?44,
            last_user_edit = CURRENT_TIMESTAMP
        WHERE id = ?1
        "#,
    )
    .bind(id)
    .bind(&input.name_long)
    .bind(&input.name_short)
    .bind(&input.callsign)
    .bind(input.rx_freq)
    .bind(input.tx_freq)
    .bind(offset)
    .bind(&duplex)
    .bind(band)
    .bind(&input.mode)
    .bind(&input.tone_mode)
    .bind(input.ctcss_uplink)
    .bind(input.ctcss_downlink)
    .bind(&input.dcs_code)
    .bind(input.dmr_color_code)
    .bind(input.dmr_timeslot)
    .bind(input.dmr_talkgroup)
    .bind(input.dstar_capable)
    .bind(input.ysf_capable)
    .bind(input.nxdn_capable)
    .bind(input.p25_capable)
    .bind(&input.p25_nac)
    .bind(input.m17_capable)
    .bind(input.m17_can)
    .bind(&input.use_type)
    .bind(&input.operational_status)
    .bind(&input.service_type)
    .bind(&input.city)
    .bind(&input.county)
    .bind(&input.state)
    .bind(&input.country)
    .bind(input.latitude)
    .bind(input.longitude)
    .bind(&input.notes)
    .bind(ctcss_up_over)
    .bind(ctcss_dn_over)
    .bind(status_over)
    .bind(notes_over)
    .bind(has_overrides)
    .bind(&input.dcs_rx_code)
    .bind(&input.dcs_polarity)
    .bind(&input.cross_mode)
    .bind(&input.power)
    .bind(&source)
    .bind(normalize_dstar_call(input.dstar_ur_call.as_deref()))
    .bind(normalize_dstar_call(input.dstar_rpt1.as_deref()))
    .bind(normalize_dstar_call(input.dstar_rpt2.as_deref()))
    .execute(pool)
    .await
    .estr()?;

    fetch_channel(pool, id).await
}

/// Build the display name for a copy: the original with "Copy " in front,
/// trimmed to the radio's `max` character budget ("W0CPH Denver" →
/// "Copy W0CPH Denv"). `taken` holds the lowercased names already in use; when
/// that name is one of them — a second copy of the same channel — a counter is
/// appended so the radio never shows two identical names.
fn copy_name(original: &str, max: usize, taken: &std::collections::HashSet<String>) -> String {
    let original = original.trim();
    if original.is_empty() {
        return String::new();
    }
    let prefixed = truncate(&format!("Copy {original}"), max);
    if !taken.contains(&prefixed.to_lowercase()) {
        return prefixed;
    }
    let mut candidate = prefixed.clone();
    for n in 2..=999 {
        let suffix = format!(" {n}");
        let room = max.saturating_sub(suffix.chars().count());
        candidate = format!("{}{}", truncate(&prefixed, room), suffix);
        if !taken.contains(&candidate.to_lowercase()) {
            return candidate;
        }
    }
    candidate
}

/// Copy a channel — the starting point for a variant of the same repeater (a
/// different tone for an event, a second mode, …). Everything carries over
/// including any DMR talkgroup assignments; the long name is prefixed with
/// "Copy " so the copy is recognizable, and the RepeaterBook link is dropped so
/// a later re-import updates only the original.
#[tauri::command]
pub async fn duplicate_channel(
    state: State<'_, AppState>,
    id: i64,
) -> Result<Channel, String> {
    let new_id = duplicate_impl(&state.pool, id).await?;
    get_channel(state, new_id).await
}

/// Insert the copy and return its id.
async fn duplicate_impl(pool: &sqlx::SqlitePool, id: i64) -> Result<i64, String> {
    let (name_long,): (Option<String>,) =
        sqlx::query_as("SELECT name_long FROM channels WHERE id = ?1")
            .bind(id)
            .fetch_one(pool)
            .await
            .estr()?;

    // Long names already in use, so a repeat copy doesn't land on one of them.
    let existing: Vec<(Option<String>,)> =
        sqlx::query_as("SELECT name_long FROM channels")
            .fetch_all(pool)
            .await
            .estr()?;
    let longs: std::collections::HashSet<String> = existing
        .into_iter()
        .filter_map(|(l,)| l)
        .map(|l| l.trim().to_lowercase())
        .collect();

    // The short name is left alone: at 7 characters a "Copy " prefix would eat
    // the callsign, which is the only thing a narrow radio display has room for.
    let new_long = copy_name(name_long.as_deref().unwrap_or_default(), 16, &longs);

    let mut conn = pool.acquire().await.estr()?;
    let mut tx = conn.begin().await.estr()?;

    // Every column carries over except the identity ones: `id` is fresh,
    // `repeaterbook_id` is dropped (it is UNIQUE, and the copy is the user's own
    // variant), and the timestamps mark this as a new manual edit.
    let new_id = sqlx::query(
        r#"
        INSERT INTO channels (
            name_long, name_short, rb_name, callsign, rx_freq, tx_freq, offset,
            duplex, band, mode, tone_mode, ctcss_uplink, ctcss_downlink,
            dcs_code, dcs_rx_code, dcs_polarity, cross_mode, power,
            dmr_color_code, dmr_timeslot, dmr_talkgroup,
            dstar_capable, dstar_ur_call, dstar_rpt1, dstar_rpt2,
            ysf_capable, nxdn_capable, p25_capable, p25_nac,
            m17_capable, m17_can, tetra_capable,
            allstar_node, echolink_node, irlp_node, wires_node,
            ares, races, skywarn, canwarn,
            use_type, operational_status, service_type,
            city, county, state, country, latitude, longitude, notes, source,
            rb_ctcss_uplink, rb_ctcss_downlink, rb_operational_status, rb_notes,
            ctcss_uplink_overridden, ctcss_downlink_overridden,
            operational_status_overridden, notes_overridden, has_overrides,
            last_rb_update, last_user_edit
        )
        SELECT
            ?2, name_short, rb_name, callsign, rx_freq, tx_freq, offset,
            duplex, band, mode, tone_mode, ctcss_uplink, ctcss_downlink,
            dcs_code, dcs_rx_code, dcs_polarity, cross_mode, power,
            dmr_color_code, dmr_timeslot, dmr_talkgroup,
            dstar_capable, dstar_ur_call, dstar_rpt1, dstar_rpt2,
            ysf_capable, nxdn_capable, p25_capable, p25_nac,
            m17_capable, m17_can, tetra_capable,
            allstar_node, echolink_node, irlp_node, wires_node,
            ares, races, skywarn, canwarn,
            use_type, operational_status, service_type,
            city, county, state, country, latitude, longitude, notes, source,
            rb_ctcss_uplink, rb_ctcss_downlink, rb_operational_status, rb_notes,
            ctcss_uplink_overridden, ctcss_downlink_overridden,
            operational_status_overridden, notes_overridden, has_overrides,
            last_rb_update, CURRENT_TIMESTAMP
        FROM channels WHERE id = ?1
        "#,
    )
    .bind(id)
    .bind(&new_long)
    .execute(&mut *tx)
    .await
    .estr()?
    .last_insert_rowid();

    // A DMR repeater's talkgroups are part of what makes it that repeater, so
    // the copy carries them too.
    sqlx::query(
        r#"
        INSERT INTO repeater_talkgroups (channel_id, talkgroup_id, timeslot, position, name_override)
        SELECT ?1, talkgroup_id, timeslot, position, name_override
        FROM repeater_talkgroups WHERE channel_id = ?2
        "#,
    )
    .bind(new_id)
    .bind(id)
    .execute(&mut *tx)
    .await
    .estr()?;

    tx.commit().await.estr()?;

    Ok(new_id)
}

/// Drop any scan-list priority pointer aimed at `id`.
///
/// `scan_lists.priority_channel_id` and `priority_channel_2_id` are the only
/// channel foreign keys in the schema with no `ON DELETE` clause, so SQLite
/// uses NO ACTION and the delete fails with `FOREIGN KEY constraint failed`.
/// Clearing them first inside the same transaction gives `ON DELETE SET NULL`
/// behaviour without rebuilding the table (SQLite cannot alter a constraint in
/// place, and a rebuild inside a migration transaction cannot turn foreign keys
/// off, which would cascade `scan_list_entries` away with the old table).
async fn clear_priority_refs(
    tx: &mut sqlx::SqliteConnection,
    id: i64,
) -> Result<(), String> {
    sqlx::query(
        "UPDATE scan_lists
            SET priority_channel_id   = CASE WHEN priority_channel_id   = ?1 THEN NULL
                                             ELSE priority_channel_id   END,
                priority_channel_2_id = CASE WHEN priority_channel_2_id = ?1 THEN NULL
                                             ELSE priority_channel_2_id END
          WHERE priority_channel_id = ?1 OR priority_channel_2_id = ?1",
    )
    .bind(id)
    .execute(&mut *tx)
    .await
    .estr()?;
    Ok(())
}

#[tauri::command]
pub async fn delete_channel(state: State<'_, AppState>, id: i64) -> Result<(), String> {
    delete_impl(&state.pool, &[id]).await?;
    Ok(())
}

/// Delete many channels at once (e.g. to redo an import). Returns how many rows
/// were removed.
#[tauri::command]
pub async fn delete_channels(
    state: State<'_, AppState>,
    ids: Vec<i64>,
) -> Result<u64, String> {
    delete_impl(&state.pool, &ids).await
}

/// Delete `ids` in one transaction, dropping any scan-list priority pointer at
/// them first. Shared by the single and bulk commands so neither can forget.
async fn delete_impl(pool: &sqlx::SqlitePool, ids: &[i64]) -> Result<u64, String> {
    if ids.is_empty() {
        return Ok(0);
    }
    let mut conn = pool.acquire().await.estr()?;
    let mut tx = conn.begin().await.estr()?;
    let mut deleted = 0u64;
    for id in ids {
        clear_priority_refs(&mut tx, *id).await?;
        deleted += sqlx::query("DELETE FROM channels WHERE id = ?1")
            .bind(id)
            .execute(&mut *tx)
            .await
            .estr()?
            .rows_affected();
    }
    tx.commit().await.estr()?;
    Ok(deleted)
}

/// Revert a single overridable field back to its RepeaterBook value and clear
/// the corresponding override flag. `field` is one of: ctcss_uplink,
/// ctcss_downlink, operational_status, notes.
#[tauri::command]
pub async fn accept_rb_value(
    state: State<'_, AppState>,
    id: i64,
    field: String,
) -> Result<Channel, String> {
    let sql = match field.as_str() {
        "ctcss_uplink" => {
            "UPDATE channels SET ctcss_uplink = rb_ctcss_uplink, ctcss_uplink_overridden = 0 WHERE id = ?1"
        }
        "ctcss_downlink" => {
            "UPDATE channels SET ctcss_downlink = rb_ctcss_downlink, ctcss_downlink_overridden = 0 WHERE id = ?1"
        }
        "operational_status" => {
            "UPDATE channels SET operational_status = rb_operational_status, operational_status_overridden = 0 WHERE id = ?1"
        }
        "notes" => "UPDATE channels SET notes = rb_notes, notes_overridden = 0 WHERE id = ?1",
        other => return Err(format!("Unknown overridable field: {other}")),
    };

    sqlx::query(sql).bind(id).execute(&state.pool).await.estr()?;

    // Recompute the aggregate has_overrides flag.
    sqlx::query(
        "UPDATE channels SET has_overrides = (ctcss_uplink_overridden OR ctcss_downlink_overridden OR operational_status_overridden OR notes_overridden) WHERE id = ?1",
    )
    .bind(id)
    .execute(&state.pool)
    .await
    .estr()?;

    // Putting a tone back changes which tone scheme the channel is on, and the
    // two have to agree: accepting RepeaterBook's 100.0 Hz onto a channel the
    // operator had set to "off" otherwise leaves a tone every encoder ignores.
    // Re-derived exactly as the importer does, DCS included.
    if field.starts_with("ctcss_") {
        rederive_tone_mode(&state.pool, id).await?;
    }

    get_channel(state, id).await
}

/// Put `tone_mode`/`cross_mode` back in step with the channel's tone pair,
/// leaving a DCS scheme alone (RepeaterBook cannot describe one, so it is the
/// operator's own — see [`keeps_dcs`]).
async fn rederive_tone_mode(pool: &sqlx::SqlitePool, id: i64) -> Result<(), String> {
    let row: (Option<String>, String, Option<f64>, Option<f64>) = sqlx::query_as(
        "SELECT tone_mode, cross_mode, ctcss_uplink, ctcss_downlink FROM channels WHERE id = ?1",
    )
    .bind(id)
    .fetch_one(pool)
    .await
    .estr()?;
    if keeps_dcs(row.0.as_deref(), &row.1) {
        return Ok(());
    }
    let (tone_mode, cross_mode) = derive_tone_mode(row.2, row.3);
    sqlx::query("UPDATE channels SET tone_mode = ?2, cross_mode = ?3 WHERE id = ?1")
        .bind(id)
        .bind(&tone_mode)
        .bind(&cross_mode)
        .execute(pool)
        .await
        .estr()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn freq_term_strips_trailing_zeros() {
        // The user types the full frequency; we match the stored "445.05".
        assert_eq!(normalize_freq_term("445.050"), "445.05");
        assert_eq!(normalize_freq_term("146.520"), "146.52");
        assert_eq!(normalize_freq_term("445.0500"), "445.05");
        // Whole numbers expressed with a fractional part collapse cleanly.
        assert_eq!(normalize_freq_term("445.00"), "445");
        assert_eq!(normalize_freq_term("445."), "445");
    }

    #[test]
    fn freq_term_leaves_other_searches_alone() {
        // No fractional part: nothing to strip.
        assert_eq!(normalize_freq_term("275"), "275");
        // Non-numeric text searches pass through untouched.
        assert_eq!(normalize_freq_term("W7ABC"), "W7ABC");
        assert_eq!(normalize_freq_term("Seattle"), "Seattle");
        // A meaningful trailing digit is preserved.
        assert_eq!(normalize_freq_term("447.275"), "447.275");
    }

    #[test]
    fn centroid_match_respects_state_and_prefers_most_repeaters() {
        let centroids = vec![
            CityCentroid {
                city: "Springfield".into(),
                state: Some("IL".into()),
                latitude: 39.8,
                longitude: -89.6,
                count: 5,
            },
            CityCentroid {
                city: "Springfield".into(),
                state: Some("MO".into()),
                latitude: 37.2,
                longitude: -93.3,
                count: 12,
            },
        ];
        // Exact state wins even when it has fewer repeaters.
        let il = match_centroid(&centroids, "springfield", Some("IL")).unwrap();
        assert_eq!(il.state.as_deref(), Some("IL"));
        // No state given → the town backed by the most repeaters wins.
        let best = match_centroid(&centroids, "Springfield", None).unwrap();
        assert_eq!(best.state.as_deref(), Some("MO"));
        // A state we don't have → no local match (would fall through to online).
        assert!(match_centroid(&centroids, "Springfield", Some("TX")).is_none());
    }

    #[test]
    fn copy_names_are_prefixed_and_fit_the_radio() {
        let mut taken = std::collections::HashSet::new();
        // Short enough to keep whole.
        assert_eq!(copy_name("W0CPH", 16, &taken), "Copy W0CPH");
        // The tail is trimmed so "Copy " still fits the 16-char budget.
        assert_eq!(copy_name("W0CPH Denver", 16, &taken), "Copy W0CPH Denve");
        // A second copy of the same channel counts on instead of colliding.
        taken.insert("copy w0cph denve".to_string());
        assert_eq!(copy_name("W0CPH Denver", 16, &taken), "Copy W0CPH Den 2");
        // A blank name stays blank — nothing to prefix.
        assert_eq!(copy_name("", 16, &taken), "");
    }

    /// A copy carries every field plus the DMR talkgroup assignments, gets a
    /// counted name, and drops the RepeaterBook link so a re-import of the
    /// original never overwrites the user's variant.
    #[tokio::test]
    async fn duplicate_copies_fields_and_talkgroups() {
        let dir = std::env::temp_dir().join(format!("cpm_dup_{}", std::process::id()));
        let db_path = dir.join("test.sqlite3");
        let _ = std::fs::remove_file(&db_path);
        let pool = crate::db::init_pool(&db_path).await.expect("init_pool");

        let src = sqlx::query(
            "INSERT INTO channels (name_long, name_short, callsign, rx_freq, tx_freq,
                 mode, tone_mode, ctcss_uplink, city, state, source, repeaterbook_id)
             VALUES ('W0CPH Denver', 'W0CPH', 'W0CPH', 447.0, 442.0, 'DMR', 'Tone',
                     100.0, 'Denver', 'CO', 'repeaterbook', 'RB|W0CPH|447')",
        )
        .execute(&pool)
        .await
        .unwrap()
        .last_insert_rowid();

        // A number the seeded library doesn't already carry.
        let tg = sqlx::query("INSERT INTO talkgroups (tg_number, name) VALUES (9999901, 'Test TG')")
            .execute(&pool)
            .await
            .unwrap()
            .last_insert_rowid();
        sqlx::query(
            "INSERT INTO repeater_talkgroups (channel_id, talkgroup_id, timeslot) VALUES (?1, ?2, 2)",
        )
        .bind(src)
        .bind(tg)
        .execute(&pool)
        .await
        .unwrap();

        let copy_id = duplicate_impl(&pool, src).await.expect("duplicate");
        assert_ne!(copy_id, src);

        let (long, short, rx, tone, rb_id, source): (
            Option<String>,
            Option<String>,
            f64,
            Option<f64>,
            Option<String>,
            String,
        ) = sqlx::query_as(
            "SELECT name_long, name_short, rx_freq, ctcss_uplink, repeaterbook_id, source
             FROM channels WHERE id = ?1",
        )
        .bind(copy_id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(long.as_deref(), Some("Copy W0CPH Denve"));
        assert_eq!(short.as_deref(), Some("W0CPH"), "short name is left alone");
        assert_eq!(rx, 447.0, "settings carry over untouched");
        assert_eq!(tone, Some(100.0));
        assert_eq!(rb_id, None, "the copy is not the RepeaterBook row");
        assert_eq!(source, "repeaterbook", "provenance is preserved");

        let slots: Vec<(i64, i64)> = sqlx::query_as(
            "SELECT talkgroup_id, timeslot FROM repeater_talkgroups WHERE channel_id = ?1",
        )
        .bind(copy_id)
        .fetch_all(&pool)
        .await
        .unwrap();
        assert_eq!(slots, vec![(tg, 2)], "talkgroup assignments come along");

        // Copying again counts on instead of repeating the same name.
        let second = duplicate_impl(&pool, src).await.expect("duplicate again");
        let (long2,): (Option<String>,) =
            sqlx::query_as("SELECT name_long FROM channels WHERE id = ?1")
                .bind(second)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(long2.as_deref(), Some("Copy W0CPH Den 2"));

        pool.close().await;
        let _ = std::fs::remove_file(&db_path);
    }

    /// End-to-end offline path: a channel with coordinates seeds a centroid, and
    /// backfill fills a coordinate-less channel in the same town from it — no
    /// network, and channels that already have coordinates are left untouched.
    #[tokio::test]
    async fn backfill_fills_from_local_centroid() {
        let dir = std::env::temp_dir().join(format!("cpm_backfill_{}", std::process::id()));
        let db_path = dir.join("test.sqlite3");
        let _ = std::fs::remove_file(&db_path);
        let pool = crate::db::init_pool(&db_path).await.expect("init_pool");

        // A located repeater in Denver, CO → seeds the centroid.
        sqlx::query(
            "INSERT INTO channels (rx_freq, city, state, latitude, longitude)
             VALUES (447.0, 'Denver', 'CO', 39.74, -104.99)",
        )
        .execute(&pool)
        .await
        .unwrap();
        // A second Denver repeater with NO coordinates → the backfill target.
        let target = sqlx::query(
            "INSERT INTO channels (rx_freq, city, state) VALUES (448.0, 'Denver', 'CO')",
        )
        .execute(&pool)
        .await
        .unwrap()
        .last_insert_rowid();

        let res = backfill_impl(&pool).await.expect("backfill");
        assert_eq!(res.scanned, 1, "only the coordinate-less channel is scanned");
        assert_eq!(res.from_data, 1);
        assert_eq!(res.from_online, 0, "local hit must not touch the network");

        let (lat, lon): (Option<f64>, Option<f64>) =
            sqlx::query_as("SELECT latitude, longitude FROM channels WHERE id = ?1")
                .bind(target)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(lat, Some(39.74));
        assert_eq!(lon, Some(-104.99));

        pool.close().await;
        let _ = std::fs::remove_file(&db_path);
    }

    /// Deleting a channel a scan list names as priority 1 or 2 used to fail
    /// with a raw `FOREIGN KEY constraint failed` — those two columns are the
    /// only channel FKs in the schema with no `ON DELETE`. The pointer is
    /// cleared inside the delete's own transaction instead.
    #[tokio::test]
    async fn deleting_a_priority_channel_clears_the_pointer() {
        let dir = std::env::temp_dir().join(format!("cpm_prio_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let db_path = dir.join("prio.sqlite3");
        let _ = std::fs::remove_file(&db_path);
        let pool = crate::db::init_pool(&db_path).await.expect("init_pool");

        let a = sqlx::query("INSERT INTO channels (rx_freq, name_long) VALUES (146.94, 'A')")
            .execute(&pool)
            .await
            .unwrap()
            .last_insert_rowid();
        let b = sqlx::query("INSERT INTO channels (rx_freq, name_long) VALUES (147.12, 'B')")
            .execute(&pool)
            .await
            .unwrap()
            .last_insert_rowid();
        let c = sqlx::query("INSERT INTO channels (rx_freq, name_long) VALUES (145.31, 'C')")
            .execute(&pool)
            .await
            .unwrap()
            .last_insert_rowid();

        sqlx::query(
            "INSERT INTO scan_lists (name, priority_channel_id, priority_channel_2_id)
             VALUES ('Priority list', ?1, ?2)",
        )
        .bind(a)
        .bind(b)
        .execute(&pool)
        .await
        .unwrap();

        // Single delete: priority 1 goes, priority 2 is untouched.
        delete_impl(&pool, &[a]).await.expect("delete priority 1");
        let (p1, p2): (Option<i64>, Option<i64>) = sqlx::query_as(
            "SELECT priority_channel_id, priority_channel_2_id FROM scan_lists",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(p1, None, "the deleted channel's pointer must be cleared");
        assert_eq!(p2, Some(b), "the other pointer must survive");

        // Bulk delete: one offending id must not fail the whole transaction.
        let deleted = delete_impl(&pool, &[b, c]).await.expect("bulk delete");
        assert_eq!(deleted, 2, "both channels deleted");
        let (p1, p2): (Option<i64>, Option<i64>) = sqlx::query_as(
            "SELECT priority_channel_id, priority_channel_2_id FROM scan_lists",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!((p1, p2), (None, None));

        pool.close().await;
        let _ = std::fs::remove_file(&db_path);
    }

    /// Sixty repeaters in one town must produce one geocoder query, not sixty.
    /// `centroids` is read once before the loop, so rows resolved during the
    /// run could never feed the offline short-circuit — Nominatim's usage
    /// policy names repeated identical queries as grounds for blocking, and the
    /// User-Agent is shared by every user of the app.
    #[tokio::test]
    async fn backfill_asks_the_geocoder_once_per_town() {
        let dir = std::env::temp_dir().join(format!("cpm_memo_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let db_path = dir.join("memo.sqlite3");
        let _ = std::fs::remove_file(&db_path);
        let pool = crate::db::init_pool(&db_path).await.expect("init_pool");

        // A RepeaterBook template without Lat/Long: every row has a city and no
        // coordinates, and nothing in the database to match against.
        for i in 0..60 {
            sqlx::query(
                "INSERT INTO channels (rx_freq, city, state) VALUES (?1, 'Denver', 'CO')",
            )
            .bind(146.0 + f64::from(i) / 1000.0)
            .execute(&pool)
            .await
            .unwrap();
        }
        // Same town, spelled differently — still one town.
        sqlx::query("INSERT INTO channels (rx_freq, city, state) VALUES (147.0, ' denver ', 'co')")
            .execute(&pool)
            .await
            .unwrap();
        // A town the geocoder cannot find, twice: the miss is remembered too.
        for i in 0..2 {
            sqlx::query("INSERT INTO channels (rx_freq, city, state) VALUES (?1, 'Nowhere', 'CO')")
                .bind(148.0 + f64::from(i) / 1000.0)
                .execute(&pool)
                .await
                .unwrap();
        }

        let asked = std::cell::RefCell::new(Vec::<String>::new());
        let res = backfill_with(&pool, |city, _st, _country| {
            asked.borrow_mut().push(city.clone());
            async move {
                if city.trim().eq_ignore_ascii_case("denver") {
                    Ok(Some(GeoPoint { latitude: 39.74, longitude: -104.99 }))
                } else {
                    Ok(None)
                }
            }
        })
        .await
        .expect("backfill");

        assert_eq!(asked.borrow().len(), 2, "one query per town: {:?}", asked.borrow());
        assert_eq!(res.scanned, 63);
        assert_eq!(res.from_online, 61, "all 61 Denver rows get coordinates");
        assert_eq!(res.not_found, 2);

        // Every Denver row was written, not just the one that made the call.
        let filled: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM channels WHERE latitude IS NOT NULL AND longitude IS NOT NULL",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(filled, 61);

        pool.close().await;
        std::fs::remove_dir_all(&dir).ok();
    }
}
