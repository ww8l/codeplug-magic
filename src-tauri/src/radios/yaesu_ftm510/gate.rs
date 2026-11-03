//! The step-3 gate: decode every record a real FTM-510 wrote into a channel,
//! re-encode it, and compare with the radio's bytes. `#[ignore]`d and env-gated
//! because the inputs are a personal radio's backup in the gitignored scratchpad:
//!
//! ```text
//! FTM510_DAT=../scratchpad/yaesu_ftm510/captures/ftm510_01_new_radio_callsign.dat \
//! FTM500_DAT=../scratchpad/yaesu_ftm510/sources/captures-thirdparty/CLNFTM500D.dat \
//!   cargo test --lib yaesu_ftm510::gate -- --ignored --nocapture
//! ```
//!
//! A factory-fresh radio holds two memories, so the 20 records it wrote in the
//! memory format — memories 1-2, the two live VFO records, and 4 groups × 4
//! bands of VFO / Home records — are the corpus. Two things have no column in
//! the channel database and are masked and COUNTED, never silently ignored:
//! byte 0's used bit on VFO / Home records (they are never "used"), and byte 5's
//! step nibble (a VFO keeps a configured step; the encoder derives one from the
//! frequency). Everything else must match byte for byte.
//!
//! `FTM500_DAT`, if set, runs the same comparison over an FTM-500 file someone
//! edited with ADMS — a sibling, reported, never asserted.

use super::sd_image::{decode_freq, patch_backup, validate_backup, IMAGE_LEN, MEM_SLOTS};
use crate::commands::export::ExpandedChannel;
use crate::models::Channel;
use crate::radios::yaesu_ftm300::sd_image::encode_record;

const TONES: [f64; 50] = [
    67.0, 69.3, 71.9, 74.4, 77.0, 79.7, 82.5, 85.4, 88.5, 91.5, 94.8, 97.4, 100.0, 103.5, 107.2, 110.9,
    114.8, 118.8, 123.0, 127.3, 131.8, 136.5, 141.3, 146.2, 151.4, 156.7, 159.8, 162.2, 165.5, 167.9,
    171.3, 173.8, 177.3, 179.9, 183.5, 186.2, 189.9, 192.8, 196.6, 199.5, 203.5, 206.5, 210.7, 218.1,
    225.7, 229.1, 233.6, 241.8, 250.3, 254.1,
];

/// A record read back into the channel that would produce it. Returns None for
/// a record using something the channel database cannot say (cross tone modes,
/// AMS, a user CTCSS other than the radio's default).
fn record_to_channel(r: &[u8]) -> Option<Channel> {
    let rx = decode_freq(&r[2..5]);
    let shift = u16::from_be_bytes([r[12], r[13]]) as i64 * 50_000;
    let tx = match r[1] & 0x07 {
        0 => rx,
        2 => rx - shift,
        3 => rx + shift,
        4 => decode_freq(&r[6..9]),
        _ => return None,
    };
    if r[1] & 0x40 != 0 || r[11] & 0x1F != 0x0F {
        return None; // AMS, or a user CTCSS this encoder never writes
    }
    let mode = match (r[1] & 0x30, r[11] & 0x60) {
        (0x10, 0) => "AM",
        (0x20, 0) => "NFM",
        (0, 0x40) => "YSF",
        (0, 0x60) => "YSF-VW",
        (0, 0) => "FM",
        _ => return None,
    };
    let tone = TONES[(r[9] & 0x3F) as usize];
    let tone_mode = match r[5] >> 4 {
        0 => "off",
        1 => "Tone",
        2 => "TSQL",
        4 => "DTCS",
        _ => return None,
    };
    let power = ["High", "Med", "Low"][(r[9] >> 6) as usize % 3];
    Some(Channel {
        id: 1,
        rx_freq: rx as f64 / 1e6,
        tx_freq: Some(tx as f64 / 1e6),
        mode: Some(mode.into()),
        tone_mode: Some(tone_mode.into()),
        ctcss_uplink: Some(tone),
        ctcss_downlink: Some(tone),
        dcs_code: Some(String::new()),
        power: Some(power.into()),
        dcs_polarity: "NN".into(),
        cross_mode: "Tone->Tone".into(),
        source: "gate".into(),
        ..Default::default()
    })
}

fn wrap(c: Channel) -> ExpandedChannel {
    ExpandedChannel { channel: c, tg_label: None, timeslot: None, tg_number: None, tg_call_type: None, tg_inline: false }
}

/// (offset, matched, masked-step, masked-used, mismatched)
fn compare(label: &str, img: &[u8], at: usize, used_expected: bool) -> (bool, bool, bool) {
    let r = &img[at..at + 16];
    let Some(c) = record_to_channel(r) else {
        println!("  {label} @{at:#06x}: not expressible as a channel: {:02x?}", r);
        return (false, false, false);
    };
    let mut ours = encode_record(&wrap(c));
    let mut step_masked = false;
    let mut used_masked = false;
    if !used_expected && ours[0] & 0x80 != 0 && r[0] & 0x80 == 0 {
        ours[0] &= 0x7F;
        used_masked = true;
    }
    if ours[5] & 0x0F != r[5] & 0x0F {
        ours[5] = (ours[5] & 0xF0) | (r[5] & 0x0F);
        step_masked = true;
    }
    let ok = ours[..] == r[..];
    if !ok {
        println!("  {label} @{at:#06x}: MISMATCH\n    radio {:02x?}\n    ours  {:02x?}", r, ours);
    }
    (ok, step_masked, used_masked)
}

#[test]
#[ignore]
fn reencodes_a_real_ftm510s_records() {
    let Ok(path) = std::env::var("FTM510_DAT") else {
        println!("FTM510_DAT not set — skipped");
        return;
    };
    let img = std::fs::read(&path).unwrap();
    validate_backup(&img).unwrap();

    let mut sites: Vec<(String, usize, bool)> = Vec::new();
    for s in 0..MEM_SLOTS {
        if img[0x800 + s * 16] & 0x80 != 0 {
            sites.push((format!("memory {}", s + 1), 0x800 + s * 16, true));
        }
    }
    sites.push(("live VFO A".into(), 0x080, false));
    sites.push(("live VFO B".into(), 0x090, false));
    for (name, base) in [("VFO A", 0x100), ("VFO B", 0x150), ("HOME A", 0x1A0), ("HOME B", 0x1F0)] {
        // Band 4 (480+, default 850.000 MHz) is outside this radio; ADMS-18 hides it.
        for band in 0..4 {
            sites.push((format!("{name} band {band}"), base + band * 16, false));
        }
    }
    let (mut ok, mut steps, mut used) = (0, 0, 0);
    for (label, at, u) in &sites {
        let (m, s, us) = compare(label, &img, *at, *u);
        ok += m as usize;
        steps += s as usize;
        used += us as usize;
    }
    println!("{ok}/{} records re-encode ({steps} step nibbles masked, {used} used bits masked)", sites.len());
    assert_eq!(ok, sites.len(), "a record the radio wrote did not re-encode");
    assert!(sites.len() >= 20, "expected at least the 20 factory records, found {}", sites.len());

    // Whole file: patching the radio's own memories back into it changes nothing
    // but the factory empty slots holding step 0 — and the names the export's
    // shared de-duplication gives channels that collide. A factory radio's two
    // memories are identical and nameless, so the second one is named; that is
    // the app's rule for every radio, reported here rather than masked.
    let mems: Vec<ExpandedChannel> = (0..MEM_SLOTS)
        .filter(|s| img[0x800 + s * 16] & 0x80 != 0)
        .map(|s| {
            let mut c = record_to_channel(&img[0x800 + s * 16..0x810 + s * 16]).unwrap();
            c.name_short = None;
            wrap(c)
        })
        .collect();
    let refs: Vec<&ExpandedChannel> = mems.iter().collect();
    let model = crate::seed::test_model("FTM-510DR");
    let names = crate::commands::export::expanded_names(refs.iter().copied(), &model);
    let renamed: Vec<usize> = (0..names.len()).filter(|&s| !names[s].is_empty()).collect();
    for &s in &renamed {
        println!("memory {} named {:?} by the export's de-duplication", s + 1, names[s]);
    }
    let in_renamed_name = |i: usize| {
        renamed.iter().any(|&s| (0x4CB0 + s * 2..0x4CB2 + s * 2).contains(&i) || (0x10000 + s * 16..0x10010 + s * 16).contains(&i))
    };
    let out = patch_backup(&img, &refs, &model).unwrap();
    let diff: Vec<usize> = (0..IMAGE_LEN).filter(|&i| out[i] != img[i] && !in_renamed_name(i)).collect();
    let all_step = diff.iter().all(|&i| i >= 0x800 && (i - 0x800) % 16 == 5 && img[i] == 0 && out[i] == 4);
    println!("whole-file patch: {} other bytes differ, all of them factory step-0 empties: {all_step}", diff.len());
    assert!(all_step && diff.len() <= 14, "{:x?}", &diff[..diff.len().min(20)]);

    if let Ok(p) = std::env::var("FTM500_DAT") {
        let s = std::fs::read(p).unwrap();
        let (mut n, mut good) = (0, 0);
        for slot in 0..MEM_SLOTS {
            let at = 0x800 + slot * 16;
            if s[at] & 0x80 == 0 {
                continue;
            }
            n += 1;
            good += compare(&format!("FTM-500 memory {}", slot + 1), &s, at, true).0 as usize;
        }
        println!("FTM-500 sibling (reported, not asserted): {good}/{n} re-encode");
    }
}

/// The settings table's self-check: decode every ADMS-18 signature-pass file
/// with the generated table and compare with what ADMS-18 DISPLAYED for that
/// file (each control's achieved option, re-read after setting). 14 independent
/// samples per control.
///
/// ```text
/// FTM510_ADMS=../scratchpad/yaesu_ftm510/adms \
///   cargo test --lib yaesu_ftm510::gate::settings -- --ignored --nocapture
/// ```
#[test]
#[ignore]
fn settings_decode_what_adms18_displayed() {
    use serde_json::Value;
    let Ok(dir) = std::env::var("FTM510_ADMS") else {
        println!("FTM510_ADMS not set — skipped");
        return;
    };
    let dir = std::path::PathBuf::from(dir);
    let x: Value = serde_json::from_str(&std::fs::read_to_string(dir.join("xref.json")).unwrap()).unwrap();
    let (mut ok, mut bad, mut n) = (0, 0, 0);
    for k in 0..14 {
        let img = std::fs::read(dir.join(format!("bits/pass{k}.dat"))).unwrap();
        let ach: Value = serde_json::from_str(&std::fs::read_to_string(dir.join(format!("bits/pass{k}.achieved.json"))).unwrap()).unwrap();
        let ours = super::settings::decode_settings(&img);
        n += 1;
        for (src, key) in x["xref"].as_object().unwrap() {
            let Some(idx) = ach["achieved"][src].as_u64() else { continue };
            let shown = x["items"][src][idx as usize].as_str().unwrap();
            let want = match ours[key.as_str().unwrap()] {
                Value::Bool(_) => Value::Bool(shown == "ON"),
                _ => Value::String(shown.to_string()),
            };
            // Options deliberately left out of the table decode as `#raw`.
            if ours[key.as_str().unwrap()] == want {
                ok += 1;
            } else if key.as_str().unwrap().starts_with("function-key") && (idx == 0 || idx == 127) {
                continue;
            } else {
                bad += 1;
                println!("  pass {k} {src} -> {key}: ADMS showed {shown:?}, table decodes {}", ours[key.as_str().unwrap()]);
            }
        }
    }
    println!("{n} files: {ok} agree, {bad} disagree");
    assert_eq!(bad, 0);
    assert!(ok > 1500, "too few comparisons ({ok}) — is the cross-reference empty?");
}
