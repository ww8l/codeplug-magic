//! THROWAWAY (#145): the hardware ladder, through the shipping code. Each test
//! is ONE radio operation; the operator does the front-panel step BEFORE a write
//! is started, because nothing here can wait for RCV.
//!
//! ```sh
//! export FT857_PORT=/dev/cu.usbserial-2110 FT857_DIR=../scratchpad/yaesu_ft857d/ladder
//! # read: start it, then press [C] SEND
//! FT857_TAG=before cargo test --lib yaesu_ft857d::hw_ladder::ladder_read -- --ignored --nocapture
//! # write: press [A] RCV first, then start it
//! FT857_IMG=…/x.img cargo test --lib yaesu_ft857d::hw_ladder::ladder_write -- --ignored --nocapture
//! # build a one-name change (pure, no radio)
//! FT857_IMG=…/x.img cargo test --lib yaesu_ft857d::hw_ladder::ladder_make_one_name -- --ignored --nocapture
//! # compare two images
//! FT857_A=… FT857_B=… cargo test --lib yaesu_ft857d::hw_ladder::ladder_compare -- --ignored --nocapture
//! ```

use std::path::PathBuf;

use super::layout::CHANNEL_COUNT;
use super::memory::{decode_channel, is_visible, read_memory, tag_text};
use super::program::{build_image, decode_sample};
use super::protocol::{check_image, open_port, read_clone, write_clone, PATIENCE};
use crate::commands::export::SlotChannel;
use crate::models::RadioModel;

fn env(k: &str) -> String {
    std::env::var(k).unwrap_or_else(|_| panic!("set {k}"))
}

fn dir() -> PathBuf {
    let d = PathBuf::from(std::env::var("FT857_DIR").unwrap_or_else(|_| ".".into()));
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn stamp() -> String {
    chrono::Local::now().format("%Y%m%d-%H%M%S").to_string()
}

fn show(image: &[u8]) {
    for c in decode_sample(image) {
        println!(
            "  {:>3} {:<8} {:>11.5} {:<4} {:<8} {}",
            c.index,
            c.name,
            c.rx_mhz,
            c.mode.unwrap_or_default(),
            c.shift.unwrap_or_default(),
            c.tone
        );
    }
}

#[test]
#[ignore]
fn ladder_read() {
    let mut p = open_port(&env("FT857_PORT")).unwrap();
    println!("listening — press [C] SEND on the radio now");
    let image = read_clone(&mut *p, PATIENCE, &mut |d, t| {
        if d % 10 == 0 || d == t {
            println!("  block {d}/{t}");
        }
    })
    .unwrap();
    let path = dir().join(format!("ft897-ladder-{}-{}.img", env("FT857_TAG"), stamp()));
    std::fs::write(&path, &image).unwrap();
    println!("read {} bytes -> {}", image.len(), path.display());
    show(&image);
}

#[test]
#[ignore]
fn ladder_write() {
    let image = std::fs::read(env("FT857_IMG")).unwrap();
    check_image(&image).unwrap();
    let mut p = open_port(&env("FT857_PORT")).unwrap();
    println!("sending {} bytes — the radio must already be in RCV", image.len());
    write_clone(&mut *p, &image, PATIENCE, &mut |d, t| {
        if d % 10 == 0 || d == t {
            println!("  block {d}/{t} acknowledged");
        }
    })
    .unwrap();
    println!("every block acknowledged — an ACK is not a commit: read it back");
}

/// Ladder step 2's file: the radio's own memories through the channel model,
/// with memory 1 renamed. No radio involved.
#[test]
#[ignore]
fn ladder_make_one_name() {
    let src = env("FT857_IMG");
    let base = std::fs::read(&src).unwrap();
    let new_name = std::env::var("FT857_NAME").unwrap_or_else(|_| "LADDER2".into());
    let slots: Vec<SlotChannel> = (0..CHANNEL_COUNT)
        .filter(|&s| is_visible(&base, s))
        .map(|s| {
            let m = read_memory(&base, s);
            let name = if s == 0 { new_name.clone() } else { tag_text(&m) };
            SlotChannel { slot: s, name, channel: decode_channel(&m) }
        })
        .collect();
    let model = RadioModel { memory_channels: Some(200), ..Default::default() };
    let out = build_image(&model, &slots, &base).unwrap();
    let diff: Vec<usize> = (0..base.len()).filter(|&i| base[i] != out[i]).collect();
    for &i in &diff {
        println!("  0x{i:04X}: {:02X} -> {:02X}", base[i], out[i]);
    }
    assert!(!diff.is_empty() && diff.len() <= 8, "expected only memory 1's tag to change");
    let path = PathBuf::from(&src).with_file_name(format!("ft897-ladder-onename-{}.img", stamp()));
    std::fs::write(&path, &out).unwrap();
    println!("{} bytes differ; wrote {}", diff.len(), path.display());
}

#[test]
#[ignore]
fn ladder_compare() {
    let a = std::fs::read(env("FT857_A")).unwrap();
    let b = std::fs::read(env("FT857_B")).unwrap();
    assert_eq!(a.len(), b.len(), "lengths differ");
    let diff: Vec<usize> = (0..a.len()).filter(|&i| a[i] != b[i]).collect();
    for &i in diff.iter().take(40) {
        println!("  0x{i:04X}: {:02X} -> {:02X}", a[i], b[i]);
    }
    println!("{} bytes differ", diff.len());
}

/// Ladder step 3's file: a codeplug that exercises every encoding the radio's
/// own records have not shown — shift, tone, TSQL, DCS, narrow, USB, CW, AM.
/// Built from FT857_IMG through the shipping `build_image`. No radio involved.
#[test]
#[ignore]
fn ladder_make_codeplug() {
    use crate::models::Channel;
    let src = env("FT857_IMG");
    let base = std::fs::read(&src).unwrap();
    let ch = |rx: f64, tx: f64, mode: &str| Channel {
        rx_freq: rx,
        tx_freq: Some(tx),
        mode: Some(mode.into()),
        dcs_polarity: "NN".into(),
        cross_mode: "Tone->Tone".into(),
        ..Default::default()
    };
    let mut c = Vec::new();
    let mut t = ch(146.94, 146.34, "FM");
    t.tone_mode = Some("Tone".into());
    t.ctcss_uplink = Some(100.0);
    c.push(("RPT T100", t));
    let mut t = ch(442.1, 447.1, "FM");
    t.tone_mode = Some("TSQL".into());
    t.ctcss_uplink = Some(131.8);
    t.ctcss_downlink = Some(131.8);
    c.push(("TSQL1318", t));
    let mut t = ch(147.0, 147.0, "FM");
    t.tone_mode = Some("DTCS".into());
    t.dcs_code = Some("023".into());
    c.push(("DCS 023", t));
    c.push(("NFM CALL", ch(146.52, 146.52, "NFM")));
    c.push(("FT8 20M", ch(14.074, 14.074, "USB")));
    c.push(("LSB 40M", ch(7.185, 7.185, "LSB")));
    c.push(("CW 40M", ch(7.03, 7.03, "CW")));
    c.push(("AIR 1215", ch(121.5, 121.5, "AM")));
    let slots: Vec<SlotChannel> = c
        .into_iter()
        .enumerate()
        .map(|(i, (n, ch))| SlotChannel { slot: i, name: n.into(), channel: ch })
        .collect();
    let model = RadioModel { memory_channels: Some(200), ..Default::default() };
    let out = build_image(&model, &slots, &base).unwrap();
    show(&out);
    let path = PathBuf::from(&src).with_file_name(format!("ft897-ladder-codeplug-{}.img", stamp()));
    std::fs::write(&path, &out).unwrap();
    println!("wrote {}", path.display());
}

/// Ladder step 4's file: one memory at each edge of the receive and transmit
/// coverage the seed row claims (manual spec). Built through `build_image`.
#[test]
#[ignore]
fn ladder_make_bandprobe() {
    use crate::models::Channel;
    let src = env("FT857_IMG");
    let base = std::fs::read(&src).unwrap();
    let probes: [(&str, f64, &str); 12] = [
        ("RX 0.100", 0.1, "AM"),
        ("TX 1.800", 1.8, "LSB"),
        ("TX 29.69", 29.69, "USB"),
        ("TX 50.00", 50.0, "USB"),
        ("RX 55.99", 55.995, "FM"),
        ("RX 76.00", 76.0, "FM"),
        ("RX 107.9", 107.9, "FM"),
        ("RX 118.0", 118.0, "AM"),
        ("RX 163.9", 163.995, "FM"),
        ("RX 420.0", 420.0, "FM"),
        ("TX 449.9", 449.995, "FM"),
        ("RX 469.9", 469.995, "FM"),
    ];
    let slots: Vec<SlotChannel> = probes
        .iter()
        .enumerate()
        .map(|(i, &(n, f, m))| SlotChannel {
            slot: i,
            name: n.into(),
            channel: Channel {
                rx_freq: f,
                tx_freq: Some(f),
                mode: Some(m.into()),
                dcs_polarity: "NN".into(),
                cross_mode: "Tone->Tone".into(),
                ..Default::default()
            },
        })
        .collect();
    let model = RadioModel { memory_channels: Some(200), ..Default::default() };
    let out = build_image(&model, &slots, &base).unwrap();
    show(&out);
    let path = PathBuf::from(&src).with_file_name(format!("ft897-ladder-bandprobe-{}.img", stamp()));
    std::fs::write(&path, &out).unwrap();
    println!("wrote {}", path.display());
}
