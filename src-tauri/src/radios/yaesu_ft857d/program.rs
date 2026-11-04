//! Codeplug → image, and image → the download sanity sample.

use super::layout::CHANNEL_COUNT;
use super::memory::{
    decode_channel, encode_channel, existing_memory, hide_memory, is_visible, mode_text,
    read_memory, store_memory, tag_text,
};
use super::protocol::check_image;
use crate::commands::export::SlotChannel;
use crate::models::RadioModel;
use crate::radios::driver::DecodedChannelSample;

/// Patch `channels` into `base` (an image just read off the radio) and return
/// the image to write.
///
/// Full replace of the 200 regular memories: every slot the codeplug does not
/// fill is hidden, the way the radio erases one (record and filled bit kept).
/// PMS edges, VFOs, HOME, the 60 m channels and every setting go back exactly as
/// they came.
///
/// Every channel is encoded before a byte changes, so a codeplug with one
/// channel the radio cannot hold is refused whole rather than half applied.
pub(crate) fn build_image(
    model: &RadioModel,
    channels: &[SlotChannel],
    base: &[u8],
) -> Result<Vec<u8>, String> {
    check_image(base)?;
    let capacity = model
        .memory_channels
        .map(|n| (n as usize).min(CHANNEL_COUNT))
        .unwrap_or(CHANNEL_COUNT);
    if channels.len() > capacity {
        return Err(format!(
            "Codeplug has {} programmable channels, but the radio holds only {capacity}.",
            channels.len()
        ));
    }
    // CHIRP: "if you upload an empty image you can brick your radio".
    if !channels.iter().any(|sc| sc.slot == 0) {
        return Err("Memory 1 must hold a channel on this radio; the codeplug leaves it empty."
            .to_string());
    }

    let mut records = Vec::with_capacity(channels.len());
    for sc in channels {
        if sc.slot >= capacity {
            return Err(format!(
                "channel \"{}\" resolved to memory {} — beyond the radio's {capacity}.",
                sc.name,
                sc.slot + 1
            ));
        }
        let m = encode_channel(&sc.channel, &sc.name, existing_memory(base, sc.slot).as_ref())
            .map_err(|e| {
                format!(
                    "channel \"{}\" ({:.4} MHz) cannot be programmed: {e}",
                    sc.name, sc.channel.rx_freq
                )
            })?;
        records.push((sc.slot, m));
    }

    let mut image = base.to_vec();
    let mut taken = [false; CHANNEL_COUNT];
    for (slot, m) in &records {
        taken[*slot] = true;
        store_memory(&mut image, *slot, m);
    }
    for (slot, used) in taken.iter().enumerate() {
        if !used && is_visible(&image, slot) {
            hide_memory(&mut image, slot)?;
        }
    }
    Ok(image)
}

/// The memories the radio shows, for the "is this read real?" table.
pub(crate) fn decode_sample(image: &[u8]) -> Vec<DecodedChannelSample> {
    if check_image(image).is_err() {
        return Vec::new();
    }
    (0..CHANNEL_COUNT)
        .filter(|&s| is_visible(image, s))
        .map(|slot| {
            let m = read_memory(image, slot);
            let c = decode_channel(&m);
            let shift = match c.duplex.as_deref() {
                Some("+") => format!("+{:.3}", c.offset.unwrap_or(0.0)),
                Some("-") => format!("−{:.3}", c.offset.unwrap_or(0.0)),
                Some("split") => format!("TX {:.4}", c.tx_freq.unwrap_or(0.0)),
                _ => String::new(),
            };
            let tone = match c.tone_mode.as_deref() {
                Some("Tone") => format!("T {:.1}", c.ctcss_uplink.unwrap_or(0.0)),
                Some("TSQL") => format!("TSQL {:.1}", c.ctcss_downlink.unwrap_or(0.0)),
                Some("DTCS") => format!("DTCS {}", c.dcs_code.clone().unwrap_or_default()),
                Some("Cross") => format!("Cross {}", c.cross_mode),
                _ => "—".to_string(),
            };
            DecodedChannelSample {
                index: slot + 1,
                name: tag_text(&m),
                rx_mhz: c.rx_freq,
                shift: Some(shift),
                tone,
                // No per-memory power: RF POWER SET (menu 075) is per band.
                power: "—".to_string(),
                mode: Some(mode_text(&m).to_string()),
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::Channel;
    use crate::radios::yaesu_ft857d::layout::{memory_offset, IMAGE_LEN_US, VISIBLE_BASE};
    use crate::radios::yaesu_ft857d::memory::{decode_memory, encode_memory};

    /// A blank US image with memories 1-3 visible, filled from a real record.
    fn base() -> Vec<u8> {
        let mut img = vec![0xFFu8; IMAGE_LEN_US];
        img[VISIBLE_BASE..VISIBLE_BASE + 27].fill(0);
        img[super::super::layout::FILLED_BASE..super::super::layout::FILLED_BASE + 27].fill(0);
        let real: [u8; 28] = [
            0x82, 0x21, 0x00, 0x08, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x4C,
            0x4B, 0x40, 0x00, 0x01, 0x86, 0xA0, b'6', b'C', b'W', b' ', b'5', b'0', b'.', b'0',
        ];
        for slot in 0..3 {
            store_memory(&mut img, slot, &decode_memory(&real));
        }
        img
    }

    fn model() -> RadioModel {
        RadioModel { memory_channels: Some(200), ..Default::default() }
    }

    fn sc(slot: usize, name: &str, rx: f64, mode: &str) -> SlotChannel {
        SlotChannel {
            slot,
            name: name.into(),
            channel: Channel {
                rx_freq: rx,
                tx_freq: Some(rx),
                mode: Some(mode.into()),
                dcs_polarity: "NN".into(),
                cross_mode: "Tone->Tone".into(),
                ..Default::default()
            },
        }
    }

    #[test]
    fn a_codeplug_replaces_the_memories_and_touches_nothing_else() {
        let b = base();
        let out = build_image(
            &model(),
            &[sc(0, "20M FT8", 14.074, "USB"), sc(1, "2M CALL", 146.52, "FM")],
            &b,
        )
        .unwrap();
        let sample = decode_sample(&out);
        assert_eq!(sample.len(), 2, "memory 3 is hidden");
        assert_eq!((sample[0].name.as_str(), sample[0].mode.as_deref()), ("20M FT8", Some("USB")));
        assert_eq!((sample[1].rx_mhz, sample[1].mode.as_deref()), (146.52, Some("FM")));

        // Outside the memory table and its bitmaps, not one byte moved.
        let changed: Vec<usize> = (0..b.len()).filter(|&i| b[i] != out[i]).collect();
        for i in changed {
            let in_table = (memory_offset(0)..memory_offset(3)).contains(&i);
            let in_visible = i == VISIBLE_BASE;
            assert!(in_table || in_visible, "byte 0x{i:04X} changed");
        }
        // Memory 3 is hidden but its record stays, like the radio's own erase.
        let at = memory_offset(2);
        assert_eq!(&out[at..at + 28], &b[at..at + 28]);
    }

    #[test]
    fn writing_back_what_was_read_changes_nothing() {
        let b = base();
        let back: Vec<SlotChannel> = (0..3)
            .map(|s| {
                let m = read_memory(&b, s);
                SlotChannel { slot: s, name: tag_text(&m), channel: decode_channel(&m) }
            })
            .collect();
        assert_eq!(build_image(&model(), &back, &b).unwrap(), b);
        let _ = encode_memory(&read_memory(&b, 0));
    }

    #[test]
    fn refusals_come_before_any_byte_changes() {
        let b = base();
        let e = build_image(&model(), &[sc(1, "X", 146.52, "FM")], &b).unwrap_err();
        assert!(e.contains("Memory 1"), "{e}");
        let e = build_image(&model(), &[sc(0, "OK", 146.52, "FM"), sc(1, "GAP", 222.0, "FM")], &b)
            .unwrap_err();
        assert!(e.contains("\"GAP\""), "{e}");
        assert!(build_image(&model(), &[sc(0, "A", 146.52, "FM")], &[0u8; 10]).is_err());
    }
}
