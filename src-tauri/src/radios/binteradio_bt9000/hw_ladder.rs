//! THROWAWAY (issue #43): hardware ladder steps 3, 4 and 5 for the BT-9000.
//!
//! Steps 1 and 2 — identity write and a one-name write — were run with the
//! scratch tooling and passed; they proved the container and that there is no
//! checksum. What they did NOT prove is that *this driver's* encoder produces
//! an image the radio accepts, so everything here goes through the shipping
//! code: [`patch_image`], [`handshake`], [`upload`], [`download`]. A harness
//! with its own encoder would show the radio accepts something; it would not
//! show that this driver can program it.
//!
//! ```sh
//! CPM_BT9000_PORT=/dev/cu.usbserial-10 \
//!   cargo test --lib binteradio_bt9000::hw_ladder -- --ignored --nocapture
//! ```
//!
//! ⚠ All three tests WRITE to the radio. Each takes its own backup first and prints
//! the path. ⚠ And an ACK from this radio does not mean a commit — every
//! assertion below is made against a fresh read-back, never against the write.

use std::path::PathBuf;

use super::*;
use crate::models::Channel;

fn port() -> String {
    std::env::var("CPM_BT9000_PORT").expect("set CPM_BT9000_PORT to the radio's serial port")
}

fn backup_dir() -> PathBuf {
    PathBuf::from(std::env::var("CPM_BT9000_DIR").unwrap_or_else(|_| ".".to_string()))
}

fn chan(rx: f64, tx: f64) -> Channel {
    Channel {
        rx_freq: rx,
        tx_freq: Some(tx),
        mode: Some("FM".to_string()),
        power: Some("High".to_string()),
        ..Default::default()
    }
}

fn slot(slot: usize, name: &str, rx: f64, tx: f64) -> SlotChannel {
    SlotChannel { slot, name: name.to_string(), channel: chan(rx, tx) }
}

/// Read, back up, patch with `slots`, write, and read back. Returns the image
/// the radio holds afterwards.
fn program(slots: &[SlotChannel], tag: &str) -> Vec<u8> {
    // A DELIBERATELY permissive model, so every probe goes out transmit-enabled.
    // The seeded model would mark an out-of-band probe receive-only and write it
    // with the PTT disabled — which is correct for a codeplug and would defeat
    // the band probe, whose entire question is whether the radio keys there.
    let model = RadioModel {
        analog_capable: true,
        tx_bands: Some("[[1.0,1000.0]]".to_string()),
        rx_bands: Some("[[1.0,1000.0]]".to_string()),
        freq_min: Some(1.0),
        freq_max: Some(1000.0),
        ..Default::default()
    };
    let port = port();
    let mut p = open_port(&port).expect("open the port");

    let hs = handshake(&mut *p).expect("handshake");
    println!("  model {:?}, F blob {}", hs.model, hex(&hs.probe));
    let base = download(&mut *p, &hs).expect("download");

    let backup = backup_dir().join(format!(
        "bt9000-hwladder-{tag}-{}.img",
        chrono::Local::now().format("%Y%m%d-%H%M%S")
    ));
    std::fs::write(&backup, &base).expect("write the backup");
    println!("  backup: {}", backup.display());

    let mut image = base.clone();
    patch_image(&mut image, slots, &model);

    std::thread::sleep(SETTLE);
    let hs = handshake(&mut *p).expect("re-handshake before writing");
    upload(&mut *p, &hs, &image).expect("upload");

    std::thread::sleep(SETTLE);
    let hs = handshake(&mut *p).expect("re-handshake before reading back");
    let after = download(&mut *p, &hs).expect("read back");

    // Only the regions we own. The VFO journal is the radio's, and comparing it
    // would report a difference after every single write.
    for seg in WRITE_SEGMENTS {
        let r = seg.file_offset..seg.file_offset + seg.length;
        assert_eq!(
            image[r.clone()],
            after[r],
            "segment {} did not come back as written",
            seg.name
        );
    }
    after
}

/// Ladder step 3 — a full codeplug, spread so that **every zone** is exercised.
///
/// The step-3 rule is to check the memory list in every zone, not just the one
/// the radio powers up on. Here that is mechanical: this radio's zones are
/// index arithmetic, so a channel in the first and last slot of each of the 15
/// zones proves the whole 960-slot map at once.
#[test]
#[ignore = "writes to a real BT-9000 on the cable"]
fn step3_full_codeplug_reaches_every_zone() {
    let mut slots = Vec::new();
    for zone in 0..ZONE_COUNT {
        let base = zone * CHANNELS_PER_ZONE;
        // First and last slot of the zone, on distinguishable frequencies so a
        // misplaced channel is visible on the radio rather than merely absent.
        slots.push(slot(base, &format!("Z{:02}FIRST", zone + 1), 145.0 + zone as f64 * 0.1, 145.0 + zone as f64 * 0.1));
        slots.push(slot(
            base + CHANNELS_PER_ZONE - 1,
            &format!("Z{:02}LAST", zone + 1),
            440.0 + zone as f64 * 0.1,
            440.0 + zone as f64 * 0.1,
        ));
    }
    let after = program(&slots, "step3");

    let decoded = decode_channels(&after);
    assert_eq!(decoded.len(), slots.len(), "channel count on the radio");

    for zone in 1..=ZONE_COUNT {
        let in_zone: Vec<_> = decoded.iter().filter(|c| c.zone == zone).collect();
        assert_eq!(in_zone.len(), 2, "zone {zone} should hold 2 channels");
        println!(
            "  zone {zone:2}: {} @ {:.4}   {} @ {:.4}",
            in_zone[0].name, in_zone[0].rx_mhz, in_zone[1].name, in_zone[1].rx_mhz
        );
    }
    println!("\n  CHECK ON THE RADIO: every one of the {ZONE_COUNT} zones holds Z<n>FIRST and Z<n>LAST.");
}

/// Ladder step 4 — the band probe, and ★ the measurement that turned out to be
/// impossible from the image.
///
/// The usual shape of this step is: write a channel at each band edge, read
/// back, and see which became a silently empty slot. On most radios an
/// out-of-coverage frequency is dropped, and that is how coverage gets mapped.
///
/// **Not on this one.** All 13 probes survive, 27.5 MHz and 580 MHz included —
/// frequencies no source claims this radio can even receive. That is the same
/// behaviour measured in the settings block, where it stored `127` in fields
/// whose maxima are 9, 2, 3 and 1: **this radio validates nothing it is
/// written.** It is a store, not a filter.
///
/// So a passing run here proves the encoder and transport round-trip cleanly,
/// and says *nothing whatever* about band coverage. `tx_bands` can only be
/// widened by selecting each channel on the radio and confirming it tunes and
/// keys up. Until then the seed stays deliberately narrow: an over-claimed
/// `tx_bands` writes a memory the radio cannot use while reporting success.
#[test]
#[ignore = "writes to a real BT-9000 on the cable"]
fn step4_band_probe() {
    // (frequency, what it tests)
    let probes: &[(f64, &str)] = &[
        (27.500, "CB — the vendor's web copy claims TX here"),
        (50.125, "6 m — inside the claimed 18-64 of the reference driver"),
        (108.000, "airband bottom — RX only if present at all"),
        (136.000, "VHF low edge, manual-stated"),
        (145.100, "2 m, known good (the radio shipped with it)"),
        (174.000, "VHF high edge, manual-stated"),
        (200.000, "F-blob third pair, low edge"),
        (223.500, "1.25 m — the band the 'Work Band' menu hints at"),
        (260.000, "F-blob third pair, high edge"),
        (400.000, "UHF low edge, manual-stated"),
        (431.100, "70 cm, known good (the radio shipped with it)"),
        (520.000, "UHF high edge, manual-stated"),
        (580.000, "above every claim — expected to fail"),
    ];
    let slots: Vec<SlotChannel> = probes
        .iter()
        .enumerate()
        .map(|(i, (mhz, _))| slot(i, &format!("B{:03}", *mhz as u32), *mhz, *mhz))
        .collect();

    let after = program(&slots, "step4");
    let decoded = decode_channels(&after);

    println!("\n  landed  frequency   note");
    let mut landed = Vec::new();
    for (i, (mhz, note)) in probes.iter().enumerate() {
        let got = decoded.iter().find(|c| c.index == i);
        let ok = got.map(|c| (c.rx_mhz - mhz).abs() < 1e-6).unwrap_or(false);
        if ok {
            landed.push(*mhz);
        }
        println!(
            "  {:^6}  {mhz:>9.3}   {note}{}",
            if ok { "yes" } else { "NO" },
            match got {
                Some(c) if !ok => format!("  [stored as {:.3}]", c.rx_mhz),
                None => "  [slot is empty]".to_string(),
                _ => String::new(),
            }
        );
    }
    println!(
        "\n  {} of {} probes survived the round trip.",
        landed.len(),
        probes.len()
    );

    // The two frequencies the radio itself shipped with must always survive.
    assert!(landed.contains(&145.100), "2 m round-trip");
    assert!(landed.contains(&431.100), "70 cm round-trip");

    // ★ The finding, asserted so it cannot quietly stop being true: this radio
    // accepts EVERYTHING, so the image is not a band filter and this test can
    // never map coverage. If a probe ever does fail to survive, the radio has
    // started validating and the band question becomes answerable here — which
    // is worth knowing loudly rather than passing silently.
    assert_eq!(
        landed.len(),
        probes.len(),
        "this radio has always stored every frequency written to it, in or out of band; \
         a failure here means that changed and the band map can now be measured"
    );
    println!(
        "  ★ Every probe survived, 27.5 and 580 MHz included. This radio stores what it is\n  \
           given without validating it, so the IMAGE cannot settle tx_bands. Only selecting\n  \
           each channel on the radio and keying up can."
    );
}

/// Ladder step 5 — the settings spot check, through the driver's own
/// `SettingsWriter` rather than the scratch Python.
///
/// The Python tooling in `scratchpad/binteradio_bt9000/` has already put values
/// in this block and read them back, so this is not asking whether the radio
/// stores settings — it is asking whether *this driver's* narrowed write does.
/// That is a different question, and it is the one step 3 had to be re-run to
/// answer for channels.
///
/// What it proves, all against a fresh read-back and never against an ACK:
///
/// 1. `write_settings` reaches the radio at all.
/// 2. It writes **only the function block** — every other segment is compared
///    against the pre-write backup and must be untouched. On a driver whose
///    whole-image `upload` would rewrite 960 channel records to change a
///    squelch level, that is the assertion that matters.
/// 3. `read_settings` decodes back exactly what was asked for, including the
///    two "Level 1-9" fields two bytes apart that store their values
///    differently.
///
/// ⚠ It restores the settings it found before returning, so the radio is left
/// as it was even though the backup would also serve.
#[test]
#[ignore = "writes to a real BT-9000 on the cable"]
fn step5_settings_write_through_the_driver() {
    use crate::radios::driver::{SettingsReader, SettingsWriter};
    use serde_json::json;

    let port = port();
    let dir = backup_dir();
    let schema = crate::seed::BT9000_SETTINGS_SCHEMA;

    // 1. What the radio holds now, decoded by the shipping reader, plus a whole
    //    image to compare every untouched segment against later.
    let before = DRIVER.read_settings(&port, schema).expect("read settings");
    println!("  before: {}", before.settings);
    let base = before.backup.clone();

    // 2. A value in every settled field that is NOT what the radio has, so a
    //    field that did not move is distinguishable from one that did.
    let want = json!({
        "squelch": if before.settings["squelch"] == json!(3) { 7 } else { 3 },
        "vox-level": if before.settings["vox-level"] == json!(4) { 8 } else { 4 },
        "power-on-display":
            if before.settings["power-on-display"] == json!("Voltage") { "Picture" } else { "Voltage" },
    });
    println!("  writing: {want}");

    std::thread::sleep(SETTLE);
    let report = DRIVER
        .write_settings(&port, &want, schema, &dir)
        .expect("write settings");
    println!(
        "  wrote {} field(s), verified {:?}, backup {}",
        report.fields_written, report.verified, report.backup_path
    );
    assert_eq!(report.fields_written, 3, "every settled field should be written");
    // ⚠ This radio acknowledges blocks it does not always commit — the APRS
    // block answers 0x06 forever and never changes — so the driver's own
    // read-back verdict is the claim under test, not a formality.
    assert_eq!(report.verified, Some(true), "the driver's read-back must confirm the write");

    // 3. Read it back independently of the write session.
    std::thread::sleep(SETTLE);
    let after = DRIVER.read_settings(&port, schema).expect("read settings back");
    println!("  after:  {}", after.settings);
    for key in ["squelch", "vox-level", "power-on-display"] {
        assert_eq!(after.settings[key], want[key], "{key} did not come back as written");
    }

    // 4. ★ Nothing outside the function block moved. This is the assertion that
    //    a narrowed write exists for: the operator's 960 channels, the VFO, the
    //    DTMF codes and the modulation memories are all still exactly as read.
    for seg in READ_SEGMENTS {
        if seg.name == "function" {
            continue;
        }
        let r = seg.file_offset..seg.file_offset + seg.length;
        assert_eq!(
            base[r.clone()],
            after.backup[r],
            "segment {} changed during a SETTINGS write",
            seg.name
        );
    }
    println!("  ★ every segment but `function` is byte-identical to the pre-write image.");

    // 5. Put back what the radio had, so the campaign's own baseline survives.
    std::thread::sleep(SETTLE);
    let restored = DRIVER
        .write_settings(&port, &before.settings, schema, &dir)
        .expect("restore the original settings");
    assert_eq!(restored.verified, Some(true), "restore must verify");
    println!("  restored the settings the radio started with.");
}
