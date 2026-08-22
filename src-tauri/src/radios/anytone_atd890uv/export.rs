//! AnyTone AT-D890UV codeplug file export: the dual-CSV "bundle"
//! (`<base>_Channels.csv` + `<base>_TalkGroups.csv`) that AnyTone CPS imports.
//! Moved here from commands/export.rs in Chunk 3.5 and wrapped in the driver's
//! CodeplugExporter impl. The generic export helpers (`expanded_name`,
//! `tx_frequency`, `ExpandedChannel`) stay in commands/export.rs.

use std::collections::HashSet;

use crate::commands::export::{expanded_names, tx_frequency, ExpandedChannel};
use crate::error::MapErrString;
use crate::models::RadioModel;
use crate::radios::driver::{CodeplugExporter, ExportRequest};

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

/// `Transmit Power`, the CPS spellings of the radio's four levels.
///
/// Mirrors the cable path's `program::invert_power`, which is the encoding
/// proven on the radio: NULL means "the radio's own maximum" and round-trips as
/// Turbo. This column used to be hard-coded "High", so every FRS/GMRS/MURS
/// channel authored LOW per Part 95 loaded into the CPS at High, and any level
/// the operator set was silently promoted (#82).
/// The level names are qdmr's for the same enum (`Low` / `Medium` / `High` /
/// `Turbo`, d890uv v1.05); the CPS column spelling is not confirmed against an
/// export either — see [`tone_side`].
pub(crate) fn csv_power(power: Option<&str>) -> &'static str {
    match power {
        None => "Turbo",
        Some(p) if p.eq_ignore_ascii_case("Low") => "Low",
        Some(p) if p.eq_ignore_ascii_case("Med") || p.eq_ignore_ascii_case("Medium") => "Medium",
        // An unrecognised label is the same fallback the cable path takes.
        Some(_) => "High",
    }
}

/// One side of the `CTCSS/DCS Decode` / `Encode` pair: a CTCSS frequency, a DCS
/// code in the CPS's `D023N` notation, or `Off`.
///
/// `polarity` is one character of the stored CHIRP two-letter form (`NN`,
/// `RN`, ...) — `R` (reversed) is the CPS's `I` (inverted).
///
/// **The `D023N` spelling is not confirmed against a CPS export** — no capture
/// here carries a DCS channel, and the AnyTone CSV column set is not published.
/// It is the notation CHIRP and qdmr both use for this radio family. If it is
/// wrong the CPS import rejects the field visibly, which is still better than
/// the `Off` this column used to write: that programmed a channel that silently
/// could not key its repeater.
fn tone_side(ctcss: Option<f64>, dcs: Option<&str>, polarity: Option<char>) -> String {
    if let Some(hz) = ctcss {
        return format!("{hz:.1}");
    }
    match dcs {
        Some(code) => {
            let inverted = matches!(polarity, Some('R') | Some('r') | Some('I') | Some('i'));
            format!("D{}{}", code.trim(), if inverted { "I" } else { "N" })
        }
        None => "Off".to_string(),
    }
}

/// The `CTCSS/DCS Decode` (RX) and `Encode` (TX) columns.
///
/// The two used to come from a match whose last arm was `("Off", "Off")`, so
/// `DTCS` and `Cross` both landed there: download a D890, import it, then use
/// **Generate codeplug** instead of **Program radio** and every DCS channel came
/// back with no tone at all — while the same channel over the cable was correct
/// (#82). Each side is read independently, exactly as the cable path's
/// `program::invert_tone` does, so a cross-tone channel keeps both halves.
fn csv_tones(c: &crate::models::Channel) -> (String, String) {
    let pol = c.dcs_polarity.as_bytes();
    let tx_pol = pol.first().map(|b| *b as char);
    let rx_pol = pol.get(1).map(|b| *b as char);
    let dcs_tx = c.dcs_code.as_deref();
    let dcs_rx = c.dcs_rx_code.as_deref();

    match c.tone_mode.as_deref() {
        Some(m) if m.eq_ignore_ascii_case("Tone") => (
            "Off".to_string(),
            tone_side(c.ctcss_uplink, None, None),
        ),
        // Tone squelch keys on the downlink tone both ways.
        Some(m) if m.eq_ignore_ascii_case("TSQL") => {
            let hz = c.ctcss_downlink.or(c.ctcss_uplink);
            let side = tone_side(hz, None, None);
            (side.clone(), side)
        }
        Some(m) if m.eq_ignore_ascii_case("DTCS") => (
            tone_side(None, dcs_rx.or(dcs_tx), rx_pol),
            tone_side(None, dcs_tx, tx_pol),
        ),
        // Each side carries exactly one of CTCSS/DCS in the stored columns, so
        // the sides render independently of the cross_mode label.
        Some(m) if m.eq_ignore_ascii_case("Cross") => (
            tone_side(c.ctcss_downlink, dcs_rx, rx_pol),
            tone_side(c.ctcss_uplink, dcs_tx, tx_pol),
        ),
        _ => ("Off".to_string(), "Off".to_string()),
    }
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

    let names = expanded_names(channels.iter().copied(), model);
    for (i, ec) in channels.iter().enumerate() {
        let c = &ec.channel;
        let is_dmr = c.mode.as_deref() == Some("DMR");

        let channel_type = if is_dmr { "D-Digital" } else { "A-Analog" };
        let band_width = if is_dmr || c.mode.as_deref() == Some("NFM") {
            "12.5K"
        } else {
            "25K"
        };
        let (dec, enc) = if is_dmr {
            ("Off".to_string(), "Off".to_string())
        } else {
            csv_tones(c)
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
            names[i].clone(),
            format!("{:.5}", c.rx_freq),
            format!("{:.5}", tx_frequency(c)),
            channel_type.to_string(),
            csv_power(c.power.as_deref()).to_string(),
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

    /// Write the AnyTone dual-CSV bundle, returning the channel count written.
    /// The CPS CSV import has no column for zone membership or radio settings,
    /// so `groups` and `profile_settings` go unused here — both reach a D890
    /// through the direct programmer instead.
    fn export(&self, path: &str, req: &ExportRequest) -> Result<usize, String> {
        write_anytone_bundle(path, req.channels, req.model)?;
        Ok(req.channels.len())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::Channel;

    fn ec(c: Channel) -> ExpandedChannel {
        ExpandedChannel {
            channel: c,
            tg_label: None,
            timeslot: None,
            tg_number: None,
            tg_call_type: None,
            tg_inline: false,
        }
    }

    fn model() -> RadioModel {
        RadioModel {
            display_name: "AT-D890UV".into(),
            max_name_length: Some(16),
            ..Default::default()
        }
    }

    /// The tone columns of an `A-Analog` row: `<RX>,<TX>` out of the rendered
    /// line (columns 8 and 9, 1-indexed).
    fn tones(line: &str) -> (String, String) {
        let f: Vec<&str> = line.split(',').collect();
        (f[7].to_string(), f[8].to_string())
    }

    fn power(line: &str) -> String {
        line.split(',').nth(5).unwrap().to_string()
    }

    /// Issue #82: `DTCS` and `Cross` used to fall through to `("Off", "Off")`,
    /// so a DCS repeater downloaded from a D890 and re-exported through
    /// **Generate codeplug** came back with no tone at all — while the same
    /// channel over the cable was correct.
    #[test]
    fn dcs_and_cross_tones_reach_the_csv() {
        let dtcs = ec(Channel {
            name_long: Some("DCS".into()),
            rx_freq: 146.94,
            tx_freq: Some(146.34),
            mode: Some("FM".into()),
            tone_mode: Some("DTCS".into()),
            dcs_code: Some("023".into()),
            dcs_rx_code: Some("023".into()),
            dcs_polarity: "NN".into(),
            ..Default::default()
        });
        // Reversed RX polarity is the CPS's "I".
        let dtcs_rev = ec(Channel {
            name_long: Some("DCSREV".into()),
            rx_freq: 146.94,
            mode: Some("FM".into()),
            tone_mode: Some("DTCS".into()),
            dcs_code: Some("754".into()),
            dcs_rx_code: Some("754".into()),
            dcs_polarity: "NR".into(),
            ..Default::default()
        });
        // Cross: CTCSS out, DCS in — both halves have to survive.
        let cross = ec(Channel {
            name_long: Some("CROSS".into()),
            rx_freq: 449.0,
            tx_freq: Some(444.0),
            mode: Some("FM".into()),
            tone_mode: Some("Cross".into()),
            cross_mode: "Tone->DTCS".into(),
            ctcss_uplink: Some(100.0),
            dcs_rx_code: Some("131".into()),
            dcs_polarity: "NN".into(),
            ..Default::default()
        });
        // The cross shape the RepeaterBook importer produces from uplink != downlink.
        let cross_tones = ec(Channel {
            name_long: Some("XTONE".into()),
            rx_freq: 147.0,
            mode: Some("FM".into()),
            tone_mode: Some("Cross".into()),
            cross_mode: "Tone->Tone".into(),
            ctcss_uplink: Some(88.5),
            ctcss_downlink: Some(123.0),
            ..Default::default()
        });

        let all = [dtcs, dtcs_rev, cross, cross_tones];
        let refs: Vec<&ExpandedChannel> = all.iter().collect();
        let csv = render_anytone_channels(&refs, &model()).unwrap();
        let lines: Vec<&str> = csv.lines().skip(1).collect();

        // Decode = RX, Encode = TX.
        assert_eq!(tones(lines[0]), ("D023N".to_string(), "D023N".to_string()));
        assert_eq!(tones(lines[1]), ("D754I".to_string(), "D754N".to_string()));
        assert_eq!(tones(lines[2]), ("D131N".to_string(), "100.0".to_string()));
        assert_eq!(tones(lines[3]), ("123.0".to_string(), "88.5".to_string()));
    }

    /// The `Transmit Power` column used to be the literal "High" on every row,
    /// so the 22 FRS channels — authored LOW per Part 95 — all loaded into the
    /// CPS at High, and any level the operator chose was silently promoted.
    #[test]
    fn transmit_power_comes_from_the_channel() {
        let lvl = |p: Option<&str>| {
            ec(Channel {
                name_long: Some("CH".into()),
                rx_freq: 146.94,
                mode: Some("FM".into()),
                power: p.map(Into::into),
                ..Default::default()
            })
        };
        let all = [lvl(Some("Low")), lvl(Some("Med")), lvl(Some("High")), lvl(None)];
        let refs: Vec<&ExpandedChannel> = all.iter().collect();
        let csv = render_anytone_channels(&refs, &model()).unwrap();
        let lines: Vec<&str> = csv.lines().skip(1).collect();

        assert_eq!(power(lines[0]), "Low");
        assert_eq!(power(lines[1]), "Medium");
        assert_eq!(power(lines[2]), "High");
        // NULL = "the radio's own maximum", the same round-trip the cable path
        // uses (program::invert_power).
        assert_eq!(power(lines[3]), "Turbo");
    }

    /// Every shape the channel editor's cross-mode list can produce has to put
    /// something in the tone columns — `Off,Off` is a channel that cannot key.
    #[test]
    fn no_cross_mode_renders_as_no_tone_at_all() {
        for cross in [
            "Tone->Tone", "Tone->DTCS", "DTCS->Tone", "DTCS->DTCS", "->Tone", "->DTCS",
            "DTCS->", "Tone->",
        ] {
            let c = ec(Channel {
                name_long: Some("X".into()),
                rx_freq: 146.94,
                mode: Some("FM".into()),
                tone_mode: Some("Cross".into()),
                cross_mode: cross.into(),
                ctcss_uplink: cross.starts_with("Tone").then_some(100.0),
                ctcss_downlink: cross.ends_with("Tone").then_some(107.2),
                dcs_code: cross.starts_with("DTCS").then(|| "023".to_string()),
                dcs_rx_code: cross.ends_with("DTCS").then(|| "131".to_string()),
                dcs_polarity: "NN".into(),
                ..Default::default()
            });
            let refs = vec![&c];
            let csv = render_anytone_channels(&refs, &model()).unwrap();
            let line = csv.lines().nth(1).unwrap();
            let (dec, enc) = tones(line);
            assert!(
                dec != "Off" || enc != "Off",
                "{cross} renders as no tone at all"
            );
        }
    }

    /// A DMR row has no analog tone, whatever the channel carries.
    #[test]
    fn a_dmr_row_still_writes_no_tone() {
        let dmr = ec(Channel {
            name_long: Some("DMR".into()),
            rx_freq: 449.0,
            mode: Some("DMR".into()),
            tone_mode: Some("DTCS".into()),
            dcs_code: Some("023".into()),
            dcs_polarity: "NN".into(),
            ..Default::default()
        });
        let refs = vec![&dmr];
        let csv = render_anytone_channels(&refs, &model()).unwrap();
        let line = csv.lines().nth(1).unwrap();
        assert_eq!(tones(line), ("Off".to_string(), "Off".to_string()));
    }
}
