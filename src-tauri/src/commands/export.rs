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

    // Frequency: the min/max pair is one contiguous span, so it cannot describe
    // a radio with a hole in the middle (a UV-5R is 136–520 but dead between
    // 174 and 400). Check the band-coverage flags too, or a 224 MHz channel
    // sails through 136–520 onto a radio that cannot transmit on it.
    if let (Some(min), Some(max)) = (model.freq_min, model.freq_max) {
        if channel.rx_freq < min || channel.rx_freq > max {
            return Some(format!(
                "Frequency {:.4} MHz is outside the {} range ({:.1}–{:.1} MHz)",
                channel.rx_freq, model.display_name, min, max
            ));
        }
    }
    if let Some((covered, band)) = band_coverage(model, channel.rx_freq) {
        if !covered {
            return Some(format!(
                "Frequency {:.4} MHz is in the {band} band, which {} does not cover",
                channel.rx_freq, model.display_name
            ));
        }
    }

    None
}

/// Which `covers_*` flag governs a frequency, and whether the model sets it.
/// Returns `None` for frequencies in no band we track (the 174–219 and 225–400
/// stretches, anything above 928) — those are judged by `freq_min`/`freq_max`
/// alone rather than guessed at.
fn band_coverage(model: &RadioModel, freq: f64) -> Option<(bool, &'static str)> {
    match freq {
        f if f < 30.0 => Some((model.covers_hf, "HF")),
        f if (30.0..=174.0).contains(&f) => Some((model.covers_vhf, "VHF")),
        f if (219.0..=225.0).contains(&f) => Some((model.covers_220, "220 MHz")),
        f if (400.0..=520.0).contains(&f) => Some((model.covers_uhf, "UHF")),
        f if (902.0..=928.0).contains(&f) => Some((model.covers_900, "900 MHz")),
        _ => None,
    }
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
    let included: Vec<&ExpandedChannel> = expanded
        .iter()
        .filter(|ec| exclusion_reason(&ec.channel, &model).is_none())
        .collect();
    let names = expanded_names(included.iter().copied(), &model);
    let slots = included
        .into_iter()
        .zip(names)
        .enumerate()
        .map(|(slot, (ec, name))| SlotChannel {
            slot,
            name,
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

    // Dedup runs over the included set only — those are the names that share a
    // radio — so the preview shows exactly what programming will write.
    let reasons: Vec<Option<String>> = expanded
        .iter()
        .map(|ec| exclusion_reason(&ec.channel, &model))
        .collect();
    let mut included_names = expanded_names(
        expanded
            .iter()
            .zip(&reasons)
            .filter(|(_, r)| r.is_none())
            .map(|(ec, _)| ec),
        &model,
    )
    .into_iter();

    for (ec, reason) in expanded.iter().zip(reasons) {
        let is_included = reason.is_none();
        let name = if is_included {
            included_names.next().unwrap_or_default()
        } else {
            expanded_name(ec, &model)
        };
        if is_included {
            included += 1;
        } else {
            excluded += 1;
        }
        rows.push(ExportPreviewRow {
            channel_id: ec.channel.id,
            name,
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

    // Format-keyed registry lookup (Chunk 3.8): a model picks its exporter by
    // `export_format`, never by name and never by driver — an export-only model
    // has no driver but still names a format. Unclaimed formats (and NULL) get
    // the shared CHIRP-compatible analog CSV.
    match model
        .export_format
        .as_deref()
        .and_then(crate::radios::registry::exporter_for_format)
    {
        Some(exporter) => {
            exporter.export(&path, &included, &model)?;
        }
        None => {
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
    let max = model.max_name_length.unwrap_or(16) as usize;
    let compose = |base: &str| match &ec.tg_label {
        Some(tg) => format!("{base} {tg}").trim().to_string(),
        None => base.to_string(),
    };
    let long = compose(ec.channel.name_long.as_deref().unwrap_or_default());
    let short = compose(ec.channel.name_short.as_deref().unwrap_or_default());

    // Take the long name only when the whole thing fits. Truncating it to a
    // narrow radio is how two different repeaters both ended up as "W0QEY Fo"
    // on an 8-char TD-H3 while the 7-char UV-5R showed clean callsigns — the
    // short name is already a distinct label, so prefer it over a stump.
    let chosen = if !long.is_empty() && long.chars().count() <= max {
        long
    } else if !short.is_empty() {
        short
    } else {
        long
    };
    chosen.chars().take(max).collect()
}

/// Name every channel that will reach the radio, then pull apart any that came
/// out identical. Callers pass the *included* set in slot order — dedup is only
/// meaningful across the channels that share one radio's memory.
pub(crate) fn expanded_names<'a, I>(ecs: I, model: &RadioModel) -> Vec<String>
where
    I: IntoIterator<Item = &'a ExpandedChannel>,
{
    let max = model.max_name_length.unwrap_or(16) as usize;
    let mut rows: Vec<(String, f64)> = ecs
        .into_iter()
        .map(|ec| (expanded_name(ec, model), ec.channel.rx_freq))
        .collect();
    disambiguate_names(&mut rows, max);
    rows.into_iter().map(|(name, _)| name).collect()
}

/// Make `(name, rx_freq)` rows unique in place, marking collisions by the thing
/// that actually distinguishes the channels — the frequency. Two W0QEY
/// repeaters on different bands become "W0QEY U" / "W0QEY V"; a same-band pair
/// falls back to the frequency's hundredths ("W0QEY 85" / "W0QEY 58"), and an
/// unresolvable tie to a counter. Names are re-truncated to `max`, so the
/// marker always displaces base characters rather than overrunning the radio.
pub(crate) fn disambiguate_names(rows: &mut [(String, f64)], max: usize) {
    use std::collections::{HashMap, HashSet};

    let mut groups: HashMap<String, Vec<usize>> = HashMap::new();
    for (i, (name, _)) in rows.iter().enumerate() {
        groups.entry(name.clone()).or_default().push(i);
    }
    let mut collisions: Vec<Vec<usize>> = groups.into_values().filter(|g| g.len() > 1).collect();
    // HashMap order is arbitrary; process by first appearance so the output is
    // reproducible run to run.
    collisions.sort_by_key(|g| g[0]);

    let mut taken: HashSet<String> = rows.iter().map(|(n, _)| n.clone()).collect();

    for group in collisions {
        let base = rows[group[0]].0.clone();
        // Everything in this group currently answers to `base`; free it so a
        // candidate is only rejected for clashing with some *other* name.
        taken.remove(&base);

        let mut applied = false;
        for level in 0..2 {
            let candidates: Vec<String> = group
                .iter()
                .map(|&i| {
                    let marker = match level {
                        0 => band_marker(rows[i].1).to_string(),
                        _ => freq_marker(rows[i].1),
                    };
                    with_marker(&base, &marker, max)
                })
                .collect();
            let distinct: HashSet<&String> = candidates.iter().collect();
            if distinct.len() == group.len() && !candidates.iter().any(|c| taken.contains(c)) {
                for (&i, name) in group.iter().zip(candidates) {
                    taken.insert(name.clone());
                    rows[i].0 = name;
                }
                applied = true;
                break;
            }
        }

        if !applied {
            // Last resort: the first keeps the bare name, the rest are counted.
            taken.insert(base.clone());
            let mut n = 2;
            for &i in group.iter().skip(1) {
                let name = loop {
                    let candidate = with_marker(&base, &n.to_string(), max);
                    n += 1;
                    if !taken.contains(&candidate) || n > 99 {
                        break candidate;
                    }
                };
                taken.insert(name.clone());
                rows[i].0 = name;
            }
        }
    }
}

/// Single-character band tag: the coarsest thing that tells two same-named
/// channels apart, and the one an operator can read off the front panel.
fn band_marker(freq: f64) -> &'static str {
    match freq {
        f if f < 30.0 => "H",
        f if f < 174.0 => "V",
        f if f < 300.0 => "2",
        f if f < 902.0 => "U",
        _ => "9",
    }
}

/// The frequency's first two decimal digits, read straight off the number the
/// operator sees — 449.850 → "85", 146.625 → "62". Never rounded: 146.625 must
/// not come back as "63", or the marker stops matching the dial.
fn freq_marker(freq: f64) -> String {
    let text = format!("{:.4}", freq.abs());
    let decimals = text.split('.').nth(1).unwrap_or("00");
    decimals.chars().take(2).collect()
}

/// Fit `base` + `marker` into `max` characters, keeping a separating space when
/// the base can spare it and dropping it when it cannot.
fn with_marker(base: &str, marker: &str, max: usize) -> String {
    let marker_len = marker.chars().count();
    if marker_len + 1 > max {
        return marker.chars().take(max).collect();
    }
    for sep in [" ", ""] {
        let keep = max - marker_len - sep.len();
        if keep >= 2 || (sep.is_empty() && keep >= 1) {
            let head: String = base.chars().take(keep).collect();
            return format!("{}{sep}{marker}", head.trim_end());
        }
    }
    marker.chars().take(max).collect()
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

    let names = expanded_names(channels.iter().copied(), model);
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
            names[i].clone(),
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
        // "W0XYZ Denver Colorado" overruns 16, so the short name carries the
        // talkgroup instead of shipping a truncated stump (issue #26).
        assert_eq!(expanded_name(&expanded[0], &model), "W0XYZ Colorado");
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

    /// A radio model with just the fields the name/exclusion rules read.
    fn model_with(max_name_length: i64) -> RadioModel {
        RadioModel {
            display_name: "Test Radio".into(),
            analog_capable: true,
            covers_vhf: true,
            covers_uhf: true,
            freq_min: Some(136.0),
            freq_max: Some(520.0),
            max_name_length: Some(max_name_length),
            ..Default::default()
        }
    }

    fn expanded(name_long: &str, name_short: &str, tg: Option<&str>) -> ExpandedChannel {
        ExpandedChannel {
            channel: Channel {
                name_long: Some(name_long.into()),
                name_short: Some(name_short.into()),
                rx_freq: 146.94,
                ..Default::default()
            },
            tg_label: tg.map(Into::into),
            timeslot: None,
            tg_number: None,
            tg_call_type: None,
            tg_inline: false,
        }
    }

    /// Issue #26: an 8-char radio used to truncate the long name, so two
    /// different repeaters both shipped as "W0QEY Fo". The short name wins
    /// whenever the long one does not fit whole.
    #[test]
    fn short_name_wins_when_the_long_one_would_be_truncated() {
        let ec = expanded("W0QEY Fort Collins", "W0QEY", None);
        assert_eq!(expanded_name(&ec, &model_with(8)), "W0QEY");
        assert_eq!(expanded_name(&ec, &model_with(7)), "W0QEY");
        // Room for the whole long name — take it, it is more informative.
        assert_eq!(expanded_name(&ec, &model_with(18)), "W0QEY Fort Collins");
        assert_eq!(expanded_name(&ec, &model_with(16)), "W0QEY");
    }

    #[test]
    fn name_falls_back_and_still_respects_the_limit() {
        // No short name: truncating the long one is all that is left.
        let no_short = expanded("W0QEY Fort Collins", "", None);
        assert_eq!(expanded_name(&no_short, &model_with(8)), "W0QEY Fo");
        // Short name longer than the radio allows is truncated too.
        let long_short = expanded("", "WW8LREPEATER", None);
        assert_eq!(expanded_name(&long_short, &model_with(8)), "WW8LREPE");
        // The talkgroup label counts toward the fit test.
        let dmr = expanded("W0QEY Fort Collins", "W0QEY", Some("Colorado"));
        assert_eq!(expanded_name(&dmr, &model_with(16)), "W0QEY Colorado");
    }

    /// Issue #25: 224 MHz sits inside a UV-5R's 136–520 span but the radio has
    /// no 220 band, so the range check alone let it through.
    #[test]
    fn band_flags_exclude_channels_the_range_check_admits() {
        let mut model = model_with(7);
        model.covers_220 = false;
        let ch = Channel {
            rx_freq: 224.840,
            mode: Some("FM".into()),
            ..Default::default()
        };
        let reason = exclusion_reason(&ch, &model).expect("224 MHz must be excluded");
        assert!(reason.contains("220 MHz"), "unexpected reason: {reason}");

        model.covers_220 = true;
        assert!(exclusion_reason(&ch, &model).is_none());
    }

    /// The real UV-5R case: W0QEY on 449.850 and on 147.360 both shipped as
    /// "W0QEY" with no way to tell them apart on the front panel.
    #[test]
    fn duplicate_names_are_split_by_band_then_by_frequency() {
        let mut rows = vec![
            ("W0QEY".to_string(), 449.850),
            ("W0QEY".to_string(), 147.360),
            ("W0LRA".to_string(), 146.940),
        ];
        disambiguate_names(&mut rows, 7);
        assert_eq!(rows[0].0, "W0QEY U");
        assert_eq!(rows[1].0, "W0QEY V");
        assert_eq!(rows[2].0, "W0LRA"); // untouched — it never collided

        // Same band: the band letter cannot split them, so the frequency does.
        let mut same_band = vec![
            ("W0QEY".to_string(), 449.850),
            ("W0QEY".to_string(), 449.575),
        ];
        disambiguate_names(&mut same_band, 8);
        assert_eq!(same_band[0].0, "W0QEY 85");
        assert_eq!(same_band[1].0, "W0QEY 57");
    }

    /// The marker is the digits as they read on the dial, not a rounded value:
    /// 146.625 is "62". Rounding it to "63" would name a frequency the operator
    /// cannot find anywhere on the radio.
    #[test]
    fn frequency_markers_truncate_rather_than_round() {
        assert_eq!(freq_marker(146.625), "62");
        assert_eq!(freq_marker(449.575), "57");
        assert_eq!(freq_marker(449.850), "85");
        assert_eq!(freq_marker(147.360), "36");
        assert_eq!(freq_marker(146.940), "94");
        assert_eq!(freq_marker(224.840), "84");
        assert_eq!(freq_marker(146.600), "60");
    }

    #[test]
    fn markers_displace_characters_rather_than_overrun_the_limit() {
        let mut rows = vec![
            ("W0QEY REPEATER".to_string(), 449.850),
            ("W0QEY REPEATER".to_string(), 147.360),
        ];
        disambiguate_names(&mut rows, 7);
        assert_eq!(rows[0].0, "W0QEY U");
        assert_eq!(rows[1].0, "W0QEY V");
        assert!(rows.iter().all(|(n, _)| n.chars().count() <= 7));
    }

    /// Identical name AND identical frequency (the same repeater reached by two
    /// routes) has nothing left to distinguish it — count them.
    #[test]
    fn unsplittable_duplicates_fall_back_to_a_counter() {
        let mut rows = vec![
            ("W0QEY".to_string(), 449.850),
            ("W0QEY".to_string(), 449.850),
            ("W0QEY".to_string(), 449.850),
        ];
        disambiguate_names(&mut rows, 8);
        assert_eq!(rows[0].0, "W0QEY");
        assert_eq!(rows[1].0, "W0QEY 2");
        assert_eq!(rows[2].0, "W0QEY 3");
    }

    #[test]
    fn disambiguation_never_creates_a_new_collision() {
        // "W0QEY U" is already spoken for, so the pair must route around it.
        let mut rows = vec![
            ("W0QEY U".to_string(), 448.000),
            ("W0QEY".to_string(), 449.850),
            ("W0QEY".to_string(), 449.575),
        ];
        disambiguate_names(&mut rows, 8);
        let names: HashSet<&String> = rows.iter().map(|(n, _)| n).collect();
        assert_eq!(names.len(), 3, "names not unique: {rows:?}");
    }

    #[test]
    fn in_band_and_untracked_frequencies_still_pass() {
        let model = model_with(7);
        for freq in [146.94, 449.850, 380.0] {
            let ch = Channel {
                rx_freq: freq,
                mode: Some("FM".into()),
                ..Default::default()
            };
            assert!(
                exclusion_reason(&ch, &model).is_none(),
                "{freq} should be included"
            );
        }
    }
}
