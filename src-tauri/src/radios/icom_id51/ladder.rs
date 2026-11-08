//! THROWAWAY (#50, step 7): build the hardware-ladder files from the radio's own
//! save, each checked for the ONE property its rung tests before Tim loads it.
//!
//! ```sh
//! CPM_DEV_DB="$HOME/Library/Application Support/com.ww8l.codeplugmagic.dev/codeplug_manager.sqlite3" \
//! CPM_CODEPLUG=2 cargo test --lib icom_id51::ladder -- --ignored --nocapture
//! ```
//! Writes `scratchpad/icom_id51/ladder/LAD*.icf`.

use crate::commands::export::{exclusion_reason, expand_for_export, resolve_codeplug_groups, CodeplugGroup, ExpandedChannel};
use crate::models::Channel;
use crate::radios::driver::ExportRequest;
use crate::radios::icom_id52::icf::IcfFile;

use super::layout::{record_offset, BANK_NAMES, CHANNEL_COUNT, EMPTY_BITMAP, REC_LEN};

fn root() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("..")
}

fn base_text() -> String {
    std::fs::read_to_string(root().join("scratchpad/icom_id51/id51_01_base.icf")).expect("the radio's own file")
}

fn moved(a: &[u8], b: &[u8]) -> Vec<usize> {
    (0..a.len()).filter(|&i| a[i] != b[i]).collect()
}

fn in_memory_tables(i: usize) -> bool {
    i < CHANNEL_COUNT * REC_LEN || (EMPTY_BITMAP..BANK_NAMES + 26 * 16).contains(&i)
}

fn used(image: &[u8]) -> Vec<usize> {
    (0..CHANNEL_COUNT).filter(|&s| image[EMPTY_BITMAP + s / 8] & (1 << (s % 8)) == 0).collect()
}

fn ec(c: Channel) -> ExpandedChannel {
    ExpandedChannel { channel: c, tg_label: None, timeslot: None, tg_number: None, tg_call_type: None, tg_inline: false }
}

fn write(dir: &std::path::Path, name: &str, icf: &IcfFile) {
    let text = icf.render();
    // Every file must read back as an ID-51 Plus file with a verifying #CD.
    let back = IcfFile::parse(&text).expect("re-parses, #CD verifies");
    super::check_is_an_id51_file(&back).unwrap();
    std::fs::write(dir.join(name), text).unwrap();
    println!("  wrote {name}");
}

#[tokio::test]
#[ignore = "needs the dev database and scratchpad/icom_id51/id51_01_base.icf"]
async fn build_the_ladder() {
    let dir = root().join("scratchpad/icom_id51/ladder");
    std::fs::create_dir_all(&dir).unwrap();
    let text = base_text();
    let base = IcfFile::parse(&text).unwrap();
    let b = base.image().to_vec();
    let model = crate::seed::test_model("ID-51 Plus");

    // 1. Identity: the radio's own file, re-rendered by the app. Byte-identical.
    assert_eq!(base.render(), text);
    write(&dir, "LAD1_IDENT.icf", &base);

    // 2. One field: memory 0's name -> "LADDER TEST". Only name bytes move, so
    //    a refusal here and not on rung 1 is the #CD.
    let mut two = base.clone();
    let at = record_offset(0) + 11;
    crate::radios::icom_id52::memory::ascii_field(&mut two.image_mut()[at..at + 16], "LADDER TEST");
    let m = moved(&b, two.image());
    assert!(!m.is_empty() && m.iter().all(|&i| (at..at + 16).contains(&i)), "{m:x?}");
    println!("rung 2: {} name bytes moved, nothing else", m.len());
    write(&dir, "LAD2_RENAME.icf", &two);

    // 3. Full codeplug through the real exporter: HOME-HT from the dev DB.
    let db = std::env::var("CPM_DEV_DB").expect("CPM_DEV_DB");
    let cp: i64 = std::env::var("CPM_CODEPLUG").expect("CPM_CODEPLUG").parse().unwrap();
    let pool = sqlx::SqlitePool::connect(&format!("sqlite:file:{db}?mode=ro")).await.expect("open dev db");
    let groups = resolve_codeplug_groups(&pool, cp).await.unwrap();
    let mut seen = std::collections::HashSet::new();
    let chans: Vec<_> = groups.iter().flat_map(|g| g.channels.iter().cloned()).filter(|c| seen.insert(c.id)).collect();
    let expanded = expand_for_export(&pool, chans).await.unwrap();
    let included: Vec<&ExpandedChannel> = expanded.iter().filter(|e| exclusion_reason(&e.channel, &model).is_none()).collect();
    let tmp = std::env::temp_dir().join("id51_ladder3.icf");
    std::fs::write(&tmp, &text).unwrap();
    let exporter = crate::radios::registry::exporter_for_format("icom_id51_icf").unwrap();
    let n = exporter
        .export(tmp.to_str().unwrap(), &ExportRequest { channels: &included, groups: &groups, model: &model, profile_settings: None, memory_group: &[] })
        .unwrap();
    let three = IcfFile::parse(&std::fs::read_to_string(&tmp).unwrap()).unwrap();
    let _ = std::fs::remove_file(&tmp);
    let _ = std::fs::remove_file(format!("{}.orig", tmp.display()));
    assert_eq!(used(three.image()).len(), n);
    assert!(moved(&b, three.image()).iter().all(|&i| in_memory_tables(i)));
    println!("rung 3: {n} memories; memory list:");
    for s in used(three.image()) {
        let r = super::memory::Record::decode(&three.image()[record_offset(s)..record_offset(s) + REC_LEN]);
        let bank = three.image()[super::layout::BANK_TABLE + 2 * s];
        println!("    {s:3} {:<16} {:>9.4}  bank {}", r.name(), r.rx_hz() / 1e6, (b'A' + (bank & 0x1F)) as char);
    }
    write(&dir, "LAD3_CODEPLG.icf", &three);

    // 4. Band probe: the receive edges of both A/B ranges, plus a 2 m and a
    //    70 cm simplex channel the radio can transmit on.
    let probe: Vec<(f64, &str, &str)> = vec![
        (108.000, "AM", "P108.000 AM"),
        (173.995, "FM", "P173.995"),
        (380.000, "FM", "P380.000"),
        (478.995, "FM", "P478.995"),
        (146.520, "FM", "P146.520 TX"),
        (446.000, "FM", "P446.000 TX"),
    ];
    let pchans: Vec<Channel> = probe
        .iter()
        .enumerate()
        .map(|(i, (f, mode, name))| Channel {
            id: 9000 + i as i64,
            rx_freq: *f,
            mode: Some((*mode).into()),
            name_long: Some((*name).into()),
            name_short: Some((*name).into()),
            dcs_polarity: "NN".into(),
            ..Default::default()
        })
        .collect();
    let pecs: Vec<ExpandedChannel> = pchans.iter().cloned().map(ec).collect();
    let prefs: Vec<&ExpandedChannel> = pecs.iter().collect();
    let pgroups = vec![CodeplugGroup { list_id: 1, list_name: "PROBE".into(), channels: pchans.clone() }];
    let mut four = base.clone();
    let n4 = super::memory::write_memories(four.image_mut(), &prefs, &pgroups, &model).unwrap();
    assert_eq!((n4, used(four.image()).len()), (6, 6));
    assert!(moved(&b, four.image()).iter().all(|&i| in_memory_tables(i)));
    for (s, (f, _, _)) in probe.iter().enumerate() {
        let r = super::memory::Record::decode(&four.image()[record_offset(s)..record_offset(s) + REC_LEN]);
        assert!((r.rx_hz() / 1e6 - f).abs() < 1e-6, "slot {s}");
        println!("rung 4: memory {s} {:<16} {:.4} MHz", r.name(), r.rx_hz() / 1e6);
    }
    write(&dir, "LAD4_BAND.icf", &four);

    // 5. Settings: two values the radio does not have now, nothing else.
    let mut five = base.clone();
    let cur = super::settings::decode_settings(&b);
    println!("rung 5: radio has Beep Level {} and Backlight {}", cur["sounds-beep-level"], cur["display-backlight"]);
    let mut s = serde_json::Map::new();
    s.insert("sounds-beep-level".into(), "8".into());
    s.insert("display-backlight".into(), "ON".into());
    assert_ne!(cur["sounds-beep-level"], "8");
    assert_ne!(cur["display-backlight"], "ON");
    let (applied, notes) = super::settings::apply_settings(five.image_mut(), &s);
    assert_eq!((applied, notes.len()), (2, 0));
    let m5 = moved(&b, five.image());
    assert_eq!(m5, vec![0x1AAB2, 0x1AABE], "only Backlight and Beep Level move");
    write(&dir, "LAD5_SETTNG.icf", &five);
    println!("all five built and checked");
}
