//! The step-3 gate: re-encode a real radio's memories out of a plaintext reading
//! of its own backup and compare with the bytes the radio wrote.
//!
//! The plaintext is RT Systems' tab-delimited export of the SAME file, so every
//! channel arrives here exactly as the operator programmed it. `#[ignore]`d and
//! env-gated because both inputs are a personal radio's data and live in the
//! gitignored scratchpad:
//!
//! ```text
//! FTM300_DAT=scratchpad/ftm300/captures/ftm300_01_real.dat \
//! FTM300_TAB=scratchpad/ftm300/rts/s01_memories.tab \
//!   cargo test --lib yaesu_ftm300::gate -- --ignored --nocapture
//! ```
//!
//! Three per-channel bits have no column in the channel database, so they are
//! masked and COUNTED rather than compared: skip, M-GRP and clock shift. VW
//! mode is the fourth (the database's YSF becomes DN). Everything else must
//! match byte for byte, names and the name-length table included.

use super::sd_image::{encode_record, patch_backup, MEM_SLOTS};
use crate::commands::export::ExpandedChannel;
use crate::models::{Channel, RadioModel};

fn row_to_channel(cols: &[&str], id: i64) -> Channel {
    let mhz = |s: &str| s.trim().parse::<f64>().unwrap();
    let mode = match cols[5] {
        "AM" => "AM",
        "FM Narrow" => "NFM",
        "DN" | "VW" => "YSF",
        _ => "FM",
    };
    let tone_mode = match cols[8] {
        "Tone" => "Tone",
        "T Sql" => "TSQL",
        "DCS" => "DTCS",
        _ => "off",
    };
    let tone: f64 = cols[9].trim_end_matches(" Hz").parse().unwrap();
    Channel {
        id,
        name_short: Some(cols[7].to_string()),
        rx_freq: mhz(cols[1]),
        tx_freq: Some(mhz(cols[2])),
        mode: Some(mode.into()),
        tone_mode: Some(tone_mode.into()),
        ctcss_uplink: Some(tone),
        ctcss_downlink: Some(tone),
        dcs_code: Some(cols[10].trim().to_string()),
        power: Some(cols[14].replace("Medium", "Med")),
        dcs_polarity: "NN".into(),
        cross_mode: "Tone->Tone".into(),
        source: "gate".into(),
        ..Default::default()
    }
}

#[test]
#[ignore]
fn reencodes_a_real_radios_memories() {
    let (Ok(dat), Ok(tab)) = (std::env::var("FTM300_DAT"), std::env::var("FTM300_TAB")) else {
        eprintln!("set FTM300_DAT and FTM300_TAB");
        return;
    };
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let radio = std::fs::read(root.join(&dat)).unwrap();
    let text = std::fs::read_to_string(root.join(&tab)).unwrap();

    let (mut exact, mut masked_only, mut wrong) = (0, 0, Vec::new());
    for line in text.lines().skip(1) {
        let cols: Vec<&str> = line.split('\t').collect();
        let slot: usize = cols[0].parse().unwrap();
        let ec = ExpandedChannel {
            channel: row_to_channel(&cols, slot as i64),
            tg_label: None,
            timeslot: None,
            tg_number: None,
            tg_call_type: None,
            tg_inline: false,
        };
        let ours = encode_record(&ec);
        let at = 0x800 + (slot - 1) * 16;
        let theirs = &radio[at..at + 16];
        if ours == theirs {
            exact += 1;
            continue;
        }
        let mask = |r: &[u8]| {
            let mut m = r.to_vec();
            m[0] &= !0x30; // skip, M-GRP
            m[10] &= !0x80; // clock shift
            m[11] &= !0x20; // VW
            m
        };
        if mask(&ours) == mask(theirs) {
            masked_only += 1;
        } else {
            wrong.push(format!("slot {slot}: ours {ours:02x?}\n         radio {theirs:02x?}"));
        }
    }
    eprintln!("{exact} exact, {masked_only} differ only in skip/M-GRP/clock-shift/VW, {} wrong", wrong.len());
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));

    // Names and the name-length table: every name the radio holds, run through
    // our tag cleaner and length code, must come back as the radio's own bytes.
    // (Through `patch_backup` the app's duplicate-name disambiguation would
    // rename Tim's six "NCARC"s, which is app behaviour, not encoding.)
    let mut named = 0;
    for slot in 0..MEM_SLOTS {
        let raw = &radio[0x10000 + slot * 16..0x10000 + slot * 16 + 16];
        let code = u16::from_be_bytes([radio[0x4CB0 + slot * 2], radio[0x4CB1 + slot * 2]]);
        let text = String::from_utf8_lossy(raw.split(|&b| b == 0xFF).next().unwrap()).into_owned();
        let (ours_raw, ours_code) = super::sd_image::name_fields(&super::sd_image::tag(&text));
        assert_eq!((&ours_raw[..], ours_code), (raw, code), "slot {} name {text:?}", slot + 1);
        named += usize::from(!text.is_empty());
    }
    eprintln!("{named} names and all {MEM_SLOTS} length codes identical");

    // The real patcher keeps the 0x400 block RT Systems would blank.
    let model = RadioModel { max_name_length: Some(16), ..Default::default() };
    let img = patch_backup(&radio, &[], &model, &Default::default()).unwrap();
    assert_eq!(&img[0x400..0x480], &radio[0x400..0x480]);
}
