//! The ID-52's Memory CH CSV — the channel half of the card programming path.
//!
//! The operator drops this file in `ID-52/Csv/MemoryCh/` and imports it from
//! **SET > SD Card > Import/Export > Import > Memory CH**, then restarts the
//! radio. Unlike the settings `.icf`, this is a *generated* file rather than a
//! patched one: it is a plain list of memories with nothing else riding along,
//! so there is nothing of the operator's to preserve inside it.
//!
//! That separation is the whole reason the ID-52 can do what issue #38 asks —
//! channels and settings written independently — and it comes from the radio's
//! own menu structure rather than anything invented here.
//!
//! ## Groups are the radio's banks, with one difference that matters
//!
//! A channel list becomes a memory group, in codeplug order. But an Icom memory
//! **belongs to exactly one group** — the group number is a column of the
//! memory itself, not a separate membership table — where the FT5D's banks are
//! an overlay that can name the same channel from several banks at once. So a
//! channel that appears in two of the codeplug's lists lands in the first one,
//! and [`assign_groups`] is where that happens.
//!
//! Capacities, from the Advanced Manual p. 9-2: **1000 memories** spread across
//! **100 groups** (`00`–`99`), each holding at most 100 (`CH No` is `00`–`99`).
//! Both limits bind independently.
//!
//! ## Provenance, and the one column still in dispute
//!
//! The column set below is agreed by two independent sources that target this
//! radio: `mdavey/wia-to-52a` (a WIA repeater-directory converter written
//! against an ID-52A) and the Austrian repeater channel sets published for the
//! IC-705/ID-52 family. A third, `memory-channels-processor`, emits the same
//! columns **plus a `SEL` scan-select column after `Name`** for its combined
//! IC-705/IC-9700/ID-52 target.
//!
//! ## Why half of this module is `pub(super)`
//!
//! [`super::memory`] writes the same memories into the `.icf` image, and the two
//! writers must not disagree about what a channel *is* — which mode it lands in,
//! which way its shift goes, which tone column the radio will actually read. So
//! every one of those decisions is made exactly once, here, and each writer only
//! renders the answer: this one as CSV text, the image writer as bytes. That is
//! also what makes the CSV a usable cross-check on the image — the same codeplug
//! through both paths should describe the same radio.
//!
//! Which is right for *this* radio is not settled from a desk, so the header is
//! a single constant here and `scratchpad/id52/CAPTURE-CHECKLIST.md` step 1 asks
//! for a real export off the radio to decide it. Value spellings are the
//! exception: those come from the Advanced Manual, which names the tone types
//! (`TONE`, `TSQL`, `DTCS`, `TONE(T)/TSQL(R)`, …) and the DTCS polarities
//! (`Both N`, `TN-RR`, `TR-RN`, `Both R`) outright.

use std::collections::HashMap;

use crate::commands::export::{expanded_names, CodeplugGroup, ExpandedChannel};
use crate::models::RadioModel;
use crate::radios::driver::{CodeplugExporter, ExportRequest};

use super::IcomId52;

/// Memory capacity, and how it is divided up (Advanced Manual p. 9-2).
pub(super) const MAX_MEMORIES: usize = 1000;
pub(super) const MAX_GROUPS: usize = 100;
pub(super) const MAX_PER_GROUP: usize = 100;

/// Group and memory names are 16 characters (Advanced Manual p. iv).
pub(super) const MAX_NAME: usize = 16;

/// Where a channel goes when the codeplug has no channel lists to make groups
/// out of. The radio always shows *some* group, so an unnamed one would just
/// read as blank on the front panel.
const DEFAULT_GROUP_NAME: &str = "Memories";

/// The header row. One constant, because it is the part of this format most
/// likely to be corrected by a real export off the radio (see the module docs).
const HEADER: [&str; 20] = [
    "Group No",
    "Group Name",
    "CH No",
    "Name",
    "Frequency",
    "Dup",
    "Offset",
    "TS",
    "Mode",
    "SKIP",
    "TONE",
    "Repeater Tone",
    "TSQL Frequency",
    "DTCS Code",
    "DTCS Polarity",
    "DV SQL",
    "DV CSQL Code",
    "Your Call Sign",
    "RPT1 Call Sign",
    "RPT2 Call Sign",
];

/// Tuning step. The channel database has no per-channel step, and the step only
/// affects what happens when you dial *off* a memory — the stored frequency is
/// exact regardless.
///
/// These are the two values the radio writes for itself: `5kHz` for everything,
/// and `8.33kHz` for airband AM, which is that band's real channel spacing.
/// (Both third-party ID-52 converters write `12.5kHz` instead; the radio's own
/// export says otherwise, and the radio wins.)
pub(super) fn tune_step(ec: &ExpandedChannel) -> &'static str {
    let f = ec.channel.rx_freq;
    if mode_of(ec) == "AM" && (108.0..137.0).contains(&f) {
        "8.33kHz"
    } else {
        "5kHz"
    }
}

/// Fallback tone frequency. The radio wants both tone columns populated on
/// every row whatever the TONE column says, and 88.5 Hz is its own default.
const DEFAULT_TONE_HZ: f64 = 88.5;
const DEFAULT_DTCS: &str = "023";

/// One memory row's group placement: which group it landed in, and its number
/// within that group.
pub(super) struct Placement {
    pub(super) group_no: usize,
    pub(super) ch_no: usize,
}

/// Decide which group each memory belongs to and number it within that group.
///
/// Channels arrive in memory order and groups in codeplug order. A channel
/// named by several lists takes the **first** — an Icom memory carries exactly
/// one group number, so there is no way to honour more than one, and silently
/// duplicating the memory into a second group would consume capacity the
/// operator never asked to spend.
///
/// Channels in no list at all collect in one trailing group, so a codeplug with
/// no channel lists still exports as a usable single group rather than nothing.
pub(super) fn assign_groups(
    channels: &[&ExpandedChannel],
    groups: &[CodeplugGroup],
) -> Result<(Vec<Placement>, Vec<String>), String> {
    // Channel id -> the first list that names it.
    let mut first_list: HashMap<i64, usize> = HashMap::new();
    for (gi, g) in groups.iter().enumerate() {
        for c in &g.channels {
            first_list.entry(c.id).or_insert(gi);
        }
    }

    // Only lists that actually received a memory become groups, so an empty or
    // wholly-excluded list does not burn a group number (or leave a group the
    // operator scrolls past on the radio for nothing).
    let mut group_names: Vec<String> = Vec::new();
    let mut group_no_of_list: HashMap<usize, usize> = HashMap::new();
    let mut ungrouped: Option<usize> = None;
    let mut counts: Vec<usize> = Vec::new();
    let mut placements = Vec::with_capacity(channels.len());

    for ec in channels {
        let group_no = match first_list.get(&ec.channel.id) {
            Some(&li) => *group_no_of_list.entry(li).or_insert_with(|| {
                group_names.push(truncate(&groups[li].list_name, MAX_NAME));
                counts.push(0);
                group_names.len() - 1
            }),
            None => *ungrouped.get_or_insert_with(|| {
                group_names.push(DEFAULT_GROUP_NAME.to_string());
                counts.push(0);
                group_names.len() - 1
            }),
        };
        let ch_no = counts[group_no];
        counts[group_no] += 1;
        placements.push(Placement { group_no, ch_no });
    }

    if group_names.len() > MAX_GROUPS {
        return Err(format!(
            "This codeplug fills {} memory groups; the ID-52 has {MAX_GROUPS}. \
             Combine or unassign {} channel lists before exporting.",
            group_names.len(),
            group_names.len() - MAX_GROUPS
        ));
    }
    if let Some(over) = counts.iter().position(|&n| n > MAX_PER_GROUP) {
        return Err(format!(
            "Channel list \"{}\" holds {} channels; an ID-52 memory group holds \
             {MAX_PER_GROUP}.",
            group_names[over], counts[over]
        ));
    }

    Ok((placements, group_names))
}

/// Render the whole file. Pure, so the column mapping is testable without a
/// filesystem or a radio.
pub(crate) fn render_csv(
    channels: &[&ExpandedChannel],
    groups: &[CodeplugGroup],
    model: &RadioModel,
) -> Result<String, String> {
    if channels.len() > MAX_MEMORIES {
        return Err(format!(
            "This codeplug has {} channels; the ID-52 holds {MAX_MEMORIES}. \
             Remove {} before exporting.",
            channels.len(),
            channels.len() - MAX_MEMORIES
        ));
    }

    let (placements, group_names) = assign_groups(channels, groups)?;
    let names = expanded_names(channels.iter().copied(), model);

    let mut wtr = csv::Writer::from_writer(vec![]);
    wtr.write_record(HEADER)
        .map_err(|e| format!("Could not write the CSV header: {e}"))?;

    for (i, ec) in channels.iter().enumerate() {
        let c = &ec.channel;
        let p = &placements[i];
        let (dup, offset) = duplex_and_offset(ec);
        let mode = mode_of(ec);
        // Which family of columns this row fills. The radio's own export is
        // strict about this: an FM row leaves every digital column empty, a DV
        // row leaves every tone column empty, and an AM row — which has no
        // squelch of either kind — leaves both sets empty.
        let dv = mode == "DV";
        let analog = matches!(mode, "FM" | "FM-N");
        let (tone, rpt_tone, tsql_tone) = tone_columns(ec);
        let (your, rpt1, rpt2) = call_signs(ec, dv);
        let blank = String::new();

        wtr.write_record([
            format!("{:02}", p.group_no),
            group_names[p.group_no].clone(),
            format!("{:02}", p.ch_no),
            names[i].clone(),
            format!("{:.6}", c.rx_freq),
            dup.to_string(),
            format!("{offset:.6}"),
            tune_step(ec).to_string(),
            mode.to_string(),
            // The channel database has no per-channel scan skip, so nothing is
            // skipped. Same gap the FT5D writer has, and for the same reason.
            "OFF".to_string(),
            if analog { tone.into() } else { blank.clone() },
            if analog {
                format!("{rpt_tone:.1}Hz")
            } else {
                blank.clone()
            },
            if analog {
                format!("{tsql_tone:.1}Hz")
            } else {
                blank.clone()
            },
            if analog {
                c.dcs_code.clone().unwrap_or_else(|| DEFAULT_DTCS.into())
            } else {
                blank.clone()
            },
            if analog {
                dtcs_polarity(&c.dcs_polarity).into()
            } else {
                blank.clone()
            },
            if dv { "OFF".into() } else { blank.clone() },
            if dv { "0".into() } else { blank },
            your,
            rpt1,
            rpt2,
        ])
        .map_err(|e| format!("Could not write channel \"{}\": {e}", names[i]))?;
    }

    let bytes = wtr
        .into_inner()
        .map_err(|e| format!("Could not finish the CSV: {e}"))?;
    String::from_utf8(bytes).map_err(|e| format!("The CSV is not valid UTF-8: {e}"))
}

/// `Dup` and `Offset`, always as a positive offset plus a direction.
///
/// Derived from the two frequencies rather than read off `duplex`, so an odd
/// split — which this radio has no separate encoding for — comes out as a shift
/// of the real difference. That is how the radio behaves anyway, and it is what
/// the FT5D writer does with the same case.
pub(super) fn duplex_and_offset(ec: &ExpandedChannel) -> (&'static str, f64) {
    let c = &ec.channel;
    let tx = match c.tx_freq {
        Some(tx) if (tx - c.rx_freq).abs() > 1e-9 => tx,
        // No transmit frequency of its own: fall back to the stored shift, which
        // is what a channel edited as "− 0.600" carries.
        _ => match (c.duplex.as_deref(), c.offset) {
            (Some("-"), Some(off)) if off.abs() > 1e-9 => c.rx_freq - off.abs(),
            (Some("+"), Some(off)) if off.abs() > 1e-9 => c.rx_freq + off.abs(),
            // Simplex. The radio still stores an offset it is not using, and
            // fills it with the band's standard shift — so a memory the operator
            // later flips to DUP on the front panel lands somewhere sensible
            // instead of on top of itself. Matching that keeps a file we wrote
            // indistinguishable from one the radio exported.
            //
            // Airband is the exception: there is no repeater shift to fall back
            // on, and the radio writes a flat zero for its own airband memories.
            // Measured — an AM memory that carried 0.600 would be claiming a 2 m
            // convention on a band that has none.
            _ => {
                let standard = if tune_step(ec) == "8.33kHz" {
                    0.0
                } else {
                    crate::util::standard_offsets(c.rx_freq)
                        .first()
                        .copied()
                        .unwrap_or(0.0)
                };
                return ("OFF", standard);
            }
        },
    };
    let diff = tx - c.rx_freq;
    if diff < 0.0 {
        ("DUP-", -diff)
    } else {
        ("DUP+", diff)
    }
}

fn is_dv(ec: &ExpandedChannel) -> bool {
    ec.channel.mode.as_deref().is_some_and(|m| {
        m.eq_ignore_ascii_case("dstar") || m.eq_ignore_ascii_case("d-star") || m.eq_ignore_ascii_case("dv")
    })
}

/// `Mode`. The ID-52 spells narrow FM `FM-N`; everything it cannot do is
/// programmed as plain FM, which is what a receive-only memory needs anyway.
pub(super) fn mode_of(ec: &ExpandedChannel) -> &'static str {
    if is_dv(ec) {
        return "DV";
    }
    match ec.channel.mode.as_deref().unwrap_or("FM").to_uppercase().as_str() {
        "NFM" | "FM-N" | "FMN" => "FM-N",
        "AM" => "AM",
        "WFM" => "WFM",
        _ => "FM",
    }
}

/// `TONE`, `Repeater Tone` and `TSQL Frequency` together, because on this radio
/// they are one decision: the TONE column says which of the two frequency
/// columns the radio actually uses, and both are written on every row.
///
/// The ID-52 has real cross-tone modes (Advanced Manual pp. 15-8/15-9), so a
/// repeater with different uplink and downlink tones is programmed exactly as
/// stored — no need for the "keep the TX tone, drop the RX tone" fallback the
/// radios without cross modes get.
pub(super) fn tone_columns(ec: &ExpandedChannel) -> (&'static str, f64, f64) {
    let c = &ec.channel;
    let up = c.ctcss_uplink;
    let down = c.ctcss_downlink;
    let tone = match c.tone_mode.as_deref() {
        Some(m) if m.eq_ignore_ascii_case("tone") => "TONE",
        Some(m) if m.eq_ignore_ascii_case("tsql") => "TSQL",
        Some(m) if m.eq_ignore_ascii_case("dtcs") => "DTCS",
        Some(m) if m.eq_ignore_ascii_case("cross") => match c.cross_mode.as_str() {
            "Tone->DTCS" => "TONE(T)/DTCS(R)",
            "DTCS->Tone" => "DTCS(T)/TSQL(R)",
            // The only cross mode the importer actually produces: a TX tone and
            // a different RX tone squelch.
            _ => "TONE(T)/TSQL(R)",
        },
        _ => "OFF",
    };
    // Which of the two frequency columns the radio reads depends on the TONE
    // column, and the other one is a leftover it ignores — the real export is
    // full of rows carrying an unrelated 88.5 Hz next to a live tone. So the
    // live one is placed deliberately and the spare gets the radio's default:
    // `TONE` transmits the Repeater Tone, `TSQL` uses the TSQL Frequency both
    // ways, and cross mode is the only one that reads both.
    let (rpt, tsql) = match tone {
        "TONE" => (up, down),
        "TSQL" => {
            let t = down.or(up);
            (t, t)
        }
        // DTCS carries its code in its own column; neither tone is read.
        "DTCS" => (None, None),
        _ => (up, down),
    };
    (
        tone,
        rpt.unwrap_or(DEFAULT_TONE_HZ),
        tsql.unwrap_or(DEFAULT_TONE_HZ),
    )
}

/// `DTCS Polarity`. The database stores CHIRP's two-letter form.
///
/// The manual writes these as "Both N" but the CSV the radio exports uses
/// `BOTH N` — the file's spelling is the one that has to match.
pub(super) fn dtcs_polarity(stored: &str) -> &'static str {
    match stored.to_uppercase().as_str() {
        "NR" => "TN-RR",
        "RN" => "TR-RN",
        "RR" => "BOTH R",
        _ => "BOTH N",
    }
}

/// The three D-STAR call sign columns, blank on an analog memory.
///
/// Icom packs a call sign into 8 characters with the **port letter last**:
/// `OE1XDS C`. The port is the repeater's band — `C` on 2 m, `B` on 70 cm, `A`
/// on 23 cm — and the gateway is the same call with `G`. `CQCQCQ` in `Your` is
/// the ordinary "call anyone" destination.
pub(super) fn call_signs(ec: &ExpandedChannel, dv: bool) -> (String, String, String) {
    if !dv {
        return (String::new(), String::new(), String::new());
    }
    let Some(call) = ec
        .channel
        .callsign
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    else {
        // A DV memory with no repeater call sign is a simplex DV channel: the
        // radio wants a destination but no repeaters.
        return ("CQCQCQ".into(), String::new(), String::new());
    };
    let port = match ec.channel.rx_freq {
        f if f >= 1200.0 => 'A',
        f if f >= 400.0 => 'B',
        _ => 'C',
    };
    let call: String = call.chars().take(7).collect();
    (
        "CQCQCQ".into(),
        format!("{call:<7}{port}"),
        format!("{call:<7}G"),
    )
}

pub(super) fn truncate(s: &str, max: usize) -> String {
    s.chars().take(max).collect()
}

impl CodeplugExporter for IcomId52 {
    fn export_format(&self) -> &'static str {
        "icom_id52_csv"
    }

    /// `path` is a **new** file, unlike the FT5D's in-place patch: the Memory CH
    /// CSV holds nothing but memories, so there is no operator content to
    /// preserve. It belongs in `ID-52/Csv/MemoryCh/` on the card, under a name
    /// of 23 characters or less — longer names are invisible to the radio's own
    /// file picker (Advanced Manual p. 2-7).
    fn export(&self, path: &str, req: &ExportRequest) -> Result<usize, String> {
        let csv = render_csv(req.channels, req.groups, req.model)?;
        std::fs::write(path, csv).map_err(|e| format!("Could not write {path}: {e}"))?;
        Ok(req.channels.len())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::Channel;

    fn model() -> RadioModel {
        RadioModel {
            display_name: "Icom ID-52".into(),
            max_name_length: Some(MAX_NAME as i64),
            ..Default::default()
        }
    }

    fn chan(id: i64, name: &str, rx: f64) -> Channel {
        Channel {
            id,
            name_short: Some(name.into()),
            rx_freq: rx,
            dcs_polarity: "NN".into(),
            cross_mode: "Tone->Tone".into(),
            mode: Some("FM".into()),
            ..Default::default()
        }
    }

    fn expand(c: Channel) -> ExpandedChannel {
        ExpandedChannel {
            channel: c,
            tg_label: None,
            timeslot: None,
            tg_number: None,
            tg_call_type: None,
            tg_inline: false,
        }
    }

    /// Split a rendered row back into fields, for readable assertions.
    fn rows(csv: &str) -> Vec<Vec<String>> {
        csv.lines()
            .skip(1)
            .map(|l| {
                csv::ReaderBuilder::new()
                    .has_headers(false)
                    .from_reader(l.as_bytes())
                    .records()
                    .next()
                    .unwrap()
                    .unwrap()
                    .iter()
                    .map(str::to_string)
                    .collect()
            })
            .collect()
    }

    const GROUP_NO: usize = 0;
    const GROUP_NAME: usize = 1;
    const CH_NO: usize = 2;
    const NAME: usize = 3;
    const FREQ: usize = 4;
    const DUP: usize = 5;
    const OFFSET: usize = 6;
    const TS: usize = 7;
    const MODE: usize = 8;
    const TONE: usize = 10;
    const RPT_TONE: usize = 11;
    const TSQL_TONE: usize = 12;
    const DTCS: usize = 13;
    const DTCS_POL: usize = 14;
    const DV_SQL: usize = 15;
    const DV_CSQL: usize = 16;
    const YOUR: usize = 17;
    const RPT1: usize = 18;
    const RPT2: usize = 19;

    /// The header is what the radio's importer matches on, so it is pinned
    /// exactly — including column order. CONFIRMED against a real Memory CH
    /// export off Tim's ID-52: 20 columns, and no `SEL` (that column belongs to
    /// a third-party tool's combined IC-705/ID-52 target, not to this radio).
    #[test]
    fn the_header_is_the_radios_own_twenty_columns() {
        let csv = render_csv(&[], &[], &model()).unwrap();
        assert_eq!(
            csv.lines().next().unwrap(),
            "Group No,Group Name,CH No,Name,Frequency,Dup,Offset,TS,Mode,SKIP,TONE,\
             Repeater Tone,TSQL Frequency,DTCS Code,DTCS Polarity,DV SQL,DV CSQL Code,\
             Your Call Sign,RPT1 Call Sign,RPT2 Call Sign"
        );
    }

    /// A repeater is the ordinary case: output frequency, a minus shift, and a
    /// transmit tone. Offset is positive with the direction in its own column.
    #[test]
    fn a_repeater_becomes_a_shifted_memory_with_a_tone() {
        let mut c = chan(1, "W0ABC", 146.940);
        c.tx_freq = Some(146.340);
        c.tone_mode = Some("Tone".into());
        c.ctcss_uplink = Some(100.0);
        let ec = expand(c);

        let csv = render_csv(&[&ec], &[], &model()).unwrap();
        let r = &rows(&csv)[0];

        assert_eq!(r[FREQ], "146.940000");
        assert_eq!(r[DUP], "DUP-");
        assert_eq!(r[OFFSET], "0.600000");
        assert_eq!(r[TONE], "TONE");
        assert_eq!(r[RPT_TONE], "100.0Hz");
        // TONE transmits the Repeater Tone and never reads this column, so it
        // gets the radio's default rather than an echo of the TX tone — which
        // is exactly what the radio's own export does.
        assert_eq!(r[TSQL_TONE], "88.5Hz");
        assert_eq!(r[MODE], "FM");
        assert_eq!(r[TS], "5kHz");
        assert_eq!(r[DTCS_POL], "BOTH N");
    }

    /// Tone squelch is the same tone both ways, and the radio reads it from the
    /// TSQL column — so a channel that only carries an uplink tone must still
    /// land there, not just in Repeater Tone.
    #[test]
    fn tone_squelch_fills_the_column_the_radio_reads() {
        let mut c = chan(1, "W0UPS", 145.115);
        c.tone_mode = Some("TSQL".into());
        c.ctcss_uplink = Some(100.0);
        let ec = expand(c);

        let r = &rows(&render_csv(&[&ec], &[], &model()).unwrap())[0];
        assert_eq!(r[TONE], "TSQL");
        assert_eq!(r[TSQL_TONE], "100.0Hz");
        assert_eq!(r[RPT_TONE], "100.0Hz");
    }

    /// Simplex: no shift. The offset column still carries the band's standard
    /// shift, because that is what the radio stores for its own simplex
    /// memories — an unused value, but one that makes sense if the operator
    /// later turns duplex on from the front panel.
    #[test]
    fn a_simplex_channel_has_no_shift_and_no_tone() {
        let ec = expand(chan(1, "Calling", 146.520));
        let csv = render_csv(&[&ec], &[], &model()).unwrap();
        let r = &rows(&csv)[0];

        assert_eq!(r[DUP], "OFF");
        assert_eq!(r[OFFSET], "0.600000", "2m standard shift");
        assert_eq!(r[TONE], "OFF");
        assert_eq!(r[RPT_TONE], "88.5Hz");
        assert_eq!(r[DTCS], "023");
    }

    /// Airband AM has neither kind of squelch, and the radio writes its real
    /// 8.33 kHz channel spacing — both straight off the radio's own export.
    #[test]
    fn an_airband_am_memory_has_no_squelch_columns_at_all() {
        let mut c = chan(1, "FNL CTAF", 118.400);
        c.mode = Some("AM".into());
        let ec = expand(c);

        let r = &rows(&render_csv(&[&ec], &[], &model()).unwrap())[0];
        assert_eq!(r[MODE], "AM");
        assert_eq!(r[TS], "8.33kHz");
        for col in [TONE, RPT_TONE, TSQL_TONE, DTCS, DTCS_POL, DV_SQL, YOUR] {
            assert_eq!(r[col], "", "column {col} should be empty on an AM memory");
        }
    }

    /// Airband is the one band with no standard shift to fall back on, and the
    /// radio writes a flat zero for its own airband memories. Measured: carrying
    /// 0.600 here would be claiming a 2 m convention on a band that has none.
    #[test]
    fn an_airband_memory_carries_no_standard_shift() {
        let mut c = chan(1, "FNL CTAF", 118.400);
        c.mode = Some("AM".into());
        let ec = expand(c);

        let r = &rows(&render_csv(&[&ec], &[], &model()).unwrap())[0];
        assert_eq!(r[DUP], "OFF");
        assert_eq!(r[OFFSET], "0.000000");
    }

    /// A shift stored as duplex + offset, with no transmit frequency of its own,
    /// must still come out as a shift — this is how a hand-entered channel
    /// arrives.
    #[test]
    fn a_stored_shift_works_without_an_explicit_transmit_frequency() {
        let mut c = chan(1, "Plus", 145.310);
        c.duplex = Some("+".into());
        c.offset = Some(0.6);
        let ec = expand(c);

        let r = &rows(&render_csv(&[&ec], &[], &model()).unwrap())[0];
        assert_eq!(r[DUP], "DUP+");
        assert_eq!(r[OFFSET], "0.600000");
    }

    /// Different uplink and downlink tones are what cross mode is for, and this
    /// radio can represent it exactly — both frequencies survive.
    #[test]
    fn a_cross_tone_repeater_keeps_both_frequencies() {
        let mut c = chan(1, "Cross", 147.000);
        c.tone_mode = Some("Cross".into());
        c.ctcss_uplink = Some(103.5);
        c.ctcss_downlink = Some(107.2);
        let ec = expand(c);

        let r = &rows(&render_csv(&[&ec], &[], &model()).unwrap())[0];
        assert_eq!(r[TONE], "TONE(T)/TSQL(R)");
        assert_eq!(r[RPT_TONE], "103.5Hz");
        assert_eq!(r[TSQL_TONE], "107.2Hz");
    }

    /// D-STAR is the reason half these columns exist. The port letter comes from
    /// the band, and the gateway is the same call with `G`.
    #[test]
    fn a_dstar_memory_carries_packed_call_signs() {
        let mut c = chan(1, "W0ABC DV", 442.100);
        c.mode = Some("DSTAR".into());
        c.callsign = Some("W0ABC".into());
        c.tx_freq = Some(447.100);
        let ec = expand(c);

        let r = &rows(&render_csv(&[&ec], &[], &model()).unwrap())[0];
        assert_eq!(r[MODE], "DV");
        assert_eq!(r[YOUR], "CQCQCQ");
        assert_eq!(r[RPT1], "W0ABC  B", "70cm repeaters use port B");
        assert_eq!(r[RPT2], "W0ABC  G");
        assert_eq!(r[DV_SQL], "OFF");
        assert_eq!(r[DV_CSQL], "0");
        assert_eq!(r[RPT1].len(), 8, "Icom call signs are exactly 8 characters");
        // A DV memory has no analog squelch, and the radio leaves every tone
        // column empty rather than carrying a default through.
        for col in [TONE, RPT_TONE, TSQL_TONE, DTCS, DTCS_POL] {
            assert_eq!(r[col], "", "column {col} should be empty on a DV memory");
        }
    }

    /// An analog memory must leave the digital columns empty — an FM row with
    /// `OFF` in DV SQL is not what the radio writes for itself.
    #[test]
    fn an_analog_memory_leaves_the_digital_columns_blank() {
        let ec = expand(chan(1, "FM only", 146.520));
        let r = &rows(&render_csv(&[&ec], &[], &model()).unwrap())[0];
        assert_eq!(r[DV_SQL], "");
        assert_eq!(r[YOUR], "");
        assert_eq!(r[RPT1], "");
    }

    /// Channel lists become groups in codeplug order, and channel numbering
    /// restarts inside each group — that is what makes them separate groups on
    /// the radio rather than one long list.
    #[test]
    fn channel_lists_become_groups_and_numbering_restarts() {
        let a = expand(chan(1, "A1", 146.520));
        let b = expand(chan(2, "A2", 146.540));
        let d = expand(chan(3, "B1", 446.000));
        let channels = vec![&a, &b, &d];
        let groups = vec![
            CodeplugGroup {
                list_id: 1,
                list_name: "Two Metres".into(),
                channels: vec![a.channel.clone(), b.channel.clone()],
            },
            CodeplugGroup {
                list_id: 2,
                list_name: "Seventy".into(),
                channels: vec![d.channel.clone()],
            },
        ];

        let r = rows(&render_csv(&channels, &groups, &model()).unwrap());
        assert_eq!(
            r.iter()
                .map(|r| (r[GROUP_NO].as_str(), r[GROUP_NAME].as_str(), r[CH_NO].as_str()))
                .collect::<Vec<_>>(),
            vec![
                ("00", "Two Metres", "00"),
                ("00", "Two Metres", "01"),
                ("01", "Seventy", "00"),
            ]
        );
    }

    /// An Icom memory holds one group number, so a channel named by two lists
    /// has to pick one. Taking the first is the rule; duplicating it into both
    /// would spend memory capacity the operator never asked for.
    #[test]
    fn a_channel_in_two_lists_lands_in_the_first() {
        let a = expand(chan(1, "Shared", 146.520));
        let groups = vec![
            CodeplugGroup {
                list_id: 1,
                list_name: "First".into(),
                channels: vec![a.channel.clone()],
            },
            CodeplugGroup {
                list_id: 2,
                list_name: "Second".into(),
                channels: vec![a.channel.clone()],
            },
        ];

        let r = rows(&render_csv(&[&a], &groups, &model()).unwrap());
        assert_eq!(r.len(), 1);
        assert_eq!(r[0][GROUP_NAME], "First");
    }

    /// A codeplug with no channel lists still has to produce a usable file —
    /// the radio always displays some group.
    #[test]
    fn ungrouped_channels_get_one_group_of_their_own() {
        let a = expand(chan(1, "Loose", 146.520));
        let r = rows(&render_csv(&[&a], &[], &model()).unwrap());
        assert_eq!(r[0][GROUP_NO], "00");
        assert_eq!(r[0][GROUP_NAME], DEFAULT_GROUP_NAME);
    }

    /// A list that contributed no memories must not consume a group number: the
    /// numbering the operator sees on the radio would otherwise have holes in it.
    #[test]
    fn an_empty_list_does_not_burn_a_group_number() {
        let a = expand(chan(1, "Only", 146.520));
        let groups = vec![
            CodeplugGroup {
                list_id: 1,
                list_name: "Empty".into(),
                channels: vec![],
            },
            CodeplugGroup {
                list_id: 2,
                list_name: "Real".into(),
                channels: vec![a.channel.clone()],
            },
        ];

        let r = rows(&render_csv(&[&a], &groups, &model()).unwrap());
        assert_eq!(r[0][GROUP_NO], "00");
        assert_eq!(r[0][GROUP_NAME], "Real");
    }

    /// Over capacity is a problem the operator can fix in the app; finding out
    /// from the radio's own importer, after copying a card, is not.
    #[test]
    fn too_many_channels_for_the_radio_is_refused_by_name() {
        let all: Vec<ExpandedChannel> = (0..=MAX_MEMORIES as i64)
            .map(|i| expand(chan(i, "x", 146.0 + i as f64 / 1000.0)))
            .collect();
        let refs: Vec<&ExpandedChannel> = all.iter().collect();

        let err = render_csv(&refs, &[], &model()).unwrap_err();
        assert!(err.contains("1000"), "{err}");
    }

    /// Names are the radio's, not the database's: 16 characters, and distinct,
    /// so two repeaters do not both arrive as the same stump.
    #[test]
    fn names_are_truncated_to_the_radios_limit() {
        let ec = expand(chan(1, "A very long repeater name", 146.520));
        let r = rows(&render_csv(&[&ec], &[], &model()).unwrap());
        assert_eq!(r[0][NAME].chars().count(), MAX_NAME);
    }
}
