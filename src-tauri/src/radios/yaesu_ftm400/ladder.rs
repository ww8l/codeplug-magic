//! THROWAWAY (#52, step 7): build the ONE hardware-verification file from the
//! radio's own backup. The FTM-400's restore menu has no file list — Read from
//! SD → ALL loads `CLNFTM400D.dat` and nothing else — so every load is a card
//! trip, and Tim's call is one file that verifies everything at once. Each
//! property is asserted in the file before it goes on the card, so a failure at
//! the radio is the radio's answer, not a bad file.
//!
//! ```sh
//! FTM400_DAT=../scratchpad/yaesu_ftm400/captures/CLNFTM400D_card_20260201.dat \
//! FTM400_OUT=../scratchpad/yaesu_ftm400/ladder \
//! CPM_DEV_DB="$HOME/Library/Application Support/com.ww8l.codeplugmagic.dev/codeplug_manager.sqlite3" CPM_CODEPLUG=2 \
//!   cargo test --lib yaesu_ftm400::ladder -- --ignored --nocapture
//! ```
//!
//! `CLNFTM400D_VERIFY.dat`: codeplug 2 through the app's export pipeline, then
//! a 19-point band probe (every coverage edge, the US cellular gaps, the four
//! 8.33 kHz airband codes), then five settings visible on the radio's screens.

use super::sd_image::{decode_freq, decode_name, patch_backup, BANKS, IMAGE_LEN, MEM_SLOTS, MEM_STRIDE, NAME_LEN};
use super::settings::{apply_settings, decode_settings};
use crate::commands::export::{exclusion_reason, expand_for_export, resolve_codeplug_groups, ExpandedChannel};
use crate::models::Channel;

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

const PROBE: [(f64, &str); 19] = [
    (108.0, "AM"), (127.80833, "AM"), (127.81667, "AM"), (127.83333, "AM"), (127.84167, "AM"), (136.99375, "AM"),
    (137.0, "FM"), (173.99375, "FM"), (174.0, "FM"), (222.1, "FM"), (399.99375, "FM"), (400.0, "FM"),
    (479.99375, "FM"), (480.0, "FM"), (823.9875, "FM"), (851.0125, "FM"), (868.9875, "FM"), (894.0, "FM"), (999.99, "FM"),
];

#[tokio::test]
#[ignore = "builds the hardware-verification file from a personal radio's backup"]
async fn build_the_verification_file() {
    let radio = std::fs::read(std::env::var("FTM400_DAT").expect("FTM400_DAT")).unwrap();
    let out = std::path::PathBuf::from(std::env::var("FTM400_OUT").expect("FTM400_OUT"));
    std::fs::create_dir_all(&out).unwrap();
    assert_eq!(radio.len(), IMAGE_LEN);
    let model = crate::seed::test_model("FTM-400");

    let db = std::env::var("CPM_DEV_DB").expect("CPM_DEV_DB");
    let cp: i64 = std::env::var("CPM_CODEPLUG").expect("CPM_CODEPLUG").parse().unwrap();
    let pool = sqlx::SqlitePool::connect(&format!("sqlite:file:{db}?mode=ro")).await.unwrap();
    let groups = resolve_codeplug_groups(&pool, cp).await.unwrap();
    let mut seen = std::collections::HashSet::new();
    let chans: Vec<_> = groups.iter().flat_map(|g| g.channels.iter().cloned()).filter(|c| seen.insert(c.id)).collect();
    let expanded = expand_for_export(&pool, chans).await.unwrap();
    let included: Vec<&ExpandedChannel> = expanded.iter().filter(|ec| exclusion_reason(&ec.channel, &model).is_none()).collect();
    let probe: Vec<ExpandedChannel> =
        PROBE.iter().enumerate().map(|(i, (f, m))| chan(i as i64 + 1, &format!("{f:.4}")[..8], *f, m)).collect();
    let mut all: Vec<&ExpandedChannel> = included.clone();
    all.extend(probe.iter());

    let mut img = patch_backup(&radio, &all, &model).unwrap();
    let want = serde_json::json!({
        "background-color": "BLUE", "time-vdd": "TIME", "auto-dialer": "OFF", "time-format": "12 hour", "beep": "HIGH",
    });
    assert_eq!(apply_settings(&mut img, want.as_object().unwrap()), 5);
    let s = decode_settings(&img);
    for (k, v) in want.as_object().unwrap() {
        assert_eq!(&s[k], v, "{k}");
    }
    assert_eq!((&s["rx-coverage-a"], &s["rx-coverage-b"]), (&"WIDE".into(), &"WIDE".into()), "the probe needs both bands WIDE");
    assert_eq!(&img[..0x80], &radio[..0x80], "header and its checksum are the radio's");

    for (b, (bank, n)) in BANKS.iter().zip([all.len(), all.len()]).enumerate() {
        let used = (0..MEM_SLOTS).filter(|i| img[bank.mem + i * MEM_STRIDE] & 0x80 != 0).count();
        assert_eq!(used, n);
        // No holes: the used records are exactly slots 1..=n.
        assert!((0..n).all(|i| img[bank.mem + i * MEM_STRIDE] & 0x80 != 0));
        println!("band {}: {n} memories", "AB".as_bytes()[b] as char);
        for i in 0..n {
            let r = &img[bank.mem + i * MEM_STRIDE..bank.mem + (i + 1) * MEM_STRIDE];
            let name = decode_name(&img[bank.name + i * NAME_LEN..bank.name + (i + 1) * NAME_LEN]);
            if i >= n.saturating_sub(19) {
                println!("  {:3} {name:8} {:>12.5}", i + 1, decode_freq(&r[2..5]) as f64 / 1e6);
            }
        }
    }
    std::fs::write(out.join("CLNFTM400D_VERIFY.dat"), &img).unwrap();
}
