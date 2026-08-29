//! Building a TH-D72 codeplug: an app codeplug patched into an image the radio
//! just handed us.
//!
//! Pure — no serial I/O, no radio. `mod.rs` calls [`build_image`] from
//! `ImageProgrammer::build_image` and again inside `program_codeplug`, between
//! the download and the upload, so everything here is unit-testable without
//! hardware.
//!
//! ## Full replace, through the container
//!
//! [`build_image`] rewrites all 1000 programmable memories: the codeplug's
//! channels from slot 0 up, and every previously-programmed slot above them
//! emptied. It never touches the image outside those three cell arrays, so the
//! radio's APRS, GPS, TNC, settings and calibration regions come back exactly as
//! they were read.
//!
//! Every write goes through [`Thd72Image::patch`], which is the container's only
//! mutator. That is not ceremony: `patch` marks a 256-byte block dirty only when
//! a byte actually changes, and the upload sends only dirty blocks. Building a
//! `Vec<u8>` and indexing into it here would produce the right bytes and lose
//! the bookkeeping, so a re-program that changed one channel would push the
//! whole 64 KiB at the radio.
//!
//! ## What this phase does not do
//!
//! **Group names are not written.** The D72 has ten memory groups, but they are
//! *positional* — group `n` is memories `n*100 ..= n*100+99`, and only the ten
//! names are stored anywhere. `ImageProgramRequest` carries no groups (clone
//! radios program channels and names only), so channels land in a group purely
//! by slot number and the operator's own group names survive untouched. That is
//! a real limitation of Phase 3 rather than an oversight: naming them needs the
//! codeplug's channel lists plumbed through the image-program path, which is a
//! change to the trait, not to this file.

use super::container::Thd72Image;
use super::layout::CHANNEL_COUNT;
use super::memory::{
    apply_record, clear_record, decode_channels, encode_channel, read_record, Thd72Record,
};
use crate::commands::export::SlotChannel;
use crate::models::RadioModel;
use crate::radios::driver::DecodedChannelSample;

/// Patch `channels` into `base`, returning the image to upload.
///
/// `base` must be an image just read from the radio: the programmable-VFO table
/// is taken out of it, never from a constant, because Menu 130 lets the operator
/// move those band edges and every memory has to carry an index that agrees with
/// its own frequency.
pub(crate) fn build_image(
    model: &RadioModel,
    channels: &[SlotChannel],
    base: &[u8],
) -> Result<Vec<u8>, String> {
    let capacity = model
        .memory_channels
        .map(|n| (n as usize).min(CHANNEL_COUNT))
        .unwrap_or(CHANNEL_COUNT);
    if channels.len() > capacity {
        return Err(format!(
            "Codeplug has {} programmable channels, but the TH-D72 holds only {capacity}.",
            channels.len()
        ));
    }

    let mut image = Thd72Image::parse(base)?;
    let table = image.prog_vfo_table()?;

    // Resolve every channel BEFORE writing a byte. A codeplug with one
    // unencodable channel must leave the image untouched rather than
    // half-patched — the caller may still upload what it is handed.
    let mut records: Vec<(usize, Thd72Record)> = Vec::with_capacity(channels.len());
    for sc in channels {
        if sc.slot >= capacity {
            return Err(format!(
                "channel \"{}\" resolved to memory {} — beyond the TH-D72's {capacity}.",
                sc.name, sc.slot
            ));
        }
        // Not skipped, refused. `rx_bands` (118-174 and 320-524) should have
        // filtered an out-of-coverage channel upstream, so reaching here means
        // the two disagree — and the ID-52 shipped three 220 MHz repeaters as
        // silently EMPTY memories while reporting 31 channels written. Failing
        // the whole build is the direction that cannot lie to the operator.
        let mut rec = encode_channel(&sc.channel, &table).map_err(|e| {
            format!("channel \"{}\" ({:.4} MHz) cannot be programmed: {e}", sc.name, sc.channel.rx_freq)
        })?;
        rec.set_name(&sc.name);
        records.push((sc.slot, rec));
    }

    let mut occupied = vec![false; CHANNEL_COUNT];
    for (slot, rec) in &records {
        occupied[*slot] = true;
        for (off, cell) in apply_record(*slot, rec) {
            image.patch(off, &cell)?;
        }
    }

    // Full replace: anything the radio still holds above the codeplug goes.
    // `read_record` returning None means the slot is already empty, so this
    // writes nothing and dirties nothing for a radio that was already blank.
    for (slot, taken) in occupied.iter().enumerate() {
        if *taken || read_record(image.body(), slot).is_none() {
            continue;
        }
        for (off, cell) in clear_record(slot) {
            image.patch(off, &cell)?;
        }
    }

    Ok(image.into_raw())
}

/// Decode an image's memories for the download sanity sample.
///
/// `power` is `"—"` for every row: the D72's record has no per-memory power
/// field. See `memory::Thd72DecodedChannel`.
pub(crate) fn decode_sample(image: &[u8]) -> Vec<DecodedChannelSample> {
    decode_channels(image)
        .into_iter()
        .map(|c| DecodedChannelSample {
            index: c.index,
            name: c.name,
            rx_mhz: c.rx_mhz,
            shift: Some(c.shift),
            tone: c.tone,
            power: c.power,
            mode: Some(c.mode),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::Channel;
    use crate::radios::kenwood_thd72::layout::{
        group_of, prog_vfo_index, CALIBRATION_BASE, IMAGE_LEN, PROG_VFO_BASE,
    };
    use crate::radios::kenwood_thd72::memory::decode_memory;

    /// The 48 bytes at 0x02C0 of `000-factory-reset.img`, verbatim — the same
    /// constant `layout.rs` tests against, repeated rather than shared so this
    /// module's fixtures do not silently follow a change made over there.
    const REAL_PROG_VFO_BYTES: [u8; 48] = [
        0x00, 0x32, 0x1b, 0x08, 0x80, 0x07, 0x5f, 0x0a, 0x80, 0x1a, 0x70, 0x18, 0x80, 0xa1, 0x03,
        0x1c, 0x80, 0x89, 0x08, 0x07, 0x00, 0x32, 0x1b, 0x08, 0x00, 0x32, 0x1b, 0x08, 0x80, 0x07,
        0x5f, 0x0a, 0x00, 0xd0, 0x12, 0x13, 0x00, 0x84, 0xd7, 0x17, 0x00, 0x84, 0xd7, 0x17, 0x00,
        0x9b, 0x3b, 0x1f,
    ];

    /// The first 0x20 bytes of `002-set-channel-1-from-radio.img`.
    const FRONTMATTER: [u8; 0x20] = [
        0x1b, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff,
        0xff, 0x00, 0x00, 0x00, 0x00, 0x02, 0x00, 0x30, 0x30, 0x30, 0x00, 0xff, 0xff, 0xff, 0xff,
        0xff, 0xff,
    ];

    /// A blank image shaped like one off the radio: 0xFF fill, real front
    /// matter, the radio's own prog-VFO table, and per-radio bytes in the
    /// calibration region so a bug that overwrote it would show.
    fn base() -> Vec<u8> {
        let mut image = vec![0xFFu8; IMAGE_LEN];
        image[..0x20].copy_from_slice(&FRONTMATTER);
        image[PROG_VFO_BASE..PROG_VFO_BASE + REAL_PROG_VFO_BYTES.len()]
            .copy_from_slice(&REAL_PROG_VFO_BYTES);
        image[CALIBRATION_BASE] = 0x17;
        image[CALIBRATION_BASE + 1] = 0x03;
        image
    }

    fn model() -> RadioModel {
        RadioModel {
            display_name: "Kenwood TH-D72".into(),
            memory_channels: Some(1000),
            ..Default::default()
        }
    }

    fn slot(n: usize, name: &str, rx: f64) -> SlotChannel {
        SlotChannel {
            slot: n,
            name: name.to_string(),
            channel: Channel {
                rx_freq: rx,
                dcs_polarity: "NN".to_string(),
                ..Default::default()
            },
        }
    }

    fn repeater(n: usize, name: &str, rx: f64, duplex: &str, offset: f64, tone: f64) -> SlotChannel {
        let mut s = slot(n, name, rx);
        s.channel.duplex = Some(duplex.to_string());
        s.channel.offset = Some(offset);
        s.channel.tone_mode = Some("Tone".to_string());
        s.channel.ctcss_uplink = Some(tone);
        s
    }

    fn codeplug() -> Vec<SlotChannel> {
        vec![
            slot(0, "SIMPLEX", 146.520),
            repeater(1, "W8UM", 145.230, "-", 0.600, 100.0),
            repeater(2, "K8UM70CM", 442.575, "+", 5.000, 100.0),
            slot(3, "AIR", 121.500),
        ]
    }

    #[test]
    fn a_codeplug_reads_back_as_the_channels_that_went_in() {
        let out = build_image(&model(), &codeplug(), &base()).expect("build");
        let decoded = decode_channels(&out);
        assert_eq!(decoded.len(), 4);

        assert_eq!(decoded[0].name, "SIMPLEX");
        assert!((decoded[0].rx_mhz - 146.520).abs() < 1e-6);
        assert_eq!(decoded[0].shift, "", "simplex has no shift");

        assert_eq!(decoded[1].name, "W8UM");
        assert!((decoded[1].rx_mhz - 145.230).abs() < 1e-6);
        assert_eq!(decoded[1].shift, "-0.600");
        assert_eq!(decoded[1].tone, "Tone 100.0");

        assert_eq!(decoded[2].name, "K8UM70CM");
        assert_eq!(decoded[2].shift, "+5.000");

        assert_eq!(decoded[3].name, "AIR");
        assert!((decoded[3].rx_mhz - 121.500).abs() < 1e-6);
    }

    /// The invariant the whole driver is built around, asserted directly rather
    /// than trusted: every memory's band index must contain that memory's own
    /// frequency. A channel that fails this is stored, displayed and received —
    /// and cannot transmit.
    #[test]
    fn every_built_channel_lands_in_a_band_that_contains_it() {
        let out = build_image(&model(), &codeplug(), &base()).expect("build");
        let table = Thd72Image::parse(&out).unwrap().prog_vfo_table().unwrap();
        for slot in 0..CHANNEL_COUNT {
            let Some(rec) = read_record(&out, slot) else {
                continue;
            };
            let freq = decode_memory(&rec.memory).freq_hz;
            assert_eq!(
                prog_vfo_index(&table, freq),
                Some(rec.prog_vfo()),
                "memory {slot} at {freq} Hz claims band {} — it could not transmit",
                rec.prog_vfo()
            );
        }
    }

    /// 220 MHz is the band this radio cannot hear at all, and the ID-52's
    /// silent-empty-memory failure is what this refusal exists to prevent.
    #[test]
    fn a_channel_outside_every_band_fails_the_build_by_name() {
        let mut cp = codeplug();
        cp.push(slot(4, "W8UM220", 224.500));
        let err = build_image(&model(), &cp, &base()).expect_err("224 MHz must be refused");
        assert!(err.contains("W8UM220"), "the error must name the channel: {err}");
        assert!(err.contains("224.5"), "the error must name the frequency: {err}");
    }

    /// One bad channel must leave the image alone, not half-patched.
    #[test]
    fn a_refused_channel_leaves_the_image_untouched() {
        let mut cp = codeplug();
        cp.push(slot(4, "W8UM220", 224.500));
        let before = base();
        assert!(build_image(&model(), &cp, &before).is_err());
        // build_image takes `base` by reference and owns its copy; the proof
        // that nothing partial escapes is that the error path returns no image
        // at all, so a caller cannot upload one.
        assert_eq!(before, base(), "the caller's image is never mutated");
    }

    #[test]
    fn over_capacity_is_refused_naming_both_numbers() {
        let many: Vec<SlotChannel> = (0..CHANNEL_COUNT + 1)
            .map(|n| slot(n, "CH", 146.520))
            .collect();
        let err = build_image(&model(), &many, &base()).expect_err("over capacity");
        assert!(err.contains("1001"), "{err}");
        assert!(err.contains("1000"), "{err}");
    }

    /// Full replace: a memory the radio still holds above the new codeplug is
    /// emptied, not left behind for the operator to find.
    #[test]
    fn a_slot_the_new_codeplug_does_not_use_is_cleared() {
        let first = build_image(&model(), &codeplug(), &base()).expect("first build");
        assert!(read_record(&first, 3).is_some(), "slot 3 was programmed");

        let shorter = vec![slot(0, "ONLYONE", 146.520)];
        let second = build_image(&model(), &shorter, &first).expect("second build");
        assert!(read_record(&second, 0).is_some());
        for slot in 1..4 {
            assert!(
                read_record(&second, slot).is_none(),
                "memory {slot} should have been cleared"
            );
        }
    }

    /// Rebuilding the same codeplug onto its own output must dirty nothing —
    /// that is what keeps an upload as small as the edit, and it is the entire
    /// safety argument for writing a subset of blocks.
    #[test]
    fn rebuilding_the_same_codeplug_dirties_no_blocks() {
        let once = build_image(&model(), &codeplug(), &base()).expect("first build");
        let twice = build_image(&model(), &codeplug(), &once).expect("second build");
        assert_eq!(once, twice, "a second identical build changes nothing");

        // And the same again through the container, watching the bookkeeping.
        let mut image = Thd72Image::parse(&once).unwrap();
        let table = image.prog_vfo_table().unwrap();
        for sc in codeplug() {
            let mut rec = encode_channel(&sc.channel, &table).unwrap();
            rec.set_name(&sc.name);
            for (off, cell) in apply_record(sc.slot, &rec) {
                image.patch(off, &cell).unwrap();
            }
        }
        assert!(
            image.dirty_blocks().is_empty(),
            "rewriting identical memories dirtied {:?}",
            image.dirty_blocks()
        );
    }

    #[test]
    fn a_name_longer_than_the_cell_is_truncated_not_overflowed() {
        let cp = vec![slot(0, "VERYLONGREPEATERNAME", 146.520)];
        let out = build_image(&model(), &cp, &base()).expect("build");
        let decoded = decode_channels(&out);
        assert_eq!(decoded[0].name, "VERYLONG");
        // The neighbouring cell must be untouched by the overflow.
        assert!(read_record(&out, 1).is_none());
    }

    /// The regions this driver does not own come back exactly as they were read.
    #[test]
    fn nothing_outside_the_memory_cells_is_disturbed() {
        let before = base();
        let after = build_image(&model(), &codeplug(), &before).expect("build");
        assert_eq!(&after[..0x20], &before[..0x20], "front matter");
        assert_eq!(
            &after[PROG_VFO_BASE..PROG_VFO_BASE + 48],
            &before[PROG_VFO_BASE..PROG_VFO_BASE + 48],
            "the radio's own prog-VFO table"
        );
        assert_eq!(
            &after[CALIBRATION_BASE..],
            &before[CALIBRATION_BASE..],
            "per-radio calibration"
        );
    }

    #[test]
    fn channels_fall_into_groups_by_position() {
        let cp = vec![slot(0, "A", 146.520), slot(150, "B", 146.520)];
        let out = build_image(&model(), &cp, &base()).expect("build");
        let decoded = decode_channels(&out);
        assert_eq!(decoded[0].group, 0);
        assert_eq!(decoded[1].group, 1);
        assert_eq!(group_of(150), 1);
    }

    #[test]
    fn the_sample_carries_shift_and_mode_and_no_power() {
        let out = build_image(&model(), &codeplug(), &base()).expect("build");
        let sample = decode_sample(&out);
        assert_eq!(sample.len(), 4);
        assert_eq!(sample[1].shift.as_deref(), Some("-0.600"));
        assert_eq!(sample[1].mode.as_deref(), Some("FM"));
        assert!(sample.iter().all(|s| s.power == "—"), "the D72 has no power field");
    }
}

/// THROWAWAY (issue #55, Phase 3): run the app's own export pipeline against the
/// dev database and build a real TH-D72 image from a real codeplug.
///
/// Not a unit test — a harness, in the same shape as
/// `kenwood_thd75::dev_export`: it needs Tim's dev SQLite DB, which is not in
/// the repo. It answers the one question the unit tests above cannot — does a
/// codeplug of real channels, filtered by the model row actually seeded in the
/// running app, reach the encoder intact, and does the count that comes out
/// match the count that went in?
///
/// The base image is optional. Without one it builds onto a synthetic blank
/// carrying the factory prog-VFO table, which exercises the whole pipeline;
/// pointing `CPM_THD72_BASE` at a real clone image additionally proves it
/// against that radio's own band edges.
///
/// ```sh
/// CPM_DEV_DB="$HOME/Library/Application Support/com.ww8l.codeplugmagic.dev/codeplug_manager.sqlite3" \
/// CPM_CODEPLUG=3 \
/// CPM_THD72_BASE=scratchpad/kenwood_thd72/images/000-factory-reset.img \
/// cargo test --lib kenwood_thd72::program::dev_export -- --ignored --nocapture
/// ```
#[cfg(test)]
mod dev_export {
    use super::*;
    use crate::commands::export::{
        codeplug_model, exclusion_reason, expand_for_export, resolve_codeplug_groups,
        ExpandedChannel,
    };
    use crate::radios::kenwood_thd72::layout::{CALIBRATION_BASE, IMAGE_LEN, PROG_VFO_BASE};
    use crate::radios::kenwood_thd72::memory::{decode_channels, decode_group_names};

    fn synthetic_base() -> Vec<u8> {
        let mut image = vec![0xFFu8; IMAGE_LEN];
        image[0x00] = 0x1B;
        image[0x10..0x18].copy_from_slice(&[0x00, 0x00, 0x00, 0x00, 0x02, 0x00, 0x30, 0x30]);
        for (i, (start, end)) in [
            (136_000_000u32, 174_000_000u32),
            (410_000_000, 470_000_000),
            (118_000_000, 136_000_000),
            (136_000_000, 174_000_000),
            (320_000_000, 400_000_000),
            (400_000_000, 524_000_000),
        ]
        .into_iter()
        .enumerate()
        {
            let off = PROG_VFO_BASE + i * 8;
            image[off..off + 4].copy_from_slice(&start.to_le_bytes());
            image[off + 4..off + 8].copy_from_slice(&end.to_le_bytes());
        }
        image[CALIBRATION_BASE] = 0x17;
        image
    }

    #[tokio::test]
    #[ignore = "needs the dev database"]
    async fn a_real_codeplug_builds_a_thd72_image() {
        let db = std::env::var("CPM_DEV_DB").expect("CPM_DEV_DB");
        let codeplug_id: i64 = std::env::var("CPM_CODEPLUG")
            .expect("CPM_CODEPLUG")
            .parse()
            .unwrap();

        // The codeplug in the dev database belongs to whichever radio Tim built
        // it for, and `exclusion_reason` filters against THAT model. Running it
        // unchanged measured the wrong thing: a TH-D75 codeplug kept its three
        // 224 MHz repeaters — legal on a D75, which transmits there — and the
        // D72 builder then refused them, correctly but too late to be the test.
        //
        // So: work on a COPY, seed it (which introduces the TH-D72 row, absent
        // from any database seeded before this session), and re-point the
        // codeplug's profile at that model. The pipeline then runs exactly as
        // the app would for a real TH-D72 codeplug, which is the thing being
        // gated. The copy also means a harness cannot damage the dev database.
        let work = std::env::temp_dir().join(format!("thd72-gate-{codeplug_id}.sqlite3"));
        std::fs::copy(&db, &work).expect("copy the dev database");
        let pool = sqlx::SqlitePool::connect(&format!("sqlite:file:{}", work.display()))
            .await
            .expect("open the copy");
        crate::seed::seed_radio_models(&pool).await.expect("seed");
        let (d72_id,): (i64,) =
            sqlx::query_as("SELECT id FROM radio_models WHERE model = 'TH-D72'")
                .fetch_one(&pool)
                .await
                .expect("the TH-D72 is seeded");
        sqlx::query(
            "UPDATE radio_profiles SET radio_model_id = ?
              WHERE id = (SELECT radio_profile_id FROM codeplugs WHERE id = ?)",
        )
        .bind(d72_id)
        .bind(codeplug_id)
        .execute(&pool)
        .await
        .expect("re-point the codeplug at the TH-D72");

        // Exactly what `generate_codeplug` does, in the same order — see the
        // note in `kenwood_thd75::dev_export` about `codeplug_channels` being
        // private to the export module.
        let model = codeplug_model(&pool, codeplug_id).await.expect("model");
        let groups = resolve_codeplug_groups(&pool, codeplug_id).await.expect("groups");
        let mut seen = std::collections::HashSet::new();
        let channels: Vec<_> = groups
            .iter()
            .flat_map(|g| g.channels.iter().cloned())
            .filter(|c| seen.insert(c.id))
            .collect();
        let expanded = expand_for_export(&pool, channels).await.expect("expand");

        println!("model: {} ({:?})", model.display_name, model.export_format);
        let mut excluded = Vec::new();
        let included: Vec<&ExpandedChannel> = expanded
            .iter()
            .filter(|ec| match exclusion_reason(&ec.channel, &model) {
                Some(why) => {
                    excluded.push(format!("  {:>10.4}  {}", ec.channel.rx_freq, why));
                    false
                }
                None => true,
            })
            .collect();
        println!("{} channels in, {} excluded:", expanded.len(), excluded.len());
        for line in &excluded {
            println!("{line}");
        }

        // The band this radio cannot hear. `rx_bands` should have taken these
        // out already; if any survive, the seed and the encoder disagree and the
        // build below will refuse rather than write a dead memory.
        let deaf: Vec<f64> = included
            .iter()
            .map(|ec| ec.channel.rx_freq)
            .filter(|f| (174.0..320.0).contains(f))
            .collect();
        println!("channels in the 174-320 MHz gap that survived the filter: {deaf:?}");

        let slots: Vec<SlotChannel> = included
            .iter()
            .enumerate()
            .map(|(slot, ec)| SlotChannel {
                slot,
                name: ec.channel.name_short.clone().unwrap_or_default(),
                channel: ec.channel.clone(),
            })
            .collect();

        let base = match std::env::var("CPM_THD72_BASE") {
            Ok(path) => {
                println!("base image: {path}");
                std::fs::read(&path).expect("read base image")
            }
            Err(_) => {
                println!("base image: synthetic blank (factory prog-VFO table)");
                synthetic_base()
            }
        };

        let out = build_image(&model, &slots, &base).expect("build the image");
        // Hand the built image to the hardware ladder rather than rebuilding the
        // pipeline there: step 3 must write the bytes THIS produced, not a
        // second implementation that happens to agree.
        if let Ok(path) = std::env::var("CPM_THD72_BUILD_OUT") {
            std::fs::write(&path, &out).expect("save the built image");
            println!("built image written to {path}");
        }
        let decoded = decode_channels(&out);
        println!(
            "{} channels in -> {} memories present in the image",
            slots.len(),
            decoded.len()
        );
        assert_eq!(
            decoded.len(),
            slots.len(),
            "every channel handed in must be a memory in the image"
        );
        println!("group names (untouched by this phase): {:?}", decode_group_names(&out));
        for c in decoded.iter().take(10) {
            println!(
                "  {:3} g{} vfo{} {:>10.4}  {:<8}  {:<8}  {}",
                c.index, c.group, c.prog_vfo, c.rx_mhz, c.name, c.shift, c.tone
            );
        }
    }
}
