//! FT5D microSD codeplug writer: patch memories, flags and banks into the
//! radio's own `FT5D/BACKUP/BACKUP.dat`.
//!
//! ## Why patch instead of generate
//!
//! `BACKUP.dat` is 130496 bytes and we understand maybe 40 KB of it. The rest is
//! APRS beacons, GPS logs, DTMF strings, the opening message, WIRES-X settings —
//! everything the operator has configured on the radio itself. Generating a file
//! from scratch would mean silently reverting all of it, so this takes the
//! user's own backup as the template and overwrites exactly four regions:
//!
//! | region | address | what |
//! |---|---|---|
//! | `bank_info[24].name` | `0xEF4` | 16-char ASCII bank labels |
//! | `bank_members[24]` | `0x1540` | 100 × u16-BE per bank, value = channel − 1 |
//! | `flag[900]` | `0x2800` | one byte per slot: valid / used / skip / nosubvfo |
//! | `memory[900]` | `0x2D40` | 32-byte records |
//!
//! Every other byte is copied verbatim — except the scattered individual bytes
//! of any non-channel settings the codeplug's radio profile carries, which
//! [`super::settings`] applies on top. The workflow is the radio's own: back up
//! to the SD card, patch the file here, then Restore from the same card — no
//! cable, and no third-party software in the loop.
//!
//! ## Addresses are clone-image coordinates minus 10
//!
//! CHIRP's `ft1d.py` MEM_FORMAT addresses the *clone image*, which carries a
//! 10-byte ident header the SD file does not (`BACKUP.dat` is the clone image
//! minus that header and a trailing checksum byte). So CHIRP's `0x2D4A` for
//! `memory[]` is `0x2D40` here, `0x154A` is `0x1540`, `0x0EFE` is `0x0EF4`.
//! Every constant below is in *SD-file* coordinates.
//!
//! ## Provenance
//!
//! The layout was measured against a real SD backup paired with an RT Systems
//! export of the same codeplug (exact known plaintext), then cross-checked
//! field-for-field against CHIRP's `ft1d.py` struct: a decoder written from the
//! constants below reproduces all 53 CSV columns for all 200 channels with zero
//! mismatches. See `scratchpad/ft5d/FINDINGS.md` for the byte tables.
//!
//! ## Where this encoder differs from RT Systems, and why
//!
//! Re-encoding those same 200 real channels and diffing against the bytes the
//! radio actually holds (`scratchpad/ft5d/encode_check.py`) leaves only these,
//! all understood and all inert:
//!
//! - **`0x1B`/`0x1C`/`0x1D` on channels whose tone mode does not use them.** RT
//!   leaves a stale CTCSS/DCS/user-tone index sitting in the record; we write 0.
//!   The tone mode nibble is what the radio reads, so both behave identically.
//! - **The offset field on simplex channels.** RT keeps the band's default
//!   shift; we write 0. `duplex = 0` means the radio never looks at it.
//! - **`0x01`'s tune_step on 12 channels** where RT chose 25 or 12.5 kHz. We
//!   always store 5 kHz, which only matters when dialling off a memory, and
//!   every one of those channels is in a band this model does not program.
//! - **The `skip` flag bit** — the channel database has no per-channel skip, so
//!   the 3 skipped channels come back unskipped.
//! - **Split (`duplex = 3`) is never emitted.** An odd split is encoded as a
//!   plus/minus shift with the real difference as the offset, which the radio
//!   treats the same. Only the 900 MHz pair in the dump used true Split.
//!
//! ## Receive-only channels
//!
//! The FT5D hears far more than it can transmit on, so GMRS, marine, NOAA,
//! airband and the rest arrive here as ordinary channels flagged receive-only
//! by the model's `rx_bands`/`tx_bands` (issue #32). There is nothing in the
//! `memslot` struct to mark them with — the radio enforces its own TX bands —
//! so they are encoded exactly like any other memory, TX frequency and all.
//! That is deliberate: a stock radio refuses to key up on them, and a radio
//! whose TX range has been opened up transmits on precisely the frequency the
//! channel says. The warning that a channel is listen-only belongs in the
//! program dialog, not in a byte the radio ignores.

use std::collections::HashMap;

use crate::commands::export::{expanded_names, tx_frequency, CodeplugGroup, ExpandedChannel};
use crate::error::MapErrString;
use crate::models::RadioModel;
use crate::radios::driver::{CodeplugExporter, ExportRequest};

use super::YaesuFt5d;

/// Exact size of `FT5D/BACKUP/BACKUP.dat`. The FT3D clone image is 130507 =
/// this + a 10-byte ident header + 1 checksum byte.
const IMAGE_LEN: usize = 130496;

/// Model token, e.g. `AH82M` for the FT5DR. Only the family prefix is checked:
/// the FT2D's second firmware revision ships `AH60G` where the first ships
/// `AH60M`, so an FT5D variant may well differ in that last byte.
const MODEL_OFF: usize = 0x220;
const MODEL_PREFIX: &[u8] = b"AH82";

/// `struct memslot memory[900]`, 32 bytes each.
const MEM_BASE: usize = 0x2D40;
const MEM_STRIDE: usize = 32;
/// 900 regular memories. The 99 skip-search slots and 100 PMS pairs that follow
/// in the image are deliberately left alone — they are separate arrays with
/// their own flags, not extra channel space.
const MEM_SLOTS: usize = 900;

/// `struct flagslot flag[900]`, one byte per memory. **This is what makes a
/// channel exist**: a slot whose record holds a perfectly good frequency but
/// whose flag says unused simply is not there. The real dump had exactly one
/// such stale record, which is why the writer clears flags and records
/// together rather than trusting `0xFF` fill.
const FLAG_BASE: usize = 0x2800;

/// `struct { u16 channel[100]; } bank_members[24]` — a membership *list*, not a
/// bitmap: each entry is a big-endian channel number − 1, terminated by
/// `0xFFFF`.
const BANK_BASE: usize = 0x1540;
const BANK_STRIDE: usize = 200;
const BANK_SLOTS: usize = 100;
const BANK_COUNT: usize = 24;

/// `struct { u8 unknown[2]; u8 name[16]; } bank_info[24]`. Names are plain
/// ASCII (the FT1D's packed charset does not apply from the FT2D on),
/// `0xFF`-padded.
const BANK_INFO_BASE: usize = 0xEF4;
const BANK_INFO_STRIDE: usize = 18;
const BANK_NAME_OFF: usize = 2;
const NAME_LEN: usize = 16;

/// Standard 50-entry CTCSS table; the record stores an index into it.
const TONES: [f64; 50] = [
    67.0, 69.3, 71.9, 74.4, 77.0, 79.7, 82.5, 85.4, 88.5, 91.5, 94.8, 97.4, 100.0, 103.5, 107.2,
    110.9, 114.8, 118.8, 123.0, 127.3, 131.8, 136.5, 141.3, 146.2, 151.4, 156.7, 159.8, 162.2,
    165.5, 167.9, 171.3, 173.8, 177.3, 179.9, 183.5, 186.2, 189.9, 192.8, 196.6, 199.5, 203.5,
    206.5, 210.7, 218.1, 225.7, 229.1, 233.6, 241.8, 250.3, 254.1,
];

/// `chirp_common.DTCS_CODES` — the standard 104-code DCS table, indexed by the
/// record's 7-bit `dcs` field. (Note this is the UV-5R's table *without* 645,
/// which that radio adds on its own.)
const DTCS: [u16; 104] = [
    23, 25, 26, 31, 32, 36, 43, 47, 51, 53, 54, 65, 71, 72, 73, 74, 114, 115, 116, 122, 125, 131,
    132, 134, 143, 145, 152, 155, 156, 162, 165, 172, 174, 205, 212, 223, 225, 226, 243, 244, 245,
    246, 251, 252, 255, 261, 263, 265, 266, 271, 274, 306, 311, 315, 325, 331, 332, 343, 346, 351,
    356, 364, 365, 371, 411, 412, 413, 423, 431, 432, 445, 446, 452, 454, 455, 462, 464, 465, 466,
    503, 506, 516, 523, 526, 532, 546, 565, 606, 612, 624, 627, 631, 632, 654, 662, 664, 703, 712,
    723, 731, 732, 734, 743, 754,
];

/// Reject anything that is not an FT5D SD backup before we overwrite it.
pub(crate) fn validate_backup(image: &[u8]) -> Result<(), String> {
    if image.len() != IMAGE_LEN {
        return Err(format!(
            "This is not an FT5D backup: expected {IMAGE_LEN} bytes, got {}. \
             Select FT5D/BACKUP/BACKUP.dat from the radio's microSD card.",
            image.len()
        ));
    }
    // Reject an image whose checksum already disagrees with its contents. The
    // radio would refuse it and reset itself, and patching it would only bake
    // our channels into a file that was never going to load — better to say so
    // while the operator can still take a fresh backup.
    let want = image_sum(image);
    let got = stored_checksum(image);
    if want != got {
        return Err(format!(
            "This backup is corrupt: its checksum is {got:08X} but its contents \
             sum to {want:08X}. Take a fresh backup on the radio (SD CARD menu \
             → Backup → Memory → SD card) and pick that."
        ));
    }
    let token = &image[MODEL_OFF..MODEL_OFF + MODEL_PREFIX.len()];
    if token != MODEL_PREFIX {
        return Err(format!(
            "This backup is from a different radio (model token {}, expected {}). \
             The FT5D writer only patches FT5D backups.",
            String::from_utf8_lossy(token),
            String::from_utf8_lossy(MODEL_PREFIX)
        ));
    }
    Ok(())
}

/// Six BCD digits, big-endian — the frequency/offset encoding, in kHz.
fn bcd3(v: u32) -> [u8; 3] {
    let mut out = [0u8; 3];
    let mut v = v % 1_000_000;
    for b in out.iter_mut().rev() {
        *b = (((v / 10 % 10) as u8) << 4) | ((v % 10) as u8);
        v /= 100;
    }
    out
}

/// Hz → the stored kHz value. Yaesu truncates rather than rounds, so a
/// 12.5 kHz-spaced 145.0125 MHz stores 145012 and the radio infers the missing
/// 500 Hz on read (CHIRP's `fix_rounded_step`). Rounding here would put the
/// channel 500 Hz high.
fn khz(hz: i64) -> u32 {
    (hz / 1000).max(0) as u32
}

/// MHz as the database stores it → Hz, the integer domain everything else uses.
fn hz(mhz: f64) -> i64 {
    (mhz * 1_000_000.0).round() as i64
}

/// Nearest entry in the CTCSS table. An exact match is the norm; the fallback
/// keeps an oddball tone from silently becoming 67.0 Hz.
fn tone_index(hz: f64) -> u8 {
    let mut best = 0usize;
    for (i, t) in TONES.iter().enumerate() {
        if (t - hz).abs() < (TONES[best] - hz).abs() {
            best = i;
        }
    }
    best as u8
}

fn dcs_index(code: Option<&str>) -> Option<u8> {
    let n: u16 = code?.trim().parse().ok()?;
    DTCS.iter().position(|&c| c == n).map(|i| i as u8)
}

/// Byte `0x00`'s low nibble, which RT Systems fills with a band code. Measured:
/// 1 on the two airband channels, 2 across all 83 2 m channels, 3 across all
/// 104 70 cm ones, 7 on 220 MHz *and* 900 MHz alike. CHIRP leaves this field
/// alone entirely, so it is very likely cosmetic — but matching RT Systems
/// costs nothing and keeps our output byte-comparable with theirs.
fn band_code(mhz: f64) -> u8 {
    match mhz {
        f if (108.0..137.0).contains(&f) => 1,
        f if (137.0..174.0).contains(&f) => 2,
        f if (400.0..480.0).contains(&f) => 3,
        _ => 7,
    }
}

/// `flagslot`: `nosubvfo:1, unknown:3, pskip:1, skip:1, used:1, valid:1` from
/// the high bit down. `nosubvfo` masks the channel from VFO B and follows
/// CHIRP's rule exactly — the sub-band receiver cannot reach these ranges.
fn flag_byte(mhz: f64) -> u8 {
    let nosubvfo = mhz < 30.0 || (88.0..108.0).contains(&mhz) || mhz > 580.0;
    let mut f = 0b0000_0011; // valid + used
    if nosubvfo {
        f |= 0b1000_0000;
    }
    f
}

/// Encode one channel into its 32-byte `memslot`.
fn encode_record(ec: &ExpandedChannel, name: &str) -> [u8; MEM_STRIDE] {
    let c = &ec.channel;
    let mut r = [0u8; MEM_STRIDE];

    let rx_hz = hz(c.rx_freq);
    let tx_hz = hz(tx_frequency(c));
    let (duplex, offset_hz) = match tx_hz - rx_hz {
        0 => (0u8, 0i64),
        d if d > 0 => (2, d),
        d => (1, -d),
    };

    // 0x00: band code in the low nibble; clock_shift (bit 4) stays off.
    r[0] = band_code(c.rx_freq);

    // 0x01: mode:2 | duplex:2 | tune_step:4. Mode 1 is AM (the FT5D's own
    // airband setting); everything else we program is FM or C4FM, which both
    // ride on mode 0. tune_step is left at 5 kHz because autostep below tells
    // the radio to pick the step from the band anyway.
    let am = c.mode.as_deref().map(|m| m.eq_ignore_ascii_case("AM")) == Some(true);
    let mode_bits: u8 = if am { 1 } else { 0 };
    r[1] = (mode_bits << 6) | (duplex << 4);

    r[2..5].copy_from_slice(&bcd3(khz(rx_hz)));

    // 0x05: power:2 | digmode:2 | tone_mode:4.
    let power = match c.power.as_deref() {
        Some(p) if p.eq_ignore_ascii_case("Low") => 0,   // L1
        Some(p) if p.eq_ignore_ascii_case("Med") => 2,   // L3, 2.5 W
        Some(p) if p.eq_ignore_ascii_case("Medium") => 2,
        _ => 3,                                          // High, 5 W
    };
    // 0 Analog / 1 AMS / 2 DN / 3 VW. A YSF channel is programmed as DN
    // (the C4FM mode the FT5D uses for repeaters); analog stays 0.
    let digmode = if c.mode.as_deref().map(|m| m.eq_ignore_ascii_case("YSF")) == Some(true) {
        2u8
    } else {
        0
    };
    let (tone_mode, tone, dcs) = encode_tone(c);
    r[5] = (power << 6) | (digmode << 4) | tone_mode;

    // 0x06-0x07 charsetbits: zero for the plain-ASCII label the FT2D family
    // switched to. 0x08-0x17 label, 0xFF-padded.
    r[8..8 + NAME_LEN].copy_from_slice(&ascii_field(name));

    // 0x18-0x1A: the repeater offset, same BCD-kHz encoding as the frequency.
    r[0x18..0x1B].copy_from_slice(&bcd3(khz(offset_hz)));

    r[0x1B] = tone;
    r[0x1C] = dcs;
    // 0x1D user CTCSS (300 Hz + n*100); 0x1E unused.
    r[0x1D] = 0;

    // 0x1F: att:1 (bit 5) | autostep:1 (bit 4) | automode:1 (bit 3).
    //
    // The channel database has no tuning-step column, so autostep follows the
    // rule the real codeplug turned out to use: a repeater channel gets the
    // explicit 5 kHz step left in `0x01`, a simplex one gets Auto and lets the
    // radio pick per band. Measured, that reproduces 194 of 200 real channels;
    // a blanket Auto reproduced 22. automode is the AMS auto-detect: on for
    // analog, off where we have pinned DN — exactly the split the dump showed.
    let autostep = if duplex == 0 { 0b0001_0000 } else { 0 };
    r[0x1F] = autostep | if digmode == 0 { 0b0000_1000 } else { 0 };

    r
}

/// (tone_mode nibble, CTCSS index, DCS index) from the project's universal
/// tone scheme. The FT5D has no cross-tone mode, so per
/// `cross-tone-unsupported-fallback` a Cross channel keeps its TX tone and
/// drops the RX side.
fn encode_tone(c: &crate::models::Channel) -> (u8, u8, u8) {
    let mode = c.tone_mode.as_deref().unwrap_or("off");
    let ct = |t: Option<f64>| t.map(tone_index).unwrap_or(0);

    if mode.eq_ignore_ascii_case("Tone") {
        (1, ct(c.ctcss_uplink), 0)
    } else if mode.eq_ignore_ascii_case("TSQL") {
        (2, ct(c.ctcss_downlink), 0)
    } else if mode.eq_ignore_ascii_case("DTCS") {
        (3, 0, dcs_index(c.dcs_code.as_deref()).unwrap_or(0))
    } else if mode.eq_ignore_ascii_case("Cross") {
        let (txmode, _) = c.cross_mode.split_once("->").unwrap_or(("", ""));
        if txmode.eq_ignore_ascii_case("DTCS") {
            (3, 0, dcs_index(c.dcs_code.as_deref()).unwrap_or(0))
        } else if txmode.eq_ignore_ascii_case("Tone") {
            (1, ct(c.ctcss_uplink), 0)
        } else {
            (0, 0, 0)
        }
    } else {
        (0, 0, 0)
    }
}

/// Patch `template` with `channels` (memory slot order) and `groups` (banks).
///
/// Overflowing any of the radio's three limits is an error rather than a silent
/// truncation: the operator's codeplug does not fit, and dropping the tail
/// quietly would hand back a radio that looks programmed but is missing
/// channels they asked for.
pub(crate) fn patch_backup(
    template: &[u8],
    channels: &[&ExpandedChannel],
    groups: &[CodeplugGroup],
    model: &RadioModel,
    profile_settings: Option<&str>,
) -> Result<Vec<u8>, String> {
    validate_backup(template)?;
    if channels.len() > MEM_SLOTS {
        return Err(format!(
            "This codeplug has {} channels; the FT5D holds {MEM_SLOTS}. \
             Remove {} before exporting.",
            channels.len(),
            channels.len() - MEM_SLOTS
        ));
    }
    if groups.len() > BANK_COUNT {
        return Err(format!(
            "This codeplug has {} channel lists; the FT5D has {BANK_COUNT} banks. \
             Remove {} before exporting.",
            groups.len(),
            groups.len() - BANK_COUNT
        ));
    }

    let mut img = template.to_vec();

    // Wipe every regular memory and its flag first. Clearing the flag is the
    // part that matters — a leftover record with a live flag would show up as a
    // channel the user never programmed.
    for slot in 0..MEM_SLOTS {
        img[FLAG_BASE + slot] = 0;
        let at = MEM_BASE + slot * MEM_STRIDE;
        img[at..at + MEM_STRIDE].fill(0xFF);
    }

    let names = expanded_names(channels.iter().copied(), model);
    // Channel id -> 1-based memory slot, for turning channel lists into banks.
    let mut slot_of: HashMap<i64, usize> = HashMap::new();
    for (i, ec) in channels.iter().enumerate() {
        let at = MEM_BASE + i * MEM_STRIDE;
        img[at..at + MEM_STRIDE].copy_from_slice(&encode_record(ec, &names[i]));
        img[FLAG_BASE + i] = flag_byte(ec.channel.rx_freq);
        slot_of.entry(ec.channel.id).or_insert(i + 1);
    }

    // Banks: one channel list each, in codeplug assignment order.
    for b in 0..BANK_COUNT {
        let at = BANK_BASE + b * BANK_STRIDE;
        img[at..at + BANK_STRIDE].fill(0xFF);
    }
    for (b, g) in groups.iter().enumerate() {
        // A list can name a channel the codeplug excluded (wrong band for this
        // radio, say); those simply have no slot to point at.
        let members: Vec<usize> = g
            .channels
            .iter()
            .filter_map(|c| slot_of.get(&c.id).copied())
            .collect();
        if members.len() > BANK_SLOTS {
            return Err(format!(
                "Channel list \"{}\" has {} channels; an FT5D bank holds {BANK_SLOTS}.",
                g.list_name,
                members.len()
            ));
        }
        let at = BANK_BASE + b * BANK_STRIDE;
        for (i, slot) in members.iter().enumerate() {
            let entry = (slot - 1) as u16;
            img[at + i * 2] = (entry >> 8) as u8;
            img[at + i * 2 + 1] = entry as u8;
        }

        // Name the bank after its list; banks we do not fill go back to the
        // radio's factory "BANK n" so a stale label never outlives its contents.
        write_bank_name(&mut img, b, &g.list_name);
    }
    // Right-aligned to match the radio's own factory labels exactly: the dump
    // reads "BANK 1".."BANK 9" then "BANK10".."BANK24".
    for b in groups.len()..BANK_COUNT {
        write_bank_name(&mut img, b, &format!("BANK{:>2}", b + 1));
    }

    // Non-channel settings from the codeplug's radio profile. A codeplug with
    // no profile leaves every one of them exactly as the operator's backup had
    // it, which is the same promise the rest of the image gets.
    if let Some(json) = profile_settings {
        let parsed: serde_json::Value = serde_json::from_str(json)
            .map_err(|e| format!("This profile's settings are not valid JSON: {e}"))?;
        if let Some(map) = parsed.as_object() {
            super::settings::apply_settings(&mut img, map);
        }
    }

    write_checksum(&mut img);
    Ok(img)
}

/// The image checksum: a **32-bit** big-endian sum of every preceding byte,
/// occupying the last four bytes of the file.
///
/// ## Why this is not optional
///
/// The radio validates it. An image whose contents do not match the stored sum
/// is rejected and the radio **factory-resets itself** — it comes up at the
/// first-boot callsign prompt with no memories. Patching memories without
/// recomputing this is worse than not patching at all.
///
/// ## Why 32 bits, learned the hard way
///
/// It first looked like a 16-bit sum at `0x1FDBE` with `01 C3` in front as a
/// fixed trailer, because the two images available at the time both happened to
/// hold `01 C3` in the high half. Writing only those low 16 bits produced three
/// separate failures on real hardware — and the tell was that every rejected
/// image was wrong *only* above bit 16.
///
/// A one-name edit (5 bytes) was accepted, because so small a delta never
/// carried past bit 16 and the stale high half stayed accidentally right. Any
/// bulk edit failed, and wiping records to `0xFF` failed hardest: emptying a
/// slot *raises* the sum by ~8k, so clearing a few hundred moves it by millions.
///
/// What settled it was a third image — RT Systems writing the same codeplug with
/// most channels deleted (`scratchpad/ft5d/diag/06_rt_fewer.dat`). Its high half
/// read `01 CF`, not `01 C3`, proving those bytes vary with content. Across
/// seven images — four the radio accepted, three it rejected — the 32-bit
/// reading matches every acceptance and fails every rejection.
const CKSUM_AT: usize = 0x1FDBC;
const CKSUM_LEN: usize = 4;

fn image_sum(img: &[u8]) -> u32 {
    img[..CKSUM_AT].iter().fold(0u32, |acc, &b| acc + b as u32)
}

fn stored_checksum(img: &[u8]) -> u32 {
    u32::from_be_bytes([
        img[CKSUM_AT],
        img[CKSUM_AT + 1],
        img[CKSUM_AT + 2],
        img[CKSUM_AT + 3],
    ])
}

fn write_checksum(img: &mut [u8]) {
    let sum = image_sum(img).to_be_bytes();
    img[CKSUM_AT..CKSUM_AT + CKSUM_LEN].copy_from_slice(&sum);
}

/// 16 bytes of printable ASCII, `0xFF`-padded — the label encoding shared by
/// memory names and bank names.
fn ascii_field(name: &str) -> [u8; NAME_LEN] {
    let mut buf = [0xFFu8; NAME_LEN];
    for (i, ch) in name
        .chars()
        .filter(|c| c.is_ascii_graphic() || *c == ' ')
        .take(NAME_LEN)
        .enumerate()
    {
        buf[i] = ch as u8;
    }
    buf
}

fn write_bank_name(img: &mut [u8], bank: usize, name: &str) {
    let at = BANK_INFO_BASE + bank * BANK_INFO_STRIDE + BANK_NAME_OFF;
    img[at..at + NAME_LEN].copy_from_slice(&ascii_field(name));
}

impl CodeplugExporter for YaesuFt5d {
    fn export_format(&self) -> &'static str {
        "yaesu_ft5d_sd"
    }

    /// `path` is the operator's existing `FT5D/BACKUP/BACKUP.dat` — the file is
    /// read as the template and written back in place, because that exact path
    /// is what the radio's Restore menu reads. The untouched original is kept
    /// beside it as `BACKUP.dat.orig`, written once and never overwritten, so a
    /// second export cannot clobber the only pristine copy.
    fn export(&self, path: &str, req: &ExportRequest) -> Result<usize, String> {
        let template = std::fs::read(path).map_err(|e| {
            format!(
                "Could not read {path}: {e}. Pick FT5D/BACKUP/BACKUP.dat on the \
                 radio's microSD card — back the radio up first if it is not there."
            )
        })?;
        let image = patch_backup(
            &template,
            req.channels,
            req.groups,
            req.model,
            req.profile_settings,
        )?;

        let orig = format!("{path}.orig");
        if !std::path::Path::new(&orig).exists() {
            std::fs::write(&orig, &template)
                .map_err(|e| format!("Could not save the original as {orig}: {e}"))?;
        }
        std::fs::write(path, &image).estr()?;
        Ok(req.channels.len())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::Channel;

    /// A template that is structurally an FT5D backup: right size, right model
    /// token, and every region we patch pre-filled with a byte pattern we can
    /// assert got replaced (or preserved).
    fn template() -> Vec<u8> {
        let mut t = vec![0xAAu8; IMAGE_LEN];
        t[MODEL_OFF..MODEL_OFF + 5].copy_from_slice(b"AH82M");
        // A real backup always carries a checksum matching its contents, and
        // `validate_backup` now insists on it, so the fixture must too.
        write_checksum(&mut t);
        t
    }

    fn chan(id: i64, name: &str, rx: f64) -> Channel {
        Channel {
            id,
            name_short: Some(name.into()),
            rx_freq: rx,
            source: "test".into(),
            dcs_polarity: "NN".into(),
            cross_mode: "Tone->Tone".into(),
            ..Default::default()
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

    fn model() -> RadioModel {
        RadioModel {
            display_name: "Yaesu FT5D".into(),
            max_name_length: Some(16),
            ..Default::default()
        }
    }

    /// The whole premise of patching: everything outside the three regions we
    /// own survives byte-for-byte. If this ever fails, an operator's APRS and
    /// WIRES-X settings are being silently reverted.
    #[test]
    fn patch_preserves_every_byte_outside_the_patched_regions() {
        let t = template();
        let ec = expanded(chan(1, "W0UPS", 147.0));
        let img = patch_backup(&t, &[&ec], &[], &model(), None).expect("patch");

        assert_eq!(img.len(), t.len());
        for (i, (a, b)) in t.iter().zip(img.iter()).enumerate() {
            let patched = (BANK_INFO_BASE..BANK_INFO_BASE + BANK_COUNT * BANK_INFO_STRIDE)
                .contains(&i)
                || (BANK_BASE..BANK_BASE + BANK_COUNT * BANK_STRIDE).contains(&i)
                || (FLAG_BASE..FLAG_BASE + MEM_SLOTS).contains(&i)
                || (MEM_BASE..MEM_BASE + MEM_SLOTS * MEM_STRIDE).contains(&i)
                // The checksum describes the patched contents, so it is ours to
                // rewrite by definition — it is the one byte pair that MUST
                // change when anything else does.
                || (CKSUM_AT..CKSUM_AT + CKSUM_LEN).contains(&i);
            if !patched {
                assert_eq!(a, b, "byte 0x{i:X} outside the patched regions changed");
            }
        }
    }

    /// Frequency, name, duplex and offset round-trip through the record exactly
    /// as the decoder read them off the real radio.
    #[test]
    fn encodes_a_repeater_channel() {
        let mut c = chan(1, "W0UPS", 447.275);
        c.duplex = Some("-".into());
        c.offset = Some(5.0);
        c.tone_mode = Some("TSQL".into());
        c.ctcss_downlink = Some(100.0);
        let ec = expanded(c);
        let img = patch_backup(&template(), &[&ec], &[], &model(), None).expect("patch");

        let r = &img[MEM_BASE..MEM_BASE + MEM_STRIDE];
        assert_eq!(&r[2..5], &[0x44, 0x72, 0x75], "447.275 MHz as BCD kHz");
        assert_eq!((r[1] >> 4) & 3, 1, "minus duplex");
        assert_eq!(&r[0x18..0x1B], &[0x00, 0x50, 0x00], "5 MHz offset");
        assert_eq!(r[0] & 0x0F, 3, "70 cm band code");
        assert_eq!(r[5] & 0x0F, 2, "T Sql");
        assert_eq!(r[0x1B], 12, "100.0 Hz is index 12");
        assert_eq!(&r[8..13], b"W0UPS");
        assert_eq!(r[13], 0xFF, "name is 0xFF-padded");
        assert_eq!(img[FLAG_BASE], 0b0000_0011, "valid + used, sub-VFO allowed");
    }

    /// Issue #32: channels the FT5D can only listen on now reach the encoder,
    /// and they are ordinary memories — the radio polices its own TX bands, and
    /// a radio modified to transmit out of band needs the real TX frequency to
    /// be there. The frequencies themselves are the point: GMRS repeater pairs,
    /// marine, weather and 900 MHz all encode without wrapping the BCD field.
    #[test]
    fn receive_only_channels_encode_as_ordinary_memories() {
        let mut gmrs = chan(1, "GMRS 16", 462.5750);
        gmrs.duplex = Some("+".into());
        gmrs.offset = Some(5.0);
        let noaa = chan(2, "WX1", 162.5500);
        let nine = chan(3, "900", 927.0125);
        let ecs: Vec<_> = [gmrs, noaa, nine].into_iter().map(expanded).collect();
        let refs: Vec<_> = ecs.iter().collect();
        let img = patch_backup(&template(), &refs, &[], &model(), None).expect("patch");

        let r = &img[MEM_BASE..MEM_BASE + MEM_STRIDE];
        assert_eq!(&r[2..5], &[0x46, 0x25, 0x75], "462.575 MHz as BCD kHz");
        assert_eq!((r[1] >> 4) & 3, 2, "plus duplex — the 467 MHz input is kept");
        assert_eq!(&r[0x18..0x1B], &[0x00, 0x50, 0x00], "5 MHz offset");
        assert_eq!(img[FLAG_BASE], 0b0000_0011, "valid + used");

        let wx = &img[MEM_BASE + MEM_STRIDE..MEM_BASE + 2 * MEM_STRIDE];
        assert_eq!(&wx[2..5], &[0x16, 0x25, 0x50], "162.550 MHz");

        let nine_hundred = &img[MEM_BASE + 2 * MEM_STRIDE..MEM_BASE + 3 * MEM_STRIDE];
        assert_eq!(&nine_hundred[2..5], &[0x92, 0x70, 0x12], "927.0125 truncates");
        assert_eq!(
            img[FLAG_BASE + 2] & 0x80,
            0x80,
            "above 580 MHz the sub-band receiver cannot reach it"
        );
    }

    /// Yaesu truncates the frequency to whole kHz and the radio infers the
    /// missing 500 Hz. Rounding instead would program the channel 500 Hz high.
    #[test]
    fn twelve_and_a_half_khz_spacing_truncates_rather_than_rounds() {
        let ec = expanded(chan(1, "ODD", 145.0125));
        let img = patch_backup(&template(), &[&ec], &[], &model(), None).expect("patch");
        assert_eq!(&img[MEM_BASE + 2..MEM_BASE + 5], &[0x14, 0x50, 0x12]);
    }

    /// A 900 MHz channel is out of the sub-band receiver's reach, so it must be
    /// masked from VFO B or the radio shows it where it cannot tune.
    #[test]
    fn sets_nosubvfo_above_580_mhz() {
        let ec = expanded(chan(1, "NINE", 927.9));
        let img = patch_backup(&template(), &[&ec], &[], &model(), None).expect("patch");
        assert_eq!(img[FLAG_BASE] & 0x80, 0x80);
    }

    /// Channel lists become banks: membership is a list of (slot − 1) as u16-BE,
    /// terminated by 0xFFFF, and the bank takes the list's name.
    #[test]
    fn channel_lists_become_banks() {
        let a = expanded(chan(10, "A", 146.52));
        let b = expanded(chan(20, "B", 147.0));
        let group = CodeplugGroup {
            list_id: 1,
            list_name: "Denver".into(),
            channels: vec![b.channel.clone()],
        };
        let img =
            patch_backup(&template(), &[&a, &b], &[group], &model(), None).expect("patch");

        // Channel "B" landed in memory slot 2, so the bank entry is 1.
        assert_eq!(&img[BANK_BASE..BANK_BASE + 4], &[0x00, 0x01, 0xFF, 0xFF]);
        let name = &img[BANK_INFO_BASE + 2..BANK_INFO_BASE + 8];
        assert_eq!(name, b"Denver");
        // Bank 2 was not used, so it keeps the factory label and no members.
        assert_eq!(&img[BANK_BASE + BANK_STRIDE..BANK_BASE + BANK_STRIDE + 2], &[0xFF, 0xFF]);
    }

    /// The stale-record trap: a slot the new codeplug does not fill must come
    /// back with its flag cleared, or the radio keeps showing whatever the
    /// operator had there before.
    #[test]
    fn clears_flags_for_slots_the_codeplug_does_not_fill() {
        let mut t = template();
        t[FLAG_BASE + 5] = 0x03; // a channel from the operator's previous codeplug
        write_checksum(&mut t); // the radio's backup would agree with itself
        let ec = expanded(chan(1, "ONE", 146.52));
        let img = patch_backup(&t, &[&ec], &[], &model(), None).expect("patch");

        assert_eq!(img[FLAG_BASE], 0x03, "slot 1 is the channel we programmed");
        assert_eq!(img[FLAG_BASE + 5], 0x00, "slot 6 must no longer exist");
        let at = MEM_BASE + 5 * MEM_STRIDE;
        assert!(img[at..at + MEM_STRIDE].iter().all(|&b| b == 0xFF));
    }

    /// Patch a REAL `BACKUP.dat` and leave the result where a decoder can check
    /// it. Ignored by default and skipped when the file is absent: the input is
    /// a dump of a personal radio, so it lives in the gitignored scratchpad and
    /// can never be part of CI.
    ///
    /// ```text
    /// cargo test --lib patches_the_real_backup -- --ignored --nocapture
    /// python3 scratchpad/ft5d/verify_patched.py
    /// ```
    /// The checksum rule, checked against every image whose fate on real
    /// hardware we know. The rejected ones matter as much as the accepted ones:
    /// a 16-bit reading also matched all four acceptances, and only the
    /// rejections show it is 32 bits. Any rule that cannot tell these two
    /// groups apart is the bug we already shipped.
    #[test]
    #[ignore = "needs scratchpad/ft5d/diag/, dumps of a personal radio"]
    fn checksum_separates_images_the_radio_accepted_from_the_ones_it_reset_on() {
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../scratchpad/ft5d/diag");
        let cases = [
            ("01_radio_original.dat", true),
            ("04_rt_good.dat", true),
            ("06_rt_fewer.dat", true),
            ("T1_rt_plus_one_name.dat", true),
            ("05_ours_cksum.dat", false),
            ("H1_ours_memory_and_flags.dat", false),
            ("H4_wipe_only.dat", false),
        ];
        for (name, accepted) in cases {
            let img = std::fs::read(format!("{dir}/{name}")).expect(name);
            let agrees = stored_checksum(&img) == image_sum(&img);
            assert_eq!(
                agrees, accepted,
                "{name}: radio {} it, but checksum agreement says {agrees}",
                if accepted { "accepted" } else { "reset on" }
            );
            assert_eq!(validate_backup(&img).is_ok(), accepted, "{name}: validate");
            if accepted {
                // Our writer must reproduce it byte for byte, not merely agree.
                let mut ours = img.clone();
                write_checksum(&mut ours);
                assert_eq!(ours, img, "{name}: recomputing changed the image");
            }
        }
    }

    /// The specific trap: a change small enough to leave the low 16 bits of the
    /// sum looking right must still rewrite the high half.
    #[test]
    fn checksum_covers_the_high_half_not_just_the_low_word() {
        let mut a = template();
        // Push the sum past a 16-bit boundary the way emptying a slot does.
        a[MEM_BASE] = 0xFF;
        write_checksum(&mut a);
        assert_eq!(stored_checksum(&a), image_sum(&a));
        assert!(
            image_sum(&a) > 0xFFFF,
            "a real image sums well past 16 bits, so the high half carries meaning"
        );
    }

    /// A patched image must carry a checksum matching its own new contents —
    /// the failure that factory-reset a real FT5D was carrying the template's.
    #[test]
    fn patching_rewrites_the_checksum() {
        let t = template();
        let ec = expanded(chan(1, "W0UPS", 146.520));
        let img = patch_backup(&t, &[&ec], &[], &model(), None).expect("patch");
        let stored = stored_checksum(&img);
        let want = image_sum(&img);
        assert_eq!(stored, want, "checksum must describe the patched contents");
        assert_ne!(
            stored,
            stored_checksum(&t),
            "and must not still be the template's"
        );
    }

    #[test]
    #[ignore = "needs scratchpad/ft5d/ft5d_01_real.dat, a dump of a personal radio"]
    fn patches_the_real_backup() {
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../scratchpad/ft5d");
        let src = format!("{dir}/ft5d_01_real.dat");
        let Ok(template) = std::fs::read(&src) else {
            eprintln!("skipped: {src} not present");
            return;
        };

        let mut repeater = chan(1, "W0UPS", 447.275);
        repeater.duplex = Some("-".into());
        repeater.offset = Some(5.0);
        repeater.tone_mode = Some("TSQL".into());
        repeater.ctcss_downlink = Some(100.0);
        let mut simplex = chan(2, "146.520", 146.52);
        simplex.power = Some("Low".into());
        let mut fusion = chan(3, "PI.C4FM", 438.55);
        fusion.mode = Some("YSF".into());

        let rows = [expanded(repeater), expanded(simplex), expanded(fusion)];
        let refs: Vec<&ExpandedChannel> = rows.iter().collect();
        let group = CodeplugGroup {
            list_id: 1,
            list_name: "Front Range".into(),
            channels: rows.iter().map(|ec| ec.channel.clone()).collect(),
        };
        // A profile's worth of settings rides along, so the dump exercises the
        // settings writer against real neighbouring bytes rather than zeros.
        let settings = r#"{
            "squelch-vfo-a": 4,
            "mycall-callsign": "WW8L",
            "opening-message-flag": "Message",
            "opening-message-message-padded-yaesu": "CODEPLUG MAGIC",
            "aprs-aprs-units-temperature-f": "C"
        }"#;
        let img = patch_backup(&template, &refs, &[group], &model(), Some(settings))
            .expect("patch");

        assert_eq!(img.len(), template.len());
        std::fs::write(format!("{dir}/ft5d_patched.dat"), &img).expect("write");
        eprintln!("wrote {dir}/ft5d_patched.dat");
    }

    /// Refuse to patch anything that is not an FT5D backup — the file is
    /// overwritten in place, so a wrong guess costs the operator their data.
    #[test]
    fn rejects_files_that_are_not_ft5d_backups() {
        assert!(validate_backup(&[0u8; 128]).is_err(), "wrong size");
        let mut wrong = template();
        wrong[MODEL_OFF..MODEL_OFF + 5].copy_from_slice(b"AH72M"); // an FT3D
        assert!(validate_backup(&wrong).is_err(), "wrong model token");
        assert!(validate_backup(&template()).is_ok());
    }
}
