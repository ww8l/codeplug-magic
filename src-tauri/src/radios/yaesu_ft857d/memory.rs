//! FT-857D / FT-897 memories: the 28-byte record, the two bitmaps, and the
//! channel encoder.
//!
//! The record is CHIRP's `mem_struct` from `ft857.py`. Bit fields are listed
//! high bit first, as CHIRP's bitwise syntax reads them:
//!
//! ```text
//! byte  0      tag_on_off:1  tag_default:1  unknown:3  mode:3
//! byte  1      duplex:2  is_duplex:1  is_cwdig_narrow:1  is_fm_narrow:1  freq_range:3
//! byte  2      skip:1  unknown:1  ipo:1  att:1  unknown:4
//! byte  3      ssb_step:2  am_step:3  fm_step:3
//! byte  4      unknown:3  is_split_tone:1  tmode:4
//! byte  5      unknown:2  tx_mode:3  tx_freq_range:3
//! byte  6      unknown:1  toneflag:1  tone:6         (index into TONES_DHZ)
//! byte  7      unknown:1  rxtoneflag:1  rxtone:6
//! byte  8      unknown:1  dcs:7                      (index into DTCS_CODES)
//! byte  9      unknown:1  rxdcs:7
//! byte 10..12  rit, u16 little-endian
//! byte 12..16  freq, u32 BIG-endian, units of 10 Hz
//! byte 16..20  offset, u32 big-endian, 10 Hz — the shift, or the TX frequency
//!              when duplex is split
//! byte 20..28  tag (name), ASCII, space padded
//! ```
//!
//! ## What the FT-897 images settled, and what they could not
//!
//! Seventeen real memories — all HF/6 m CW and SSB — decode coherently against
//! this layout: frequency, mode, `freq_range` (0 on HF, 1 on 6 m), name. **None
//! is FM**, so the tone, DCS, shift and cross-tone encodings below are CHIRP's and
//! have not been seen in a radio-written record. They are confirmed only once
//! ladder step 3 programs an FM repeater and the radio's own screen shows it.
//!
//! The same images also showed two places where the radio and CHIRP's *writer*
//! disagree, and the encoder is built around both:
//!
//! - `is_duplex` is **set on every simplex memory** the radio wrote; CHIRP's
//!   writer clears it for simplex. The encoder leaves the bit alone on simplex
//!   (the radio's value survives) and sets it for +/-/split, as CHIRP does.
//! - The step byte is `0x08` on CW memories and `0x88` on SSB ones. Channels
//!   carry no tuning step, so the encoder never recomputes it.
//!
//! The rule that follows from both: [`encode_channel`] starts from the record
//! already in the slot and changes only what the channel says. A slot that never
//! held a record starts from zero, which is what CHIRP writes into one — the
//! behaviour that has programmed these radios for a decade.

use super::layout::{
    memory_offset, ENTRY_LEN, FILLED_BASE, NAME_LEN, NAME_OFFSET,
    VISIBLE_BASE,
};
use crate::commands::export;
use crate::models::Channel;

/// CTCSS tones in tenths of a hertz — CHIRP's `chirp_common.TONES`, the 50
/// standard tones the manual's menu 083 lists. Index order unverified on the
/// radio (no toned memory in the images).
pub(crate) const TONES_DHZ: [u16; 50] = [
    670, 693, 719, 744, 770, 797, 825, 854, 885, 915, 948, 974, 1000, 1035, 1072, 1109, 1148,
    1188, 1230, 1273, 1318, 1365, 1413, 1462, 1514, 1567, 1598, 1622, 1655, 1679, 1713, 1738,
    1773, 1799, 1835, 1862, 1899, 1928, 1966, 1995, 2035, 2065, 2107, 2181, 2257, 2291, 2336,
    2418, 2503, 2541,
];

/// DCS codes, written the way the radio displays them (octal digits) — CHIRP's
/// `chirp_common.DTCS_CODES`, the 104 the manual's menu 033 lists.
pub(crate) const DTCS_CODES: [u16; 104] = [
    23, 25, 26, 31, 32, 36, 43, 47, 51, 53, 54, 65, 71, 72, 73, 74, 114, 115, 116, 122, 125, 131,
    132, 134, 143, 145, 152, 155, 156, 162, 165, 172, 174, 205, 212, 223, 225, 226, 243, 244, 245,
    246, 251, 252, 255, 261, 263, 265, 266, 271, 274, 306, 311, 315, 325, 331, 332, 343, 346, 351,
    356, 364, 365, 371, 411, 412, 413, 423, 431, 432, 445, 446, 452, 454, 455, 462, 464, 465, 466,
    503, 506, 516, 523, 526, 532, 546, 565, 606, 612, 624, 627, 631, 632, 654, 662, 664, 703, 712,
    723, 731, 732, 734, 743, 754,
];

// `mode` — CHIRP's MODES order. Narrow is a separate bit, not a mode.
pub(crate) const MODE_LSB: u8 = 0;
pub(crate) const MODE_USB: u8 = 1;
pub(crate) const MODE_CW: u8 = 2;
pub(crate) const MODE_CWR: u8 = 3;
pub(crate) const MODE_AM: u8 = 4;
pub(crate) const MODE_FM: u8 = 5;
pub(crate) const MODE_DIG: u8 = 6;
pub(crate) const MODE_PKT: u8 = 7;

pub(crate) const DUPLEX_NONE: u8 = 0;
pub(crate) const DUPLEX_MINUS: u8 = 1;
pub(crate) const DUPLEX_PLUS: u8 = 2;
pub(crate) const DUPLEX_SPLIT: u8 = 3;

// `tmode` with `is_split_tone` clear — CHIRP's FT857 TMODES. Unverified.
pub(crate) const TMODE_OFF: u8 = 0x00;
pub(crate) const TMODE_TONE: u8 = 0x04;
pub(crate) const TMODE_TSQL: u8 = 0x05;
pub(crate) const TMODE_DTCS: u8 = 0x0A;

/// `tmode` with `is_split_tone` set — CHIRP's CROSS_MODES, in the app's
/// `cross_mode` vocabulary. Unverified.
const CROSS_MODES: [(&str, u8); 8] = [
    ("->Tone", 0x01),
    ("->DTCS", 0x02),
    ("Tone->", 0x04),
    ("Tone->Tone", 0x05),
    ("Tone->DTCS", 0x06),
    ("DTCS->", 0x08),
    ("DTCS->Tone", 0x09),
    ("DTCS->DTCS", 0x0A),
];

/// The radio's `freq_range` code is an index into this list (Hz, `lo..hi`).
/// CHIRP's `VALID_BANDS`, and the order is load-bearing — the images show 0 on
/// HF and 1 on 6 m, matching it. A frequency in none of them is outside the
/// receiver and cannot be stored.
pub(crate) const FREQ_RANGES: [(u32, u32); 6] = [
    (100_000, 33_000_000),
    (33_000_000, 56_000_000),
    (76_000_000, 108_000_000),
    (108_000_000, 137_000_000),
    (137_000_000, 164_000_000),
    (420_000_000, 470_000_000),
];

/// The radio's `freq_range` code for `hz`, or `None` outside every range.
pub(crate) fn freq_range(hz: u32) -> Option<u8> {
    FREQ_RANGES.iter().position(|&(lo, hi)| hz >= lo && hz < hi).map(|i| i as u8)
}

/// One record, fully decoded. Every bit is carried, named or not, so
/// [`encode_memory`] is byte-identical by construction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Ft857Memory {
    pub tag_on_off: bool,
    pub tag_default: bool,
    pub unknown0: u8,
    pub mode: u8,
    pub duplex: u8,
    pub is_duplex: bool,
    pub is_cwdig_narrow: bool,
    pub is_fm_narrow: bool,
    pub freq_range: u8,
    /// Byte 2 whole: skip, ipo, att and five unknown bits. The app has no
    /// channel field for any of them, so they are only ever preserved.
    pub flags2: u8,
    /// Byte 3 whole: the three per-mode tuning steps. Preserved, never computed.
    pub steps: u8,
    pub unknown4: u8,
    pub is_split_tone: bool,
    pub tmode: u8,
    pub unknown5: u8,
    pub tx_mode: u8,
    pub tx_freq_range: u8,
    /// Bytes 6-9 whole, flag bits and all.
    pub tone: u8,
    pub rxtone: u8,
    pub dcs: u8,
    pub rxdcs: u8,
    pub rit: u16,
    pub freq_10hz: u32,
    pub offset_10hz: u32,
    pub name: [u8; NAME_LEN],
}

/// The low six bits of a tone byte are the index; bit 6 is CHIRP's
/// `unknown_toneflag`, which it forces to 0 because a set flag garbles menu 083.
const TONE_INDEX: u8 = 0x3F;
const DCS_INDEX: u8 = 0x7F;

/// Decode a 28-byte record. Total: never fails, never normalises.
pub(crate) fn decode_memory(m: &[u8; ENTRY_LEN]) -> Ft857Memory {
    let mut name = [0u8; NAME_LEN];
    name.copy_from_slice(&m[NAME_OFFSET..NAME_OFFSET + NAME_LEN]);
    Ft857Memory {
        tag_on_off: m[0] & 0x80 != 0,
        tag_default: m[0] & 0x40 != 0,
        unknown0: (m[0] >> 3) & 0x07,
        mode: m[0] & 0x07,
        duplex: m[1] >> 6,
        is_duplex: m[1] & 0x20 != 0,
        is_cwdig_narrow: m[1] & 0x10 != 0,
        is_fm_narrow: m[1] & 0x08 != 0,
        freq_range: m[1] & 0x07,
        flags2: m[2],
        steps: m[3],
        unknown4: m[4] >> 5,
        is_split_tone: m[4] & 0x10 != 0,
        tmode: m[4] & 0x0F,
        unknown5: m[5] >> 6,
        tx_mode: (m[5] >> 3) & 0x07,
        tx_freq_range: m[5] & 0x07,
        tone: m[6],
        rxtone: m[7],
        dcs: m[8],
        rxdcs: m[9],
        rit: u16::from_le_bytes([m[10], m[11]]),
        freq_10hz: u32::from_be_bytes([m[12], m[13], m[14], m[15]]),
        offset_10hz: u32::from_be_bytes([m[16], m[17], m[18], m[19]]),
        name,
    }
}

/// Re-emit a decoded record. Byte-identical for anything [`decode_memory`]
/// produced.
pub(crate) fn encode_memory(d: &Ft857Memory) -> [u8; ENTRY_LEN] {
    let mut m = [0u8; ENTRY_LEN];
    m[0] = (d.tag_on_off as u8) << 7
        | (d.tag_default as u8) << 6
        | (d.unknown0 & 0x07) << 3
        | (d.mode & 0x07);
    m[1] = (d.duplex & 0x03) << 6
        | (d.is_duplex as u8) << 5
        | (d.is_cwdig_narrow as u8) << 4
        | (d.is_fm_narrow as u8) << 3
        | (d.freq_range & 0x07);
    m[2] = d.flags2;
    m[3] = d.steps;
    m[4] = (d.unknown4 & 0x07) << 5 | (d.is_split_tone as u8) << 4 | (d.tmode & 0x0F);
    m[5] = (d.unknown5 & 0x03) << 6 | (d.tx_mode & 0x07) << 3 | (d.tx_freq_range & 0x07);
    m[6] = d.tone;
    m[7] = d.rxtone;
    m[8] = d.dcs;
    m[9] = d.rxdcs;
    m[10..12].copy_from_slice(&d.rit.to_le_bytes());
    m[12..16].copy_from_slice(&d.freq_10hz.to_be_bytes());
    m[16..20].copy_from_slice(&d.offset_10hz.to_be_bytes());
    m[NAME_OFFSET..NAME_OFFSET + NAME_LEN].copy_from_slice(&d.name);
    m
}

/// The record CHIRP writes into a slot that has never held one: all zero.
pub(crate) fn blank_memory() -> Ft857Memory {
    decode_memory(&[0u8; ENTRY_LEN])
}

// ============================================================
// Bitmaps
// ============================================================

fn bit(slot: usize) -> (usize, u8) {
    (slot / 8, 1u8 << (slot % 8))
}

/// Whether regular memory `slot` (0-based) is shown by the radio.
pub(crate) fn is_visible(image: &[u8], slot: usize) -> bool {
    let (byte, mask) = bit(slot);
    image[VISIBLE_BASE + byte] & mask != 0
}

/// Whether regular memory `slot` (0-based) holds a record.
pub(crate) fn is_filled(image: &[u8], slot: usize) -> bool {
    let (byte, mask) = bit(slot);
    image[FILLED_BASE + byte] & mask != 0
}

/// Read regular memory `slot` (0-based), whatever its bitmaps say.
pub(crate) fn read_memory(image: &[u8], slot: usize) -> Ft857Memory {
    let at = memory_offset(slot);
    let mut raw = [0u8; ENTRY_LEN];
    raw.copy_from_slice(&image[at..at + ENTRY_LEN]);
    decode_memory(&raw)
}

/// The record already in `slot`, if the radio considers it one. A slot that was
/// never filled holds `FF` filler, which must not be used as a base.
pub(crate) fn existing_memory(image: &[u8], slot: usize) -> Option<Ft857Memory> {
    is_filled(image, slot).then(|| read_memory(image, slot))
}

/// Write `m` into `slot` and mark it visible and filled.
pub(crate) fn store_memory(image: &mut [u8], slot: usize, m: &Ft857Memory) {
    let at = memory_offset(slot);
    image[at..at + ENTRY_LEN].copy_from_slice(&encode_memory(m));
    let (byte, mask) = bit(slot);
    image[VISIBLE_BASE + byte] |= mask;
    image[FILLED_BASE + byte] |= mask;
}

/// Hide `slot`, the way the radio erases a memory: the record and its filled
/// bit stay, only visible goes. CHIRP refuses to erase memory 1 ("if you upload
/// an empty image you can brick your radio"), and so does this.
pub(crate) fn hide_memory(image: &mut [u8], slot: usize) -> Result<(), String> {
    if slot == 0 {
        return Err("memory 1 cannot be left empty on this radio".to_string());
    }
    let (byte, mask) = bit(slot);
    image[VISIBLE_BASE + byte] &= !mask;
    Ok(())
}

/// The slots (0-based) the radio shows.
#[cfg(test)]
pub(crate) fn visible_slots(image: &[u8]) -> Vec<usize> {
    (0..super::layout::CHANNEL_COUNT).filter(|&s| is_visible(image, s)).collect()
}

// ============================================================
// Channel <-> record
// ============================================================

fn mhz_to_10hz(mhz: f64) -> u32 {
    (mhz * 100_000.0).round() as u32
}

fn tone_index(hz: f64) -> Option<u8> {
    let dhz = (hz * 10.0).round() as u16;
    TONES_DHZ.iter().position(|&t| t == dhz).map(|i| i as u8)
}

fn dtcs_index(code: &str) -> Option<u8> {
    let n: u16 = code.trim().parse().ok()?;
    DTCS_CODES.iter().position(|&c| c == n).map(|i| i as u8)
}

/// Encode a tag: printable ASCII, space padded. CHIRP pads with spaces because
/// some FT-857Ds misbehave on `FF`-padded short tags (CHIRP bug #937), and every
/// tag the FT-897 wrote is space padded. `\ { | }` are not in the radio's set.
pub(crate) fn name_bytes(name: &str) -> [u8; NAME_LEN] {
    let mut out = [b' '; NAME_LEN];
    let ok = |c: &char| c.is_ascii_graphic() || *c == ' ';
    for (i, ch) in name
        .chars()
        .filter(ok)
        .filter(|c| !"\\{|}".contains(*c))
        .take(NAME_LEN)
        .enumerate()
    {
        out[i] = ch as u8;
    }
    out
}

/// The record's tag as text, or empty when the tag is switched off.
pub(crate) fn tag_text(m: &Ft857Memory) -> String {
    if !m.tag_on_off {
        return String::new();
    }
    m.name
        .iter()
        .take_while(|&&b| b != 0xFF)
        .map(|&b| if (0x20..0x7F).contains(&b) { b as char } else { '*' })
        .collect::<String>()
        .trim_end()
        .to_string()
}

/// The app's mode string for a record. CWR, DIG and PKT have no channel-library
/// equivalent; they are named for display and refused by [`encode_channel`].
pub(crate) fn mode_text(m: &Ft857Memory) -> &'static str {
    match m.mode {
        MODE_LSB => "LSB",
        MODE_USB => "USB",
        MODE_CW => "CW",
        MODE_CWR => "CWR",
        MODE_AM => "AM",
        MODE_FM if m.is_fm_narrow => "NFM",
        MODE_FM => "FM",
        MODE_DIG => "DIG",
        MODE_PKT => "PKT",
        _ => "?",
    }
}

/// Build the record for `c` in a slot that holds `base` (or nothing).
///
/// Starts from `base` and changes only what the channel determines, so a
/// channel written over the memory it came from leaves every byte alone — the
/// property [`super::real_images`] measures. Refuses rather than guesses: a
/// frequency outside the receiver, a mode the radio has no slot for, a tone not
/// in its table.
///
/// Per-channel DCS polarity does not exist on this radio — it is global, menu
/// 034 DCS INV — so `dcs_polarity` is not encoded.
pub(crate) fn encode_channel(
    c: &Channel,
    name: &str,
    base: Option<&Ft857Memory>,
) -> Result<Ft857Memory, String> {
    let mut m = base.cloned().unwrap_or_else(blank_memory);

    let rx = mhz_to_10hz(c.rx_freq);
    m.freq_10hz = rx;
    m.freq_range = freq_range(rx * 10).ok_or_else(|| {
        format!("{:.5} MHz is outside the radio's receive coverage", c.rx_freq)
    })?;

    let (mode, narrow) = match c.mode.as_deref().unwrap_or("FM").to_ascii_uppercase().as_str() {
        "FM" => (MODE_FM, false),
        "NFM" => (MODE_FM, true),
        "AM" => (MODE_AM, false),
        "USB" => (MODE_USB, false),
        "LSB" => (MODE_LSB, false),
        "CW" => (MODE_CW, false),
        other => return Err(format!("mode {other} has no memory mode on this radio")),
    };
    m.mode = mode;
    // CHIRP sets both narrow bits together; for a non-narrow mode both clear.
    m.is_fm_narrow = narrow;
    m.is_cwdig_narrow = narrow;

    let tx = mhz_to_10hz(export::tx_frequency(c));
    if c.duplex.as_deref() == Some("split") {
        m.duplex = DUPLEX_SPLIT;
        m.is_duplex = true;
        m.offset_10hz = tx;
        m.tx_mode = mode;
        m.tx_freq_range = freq_range(tx * 10).ok_or_else(|| {
            format!("split TX {:.5} MHz is outside the radio's coverage", tx as f64 / 100_000.0)
        })?;
    } else if tx != rx {
        m.duplex = if tx > rx { DUPLEX_PLUS } else { DUPLEX_MINUS };
        m.is_duplex = true;
        m.offset_10hz = tx.abs_diff(rx);
    } else {
        // Simplex. `is_duplex` and the offset field keep whatever the slot held:
        // the radio itself leaves both set on simplex memories (the offset is the
        // band's default shift), and CHIRP's writer clearing them is not evidence
        // the radio needs that.
        m.duplex = DUPLEX_NONE;
    }

    encode_tone(c, &mut m)?;

    m.tag_on_off = !name.trim().is_empty();
    // Never the radio's default "CH-nnn" label — CHIRP's rule.
    m.tag_default = false;
    m.name = name_bytes(name);
    // The clarifier offset is operating state, not codeplug; CHIRP zeroes it.
    m.rit = 0;
    Ok(m)
}

fn encode_tone(c: &Channel, m: &mut Ft857Memory) -> Result<(), String> {
    let want = c.tone_mode.as_deref().unwrap_or("off");
    let tone = |hz: Option<f64>| -> Result<u8, String> {
        let hz = hz.ok_or("a CTCSS tone mode with no tone")?;
        tone_index(hz).ok_or_else(|| format!("{hz:.1} Hz is not one of the radio's 50 CTCSS tones"))
    };
    let dcs = |code: Option<&str>| -> Result<u8, String> {
        let code = code.ok_or("a DCS tone mode with no code")?;
        dtcs_index(code).ok_or_else(|| format!("DCS {code} is not one of the radio's 104 codes"))
    };
    // Keep the flag bits (CHIRP forces them to 0 only when it writes a value).
    let set_tone = |byte: &mut u8, idx: u8| *byte = idx & TONE_INDEX;
    let set_dcs = |byte: &mut u8, idx: u8| *byte = (*byte & !DCS_INDEX) | (idx & DCS_INDEX);

    if want.eq_ignore_ascii_case("Tone") {
        let t = tone(c.ctcss_uplink.or(c.ctcss_downlink))?;
        m.is_split_tone = false;
        m.tmode = TMODE_TONE;
        set_tone(&mut m.tone, t);
        set_tone(&mut m.rxtone, t);
    } else if want.eq_ignore_ascii_case("TSQL") {
        let t = tone(c.ctcss_downlink.or(c.ctcss_uplink))?;
        m.is_split_tone = false;
        m.tmode = TMODE_TSQL;
        set_tone(&mut m.tone, t);
        set_tone(&mut m.rxtone, t);
    } else if want.eq_ignore_ascii_case("DTCS") {
        let d = dcs(c.dcs_code.as_deref())?;
        m.is_split_tone = false;
        m.tmode = TMODE_DTCS;
        set_dcs(&mut m.dcs, d);
        set_dcs(&mut m.rxdcs, d);
    } else if want.eq_ignore_ascii_case("Cross") {
        let code = CROSS_MODES
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(&c.cross_mode))
            .map(|&(_, v)| v)
            .ok_or_else(|| format!("cross mode {} is not one this radio has", c.cross_mode))?;
        let (tx_kind, rx_kind) = c.cross_mode.split_once("->").unwrap_or(("", ""));
        if tx_kind.eq_ignore_ascii_case("Tone") {
            set_tone(&mut m.tone, tone(c.ctcss_uplink)?);
        } else if tx_kind.eq_ignore_ascii_case("DTCS") {
            set_dcs(&mut m.dcs, dcs(c.dcs_code.as_deref())?);
        }
        if rx_kind.eq_ignore_ascii_case("Tone") {
            set_tone(&mut m.rxtone, tone(c.ctcss_downlink)?);
        } else if rx_kind.eq_ignore_ascii_case("DTCS") {
            set_dcs(&mut m.rxdcs, dcs(c.dcs_rx_code.as_deref().or(c.dcs_code.as_deref()))?);
        }
        m.is_split_tone = true;
        m.tmode = code;
    } else {
        // Off. The tone values stay as they were — the radio keeps them too.
        m.is_split_tone = false;
        m.tmode = TMODE_OFF;
    }
    Ok(())
}

/// The channel a record describes, in the app's vocabulary — the inverse of
/// [`encode_channel`] for everything the channel model can say.
pub(crate) fn decode_channel(m: &Ft857Memory) -> Channel {
    let rx = m.freq_10hz as f64 / 100_000.0;
    let mut c = Channel {
        rx_freq: rx,
        mode: Some(mode_text(m).to_string()),
        dcs_polarity: "NN".to_string(),
        cross_mode: "Tone->Tone".to_string(),
        ..Default::default()
    };
    let off = m.offset_10hz as f64 / 100_000.0;
    match m.duplex {
        DUPLEX_PLUS => {
            c.duplex = Some("+".into());
            c.offset = Some(off);
            c.tx_freq = Some(rx + off);
        }
        DUPLEX_MINUS => {
            c.duplex = Some("-".into());
            c.offset = Some(off);
            c.tx_freq = Some(rx - off);
        }
        DUPLEX_SPLIT => {
            c.duplex = Some("split".into());
            c.tx_freq = Some(off);
        }
        _ => {}
    }
    let tone_hz = |b: u8| TONES_DHZ.get((b & TONE_INDEX) as usize).map(|&t| t as f64 / 10.0);
    let dcs_text =
        |b: u8| DTCS_CODES.get((b & DCS_INDEX) as usize).map(|&d| format!("{d:03}"));
    if m.is_split_tone {
        if let Some(&(name, _)) = CROSS_MODES.iter().find(|&&(_, v)| v == m.tmode) {
            c.tone_mode = Some("Cross".into());
            c.cross_mode = name.to_string();
            c.ctcss_uplink = tone_hz(m.tone);
            c.ctcss_downlink = tone_hz(m.rxtone);
            c.dcs_code = dcs_text(m.dcs);
            c.dcs_rx_code = dcs_text(m.rxdcs);
        }
    } else {
        match m.tmode {
            TMODE_TONE => {
                c.tone_mode = Some("Tone".into());
                c.ctcss_uplink = tone_hz(m.tone);
            }
            TMODE_TSQL => {
                c.tone_mode = Some("TSQL".into());
                c.ctcss_uplink = tone_hz(m.tone);
                c.ctcss_downlink = tone_hz(m.rxtone);
            }
            TMODE_DTCS => {
                c.tone_mode = Some("DTCS".into());
                c.dcs_code = dcs_text(m.dcs);
            }
            _ => c.tone_mode = Some("off".into()),
        }
    }
    c
}

#[cfg(test)]
mod tests {
    use super::super::layout::{BITMAP_LEN, IMAGE_LEN_US};
    use super::*;

    /// Memory 1 exactly as the FT-897 wrote it (capture A, 2026-10-17):
    /// 50.000 MHz CW, tag "6CW 50.0", simplex with `is_duplex` set and a 1 MHz
    /// default shift in the offset field.
    const REAL_6M_CW: [u8; ENTRY_LEN] = [
        0x82, 0x21, 0x00, 0x08, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x4C, 0x4B,
        0x40, 0x00, 0x01, 0x86, 0xA0, b'6', b'C', b'W', b' ', b'5', b'0', b'.', b'0',
    ];
    /// Memory 15: 7.175 MHz LSB, "40SSB 7.", step byte 0x88.
    const REAL_40M_LSB: [u8; ENTRY_LEN] = [
        0x80, 0x20, 0x00, 0x88, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x0A, 0xF2,
        0xBC, 0x00, 0x00, 0x27, 0x10, b'4', b'0', b'S', b'S', b'B', b' ', b'7', b'.',
    ];

    #[test]
    fn real_records_decode_to_what_the_radio_showed() {
        let m = decode_memory(&REAL_6M_CW);
        assert_eq!(m.freq_10hz * 10, 50_000_000);
        assert_eq!(mode_text(&m), "CW");
        assert_eq!(m.freq_range, 1, "6 m is range 1");
        assert_eq!(tag_text(&m), "6CW 50.0");
        assert!(m.is_duplex && m.duplex == DUPLEX_NONE, "the radio's simplex quirk");

        let m = decode_memory(&REAL_40M_LSB);
        assert_eq!(m.freq_10hz * 10, 7_175_000);
        assert_eq!(mode_text(&m), "LSB");
        assert_eq!(m.freq_range, 0);
        assert_eq!(m.steps, 0x88);
    }

    #[test]
    fn real_records_survive_both_round_trips() {
        for raw in [REAL_6M_CW, REAL_40M_LSB] {
            let m = decode_memory(&raw);
            assert_eq!(encode_memory(&m), raw, "lossless");
            let c = decode_channel(&m);
            let again = encode_channel(&c, &tag_text(&m), Some(&m)).unwrap();
            assert_eq!(encode_memory(&again), raw, "channel-level");
        }
    }

    fn fm(rx: f64, tx: f64) -> Channel {
        Channel {
            rx_freq: rx,
            tx_freq: Some(tx),
            mode: Some("FM".into()),
            dcs_polarity: "NN".into(),
            cross_mode: "Tone->Tone".into(),
            ..Default::default()
        }
    }

    #[test]
    fn a_fresh_repeater_record_has_every_field_chirp_would_write() {
        let mut c = fm(146.94, 146.34);
        c.tone_mode = Some("Tone".into());
        c.ctcss_uplink = Some(100.0);
        let m = encode_channel(&c, "W8ABC", None).unwrap();
        assert_eq!(m.freq_10hz, 14_694_000);
        assert_eq!(m.freq_range, 4, "2 m is range 4");
        assert_eq!((m.duplex, m.is_duplex, m.offset_10hz), (DUPLEX_MINUS, true, 60_000));
        assert_eq!((m.is_split_tone, m.tmode), (false, TMODE_TONE));
        assert_eq!(TONES_DHZ[m.tone as usize], 1000);
        assert!(m.tag_on_off);
        assert_eq!(&m.name, b"W8ABC   ");
        let back = decode_channel(&m);
        assert_eq!(back.tx_freq.map(|f| (f * 1000.0).round()), Some(146_340.0));
        assert_eq!(back.ctcss_uplink, Some(100.0));
    }

    #[test]
    fn narrow_split_and_dcs() {
        let mut c = fm(446.0, 446.0);
        c.mode = Some("NFM".into());
        c.tone_mode = Some("DTCS".into());
        c.dcs_code = Some("023".into());
        let m = encode_channel(&c, "", None).unwrap();
        assert!(m.is_fm_narrow && m.mode == MODE_FM);
        assert_eq!((m.tmode, m.dcs, m.rxdcs), (TMODE_DTCS, 0, 0));
        assert!(!m.tag_on_off);

        let mut c = fm(144.39, 430.0);
        c.duplex = Some("split".into());
        let m = encode_channel(&c, "SPLIT", None).unwrap();
        assert_eq!((m.duplex, m.offset_10hz, m.tx_freq_range), (DUPLEX_SPLIT, 43_000_000, 5));
    }

    #[test]
    fn refusals_name_the_problem() {
        let gap = encode_channel(&fm(222.0, 222.0), "", None).unwrap_err();
        assert!(gap.contains("222.00000 MHz"), "{gap}");
        let mut c = fm(146.52, 146.52);
        c.mode = Some("DMR".into());
        assert!(encode_channel(&c, "", None).unwrap_err().contains("DMR"));
        c.mode = Some("FM".into());
        c.tone_mode = Some("Tone".into());
        c.ctcss_uplink = Some(150.0);
        assert!(encode_channel(&c, "", None).unwrap_err().contains("150.0"));
    }

    #[test]
    fn bitmaps_are_lsb_first_and_memory_one_cannot_be_hidden() {
        let mut image = vec![0xFFu8; IMAGE_LEN_US];
        image[VISIBLE_BASE..VISIBLE_BASE + BITMAP_LEN].fill(0);
        image[FILLED_BASE..FILLED_BASE + BITMAP_LEN].fill(0);
        store_memory(&mut image, 16, &decode_memory(&REAL_6M_CW));
        // Memory 17 is bit 0 of byte 2 — where the FT-897 put its 17th memory.
        assert_eq!(image[VISIBLE_BASE + 2], 0x01);
        assert_eq!(image[FILLED_BASE + 2], 0x01);
        assert_eq!(visible_slots(&image), vec![16]);
        hide_memory(&mut image, 16).unwrap();
        assert!(!is_visible(&image, 16) && is_filled(&image, 16));
        assert!(hide_memory(&mut image, 0).is_err());
    }
}
