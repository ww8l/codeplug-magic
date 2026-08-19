use serde::{Deserialize, Serialize};
use sqlx::{Acquire, SqlitePool};
use tauri::State;

use crate::db::AppState;
use crate::error::MapErrString;
use crate::models::{Channel, ImportSummary};

/// Magic value stamped into the export so re-import can recognise our own files
/// and tell them apart from a RepeaterBook export.
const BACKUP_FORMAT: &str = "codeplug-magic-channels";
/// Legacy format id from before the product was renamed; still accepted on import.
const LEGACY_BACKUP_FORMAT: &str = "73plug-channels";
/// Current backup schema version (bump if the on-disk shape changes).
const BACKUP_VERSION: u32 = 1;
/// Cap on preview rows sent to the UI (matches the RepeaterBook importer).
const PREVIEW_CAP: usize = 1000;

/// A talkgroup assignment carried alongside a DMR channel so the round-trip is
/// lossless. On import the talkgroup is resolved (or created) by
/// (network, tg_number) and the assignment is re-created.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackupTalkgroup {
    pub tg_number: i64,
    pub name: String,
    pub network: String,
    pub call_type: String,
    pub timeslot: i64,
    pub name_override: Option<String>,
}

/// One channel in a backup: the full channel row plus its talkgroup
/// assignments. The channel fields are flattened so the file reads as a plain
/// channel object with an extra `talkgroups` array.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackupChannel {
    #[serde(flatten)]
    pub channel: Channel,
    #[serde(default)]
    pub talkgroups: Vec<BackupTalkgroup>,
}

/// The whole backup file. `format`/`version` let the importer validate it is one
/// of ours before trying to read it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChannelBackup {
    pub format: String,
    pub version: u32,
    pub exported_at: String,
    pub count: usize,
    pub channels: Vec<BackupChannel>,
}

/// A preview row for the import dialog. Mirrors the RepeaterBook importer's
/// `ParsedChannel` (and the TS `ImportPreviewRow`) so the same preview table can
/// render either source. Full fidelity (DCS/power/talkgroups) is preserved in
/// the file itself and applied on import, regardless of what the preview shows.
#[derive(Debug, Clone, Serialize)]
pub struct BackupPreviewRow {
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

#[derive(Debug, Clone, Serialize)]
pub struct ChannelBackupPreview {
    pub total: usize,
    pub rows: Vec<BackupPreviewRow>,
}

// ============================================================
// Tauri commands
// ============================================================

/// Write the given channels (in the supplied order) to a Codeplug Magic backup JSON
/// file at `path`. Returns how many channels were written. Channel ids that no
/// longer exist are silently skipped.
#[tauri::command]
pub async fn export_channels(
    state: State<'_, AppState>,
    ids: Vec<i64>,
    path: String,
) -> Result<usize, String> {
    if ids.is_empty() {
        return Err("No channels selected to export.".to_string());
    }

    let mut channels = Vec::with_capacity(ids.len());
    for id in &ids {
        let Some(channel) = sqlx::query_as::<_, Channel>("SELECT * FROM channels WHERE id = ?1")
            .bind(id)
            .fetch_optional(&state.pool)
            .await
            .estr()?
        else {
            continue;
        };

        let talkgroups = sqlx::query_as::<_, (i64, String, String, String, i64, Option<String>)>(
            r#"
            SELECT tg.tg_number, tg.name, tg.network, tg.call_type,
                   rtg.timeslot, rtg.name_override
            FROM repeater_talkgroups rtg
            JOIN talkgroups tg ON tg.id = rtg.talkgroup_id
            WHERE rtg.channel_id = ?1
            ORDER BY rtg.position
            "#,
        )
        .bind(id)
        .fetch_all(&state.pool)
        .await
        .estr()?
        .into_iter()
        .map(
            |(tg_number, name, network, call_type, timeslot, name_override)| BackupTalkgroup {
                tg_number,
                name,
                network,
                call_type,
                timeslot,
                name_override,
            },
        )
        .collect();

        channels.push(BackupChannel {
            channel,
            talkgroups,
        });
    }

    let backup = ChannelBackup {
        format: BACKUP_FORMAT.to_string(),
        version: BACKUP_VERSION,
        exported_at: now_iso8601(),
        count: channels.len(),
        channels,
    };

    let json = serde_json::to_string_pretty(&backup)
        .map_err(|e| format!("Could not serialize channels: {e}"))?;
    std::fs::write(&path, json).map_err(|e| format!("Could not write file: {e}"))?;

    Ok(backup.count)
}

/// Cheap probe used by the import dialog to route a `.json` file to the right
/// importer: `true` if it parses as a Codeplug Magic channel backup, `false` otherwise
/// (e.g. a RepeaterBook JSON export). Never errors on a parse mismatch.
///
/// Accepts the legacy `73plug-channels` id as well, matching `read_backup` — a
/// probe that recognised fewer formats than the importer routed the user's own
/// older backups to the RepeaterBook parser, which read zero channels out of
/// them and reported success.
#[tauri::command]
pub async fn is_channel_backup(path: String) -> Result<bool, String> {
    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(_) => return Ok(false),
    };
    let v: serde_json::Value = match serde_json::from_str(&text) {
        Ok(v) => v,
        Err(_) => return Ok(false),
    };
    let format = v.get("format").and_then(|f| f.as_str());
    Ok(format == Some(BACKUP_FORMAT) || format == Some(LEGACY_BACKUP_FORMAT))
}

#[tauri::command]
pub async fn preview_channel_import(path: String) -> Result<ChannelBackupPreview, String> {
    let backup = read_backup(&path)?;
    let rows = backup
        .channels
        .iter()
        .take(PREVIEW_CAP)
        .map(|bc| preview_row(&bc.channel))
        .collect();
    Ok(ChannelBackupPreview {
        total: backup.channels.len(),
        rows,
    })
}

#[tauri::command]
pub async fn import_channels(
    state: State<'_, AppState>,
    path: String,
) -> Result<ImportSummary, String> {
    let backup = read_backup(&path)?;
    insert_backup(&state.pool, &backup).await
}

// ============================================================
// Helpers
// ============================================================

/// Read and validate a backup file, rejecting anything that is not one of ours.
fn read_backup(path: &str) -> Result<ChannelBackup, String> {
    let text =
        std::fs::read_to_string(path).map_err(|e| format!("Could not open file: {e}"))?;
    let backup: ChannelBackup = serde_json::from_str(&text)
        .map_err(|e| format!("This file is not a Codeplug Magic channel backup: {e}"))?;
    if backup.format != BACKUP_FORMAT && backup.format != LEGACY_BACKUP_FORMAT {
        return Err("This file is not a Codeplug Magic channel backup.".to_string());
    }
    Ok(backup)
}

/// Insert every channel in the backup, skipping ones that already exist, and
/// re-creating talkgroup assignments (resolving/creating talkgroups by
/// network + number). Runs in a single transaction.
async fn insert_backup(
    pool: &SqlitePool,
    backup: &ChannelBackup,
) -> Result<ImportSummary, String> {
    let mut conn = pool.acquire().await.estr()?;
    let mut tx = conn.begin().await.estr()?;

    let mut added = 0usize;
    let mut skipped = 0usize;

    for bc in &backup.channels {
        let c = &bc.channel;

        // Dedupe: RepeaterBook-sourced channels by their synthetic id; manual
        // channels by (rx_freq, name, tx_freq) so re-importing a backup into the
        // same library doesn't create duplicates.
        let duplicate = match c.repeaterbook_id.as_deref().filter(|s| !s.is_empty()) {
            Some(rbid) => {
                sqlx::query_as::<_, (i64,)>("SELECT id FROM channels WHERE repeaterbook_id = ?1")
                    .bind(rbid)
                    .fetch_optional(&mut *tx)
                    .await
                    .estr()?
                    .is_some()
            }
            None => sqlx::query_as::<_, (i64,)>(
                "SELECT id FROM channels
                 WHERE rx_freq = ?1
                   AND IFNULL(name_long, '') = IFNULL(?2, '')
                   AND IFNULL(tx_freq, -1) = IFNULL(?3, -1)",
            )
            .bind(c.rx_freq)
            .bind(&c.name_long)
            .bind(c.tx_freq)
            .fetch_optional(&mut *tx)
            .await
            .estr()?
            .is_some(),
        };
        if duplicate {
            skipped += 1;
            continue;
        }

        let new_id = sqlx::query(
            r#"
            INSERT INTO channels (
                rb_name, name_long, name_short, callsign, rx_freq, tx_freq,
                offset, duplex, band, mode, tone_mode, ctcss_uplink,
                ctcss_downlink, dcs_code, dcs_rx_code, dcs_polarity, cross_mode,
                power, dmr_color_code, dmr_timeslot, dmr_talkgroup, dstar_capable,
                ysf_capable, nxdn_capable, p25_capable, p25_nac, m17_capable,
                m17_can, tetra_capable, allstar_node, echolink_node, irlp_node,
                wires_node, ares, races, skywarn, canwarn, use_type,
                operational_status, service_type, city, county, state, country,
                latitude, longitude, notes, source, repeaterbook_id,
                rb_ctcss_uplink, rb_ctcss_downlink, rb_operational_status,
                rb_notes, ctcss_uplink_overridden, ctcss_downlink_overridden,
                operational_status_overridden, notes_overridden, has_overrides,
                last_rb_update, last_user_edit,
                dstar_ur_call, dstar_rpt1, dstar_rpt2
            ) VALUES (
                ?1, ?2, ?3, ?4, ?5, ?6,
                ?7, ?8, ?9, ?10, ?11, ?12,
                ?13, ?14, ?15, ?16, ?17,
                ?18, ?19, ?20, ?21, ?22,
                ?23, ?24, ?25, ?26, ?27,
                ?28, ?29, ?30, ?31, ?32,
                ?33, ?34, ?35, ?36, ?37, ?38,
                ?39, ?40, ?41, ?42, ?43, ?44,
                ?45, ?46, ?47, ?48, ?49,
                ?50, ?51, ?52,
                ?53, ?54, ?55,
                ?56, ?57, ?58,
                ?59, ?60,
                ?61, ?62, ?63
            )
            "#,
        )
        .bind(&c.rb_name)
        .bind(&c.name_long)
        .bind(&c.name_short)
        .bind(&c.callsign)
        .bind(c.rx_freq)
        .bind(c.tx_freq)
        .bind(c.offset)
        .bind(&c.duplex)
        .bind(&c.band)
        .bind(&c.mode)
        .bind(&c.tone_mode)
        .bind(c.ctcss_uplink)
        .bind(c.ctcss_downlink)
        .bind(&c.dcs_code)
        .bind(&c.dcs_rx_code)
        .bind(&c.dcs_polarity)
        .bind(&c.cross_mode)
        .bind(&c.power)
        .bind(c.dmr_color_code)
        .bind(c.dmr_timeslot)
        .bind(c.dmr_talkgroup)
        .bind(c.dstar_capable)
        .bind(c.ysf_capable)
        .bind(c.nxdn_capable)
        .bind(c.p25_capable)
        .bind(&c.p25_nac)
        .bind(c.m17_capable)
        .bind(c.m17_can)
        .bind(c.tetra_capable)
        .bind(&c.allstar_node)
        .bind(&c.echolink_node)
        .bind(&c.irlp_node)
        .bind(&c.wires_node)
        .bind(c.ares)
        .bind(c.races)
        .bind(c.skywarn)
        .bind(c.canwarn)
        .bind(&c.use_type)
        .bind(&c.operational_status)
        .bind(&c.service_type)
        .bind(&c.city)
        .bind(&c.county)
        .bind(&c.state)
        .bind(&c.country)
        .bind(c.latitude)
        .bind(c.longitude)
        .bind(&c.notes)
        .bind(&c.source)
        .bind(&c.repeaterbook_id)
        .bind(c.rb_ctcss_uplink)
        .bind(c.rb_ctcss_downlink)
        .bind(&c.rb_operational_status)
        .bind(&c.rb_notes)
        .bind(c.ctcss_uplink_overridden)
        .bind(c.ctcss_downlink_overridden)
        .bind(c.operational_status_overridden)
        .bind(c.notes_overridden)
        .bind(c.has_overrides)
        .bind(&c.last_rb_update)
        .bind(&c.last_user_edit)
        .bind(&c.dstar_ur_call)
        .bind(&c.dstar_rpt1)
        .bind(&c.dstar_rpt2)
        .execute(&mut *tx)
        .await
        .estr()?
        .last_insert_rowid();

        // Re-create talkgroup assignments, resolving (or creating) each
        // talkgroup by (network, tg_number).
        for (pos, tg) in bc.talkgroups.iter().enumerate() {
            sqlx::query(
                "INSERT INTO talkgroups (tg_number, name, network, call_type, source)
                 VALUES (?1, ?2, ?3, ?4, 'import')
                 ON CONFLICT(network, tg_number) DO NOTHING",
            )
            .bind(tg.tg_number)
            .bind(&tg.name)
            .bind(&tg.network)
            .bind(&tg.call_type)
            .execute(&mut *tx)
            .await
            .estr()?;

            let tg_id: (i64,) = sqlx::query_as(
                "SELECT id FROM talkgroups WHERE network = ?1 AND tg_number = ?2",
            )
            .bind(&tg.network)
            .bind(tg.tg_number)
            .fetch_one(&mut *tx)
            .await
            .estr()?;

            sqlx::query(
                "INSERT INTO repeater_talkgroups
                    (channel_id, talkgroup_id, timeslot, position, name_override)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
            )
            .bind(new_id)
            .bind(tg_id.0)
            .bind(tg.timeslot)
            .bind(pos as i64)
            .bind(&tg.name_override)
            .execute(&mut *tx)
            .await
            .estr()?;
        }

        added += 1;
    }

    tx.commit().await.estr()?;
    Ok(ImportSummary { added, skipped, updated: 0 })
}

/// Project a full channel down to the importer's preview shape.
fn preview_row(c: &Channel) -> BackupPreviewRow {
    BackupPreviewRow {
        repeaterbook_id: c.repeaterbook_id.clone().unwrap_or_default(),
        rb_name: c.rb_name.clone().unwrap_or_default(),
        name_long: c.name_long.clone().unwrap_or_default(),
        name_short: c.name_short.clone().unwrap_or_default(),
        callsign: c.callsign.clone().unwrap_or_default(),
        rx_freq: c.rx_freq,
        tx_freq: c.tx_freq,
        offset: c.offset.unwrap_or(0.0),
        duplex: c.duplex.clone().unwrap_or_default(),
        band: c.band.clone().unwrap_or_default(),
        mode: c.mode.clone().unwrap_or_default(),
        tone_mode: c.tone_mode.clone().unwrap_or_default(),
        ctcss_uplink: c.ctcss_uplink,
        ctcss_downlink: c.ctcss_downlink,
        dmr_color_code: c.dmr_color_code,
        dstar_capable: c.dstar_capable,
        ysf_capable: c.ysf_capable,
        nxdn_capable: c.nxdn_capable,
        p25_capable: c.p25_capable,
        p25_nac: c.p25_nac.clone(),
        m17_capable: c.m17_capable,
        tetra_capable: c.tetra_capable,
        allstar_node: c.allstar_node.clone(),
        echolink_node: c.echolink_node.clone(),
        irlp_node: c.irlp_node.clone(),
        wires_node: c.wires_node.clone(),
        use_type: c.use_type.clone(),
        operational_status: c.operational_status.clone(),
        city: c.city.clone(),
        county: c.county.clone(),
        state: c.state.clone(),
        country: c.country.clone(),
        latitude: c.latitude,
        longitude: c.longitude,
        notes: c.notes.clone(),
        ares: c.ares,
        races: c.races,
        skywarn: c.skywarn,
        canwarn: c.canwarn,
    }
}

/// A dependency-free UTC timestamp, e.g. "2026-06-21T14:03:55Z". Used only as a
/// human-readable stamp in the file header.
pub(crate) fn now_iso8601() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);

    let days = secs / 86_400;
    let tod = secs % 86_400;
    let (h, mi, s) = (tod / 3600, (tod % 3600) / 60, tod % 60);

    // Civil date from days since the Unix epoch (Howard Hinnant's algorithm).
    let z = days as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };

    format!("{y:04}-{m:02}-{d:02}T{h:02}:{mi:02}:{s:02}Z")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A manual channel with DCS + power survives an export → import round-trip
    /// with full fidelity, and a re-import is deduped (skipped). A talkgroup
    /// assignment is preserved too.
    #[tokio::test]
    async fn round_trips_channels_with_full_fidelity() {
        let dir = std::env::temp_dir().join(format!("cpm_io_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let db_path = dir.join("test.sqlite3");
        let _ = std::fs::remove_file(&db_path);
        let pool = crate::db::init_pool(&db_path).await.expect("init_pool");

        // A manual DTCS channel with a Low power setting, plus a DMR channel that
        // carries a talkgroup assignment.
        sqlx::query(
            "INSERT INTO channels (id, name_long, name_short, callsign, rx_freq, tx_freq,
                offset, duplex, band, mode, tone_mode, dcs_code, dcs_rx_code,
                dcs_polarity, power, source)
             VALUES (1, 'Manual DTCS', 'MANUAL', 'W0XYZ', 146.625, 146.025, 0.6, '-',
                     'VHF', 'NFM', 'DTCS', '023', '047', 'NR', 'Low', 'manual'),
                    (2, 'DMR Box', 'DMR', 'K0DMR', 449.0, 444.0, 5.0, '-', 'UHF',
                     'DMR', 'off', NULL, NULL, 'NN', NULL, 'manual')",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO talkgroups (tg_number, name, network, call_type, source)
             VALUES (3108, 'Colorado', 'TestNet', 'Group', 'manual')",
        )
        .execute(&pool)
        .await
        .unwrap();
        let tg: (i64,) =
            sqlx::query_as("SELECT id FROM talkgroups WHERE tg_number = 3108")
                .fetch_one(&pool)
                .await
                .unwrap();
        sqlx::query(
            "INSERT INTO repeater_talkgroups (channel_id, talkgroup_id, timeslot, position)
             VALUES (2, ?1, 1, 0)",
        )
        .bind(tg.0)
        .execute(&pool)
        .await
        .unwrap();

        // --- Export both channels ---
        let out = dir.join("backup.json");
        let backup = build_backup(&pool, &[1, 2]).await;
        std::fs::write(&out, serde_json::to_string_pretty(&backup).unwrap()).unwrap();
        assert_eq!(backup.count, 2);
        assert!(is_native(&out).await);

        // --- Import into a fresh library ---
        let db2 = dir.join("test2.sqlite3");
        let _ = std::fs::remove_file(&db2);
        let pool2 = crate::db::init_pool(&db2).await.expect("init_pool2");
        let parsed = read_backup(out.to_str().unwrap()).unwrap();
        let summary = insert_backup(&pool2, &parsed).await.unwrap();
        assert_eq!(summary.added, 2);
        assert_eq!(summary.skipped, 0);

        let dtcs = sqlx::query_as::<_, Channel>(
            "SELECT * FROM channels WHERE name_long = 'Manual DTCS'",
        )
        .fetch_one(&pool2)
        .await
        .unwrap();
        assert_eq!(dtcs.tone_mode.as_deref(), Some("DTCS"));
        assert_eq!(dtcs.dcs_code.as_deref(), Some("023"));
        assert_eq!(dtcs.dcs_rx_code.as_deref(), Some("047"));
        assert_eq!(dtcs.dcs_polarity, "NR");
        assert_eq!(dtcs.power.as_deref(), Some("Low"));
        assert_eq!(dtcs.source, "manual");

        // The DMR channel's talkgroup assignment came across.
        let dmr_id: (i64,) =
            sqlx::query_as("SELECT id FROM channels WHERE name_long = 'DMR Box'")
                .fetch_one(&pool2)
                .await
                .unwrap();
        let assigns: Vec<(i64, i64)> = sqlx::query_as(
            "SELECT tg.tg_number, rtg.timeslot
             FROM repeater_talkgroups rtg JOIN talkgroups tg ON tg.id = rtg.talkgroup_id
             WHERE rtg.channel_id = ?1",
        )
        .bind(dmr_id.0)
        .fetch_all(&pool2)
        .await
        .unwrap();
        assert_eq!(assigns, vec![(3108, 1)]);

        // --- Re-import is deduped ---
        let again = insert_backup(&pool2, &parsed).await.unwrap();
        assert_eq!(again.added, 0);
        assert_eq!(again.skipped, 2);

        let _ = std::fs::remove_file(&db_path);
        let _ = std::fs::remove_file(&db2);
        let _ = std::fs::remove_file(&out);
    }

    /// A backup written before the format id was renamed still routes to the
    /// native importer. It used to fail this probe, land in the RepeaterBook
    /// parser, and import zero channels as a *success*.
    #[tokio::test]
    async fn probe_accepts_the_legacy_format_id() {
        let dir = std::env::temp_dir();
        let path = dir.join(format!("cpm_legacy_{}.json", std::process::id()));
        std::fs::write(
            &path,
            format!(
                r#"{{"format":"{LEGACY_BACKUP_FORMAT}","version":1,"exported_at":"x","count":0,"channels":[]}}"#
            ),
        )
        .unwrap();
        assert!(
            is_native(&path).await,
            "legacy backup must route to the native importer"
        );
        // And the importer that probe selects still reads it.
        assert!(read_backup(path.to_str().unwrap()).is_ok());
        std::fs::remove_file(&path).ok();
    }

    #[tokio::test]
    async fn rejects_non_native_file() {
        let dir = std::env::temp_dir();
        let path = dir.join(format!("cpm_not_native_{}.json", std::process::id()));
        std::fs::write(&path, r#"{"records":[{"callsign":"W0X"}]}"#).unwrap();
        assert!(!is_native(&path).await);
        assert!(read_backup(path.to_str().unwrap()).is_err());
        std::fs::remove_file(&path).ok();
    }

    // -- test helpers (mirror the command bodies without a Tauri State) --

    async fn build_backup(pool: &SqlitePool, ids: &[i64]) -> ChannelBackup {
        let mut channels = Vec::new();
        for id in ids {
            let channel = sqlx::query_as::<_, Channel>("SELECT * FROM channels WHERE id = ?1")
                .bind(id)
                .fetch_one(pool)
                .await
                .unwrap();
            let talkgroups = sqlx::query_as::<_, (i64, String, String, String, i64, Option<String>)>(
                "SELECT tg.tg_number, tg.name, tg.network, tg.call_type, rtg.timeslot, rtg.name_override
                 FROM repeater_talkgroups rtg JOIN talkgroups tg ON tg.id = rtg.talkgroup_id
                 WHERE rtg.channel_id = ?1 ORDER BY rtg.position",
            )
            .bind(id)
            .fetch_all(pool)
            .await
            .unwrap()
            .into_iter()
            .map(|(tg_number, name, network, call_type, timeslot, name_override)| BackupTalkgroup {
                tg_number, name, network, call_type, timeslot, name_override,
            })
            .collect();
            channels.push(BackupChannel { channel, talkgroups });
        }
        ChannelBackup {
            format: BACKUP_FORMAT.to_string(),
            version: BACKUP_VERSION,
            exported_at: now_iso8601(),
            count: channels.len(),
            channels,
        }
    }

    /// Runs the real probe the import dialog routes on, rather than a copy of
    /// it — a copy would have gone on passing while the shipped probe rejected
    /// the legacy format.
    async fn is_native(path: &std::path::Path) -> bool {
        is_channel_backup(path.to_string_lossy().to_string())
            .await
            .unwrap()
    }
}
