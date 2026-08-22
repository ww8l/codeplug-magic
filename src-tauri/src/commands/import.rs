use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::{Acquire, SqlitePool};
use tauri::State;

use super::anytone::{
    AnytoneDecodedChannel, AnytoneDecodedContact, AnytoneDecodedZone, AnytoneSubTone,
};
use crate::db::AppState;
use crate::error::MapErrString;
use crate::models::ImportSummary;
use crate::util::{
    derive_band, derive_duplex, gen_name_long, gen_name_short, repair_truncated_tx, truncate,
};

/// A single RepeaterBook record (from CSV or JSON) parsed and mapped into our
/// schema, ready to be inserted or previewed. Serialized directly to the import
/// preview so the UI can show every field that will be imported.
#[derive(Debug, Clone, Serialize)]
pub struct ParsedChannel {
    repeaterbook_id: String,
    rb_name: String,
    name_long: String,
    name_short: String,
    callsign: String,
    rx_freq: f64,
    tx_freq: Option<f64>,
    offset: f64,
    duplex: String,
    band: String,
    mode: String,
    tone_mode: String,
    cross_mode: String,
    ctcss_uplink: Option<f64>,
    ctcss_downlink: Option<f64>,
    dmr_color_code: Option<i64>,
    dstar_capable: bool,
    ysf_capable: bool,
    nxdn_capable: bool,
    p25_capable: bool,
    p25_nac: Option<String>,
    m17_capable: bool,
    tetra_capable: bool,
    allstar_node: Option<String>,
    echolink_node: Option<String>,
    irlp_node: Option<String>,
    wires_node: Option<String>,
    use_type: Option<String>,
    operational_status: Option<String>,
    city: Option<String>,
    county: Option<String>,
    state: Option<String>,
    country: Option<String>,
    latitude: Option<f64>,
    longitude: Option<f64>,
    notes: Option<String>,
    ares: bool,
    races: bool,
    skywarn: bool,
    canwarn: bool,
}

/// The full parsed result shown before confirming an import. `rows` is capped
/// for very large files; `total` is the true count that will be imported.
#[derive(Debug, Clone, Serialize)]
pub struct ImportPreview {
    pub total: usize,
    pub rows: Vec<ParsedChannel>,
}

/// Cap on preview rows sent to the UI, to keep large state-wide exports light.
const PREVIEW_CAP: usize = 1000;

// ============================================================
// Tauri commands
// ============================================================
#[tauri::command]
pub async fn preview_csv_import(path: String) -> Result<ImportPreview, String> {
    Ok(build_preview(&parse_repeaterbook_csv(&path)?))
}

#[tauri::command]
pub async fn import_csv(
    state: State<'_, AppState>,
    path: String,
) -> Result<ImportSummary, String> {
    insert_parsed(&state.pool, &parse_repeaterbook_csv(&path)?).await
}

#[tauri::command]
pub async fn preview_json_import(path: String) -> Result<ImportPreview, String> {
    Ok(build_preview(&parse_repeaterbook_json(&path)?))
}

#[tauri::command]
pub async fn import_json(
    state: State<'_, AppState>,
    path: String,
) -> Result<ImportSummary, String> {
    insert_parsed(&state.pool, &parse_repeaterbook_json(&path)?).await
}

// ============================================================
// Shared preview + insert
// ============================================================
fn build_preview(parsed: &[ParsedChannel]) -> ImportPreview {
    ImportPreview {
        total: parsed.len(),
        rows: parsed.iter().take(PREVIEW_CAP).cloned().collect(),
    }
}

/// The subset of an existing channel row needed to merge a re-import: the four
/// user-overridable RepeaterBook fields and their override flags.
#[derive(sqlx::FromRow)]
struct ExistingChannel {
    id: i64,
    ctcss_uplink: Option<f64>,
    ctcss_downlink: Option<f64>,
    operational_status: Option<String>,
    notes: Option<String>,
    ctcss_uplink_overridden: bool,
    ctcss_downlink_overridden: bool,
    operational_status_overridden: bool,
    notes_overridden: bool,
}

fn merge_differs_f64(a: Option<f64>, b: Option<f64>) -> bool {
    match (a, b) {
        (Some(x), Some(y)) => (x - y).abs() > f64::EPSILON,
        (None, None) => false,
        _ => true,
    }
}

fn merge_differs_str(a: &Option<String>, b: &Option<String>) -> bool {
    a.as_deref() != b.as_deref()
}

/// Merge one user-overridable numeric RB field on re-import. If the user had
/// overridden it, keep their value; otherwise adopt the fresh RB value. The
/// snapshot always advances to the fresh RB value, and the override flag is
/// recomputed against it (so it clears if RB has caught up to the user's edit).
/// Returns `(value, snapshot, overridden)`.
fn merge_tracked_f64(
    overridden: bool,
    current: Option<f64>,
    rb: Option<f64>,
) -> (Option<f64>, Option<f64>, bool) {
    let value = if overridden { current } else { rb };
    (value, rb, merge_differs_f64(value, rb))
}

/// String twin of [`merge_tracked_f64`].
fn merge_tracked_str(
    overridden: bool,
    current: Option<String>,
    rb: Option<String>,
) -> (Option<String>, Option<String>, bool) {
    let value = if overridden { current } else { rb.clone() };
    let over = merge_differs_str(&value, &rb);
    (value, rb, over)
}

async fn insert_parsed(
    pool: &SqlitePool,
    parsed: &[ParsedChannel],
) -> Result<ImportSummary, String> {
    let mut conn = pool.acquire().await.estr()?;
    let mut tx = conn.begin().await.estr()?;

    let mut added = 0usize;
    let mut updated = 0usize;

    for p in parsed {
        // Dedupe on the (synthetic) RepeaterBook id. An existing row is MERGED
        // (RB-authoritative technical fields refreshed, user overrides and
        // custom names preserved) rather than skipped, so re-importing a fresh
        // export corrects stale data — e.g. a mode wrongly flagged YSF.
        let existing: Option<ExistingChannel> = sqlx::query_as(
            "SELECT id, ctcss_uplink, ctcss_downlink, operational_status, notes, \
             ctcss_uplink_overridden, ctcss_downlink_overridden, \
             operational_status_overridden, notes_overridden \
             FROM channels WHERE repeaterbook_id = ?1",
        )
        .bind(&p.repeaterbook_id)
        .fetch_optional(&mut *tx)
        .await
        .estr()?;
        if let Some(ex) = existing {
            merge_existing(&mut tx, &ex, p).await?;
            updated += 1;
            continue;
        }

        sqlx::query(
            r#"
            INSERT INTO channels (
                rb_name, name_long, name_short, callsign, rx_freq, tx_freq,
                offset, duplex, band, mode, tone_mode, ctcss_uplink,
                ctcss_downlink, dmr_color_code, dstar_capable, ysf_capable,
                nxdn_capable, p25_capable, p25_nac, m17_capable, tetra_capable,
                allstar_node, echolink_node, irlp_node, wires_node,
                use_type, operational_status, service_type,
                city, county, state, country, latitude, longitude, notes,
                ares, races, skywarn, canwarn, source, repeaterbook_id,
                rb_ctcss_uplink, rb_ctcss_downlink, rb_operational_status,
                rb_notes, cross_mode, last_rb_update
            ) VALUES (
                ?1, ?2, ?3, ?4, ?5, ?6,
                ?7, ?8, ?9, ?10, ?11, ?12,
                ?13, ?14, ?15, ?16,
                ?17, ?18, ?19, ?20, ?21,
                ?22, ?23, ?24, ?25,
                ?26, ?27, 'Amateur',
                ?28, ?29, ?30, ?31, ?32, ?33, ?34,
                ?35, ?36, ?37, ?38, 'repeaterbook', ?39,
                ?40, ?41, ?42,
                ?43, ?44, CURRENT_TIMESTAMP
            )
            "#,
        )
        .bind(&p.rb_name)
        .bind(&p.name_long)
        .bind(&p.name_short)
        .bind(&p.callsign)
        .bind(p.rx_freq)
        .bind(p.tx_freq)
        .bind(p.offset)
        .bind(&p.duplex)
        .bind(&p.band)
        .bind(&p.mode)
        .bind(&p.tone_mode)
        .bind(p.ctcss_uplink)
        .bind(p.ctcss_downlink)
        .bind(p.dmr_color_code)
        .bind(p.dstar_capable)
        .bind(p.ysf_capable)
        .bind(p.nxdn_capable)
        .bind(p.p25_capable)
        .bind(&p.p25_nac)
        .bind(p.m17_capable)
        .bind(p.tetra_capable)
        .bind(&p.allstar_node)
        .bind(&p.echolink_node)
        .bind(&p.irlp_node)
        .bind(&p.wires_node)
        .bind(&p.use_type)
        .bind(&p.operational_status)
        .bind(&p.city)
        .bind(&p.county)
        .bind(&p.state)
        .bind(&p.country)
        .bind(p.latitude)
        .bind(p.longitude)
        .bind(&p.notes)
        .bind(p.ares)
        .bind(p.races)
        .bind(p.skywarn)
        .bind(p.canwarn)
        .bind(&p.repeaterbook_id)
        .bind(p.ctcss_uplink) // rb_ctcss_uplink snapshot
        .bind(p.ctcss_downlink) // rb_ctcss_downlink snapshot
        .bind(&p.operational_status) // rb_operational_status snapshot
        .bind(&p.notes) // rb_notes snapshot
        .bind(&p.cross_mode)
        .execute(&mut *tx)
        .await
        .estr()?;

        added += 1;
    }

    tx.commit().await.estr()?;
    Ok(ImportSummary { added, updated, skipped: 0 })
}

/// Re-sync one already-imported channel from a fresh RepeaterBook record.
///
/// Merge policy:
///   * RB-authoritative technical facts (mode + every digital-mode flag,
///     tx_freq/offset/duplex/band, nodes, EmComm flags, city/county/country) are
///     refreshed from RB — this is what corrects a stale/mis-flagged mode.
///   * The four user-overridable fields (uplink/downlink tone, operational
///     status, notes) keep the user's value when overridden; otherwise they
///     adopt the fresh RB value. Their `rb_*` snapshots always advance and the
///     override flags are recomputed. `tone_mode`/`cross_mode` are re-derived
///     from the merged tone pair so they stay consistent with a kept override.
///   * User-facing names (`name_long`, `name_short`), DMR slot/talkgroup, DCS,
///     and power are NOT touched — those are the operator's to curate.
///   * `latitude`/`longitude` use COALESCE so an RB record without coordinates
///     never wipes coordinates the user set or the geocoder filled in.
///
/// `callsign`, `rx_freq`, and `state` are part of the dedupe id, so a matched
/// row necessarily agrees on them; they are intentionally left as-is.
async fn merge_existing(
    tx: &mut sqlx::SqliteConnection,
    ex: &ExistingChannel,
    p: &ParsedChannel,
) -> Result<(), String> {
    let (ctcss_uplink, rb_up, up_over) =
        merge_tracked_f64(ex.ctcss_uplink_overridden, ex.ctcss_uplink, p.ctcss_uplink);
    let (ctcss_downlink, rb_dn, dn_over) = merge_tracked_f64(
        ex.ctcss_downlink_overridden,
        ex.ctcss_downlink,
        p.ctcss_downlink,
    );
    let (operational_status, rb_status, status_over) = merge_tracked_str(
        ex.operational_status_overridden,
        ex.operational_status.clone(),
        p.operational_status.clone(),
    );
    let (notes, rb_notes, notes_over) =
        merge_tracked_str(ex.notes_overridden, ex.notes.clone(), p.notes.clone());
    let has_overrides = up_over || dn_over || status_over || notes_over;

    // Re-derive the tone scheme from the merged (possibly overridden) tone pair.
    let (tone_mode, cross_mode) = derive_tone_mode(ctcss_uplink, ctcss_downlink);

    sqlx::query(
        r#"
        UPDATE channels SET
            rb_name = ?2, tx_freq = ?3, offset = ?4, duplex = ?5, band = ?6,
            mode = ?7, tone_mode = ?8, cross_mode = ?9,
            ctcss_uplink = ?10, ctcss_downlink = ?11,
            dmr_color_code = ?12, dstar_capable = ?13, ysf_capable = ?14,
            nxdn_capable = ?15, p25_capable = ?16, p25_nac = ?17,
            m17_capable = ?18, tetra_capable = ?19,
            allstar_node = ?20, echolink_node = ?21, irlp_node = ?22,
            wires_node = ?23, use_type = ?24, operational_status = ?25,
            city = ?26, county = ?27, country = ?28,
            latitude = COALESCE(?29, latitude),
            longitude = COALESCE(?30, longitude),
            notes = ?31, ares = ?32, races = ?33, skywarn = ?34, canwarn = ?35,
            rb_ctcss_uplink = ?36, rb_ctcss_downlink = ?37,
            rb_operational_status = ?38, rb_notes = ?39,
            ctcss_uplink_overridden = ?40, ctcss_downlink_overridden = ?41,
            operational_status_overridden = ?42, notes_overridden = ?43,
            has_overrides = ?44,
            last_rb_update = CURRENT_TIMESTAMP
        WHERE id = ?1
        "#,
    )
    .bind(ex.id)
    .bind(&p.rb_name)
    .bind(p.tx_freq)
    .bind(p.offset)
    .bind(&p.duplex)
    .bind(&p.band)
    .bind(&p.mode)
    .bind(&tone_mode)
    .bind(&cross_mode)
    .bind(ctcss_uplink)
    .bind(ctcss_downlink)
    .bind(p.dmr_color_code)
    .bind(p.dstar_capable)
    .bind(p.ysf_capable)
    .bind(p.nxdn_capable)
    .bind(p.p25_capable)
    .bind(&p.p25_nac)
    .bind(p.m17_capable)
    .bind(p.tetra_capable)
    .bind(&p.allstar_node)
    .bind(&p.echolink_node)
    .bind(&p.irlp_node)
    .bind(&p.wires_node)
    .bind(&p.use_type)
    .bind(&operational_status)
    .bind(&p.city)
    .bind(&p.county)
    .bind(&p.country)
    .bind(p.latitude)
    .bind(p.longitude)
    .bind(&notes)
    .bind(p.ares)
    .bind(p.races)
    .bind(p.skywarn)
    .bind(p.canwarn)
    .bind(rb_up)
    .bind(rb_dn)
    .bind(&rb_status)
    .bind(&rb_notes)
    .bind(up_over)
    .bind(dn_over)
    .bind(status_over)
    .bind(notes_over)
    .bind(has_overrides)
    .execute(&mut *tx)
    .await
    .estr()?;

    Ok(())
}

// ============================================================
// Field derivation shared by both parsers
// ============================================================
/// Build the derived/name/dedupe fields common to every parsed channel from the
/// already-extracted primitive values.
fn finalize(
    callsign: &str,
    rx_freq: f64,
    tx_freq: Option<f64>,
    ctcss_uplink: Option<f64>,
    ctcss_downlink: Option<f64>,
    city: Option<&str>,
    state: Option<&str>,
) -> ParsedChannelStub {
    let (duplex, offset) = derive_duplex(rx_freq, tx_freq);
    // RepeaterBook truncates the input/TX frequency to 3 decimals; rebuild it at
    // full precision when the offset is a rounding error off a standard offset.
    let (tx_freq, offset) = repair_truncated_tx(rx_freq, tx_freq, &duplex, offset);
    let band = derive_band(rx_freq).to_string();

    let city_str = city.unwrap_or_default();
    let rb_name = format!("{} {}", callsign, city_str).trim().to_string();
    let name_long = gen_name_long(callsign, city_str);
    let name_short = gen_name_short(callsign);

    // Synthetic dedupe id. The "Full Data" export carries no unique repeater id,
    // so we build one from callsign + frequency + state + CITY. City is
    // essential: co-state repeaters can share a callsign and frequency (e.g.
    // N2SKY 448.400 exists in both Fort Collins and Buena Vista, CO) and would
    // otherwise collapse into a single row. Kept in sync with migration 0011,
    // which recomputes this for already-imported rows.
    let repeaterbook_id = format!(
        "{}|{:.4}|{}|{}",
        callsign.to_uppercase(),
        rx_freq,
        state.unwrap_or_default().to_uppercase(),
        city_str.to_uppercase(),
    );

    // RepeaterBook gives an uplink tone (TX, what you must transmit to key the
    // repeater) and a downlink tone (RX, what the repeater sends back). Map to
    // CHIRP's universal scheme:
    //   up only          -> Tone  (TX tone, RX open)
    //   up == down       -> TSQL  (tone squelch on the shared tone)
    //   up != down       -> Cross "Tone->Tone" (TX uplink, RX downlink)
    //   down only        -> Cross "->Tone" (RX tone only, no TX tone)
    //   neither          -> off
    let (tone_mode, cross_mode) = derive_tone_mode(ctcss_uplink, ctcss_downlink);

    ParsedChannelStub {
        repeaterbook_id,
        rb_name,
        name_long,
        name_short,
        tx_freq,
        duplex,
        offset,
        band,
        tone_mode,
        cross_mode,
    }
}

/// Map a RepeaterBook uplink/downlink CTCSS pair to CHIRP's universal tone
/// scheme (`tone_mode`, `cross_mode`). Shared by the initial parse and the
/// re-import merge so a merged (possibly user-overridden) tone pair always
/// re-derives a consistent tone_mode.
fn derive_tone_mode(
    ctcss_uplink: Option<f64>,
    ctcss_downlink: Option<f64>,
) -> (String, String) {
    let eps = 0.05; // tones are tenths of a Hz; compare with a small epsilon
    match (ctcss_uplink, ctcss_downlink) {
        (Some(up), Some(dn)) if (up - dn).abs() < eps => {
            ("TSQL".to_string(), "Tone->Tone".to_string())
        }
        (Some(_), Some(_)) => ("Cross".to_string(), "Tone->Tone".to_string()),
        (Some(_), None) => ("Tone".to_string(), "Tone->Tone".to_string()),
        (None, Some(_)) => ("Cross".to_string(), "->Tone".to_string()),
        (None, None) => ("off".to_string(), "Tone->Tone".to_string()),
    }
}

/// Pick the primary `mode` string from the digital-capability flags. RepeaterBook
/// has no explicit mode column, so a repeater with no digital flag is plain "FM"
/// and a digital one reports its protocol. A machine can be mixed-mode; we surface
/// the first protocol in this fixed precedence so mode filtering/UI behaves.
fn derive_mode(
    dmr: bool,
    dstar: bool,
    ysf: bool,
    nxdn: bool,
    p25: bool,
    m17: bool,
) -> String {
    if dmr {
        "DMR"
    } else if dstar {
        "DSTAR"
    } else if ysf {
        "YSF"
    } else if nxdn {
        "NXDN"
    } else if p25 {
        "P25"
    } else if m17 {
        "M17"
    } else {
        "FM"
    }
    .to_string()
}

/// The handful of derived fields produced by [`finalize`].
struct ParsedChannelStub {
    repeaterbook_id: String,
    rb_name: String,
    name_long: String,
    name_short: String,
    tx_freq: Option<f64>,
    duplex: String,
    offset: f64,
    band: String,
    tone_mode: String,
    cross_mode: String,
}

// ============================================================
// CSV parser
// ============================================================
/// Parse a RepeaterBook CSV export at `path` into our channel representation.
fn parse_repeaterbook_csv(path: &str) -> Result<Vec<ParsedChannel>, String> {
    let mut reader = csv::ReaderBuilder::new()
        .flexible(true)
        .trim(csv::Trim::All)
        .from_path(path)
        .map_err(|e| format!("Could not open CSV: {e}"))?;

    let headers = reader.headers().estr()?.clone();
    let mut col: HashMap<String, usize> = HashMap::new();
    for (i, h) in headers.iter().enumerate() {
        col.insert(h.trim().to_lowercase(), i);
    }

    let get = |rec: &csv::StringRecord, name: &str| -> Option<String> {
        col.get(&name.to_lowercase())
            .and_then(|&i| rec.get(i))
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
    };

    let mut out = Vec::new();
    for result in reader.records() {
        let rec = result.estr()?;

        let rx_freq = match get(&rec, "Frequency").and_then(|s| parse_leading_f64(&s)) {
            Some(f) => f,
            None => continue,
        };
        let tx_freq = get(&rec, "Input Freq").and_then(|s| parse_leading_f64(&s));
        let callsign = get(&rec, "Call").unwrap_or_default();
        let city = get(&rec, "Location");
        let state = get(&rec, "State");
        let ctcss_uplink = get(&rec, "PL").and_then(|s| parse_leading_f64(&s));
        let ctcss_downlink = get(&rec, "TSQ").and_then(|s| parse_leading_f64(&s));

        let s = finalize(
            &callsign,
            rx_freq,
            tx_freq,
            ctcss_uplink,
            ctcss_downlink,
            city.as_deref(),
            state.as_deref(),
        );

        let dmr_color_code = get(&rec, "DMR Color Code").and_then(|s| s.parse::<i64>().ok());
        let dstar_capable = parse_bool(get(&rec, "D-Star"));
        let ysf_capable = parse_bool(get(&rec, "System Fusion"));
        let nxdn_capable = parse_bool(get(&rec, "NXDN"));
        let p25_capable = parse_bool(get(&rec, "P25"));
        let mode = derive_mode(
            dmr_color_code.is_some(),
            dstar_capable,
            ysf_capable,
            nxdn_capable,
            p25_capable,
            false,
        );

        out.push(ParsedChannel {
            repeaterbook_id: s.repeaterbook_id,
            rb_name: s.rb_name,
            name_long: s.name_long,
            name_short: s.name_short,
            callsign,
            rx_freq,
            tx_freq: s.tx_freq,
            offset: s.offset,
            duplex: s.duplex,
            band: s.band,
            mode,
            tone_mode: s.tone_mode,
            cross_mode: s.cross_mode,
            ctcss_uplink,
            ctcss_downlink,
            dmr_color_code,
            dstar_capable,
            ysf_capable,
            nxdn_capable,
            p25_capable,
            p25_nac: None,
            m17_capable: false,
            tetra_capable: false,
            allstar_node: None,
            echolink_node: None,
            irlp_node: None,
            wires_node: None,
            use_type: get(&rec, "Use"),
            operational_status: get(&rec, "Operational Status"),
            city,
            county: get(&rec, "County"),
            state,
            country: Some("United States".to_string()),
            latitude: get(&rec, "Lat").and_then(|s| parse_leading_f64(&s)),
            longitude: get(&rec, "Long").and_then(|s| parse_leading_f64(&s)),
            notes: s_notes_from(&rec, &get),
            ares: parse_bool(get(&rec, "ARES")),
            races: parse_bool(get(&rec, "RACES")),
            skywarn: parse_bool(get(&rec, "SKYWARN")),
            canwarn: parse_bool(get(&rec, "CANWARN")),
        });
    }

    Ok(out)
}

/// Recompute notes for a CSV record (Notes + Landmark) to keep CSV behavior.
fn s_notes_from(
    rec: &csv::StringRecord,
    get: &impl Fn(&csv::StringRecord, &str) -> Option<String>,
) -> Option<String> {
    let mut parts = Vec::new();
    if let Some(n) = get(rec, "Notes") {
        parts.push(n);
    }
    if let Some(lm) = get(rec, "Landmark") {
        parts.push(format!("Landmark: {lm}"));
    }
    (!parts.is_empty()).then(|| parts.join(" | "))
}

// ============================================================
// JSON parser
// ============================================================
#[derive(Debug, Deserialize)]
struct JsonExport {
    /// Optional so a document without the key is a clear rejection rather than
    /// a silent empty import — defaulting it to `[]` made every non-RepeaterBook
    /// JSON (a channel backup, an `.icf`, a settings export) parse cleanly to
    /// zero records and report success.
    records: Option<Vec<HashMap<String, Value>>>,
}

/// Pull a trimmed, non-empty string value out of a JSON record (string or
/// number).
fn jstr(rec: &HashMap<String, Value>, key: &str) -> Option<String> {
    rec.get(key)
        .and_then(|v| match v {
            Value::String(s) => Some(s.trim().to_string()),
            Value::Number(n) => Some(n.to_string()),
            _ => None,
        })
        .filter(|s| !s.is_empty())
}

/// Parse a RepeaterBook JSON export (template "Full Data" / `export_format:
/// json`) at `path` into our channel representation.
fn parse_repeaterbook_json(path: &str) -> Result<Vec<ParsedChannel>, String> {
    let text =
        std::fs::read_to_string(path).map_err(|e| format!("Could not open JSON: {e}"))?;
    let export: JsonExport =
        serde_json::from_str(&text).map_err(|e| format!("Could not parse JSON: {e}"))?;
    let records = export.records.ok_or_else(|| {
        "This JSON has no \"records\" list, so it is not a RepeaterBook export."
            .to_string()
    })?;

    let mut out = Vec::new();
    for rec in &records {
        let rx_freq = match jstr(rec, "freq_mhz").and_then(|s| parse_leading_f64(&s)) {
            Some(f) => f,
            None => continue,
        };
        let tx_freq = jstr(rec, "input_freq").and_then(|s| parse_leading_f64(&s));
        let callsign = jstr(rec, "callsign").unwrap_or_default();
        let city = jstr(rec, "city");
        let state = jstr(rec, "state");

        // pl_tone = uplink (encode); tsq_tone = downlink (decode). Fall back to
        // the generic `tone` for uplink if pl_tone is absent.
        let ctcss_uplink = jstr(rec, "pl_tone")
            .or_else(|| jstr(rec, "tone"))
            .and_then(|s| parse_leading_f64(&s));
        let ctcss_downlink = jstr(rec, "tsq_tone").and_then(|s| parse_leading_f64(&s));

        let s = finalize(
            &callsign,
            rx_freq,
            tx_freq,
            ctcss_uplink,
            ctcss_downlink,
            city.as_deref(),
            state.as_deref(),
        );

        // DMR: the schema has no dmr_capable column, so we only keep the color
        // code, and only when the repeater is actually flagged DMR.
        let dmr_color_code = if parse_bool(jstr(rec, "dmr")) {
            jstr(rec, "dmr_cc").and_then(|s| s.parse::<i64>().ok())
        } else {
            None
        };

        // D-STAR is indicated by the presence of a node/service, not a bool.
        let dstar_capable =
            jstr(rec, "dstar_node").is_some() || jstr(rec, "dstar_service").is_some();
        // YSF comes ONLY from the explicit System Fusion flag. A WIRES-X node is
        // NOT a reliable YSF signal: RepeaterBook tracks "WIRES equipped"
        // separately from "System Fusion", and many WIRES-X repeaters transmit
        // plain FM analog (AMS / analog-friendly). Folding `wires` in here
        // mis-flagged FM repeaters (e.g. N2SKY 448.400, WIRES + FM analog) as
        // YSF, which then got skipped as unsupported digital. The Wires node is
        // still captured below in `wires_node`.
        let ysf_capable = parse_bool(jstr(rec, "ysf"));
        let nxdn_capable = parse_bool(jstr(rec, "nxdn"));
        let p25_capable = parse_bool(jstr(rec, "p25"));
        let m17_capable = parse_bool(jstr(rec, "m17"));
        let tetra_capable = parse_bool(jstr(rec, "tetra"));

        // Derive the primary mode from the digital flags. RepeaterBook has no
        // explicit mode column, so an FM-only repeater stays "FM" while a digital
        // one reports its protocol (so mode filtering/UI works).
        let mode = derive_mode(
            dmr_color_code.is_some(),
            dstar_capable,
            ysf_capable,
            nxdn_capable,
            p25_capable,
            m17_capable,
        );

        out.push(ParsedChannel {
            repeaterbook_id: s.repeaterbook_id,
            rb_name: s.rb_name,
            name_long: s.name_long,
            name_short: s.name_short,
            callsign,
            rx_freq,
            tx_freq: s.tx_freq,
            offset: s.offset,
            duplex: s.duplex,
            band: s.band,
            mode,
            tone_mode: s.tone_mode,
            cross_mode: s.cross_mode,
            ctcss_uplink,
            ctcss_downlink,
            dmr_color_code,
            dstar_capable,
            ysf_capable,
            nxdn_capable,
            p25_capable,
            p25_nac: jstr(rec, "p25_nac"),
            m17_capable,
            tetra_capable,
            allstar_node: jstr(rec, "allstar_node"),
            echolink_node: jstr(rec, "echolink_node"),
            irlp_node: jstr(rec, "irlp_node_id"),
            wires_node: jstr(rec, "wires_node"),
            use_type: None,
            operational_status: None,
            city,
            county: jstr(rec, "county"),
            state,
            country: jstr(rec, "country").map(normalize_country),
            latitude: jstr(rec, "lat").and_then(|s| parse_leading_f64(&s)),
            longitude: jstr(rec, "lon").and_then(|s| parse_leading_f64(&s)),
            notes: s_notes_json(rec),
            ares: parse_bool(jstr(rec, "ares")),
            races: parse_bool(jstr(rec, "races")),
            skywarn: parse_bool(jstr(rec, "skywarn")),
            canwarn: parse_bool(jstr(rec, "canwarn")),
        });
    }

    Ok(out)
}

/// Build notes for a JSON record from its landmark.
fn s_notes_json(rec: &HashMap<String, Value>) -> Option<String> {
    jstr(rec, "landmark").map(|lm| format!("Landmark: {lm}"))
}

/// Normalize a RepeaterBook country code to match manually-entered channels.
fn normalize_country(c: String) -> String {
    match c.to_uppercase().as_str() {
        "US" | "USA" => "United States".to_string(),
        _ => c,
    }
}

// ============================================================
// AnyTone radio-download import
// ============================================================
// Takes the decoded result of `download_anytone_image` (handed back by the
// frontend, no second radio session) and imports it into the library:
// channels → `channels`, zones → `channel_lists` + entries, DMR contacts →
// `talkgroups`. Everything dedupes so re-importing the same radio is a no-op.

/// Per-table added/skipped counts for an AnyTone radio-download import.
#[derive(Debug, Default, Serialize)]
pub struct AnytoneImportSummary {
    pub channels_added: usize,
    pub channels_skipped: usize,
    pub talkgroups_added: usize,
    pub talkgroups_skipped: usize,
    pub lists_added: usize,
    pub lists_skipped: usize,
}

#[tauri::command]
pub async fn import_anytone_download(
    state: State<'_, AppState>,
    channels: Vec<AnytoneDecodedChannel>,
    zones: Vec<AnytoneDecodedZone>,
    contacts: Vec<AnytoneDecodedContact>,
) -> Result<AnytoneImportSummary, String> {
    import_anytone(&state.pool, &channels, &zones, &contacts).await
}

/// The decoded-channel → schema-column mapping, pure so it's unit-testable.
#[derive(Debug, PartialEq)]
struct MappedAnytoneChannel {
    name_long: String,
    name_short: String,
    rx_freq: f64,
    tx_freq: f64,
    duplex: String,
    offset: f64,
    band: String,
    mode: String,
    tone_mode: String,
    ctcss_uplink: Option<f64>,
    ctcss_downlink: Option<f64>,
    dcs_code: Option<String>,
    dcs_rx_code: Option<String>,
    cross_mode: String,
    power: Option<String>,
    dmr_color_code: Option<i64>,
    dmr_timeslot: Option<i64>,
    dmr_talkgroup: Option<i64>,
    notes: String,
}

/// Map the structured TX/RX sub-tones onto CHIRP's universal tone scheme
/// (`tone_mode` off|Tone|TSQL|DTCS|Cross + per-side fields + `cross_mode`).
/// The radio stores DCS as a raw binary u16 and displays it in octal (the
/// universal DCS notation, HW-confirmed); DCS codes are stored as the
/// zero-padded OCTAL code string, matching CHIRP and the rest of the DB.
pub(crate) fn map_anytone_tone(
    tx: &Option<AnytoneSubTone>,
    rx: &Option<AnytoneSubTone>,
) -> (
    String,
    Option<f64>,
    Option<f64>,
    Option<String>,
    Option<String>,
    String,
) {
    use AnytoneSubTone::{Ctcss, Dcs};
    let dcs = |c: u16| format!("{c:03o}");
    let eps = 0.05; // tones are tenths of a Hz
    let (tone_mode, up, down, dcs_tx, dcs_rx, cross) = match (tx, rx) {
        (None, None) => ("off", None, None, None, None, "Tone->Tone"),
        (Some(Ctcss(u)), None) => ("Tone", Some(*u), None, None, None, "Tone->Tone"),
        (Some(Ctcss(u)), Some(Ctcss(d))) if (u - d).abs() < eps => {
            ("TSQL", Some(*u), Some(*d), None, None, "Tone->Tone")
        }
        (Some(Ctcss(u)), Some(Ctcss(d))) => {
            ("Cross", Some(*u), Some(*d), None, None, "Tone->Tone")
        }
        (None, Some(Ctcss(d))) => ("Cross", None, Some(*d), None, None, "->Tone"),
        (Some(Dcs(a)), Some(Dcs(b))) if a == b => {
            ("DTCS", None, None, Some(dcs(*a)), Some(dcs(*b)), "Tone->Tone")
        }
        (Some(Dcs(a)), Some(Dcs(b))) => {
            ("Cross", None, None, Some(dcs(*a)), Some(dcs(*b)), "DTCS->DTCS")
        }
        (Some(Dcs(a)), None) => ("Cross", None, None, Some(dcs(*a)), None, "DTCS->"),
        (None, Some(Dcs(b))) => ("Cross", None, None, None, Some(dcs(*b)), "->DTCS"),
        (Some(Ctcss(u)), Some(Dcs(b))) => {
            ("Cross", Some(*u), None, None, Some(dcs(*b)), "Tone->DTCS")
        }
        (Some(Dcs(a)), Some(Ctcss(d))) => {
            ("Cross", None, Some(*d), Some(dcs(*a)), None, "DTCS->Tone")
        }
    };
    (
        tone_mode.to_string(),
        up,
        down,
        dcs_tx,
        dcs_rx,
        cross.to_string(),
    )
}

/// Map one decoded channel (plus its resolved DMR contact, if any) onto our
/// schema's columns.
fn map_anytone_channel(
    ch: &AnytoneDecodedChannel,
    contact: Option<&AnytoneDecodedContact>,
) -> MappedAnytoneChannel {
    let (duplex, offset) = derive_duplex(ch.rx_mhz, Some(ch.tx_mhz));
    let digital = ch.color_code.is_some();
    let (tone_mode, ctcss_uplink, ctcss_downlink, dcs_code, dcs_rx_code, cross_mode) =
        map_anytone_tone(&ch.tone_tx, &ch.tone_rx);
    // Radio-agnostic power level; Turbo (the radio's max) maps to NULL =
    // "radio default (max)", which round-trips back to the top level on program.
    let power = match ch.power.as_str() {
        "Low" => Some("Low".to_string()),
        "Medium" => Some("Med".to_string()),
        "High" => Some("High".to_string()),
        _ => None,
    };
    let mut notes = format!("Imported from AT-D890UV slot {}", ch.index);
    if digital && ch.mode != "DMR" {
        // Mixed FM+DMR modes flatten to "DMR" in our schema; keep the original.
        notes.push_str(&format!(" · radio mode {}", ch.mode));
    }
    MappedAnytoneChannel {
        name_long: truncate(&ch.name, 16),
        name_short: truncate(&ch.name, 7),
        rx_freq: ch.rx_mhz,
        tx_freq: ch.tx_mhz,
        duplex,
        offset,
        band: derive_band(ch.rx_mhz).to_string(),
        // Narrow analog keeps its bandwidth as mode "NFM" so programming back
        // to a radio doesn't silently widen it to 25 kHz.
        mode: if digital {
            "DMR"
        } else if ch.bandwidth == "12.5 kHz" {
            "NFM"
        } else {
            "FM"
        }
        .to_string(),
        tone_mode,
        ctcss_uplink,
        ctcss_downlink,
        dcs_code,
        dcs_rx_code,
        cross_mode,
        power,
        dmr_color_code: ch.color_code.map(i64::from),
        dmr_timeslot: ch.time_slot.map(i64::from),
        dmr_talkgroup: contact.map(|c| i64::from(c.dmr_id)),
        notes,
    }
}

/// Insert the decoded download into the library inside one transaction.
/// Channels dedupe on (name_long, rx_freq); talkgroups on tg_number (any
/// network, so the Brandmeister seed isn't duplicated); channel lists on name.
async fn import_anytone(
    pool: &SqlitePool,
    channels: &[AnytoneDecodedChannel],
    zones: &[AnytoneDecodedZone],
    contacts: &[AnytoneDecodedContact],
) -> Result<AnytoneImportSummary, String> {
    let mut conn = pool.acquire().await.estr()?;
    let mut tx = conn.begin().await.estr()?;
    let mut sum = AnytoneImportSummary::default();

    // Talkgroups. Call type 2 (All Call) isn't a real talkgroup — skip it.
    for c in contacts {
        if c.call_type > 1 {
            continue;
        }
        let existing: Option<(i64,)> =
            sqlx::query_as("SELECT id FROM talkgroups WHERE tg_number = ?1")
                .bind(i64::from(c.dmr_id))
                .fetch_optional(&mut *tx)
                .await
                .estr()?;
        if existing.is_some() {
            sum.talkgroups_skipped += 1;
            continue;
        }
        sqlx::query(
            "INSERT INTO talkgroups (tg_number, name, network, call_type, notes, source)
             VALUES (?1, ?2, 'Other', ?3, 'Imported from AT-D890UV', 'radio')",
        )
        .bind(i64::from(c.dmr_id))
        .bind(&c.name)
        .bind(if c.call_type == 0 { "Private" } else { "Group" })
        .execute(&mut *tx)
        .await
        .estr()?;
        sum.talkgroups_added += 1;
    }

    // Channels. Track 0-based radio slot → channel row id (new OR existing) so
    // zone membership resolves even for channels that were already in the DB.
    let contact_by_idx: HashMap<u16, &AnytoneDecodedContact> =
        contacts.iter().map(|c| (c.index, c)).collect();
    let mut slot_ids: HashMap<u16, i64> = HashMap::new();
    for ch in channels {
        let m = map_anytone_channel(
            ch,
            ch.contact_index
                .and_then(|i| contact_by_idx.get(&i).copied()),
        );
        let slot0 = (ch.index - 1) as u16;
        let existing: Option<(i64,)> = sqlx::query_as(
            "SELECT id FROM channels WHERE name_long = ?1 AND ABS(rx_freq - ?2) < 0.0001",
        )
        .bind(&m.name_long)
        .bind(m.rx_freq)
        .fetch_optional(&mut *tx)
        .await
        .estr()?;
        if let Some((id,)) = existing {
            slot_ids.insert(slot0, id);
            sum.channels_skipped += 1;
            continue;
        }
        let res = sqlx::query(
            r#"
            INSERT INTO channels (
                name_long, name_short, rx_freq, tx_freq, offset, duplex, band,
                mode, tone_mode, ctcss_uplink, ctcss_downlink, dcs_code,
                dcs_rx_code, cross_mode, power, dmr_color_code, dmr_timeslot,
                dmr_talkgroup, notes, source
            ) VALUES (
                ?1, ?2, ?3, ?4, ?5, ?6, ?7,
                ?8, ?9, ?10, ?11, ?12,
                ?13, ?14, ?15, ?16, ?17,
                ?18, ?19, 'radio'
            )
            "#,
        )
        .bind(&m.name_long)
        .bind(&m.name_short)
        .bind(m.rx_freq)
        .bind(m.tx_freq)
        .bind(m.offset)
        .bind(&m.duplex)
        .bind(&m.band)
        .bind(&m.mode)
        .bind(&m.tone_mode)
        .bind(m.ctcss_uplink)
        .bind(m.ctcss_downlink)
        .bind(&m.dcs_code)
        .bind(&m.dcs_rx_code)
        .bind(&m.cross_mode)
        .bind(&m.power)
        .bind(m.dmr_color_code)
        .bind(m.dmr_timeslot)
        .bind(m.dmr_talkgroup)
        .bind(&m.notes)
        .execute(&mut *tx)
        .await
        .estr()?;
        slot_ids.insert(slot0, res.last_insert_rowid());
        sum.channels_added += 1;
    }

    // Zones → channel lists. A list with the zone's name already existing means
    // this zone (or a same-named list) was imported before — leave it untouched.
    for z in zones {
        let existing: Option<(i64,)> =
            sqlx::query_as("SELECT id FROM channel_lists WHERE name = ?1")
                .bind(&z.name)
                .fetch_optional(&mut *tx)
                .await
                .estr()?;
        if existing.is_some() {
            sum.lists_skipped += 1;
            continue;
        }
        let res = sqlx::query(
            "INSERT INTO channel_lists (name, description)
             VALUES (?1, 'Imported from AT-D890UV radio download')",
        )
        .bind(&z.name)
        .execute(&mut *tx)
        .await
        .estr()?;
        let list_id = res.last_insert_rowid();
        let mut position = 0i64;
        for slot in &z.member_slots {
            // A member slot without a decoded channel (empty/unread bank) is
            // skipped rather than failing the whole import.
            let Some(&channel_id) = slot_ids.get(slot) else {
                continue;
            };
            sqlx::query(
                "INSERT INTO channel_list_entries (channel_list_id, channel_id, position)
                 VALUES (?1, ?2, ?3)",
            )
            .bind(list_id)
            .bind(channel_id)
            .bind(position)
            .execute(&mut *tx)
            .await
            .estr()?;
            position += 1;
        }
        sum.lists_added += 1;
    }

    tx.commit().await.estr()?;
    Ok(sum)
}

// ============================================================
// Helpers
// ============================================================
/// Parse a float from the leading numeric portion of a string (e.g. "100.0 PL"
/// -> 100.0).
fn parse_leading_f64(s: &str) -> Option<f64> {
    let trimmed: String = s
        .trim()
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == '.' || *c == '-' || *c == '+')
        .collect();
    trimmed.parse::<f64>().ok()
}

/// Interpret a RepeaterBook yes/no style flag.
fn parse_bool(v: Option<String>) -> bool {
    matches!(
        v.map(|s| s.trim().to_lowercase()).as_deref(),
        Some("yes") | Some("y") | Some("true") | Some("1") | Some("x")
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tone_mode_maps_uplink_and_downlink() {
        let fin = |up, dn| finalize("W0CPH", 146.94, Some(146.34), up, dn, None, None);
        // up only -> Tone
        let t = fin(Some(100.0), None);
        assert_eq!((t.tone_mode.as_str(), t.cross_mode.as_str()), ("Tone", "Tone->Tone"));
        // up == down -> TSQL
        let t = fin(Some(100.0), Some(100.0));
        assert_eq!(t.tone_mode, "TSQL");
        // up != down -> Cross Tone->Tone (the KB0VJJ 88.5/123.0 case)
        let t = fin(Some(88.5), Some(123.0));
        assert_eq!((t.tone_mode.as_str(), t.cross_mode.as_str()), ("Cross", "Tone->Tone"));
        // down only -> Cross ->Tone (RX tone, no TX)
        let t = fin(None, Some(123.0));
        assert_eq!((t.tone_mode.as_str(), t.cross_mode.as_str()), ("Cross", "->Tone"));
        // neither -> off
        assert_eq!(fin(None, None).tone_mode, "off");
    }

    /// A JSON document with no `records` key is rejected outright. It used to
    /// default to an empty list, so a channel backup, an `.icf` or any other
    /// JSON parsed to zero records and reported "Imported 0 channels" as a
    /// success.
    #[test]
    fn json_without_a_records_key_is_rejected() {
        let dir = std::env::temp_dir();
        let path = dir.join(format!("cpm_no_records_{}.json", std::process::id()));

        std::fs::write(&path, r#"{"format":"73plug-channels","channels":[]}"#).unwrap();
        let err = parse_repeaterbook_json(path.to_str().unwrap())
            .expect_err("a document with no records list must not parse");
        assert!(err.contains("records"), "error should name the missing key: {err}");

        // An export that genuinely carries the key still parses, empty or not.
        std::fs::write(&path, r#"{"records":[]}"#).unwrap();
        assert_eq!(parse_repeaterbook_json(path.to_str().unwrap()).unwrap().len(), 0);

        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn merge_tracked_keeps_user_override_and_advances_snapshot() {
        // User overrode the uplink tone to 100.0; RB now reports 88.5.
        // Keep the user's 100.0, advance the snapshot to 88.5, stay overridden.
        let (val, snap, over) = merge_tracked_f64(true, Some(100.0), Some(88.5));
        assert_eq!((val, snap, over), (Some(100.0), Some(88.5), true));

        // Not overridden: adopt the fresh RB value, snapshot matches, flag clears.
        let (val, snap, over) = merge_tracked_f64(false, Some(100.0), Some(88.5));
        assert_eq!((val, snap, over), (Some(88.5), Some(88.5), false));

        // Override clears when RB catches up to the user's edit.
        let (val, _snap, over) = merge_tracked_f64(true, Some(88.5), Some(88.5));
        assert_eq!((val, over), (Some(88.5), false));

        // String twin: user-set notes preserved, snapshot advances.
        let (val, snap, over) = merge_tracked_str(
            true,
            Some("my note".into()),
            Some("rb note".into()),
        );
        assert_eq!(
            (val, snap, over),
            (Some("my note".into()), Some("rb note".into()), true),
        );
    }

    #[test]
    fn wires_x_without_system_fusion_stays_fm() {
        // N2SKY 448.400 regression: RepeaterBook flags WIRES-X but NOT System
        // Fusion, and it's an FM analog repeater. A WIRES node alone must not
        // promote the mode to YSF.
        use serde_json::json;
        let rec: HashMap<String, Value> = json!({
            "freq_mhz": "448.400",
            "callsign": "N2SKY",
            "ysf": "No",
            "wires": "Yes",
            "wires_node": "12345",
        })
        .as_object()
        .unwrap()
        .clone()
        .into_iter()
        .collect();

        let ysf = parse_bool(jstr(&rec, "ysf"));
        assert!(!ysf, "System Fusion flag is No -> not YSF");
        let mode = derive_mode(false, false, ysf, false, false, false);
        assert_eq!(mode, "FM");
        // The WIRES node is still captured for reference.
        assert_eq!(jstr(&rec, "wires_node").as_deref(), Some("12345"));
    }

    #[test]
    fn rebuilds_tx_truncated_by_repeaterbook() {
        // K0HRV Arvada: RepeaterBook gives the output at 4 decimals but truncates
        // the input to 3 (441.863 instead of 441.8625), so the raw offset is
        // 4.9995. finalize should snap it to the standard 70cm 5 MHz offset and
        // rebuild the TX frequency at full precision.
        let s = finalize("K0HRV", 446.8625, Some(441.863), None, None, Some("Arvada"), Some("CO"));
        assert_eq!(s.duplex, "-");
        assert!((s.offset - 5.0).abs() < 1e-9, "offset should snap to 5.0, got {}", s.offset);
        assert_eq!(s.tx_freq, Some(441.8625));

        // An exact standard offset is preserved unchanged.
        let s = finalize("W0CPH", 145.385, Some(144.785), None, None, None, None);
        assert!((s.offset - 0.6).abs() < 1e-9);
        assert_eq!(s.tx_freq, Some(144.785));

        // A genuine odd split (not a recognized offset) is left untouched.
        let s = finalize("N0ODD", 446.8625, Some(441.0), None, None, None, None);
        assert_eq!(s.tx_freq, Some(441.0));
        assert!((s.offset - 5.8625).abs() < 1e-9);
    }

    #[test]
    fn parses_sample_repeaterbook_csv() {
        let parsed = parse_repeaterbook_csv("../sample-data/repeaterbook-sample.csv")
            .expect("parse failed");
        assert_eq!(parsed.len(), 10);

        let r0 = &parsed[0];
        assert_eq!(r0.callsign, "W0CPH");
        assert_eq!(r0.duplex, "-");
        assert!((r0.offset - 0.6).abs() < 1e-6);
        assert_eq!(r0.band, "VHF");
        // PL 100.0 == TSQ 100.0 -> tone squelch.
        assert_eq!(r0.tone_mode, "TSQL");
        assert_eq!(r0.ctcss_uplink, Some(100.0));
        assert_eq!(r0.ctcss_downlink, Some(100.0));
        assert_eq!(r0.name_short, "W0CPH");

        assert!(parsed.iter().any(|p| p.callsign == "W0ARK" && p.dstar_capable));
        assert!(parsed.iter().any(|p| p.dmr_color_code == Some(1)));
        assert!(parsed.iter().any(|p| p.band == "900"));
        assert!(r0.repeaterbook_id.contains("W0CPH"));
    }

    #[test]
    fn parses_sample_repeaterbook_json() {
        let parsed = parse_repeaterbook_json("../sample-data/repeaterbook-full-sample.json")
            .expect("parse failed");
        // 14 records in the file; the last one carries no frequency and is skipped.
        assert_eq!(parsed.len(), 13);
        assert!(parsed.iter().all(|p| p.callsign != "QQ0ZZZ"));

        // Every record should derive a usable band and dedupe id.
        assert!(parsed.iter().all(|p| !p.band.is_empty()));
        assert!(parsed.iter().all(|p| p.repeaterbook_id.contains('|')));
        for want in ["VHF", "220", "UHF", "900"] {
            assert!(
                parsed.iter().any(|p| p.band == want),
                "no {want} record in the sample"
            );
        }

        // At least one P25 machine with a NAC should be captured.
        assert!(parsed
            .iter()
            .any(|p| p.p25_capable && p.p25_nac.is_some()));
        // Digital repeaters must report their protocol as the mode, not a blanket
        // "FM": a DMR machine derives "DMR", a P25 machine "P25", etc., while an
        // analog-only machine stays "FM". The fixture carries one machine per
        // protocol so every arm of `derive_mode` is exercised here.
        assert!(parsed
            .iter()
            .all(|p| p.dmr_color_code.is_none() || p.mode == "DMR"));
        assert!(parsed
            .iter()
            .all(|p| !p.p25_capable || p.mode != "FM"));
        for want in ["FM", "DMR", "DSTAR", "YSF", "NXDN", "P25", "M17"] {
            assert!(
                parsed.iter().any(|p| p.mode == want),
                "no {want} record in the sample"
            );
        }
        // A WIRES-X node is NOT a System Fusion flag: QQ0MMM is WIRES-equipped
        // analog and must stay FM (see the note in `parse_repeaterbook_json`).
        let wires_only = parsed
            .iter()
            .find(|p| p.callsign == "QQ0MMM")
            .expect("QQ0MMM missing");
        assert!(wires_only.wires_node.is_some());
        assert!(!wires_only.ysf_capable);
        assert_eq!(wires_only.mode, "FM");
        // Node numbers should be captured from the richer JSON fields.
        assert!(parsed.iter().any(|p| p.echolink_node.is_some()));
        assert!(parsed.iter().any(|p| p.allstar_node.is_some()));
        assert!(parsed.iter().any(|p| p.irlp_node.is_some()));
        // System Fusion (incl. Wires-X) should set the YSF flag.
        assert!(parsed.iter().any(|p| p.ysf_capable));
        // Country should be normalized away from the raw "US" code.
        assert!(parsed
            .iter()
            .any(|p| p.country.as_deref() == Some("United States")));
    }

    // ---- AnyTone radio-download import ----

    fn decoded_channel(index: usize, name: &str) -> AnytoneDecodedChannel {
        AnytoneDecodedChannel {
            index,
            name: name.into(),
            rx_mhz: 446.0,
            tx_mhz: 446.0,
            shift: String::new(),
            mode: "FM".into(),
            power: "Turbo".into(),
            bandwidth: "25 kHz".into(),
            tone: "—".into(),
            tone_tx: None,
            tone_rx: None,
            color_code: None,
            time_slot: None,
            contact_index: None,
            contact_name: None,
        }
    }

    #[test]
    fn anytone_tone_maps_to_chirp_scheme() {
        use AnytoneSubTone::{Ctcss, Dcs};
        let map = |tx, rx| map_anytone_tone(&tx, &rx);
        // No tone.
        assert_eq!(map(None, None).0, "off");
        // TX only → Tone.
        let (m, up, down, ..) = map(Some(Ctcss(100.0)), None);
        assert_eq!((m.as_str(), up, down), ("Tone", Some(100.0), None));
        // Equal both sides → TSQL.
        assert_eq!(map(Some(Ctcss(100.0)), Some(Ctcss(100.0))).0, "TSQL");
        // Split CTCSS → Cross Tone->Tone.
        let (m, up, down, _, _, cross) = map(Some(Ctcss(88.5)), Some(Ctcss(123.0)));
        assert_eq!((m.as_str(), cross.as_str()), ("Cross", "Tone->Tone"));
        assert_eq!((up, down), (Some(88.5), Some(123.0)));
        // RX-only CTCSS → Cross ->Tone.
        assert_eq!(map(None, Some(Ctcss(123.0))).5, "->Tone");
        // Equal DCS both sides → DTCS, zero-padded octal code (raw 265 = D411,
        // the HW-confirmed JACKRABBIT case).
        let (m, _, _, dtx, drx, _) = map(Some(Dcs(265)), Some(Dcs(265)));
        assert_eq!(m, "DTCS");
        assert_eq!((dtx.as_deref(), drx.as_deref()), (Some("411"), Some("411")));
        // Mixed CTCSS/DCS → Cross Tone->DTCS; raw 30 = D036 (GMRS 700 BUCK).
        let (m, up, _, _, drx, cross) = map(Some(Ctcss(100.0)), Some(Dcs(30)));
        assert_eq!((m.as_str(), cross.as_str()), ("Cross", "Tone->DTCS"));
        assert_eq!((up, drx.as_deref()), (Some(100.0), Some("036")));
    }

    #[test]
    fn maps_decoded_dmr_channel_to_schema_columns() {
        let mut ch = decoded_channel(118, "RMH700-FOCOBUCK");
        ch.rx_mhz = 445.2;
        ch.tx_mhz = 440.2;
        ch.mode = "DMR".into();
        ch.bandwidth = "12.5 kHz".into();
        ch.color_code = Some(7);
        ch.time_slot = Some(1);
        ch.contact_index = Some(23);
        let tg = AnytoneDecodedContact {
            index: 23,
            name: "700 RMH WIDE".into(),
            dmr_id: 700,
            call_type: 1,
        };
        let m = map_anytone_channel(&ch, Some(&tg));
        assert_eq!(m.name_long, "RMH700-FOCOBUCK");
        assert_eq!(m.name_short, "RMH700-"); // 7-char truncation
        assert_eq!(m.duplex, "-");
        assert!((m.offset - 5.0).abs() < 1e-9);
        assert_eq!(m.band, "UHF");
        assert_eq!(m.mode, "DMR");
        assert_eq!(m.tone_mode, "off");
        assert_eq!(m.power, None); // Turbo = radio max = NULL default
        assert_eq!(m.dmr_color_code, Some(7));
        assert_eq!(m.dmr_timeslot, Some(1));
        assert_eq!(m.dmr_talkgroup, Some(700));
        assert!(m.notes.contains("slot 118"));
    }

    #[test]
    fn maps_decoded_analog_channel_power_and_tone() {
        let mut ch = decoded_channel(1, "SCAN NC DMRGMRS");
        ch.power = "High".into();
        ch.tone_tx = Some(AnytoneSubTone::Ctcss(100.0));
        ch.tone_rx = Some(AnytoneSubTone::Ctcss(100.0));
        let m = map_anytone_channel(&ch, None);
        assert_eq!(m.mode, "FM");
        assert_eq!(m.duplex, "none"); // simplex
        assert_eq!(m.tone_mode, "TSQL");
        assert_eq!(m.ctcss_uplink, Some(100.0));
        assert_eq!(m.power.as_deref(), Some("High"));
        assert_eq!(m.dmr_talkgroup, None);

        // Narrow analog keeps its bandwidth as NFM (12.5 kHz round-trips).
        let mut narrow = decoded_channel(2, "NARROW FM");
        narrow.bandwidth = "12.5 kHz".into();
        assert_eq!(map_anytone_channel(&narrow, None).mode, "NFM");
    }

    #[tokio::test]
    async fn reimport_merges_corrects_mode_and_keeps_user_edits() {
        let dir = std::env::temp_dir().join(format!("cpm_merge_{}", std::process::id()));
        let db_path = dir.join("test.sqlite3");
        let _ = std::fs::remove_file(&db_path);
        let pool = crate::db::init_pool(&db_path).await.expect("init_pool");

        // Seed a stale row as the old buggy import would have: WIRES-X FM
        // repeater wrongly flagged YSF. The user has renamed it and overridden
        // the uplink tone (snapshot 88.5, user value 100.0).
        let rb_id = "N2SKY|448.4000|CO|FORT COLLINS";
        sqlx::query(
            "INSERT INTO channels (rb_name, name_long, name_short, callsign, rx_freq, \
             tx_freq, mode, ysf_capable, ctcss_uplink, rb_ctcss_uplink, \
             ctcss_uplink_overridden, has_overrides, source, repeaterbook_id, state, city) \
             VALUES ('N2SKY Fort Collins', 'N2SKY Fort Collins', 'MY NAME', 'N2SKY', \
             448.4, 443.4, 'YSF', 1, 100.0, 88.5, 1, 1, 'repeaterbook', ?1, 'CO', 'Fort Collins')",
        )
        .bind(rb_id)
        .execute(&pool)
        .await
        .expect("seed");

        // Re-import the corrected RepeaterBook record (FM, WIRES-X, no Fusion).
        let json = r#"{"records":[{
            "freq_mhz":"448.400","input_freq":"443.400","callsign":"N2SKY",
            "state":"CO","city":"Fort Collins","pl_tone":"88.5",
            "ysf":"No","wires":"Yes","wires_node":"12345"
        }]}"#;
        let jpath = dir.join("reimport.json");
        std::fs::write(&jpath, json).expect("write json");
        let parsed = parse_repeaterbook_json(jpath.to_str().unwrap()).expect("parse");
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].repeaterbook_id, rb_id, "dedupe id must match seed");

        let summary = insert_parsed(&pool, &parsed).await.expect("merge");
        assert_eq!((summary.added, summary.updated, summary.skipped), (0, 1, 0));

        let (mode, ysf, name_short, tone, rb_tone, over): (
            String,
            bool,
            String,
            Option<f64>,
            Option<f64>,
            bool,
        ) = sqlx::query_as(
            "SELECT mode, ysf_capable, name_short, ctcss_uplink, rb_ctcss_uplink, \
             ctcss_uplink_overridden FROM channels WHERE repeaterbook_id = ?1",
        )
        .bind(rb_id)
        .fetch_one(&pool)
        .await
        .expect("row");

        // Mode corrected, YSF flag cleared.
        assert_eq!(mode, "FM");
        assert!(!ysf);
        // User's custom name preserved.
        assert_eq!(name_short, "MY NAME");
        // Overridden tone preserved; snapshot advanced (RB still reports 88.5);
        // override flag stays set because the user value (100.0) still differs.
        assert_eq!(tone, Some(100.0));
        assert_eq!(rb_tone, Some(88.5));
        assert!(over);

        // Exactly one row — the merge updated in place, did not duplicate.
        let count: (i64,) =
            sqlx::query_as("SELECT COUNT(*) FROM channels WHERE repeaterbook_id = ?1")
                .bind(rb_id)
                .fetch_one(&pool)
                .await
                .expect("count");
        assert_eq!(count.0, 1);
    }

    #[tokio::test]
    async fn same_call_freq_state_different_city_import_as_two_rows() {
        // N2SKY 448.400 exists in both Fort Collins and Buena Vista, CO. They
        // share callsign, frequency and state, so the city must keep them
        // distinct — both should import, not collapse into one.
        let dir = std::env::temp_dir().join(format!("cpm_twocity_{}", std::process::id()));
        let db_path = dir.join("test.sqlite3");
        let _ = std::fs::remove_file(&db_path);
        let pool = crate::db::init_pool(&db_path).await.expect("init_pool");

        let json = r#"{"records":[
            {"freq_mhz":"448.400000","input_freq":"443.400000","callsign":"N2SKY",
             "state":"CO","city":"Fort Collins","county":"Larimer","wires":"Yes"},
            {"freq_mhz":"448.400000","input_freq":"443.400000","callsign":"N2SKY",
             "state":"CO","city":"Buena Vista","county":"Chaffee"}
        ]}"#;
        let jpath = dir.join("twocity.json");
        std::fs::write(&jpath, json).expect("write json");
        let parsed = parse_repeaterbook_json(jpath.to_str().unwrap()).expect("parse");
        assert_eq!(parsed.len(), 2);
        assert_ne!(
            parsed[0].repeaterbook_id, parsed[1].repeaterbook_id,
            "city must make the two dedupe ids distinct",
        );

        let summary = insert_parsed(&pool, &parsed).await.expect("insert");
        assert_eq!((summary.added, summary.updated), (2, 0));

        let cities: Vec<(String,)> = sqlx::query_as(
            "SELECT city FROM channels WHERE callsign = 'N2SKY' ORDER BY city",
        )
        .fetch_all(&pool)
        .await
        .expect("rows");
        let cities: Vec<&str> = cities.iter().map(|(c,)| c.as_str()).collect();
        assert_eq!(cities, vec!["Buena Vista", "Fort Collins"]);

        // A second import of the same file merges both, never duplicates.
        let again = insert_parsed(&pool, &parsed).await.expect("reimport");
        assert_eq!((again.added, again.updated), (0, 2));
        let count: (i64,) =
            sqlx::query_as("SELECT COUNT(*) FROM channels WHERE callsign = 'N2SKY'")
                .fetch_one(&pool)
                .await
                .expect("count");
        assert_eq!(count.0, 2);
    }

    #[tokio::test]
    async fn imports_anytone_download_and_is_idempotent() {
        let dir = std::env::temp_dir().join(format!("cpm_any_import_{}", std::process::id()));
        let db_path = dir.join("test.sqlite3");
        let _ = std::fs::remove_file(&db_path);
        let pool = crate::db::init_pool(&db_path).await.expect("init_pool");

        let mut dmr = decoded_channel(2, "IMPORT DMR");
        dmr.rx_mhz = 445.2;
        dmr.tx_mhz = 440.2;
        dmr.mode = "DMR".into();
        dmr.color_code = Some(7);
        dmr.time_slot = Some(2);
        dmr.contact_index = Some(5);
        let mut analog = decoded_channel(1, "IMPORT ANALOG");
        analog.tone_tx = Some(AnytoneSubTone::Ctcss(100.0));
        let channels = vec![analog, dmr];
        let zones = vec![AnytoneDecodedZone {
            index: 1,
            name: "IMPORT TEST ZONE".into(),
            channels: vec!["IMPORT ANALOG".into(), "IMPORT DMR".into()],
            member_slots: vec![0, 1],
        }];
        let contacts = vec![
            AnytoneDecodedContact {
                index: 5,
                name: "IMPORT TG".into(),
                // A number no seeded Brandmeister TG uses, so it inserts.
                dmr_id: 99_999_901,
                call_type: 1,
            },
            AnytoneDecodedContact {
                index: 6,
                name: "ALL CALL".into(),
                dmr_id: 16_777_215,
                call_type: 2, // All Call → not imported as a talkgroup
            },
        ];

        let s = import_anytone(&pool, &channels, &zones, &contacts)
            .await
            .expect("import");
        assert_eq!(
            (s.channels_added, s.talkgroups_added, s.lists_added),
            (2, 1, 1)
        );
        assert_eq!((s.channels_skipped, s.lists_skipped), (0, 0));

        // The DMR channel carries its talkgroup number + slot/color code.
        let (tg, ts, mode): (Option<i64>, Option<i64>, String) = sqlx::query_as(
            "SELECT dmr_talkgroup, dmr_timeslot, mode FROM channels WHERE name_long = 'IMPORT DMR'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!((tg, ts, mode.as_str()), (Some(99_999_901), Some(2), "DMR"));

        // The zone became a channel list with both members in order.
        let entries: Vec<(i64, String)> = sqlx::query_as(
            "SELECT e.position, c.name_long FROM channel_list_entries e
             JOIN channel_lists l ON l.id = e.channel_list_id
             JOIN channels c ON c.id = e.channel_id
             WHERE l.name = 'IMPORT TEST ZONE' ORDER BY e.position",
        )
        .fetch_all(&pool)
        .await
        .unwrap();
        assert_eq!(
            entries,
            vec![(0, "IMPORT ANALOG".to_string()), (1, "IMPORT DMR".to_string())]
        );

        // Re-importing the same download adds nothing.
        let s2 = import_anytone(&pool, &channels, &zones, &contacts)
            .await
            .expect("re-import");
        assert_eq!(
            (s2.channels_added, s2.talkgroups_added, s2.lists_added),
            (0, 0, 0)
        );
        assert_eq!(
            (s2.channels_skipped, s2.talkgroups_skipped, s2.lists_skipped),
            (2, 1, 1)
        );

        let _ = std::fs::remove_file(&db_path);
    }

    #[test]
    fn derives_dmr_mode_from_flags() {
        let json = r#"{"schema_version":1,"export_format":"json",
            "generated_at":"2026-06-18T19:54:15Z",
            "template":{"template_id":173,"name":"Full Data"},
            "fields":[],"count":2,"records":[
              {"callsign":"KG0RP","freq_mhz":"439.800000","input_freq":"434.800000",
               "city":"Loveland","state":"CO","dmr":"Yes","dmr_cc":"11"},
              {"callsign":"W0FM","freq_mhz":"146.940000","input_freq":"146.340000",
               "city":"Denver","state":"CO"}
            ]}"#;
        let dir = std::env::temp_dir();
        let path = dir.join("cpm_dmr_test.json");
        std::fs::write(&path, json).unwrap();
        let parsed = parse_repeaterbook_json(path.to_str().unwrap()).expect("parse failed");
        std::fs::remove_file(&path).ok();

        let dmr = parsed.iter().find(|p| p.callsign == "KG0RP").unwrap();
        assert_eq!(dmr.mode, "DMR");
        assert_eq!(dmr.dmr_color_code, Some(11));
        let fm = parsed.iter().find(|p| p.callsign == "W0FM").unwrap();
        assert_eq!(fm.mode, "FM");
    }
}
