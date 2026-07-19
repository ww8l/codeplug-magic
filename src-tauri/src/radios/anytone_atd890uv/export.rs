//! AnyTone AT-D890UV codeplug file export: the dual-CSV "bundle"
//! (`<base>_Channels.csv` + `<base>_TalkGroups.csv`) that AnyTone CPS imports.
//! Moved here from commands/export.rs in Chunk 3.5 and wrapped in the driver's
//! CodeplugExporter impl. The generic export helpers (`expanded_name`,
//! `tx_frequency`, `ExpandedChannel`) stay in commands/export.rs.

use std::collections::HashSet;

use crate::commands::export::{expanded_name, tx_frequency, ExpandedChannel};
use crate::error::MapErrString;
use crate::models::RadioModel;
use crate::radios::driver::CodeplugExporter;

use super::AnytoneAtd890uv;

/// "Group Call" / "Private Call" as Anytone CPS spells it.
pub(crate) fn anytone_call_type(call_type: Option<&str>) -> &'static str {
    match call_type {
        Some(t) if t.eq_ignore_ascii_case("Private") => "Private Call",
        _ => "Group Call",
    }
}

/// Anytone contact names are capped at 16 characters.
pub(crate) fn contact_name(label: &str) -> String {
    label.chars().take(16).collect()
}

/// Write the Anytone bundle: `<base>_Channels.csv` + `<base>_TalkGroups.csv`,
/// where `<base>` is the chosen path with any extension stripped.
pub(crate) fn write_anytone_bundle(
    path: &str,
    channels: &[&ExpandedChannel],
    model: &RadioModel,
) -> Result<(), String> {
    let p = std::path::Path::new(path);
    let parent = p.parent().unwrap_or_else(|| std::path::Path::new("."));
    let stem = p
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("codeplug");

    let channels_path = parent.join(format!("{stem}_Channels.csv"));
    let talkgroups_path = parent.join(format!("{stem}_TalkGroups.csv"));

    let channels_csv = render_anytone_channels(channels, model)?;
    let talkgroups_csv = render_anytone_talkgroups(channels)?;

    std::fs::write(&channels_path, channels_csv)
        .map_err(|e| format!("Could not write {}: {e}", channels_path.display()))?;
    std::fs::write(&talkgroups_path, talkgroups_csv)
        .map_err(|e| format!("Could not write {}: {e}", talkgroups_path.display()))?;
    Ok(())
}

/// Render the Anytone CPS Channel CSV. A pragmatic subset of the real Anytone
/// column set — the fields that carry meaningful per-channel programming — using
/// Anytone's own header spellings so CPS recognises them. DMR rows get
/// Channel Type "D-Digital" with Contact/TG/Slot/Color Code; analog rows get
/// "A-Analog" with CTCSS.
pub(crate) fn render_anytone_channels(
    channels: &[&ExpandedChannel],
    model: &RadioModel,
) -> Result<String, String> {
    let mut wtr = csv::Writer::from_writer(vec![]);

    wtr.write_record([
        "No.",
        "Channel Name",
        "Receive Frequency",
        "Transmit Frequency",
        "Channel Type",
        "Transmit Power",
        "Band Width",
        "CTCSS/DCS Decode",
        "CTCSS/DCS Encode",
        "Contact",
        "Contact Call Type",
        "Contact TG/DMR ID",
        "Color Code",
        "Slot",
        "Busy Lock/TX Permit",
        "Reverse",
        "Talk Around(Simplex)",
        "DMR MODE",
    ])
    .estr()?;

    for (i, ec) in channels.iter().enumerate() {
        let c = &ec.channel;
        let is_dmr = c.mode.as_deref() == Some("DMR");

        let channel_type = if is_dmr { "D-Digital" } else { "A-Analog" };
        let band_width = if is_dmr || c.mode.as_deref() == Some("NFM") {
            "12.5K"
        } else {
            "25K"
        };
        // CTCSS only applies to analog channels. dec = RX (decode) tone, enc = TX
        // (encode) tone. "Tone" transmits only; "TSQL" squelches both ways on the
        // ctone (downlink). DTCS/Cross aren't represented in these columns.
        let fmt = |t: Option<f64>| t.map(|v| format!("{v:.1}")).unwrap_or_else(|| "Off".into());
        let (dec, enc) = match c.tone_mode.as_deref() {
            Some(m) if is_dmr => {
                let _ = m;
                ("Off".to_string(), "Off".to_string())
            }
            Some(m) if m.eq_ignore_ascii_case("Tone") => ("Off".to_string(), fmt(c.ctcss_uplink)),
            Some(m) if m.eq_ignore_ascii_case("TSQL") => {
                (fmt(c.ctcss_downlink), fmt(c.ctcss_downlink))
            }
            _ => ("Off".to_string(), "Off".to_string()),
        };

        let (contact, call_type, tg_id) = match ec.tg_number {
            Some(n) => (
                ec.tg_label.as_deref().map(contact_name).unwrap_or_default(),
                anytone_call_type(ec.tg_call_type.as_deref()).to_string(),
                n.to_string(),
            ),
            None => (String::new(), String::new(), String::new()),
        };
        let color_code = if is_dmr {
            c.dmr_color_code.unwrap_or(1).to_string()
        } else {
            String::new()
        };
        let slot = ec.timeslot.map(|t| t.to_string()).unwrap_or_default();

        wtr.write_record([
            (i + 1).to_string(),
            expanded_name(ec, model),
            format!("{:.5}", c.rx_freq),
            format!("{:.5}", tx_frequency(c)),
            channel_type.to_string(),
            "High".to_string(),
            band_width.to_string(),
            dec,
            enc,
            contact,
            call_type,
            tg_id,
            color_code,
            slot,
            "Always".to_string(),
            "Off".to_string(),
            "Off".to_string(),
            if is_dmr { "1" } else { "0" }.to_string(),
        ])
        .estr()?;
    }

    let bytes = wtr.into_inner().map_err(|e| e.to_string())?;
    String::from_utf8(bytes).map_err(|e| e.to_string())
}

/// Render the Anytone Digital Contacts (TalkGroups) CSV: the distinct
/// talkgroups referenced by the DMR channels, keyed by (TG id, contact name) so
/// the channel's Contact column links to a real contact. Ordered by first
/// appearance to keep diffs stable.
pub(crate) fn render_anytone_talkgroups(channels: &[&ExpandedChannel]) -> Result<String, String> {
    let mut wtr = csv::Writer::from_writer(vec![]);
    wtr.write_record(["No.", "Radio ID", "Name", "Call Type", "Call Alert"])
        .estr()?;

    let mut seen = HashSet::new();
    let mut no = 0usize;
    for ec in channels {
        let (Some(tg_num), Some(label)) = (ec.tg_number, ec.tg_label.as_deref()) else {
            continue;
        };
        let name = contact_name(label);
        if !seen.insert((tg_num, name.clone())) {
            continue;
        }
        no += 1;
        wtr.write_record([
            no.to_string(),
            tg_num.to_string(),
            name,
            anytone_call_type(ec.tg_call_type.as_deref()).to_string(),
            "None".to_string(),
        ])
        .estr()?;
    }

    let bytes = wtr.into_inner().map_err(|e| e.to_string())?;
    String::from_utf8(bytes).map_err(|e| e.to_string())
}

impl CodeplugExporter for AnytoneAtd890uv {
    fn export_format(&self) -> &'static str {
        "anytone_csv"
    }

    /// Write the AnyTone dual-CSV bundle. Adapts the trait's owned-slice input to
    /// the reference-slice the renderers consume, then delegates to
    /// [`write_anytone_bundle`]. Returns the channel count written.
    fn export(
        &self,
        path: &str,
        channels: &[ExpandedChannel],
        model: &RadioModel,
    ) -> Result<usize, String> {
        let refs: Vec<&ExpandedChannel> = channels.iter().collect();
        write_anytone_bundle(path, &refs, model)?;
        Ok(refs.len())
    }
}
