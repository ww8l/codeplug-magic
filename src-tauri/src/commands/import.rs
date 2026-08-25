use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::{Acquire, SqlitePool};
use tauri::State;

use super::rb_regions;
use super::anytone::{
    AnytoneDecodedChannel, AnytoneDecodedContact, AnytoneDecodedZone, AnytoneSubTone,
};
use crate::db::AppState;
use crate::error::MapErrString;
use crate::models::ImportSummary;
use crate::util::{
    derive_band, derive_duplex, derive_tone_mode, differs_from_rb_f64, differs_from_rb_str,
    gen_name_long, gen_name_short, keeps_dcs, repair_truncated_tx, truncate,
};

/// Which optional columns the export a record came from actually carries.
///
/// `Option::None` is ambiguous on its own and the two readings need opposite
/// handling on a re-import: in a column the export *has*, None means
/// RepeaterBook reports nothing there and the stored value should follow it
/// down; in a column the export does not have, None means we know nothing and
/// whatever is stored must survive untouched.
///
/// Only the premium JSON carries link node numbers, and only the wide CSV
/// carries Operational Status, so every export is missing something here.
#[derive(Debug, Clone, Copy)]
pub(crate) struct SourceColumns {
    /// allstar_node, echolink_node, irlp_node, wires_node.
    pub(crate) link_nodes: bool,
    pub(crate) operational_status: bool,
    /// Whether the tone columns can express DCS. Only the free-tier CSV can;
    /// for the other two a stored DCS scheme can only be an operator's edit,
    /// which is the assumption `keeps_dcs` was written on.
    pub(crate) dcs: bool,
}

/// A single RepeaterBook record (from CSV or JSON) parsed and mapped into our
/// schema, ready to be inserted or previewed. Serialized directly to the import
/// preview so the UI can show every field that will be imported.
#[derive(Debug, Clone, Serialize)]
pub struct ParsedChannel {
    pub(crate) repeaterbook_id: String,
    pub(crate) rb_name: String,
    pub(crate) name_long: String,
    pub(crate) name_short: String,
    pub(crate) callsign: String,
    pub(crate) rx_freq: f64,
    pub(crate) tx_freq: Option<f64>,
    pub(crate) offset: f64,
    pub(crate) duplex: String,
    pub(crate) band: String,
    pub(crate) mode: String,
    pub(crate) tone_mode: String,
    pub(crate) cross_mode: String,
    pub(crate) ctcss_uplink: Option<f64>,
    pub(crate) ctcss_downlink: Option<f64>,
    /// TX DCS code, 3-digit octal (migration 0008). Only the standard CSV
    /// supplies these — the "Full Data" JSON has no DCS field at all.
    pub(crate) dcs_code: Option<String>,
    /// RX DCS code, 3-digit octal. `None` with `dcs_code` set means the same
    /// code both ways, which is stored as tone_mode `DTCS`.
    pub(crate) dcs_rx_code: Option<String>,
    pub(crate) dmr_color_code: Option<i64>,
    /// The three fields no RepeaterBook export has a column for. They stay
    /// `None` on every RepeaterBook path and are filled only by a mapped
    /// import of the operator's own CSV (`csv_map`).
    pub(crate) dmr_timeslot: Option<i64>,
    pub(crate) dmr_talkgroup: Option<i64>,
    pub(crate) power: Option<String>,
    pub(crate) dstar_capable: bool,
    pub(crate) ysf_capable: bool,
    pub(crate) nxdn_capable: bool,
    pub(crate) p25_capable: bool,
    pub(crate) p25_nac: Option<String>,
    pub(crate) m17_capable: bool,
    pub(crate) tetra_capable: bool,
    pub(crate) allstar_node: Option<String>,
    pub(crate) echolink_node: Option<String>,
    pub(crate) irlp_node: Option<String>,
    pub(crate) wires_node: Option<String>,
    pub(crate) use_type: Option<String>,
    pub(crate) operational_status: Option<String>,
    pub(crate) city: Option<String>,
    pub(crate) county: Option<String>,
    pub(crate) state: Option<String>,
    pub(crate) country: Option<String>,
    pub(crate) latitude: Option<f64>,
    pub(crate) longitude: Option<f64>,
    pub(crate) notes: Option<String>,
    /// Not part of the preview: this describes the *export*, not the channel.
    #[serde(skip)]
    pub(crate) covers: SourceColumns,
}

/// The full parsed result shown before confirming an import. `rows` is capped
/// for very large files; `total` is the true count that will be imported.
#[derive(Debug, Clone, Serialize)]
pub struct ImportPreview {
    pub total: usize,
    pub rows: Vec<ParsedChannel>,
}

/// Cap on preview rows sent to the UI, to keep large state-wide exports light.
pub(crate) const PREVIEW_CAP: usize = 1000;

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
pub(crate) fn build_preview(parsed: &[ParsedChannel]) -> ImportPreview {
    ImportPreview {
        total: parsed.len(),
        rows: parsed.iter().take(PREVIEW_CAP).cloned().collect(),
    }
}

/// The subset of an existing channel row needed to merge a re-import: the four
/// user-overridable RepeaterBook fields and their override flags, plus the
/// stored tone scheme so a DCS one survives the merge (see [`keeps_dcs`]).
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
    tone_mode: Option<String>,
    cross_mode: String,
    dcs_code: Option<String>,
    dcs_rx_code: Option<String>,
    /// What RepeaterBook last said. A stored code that differs from its
    /// snapshot is the operator's edit; one that matches came from an import
    /// and is ours to refresh. NULL pre-dates migration 0019 and reads as
    /// "differs", which preserves whatever is stored.
    rb_dcs_code: Option<String>,
    rb_dcs_rx_code: Option<String>,
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
    (value, rb, differs_from_rb_f64(value, rb))
}

/// String twin of [`merge_tracked_f64`].
fn merge_tracked_str(
    overridden: bool,
    current: Option<String>,
    rb: Option<String>,
) -> (Option<String>, Option<String>, bool) {
    let value = if overridden { current } else { rb.clone() };
    let over = differs_from_rb_str(&value, &rb);
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
             operational_status_overridden, notes_overridden, \
             tone_mode, cross_mode, dcs_code, dcs_rx_code, \
             rb_dcs_code, rb_dcs_rx_code \
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
                ctcss_downlink, dcs_code, dcs_rx_code,
                rb_dcs_code, rb_dcs_rx_code,
                dmr_color_code, dstar_capable, ysf_capable,
                nxdn_capable, p25_capable, p25_nac, m17_capable, tetra_capable,
                allstar_node, echolink_node, irlp_node, wires_node,
                use_type, operational_status, service_type,
                city, county, state, country, latitude, longitude, notes,
                source, repeaterbook_id,
                rb_ctcss_uplink, rb_ctcss_downlink, rb_operational_status,
                rb_notes, cross_mode, last_rb_update
            ) VALUES (
                ?1, ?2, ?3, ?4, ?5, ?6,
                ?7, ?8, ?9, ?10, ?11, ?12,
                ?13, ?14, ?15,
                ?43, ?44,
                ?16, ?17, ?18,
                ?19, ?20, ?21, ?22, ?23,
                ?24, ?25, ?26, ?27,
                ?28, ?29, 'Amateur',
                ?30, ?31, ?32, ?33, ?34, ?35, ?36,
                'repeaterbook', ?37,
                ?38, ?39, ?40,
                ?41, ?42, CURRENT_TIMESTAMP
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
        .bind(&p.dcs_code)
        .bind(&p.dcs_rx_code)
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
        .bind(&p.repeaterbook_id)
        .bind(p.ctcss_uplink) // rb_ctcss_uplink snapshot
        .bind(p.ctcss_downlink) // rb_ctcss_downlink snapshot
        .bind(&p.operational_status) // rb_operational_status snapshot
        .bind(&p.notes) // rb_notes snapshot
        .bind(&p.cross_mode)
        // rb_dcs_* snapshots: what this export said, so a later import can tell
        // an operator's edit from one of ours.
        .bind(&p.dcs_code) // ?43
        .bind(&p.dcs_rx_code) // ?44
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
///     tx_freq/offset/duplex/band, city/county/country) are refreshed from RB —
///     this is what corrects a stale/mis-flagged mode.
///   * The four user-overridable fields (uplink/downlink tone, operational
///     status, notes) keep the user's value when overridden; otherwise they
///     adopt the fresh RB value. Their `rb_*` snapshots always advance and the
///     override flags are recomputed. `tone_mode`/`cross_mode` are re-derived
///     from the merged tone pair so they stay consistent with a kept override —
///     except on a DCS scheme, which is kept verbatim (see [`keeps_dcs`]).
///   * User-facing names (`name_long`, `name_short`), DMR slot/talkgroup and
///     power are NOT touched — those are the operator's to curate.
///   * DCS is the operator's to curate *once they have curated it*. The
///     free-tier CSV supplies DCS, so a stored code is compared against its
///     `rb_dcs_*` snapshot: matching means an import wrote it and a fresh
///     export may refresh it, differing means the operator typed it and it
///     stays. Only an export that can express DCS gets a say.
///   * `latitude`/`longitude` use COALESCE, and the link nodes and
///     `operational_status` are gated on `SourceColumns`, so an export that
///     has no such column never wipes what another export established.
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
    // An export with no Status column says nothing about status, so it must not
    // drive the tracked triple: merge_tracked_str would read its None as "RB
    // reports nothing", blank the field AND its rb_ snapshot, and reset the
    // override flag — losing the baseline that decides future overrides. Both
    // real exports lack the column, so this is the common path, not the corner.
    let (operational_status, rb_status, status_over) = if p.covers.operational_status {
        merge_tracked_str(
            ex.operational_status_overridden,
            ex.operational_status.clone(),
            p.operational_status.clone(),
        )
    } else {
        (
            ex.operational_status.clone(),
            None, // ignored by the CASE below; the stored snapshot is kept
            ex.operational_status_overridden,
        )
    };
    let (notes, rb_notes, notes_over) =
        merge_tracked_str(ex.notes_overridden, ex.notes.clone(), p.notes.clone());
    let has_overrides = up_over || dn_over || status_over || notes_over;

    // Is the stored DCS the operator's, or one an import wrote? Comparing
    // against the snapshot is the same test the tracked tone fields use. A row
    // predating migration 0019 has NULL snapshots and so reads as the
    // operator's, which preserves whatever it holds.
    let dcs_curated = ex.dcs_code != ex.rb_dcs_code || ex.dcs_rx_code != ex.rb_dcs_rx_code;

    // An export whose tone columns can express DCS describes the machine's
    // squelch completely: what it omits is genuinely absent, not merely
    // unknown. So its scheme is adopted whole — including moving a channel OFF
    // DCS when RepeaterBook now lists CTCSS, which is the case that otherwise
    // sticks forever and programs a tone the repeater no longer uses.
    //
    // Not adopted when the operator has curated the tones or the DCS codes
    // themselves; their edit wins, exactly as it does for CTCSS.
    let adopt_rb_dcs = p.covers.dcs && !dcs_curated && !up_over && !dn_over;

    let (tone_mode, cross_mode) = if adopt_rb_dcs {
        (p.tone_mode.clone(), p.cross_mode.clone())
    } else if keeps_dcs(ex.tone_mode.as_deref(), &ex.cross_mode) {
        // The export cannot describe DCS, so a stored DCS scheme can only be
        // the operator's — or one an earlier CSV import wrote and this export
        // has no standing to contradict. Keep it verbatim; re-deriving from a
        // CTCSS pair would silently destroy it (issue #71).
        (
            ex.tone_mode.clone().unwrap_or_else(|| "off".to_string()),
            ex.cross_mode.clone(),
        )
    } else {
        derive_tone_mode(ctcss_uplink, ctcss_downlink)
    };

    sqlx::query(
        r#"
        UPDATE channels SET
            rb_name = ?2, tx_freq = ?3, offset = ?4, duplex = ?5, band = ?6,
            mode = ?7, tone_mode = ?8, cross_mode = ?9,
            ctcss_uplink = ?10, ctcss_downlink = ?11,
            dmr_color_code = ?12, dstar_capable = ?13, ysf_capable = ?14,
            nxdn_capable = ?15, p25_capable = ?16, p25_nac = ?17,
            m17_capable = ?18, tetra_capable = ?19,
            -- Gated on ?43/?44, not COALESCE. COALESCE would also block the
            -- one export that DOES carry these from ever clearing a value
            -- RepeaterBook has removed; the flag distinguishes "the export
            -- says empty" from "the export has no such column".
            allstar_node = CASE WHEN ?43 THEN ?20 ELSE allstar_node END,
            echolink_node = CASE WHEN ?43 THEN ?21 ELSE echolink_node END,
            irlp_node = CASE WHEN ?43 THEN ?22 ELSE irlp_node END,
            wires_node = CASE WHEN ?43 THEN ?23 ELSE wires_node END,
            use_type = COALESCE(?24, use_type),
            operational_status = CASE WHEN ?44 THEN ?25 ELSE operational_status END,
            city = ?26, county = ?27, country = ?28,
            -- COALESCE, not a straight write: an export with no such column
            -- reports None and must leave what another export established.
            -- The free-tier CSV carries neither a Use column nor coordinates.
            latitude = COALESCE(?29, latitude),
            longitude = COALESCE(?30, longitude),
            -- Gated, not COALESCE: COALESCE could never clear either code, so
            -- a cross-DCS scheme whose RX half RepeaterBook dropped would
            -- squelch on a code the repeater no longer sends, permanently.
            dcs_code = CASE WHEN ?45 THEN ?31 ELSE dcs_code END,
            dcs_rx_code = CASE WHEN ?45 THEN ?32 ELSE dcs_rx_code END,
            rb_dcs_code = CASE WHEN ?46 THEN ?31 ELSE rb_dcs_code END,
            rb_dcs_rx_code = CASE WHEN ?46 THEN ?32 ELSE rb_dcs_rx_code END,
            notes = ?33,
            rb_ctcss_uplink = ?34, rb_ctcss_downlink = ?35,
            rb_operational_status = CASE WHEN ?44 THEN ?36 ELSE rb_operational_status END,
            rb_notes = ?37,
            ctcss_uplink_overridden = ?38, ctcss_downlink_overridden = ?39,
            operational_status_overridden = ?40, notes_overridden = ?41,
            has_overrides = ?42,
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
    .bind(&p.dcs_code)
    .bind(&p.dcs_rx_code)
    .bind(&notes)
    .bind(rb_up)
    .bind(rb_dn)
    .bind(&rb_status)
    .bind(&rb_notes)
    .bind(up_over)
    .bind(dn_over)
    .bind(status_over)
    .bind(notes_over)
    .bind(has_overrides)
    .bind(p.covers.link_nodes) // ?43
    .bind(p.covers.operational_status) // ?44
    .bind(adopt_rb_dcs) // ?45
    // The snapshot advances whenever the export could see DCS at all, even if
    // the operator's edit is what stays in the value — otherwise a curated
    // channel never learns what RepeaterBook currently says and stays "curated"
    // for ever, including after the operator reverts to RB's own code.
    .bind(p.covers.dcs) // ?46
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
pub(crate) fn finalize(
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

/// Pick the primary `mode` string from the digital-capability flags. RepeaterBook
/// has no explicit mode column, so a repeater with no digital flag is plain "FM"
/// and a digital one reports its protocol. A machine can be mixed-mode; we surface
/// the first protocol in this fixed precedence so mode filtering/UI behaves.
pub(crate) fn derive_mode(
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
pub(crate) struct ParsedChannelStub {
    pub(crate) repeaterbook_id: String,
    pub(crate) rb_name: String,
    pub(crate) name_long: String,
    pub(crate) name_short: String,
    pub(crate) tx_freq: Option<f64>,
    pub(crate) duplex: String,
    pub(crate) offset: f64,
    pub(crate) band: String,
    pub(crate) tone_mode: String,
    pub(crate) cross_mode: String,
}

// ============================================================
// CSV parser
// ============================================================
/// Which RepeaterBook CSV shape a header row is, if either.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RbCsvShape {
    /// The free-tier export: leads with `Output Freq`.
    Standard,
    /// The wide export: leads with `Frequency`, same field set as the premium
    /// "Full Data" JSON.
    Full,
}

impl RbCsvShape {
    /// What to call the shape in the UI.
    pub(crate) fn label(self) -> &'static str {
        match self {
            RbCsvShape::Standard => "RepeaterBook CSV export",
            RbCsvShape::Full => "RepeaterBook \"Full Data\" CSV export",
        }
    }
}

/// Recognise a header row as one of the two RepeaterBook CSV shapes.
///
/// RepeaterBook's free export leads with `Output Freq`; the wide shape leads
/// with `Frequency`. Nothing else distinguishes them, and they share no column
/// name for the frequency, so a file handed to the wrong parser yields zero
/// channels without reporting an error — which is exactly what a real free-tier
/// export did before this split existed.
///
/// `Frequency` alone is *not* enough for the wide shape: it is also what CHIRP
/// and half the world's own spreadsheets call their frequency column, and those
/// files are the ones issue #115's column mapper exists for. Requiring `Call`
/// plus one wide-shape-only column keeps them out. This is the single
/// definition of "recognised" — [`super::csv_map::inspect_csv`] asks the same
/// question before offering the mapper.
pub(crate) fn recognize_csv(headers: &csv::StringRecord) -> Option<RbCsvShape> {
    let has = |name: &str| {
        headers
            .iter()
            .any(|h| h.trim().eq_ignore_ascii_case(name))
    };
    if has("Output Freq") {
        Some(RbCsvShape::Standard)
    } else if has("Frequency")
        && has("Call")
        && (has("PL") || has("TSQ") || has("Operational Status"))
    {
        Some(RbCsvShape::Full)
    } else {
        None
    }
}

/// Read just the header row of a CSV.
pub(crate) fn csv_headers(path: &str) -> Result<csv::StringRecord, String> {
    let mut probe = csv::ReaderBuilder::new()
        .flexible(true)
        .trim(csv::Trim::All)
        .from_path(path)
        .map_err(|e| format!("Could not open CSV: {e}"))?;
    Ok(probe.headers().estr()?.clone())
}

/// Route a `.csv` to the parser for its shape.
///
/// An unrecognised header row is an error rather than a guess: it used to fall
/// through to the wide parser, which happily returned rows with an empty
/// callsign and no tones for any CSV that merely had a `Frequency` column. The
/// UI routes such a file to the column mapper instead and never gets here.
pub(crate) fn parse_repeaterbook_csv(path: &str) -> Result<Vec<ParsedChannel>, String> {
    match recognize_csv(&csv_headers(path)?) {
        Some(RbCsvShape::Standard) => parse_repeaterbook_standard_csv(path),
        Some(RbCsvShape::Full) => parse_repeaterbook_full_csv(path),
        None => Err(
            "This CSV is not a RepeaterBook export. Map its columns to channel \
             fields instead."
                .to_string(),
        ),
    }
}

/// Parse the wide RepeaterBook CSV shape at `path`.
///
/// This carries the same field set as the premium "Full Data" JSON — per-mode
/// columns, Lat/Long, ARES/RACES/SKYWARN, Landmark. No export we have has this
/// shape (the premium download we have seen is JSON), so unlike the standard
/// parser below, nothing here has been checked against a real file.
fn parse_repeaterbook_full_csv(path: &str) -> Result<Vec<ParsedChannel>, String> {
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
            dcs_code: None,
            dcs_rx_code: None,
            covers: SourceColumns { link_nodes: false, operational_status: true, dcs: false },
            dmr_color_code,
            // No RepeaterBook export has a column for these three.
            dmr_timeslot: None,
            dmr_talkgroup: None,
            power: None,
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
// Standard (free-tier) CSV parser
// ============================================================
// RepeaterBook offers two downloads. The premium "Full Data" export gives every
// field and is what `parse_repeaterbook_json` handles; the free CSV gives
// eleven columns and is what most users can actually get. They share no column
// name for the frequency, so the two shapes are told apart by their header:
//
//   standard : Output Freq,Input Freq,Offset,Uplink Tone,Downlink Tone,Call,
//              Location,County,State,Modes,Digital Access
//
// Built and verified against three real exports (1,104 rows, two dates two
// months apart). Everything asserted below was observed in those files; where
// a rule is inferred rather than seen, the comment says so.

/// One RepeaterBook tone cell. The column holds a CTCSS frequency, a DCS code
/// written `D073`, the literal `CSQ` for carrier squelch, or nothing at all.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum RbTone {
    /// Blank or `CSQ` — no tone. RepeaterBook uses both; they mean the same.
    None,
    Ctcss(f64),
    /// 3-digit octal, matching how `dcs_code` is stored (migration 0008).
    Dcs(String),
}

/// Read one tone cell.
///
/// `CSQ` (carrier squelch) is an explicit "no tone" and must not be mistaken
/// for a missing value that some later step might fill in. A `Dxxx` code is
/// DCS: RepeaterBook does supply these, despite what the free-tier column
/// header suggests, and dropping them leaves a channel that cannot key its
/// repeater.
pub(crate) fn parse_rb_tone(cell: Option<String>) -> RbTone {
    let raw = match cell {
        Some(v) => v.trim().to_string(),
        None => return RbTone::None,
    };
    if raw.is_empty() || raw.eq_ignore_ascii_case("CSQ") {
        return RbTone::None;
    }
    if let Some(rest) = raw.strip_prefix(['D', 'd']) {
        // DCS codes are conventionally written in octal, which is also how the
        // channels table stores them, so the digits carry across unchanged.
        // Anything with an 8 or a 9 is not octal and is not a code we know how
        // to store, so it is dropped rather than guessed at.
        if !rest.is_empty() && rest.len() <= 3 && rest.bytes().all(|b| (b'0'..=b'7').contains(&b)) {
            return RbTone::Dcs(format!("{rest:0>3}"));
        }
        return RbTone::None;
    }
    match parse_leading_f64(&raw) {
        Some(f) => RbTone::Ctcss(f),
        None => RbTone::None,
    }
}

/// The tone scheme for an uplink/downlink pair that may be CTCSS or DCS.
///
/// Returns `(tone_mode, cross_mode, dcs_code, dcs_rx_code)`. The CTCSS-only
/// cases match `derive_tone_mode` exactly — this is a superset of it, not a
/// replacement — and the DCS cases follow the storage convention already in the
/// database: `DTCS` for the same code both ways with only `dcs_code` set, and
/// `Cross` with an explicit `cross_mode` for anything mixed.
pub(crate) fn derive_tone_mode_rb(
    up: &RbTone,
    dn: &RbTone,
) -> (String, String, Option<String>, Option<String>) {
    let m = |mode: &str, cross: &str, tx: Option<&str>, rx: Option<&str>| {
        (
            mode.to_string(),
            cross.to_string(),
            tx.map(str::to_string),
            rx.map(str::to_string),
        )
    };
    match (up, dn) {
        (RbTone::None, RbTone::None) => m("off", "Tone->Tone", None, None),

        // CTCSS only: identical to derive_tone_mode's behaviour.
        (RbTone::Ctcss(u), RbTone::Ctcss(d)) if (u - d).abs() < 0.05 => {
            m("TSQL", "Tone->Tone", None, None)
        }
        (RbTone::Ctcss(_), RbTone::Ctcss(_)) => m("Cross", "Tone->Tone", None, None),
        (RbTone::Ctcss(_), RbTone::None) => m("Tone", "Tone->Tone", None, None),
        (RbTone::None, RbTone::Ctcss(_)) => m("Cross", "->Tone", None, None),

        // Same DCS code both ways is the common case, and is stored as plain
        // DTCS with a single code rather than as a cross scheme.
        (RbTone::Dcs(u), RbTone::Dcs(d)) if u == d => {
            m("DTCS", "Tone->Tone", Some(u), None)
        }
        (RbTone::Dcs(u), RbTone::Dcs(d)) => m("Cross", "DTCS->DTCS", Some(u), Some(d)),
        (RbTone::Dcs(u), RbTone::None) => m("Cross", "DTCS->", Some(u), None),
        (RbTone::Dcs(u), RbTone::Ctcss(_)) => m("Cross", "DTCS->Tone", Some(u), None),
        (RbTone::Ctcss(_), RbTone::Dcs(d)) => m("Cross", "Tone->DTCS", None, Some(d)),
        (RbTone::None, RbTone::Dcs(d)) => m("Cross", "->DTCS", None, Some(d)),
    }
}

/// The digital/link services named in the `Modes` column.
#[derive(Debug, Default, Clone, PartialEq)]
struct RbModes {
    dmr: bool,
    dstar: bool,
    ysf: bool,
    nxdn: bool,
    p25: bool,
    m17: bool,
    tetra: bool,
    /// Link and other services with nowhere of their own to live, kept in the
    /// order RepeaterBook listed them so they can go into the notes.
    extras: Vec<String>,
}

/// Split the space-separated `Modes` cell.
///
/// The whole vocabulary seen across three real exports is `FM DMR DSTAR Fusion
/// WIRES-X AllStar EchoLink IRLP P-25 ATV`. RepeaterBook's spellings differ
/// from ours (`DSTAR` not `D-Star`, `P-25` not `P25`, `Fusion` not `System
/// Fusion`), and NXDN, M17 and TETRA are accepted here without having been
/// observed, on the grounds that RepeaterBook lists all three elsewhere.
///
/// An unrecognised token is kept as an extra rather than dropped: a mode we
/// have never seen is worth showing the operator, and silently discarding it
/// would be indistinguishable from a parser bug.
fn parse_rb_modes(cell: Option<String>) -> RbModes {
    let mut out = RbModes::default();
    let raw = cell.unwrap_or_default();
    for tok in raw.split_whitespace() {
        match tok.to_ascii_uppercase().as_str() {
            "FM" | "ANALOG" => {}
            "DMR" => out.dmr = true,
            "DSTAR" | "D-STAR" => out.dstar = true,
            "FUSION" | "YSF" | "C4FM" => out.ysf = true,
            "NXDN" => out.nxdn = true,
            "P-25" | "P25" => out.p25 = true,
            "M17" => out.m17 = true,
            "TETRA" => out.tetra = true,
            _ => out.extras.push(tok.to_string()),
        }
    }
    out
}

/// Parse RepeaterBook's free-tier CSV export at `path`.
fn parse_repeaterbook_standard_csv(path: &str) -> Result<Vec<ParsedChannel>, String> {
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

        let rx_freq = match get(&rec, "Output Freq").and_then(|s| parse_leading_f64(&s)) {
            Some(f) => f,
            None => continue,
        };
        // The Offset column ("+", "-", or "s" for a split) is deliberately
        // unused: both frequencies arrive at full precision here, so
        // derive_duplex reads the real relationship straight off them. Trusting
        // the column instead would misread the three 900 MHz splits in the
        // sample as simplex.
        let tx_freq = get(&rec, "Input Freq").and_then(|s| parse_leading_f64(&s));
        let callsign = get(&rec, "Call").unwrap_or_default();

        // "Colorado Springs - Cheyenne Mtn" is one column here and two fields in
        // the premium export. Splitting is what lets a CSV import land on the
        // same synthetic id as a JSON import of the same repeater; without it
        // the whole library duplicates. Verified across 212 distinct Locations:
        // every one either has exactly one " - " or none, and none has a
        // hyphen without the surrounding spaces.
        let location = get(&rec, "Location");
        let (city, landmark) = match location {
            Some(l) => match l.split_once(" - ") {
                Some((c, lm)) => (Some(c.trim().to_string()), Some(lm.trim().to_string())),
                None => (Some(l), None),
            },
            None => (None, None),
        };

        // Spelled-out name -> postal code, so the dedupe id matches the JSON's.
        // An unknown region keeps its name rather than being dropped.
        let raw_state = get(&rec, "State");
        let region = raw_state.as_deref().and_then(rb_regions::lookup);
        let state = match region {
            Some((code, _)) => Some(code.to_string()),
            None => raw_state.clone(),
        };
        let country = region.map(|(_, c)| c.to_string());

        let up = parse_rb_tone(get(&rec, "Uplink Tone"));
        let dn = parse_rb_tone(get(&rec, "Downlink Tone"));
        let (tone_mode, cross_mode, dcs_code, dcs_rx_code) = derive_tone_mode_rb(&up, &dn);
        let ctcss_uplink = match up {
            RbTone::Ctcss(f) => Some(f),
            _ => None,
        };
        let ctcss_downlink = match dn {
            RbTone::Ctcss(f) => Some(f),
            _ => None,
        };

        let modes = parse_rb_modes(get(&rec, "Modes"));

        // One column, two meanings: a DMR colour code (0-15) or a P25 NAC.
        // Which one it is depends on the Modes cell, and reading it as a colour
        // code regardless would store a NAC of 293 as a colour code.
        //
        // A machine listing both DMR and P25 leaves the cell genuinely
        // ambiguous — one value, two possible meanings, no way to tell which —
        // so neither field is filled and the raw value goes to the notes for
        // the operator to resolve. Guessing here writes a wrong byte to a radio.
        let digital_access = get(&rec, "Digital Access");
        let ambiguous_access =
            modes.dmr && modes.p25 && digital_access.as_deref().is_some_and(|s| !s.is_empty());
        let dmr_color_code = if modes.dmr && !modes.p25 {
            digital_access
                .as_deref()
                .and_then(|s| s.parse::<i64>().ok())
                // The manual create/update path enforces this range
                // (channels.rs `checks`); an import must not be the way round
                // it. Out of range means the cell is not a colour code.
                .filter(|cc| (0..=15).contains(cc))
        } else {
            None
        };
        let p25_nac = if modes.p25 && !modes.dmr {
            digital_access.clone()
        } else {
            None
        };

        // Mode comes from the Modes tokens, never from "a colour code is
        // present": real DMR repeaters ship with the Digital Access cell empty,
        // and inferring from it would file them as analog.
        let mode = derive_mode(
            modes.dmr,
            modes.dstar,
            modes.ysf,
            modes.nxdn,
            modes.p25,
            modes.m17,
        );

        let s = finalize(
            &callsign,
            rx_freq,
            tx_freq,
            ctcss_uplink,
            ctcss_downlink,
            city.as_deref(),
            state.as_deref(),
        );

        // Landmark has no column of its own in our schema; the JSON importer
        // puts it in the notes and this matches. Link services (AllStar,
        // EchoLink, IRLP, WIRES-X) have node-number columns here, but the CSV
        // carries only the fact that they exist, so they go to the notes too
        // rather than being invented as node ids.
        let mut note_parts = Vec::new();
        if let Some(lm) = landmark {
            note_parts.push(format!("Landmark: {lm}"));
        }
        if !modes.extras.is_empty() {
            note_parts.push(format!("Links: {}", modes.extras.join(", ")));
        }
        if ambiguous_access {
            note_parts.push(format!(
                "Digital Access {} (DMR+P25 listed; colour code or NAC unresolved)",
                digital_access.clone().unwrap_or_default()
            ));
        }
        let notes = (!note_parts.is_empty()).then(|| note_parts.join(" | "));

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
            // finalize's CTCSS-only reading is replaced by the DCS-aware one.
            tone_mode,
            cross_mode,
            ctcss_uplink,
            ctcss_downlink,
            dcs_code,
            dcs_rx_code,
            covers: SourceColumns { link_nodes: false, operational_status: false, dcs: true },
            dmr_color_code,
            // No RepeaterBook export has a column for these three.
            dmr_timeslot: None,
            dmr_talkgroup: None,
            power: None,
            dstar_capable: modes.dstar,
            ysf_capable: modes.ysf,
            nxdn_capable: modes.nxdn,
            p25_capable: modes.p25,
            p25_nac,
            m17_capable: modes.m17,
            tetra_capable: modes.tetra,
            // Presence is known, node numbers are not; see the notes above.
            allstar_node: None,
            echolink_node: None,
            irlp_node: None,
            wires_node: None,
            // Columns this export simply does not have. None, not a default,
            // so a re-import cannot overwrite what the premium export knew.
            use_type: None,
            operational_status: None,
            city,
            county: get(&rec, "County"),
            state,
            country,
            latitude: None,
            longitude: None,
            notes,
        });
    }

    Ok(out)
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
            dcs_code: None,
            dcs_rx_code: None,
            covers: SourceColumns { link_nodes: true, operational_status: false, dcs: false },
            dmr_color_code,
            // No RepeaterBook export has a column for these three.
            dmr_timeslot: None,
            dmr_talkgroup: None,
            power: None,
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
pub(crate) fn parse_leading_f64(s: &str) -> Option<f64> {
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

    /// A DCS tone scheme is the operator's own — RepeaterBook carries CTCSS
    /// only — so a re-import must not re-derive it away (issue #71).
    ///
    /// Without the `keeps_dcs` guard the merge writes whatever
    /// `derive_tone_mode` makes of the CTCSS pair: "off" for a machine
    /// RepeaterBook lists with no PL (leaving `dcs_code` orphaned and the
    /// channel programming with NO squelch tone, so it will not key the
    /// repeater), or "TSQL" for one that does list a PL — CTCSS silently
    /// replacing the operator's DCS.
    #[tokio::test]
    async fn a_reimport_keeps_a_dcs_tone_scheme() {
        let dir = std::env::temp_dir().join(format!("cpm_dcs_merge_{}", std::process::id()));
        let db_path = dir.join("test.sqlite3");
        let _ = std::fs::remove_file(&db_path);
        let pool = crate::db::init_pool(&db_path).await.expect("init_pool");

        let parsed =
            parse_repeaterbook_csv("../sample-data/repeaterbook-sample.csv").expect("parse");
        insert_parsed(&pool, &parsed).await.expect("first import");

        // W0CPH is listed with PL 100.0; W0ARK with no tone at all. Put both on
        // DCS the way an operator would after looking the machines up.
        for call in ["W0CPH", "W0ARK"] {
            sqlx::query(
                "UPDATE channels SET tone_mode = 'DTCS', dcs_code = '023',
                 dcs_rx_code = '023' WHERE callsign = ?1",
            )
            .bind(call)
            .execute(&pool)
            .await
            .unwrap();
        }

        // Re-import the very same export to pick up a status change.
        let summary = insert_parsed(&pool, &parsed).await.expect("re-import");
        assert_eq!(summary.updated, parsed.len(), "every row should merge");

        for call in ["W0CPH", "W0ARK"] {
            let (tone_mode, dcs): (Option<String>, Option<String>) =
                sqlx::query_as("SELECT tone_mode, dcs_code FROM channels WHERE callsign = ?1")
                    .bind(call)
                    .fetch_one(&pool)
                    .await
                    .unwrap();
            assert_eq!(
                tone_mode.as_deref(),
                Some("DTCS"),
                "{call} lost its DCS tone scheme on re-import"
            );
            assert_eq!(dcs.as_deref(), Some("023"), "{call} lost its DCS code");
        }

        // A channel left on CTCSS still re-derives normally — the guard is not a
        // blanket freeze on the tone scheme.
        let tone_mode: Option<String> =
            sqlx::query_scalar("SELECT tone_mode FROM channels WHERE callsign = 'N0BLD'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(tone_mode.as_deref(), Some("off"));

        let _ = std::fs::remove_file(&db_path);
    }

    /// `keeps_dcs` keys on the tone scheme, not on a code being present: a
    /// leftover `dcs_code` under a CTCSS scheme is inert and must not freeze it.
    #[test]
    fn keeps_dcs_only_for_schemes_that_squelch_on_dcs() {
        assert!(keeps_dcs(Some("DTCS"), "Tone->Tone"));
        assert!(keeps_dcs(Some("dtcs"), "Tone->Tone"));
        assert!(keeps_dcs(Some("Cross"), "Tone->DTCS"));
        assert!(keeps_dcs(Some("Cross"), "DTCS->Tone"));
        assert!(keeps_dcs(Some("Cross"), "DTCS->DTCS"));
        assert!(!keeps_dcs(Some("Cross"), "Tone->Tone"));
        assert!(!keeps_dcs(Some("Tone"), "Tone->Tone"));
        assert!(!keeps_dcs(Some("TSQL"), "Tone->Tone"));
        assert!(!keeps_dcs(Some("off"), "Tone->Tone"));
        assert!(!keeps_dcs(None, "Tone->Tone"));
    }

    /// Deliberately clearing a tone RepeaterBook reports must be recorded as an
    /// override, or the next re-import puts the tone straight back (issue #86).
    ///
    /// The editor and the importer each had their own idea of "differs": the
    /// editor counted a change only when BOTH sides had a value, so clearing a
    /// tone left `ctcss_uplink_overridden = 0` and the merge adopted
    /// RepeaterBook's 88.5 Hz again — the channel transmits a tone the machine
    /// does not want, and nothing says so. Both now call
    /// `util::differs_from_rb_f64`.
    #[tokio::test]
    async fn a_standard_csv_reimport_keeps_what_only_the_premium_export_knew() {
        let dir = std::env::temp_dir().join(format!("cpm_csv_merge_{}", std::process::id()));
        std::fs::create_dir_all(&dir).ok();
        let db_path = dir.join("test.sqlite3");
        let _ = std::fs::remove_file(&db_path);
        let pool = crate::db::init_pool(&db_path).await.expect("init_pool");

        // A premium "Full Data" import establishes the service flags, the use
        // type and the site coordinates. The free CSV has a column for none of
        // those four.
        let json = r#"{"records":[{
            "freq_mhz":"145.110","input_freq":"144.510","callsign":"QQ0AAA",
            "state":"CO","city":"Anytown","pl_tone":"100.0",
            "lat":"39.00000","lon":"-105.00000"
        }]}"#;
        let jpath = dir.join("rb.json");
        std::fs::write(&jpath, json).expect("write json");
        let from_json = parse_repeaterbook_json(jpath.to_str().unwrap()).expect("parse json");
        insert_parsed(&pool, &from_json).await.expect("premium import");

        // The JSON parser does not map RepeaterBook's "use" field today, so
        // set it here directly: the COALESCE that protects it still has to
        // hold for any row that has one, wherever it came from.
        sqlx::query("UPDATE channels SET use_type = 'OPEN' WHERE callsign = 'QQ0AAA'")
            .execute(&pool)
            .await
            .expect("seed use_type");

        let before = sqlx::query_as::<_, crate::models::Channel>(
            "SELECT * FROM channels WHERE callsign = 'QQ0AAA'",
        )
        .fetch_one(&pool)
        .await
        .expect("row");
        assert_eq!(before.latitude, Some(39.0));
        assert_eq!(before.use_type.as_deref(), Some("OPEN"));

        // The same repeater, now from the free CSV. It has to land on the same
        // row: that is the whole point of normalising Location and State.
        let from_csv = parse_repeaterbook_standard_csv(
            "../sample-data/repeaterbook-standard-sample.csv",
        )
        .expect("parse csv");
        let summary = insert_parsed(&pool, &from_csv).await.expect("csv import");
        assert_eq!(summary.updated, 1, "QQ0AAA merged rather than duplicating");

        let after = sqlx::query_as::<_, crate::models::Channel>(
            "SELECT * FROM channels WHERE callsign = 'QQ0AAA'",
        )
        .fetch_one(&pool)
        .await
        .expect("row");

        // The failure this test exists for: a straight write of a column this
        // export does not have clears it, silently, on every matched channel.
        assert_eq!(after.use_type.as_deref(), Some("OPEN"), "Use cleared");
        assert_eq!(after.latitude, Some(39.0), "coordinates cleared");

        // What the CSV does carry is still applied.
        assert_eq!(after.city.as_deref(), Some("Anytown"));
        assert_eq!(after.notes.as_deref(), Some("Landmark: Sample Hill"));
    }

    /// Import the sample CSV, then re-import it with one row's tone cells
    /// rewritten. Returns the resulting channel.
    async fn reimport_qq0fff_with_tones(
        tag: &str,
        uplink: &str,
        downlink: &str,
    ) -> (sqlx::SqlitePool, crate::models::Channel) {
        let dir = std::env::temp_dir().join(format!("cpm_{tag}_{}", std::process::id()));
        std::fs::create_dir_all(&dir).ok();
        let db_path = dir.join("test.sqlite3");
        let _ = std::fs::remove_file(&db_path);
        let pool = crate::db::init_pool(&db_path).await.expect("init_pool");

        let first =
            parse_repeaterbook_standard_csv("../sample-data/repeaterbook-standard-sample.csv")
                .expect("parse");
        insert_parsed(&pool, &first).await.expect("first import");

        // Same export with QQ0FFF's tone columns changed.
        let orig = std::fs::read_to_string("../sample-data/repeaterbook-standard-sample.csv")
            .expect("read sample");
        let edited: String = orig
            .lines()
            .map(|l| {
                if l.contains("QQ0FFF") {
                    let f: Vec<&str> = l.split(',').collect();
                    format!(
                        "{},{},{},{uplink},{downlink},{}",
                        f[0],
                        f[1],
                        f[2],
                        f[5..].join(",")
                    )
                } else {
                    l.to_string()
                }
            })
            .collect::<Vec<_>>()
            .join("\n");
        let p2 = dir.join("second.csv");
        std::fs::write(&p2, edited).expect("write");
        let second = parse_repeaterbook_standard_csv(p2.to_str().unwrap()).expect("parse 2");
        insert_parsed(&pool, &second).await.expect("re-import");

        let ch = sqlx::query_as::<_, crate::models::Channel>(
            "SELECT * FROM channels WHERE callsign = 'QQ0FFF'",
        )
        .fetch_one(&pool)
        .await
        .expect("row");
        (pool, ch)
    }

    #[tokio::test]
    async fn a_repeater_that_moves_off_dcs_stops_programming_dcs() {
        // QQ0FFF imports as DTCS/073. RepeaterBook then lists it as CTCSS
        // 100.0 — a retune, or a corrected entry.
        //
        // The bug this guards: `keeps_dcs` keyed on the STORED tone_mode, which
        // was only ever DTCS by an operator's hand until the free CSV started
        // supplying DCS. It would freeze tone_mode at DTCS for ever; every
        // encoder gates on tone_mode, so the radio keeps transmitting DCS 073
        // and will not key the repeater. That is issue #71's failure, arriving
        // from the other direction.
        let (_pool, ch) = reimport_qq0fff_with_tones("dcs_off", "100.0", "100.0").await;
        assert_eq!(ch.tone_mode.as_deref(), Some("TSQL"), "still stuck on DCS");
        assert_eq!(ch.dcs_code, None, "stale DCS code still programmed");
        assert_eq!(ch.ctcss_uplink, Some(100.0));
    }

    #[tokio::test]
    async fn a_cross_dcs_scheme_can_lose_its_rx_half() {
        // QQ0GGG is DTCS->DTCS 023/114. RepeaterBook later lists D023 both
        // ways, so the fresh record has no RX code at all. Under COALESCE the
        // old 114 could never clear and the channel would squelch for ever on
        // a code the repeater no longer sends.
        let dir = std::env::temp_dir().join(format!("cpm_dcs_rx_{}", std::process::id()));
        std::fs::create_dir_all(&dir).ok();
        let db_path = dir.join("test.sqlite3");
        let _ = std::fs::remove_file(&db_path);
        let pool = crate::db::init_pool(&db_path).await.expect("init_pool");

        let first =
            parse_repeaterbook_standard_csv("../sample-data/repeaterbook-standard-sample.csv")
                .expect("parse");
        insert_parsed(&pool, &first).await.expect("first");
        let before: Option<String> =
            sqlx::query_scalar("SELECT dcs_rx_code FROM channels WHERE callsign = 'QQ0GGG'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(before.as_deref(), Some("114"), "fixture no longer cross-DCS");

        let orig = std::fs::read_to_string("../sample-data/repeaterbook-standard-sample.csv")
            .unwrap();
        let edited = orig.replace(",D023,D114,QQ0GGG,", ",D023,D023,QQ0GGG,");
        assert_ne!(edited, orig, "fixture row not found");
        let p2 = dir.join("second.csv");
        std::fs::write(&p2, edited).unwrap();
        insert_parsed(
            &pool,
            &parse_repeaterbook_standard_csv(p2.to_str().unwrap()).unwrap(),
        )
        .await
        .expect("re-import");

        let ch = sqlx::query_as::<_, crate::models::Channel>(
            "SELECT * FROM channels WHERE callsign = 'QQ0GGG'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(ch.tone_mode.as_deref(), Some("DTCS"), "same code both ways now");
        assert_eq!(ch.dcs_code.as_deref(), Some("023"));
        assert_eq!(ch.dcs_rx_code, None, "stale RX code never cleared");
    }

    #[tokio::test]
    async fn an_operators_own_dcs_code_survives_a_reimport() {
        // The other half of the policy. The operator looks the machine up,
        // decides RepeaterBook's D073 is wrong and types D047. A re-import must
        // not quietly put 073 back — merge_existing's documented policy is that
        // DCS is the operator's to curate.
        let dir = std::env::temp_dir().join(format!("cpm_dcs_curated_{}", std::process::id()));
        std::fs::create_dir_all(&dir).ok();
        let db_path = dir.join("test.sqlite3");
        let _ = std::fs::remove_file(&db_path);
        let pool = crate::db::init_pool(&db_path).await.expect("init_pool");

        let parsed =
            parse_repeaterbook_standard_csv("../sample-data/repeaterbook-standard-sample.csv")
                .expect("parse");
        insert_parsed(&pool, &parsed).await.expect("first");

        sqlx::query("UPDATE channels SET dcs_code = '047' WHERE callsign = 'QQ0FFF'")
            .execute(&pool)
            .await
            .expect("operator edit");

        insert_parsed(&pool, &parsed).await.expect("re-import");

        let code: Option<String> =
            sqlx::query_scalar("SELECT dcs_code FROM channels WHERE callsign = 'QQ0FFF'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(code.as_deref(), Some("047"), "operator's DCS overwritten");
    }

    #[tokio::test]
    async fn a_free_csv_reimport_keeps_link_nodes_and_status_it_cannot_see() {
        let dir = std::env::temp_dir().join(format!("cpm_csv_cover_{}", std::process::id()));
        std::fs::create_dir_all(&dir).ok();
        let db_path = dir.join("test.sqlite3");
        let _ = std::fs::remove_file(&db_path);
        let pool = crate::db::init_pool(&db_path).await.expect("init_pool");

        // The premium JSON is the only export that carries link node numbers.
        let json = r#"{"records":[{
            "freq_mhz":"145.110","input_freq":"144.510","callsign":"QQ0AAA",
            "state":"CO","city":"Anytown","pl_tone":"100.0",
            "allstar_node":"48291","echolink_node":"12345",
            "irlp_node_id":"7700","wires_node":"21001"
        }]}"#;
        let jpath = dir.join("rb.json");
        std::fs::write(&jpath, json).expect("write json");
        let from_json = parse_repeaterbook_json(jpath.to_str().unwrap()).expect("parse json");
        insert_parsed(&pool, &from_json).await.expect("premium import");

        // Operational Status comes only from the wide CSV, so seed it directly.
        sqlx::query(
            "UPDATE channels SET operational_status = 'On Air', \
             rb_operational_status = 'On Air' WHERE callsign = 'QQ0AAA'",
        )
        .execute(&pool)
        .await
        .expect("seed status");

        // Now the same repeater from the free CSV, which has neither column.
        let from_csv = parse_repeaterbook_standard_csv(
            "../sample-data/repeaterbook-standard-sample.csv",
        )
        .expect("parse csv");
        let summary = insert_parsed(&pool, &from_csv).await.expect("csv import");
        assert_eq!(summary.updated, 1, "QQ0AAA merged rather than duplicating");

        let after = sqlx::query_as::<_, crate::models::Channel>(
            "SELECT * FROM channels WHERE callsign = 'QQ0AAA'",
        )
        .fetch_one(&pool)
        .await
        .expect("row");

        // The failure this exists for: the CSV reports None for all five
        // because it has no such column, and a straight write turns that into
        // a silent delete. None of these are editable in the UI, so the only
        // way back would be another premium import.
        assert_eq!(after.allstar_node.as_deref(), Some("48291"), "AllStar node wiped");
        assert_eq!(after.echolink_node.as_deref(), Some("12345"), "EchoLink node wiped");
        assert_eq!(after.irlp_node.as_deref(), Some("7700"), "IRLP node wiped");
        assert_eq!(after.wires_node.as_deref(), Some("21001"), "Wires-X node wiped");
        assert_eq!(after.operational_status.as_deref(), Some("On Air"), "Status wiped");
        // The snapshot has to survive too, or the next override comparison has
        // no baseline to work from.
        let snap: Option<String> = sqlx::query_scalar(
            "SELECT rb_operational_status FROM channels WHERE callsign = 'QQ0AAA'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(snap.as_deref(), Some("On Air"), "Status rb_ snapshot wiped");
    }

    #[tokio::test]
    async fn the_export_that_does_carry_nodes_can_still_clear_one() {
        // The other half of the rule: a flat COALESCE would have made these
        // columns write-once, so an export that HAS the column must still be
        // able to report that RepeaterBook dropped the node.
        let dir = std::env::temp_dir().join(format!("cpm_csv_clear_{}", std::process::id()));
        std::fs::create_dir_all(&dir).ok();
        let db_path = dir.join("test.sqlite3");
        let _ = std::fs::remove_file(&db_path);
        let pool = crate::db::init_pool(&db_path).await.expect("init_pool");

        let with_node = r#"{"records":[{
            "freq_mhz":"145.110","input_freq":"144.510","callsign":"QQ0AAA",
            "state":"CO","city":"Anytown","echolink_node":"12345"
        }]}"#;
        let p1 = dir.join("a.json");
        std::fs::write(&p1, with_node).unwrap();
        insert_parsed(&pool, &parse_repeaterbook_json(p1.to_str().unwrap()).unwrap())
            .await
            .expect("first");

        let node_gone = r#"{"records":[{
            "freq_mhz":"145.110","input_freq":"144.510","callsign":"QQ0AAA",
            "state":"CO","city":"Anytown","echolink_node":""
        }]}"#;
        let p2 = dir.join("b.json");
        std::fs::write(&p2, node_gone).unwrap();
        insert_parsed(&pool, &parse_repeaterbook_json(p2.to_str().unwrap()).unwrap())
            .await
            .expect("second");

        let node: Option<String> =
            sqlx::query_scalar("SELECT echolink_node FROM channels WHERE callsign = 'QQ0AAA'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(node, None, "a delisted node must actually clear");
    }

    #[tokio::test]
    async fn a_standard_csv_import_stores_dcs_the_json_could_never_describe() {
        let dir = std::env::temp_dir().join(format!("cpm_csv_dcs_{}", std::process::id()));
        std::fs::create_dir_all(&dir).ok();
        let db_path = dir.join("test.sqlite3");
        let _ = std::fs::remove_file(&db_path);
        let pool = crate::db::init_pool(&db_path).await.expect("init_pool");

        let parsed = parse_repeaterbook_standard_csv(
            "../sample-data/repeaterbook-standard-sample.csv",
        )
        .expect("parse csv");
        insert_parsed(&pool, &parsed).await.expect("import");

        // QQ0FFF is D073 both ways. Before this parser existed the tone was
        // dropped and the channel programmed with no squelch at all, which is
        // the failure issue #71 describes.
        let ch = sqlx::query_as::<_, crate::models::Channel>(
            "SELECT * FROM channels WHERE callsign = 'QQ0FFF'",
        )
        .fetch_one(&pool)
        .await
        .expect("row");
        assert_eq!(ch.tone_mode.as_deref(), Some("DTCS"));
        assert_eq!(ch.dcs_code.as_deref(), Some("073"));

        // And it survives a re-import of the same export.
        insert_parsed(&pool, &parsed).await.expect("re-import");
        let again: Option<String> =
            sqlx::query_scalar("SELECT dcs_code FROM channels WHERE callsign = 'QQ0FFF'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(again.as_deref(), Some("073"));
    }

    #[tokio::test]
    async fn clearing_a_repeaterbook_tone_survives_a_reimport() {
        let dir = std::env::temp_dir().join(format!("cpm_clear_tone_{}", std::process::id()));
        let db_path = dir.join("test.sqlite3");
        let _ = std::fs::remove_file(&db_path);
        let pool = crate::db::init_pool(&db_path).await.expect("init_pool");

        // RepeaterBook reports PL 88.5 on a machine that is really carrier-access.
        let json = r#"{"records":[{
            "freq_mhz":"146.940","input_freq":"146.340","callsign":"W0CPH",
            "state":"CO","city":"Colorado Springs","pl_tone":"88.5"
        }]}"#;
        let jpath = dir.join("rb.json");
        std::fs::create_dir_all(&dir).ok();
        std::fs::write(&jpath, json).expect("write json");
        let parsed = parse_repeaterbook_json(jpath.to_str().unwrap()).expect("parse");
        insert_parsed(&pool, &parsed).await.expect("first import");

        let ch = sqlx::query_as::<_, crate::models::Channel>(
            "SELECT * FROM channels WHERE callsign = 'W0CPH'",
        )
        .fetch_one(&pool)
        .await
        .expect("row");
        assert_eq!(ch.ctcss_uplink, Some(88.5));

        // The operator clears the tone field in the editor and saves.
        let mut input = channel_input_from(&ch);
        input.ctcss_uplink = None;
        input.tone_mode = Some("off".to_string());
        let saved = crate::commands::channels::update_impl(&pool, ch.id, input)
            .await
            .expect("save");
        assert_eq!(saved.ctcss_uplink, None);
        assert!(
            saved.ctcss_uplink_overridden,
            "clearing a tone RepeaterBook reports is an override"
        );

        // A fresh export still lists 88.5 — the clear has to hold.
        insert_parsed(&pool, &parsed).await.expect("re-import");
        let after: Option<f64> =
            sqlx::query_scalar("SELECT ctcss_uplink FROM channels WHERE callsign = 'W0CPH'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(after, None, "the re-import brought the cleared tone back");

        let _ = std::fs::remove_file(&db_path);
    }

    /// A manually created channel has no RepeaterBook baseline to differ from,
    /// so editing it must never raise an "overridden" flag.
    #[tokio::test]
    async fn a_manual_channel_never_reports_an_override() {
        let dir = std::env::temp_dir().join(format!("cpm_manual_over_{}", std::process::id()));
        let db_path = dir.join("test.sqlite3");
        let _ = std::fs::remove_file(&db_path);
        let pool = crate::db::init_pool(&db_path).await.expect("init_pool");

        sqlx::query(
            "INSERT INTO channels (id, name_long, rx_freq, mode, source) \
             VALUES (1, 'Simplex', 146.52, 'FM', 'manual')",
        )
        .execute(&pool)
        .await
        .unwrap();
        let ch = sqlx::query_as::<_, crate::models::Channel>("SELECT * FROM channels WHERE id = 1")
            .fetch_one(&pool)
            .await
            .unwrap();

        let mut input = channel_input_from(&ch);
        input.ctcss_uplink = Some(100.0);
        input.notes = Some("club net".to_string());
        let saved = crate::commands::channels::update_impl(&pool, 1, input)
            .await
            .expect("save");
        assert!(!saved.ctcss_uplink_overridden);
        assert!(!saved.notes_overridden);
        assert!(!saved.has_overrides);

        let _ = std::fs::remove_file(&db_path);
    }

    /// What the editor form does: load the row, hand its user-editable fields
    /// back as the input, with whatever the operator changed.
    fn channel_input_from(c: &crate::models::Channel) -> crate::models::ChannelInput {
        crate::models::ChannelInput {
            name_long: c.name_long.clone(),
            name_short: c.name_short.clone(),
            callsign: c.callsign.clone(),
            rx_freq: c.rx_freq,
            tx_freq: c.tx_freq,
            offset: c.offset,
            duplex: c.duplex.clone(),
            mode: c.mode.clone(),
            tone_mode: c.tone_mode.clone(),
            ctcss_uplink: c.ctcss_uplink,
            ctcss_downlink: c.ctcss_downlink,
            dcs_code: c.dcs_code.clone(),
            dcs_rx_code: c.dcs_rx_code.clone(),
            dcs_polarity: c.dcs_polarity.clone(),
            cross_mode: c.cross_mode.clone(),
            power: c.power.clone(),
            dmr_color_code: c.dmr_color_code,
            dmr_timeslot: c.dmr_timeslot,
            dmr_talkgroup: c.dmr_talkgroup,
            dstar_capable: c.dstar_capable,
            dstar_ur_call: c.dstar_ur_call.clone(),
            dstar_rpt1: c.dstar_rpt1.clone(),
            dstar_rpt2: c.dstar_rpt2.clone(),
            ysf_capable: c.ysf_capable,
            nxdn_capable: c.nxdn_capable,
            p25_capable: c.p25_capable,
            p25_nac: c.p25_nac.clone(),
            m17_capable: c.m17_capable,
            m17_can: c.m17_can,
            use_type: c.use_type.clone(),
            operational_status: c.operational_status.clone(),
            service_type: c.service_type.clone(),
            city: c.city.clone(),
            county: c.county.clone(),
            state: c.state.clone(),
            country: c.country.clone(),
            latitude: c.latitude,
            longitude: c.longitude,
            notes: c.notes.clone(),
            source: Some(c.source.clone()),
        }
    }

    #[test]
    fn parses_sample_standard_csv() {
        let parsed = parse_repeaterbook_csv("../sample-data/repeaterbook-standard-sample.csv")
            .expect("parse failed");
        assert_eq!(parsed.len(), 20, "every row parses; none is silently skipped");

        let by_call = |c: &str| {
            parsed
                .iter()
                .find(|p| p.callsign == c)
                .unwrap_or_else(|| panic!("{c} missing from the parse"))
        };

        // "Anytown - Sample Hill" is one column here and two fields in the
        // premium JSON. The split is what makes the two imports agree on the
        // dedupe id instead of duplicating the library.
        let a = by_call("QQ0AAA");
        assert_eq!(a.city.as_deref(), Some("Anytown"));
        assert_eq!(a.notes.as_deref(), Some("Landmark: Sample Hill"));
        // State is spelled out in this export and must become the postal code
        // the JSON importer stores, or every id differs.
        assert_eq!(a.state.as_deref(), Some("CO"));
        assert_eq!(a.country.as_deref(), Some("United States"));
        assert_eq!(a.repeaterbook_id, "QQ0AAA|145.1100|CO|ANYTOWN");

        // A Location with no landmark keeps the whole cell as the city.
        assert_eq!(by_call("QQ0BBB").city.as_deref(), Some("Testville"));
        assert_eq!(by_call("QQ0BBB").notes, None);
    }

    #[test]
    fn standard_csv_reads_every_tone_shape() {
        let parsed = parse_repeaterbook_csv("../sample-data/repeaterbook-standard-sample.csv")
            .expect("parse failed");
        let t = |c: &str| {
            let p = parsed.iter().find(|p| p.callsign == c).unwrap();
            (
                p.tone_mode.as_str(),
                p.cross_mode.as_str(),
                p.dcs_code.as_deref(),
                p.dcs_rx_code.as_deref(),
            )
        };

        // CTCSS, identical to what derive_tone_mode produces.
        assert_eq!(t("QQ0AAA"), ("Tone", "Tone->Tone", None, None));
        assert_eq!(t("QQ0BBB"), ("TSQL", "Tone->Tone", None, None));
        assert_eq!(t("QQ0CCC"), ("Cross", "Tone->Tone", None, None));
        assert_eq!(t("QQ0DDD"), ("Cross", "->Tone", None, None));

        // CSQ is carrier squelch: an explicit "no tone", not a missing value.
        assert_eq!(t("QQ0EEE"), ("off", "Tone->Tone", None, None));
        let e = parsed.iter().find(|p| p.callsign == "QQ0EEE").unwrap();
        assert_eq!(e.ctcss_uplink, None);
        assert_eq!(e.ctcss_downlink, None);

        // DCS. RepeaterBook does supply it, and dropping it leaves a channel
        // that cannot key its repeater (the failure issue #71 is about).
        assert_eq!(t("QQ0FFF"), ("DTCS", "Tone->Tone", Some("073"), None));
        assert_eq!(t("QQ0GGG"), ("Cross", "DTCS->DTCS", Some("023"), Some("114")));
        assert_eq!(t("QQ0HHH"), ("Cross", "DTCS->", Some("205"), None));
        assert_eq!(t("QQ0III"), ("Cross", "DTCS->Tone", Some("116"), None));
        assert_eq!(t("QQ0JJJ"), ("Cross", "Tone->DTCS", None, Some("054")));

        // A DCS cell never leaks into the CTCSS columns.
        let f = parsed.iter().find(|p| p.callsign == "QQ0FFF").unwrap();
        assert_eq!(f.ctcss_uplink, None);
        assert_eq!(f.ctcss_downlink, None);
        // ...and a mixed pair keeps only the CTCSS half.
        let i = parsed.iter().find(|p| p.callsign == "QQ0III").unwrap();
        assert_eq!(i.ctcss_uplink, None);
        assert_eq!(i.ctcss_downlink, Some(131.8));
    }

    #[test]
    fn standard_csv_reads_mode_from_the_modes_column() {
        let parsed = parse_repeaterbook_standard_csv(
            "../sample-data/repeaterbook-standard-sample.csv",
        )
        .expect("parse failed");
        let p = |c: &str| parsed.iter().find(|p| p.callsign == c).unwrap();

        assert_eq!(p("QQ0KKK").mode, "DMR");
        assert_eq!(p("QQ0KKK").dmr_color_code, Some(7));

        // The trap: a real DMR repeater with an empty Digital Access cell.
        // Inferring the mode from "a colour code is present" files it as FM.
        assert_eq!(p("QQ0LLL").mode, "DMR");
        assert_eq!(p("QQ0LLL").dmr_color_code, None);

        // Digital Access means NAC, not colour code, when the mode is P25.
        assert_eq!(p("QQ0MMM").mode, "P25");
        assert_eq!(p("QQ0MMM").p25_nac.as_deref(), Some("293"));
        assert_eq!(
            p("QQ0MMM").dmr_color_code,
            None,
            "a NAC of 293 is not a colour code (the range is 0-15)"
        );

        assert_eq!(p("QQ0NNN").mode, "DSTAR");
        assert!(p("QQ0NNN").dstar_capable);

        // Link services have no node number in this export, so presence is
        // recorded in the notes rather than invented as a node id.
        let o = p("QQ0OOO");
        assert!(o.ysf_capable);
        assert_eq!(o.mode, "YSF");
        assert_eq!(o.wires_node, None);
        assert_eq!(
            o.notes.as_deref(),
            Some("Landmark: Round Butte | Links: WIRES-X")
        );
        assert_eq!(
            p("QQ0PPP").notes.as_deref(),
            Some("Landmark: Long Mesa | Links: AllStar, EchoLink, IRLP")
        );
        // An unrecognised token is surfaced, not silently dropped.
        assert_eq!(p("QQ0QQQ").notes.as_deref(), Some("Links: ATV"));
    }

    #[test]
    fn standard_csv_leaves_absent_columns_absent() {
        let parsed = parse_repeaterbook_standard_csv(
            "../sample-data/repeaterbook-standard-sample.csv",
        )
        .expect("parse failed");
        // This export has no Use, Status or Lat/Long columns at all. None, not
        // a default: merge_existing COALESCEs on them, so reporting a value
        // here would clear whatever a premium import had established.
        for p in &parsed {
            assert_eq!(p.use_type, None, "{} reported a Use it cannot know", p.callsign);
            assert_eq!(p.operational_status, None);
            assert_eq!(p.latitude, None);
            assert_eq!(p.longitude, None);
        }
    }

    #[test]
    fn standard_csv_derives_duplex_from_the_frequencies_not_the_offset_column() {
        let parsed = parse_repeaterbook_standard_csv(
            "../sample-data/repeaterbook-standard-sample.csv",
        )
        .expect("parse failed");
        let p = |c: &str| parsed.iter().find(|p| p.callsign == c).unwrap();

        assert_eq!(p("QQ0AAA").duplex, "-");
        assert_eq!(p("QQ0FFF").duplex, "+");

        // Offset "s" marks a split, not simplex. This 900 MHz pair is 25.5 MHz
        // apart; reading the column literally would make it a simplex channel
        // that transmits on its own output frequency.
        let s = p("QQ0SSS");
        assert_ne!(s.duplex, "simplex");
        assert_eq!(s.tx_freq, Some(902.2));
        assert_eq!(s.band, "900");
    }

    #[test]
    fn an_unknown_region_still_imports() {
        let parsed = parse_repeaterbook_standard_csv(
            "../sample-data/repeaterbook-standard-sample.csv",
        )
        .expect("parse failed");
        // "Atlantis" is in no table. The row must still import, keeping the
        // name it was given rather than being dropped or blanked.
        let r = parsed.iter().find(|p| p.callsign == "QQ0RRR").unwrap();
        assert_eq!(r.state.as_deref(), Some("Atlantis"));
        assert_eq!(r.country, None);
        assert_eq!(r.repeaterbook_id, "QQ0RRR|442.2400|ATLANTIS|FARAWAY");
    }

    #[test]
    fn the_two_csv_shapes_route_to_their_own_parsers() {
        // Both files are `.csv` and share not one column name for the
        // frequency. Handing either to the wrong parser yields zero channels
        // and no error, which is how a real free-tier export imported nothing.
        let standard = parse_repeaterbook_csv("../sample-data/repeaterbook-standard-sample.csv")
            .expect("standard parse");
        let wide = parse_repeaterbook_csv("../sample-data/repeaterbook-sample.csv")
            .expect("wide parse");
        assert_eq!(standard.len(), 20);
        assert_eq!(wide.len(), 10);

        // The wide shape's own parser cannot read the standard export: this is
        // the exact silent failure the header sniff exists to prevent.
        let wrong = parse_repeaterbook_full_csv("../sample-data/repeaterbook-standard-sample.csv")
            .expect("parses, but finds nothing");
        assert_eq!(wrong.len(), 0);
    }

    #[test]
    fn a_dcs_cell_that_is_not_octal_is_dropped_rather_than_guessed() {
        // DCS codes are octal, and that is how the channels table stores them.
        // An 8 or a 9 means this is not the notation we think it is.
        assert_eq!(parse_rb_tone(Some("D073".into())), RbTone::Dcs("073".into()));
        assert_eq!(parse_rb_tone(Some("D23".into())), RbTone::Dcs("023".into()));
        assert_eq!(parse_rb_tone(Some("D089".into())), RbTone::None);
        assert_eq!(parse_rb_tone(Some("D".into())), RbTone::None);
        assert_eq!(parse_rb_tone(Some("CSQ".into())), RbTone::None);
        assert_eq!(parse_rb_tone(Some("csq".into())), RbTone::None);
        assert_eq!(parse_rb_tone(Some("".into())), RbTone::None);
        assert_eq!(parse_rb_tone(None), RbTone::None);
        assert_eq!(parse_rb_tone(Some("100.0".into())), RbTone::Ctcss(100.0));
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
