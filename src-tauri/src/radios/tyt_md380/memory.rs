//! The MD-380's records: channels, zones, contacts, RX group lists, scan lists.
//!
//! Each record decodes into the fields the app programs plus `other` — every
//! remaining bit, kept verbatim. Encoding puts the fields back over `other`, so
//! a record the app does not change goes back out exactly as it came in, and a
//! new one starts from [`Channel::template`] and its siblings.
//!
//! ★ Why `other` and not "fixed bits": dmrconfig documents several channel bits
//! as fixed (byte 0 bit 6 = 1, byte 5 = 0b11xx0000, byte 15 = 0xFF, bytes
//! 30-31 = 0xFF), and TYT's CPS template and all eight md380tools factory dumps
//! agree. Every one of the 329 channels on Tim's radio has ALL of those bits at
//! zero, and his radio works. So the values are a convention, not a rule the
//! radio enforces, and they are preserved rather than asserted. New records get
//! the factory convention.
//!
//! Bit numbers are LSB = 0 (Farnsworth's tables count from the MSB).

use super::layout::NAME_UNITS;

// --- shared encodings ----------------------------------------------------

/// UTF-16LE, 16 code units, NUL-padded; stops at the first NUL.
pub(crate) fn decode_name(raw: &[u8]) -> String {
    let units: Vec<u16> = raw
        .as_chunks::<2>()
        .0
        .iter()
        .map(|&c| u16::from_le_bytes(c))
        .take_while(|&u| u != 0)
        .collect();
    String::from_utf16_lossy(&units)
}

/// The inverse of [`decode_name`], truncating at 16 code units (never inside a
/// surrogate pair).
pub(crate) fn encode_name(name: &str, out: &mut [u8]) {
    out.fill(0);
    let mut units = Vec::with_capacity(NAME_UNITS);
    for ch in name.chars() {
        let mut buf = [0u16; 2];
        let enc = ch.encode_utf16(&mut buf);
        if units.len() + enc.len() > out.len() / 2 {
            break;
        }
        units.extend_from_slice(enc);
    }
    for (i, u) in units.iter().enumerate() {
        out[2 * i..2 * i + 2].copy_from_slice(&u.to_le_bytes());
    }
}

/// Eight BCD digits, little-endian, in 10 Hz units — `00 25 15 45` = 451.52500.
/// `None` for anything that is not BCD.
pub(crate) fn decode_freq(raw: &[u8]) -> Option<u32> {
    let mut n = 0u32;
    for &b in raw.iter().rev() {
        let (hi, lo) = (b >> 4, b & 0xF);
        if hi > 9 || lo > 9 {
            return None;
        }
        n = n * 100 + (hi as u32) * 10 + lo as u32;
    }
    Some(n)
}

pub(crate) fn encode_freq(ten_hz: u32, out: &mut [u8]) {
    let mut n = ten_hz;
    for b in out.iter_mut() {
        let lo = n % 10;
        let hi = (n / 10) % 10;
        *b = ((hi << 4) | lo) as u8;
        n /= 100;
    }
}

pub(crate) fn mhz_to_10hz(mhz: f64) -> u32 {
    (mhz * 100_000.0).round() as u32
}

pub(crate) fn ten_hz_to_mhz(v: u32) -> f64 {
    v as f64 / 100_000.0
}

/// A CTCSS/DCS field: 16-bit LE, top two bits the kind (00 CTCSS, 10 DCS
/// normal, 11 DCS inverted), the low 14 bits BCD — CTCSS in 0.1 Hz, DCS as its
/// octal digits. `0xFFFF` is none.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum Tone {
    /// The raw value, kept so a radio's own spelling of "none" (0xFFFF, or the
    /// 0x0000 some tools write) survives a round trip.
    None(u16),
    /// Tenths of a hertz: 100.0 Hz = 1000.
    Ctcss(u16),
    /// The code as written, octal digits: D023 = 0o23 → stored as 23.
    Dcs { code: u16, inverted: bool },
}

fn bcd16(v: u16) -> Option<u16> {
    let mut n = 0u16;
    for shift in [12, 8, 4, 0] {
        let d = (v >> shift) & 0xF;
        if d > 9 {
            return None;
        }
        n = n * 10 + d;
    }
    Some(n)
}

fn to_bcd16(n: u16) -> u16 {
    ((n / 1000 % 10) << 12) | ((n / 100 % 10) << 8) | ((n / 10 % 10) << 4) | (n % 10)
}

impl Tone {
    pub(crate) const NONE: Tone = Tone::None(0xFFFF);

    pub(crate) fn decode(raw: [u8; 2]) -> Tone {
        let v = u16::from_le_bytes(raw);
        if v == 0xFFFF || v == 0 {
            return Tone::None(v);
        }
        let digits = bcd16(v & 0x3FFF);
        match (v >> 14, digits) {
            (0, Some(d)) => Tone::Ctcss(d),
            (2, Some(d)) => Tone::Dcs { code: d, inverted: false },
            (3, Some(d)) => Tone::Dcs { code: d, inverted: true },
            _ => Tone::None(v),
        }
    }

    pub(crate) fn encode(self) -> [u8; 2] {
        let v = match self {
            Tone::None(raw) => raw,
            Tone::Ctcss(d) => to_bcd16(d),
            Tone::Dcs { code, inverted } => to_bcd16(code) | if inverted { 0xC000 } else { 0x8000 },
        };
        v.to_le_bytes()
    }
}

/// Whether a named record (zone, RX group, scan list) is in use: its name's
/// first code unit is neither NUL nor erased flash. Tim's radio holds one
/// 256-byte page of `FF` inside the scan-list table (0x1A400..0x1A500, where
/// his 2025 CPS file has zeros) — the radio's own flash, never written by the
/// CPS — and every tool reads that as free.
fn named_record_used(raw: &[u8]) -> bool {
    !matches!((raw[0], raw[1]), (0, 0) | (0xFF, 0xFF))
}

// --- channels ------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum Mode {
    Analog,
    Digital,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum Bandwidth {
    Narrow, // 12.5 kHz
    Mid,    // 20 kHz
    Wide,   // 25 kHz
}

/// Admit criteria (byte 4 bits 7..6).
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum Admit {
    Always,
    ChannelFree,
    Tone,
    ColorCode,
}

// Masks of the bits the typed fields own, per flag byte.
const B0_OWNED: u8 = 0b0010_1111; // mode 1..0, bandwidth 3..2, squelch 5
const B1_OWNED: u8 = 0b1111_1110; // rx-only 1, slot 3..2, colour code 7..4
const B4_OWNED: u8 = 0b1110_0000; // power 5, admit 7..6

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Channel {
    pub mode: Mode,
    pub bandwidth: Bandwidth,
    /// Squelch Normal (true) or Tight.
    pub squelch_normal: bool,
    pub rx_only: bool,
    /// 1 or 2 on a digital channel; analog channels store 0.
    pub slot: u8,
    pub color_code: u8,
    pub high_power: bool,
    pub admit: Admit,
    /// 1-based contact index, 0 = none.
    pub contact: u16,
    /// 1-based scan list, 0 = none.
    pub scan_list: u8,
    /// 1-based RX group list, 0 = none.
    pub rx_group: u8,
    pub rx_10hz: u32,
    pub tx_10hz: u32,
    pub rx_tone: Tone,
    pub tx_tone: Tone,
    pub name: String,
    /// Every byte as read; the fields above are written over it.
    pub other: [u8; 64],
}

impl Channel {
    /// A free slot: 32 bytes of `FF`, then a name of zeros — on Tim's radio, in
    /// the CPS template and in all eight factory dumps.
    pub(crate) const UNUSED: [u8; 64] = {
        let mut r = [0u8; 64];
        let mut i = 0;
        while i < 32 {
            r[i] = 0xFF;
            i += 1;
        }
        r
    };

    pub(crate) fn is_used(raw: &[u8]) -> bool {
        raw[16] != 0xFF
    }

    /// TYT's CPS "Channel1" (Farnsworth's `MD-380_400-480.rdt`), whose flag
    /// bytes match the md380tools factory dumps. Fields are overwritten.
    pub(crate) fn template() -> [u8; 64] {
        let mut t = [0u8; 64];
        t[..16].copy_from_slice(&[
            0x62, 0x14, 0x00, 0xE0, 0x24, 0xC3, 0x00, 0x00, 0x04, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0xFF,
        ]);
        t[24..32].copy_from_slice(&[0xFF, 0xFF, 0xFF, 0xFF, 0x00, 0x00, 0xFF, 0xFF]);
        t
    }

    pub(crate) fn decode(raw: &[u8]) -> Option<Channel> {
        let other: [u8; 64] = raw.try_into().ok()?;
        let mode = match raw[0] & 3 {
            1 => Mode::Analog,
            2 => Mode::Digital,
            _ => return None,
        };
        let bandwidth = match (raw[0] >> 2) & 3 {
            0 => Bandwidth::Narrow,
            1 => Bandwidth::Mid,
            2 => Bandwidth::Wide,
            _ => return None,
        };
        // Analog channels store slot 0 (all 329 on Tim's radio that are analog do).
        let slot = (raw[1] >> 2) & 3;
        Some(Channel {
            mode,
            bandwidth,
            squelch_normal: raw[0] & 0x20 != 0,
            rx_only: raw[1] & 0x02 != 0,
            slot,
            color_code: raw[1] >> 4,
            high_power: raw[4] & 0x20 != 0,
            admit: match raw[4] >> 6 {
                0 => Admit::Always,
                1 => Admit::ChannelFree,
                2 => Admit::Tone,
                _ => Admit::ColorCode,
            },
            contact: u16::from_le_bytes([raw[6], raw[7]]),
            scan_list: raw[11],
            rx_group: raw[12],
            rx_10hz: decode_freq(&raw[16..20])?,
            tx_10hz: decode_freq(&raw[20..24])?,
            rx_tone: Tone::decode([raw[24], raw[25]]),
            tx_tone: Tone::decode([raw[26], raw[27]]),
            name: decode_name(&raw[32..64]),
            other,
        })
    }

    pub(crate) fn encode(&self) -> [u8; 64] {
        let mut r = self.other;
        let mode = match self.mode {
            Mode::Analog => 1,
            Mode::Digital => 2,
        };
        let bw = match self.bandwidth {
            Bandwidth::Narrow => 0,
            Bandwidth::Mid => 1,
            Bandwidth::Wide => 2,
        };
        r[0] = (r[0] & !B0_OWNED) | mode | (bw << 2) | if self.squelch_normal { 0x20 } else { 0 };
        r[1] = (r[1] & !B1_OWNED)
            | if self.rx_only { 0x02 } else { 0 }
            | ((self.slot & 3) << 2)
            | ((self.color_code & 0xF) << 4);
        let admit = match self.admit {
            Admit::Always => 0,
            Admit::ChannelFree => 1,
            Admit::Tone => 2,
            Admit::ColorCode => 3,
        };
        r[4] = (r[4] & !B4_OWNED) | if self.high_power { 0x20 } else { 0 } | (admit << 6);
        r[6..8].copy_from_slice(&self.contact.to_le_bytes());
        r[11] = self.scan_list;
        r[12] = self.rx_group;
        encode_freq(self.rx_10hz, &mut r[16..20]);
        encode_freq(self.tx_10hz, &mut r[20..24]);
        r[24..26].copy_from_slice(&self.rx_tone.encode());
        r[26..28].copy_from_slice(&self.tx_tone.encode());
        encode_name(&self.name, &mut r[32..64]);
        r
    }
}

// --- zones ---------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Zone {
    pub name: String,
    /// 1-based channel numbers, 0 = empty.
    pub members: [u16; 16],
}

impl Zone {
    pub(crate) fn is_used(raw: &[u8]) -> bool {
        named_record_used(raw)
    }

    #[cfg(test)]
    pub(crate) fn decode(raw: &[u8]) -> Zone {
        let mut members = [0u16; 16];
        for (i, m) in members.iter_mut().enumerate() {
            *m = u16::from_le_bytes([raw[32 + 2 * i], raw[33 + 2 * i]]);
        }
        Zone { name: decode_name(&raw[..32]), members }
    }

    pub(crate) fn encode(&self) -> [u8; 64] {
        let mut r = [0u8; 64];
        encode_name(&self.name, &mut r[..32]);
        for (i, m) in self.members.iter().enumerate() {
            r[32 + 2 * i..34 + 2 * i].copy_from_slice(&m.to_le_bytes());
        }
        r
    }
}

// --- contacts ------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum CallType {
    Group,
    Private,
    /// Only ever decoded — Tim's radio has an All Call contact; the app plans none.
    #[cfg_attr(not(test), allow(dead_code))]
    All,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Contact {
    pub id: u32,
    pub call_type: CallType,
    pub name: String,
    /// Byte 3 as read: the call type is its low two bits, the rest (receive
    /// tone, bit 5; 0b11 in bits 7..6 on every record seen) is kept.
    pub flags: u8,
}

impl Contact {
    /// A free slot: `FF FF FF C0` then zeros, on Tim's radio and in the CPS.
    pub(crate) const UNUSED: [u8; 36] = {
        let mut r = [0u8; 36];
        r[0] = 0xFF;
        r[1] = 0xFF;
        r[2] = 0xFF;
        r[3] = 0xC0;
        r
    };

    pub(crate) fn is_used(raw: &[u8]) -> bool {
        raw[3] & 3 != 0
    }

    pub(crate) fn decode(raw: &[u8]) -> Option<Contact> {
        let call_type = match raw[3] & 3 {
            1 => CallType::Group,
            2 => CallType::Private,
            3 => CallType::All,
            _ => return None,
        };
        Some(Contact {
            id: u32::from_le_bytes([raw[0], raw[1], raw[2], 0]),
            call_type,
            name: decode_name(&raw[4..36]),
            flags: raw[3],
        })
    }

    pub(crate) fn encode(&self) -> [u8; 36] {
        let mut r = [0u8; 36];
        r[..3].copy_from_slice(&self.id.to_le_bytes()[..3]);
        let t = match self.call_type {
            CallType::Group => 1,
            CallType::Private => 2,
            CallType::All => 3,
        };
        r[3] = (self.flags & !3) | t;
        encode_name(&self.name, &mut r[4..36]);
        r
    }
}

// --- RX group lists ------------------------------------------------------
//
// Test-only: a program clears the table (see `program.rs`), so only the
// byte-identical gate reads these records.

#[cfg(test)]
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct RxGroup {
    pub name: String,
    /// 1-based contact indices, 0 = empty.
    pub members: [u16; 32],
}

#[cfg(test)]
impl RxGroup {
    pub(crate) fn is_used(raw: &[u8]) -> bool {
        named_record_used(raw)
    }

    pub(crate) fn decode(raw: &[u8]) -> RxGroup {
        let mut members = [0u16; 32];
        for (i, m) in members.iter_mut().enumerate() {
            *m = u16::from_le_bytes([raw[32 + 2 * i], raw[33 + 2 * i]]);
        }
        RxGroup { name: decode_name(&raw[..32]), members }
    }

    pub(crate) fn encode(&self) -> [u8; 96] {
        let mut r = [0u8; 96];
        encode_name(&self.name, &mut r[..32]);
        for (i, m) in self.members.iter().enumerate() {
            r[32 + 2 * i..34 + 2 * i].copy_from_slice(&m.to_le_bytes());
        }
        r
    }
}

// --- scan lists ----------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ScanList {
    pub name: String,
    /// 1-based channel numbers; 0 = "Selected", 0xFFFF = none.
    pub priority_1: u16,
    pub priority_2: u16,
    /// 0 = "Selected", 0xFFFF = last active channel.
    pub tx_designated: u16,
    /// 1-based channel numbers, 0 = empty.
    pub members: [u16; 31],
    /// Bytes 38..42 as read: an unknown byte, signalling hold time (×25 ms),
    /// priority sample time (×250 ms) and another unknown byte.
    pub timing: [u8; 4],
}

impl ScanList {
    /// TYT's CPS defaults for bytes 38..42: `F1`, 500 ms, 2000 ms, `FF`.
    pub(crate) const DEFAULT_TIMING: [u8; 4] = [0xF1, 0x14, 0x08, 0xFF];

    /// A free slot as the CPS and all eight factory dumps write it: no name,
    /// no priority channels, last-active TX, default timing, no members. (Tim's
    /// radio holds zeros instead — a different tool's habit, equally accepted.)
    pub(crate) const UNUSED: [u8; 104] = {
        let mut r = [0u8; 104];
        let tail = [0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xF1, 0x14, 0x08, 0xFF];
        let mut i = 0;
        while i < tail.len() {
            r[32 + i] = tail[i];
            i += 1;
        }
        r
    };

    pub(crate) fn is_used(raw: &[u8]) -> bool {
        named_record_used(raw)
    }

    #[cfg(test)]
    pub(crate) fn decode(raw: &[u8]) -> ScanList {
        let w = |o: usize| u16::from_le_bytes([raw[o], raw[o + 1]]);
        let mut members = [0u16; 31];
        for (i, m) in members.iter_mut().enumerate() {
            *m = w(42 + 2 * i);
        }
        ScanList {
            name: decode_name(&raw[..32]),
            priority_1: w(32),
            priority_2: w(34),
            tx_designated: w(36),
            members,
            timing: raw[38..42].try_into().unwrap(),
        }
    }

    pub(crate) fn encode(&self) -> [u8; 104] {
        let mut r = [0u8; 104];
        encode_name(&self.name, &mut r[..32]);
        r[32..34].copy_from_slice(&self.priority_1.to_le_bytes());
        r[34..36].copy_from_slice(&self.priority_2.to_le_bytes());
        r[36..38].copy_from_slice(&self.tx_designated.to_le_bytes());
        r[38..42].copy_from_slice(&self.timing);
        for (i, m) in self.members.iter().enumerate() {
            r[42 + 2 * i..44 + 2 * i].copy_from_slice(&m.to_le_bytes());
        }
        r
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn h(s: &str) -> Vec<u8> {
        (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap()).collect()
    }

    /// Channel #108 off Tim's radio (s136): RMH700-FOCOBUCK, DMR 445.200 /
    /// 440.200, CC7 TS1 — the values his screen showed.
    const TIM_108: &str = "227400e020001900000000000000000000005244000002440000000000000000\
                           52004d0048003700300030002d0046004f0043004f004200550043004b000000";

    #[test]
    fn tims_channel_108_decodes_to_what_his_screen_showed() {
        let raw = h(TIM_108);
        let c = Channel::decode(&raw).expect("decodes");
        assert_eq!(c.name, "RMH700-FOCOBUCK");
        assert_eq!(c.mode, Mode::Digital);
        assert_eq!((c.rx_10hz, c.tx_10hz), (44_520_000, 44_020_000));
        assert_eq!((c.color_code, c.slot), (7, 1));
        assert_eq!(c.encode().to_vec(), raw);
    }

    #[test]
    fn tones_round_trip_through_their_bcd_spelling() {
        assert_eq!(Tone::decode([0x00, 0x10]), Tone::Ctcss(1000));
        assert_eq!(Tone::Ctcss(885).encode(), [0x85, 0x08]);
        assert_eq!(Tone::decode([0x23, 0x80]), Tone::Dcs { code: 23, inverted: false });
        assert_eq!(Tone::Dcs { code: 754, inverted: true }.encode(), [0x54, 0xC7]);
        assert_eq!(Tone::decode([0, 0]).encode(), [0, 0]);
        assert_eq!(Tone::NONE.encode(), [0xFF, 0xFF]);
    }

    #[test]
    fn frequencies_are_little_endian_bcd_in_ten_hertz() {
        let mut b = [0u8; 4];
        encode_freq(mhz_to_10hz(451.525), &mut b);
        assert_eq!(b, [0x00, 0x25, 0x15, 0x45]);
        assert_eq!(decode_freq(&b), Some(45_152_500));
        assert_eq!(decode_freq(&[0xFF; 4]), None);
    }

    #[test]
    fn a_name_is_cut_at_sixteen_units_and_padded_with_nul() {
        let mut b = [0xAAu8; 32];
        encode_name("ABCDEFGHIJKLMNOPQRS", &mut b);
        assert_eq!(decode_name(&b), "ABCDEFGHIJKLMNOP");
        encode_name("Z", &mut b);
        assert_eq!(&b[..4], &[b'Z', 0, 0, 0]);
        assert!(b[2..].iter().all(|&x| x == 0));
    }

    #[test]
    fn the_template_is_a_valid_analog_or_digital_record_once_fields_are_set() {
        let mut t = Channel::template();
        encode_freq(mhz_to_10hz(446.0), &mut t[16..20]);
        encode_freq(mhz_to_10hz(446.0), &mut t[20..24]);
        let c = Channel::decode(&t).expect("template decodes");
        assert_eq!(c.mode, Mode::Digital);
        assert_eq!(c.encode(), t);
    }
}
