//! The D868UV's map, held against the radio's own records.
//!
//! `testdata/d868_codeplug_records.bin` is the record regions of a read-only dump
//! of a real D868UV (`ID868UVE\0V102`): the channel and zone/radio-ID/scan present
//! bitmaps, every present channel record, every zone list and name, and every
//! scan list. `testdata/d868_contacts.bin` is a second read-only dump's contact
//! regions: the index list, the present bitmap, the 40 contact records and the
//! contact ID table. Settings, boot text and the radio ID were left out.
//!
//! These are the step-3 gate of #51: the D578's encoders, which this driver
//! reuses, must reproduce the D868's own bytes.

use std::collections::BTreeMap;

// The D578's map and encoders; this module's own items, named explicitly,
// shadow the D578's same-named ones.
use crate::radios::anytone_atd578uv::*;
use crate::radios::anytone_atd890uv::parse_dump;
use super::{channel_record_template, CONTACT_ID_TABLE, D868_IDENT};

const FIXTURE: &[u8] = include_bytes!("testdata/d868_codeplug_records.bin");
const CONTACTS: &[u8] = include_bytes!("testdata/d868_contacts.bin");

const RADIO_CHANNELS: usize = 424;
const RADIO_ZONES: usize = 23;
const RADIO_SCAN_LISTS: usize = 22;
const RADIO_CONTACTS: usize = 40;

fn fixture() -> BTreeMap<u32, Vec<u8>> {
    parse_dump(FIXTURE).unwrap().into_iter().collect()
}

fn contacts_fixture() -> BTreeMap<u32, Vec<u8>> {
    parse_dump(CONTACTS).unwrap().into_iter().collect()
}

fn present(fx: &BTreeMap<u32, Vec<u8>>, base: u32, off: usize, bytes: usize, n: usize) -> Vec<usize> {
    bitmap_indices(&fx[&base][off..off + bytes], n)
}

fn channel_slots(fx: &BTreeMap<u32, Vec<u8>>) -> Vec<usize> {
    present(fx, CHANNEL_BITMAP_BASE, 0, CHANNEL_BITMAP_BYTES, MAX_CHANNELS)
}

/// The radio's present bitmaps agree with what the fixture holds: every set bit
/// has a record behind it, and nothing else does. A bitmap at another address,
/// or an inverted one, fails this.
#[test]
fn the_present_bitmaps_match_the_radios_records() {
    let fx = fixture();
    let slots = channel_slots(&fx);
    assert_eq!(slots.len(), RADIO_CHANNELS);
    for &s in &slots {
        assert!(decode_channel(&fx[&channel_addr(s)]).is_some(), "slot {} blank", s + 1);
    }
    assert_eq!(present(&fx, ZONE_BITMAP_BASE, 0, ZONE_BITMAP_BYTES, MAX_ZONES).len(), RADIO_ZONES);
    assert_eq!(present(&fx, ZONE_BITMAP_BASE, 0x40, SCAN_BITMAP_BYTES, MAX_SCAN_LISTS).len(), RADIO_SCAN_LISTS);
}

/// ★ The gate: every channel the radio wrote decodes and re-encodes onto its own
/// bytes with nothing changed.
#[test]
fn every_radio_channel_reencodes_byte_identically() {
    let fx = fixture();
    for slot in channel_slots(&fx) {
        let original = &fx[&channel_addr(slot)];
        let edit = decode_channel(original).unwrap();
        let mut rec = original.clone();
        encode_channel_into(&mut rec, &edit).unwrap();
        assert_eq!(&rec, original, "slot {} ({}) re-encoded differently", slot + 1, edit.name);
    }
}

/// The stronger half: encoded onto the D868's BLANK template, every byte the
/// encoder owns lands where the radio put it, and the record decodes back to the
/// same channel.
#[test]
fn every_radio_channel_encodes_onto_the_template_at_the_radios_offsets() {
    let fx = fixture();
    let owned: Vec<usize> = (0x00..0x10)
        .chain(0x14..0x18)
        .chain([CH_SCAN_LIST, CH_COLOR_CODE])
        .chain(CH_NAME..CH_NAME + NAME_CHARS)
        .collect();
    for slot in channel_slots(&fx) {
        let original = &fx[&channel_addr(slot)];
        let edit = decode_channel(original).unwrap();
        let mut rec = channel_record_template().to_vec();
        encode_channel_into(&mut rec, &edit).unwrap();
        for &o in &owned {
            // A side with no tone keeps the template's code bytes; the radio's
            // stale value there is unreachable through the enable bits.
            let tone_byte = (CH_TX_CTCSS..CH_RX_DCS + 2).contains(&o);
            if tone_byte && rec[o] != original[o] {
                continue;
            }
            assert_eq!(rec[o], original[o], "slot {} byte 0x{o:02X}", slot + 1);
        }
        assert_eq!(rec[CH_TIME_SLOT] & 1, original[CH_TIME_SLOT] & 1, "slot {} TS", slot + 1);
        assert_eq!(decode_channel(&rec), Some(edit), "slot {} round trip", slot + 1);
    }
}

/// The D868 template is the radio's majority record outside the encoder's
/// fields — including the whole 0x33-0x3F tail, which is the D868's own.
#[test]
fn the_template_matches_the_radios_majority_record() {
    let fx = fixture();
    let t = channel_record_template();
    let recs: Vec<&Vec<u8>> = channel_slots(&fx).iter().map(|&s| &fx[&channel_addr(s)]).collect();
    // The CTCSS index bytes are left out: 0x00 is only the radio's most common
    // value there (190 of 424), not a majority, and they are inert without an
    // enable bit.
    for o in [0x0C, 0x0D, 0x0E, 0x0F, 0x10, 0x11, 0x12, 0x13, 0x15, 0x16, 0x17]
        .into_iter()
        .chain([0x18, 0x1A, 0x1B, 0x1C, 0x1D, 0x1E, 0x1F, 0x22])
        .chain(0x33..0x40)
    {
        let agree = recs.iter().filter(|r| r[o] == t[o]).count();
        assert!(agree * 2 > recs.len(), "template byte 0x{o:02X} disagrees with the radio");
    }
}

#[test]
fn every_radio_zone_reencodes_byte_identically() {
    let fx = fixture();
    for z in present(&fx, ZONE_BITMAP_BASE, 0, ZONE_BITMAP_BYTES, MAX_ZONES) {
        let list = &fx[&(ZONE_LIST_BASE + z as u32 * ZONE_LIST_STEP)];
        let members: Vec<u16> = list
            .chunks(2)
            .map(|p| u16::from_le_bytes([p[0], p[1]]))
            .take_while(|&m| m != 0xFFFF)
            .collect();
        assert!(members.iter().all(|&m| (m as usize) < MAX_CHANNELS), "zone {} member", z + 1);
        assert_eq!(&encode_zone_list(&members).unwrap(), list, "zone {} list", z + 1);
        // The radio may leave stale bytes after a shortened name's NUL; the
        // encoder zero-fills, so the name is held to its bytes up to the NUL.
        let name = &fx[&(ZONE_NAME_BASE + z as u32 * ZONE_NAME_STEP)];
        let enc = encode_zone_name(&decode_ascii_field(&name[..16]));
        let end = name.iter().position(|&b| b == 0).unwrap_or(16);
        assert_eq!(&enc[..end], &name[..end], "zone {} name", z + 1);
        assert!(enc[end..].iter().all(|&b| b == 0), "zone {} name padding", z + 1);
    }
}

#[test]
fn every_radio_scan_list_reencodes_its_owned_fields() {
    let fx = fixture();
    let lists = present(&fx, ZONE_BITMAP_BASE, 0x40, SCAN_BITMAP_BYTES, MAX_SCAN_LISTS);
    // 22 lists: the 17th onward sit in the second 0x40000 bank, so the
    // 16-per-bank layout is exercised by the radio's own data.
    assert!(lists.iter().any(|&i| i >= SCAN_LISTS_PER_BANK));
    for i in lists {
        let original = &fx[&scan_list_addr(i)];
        let fields = decode_scan_list(original);
        assert!(!fields.name.is_empty(), "list {} has no name", i + 1);
        let mut rec = original.clone();
        encode_scan_list_into(&mut rec, &fields).unwrap();
        assert_eq!(&rec[..SCAN_NAME], &original[..SCAN_NAME], "list {} header", i + 1);
        assert_eq!(&rec[SCAN_MEMBERS..], &original[SCAN_MEMBERS..], "list {} members+tail", i + 1);
        assert_eq!(decode_scan_list(&rec), fields, "list {} round trip", i + 1);
    }
}

/// ★ Every contact re-encodes to its own 100 bytes, and the index list and the
/// INVERTED present bitmap the program writes for 40 contacts are the radio's
/// own. The records sit at qdmr's 0x02680000; dmr-tools' 0x02640500 read blank.
#[test]
fn every_radio_contact_reencodes_byte_identically() {
    let fx = contacts_fixture();
    let recs = &fx[&CONTACT_BASE];
    for i in 0..RADIO_CONTACTS {
        let original = &recs[i * CONTACT_REC_LEN..(i + 1) * CONTACT_REC_LEN];
        let (call_type, id, name) = decode_contact(original);
        assert_eq!(encode_contact(call_type, id, &name).unwrap(), original, "contact {i} ({name})");
    }
    let index = &fx[&CONTACT_INDEX_BASE];
    for i in 0..RADIO_CONTACTS {
        assert_eq!(&index[i * 4..i * 4 + 4], &(i as u32).to_le_bytes());
    }
    assert_eq!(&index[RADIO_CONTACTS * 4..RADIO_CONTACTS * 4 + 4], &[0xFF; 4]);
    assert_eq!(&fx[&CONTACT_BITMAP_BASE][..CONTACT_BITMAP_BYTES], &contact_bitmap(RADIO_CONTACTS)[..]);
}

/// ★ The contact ID table at the D868's own address rebuilds byte for byte from
/// the radio's contacts with the D578's encoder.
#[test]
fn the_contact_id_table_rebuilds_byte_identically() {
    let fx = contacts_fixture();
    let recs = &fx[&CONTACT_BASE];
    let contacts: Vec<(u8, u32, u32)> = (0..RADIO_CONTACTS)
        .map(|i| {
            let (t, id, _) = decode_contact(&recs[i * CONTACT_REC_LEN..(i + 1) * CONTACT_REC_LEN]);
            (t, id, i as u32)
        })
        .collect();
    let table = encode_contact_id_table(&contacts);
    assert_eq!(table.len(), (RADIO_CONTACTS + 1) * CONTACT_ID_ENTRY);
    assert_eq!(&fx[&super::CONTACT_ID_TABLE][..], &table[..]);
    assert_ne!(CONTACT_ID_TABLE, 0x0480_0000, "the D578's address is not the D868's");
}

#[test]
fn the_ident_gate_is_the_d868s() {
    assert_eq!(D868_IDENT.token, "868");
    assert_eq!(D868_IDENT.revision, Some("V102"));
}

// ============================================================
// The programmer, on a simulated radio built from the anchor dump
// ============================================================

mod program {
    use std::collections::BTreeMap;
    use std::path::Path;

    use crate::commands::export::ExpandedChannel;
    use crate::radios::anytone_atd578uv::program::{
        patches_for_model, plan_for, windows_to_read_for, OnRadio, BITMAP_WINDOW,
    };
    use crate::radios::anytone_atd578uv::*;
    use crate::radios::anytone_atd890uv::program::{
        AnytoneProgramPlan, PlannedChannel, PlannedContact, PlannedScanList, PlannedZone,
    };
    use crate::radios::anytone_atd890uv::{
        parse_dump, plans_from_pre_read, BankPlan, ScanListRecordSettings, WRITE_WINDOW,
    };
    use crate::radios::driver::CodeplugPayload;

    use super::super::{check_restore_blocks, CONTACT_ID_TABLE, D868_MODEL};
    use super::{CONTACTS, FIXTURE, RADIO_CHANNELS, RADIO_CONTACTS, RADIO_SCAN_LISTS, RADIO_ZONES};

    fn window_of(addr: u32) -> u32 {
        addr & !(WRITE_WINDOW as u32 - 1)
    }

    /// The anchor dump's records and contact regions laid into 0x4000 windows,
    /// 0xFF wherever the dumps did not reach (what unwritten flash reads).
    fn radio() -> BTreeMap<u32, Vec<u8>> {
        let mut windows: BTreeMap<u32, Vec<u8>> = BTreeMap::new();
        for fx in [FIXTURE, CONTACTS] {
            for (addr, data) in parse_dump(fx).unwrap() {
                for (i, b) in data.iter().enumerate() {
                    let a = addr + i as u32;
                    let w = windows.entry(window_of(a)).or_insert_with(|| vec![0xFF; WRITE_WINDOW]);
                    w[(a - window_of(a)) as usize] = *b;
                }
            }
        }
        windows
    }

    fn read(radio: &BTreeMap<u32, Vec<u8>>, addr: u32, len: usize) -> Vec<u8> {
        let w = radio.get(&window_of(addr)).cloned().unwrap_or_else(|| vec![0xFF; WRITE_WINDOW]);
        let o = (addr - window_of(addr)) as usize;
        w[o..o + len].to_vec()
    }

    /// The plan the radio itself holds: its channels, zones, scan lists and
    /// contacts, decoded off the fixtures.
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
            .map(|&z| PlannedZone {
                name: decode_ascii_field(&read(radio, ZONE_NAME_BASE + z as u32 * ZONE_NAME_STEP, 16)),
                member_slots: read(radio, ZONE_LIST_BASE + z as u32 * ZONE_LIST_STEP, 500)
                    .chunks(2)
                    .map(|p| u16::from_le_bytes([p[0], p[1]]))
                    .take_while(|&m| m != 0xFFFF)
                    .collect(),
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
        let contacts = (0..RADIO_CONTACTS)
            .map(|i| {
                let (call_type, tg_number, name) = decode_contact(&read(radio, contact_addr(i), CONTACT_REC_LEN));
                PlannedContact { index: i as u16, tg_number, name, call_type }
            })
            .collect();
        AnytoneProgramPlan {
            radio: "AnyTone AT-D868UV".into(),
            channels,
            zones,
            scan_lists,
            contacts,
            skipped: Vec::new(),
            warnings: Vec::new(),
        }
    }

    /// Program a plan into the simulated radio through the D868's model, as the
    /// session would (old contact count from the radio's index list).
    fn program(radio: &mut BTreeMap<u32, Vec<u8>>, plan: &AnytoneProgramPlan) -> Vec<BankPlan> {
        let on = OnRadio::from_bitmap_window(&radio[&BITMAP_WINDOW]);
        let old_contacts = read(radio, CONTACT_INDEX_BASE, 0x4000)
            .chunks(4)
            .take_while(|c| c != &[0xFF; 4])
            .count();
        let want = windows_to_read_for(&D868_MODEL, plan, &[], &on, old_contacts);
        let windows: BTreeMap<u32, Vec<u8>> = want
            .iter()
            .map(|&a| (a, radio.get(&a).cloned().unwrap_or_else(|| vec![0xFF; WRITE_WINDOW])))
            .collect();
        let (patches, _) = patches_for_model(&D868_MODEL, plan, &[], &on, old_contacts, &windows).unwrap();
        let plans = plans_from_pre_read(&windows, &patches).unwrap();
        for p in &plans {
            radio.insert(p.base, p.patched.clone());
        }
        plans
    }

    /// The radio's own codeplug — 424 channels scattered over slots 1..445 —
    /// programs onto it and reads back record for record, packed into 1..424,
    /// with its contacts and its contact ID table at the D868's address.
    #[test]
    fn the_radios_own_codeplug_programs_and_reads_back() {
        let mut radio = radio();
        let table_before = read(&radio, CONTACT_ID_TABLE, (RADIO_CONTACTS + 1) * CONTACT_ID_ENTRY);
        let plan = plan_from_radio(&radio);
        assert_eq!(plan.channels.len(), RADIO_CHANNELS);
        let plans = program(&mut radio, &plan);

        let on = OnRadio::from_bitmap_window(&radio[&BITMAP_WINDOW]);
        assert_eq!(on.channels, (0..RADIO_CHANNELS).collect::<Vec<_>>());
        assert_eq!(on.zones, (0..RADIO_ZONES).collect::<Vec<_>>());
        assert_eq!(on.scan_lists, (0..RADIO_SCAN_LISTS).collect::<Vec<_>>());
        for (slot0, pc) in plan.channels.iter().enumerate() {
            let got = decode_channel(&read(&radio, channel_addr(slot0), CH_REC_LEN)).unwrap();
            assert_eq!(got, pc.edit, "slot {}", slot0 + 1);
        }
        for c in &plan.contacts {
            let got = decode_contact(&read(&radio, contact_addr(c.index as usize), CONTACT_REC_LEN));
            assert_eq!(got, (c.call_type, c.tg_number, c.name.clone()));
        }
        // The contacts were already in order, so the rebuilt table is the
        // radio's own, at 0x04340000 — and nothing touched the D578's address.
        assert_eq!(read(&radio, CONTACT_ID_TABLE, table_before.len()), table_before);
        assert!(plans.iter().all(|p| window_of(p.base) != 0x0480_0000));
        assert!(plans.iter().all(|p| p.base != AIR_BASE));
        assert_eq!(read(&radio, channel_addr(RADIO_CHANNELS + 5), CH_REC_LEN), vec![0xFF; CH_REC_LEN]);
    }

    /// A second program of the same codeplug finds nothing to change.
    #[test]
    fn reprogramming_the_same_codeplug_writes_nothing() {
        let mut radio = radio();
        let plan = plan_from_radio(&radio);
        program(&mut radio, &plan);
        let again = plan_from_radio(&radio);
        let plans = program(&mut radio, &again);
        assert!(plans.is_empty(), "second program rewrote {} windows", plans.len());
    }

    /// The D868's write guard: its contact ID table, not the D578's; no air band.
    #[test]
    fn the_write_guard_is_the_d868s() {
        let name = |a| writable_window_name_for(&D868_MODEL, a);
        assert_eq!(name(CONTACT_ID_TABLE), Some("contact ID table"));
        assert_eq!(name(0x0480_0000), None, "the D578's contact ID table");
        assert_eq!(name(AIR_BASE), None, "the D578's air band");
        assert_eq!(name(0x0450_0000), None, "the D868's call-sign records");
        assert_eq!(name(CHANNEL_BASE), Some("channel bank"));
        let w = vec![0u8; WRITE_WINDOW];
        assert!(check_restore_blocks(&[(CONTACT_ID_TABLE, w.clone())]).is_ok());
        assert!(check_restore_blocks(&[(0x0480_0000, w.clone())]).is_err());
        assert!(check_restore_blocks(&[(AIR_BASE, w)]).is_err());
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

    /// The D868 has no AM receive and no air band: every AM channel is skipped
    /// with a reason, once, and never reaches a channel memory as FM.
    #[test]
    fn am_channels_are_skipped_not_programmed_as_fm() {
        let model = crate::seed::test_model("AT-D868UV");
        let channels = vec![
            chan(1, "Tower", 118.300, "AM"),
            chan(1, "Tower", 118.300, "AM"),
            chan(2, "Calling", 146.520, "FM"),
        ];
        let payload = CodeplugPayload {
            model: &model,
            groups: &[],
            channels: &channels,
            scan_lists: &[],
            scan_list_overrides: &[],
            contact_users: &[],
        };
        let p = plan_for(&D868_MODEL, &payload).unwrap();
        assert!(p.air.is_empty());
        let skipped: Vec<_> = p.base.skipped.iter().filter(|s| s.name == "Tower").collect();
        assert_eq!(skipped.len(), 1, "{:?}", p.base.skipped);
        assert!(skipped[0].reason.contains("no AM receive"), "{}", skipped[0].reason);
        assert!(p.base.channels.iter().all(|c| c.edit.name != "Tower"));
    }

    /// Step-4 gate: the app's own pipeline — `resolve_codeplug_payload` and the
    /// planner — run on a COPY of a real database, programmed into the simulated
    /// anchor radio and read back record by record. Every channel list and scan
    /// list any codeplug in the copy uses goes into one D868 codeplug.
    ///   cp "<app data>/codeplug_manager.sqlite3" /tmp/d868-copy.sqlite3 && \
    ///   CPM_LIVE_DB_COPY=/tmp/d868-copy.sqlite3 \
    ///     cargo test d868_live_db_copy -- --ignored --nocapture
    #[tokio::test]
    #[ignore = "needs CPM_LIVE_DB_COPY pointing at a copy of a real app DB"]
    async fn d868_live_db_copy_programs_and_reads_back() {
        let Ok(path) = std::env::var("CPM_LIVE_DB_COPY") else {
            eprintln!("CPM_LIVE_DB_COPY not set — skipping");
            return;
        };
        let pool = crate::db::init_pool(Path::new(&path)).await.expect("open db copy");
        let (model_id,): (i64,) = sqlx::query_as("SELECT id FROM radio_models WHERE model = 'AT-D868UV'")
            .fetch_one(&pool)
            .await
            .unwrap();
        let profile_id = sqlx::query(
            "INSERT INTO radio_profiles (display_name, radio_model_id) VALUES ('d868 gate', ?1)",
        )
        .bind(model_id)
        .execute(&pool)
        .await
        .unwrap()
        .last_insert_rowid();
        let cp_id = sqlx::query("INSERT INTO codeplugs (name, radio_profile_id) VALUES ('d868 gate', ?1)")
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
        let input_rows = resolved.payload().channels.len();
        let plan = plan_for(&D868_MODEL, &resolved.payload()).expect("plans").base;
        eprintln!(
            "input: {input_rows} expanded channel rows; planned: {} channels, {} zones, {} scan lists, \
             {} contacts; {} skipped, {} warnings",
            plan.channels.len(),
            plan.zones.len(),
            plan.scan_lists.len(),
            plan.contacts.len(),
            plan.skipped.len(),
            plan.warnings.len()
        );
        let mut reasons: BTreeMap<String, usize> = BTreeMap::new();
        for sk in &plan.skipped {
            let key = sk.reason.split(" — ").last().unwrap_or(&sk.reason).to_string();
            *reasons.entry(key).or_default() += 1;
        }
        for (r, n) in &reasons {
            eprintln!("  skipped ×{n}: {r}");
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
            let members: Vec<u16> = read(&radio, ZONE_LIST_BASE + z as u32 * ZONE_LIST_STEP, 500)
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
}

/// ★ The call-sign gate: decode the anchor radio's own database (a full
/// read-only dump, kept out of the repo — it is radioid.net user data) and
/// re-encode it at the D868's addresses. Every block the encoder writes must
/// match the radio's bytes. The dump reads whole banks, so the radio's bytes
/// past the encoder's last frame are not compared.
///   CPM_CSDB_D868=scratchpad/anytone_atd868uv/csdb_full.bin \
///     cargo test --lib d868_csdb_gate -- --ignored --nocapture
#[test]
#[ignore = "needs CPM_CSDB_D868 pointing at a full D868 call-sign DB dump"]
fn d868_csdb_gate_the_radios_database_reencodes_byte_identically() {
    use crate::radios::anytone_atd578uv::callsign_db::{decode_with, encode_with};
    let path = std::env::var("CPM_CSDB_D868").expect("CPM_CSDB_D868");
    let blocks: BTreeMap<u32, Vec<u8>> = parse_dump(&std::fs::read(path).unwrap()).unwrap().into_iter().collect();
    let entries = decode_with(&super::CALLSIGN, &blocks);
    eprintln!("decoded {} entries", entries.len());
    let enc = encode_with(&super::CALLSIGN, &entries).unwrap();
    assert_eq!(enc.entries, entries.len(), "no entry was a duplicate key");
    for (addr, data) in &enc.blocks {
        let radio = &blocks[addr];
        assert!(data.len() <= radio.len(), "block 0x{addr:08X} is longer than the dump read");
        let first = (0..data.len()).find(|&i| data[i] != radio[i]);
        assert_eq!(first, None, "block 0x{addr:08X} differs at +0x{:X}", first.unwrap_or(0));
    }
    eprintln!("{} blocks byte-identical", enc.blocks.len());
}

/// The D868's database lands at its own addresses, never the D578's.
#[test]
fn the_callsign_database_is_written_at_the_d868s_addresses() {
    use crate::radios::anytone_atd578uv::callsign_db::{encode_with, Entry};
    let e = |id, name: &str| Entry {
        dmr_id: id,
        group_call: false,
        fields: [name.as_bytes().to_vec(), vec![], b"CALL".to_vec(), vec![], vec![], vec![]],
    };
    let enc = encode_with(&super::CALLSIGN, &[e(3_100_001, "A"), e(3_100_002, "B")]).unwrap();
    let addrs: Vec<u32> = enc.blocks.iter().map(|(a, _)| *a).collect();
    assert_eq!(addrs, vec![0x0400_0000, 0x044C_0000, 0x0450_0000]);
    let lim = &enc.blocks[1].1;
    assert_eq!(&lim[0..4], &2u32.to_le_bytes());
    let end = u32::from_le_bytes(lim[4..8].try_into().unwrap());
    assert!((0x0450_0000..0x0450_0100).contains(&end), "end 0x{end:08X} is a real address in bank 0");
}
