//! Building an ID-5100 codeplug: the app's channels and banks patched into an
//! image the radio just handed us.
//!
//! Pure — no serial I/O. `mod.rs` calls [`build_codeplug`] between the download
//! and the upload.
//!
//! ## Full replace, of the memory area only
//!
//! All 1000 memories are rewritten: the codeplug's channels at the slots the
//! resolver chose, every other slot emptied, and all 26 bank names rewritten —
//! the codeplug's lists in order, the rest blank, which is what an unused bank
//! holds on the radio (16 spaces). Nothing outside the record array, the three
//! bitmaps, the bank table and the bank names is touched, so the call channels,
//! the repeater list, Your Call Sign memory, GPS memory and every menu setting
//! go back exactly as they were read.
//!
//! ## Banks
//!
//! Lists become banks A, B, C … in codeplug order. A bank indexes 100 memories
//! (`00`-`99`) and there are 26 of them; what does not fit stays programmed but
//! in no bank, and the operator is told which. A channel in two lists is in the
//! first list's bank — a memory carries one bank byte.

use crate::commands::export::{ExpandedChannel, SlotBank, SlotChannel};
use crate::models::RadioModel;
use crate::radios::driver::DecodedChannelSample;

use super::layout::{BANK_CAPACITY, BANK_COUNT, CHANNEL_COUNT, IMAGE_LEN, RX_COVERAGE};
use super::memory::{
    clear, decode_memories, encode_channel, is_used, set_bank_name, store, Record, ScanSkip,
};

pub(crate) struct Built {
    pub image: Vec<u8>,
    pub banks_written: usize,
    pub warnings: Vec<String>,
}

pub(crate) fn build_codeplug(
    model: &RadioModel,
    channels: &[SlotChannel],
    banks: &[SlotBank],
    base: &[u8],
) -> Result<Built, String> {
    if base.len() != IMAGE_LEN {
        return Err(format!(
            "expected a {IMAGE_LEN}-byte ID-5100 image, got {} bytes",
            base.len()
        ));
    }
    let capacity = model
        .memory_channels
        .map(|n| (n as usize).min(CHANNEL_COUNT))
        .unwrap_or(CHANNEL_COUNT);
    if channels.len() > capacity {
        return Err(format!(
            "Codeplug has {} programmable channels, but the ID-5100 holds only {capacity}.",
            channels.len()
        ));
    }

    // Encode every channel BEFORE touching the image, so one bad channel leaves
    // nothing half-patched.
    let mut records: Vec<(usize, Record)> = Vec::with_capacity(channels.len());
    for sc in channels {
        if sc.slot >= capacity {
            return Err(format!(
                "channel \"{}\" resolved to memory {} — beyond the ID-5100's {capacity}.",
                sc.name, sc.slot
            ));
        }
        // ★★ The radio refuses a whole clone over one memory it cannot hold,
        // and FACTORY RESETS (layout::RX_COVERAGE). `rx_bands` should have
        // excluded this channel upstream; refusing here as well is what makes
        // that a guarantee rather than an assumption.
        let f = sc.channel.rx_freq;
        if !RX_COVERAGE.iter().any(|&(lo, hi)| f >= lo && f <= hi) {
            return Err(format!(
                "channel \"{}\" ({f:.4} MHz) is outside the ID-5100's receive coverage \
                 (118–174 and 375–550 MHz). Nothing was written: the radio rejects an \
                 image carrying such a memory and resets itself to factory defaults.",
                sc.name
            ));
        }
        let ec = ExpandedChannel {
            channel: sc.channel.clone(),
            tg_label: None,
            timeslot: None,
            tg_number: None,
            tg_call_type: None,
            tg_inline: false,
        };
        let rec = encode_channel(&ec, &sc.name).map_err(|e| {
            format!("channel \"{}\" ({:.4} MHz) cannot be programmed: {e}", sc.name, sc.channel.rx_freq)
        })?;
        records.push((sc.slot, rec));
    }

    let mut warnings = Vec::new();
    let mut bank_of: Vec<Option<(u8, u8)>> = vec![None; CHANNEL_COUNT];
    let mut names: Vec<&str> = Vec::new();
    for bank in banks.iter().filter(|b| !b.slots.is_empty()) {
        if names.len() == BANK_COUNT {
            warnings.push(format!(
                "List \"{}\" is programmed but in no bank: the ID-5100 has {BANK_COUNT} banks \
                 and the lists before it used them all.",
                bank.name
            ));
            continue;
        }
        let letter = names.len() as u8;
        names.push(&bank.name);
        for (idx, &slot) in bank.slots.iter().enumerate() {
            if idx < BANK_CAPACITY && slot < CHANNEL_COUNT {
                bank_of[slot] = Some((letter, idx as u8));
            }
        }
        if bank.slots.len() > BANK_CAPACITY {
            warnings.push(format!(
                "List \"{}\" has {} channels; bank {} holds {BANK_CAPACITY}, so the last {} \
                 are programmed but in no bank.",
                bank.name,
                bank.slots.len(),
                (b'A' + letter) as char,
                bank.slots.len() - BANK_CAPACITY
            ));
        }
    }

    let mut image = base.to_vec();
    let mut occupied = vec![false; CHANNEL_COUNT];
    for (slot, rec) in &records {
        occupied[*slot] = true;
        store(&mut image, *slot, rec, bank_of[*slot]);
    }
    for slot in (0..CHANNEL_COUNT).filter(|&s| !occupied[s] && is_used(base, s)) {
        clear(&mut image, slot);
    }
    for bank in 0..BANK_COUNT {
        set_bank_name(&mut image, bank, names.get(bank).copied().unwrap_or(""));
    }

    Ok(Built {
        image,
        banks_written: names.len(),
        warnings,
    })
}

/// Decode an image's memories for the download sanity sample. `power` is `"—"`:
/// the ID-5100 keeps power per band, not per memory.
pub(crate) fn decode_sample(image: &[u8]) -> Vec<DecodedChannelSample> {
    decode_memories(image)
        .into_iter()
        .map(|m| DecodedChannelSample {
            index: m.slot,
            // Bank and skip ride in the name column: the sample has no field
            // for either, and both are what the operator checks the radio by.
            name: format!(
                "{}{}{}",
                m.name,
                m.bank.map(|(b, i)| format!(" [{b}{i:02}]")).unwrap_or_default(),
                match m.skip {
                    ScanSkip::None => "",
                    ScanSkip::Memory => " (skip)",
                    ScanSkip::Program => " (P-skip)",
                }
            ),
            rx_mhz: m.rx_mhz,
            shift: Some(m.shift),
            tone: m.tone,
            power: "—".into(),
            mode: Some(m.mode),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::Channel;
    use crate::radios::icom_id5100::layout::{BANK_NAMES, BANK_TABLE, EMPTY_BITMAP};
    use crate::radios::icom_id5100::memory::{bank_name, read_bank, read_record};

    fn model() -> RadioModel {
        RadioModel {
            display_name: "Icom ID-5100".into(),
            memory_channels: Some(1000),
            ..Default::default()
        }
    }

    /// Shaped like a read off the radio: every slot empty with the radio's own
    /// default record and `1F 00` bank entry, all bank names blank — then two
    /// memories and a named bank the operator made, which the build must clear.
    fn base() -> Vec<u8> {
        let mut image = vec![0u8; IMAGE_LEN];
        image[EMPTY_BITMAP..EMPTY_BITMAP + 125].fill(0xFF);
        for s in 0..CHANNEL_COUNT {
            image[BANK_TABLE + 2 * s] = 0x1F;
        }
        image[BANK_NAMES..BANK_NAMES + 26 * 16].fill(b' ');
        let mut c = Channel { rx_freq: 146.52, dcs_polarity: "NN".into(), ..Default::default() };
        c.id = 900;
        let ec = ExpandedChannel {
            channel: c,
            tg_label: None,
            timeslot: None,
            tg_number: None,
            tg_call_type: None,
            tg_inline: false,
        };
        let rec = encode_channel(&ec, "OLD").unwrap();
        store(&mut image, 500, &rec, Some((3, 0)));
        set_bank_name(&mut image, 3, "OLD BANK");
        // Settings live above the memory area; they must come through as read.
        image[0x29AFC] = 5;
        image
    }

    fn slot(n: usize, id: i64, name: &str, rx: f64) -> SlotChannel {
        SlotChannel {
            slot: n,
            name: name.into(),
            channel: Channel {
                id,
                rx_freq: rx,
                dcs_polarity: "NN".into(),
                ..Default::default()
            },
        }
    }

    #[test]
    fn lists_become_named_banks_and_the_rest_is_cleared() {
        let base = base();
        let slots = vec![
            slot(0, 1, "ONE", 146.94),
            slot(1, 2, "TWO", 447.275),
            slot(2, 3, "THREE", 145.3875),
        ];
        let banks = vec![
            SlotBank { name: "Local".into(), slots: vec![0, 2] },
            SlotBank { name: "Empty".into(), slots: vec![] },
            SlotBank { name: "UHF".into(), slots: vec![1] },
        ];
        let built = build_codeplug(&model(), &slots, &banks, &base).unwrap();
        let image = &built.image;
        assert_eq!(built.banks_written, 2);
        assert!(built.warnings.is_empty());
        // The empty list takes no letter; the next list is B.
        assert_eq!(read_bank(image, 0), Some((0, 0)));
        assert_eq!(read_bank(image, 2), Some((0, 1)));
        assert_eq!(read_bank(image, 1), Some((1, 0)));
        assert_eq!(bank_name(image, 0), "Local");
        assert_eq!(bank_name(image, 1), "UHF");
        // The operator's old memory and bank name are gone.
        assert!(!is_used(image, 500));
        assert_eq!(bank_name(image, 3), "");
        assert_eq!(read_record(image, 1).name(), "TWO");
        // A 6.25 kHz memory's bank byte carries its multiplier.
        assert_eq!(image[BANK_TABLE + 4], 0x20);
        // Outside the memory area, nothing moved.
        assert_eq!(image[0x29AFC], 5);
        let changed_high = (super::super::layout::VOLATILE_BASE..IMAGE_LEN)
            .filter(|&i| image[i] != base[i])
            .count();
        assert_eq!(changed_high, 0);
    }

    #[test]
    fn overflowing_banks_are_programmed_unbanked_and_reported() {
        let slots: Vec<SlotChannel> =
            (0..130).map(|i| slot(i, i as i64, "X", 146.0 + i as f64 * 0.01)).collect();
        let mut banks: Vec<SlotBank> = vec![SlotBank {
            name: "Big".into(),
            slots: (0..102).collect(),
        }];
        for i in 102..130 {
            banks.push(SlotBank { name: format!("L{i}"), slots: vec![i] });
        }
        let built = build_codeplug(&model(), &slots, &banks, &base()).unwrap();
        assert_eq!(built.banks_written, 26);
        assert_eq!(read_bank(&built.image, 99), Some((0, 99)));
        assert_eq!(read_bank(&built.image, 100), None);
        assert!(is_used(&built.image, 100));
        // Lists 27+ (L127, L128, L129) get no bank.
        assert_eq!(read_bank(&built.image, 127), None);
        assert!(is_used(&built.image, 129));
        assert_eq!(built.warnings.len(), 1 + 3, "{:#?}", built.warnings);
    }

    #[test]
    fn a_channel_outside_coverage_is_refused_before_anything_is_built() {
        for f in [117.975, 174.005, 300.0, 374.995, 550.005] {
            let Err(e) = build_codeplug(&model(), &[slot(0, 1, "OUT", f)], &[], &base()) else {
                panic!("{f} MHz was built");
            };
            assert!(e.contains("factory defaults"), "{f}: {e}");
        }
        for f in [118.0, 174.0, 375.0, 550.0] {
            assert!(build_codeplug(&model(), &[slot(0, 1, "IN", f)], &[], &base()).is_ok(), "{f}");
        }
    }

    /// The seed's `rx_bands` is what keeps out-of-coverage channels away from
    /// this driver in the first place; the driver's guard is the backstop. If
    /// they ever disagree, one of them is wrong about a limit that resets the
    /// radio.
    #[test]
    fn the_guard_and_the_seed_agree_on_coverage() {
        let seeded = crate::seed::model_rx_bands("ID-5100").expect("seeded");
        let seeded: Vec<(f64, f64)> = serde_json::from_str::<Vec<Vec<f64>>>(seeded)
            .unwrap()
            .into_iter()
            .map(|b| (b[0], b[1]))
            .collect();
        assert_eq!(seeded, RX_COVERAGE.to_vec());
    }

    #[test]
    fn one_bad_channel_leaves_the_image_untouched() {
        let slots = vec![slot(0, 1, "OK", 146.94), slot(1, 2, "ODD", 146.521)];
        assert!(build_codeplug(&model(), &slots, &[], &base()).is_err());
    }
}
