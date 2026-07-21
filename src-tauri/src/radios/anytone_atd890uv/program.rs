//! Full-codeplug programming for the AnyTone AT-D890UV: push a codeplug from
//! the app database to the radio as a FULL REPLACE of its channel set —
//! channels, zones (from the codeplug's channel lists), scan lists, and DMR
//! contacts (talkgroups) — in ONE PC-mode session with a single END/commit/reboot.
//!
//! Two halves, split along the sync/async line the driver traits require:
//!   1. [`plan_program`] — pure payload → plan assembly (no hardware, no DB).
//!      The inverse of the radio-download import mapping: CHIRP tone scheme back
//!      to TX/RX sub-tones, power levels back to the mode-byte bits, DMR
//!      channels expanded per (talkgroup × timeslot) with contact indices
//!      assigned in first-use order. Drives the preview.
//!   2. [`run_program`] — the hardware session. Reads every region it will touch
//!      (whole-bank/0x4000-window RMW, the HW-proven safe write unit), computes
//!      the patched windows (slots/zones/scan lists/contacts beyond the plan are
//!      cleared to each region's empty pattern), writes a mandatory backup of the
//!      ORIGINAL bytes plus an `.expected.bin` image of the patched bytes BEFORE
//!      any write, then commits and ENDs once.
//!
//! The DB resolution that used to sit inside the assembly half now lives in
//! `commands::export::resolve_codeplug_payload`, so the planner is a pure
//! function and unit-testable without a database.

use std::collections::HashMap;
use std::path::Path;

use serialport::SerialPort;

use crate::commands::export::{exclusion_reason, expanded_name, tx_frequency, ExpandedChannel};
use crate::models::RadioModel;
use crate::radios::driver::{
    CodeplugPayload, CodeplugPreview, CodeplugProgrammer, ProgramReport, SkippedChannel,
};
use crate::util::truncate;

use super::{
    commit_channel_writes, ctcss_index, decode_utf16_name, encode_channel_into_record,
    encode_contact_record, encode_scan_list_record, encode_utf16_field, encode_zone_list_record,
    end_session, enter_program_and_ident, is_empty_record, new_channel_record_template, open_port,
    plans_from_pre_read, read_block, serialize_backup,
    AnytoneChannelEdit, AnytoneSubTone, AnytoneToneEdit, BankPlan, RegionPatch,
    ScanListRecordSettings, BANK_STEP, CHANNEL_BASE, CH_PER_BANK, CH_REC_LEN, CONTACT_BASE,
    CONTACT_REC_LEN, MAX_CONTACTS_READ, MAX_SCAN_LISTS, MAX_ZONES, NUM_BANKS, SCAN_LIST_BASE,
    SCAN_BITMAP_BASE, SCAN_BITMAP_BYTES, SCAN_LIST_STEP, SCAN_MAX_CHANNELS, WRITE_WINDOW,
    ZONE_BITMAP_BASE, ZONE_BITMAP_BYTES,
    ZONE_LIST_BASE, ZONE_LIST_STEP, ZONE_MAX_CHANNELS, ZONE_NAME_BASE, ZONE_NAME_CHARS,
    ZONE_NAME_STEP,
};

/// Name length for channel/zone/scan-list/contact name fields (16 UTF-16 chars).
const NAME_CHARS: usize = 16;

// ============================================================
// Plan model (pure assembly output)
// ============================================================

/// One channel destined for a radio memory slot. Slots are 1-based and packed
/// contiguously from 1 (full replace: everything beyond the last slot is
/// cleared on the radio).
pub(crate) struct PlannedChannel {
    pub slot: usize,
    pub edit: AnytoneChannelEdit,
}

/// One zone (from a codeplug channel list), members as 0-based slot indices.
pub(crate) struct PlannedZone {
    pub name: String,
    pub member_slots: Vec<u16>,
}

/// One scan list, members as 0-based slot indices, plus its radio-encoded
/// settings (priority channels resolved to slots, timers/enums ready to write).
pub(crate) struct PlannedScanList {
    pub name: String,
    pub member_slots: Vec<u16>,
    pub settings: ScanListRecordSettings,
}

/// One DMR contact (talkgroup), at the 0-based contact index channels reference.
pub(crate) struct PlannedContact {
    pub index: u16,
    pub tg_number: u32,
    pub name: String,
    /// 0 = Private, 1 = Group (the radio's Call Type enum).
    pub call_type: u8,
}

/// The full assembled program: what `program_anytone_codeplug` writes and what
/// the preview summarises.
pub(crate) struct AnytoneProgramPlan {
    pub radio: String,
    pub channels: Vec<PlannedChannel>,
    pub zones: Vec<PlannedZone>,
    pub scan_lists: Vec<PlannedScanList>,
    pub contacts: Vec<PlannedContact>,
    pub skipped: Vec<SkippedChannel>,
    pub warnings: Vec<String>,
}

impl AnytoneProgramPlan {
    fn preview(&self) -> CodeplugPreview {
        CodeplugPreview {
            radio: self.radio.clone(),
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
}

// ============================================================
// Pure inversions (DB columns → radio encoding)
// ============================================================

/// Invert `import::map_anytone_tone`: the CHIRP-style tone columns back to the
/// radio's structured TX/RX sub-tones. An off-table CTCSS or an unparseable
/// DCS code degrades THAT side to no-tone with a warning instead of failing
/// the whole program. Returns the tone edit plus warnings (unprefixed — the
/// caller adds the channel name).
fn ctcss_side(hz: Option<f64>, side: &str, warnings: &mut Vec<String>) -> Option<AnytoneSubTone> {
    let hz = hz?;
    if ctcss_index(hz).is_none() {
        warnings.push(format!(
            "{side} CTCSS {hz} Hz is not in the radio's tone table — dropped"
        ));
        return None;
    }
    Some(AnytoneSubTone::Ctcss(hz))
}

fn dcs_side(code: Option<&str>, side: &str, warnings: &mut Vec<String>) -> Option<AnytoneSubTone> {
    let code = code?;
    // Stored DCS codes are the octal notation (e.g. "411"); the radio wants the
    // raw binary value (411₈ = 265). HW-confirmed on the D890UV.
    match u16::from_str_radix(code.trim(), 8) {
        Ok(c) => Some(AnytoneSubTone::Dcs(c)),
        Err(_) => {
            warnings.push(format!(
                "{side} DCS code '{code}' is not a valid octal DCS code — dropped"
            ));
            None
        }
    }
}

fn invert_tone(
    tone_mode: Option<&str>,
    cross_mode: &str,
    up: Option<f64>,
    down: Option<f64>,
    dcs_tx: Option<&str>,
    dcs_rx: Option<&str>,
) -> (AnytoneToneEdit, Vec<String>) {
    let mut warnings = Vec::new();
    let w = &mut warnings;
    let mode = tone_mode.unwrap_or("off");
    let tone = if mode.eq_ignore_ascii_case("off") || mode.is_empty() {
        AnytoneToneEdit::default()
    } else if mode.eq_ignore_ascii_case("Tone") {
        AnytoneToneEdit {
            tx: ctcss_side(up, "TX", w),
            rx: None,
        }
    } else if mode.eq_ignore_ascii_case("TSQL") {
        // Tone squelch keys on the downlink tone both ways.
        let hz = down.or(up);
        AnytoneToneEdit {
            tx: ctcss_side(hz, "TX", w),
            rx: ctcss_side(hz, "RX", w),
        }
    } else if mode.eq_ignore_ascii_case("DTCS") {
        AnytoneToneEdit {
            tx: dcs_side(dcs_tx, "TX", w),
            rx: dcs_side(dcs_rx.or(dcs_tx), "RX", w),
        }
    } else if mode.eq_ignore_ascii_case("Cross") {
        // Each side carries exactly one of CTCSS/DCS in the stored columns, so
        // the sides invert independently of the cross_mode label.
        let _ = cross_mode;
        let tx = ctcss_side(up, "TX", w).or_else(|| dcs_side(dcs_tx, "TX", w));
        let rx = ctcss_side(down, "RX", w).or_else(|| dcs_side(dcs_rx, "RX", w));
        AnytoneToneEdit { tx, rx }
    } else {
        warnings.push(format!("unknown tone mode '{mode}' — programmed with no tone"));
        AnytoneToneEdit::default()
    };
    (tone, warnings)
}

/// Invert the import's power mapping: Low/Med/High/NULL → mode-byte power bits
/// 0/1/2/3 (NULL = radio default = Turbo). An unrecognised label falls back to
/// High with a warning.
fn invert_power(power: Option<&str>, warnings: &mut Vec<String>, who: &str) -> u8 {
    match power {
        None => 3, // Turbo (radio max) round-trips as NULL
        Some(p) if p.eq_ignore_ascii_case("Low") => 0,
        Some(p) if p.eq_ignore_ascii_case("Med") || p.eq_ignore_ascii_case("Medium") => 1,
        Some(p) if p.eq_ignore_ascii_case("High") => 2,
        Some(p) => {
            warnings.push(format!("{who}: unknown power '{p}' — programmed as High"));
            2
        }
    }
}

/// Build the radio channel name: expanded rows from curated talkgroup
/// assignments get the "<name> <talkgroup>" treatment (same as the CSV
/// exporter); inline-talkgroup rows (radio-imported) and plain channels keep
/// the channel's own name so a round-trip doesn't rename anything.
fn channel_name(ec: &ExpandedChannel, model: &RadioModel, slot: usize) -> String {
    if ec.tg_label.is_some() && !ec.tg_inline {
        return expanded_name(ec, model);
    }
    let base = ec
        .channel
        .name_long
        .clone()
        .or_else(|| ec.channel.name_short.clone())
        .or_else(|| ec.channel.callsign.clone())
        .unwrap_or_else(|| format!("CH {slot}"));
    truncate(&base, NAME_CHARS)
}

/// Invert one expanded row to a channel edit. `contact_index` is the radio
/// contact slot assigned to the row's talkgroup (0 when the row has none).
fn invert_channel(
    ec: &ExpandedChannel,
    model: &RadioModel,
    slot: usize,
    contact_index: u16,
    warnings: &mut Vec<String>,
) -> AnytoneChannelEdit {
    let c = &ec.channel;
    let name = channel_name(ec, model, slot);
    let mode_str = c.mode.as_deref().unwrap_or("FM").to_uppercase();
    let digital = mode_str == "DMR";

    let tone = if digital {
        AnytoneToneEdit::default()
    } else {
        let (tone, tone_warnings) = invert_tone(
            c.tone_mode.as_deref(),
            &c.cross_mode,
            c.ctcss_uplink,
            c.ctcss_downlink,
            c.dcs_code.as_deref(),
            c.dcs_rx_code.as_deref(),
        );
        for w in tone_warnings {
            warnings.push(format!("{name}: {w}"));
        }
        tone
    };

    AnytoneChannelEdit {
        power: invert_power(c.power.as_deref(), warnings, &name),
        name,
        rx_mhz: c.rx_freq,
        tx_mhz: tx_frequency(c),
        // FM = wide 25 kHz; NFM and DMR = narrow 12.5 kHz.
        wide: !digital && mode_str != "NFM",
        mode: if digital { 1 } else { 0 },
        tone,
        color_code: c.dmr_color_code.unwrap_or(1).clamp(0, 15) as u8,
        time_slot: ec
            .timeslot
            .or(c.dmr_timeslot)
            .unwrap_or(1)
            .clamp(1, 2) as u8,
        contact_index,
        // Full replace: every channel gets an explicit scan-list byte —
        // 0xFF (none) unless a planned scan list claims the slot below.
        scan_list_index: Some(0xFF),
    }
}

// ============================================================
// Assembly (payload → plan)
// ============================================================

/// Assemble the full program from an already-resolved codeplug payload. Pure:
/// no port access and no database, so it powers the preview and is testable
/// without either. Errors only on structural problems (wrong model, over
/// capacity); per-channel issues become `skipped` entries or `warnings`.
pub(crate) fn plan_program(payload: &CodeplugPayload) -> Result<AnytoneProgramPlan, String> {
    let model = payload.model;
    if model.model != "AT-D890UV" {
        return Err(format!(
            "direct programming is only wired up for the AT-D890UV (codeplug targets {})",
            model.display_name
        ));
    }

    // Channel pool is pre-deduped and DMR-expanded by the resolver: the
    // codeplug's lists in assignment order, first list winning the memory slots
    // (later lists still reference the same slots through `groups`).
    let groups = payload.groups;
    let expanded = payload.channels;

    let mut warnings: Vec<String> = Vec::new();
    let mut skipped: Vec<SkippedChannel> = Vec::new();
    let mut channels: Vec<PlannedChannel> = Vec::new();
    let mut contacts: Vec<PlannedContact> = Vec::new();
    let mut contact_by_tg: HashMap<i64, u16> = HashMap::new();
    // channel row id → every radio slot (0-based) it landed in, for zone/scan
    // membership (a DMR channel with N talkgroups owns N slots).
    let mut slots_by_channel: HashMap<i64, Vec<u16>> = HashMap::new();

    let max_slots = (model.memory_channels.unwrap_or(4000) as usize).min(NUM_BANKS * CH_PER_BANK);
    for ec in expanded {
        if let Some(reason) = exclusion_reason(&ec.channel, model) {
            skipped.push(SkippedChannel {
                name: channel_name(ec, model, 0),
                reason,
            });
            continue;
        }
        let slot = channels.len() + 1;
        if slot > max_slots {
            return Err(format!(
                "codeplug expands to more than {max_slots} channels — trim the channel lists"
            ));
        }

        // Contact index: distinct tg_numbers in first-use order.
        let contact_index = match ec.tg_number {
            Some(tg) => {
                let next = contact_by_tg.len() as u16;
                let label = match ec.tg_label.as_deref() {
                    Some(l) => l.to_string(),
                    None => format!("TG {tg}"),
                };
                *contact_by_tg.entry(tg).or_insert_with(|| {
                    contacts.push(PlannedContact {
                        index: next,
                        tg_number: tg as u32,
                        name: truncate(&label, NAME_CHARS),
                        call_type: match ec.tg_call_type.as_deref() {
                            Some(t) if t.eq_ignore_ascii_case("Private") => 0,
                            _ => 1,
                        },
                    });
                    next
                })
            }
            None => {
                let is_dmr = ec.channel.mode.as_deref() == Some("DMR");
                if is_dmr {
                    warnings.push(format!(
                        "{}: DMR channel has no talkgroup — programmed pointing at contact 1",
                        channel_name(ec, model, slot)
                    ));
                }
                0
            }
        };

        let edit = invert_channel(ec, model, slot, contact_index, &mut warnings);
        slots_by_channel
            .entry(ec.channel.id)
            .or_default()
            .push((slot - 1) as u16);
        channels.push(PlannedChannel { slot, edit });
    }

    if contacts.len() > MAX_CONTACTS_READ {
        return Err(format!(
            "codeplug references {} talkgroups (radio limit {MAX_CONTACTS_READ})",
            contacts.len()
        ));
    }
    // Zones: one per codeplug channel list, in assignment order.
    let mut zones: Vec<PlannedZone> = Vec::new();
    for g in groups {
        let mut members: Vec<u16> = g
            .channels
            .iter()
            .flat_map(|c| slots_by_channel.get(&c.id).cloned().unwrap_or_default())
            .collect();
        if members.is_empty() {
            warnings.push(format!(
                "list '{}' has no programmable channels — zone omitted",
                g.list_name
            ));
            continue;
        }
        if members.len() > ZONE_MAX_CHANNELS {
            warnings.push(format!(
                "zone '{}' has {} channels — truncated to the radio's {ZONE_MAX_CHANNELS}",
                g.list_name,
                members.len()
            ));
            members.truncate(ZONE_MAX_CHANNELS);
        }
        if g.list_name.chars().count() > ZONE_NAME_CHARS {
            warnings.push(format!(
                "zone name '{}' is longer than {ZONE_NAME_CHARS} characters — truncated",
                g.list_name
            ));
        }
        zones.push(PlannedZone {
            name: truncate(&g.list_name, ZONE_NAME_CHARS),
            member_slots: members,
        });
    }
    if zones.len() > MAX_ZONES {
        warnings.push(format!(
            "codeplug has {} zones — only the first {MAX_ZONES} are programmed",
            zones.len()
        ));
        zones.truncate(MAX_ZONES);
    }

    // Scan lists assigned to the codeplug (resolver hands them over in id order
    // — `codeplug_scan_lists` has no position column).
    let mut scan_lists: Vec<PlannedScanList> = Vec::new();
    // DB scan_list id for each planned list, same order/index as `scan_lists`,
    // so per-channel overrides (keyed by DB id) can resolve to a slot byte.
    let mut scan_list_ids: Vec<i64> = Vec::new();
    // Resolve a priority channel DB id to its first radio slot in THIS codeplug.
    // Returns None (=> 0xFFFF) when the channel isn't programmed here.
    let resolve_priority = |id: Option<i64>| -> Option<u16> {
        id.and_then(|i| slots_by_channel.get(&i))
            .and_then(|v| v.first().copied())
    };
    for sl in payload.scan_lists {
        let (list_id, list_name) = (sl.id, sl.name.as_str());
        let (prio1_id, prio2_id) = (sl.priority_channel_id, sl.priority_channel_2_id);
        let (lb_a, lb_b) = (sl.look_back_a, sl.look_back_b);
        let (dropout, dwell, revert) = (sl.dropout_delay, sl.dwell_time, sl.revert_channel);
        let mut priority_select = sl.priority_select;
        let mut members: Vec<u16> = sl
            .member_channel_ids
            .iter()
            .flat_map(|id| slots_by_channel.get(id).cloned().unwrap_or_default())
            .collect();
        if members.is_empty() {
            warnings.push(format!(
                "scan list '{list_name}' has no channels in this codeplug — omitted"
            ));
            continue;
        }
        if members.len() > SCAN_MAX_CHANNELS {
            warnings.push(format!(
                "scan list '{list_name}' has {} channels — truncated to the radio's {SCAN_MAX_CHANNELS}",
                members.len()
            ));
            members.truncate(SCAN_MAX_CHANNELS);
        }
        // Resolve priority channels to slots; if the select byte enables a
        // priority whose channel isn't in this codeplug, downgrade + warn rather
        // than emit an enabled-but-unset priority.
        let prio1_slot = resolve_priority(prio1_id);
        let prio2_slot = resolve_priority(prio2_id);
        if (priority_select == 1 || priority_select == 3) && prio1_id.is_some() && prio1_slot.is_none()
        {
            warnings.push(format!(
                "scan list '{list_name}': priority channel 1 isn't in this codeplug — priority disabled for it"
            ));
            priority_select = if priority_select == 3 { 2 } else { 0 };
        }
        if (priority_select == 2 || priority_select == 3) && prio2_id.is_some() && prio2_slot.is_none()
        {
            warnings.push(format!(
                "scan list '{list_name}': priority channel 2 isn't in this codeplug — priority disabled for it"
            ));
            priority_select = if priority_select == 3 { 1 } else { 0 };
        }
        let settings = ScanListRecordSettings {
            priority_select: priority_select.clamp(0, 3) as u8,
            priority_slot_1: prio1_slot,
            priority_slot_2: prio2_slot,
            look_back_a: lb_a.clamp(0, u16::MAX as i64) as u16,
            look_back_b: lb_b.clamp(0, u16::MAX as i64) as u16,
            dropout_delay: dropout.clamp(0, u16::MAX as i64) as u16,
            dwell_time: dwell.clamp(0, u16::MAX as i64) as u16,
            revert_channel: revert.clamp(0, 7) as u8,
        };
        scan_lists.push(PlannedScanList {
            name: truncate(list_name, NAME_CHARS),
            member_slots: members,
            settings,
        });
        scan_list_ids.push(list_id);
    }
    if scan_lists.len() > MAX_SCAN_LISTS {
        warnings.push(format!(
            "codeplug has {} scan lists — only the first {MAX_SCAN_LISTS} are programmed",
            scan_lists.len()
        ));
        scan_lists.truncate(MAX_SCAN_LISTS);
        scan_list_ids.truncate(MAX_SCAN_LISTS);
    }
    // DB scan_list id → planned slot byte (index into `scan_lists`).
    let index_by_list_id: HashMap<i64, u8> = scan_list_ids
        .iter()
        .enumerate()
        .map(|(si, &id)| (id, si as u8))
        .collect();

    // The channel's scan-list byte (radio channel field 0x1B) comes from ONE
    // place: the explicit per-channel assignment in `codeplug_channel_scan_lists`.
    // Membership in a scan list does NOT imply the byte — a channel can be
    // scanned in list X without launching it, and can launch X without being
    // scanned in it. A channel with no assignment programs as 0xFF (none).
    //
    // Earlier builds derived the byte from membership when a list had no explicit
    // assignments, which made the two concepts fight: derivation drowned out a
    // dedicated launch channel (s52), and the per-list rule added to stop that
    // wiped the byte off every other channel (issue #21). Neither can happen now.
    //
    // An assignment to a list not planned for this codeplug (unassigned/empty/
    // over-cap) is ignored with a warning.
    for &crate::commands::export::ChannelScanListOverride {
        channel_id,
        scan_list_id: list_id,
    } in payload.scan_list_overrides
    {
        let Some(slots) = slots_by_channel.get(&channel_id) else {
            continue; // channel isn't in this codeplug's programmed set
        };
        match index_by_list_id.get(&list_id) {
            Some(&si) => {
                for &slot0 in slots {
                    channels[slot0 as usize].edit.scan_list_index = Some(si);
                }
            }
            None => warnings.push(
                "a channel is assigned to a scan list that isn't programmed in this codeplug — assignment ignored"
                    .to_string(),
            ),
        }
    }
    // The byte is assignment-only, so a codeplug can carry scan lists that no
    // channel launches. That is legal but rarely intended — surface it before
    // the write rather than after.
    if !scan_lists.is_empty() {
        let unassigned = channels
            .iter()
            .filter(|c| c.edit.scan_list_index == Some(0xFF))
            .count();
        if unassigned > 0 {
            warnings.push(format!(
                "{unassigned} of {} channels have no scan list assigned — pressing Scan on them does nothing",
                channels.len()
            ));
        }
    }

    Ok(AnytoneProgramPlan {
        radio: model.display_name.clone(),
        channels,
        zones,
        scan_lists,
        contacts,
        skipped,
        warnings,
    })
}

// ============================================================
// Hardware session (reads → patched windows → backup → write → END)
// ============================================================

/// Counters gathered while computing the patched windows.
#[derive(Default)]
struct ClearCounts {
    slots: usize,
    zones: usize,
    scan_lists: usize,
    contacts: usize,
}

/// Read the channel banks this program touches and build their patched images:
/// slot 1..N encoded (RMW onto the existing record, or onto the capture-anchored
/// template when the slot was empty), every other previously-programmed slot in
/// a touched bank 0xFF-cleared. Banks past both the plan and the radio's last
/// programmed bank are left alone (they are already empty).
fn build_channel_bank_plans(
    p: &mut dyn SerialPort,
    channels: &[PlannedChannel],
    cleared: &mut ClearCounts,
) -> Result<Vec<BankPlan>, String> {
    let banks_needed = channels.len().div_ceil(CH_PER_BANK);
    let mut plans = Vec::new();
    for bank in 0..NUM_BANKS {
        let base = CHANNEL_BASE + (bank as u32) * BANK_STEP;
        let original = read_block(p, base, WRITE_WINDOW)?;
        let beyond_plan = bank >= banks_needed;
        if beyond_plan && original.iter().all(|&b| b == 0xFF) {
            // First fully-empty bank past the plan ends the radio's sequential
            // channel allocation — nothing further to clear.
            break;
        }
        if let Some(patched) = patch_channel_bank(&original, bank, channels, cleared)? {
            plans.push(BankPlan {
                base,
                original,
                patched,
            });
        }
    }
    Ok(plans)
}

/// Pure per-bank patcher behind [`build_channel_bank_plans`]: encode the plan's
/// slots onto the bank's original bytes (template for previously-empty slots,
/// RMW otherwise) and 0xFF-clear every stale slot. `None` when nothing in the
/// bank changes.
fn patch_channel_bank(
    original: &[u8],
    bank: usize,
    channels: &[PlannedChannel],
    cleared: &mut ClearCounts,
) -> Result<Option<Vec<u8>>, String> {
    let mut patched = original.to_vec();
    for i in 0..CH_PER_BANK {
        let slot0 = bank * CH_PER_BANK + i;
        let off = i * CH_REC_LEN;
        let rec_orig = &original[off..off + CH_REC_LEN];
        if slot0 < channels.len() {
            debug_assert_eq!(channels[slot0].slot, slot0 + 1, "plan slots must be packed");
            let mut rec = if is_empty_record(rec_orig) {
                new_channel_record_template()
            } else {
                rec_orig.to_vec()
            };
            encode_channel_into_record(&mut rec, &channels[slot0].edit)?;
            patched[off..off + CH_REC_LEN].copy_from_slice(&rec);
        } else if !is_empty_record(rec_orig) {
            // Full replace: blank the stale slot (0xFF = the radio's own
            // empty pattern).
            patched[off..off + CH_REC_LEN].fill(0xFF);
            cleared.slots += 1;
        }
    }
    Ok((patched != original).then_some(patched))
}

/// Read the 0x4000 windows of a record region sequentially, stopping after the
/// first window that is both past `needed_bytes` and entirely 0xFF (the flash
/// empty pattern) — that window still gets kept so patches may spill into it.
fn read_region_windows(
    p: &mut dyn SerialPort,
    base: u32,
    max_windows: usize,
    needed_bytes: usize,
) -> Result<std::collections::BTreeMap<u32, Vec<u8>>, String> {
    let mut out = std::collections::BTreeMap::new();
    for w in 0..max_windows {
        let addr = base + (w * WRITE_WINDOW) as u32;
        let data = read_block(p, addr, WRITE_WINDOW)?;
        let all_ff = data.iter().all(|&b| b == 0xFF);
        out.insert(addr, data);
        if all_ff && (w + 1) * WRITE_WINDOW >= needed_bytes {
            break;
        }
    }
    Ok(out)
}

/// Highest 1-based zone number with a non-empty name in the zone-name window
/// (a zone's presence IS its non-empty name — there is no bitmap).
fn zone_count_in(name_window: &[u8]) -> usize {
    (0..MAX_ZONES)
        .rev()
        .find(|&i| {
            let off = i * ZONE_NAME_STEP as usize;
            !decode_utf16_name(
                &name_window[off..off + ZONE_NAME_STEP as usize],
                ZONE_NAME_CHARS,
            )
            .is_empty()
        })
        .map_or(0, |i| i + 1)
}

/// A present-bitmap for `n` records: bits `0..n` set (LSB = record 1), all
/// higher bits cleared so stale records are removed. Shared by the zone bitmap
/// at `ZONE_BITMAP_BASE` and the scan-list bitmap at `SCAN_BITMAP_BASE`.
fn present_bitmap(n: usize, bytes: usize) -> Vec<u8> {
    let mut bitmap = vec![0u8; bytes];
    for z in 0..n.min(bytes * 8) {
        bitmap[z / 8] |= 1 << (z % 8);
    }
    bitmap
}

/// Highest 1-based scan-list number with a non-empty record across the read
/// windows (record-aligned: 0x200 divides the window size).
fn scan_count_in(windows: &std::collections::BTreeMap<u32, Vec<u8>>) -> usize {
    windows
        .iter()
        .flat_map(|(addr, data)| {
            let first = ((addr - SCAN_LIST_BASE) as usize) / SCAN_LIST_STEP as usize;
            data.chunks_exact(SCAN_LIST_STEP as usize)
                .enumerate()
                .filter(|(_, rec)| !is_empty_record(rec))
                .map(move |(i, _)| first + i + 1)
                .collect::<Vec<_>>()
        })
        .max()
        .unwrap_or(0)
        .min(MAX_SCAN_LISTS)
}

/// Highest 1-based contact number with a non-empty record in the concatenated
/// contact-region bytes.
fn contact_count_in(bytes: &[u8]) -> usize {
    (0..bytes.len() / CONTACT_REC_LEN)
        .rev()
        .find(|&i| !is_empty_record(&bytes[i * CONTACT_REC_LEN..(i + 1) * CONTACT_REC_LEN]))
        .map_or(0, |i| i + 1)
}

/// The blocking hardware session shared by `program_anytone_codeplug`.
fn run_program(
    port: &str,
    plan: &AnytoneProgramPlan,
    backup_path: &std::path::Path,
    expected_path: &std::path::Path,
) -> Result<ProgramReport, String> {
    let mut p = open_port(port)?;
    let _ident = enter_program_and_ident(&mut *p)?;
    let mut cleared = ClearCounts::default();

    // ---- Phase 1: reads + patched-window computation (no writes yet). Any
    // error here ends the session cleanly, leaving the radio untouched.
    let phase1 = (|| -> Result<Vec<BankPlan>, String> {
        let mut all_plans = build_channel_bank_plans(&mut *p, &plan.channels, &mut cleared)?;

        // Zone names all live in ONE window (250 × 0x40 = 0x3E80 < 0x4000).
        let name_window = read_block(&mut *p, ZONE_NAME_BASE, WRITE_WINDOW)?;
        let old_zone_count = zone_count_in(&name_window);
        cleared.zones = old_zone_count.saturating_sub(plan.zones.len());

        let mut patches: Vec<RegionPatch> = Vec::new();
        for (i, z) in plan.zones.iter().enumerate() {
            patches.push(RegionPatch {
                addr: ZONE_NAME_BASE + (i as u32) * ZONE_NAME_STEP,
                data: encode_utf16_field(&z.name, ZONE_NAME_CHARS),
            });
            patches.push(RegionPatch {
                addr: ZONE_LIST_BASE + (i as u32) * ZONE_LIST_STEP,
                data: encode_zone_list_record(&z.member_slots)?,
            });
        }
        // Clear stale zones: blank the name and 0xFF the member list. The
        // zone-present bitmap below is what actually removes them from the radio;
        // this keeps the name/list arrays tidy (belt-and-suspenders).
        for i in plan.zones.len()..old_zone_count {
            patches.push(RegionPatch {
                addr: ZONE_NAME_BASE + (i as u32) * ZONE_NAME_STEP,
                data: vec![0u8; ZONE_NAME_STEP as usize],
            });
            patches.push(RegionPatch {
                addr: ZONE_LIST_BASE + (i as u32) * ZONE_LIST_STEP,
                data: vec![0xFFu8; ZONE_LIST_STEP as usize],
            });
        }

        // Zone-present bitmap (0x03482C00): the radio shows a zone iff its bit is
        // set here (1 bit/zone, LSB = zone 1). Write the WHOLE bitmap every
        // program — set bits for the planned zones and clear all others — so
        // stale bits from a previous codeplug can't leave phantom zones and every
        // planned zone is activated. (HW-verified 2026-07-09.)
        patches.push(RegionPatch {
            addr: ZONE_BITMAP_BASE,
            data: present_bitmap(plan.zones.len(), ZONE_BITMAP_BYTES),
        });
        // Scan lists are gated the same way (SCAN_BITMAP_BASE). Without this the
        // records land and the radio still shows whatever the stale bits say —
        // three programmed lists reading back as one.
        patches.push(RegionPatch {
            addr: SCAN_BITMAP_BASE,
            data: present_bitmap(plan.scan_lists.len(), SCAN_BITMAP_BYTES),
        });

        let mut originals = std::collections::BTreeMap::new();
        originals.insert(ZONE_NAME_BASE, name_window);
        // Both present-bitmaps live in the same settings window; read it for
        // the RMW/backup.
        let bitmap_window = ZONE_BITMAP_BASE & !(WRITE_WINDOW as u32 - 1);
        originals.insert(bitmap_window, read_block(&mut *p, bitmap_window, WRITE_WINDOW)?);
        // Zone list windows covering every zone written or cleared.
        let zones_touched = plan.zones.len().max(old_zone_count);
        let zone_list_windows = (zones_touched * ZONE_LIST_STEP as usize).div_ceil(WRITE_WINDOW);
        for w in 0..zone_list_windows {
            let addr = ZONE_LIST_BASE + (w * WRITE_WINDOW) as u32;
            originals.insert(addr, read_block(&mut *p, addr, WRITE_WINDOW)?);
        }

        // Scan lists: read far enough to cover the plan and any stale records.
        let scan_region = MAX_SCAN_LISTS * SCAN_LIST_STEP as usize;
        let scan_windows = read_region_windows(
            &mut *p,
            SCAN_LIST_BASE,
            scan_region.div_ceil(WRITE_WINDOW),
            plan.scan_lists.len() * SCAN_LIST_STEP as usize,
        )?;
        let old_scan_count = scan_count_in(&scan_windows);
        cleared.scan_lists = old_scan_count.saturating_sub(plan.scan_lists.len());
        for (i, sl) in plan.scan_lists.iter().enumerate() {
            patches.push(RegionPatch {
                addr: SCAN_LIST_BASE + (i as u32) * SCAN_LIST_STEP,
                data: encode_scan_list_record(&sl.name, &sl.member_slots, &sl.settings)?,
            });
        }
        for i in plan.scan_lists.len()..old_scan_count {
            patches.push(RegionPatch {
                addr: SCAN_LIST_BASE + (i as u32) * SCAN_LIST_STEP,
                data: vec![0xFFu8; SCAN_LIST_STEP as usize],
            });
        }
        originals.extend(scan_windows);

        // Contacts: same sequential-read discipline (records straddle windows;
        // the stop rule keeps the window after the last stale record in hand).
        let contact_region = MAX_CONTACTS_READ * CONTACT_REC_LEN;
        let contact_windows = read_region_windows(
            &mut *p,
            CONTACT_BASE,
            contact_region.div_ceil(WRITE_WINDOW),
            plan.contacts.len() * CONTACT_REC_LEN,
        )?;
        let contact_bytes: Vec<u8> = contact_windows.values().flatten().copied().collect();
        let old_contact_count = contact_count_in(&contact_bytes);
        cleared.contacts = old_contact_count.saturating_sub(plan.contacts.len());
        for c in &plan.contacts {
            patches.push(RegionPatch {
                addr: CONTACT_BASE + (c.index as usize * CONTACT_REC_LEN) as u32,
                data: encode_contact_record(c.call_type, c.tg_number, &c.name)?,
            });
        }
        for i in plan.contacts.len()..old_contact_count {
            patches.push(RegionPatch {
                addr: CONTACT_BASE + (i * CONTACT_REC_LEN) as u32,
                data: vec![0xFFu8; CONTACT_REC_LEN],
            });
        }
        originals.extend(contact_windows);

        all_plans.extend(plans_from_pre_read(&originals, &patches)?);
        Ok(all_plans)
    })();
    let all_plans = match phase1 {
        Ok(v) => v,
        Err(e) => {
            let _ = end_session(&mut *p);
            return Err(e);
        }
    };

    // ---- Backup (originals) + expected image (patched) BEFORE any write.
    let to_blocks = |f: fn(&BankPlan) -> &Vec<u8>| -> Vec<(u32, Vec<u8>)> {
        all_plans.iter().map(|pl| (pl.base, f(pl).clone())).collect()
    };
    let backup = serialize_backup(&to_blocks(|pl| &pl.original));
    let expected = serialize_backup(&to_blocks(|pl| &pl.patched));
    for (path, bytes) in [(backup_path, &backup), (expected_path, &expected)] {
        if let Err(e) = std::fs::write(path, bytes) {
            let _ = end_session(&mut *p);
            return Err(format!("could not write {}: {e}", path.display()));
        }
    }

    // ---- Phase 2: write every changed window, then END once (commit+reboot).
    // A write error deliberately does NOT end the session (ENDing would commit
    // a half-written image); the message tells the user to restore.
    let written = commit_channel_writes(&mut *p, &all_plans)?;
    let _ = end_session(&mut *p);

    Ok(ProgramReport {
        channels_written: plan.channels.len(),
        slots_cleared: cleared.slots,
        zones_written: plan.zones.len(),
        zones_cleared: cleared.zones,
        scan_lists_written: plan.scan_lists.len(),
        scan_lists_cleared: cleared.scan_lists,
        contacts_written: plan.contacts.len(),
        contacts_cleared: cleared.contacts,
        windows_written: written.iter().map(|a| format!("0x{a:08X}")).collect(),
        backup_path: backup_path.to_string_lossy().to_string(),
        expected_path: expected_path.to_string_lossy().to_string(),
        warnings: plan.warnings.clone(),
        note: "Written and committed — the radio reboots and re-enumerates USB. \
               Rescan, reselect the port, then run Verify against the expected image."
            .to_string(),
    })
}

// ============================================================
// Capability impl
// ============================================================

impl CodeplugProgrammer for super::AnytoneAtd890uv {
    fn preview(&self, payload: &CodeplugPayload) -> Result<CodeplugPreview, String> {
        Ok(plan_program(payload)?.preview())
    }

    fn program(
        &self,
        port: &str,
        payload: &CodeplugPayload,
        backup_dir: &Path,
    ) -> Result<ProgramReport, String> {
        let plan = plan_program(payload)?;
        if plan.channels.is_empty() {
            return Err(
                "this codeplug has no programmable channels — add channel lists (or check the \
                 preview's skipped reasons)"
                    .to_string(),
            );
        }
        // Backup + expected image land beside each other under a shared stamp;
        // `latest_anytone_program` finds the pair by the `anytone-program-`
        // prefix so Verify/Restore stay reachable if the commit reboot drops the
        // IPC response.
        std::fs::create_dir_all(backup_dir).map_err(|e| e.to_string())?;
        let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
        let backup_path = backup_dir.join(format!("anytone-program-{stamp}.bin"));
        let expected_path = backup_dir.join(format!("anytone-program-{stamp}.expected.bin"));
        run_program(port, &plan, &backup_path, &expected_path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::export::resolve_codeplug_payload;
    use crate::commands::import::map_anytone_tone;
    use crate::radios::anytone_atd890uv::{diff_dumps, parse_dump};

    /// Resolve a codeplug from the DB and plan it — what `build_anytone_program`
    /// used to do in one call, now the two halves 3.6b split apart.
    async fn build_anytone_program(
        pool: &sqlx::SqlitePool,
        codeplug_id: i64,
    ) -> Result<AnytoneProgramPlan, String> {
        let resolved = resolve_codeplug_payload(pool, codeplug_id).await?;
        plan_program(&resolved.payload())
    }

    // --- tone inversion ---------------------------------------------------

    /// `invert_tone` must be the exact inverse of the import's
    /// `map_anytone_tone` for every branch of its truth table: radio sub-tones
    /// → CHIRP columns → back to the same sub-tones, warning-free.
    #[test]
    fn tone_inversion_round_trips_the_import_truth_table() {
        use AnytoneSubTone::{Ctcss, Dcs};
        let cases: Vec<(Option<AnytoneSubTone>, Option<AnytoneSubTone>)> = vec![
            (None, None),
            (Some(Ctcss(100.0)), None),                  // Tone
            (Some(Ctcss(100.0)), Some(Ctcss(100.0))),    // TSQL
            (Some(Ctcss(88.5)), Some(Ctcss(123.0))),     // Cross Tone->Tone
            (None, Some(Ctcss(123.0))),                  // Cross ->Tone
            (Some(Dcs(23)), Some(Dcs(23))),              // DTCS
            (Some(Dcs(23)), Some(Dcs(47))),              // Cross DTCS->DTCS
            (Some(Dcs(23)), None),                       // Cross DTCS->
            (None, Some(Dcs(23))),                       // Cross ->DTCS
            (Some(Ctcss(100.0)), Some(Dcs(23))),         // Cross Tone->DTCS
            (Some(Dcs(23)), Some(Ctcss(100.0))),         // Cross DTCS->Tone
        ];
        for (tx, rx) in cases {
            let (mode, up, down, dcs_tx, dcs_rx, cross) = map_anytone_tone(&tx, &rx);
            let (tone, warnings) = invert_tone(
                Some(&mode),
                &cross,
                up,
                down,
                dcs_tx.as_deref(),
                dcs_rx.as_deref(),
            );
            assert!(warnings.is_empty(), "unexpected warnings for {tx:?}/{rx:?}: {warnings:?}");
            assert_eq!(tone.tx, tx, "TX side diverged for mode {mode}");
            assert_eq!(tone.rx, rx, "RX side diverged for mode {mode}");
        }
    }

    #[test]
    fn tone_inversion_degrades_bad_input_with_warnings() {
        // Off-table CTCSS: that side dropped, warned.
        let (tone, w) = invert_tone(Some("Tone"), "Tone->Tone", Some(101.0), None, None, None);
        assert_eq!((tone.tx, tone.rx), (None, None));
        assert_eq!(w.len(), 1);
        // Non-numeric DCS: dropped, warned.
        let (tone, w) = invert_tone(Some("DTCS"), "Tone->Tone", None, None, Some("D23N"), None);
        assert_eq!((tone.tx, tone.rx), (None, None));
        assert!(!w.is_empty());
        // DCS codes are octal: digits 8/9 are invalid → dropped, warned.
        let (tone, w) = invert_tone(Some("DTCS"), "Tone->Tone", None, None, Some("298"), None);
        assert_eq!((tone.tx, tone.rx), (None, None));
        assert!(!w.is_empty());
        // Octal parse: stored "411" (the printed code) → raw 265 on the radio.
        let (tone, w) = invert_tone(Some("DTCS"), "Tone->Tone", None, None, Some("411"), None);
        assert_eq!(tone.tx, Some(AnytoneSubTone::Dcs(265)));
        assert!(w.is_empty());
        // Unknown tone mode: off + warned.
        let (tone, w) = invert_tone(Some("Sepulcher"), "Tone->Tone", None, None, None, None);
        assert_eq!((tone.tx, tone.rx), (None, None));
        assert_eq!(w.len(), 1);
        // Plain off: silent.
        let (tone, w) = invert_tone(None, "Tone->Tone", Some(100.0), None, None, None);
        assert_eq!((tone.tx, tone.rx), (None, None));
        assert!(w.is_empty());
    }

    #[test]
    fn power_inversion_matches_import_levels() {
        let mut w = Vec::new();
        assert_eq!(invert_power(Some("Low"), &mut w, "X"), 0);
        assert_eq!(invert_power(Some("Med"), &mut w, "X"), 1);
        assert_eq!(invert_power(Some("High"), &mut w, "X"), 2);
        assert_eq!(invert_power(None, &mut w, "X"), 3); // NULL = Turbo
        assert!(w.is_empty());
        assert_eq!(invert_power(Some("50W"), &mut w, "X"), 2); // unknown → High + warn
        assert_eq!(w.len(), 1);
    }

    // --- bank patching / clears -------------------------------------------

    fn planned(slot: usize, name: &str, digital: bool) -> PlannedChannel {
        PlannedChannel {
            slot,
            edit: AnytoneChannelEdit {
                name: name.to_string(),
                rx_mhz: 446.0,
                tx_mhz: 446.0,
                power: 2,
                wide: !digital,
                mode: if digital { 1 } else { 0 },
                tone: AnytoneToneEdit::default(),
                color_code: if digital { 7 } else { 0 },
                time_slot: 1,
                contact_index: 0,
                scan_list_index: Some(0xFF),
            },
        }
    }

    #[test]
    fn bank_patch_writes_slots_and_clears_stale_records() {
        // Original bank: slot 1 programmed (will be overwritten), slot 3 stale
        // (must be 0xFF-cleared), slot 2 empty (gets the template).
        let mut original = vec![0xFFu8; WRITE_WINDOW];
        original[0..CH_REC_LEN].fill(0x55); // slot 1: existing bytes
        original[2 * CH_REC_LEN..3 * CH_REC_LEN].fill(0x66); // slot 3: stale
        let channels = vec![planned(1, "ONE", false), planned(2, "TWO", true)];
        let mut cleared = ClearCounts::default();
        let patched = patch_channel_bank(&original, 0, &channels, &mut cleared)
            .unwrap()
            .expect("bank changes");
        assert_eq!(cleared.slots, 1);
        // Slot 1 RMW onto the 0x55 pattern: unmodeled byte 0x30 preserved.
        assert_eq!(patched[0x30], 0x55);
        // Slot 2 was empty → template defaults (0x1C RX group list = none).
        assert_eq!(patched[CH_REC_LEN + 0x1C], 0xFF);
        // Slot 3 cleared to the radio's empty pattern.
        assert!(patched[2 * CH_REC_LEN..3 * CH_REC_LEN].iter().all(|&b| b == 0xFF));
        // The rest of the bank is untouched.
        assert!(patched[3 * CH_REC_LEN..].iter().all(|&b| b == 0xFF));

        // A bank that needs no changes returns None.
        let empty_bank = vec![0xFFu8; WRITE_WINDOW];
        let mut cleared = ClearCounts::default();
        assert!(patch_channel_bank(&empty_bank, 1, &channels, &mut cleared)
            .unwrap()
            .is_none());
        assert_eq!(cleared.slots, 0);
    }

    // --- stale-count detection ---------------------------------------------

    #[test]
    fn stale_counts_detect_last_programmed_records() {
        // Zones: names at 0x40 stride; zone 3 named, 1 named, 2 blank → count 3.
        let mut names = vec![0u8; WRITE_WINDOW];
        let n1 = encode_utf16_field("ZONE ONE", ZONE_NAME_CHARS);
        names[..n1.len()].copy_from_slice(&n1);
        let off3 = 2 * ZONE_NAME_STEP as usize;
        names[off3..off3 + n1.len()].copy_from_slice(&n1);
        assert_eq!(zone_count_in(&names), 3);
        assert_eq!(zone_count_in(&vec![0xFFu8; WRITE_WINDOW]), 0);

        // Scan lists: record 33 (second window) non-empty → count 34.
        let mut windows = std::collections::BTreeMap::new();
        windows.insert(SCAN_LIST_BASE, vec![0xFFu8; WRITE_WINDOW]);
        let mut w1 = vec![0xFFu8; WRITE_WINDOW];
        w1[SCAN_LIST_STEP as usize..2 * SCAN_LIST_STEP as usize]
            .copy_from_slice(&encode_scan_list_record("STALE", &[1], &Default::default()).unwrap());
        windows.insert(SCAN_LIST_BASE + WRITE_WINDOW as u32, w1);
        assert_eq!(scan_count_in(&windows), 34);

        // Contacts: record 2 non-empty → count 3.
        let mut contacts = vec![0xFFu8; 4 * CONTACT_REC_LEN];
        contacts[2 * CONTACT_REC_LEN..3 * CONTACT_REC_LEN]
            .copy_from_slice(&encode_contact_record(1, 700, "RMH WIDE").unwrap());
        assert_eq!(contact_count_in(&contacts), 3);
        assert_eq!(contact_count_in(&vec![0xFFu8; 2 * CONTACT_REC_LEN]), 0);
    }

    /// The bitmaps sit in a run of 32-byte fields and were confused once
    /// already: writing the scan-list count into the radio-ID bitmap made CPS
    /// fail its read. Keep them distinct and non-overlapping.
    #[test]
    fn scan_and_radio_id_bitmaps_do_not_overlap() {
        use crate::radios::anytone_atd890uv::settings::RADIO_ID_BITMAP_BASE;
        assert_ne!(SCAN_BITMAP_BASE, RADIO_ID_BITMAP_BASE);
        assert!(SCAN_BITMAP_BASE >= RADIO_ID_BITMAP_BASE + 32);
        assert!(SCAN_BITMAP_BASE >= ZONE_BITMAP_BASE + ZONE_BITMAP_BYTES as u32);
    }

    #[test]
    fn present_bitmap_sets_low_bits_and_clears_the_rest() {
        // Full width, LSB = zone 1.
        assert_eq!(present_bitmap(0, ZONE_BITMAP_BYTES), vec![0u8; ZONE_BITMAP_BYTES]);
        // 11 zones → bits 0..11: 0xFF (z1-8) then 0x07 (z9-11), rest zero.
        let b = present_bitmap(11, ZONE_BITMAP_BYTES);
        assert_eq!(b.len(), ZONE_BITMAP_BYTES);
        assert_eq!(b[0], 0xFF);
        assert_eq!(b[1], 0x07);
        assert!(b[2..].iter().all(|&x| x == 0));
        // One zone → only bit 0 (matches the HW-observed 0x01 for a lone zone).
        assert_eq!(present_bitmap(1, ZONE_BITMAP_BYTES)[0], 0x01);
        // Adding a 2nd zone flips exactly bit 1 (the HW-observed 0x01→0x03).
        assert_eq!(present_bitmap(2, ZONE_BITMAP_BYTES)[0], 0x03);
        // Never overflows the fixed-width bitmap.
        assert_eq!(present_bitmap(MAX_ZONES, ZONE_BITMAP_BYTES).len(), ZONE_BITMAP_BYTES);
    }

    // --- window splicing + expected-file round-trip -------------------------

    #[test]
    fn pre_read_splice_builds_only_changed_windows_and_round_trips() {
        let mut originals = std::collections::BTreeMap::new();
        originals.insert(ZONE_LIST_BASE, vec![0xAAu8; WRITE_WINDOW]);
        originals.insert(ZONE_NAME_BASE, vec![0xBBu8; WRITE_WINDOW]);
        let patches = vec![
            RegionPatch { addr: ZONE_LIST_BASE + 0x10, data: vec![1, 2, 3] },
            // A no-op patch: identical bytes → the window is skipped entirely.
            RegionPatch { addr: ZONE_NAME_BASE, data: vec![0xBB; 4] },
        ];
        let plans = plans_from_pre_read(&originals, &patches).unwrap();
        assert_eq!(plans.len(), 1);
        assert_eq!(plans[0].base, ZONE_LIST_BASE);
        assert_eq!(&plans[0].patched[0x10..0x13], &[1, 2, 3]);
        assert_eq!(plans[0].patched[0x13], 0xAA);

        // Backup/expected files round-trip through parse_dump and diff clean
        // against themselves; original vs patched diffs at the real address.
        let backup = serialize_backup(
            &plans.iter().map(|pl| (pl.base, pl.original.clone())).collect::<Vec<_>>(),
        );
        let expected = serialize_backup(
            &plans.iter().map(|pl| (pl.base, pl.patched.clone())).collect::<Vec<_>>(),
        );
        let b = parse_dump(&backup).unwrap();
        let e = parse_dump(&expected).unwrap();
        assert!(diff_dumps(&e, &e).is_empty());
        let diffs = diff_dumps(&b, &e);
        assert_eq!(diffs.len(), 1);
        assert_eq!(diffs[0].addr, format!("0x{:08X}", ZONE_LIST_BASE + 0x10));

        // A patch into an unread window is a hard error, not a silent skip.
        let stray = vec![RegionPatch { addr: CONTACT_BASE, data: vec![1] }];
        assert!(plans_from_pre_read(&originals, &stray).is_err());
    }

    // --- full assembly against a real (in-memory) schema --------------------

    async fn fixture_pool(tag: &str) -> sqlx::SqlitePool {
        let dir = std::env::temp_dir().join(format!("cpm_prog_{tag}_{}", std::process::id()));
        let db_path = dir.join("test.sqlite3");
        let _ = std::fs::remove_file(&db_path);
        crate::db::init_pool(&db_path).await.expect("init_pool")
    }

    /// Wire a codeplug to the seeded AT-D890UV via a radio profile and return
    /// the codeplug id.
    async fn d890_codeplug(pool: &sqlx::SqlitePool) -> i64 {
        let (model_id,): (i64,) =
            sqlx::query_as("SELECT id FROM radio_models WHERE model = 'AT-D890UV'")
                .fetch_one(pool)
                .await
                .unwrap();
        sqlx::query("INSERT INTO radio_profiles (id, display_name, radio_model_id) VALUES (1, 'HT', ?1)")
            .bind(model_id)
            .execute(pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO codeplugs (id, name, radio_profile_id) VALUES (100, 'CP', 1)")
            .execute(pool)
            .await
            .unwrap();
        100
    }

    #[tokio::test]
    async fn assembles_slots_zones_contacts_with_tg_expansion_and_inline_fallback() {
        let pool = fixture_pool("asm").await;
        let cp = d890_codeplug(&pool).await;

        // Ch 1: DMR with TWO curated talkgroup assignments (expands to 2 slots).
        // Ch 2: radio-imported DMR with an INLINE talkgroup only (1 slot, keeps
        //       its name, TG resolved by number).
        // Ch 3: narrow analog with TSQL (1 slot).
        // Ch 4: out-of-band 900 MHz (skipped).
        sqlx::query(
            "INSERT INTO channels (id, name_long, name_short, rx_freq, offset, duplex, mode,
                                   tone_mode, ctcss_uplink, ctcss_downlink, dmr_color_code,
                                   dmr_timeslot, dmr_talkgroup, power, source)
             VALUES (1, 'W0XYZ Denver', 'W0XYZ', 449.0, 5.0, '+', 'DMR', NULL, NULL, NULL, 1, NULL, NULL, NULL, 'manual'),
                    (2, 'RMH700-FOCOBUCK', 'RMH700', 445.2, 5.0, '-', 'DMR', NULL, NULL, NULL, 7, 1, 99999700, NULL, 'radio'),
                    (3, 'K0FM Boulder', 'K0FM', 146.94, 0.6, '-', 'NFM', 'TSQL', 100.0, 100.0, NULL, NULL, NULL, 'High', 'manual'),
                    (4, 'FAR AWAY', 'FAR', 927.5, NULL, NULL, 'FM', NULL, NULL, NULL, NULL, NULL, NULL, NULL, 'manual')",
        )
        .execute(&pool)
        .await
        .unwrap();
        // Own talkgroups on a private test network (ids auto-assigned so the
        // seeded Brandmeister list can't collide); the inline TG uses a number
        // no seed entry has, so the by-number lookup resolves to OUR name.
        sqlx::query(
            "INSERT INTO talkgroups (tg_number, name, network, call_type, source)
             VALUES (3108, 'Colorado', 'TestNet', 'Group', 'manual'),
                    (91, 'Worldwide', 'TestNet', 'Group', 'manual'),
                    (99999700, 'RMH WIDE', 'TestNet', 'Group', 'manual')",
        )
        .execute(&pool)
        .await
        .unwrap();
        let co: (i64,) = sqlx::query_as(
            "SELECT id FROM talkgroups WHERE tg_number = 3108 AND network = 'TestNet'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        let ww: (i64,) = sqlx::query_as(
            "SELECT id FROM talkgroups WHERE tg_number = 91 AND network = 'TestNet'",
        )
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
        sqlx::query("INSERT INTO channel_lists (id, name) VALUES (10, 'NOCO DMR')")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO channel_list_entries (channel_list_id, channel_id, position)
             VALUES (10, 1, 0), (10, 2, 1), (10, 3, 2), (10, 4, 3)",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO codeplug_channel_lists (codeplug_id, channel_list_id, position)
             VALUES (100, 10, 0)",
        )
        .execute(&pool)
        .await
        .unwrap();
        // A scan list containing channels 2 and 3.
        sqlx::query("INSERT INTO scan_lists (id, name) VALUES (20, 'NOCO SCAN')")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO scan_list_entries (scan_list_id, channel_id, position)
             VALUES (20, 2, 0), (20, 3, 1)",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query("INSERT INTO codeplug_scan_lists (codeplug_id, scan_list_id) VALUES (100, 20)")
            .execute(&pool)
            .await
            .unwrap();

        let plan = build_anytone_program(&pool, cp).await.expect("assemble");

        // 2 (ch1 × TGs) + 1 (ch2 inline) + 1 (ch3) = 4 slots, packed 1..=4.
        assert_eq!(plan.channels.len(), 4);
        assert_eq!(
            plan.channels.iter().map(|c| c.slot).collect::<Vec<_>>(),
            vec![1, 2, 3, 4]
        );
        // Curated expansion appends the TG label; timeslot from the assignment.
        assert_eq!(plan.channels[0].edit.name, "W0XYZ Denver Col");
        assert_eq!(plan.channels[0].edit.time_slot, 2);
        assert_eq!(plan.channels[0].edit.mode, 1);
        assert!((plan.channels[0].edit.tx_mhz - 454.0).abs() < 1e-9);
        assert_eq!(plan.channels[1].edit.name, "W0XYZ Denver Wor");
        assert_eq!(plan.channels[1].edit.time_slot, 1);
        // Inline fallback keeps the original name and its stored TS/CC.
        assert_eq!(plan.channels[2].edit.name, "RMH700-FOCOBUCK");
        assert_eq!(plan.channels[2].edit.time_slot, 1);
        assert_eq!(plan.channels[2].edit.color_code, 7);
        // NFM analog → narrow, High power, TSQL both sides.
        let analog = &plan.channels[3].edit;
        assert!(!analog.wide);
        assert_eq!(analog.power, 2);
        assert_eq!(analog.tone.tx, Some(AnytoneSubTone::Ctcss(100.0)));
        assert_eq!(analog.tone.rx, Some(AnytoneSubTone::Ctcss(100.0)));

        // Contacts in first-use order: 3108, 91, then the inline 700 (resolved
        // to its library name by number).
        assert_eq!(
            plan.contacts
                .iter()
                .map(|c| (c.index, c.tg_number, c.name.as_str()))
                .collect::<Vec<_>>(),
            vec![(0, 3108, "Colorado"), (1, 91, "Worldwide"), (2, 99999700, "RMH WIDE")]
        );
        // Channel contact indices point at those assignments.
        assert_eq!(plan.channels[0].edit.contact_index, 0);
        assert_eq!(plan.channels[1].edit.contact_index, 1);
        assert_eq!(plan.channels[2].edit.contact_index, 2);

        // One zone from the one list, holding every programmed slot (0-based).
        assert_eq!(plan.zones.len(), 1);
        assert_eq!(plan.zones[0].name, "NOCO DMR");
        assert_eq!(plan.zones[0].member_slots, vec![0, 1, 2, 3]);

        // Scan list: ch2's slot (2) + ch3's slot (3). Membership alone never
        // sets the per-channel byte, so with no explicit assignments every
        // channel programs as 0xFF (none).
        assert_eq!(plan.scan_lists.len(), 1);
        assert_eq!(plan.scan_lists[0].member_slots, vec![2, 3]);
        assert!(plan
            .channels
            .iter()
            .all(|c| c.edit.scan_list_index == Some(0xFF)));

        // The 900 MHz channel is skipped with a frequency reason.
        assert_eq!(plan.skipped.len(), 1);
        assert!(plan.skipped[0].reason.contains("outside"));

        // Preview mirrors the plan.
        let preview = plan.preview();
        assert_eq!(
            (preview.channels, preview.zones, preview.scan_lists, preview.contacts),
            (4, 1, 1, 3)
        );
    }

    /// The per-channel scan-list assignment (radio byte 0x1B) is the only source
    /// of the byte: it applies whether or not the channel is a member of the
    /// target list, and an unassigned channel gets none even when it is a member
    /// of a programmed list.
    #[tokio::test]
    async fn explicit_channel_scan_list_overrides_membership() {
        let pool = fixture_pool("ovr").await;
        let cp = d890_codeplug(&pool).await;

        // Three plain analog channels, one memory slot each (slots 1,2,3).
        sqlx::query(
            "INSERT INTO channels (id, name_long, rx_freq, mode, source)
             VALUES (1, 'ALPHA', 146.52, 'FM', 'manual'),
                    (2, 'BRAVO', 146.55, 'FM', 'manual'),
                    (3, 'CHARLIE', 146.58, 'FM', 'manual')",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query("INSERT INTO channel_lists (id, name) VALUES (10, 'MAIN')")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO channel_list_entries (channel_list_id, channel_id, position)
             VALUES (10, 1, 0), (10, 2, 1), (10, 3, 2)",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO codeplug_channel_lists (codeplug_id, channel_list_id, position)
             VALUES (100, 10, 0)",
        )
        .execute(&pool)
        .await
        .unwrap();
        // Two scan lists on the codeplug. List 20 (planned index 0) contains
        // ch1 & ch2; list 21 (planned index 1) contains ch3 only.
        sqlx::query("INSERT INTO scan_lists (id, name) VALUES (20, 'SCAN A'), (21, 'SCAN B')")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO scan_list_entries (scan_list_id, channel_id, position)
             VALUES (20, 1, 0), (20, 2, 1), (21, 3, 0)",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO codeplug_scan_lists (codeplug_id, scan_list_id) VALUES (100, 20), (100, 21)",
        )
        .execute(&pool)
        .await
        .unwrap();
        // Explicit assignments:
        //  - ch1 → list 21 (index 1) even though ch1 is a member of list 20.
        //  - ch2 → unassigned, so none (0xFF) despite being a member of list 20.
        //  - ch3 → list 20 (index 0) even though ch3 is NOT a member of list 20.
        sqlx::query(
            "INSERT INTO codeplug_channel_scan_lists (codeplug_id, channel_id, scan_list_id)
             VALUES (100, 1, 21), (100, 3, 20)",
        )
        .execute(&pool)
        .await
        .unwrap();

        let plan = build_anytone_program(&pool, cp).await.expect("assemble");

        assert_eq!(plan.channels.len(), 3);
        // ch1 (slot 0): override to index 1, beats membership in list 0.
        assert_eq!(plan.channels[0].edit.scan_list_index, Some(1));
        // ch2 (slot 1): unassigned → membership in list 0 implies nothing.
        assert_eq!(plan.channels[1].edit.scan_list_index, Some(0xFF));
        // ch3 (slot 2): override to index 0 despite not being a member of it.
        assert_eq!(plan.channels[2].edit.scan_list_index, Some(0));
    }

    /// Issue #21: assigning a launch channel to one list must not touch any
    /// other channel's byte. A list with an assignment and a list without one
    /// behave identically for every channel the user didn't assign.
    #[tokio::test]
    async fn assignment_on_one_list_leaves_other_channels_alone() {
        let pool = fixture_pool("manual").await;
        let cp = d890_codeplug(&pool).await;

        sqlx::query(
            "INSERT INTO channels (id, name_long, rx_freq, mode, source)
             VALUES (1, 'ALPHA', 146.52, 'FM', 'manual'),
                    (2, 'BRAVO', 146.55, 'FM', 'manual'),
                    (3, 'CHARLIE', 146.58, 'FM', 'manual')",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query("INSERT INTO channel_lists (id, name) VALUES (10, 'MAIN')")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO channel_list_entries (channel_list_id, channel_id, position)
             VALUES (10, 1, 0), (10, 2, 1), (10, 3, 2)",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO codeplug_channel_lists (codeplug_id, channel_list_id, position)
             VALUES (100, 10, 0)",
        )
        .execute(&pool)
        .await
        .unwrap();
        // List 20 (index 0) contains ch1 & ch2, with ch1 assigned as its launch
        // channel. List 21 (index 1) contains ch3 and has no assignments.
        sqlx::query("INSERT INTO scan_lists (id, name) VALUES (20, 'SCAN A'), (21, 'SCAN B')")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO scan_list_entries (scan_list_id, channel_id, position)
             VALUES (20, 1, 0), (20, 2, 1), (21, 3, 0)",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO codeplug_scan_lists (codeplug_id, scan_list_id) VALUES (100, 20), (100, 21)",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO codeplug_channel_scan_lists (codeplug_id, channel_id, scan_list_id)
             VALUES (100, 1, 20)",
        )
        .execute(&pool)
        .await
        .unwrap();

        let plan = build_anytone_program(&pool, cp).await.expect("assemble");

        assert_eq!(plan.channels.len(), 3);
        // ch1: explicitly assigned to list 0.
        assert_eq!(plan.channels[0].edit.scan_list_index, Some(0));
        // ch2 & ch3: unassigned, so ch1's assignment changed nothing for them —
        // membership in list 0 / list 1 respectively implies no byte either way.
        assert_eq!(plan.channels[1].edit.scan_list_index, Some(0xFF));
        assert_eq!(plan.channels[2].edit.scan_list_index, Some(0xFF));
        // …and the user is told before the write, not after.
        assert!(plan
            .warnings
            .iter()
            .any(|w| w.contains("2 of 3 channels have no scan list")));
    }

    /// An assignment to a scan list not programmed in the codeplug is ignored
    /// with a warning; the channel programs with no scan list.
    #[tokio::test]
    async fn override_to_unplanned_list_is_ignored_with_warning() {
        let pool = fixture_pool("ovrbad").await;
        let cp = d890_codeplug(&pool).await;

        sqlx::query(
            "INSERT INTO channels (id, name_long, rx_freq, mode, source)
             VALUES (1, 'ALPHA', 146.52, 'FM', 'manual')",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query("INSERT INTO channel_lists (id, name) VALUES (10, 'MAIN')")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO channel_list_entries (channel_list_id, channel_id, position) VALUES (10, 1, 0)",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO codeplug_channel_lists (codeplug_id, channel_list_id, position) VALUES (100, 10, 0)",
        )
        .execute(&pool)
        .await
        .unwrap();
        // Scan list 99 exists and is assigned to the channel, but is NOT assigned
        // to the codeplug, so it never becomes a planned list.
        sqlx::query("INSERT INTO scan_lists (id, name) VALUES (99, 'ORPHAN')")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO codeplug_channel_scan_lists (codeplug_id, channel_id, scan_list_id)
             VALUES (100, 1, 99)",
        )
        .execute(&pool)
        .await
        .unwrap();

        let plan = build_anytone_program(&pool, cp).await.expect("assemble");
        assert_eq!(plan.channels[0].edit.scan_list_index, Some(0xFF));
        assert!(plan
            .warnings
            .iter()
            .any(|w| w.contains("isn't programmed in this codeplug")));
    }

    /// Off-by-default integration check: run the assembly against a COPY of a
    /// real app database (env `CPM_LIVE_DB_COPY` = path to the copy), wiring a
    /// synthetic codeplug to the D890UV profile with every radio-imported list
    /// assigned. Never touches the original DB. Run with:
    ///   cp <live db> /tmp/live-copy.sqlite3 && \
    ///   CPM_LIVE_DB_COPY=/tmp/live-copy.sqlite3 cargo test live_db -- --ignored --nocapture
    #[tokio::test]
    #[ignore = "needs CPM_LIVE_DB_COPY pointing at a copy of a real app DB"]
    async fn live_db_copy_assembles_clean() {
        let Ok(path) = std::env::var("CPM_LIVE_DB_COPY") else {
            eprintln!("CPM_LIVE_DB_COPY not set — skipping");
            return;
        };
        let pool = sqlx::SqlitePool::connect(&format!("sqlite:{path}"))
            .await
            .expect("open db copy");

        // Wire a synthetic codeplug to the existing D890UV profile (or make one).
        let (model_id,): (i64,) =
            sqlx::query_as("SELECT id FROM radio_models WHERE model = 'AT-D890UV'")
                .fetch_one(&pool)
                .await
                .unwrap();
        let profile_id: i64 = match sqlx::query_as::<_, (i64,)>(
            "SELECT id FROM radio_profiles WHERE radio_model_id = ?1 LIMIT 1",
        )
        .bind(model_id)
        .fetch_optional(&pool)
        .await
        .unwrap()
        {
            Some((id,)) => id,
            None => sqlx::query(
                "INSERT INTO radio_profiles (display_name, radio_model_id) VALUES ('test', ?1)",
            )
            .bind(model_id)
            .execute(&pool)
            .await
            .unwrap()
            .last_insert_rowid(),
        };
        let cp_id = sqlx::query("INSERT INTO codeplugs (name, radio_profile_id) VALUES ('live-copy test', ?1)")
            .bind(profile_id)
            .execute(&pool)
            .await
            .unwrap()
            .last_insert_rowid();
        let lists: Vec<(i64,)> = sqlx::query_as(
            "SELECT id FROM channel_lists WHERE description LIKE '%AT-D890UV%' ORDER BY id",
        )
        .fetch_all(&pool)
        .await
        .unwrap();
        assert!(!lists.is_empty(), "the DB copy has no radio-imported lists");
        for (pos, (list_id,)) in lists.iter().enumerate() {
            sqlx::query(
                "INSERT INTO codeplug_channel_lists (codeplug_id, channel_list_id, position) VALUES (?1, ?2, ?3)",
            )
            .bind(cp_id)
            .bind(list_id)
            .bind(pos as i64)
            .execute(&pool)
            .await
            .unwrap();
        }

        let plan = build_anytone_program(&pool, cp_id).await.expect("assembles");
        eprintln!(
            "assembled: {} channels, {} zones, {} scan lists, {} contacts, {} skipped",
            plan.channels.len(),
            plan.zones.len(),
            plan.scan_lists.len(),
            plan.contacts.len(),
            plan.skipped.len()
        );
        for s in &plan.skipped {
            eprintln!("  skipped: {} — {}", s.name, s.reason);
        }
        for w in &plan.warnings {
            eprintln!("  warning: {w}");
        }
        // Structural sanity on real data: packed slots, in-range members,
        // resolvable contact indices, DMR channels keep their talkgroups.
        for (i, pc) in plan.channels.iter().enumerate() {
            assert_eq!(pc.slot, i + 1);
            assert!(!pc.edit.name.is_empty(), "slot {} has an empty name", pc.slot);
            if pc.edit.mode == 1 {
                assert!(
                    (pc.edit.contact_index as usize) < plan.contacts.len(),
                    "slot {} points past the contact table",
                    pc.slot
                );
            }
        }
        for z in &plan.zones {
            assert!(z.member_slots.len() <= ZONE_MAX_CHANNELS);
            for &m in &z.member_slots {
                assert!((m as usize) < plan.channels.len());
            }
        }
        let dmr_count = plan.channels.iter().filter(|c| c.edit.mode == 1).count();
        eprintln!("  {dmr_count} DMR channels, all with contact indices in range");
    }

    #[tokio::test]
    async fn rejects_non_d890_codeplugs() {
        let pool = fixture_pool("model").await;
        let (model_id,): (i64,) =
            sqlx::query_as("SELECT id FROM radio_models WHERE model = 'UV-5R'")
                .fetch_one(&pool)
                .await
                .unwrap();
        sqlx::query("INSERT INTO radio_profiles (id, display_name, radio_model_id) VALUES (1, 'HT', ?1)")
            .bind(model_id)
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO codeplugs (id, name, radio_profile_id) VALUES (100, 'CP', 1)")
            .execute(&pool)
            .await
            .unwrap();
        let err = match build_anytone_program(&pool, 100).await {
            Ok(_) => panic!("expected a wrong-model error"),
            Err(e) => e,
        };
        assert!(err.contains("AT-D890UV"), "unexpected error: {err}");
    }
}