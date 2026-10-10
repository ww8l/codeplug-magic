//! The codeplug build: a database codeplug becomes the MD-380's channel,
//! contact, zone and scan-list tables, patched into an image just read off the
//! radio.
//!
//! A program is a **full replace** of those four tables, as on the AnyTone:
//! every record the codeplug produces is written, every other slot is put in
//! the free form. Everything else in the image — general settings, buttons,
//! menus, text messages, privacy keys, GPS, the radio's own byte at 0x2F003 —
//! comes from the read and goes back unchanged.
//!
//! RX group lists are cleared rather than built. Tim's working codeplug has a
//! group list on none of its 106 digital channels (s136), so a channel with no
//! list receives its own TX talkgroup — that is the arrangement this program
//! reproduces. Whether the radio ALSO needs a list to hear it is a ladder-step-3
//! question, not settled here.

use std::collections::HashMap;

use crate::commands::export::{
    channel_fit, disambiguate_names, expanded_name, invert_chirp_tones, tx_frequency, ChannelFit,
    ExpandedChannel,
};
use crate::models::{Channel as DbChannel, RadioModel};
use crate::radios::driver::{CodeplugPayload, CodeplugPreview, SkippedChannel};

use super::layout::*;
use super::memory::{
    mhz_to_10hz, Admit, Bandwidth, CallType, Channel, Contact, Mode, ScanList, Tone, Zone,
};

/// The model row this driver programs.
pub(crate) const MODEL_NAME: &str = "MD-380";

pub(crate) struct Plan {
    pub channels: Vec<Channel>,
    pub contacts: Vec<Contact>,
    pub zones: Vec<Zone>,
    pub scan_lists: Vec<ScanList>,
    pub skipped: Vec<SkippedChannel>,
    pub warnings: Vec<String>,
}

impl Plan {
    pub(crate) fn preview(&self) -> CodeplugPreview {
        CodeplugPreview {
            radio: "TYT MD-380".into(),
            channels: self.channels.len(),
            zones: self.zones.len(),
            scan_lists: self.scan_lists.len(),
            contacts: self.contacts.len(),
            zone_names: self.zones.iter().map(|z| z.name.clone()).collect(),
            scan_list_names: self.scan_lists.iter().map(|s| s.name.clone()).collect(),
            skipped: self.skipped.clone(),
            warnings: self.warnings.clone(),
        }
    }

    /// Patch the plan into `base` (a full image read off the radio).
    pub(crate) fn apply(&self, base: &[u8]) -> Result<Vec<u8>, String> {
        if base.len() != IMAGE_LEN {
            return Err(format!("expected a {IMAGE_LEN}-byte MD-380 image, got {}", base.len()));
        }
        let mut img = base.to_vec();
        fill(&mut img, CHANNELS, CHANNEL_LEN, CHANNEL_COUNT, &Channel::UNUSED, |i| {
            self.channels.get(i).map(|c| c.encode().to_vec())
        });
        fill(&mut img, CONTACTS, CONTACT_LEN, CONTACT_COUNT, &Contact::UNUSED, |i| {
            self.contacts.get(i).map(|c| c.encode().to_vec())
        });
        fill(&mut img, ZONES, ZONE_LEN, ZONE_COUNT, &[0u8; ZONE_LEN], |i| {
            self.zones.get(i).map(|z| z.encode().to_vec())
        });
        fill(&mut img, SCAN_LISTS, SCAN_LIST_LEN, SCAN_LIST_COUNT, &ScanList::UNUSED, |i| {
            self.scan_lists.get(i).map(|s| s.encode().to_vec())
        });
        fill(&mut img, RX_GROUPS, RX_GROUP_LEN, RX_GROUP_COUNT, &[0u8; RX_GROUP_LEN], |_| None);
        self.remap_quick_keys(base, &mut img);
        // The radio's current-zone byte (see `CURRENT_ZONE`) must not point
        // past the zones this codeplug has: send it to zone 1 if it would.
        let zone = img[CURRENT_ZONE];
        if zone != 0xFF && zone as usize > self.zones.len() {
            img[CURRENT_ZONE] = 1;
        }
        Ok(img)
    }

    /// One Touch and Number Keys name contacts by index into a table this
    /// program just rebuilt. Point each at the same talkgroup's new index, or
    /// clear it when the codeplug no longer has that talkgroup — never leave it
    /// on whatever contact now happens to sit at the old index.
    fn remap_quick_keys(&self, base: &[u8], img: &mut [u8]) {
        let remap = |old: u16| -> u16 {
            if old == 0 || old as usize > CONTACT_COUNT {
                return 0;
            }
            let at0 = at(CONTACTS, CONTACT_LEN, old as usize - 1);
            let Some(was) = Contact::decode(&base[at0..at0 + CONTACT_LEN]) else { return 0 };
            self.contacts
                .iter()
                .position(|c| c.id == was.id && c.call_type == was.call_type)
                .map_or(0, |i| i as u16 + 1)
        };
        for k in 0..NUMBER_KEY_COUNT {
            let at0 = NUMBER_KEYS + 2 * k;
            let old = u16::from_le_bytes([img[at0], img[at0 + 1]]);
            img[at0..at0 + 2].copy_from_slice(&remap(old).to_le_bytes());
        }
        for k in 0..ONE_TOUCH_COUNT {
            let at0 = ONE_TOUCH + 4 * k;
            if img[at0] >> 2 != 52 {
                continue; // not a digital call: the contact field is unused
            }
            let old = u16::from_le_bytes([img[at0 + 2], img[at0 + 3]]);
            let new = remap(old);
            img[at0 + 2..at0 + 4].copy_from_slice(&new.to_le_bytes());
            if new == 0 {
                img[at0] = (48 << 2) | (img[at0] & 3); // mode None
            }
        }
    }
}

fn fill(
    img: &mut [u8],
    base: usize,
    len: usize,
    count: usize,
    unused: &[u8],
    record: impl Fn(usize) -> Option<Vec<u8>>,
) {
    for i in 0..count {
        let at = at(base, len, i);
        match record(i) {
            Some(r) => img[at..at + len].copy_from_slice(&r),
            None => img[at..at + len].copy_from_slice(unused),
        }
    }
}

/// How many records of each table are in use in an image — for the report's
/// "cleared" counts.
pub(crate) struct Used {
    pub channels: usize,
    pub contacts: usize,
    pub zones: usize,
    pub scan_lists: usize,
}

pub(crate) fn count_used(img: &[u8]) -> Used {
    let n = |base, len, count, used: fn(&[u8]) -> bool| {
        (0..count).filter(|&i| used(&img[at(base, len, i)..][..len])).count()
    };
    Used {
        channels: n(CHANNELS, CHANNEL_LEN, CHANNEL_COUNT, Channel::is_used),
        contacts: n(CONTACTS, CONTACT_LEN, CONTACT_COUNT, Contact::is_used),
        zones: n(ZONES, ZONE_LEN, ZONE_COUNT, Zone::is_used),
        scan_lists: n(SCAN_LISTS, SCAN_LIST_LEN, SCAN_LIST_COUNT, ScanList::is_used),
    }
}

fn truncate(s: &str, max: usize) -> String {
    s.chars().take(max).collect()
}

/// The channel's tone columns onto the MD-380's two fields, (rx, tx). The
/// mode logic is the shared `export::invert_chirp_tones` (also the AnyTone's);
/// only the encodings are this radio's: CTCSS in 0.1 Hz, DCS as its octal
/// digits with the channel's polarity.
fn tones(c: &DbChannel, warnings: &mut Vec<String>, who: &str) -> (Tone, Tone) {
    let (sides, w) = invert_chirp_tones(
        c.tone_mode.as_deref(),
        c.ctcss_uplink,
        c.ctcss_downlink,
        c.dcs_code.as_deref(),
        c.dcs_rx_code.as_deref(),
        &c.dcs_polarity,
        |hz, side, w| {
            if (60.0..=260.0).contains(&hz) {
                Some(Tone::Ctcss((hz * 10.0).round() as u16))
            } else {
                w.push(format!("{side} CTCSS {hz} Hz is out of range — dropped"));
                None
            }
        },
        |code, inverted, side, w| {
            let code = code.trim();
            // Stored as the octal digits ("023"); the radio keeps those digits
            // as BCD, so the digit string is what matters — checked for octal.
            match (u16::from_str_radix(code, 8), code.parse::<u16>()) {
                (Ok(_), Ok(d)) => Some(Tone::Dcs { code: d, inverted }),
                _ => {
                    w.push(format!("{side} DCS code '{code}' is not a valid code — dropped"));
                    None
                }
            }
        },
    );
    warnings.extend(w.into_iter().map(|w| format!("{who}: {w}")));
    (sides.rx.unwrap_or(Tone::NONE), sides.tx.unwrap_or(Tone::NONE))
}

/// The name a row goes to the radio under. A curated talkgroup row gets the
/// talkgroup appended (the shared `expanded_name`); a row whose talkgroup came
/// from the channel itself (radio-imported) keeps its own name, as on the
/// AnyTone.
fn row_name(ec: &ExpandedChannel, model: &RadioModel) -> String {
    if ec.tg_inline || ec.tg_label.is_none() {
        let bare = ExpandedChannel {
            channel: ec.channel.clone(),
            tg_label: None,
            timeslot: ec.timeslot,
            tg_number: ec.tg_number,
            tg_call_type: ec.tg_call_type.clone(),
            tg_inline: ec.tg_inline,
        };
        let n = expanded_name(&bare, model);
        if !n.trim().is_empty() {
            return n;
        }
        // An imported row can hold "" rather than NULL in every name column.
        match ec.channel.callsign.as_deref().map(str::trim) {
            Some(cs) if !cs.is_empty() => cs.to_string(),
            _ => format!("{:.4}", ec.channel.rx_freq),
        }
    } else {
        expanded_name(ec, model)
    }
}

pub(crate) fn plan_program(payload: &CodeplugPayload) -> Result<Plan, String> {
    let model = payload.model;
    if model.model != MODEL_NAME {
        return Err(format!("the MD-380 driver cannot program a {}", model.display_name));
    }
    let mut warnings = Vec::new();
    let mut skipped = Vec::new();

    // ---- channel rows that reach the radio, in slot order
    let mut rows: Vec<(&ExpandedChannel, bool)> = Vec::new(); // (row, receive-only)
    for ec in payload.channels {
        match channel_fit(&ec.channel, model) {
            ChannelFit::Excluded(reason) => skipped.push(SkippedChannel {
                name: row_name(ec, model),
                reason,
            }),
            ChannelFit::ReceiveOnly(_) => rows.push((ec, true)),
            ChannelFit::Included => rows.push((ec, false)),
        }
    }
    if rows.len() > CHANNEL_COUNT {
        return Err(format!(
            "this codeplug has {} channels for the MD-380; it holds {CHANNEL_COUNT}",
            rows.len()
        ));
    }

    // ---- names, made unique across the radio
    let mut named: Vec<(String, f64)> = rows
        .iter()
        .map(|(ec, _)| (truncate(&row_name(ec, model), NAME_UNITS), ec.channel.rx_freq))
        .collect();
    disambiguate_names(&mut named, NAME_UNITS);

    // ---- contacts: one per talkgroup, in first-use order
    let mut contacts: Vec<Contact> = Vec::new();
    let mut contact_of: HashMap<(i64, bool), u16> = HashMap::new();
    let mut slots_of: HashMap<i64, Vec<u16>> = HashMap::new();
    let mut channels = Vec::with_capacity(rows.len());

    for (slot0, ((ec, rx_only), (name, _))) in rows.iter().zip(&named).enumerate() {
        let c = &ec.channel;
        let digital = c.mode.as_deref().is_some_and(|m| m.eq_ignore_ascii_case("DMR"));
        let who = name.as_str();
        let rx = mhz_to_10hz(c.rx_freq);
        let tx = if *rx_only { rx } else { mhz_to_10hz(tx_frequency(c)) };

        let mut ch = Channel::decode(&{
            let mut t = Channel::template();
            super::memory::encode_freq(rx, &mut t[16..20]);
            super::memory::encode_freq(tx, &mut t[20..24]);
            t
        })
        .expect("the template decodes");
        ch.name = name.clone();
        ch.rx_10hz = rx;
        ch.tx_10hz = tx;
        ch.rx_only = *rx_only;
        ch.high_power = !c.power.as_deref().is_some_and(|p| p.eq_ignore_ascii_case("Low"));
        ch.admit = Admit::Always;

        if digital {
            ch.mode = Mode::Digital;
            ch.bandwidth = Bandwidth::Narrow;
            let cc = c.dmr_color_code.unwrap_or(1);
            if !(0..=15).contains(&cc) {
                warnings.push(format!("{who}: colour code {cc} is out of range — programmed as 1"));
            }
            ch.color_code = if (0..=15).contains(&cc) { cc as u8 } else { 1 };
            let ts = ec.timeslot.or(c.dmr_timeslot).unwrap_or(1);
            if ts != 1 && ts != 2 {
                warnings.push(format!("{who}: timeslot {ts} is not 1 or 2 — programmed as 1"));
            }
            ch.slot = if ts == 2 { 2 } else { 1 };
            ch.rx_tone = Tone::NONE;
            ch.tx_tone = Tone::NONE;
            match ec.tg_number {
                Some(tg) if (1..=0xFF_FFFF).contains(&tg) => {
                    let private = ec.tg_call_type.as_deref() == Some("Private");
                    let next = contacts.len() as u16 + 1;
                    let idx = *contact_of.entry((tg, private)).or_insert_with(|| {
                        let label = ec.tg_label.clone().unwrap_or_else(|| format!("TG {tg}"));
                        contacts.push(Contact {
                            id: tg as u32,
                            call_type: if private { CallType::Private } else { CallType::Group },
                            name: truncate(&label, NAME_UNITS),
                            flags: Contact::UNUSED[3] & !3,
                        });
                        next
                    });
                    ch.contact = idx;
                }
                Some(tg) => {
                    warnings.push(format!("{who}: talkgroup {tg} is not a valid DMR ID — no contact"));
                    ch.contact = 0;
                }
                None => {
                    warnings.push(format!(
                        "{who}: DMR channel with no talkgroup — it will receive but cannot transmit a call"
                    ));
                    ch.contact = 0;
                }
            }
        } else {
            ch.mode = Mode::Analog;
            ch.bandwidth = if c.mode.as_deref().is_some_and(|m| m.eq_ignore_ascii_case("NFM")) {
                Bandwidth::Narrow
            } else {
                Bandwidth::Wide
            };
            ch.slot = 1;
            ch.color_code = 1;
            let (rxt, txt) = tones(c, &mut warnings, who);
            ch.rx_tone = rxt;
            ch.tx_tone = txt;
        }
        slots_of.entry(c.id).or_default().push(slot0 as u16 + 1);
        channels.push(ch);
    }
    if contacts.len() > CONTACT_COUNT {
        return Err(format!(
            "this codeplug uses {} talkgroups; the MD-380 holds {CONTACT_COUNT} contacts",
            contacts.len()
        ));
    }

    // ---- zones: one per channel list, split into 16s
    let mut zones = Vec::new();
    for g in payload.groups {
        let members: Vec<u16> = g
            .channels
            .iter()
            .flat_map(|c| slots_of.get(&c.id).cloned().unwrap_or_default())
            .collect();
        if members.is_empty() {
            warnings.push(format!("list '{}' has no channel the MD-380 can hold — no zone", g.list_name));
            continue;
        }
        let parts = members.len().div_ceil(ZONE_MEMBERS);
        if parts > 1 {
            warnings.push(format!(
                "list '{}' has {} channels; an MD-380 zone holds {ZONE_MEMBERS}, so it becomes {parts} zones",
                g.list_name,
                members.len()
            ));
        }
        for (k, chunk) in members.chunks(ZONE_MEMBERS).enumerate() {
            let name = if parts == 1 {
                truncate(&g.list_name, NAME_UNITS)
            } else {
                let suffix = format!(" {}", k + 1);
                truncate(&g.list_name, NAME_UNITS - suffix.chars().count()) + &suffix
            };
            let mut m = [0u16; ZONE_MEMBERS];
            m[..chunk.len()].copy_from_slice(chunk);
            zones.push(Zone { name, members: m });
        }
    }
    if zones.len() > ZONE_COUNT {
        warnings.push(format!(
            "{} zones; the MD-380 holds {ZONE_COUNT} — the rest are dropped",
            zones.len()
        ));
        zones.truncate(ZONE_COUNT);
    }

    // ---- scan lists
    let first_slot = |id: Option<i64>| id.and_then(|id| slots_of.get(&id)).and_then(|v| v.first().copied());
    let mut scan_lists = Vec::new();
    let mut scan_index: HashMap<i64, u8> = HashMap::new();
    for sl in payload.scan_lists {
        let members: Vec<u16> = sl
            .member_channel_ids
            .iter()
            .flat_map(|id| slots_of.get(id).cloned().unwrap_or_default())
            .collect();
        if members.is_empty() {
            warnings.push(format!("scan list '{}' has no programmed channel — omitted", sl.name));
            continue;
        }
        if scan_lists.len() == SCAN_LIST_COUNT {
            warnings.push(format!("scan list '{}' dropped — the MD-380 holds {SCAN_LIST_COUNT}", sl.name));
            continue;
        }
        if members.len() > SCAN_LIST_MEMBERS {
            warnings.push(format!(
                "scan list '{}' has {} channels; the MD-380 scans {SCAN_LIST_MEMBERS} — the rest are dropped",
                sl.name,
                members.len()
            ));
        }
        let mut m = [0u16; SCAN_LIST_MEMBERS];
        for (i, s) in members.iter().take(SCAN_LIST_MEMBERS).enumerate() {
            m[i] = *s;
        }
        let mut priority = |bit: i64, id: Option<i64>, which: &str| -> u16 {
            if sl.priority_select & bit == 0 {
                return 0xFFFF;
            }
            match first_slot(id) {
                Some(s) => s,
                None => {
                    warnings.push(format!(
                        "scan list '{}': priority {which} channel is not programmed — no priority",
                        sl.name
                    ));
                    0xFFFF
                }
            }
        };
        let priority_1 = priority(1, sl.priority_channel_id, "1");
        let priority_2 = priority(2, sl.priority_channel_2_id, "2");
        scan_lists.push(ScanList {
            name: truncate(&sl.name, NAME_UNITS),
            priority_1,
            priority_2,
            tx_designated: 0xFFFF,
            members: m,
            timing: ScanList::DEFAULT_TIMING,
        });
        scan_index.insert(sl.id, scan_lists.len() as u8);
    }

    // ---- a channel's own scan list comes only from an explicit assignment
    for o in payload.scan_list_overrides {
        let Some(&idx) = scan_index.get(&o.scan_list_id) else {
            warnings.push("a channel's scan list was not programmed — its assignment is dropped".into());
            continue;
        };
        for &slot in slots_of.get(&o.channel_id).into_iter().flatten() {
            channels[slot as usize - 1].scan_list = idx;
        }
    }

    Ok(Plan { channels, contacts, zones, scan_lists, skipped, warnings })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::export::{CodeplugGroup, CodeplugScanList};
    use crate::radios::tyt_md380::memory::Tone;

    fn model() -> RadioModel {
        crate::seed::test_model(MODEL_NAME)
    }

    fn chan(id: i64, name: &str, rx: f64, mode: &str) -> DbChannel {
        DbChannel {
            id,
            name_long: Some(name.into()),
            rx_freq: rx,
            mode: Some(mode.into()),
            dcs_polarity: "NN".into(),
            ..Default::default()
        }
    }

    fn row(c: DbChannel, tg: Option<(i64, &str, i64)>) -> ExpandedChannel {
        ExpandedChannel {
            channel: c,
            tg_label: tg.map(|t| t.1.to_string()),
            timeslot: tg.map(|t| t.2),
            tg_number: tg.map(|t| t.0),
            tg_call_type: tg.map(|_| "Group".to_string()),
            tg_inline: false,
        }
    }

    #[test]
    fn a_repeater_with_two_talkgroups_is_two_channels_sharing_contacts_and_one_zone() {
        let m = model();
        let mut rpt = chan(1, "RMH700", 445.2, "DMR");
        rpt.duplex = Some("-".into());
        rpt.offset = Some(5.0);
        rpt.dmr_color_code = Some(7);
        let mut fm = chan(2, "W0UPS", 447.275, "FM");
        fm.duplex = Some("-".into());
        fm.offset = Some(5.0);
        fm.tone_mode = Some("TSQL".into());
        fm.ctcss_downlink = Some(100.0);
        let rows = vec![
            row(rpt.clone(), Some((3108, "CO", 1))),
            row(rpt.clone(), Some((91, "WW", 1))),
            row(fm.clone(), None),
        ];
        let groups = vec![CodeplugGroup { list_id: 1, list_name: "NOCO".into(), channels: vec![rpt, fm] }];
        let p = plan_program(&CodeplugPayload {
            model: &m,
            groups: &groups,
            channels: &rows,
            scan_lists: &[],
            scan_list_overrides: &[],
        })
        .unwrap();
        assert_eq!(p.channels.len(), 3);
        assert_eq!(p.channels[0].name, "RMH700 CO");
        assert_eq!((p.channels[0].rx_10hz, p.channels[0].tx_10hz), (44_520_000, 44_020_000));
        assert_eq!((p.channels[0].color_code, p.channels[0].contact), (7, 1));
        assert_eq!(p.channels[1].contact, 2);
        assert_eq!(p.contacts.iter().map(|c| c.id).collect::<Vec<_>>(), [3108, 91]);
        assert_eq!(p.channels[2].mode, Mode::Analog);
        assert_eq!(p.channels[2].rx_tone, Tone::Ctcss(1000));
        assert_eq!(p.channels[2].tx_tone, Tone::Ctcss(1000));
        assert_eq!(p.zones.len(), 1);
        assert_eq!(&p.zones[0].members[..4], &[1, 2, 3, 0]);
    }

    #[test]
    fn dcs_takes_its_octal_digits_and_each_sides_polarity() {
        let mut c = chan(1, "D", 446.0, "FM");
        c.tone_mode = Some("DTCS".into());
        c.dcs_code = Some("023".into());
        c.dcs_polarity = "NR".into();
        let mut w = Vec::new();
        let (rx, tx) = tones(&c, &mut w, "D");
        assert_eq!(tx, Tone::Dcs { code: 23, inverted: false });
        assert_eq!(rx, Tone::Dcs { code: 23, inverted: true });
        c.dcs_code = Some("298".into()); // 8 and 9 are not octal
        let (_, tx) = tones(&c, &mut w, "D");
        assert_eq!(tx, Tone::NONE);
        assert!(w.iter().any(|w| w.contains("298")), "{w:?}");
    }

    #[test]
    fn a_long_list_splits_into_numbered_zones_of_sixteen() {
        let m = model();
        let chans: Vec<DbChannel> =
            (0..20).map(|i| chan(i, &format!("C{i}"), 446.0 + i as f64 * 0.0125, "FM")).collect();
        let rows: Vec<_> = chans.iter().cloned().map(|c| row(c, None)).collect();
        let groups = vec![CodeplugGroup { list_id: 1, list_name: "A VERY LONG LIST".into(), channels: chans }];
        let p = plan_program(&CodeplugPayload {
            model: &m,
            groups: &groups,
            channels: &rows,
            scan_lists: &[],
            scan_list_overrides: &[],
        })
        .unwrap();
        assert_eq!(p.zones.len(), 2);
        assert_eq!(p.zones[0].name, "A VERY LONG LI 1");
        assert_eq!(p.zones[1].members[3], 20);
        assert_eq!(p.zones[1].members[4], 0);
        assert!(p.warnings.iter().any(|w| w.contains("becomes 2 zones")));
    }

    /// A Number Key on TG 3108 follows TG 3108 to its new index; one on a
    /// talkgroup the new codeplug lacks is cleared, as is a digital One Touch.
    #[test]
    fn quick_keys_follow_their_talkgroup_or_are_cleared() {
        let m = model();
        let c = chan(1, "RPT", 445.2, "DMR");
        let rows = vec![row(c.clone(), Some((91, "WW", 1))), row(c.clone(), Some((3108, "CO", 1)))];
        let groups = vec![CodeplugGroup { list_id: 1, list_name: "Z".into(), channels: vec![c] }];
        let p = plan_program(&CodeplugPayload {
            model: &m,
            groups: &groups,
            channels: &rows,
            scan_lists: &[],
            scan_list_overrides: &[],
        })
        .unwrap();
        let mut base = vec![0u8; IMAGE_LEN];
        for i in 0..CONTACT_COUNT {
            base[at(CONTACTS, CONTACT_LEN, i)..][..CONTACT_LEN].copy_from_slice(&Contact::UNUSED);
        }
        let old = |id: u32, name: &str| Contact { id, call_type: CallType::Group, name: name.into(), flags: 0xC0 };
        base[at(CONTACTS, CONTACT_LEN, 0)..][..CONTACT_LEN].copy_from_slice(&old(3108, "CO").encode());
        base[at(CONTACTS, CONTACT_LEN, 1)..][..CONTACT_LEN].copy_from_slice(&old(9999, "GONE").encode());
        base[NUMBER_KEYS..NUMBER_KEYS + 4].copy_from_slice(&[1, 0, 2, 0]); // keys 0,1 -> 3108, 9999
        base[ONE_TOUCH] = 52 << 2; // digital call, call type 0
        base[ONE_TOUCH + 2] = 2; // -> 9999
        let img = p.apply(&base).unwrap();
        // New table: 1 = TG 91, 2 = TG 3108.
        assert_eq!(&img[NUMBER_KEYS..NUMBER_KEYS + 4], &[2, 0, 0, 0]);
        assert_eq!(img[ONE_TOUCH] >> 2, 48);
        assert_eq!(&img[ONE_TOUCH + 2..ONE_TOUCH + 4], &[0, 0]);
    }

    #[test]
    fn applying_a_plan_replaces_the_tables_and_leaves_the_rest_of_the_image_alone() {
        let m = model();
        let c = chan(1, "SIMPLEX", 446.0, "FM");
        let rows = vec![row(c.clone(), None)];
        let groups = vec![CodeplugGroup { list_id: 1, list_name: "Z".into(), channels: vec![c.clone()] }];
        let sl = vec![CodeplugScanList {
            id: 9,
            name: "SCAN".into(),
            priority_channel_id: Some(1),
            priority_channel_2_id: None,
            priority_select: 1,
            look_back_a: 0,
            look_back_b: 0,
            dropout_delay: 0,
            dwell_time: 0,
            revert_channel: 0,
            member_channel_ids: vec![1],
        }];
        let p = plan_program(&CodeplugPayload {
            model: &m,
            groups: &groups,
            channels: &rows,
            scan_lists: &sl,
            scan_list_overrides: &[],
        })
        .unwrap();
        let mut base = vec![0xA5u8; IMAGE_LEN];
        base[CURRENT_ZONE] = 0x04;
        let img = p.apply(&base).unwrap();
        let used = count_used(&img);
        assert_eq!((used.channels, used.zones, used.contacts, used.scan_lists), (1, 1, 0, 1));
        // Zone 4 of a 1-zone codeplug: back to zone 1.
        assert_eq!(img[CURRENT_ZONE], 1);
        assert_eq!(&img[GENERAL_SETTINGS..GENERAL_SETTINGS + 16], &[0xA5; 16]);
        assert_eq!(&img[at(CHANNELS, CHANNEL_LEN, 1)..][..CHANNEL_LEN], Channel::UNUSED.as_slice());
        let s = ScanList::decode(&img[SCAN_LISTS..SCAN_LISTS + SCAN_LIST_LEN]);
        assert_eq!((s.priority_1, s.priority_2, s.members[0]), (1, 0xFFFF, 1));
    }
}
