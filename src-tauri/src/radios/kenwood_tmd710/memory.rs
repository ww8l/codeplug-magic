//! One memory slot of a TM-D710, as the radio itself states it (issue #113).
//!
//! Live mode has no image and no file: a memory *is* the `ME` line the radio
//! prints, and programming one is sending that line back. So this module models
//! the line, and its gate is that a line read off the radio re-emits
//! **character-identically** — the live-mode equivalent of the byte-identical
//! re-encode every card radio here is held to.
//!
//! ```text
//! ME 000,0447275000,0,2,0,0,1,0,12,12,000,05000000,0,0000000000,0,0
//! MN 000,W0UPS
//! ```
//!
//! Field widths are fixed and zero-padded, and that matters: `0` and `000` are
//! the same number and **not** the same line. Everything here therefore round
//! trips through the exact text, never through a parsed number alone.
//!
//! ## What is measured and what is not
//!
//! Measured on Tim's radio on 2026-08-22 (`scratchpad/kenwood_tmd710/`):
//!
//! - the 16 fields and their widths, over 38 populated slots
//! - **`Shift::Plus` = 1 and `Shift::Minus` = 2**, cross-checked against real
//!   repeaters: 447.275 and 145.310 are minus, 147.360 is plus
//! - an **empty** slot answers [`EMPTY_REPLY`] — `N`, not an error and not a
//!   blank line. 962 of 1000 slots answered that way, with zero surprises
//!
//! ⚠ Not measured, and therefore not yet used to build a line from a channel:
//! the **tone and DCS index tables**. The captured lines carry indices (`12`,
//! `08`, `18`) whose meaning nothing here has established — a published table
//! would be a guess about what a number means, and writing a wrong tone to a
//! real repeater is the failure this project has hit most often. Building a
//! `Memory` from an app channel waits on one measurement pass.

// ⚠ Phase 2 lands the encoder before the path that will call it, so in a
// non-test build every item below is unused.
//
// A `never used` warning on an encoder is normally a **bug report** in this repo
// — it is exactly how the ID-52's dead settings-write path was found, after the
// read path had been working for weeks and hid it. So this is silenced as
// narrowly as possible, with the reason, rather than by habit: nothing here is
// reachable from the app **on purpose**, because no byte has ever been written
// to this radio and the tone tables are unmeasured. The moment a capability
// trait calls into this module, this attribute comes out and the warning
// becomes meaningful again.
#![cfg_attr(not(test), allow(dead_code))]

/// What the radio answers for a slot with nothing in it. Measured, not assumed.
pub(crate) const EMPTY_REPLY: &str = "N";

/// The radio's longest memory name — Menu 200, "up to 8 characters", and the
/// longest in the capture is exactly 8 (`FNL TOWE`, spaces included).
pub(crate) const MAX_NAME: usize = 8;

/// Repeater shift, as field 4 encodes it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Shift {
    Simplex,
    Plus,
    Minus,
    /// Transmit on field 14's frequency instead of an offset. Present in
    /// CHIRP's table; **not** seen in the capture, so it is carried through
    /// verbatim rather than acted on.
    Split,
}

impl Shift {
    fn from_field(f: &str) -> Result<Self, String> {
        match f {
            "0" => Ok(Shift::Simplex),
            "1" => Ok(Shift::Plus),
            "2" => Ok(Shift::Minus),
            "3" => Ok(Shift::Split),
            other => Err(format!("unknown shift {other:?} in field 4")),
        }
    }

    fn field(self) -> &'static str {
        match self {
            Shift::Simplex => "0",
            Shift::Plus => "1",
            Shift::Minus => "2",
            Shift::Split => "3",
        }
    }
}

/// A memory slot, one member per `ME` parameter, in the radio's own order.
///
/// Fields whose meaning is not yet established are kept as the **text the radio
/// sent**. That is deliberate: a value carried through verbatim cannot be
/// corrupted by a wrong guess about what it means, and a slot can be read,
/// stored and written back long before every field is understood.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Memory {
    pub slot: u16,
    pub rx_hz: u64,
    pub step: String,
    pub shift: Shift,
    pub reverse: String,
    pub tone_on: String,
    pub ctcss_on: String,
    pub dcs_on: String,
    pub tone_idx: String,
    pub ctcss_idx: String,
    pub dcs_idx: String,
    pub offset_hz: u64,
    pub mode: String,
    pub tx_hz: u64,
    pub tx_step: String,
    pub lockout: String,
}

impl Memory {
    /// Parse one `ME` reply.
    ///
    /// Strict on purpose. A line with the wrong number of fields is a different
    /// firmware or a different radio, and guessing which fields moved is how a
    /// driver writes a plausible-looking wrong value. `N` — an empty slot — is
    /// not a memory and is refused here rather than parsed into a blank one.
    pub(crate) fn parse(line: &str) -> Result<Self, String> {
        if line == EMPTY_REPLY {
            return Err("empty slot".into());
        }
        let body = line
            .strip_prefix("ME ")
            .ok_or_else(|| format!("not an ME reply: {line:?}"))?;
        let f: Vec<&str> = body.split(',').collect();
        if f.len() != 16 {
            return Err(format!(
                "expected 16 fields in an ME reply, got {}: {line:?}",
                f.len()
            ));
        }
        let width = |i: usize, want: usize| -> Result<&str, String> {
            if f[i].len() == want {
                Ok(f[i])
            } else {
                Err(format!(
                    "field {} is {:?}, expected {want} characters",
                    i + 1,
                    f[i]
                ))
            }
        };
        let num = |i: usize, want: usize| -> Result<u64, String> {
            width(i, want)?
                .parse::<u64>()
                .map_err(|e| format!("field {} is not a number: {e}", i + 1))
        };

        Ok(Memory {
            slot: num(0, 3)? as u16,
            rx_hz: num(1, 10)?,
            step: width(2, 1)?.into(),
            shift: Shift::from_field(width(3, 1)?)?,
            reverse: width(4, 1)?.into(),
            tone_on: width(5, 1)?.into(),
            ctcss_on: width(6, 1)?.into(),
            dcs_on: width(7, 1)?.into(),
            tone_idx: width(8, 2)?.into(),
            ctcss_idx: width(9, 2)?.into(),
            dcs_idx: width(10, 3)?.into(),
            offset_hz: num(11, 8)?,
            mode: width(12, 1)?.into(),
            tx_hz: num(13, 10)?,
            tx_step: width(14, 1)?.into(),
            lockout: width(15, 1)?.into(),
        })
    }

    /// Emit the `ME` line. Widths are the radio's, not Rust's defaults — see
    /// the module doc on why `0` and `000` are not interchangeable here.
    pub(crate) fn to_line(&self) -> String {
        format!(
            "ME {:03},{:010},{},{},{},{},{},{},{},{},{},{:08},{},{:010},{},{}",
            self.slot,
            self.rx_hz,
            self.step,
            self.shift.field(),
            self.reverse,
            self.tone_on,
            self.ctcss_on,
            self.dcs_on,
            self.tone_idx,
            self.ctcss_idx,
            self.dcs_idx,
            self.offset_hz,
            self.mode,
            self.tx_hz,
            self.tx_step,
            self.lockout
        )
    }
}

/// A memory's name, which the radio keeps in a separate command from the
/// memory itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MemoryName {
    pub slot: u16,
    pub text: String,
}

impl MemoryName {
    pub(crate) fn parse(line: &str) -> Result<Self, String> {
        let body = line
            .strip_prefix("MN ")
            .ok_or_else(|| format!("not an MN reply: {line:?}"))?;
        let (slot, text) = body
            .split_once(',')
            .ok_or_else(|| format!("no name field in {line:?}"))?;
        if slot.len() != 3 {
            return Err(format!("slot {slot:?} is not 3 digits"));
        }
        Ok(MemoryName {
            slot: slot.parse().map_err(|e| format!("slot: {e}"))?,
            text: text.to_string(),
        })
    }

    pub(crate) fn to_line(&self) -> String {
        format!("MN {:03},{}", self.slot, self.text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Real lines off Tim's TM-D710A, 2026-08-22. Kept verbatim: the point of
    /// the gate is that these exact characters survive a round trip, so a
    /// tidied-up copy would test nothing. (Repeater frequencies and call signs
    /// are public record.)
    const REAL: &[(&str, &str)] = &[
        (
            "ME 000,0447275000,0,2,0,0,1,0,12,12,000,05000000,0,0000000000,0,0",
            "MN 000,W0UPS",
        ),
        (
            "ME 007,0147360000,0,1,0,0,1,0,12,12,000,00600000,0,0000000000,0,0",
            "MN 007,W0QEY",
        ),
        (
            "ME 009,0145310000,0,2,0,0,1,0,08,18,000,00600000,0,0000000000,0,0",
            "MN 009,KB0VJJ",
        ),
        (
            "ME 005,0224840000,0,2,0,0,1,0,12,12,000,01600000,0,0000000000,0,0",
            "MN 005,W0UPS",
        ),
    ];

    /// ★ The Phase 2 gate. A memory read off the radio must come back out as
    /// the identical line — the live-mode form of the byte-identical re-encode
    /// that has caught a real bug on every radio in this project.
    #[test]
    fn a_real_memory_re_emits_character_identically() {
        for (me, mn) in REAL {
            let parsed = Memory::parse(me).unwrap_or_else(|e| panic!("{me}: {e}"));
            assert_eq!(&parsed.to_line(), me);
            let name = MemoryName::parse(mn).unwrap_or_else(|e| panic!("{mn}: {e}"));
            assert_eq!(&name.to_line(), mn);
        }
    }

    /// The shift decode, checked against what the repeaters actually are rather
    /// than against the documentation that describes them.
    #[test]
    fn shift_matches_the_real_repeaters() {
        // 447.275 UHF, 5 MHz down.
        let uhf = Memory::parse(REAL[0].0).unwrap();
        assert_eq!(uhf.shift, Shift::Minus);
        assert_eq!(uhf.offset_hz, 5_000_000);
        // 147.360, 600 kHz up — the one plus-shift channel in the capture.
        let vhf = Memory::parse(REAL[1].0).unwrap();
        assert_eq!(vhf.shift, Shift::Plus);
        assert_eq!(vhf.offset_hz, 600_000);
        // 224.840, 1.6 MHz down — the 220 band's own offset.
        let band220 = Memory::parse(REAL[3].0).unwrap();
        assert_eq!(band220.shift, Shift::Minus);
        assert_eq!(band220.offset_hz, 1_600_000);
    }

    /// Tone and CTCSS are separate fields with separate indices, so a driver
    /// that reads one into both would corrupt this slot. ME 009 is the proof:
    /// the two differ.
    #[test]
    fn tone_and_ctcss_indices_are_independent() {
        let m = Memory::parse(REAL[2].0).unwrap();
        assert_eq!(m.tone_idx, "08");
        assert_eq!(m.ctcss_idx, "18");
        assert_ne!(m.tone_idx, m.ctcss_idx);
    }

    /// An empty slot is `N`, and it is not a memory. Refusing it here is what
    /// stops 962 of Tim's 1000 slots turning into blank channels.
    #[test]
    fn an_empty_slot_is_refused_rather_than_parsed_blank() {
        let err = Memory::parse(EMPTY_REPLY).unwrap_err();
        assert!(err.contains("empty"), "{err}");
    }

    /// Strictness, field by field: a short line, a wrong-width field and an
    /// unknown shift are all refused with the field named. A driver that
    /// shrugs these off writes a plausible wrong value to a real radio.
    #[test]
    fn a_malformed_line_is_refused_and_says_which_field() {
        assert!(Memory::parse("ME 000,0447275000,0").unwrap_err().contains("16 fields"));
        let short_slot = "ME 00,0447275000,0,2,0,0,1,0,12,12,000,05000000,0,0000000000,0,0";
        assert!(Memory::parse(short_slot).unwrap_err().contains("field 1"));
        let bad_shift = "ME 000,0447275000,0,9,0,0,1,0,12,12,000,05000000,0,0000000000,0,0";
        assert!(Memory::parse(bad_shift).unwrap_err().contains("shift"));
        assert!(Memory::parse("MN 000,W0UPS").unwrap_err().contains("not an ME"));
    }

    /// Zero padding is not cosmetic. Slot 7 is `007`, and an offset of 600 kHz
    /// is eight characters — a driver that emitted `7` or `600000` would send a
    /// line the radio parses differently.
    #[test]
    fn widths_are_preserved_not_normalised() {
        let m = Memory::parse(REAL[1].0).unwrap();
        assert_eq!(m.slot, 7);
        let line = m.to_line();
        assert!(line.starts_with("ME 007,"), "{line}");
        assert!(line.contains(",00600000,"), "{line}");
    }

    /// Names can carry a space and can be the full 8 characters, so neither
    /// trimming nor a shorter cap is safe.
    #[test]
    fn a_name_keeps_its_spaces_and_its_full_width() {
        let n = MemoryName::parse("MN 012,FNL TOWE").unwrap();
        assert_eq!(n.text, "FNL TOWE");
        assert_eq!(n.text.len(), MAX_NAME);
        assert_eq!(n.to_line(), "MN 012,FNL TOWE");
    }

    /// The whole capture, when it is on this machine. Gitignored, so this is a
    /// no-op in CI and on anyone else's checkout — the four lines above are the
    /// part that always runs. See the `test-the-gate-against-real-files` note:
    /// the real corpus answers questions a handful of samples cannot.
    #[test]
    fn every_captured_memory_re_emits_identically() {
        let Ok(text) = std::fs::read_to_string("../scratchpad/kenwood_tmd710/memories.txt") else {
            return;
        };
        let mut checked = 0;
        for line in text.lines().filter(|l| !l.starts_with('#')) {
            let round_tripped = if line.starts_with("ME ") {
                Memory::parse(line).unwrap_or_else(|e| panic!("{line}: {e}")).to_line()
            } else {
                MemoryName::parse(line).unwrap_or_else(|e| panic!("{line}: {e}")).to_line()
            };
            assert_eq!(round_tripped, line);
            checked += 1;
        }
        assert!(checked >= 76, "expected the 38 captured slots, saw {checked} lines");
    }
}
