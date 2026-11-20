//! The step-3 gate: decode every memory a real FTM-400 holds into a channel,
//! re-encode it, and compare with the radio's bytes — record AND name.
//! `#[ignore]`d and env-gated because the input is a personal radio's backup in
//! the gitignored scratchpad:
//!
//! ```text
//! FTM400_DAT=../scratchpad/yaesu_ftm400/captures/CLNFTM400D_card_20260201.dat \
//!   cargo test --lib yaesu_ftm400::gate -- --ignored --nocapture
//! ```
//!
//! Three things have no column in the channel database and are masked and
//! COUNTED, never silently ignored: byte 0's skip bits, byte 11's PR FREQ tone
//! (most of Tim's memories carry 300 Hz, the encoder writes the radio's own
//! 1500 Hz), and byte 5's step nibble. Everything else must match byte for byte.

use super::sd_image::{decode_freq, decode_name, encode_record, name_field, tag, validate_backup, BANKS, MEM_SLOTS, MEM_STRIDE, NAME_LEN};
use crate::commands::export::ExpandedChannel;
use crate::models::Channel;

const TONES: [f64; 50] = [
    67.0, 69.3, 71.9, 74.4, 77.0, 79.7, 82.5, 85.4, 88.5, 91.5, 94.8, 97.4, 100.0, 103.5, 107.2, 110.9,
    114.8, 118.8, 123.0, 127.3, 131.8, 136.5, 141.3, 146.2, 151.4, 156.7, 159.8, 162.2, 165.5, 167.9,
    171.3, 173.8, 177.3, 179.9, 183.5, 186.2, 189.9, 192.8, 196.6, 199.5, 203.5, 206.5, 210.7, 218.1,
    225.7, 229.1, 233.6, 241.8, 250.3, 254.1,
];
const DCS: [&str; 104] = [
    "023", "025", "026", "031", "032", "036", "043", "047", "051", "053", "054", "065", "071", "072", "073", "074",
    "114", "115", "116", "122", "125", "131", "132", "134", "143", "145", "152", "155", "156", "162", "165", "172",
    "174", "205", "212", "223", "225", "226", "243", "244", "245", "246", "251", "252", "255", "261", "263", "265",
    "266", "271", "274", "306", "311", "315", "325", "331", "332", "343", "346", "351", "356", "364", "365", "371",
    "411", "412", "413", "423", "431", "432", "445", "446", "452", "454", "455", "462", "464", "465", "466", "503",
    "506", "516", "523", "526", "532", "546", "565", "606", "612", "624", "627", "631", "632", "654", "662", "664",
    "703", "712", "723", "731", "732", "734", "743", "754",
];

/// A record read back into the channel that would produce it. None for a
/// record using something the channel database cannot say (REV TONE, PR FREQ,
/// pager and the SQL-expansion modes, clock shift).
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
    let mode = match r[1] & 0x30 {
        0x00 => "FM",
        0x10 => "AM",
        0x20 => "NFM",
        _ => return None,
    };
    let tone_mode = match r[5] >> 4 {
        0 => "off",
        1 => "Tone",
        2 => "TSQL",
        4 => "DTCS",
        _ => return None,
    };
    if r[10] & 0x80 != 0 || r[1] & 0xC8 != 0 || r[14] != 0 || r[15] != 0 {
        return None;
    }
    let tone = TONES[(r[9] & 0x3F) as usize];
    Some(Channel {
        id: 1,
        rx_freq: rx as f64 / 1e6,
        tx_freq: Some(tx as f64 / 1e6),
        mode: Some(mode.into()),
        tone_mode: Some(tone_mode.into()),
        ctcss_uplink: Some(tone),
        ctcss_downlink: Some(tone),
        dcs_code: Some(DCS[(r[10] & 0x7F) as usize].into()),
        power: Some(["High", "Med", "Low", "?"][(r[9] >> 6) as usize].into()),
        dcs_polarity: "NN".into(),
        cross_mode: "Tone->Tone".into(),
        source: "gate".into(),
        ..Default::default()
    })
}

#[test]
#[ignore = "needs a real CLNFTM400D.dat (FTM400_DAT)"]
fn the_radios_own_memories_reencode_byte_for_byte() {
    let path = std::env::var("FTM400_DAT").expect("FTM400_DAT");
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let img = std::fs::read(root.join(&path)).expect("read FTM400_DAT");
    validate_backup(&img).expect("an FTM-400 ALL backup");
    let sum = img[..0x7F].iter().fold(0u8, |a, &b| a.wrapping_add(b));
    assert_eq!(sum, img[0x7F], "header checksum");

    let (mut compared, mut unsayable, mut skip_masked, mut pr_masked, mut step_masked) = (0, 0, 0, 0, 0);
    let mut mismatches = Vec::new();
    for (b, bank) in BANKS.iter().enumerate() {
        for slot in 0..MEM_SLOTS {
            let r = &img[bank.mem + slot * MEM_STRIDE..bank.mem + (slot + 1) * MEM_STRIDE];
            if r[0] & 0x80 == 0 {
                continue;
            }
            let field = &img[bank.name + slot * NAME_LEN..bank.name + (slot + 1) * NAME_LEN];
            let name = decode_name(field);
            assert!(!name.contains('?'), "{}{}: name bytes {field:02x?}", "AB".as_bytes()[b] as char, slot + 1);
            let Some(mut ch) = record_to_channel(r) else {
                unsayable += 1;
                continue;
            };
            ch.name_short = Some(name.clone());
            let ec = ExpandedChannel { channel: ch, tg_label: None, timeslot: None, tg_number: None, tg_call_type: None, tg_inline: false };
            let mut ours = encode_record(&ec, !tag(&name).is_empty());
            let mut theirs = [0u8; MEM_STRIDE];
            theirs.copy_from_slice(r);
            if theirs[0] & 0x60 != 0 {
                skip_masked += 1;
                theirs[0] &= !0x60;
            }
            if theirs[11] & 0x1F != ours[11] & 0x1F {
                pr_masked += 1;
                ours[11] = (ours[11] & !0x1F) | (theirs[11] & 0x1F);
            }
            if theirs[5] & 0x0F != ours[5] & 0x0F {
                step_masked += 1;
                ours[5] = (ours[5] & 0xF0) | (theirs[5] & 0x0F);
            }
            compared += 1;
            if ours != theirs {
                mismatches.push(format!("{}{:<3} {name:8} radio {theirs:02x?}\n            ours  {ours:02x?}", "AB".as_bytes()[b] as char, slot + 1));
            }
            if name_field(&tag(&name)) != field {
                mismatches.push(format!("{}{:<3} name {name:?}: radio {field:02x?} ours {:02x?}", "AB".as_bytes()[b] as char, slot + 1, name_field(&tag(&name))));
            }
        }
    }
    println!(
        "{compared} memories compared, {unsayable} unsayable; masked: skip {skip_masked}, \
         PR tone {pr_masked}, step {step_masked}; {} mismatches",
        mismatches.len()
    );
    for m in &mismatches {
        println!("  {m}");
    }
    assert!(compared > 0);
    assert!(mismatches.is_empty());
}
