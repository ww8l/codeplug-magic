//! FTM-510DR microSD codeplug writer: patch memories into the radio's own
//! `FTM510D/BACKUP/CLNFTM510D.dat`.
//!
//! Same layout as the FTM-300's (`yaesu_ftm300::sd_image`), whose record and
//! name encoders this reuses; see the module doc in `mod.rs` for what was checked
//! on the 510 itself. Only the 999 regular memories are rewritten:
//!
//! | region | address | what |
//! |---|---|---|
//! | `memory[999]` | `0x0800` | 16-byte records |
//! | `name_len[999]` | `0x4CB0` | u16 BE `(1 << (16 - len)) - 1`, `0xFFFF` = no name |
//! | `name[999]` | `0x10000` | 16 ASCII bytes, `0xFF`-padded |
//!
//! Each table runs on for the 100 PMS entries, and the VFO / Home records sit at
//! `0x0080`-`0x023F` in the same record format; all of those are left exactly as
//! the operator had them, as is every setting.

use crate::commands::export::{expanded_names, tx_frequency, ExpandedChannel};
use crate::models::RadioModel;
use crate::radios::yaesu_ftm300::sd_image::{encode_record, name_fields, tag, NAME_LEN};

pub(crate) const IMAGE_LEN: usize = 0x18000;
const MAGIC: &[u8] = b"AH089$";

const MEM_BASE: usize = 0x0800;
const MEM_STRIDE: usize = 16;
pub(crate) const MEM_SLOTS: usize = 999;
const NAME_LEN_BASE: usize = 0x4CB0;
const NAME_BASE: usize = 0x10000;

/// What the radio holds in an empty slot: 1,083 of the 1,097 empty slots in
/// Tim's new radio are exactly this (144.000 MHz, 12.5 kHz step, 100.0 Hz,
/// 600 kHz) — the FTM-300's empty record, byte for byte. The other 14 carry
/// step 0 instead; factory leftovers, not written back.
const EMPTY_RECORD: [u8; MEM_STRIDE] = [
    0x01, 0x00, 0x01, 0x44, 0x00, 0x04, 0x00, 0x00, 0x00, 0x0C, 0x00, 0x0F, 0x00, 0x0C, 0x00, 0x00,
];

/// The radio's receive range (Operating Manual specifications: 108–550 MHz,
/// continuous in five ranges). A frequency outside it would become a slot the
/// radio shows empty while the export reported success.
const RX_MIN_HZ: i64 = 108_000_000;
const RX_MAX_HZ: i64 = 550_000_000;

/// Refuse a file that is not an FTM-510 ALL backup, naming what it is.
pub(crate) fn validate_backup(image: &[u8]) -> Result<(), String> {
    if image.len() != IMAGE_LEN {
        return Err(format!(
            "This is not an FTM-510 backup: it is {} bytes, and the radio's \
             FTM510D/BACKUP/CLNFTM510D.dat is always {IMAGE_LEN}. On the radio, \
             Setup Menu 107 BACKUP → WRITE TO SD → ALL, then pick that file.",
            image.len()
        ));
    }
    if &image[..MAGIC.len()] != MAGIC {
        let other = match &image[..MAGIC.len()] {
            b"AH071$" => " (that is an FTM-300 backup)",
            b"AH081$" => " (that is an FTM-500 backup)",
            _ => "",
        };
        return Err(format!(
            "This backup is from a different radio: it starts {:?}{other}, and an FTM-510 \
             backup starts \"AH089$\". The FTM-510 writer only patches FTM-510 backups.",
            String::from_utf8_lossy(&image[..MAGIC.len()])
        ));
    }
    Ok(())
}

/// Whether the 510 is known to display `hz` as written: inside its receive
/// range and on the 625 Hz grid every 5 / 6.25 / 12.5 kHz channel sits on.
/// The FTM-300's measured +8.33 kHz airband code is NOT accepted here — it has
/// never been seen on a 510's screen.
fn writable(hz: i64) -> bool {
    (RX_MIN_HZ..=RX_MAX_HZ).contains(&hz) && hz.rem_euclid(10_000) % 625 == 0
}

fn write_slot(img: &mut [u8], slot: usize, record: &[u8; MEM_STRIDE], name: &str) {
    let at = MEM_BASE + slot * MEM_STRIDE;
    img[at..at + MEM_STRIDE].copy_from_slice(record);
    let (field, code) = name_fields(name);
    img[NAME_BASE + slot * NAME_LEN..NAME_BASE + (slot + 1) * NAME_LEN].copy_from_slice(&field);
    img[NAME_LEN_BASE + slot * 2..NAME_LEN_BASE + slot * 2 + 2].copy_from_slice(&code.to_be_bytes());
}

/// Patch `template` with `channels` in memory-slot order (slot 1 first).
///
/// Every one of the 999 regular slots is rewritten — a filled one with its
/// channel, the rest with the radio's own empty record — so nothing from the
/// operator's old memory list survives into the new one. PMS, VFO, Home and
/// every setting are the template's. No banks, so channel lists flatten.
///
/// Overflow and frequencies the radio would not show as written are errors,
/// never a silent truncation or rounding.
pub(crate) fn patch_backup(
    template: &[u8],
    channels: &[&ExpandedChannel],
    model: &RadioModel,
) -> Result<Vec<u8>, String> {
    validate_backup(template)?;
    if channels.len() > MEM_SLOTS {
        return Err(format!(
            "This codeplug has {} channels; the FTM-510 holds {MEM_SLOTS}. \
             Remove {} before exporting.",
            channels.len(),
            channels.len() - MEM_SLOTS
        ));
    }
    let bad: Vec<String> = channels
        .iter()
        .filter(|ec| !writable(hz(ec.channel.rx_freq)) || !writable(hz(tx_frequency(&ec.channel))))
        .map(|ec| format!("{:.5} MHz", ec.channel.rx_freq))
        .collect();
    if !bad.is_empty() {
        return Err(format!(
            "The FTM-510 cannot hold these as written — outside its 108–550 MHz range, or an \
             8.33 kHz airband point whose code has not been measured on this radio: {}. \
             Remove them from this codeplug.",
            bad.join(", ")
        ));
    }
    let mut img = template.to_vec();
    let names = expanded_names(channels.iter().copied(), model);
    for (slot, (ec, name)) in channels.iter().zip(&names).enumerate() {
        write_slot(&mut img, slot, &encode_record(ec), &tag(name));
    }
    for slot in channels.len()..MEM_SLOTS {
        write_slot(&mut img, slot, &EMPTY_RECORD, "");
    }
    debug_assert_eq!(&img[..MEM_BASE], &template[..MEM_BASE]);
    Ok(img)
}

fn hz(mhz: f64) -> i64 {
    (mhz * 1_000_000.0).round() as i64
}

#[cfg(test)]
pub(super) fn decode_freq(b: &[u8]) -> i64 {
    let bcd = |x: u8| ((x >> 4) * 10 + (x & 0x0F)) as i64;
    let tens = (b[0] & 0x0F) as i64 * 10_000 + bcd(b[1]) * 100 + bcd(b[2]);
    let mut hz = tens * 10_000;
    for (bit, add) in [(0x80u8, 5000i64), (0x40, 2500), (0x20, 1250), (0x10, 625)] {
        if b[0] & bit != 0 {
            hz += add;
        }
    }
    hz
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::Channel;

    fn chan(rx: f64, tx: Option<f64>) -> Channel {
        Channel {
            id: 1,
            name_short: Some("TEST".into()),
            rx_freq: rx,
            tx_freq: tx,
            dcs_polarity: "NN".into(),
            cross_mode: "Tone->Tone".into(),
            source: "test".into(),
            ..Default::default()
        }
    }

    fn ec(c: Channel) -> ExpandedChannel {
        ExpandedChannel {
            channel: c,
            tg_label: None,
            timeslot: None,
            tg_number: None,
            tg_call_type: None,
            tg_inline: false,
        }
    }

    fn template() -> Vec<u8> {
        let mut t = vec![0x5Au8; IMAGE_LEN];
        t[..6].copy_from_slice(MAGIC);
        t
    }

    fn model() -> RadioModel {
        crate::seed::test_model("FTM-510DR")
    }

    /// Records copied out of Tim's factory-fresh FTM-510 (`captures/ftm510_01`):
    /// memories 1 and 2 as the factory left them (144.000, no name). Both must
    /// come out of the encoder byte for byte.
    #[test]
    fn the_factory_memories_reencode_byte_for_byte() {
        let radio = [0x81, 0x00, 0x01, 0x44, 0x00, 0x00, 0x00, 0x00, 0x00, 0x0C, 0x00, 0x0F, 0x00, 0x0C, 0x00, 0x00];
        let mut c = chan(144.0, None);
        c.name_short = None;
        assert_eq!(encode_record(&ec(c)), radio);
    }

    /// ADMS-18 (Yaesu's own programmer) writing memory 3 = 146.520 named `TEST`:
    /// the record it wrote, with the AMS flag it sets on every new memory
    /// (byte 1 `0x40`) masked, because this encoder programs FM, not AMS.
    #[test]
    fn matches_what_adms18_wrote_for_a_new_memory() {
        let adms = [0x81, 0x40, 0x01, 0x46, 0x52, 0x00, 0x00, 0x00, 0x00, 0x0C, 0x00, 0x0F, 0x00, 0x0C, 0x00, 0x00];
        let mut ours = encode_record(&ec(chan(146.52, None)));
        ours[1] |= 0x40;
        assert_eq!(ours, adms);
        let (field, _) = name_fields(&tag("TEST"));
        assert_eq!(&field[..5], b"TEST\xFF");
    }

    /// The VFO / Home records the radio wrote, in the memory format: one per
    /// band, each decoding to the defaults ADMS-18 displays for them.
    #[test]
    fn the_radios_band_records_decode() {
        let rows: &[(&[u8; 16], i64, u16)] = &[
            (&[0x00, 0x10, 0x01, 0x08, 0x00, 0x07, 0, 0, 0, 0x0C, 0, 0x0F, 0x00, 0x00, 0, 0], 108_000_000, 0),
            (&[0x01, 0x00, 0x01, 0x44, 0x00, 0x00, 0, 0, 0, 0x0C, 0, 0x0F, 0x00, 0x0C, 0, 0], 144_000_000, 12),
            (&[0x02, 0x00, 0x02, 0x22, 0x00, 0x04, 0, 0, 0, 0x0C, 0, 0x0F, 0x00, 0x20, 0, 0], 222_000_000, 32),
            (&[0x03, 0x00, 0x04, 0x30, 0x00, 0x07, 0, 0, 0, 0x0C, 0, 0x0F, 0x00, 0x64, 0, 0], 430_000_000, 100),
        ];
        for (r, want_hz, want_shift) in rows {
            assert_eq!(decode_freq(&r[2..5]), *want_hz);
            assert_eq!(u16::from_be_bytes([r[12], r[13]]), *want_shift, "{want_hz}");
        }
        // AIR is AM: the same byte-1 flag the encoder writes for an AM channel.
        let mut c = chan(108.0, None);
        c.mode = Some("AM".into());
        assert_eq!(encode_record(&ec(c))[1], rows[0].0[1]);
    }

    #[test]
    fn rejects_other_radios_files() {
        assert!(validate_backup(&template()).is_ok());
        assert!(validate_backup(&[0u8; 100]).unwrap_err().contains("98304"));
        let mut t = template();
        t[..6].copy_from_slice(b"AH071$");
        assert!(validate_backup(&t).unwrap_err().contains("FTM-300"));
        t[..6].copy_from_slice(b"AH081$");
        assert!(validate_backup(&t).unwrap_err().contains("FTM-500"));
    }

    /// Above 550 MHz is outside this radio (the FTM-300 went to 999), and the
    /// 8.33 kHz airband code is unmeasured here — both refused by name.
    #[test]
    fn refuses_what_the_radio_would_not_show() {
        for (rx, tx) in [(851.0125, None), (107.9, None), (127.80833, None), (146.52, Some(600.0))] {
            let mut c = chan(rx, tx);
            if rx < 137.0 {
                c.mode = Some("AM".into());
            }
            let e = ec(c);
            let err = patch_backup(&template(), &[&e], &model()).unwrap_err();
            assert!(err.contains(&format!("{rx:.5}")), "{rx}: {err}");
        }
        let e = ec(chan(549.9875, None));
        assert!(patch_backup(&template(), &[&e], &model()).is_ok());
    }

    #[test]
    fn overflow_is_an_error() {
        let e = ec(chan(146.52, None));
        let many: Vec<&ExpandedChannel> = std::iter::repeat_n(&e, MEM_SLOTS + 1).collect();
        assert!(patch_backup(&template(), &many, &model()).unwrap_err().contains("999"));
    }

    /// Only the three memory tables change: every byte outside them, the
    /// settings, VFO / Home and PMS entries included, is the template's.
    #[test]
    fn patches_only_the_regular_memories() {
        let t = template();
        let mut c = chan(447.275, Some(442.275));
        c.tone_mode = Some("TSQL".into());
        c.ctcss_uplink = Some(100.0);
        c.name_short = Some("NCARC".into());
        let e = ec(c);
        let out = patch_backup(&t, &[&e], &model()).unwrap();
        assert_eq!(&out[0x800..0x810], &[0x83, 0x02, 0x84, 0x47, 0x27, 0x20, 0, 0, 0, 0x0C, 0, 0x0F, 0, 0x64, 0, 0]);
        assert_eq!(&out[NAME_BASE..NAME_BASE + 6], b"NCARC\xFF");
        assert_eq!(&out[NAME_LEN_BASE..NAME_LEN_BASE + 2], &[0x07, 0xFF]);
        assert_eq!(&out[0x810..0x820], &EMPTY_RECORD);
        let in_tables = |i: usize| {
            (MEM_BASE..MEM_BASE + MEM_SLOTS * MEM_STRIDE).contains(&i)
                || (NAME_LEN_BASE..NAME_LEN_BASE + MEM_SLOTS * 2).contains(&i)
                || (NAME_BASE..NAME_BASE + MEM_SLOTS * NAME_LEN).contains(&i)
        };
        assert!((0..IMAGE_LEN).filter(|&i| !in_tables(i)).all(|i| out[i] == t[i]));
        // PMS entries (slots 1000-1099) are past the 999 and untouched.
        assert_eq!(out[MEM_BASE + MEM_SLOTS * MEM_STRIDE], 0x5A);
    }

    /// Encode → decode over the fields this encoder writes, at every band
    /// edge the radio claims and both sides of each internal boundary.
    #[test]
    fn frequencies_round_trip_across_the_coverage() {
        for mhz in [108.0, 136.99375, 137.0, 146.52, 173.99375, 174.0, 222.1, 399.99375, 400.0, 446.10625, 479.99375, 480.0, 549.9875] {
            let r = encode_record(&ec(chan(mhz, None)));
            assert_eq!(decode_freq(&r[2..5]), hz(mhz), "{mhz}");
            let band = r[0] & 0x0F;
            let want = [137.0, 174.0, 400.0, 480.0].iter().filter(|&&b| mhz >= b).count() as u8;
            assert_eq!(band, want, "{mhz}");
        }
    }
}
