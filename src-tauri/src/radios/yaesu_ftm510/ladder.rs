//! THROWAWAY (#151, step 7): build the hardware-ladder files from the radio's
//! own backup. Each file asserts its property before Tim loads it, so a failure
//! at the radio is the radio's answer, not a bad file.
//!
//! ```sh
//! FTM510_DAT=../scratchpad/yaesu_ftm510/captures/ftm510_01_new_radio_callsign.dat \
//! FTM510_OUT=../scratchpad/yaesu_ftm510/ladder CPM_DEV_DB=<copy of the dev db> CPM_CODEPLUG=6 \
//!   cargo test --lib yaesu_ftm510::ladder -- --ignored --nocapture
//! ```
//!
//! | file | step | what changes against the radio's own file |
//! |---|---|---|
//! | `CLNFTM510D01.dat` | 1 identity | nothing — proves the container and the numbered name |
//! | `CLNFTM510D02.dat` | 2 one name | memory 1's name + its name-length entry: **the checksum test** |
//! | `CLNFTM510D03.dat` | 3 full codeplug | codeplug 6 through the app's export pipeline |
//! | `CLNFTM510D04.dat` | 4 band probe | 11 memories at every coverage edge |
//! | `CLNFTM510D05.dat` | 5 settings | four settings, nothing else |

use super::sd_image::{patch_backup, IMAGE_LEN};
use super::settings::{apply_settings, decode_settings};
use crate::commands::export::{exclusion_reason, expand_for_export, resolve_codeplug_groups, ExpandedChannel};
use crate::models::Channel;
use crate::radios::yaesu_ftm300::sd_image::{name_fields, tag};

fn chan(id: i64, name: &str, rx: f64, mode: &str) -> ExpandedChannel {
    ExpandedChannel {
        channel: Channel {
            id,
            name_short: Some(name.into()),
            rx_freq: rx,
            tx_freq: Some(rx),
            mode: Some(mode.into()),
            dcs_polarity: "NN".into(),
            cross_mode: "Tone->Tone".into(),
            source: "ladder".into(),
            ..Default::default()
        },
        tg_label: None,
        timeslot: None,
        tg_number: None,
        tg_call_type: None,
        tg_inline: false,
    }
}

fn diff(a: &[u8], b: &[u8]) -> Vec<usize> {
    (0..a.len()).filter(|&i| a[i] != b[i]).collect()
}

#[tokio::test]
#[ignore = "builds hardware-ladder files from a personal radio's backup"]
async fn build_the_ladder_files() {
    let radio = std::fs::read(std::env::var("FTM510_DAT").expect("FTM510_DAT")).unwrap();
    let out = std::path::PathBuf::from(std::env::var("FTM510_OUT").expect("FTM510_OUT"));
    std::fs::create_dir_all(&out).unwrap();
    assert_eq!(radio.len(), IMAGE_LEN);
    let model = crate::seed::test_model("FTM-510DR");
    let write = |n: u32, img: &[u8]| std::fs::write(out.join(format!("CLNFTM510D{n:02}.dat")), img).unwrap();

    // 1 — identity.
    write(1, &radio);

    // 2 — one name: memory 1 (factory 144.000, no name) becomes "LADDER 2". The
    // name field and its length entry are the only bytes that may move.
    let mut two = radio.clone();
    let (field, code) = name_fields(&tag("LADDER 2"));
    two[0x10000..0x10010].copy_from_slice(&field);
    two[0x4CB0..0x4CB2].copy_from_slice(&code.to_be_bytes());
    let d = diff(&radio, &two);
    println!("step 2: {} bytes differ {:x?}; length code {code:#06x}", d.len(), d);
    assert!(d.iter().all(|&i| (0x10000..0x10010).contains(&i) || (0x4CB0..0x4CB2).contains(&i)));
    write(2, &two);

    // 3 — the full codeplug through the app's own pipeline.
    if let (Ok(db), Ok(cp)) = (std::env::var("CPM_DEV_DB"), std::env::var("CPM_CODEPLUG")) {
        let pool = sqlx::SqlitePool::connect(&format!("sqlite:file:{db}?mode=ro")).await.unwrap();
        let groups = resolve_codeplug_groups(&pool, cp.parse().unwrap()).await.unwrap();
        let mut seen = std::collections::HashSet::new();
        let chans: Vec<_> = groups.iter().flat_map(|g| g.channels.iter().cloned()).filter(|c| seen.insert(c.id)).collect();
        let expanded = expand_for_export(&pool, chans).await.unwrap();
        let included: Vec<&ExpandedChannel> = expanded.iter().filter(|ec| exclusion_reason(&ec.channel, &model).is_none()).collect();
        let three = patch_backup(&radio, &included, &model).unwrap();
        let used = (0..999).filter(|i| three[0x800 + i * 16] & 0x80 != 0).count();
        println!("step 3: {} channels in, {} written, {used} marked used", expanded.len(), included.len());
        for (slot, ec) in included.iter().enumerate() {
            let name = String::from_utf8_lossy(three[0x10000 + slot * 16..0x10010 + slot * 16].split(|&b| b == 0xFF).next().unwrap()).into_owned();
            println!("  {:3} {:>10.4} {:<5} {name}", slot + 1, ec.channel.rx_freq, ec.channel.mode.as_deref().unwrap_or(""));
        }
        assert_eq!(used, included.len());
        assert_eq!(&three[..0x800], &radio[..0x800], "settings must be the radio's");
        write(3, &three);
    } else {
        println!("step 3 skipped: CPM_DEV_DB / CPM_CODEPLUG not set");
    }

    // 4 — band probe: every edge of 108-550 MHz and both sides of each internal
    // boundary (137 / 174 / 400 / 480), 222.1 receive-only. Count what lands.
    let probe = [
        (108.0, "AM"), (136.99375, "AM"), (137.0, "FM"), (173.99375, "FM"), (174.0, "FM"), (222.1, "FM"),
        (399.99375, "FM"), (400.0, "FM"), (479.99375, "FM"), (480.0, "FM"), (549.9875, "FM"),
    ];
    let pc: Vec<ExpandedChannel> = probe.iter().enumerate().map(|(i, (f, m))| chan(i as i64 + 1, &format!("P{f}"), *f, m)).collect();
    let refs: Vec<&ExpandedChannel> = pc.iter().collect();
    let four = patch_backup(&radio, &refs, &model).unwrap();
    for (slot, (f, _)) in probe.iter().enumerate() {
        let r = &four[0x800 + slot * 16..0x810 + slot * 16];
        assert_eq!(super::sd_image::decode_freq(&r[2..5]), (f * 1e6).round() as i64);
        println!("  step 4 memory {:2}: {f:>10} MHz band {} {:02x?}", slot + 1, r[0] & 0x0F, &r[..6]);
    }
    write(4, &four);

    // 5 — settings, each visible on the radio's own screen, nothing else moved.
    let before = decode_settings(&radio);
    let want = serde_json::json!({"lcd-brightness": "MID", "beep-level": "HIGH", "sub-band-mute": "ON", "s-dx": "AUTO"});
    let mut five = radio.clone();
    assert_eq!(apply_settings(&mut five, want.as_object().unwrap()), 4);
    let after = decode_settings(&five);
    for (k, v) in want.as_object().unwrap() {
        println!("  step 5 {k}: {} -> {}", before[k], after[k]);
        assert_eq!(&after[k], v);
    }
    let d = diff(&radio, &five);
    println!("step 5: bytes moved {:x?}", d.iter().map(|&i| (i, radio[i], five[i])).collect::<Vec<_>>());
    write(5, &five);
}
