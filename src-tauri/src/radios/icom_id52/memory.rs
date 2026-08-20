//! The memory pool inside an ID-52 `.icf` — records, groups and flags.
//!
//! [`icf`](super::icf) is the envelope; this is the letter. It writes a whole
//! codeplug's worth of memories into a settings image the radio itself wrote,
//! so that **one file** programs the radio: `SET > SD Card > Load Setting` puts
//! back the memories, every MENU setting and the repeater list in a single
//! operation. That is why the `.icf` is the primary path rather than the Memory
//! CH CSV — the radio's own `Load Setting` menu says "ALL: Loads all Memory
//! channels, settings on the MENU screen, and the Repeater List".
//!
//! ## Six structures, not one
//!
//! A memory record says nothing about which group it is in, whether it is
//! skipped, or what channel number it wears. All of that lives in tables beside
//! the pool, and writing a record without maintaining them is the AnyTone
//! present-bitmap bug again: the radio simply does not show the memory.
//!
//! | address | shape | what |
//! |---|---|---|
//! | `0x000000` | 1000 x 51 | the memory records; all-`FF` means empty |
//! | `0x00DE00` | 125 bytes | "skipped at all" bitmap, LSB-first |
//! | `0x00DE7D` | 125 bytes | narrows the above to PSKIP |
//! | `0x00DEFA` | 125 bytes | a third flag bitmap, unidentified |
//! | `0x03CF58` | 100 x 19 | group table: `[ordinal][u16 BE head slot][16-char name]` |
//! | `0x03D6C4` | 1000 x u16 BE | `next[slot]`, `FFFF` ends a chain |
//! | `0x03DE94` | 100 x 13 | per-group channel-number map, LSB-first, `0` = occupied |
//!
//! Every one of those addresses was measured, never inferred from CHIRP's
//! `id31.py` — that driver describes 500 memories in 26 banks where this radio
//! has 1000 in 100 groups. They came from two sources: a paired Memory CH CSV
//! export and `.icf` taken off the radio in the same minute (which decoded the
//! record layout against 80 memories, 11 fields each, with no mismatches), and a
//! second `.icf` pair taken either side of moving four channels into four
//! different groups (which decoded the group tables). `scratchpad/id52/FINDINGS.md`
//! has the working.
//!
//! One structural check falls out for free, and it is worth stating because it
//! is independent of how the addresses were found: `0x03CF58 + 100*19` is exactly
//! `0x03D6C4`, and `0x03D6C4 + 1000*2` is exactly `0x03DE94`. Three separately
//! measured bases chain with no slack, which they would not if an entry size or
//! an alignment were wrong. [`TABLES_ARE_CONTIGUOUS`] holds the compiler to it.
//!
//! ## Chains
//!
//! `next[]` is one array serving many singly-linked lists: each group's members,
//! plus a free chain over every unused slot. Measured invariant — **each chain
//! lists its own members in ascending slot order**. In the radio's own file,
//! `next[3]` was `5` because slot 4 was free and headed the free chain, and the
//! free chain ran 4, 37, 38, 39, 49, … strictly ascending. This writer
//! reproduces that canonical shape: memories take slots `0..n`, the free chain
//! is `n..1000`, and channel numbers within a group run `0..count`.
//!
//! Where the free chain's *head* is stored is still unknown — searching for it
//! near the group table found nothing, and the block at `0x03CF45` that looked
//! like a candidate is a run of zeros. It does not block writing, because a full
//! program leaves the free chain in the one shape the radio would itself
//! produce, but it is why this module rewrites the chains wholesale rather than
//! trying to splice a single memory into an existing file.
//!
//! ## Patch, don't generate
//!
//! Everything outside the tables above is the operator's: APRS, GPS, Bluetooth
//! pairings, the opening message, the repeater list, and the 100 zeroed records
//! at slots 1010-1109 that nothing has identified yet. A real file goes in, the
//! memory pool is overwritten, the rest comes back untouched — and
//! [`only_the_memory_pool_changes`](tests::only_the_memory_pool_changes) proves
//! it against a real radio's file.

use crate::commands::export::{expanded_names, CodeplugGroup, ExpandedChannel};
use crate::models::RadioModel;
use crate::radios::driver::{CodeplugExporter, ExportRequest};

use super::icf::IcfFile;
use super::IcomId52;
use super::memory_csv::{
    assign_groups, call_signs, dtcs_polarity, duplex_and_offset, mode_of, tone_columns, truncate,
    tune_step, MAX_MEMORIES, MAX_NAME,
};

/// One memory record, and how many of them the pool holds for user memories.
///
/// The array continues past [`SLOTS`] — 10 records of `FF` filler, then 100
/// zeroed records nothing has identified, then the four call channels — and
/// this writer touches none of it.
const REC_LEN: usize = 51;
const SLOTS: usize = 1000;
const POOL: usize = 0x000000;

/// Flag bitmaps, 125 bytes each (1000 bits, LSB-first within a byte).
///
/// All three are **set** for every unused slot and clear for every used one, so
/// an empty slot reads `1` in all three; the flags only mean anything on slots
/// that are in use.
///
/// Skip is a pair, not one flag. `0xDE00` means "skipped at all" and `0xDE7D`
/// narrows that to PSKIP, so a plain-SKIP memory sets the first alone and a
/// PSKIP memory sets both — measured from one memory switched to PSKIP, against
/// 58 plain-SKIP memories that leave `0xDE7D` clear.
///
/// `0xDEFA` is still unidentified: it has never been seen set on a slot in use.
const SKIP_BITMAP: usize = 0x00DE00;
const PSKIP_BITMAP: usize = 0x00DE7D;
const UNKNOWN_BITMAP: usize = 0x00DEFA;
const BITMAP_LEN: usize = 125;

/// The three group tables. See the module docs for the shapes.
const GROUP_TABLE: usize = 0x03CF58;
const GROUP_ENTRY_LEN: usize = 19;
const NEXT_TABLE: usize = 0x03D6C4;
const POSMAP: usize = 0x03DE94;
const POSMAP_LEN: usize = 13;
const GROUPS: usize = 100;

/// End of a chain, and the value an empty group's head carries.
const NO_SLOT: u16 = 0xFFFF;

/// The three tables sit back to back with no padding. If a future capture moves
/// one of them, or an entry size turns out to be wrong, this stops compiling
/// rather than quietly writing over the next table.
///
/// Nothing reads it, and nothing should: it is evaluated at compile time, and
/// the failure it guards against is a build error, not a runtime one.
#[allow(dead_code)]
const TABLES_ARE_CONTIGUOUS: () = {
    assert!(GROUP_TABLE + GROUPS * GROUP_ENTRY_LEN == NEXT_TABLE);
    assert!(NEXT_TABLE + SLOTS * 2 == POSMAP);
};

/// Byte `0x1C`: which squelch the memory uses. Measured values only — the
/// numbering clearly has room for the other cross modes and the reverse forms,
/// but nothing has pinned them, so they are absent rather than guessed.
const SQL_OFF: u8 = 0;
const SQL_TONE: u8 = 1;
const SQL_TSQL: u8 = 3;
const SQL_DTCS: u8 = 5;
/// `TONE(T)/TSQL(R)` — the radio calls it Split Tone. Measured from a memory set
/// to transmit 103.5 and receive 107.2: the squelch byte went to `0x0B`, the
/// repeater tone took the **transmit** tone and the TSQL tone the **receive**
/// one. This is the only cross mode the channel importer actually produces.
const SQL_TONE_TSQL: u8 = 0x0B;

/// Byte `0x09`: mode.
const MODE_FM: u8 = 0;
const MODE_FM_N: u8 = 1;
const MODE_AM: u8 = 3;
const MODE_DV: u8 = 5;

/// Byte `0x22` on an analog memory. Constant across all 76 analog records in the
/// sample; what it means is not known, so it is reproduced rather than reasoned
/// about.
const ANALOG_TAIL: [u8; 3] = [0xE4, 0xFF, 0xFF];

/// CTCSS tones in tenths of a hertz, indexed as the radio stores them. The same
/// 50-entry table the settings menu uses: Repeater Tone `254.1` is index 49 and
/// TSQL `67.0` is index 0.
const TONES_DHZ: [u16; 50] = [
    670, 693, 719, 744, 770, 797, 825, 854, 885, 915, 948, 974, 1000, 1035, 1072, 1109, 1148, 1188,
    1230, 1273, 1318, 1365, 1413, 1462, 1514, 1567, 1598, 1622, 1655, 1679, 1713, 1738, 1773, 1799,
    1835, 1862, 1899, 1928, 1966, 1995, 2035, 2065, 2107, 2181, 2257, 2291, 2336, 2418, 2503, 2541,
];

/// DTCS codes as the radio indexes them. Written in octal the way the front
/// panel shows them, which is also how the channel database stores them.
const DTCS_CODES: [u16; 104] = [
    23, 25, 26, 31, 32, 36, 43, 47, 51, 53, 54, 65, 71, 72, 73, 74, 114, 115, 116, 122, 125, 131,
    132, 134, 143, 145, 152, 155, 156, 162, 165, 172, 174, 205, 212, 223, 225, 226, 243, 244, 245,
    246, 251, 252, 255, 261, 263, 265, 266, 271, 274, 306, 311, 315, 325, 331, 332, 343, 346, 351,
    356, 364, 365, 371, 411, 412, 413, 423, 431, 432, 445, 446, 452, 454, 455, 462, 464, 465, 466,
    503, 506, 516, 523, 526, 532, 546, 565, 606, 612, 624, 627, 631, 632, 654, 662, 664, 703, 712,
    723, 731, 732, 734, 743, 754,
];

/// Patch a codeplug into a settings file read off the radio.
///
/// Refuses a file from another Icom, and refuses a `#MapRev` this driver has not
/// seen: the radio can be told to write an earlier firmware's file layout
/// (`SET > SD Card > Save Form`), which is exactly the switch that would move
/// every address above.
pub(crate) fn write_codeplug(
    icf: &mut IcfFile,
    channels: &[&ExpandedChannel],
    groups: &[CodeplugGroup],
    model: &RadioModel,
) -> Result<usize, String> {
    super::check_is_an_id52_file(icf)?;
    write_memories(icf.image_mut(), channels, groups, model)
}

/// Overwrite the memory pool and every table that describes it.
///
/// Wholesale, not incremental: after this returns, the image's memories are
/// exactly `channels` and nothing of whatever was there before survives.
pub(crate) fn write_memories(
    image: &mut [u8],
    channels: &[&ExpandedChannel],
    groups: &[CodeplugGroup],
    model: &RadioModel,
) -> Result<usize, String> {
    if image.len() != super::ID52_IMAGE_LEN {
        return Err(format!(
            "That is not an ID-52 settings image: expected {} bytes, got {}.",
            super::ID52_IMAGE_LEN,
            image.len()
        ));
    }
    if channels.len() > MAX_MEMORIES {
        return Err(format!(
            "This codeplug has {} channels; the ID-52 holds {MAX_MEMORIES}. \
             Remove {} before programming.",
            channels.len(),
            channels.len() - MAX_MEMORIES
        ));
    }

    // The same placement the CSV writer uses, so both paths agree about which
    // group a channel lands in and what number it wears there. It also enforces
    // the 100-group and 100-per-group limits.
    let (placements, group_names) = assign_groups(channels, groups)?;
    let names = expanded_names(channels.iter().copied(), model);
    let used = channels.len();

    // Records. Empty is all-`FF`, which is what the radio writes for a slot that
    // has never held a memory.
    image[POOL..POOL + SLOTS * REC_LEN].fill(0xFF);
    for (i, ec) in channels.iter().enumerate() {
        let at = POOL + i * REC_LEN;
        encode(&mut image[at..at + REC_LEN], ec, &names[i]);
    }

    // Flag bitmaps. Set everywhere means "unused", so filling first and clearing
    // the used slots gets the empty tail right without a second pass.
    for base in [SKIP_BITMAP, PSKIP_BITMAP, UNKNOWN_BITMAP] {
        image[base..base + BITMAP_LEN].fill(0xFF);
        for slot in 0..used {
            clear_bit(&mut image[base..base + BITMAP_LEN], slot);
        }
    }
    // Nothing sets SKIP yet: the channel database has no per-channel scan skip,
    // the same gap the FT5D and CSV writers have. When it gains one, this is the
    // only line that changes.

    // Group table, chains and channel numbers.
    for g in 0..GROUPS {
        let entry = GROUP_TABLE + g * GROUP_ENTRY_LEN;
        // Byte 0 is an ordinal the radio maintains — it reads `g + 1`, not `g`,
        // and nothing here has any business rewriting it.
        image[entry + 1..entry + 3].copy_from_slice(&NO_SLOT.to_be_bytes());
        ascii_field(&mut image[entry + 3..entry + 3 + MAX_NAME], "");
        image[POSMAP + g * POSMAP_LEN..POSMAP + (g + 1) * POSMAP_LEN].fill(0xFF);
    }
    image[NEXT_TABLE..NEXT_TABLE + SLOTS * 2].fill(0xFF);

    // Slot `i` holds channel `i`, so a group's members are already in ascending
    // slot order and its channel numbers already run from zero — the canonical
    // shape the radio produces for itself.
    let mut tail: Vec<Option<usize>> = vec![None; group_names.len()];
    for (slot, p) in placements.iter().enumerate() {
        let entry = GROUP_TABLE + p.group_no * GROUP_ENTRY_LEN;
        match tail[p.group_no] {
            None => image[entry + 1..entry + 3].copy_from_slice(&(slot as u16).to_be_bytes()),
            Some(prev) => link(image, prev, slot as u16),
        }
        tail[p.group_no] = Some(slot);
        clear_bit(
            &mut image[POSMAP + p.group_no * POSMAP_LEN..POSMAP + (p.group_no + 1) * POSMAP_LEN],
            p.ch_no,
        );
    }
    for (g, name) in group_names.iter().enumerate() {
        let entry = GROUP_TABLE + g * GROUP_ENTRY_LEN;
        ascii_field(
            &mut image[entry + 3..entry + 3 + MAX_NAME],
            &truncate(name, MAX_NAME),
        );
    }

    // The free chain: every unused slot, ascending, which is how the radio's own
    // file had it. Its head is not stored anywhere we have found, so a full
    // rewrite is the only shape we can be confident in.
    for slot in used..SLOTS.saturating_sub(1) {
        link(image, slot, slot as u16 + 1);
    }

    Ok(used)
}

impl CodeplugExporter for IcomId52 {
    fn export_format(&self) -> &'static str {
        "icom_id52_icf"
    }

    /// Two card files, one exporter, chosen by the extension of the file the
    /// operator picked.
    ///
    /// A target that already exists is patched in place, with the untouched
    /// original kept beside it as `.orig`. A **folder** means "write me a new
    /// file", which is the safer default and the one the card actions use: the
    /// operator's own saves are never modified at all, and the radio holds many
    /// settings files quite happily.
    ///
    /// The `.icf` is the programming path: `SD Card > Load Setting > ALL`
    /// restores memories *and* every MENU setting in one operation, so a single
    /// patched file carries the whole codeplug and the radio profile together.
    /// A `.csv` writes the Memory CH export instead — memories only, imported
    /// separately, and useful as a cross-check that both writers describe the
    /// same radio.
    ///
    /// A driver may claim only one export format, and this radio genuinely has
    /// two card files; the extension is the operator's own statement of which
    /// one they are pointing at.
    fn export(&self, path: &str, req: &ExportRequest) -> Result<usize, String> {
        if path.to_ascii_lowercase().ends_with(".csv") {
            return super::memory_csv::write_csv(path, req);
        }
        // A target that does not exist yet is a NEW file on the card, and the
        // template is the newest settings file sitting beside it. A target that
        // does exist is the operator pointing at one file and meaning it.
        let template = if std::path::Path::new(path).is_file() {
            path.to_string()
        } else {
            let dir = std::path::Path::new(path)
                .parent()
                .ok_or("There is no folder to write into.")?;
            newest_settings_file(dir)?
        };
        patch_icf(&template, path, req)
    }

    /// A folder means "make me a new file in here", named the way the radio
    /// names its own so it sorts into place on the Load Setting screen.
    fn resolve_target(&self, path: &str) -> Result<String, String> {
        if !std::path::Path::new(path).is_dir() {
            return Ok(path.to_string());
        }
        // Prove there is something to patch before naming the output: a folder
        // with no readable settings file cannot produce one, and finding that
        // out now beats reporting a file name and then failing.
        newest_settings_file(std::path::Path::new(path))?;
        Ok(next_settings_file(std::path::Path::new(path))
            .to_string_lossy()
            .into_owned())
    }
}

/// Every `.icf` in a folder that this driver can actually patch, oldest first.
/// The radio's own names sort chronologically, which is what makes "newest"
/// mean anything.
fn settings_files(dir: &std::path::Path) -> Vec<std::path::PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out: Vec<std::path::PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.extension().is_some_and(|e| e.eq_ignore_ascii_case("icf"))
                && std::fs::read_to_string(p).is_ok_and(|t| {
                    IcfFile::parse(&t).is_ok_and(|f| super::check_is_an_id52_file(&f).is_ok())
                })
        })
        .collect();
    out.sort();
    out
}

/// The template a new file is built from: the most recent settings file the
/// radio wrote to this card. Everything the codeplug does not describe is
/// inherited from it, so the newest is the one that reflects the radio best.
fn newest_settings_file(dir: &std::path::Path) -> Result<String, String> {
    settings_files(dir)
        .last()
        .map(|p| p.to_string_lossy().into_owned())
        .ok_or_else(|| {
            format!(
                "No ID-52 settings file to work from in {}. On the radio, save one with                  SET > SD Card > Save Setting, then try again.",
                dir.display()
            )
        })
}

/// The radio's own naming scheme, `SetYYYYMMDD_NN.icf`, with today's date and
/// the first free number. Names longer than 23 characters are invisible to the
/// radio's file picker (Advanced Manual p. 2-7); this one is 18.
fn next_settings_file(dir: &std::path::Path) -> std::path::PathBuf {
    let today = chrono::Local::now().format("%Y%m%d").to_string();
    for n in 1..=99 {
        let candidate = dir.join(format!("Set{today}_{n:02}.icf"));
        if !candidate.exists() {
            return candidate;
        }
    }
    dir.join(format!("Set{today}_99.icf"))
}

/// Patch the operator's own `.icf` in place — memories, groups, and the radio
/// profile's settings — and keep their untouched original beside it.
///
/// In place, because the radio reads a file it wrote itself and everything this
/// codeplug does not describe (the repeater list, DV call signs, Bluetooth
/// pairings) has to survive the round trip. `.orig` is written once and never
/// overwritten, so a second export cannot clobber the only pristine copy.
fn patch_icf(template: &str, path: &str, req: &ExportRequest) -> Result<usize, String> {
    let text = std::fs::read_to_string(template).map_err(|e| {
        format!(
            "Could not read {path}: {e}. Pick a settings file the radio saved for \
             itself with SET > SD Card > Save Setting — they live in ID-52/Setting/."
        )
    })?;
    let mut icf = IcfFile::parse(&text)?;
    let written = write_codeplug(&mut icf, req.channels, req.groups, req.model)?;

    // Settings ride in the same file as the memories, so a codeplug export is
    // also a settings write. Nothing happens if the profile has none — an empty
    // profile leaves the radio's own settings exactly as they were.
    if let Some(json) = req.profile_settings {
        let parsed: serde_json::Value = serde_json::from_str(json)
            .map_err(|e| format!("This profile's settings are not valid JSON: {e}"))?;
        if let Some(map) = parsed.as_object() {
            super::settings::apply_settings(icf.image_mut(), map);
        }
    }

    // Only when we are overwriting the operator's file. Writing a NEW one leaves
    // theirs untouched by construction, so a `.orig` would be a copy of a file
    // nothing has modified.
    if template == path {
        let orig = format!("{path}.orig");
        if !std::path::Path::new(&orig).exists() {
            std::fs::write(&orig, &text)
                .map_err(|e| format!("Could not save the original as {orig}: {e}"))?;
        }
    }
    std::fs::write(path, icf.render()).map_err(|e| format!("Could not write {path}: {e}"))?;
    Ok(written)
}

/// Encode one memory. Field offsets are the measured ones; see FINDINGS.md.
fn encode(rec: &mut [u8], ec: &ExpandedChannel, name: &str) {
    let c = &ec.channel;
    let (dup, offset_mhz) = duplex_and_offset(ec);
    let rx = hz(c.rx_freq);
    let offset = hz(offset_mhz);
    let duplex = match dup {
        "DUP-" => 1u8,
        "DUP+" => 2,
        _ => 0,
    };
    let tx = match duplex {
        1 => rx.saturating_sub(offset),
        2 => rx.saturating_add(offset),
        _ => rx,
    };

    let mode_name = mode_of(ec);
    let mode = match mode_name {
        "FM-N" => MODE_FM_N,
        "AM" => MODE_AM,
        "DV" => MODE_DV,
        // The radio has WFM on the dial but the sample never stored one, so
        // there is no measured byte for it. Programming it as FM is the same
        // choice the rest of the app makes for a mode a radio cannot hold, and
        // it keeps the memory listenable rather than inventing an index.
        _ => MODE_FM,
    };

    // Airband AM sits on the 8.33 kHz grid, both in the tuning-step byte and in
    // the raster's receive nibble. `tune_step` already owns that decision for
    // the CSV writer, so it owns it here too.
    let airband = tune_step(ec) == "8.33kHz";

    rec[0x00..0x04].copy_from_slice(&rx.to_be_bytes());
    rec[0x04..0x08].copy_from_slice(&offset.to_be_bytes());
    // Which channel grid each frequency sits on, so the radio can dial off the
    // memory sensibly. High nibble receive, low nibble transmit.
    rec[0x08] = if airband {
        0x20
    } else {
        (raster(rx) << 4) | raster(tx)
    };
    rec[0x09] = mode;
    rec[0x0A] = if airband { 2 } else { 0 };
    // The low nibble is `F` in every record in the sample, whatever the duplex.
    rec[0x0B] = (duplex << 4) | 0x0F;
    ascii_field(&mut rec[0x0C..0x1C], name);
    rec[0x1D] = dtcs_polarity_index(dtcs_polarity(&c.dcs_polarity));

    if mode == MODE_DV {
        let (your, rpt1, rpt2) = call_signs(ec, true);
        rec[0x1C] = SQL_OFF;
        rec[0x1D] = 0;
        rec[0x1E..0x25].copy_from_slice(&pack_call(&your));
        rec[0x25..0x2C].copy_from_slice(&pack_call(&rpt1));
        rec[0x2C..0x33].copy_from_slice(&pack_call(&rpt2));
        return;
    }

    let (tone, rpt_hz, tsql_hz) = tone_columns(ec);
    rec[0x1C] = match tone {
        "TONE" => SQL_TONE,
        "TSQL" => SQL_TSQL,
        "DTCS" => SQL_DTCS,
        "TONE(T)/TSQL(R)" => SQL_TONE_TSQL,
        "OFF" => SQL_OFF,
        // The two DTCS-flavoured cross modes are still unmeasured. Rather than
        // guess an index, degrade to the transmit tone — the fallback every
        // radio without cross modes already gets. The common cross mode, the one
        // above, is now exact.
        _ => SQL_TONE,
    };
    // On a cross-tone memory these are not interchangeable: the repeater tone is
    // what the radio transmits and the TSQL tone is what it opens squelch on.
    // `tone_columns` already returns them in that order.
    rec[0x1E] = tone_index(rpt_hz);
    rec[0x1F] = tone_index(tsql_hz);
    rec[0x20] = dtcs_index(c.dcs_code.as_deref());
    // `0x21` looked like the polarity byte for a while and is not: it is 0 in
    // every record ever captured, including six DTCS memories and the `TN-RR`
    // one that finally located the real field at `0x1D`.
    rec[0x21] = 0;
    rec[0x22..0x25].copy_from_slice(&ANALOG_TAIL);
    // An analog memory leaves both repeater call signs as `FF`, not as packed
    // spaces — that difference is measured, and it is the sort of thing that
    // makes a file we wrote distinguishable from one the radio wrote.
    rec[0x25..0x33].fill(0xFF);
}

/// Which channel grid a frequency sits on: 5 kHz, or 6.25 kHz when it is not a
/// multiple of 5. Anything on neither grid is stored as 5 kHz, which only
/// affects what happens when the operator dials off the memory — the frequency
/// itself is exact either way.
fn raster(freq_hz: u32) -> u8 {
    if freq_hz.is_multiple_of(5_000) {
        0
    } else if freq_hz.is_multiple_of(6_250) {
        1
    } else {
        0
    }
}

fn hz(mhz: f64) -> u32 {
    (mhz * 1_000_000.0).round().clamp(0.0, u32::MAX as f64) as u32
}

/// Nearest tone in the radio's table. A channel carrying a tone this radio does
/// not have is better off on the closest one it does than on a wild index.
fn tone_index(hz: f64) -> u8 {
    let want = (hz * 10.0).round().clamp(0.0, u16::MAX as f64) as u16;
    TONES_DHZ
        .iter()
        .enumerate()
        .min_by_key(|(_, &t)| t.abs_diff(want))
        .map(|(i, _)| i as u8)
        .unwrap_or(0)
}

/// DTCS polarity, byte `0x1D`.
///
/// `BOTH N` (0) and `TN-RR` (1) are measured — six of the operator's DTCS
/// memories carry 0, and one switched to `TN-RR` carries 1, while switching a
/// memory to DTCS on its own leaves the byte alone. The remaining two follow the
/// order the radio's own CSV column uses. That last step is an inference, and a
/// small one to get wrong: a bad polarity means squelch does not open, not a
/// damaged radio.
fn dtcs_polarity_index(polarity: &str) -> u8 {
    match polarity {
        "TN-RR" => 1,
        "TR-RN" => 2,
        "BOTH R" => 3,
        _ => 0,
    }
}

/// The stored DTCS code's index, or the radio's default `023` when the channel
/// has none — every analog record carries a code whether or not it uses one.
fn dtcs_index(code: Option<&str>) -> u8 {
    let Some(want) = code.and_then(|c| c.trim().parse::<u16>().ok()) else {
        return 0;
    };
    DTCS_CODES
        .iter()
        .position(|&c| c == want)
        .unwrap_or(0)
        .try_into()
        .unwrap_or(0)
}

/// Icom's call-sign packing: 8 characters at 7 bits each, most significant
/// first, into 7 bytes. The port letter sits in the 8th column, which is why the
/// field is padded rather than trimmed.
fn pack_call(call: &str) -> [u8; 7] {
    let mut bits: u64 = 0;
    let mut chars = call.chars();
    for _ in 0..8 {
        let c = chars.next().unwrap_or(' ');
        let byte = if c.is_ascii() { c as u8 } else { b' ' };
        bits = (bits << 7) | u64::from(byte & 0x7F);
    }
    let full = bits.to_be_bytes();
    full[1..8].try_into().expect("56 bits is 7 bytes")
}

/// Write an ASCII field, space-padded to the field's width and truncated to it.
/// Names reach here already cut to the radio's limit; this is the backstop that
/// keeps a stray character out of the next field.
fn ascii_field(field: &mut [u8], text: &str) {
    field.fill(b' ');
    for (slot, c) in field.iter_mut().zip(text.chars()) {
        *slot = if c.is_ascii_graphic() || c == ' ' {
            c as u8
        } else {
            b'?'
        };
    }
}

fn link(image: &mut [u8], slot: usize, next: u16) {
    let at = NEXT_TABLE + slot * 2;
    image[at..at + 2].copy_from_slice(&next.to_be_bytes());
}

fn clear_bit(bitmap: &mut [u8], bit: usize) {
    bitmap[bit / 8] &= !(1 << (bit % 8));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::Channel;
    use crate::radios::icom_id52::DRIVER;

    fn model() -> RadioModel {
        RadioModel {
            display_name: "Icom ID-52".into(),
            max_name_length: Some(MAX_NAME as i64),
            ..Default::default()
        }
    }

    fn chan(id: i64, name: &str, rx: f64) -> Channel {
        Channel {
            id,
            name_short: Some(name.into()),
            rx_freq: rx,
            dcs_polarity: "NN".into(),
            cross_mode: "Tone->Tone".into(),
            mode: Some("FM".into()),
            ..Default::default()
        }
    }

    fn expand(c: Channel) -> ExpandedChannel {
        ExpandedChannel {
            channel: c,
            tg_label: None,
            timeslot: None,
            tg_number: None,
            tg_call_type: None,
            tg_inline: false,
        }
    }

    /// A blank image of the right size. Filled with a marker rather than zeros so
    /// a test can tell "this writer put a zero here" from "nobody touched this".
    fn blank() -> Vec<u8> {
        vec![0x5A; super::super::ID52_IMAGE_LEN]
    }

    /// [`blank`] with a pattern instead of one repeated marker byte.
    ///
    /// A uniform fill hides a stray write of that same byte, and `0x5A` is
    /// exactly what this writer would leave behind in an untouched record. A
    /// pattern with no repeated runs makes any byte the writer touches outside
    /// the arrays it claims visible. (#88)
    fn patterned() -> Vec<u8> {
        (0..super::super::ID52_IMAGE_LEN)
            .map(|i| (i.wrapping_mul(31) ^ 0xA5) as u8)
            .collect()
    }

    /// The image inside a fixture `.icf`: the marker byte everywhere except the
    /// settings block, which is zeroed.
    ///
    /// The marker would decode as a *live* setting — `0x5A` reads as a checkbox
    /// that is already on, and `apply_settings` deliberately leaves a field that
    /// already holds the wanted value alone — so a fixture full of it cannot
    /// show a settings write happening. A real file holds real values here.
    fn card_image() -> Vec<u8> {
        let mut img = blank();
        img[0x03C9AC..0x03CCB6].fill(0);
        img
    }

    /// A whole `.icf` the way the radio writes one: right model id, right map
    /// revision, a full-length image, and a checksum that matches its contents.
    /// The bytes inside are a marker rather than a capture — what these tests
    /// check is which of them the exporter changes, and a real radio's file is
    /// not something to commit.
    fn synth_icf(image: &[u8]) -> String {
        let mut text = format!(
            "{}\r\n#Comment=\r\n#MapRev={}\r\n#EtcData=000000\r\n",
            super::super::ID52_MODEL_ID,
            super::super::ID52_MAP_REV
        );
        for (i, chunk) in image.chunks(64).enumerate() {
            text.push_str(&format!("{:08X}{:02X}", i * 64, chunk.len()));
            for b in chunk {
                text.push_str(&format!("{b:02X}"));
            }
            text.push_str("\r\n");
        }
        IcfFile::parse(&text)
            .expect("synthetic file parses")
            .with_digest()
            .render()
    }

    /// A scratch directory of this test's own, cleaned up when it drops. The
    /// exporter writes files beside the one it is given, so each test needs a
    /// directory rather than a path — and no test may see another's leftovers.
    struct Scratch(std::path::PathBuf);

    impl Scratch {
        fn new(name: &str) -> Self {
            let dir = std::env::temp_dir().join(format!("id52_{}_{name}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).expect("scratch dir");
            Self(dir)
        }

        fn join(&self, name: &str) -> String {
            self.0.join(name).to_string_lossy().into_owned()
        }

        /// A `.icf` on disk for the exporter to patch in place.
        fn card_file(&self, name: &str, image: &[u8]) -> String {
            let path = self.join(name);
            std::fs::write(&path, synth_icf(image)).expect("write fixture");
            path
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn request<'a>(
        channels: &'a [&'a ExpandedChannel],
        model: &'a RadioModel,
        settings: Option<&'a str>,
    ) -> ExportRequest<'a> {
        ExportRequest {
            channels,
            groups: &[],
            model,
            profile_settings: settings,
        }
    }

    /// The whole point of the `.icf` path: one file carries both halves of a
    /// codeplug. Memories land in the pool, the profile's settings land in the
    /// settings block, everything else survives, and the operator's original is
    /// kept beside it.
    #[test]
    fn an_icf_export_patches_memories_and_settings_together() {
        let scratch = Scratch::new("patch");
        let path = scratch.card_file("Set20260813_01.icf", &card_image());
        let ec = expand(chan(1, "PROBE ICF", 146.520));
        let model = model();
        // Two settings the form could produce: a select label and a checkbox.
        let settings = r#"{"squelch-a":"5","busy-led":true}"#;

        let n = DRIVER
            .export(&path, &request(&[&ec], &model, Some(settings)))
            .expect("export");
        assert_eq!(n, 1);

        let patched = std::fs::read_to_string(&path).unwrap();
        let icf = IcfFile::parse(&patched).expect("the patched file is still a valid ICF");
        let img = icf.image();
        assert_eq!(&img[POOL + 0x0C..POOL + 0x0C + 9], b"PROBE ICF");
        assert_eq!(img[0x03CA99], 6, "Squelch A: level 5 stores as 6");
        assert_eq!(img[0x03C9E2], 1, "Busy LED on");
        // A byte in neither region is the operator's and must be untouched:
        // 0x03C900 sits between the memory pool's tables and the settings
        // block, in the run this driver has never had a reason to name.
        assert_eq!(img[0x03C900], 0x5A);
    }

    /// The card action's whole point: pointed at the folder, the exporter names
    /// a new file the way the radio does and leaves every existing file exactly
    /// as it was. Nothing to overwrite means nothing to keep a `.orig` of.
    #[test]
    fn a_folder_target_writes_a_new_file_and_touches_no_existing_one() {
        let scratch = Scratch::new("newfile");
        let template = scratch.card_file("Set20260101_01.icf", &card_image());
        let before = std::fs::read_to_string(&template).unwrap();
        let ec = expand(chan(1, "PROBE NEW", 146.520));
        let model = model();

        let dir = scratch.join("");
        let target = DRIVER.resolve_target(&dir).expect("names a file");
        let n = DRIVER
            .export(&target, &request(&[&ec], &model, None))
            .expect("export");

        assert_eq!(n, 1);
        let name = std::path::Path::new(&target)
            .file_name()
            .unwrap()
            .to_string_lossy()
            .into_owned();
        // The radio's own scheme, and short enough for its file picker.
        assert!(name.starts_with("Set"), "{name}");
        assert!(name.ends_with("_01.icf"), "{name}");
        assert!(name.len() <= 23, "{name} is too long for the radio's picker");

        // The operator's file is untouched, and no .orig was left beside it.
        assert_eq!(std::fs::read_to_string(&template).unwrap(), before);
        assert!(!std::path::Path::new(&format!("{template}.orig")).exists());

        // ...and the new one really is the codeplug, built from that template.
        let written = std::fs::read_to_string(&target).unwrap();
        let img = IcfFile::parse(&written).expect("a valid ICF").image().to_vec();
        assert_eq!(&img[POOL + 0x0C..POOL + 0x0C + 9], b"PROBE NEW");
    }

    /// A second write to the same card does not collide with the first: the
    /// name carries a counter, which is how the radio avoids it too.
    #[test]
    fn a_second_new_file_gets_the_next_number() {
        let scratch = Scratch::new("nextnumber");
        scratch.card_file("Set20260101_01.icf", &card_image());
        let ec = expand(chan(1, "PROBE", 146.520));
        let model = model();
        let dir = scratch.join("");

        let first = DRIVER.resolve_target(&dir).unwrap();
        DRIVER
            .export(&first, &request(&[&ec], &model, None))
            .unwrap();
        let second = DRIVER.resolve_target(&dir).unwrap();
        DRIVER
            .export(&second, &request(&[&ec], &model, None))
            .unwrap();

        assert_ne!(first, second);
        assert!(std::path::Path::new(&first).exists());
        assert!(std::path::Path::new(&second).exists());
    }

    /// A folder with nothing to patch cannot produce a file, and says so BEFORE
    /// naming one — a file name in a success message that does not exist would
    /// send the operator looking for it on the radio.
    #[test]
    fn a_folder_with_no_usable_template_is_refused_by_name() {
        let scratch = Scratch::new("notemplate");
        // A revision the driver does not write: present, parseable, unusable.
        let stale = synth_icf(&card_image()).replace("#MapRev=3", "#MapRev=2");
        std::fs::write(scratch.join("Set20260101_01.icf"), stale).unwrap();

        let err = DRIVER.resolve_target(&scratch.join("")).unwrap_err();
        assert!(err.contains("Save Setting"), "{err}");
    }

    /// The untouched original is written once and never again: a second export
    /// must not overwrite the only pristine copy with an already-patched one.
    #[test]
    fn the_first_export_keeps_the_original_and_later_ones_leave_it_alone() {
        let scratch = Scratch::new("orig");
        let path = scratch.card_file("Set20260813_02.icf", &card_image());
        let before = std::fs::read_to_string(&path).unwrap();
        let ec = expand(chan(1, "FIRST", 146.520));
        let model = model();

        DRIVER
            .export(&path, &request(&[&ec], &model, None))
            .expect("first export");
        let orig = format!("{path}.orig");
        assert_eq!(std::fs::read_to_string(&orig).unwrap(), before);

        let ec2 = expand(chan(2, "SECOND", 147.000));
        DRIVER
            .export(&path, &request(&[&ec2], &model, None))
            .expect("second export");
        assert_eq!(
            std::fs::read_to_string(&orig).unwrap(),
            before,
            "the pristine copy was overwritten by a patched one"
        );
    }

    /// A file saved to an older firmware's layout describes different
    /// addresses, so it is refused by name — and refused *before* anything is
    /// written, leaving the operator's file exactly as it was.
    #[test]
    fn an_older_layout_revision_is_refused_without_touching_the_file() {
        let scratch = Scratch::new("maprev");
        let path = scratch.card_file("Set20260813_03.icf", &card_image());
        let stale = std::fs::read_to_string(&path)
            .unwrap()
            .replace("#MapRev=3", "#MapRev=2");
        std::fs::write(&path, &stale).unwrap();

        let ec = expand(chan(1, "PROBE", 146.520));
        let model = model();
        let err = DRIVER
            .export(&path, &request(&[&ec], &model, None))
            .unwrap_err();

        assert!(err.contains("revision 2"), "{err}");
        assert!(err.contains("Save Setting"), "{err}");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), stale);
        assert!(!std::path::Path::new(&format!("{path}.orig")).exists());
    }

    /// The radio's other card file. One driver may claim one export format, and
    /// the extension of the file the operator picked is what says which of the
    /// two they meant.
    #[test]
    fn a_csv_path_writes_the_memory_ch_export_instead() {
        let scratch = Scratch::new("csv");
        let path = scratch.join("ID52.csv");
        let ec = expand(chan(1, "PROBE CSV", 146.520));
        let model = model();

        DRIVER
            .export(&path, &request(&[&ec], &model, None))
            .expect("export");

        let csv = std::fs::read_to_string(&path).unwrap();
        assert!(csv.starts_with("Group No,Group Name,CH No,Name,"), "{csv}");
        assert!(csv.contains("PROBE CSV"), "{csv}");
    }

    /// The whole export path, run over a file the radio actually wrote: patch a
    /// real capture with a codeplug and the settings decoded out of that same
    /// file, and everything outside the memory pool's own tables must come back
    /// byte for byte — the operator's repeater list, call signs and every
    /// setting, including the ones this driver has never named.
    ///
    /// The synthetic fixtures above cannot show this: they only prove the
    /// exporter leaves alone the bytes it was never given, and the risk here is
    /// the bytes it *was* given.
    ///
    ///     cargo test --lib icom_id52 -- --ignored --nocapture
    #[test]
    #[ignore = "needs a real .icf under scratchpad/id52/"]
    fn patching_a_real_capture_disturbs_nothing_but_the_memories() {
        // A capture the RADIO wrote, not RT Systems: only the radio's own
        // files carry `#MapRev=3`, and revision 3 is the only layout this
        // driver writes. RTS saves revision 1, which the exporter refuses.
        let capture = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../scratchpad/id52/id52_01_base.icf"
        );
        let text = std::fs::read_to_string(capture).unwrap_or_else(|e| {
            panic!("this test needs the capture at {capture} ({e}). It is #[ignore]d because \
                    scratchpad/ is gitignored — running it without the file must FAIL, not \
                    quietly pass having asserted nothing. (#89)")
        });
        let before = IcfFile::parse(&text).expect("a real ID-52 file parses");
        let settings = crate::radios::icom_id52::settings::decode_settings(before.image());
        let before = before.image().to_vec();

        let scratch = Scratch::new("realcapture");
        let path = scratch.join("id52_01_base.icf");
        std::fs::write(&path, &text).expect("copy the capture");

        let ec = expand(chan(1, "PROBE REAL", 146.520));
        let model = model();
        DRIVER
            .export(
                &path,
                &request(&[&ec], &model, Some(&settings.to_string())),
            )
            .expect("export");

        let patched = std::fs::read_to_string(&path).unwrap();
        let after = IcfFile::parse(&patched).expect("the patched capture is still a valid ICF");
        let after = after.image();

        // The regions the memory writer owns, from the module table above.
        let owned = |i: usize| {
            (POOL..POOL + SLOTS * REC_LEN).contains(&i)
                || (SKIP_BITMAP..UNKNOWN_BITMAP + BITMAP_LEN).contains(&i)
                || (GROUP_TABLE..POSMAP + GROUPS * POSMAP_LEN).contains(&i)
        };
        let moved: Vec<usize> = (0..before.len())
            .filter(|&i| before[i] != after[i] && !owned(i))
            .collect();
        assert!(
            moved.is_empty(),
            "{} bytes changed outside the memory tables, first at {:06X}",
            moved.len(),
            moved.first().copied().unwrap_or(0)
        );
        assert_eq!(&after[POOL + 0x0C..POOL + 0x0C + 10], b"PROBE REAL");
        eprintln!("ok: a real capture patched with only its memory tables moving");
    }

    fn write(channels: &[&ExpandedChannel], groups: &[CodeplugGroup]) -> Vec<u8> {
        let mut img = blank();
        write_memories(&mut img, channels, groups, &model()).expect("write");
        img
    }

    fn rec(img: &[u8], slot: usize) -> &[u8] {
        &img[POOL + slot * REC_LEN..POOL + (slot + 1) * REC_LEN]
    }

    fn head(img: &[u8], g: usize) -> u16 {
        let e = GROUP_TABLE + g * GROUP_ENTRY_LEN;
        u16::from_be_bytes([img[e + 1], img[e + 2]])
    }

    fn next(img: &[u8], slot: usize) -> u16 {
        let a = NEXT_TABLE + slot * 2;
        u16::from_be_bytes([img[a], img[a + 1]])
    }

    /// Walk a group's chain the way the radio does.
    fn chain(img: &[u8], g: usize) -> Vec<u16> {
        let mut out = Vec::new();
        let mut slot = head(img, g);
        while slot != NO_SLOT && out.len() <= SLOTS {
            out.push(slot);
            slot = next(img, slot as usize);
        }
        out
    }

    /// The channel numbers a group's members occupy.
    fn positions(img: &[u8], g: usize) -> Vec<usize> {
        let blk = &img[POSMAP + g * POSMAP_LEN..POSMAP + (g + 1) * POSMAP_LEN];
        (0..100).filter(|p| (blk[p / 8] >> (p % 8)) & 1 == 0).collect()
    }

    fn bit(img: &[u8], base: usize, slot: usize) -> bool {
        (img[base + slot / 8] >> (slot % 8)) & 1 == 1
    }

    /// The three checks that caught every mistake while the format was being
    /// decoded, run against our own output: every occupied slot is chained
    /// exactly once, the free chain is exactly the rest, and each group's
    /// channel-number count matches its chain length. A writer that gets records
    /// right and bookkeeping wrong produces a radio that shows nothing.
    fn assert_invariants(img: &[u8], used: usize) {
        let mut seen: Vec<u16> = Vec::new();
        for g in 0..GROUPS {
            let c = chain(img, g);
            assert_eq!(
                c.len(),
                positions(img, g).len(),
                "group {g}: {} members chained but {} channel numbers taken",
                c.len(),
                positions(img, g).len()
            );
            seen.extend(&c);
        }
        seen.sort_unstable();
        assert_eq!(
            seen,
            (0..used as u16).collect::<Vec<_>>(),
            "the chained slots are not exactly the occupied ones"
        );

        let mut free = Vec::new();
        let mut slot = used as u16;
        while (slot as usize) < SLOTS {
            free.push(slot);
            let n = next(img, slot as usize);
            if n == NO_SLOT {
                break;
            }
            assert_eq!(n, slot + 1, "the free chain must ascend");
            slot = n;
        }
        assert_eq!(free.len(), SLOTS - used, "the free chain must cover the rest");
    }

    /// The ordinary case, byte for byte against the measured layout: an output
    /// frequency, a minus shift, a transmit tone, and a name.
    #[test]
    fn a_repeater_encodes_to_the_measured_field_layout() {
        let mut c = chan(1, "W0ABC", 146.940);
        c.tx_freq = Some(146.340);
        c.tone_mode = Some("Tone".into());
        c.ctcss_uplink = Some(100.0);
        let ec = expand(c);

        let img = write(&[&ec], &[]);
        let r = rec(&img, 0);

        assert_eq!(u32::from_be_bytes(r[0..4].try_into().unwrap()), 146_940_000);
        assert_eq!(u32::from_be_bytes(r[4..8].try_into().unwrap()), 600_000);
        assert_eq!(r[0x08], 0x00, "both frequencies are on the 5 kHz grid");
        assert_eq!(r[0x09], MODE_FM);
        assert_eq!(r[0x0A], 0);
        assert_eq!(r[0x0B], 0x1F, "DUP- with the low nibble the radio always sets");
        assert_eq!(&r[0x0C..0x1C], b"W0ABC           ");
        assert_eq!(r[0x1C], SQL_TONE);
        assert_eq!(r[0x1E], 12, "100.0 Hz is index 12");
        assert_eq!(r[0x20], 0, "no DTCS code stored means the radio's own 023");
        assert_eq!(&r[0x22..0x25], &ANALOG_TAIL);
        assert_eq!(&r[0x25..0x33], &[0xFF; 14], "an analog memory has no call signs");
    }

    /// A plus shift builds its transmit frequency from the offset, and a 6.25 kHz
    /// frequency has to declare its grid or the radio cannot dial off it.
    #[test]
    fn an_offset_grid_frequency_declares_its_raster() {
        let mut c = chan(1, "GMRS", 462.6625);
        c.duplex = Some("+".into());
        c.offset = Some(5.0);
        let ec = expand(c);

        let r = rec(&write(&[&ec], &[]), 0).to_vec();
        assert_eq!(r[0x0B] >> 4, 2, "DUP+");
        assert_eq!(u32::from_be_bytes(r[4..8].try_into().unwrap()), 5_000_000);
        // Receive is on the 6.25 kHz grid; transmit lands on 467.6625, also 6.25.
        assert_eq!(r[0x08], 0x11);
    }

    /// Airband AM is the one case with a raster and a tuning step of its own,
    /// and the two disagree on purpose: receive is 8.33 kHz, transmit is not.
    #[test]
    fn airband_am_gets_the_eight_thirty_three_grid() {
        let mut c = chan(1, "FNL CTAF", 118.400);
        c.mode = Some("AM".into());
        let ec = expand(c);

        let r = rec(&write(&[&ec], &[]), 0).to_vec();
        assert_eq!(r[0x09], MODE_AM);
        assert_eq!(r[0x0A], 2);
        assert_eq!(r[0x08], 0x20);
    }

    /// D-STAR replaces the whole tone block with a packed destination, and the
    /// two repeater fields stop being `FF`. The packing is 7 bits per character,
    /// so a round trip through the decoder is the only readable assertion.
    #[test]
    fn a_dstar_memory_packs_three_call_signs() {
        let mut c = chan(1, "W0ABC DV", 442.100);
        c.mode = Some("DSTAR".into());
        c.callsign = Some("W0ABC".into());
        c.tx_freq = Some(447.100);
        let ec = expand(c);

        let r = rec(&write(&[&ec], &[]), 0).to_vec();
        assert_eq!(r[0x09], MODE_DV);
        assert_eq!(unpack_call(&r[0x1E..0x25]), "CQCQCQ  ");
        assert_eq!(unpack_call(&r[0x25..0x2C]), "W0ABC  B");
        assert_eq!(unpack_call(&r[0x2C..0x33]), "W0ABC  G");
    }

    /// Issue #41 in the BYTES, not just in the CSV. The `.icf` is the file that
    /// actually programs this radio, and it reaches the same `call_signs` by a
    /// different route — so the override is asserted at both ends rather than
    /// inferred from the two paths sharing a function.
    #[test]
    fn a_channels_own_call_signs_reach_the_icf_record() {
        let mut c = chan(1, "REFLECTOR", 442.100);
        c.mode = Some("DSTAR".into());
        c.callsign = Some("W0ABC".into());
        c.tx_freq = Some(447.100);
        c.dstar_ur_call = Some("REF030CL".into());
        c.dstar_rpt1 = Some("w0abc a".into());
        let ec = expand(c);

        let r = rec(&write(&[&ec], &[]), 0).to_vec();
        assert_eq!(unpack_call(&r[0x1E..0x25]), "REF030CL");
        assert_eq!(
            unpack_call(&r[0x25..0x2C]),
            "W0ABC  A",
            "the stored module lost to the band rule"
        );
        // Not given, so still derived — and from the channel's own callsign
        // rather than from the overridden RPT1.
        assert_eq!(unpack_call(&r[0x2C..0x33]), "W0ABC  G");
    }

    /// The radio calls it Split Tone, and the two frequencies are not
    /// interchangeable: the repeater tone is transmitted, the TSQL tone opens
    /// squelch. Measured from a memory set to send 103.5 and receive 107.2.
    #[test]
    fn a_cross_tone_memory_keeps_transmit_and_receive_apart() {
        let mut c = chan(1, "PROBE CROSS", 145.115);
        c.tone_mode = Some("Cross".into());
        c.ctcss_uplink = Some(103.5);
        c.ctcss_downlink = Some(107.2);
        let ec = expand(c);

        let r = rec(&write(&[&ec], &[]), 0).to_vec();
        assert_eq!(r[0x1C], SQL_TONE_TSQL);
        assert_eq!(r[0x1E], 13, "103.5 Hz transmitted");
        assert_eq!(r[0x1F], 14, "107.2 Hz for squelch");
    }

    /// DTCS polarity lives at `0x1D`, which spent a while looking like a spare
    /// byte. `0x21` is the one that is genuinely always zero.
    #[test]
    fn dtcs_polarity_lands_in_the_byte_that_actually_carries_it() {
        let mut c = chan(1, "PROBE POL", 146.850);
        c.tone_mode = Some("DTCS".into());
        c.dcs_code = Some("023".into());
        c.dcs_polarity = "NR".into();
        let ec = expand(c);

        let r = rec(&write(&[&ec], &[]), 0).to_vec();
        assert_eq!(r[0x1C], SQL_DTCS);
        assert_eq!(r[0x1D], 1, "TN-RR");
        assert_eq!(r[0x21], 0, "not the polarity byte, whatever it is");
        assert_eq!(dtcs_polarity_index("BOTH N"), 0);
    }

    /// A DV memory must not carry an analog polarity into the record: `0x1D`
    /// sits right beside the packed destination call sign.
    #[test]
    fn a_dv_memory_leaves_the_polarity_byte_clear() {
        let mut c = chan(1, "DV", 442.100);
        c.mode = Some("DSTAR".into());
        c.dcs_polarity = "RR".into();
        let ec = expand(c);

        assert_eq!(rec(&write(&[&ec], &[]), 0)[0x1D], 0);
    }

    /// Decode Icom's 7-bit packing, so the encoder is checked against something
    /// other than itself.
    fn unpack_call(packed: &[u8]) -> String {
        let mut bits: u64 = 0;
        for b in packed {
            bits = (bits << 8) | u64::from(*b);
        }
        (0..8)
            .map(|i| char::from((bits >> (49 - 7 * i)) as u8 & 0x7F))
            .collect()
    }

    /// Tone and DTCS indices are shared with the settings menu, so three of them
    /// are self-checking against the capture checklist's extremes.
    #[test]
    fn tone_and_dtcs_tables_match_the_radios_indices() {
        assert_eq!(tone_index(67.0), 0);
        assert_eq!(tone_index(254.1), 49);
        assert_eq!(tone_index(88.5), 8);
        assert_eq!(dtcs_index(Some("023")), 0);
        assert_eq!(dtcs_index(Some("754")), 103);
        assert_eq!(dtcs_index(None), 0);
        // A tone this radio does not have becomes the nearest one it does.
        assert_eq!(tone_index(100.1), 12);
    }

    /// Channel lists become groups, in codeplug order, with channel numbers
    /// restarting inside each — and the chains and channel-number maps have to
    /// agree with each other, which is what the radio actually reads.
    #[test]
    fn lists_become_groups_with_their_own_chains_and_numbering() {
        let a = expand(chan(1, "A1", 146.520));
        let b = expand(chan(2, "A2", 146.540));
        let d = expand(chan(3, "B1", 446.000));
        let channels = vec![&a, &b, &d];
        let groups = vec![
            CodeplugGroup {
                list_id: 1,
                list_name: "Two Metres".into(),
                channels: vec![a.channel.clone(), b.channel.clone()],
            },
            CodeplugGroup {
                list_id: 2,
                list_name: "Seventy".into(),
                channels: vec![d.channel.clone()],
            },
        ];

        let img = write(&channels, &groups);

        assert_eq!(chain(&img, 0), vec![0, 1]);
        assert_eq!(chain(&img, 1), vec![2]);
        assert_eq!(positions(&img, 0), vec![0, 1]);
        assert_eq!(positions(&img, 1), vec![0]);
        assert_eq!(&img[GROUP_TABLE + 3..GROUP_TABLE + 19], b"Two Metres      ");
        assert_eq!(head(&img, 2), NO_SLOT, "an unused group holds no memories");
        assert_invariants(&img, 3);
    }

    /// The bookkeeping that decides whether the radio shows anything at all:
    /// every used slot clear in all three bitmaps, every unused slot set.
    #[test]
    fn flag_bitmaps_mark_exactly_the_unused_slots() {
        let a = expand(chan(1, "One", 146.520));
        let b = expand(chan(2, "Two", 146.540));
        let img = write(&[&a, &b], &[]);

        for base in [SKIP_BITMAP, PSKIP_BITMAP, UNKNOWN_BITMAP] {
            assert!(!bit(&img, base, 0));
            assert!(!bit(&img, base, 1));
            assert!(bit(&img, base, 2), "slot 2 is unused");
            assert!(bit(&img, base, SLOTS - 1), "the last slot is unused");
        }
    }

    /// An empty slot is all-`FF`, and the free chain runs from the first one to
    /// the end. Programming a smaller codeplug over a larger one must leave no
    /// trace of the memories it replaced.
    #[test]
    fn unused_slots_are_erased_and_chained_as_free() {
        let a = expand(chan(1, "Only", 146.520));
        let mut img = blank();
        write_memories(&mut img, &[&a], &[], &model()).unwrap();

        assert_eq!(rec(&img, 1), &[0xFF; REC_LEN]);
        assert_eq!(rec(&img, SLOTS - 1), &[0xFF; REC_LEN]);
        assert_eq!(next(&img, 1), 2, "the free chain starts at the first spare slot");
        assert_eq!(next(&img, SLOTS - 1), NO_SLOT, "and ends at the last");
        assert_invariants(&img, 1);
    }

    /// A full pool is the case where an off-by-one in the free chain or the
    /// channel-number map would actually bite.
    #[test]
    fn a_full_pool_leaves_no_free_chain_and_still_balances() {
        let all: Vec<ExpandedChannel> = (0..MAX_MEMORIES as i64)
            .map(|i| expand(chan(i, "x", 146.0 + i as f64 / 1000.0)))
            .collect();
        let refs: Vec<&ExpandedChannel> = all.iter().collect();
        // 1000 memories cannot fit one group of 100, so give them 10 lists.
        let groups: Vec<CodeplugGroup> = (0..10)
            .map(|g| CodeplugGroup {
                list_id: g,
                list_name: format!("List {g}"),
                channels: all[g as usize * 100..(g as usize + 1) * 100]
                    .iter()
                    .map(|ec| ec.channel.clone())
                    .collect(),
            })
            .collect();

        let img = write(&refs, &groups);
        assert_invariants(&img, MAX_MEMORIES);
        assert_eq!(positions(&img, 0).len(), 100);
        assert_eq!(
            &img[POSMAP + 12..POSMAP + 13],
            &[0xF0],
            "the four bits above channel 99 stay set, as the radio leaves them"
        );
    }

    /// Over capacity is a problem the operator can fix in the app. Finding out
    /// from the radio, after copying a card, is not.
    #[test]
    fn too_many_channels_is_refused_by_name() {
        let all: Vec<ExpandedChannel> = (0..=MAX_MEMORIES as i64)
            .map(|i| expand(chan(i, "x", 146.0 + i as f64 / 1000.0)))
            .collect();
        let refs: Vec<&ExpandedChannel> = all.iter().collect();
        let mut img = blank();
        let err = write_memories(&mut img, &refs, &[], &model()).unwrap_err();
        assert!(err.contains("1000"), "{err}");
    }

    /// `#MapRev` turns out to be an author fingerprint, not just a firmware
    /// marker: every file the radio saved declares **3**, and both files written
    /// by RT Systems declare **1** despite laying their contents out identically.
    ///
    /// So the guard put here for the Save Form case earns its keep for a second
    /// reason, and the message has to name both — a third-party file is by far
    /// the likelier cause, and sending the operator to Save Form for it would be
    /// a dead end.
    ///
    /// `#[ignore]`d because it needs the captures under `scratchpad/`.
    #[test]
    #[ignore = "needs real captures under scratchpad/id52/"]
    fn a_file_the_radio_did_not_write_is_refused_with_the_right_advice() {
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../scratchpad/id52/");
        let text = std::fs::read_to_string(format!("{dir}id52_05_rtsprobe.icf"))
            .unwrap_or_else(|e| {
                panic!("this test needs id52_05_rtsprobe.icf in {dir} ({e}). It is #[ignore]d \
                        because scratchpad/ is gitignored — running it without the file must \
                        FAIL, not quietly pass having asserted nothing. (#89)")
            });
        let mut icf = IcfFile::parse(&text).expect("a third-party file still parses");
        assert_eq!(icf.map_rev(), Some(1), "RT Systems declares revision 1");

        let a = expand(chan(1, "Nope", 146.520));
        let err = write_codeplug(&mut icf, &[&a], &[], &model()).unwrap_err();
        assert!(err.contains("Save Setting"), "{err}");
        assert!(err.contains("Save Form"), "{err}");
        eprintln!("ok: refused revision 1 with both remedies named");
    }

    /// An image of the wrong size is a file from some other radio, or a truncated
    /// one. Either way it must not be written to at measured offsets.
    #[test]
    fn a_wrong_sized_image_is_refused() {
        let mut img = vec![0u8; 1024];
        let err = write_memories(&mut img, &[], &[], &model()).unwrap_err();
        assert!(err.contains("ID-52 settings image"), "{err}");
    }

    /// The safety argument for patching an operator's own file: **only** the
     /// Byte indices the memory writer changed that it does NOT claim.
    ///
    /// The claim is the memory pool, the three bitmaps and the group/position
    /// tables. Everything else in the file — every MENU setting, the repeater
    /// list, the call channels, the 100 records at slots 1010-1109 that nothing
    /// has identified — is the operator's and must come back byte for byte.
    fn strays_outside_the_memory_pool(before: &[u8], after: &[u8]) -> Vec<usize> {
        let allowed = |i: usize| {
            (POOL..POOL + SLOTS * REC_LEN).contains(&i)
                || (SKIP_BITMAP..SKIP_BITMAP + BITMAP_LEN).contains(&i)
                || (PSKIP_BITMAP..PSKIP_BITMAP + BITMAP_LEN).contains(&i)
                || (UNKNOWN_BITMAP..UNKNOWN_BITMAP + BITMAP_LEN).contains(&i)
                || (GROUP_TABLE..POSMAP + GROUPS * POSMAP_LEN).contains(&i)
        };
        (0..before.len())
            .filter(|&i| before[i] != after[i] && !allowed(i))
            .collect()
    }

    /// ★ The patch-don't-generate bargain, runnable in CI on a synthetic card.
    ///
    /// Writing a codeplug must touch the memory pool and its tables and nothing
    /// else. The real-capture twin below stays `#[ignore]`d — it additionally
    /// proves this against a file the radio actually wrote, and re-reads the
    /// result with `memdecode.py`, neither of which a synthetic body can do.
    ///
    /// The fixture is [`patterned`], not [`blank`]: `blank` is one repeated
    /// `0x5A`, which is also what this writer leaves in an untouched record, so
    /// a stray write of it would be invisible. (#88)
    #[test]
    fn only_the_memory_pool_changes_on_a_synthetic_card() {
        let text = synth_icf(&patterned());
        let mut icf = IcfFile::parse(&text).expect("a synthetic file parses");
        let before = icf.image().to_vec();

        let probes = probe_codeplug();
        let refs: Vec<&ExpandedChannel> = probes.iter().collect();
        let groups = vec![
            CodeplugGroup {
                list_id: 1,
                list_name: "PROBE A".into(),
                channels: probes[..4].iter().map(|e| e.channel.clone()).collect(),
            },
            CodeplugGroup {
                list_id: 2,
                list_name: "PROBE B".into(),
                channels: probes[4..].iter().map(|e| e.channel.clone()).collect(),
            },
        ];
        write_codeplug(&mut icf, &refs, &groups, &model()).expect("patch");

        let after = icf.image().to_vec();
        let strays = strays_outside_the_memory_pool(&before, &after);
        assert!(
            strays.is_empty(),
            "{} bytes changed outside the memory pool, first at 0x{:06X}",
            strays.len(),
            strays.first().copied().unwrap_or(0)
        );

        assert_invariants(&after, probes.len());
        assert_eq!(chain(&after, 0), vec![0, 1, 2, 3]);
        assert_eq!(chain(&after, 1), vec![4, 5, 6]);

        // The patched file must still be a file: the checksum has to follow the
        // edit, or the radio rejects it at the door.
        let reparsed = IcfFile::parse(&icf.render()).expect("a patched file must still verify");
        assert_eq!(reparsed.image(), after);

        // The guard has to be able to fail: one byte written past the last
        // position-map entry is exactly the stride bug it exists for.
        let mut sabotaged = after.clone();
        let past_end = POSMAP + GROUPS * POSMAP_LEN;
        sabotaged[past_end] ^= 0xFF;
        assert_eq!(
            strays_outside_the_memory_pool(&before, &sabotaged),
            vec![past_end],
            "the guard did not notice a write past the last position-map entry"
        );
    }

   /// memory pool and its tables change. Everything else — every MENU setting,
    /// the repeater list, the call channels, the 100 records at slots 1010-1109
    /// that nothing has identified — comes back byte for byte.
    ///
    /// `#[ignore]`d because it needs a real capture under `scratchpad/` , which is
    /// gitignored and cannot exist in CI:
    ///
    ///     cargo test --lib icom -- --ignored --nocapture
    #[test]
    #[ignore = "needs a real .icf under scratchpad/id52/"]
    fn only_the_memory_pool_changes() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../scratchpad/id52/id52_02_probe.icf"
        );
        let text = std::fs::read_to_string(path).unwrap_or_else(|e| {
            panic!("this test needs the capture at {path} ({e}). It is #[ignore]d because \
                    scratchpad/ is gitignored — running it without the file must FAIL, not \
                    quietly pass having asserted nothing. (#89)")
        });
        let mut icf = IcfFile::parse(&text).expect("a real ID-52 file parses");
        let before = icf.image().to_vec();

        let probes = probe_codeplug();
        let refs: Vec<&ExpandedChannel> = probes.iter().collect();
        let groups = vec![
            CodeplugGroup {
                list_id: 1,
                list_name: "PROBE A".into(),
                channels: probes[..4].iter().map(|e| e.channel.clone()).collect(),
            },
            CodeplugGroup {
                list_id: 2,
                list_name: "PROBE B".into(),
                channels: probes[4..].iter().map(|e| e.channel.clone()).collect(),
            },
        ];
        write_codeplug(&mut icf, &refs, &groups, &model()).expect("patch");

        let after = icf.image();
        let strays = strays_outside_the_memory_pool(&before, after);
        assert!(
            strays.is_empty(),
            "wrote outside the memory pool at {}",
            strays
                .iter()
                .map(|i| format!("0x{i:06X}"))
                .collect::<Vec<_>>()
                .join(", ")
        );

        assert_invariants(after, probes.len());
        assert_eq!(chain(after, 0), vec![0, 1, 2, 3]);
        assert_eq!(chain(after, 1), vec![4, 5, 6]);

        // And the patched file must still be a file: the checksum has to follow
        // the edit, or the radio rejects it at the door.
        let rendered = icf.render();
        let reparsed = IcfFile::parse(&rendered).expect("a patched file must still verify");
        assert_eq!(reparsed.image(), after);

        // Leave the candidate on disk. `scratchpad/id52/memdecode.py` — written
        // from the raw bytes and checked against the radio's own CSV export —
        // reads it back, which is the only check on this encoder that does not
        // come from the same head that wrote it.
        let out = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../scratchpad/id52/id52_out_patched.icf"
        );
        std::fs::write(out, &rendered).expect("write the candidate");
        eprintln!(
            "ok: {} bytes changed, all inside the memory pool; wrote {out}",
            (0..before.len()).filter(|&i| before[i] != after[i]).count()
        );
    }

    /// The ground truth: re-encode the radio's **own 80 memories** from its own
    /// CSV export and diff the result against the bytes the radio itself wrote.
    ///
    /// Every other test here checks this encoder against a layout that came out
    /// of the same analysis that produced the encoder. This one checks it against
    /// the radio. The name field is skipped — [`expanded_names`] deduplicates, so
    /// two memories the operator gave the same name to legitimately come out
    /// different — and the SKIP bitmap is skipped because the channel database
    /// has nowhere to store a per-channel skip yet.
    ///
    ///     cargo test --lib icom -- --ignored --nocapture
    #[test]
    #[ignore = "needs a real .icf and its paired .csv under scratchpad/id52/"]
    fn the_radios_own_memories_re_encode_to_the_radios_own_bytes() {
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../scratchpad/id52/");
        let need = |name: &str| {
            std::fs::read_to_string(format!("{dir}{name}")).unwrap_or_else(|e| {
                panic!("this test needs {name} in {dir} ({e}). It is #[ignore]d because \
                        scratchpad/ is gitignored — running it without the file must FAIL, not \
                        quietly pass having asserted nothing. (#89)")
            })
        };
        let (text, csv_text) = (need("id52_02_probe.icf"), need("id52_01_base.csv"));
        let truth = IcfFile::parse(&text).expect("parse").image().to_vec();

        let channels: Vec<ExpandedChannel> = csv::Reader::from_reader(csv_text.as_bytes())
            .deserialize::<std::collections::HashMap<String, String>>()
            .map(|r| expand(from_csv_row(&r.expect("row"))))
            .collect();
        let refs: Vec<&ExpandedChannel> = channels.iter().collect();
        let mut img = blank();
        write_memories(&mut img, &refs, &[], &model()).expect("write");

        // A memory carries bytes the radio does not read: a TSQL memory keeps a
        // Repeater Tone it never sends, a DTCS memory keeps both tone indices, a
        // simplex memory keeps an offset. The radio leaves whatever was last
        // there; this writer normalises them. Those differences are real but
        // inaudible, so they are counted and printed rather than asserted on —
        // and every byte the radio *does* act on has to match exactly.
        let live = |r: &[u8], range: &std::ops::Range<usize>| -> bool {
            let (sql, duplex, dv) = (r[0x1C], r[0x0B] >> 4, r[0x09] == MODE_DV);
            match range.start {
                0x04 => duplex != 0,
                0x1D => dv || sql == SQL_DTCS,
                0x1E => dv || sql == SQL_TONE || sql == SQL_TONE_TSQL,
                0x1F => dv || sql == SQL_TSQL || sql == SQL_TONE_TSQL,
                0x20 => dv || sql == SQL_DTCS,
                _ => true,
            }
        };
        let fields: [(&str, std::ops::Range<usize>); 11] = [
            ("frequency", 0x00..0x04),
            ("offset", 0x04..0x08),
            ("raster", 0x08..0x09),
            ("mode/step/duplex", 0x09..0x0C),
            ("squelch mode", 0x1C..0x1D),
            ("DTCS polarity", 0x1D..0x1E),
            ("repeater tone", 0x1E..0x1F),
            ("TSQL tone", 0x1F..0x20),
            ("DTCS code", 0x20..0x22),
            ("analog tail", 0x22..0x25),
            ("call signs", 0x25..0x33),
        ];
        // The radio's memories are not contiguous — its pool had a hole at slot
        // 4, left by a deletion — so the CSV's Nth row is the Nth *occupied*
        // slot, not slot N. Ours are written contiguously, which is the whole
        // point: a full program compacts the pool.
        let occupied: Vec<usize> = (0..SLOTS)
            .filter(|&s| truth[s * REC_LEN..(s + 1) * REC_LEN] != [0xFF; REC_LEN])
            .collect();
        assert_eq!(
            occupied.len(),
            channels.len(),
            "the CSV and the .icf must be the same capture"
        );

        let (mut failures, mut inert): (Vec<String>, Vec<String>) = (Vec::new(), Vec::new());
        for (slot, &theirs_at) in occupied.iter().enumerate() {
            let (ours, theirs) = (
                rec(&img, slot),
                &truth[theirs_at * REC_LEN..(theirs_at + 1) * REC_LEN],
            );
            for (label, range) in &fields {
                if ours[range.clone()] == theirs[range.clone()] {
                    continue;
                }
                let note = format!(
                    "  slot {slot:3} {label:<17} ours {:<16} radio {}",
                    hex(&ours[range.clone()]),
                    hex(&theirs[range.clone()])
                );
                if live(theirs, range) {
                    failures.push(note);
                } else {
                    inert.push(note);
                }
            }
        }
        eprintln!(
            "re-encoded {} of the radio's own memories: {} live-byte mismatches, \
             {} in bytes the radio does not read",
            channels.len(),
            failures.len(),
            inert.len()
        );
        for f in failures.iter().chain(&inert) {
            eprintln!("{f}");
        }
        assert!(
            failures.is_empty(),
            "{} bytes the radio acts on differ from what it wrote itself",
            failures.len()
        );

        // Emit the round-trip candidate: the operator's own memories, written
        // back into their own settings file. Every frequency, tone, mode and
        // call sign survives byte-identically (that is what this test just
        // asserted), so loading it exercises all six structures against a real
        // radio with a known-correct answer on the screen.
        //
        // It is not a no-op, and the differences are worth predicting:
        //
        // - The pool compacts, closing the hole at slot 4 left by a deletion.
        // - Group 00 gains the name `Memories`, because a codeplug with no
        //   channel lists becomes one default group. That is the only check
        //   anything has on the group-name field, whose position is inferred
        //   from the entry stride rather than seen holding content.
        // - Duplicate names get disambiguated — this operator has seven
        //   memories called `W0UPS` — because `expanded_names` makes them
        //   distinct, the same way it does for every other radio.
        // - **The 58 SKIP flags are lost.** The channel database has nowhere to
        //   store a per-channel scan skip, so this writer clears them all. That
        //   is a genuine feature gap rather than an artefact of the test, and
        //   the CSV path has it too.
        let mut round_trip = IcfFile::parse(&text).expect("parse");
        write_codeplug(&mut round_trip, &refs, &[], &model()).expect("patch");
        std::fs::write(format!("{dir}id52_out_roundtrip.icf"), round_trip.render())
            .expect("write the round-trip candidate");
        eprintln!("wrote {dir}id52_out_roundtrip.icf — the radio's own memories, re-written");
    }

    fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|b| format!("{b:02X}")).collect()
    }

    /// Rebuild a channel from one row of the radio's Memory CH export. Only used
    /// by the ground-truth test — it is the inverse of [`super::memory_csv`], and
    /// exists so the comparison starts from the radio's description of a memory
    /// rather than from ours.
    fn from_csv_row(r: &std::collections::HashMap<String, String>) -> Channel {
        let get = |k: &str| r.get(k).map(String::as_str).unwrap_or("").trim().to_string();
        let hz = |k: &str| {
            let v = get(k);
            v.trim_end_matches("Hz").parse::<f64>().ok()
        };
        let rx: f64 = get("Frequency").parse().unwrap_or(0.0);
        let offset: f64 = get("Offset").parse().unwrap_or(0.0);
        let tone = get("TONE");

        let mut c = chan(0, &get("Name"), rx);
        c.mode = Some(
            match get("Mode").as_str() {
                "FM-N" => "NFM",
                "DV" => "DSTAR",
                "AM" => "AM",
                other => return_fm(other),
            }
            .into(),
        );
        match get("Dup").as_str() {
            "DUP-" => {
                c.duplex = Some("-".into());
                c.offset = Some(offset);
            }
            "DUP+" => {
                c.duplex = Some("+".into());
                c.offset = Some(offset);
            }
            _ => {}
        }
        c.tone_mode = match tone.as_str() {
            "TONE" => Some("Tone".into()),
            "TSQL" => Some("TSQL".into()),
            "DTCS" => Some("DTCS".into()),
            "" | "OFF" => None,
            _ => Some("Cross".into()),
        };
        c.ctcss_uplink = hz("Repeater Tone");
        c.ctcss_downlink = hz("TSQL Frequency");
        let dtcs = get("DTCS Code");
        c.dcs_code = (!dtcs.is_empty()).then_some(dtcs);
        // The radio's own three call signs, carried across as themselves
        // (issue #41). This used to strip the module letter off RPT1 to
        // reconstruct `callsign` and let the exporter derive it back, which was
        // only ever exact because the radio follows the band convention — a
        // memory that did not would have compared equal to a guess.
        c.dstar_ur_call = Some(get("Your Call Sign")).filter(|s| !s.is_empty());
        c.dstar_rpt1 = Some(get("RPT1 Call Sign")).filter(|s| !s.is_empty());
        c.dstar_rpt2 = Some(get("RPT2 Call Sign")).filter(|s| !s.is_empty());
        c.callsign = c
            .dstar_rpt1
            .as_deref()
            .map(|r| r.trim_end().trim_end_matches(['A', 'B', 'C']).trim_end().to_string());
        c
    }

    fn return_fm(_other: &str) -> &'static str {
        "FM"
    }

    /// Seven memories covering every branch of [`encode`]: each tone family, both
    /// shift directions, the two frequency grids, airband AM and D-STAR.
    fn probe_codeplug() -> Vec<ExpandedChannel> {
        let mut simplex = chan(1, "PROBE SPX", 146.520);
        simplex.mode = Some("FM".into());

        let mut minus = chan(2, "PROBE MINUS", 146.940);
        minus.tx_freq = Some(146.340);
        minus.tone_mode = Some("Tone".into());
        minus.ctcss_uplink = Some(100.0);

        let mut tsql = chan(3, "PROBE TSQL", 449.850);
        tsql.tx_freq = Some(444.850);
        tsql.tone_mode = Some("TSQL".into());
        tsql.ctcss_downlink = Some(123.0);

        let mut dtcs = chan(4, "PROBE DTCS", 145.310);
        dtcs.duplex = Some("+".into());
        dtcs.offset = Some(0.6);
        dtcs.tone_mode = Some("DTCS".into());
        dtcs.dcs_code = Some("023".into());
        dtcs.mode = Some("NFM".into());

        let mut gmrs = chan(5, "PROBE 6K25", 462.6625);
        gmrs.duplex = Some("+".into());
        gmrs.offset = Some(5.0);

        let mut am = chan(6, "PROBE AM", 118.400);
        am.mode = Some("AM".into());

        let mut dv = chan(7, "PROBE DV", 442.100);
        dv.mode = Some("DSTAR".into());
        dv.callsign = Some("W0ABC".into());
        dv.tx_freq = Some(447.100);

        [simplex, minus, tsql, dtcs, gmrs, am, dv]
            .into_iter()
            .map(expand)
            .collect()
    }
}
