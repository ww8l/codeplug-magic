use std::collections::HashSet;

use tauri::State;

use crate::db::AppState;
use crate::error::MapErrString;
use crate::models::{
    Channel, ExportPreview, ExportPreviewRow, RadioModel, RepeaterTalkgroup,
};

const MODEL_COLUMNS_PREFIXED: &str = "rm.id, rm.manufacturer, rm.model, rm.display_name, rm.analog_capable, rm.dmr_capable, rm.dstar_capable, rm.ysf_capable, rm.nxdn_capable, rm.p25_capable, rm.m17_capable, rm.aprs_capable, rm.covers_hf, rm.covers_vhf, rm.covers_uhf, rm.covers_220, rm.covers_900, rm.freq_min, rm.freq_max, rm.memory_channels, rm.zones_supported, rm.max_zones, rm.channels_per_zone, rm.scan_lists_supported, rm.max_scan_lists, rm.banks_supported, rm.max_name_length, rm.export_format, rm.connection_type, rm.non_channel_settings_schema, rm.driver_key, rm.programming_ui";

/// Resolve the radio model backing a codeplug (via its radio profile).
pub(crate) async fn codeplug_model(
    pool: &sqlx::SqlitePool,
    codeplug_id: i64,
) -> Result<RadioModel, String> {
    sqlx::query_as::<_, RadioModel>(&format!(
        r#"
        SELECT {MODEL_COLUMNS_PREFIXED}
        FROM codeplugs cp
        JOIN radio_profiles rp ON rp.id = cp.radio_profile_id
        JOIN radio_models rm ON rm.id = rp.radio_model_id
        WHERE cp.id = ?1
        "#
    ))
    .bind(codeplug_id)
    .fetch_optional(pool)
    .await
    .estr()?
    .ok_or_else(|| {
        "This codeplug has no radio profile assigned. Assign a radio profile before exporting."
            .to_string()
    })
}

/// A channel list as it applies to a codeplug: its name plus the channels it
/// contributes, in the list's own position order. This preserves which source
/// list each channel came from — the unit that maps to exactly one **zone** on
/// zone-capable radios and one **bank** on bank-capable radios. Radios that
/// support neither flatten these groups into a single deduped memory list (see
/// [`codeplug_channels`]).
///
/// Within a single group a channel appears at most once (deduped by id), but the
/// SAME channel may appear in MULTIPLE groups — a channel can legitimately live
/// in more than one zone/bank, so dedup is intentionally per-group, not global.
// Consumed by the zone/bank exporters once a zone- or bank-capable radio is
// added; for now only the flattening path and tests read it.
#[allow(dead_code)]
pub(crate) struct CodeplugGroup {
    pub list_id: i64,
    pub list_name: String,
    pub channels: Vec<Channel>,
}

/// Resolve a codeplug's assigned channel lists into ordered groups, preserving
/// list membership. Lists come back in codeplug assignment order; channels
/// within each in the list's position order. The building block for zone/bank
/// exporters; the flat exporters consume the flattened form via
/// [`codeplug_channels`].
pub(crate) async fn resolve_codeplug_groups(
    pool: &sqlx::SqlitePool,
    codeplug_id: i64,
) -> Result<Vec<CodeplugGroup>, String> {
    let lists = sqlx::query_as::<_, (i64, String)>(
        r#"
        SELECT cl.id, cl.name FROM codeplug_channel_lists ccl
        JOIN channel_lists cl ON cl.id = ccl.channel_list_id
        WHERE ccl.codeplug_id = ?1
        ORDER BY ccl.position
        "#,
    )
    .bind(codeplug_id)
    .fetch_all(pool)
    .await
    .estr()?;

    let mut groups = Vec::with_capacity(lists.len());
    for (list_id, list_name) in lists {
        let rows = sqlx::query_as::<_, Channel>(
            r#"
            SELECT c.* FROM channel_list_entries e
            JOIN channels c ON c.id = e.channel_id
            WHERE e.channel_list_id = ?1
            ORDER BY e.position
            "#,
        )
        .bind(list_id)
        .fetch_all(pool)
        .await
        .estr()?;

        let mut seen = HashSet::new();
        let channels = rows.into_iter().filter(|c| seen.insert(c.id)).collect();
        groups.push(CodeplugGroup {
            list_id,
            list_name,
            channels,
        });
    }
    Ok(groups)
}

/// Gather the distinct channels assigned to a codeplug, in channel-list order
/// then per-list position order. First occurrence of a channel wins. This is the
/// "neither zones nor banks" flattening: every channel from every applied list
/// goes into one shared memory pool.
async fn codeplug_channels(
    pool: &sqlx::SqlitePool,
    codeplug_id: i64,
) -> Result<Vec<Channel>, String> {
    let groups = resolve_codeplug_groups(pool, codeplug_id).await?;
    let mut seen = HashSet::new();
    Ok(groups
        .into_iter()
        .flat_map(|g| g.channels)
        .filter(|c| seen.insert(c.id))
        .collect())
}

/// One row destined for the radio. A DMR repeater that carries N talkgroups
/// expands into N of these (one channel per talkgroup x timeslot), because a
/// real radio channel holds exactly one talkgroup. Everything else (and a DMR
/// repeater with no assigned talkgroups) is a single passthrough row.
pub(crate) struct ExpandedChannel {
    pub(crate) channel: Channel,
    /// Talkgroup label appended to the channel name, e.g. "Colorado". Also used
    /// as the DMR contact name (Anytone links a channel to a contact by name).
    pub(crate) tg_label: Option<String>,
    pub(crate) timeslot: Option<i64>,
    pub(crate) tg_number: Option<i64>,
    /// "Group" or "Private" — the talkgroup's call type, for the contacts file.
    pub(crate) tg_call_type: Option<String>,
    /// True when the talkgroup came from the channel's inline `dmr_talkgroup`
    /// column (radio-imported) rather than a curated `repeater_talkgroups`
    /// assignment — the direct programmer keeps the original channel name for
    /// these instead of appending the talkgroup label.
    pub(crate) tg_inline: bool,
}

/// Expand DMR repeaters that have assigned talkgroups into one row per
/// (talkgroup x timeslot). Preserves input order; expanded rows follow the
/// talkgroup assignment order. A DMR channel with no `repeater_talkgroups`
/// rows but an inline `dmr_talkgroup` (every radio-imported channel looks like
/// this — the import writes the inline column only) expands to that single
/// talkgroup, so a round-trip program/export doesn't silently drop it.
/// Non-DMR channels and DMR channels with no talkgroup at all pass through
/// unchanged.
pub(crate) async fn expand_for_export(
    pool: &sqlx::SqlitePool,
    channels: Vec<Channel>,
) -> Result<Vec<ExpandedChannel>, String> {
    let mut out = Vec::new();
    for c in channels {
        if c.mode.as_deref() == Some("DMR") {
            let assigns = sqlx::query_as::<_, RepeaterTalkgroup>(
                r#"
                SELECT rtg.id, rtg.channel_id, rtg.talkgroup_id, rtg.timeslot, rtg.position,
                       rtg.name_override, tg.tg_number, tg.name, tg.network, tg.call_type
                FROM repeater_talkgroups rtg
                JOIN talkgroups tg ON tg.id = rtg.talkgroup_id
                WHERE rtg.channel_id = ?1
                ORDER BY rtg.position
                "#,
            )
            .bind(c.id)
            .fetch_all(pool)
            .await
            .estr()?;

            if !assigns.is_empty() {
                for a in assigns {
                    out.push(ExpandedChannel {
                        channel: c.clone(),
                        tg_label: Some(a.name_override.clone().unwrap_or_else(|| a.name.clone())),
                        timeslot: Some(a.timeslot),
                        tg_number: Some(a.tg_number),
                        tg_call_type: Some(a.call_type.clone()),
                        tg_inline: false,
                    });
                }
                continue;
            }

            if let Some(tg_num) = c.dmr_talkgroup {
                let named: Option<(String, String)> = sqlx::query_as(
                    "SELECT name, call_type FROM talkgroups WHERE tg_number = ?1 ORDER BY id LIMIT 1",
                )
                .bind(tg_num)
                .fetch_optional(pool)
                .await
                .estr()?;
                let (name, call_type) =
                    named.unwrap_or_else(|| (format!("TG {tg_num}"), "Group".to_string()));
                out.push(ExpandedChannel {
                    timeslot: Some(c.dmr_timeslot.unwrap_or(1)),
                    tg_label: Some(name),
                    tg_number: Some(tg_num),
                    tg_call_type: Some(call_type),
                    tg_inline: true,
                    channel: c,
                });
                continue;
            }
        }
        out.push(ExpandedChannel {
            channel: c,
            tg_label: None,
            timeslot: None,
            tg_number: None,
            tg_call_type: None,
            tg_inline: false,
        });
    }
    Ok(out)
}

/// A scan list assigned to a codeplug, with its members resolved. Carries the
/// raw DB columns; clamping them to a radio's field widths is the driver's job.
pub(crate) struct CodeplugScanList {
    pub id: i64,
    pub name: String,
    pub priority_channel_id: Option<i64>,
    pub priority_channel_2_id: Option<i64>,
    pub priority_select: i64,
    pub look_back_a: i64,
    pub look_back_b: i64,
    pub dropout_delay: i64,
    pub dwell_time: i64,
    pub revert_channel: i64,
    /// Member channel ids in list position order.
    pub member_channel_ids: Vec<i64>,
}

/// One row of `codeplug_channel_scan_lists`: "this channel launches that scan
/// list", independent of scan-list membership.
pub(crate) struct ChannelScanListOverride {
    pub channel_id: i64,
    pub scan_list_id: i64,
}

/// Everything a [`CodeplugProgrammer`](crate::radios::driver::CodeplugProgrammer)
/// needs, owned. Drivers borrow from this via `CodeplugPayload`; keeping the
/// storage here is what lets driver planners stay synchronous and DB-free.
pub(crate) struct ResolvedCodeplug {
    pub model: RadioModel,
    pub groups: Vec<CodeplugGroup>,
    pub channels: Vec<ExpandedChannel>,
    pub scan_lists: Vec<CodeplugScanList>,
    pub scan_list_overrides: Vec<ChannelScanListOverride>,
}

impl ResolvedCodeplug {
    /// Borrowed view for handing to a driver.
    pub(crate) fn payload(&self) -> crate::radios::driver::CodeplugPayload<'_> {
        crate::radios::driver::CodeplugPayload {
            model: &self.model,
            groups: &self.groups,
            channels: &self.channels,
            scan_lists: &self.scan_lists,
            scan_list_overrides: &self.scan_list_overrides,
        }
    }
}

/// Gather every row a driver needs to program `codeplug_id`, in one place.
///
/// The channel pool is the codeplug's lists in assignment order, deduped
/// globally (first list wins the memory slots; later lists still reference the
/// same slots through their groups), then DMR-expanded. Scan lists come back in
/// id order — `codeplug_scan_lists` has no position column.
pub(crate) async fn resolve_codeplug_payload(
    pool: &sqlx::SqlitePool,
    codeplug_id: i64,
) -> Result<ResolvedCodeplug, String> {
    let model = codeplug_model(pool, codeplug_id).await?;
    let groups = resolve_codeplug_groups(pool, codeplug_id).await?;

    let mut seen = HashSet::new();
    let mut pool_channels: Vec<Channel> = Vec::new();
    for g in &groups {
        for c in &g.channels {
            if seen.insert(c.id) {
                pool_channels.push(c.clone());
            }
        }
    }
    let channels = expand_for_export(pool, pool_channels).await?;

    #[allow(clippy::type_complexity)]
    let scan_rows = sqlx::query_as::<
        _,
        (i64, String, Option<i64>, Option<i64>, i64, i64, i64, i64, i64, i64),
    >(
        r#"
        SELECT sl.id, sl.name,
               sl.priority_channel_id, sl.priority_channel_2_id, sl.priority_select,
               sl.look_back_a, sl.look_back_b, sl.dropout_delay, sl.dwell_time, sl.revert_channel
        FROM codeplug_scan_lists csl
        JOIN scan_lists sl ON sl.id = csl.scan_list_id
        WHERE csl.codeplug_id = ?1
        ORDER BY sl.id
        "#,
    )
    .bind(codeplug_id)
    .fetch_all(pool)
    .await
    .estr()?;

    let mut scan_lists = Vec::with_capacity(scan_rows.len());
    for (
        id,
        name,
        priority_channel_id,
        priority_channel_2_id,
        priority_select,
        look_back_a,
        look_back_b,
        dropout_delay,
        dwell_time,
        revert_channel,
    ) in scan_rows
    {
        let member_channel_ids = sqlx::query_as::<_, (i64,)>(
            "SELECT channel_id FROM scan_list_entries WHERE scan_list_id = ?1 ORDER BY position",
        )
        .bind(id)
        .fetch_all(pool)
        .await
        .estr()?
        .into_iter()
        .map(|(cid,)| cid)
        .collect();

        scan_lists.push(CodeplugScanList {
            id,
            name,
            priority_channel_id,
            priority_channel_2_id,
            priority_select,
            look_back_a,
            look_back_b,
            dropout_delay,
            dwell_time,
            revert_channel,
            member_channel_ids,
        });
    }

    let scan_list_overrides = sqlx::query_as::<_, (i64, i64)>(
        "SELECT channel_id, scan_list_id FROM codeplug_channel_scan_lists WHERE codeplug_id = ?1",
    )
    .bind(codeplug_id)
    .fetch_all(pool)
    .await
    .estr()?
    .into_iter()
    .map(|(channel_id, scan_list_id)| ChannelScanListOverride {
        channel_id,
        scan_list_id,
    })
    .collect();

    Ok(ResolvedCodeplug {
        model,
        groups,
        channels,
        scan_lists,
        scan_list_overrides,
    })
}

/// Decide whether a channel can be exported to a given radio.
/// Returns `None` if the channel is included, or `Some(reason)` if excluded.
pub(crate) fn exclusion_reason(channel: &Channel, model: &RadioModel) -> Option<String> {
    // Mode compatibility.
    let mode = channel.mode.as_deref().unwrap_or("FM").to_uppercase();
    let mode_ok = match mode.as_str() {
        "FM" | "NFM" | "AM" | "USB" | "LSB" | "CW" => model.analog_capable,
        "DMR" => model.dmr_capable,
        "DSTAR" => model.dstar_capable,
        "YSF" => model.ysf_capable,
        "NXDN" => model.nxdn_capable,
        "P25" => model.p25_capable,
        "M17" => model.m17_capable,
        _ => model.analog_capable,
    };
    if !mode_ok {
        return Some(format!(
            "Mode {mode} not supported by {}",
            model.display_name
        ));
    }

    // Frequency / band coverage.
    if let (Some(min), Some(max)) = (model.freq_min, model.freq_max) {
        if channel.rx_freq < min || channel.rx_freq > max {
            return Some(format!(
                "Frequency {:.4} MHz is outside the {} range ({:.1}–{:.1} MHz)",
                channel.rx_freq, model.display_name, min, max
            ));
        }
    }

    None
}

/// One programmable memory slot for the direct radio programmer. `slot` is the
/// channel's 0-based memory position. Included channels are packed contiguously
/// from slot 0 with no gaps — excluded (e.g. digital-mode) channels are dropped
/// and the survivors close up behind them. Only included channels are returned;
/// the caller clears every slot not listed here.
pub(crate) struct SlotChannel {
    pub slot: usize,
    pub name: String,
    pub channel: Channel,
}

/// Resolve the channels a codeplug will program into a radio: the radio model
/// plus the included (DMR-expanded) rows, each tagged with the memory slot it
/// occupies. Included channels are packed contiguously from slot 0 (gaps from
/// excluded channels are closed up). Shares the same name-length and exclusion
/// rules as the file exporter (`export_preview`).
pub(crate) async fn resolve_codeplug_slots(
    pool: &sqlx::SqlitePool,
    codeplug_id: i64,
) -> Result<(RadioModel, Vec<SlotChannel>), String> {
    let model = codeplug_model(pool, codeplug_id).await?;
    let channels = codeplug_channels(pool, codeplug_id).await?;
    let expanded = expand_for_export(pool, channels).await?;
    let slots = expanded
        .iter()
        .filter(|ec| exclusion_reason(&ec.channel, &model).is_none())
        .enumerate()
        .map(|(slot, ec)| SlotChannel {
            slot,
            name: expanded_name(ec, &model),
            channel: ec.channel.clone(),
        })
        .collect();
    Ok((model, slots))
}

#[tauri::command]
pub async fn export_preview(
    state: State<'_, AppState>,
    codeplug_id: i64,
) -> Result<ExportPreview, String> {
    let model = codeplug_model(&state.pool, codeplug_id).await?;
    let channels = codeplug_channels(&state.pool, codeplug_id).await?;
    let expanded = expand_for_export(&state.pool, channels).await?;

    let mut rows = Vec::with_capacity(expanded.len());
    let mut included = 0usize;
    let mut excluded = 0usize;

    for ec in &expanded {
        let reason = exclusion_reason(&ec.channel, &model);
        let is_included = reason.is_none();
        if is_included {
            included += 1;
        } else {
            excluded += 1;
        }
        rows.push(ExportPreviewRow {
            channel_id: ec.channel.id,
            name: expanded_name(ec, &model),
            rx_freq: ec.channel.rx_freq,
            mode: ec.channel.mode.clone(),
            included: is_included,
            reason,
        });
    }

    Ok(ExportPreview {
        codeplug_id,
        radio_model: model.display_name,
        export_format: model.export_format.clone().unwrap_or_default(),
        included_count: included,
        excluded_count: excluded,
        rows,
    })
}

#[tauri::command]
pub async fn generate_codeplug(
    state: State<'_, AppState>,
    codeplug_id: i64,
    path: String,
) -> Result<usize, String> {
    let model = codeplug_model(&state.pool, codeplug_id).await?;
    let channels = codeplug_channels(&state.pool, codeplug_id).await?;
    let expanded = expand_for_export(&state.pool, channels).await?;

    let included: Vec<&ExpandedChannel> = expanded
        .iter()
        .filter(|ec| exclusion_reason(&ec.channel, &model).is_none())
        .collect();

    match model.export_format.as_deref() {
        // DMR-native: a Channel CSV with real DMR columns + a Digital Contacts
        // (TalkGroups) CSV, written alongside the chosen path.
        Some("anytone_csv") => {
            crate::radios::anytone_atd890uv::export::write_anytone_bundle(&path, &included, &model)?
        }
        // Default: a single CHIRP-compatible analog CSV.
        _ => {
            let csv = render_chirp_csv(&included, &model)?;
            std::fs::write(&path, csv).map_err(|e| format!("Could not write file: {e}"))?;
        }
    }

    sqlx::query(
        "UPDATE codeplugs SET last_exported = CURRENT_TIMESTAMP, last_export_kind = 'file' WHERE id = ?1",
    )
    .bind(codeplug_id)
    .execute(&state.pool)
    .await
    .estr()?;

    Ok(included.len())
}

/// Build the channel name per the radio's name-length limit, appending the
/// talkgroup label for expanded DMR rows ("W0XYZ Colorado"), then truncating to
/// the radio's max name length.
pub(crate) fn expanded_name(ec: &ExpandedChannel, model: &RadioModel) -> String {
    let max = model.max_name_length.unwrap_or(16);
    let base = if max <= 7 {
        ec.channel.name_short.clone().unwrap_or_default()
    } else {
        ec.channel.name_long.clone().unwrap_or_default()
    };
    let full = match &ec.tg_label {
        Some(tg) => format!("{base} {tg}").trim().to_string(),
        None => base,
    };
    full.chars().take(max as usize).collect()
}

/// Render the included (already-expanded) channels as a CHIRP-compatible CSV.
/// CHIRP CSV is an analog format with no DMR fields, so per-talkgroup
/// programming (color code / timeslot / talkgroup) is recorded in the Comment
/// column for reference until a DMR-native exporter is added.
fn render_chirp_csv(channels: &[&ExpandedChannel], model: &RadioModel) -> Result<String, String> {
    let mut wtr = csv::Writer::from_writer(vec![]);

    wtr.write_record([
        "Location",
        "Name",
        "Frequency",
        "Duplex",
        "Offset",
        "Tone",
        "rToneFreq",
        "cToneFreq",
        "DtcsCode",
        "DtcsPolarity",
        "RxDtcsCode",
        "CrossMode",
        "Mode",
        "TStep",
        "Skip",
        "Power",
        "Comment",
    ])
    .estr()?;

    for (i, ec) in channels.iter().enumerate() {
        let c = &ec.channel;
        let duplex = match c.duplex.as_deref() {
            Some("+") => "+",
            Some("-") => "-",
            Some("split") => "split",
            _ => "",
        };
        // tone_mode already uses CHIRP's tmode vocabulary (Tone/TSQL/DTCS/Cross);
        // "off"/none maps to CHIRP's empty tmode.
        let tone = match c.tone_mode.as_deref() {
            Some(m) if !m.eq_ignore_ascii_case("off") && !m.is_empty() => m,
            _ => "",
        };
        let rx_dtcs = c
            .dcs_rx_code
            .clone()
            .or_else(|| c.dcs_code.clone())
            .unwrap_or_else(|| "023".to_string());
        let chirp_mode = match c.mode.as_deref().unwrap_or("FM").to_uppercase().as_str() {
            "AM" => "AM",
            "DSTAR" => "DV",
            "NFM" => "NFM",
            _ => "FM",
        };
        // For expanded DMR rows, record the talkgroup programming in the comment
        // since CHIRP CSV has nowhere else to put it.
        let mut comment_parts = vec![
            c.callsign.clone().unwrap_or_default(),
            c.city.clone().unwrap_or_default(),
            c.state.clone().unwrap_or_default(),
        ];
        if let Some(tg_num) = ec.tg_number {
            let mut dmr = String::new();
            if let Some(cc) = c.dmr_color_code {
                dmr.push_str(&format!("CC{cc} "));
            }
            if let Some(ts) = ec.timeslot {
                dmr.push_str(&format!("TS{ts} "));
            }
            dmr.push_str(&format!("TG{tg_num}"));
            comment_parts.push(dmr);
        }
        let comment = comment_parts
            .join(" ")
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");

        wtr.write_record([
            i.to_string(),
            expanded_name(ec, model),
            format!("{:.6}", c.rx_freq),
            duplex.to_string(),
            format!("{:.6}", c.offset.unwrap_or(0.0).abs()),
            tone.to_string(),
            format!("{:.1}", c.ctcss_uplink.unwrap_or(88.5)),
            format!("{:.1}", c.ctcss_downlink.unwrap_or(88.5)),
            c.dcs_code.clone().unwrap_or_else(|| "023".to_string()),
            c.dcs_polarity.clone(),
            rx_dtcs,
            c.cross_mode.clone(),
            chirp_mode.to_string(),
            "5.00".to_string(),
            String::new(),
            c.power.clone().unwrap_or_default(),
            comment,
        ])
        .estr()?;
    }

    let bytes = wtr.into_inner().map_err(|e| e.to_string())?;
    String::from_utf8(bytes).map_err(|e| e.to_string())
}

// ============================================================
// Anytone CSV bundle (DMR-native)
// ============================================================

/// Compute the actual transmit frequency from rx + duplex/offset, preferring an
/// explicit tx_freq when present (e.g. odd splits).
pub(crate) fn tx_frequency(c: &Channel) -> f64 {
    if let Some(tx) = c.tx_freq {
        return tx;
    }
    let off = c.offset.unwrap_or(0.0).abs();
    match c.duplex.as_deref() {
        Some("+") => c.rx_freq + off,
        Some("-") => c.rx_freq - off,
        _ => c.rx_freq,
    }
}


#[cfg(test)]
mod tests {
    use super::*;
    use crate::radios::anytone_atd890uv::export::{render_anytone_channels, render_anytone_talkgroups};

    /// Channel lists become ordered groups (one per list, in assignment order),
    /// preserving per-list channel order. A channel shared by two lists appears
    /// in BOTH groups (a channel can be in multiple zones/banks), but the flat
    /// `codeplug_channels` view dedups it to one shared-memory entry.
    #[tokio::test]
    async fn groups_preserve_list_membership_then_flatten() {
        let dir = std::env::temp_dir().join(format!("cpm_grp_{}", std::process::id()));
        let db_path = dir.join("test.sqlite3");
        let _ = std::fs::remove_file(&db_path);
        let pool = crate::db::init_pool(&db_path).await.expect("init_pool");

        sqlx::query(
            "INSERT INTO channels (id, name_long, name_short, rx_freq, mode, source)
             VALUES (1, 'A', 'A', 146.0, 'FM', 'manual'),
                    (2, 'B', 'B', 147.0, 'FM', 'manual'),
                    (3, 'C', 'C', 148.0, 'FM', 'manual')",
        )
        .execute(&pool)
        .await
        .unwrap();

        // Two lists: "Repeaters" [1,2], "Simplex" [2,3] — channel 2 is shared.
        sqlx::query(
            "INSERT INTO channel_lists (id, name) VALUES (10, 'Repeaters'), (20, 'Simplex')",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO channel_list_entries (channel_list_id, channel_id, position)
             VALUES (10, 1, 0), (10, 2, 1), (20, 2, 0), (20, 3, 1)",
        )
        .execute(&pool)
        .await
        .unwrap();

        // Codeplug 100 applies Simplex first (position 0), then Repeaters.
        sqlx::query("INSERT INTO codeplugs (id, name) VALUES (100, 'CP')")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO codeplug_channel_lists (codeplug_id, channel_list_id, position)
             VALUES (100, 20, 0), (100, 10, 1)",
        )
        .execute(&pool)
        .await
        .unwrap();

        let groups = resolve_codeplug_groups(&pool, 100).await.unwrap();
        assert_eq!(groups.len(), 2);
        // Assignment order: Simplex then Repeaters.
        assert_eq!(groups[0].list_name, "Simplex");
        assert_eq!(
            groups[0].channels.iter().map(|c| c.id).collect::<Vec<_>>(),
            vec![2, 3]
        );
        assert_eq!(groups[1].list_name, "Repeaters");
        assert_eq!(
            groups[1].channels.iter().map(|c| c.id).collect::<Vec<_>>(),
            vec![1, 2]
        );

        // Flattened: list order then position, channel 2 deduped (first wins).
        let flat = codeplug_channels(&pool, 100).await.unwrap();
        assert_eq!(flat.iter().map(|c| c.id).collect::<Vec<_>>(), vec![2, 3, 1]);

        let _ = std::fs::remove_file(&db_path);
    }

    /// A DMR repeater with two assigned talkgroups should expand into two
    /// export rows (one per talkgroup), each named "<repeater> <talkgroup>",
    /// while a plain FM repeater passes through as a single row.
    #[tokio::test]
    async fn dmr_repeater_expands_per_talkgroup() {
        let dir = std::env::temp_dir().join(format!("cpm_exp_{}", std::process::id()));
        let db_path = dir.join("test.sqlite3");
        let _ = std::fs::remove_file(&db_path);
        let pool = crate::db::init_pool(&db_path).await.expect("init_pool");

        // One DMR repeater + one FM repeater.
        sqlx::query(
            "INSERT INTO channels (id, name_long, name_short, rx_freq, mode, dmr_color_code, source)
             VALUES (1, 'W0XYZ Denver', 'W0XYZ', 449.0, 'DMR', 1, 'manual'),
                    (2, 'K0FM Boulder', 'K0FM', 146.94, 'FM', NULL, 'manual')",
        )
        .execute(&pool)
        .await
        .unwrap();

        // Use the seeded Colorado (3108) and Worldwide (91) talkgroups, assigned
        // to the DMR repeater on different timeslots. (init_pool seeds these.)
        // Own talkgroups on a private test network with controlled names, so the
        // assertions don't depend on the seeded Brandmeister list.
        sqlx::query(
            "INSERT INTO talkgroups (tg_number, name, network, call_type, source)
             VALUES (3108, 'Colorado', 'TestNet', 'Group', 'manual'),
                    (91, 'Worldwide', 'TestNet', 'Group', 'manual')",
        )
        .execute(&pool)
        .await
        .unwrap();
        let co: (i64,) =
            sqlx::query_as("SELECT id FROM talkgroups WHERE tg_number = 3108 AND network = 'TestNet'")
                .fetch_one(&pool)
                .await
                .unwrap();
        let ww: (i64,) =
            sqlx::query_as("SELECT id FROM talkgroups WHERE tg_number = 91 AND network = 'TestNet'")
                .fetch_one(&pool)
                .await
                .unwrap();
        sqlx::query(
            "INSERT INTO repeater_talkgroups (channel_id, talkgroup_id, timeslot, position)
             VALUES (1, ?1, 2, 0), (1, ?2, 1, 1)",
        )
        .bind(co.0)
        .bind(ww.0)
        .execute(&pool)
        .await
        .unwrap();

        let channels = sqlx::query_as::<_, Channel>("SELECT * FROM channels ORDER BY id")
            .fetch_all(&pool)
            .await
            .unwrap();
        let expanded = expand_for_export(&pool, channels).await.unwrap();

        // 2 (DMR talkgroups) + 1 (FM passthrough) = 3 rows.
        assert_eq!(expanded.len(), 3);

        let model = RadioModel {
            max_name_length: Some(16),
            ..sqlx::query_as::<_, RadioModel>(&format!(
                "SELECT {} FROM radio_models rm LIMIT 1",
                MODEL_COLUMNS_PREFIXED
            ))
            .fetch_one(&pool)
            .await
            .unwrap()
        };

        assert_eq!(expanded[0].tg_number, Some(3108));
        assert_eq!(expanded[0].timeslot, Some(2));
        assert_eq!(expanded_name(&expanded[0], &model), "W0XYZ Denver Col");
        assert_eq!(expanded[1].tg_number, Some(91));
        assert!(expanded[2].tg_label.is_none()); // FM passthrough

        let _ = std::fs::remove_file(&db_path);
    }

    /// The Anytone bundle renders DMR rows as D-Digital with per-talkgroup
    /// programming in dedicated columns, FM rows as A-Analog, and emits one
    /// contact per distinct talkgroup.
    #[tokio::test]
    async fn anytone_bundle_renders_dmr_and_contacts() {
        let dir = std::env::temp_dir().join(format!("cpm_any_{}", std::process::id()));
        let db_path = dir.join("test.sqlite3");
        let _ = std::fs::remove_file(&db_path);
        let pool = crate::db::init_pool(&db_path).await.expect("init_pool");

        sqlx::query(
            "INSERT INTO channels (id, name_long, name_short, rx_freq, offset, duplex, mode, tone_mode, ctcss_uplink, ctcss_downlink, dmr_color_code, source)
             VALUES (1, 'W0XYZ Denver', 'W0XYZ', 449.0, 5.0, '+', 'DMR', NULL, NULL, NULL, 1, 'manual'),
                    (2, 'K0FM Boulder', 'K0FM', 146.94, 0.6, '-', 'FM', 'Tone', 100.0, 100.0, NULL, 'manual')",
        )
        .execute(&pool)
        .await
        .unwrap();

        // Own talkgroups on a private test network with controlled names, so the
        // assertions don't depend on the seeded Brandmeister list.
        sqlx::query(
            "INSERT INTO talkgroups (tg_number, name, network, call_type, source)
             VALUES (3108, 'Colorado', 'TestNet', 'Group', 'manual'),
                    (91, 'Worldwide', 'TestNet', 'Group', 'manual')",
        )
        .execute(&pool)
        .await
        .unwrap();
        let co: (i64,) =
            sqlx::query_as("SELECT id FROM talkgroups WHERE tg_number = 3108 AND network = 'TestNet'")
                .fetch_one(&pool)
                .await
                .unwrap();
        let ww: (i64,) =
            sqlx::query_as("SELECT id FROM talkgroups WHERE tg_number = 91 AND network = 'TestNet'")
                .fetch_one(&pool)
                .await
                .unwrap();
        sqlx::query(
            "INSERT INTO repeater_talkgroups (channel_id, talkgroup_id, timeslot, position)
             VALUES (1, ?1, 2, 0), (1, ?2, 1, 1)",
        )
        .bind(co.0)
        .bind(ww.0)
        .execute(&pool)
        .await
        .unwrap();

        let channels = sqlx::query_as::<_, Channel>("SELECT * FROM channels ORDER BY id")
            .fetch_all(&pool)
            .await
            .unwrap();
        let expanded = expand_for_export(&pool, channels).await.unwrap();
        let refs: Vec<&ExpandedChannel> = expanded.iter().collect();

        // A DMR radio that uses the Anytone bundle exporter. Inserted here since
        // the seed library is trimmed to the UV-5R.
        sqlx::query(
            "INSERT INTO radio_models (manufacturer, model, display_name, analog_capable, dmr_capable, freq_min, freq_max, max_name_length, export_format)
             VALUES ('Test', 'DMR-878', 'Test DMR', 1, 1, 136.0, 480.0, 16, 'anytone_csv')",
        )
        .execute(&pool)
        .await
        .unwrap();
        let model = sqlx::query_as::<_, RadioModel>(&format!(
            "SELECT {} FROM radio_models rm WHERE rm.model = 'DMR-878'",
            MODEL_COLUMNS_PREFIXED
        ))
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(model.export_format.as_deref(), Some("anytone_csv"));

        let chan_csv = render_anytone_channels(&refs, &model).unwrap();
        let tg_csv = render_anytone_talkgroups(&refs).unwrap();

        // DMR row: D-Digital, TG 3108 on slot 2, color code 1; TX = 449 + 5.
        assert!(chan_csv.contains("D-Digital"));
        assert!(chan_csv.contains("454.00000"));
        let dmr_line = chan_csv
            .lines()
            .find(|l| l.contains("Colorado"))
            .expect("Colorado channel line");
        assert!(dmr_line.contains(",Colorado,Group Call,3108,1,2,"));
        // FM row: A-Analog with CTCSS, TX = 146.94 - 0.6.
        assert!(chan_csv.contains("A-Analog"));
        assert!(chan_csv.contains("146.34000"));

        // Contacts file: 2 distinct talkgroups (Colorado, Worldwide) + header.
        assert_eq!(tg_csv.lines().count(), 3);
        assert!(tg_csv.contains(",3108,Colorado,Group Call,None"));
        assert!(tg_csv.contains(",91,Worldwide,Group Call,None"));

        let _ = std::fs::remove_file(&db_path);
    }
}
