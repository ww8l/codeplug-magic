//! TH-D72 memories: the 16-byte channel record, its flag record, its name cell,
//! and the ten group names.
//!
//! Every offset here comes from [`super::layout`]; nothing in this file invents
//! an address. The field breakdown below was decoded against eight real clone
//! images before it was written, including a controlled series where a single
//! channel was entered on the radio's own keypad between two saves — so the
//! record layout is confirmed against bytes a TH-D72 wrote for itself, not
//! inherited from another model.
//!
//! ```text
//! byte  0..4   freq, u32 little-endian, Hz
//! byte  4      high nibble unknown (the radio writes 0), low nibble tune_step
//! byte  5      mode: 0 = FM, 1 = NFM, 2 = AM
//! byte  6      HIGH nibble tone_mode, LOW nibble duplex
//! byte  7      rtone index into TONES_DHZ
//! byte  8      ctone index into TONES_DHZ
//! byte  9      dtcs  index into DTCS_CODES
//! byte 10      cross_mode index into CROSS_MODES
//! byte 11..15  offset, u32 little-endian, Hz  (for an odd split: the TX freq)
//! byte 15      high nibble unknown (the radio writes 0), low nibble split step
//! ```
//!
//! ## Two encoders, on purpose
//!
//! [`encode_memory`] re-emits a record that was read from an image, preserving
//! every field including ones we cannot explain. It is byte-identical by
//! construction and is what the Phase 2 gate measures.
//!
//! ⚠ `dead_code` is allowed here only because this is Phase 2: the driver is not
//! in `registry.rs` yet, so nothing outside these tests calls any of it. **Take
//! the allow off the moment Phase 3 wires the export path** — an "unused"
//! warning on an encoder is otherwise a bug report in this codebase, and it is
//! how a dead write path was caught on another radio here.
//!
//! [`encode_channel`] builds a record from an app channel and is held to a
//! different standard: it must never emit a value the radio would choke on. The
//! distinction is not academic. Real images in the bug tracker carry `0xFF` in
//! both step nibbles — CHIRP wrote them that way on a CSV import — which decodes
//! as step index 15, shows as a 0 Hz step on the radio, and makes the live-mode
//! driver report ERROR for the channel. `encode_memory` reproduces that damage
//! faithfully when re-emitting such a record; `encode_channel` can never create
//! it. See `scratchpad/kenwood_thd72/FINDINGS.md`.


use super::layout::{
    group_of, prog_vfo_index, ProgVfoTable, CHANNEL_COUNT, ENTRY_LEN, FLAG_BASE, FLAG_EMPTY,
    FLAG_LEN, GROUP_COUNT, GROUP_NAME_BASE, MEMORY_BASE, NAME_BASE, NAME_LEN,
};
use crate::commands::export;
use crate::models::Channel;

/// CTCSS tones in tenths of a hertz, as the D72 indexes them. This is **not**
/// the same list as the TH-D75's: CHIRP's `kenwood_live.KENWOOD_TONES` is
/// `chirp_common.TONES` with eight tones removed (159.8, 165.5, 171.3, 177.3,
/// 183.5, 189.9, 196.6, 199.5), leaving 42. Taking the D75's 50-entry table
/// would silently shift every tone above 156.7.
///
/// Confirmed at four points against real records: index 8 = 88.5 (the value the
/// radio itself fills into an untoned memory), 12 = 100.0 and 13 = 103.5 (on
/// real repeaters), 37 = 229.1.
const TONES_DHZ: [u16; 42] = [
    670, 693, 719, 744, 770, 797, 825, 854, 885, 915, 948, 974, 1000, 1035, 1072, 1109, 1148, 1188,
    1230, 1273, 1318, 1365, 1413, 1462, 1514, 1567, 1622, 1679, 1738, 1799, 1862, 1928, 2035, 2065,
    2107, 2181, 2257, 2291, 2336, 2418, 2503, 2541,
];

/// DTCS codes as the radio indexes them — CHIRP's `chirp_common.DTCS_CODES`,
/// written in octal the way the front panel shows them and the way the channel
/// database stores them ([[dcs-octal-display]]).
const DTCS_CODES: [u16; 104] = [
    23, 25, 26, 31, 32, 36, 43, 47, 51, 53, 54, 65, 71, 72, 73, 74, 114, 115, 116, 122, 125, 131,
    132, 134, 143, 145, 152, 155, 156, 162, 165, 172, 174, 205, 212, 223, 225, 226, 243, 244, 245,
    246, 251, 252, 255, 261, 263, 265, 266, 271, 274, 306, 311, 315, 325, 331, 332, 343, 346, 351,
    356, 364, 365, 371, 411, 412, 413, 423, 431, 432, 445, 446, 452, 454, 455, 462, 464, 465, 466,
    503, 506, 516, 523, 526, 532, 546, 565, 606, 612, 624, 627, 631, 632, 654, 662, 664, 703, 712,
    723, 731, 732, 734, 743, 754,
];

/// Byte 10, indexed into CHIRP's `chirp_common.CROSS_MODES`.
///
/// ⚠ **Unverified on this radio.** Byte 10 is `0x00` in every one of the several
/// hundred real records examined, and none of them uses cross tone at all, so
/// nothing here has been seen to move. The order is the published one; the
/// only claim this module actually stands behind is that a non-cross memory
/// writes `0x00`, which every sample does confirm.
const CROSS_MODES: [&str; 8] = [
    "Tone->Tone",
    "DTCS->",
    "->DTCS",
    "Tone->DTCS",
    "DTCS->Tone",
    "->Tone",
    "DTCS->DTCS",
    "Tone->",
];

/// Tuning steps in Hz, indexed by the low nibble of bytes 4 and 15.
///
/// 8.33 kHz (index 2) is stored as an exact `8330` here for round-tripping, but
/// it is never *chosen* by [`required_step`] — it exists for the air band, which
/// this radio can only receive, and a real 8.33 channel is not an integer
/// multiple of 8330 Hz anyway.
const TUNE_STEPS_HZ: [u32; 11] = [
    5_000, 6_250, 8_330, 10_000, 12_500, 15_000, 20_000, 25_000, 30_000, 50_000, 100_000,
];

const MODE_FM: u8 = 0;
const MODE_NFM: u8 = 1;
const MODE_AM: u8 = 2;

/// Byte 6, high nibble.
const TONE_NONE: u8 = 0x0;
const TONE_CROSS: u8 = 0x1;
const TONE_DTCS: u8 = 0x2;
const TONE_TSQL: u8 = 0x4;
const TONE_TONE: u8 = 0x8;

/// Byte 6, low nibble.
const DUPLEX_NONE: u8 = 0x0;
const DUPLEX_PLUS: u8 = 0x1;
const DUPLEX_MINUS: u8 = 0x2;
const DUPLEX_SPLIT: u8 = 0x4;

/// The tone index the radio itself writes into a memory with no tone at all —
/// 88.5 Hz. Confirmed: the two channels entered from the front panel in the
/// controlled capture carry `tone_mode = 0` and `rtone = ctone = 8`. Filling
/// zero instead would put 67.0 Hz in a field the operator never chose.
const DEFAULT_TONE_IDX: u8 = 8;

/// The name cell's pad byte. Pad to POSITION 8, never to length-of-name — a
/// short name still fills its whole cell, and getting this wrong with spaces
/// instead of the pad byte broke 91 memories on another Kenwood in this repo.
const NAME_PAD: u8 = 0xFF;

// ============================================================
// The structured record
// ============================================================

/// One channel record, fully decoded. Field names follow CHIRP's struct so the
/// two can be read side by side.
///
/// The two `unknown_*` nibbles are carried rather than dropped: the radio writes
/// zero in both, but "we have only ever seen zero" is not the same as "it must
/// be zero", and re-emitting what was there costs nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Thd72Memory {
    pub freq_hz: u32,
    pub tune_step: u8,
    pub unknown_step_high: u8,
    pub mode: u8,
    pub tone_mode: u8,
    pub duplex: u8,
    pub rtone: u8,
    pub ctone: u8,
    pub dtcs: u8,
    pub cross_mode: u8,
    /// The shift in Hz — **or the absolute TX frequency** when `duplex` is
    /// [`DUPLEX_SPLIT`]. The record has no separate TX-frequency field, so an
    /// odd split reuses this one, exactly as CHIRP's clone driver does.
    pub offset_hz: u32,
    pub split_tune_step: u8,
    pub unknown_split_high: u8,
}

/// Decode a 16-byte record. Total, never fails — an out-of-range index is
/// preserved as the raw number so [`encode_memory`] can put it back untouched.
pub(crate) fn decode_memory(m: &[u8; ENTRY_LEN]) -> Thd72Memory {
    Thd72Memory {
        freq_hz: u32::from_le_bytes([m[0], m[1], m[2], m[3]]),
        tune_step: m[4] & 0x0F,
        unknown_step_high: m[4] >> 4,
        mode: m[5],
        tone_mode: m[6] >> 4,
        duplex: m[6] & 0x0F,
        rtone: m[7],
        ctone: m[8],
        dtcs: m[9],
        cross_mode: m[10],
        offset_hz: u32::from_le_bytes([m[11], m[12], m[13], m[14]]),
        split_tune_step: m[15] & 0x0F,
        unknown_split_high: m[15] >> 4,
    }
}

/// Re-emit a decoded record. Byte-identical for anything [`decode_memory`]
/// produced, including records another tool damaged.
pub(crate) fn encode_memory(d: &Thd72Memory) -> [u8; ENTRY_LEN] {
    let mut m = [0u8; ENTRY_LEN];
    m[0..4].copy_from_slice(&d.freq_hz.to_le_bytes());
    m[4] = (d.unknown_step_high << 4) | (d.tune_step & 0x0F);
    m[5] = d.mode;
    m[6] = (d.tone_mode << 4) | (d.duplex & 0x0F);
    m[7] = d.rtone;
    m[8] = d.ctone;
    m[9] = d.dtcs;
    m[10] = d.cross_mode;
    m[11..15].copy_from_slice(&d.offset_hz.to_le_bytes());
    m[15] = (d.unknown_split_high << 4) | (d.split_tune_step & 0x0F);
    m
}

// ============================================================
// The raw record: what actually gets patched into an image
// ============================================================

/// The three cells that make up one memory. They are far apart in the image —
/// the flag at 0x0C00, the record at 0x1500, the name at 0x5E00 — so they travel
/// together to stop two of the three being written without the third, which is
/// how a memory ends up half-programmed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Thd72Record {
    pub memory: [u8; ENTRY_LEN],
    pub flag: [u8; FLAG_LEN],
    pub name: [u8; NAME_LEN],
}

impl Thd72Record {
    /// The programmable-VFO band index this record claims — the low nibble of
    /// the flag byte, and the field that decides whether the radio will key up.
    pub(crate) fn prog_vfo(&self) -> u8 {
        self.flag[0] & 0x0F
    }

    /// Set the 8-char name cell. Kept separate from [`encode_channel`] because
    /// the export path names channels through `export::expanded_name`, which
    /// disambiguates across the whole codeplug and cannot be done one channel at
    /// a time.
    pub(crate) fn set_name(&mut self, name: &str) {
        self.name = name_bytes(name);
    }
}

/// Encode an 8-byte name cell: ASCII, padded to position 8 with [`NAME_PAD`].
/// Non-ASCII is dropped rather than mangled — the radio's character set is
/// ASCII and a multi-byte character would otherwise write two junk bytes.
pub(crate) fn name_bytes(name: &str) -> [u8; NAME_LEN] {
    let mut out = [NAME_PAD; NAME_LEN];
    for (i, ch) in name.chars().filter(char::is_ascii).take(NAME_LEN).enumerate() {
        out[i] = ch as u8;
    }
    out
}

/// Decode a name cell, stopping at the pad byte.
fn decode_name(image: &[u8], slot: usize) -> String {
    let off = NAME_BASE + slot * NAME_LEN;
    if off + NAME_LEN > image.len() {
        return String::new();
    }
    image[off..off + NAME_LEN]
        .iter()
        .take_while(|&&b| b != NAME_PAD && b != 0x00)
        .map(|&b| b as char)
        .collect::<String>()
        .trim_end()
        .to_string()
}

/// Read one memory out of an image. `None` for a slot the radio considers
/// empty — the `disabled` nibble is 0xF — or one the image is too short to hold.
pub(crate) fn read_record(image: &[u8], slot: usize) -> Option<Thd72Record> {
    if slot >= CHANNEL_COUNT {
        return None;
    }
    let (m_off, f_off, n_off) = cells(slot);
    if m_off + ENTRY_LEN > image.len()
        || f_off + FLAG_LEN > image.len()
        || n_off + NAME_LEN > image.len()
    {
        return None;
    }
    if image[f_off] >> 4 == 0x0F {
        return None; // the radio's own "this slot is empty"
    }
    let mut rec = Thd72Record {
        memory: [0u8; ENTRY_LEN],
        flag: [0u8; FLAG_LEN],
        name: [0u8; NAME_LEN],
    };
    rec.memory.copy_from_slice(&image[m_off..m_off + ENTRY_LEN]);
    rec.flag.copy_from_slice(&image[f_off..f_off + FLAG_LEN]);
    rec.name.copy_from_slice(&image[n_off..n_off + NAME_LEN]);
    Some(rec)
}

/// The three image offsets slot `n` occupies.
fn cells(slot: usize) -> (usize, usize, usize) {
    (
        MEMORY_BASE + slot * ENTRY_LEN,
        FLAG_BASE + slot * FLAG_LEN,
        NAME_BASE + slot * NAME_LEN,
    )
}

/// The edits that write one memory, as `(offset, bytes)` pairs.
///
/// ⚠ Signature note: the directive for this file had these take `&mut [u8]`
/// *and* return the edits. They do not take the image at all. Mutating it here
/// would write behind the container's back and lose the dirty-block tracking
/// that keeps an upload from touching regions this driver does not own — and a
/// `&mut` parameter that is never written to is a trap for the next reader.
/// Bounds depend only on `slot`, which is checked against `CHANNEL_COUNT`.
pub(crate) fn apply_record(slot: usize, rec: &Thd72Record) -> Vec<(usize, Vec<u8>)> {
    if slot >= CHANNEL_COUNT {
        return Vec::new();
    }
    let (m_off, f_off, n_off) = cells(slot);
    vec![
        (m_off, rec.memory.to_vec()),
        (f_off, rec.flag.to_vec()),
        (n_off, rec.name.to_vec()),
    ]
}

/// The edits that empty one memory, written the way the radio writes an empty
/// slot: every one of the three cells filled with the pad byte, not zeroed.
/// A zeroed record with a zeroed flag is an *active* memory on 0 Hz.
pub(crate) fn clear_record(slot: usize) -> Vec<(usize, Vec<u8>)> {
    if slot >= CHANNEL_COUNT {
        return Vec::new();
    }
    let (m_off, f_off, n_off) = cells(slot);
    vec![
        (m_off, vec![FLAG_EMPTY; ENTRY_LEN]),
        (f_off, vec![FLAG_EMPTY; FLAG_LEN]),
        (n_off, vec![NAME_PAD; NAME_LEN]),
    ]
}

// ============================================================
// Building a record from an app channel
// ============================================================

fn mhz_to_hz(mhz: f64) -> u32 {
    (mhz * 1_000_000.0).round() as u32
}

/// The smallest listed step that divides the frequency exactly, falling back to
/// 5 kHz. 8.33 kHz is skipped — see [`TUNE_STEPS_HZ`].
fn required_step(hz: u32) -> u8 {
    TUNE_STEPS_HZ
        .iter()
        .enumerate()
        .filter(|&(i, _)| i != 2)
        .find(|&(_, &step)| hz.is_multiple_of(step))
        .map(|(i, _)| i as u8)
        .unwrap_or(0)
}

/// Nearest tone index, the way the rest of this codebase resolves a tone that is
/// not exactly on the table.
fn tone_index(hz: f64) -> u8 {
    let dhz = (hz * 10.0).round() as i32;
    let mut best = 0usize;
    for (i, t) in TONES_DHZ.iter().enumerate() {
        if (i32::from(*t) - dhz).abs() < (i32::from(TONES_DHZ[best]) - dhz).abs() {
            best = i;
        }
    }
    best as u8
}

/// DTCS index for an octal code string as the database stores it.
fn dtcs_index(code: &Option<String>) -> u8 {
    code.as_deref()
        .and_then(|s| s.trim().parse::<u16>().ok())
        .and_then(|n| DTCS_CODES.iter().position(|&c| c == n))
        .unwrap_or(0) as u8
}

/// Build a record for one channel.
///
/// The flag nibble comes from [`prog_vfo_index`] against the table read out of
/// the image being patched, and there is no fallback: a frequency no band covers
/// is an error, never a guess. A guessed nibble produces a memory that the radio
/// stores, displays and receives on but will not transmit from — the failure
/// that put 187 dead repeaters in one user's codeplug.
///
/// The name cell is left empty; the caller sets it with [`Thd72Record::set_name`]
/// once the codeplug-wide disambiguation has run.
pub(crate) fn encode_channel(c: &Channel, table: &ProgVfoTable) -> Result<Thd72Record, String> {
    let rx_hz = mhz_to_hz(c.rx_freq);
    let vfo = prog_vfo_index(table, rx_hz).ok_or_else(|| {
        format!(
            "{:.4} MHz is outside every programmable-VFO band this radio holds — the memory \
             would be stored but could not transmit. Check Menu 130, or leave the channel out.",
            c.rx_freq
        )
    })?;

    let tx_hz = mhz_to_hz(export::tx_frequency(c));
    let split = c.duplex.as_deref() == Some("split");
    let (duplex, offset_hz) = if split {
        // No separate TX-frequency field exists; an odd split puts the absolute
        // TX frequency in the offset field. CHIRP's clone driver does the same.
        (DUPLEX_SPLIT, tx_hz)
    } else if tx_hz > rx_hz {
        (DUPLEX_PLUS, tx_hz - rx_hz)
    } else if tx_hz < rx_hz {
        (DUPLEX_MINUS, rx_hz - tx_hz)
    } else {
        (DUPLEX_NONE, 0)
    };

    let mode = match c.mode.as_deref() {
        Some(m) if m.eq_ignore_ascii_case("NFM") => MODE_NFM,
        // AM is receive-only and band B only on this radio (manual, Menu 131).
        // Encoded when asked for, because refusing it would silently drop an
        // air-band listening memory the operator deliberately added.
        Some(m) if m.eq_ignore_ascii_case("AM") => MODE_AM,
        _ => MODE_FM,
    };

    let requested = c.tone_mode.as_deref().unwrap_or("");
    let tone_mode = if requested.eq_ignore_ascii_case("Tone") {
        TONE_TONE
    } else if requested.eq_ignore_ascii_case("TSQL") {
        TONE_TSQL
    } else if requested.eq_ignore_ascii_case("DTCS") {
        TONE_DTCS
    } else if requested.eq_ignore_ascii_case("Cross") {
        TONE_CROSS
    } else {
        TONE_NONE
    };

    // Both tone fields are always written, whichever mode is selected — that is
    // what the radio does, including on a memory with no tone at all, where it
    // fills 88.5 into both.
    let rtone = c
        .ctcss_uplink
        .or(c.ctcss_downlink)
        .map(tone_index)
        .unwrap_or(DEFAULT_TONE_IDX);
    let ctone = c
        .ctcss_downlink
        .or(c.ctcss_uplink)
        .map(tone_index)
        .unwrap_or(DEFAULT_TONE_IDX);

    let cross_mode = CROSS_MODES
        .iter()
        .position(|&m| m.eq_ignore_ascii_case(&c.cross_mode))
        .unwrap_or(0) as u8;

    let decoded = Thd72Memory {
        freq_hz: rx_hz,
        tune_step: required_step(rx_hz),
        unknown_step_high: 0,
        mode,
        tone_mode,
        duplex,
        rtone,
        ctone,
        dtcs: dtcs_index(&c.dcs_code),
        cross_mode,
        offset_hz,
        // CHIRP derives the split step from the *offset field*, which for an odd
        // split is the TX frequency and otherwise the shift. Non-split memories
        // simply repeat the tuning step, which is what every real record shows.
        split_tune_step: if split {
            required_step(tx_hz)
        } else {
            required_step(rx_hz)
        },
        unknown_split_high: 0,
    };

    Ok(Thd72Record {
        memory: encode_memory(&decoded),
        // disabled nibble 0 (in use), prog_vfo nibble from the image's own
        // table; byte 1 is the scan lockout, which nothing sets yet.
        flag: [vfo & 0x0F, 0x00],
        name: [NAME_PAD; NAME_LEN],
    })
}

// ============================================================
// Read-back for the download sanity sample
// ============================================================

/// One decoded channel for the "is this read real?" table the program dialogs
/// show. Mirrors the TD-H3's shape so it can feed `DecodedChannelSample`.
#[derive(Debug, Clone, serde::Serialize)]
pub(crate) struct Thd72DecodedChannel {
    pub index: usize,
    pub name: String,
    pub rx_mhz: f64,
    /// The shift as the radio applies it: `""` simplex, `"+0.600"`, `"-0.600"`,
    /// or `"split 446.0000"`.
    pub shift: String,
    pub tone: String,
    /// Always `"—"`. The D72's record carries **no per-memory power field** —
    /// CHIRP's `get_features` advertises no power levels for it, and there is no
    /// spare byte in the record that varies with power in any real image. This
    /// is not a decode we skipped; it is a field the radio does not have.
    pub power: String,
    pub mode: String,
    /// The programmable-VFO band index, surfaced because it is the one field
    /// that can leave a channel unable to transmit while everything else about
    /// it looks right.
    pub prog_vfo: u8,
    /// Which of the ten memory groups this slot falls in. Positional.
    pub group: usize,
}

fn tone_summary(d: &Thd72Memory) -> String {
    let tone = |i: u8| {
        TONES_DHZ
            .get(i as usize)
            .map(|t| format!("{:.1}", f64::from(*t) / 10.0))
            .unwrap_or_else(|| format!("?{i}"))
    };
    let dcs = || {
        DTCS_CODES
            .get(d.dtcs as usize)
            .map(|c| format!("{c:03}"))
            .unwrap_or_else(|| format!("?{}", d.dtcs))
    };
    match d.tone_mode {
        TONE_TONE => format!("Tone {}", tone(d.rtone)),
        TONE_TSQL => format!("TSQL {}", tone(d.ctone)),
        TONE_DTCS => format!("DTCS {}", dcs()),
        TONE_CROSS => format!(
            "Cross {}",
            CROSS_MODES.get(d.cross_mode as usize).copied().unwrap_or("?")
        ),
        _ => "—".to_string(),
    }
}

fn shift_summary(d: &Thd72Memory) -> String {
    let off_mhz = f64::from(d.offset_hz) / 1_000_000.0;
    match d.duplex {
        DUPLEX_PLUS => format!("{off_mhz:+.3}"),
        DUPLEX_MINUS => format!("{:+.3}", -off_mhz),
        DUPLEX_SPLIT => format!("split {off_mhz:.4}"),
        _ => String::new(),
    }
}

fn mode_label(mode: u8) -> &'static str {
    match mode {
        MODE_NFM => "NFM",
        MODE_AM => "AM",
        _ => "FM",
    }
}

/// Decode every programmed memory in an image. Empty slots are skipped.
pub(crate) fn decode_channels(image: &[u8]) -> Vec<Thd72DecodedChannel> {
    let mut out = Vec::new();
    for slot in 0..CHANNEL_COUNT {
        let Some(rec) = read_record(image, slot) else {
            continue;
        };
        let d = decode_memory(&rec.memory);
        out.push(Thd72DecodedChannel {
            index: slot,
            name: decode_name(image, slot),
            rx_mhz: f64::from(d.freq_hz) / 1_000_000.0,
            shift: shift_summary(&d),
            tone: tone_summary(&d),
            power: "—".to_string(),
            mode: mode_label(d.mode).to_string(),
            prog_vfo: rec.prog_vfo(),
            group: group_of(slot),
        });
    }
    out
}

/// The ten memory-group names. Always returns ten entries; an unnamed group is
/// an empty string, and the radio's own default is `GRP-0` … `GRP-9`.
pub(crate) fn decode_group_names(image: &[u8]) -> Vec<String> {
    (0..GROUP_COUNT)
        .map(|g| {
            let off = GROUP_NAME_BASE + g * NAME_LEN;
            if off + NAME_LEN > image.len() {
                return String::new();
            }
            image[off..off + NAME_LEN]
                .iter()
                .take_while(|&&b| b != NAME_PAD && b != 0x00)
                .map(|&b| b as char)
                .collect::<String>()
                .trim_end()
                .to_string()
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::radios::kenwood_thd72::layout::{ProgVfoBand, DEFAULT_PROG_VFO, IMAGE_LEN};

    // ---- real bytes, from real radios ----

    /// `002-set-channel-1-from-radio.img` memory 0: entered on the radio's own
    /// keypad, so every field is what a TH-D72 writes for itself.
    const RADIO_MEM0: [u8; 16] = [
        0x00, 0x44, 0x95, 0x08, 0x00, 0x00, 0x00, 0x08, 0x08, 0x00, 0x00, 0xc0, 0x27, 0x09, 0x00,
        0x00,
    ];
    const RADIO_MEM1: [u8; 16] = [
        0xa8, 0xa5, 0x95, 0x08, 0x00, 0x00, 0x00, 0x08, 0x08, 0x00, 0x00, 0xc0, 0x27, 0x09, 0x00,
        0x00,
    ];

    /// `2455-fcv.img`: a real user's repeaters, written by CHIRP — note `0xFF`
    /// in bytes 4 and 15 on every one of them.
    const CHIRP_CH1: [u8; 16] = [
        0x58, 0x45, 0xc3, 0x08, 0xff, 0x00, 0x41, 0x0d, 0x0d, 0x00, 0x00, 0xc0, 0x27, 0x09, 0x00,
        0xff,
    ];
    const CHIRP_CH4: [u8; 16] = [
        0xb0, 0x08, 0xa8, 0x08, 0xff, 0x00, 0x42, 0x0c, 0x0c, 0x00, 0x00, 0xc0, 0x27, 0x09, 0x00,
        0xff,
    ];
    const CHIRP_CH7: [u8; 16] = [
        0x10, 0x64, 0xab, 0x08, 0xff, 0x00, 0x82, 0x0c, 0x0c, 0x00, 0x00, 0xc0, 0x27, 0x09, 0x00,
        0xff,
    ];
    const CHIRP_CH8: [u8; 16] = [
        0x98, 0x28, 0x61, 0x1a, 0xff, 0x00, 0x81, 0x0c, 0x0c, 0x00, 0x00, 0x40, 0x4b, 0x4c, 0x00,
        0xff,
    ];
    const CHIRP_CH20: [u8; 16] = [
        0x58, 0x45, 0xc3, 0x08, 0xff, 0x00, 0x80, 0x0d, 0x0d, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0xff,
    ];
    const CHIRP_CH140: [u8; 16] = [
        0x98, 0x1d, 0xba, 0x08, 0xff, 0x00, 0x40, 0x25, 0x25, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0xff,
    ];

    const ALL_REAL: [[u8; 16]; 8] = [
        RADIO_MEM0, RADIO_MEM1, CHIRP_CH1, CHIRP_CH4, CHIRP_CH7, CHIRP_CH8, CHIRP_CH20, CHIRP_CH140,
    ];

    fn table() -> ProgVfoTable {
        DEFAULT_PROG_VFO
    }

    /// A bare simplex channel. `Channel` derives `Default`, so only the fields
    /// a test actually cares about are named.
    fn channel(rx: f64) -> Channel {
        Channel {
            rx_freq: rx,
            dcs_polarity: "NN".to_string(),
            ..Default::default()
        }
    }

    // ---- decode ----

    #[test]
    fn the_radios_own_memory_decodes_to_what_the_radio_shows() {
        let d = decode_memory(&RADIO_MEM0);
        assert_eq!(d.freq_hz, 144_000_000);
        assert_eq!(d.tune_step, 0); // 5 kHz
        assert_eq!(d.mode, MODE_FM);
        assert_eq!(d.tone_mode, TONE_NONE);
        assert_eq!(d.duplex, DUPLEX_NONE);
        assert_eq!(d.rtone, DEFAULT_TONE_IDX);
        assert_eq!(d.ctone, DEFAULT_TONE_IDX);
        assert_eq!(d.offset_hz, 600_000);
        assert_eq!(decode_memory(&RADIO_MEM1).freq_hz, 144_025_000);
    }

    #[test]
    fn real_repeaters_decode_to_their_published_values() {
        let cases: [([u8; 16], u32, u8, u8, u16, u32); 6] = [
            (CHIRP_CH1, 147_015_000, TONE_TSQL, DUPLEX_PLUS, 1035, 600_000),
            (CHIRP_CH4, 145_230_000, TONE_TSQL, DUPLEX_MINUS, 1000, 600_000),
            (CHIRP_CH7, 145_450_000, TONE_TONE, DUPLEX_MINUS, 1000, 600_000),
            (CHIRP_CH8, 442_575_000, TONE_TONE, DUPLEX_PLUS, 1000, 5_000_000),
            (CHIRP_CH20, 147_015_000, TONE_TONE, DUPLEX_NONE, 1035, 0),
            (CHIRP_CH140, 146_415_000, TONE_TSQL, DUPLEX_NONE, 2291, 0),
        ];
        for (raw, freq, tone_mode, duplex, tone_dhz, offset) in cases {
            let d = decode_memory(&raw);
            assert_eq!(d.freq_hz, freq, "freq of {raw:02x?}");
            assert_eq!(d.tone_mode, tone_mode, "tone mode of {raw:02x?}");
            assert_eq!(d.duplex, duplex, "duplex of {raw:02x?}");
            assert_eq!(TONES_DHZ[d.rtone as usize], tone_dhz, "tone of {raw:02x?}");
            assert_eq!(d.offset_hz, offset, "offset of {raw:02x?}");
        }
    }

    // ---- the Phase 2 gate, at record scale ----

    #[test]
    fn every_real_record_re_encodes_byte_identically() {
        for raw in ALL_REAL {
            let out = encode_memory(&decode_memory(&raw));
            assert_eq!(out, raw, "re-encode of {raw:02x?}");
        }
    }

    /// The damaged records must round-trip *as they are* — re-emission is not
    /// the place to silently repair another tool's output, because a driver that
    /// quietly rewrites bytes it was only asked to preserve cannot be trusted
    /// with the ones it was asked to keep.
    #[test]
    fn re_encoding_preserves_damage_rather_than_hiding_it() {
        let d = decode_memory(&CHIRP_CH1);
        assert_eq!(d.tune_step, 0x0F, "the damaged step nibble is visible");
        assert_eq!(encode_memory(&d), CHIRP_CH1);
    }

    // ---- encode from an app channel ----

    #[test]
    fn a_two_metre_repeater_encodes_the_way_the_radio_writes_one() {
        let mut c = channel(145.230);
        c.duplex = Some("-".to_string());
        c.offset = Some(0.600);
        c.tone_mode = Some("TSQL".to_string());
        c.ctcss_downlink = Some(100.0);
        let rec = encode_channel(&c, &table()).unwrap();
        let d = decode_memory(&rec.memory);
        assert_eq!(d.freq_hz, 145_230_000);
        assert_eq!(d.duplex, DUPLEX_MINUS);
        assert_eq!(d.offset_hz, 600_000);
        assert_eq!(d.tone_mode, TONE_TSQL);
        assert_eq!(TONES_DHZ[d.ctone as usize], 1000);
        // Band A's first range — the same nibble the radio wrote for 144 MHz.
        assert_eq!(rec.prog_vfo(), 0);
        assert_eq!(rec.flag[0] >> 4, 0, "disabled nibble clear = in use");
    }

    #[test]
    fn a_seventy_centimetre_repeater_takes_the_uhf_band_index() {
        let mut c = channel(442.575);
        c.duplex = Some("+".to_string());
        c.offset = Some(5.0);
        let rec = encode_channel(&c, &table()).unwrap();
        assert_eq!(rec.prog_vfo(), 1);
        assert_eq!(decode_memory(&rec.memory).offset_hz, 5_000_000);
    }

    /// The failure this module exists to prevent: a frequency no band covers
    /// must be refused, not given a guessed nibble.
    #[test]
    fn a_channel_outside_every_band_is_refused_by_name() {
        let err = encode_channel(&channel(223.500), &table()).unwrap_err();
        assert!(err.contains("223.5"), "message names the frequency: {err}");
        assert!(
            err.contains("transmit"),
            "message says what would go wrong: {err}"
        );
    }

    /// Rule 2, asserted rather than described: nothing we build may carry the
    /// step nibble that another tool wrote into 187 real memories.
    #[test]
    fn nothing_we_build_carries_the_damaged_step_nibble() {
        for mhz in [144.0, 145.23, 146.415, 147.015, 442.575, 443.0, 121.5] {
            let rec = encode_channel(&channel(mhz), &table()).unwrap();
            let d = decode_memory(&rec.memory);
            assert!(
                (d.tune_step as usize) < TUNE_STEPS_HZ.len(),
                "step nibble {:#X} for {mhz} MHz is not a real step",
                d.tune_step
            );
            assert!(
                (d.split_tune_step as usize) < TUNE_STEPS_HZ.len(),
                "split step nibble {:#X} for {mhz} MHz is not a real step",
                d.split_tune_step
            );
            assert_eq!(rec.memory[4] >> 4, 0, "unknown high nibble stays 0");
            assert_eq!(rec.memory[15] >> 4, 0, "unknown high nibble stays 0");
        }
    }

    #[test]
    fn an_odd_split_puts_the_transmit_frequency_in_the_offset_field() {
        let mut c = channel(145.500);
        c.duplex = Some("split".to_string());
        c.tx_freq = Some(146.100);
        let rec = encode_channel(&c, &table()).unwrap();
        let d = decode_memory(&rec.memory);
        assert_eq!(d.duplex, DUPLEX_SPLIT);
        assert_eq!(d.offset_hz, 146_100_000, "absolute TX freq, not a delta");
    }

    /// The full field matrix: every duplex x every tone mode x every mode x a
    /// spread of steps, through encode -> decode -> encode.
    #[test]
    fn the_field_matrix_round_trips() {
        let freqs = [145.0, 145.0125, 146.52, 442.575, 433.075];
        let duplexes = [None, Some("+"), Some("-"), Some("split")];
        let tones = [None, Some("Tone"), Some("TSQL"), Some("DTCS"), Some("Cross")];
        let modes = [None, Some("FM"), Some("NFM"), Some("AM")];
        let mut checked = 0usize;
        for &f in &freqs {
            for dup in &duplexes {
                for tone in &tones {
                    for mode in &modes {
                        let mut c = channel(f);
                        c.duplex = dup.map(str::to_string);
                        c.tone_mode = tone.map(str::to_string);
                        c.mode = mode.map(str::to_string);
                        c.offset = Some(0.600);
                        c.tx_freq = if *dup == Some("split") {
                            Some(f + 1.0)
                        } else {
                            None
                        };
                        c.ctcss_uplink = Some(103.5);
                        c.ctcss_downlink = Some(100.0);
                        c.dcs_code = Some("031".to_string());
                        c.cross_mode = "Tone->DTCS".to_string();
                        let rec = encode_channel(&c, &table()).unwrap();
                        let d = decode_memory(&rec.memory);
                        assert_eq!(encode_memory(&d), rec.memory, "round trip at {f} MHz");
                        assert_eq!(d.freq_hz, mhz_to_hz(f));
                        checked += 1;
                    }
                }
            }
        }
        assert_eq!(checked, 5 * 4 * 5 * 4);
    }

    #[test]
    fn dtcs_and_cross_indexes_resolve_through_the_published_tables() {
        let mut c = channel(146.520);
        c.tone_mode = Some("DTCS".to_string());
        c.dcs_code = Some("031".to_string());
        let d = decode_memory(&encode_channel(&c, &table()).unwrap().memory);
        assert_eq!(d.tone_mode, TONE_DTCS);
        assert_eq!(DTCS_CODES[d.dtcs as usize], 31);

        c.tone_mode = Some("Cross".to_string());
        c.cross_mode = "DTCS->Tone".to_string();
        let d = decode_memory(&encode_channel(&c, &table()).unwrap().memory);
        assert_eq!(d.tone_mode, TONE_CROSS);
        assert_eq!(CROSS_MODES[d.cross_mode as usize], "DTCS->Tone");
    }

    // ---- names ----

    #[test]
    fn names_pad_to_position_eight_and_round_trip() {
        let mut image = vec![NAME_PAD; IMAGE_LEN];
        for (slot, name) in [(0usize, "abcdef"), (1, "ABCDEFGH"), (2, "A"), (3, "")] {
            let cell = name_bytes(name);
            assert_eq!(cell.len(), NAME_LEN);
            let off = NAME_BASE + slot * NAME_LEN;
            image[off..off + NAME_LEN].copy_from_slice(&cell);
            assert_eq!(decode_name(&image, slot), name);
        }
        // Exactly the bytes the radio wrote for the front-panel entry.
        assert_eq!(
            name_bytes("abcdef"),
            [0x61, 0x62, 0x63, 0x64, 0x65, 0x66, 0xff, 0xff]
        );
    }

    #[test]
    fn an_over_long_name_is_truncated_not_overflowed() {
        assert_eq!(name_bytes("ABCDEFGHIJ"), *b"ABCDEFGH");
    }

    // ---- image-level plumbing ----

    fn image_with(slot: usize, rec: &Thd72Record) -> Vec<u8> {
        let mut image = vec![0xFFu8; IMAGE_LEN];
        for (off, bytes) in apply_record(slot, rec) {
            image[off..off + bytes.len()].copy_from_slice(&bytes);
        }
        image
    }

    #[test]
    fn a_record_written_into_an_image_reads_back_identically() {
        let mut rec = encode_channel(&channel(145.230), &table()).unwrap();
        rec.set_name("W8ABC");
        let image = image_with(7, &rec);
        assert_eq!(read_record(&image, 7).unwrap(), rec);
        assert_eq!(decode_name(&image, 7), "W8ABC");
        // Neighbours untouched.
        assert!(read_record(&image, 6).is_none());
        assert!(read_record(&image, 8).is_none());
    }

    #[test]
    fn an_empty_slot_reads_as_none_and_clearing_restores_that() {
        let rec = encode_channel(&channel(145.230), &table()).unwrap();
        let mut image = image_with(3, &rec);
        assert!(read_record(&image, 3).is_some());
        for (off, bytes) in clear_record(3) {
            image[off..off + bytes.len()].copy_from_slice(&bytes);
        }
        assert!(read_record(&image, 3).is_none());
        assert_eq!(image[FLAG_BASE + 6], FLAG_EMPTY);
    }

    #[test]
    fn a_slot_past_the_last_memory_is_refused_rather_than_written() {
        let rec = encode_channel(&channel(145.230), &table()).unwrap();
        assert!(apply_record(CHANNEL_COUNT, &rec).is_empty());
        assert!(clear_record(CHANNEL_COUNT).is_empty());
        assert!(read_record(&vec![0u8; IMAGE_LEN], CHANNEL_COUNT).is_none());
    }

    #[test]
    fn decode_channels_reports_the_band_index_and_the_group() {
        let mut image = vec![0xFFu8; IMAGE_LEN];
        let mut uhf = encode_channel(&channel(442.575), &table()).unwrap();
        uhf.set_name("UHF");
        for (off, bytes) in apply_record(250, &uhf) {
            image[off..off + bytes.len()].copy_from_slice(&bytes);
        }
        let decoded = decode_channels(&image);
        assert_eq!(decoded.len(), 1);
        assert_eq!(decoded[0].index, 250);
        assert_eq!(decoded[0].group, 2, "memory 250 is in group 2");
        assert_eq!(decoded[0].prog_vfo, 1);
        assert_eq!(decoded[0].name, "UHF");
        assert_eq!(decoded[0].power, "—", "the record has no power field");
        assert_eq!(decoded[0].mode, "FM");
    }

    /// A channel decoded straight out of an image that claims the wrong band is
    /// the 187-dead-repeater case. It must be visible in the sample table, since
    /// nothing else about the channel looks wrong.
    #[test]
    fn a_mis_banded_memory_is_visible_in_the_sample() {
        let mut image = vec![0xFFu8; IMAGE_LEN];
        let mut rec = encode_channel(&channel(145.230), &table()).unwrap();
        rec.flag[0] = 0x01; // what CHIRP wrote into all 187
        rec.set_name("BAD");
        for (off, bytes) in apply_record(0, &rec) {
            image[off..off + bytes.len()].copy_from_slice(&bytes);
        }
        let decoded = decode_channels(&image);
        assert_eq!(decoded[0].prog_vfo, 1);
        assert!(
            (decoded[0].rx_mhz - 145.230).abs() < 1e-6,
            "a 2 m channel claiming band index 1 still looks normal otherwise"
        );
    }

    #[test]
    fn group_names_come_back_as_ten_entries() {
        let mut image = vec![0xFFu8; IMAGE_LEN];
        for g in 0..GROUP_COUNT {
            let off = GROUP_NAME_BASE + g * NAME_LEN;
            image[off..off + NAME_LEN].copy_from_slice(&name_bytes(&format!("GRP-{g}")));
        }
        let names = decode_group_names(&image);
        assert_eq!(names.len(), GROUP_COUNT);
        assert_eq!(names[0], "GRP-0");
        assert_eq!(names[9], "GRP-9");
    }

    #[test]
    fn an_edited_prog_vfo_table_moves_where_a_channel_lands() {
        // Menu 130 narrowed to 144-146: a 147 MHz repeater falls out of band A's
        // first range into band B's, and the nibble must follow.
        let mut narrowed = DEFAULT_PROG_VFO;
        narrowed[0] = ProgVfoBand { start_hz: 144_000_000, end_hz: 146_000_000 };
        let rec = encode_channel(&channel(147.015), &narrowed).unwrap();
        assert_eq!(rec.prog_vfo(), 3);
        assert_eq!(
            encode_channel(&channel(145.0), &narrowed).unwrap().prog_vfo(),
            0
        );
    }
}
