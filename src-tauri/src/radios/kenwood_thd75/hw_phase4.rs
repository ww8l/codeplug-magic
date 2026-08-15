//! THROWAWAY (issue #40, Phase 4): build the files the hardware session loads.
//!
//! Not a unit test — a file factory, in the style of [`super::dev_export`] and
//! the AnyTone diagnostics. Everything Phase 4 asks of the radio is a *load*,
//! and a load needs a file with the radio's own name shape sitting in
//! `KENWOOD/TH-D75/SETTINGS/DATA/`. This writes them, and asserts on the way out
//! the property each step is supposed to be testing — so a step that fails on
//! the radio has failed on the radio, not because the file was wrong.
//!
//! ```sh
//! cargo test --lib kenwood_thd75::hw_phase4 -- --ignored --nocapture
//! ```
//!
//! Output lands in `scratchpad/thd75/phase4/` unless `CPM_OUT` says otherwise.
//! The names carry the step number in their seconds field so the radio's Load
//! Setting list, which shows nothing but file names, can still be read:
//!
//! | file | step | proves |
//! |---|---|---|
//! | `08152026_100001.d75` | 1 | the container — a byte-identical copy of a radio save |
//! | `08152026_100002.d75` | 2 | patching — one memory name changed, nothing else |
//! | `08152026_100003.d75` | 3 | the codeplug — written by `dev_export`, not here |
//! | `08152026_100004.d75` | 4 | the band codes — eight memories at deliberate edges |
//!
//! ⚠ Steps 2–4 replace every memory on the radio. The restore is one of Tim's
//! own saves in `scratchpad/thd75/card/`, which is why step 1 exists first.

use super::d75::D75File;
use super::memory::{write_codeplug, MAX_NAME};
use crate::commands::export::{CodeplugGroup, ExpandedChannel};
use crate::models::{Channel, RadioModel};

/// A real radio save — the template every step patches. `#[ignore]`d with the
/// rest: it is a dump of a personal radio, under gitignored `scratchpad/`.
const REAL_SAVE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../scratchpad/thd75/card/08142026_204448.d75"
);

const NAMES: usize = 0x10000;
const NAME_LEN: usize = 16;
const FLAGS: usize = 0x2000;
const FLAG_LEN: usize = 4;

fn model() -> RadioModel {
    RadioModel {
        display_name: "Kenwood TH-D75".into(),
        max_name_length: Some(MAX_NAME as i64),
        ..RadioModel::default()
    }
}

fn out_dir() -> std::path::PathBuf {
    let dir = std::env::var("CPM_OUT").map_or_else(
        |_| {
            std::path::PathBuf::from(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../scratchpad/thd75/phase4"
            ))
        },
        std::path::PathBuf::from,
    );
    std::fs::create_dir_all(&dir).expect("create the output directory");
    dir
}

fn name_at(body: &[u8], slot: usize) -> String {
    let at = NAMES + slot * NAME_LEN;
    body[at..at + NAME_LEN]
        .iter()
        .take_while(|&&b| b != 0)
        .map(|&b| b as char)
        .collect()
}

/// Every byte position at which two equal-length images differ.
fn diff_offsets(a: &[u8], b: &[u8]) -> Vec<usize> {
    assert_eq!(a.len(), b.len(), "images are different lengths");
    (0..a.len()).filter(|&i| a[i] != b[i]).collect()
}

/// Step 1 — the identity write. Parse a save and emit it again; the result must
/// be the same bytes, or the container is losing something. This is also what
/// settles the checksum question with nothing at risk: if the radio loads its
/// own bytes back under a different name, the header carries no digest over the
/// body, and the name is not part of one either.
#[test]
#[ignore = "writes the Phase 4 hardware files; needs a real .d75"]
fn step1_identity_copy_is_byte_for_byte() {
    let raw = std::fs::read(REAL_SAVE).expect("the real save");
    let file = D75File::parse(&raw).expect("parse");
    let out = file.to_bytes();
    assert_eq!(
        diff_offsets(&raw, &out).len(),
        0,
        "the container did not round-trip"
    );

    let path = out_dir().join("08152026_100001.d75");
    std::fs::write(&path, &out).expect("write");
    println!("step 1  {}  ({} bytes, identical)", path.display(), out.len());
}

/// Step 2 — the one-byte write. A single memory name changes and nothing else
/// does. The assertion is the point: the diff must be confined to slot 0's
/// 16-byte name field, so if the radio rejects the file or shows the old name,
/// the file is not what is wrong.
#[test]
#[ignore = "writes the Phase 4 hardware files; needs a real .d75"]
fn step2_one_name_changes_and_nothing_else_does() {
    let raw = std::fs::read(REAL_SAVE).expect("the real save");
    let mut file = D75File::parse(&raw).expect("parse");

    let was = name_at(file.body(), 0);
    // Not "P4 …": step 4's memories are named P1–P8, and a name that could be
    // mistaken for one of those defeats the point of reading the screen.
    let now = "STEP2 ONE BYTE";
    let at = NAMES; // slot 0's name
    file.body_mut()[at..at + NAME_LEN].fill(0);
    file.body_mut()[at..at + now.len()].copy_from_slice(now.as_bytes());

    let out = file.to_bytes();
    let diffs = diff_offsets(&raw, &out);
    let window = at + 0x100..at + 0x100 + NAME_LEN; // the body sits after the header
    assert!(
        diffs.iter().all(|d| window.contains(d)),
        "changed {} bytes outside slot 0's name: {:?}",
        diffs.iter().filter(|d| !window.contains(d)).count(),
        diffs.iter().filter(|d| !window.contains(d)).take(8).collect::<Vec<_>>()
    );
    assert!(!diffs.is_empty(), "nothing changed at all");

    let path = out_dir().join("08152026_100002.d75");
    std::fs::write(&path, &out).expect("write");
    println!(
        "step 2  {}  (memory 0 name {:?} -> {:?}, {} bytes differ)",
        path.display(),
        was,
        now,
        diffs.len()
    );
}

/// Step 4 — the band probe.
///
/// Eight memories at deliberate edges, each named after its own frequency so
/// the radio's memory list answers the question by itself: count what came back
/// and read which ones are missing. Two open questions ride on it.
///
/// **The 220 MHz one.** The ID-52 silently dropped every 224 MHz memory while
/// the app reported success ([[radio-tx-vs-rx-bands]]). The TH-D75 transmits
/// there, so P2 must survive; if it does not, `covers_220` is wrong.
///
/// **Band versus mode.** `used_flag` writes `7` for anything outside Band A,
/// but the only Band-B memories in the sample were also its only AM ones, so
/// the two are confounded. P4 is Band-B-only in **FM** and P7 is Band-B-only in
/// **AM** at the far end of the range: if both survive, the byte is about the
/// band, and if only the AM ones do, it is not.
#[test]
#[ignore = "writes the Phase 4 hardware files; needs a real .d75"]
fn step4_band_probe_covers_every_band_code() {
    // (name, rx, mode, and for the one repeater: duplex/offset/tone)
    let probes: Vec<Channel> = vec![
        chan(1, "P1 145.000 FM", 145.000, "FM"),
        chan(2, "P2 224.520 FM", 224.520, "FM"),
        chan(3, "P3 446.000 FM", 446.000, "FM"),
        chan(4, "P4 52.525 FM", 52.525, "FM"),
        chan(5, "P5 122.8 AM", 122.800, "AM"),
        chan(6, "P6 162.55 FM", 162.550, "FM"),
        chan(7, "P7 27.185 AM", 27.185, "AM"),
        {
            let mut c = chan(8, "P8 449.6 -5 T100", 449.600, "FM");
            c.duplex = Some("-".into());
            c.offset = Some(5.0);
            c.tone_mode = Some("tone".into());
            c.ctcss_uplink = Some(100.0);
            c
        },
    ];

    let group = CodeplugGroup {
        list_id: 1,
        list_name: "PROBE".into(),
        channels: probes.clone(),
    };
    let expanded: Vec<ExpandedChannel> = probes.into_iter().map(expanded).collect();
    let refs: Vec<&ExpandedChannel> = expanded.iter().collect();

    let raw = std::fs::read(REAL_SAVE).expect("the real save");
    let mut file = D75File::parse(&raw).expect("parse");
    let n = write_codeplug(&mut file, &refs, std::slice::from_ref(&group), &model())
        .expect("write the probe");
    assert_eq!(n, refs.len());

    // The prediction, printed so the radio's screen can be checked against it
    // rather than against a memory of what we expected.
    println!("step 4  band codes written:");
    for (slot, ec) in expanded.iter().enumerate() {
        println!(
            "  {:>2}  {:<17} {:>9.4}  band {}  group {}",
            slot + 1,
            name_at(file.body(), slot),
            ec.channel.rx_freq,
            file.body()[FLAGS + slot * FLAG_LEN],
            file.body()[FLAGS + slot * FLAG_LEN + 2],
        );
    }
    // Slot 8 is the last memory; slot 9 must read as empty, or the radio will
    // show leftovers from Tim's codeplug and the count means nothing.
    assert_eq!(file.body()[FLAGS + 8 * FLAG_LEN], 0xFF, "slot 9 not cleared");

    let path = out_dir().join("08152026_100004.d75");
    std::fs::write(&path, file.to_bytes()).expect("write");
    println!("step 4  {}", path.display());
}

fn chan(id: i64, name: &str, rx: f64, mode: &str) -> Channel {
    Channel {
        id,
        name_short: Some(name.into()),
        rx_freq: rx,
        mode: Some(mode.into()),
        ..Channel::default()
    }
}

fn expanded(c: Channel) -> ExpandedChannel {
    ExpandedChannel {
        channel: c,
        tg_label: None,
        timeslot: None,
        tg_number: None,
        tg_call_type: None,
        tg_inline: false,
    }
}
