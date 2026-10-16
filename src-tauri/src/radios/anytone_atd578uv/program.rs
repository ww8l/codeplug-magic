//! Full-codeplug programming for the AT-D578UV: channels, zones, scan lists and
//! DMR contacts as a FULL REPLACE, in one PC-mode session with one END.
//!
//! The plan comes from the family planner (`anytone_atd890uv::program`), given
//! this radio's limits and CTCSS table; everything after the plan is this
//! radio's map. The session is split so all of its arithmetic is pure:
//!
//!   1. read the present-bitmap window — it says what the radio holds now;
//!   2. [`windows_to_read`] — every 0x4000 window the program touches, sized by
//!      the larger of the plan and what is already on the radio, so stale records
//!      past the new set are cleared rather than left behind;
//!   3. read those windows, then [`patches_for`] computes every byte to write;
//!   4. back up the originals and save the expected image BEFORE any write,
//!      write the changed windows, END once.
//!
//! The write unit is the whole 0x4000 window, read-modify-write. That is the
//! D890UV's hardware-proven safe unit (an isolated record write erased its flash
//! neighbours there); the D578 has not been measured either way, so it gets the
//! conservative unit. A channel bank's 128 records and their 128 extension
//! records fill exactly one window.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use serialport::SerialPort;

use crate::radios::anytone_atd890uv::program::{plan_program_for, AnytoneProgramPlan, PlanLimits};
use crate::radios::anytone_atd890uv::{
    end_session, open_port, plans_from_pre_read, read_block,
    serialize_backup, BankPlan, RegionPatch, WRITE_WINDOW,
};
use crate::commands::export::ExpandedChannel;
use crate::radios::driver::{
    CodeplugPayload, CodeplugPreview, CodeplugProgrammer, ProgramReport, SkippedChannel,
};

use super::*;

pub(crate) const D578_LIMITS: PlanLimits = PlanLimits {
    model: "AT-D578UV",
    ctcss: CTCSS,
    // qdmr and dmr-tools both store an inverted DCS code as code + 512. Not
    // measured on the anchor radio, whose channels hold no inverted DCS.
    dcs_invert: Some(512),
    max_channels: MAX_CHANNELS,
    max_zones: MAX_ZONES,
    zone_max_channels: ZONE_MAX_CHANNELS,
    max_scan_lists: MAX_SCAN_LISTS,
    scan_max_channels: SCAN_MAX_CHANNELS,
    max_contacts: MAX_CONTACTS,
};

/// One air-band memory: an AM channel between 108 and 136 MHz.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct AirChannel {
    pub mhz: f64,
    pub name: String,
}

/// The D578's plan: the family plan for the channel memories, plus the AM
/// channels that go to the separate air-band memory.
pub(crate) struct D578Plan {
    pub base: AnytoneProgramPlan,
    pub air: Vec<AirChannel>,
}

fn is_am(ec: &ExpandedChannel) -> bool {
    ec.channel.mode.as_deref().is_some_and(|m| m.eq_ignore_ascii_case("AM"))
}

fn in_air_band(mhz: f64) -> bool {
    (AIR_MIN_MHZ..=AIR_MAX_MHZ).contains(&mhz)
}

/// Plan a D578 program. AM channels in 108-136 MHz become air-band memories;
/// AM anywhere else is skipped — the channel memories hold FM and DMR only, and
/// the family planner would otherwise program an AM row as FM. Anything else in
/// 108-136 is skipped too: that span is in `rx_bands` only for the air band,
/// and the channel memories do not receive there.
pub(crate) fn plan_d578(payload: &CodeplugPayload) -> Result<D578Plan, String> {
    let (am, rest): (Vec<ExpandedChannel>, Vec<ExpandedChannel>) =
        payload.channels.iter().cloned().partition(is_am);
    let (in_air, rest): (Vec<ExpandedChannel>, Vec<ExpandedChannel>) =
        rest.into_iter().partition(|ec| in_air_band(ec.channel.rx_freq) && ec.channel.rx_freq < AIR_MAX_MHZ);
    let filtered = CodeplugPayload {
        model: payload.model,
        groups: payload.groups,
        channels: &rest,
        scan_lists: payload.scan_lists,
        scan_list_overrides: payload.scan_list_overrides,
        contact_users: payload.contact_users,
    };
    let mut base = plan_program_for(&filtered, &D578_LIMITS)?;
    for ec in &in_air {
        let c = &ec.channel;
        base.skipped.push(SkippedChannel {
            name: c.name_long.clone().or_else(|| c.name_short.clone()).unwrap_or_default(),
            reason: format!(
                "{} at {:.4} MHz — the {} receives only AM between {AIR_MIN_MHZ:.0} and {AIR_MAX_MHZ:.0} MHz",
                c.mode.as_deref().unwrap_or("FM"),
                c.rx_freq,
                base.radio
            ),
        });
    }

    let mut air: Vec<AirChannel> = Vec::new();
    let mut seen = BTreeSet::new();
    for ec in &am {
        let c = &ec.channel;
        let name = c
            .name_long
            .clone()
            .or_else(|| c.name_short.clone())
            .or_else(|| c.callsign.clone())
            .unwrap_or_else(|| format!("AIR {:.3}", c.rx_freq));
        if !in_air_band(c.rx_freq) {
            base.skipped.push(SkippedChannel {
                name,
                reason: format!(
                    "AM at {:.4} MHz — the {} receives AM only on its air band ({AIR_MIN_MHZ:.0}-{AIR_MAX_MHZ:.0} MHz)",
                    c.rx_freq, base.radio
                ),
            });
            continue;
        }
        if !seen.insert(c.id) {
            continue; // one row per channel; AM has no talkgroup expansion
        }
        if air.len() == MAX_AIR {
            base.warnings.push(format!(
                "{name}: the air-band memory holds {MAX_AIR} channels — left out"
            ));
            continue;
        }
        air.push(AirChannel { mhz: c.rx_freq, name: crate::util::truncate(&name, NAME_CHARS) });
    }
    if !air.is_empty() {
        base.warnings.push(format!(
            "{} AM channel{} between {AIR_MIN_MHZ:.0} and {AIR_MAX_MHZ:.0} MHz go{} into the radio's \
             separate air-band memory, not the channel list",
            air.len(),
            if air.len() == 1 { "" } else { "s" },
            if air.len() == 1 { "es" } else { "" },
        ));
    }
    Ok(D578Plan { base, air })
}

/// File-name prefix of a D578 program backup and its expected image. Distinct
/// from the D890's `anytone-program-`, so neither radio's Verify or Restore can
/// be pointed at the other's file — the maps overlap (the D578's zone lists sit
/// at the D890's first channel bank), so an address check alone would not
/// catch it.
pub(crate) const PROGRAM_PREFIX: &str = "atd578uv-program-";

/// The 0x4000 window holding every present bitmap this program rewrites
/// (channels 0x024C1500, zones 0x024C1300, scan lists 0x024C1340).
pub(crate) const BITMAP_WINDOW: u32 = 0x024C_0000;

fn window_of(addr: u32) -> u32 {
    addr & !(WRITE_WINDOW as u32 - 1)
}

/// What the radio holds before the program, read off its present bitmaps.
#[derive(Debug, Default, PartialEq)]
pub(crate) struct OnRadio {
    pub channels: Vec<usize>,
    pub zones: Vec<usize>,
    pub scan_lists: Vec<usize>,
}

impl OnRadio {
    pub(crate) fn from_bitmap_window(w: &[u8]) -> Self {
        let at = |addr: u32, bytes: usize, n: usize| {
            let o = (addr - BITMAP_WINDOW) as usize;
            bitmap_indices(&w[o..o + bytes], n)
        };
        OnRadio {
            channels: at(CHANNEL_BITMAP_BASE, CHANNEL_BITMAP_BYTES, MAX_CHANNELS),
            zones: at(ZONE_BITMAP_BASE, ZONE_BITMAP_BYTES, MAX_ZONES),
            scan_lists: at(SCAN_BITMAP_BASE, SCAN_BITMAP_BYTES, MAX_SCAN_LISTS),
        }
    }
}

/// How many contact records sit on the radio: the index list's entries before
/// its first 0xFFFFFFFF.
fn contact_count(index_list: &[u8]) -> usize {
    index_list
        .chunks(4)
        .take_while(|c| c != &[0xFF; 4])
        .count()
}

/// Every window the program reads (and may write), given the plan, what the
/// bitmaps say is on the radio, and the old contact count.
pub(crate) fn windows_to_read(
    plan: &AnytoneProgramPlan,
    air: &[AirChannel],
    on: &OnRadio,
    old_contacts: usize,
) -> BTreeSet<u32> {
    let mut w = BTreeSet::new();
    w.insert(BITMAP_WINDOW);
    // The air-band memory is written only when the codeplug has air channels:
    // a codeplug without any leaves the radio's own alone.
    if !air.is_empty() {
        w.insert(AIR_BASE);
    }
    // Channel banks: every bank holding a planned slot or a present one.
    for slot0 in (0..plan.channels.len()).chain(on.channels.iter().copied()) {
        w.insert(window_of(channel_addr(slot0)));
    }
    // Zones: names are one window; lists run contiguously.
    let zones = plan.zones.len().max(on.zones.iter().max().map_or(0, |z| z + 1));
    if zones > 0 {
        w.insert(ZONE_NAME_BASE);
        for z in 0..zones {
            w.insert(window_of(ZONE_LIST_BASE + z as u32 * ZONE_LIST_STEP));
        }
    }
    let lists = plan.scan_lists.len().max(on.scan_lists.iter().max().map_or(0, |s| s + 1));
    for i in 0..lists {
        w.insert(window_of(scan_list_addr(i)));
    }
    // Contacts: records (a record may straddle two windows), the index list,
    // and the inverted present bitmap.
    let contacts = plan.contacts.len().max(old_contacts);
    for i in 0..contacts {
        let a = contact_addr(i);
        w.insert(window_of(a));
        w.insert(window_of(a + CONTACT_REC_LEN as u32 - 1));
    }
    for i in 0..contacts.max(1) {
        w.insert(window_of(CONTACT_INDEX_BASE + i as u32 * 4));
    }
    for i in 0..=contacts {
        w.insert(window_of(CONTACT_ID_TABLE + (i * CONTACT_ID_ENTRY) as u32));
        w.insert(window_of(CONTACT_ID_TABLE + ((i + 1) * CONTACT_ID_ENTRY) as u32 - 1));
    }
    w.insert(CONTACT_BITMAP_BASE);
    w
}

/// What a program clears, for the report.
#[derive(Debug, Default, PartialEq)]
pub(crate) struct Cleared {
    pub slots: usize,
    pub zones: usize,
    pub scan_lists: usize,
    pub contacts: usize,
}

/// Every byte range the program writes, computed from the plan and the windows
/// read in phase 1. Pure.
pub(crate) fn patches_for(
    plan: &AnytoneProgramPlan,
    air: &[AirChannel],
    on: &OnRadio,
    old_contacts: usize,
    windows: &BTreeMap<u32, Vec<u8>>,
) -> Result<(Vec<RegionPatch>, Cleared), String> {
    let read_at = |addr: u32, len: usize| -> Result<Vec<u8>, String> {
        let base = window_of(addr);
        let w = windows
            .get(&base)
            .ok_or_else(|| format!("internal: window 0x{base:08X} was not read"))?;
        let o = (addr - base) as usize;
        Ok(w[o..o + len].to_vec())
    };
    let mut patches = Vec::new();
    let mut cleared = Cleared::default();

    // ---- Channels. A slot that held a channel is edited in place, so every
    // byte the plan does not model (APRS, encryption, roaming, the D578 tail)
    // survives; a slot that was empty starts from the measured template.
    let present: BTreeSet<usize> = on.channels.iter().copied().collect();
    for (slot0, pc) in plan.channels.iter().enumerate() {
        let addr = channel_addr(slot0);
        let mut rec = if present.contains(&slot0) {
            read_at(addr, CH_REC_LEN)?
        } else {
            channel_record_template().to_vec()
        };
        encode_channel_into(&mut rec, &pc.edit)?;
        apply_tone_squelch(&mut rec, &pc.edit);
        patches.push(RegionPatch { addr, data: rec });
    }
    for &slot0 in on.channels.iter().filter(|&&s| s >= plan.channels.len()) {
        patches.push(RegionPatch { addr: channel_addr(slot0), data: vec![0xFF; CH_REC_LEN] });
        cleared.slots += 1;
    }

    // ---- Zones.
    let old_zone_end = on.zones.iter().max().map_or(0, |z| z + 1);
    for (z, zone) in plan.zones.iter().enumerate() {
        patches.push(RegionPatch {
            addr: ZONE_NAME_BASE + z as u32 * ZONE_NAME_STEP,
            data: encode_zone_name(&zone.name),
        });
        patches.push(RegionPatch {
            addr: ZONE_LIST_BASE + z as u32 * ZONE_LIST_STEP,
            data: encode_zone_list(&zone.member_slots)?,
        });
    }
    for z in plan.zones.len()..old_zone_end {
        patches.push(RegionPatch {
            addr: ZONE_NAME_BASE + z as u32 * ZONE_NAME_STEP,
            data: vec![0u8; ZONE_NAME_STEP as usize],
        });
        patches.push(RegionPatch {
            addr: ZONE_LIST_BASE + z as u32 * ZONE_LIST_STEP,
            data: vec![0xFF; ZONE_LIST_STEP as usize],
        });
    }
    cleared.zones = on.zones.iter().filter(|&&z| z >= plan.zones.len()).count();

    // ---- Scan lists: owned bytes onto the existing record, tail preserved.
    let old_scan_end = on.scan_lists.iter().max().map_or(0, |s| s + 1);
    for (i, sl) in plan.scan_lists.iter().enumerate() {
        let addr = scan_list_addr(i);
        let mut rec = read_at(addr, SCAN_LIST_STEP as usize)?;
        let s = &sl.settings;
        encode_scan_list_into(
            &mut rec,
            &ScanListFields {
                name: sl.name.clone(),
                members: sl.member_slots.clone(),
                priority_select: s.priority_select,
                priority_1: s.priority_slot_1,
                priority_2: s.priority_slot_2,
                look_back_a: s.look_back_a,
                look_back_b: s.look_back_b,
                dropout_delay: s.dropout_delay,
                dwell_time: s.dwell_time,
                revert_channel: s.revert_channel,
            },
        )?;
        patches.push(RegionPatch { addr, data: rec });
    }
    for i in plan.scan_lists.len()..old_scan_end {
        patches.push(RegionPatch { addr: scan_list_addr(i), data: vec![0xFF; SCAN_LIST_STEP as usize] });
    }
    cleared.scan_lists = on.scan_lists.iter().filter(|&&s| s >= plan.scan_lists.len()).count();

    // ---- Contacts: records at their planned index, stale ones cleared, the
    // index list rewritten 0..N with 0xFF fill, and the inverted bitmap.
    for c in &plan.contacts {
        patches.push(RegionPatch {
            addr: contact_addr(c.index as usize),
            data: encode_contact(c.call_type, c.tg_number, &c.name)?,
        });
    }
    for i in plan.contacts.len()..old_contacts {
        patches.push(RegionPatch { addr: contact_addr(i), data: vec![0xFF; CONTACT_REC_LEN] });
    }
    cleared.contacts = old_contacts.saturating_sub(plan.contacts.len());
    let index_len = plan.contacts.len().max(old_contacts).max(1) * 4;
    let mut index = vec![0xFFu8; index_len];
    for i in 0..plan.contacts.len() {
        index[i * 4..i * 4 + 4].copy_from_slice(&(i as u32).to_le_bytes());
    }
    patches.push(RegionPatch { addr: CONTACT_INDEX_BASE, data: index });
    patches.push(RegionPatch { addr: CONTACT_BITMAP_BASE, data: contact_bitmap(plan.contacts.len()) });
    // The ID table the radio names talkgroups from, rebuilt for the new bank and
    // 0xFF past its terminator over every stale entry the old table had.
    let mut table = encode_contact_id_table(
        &plan.contacts.iter().map(|c| (c.call_type, c.tg_number, c.index as u32)).collect::<Vec<_>>(),
    );
    table.resize(table.len().max((old_contacts + 1) * CONTACT_ID_ENTRY), 0xFF);
    patches.push(RegionPatch { addr: CONTACT_ID_TABLE, data: table });

    // ---- Air band: every slot rewritten (programmed or cleared), with its
    // present and scan bitmaps; every programmed air channel is scanned.
    if !air.is_empty() {
        let mut recs = Vec::with_capacity(MAX_AIR * AIR_REC_LEN);
        for a in air {
            recs.extend(encode_air_channel(a.mhz, &a.name));
        }
        recs.resize(MAX_AIR * AIR_REC_LEN, 0xFF);
        patches.push(RegionPatch { addr: AIR_BASE, data: recs });
        patches.push(RegionPatch { addr: AIR_BITMAP, data: present_bitmap(air.len(), AIR_BITMAP_BYTES) });
        patches.push(RegionPatch { addr: AIR_SCAN_BITMAP, data: present_bitmap(air.len(), AIR_BITMAP_BYTES) });
    }

    // ---- Present bitmaps: written whole, so a stale bit from an earlier,
    // longer codeplug cannot leave a phantom channel, zone or list.
    patches.push(RegionPatch {
        addr: CHANNEL_BITMAP_BASE,
        data: present_bitmap(plan.channels.len(), CHANNEL_BITMAP_BYTES),
    });
    patches.push(RegionPatch {
        addr: ZONE_BITMAP_BASE,
        data: present_bitmap(plan.zones.len(), ZONE_BITMAP_BYTES),
    });
    patches.push(RegionPatch {
        addr: SCAN_BITMAP_BASE,
        data: present_bitmap(plan.scan_lists.len(), SCAN_BITMAP_BYTES),
    });

    for p in &patches {
        for a in [window_of(p.addr), window_of(p.addr + p.data.len() as u32 - 1)] {
            if writable_window_name(a).is_none() {
                return Err(format!("internal: refusing to write window 0x{a:08X}"));
            }
        }
    }
    Ok((patches, cleared))
}

/// Read every window in `addrs`, in address order.
fn read_windows(
    p: &mut dyn SerialPort,
    addrs: &BTreeSet<u32>,
) -> Result<BTreeMap<u32, Vec<u8>>, String> {
    let mut out = BTreeMap::new();
    for &a in addrs {
        out.insert(a, read_block(p, a, WRITE_WINDOW)?);
    }
    Ok(out)
}

/// The hardware session: phase-1 reads, backup + expected image, write, END.
fn run_program(
    port: &str,
    d578: &D578Plan,
    backup_path: &Path,
    expected_path: &Path,
) -> Result<ProgramReport, String> {
    let (plan, air) = (&d578.base, &d578.air[..]);
    let mut p = open_port(port)?;
    let _ident = enter(&mut *p)?;

    // ---- Phase 1: reads only. Any error ends the session with the radio
    // untouched.
    let phase1 = (|| -> Result<(Vec<BankPlan>, Cleared), String> {
        let bitmap = read_block(&mut *p, BITMAP_WINDOW, WRITE_WINDOW)?;
        let on = OnRadio::from_bitmap_window(&bitmap);
        let index_head = read_block(&mut *p, CONTACT_INDEX_BASE, WRITE_WINDOW)?;
        let mut old_contacts = contact_count(&index_head);
        if old_contacts == WRITE_WINDOW / 4 {
            // The list runs past its first window: read on until it ends.
            let rest = read_block(&mut *p, CONTACT_INDEX_BASE + WRITE_WINDOW as u32, MAX_CONTACTS * 4 - WRITE_WINDOW)?;
            old_contacts += contact_count(&rest);
        }
        let mut want = windows_to_read(plan, air, &on, old_contacts);
        want.remove(&BITMAP_WINDOW);
        let mut windows = read_windows(&mut *p, &want)?;
        windows.insert(BITMAP_WINDOW, bitmap);
        let (patches, cleared) = patches_for(plan, air, &on, old_contacts, &windows)?;
        let plans = plans_from_pre_read(&windows, &patches)?;
        // Read the rest of every half a write lands in, and carry its data.
        let rest: BTreeSet<u32> = windows_of_touched_halves(&plans)
            .into_iter()
            .filter(|w| !windows.contains_key(w))
            .collect();
        windows.extend(read_windows(&mut *p, &rest)?);
        Ok((with_carried_windows(plans, &windows)?, cleared))
    })();
    let (bank_plans, cleared) = match phase1 {
        Ok(v) => v,
        Err(e) => {
            let _ = end_session(&mut *p);
            return Err(e);
        }
    };

    // ---- Backup (originals) and expected image (patched) before any write.
    for (path, bytes) in [
        (backup_path, serialize_backup(&plan_blocks(&bank_plans, false))),
        (expected_path, serialize_backup(&plan_blocks(&bank_plans, true))),
    ] {
        if let Err(e) = std::fs::write(path, bytes) {
            let _ = end_session(&mut *p);
            return Err(format!("could not write {}: {e}", path.display()));
        }
    }

    // ---- Phase 2: write every changed window, then END once. A write error
    // deliberately does NOT end the session — END would commit a half-written
    // image — and the message names the backup to restore from.
    let written = super::commit_windows(&mut *p, &bank_plans).map_err(|e| {
        crate::radios::driver::with_restore_hint(
            e,
            backup_path,
            "Power-cycle the radio, rescan the port, then use \"Restore backup\" in the Program \
             dialog.",
        )
    })?;
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
        note: "Written and committed — the radio reboots and drops off USB. Reconnect it, \
               rescan, reselect the port, then run Verify against the expected image."
            .to_string(),
        verified: None,
        channels: Vec::new(),
        skipped: Vec::new(),
    })
}

impl CodeplugProgrammer for AnytoneAtd578uv {
    fn preview(&self, payload: &CodeplugPayload) -> Result<CodeplugPreview, String> {
        Ok(plan_d578(payload)?.base.preview())
    }

    fn program(
        &self,
        port: &str,
        payload: &CodeplugPayload,
        backup_dir: &Path,
    ) -> Result<ProgramReport, String> {
        let plan = plan_d578(payload)?;
        if plan.base.channels.is_empty() {
            return Err(
                "this codeplug has no programmable channels — add channel lists (or check the \
                 preview's skipped reasons)"
                    .to_string(),
            );
        }
        std::fs::create_dir_all(backup_dir).map_err(|e| e.to_string())?;
        let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
        let backup_path = backup_dir.join(format!("{PROGRAM_PREFIX}{stamp}.bin"));
        let expected_path = backup_dir.join(format!("{PROGRAM_PREFIX}{stamp}.expected.bin"));
        run_program(port, &plan, &backup_path, &expected_path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::radios::anytone_atd890uv::parse_dump;
    use crate::radios::anytone_atd890uv::program::{PlannedChannel, PlannedScanList, PlannedZone};
    use crate::radios::anytone_atd890uv::ScanListRecordSettings;

    const FIXTURE: &[u8] = include_bytes!("testdata/d578_codeplug_records.bin");

    /// A simulated radio: the anchor dump's records laid into 0x4000 windows,
    /// 0xFF wherever the dump did not reach (which is what unwritten flash reads).
    fn radio() -> BTreeMap<u32, Vec<u8>> {
        let mut windows: BTreeMap<u32, Vec<u8>> = BTreeMap::new();
        for (addr, data) in parse_dump(FIXTURE).unwrap() {
            for (i, b) in data.iter().enumerate() {
                let a = addr + i as u32;
                let w = windows.entry(window_of(a)).or_insert_with(|| vec![0xFF; WRITE_WINDOW]);
                w[(a - window_of(a)) as usize] = *b;
            }
        }
        windows
    }

    fn windows_for(radio: &BTreeMap<u32, Vec<u8>>, want: &BTreeSet<u32>) -> BTreeMap<u32, Vec<u8>> {
        want.iter()
            .map(|&a| (a, radio.get(&a).cloned().unwrap_or_else(|| vec![0xFF; WRITE_WINDOW])))
            .collect()
    }

    /// Apply patches to the simulated radio, as the write would.
    fn apply(radio: &mut BTreeMap<u32, Vec<u8>>, plans: &[BankPlan]) {
        for p in plans {
            radio.insert(p.base, p.patched.clone());
        }
    }

    fn read(radio: &BTreeMap<u32, Vec<u8>>, addr: u32, len: usize) -> Vec<u8> {
        let w = &radio[&window_of(addr)];
        let o = (addr - window_of(addr)) as usize;
        w[o..o + len].to_vec()
    }

    /// The plan the radio itself would produce: its own channels, zones and scan
    /// lists, decoded off the fixture.
    fn plan_from_radio(radio: &BTreeMap<u32, Vec<u8>>) -> AnytoneProgramPlan {
        let on = OnRadio::from_bitmap_window(&radio[&BITMAP_WINDOW]);
        let channels = on
            .channels
            .iter()
            .enumerate()
            .map(|(i, &slot0)| PlannedChannel {
                slot: i + 1,
                edit: decode_channel(&read(radio, channel_addr(slot0), CH_REC_LEN)).unwrap(),
            })
            .collect();
        let zones = on
            .zones
            .iter()
            .map(|&z| {
                let list = read(radio, ZONE_LIST_BASE + z as u32 * ZONE_LIST_STEP, 500);
                PlannedZone {
                    name: decode_ascii_field(&read(radio, ZONE_NAME_BASE + z as u32 * 0x20, 16)),
                    member_slots: list
                        .chunks(2)
                        .map(|p| u16::from_le_bytes([p[0], p[1]]))
                        .take_while(|&m| m != 0xFFFF)
                        .collect(),
                }
            })
            .collect();
        let scan_lists = on
            .scan_lists
            .iter()
            .map(|&i| {
                let f = decode_scan_list(&read(radio, scan_list_addr(i), 0x200));
                PlannedScanList {
                    name: f.name,
                    member_slots: f.members,
                    settings: ScanListRecordSettings {
                        priority_select: f.priority_select,
                        priority_slot_1: f.priority_1,
                        priority_slot_2: f.priority_2,
                        look_back_a: f.look_back_a,
                        look_back_b: f.look_back_b,
                        dropout_delay: f.dropout_delay,
                        dwell_time: f.dwell_time,
                        revert_channel: f.revert_channel,
                    },
                }
            })
            .collect();
        AnytoneProgramPlan {
            radio: "AnyTone AT-D578UV".into(),
            channels,
            zones,
            scan_lists,
            contacts: Vec::new(),
            skipped: Vec::new(),
            warnings: Vec::new(),
        }
    }

    /// Program a plan into the simulated radio; returns the bank plans written.
    fn program(radio: &mut BTreeMap<u32, Vec<u8>>, plan: &AnytoneProgramPlan) -> (Vec<BankPlan>, Cleared) {
        let on = OnRadio::from_bitmap_window(&radio[&BITMAP_WINDOW]);
        let want = windows_to_read(plan, &[], &on, 0);
        let windows = windows_for(radio, &want);
        let (patches, cleared) = patches_for(plan, &[], &on, 0, &windows).unwrap();
        let plans = plans_from_pre_read(&windows, &patches).unwrap();
        apply(radio, &plans);
        (plans, cleared)
    }

    /// Packing the radio's own 392 channels into slots 1..392 (they sit at
    /// scattered slots up to 413 on the radio) and reading the result back gives
    /// every channel, zone and scan list exactly as the plan said, with the
    /// stale slots past 392 cleared and the bitmaps rewritten to match.
    #[test]
    fn the_radios_own_codeplug_programs_and_reads_back() {
        let mut radio = radio();
        let plan = plan_from_radio(&radio);
        assert_eq!(plan.channels.len(), 392);
        let before = OnRadio::from_bitmap_window(&radio[&BITMAP_WINDOW]);
        let stale = before.channels.iter().filter(|&&s| s >= 392).count();
        assert!(stale > 0, "the fixture should have channels past slot 392");
        let (_, cleared) = program(&mut radio, &plan);

        let on = OnRadio::from_bitmap_window(&radio[&BITMAP_WINDOW]);
        assert_eq!(on.channels, (0..392).collect::<Vec<_>>());
        assert_eq!(on.zones, (0..21).collect::<Vec<_>>());
        assert_eq!(on.scan_lists, (0..21).collect::<Vec<_>>());
        for (slot0, pc) in plan.channels.iter().enumerate() {
            let got = decode_channel(&read(&radio, channel_addr(slot0), CH_REC_LEN)).unwrap();
            assert_eq!(got, pc.edit, "slot {}", slot0 + 1);
        }
        for (z, zone) in plan.zones.iter().enumerate() {
            let name = read(&radio, ZONE_NAME_BASE + z as u32 * 0x20, 16);
            assert_eq!(decode_ascii_field(&name), zone.name);
        }
        for (i, sl) in plan.scan_lists.iter().enumerate() {
            let f = decode_scan_list(&read(&radio, scan_list_addr(i), 0x200));
            assert_eq!((f.name, f.members), (sl.name.clone(), sl.member_slots.clone()));
        }
        // The radio's channels sat in slots 1..413 with holes; packing them
        // clears every slot past 392 that held one.
        assert_eq!(cleared.slots, stale);
        assert_eq!(read(&radio, channel_addr(400), CH_REC_LEN), vec![0xFF; CH_REC_LEN]);
    }

    /// Programming the identical codeplug twice: the second program changes
    /// nothing — no window differs, so nothing is written.
    #[test]
    fn reprogramming_the_same_codeplug_writes_nothing() {
        let mut radio = radio();
        let plan = plan_from_radio(&radio);
        program(&mut radio, &plan);
        let plan2 = plan_from_radio(&radio);
        let (plans, _) = program(&mut radio, &plan2);
        assert!(plans.is_empty(), "second program rewrote {} windows", plans.len());
    }

    /// A smaller codeplug clears what it no longer holds, everywhere.
    #[test]
    fn a_smaller_codeplug_clears_stale_records_and_bits() {
        let mut radio = radio();
        let mut plan = plan_from_radio(&radio);
        plan.channels.truncate(10);
        plan.zones.truncate(2);
        for z in &mut plan.zones {
            z.member_slots.retain(|&m| m < 10);
        }
        plan.scan_lists.truncate(1);
        plan.scan_lists[0].member_slots.retain(|&m| m < 10);
        let (_, cleared) = program(&mut radio, &plan);
        let on = OnRadio::from_bitmap_window(&radio[&BITMAP_WINDOW]);
        assert_eq!(on.channels.len(), 10);
        assert_eq!(on.zones, vec![0, 1]);
        assert_eq!(on.scan_lists, vec![0]);
        assert_eq!(cleared.zones, 19);
        assert_eq!(cleared.scan_lists, 20);
        assert_eq!(read(&radio, ZONE_NAME_BASE + 5 * 0x20, 0x20), vec![0; 0x20]);
        assert_eq!(read(&radio, scan_list_addr(20), 0x200), vec![0xFF; 0x200]);
    }

    /// Contacts land at their index with the index list and inverted bitmap to
    /// match, and an RX tone turns on tone squelch.
    #[test]
    fn contacts_and_tone_squelch() {
        use crate::radios::anytone_atd890uv::program::PlannedContact;
        use crate::radios::anytone_atd890uv::{AnytoneSubTone, AnytoneToneEdit};
        let mut radio = radio();
        let mut plan = plan_from_radio(&radio);
        plan.channels.truncate(2);
        plan.zones.clear();
        plan.scan_lists.clear();
        plan.channels[0].edit.tone = AnytoneToneEdit {
            tx: Some(AnytoneSubTone::Ctcss(100.0)),
            rx: Some(AnytoneSubTone::Ctcss(100.0)),
        };
        plan.contacts = vec![
            PlannedContact { index: 0, tg_number: 91, name: "Worldwide".into(), call_type: 1 },
            PlannedContact { index: 1, tg_number: 3100, name: "USA".into(), call_type: 1 },
        ];
        program(&mut radio, &plan);
        assert_eq!(read(&radio, channel_addr(0), CH_REC_LEN)[CH_SQUELCH_MODE] & CH_SQ_CTCSS_DCS, CH_SQ_CTCSS_DCS);
        assert_eq!(decode_contact(&read(&radio, contact_addr(1), CONTACT_REC_LEN)), (1, 3100, "USA".into()));
        assert_eq!(read(&radio, CONTACT_INDEX_BASE, 12), [0, 0, 0, 0, 1, 0, 0, 0, 0xFF, 0xFF, 0xFF, 0xFF]);
        assert_eq!(read(&radio, CONTACT_BITMAP_BASE, 2), [0xFC, 0xFF]);
        // ID table sorted by ID: 91 (BCD 0x91 << 1 | group) before 3100.
        assert_eq!(read(&radio, CONTACT_ID_TABLE, 24), [
            0x23, 0x01, 0, 0, 0, 0, 0, 0,
            0x01, 0x62, 0, 0, 1, 0, 0, 0,
            0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF,
        ]);
    }

    /// A slot that was empty is written from the measured template, not from
    /// whatever erased flash held.
    #[test]
    fn a_new_slot_starts_from_the_template() {
        let mut radio = radio();
        let mut plan = plan_from_radio(&radio);
        // Slot 22 (0-based 21) is empty on the anchor radio.
        assert!(!OnRadio::from_bitmap_window(&radio[&BITMAP_WINDOW]).channels.contains(&21));
        let edit = plan.channels[0].edit.clone();
        plan.channels.truncate(22);
        plan.channels[21].edit = edit.clone();
        program(&mut radio, &plan);
        let mut want = channel_record_template().to_vec();
        encode_channel_into(&mut want, &edit).unwrap();
        apply_tone_squelch(&mut want, &edit);
        assert_eq!(read(&radio, channel_addr(21), CH_REC_LEN), want);
    }

    /// Step-4 gate: the app's own pipeline — `resolve_codeplug_payload` and the
    /// planner — run on a COPY of a real database, programmed into the simulated
    /// anchor radio and read back record by record. Every channel list and scan
    /// list any codeplug in the copy uses goes into one D578 codeplug.
    ///   cp "<app data>/codeplug_manager.sqlite3" /tmp/d578-copy.sqlite3 && \
    ///   CPM_LIVE_DB_COPY=/tmp/d578-copy.sqlite3 \
    ///     cargo test live_db_copy_programs -- --ignored --nocapture
    #[tokio::test]
    #[ignore = "needs CPM_LIVE_DB_COPY pointing at a copy of a real app DB"]
    async fn live_db_copy_programs_and_reads_back() {
        let Ok(path) = std::env::var("CPM_LIVE_DB_COPY") else {
            eprintln!("CPM_LIVE_DB_COPY not set — skipping");
            return;
        };
        // init_pool runs the migrations and the seed, which is what adds the
        // AT-D578UV row to an older database.
        let pool = crate::db::init_pool(Path::new(&path)).await.expect("open db copy");
        let (model_id,): (i64,) =
            sqlx::query_as("SELECT id FROM radio_models WHERE model = 'AT-D578UV'")
                .fetch_one(&pool)
                .await
                .unwrap();
        let profile_id = sqlx::query(
            "INSERT INTO radio_profiles (display_name, radio_model_id) VALUES ('d578 gate', ?1)",
        )
        .bind(model_id)
        .execute(&pool)
        .await
        .unwrap()
        .last_insert_rowid();
        let cp_id = sqlx::query("INSERT INTO codeplugs (name, radio_profile_id) VALUES ('d578 gate', ?1)")
            .bind(profile_id)
            .execute(&pool)
            .await
            .unwrap()
            .last_insert_rowid();
        sqlx::query(
            "INSERT INTO codeplug_channel_lists (codeplug_id, channel_list_id, position)
             SELECT ?1, channel_list_id, ROW_NUMBER() OVER (ORDER BY MIN(codeplug_id), MIN(position))
             FROM codeplug_channel_lists WHERE codeplug_id != ?1 GROUP BY channel_list_id",
        )
        .bind(cp_id)
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO codeplug_scan_lists (codeplug_id, scan_list_id)
             SELECT DISTINCT ?1, scan_list_id FROM codeplug_scan_lists WHERE codeplug_id != ?1",
        )
        .bind(cp_id)
        .execute(&pool)
        .await
        .unwrap();

        let resolved = crate::commands::export::resolve_codeplug_payload(&pool, cp_id)
            .await
            .expect("resolves");
        let plan = plan_d578(&resolved.payload()).expect("plans").base;
        eprintln!(
            "planned: {} channels, {} zones, {} scan lists, {} contacts; {} skipped, {} warnings",
            plan.channels.len(),
            plan.zones.len(),
            plan.scan_lists.len(),
            plan.contacts.len(),
            plan.skipped.len(),
            plan.warnings.len()
        );
        for sk in plan.skipped.iter().take(15) {
            eprintln!("  skipped {}: {}", sk.name, sk.reason);
        }
        for w in plan.warnings.iter().take(15) {
            eprintln!("  warning {w}");
        }

        let mut radio = radio();
        program(&mut radio, &plan);
        let on = OnRadio::from_bitmap_window(&radio[&BITMAP_WINDOW]);
        assert_eq!(on.channels.len(), plan.channels.len(), "channels present");
        assert_eq!(on.zones.len(), plan.zones.len(), "zones present");
        assert_eq!(on.scan_lists.len(), plan.scan_lists.len(), "scan lists present");
        for (slot0, pc) in plan.channels.iter().enumerate() {
            let got = decode_channel(&read(&radio, channel_addr(slot0), CH_REC_LEN)).unwrap();
            let mut want = pc.edit.clone();
            want.name = decode_ascii_field(&encode_ascii_field(&want.name, NAME_CHARS));
            assert_eq!(got, want, "slot {}", slot0 + 1);
        }
        for (z, zone) in plan.zones.iter().enumerate() {
            let list = read(&radio, ZONE_LIST_BASE + z as u32 * ZONE_LIST_STEP, 500);
            let members: Vec<u16> = list
                .chunks(2)
                .map(|p| u16::from_le_bytes([p[0], p[1]]))
                .take_while(|&m| m != 0xFFFF)
                .collect();
            assert_eq!(members, zone.member_slots, "zone {}", zone.name);
        }
        for (i, sl) in plan.scan_lists.iter().enumerate() {
            let f = decode_scan_list(&read(&radio, scan_list_addr(i), 0x200));
            assert_eq!(f.members, sl.member_slots, "scan list {}", sl.name);
        }
        for c in &plan.contacts {
            let (t, id, _) = decode_contact(&read(&radio, contact_addr(c.index as usize), CONTACT_REC_LEN));
            assert_eq!((t, id), (c.call_type, c.tg_number), "contact {}", c.name);
        }
        eprintln!("read back: every planned record matches");
    }

    fn chan(id: i64, name: &str, rx: f64, mode: &str) -> ExpandedChannel {
        ExpandedChannel {
            channel: crate::models::Channel {
                id,
                name_long: Some(name.into()),
                rx_freq: rx,
                tx_freq: Some(rx),
                mode: Some(mode.into()),
                dcs_polarity: "NN".into(),
                ..Default::default()
            },
            tg_label: None,
            timeslot: None,
            tg_number: None,
            tg_call_type: None,
            tg_inline: false,
        }
    }

    /// AM in the air band goes to the air-band memory; AM elsewhere and FM in
    /// the air band are skipped with a reason; the channel memories never see
    /// an AM row.
    #[test]
    fn am_channels_are_routed_to_the_air_band_and_nowhere_else() {
        let model = crate::seed::test_model("AT-D578UV");
        let channels = vec![
            chan(1, "Tower", 118.300, "AM"),
            chan(2, "Ground", 121.900, "AM"),
            chan(3, "CB 19", 27.185, "AM"),
            chan(4, "Calling", 146.520, "FM"),
            chan(5, "FM in air", 120.000, "FM"),
        ];
        let payload = CodeplugPayload {
            model: &model,
            groups: &[],
            channels: &channels,
            scan_lists: &[],
            scan_list_overrides: &[],
            contact_users: &[],
        };
        let p = plan_d578(&payload).unwrap();
        assert_eq!(
            p.air,
            vec![
                AirChannel { mhz: 118.300, name: "Tower".into() },
                AirChannel { mhz: 121.900, name: "Ground".into() },
            ]
        );
        assert_eq!(p.base.channels.len(), 1, "only the 2 m FM channel reaches a channel slot");
        assert_eq!(p.base.channels[0].edit.name, "Calling");
        let skipped: Vec<&str> = p.base.skipped.iter().map(|s| s.name.as_str()).collect();
        assert!(skipped.contains(&"CB 19") && skipped.contains(&"FM in air"), "{skipped:?}");
        assert!(p.base.warnings.iter().any(|w| w.contains("2 AM channels")));
    }

    /// A reversed-polarity DCS code is stored with +512 on the D578.
    #[test]
    fn inverted_dcs_is_stored_with_512_added() {
        let model = crate::seed::test_model("AT-D578UV");
        let mut c = chan(1, "Inverted", 146.520, "FM");
        c.channel.tone_mode = Some("DTCS".into());
        c.channel.dcs_code = Some("023".into());
        c.channel.dcs_polarity = "RR".into();
        let mut n = chan(2, "Normal", 146.550, "FM");
        n.channel.tone_mode = Some("DTCS".into());
        n.channel.dcs_code = Some("023".into());
        let channels = vec![c, n];
        let payload = CodeplugPayload {
            model: &model,
            groups: &[],
            channels: &channels,
            scan_lists: &[],
            scan_list_overrides: &[],
            contact_users: &[],
        };
        let p = plan_d578(&payload).unwrap();
        use crate::radios::anytone_atd890uv::AnytoneSubTone::Dcs;
        assert_eq!(p.base.channels[0].edit.tone.tx, Some(Dcs(0o23 + 512)));
        assert_eq!(p.base.channels[1].edit.tone.tx, Some(Dcs(0o23)));
    }

    /// Air channels land in their slots with both bitmaps; every other slot is
    /// cleared; a codeplug without air channels leaves the memory alone.
    #[test]
    fn air_band_records_and_bitmaps() {
        let mut radio = radio();
        let plan = plan_from_radio(&radio);
        let on = OnRadio::from_bitmap_window(&radio[&BITMAP_WINDOW]);
        assert!(!windows_to_read(&plan, &[], &on, 0).contains(&AIR_BASE));
        let air = vec![
            AirChannel { mhz: 118.300, name: "Tower".into() },
            AirChannel { mhz: 135.975, name: "Top".into() },
        ];
        let want = windows_to_read(&plan, &air, &on, 0);
        assert!(want.contains(&AIR_BASE));
        let windows = windows_for(&radio, &want);
        let (patches, _) = patches_for(&plan, &air, &on, 0, &windows).unwrap();
        apply(&mut radio, &plans_from_pre_read(&windows, &patches).unwrap());
        let rec = read(&radio, AIR_BASE + 0x20, 0x20);
        assert_eq!(&rec[0..4], &[0x13, 0x59, 0x75, 0x00]);
        assert_eq!(decode_ascii_field(&rec[4..20]), "Top");
        assert_eq!(&rec[20..], &[0u8; 12]);
        assert_eq!(read(&radio, AIR_BASE + 2 * 0x20, 0x20), vec![0xFF; 0x20]);
        assert_eq!(read(&radio, AIR_BITMAP, 2), [0x03, 0x00]);
        assert_eq!(read(&radio, AIR_SCAN_BITMAP, 2), [0x03, 0x00]);
    }

    #[test]
    fn every_window_the_program_touches_is_guarded() {
        let radio = radio();
        let plan = plan_from_radio(&radio);
        let on = OnRadio::from_bitmap_window(&radio[&BITMAP_WINDOW]);
        for w in windows_to_read(&plan, &[], &on, 2500) {
            assert!(writable_window_name(w).is_some(), "0x{w:08X}");
        }
    }
}
