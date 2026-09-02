//! Building an `ME` line from a channel in the app's library (issue #113, Phase 2).
//!
//! [`memory`](super::memory) models the line the *radio* prints. This module is
//! the other direction, and it is where every field's legal range has to be
//! known rather than carried through verbatim — a value the radio refuses does
//! not error loudly, it just leaves the slot as it was.
//!
//! ## Everything below was measured on the radio, not inherited
//!
//! The TM-D710 **validates a write and refuses it whole**, so acceptance is a
//! measurement: sweep a field on an empty slot, read back, and the first refused
//! value is the edge of the enum. `d710_field_bounds` is that instrument.
//!
//! | field | measured |
//! |---|---|
//! | 3 step | ten values, and **which** ten depends on the frequency — see [`step_field`] |
//! | 4 shift | `0`/`1`/`2` only. **There is no `3`** |
//! | 5, 6, 7, 8, 16 | `0`/`1` |
//! | 9, 10 tone | `0..=41` |
//! | 11 DCS | `0..=103` |
//! | 12 offset | any value up to **29 950 000 Hz**; 29 955 000 refused |
//! | 13 mode | `0`/`1`/`2` |
//! | 14 tx | an absolute frequency, and **mutually exclusive** with 4 and 12 |
//! | 15 tx step | the same table as field 3, against field 14's frequency |
//!
//! ## ★ The shift field has no split value
//!
//! CHIRP's table lists `3` as split and [`Shift::Split`](super::memory::Shift)
//! was written from it. The radio refuses `3` — with a zero offset, with a
//! 600 kHz offset, and with a TX frequency present. What it *does* accept is
//! shift `0`, offset `0`, and an absolute frequency in field 14; setting a shift
//! or an offset *and* field 14 together is refused in every combination tried.
//! So an odd split is field 14 and nothing else. That is the fourth published
//! claim about this radio to die on contact with it.
//!
//! ⚠ Accepted and stored is not the same as *transmits there*. Every memory in
//! the radio's own capture has field 14 zero, so nothing has ever confirmed the
//! split on the air. It is graded accordingly in `FINDINGS.md`.
//!
//! ## ★ A step that does not divide the frequency is refused
//!
//! The strongest result of the campaign, and an encoder constraint rather than a
//! curiosity: field 3 accepted a *non-contiguous* set of values, different at
//! every frequency, and in each case exactly the steps that divide it evenly.
//! Across 146.520, 145.000, 145.050 and TX 146.820, the table below predicted
//! **40 accept/refuse results with no misses**, which is also what pins index 9
//! as 50 kHz rather than 100 kHz — 145.050 divides by the first and not the
//! second.
//!
//! A driver that emitted a fixed step would produce memories the radio quietly
//! declines to store.
//!
//! ## What the caller owes the operator
//!
//! Several channels in a radio-agnostic library cannot be expressed here at all:
//! a tone this radio does not have, an offset over 29.95 MHz. Those come back as
//! `Err`, and the program flow must turn them into a **named skip**, the way
//! `ChannelFit` already reports a band it cannot reach. Failing the whole write
//! would be worse, and encoding a nearby value would be worse still — see
//! `radio-tx-vs-rx-bands` for what a silently-wrong memory costs.

// ⚠ Phase 2 lands the encoder before the path that will call it — the same
// note as `memory.rs` and `tone.rs`, for the same reason. See
// `read-path-working-hides-a-dead-write-path`: an "unused" warning on an
// encoder is normally a bug report, so it is silenced with a reason rather
// than by habit, and comes out when a capability trait calls in.
#![cfg_attr(not(test), allow(dead_code))]

use super::memory::{Memory, MemoryName, Shift, MAX_NAME};
use super::tone;
use crate::commands::export;
use crate::models::Channel;

/// Tuning steps in hertz, in the order field 3 indexes them.
///
/// Ten entries, because the field is **one character wide**: whatever a
/// TH-D72-style eleventh entry (100 kHz) would be, it cannot be written here.
/// Index 9 is 50 kHz, pinned at 145.050 MHz — divisible by 50 kHz and not by
/// 100 kHz, and accepted.
pub(crate) const STEPS_HZ: [u64; 10] = [
    5_000, 6_250, 8_330, 10_000, 12_500, 15_000, 20_000, 25_000, 30_000, 50_000,
];

/// Index 2. Never *chosen*, only round-tripped: a real 8.33 kHz air-band channel
/// is not an integer multiple of 8330 Hz, so offering it would produce a step
/// the radio refuses. The TH-D72 driver skips its own index 2 for the same
/// reason.
const STEP_833: usize = 2;

/// Field 12's ceiling. 29 950 000 accepted, 29 955 000 refused — and the same on
/// both bands, which is worth stating because a 60 MHz UHF ceiling would have
/// been the natural guess.
pub(crate) const MAX_OFFSET_HZ: u64 = 29_950_000;

/// Field 13. `2` is pinned by the radio's own 118.400 MHz memory — air band is
/// AM — and `0` by the 37 FM repeaters beside it. The field accepts exactly
/// three values and the radio offers exactly three modes (Menu 102: AM, FM,
/// NFM), so `1` is NFM **by elimination**, not by reading the menu's order.
const MODE_FM: &str = "0";
const MODE_NFM: &str = "1";
const MODE_AM: &str = "2";

/// What the radio itself puts in fields 9 and 10 on a memory with no tone at
/// all: index 8, 88.5 Hz. Both of the capture's two toneless memories carry
/// `08,08`, so this is the radio's habit rather than a convenient zero.
const DEFAULT_TONE_FIELD: &str = "08";

/// Field 11 on every one of the radio's 38 memories. None of them uses DCS.
const DEFAULT_DCS_FIELD: &str = "000";

/// The comma is the `ME`/`MN` field separator, and the radio rewrites it to `+`
/// rather than refusing the name. Measured over the whole printable range: 94 of
/// 95 characters survive a round trip verbatim — **lowercase included** — and
/// this is the only one that does not.
const NAME_COMMA_REPLACEMENT: char = '+';

/// The lowest-indexed step that divides `hz` exactly.
///
/// Not a preference — a requirement. The radio refuses a step that does not
/// divide the frequency, and refuses it *silently*, leaving the slot unwritten.
pub(crate) fn step_field(hz: u64) -> Result<String, String> {
    STEPS_HZ
        .iter()
        .enumerate()
        .filter(|&(i, _)| i != STEP_833)
        .find(|&(_, &step)| hz.is_multiple_of(step))
        .map(|(i, _)| i.to_string())
        .ok_or_else(|| {
            format!(
                "{:.5} MHz is not a multiple of any tuning step the TM-D710 has, so the radio \
                 would refuse the memory",
                hz as f64 / 1_000_000.0
            )
        })
}

/// Hertz from the megahertz the database stores.
fn mhz_to_hz(mhz: f64) -> u64 {
    (mhz * 1_000_000.0).round() as u64
}

/// Build the `ME` line's contents for one channel.
///
/// `Err` means the radio cannot hold this channel as described — a tone it does
/// not have, an offset past its ceiling — and the caller is expected to skip the
/// channel with the reason shown, never to substitute a nearby value.
pub(crate) fn encode_channel(slot: u16, c: &Channel) -> Result<Memory, String> {
    let rx_hz = mhz_to_hz(c.rx_freq);
    let tx_hz = mhz_to_hz(export::tx_frequency(c));

    // Field 14 or fields 4+12, never both — measured, in every combination.
    let split = c.duplex.as_deref() == Some("split");
    let (shift, offset_hz, split_tx_hz) = if split {
        (Shift::Simplex, 0, tx_hz)
    } else if tx_hz > rx_hz {
        (Shift::Plus, tx_hz - rx_hz, 0)
    } else if tx_hz < rx_hz {
        (Shift::Minus, rx_hz - tx_hz, 0)
    } else {
        (Shift::Simplex, 0, 0)
    };
    if offset_hz > MAX_OFFSET_HZ {
        return Err(format!(
            "a {:.3} MHz repeater shift is past the TM-D710's {:.2} MHz limit; the radio would \
             refuse the memory",
            offset_hz as f64 / 1_000_000.0,
            MAX_OFFSET_HZ as f64 / 1_000_000.0
        ));
    }

    let mode = match c.mode.as_deref() {
        // The radio is analog-only. A digital channel reaching here at all is a
        // question for the model's exclusion rules, not for the encoder, so it
        // lands on FM the way every other analog driver here treats one.
        Some(m) if m.eq_ignore_ascii_case("NFM") => MODE_NFM,
        Some(m) if m.eq_ignore_ascii_case("AM") => MODE_AM,
        _ => MODE_FM,
    };

    // All eight flag combinations are accepted by the radio — it enforces
    // nothing here — so exactly one is set on purpose. Cross Tone exists on this
    // radio (Menu: Tone, CTCSS, DCS, Cross Tone) but **how it is stored has not
    // been measured**, and the repo's rule for a radio that cannot express a
    // cross tone is to keep the transmit tone and drop the receive one. That is
    // strictly what a guessed flag pair might not do.
    let requested = c.tone_mode.as_deref().unwrap_or("");
    let (tone_on, ctcss_on, dcs_on) = if requested.eq_ignore_ascii_case("TSQL") {
        ("0", "1", "0")
    } else if requested.eq_ignore_ascii_case("DTCS") {
        ("0", "0", "1")
    } else if requested.eq_ignore_ascii_case("Tone") || requested.eq_ignore_ascii_case("Cross") {
        ("1", "0", "0")
    } else {
        ("0", "0", "0")
    };

    // Both tone fields are always populated, which is what the radio does even
    // on a memory with no tone at all.
    let tone_idx = match c.ctcss_uplink.or(c.ctcss_downlink) {
        Some(hz) => tone::tone_field(hz)?,
        None => DEFAULT_TONE_FIELD.to_string(),
    };
    let ctcss_idx = match c.ctcss_downlink.or(c.ctcss_uplink) {
        Some(hz) => tone::tone_field(hz)?,
        None => DEFAULT_TONE_FIELD.to_string(),
    };
    let dcs_idx = match c.dcs_code.as_deref().filter(|s| !s.is_empty()) {
        Some(code) => tone::dcs_field(code)?,
        None => DEFAULT_DCS_FIELD.to_string(),
    };

    Ok(Memory {
        slot,
        rx_hz,
        step: step_field(rx_hz)?,
        shift,
        reverse: "0".into(),
        tone_on: tone_on.into(),
        ctcss_on: ctcss_on.into(),
        dcs_on: dcs_on.into(),
        tone_idx,
        ctcss_idx,
        dcs_idx,
        offset_hz,
        mode: mode.into(),
        tx_hz: split_tx_hz,
        // Measured: with field 14 zero the radio accepts only `0` here, and with
        // a TX frequency present it accepts exactly that frequency's steps.
        tx_step: if split {
            step_field(split_tx_hz)?
        } else {
            "0".into()
        },
        lockout: "0".into(),
    })
}

/// Turn a memory the radio printed back into the channel it describes, so
/// the encoder can be asked to rebuild the same line.
///
/// This is the decode the app does not otherwise need — the D710 driver
/// reads memories as text — and it exists only to close the loop. Anything
/// it cannot express is a real gap in the mapping, which is the point.
pub(crate) fn decode_channel(m: &Memory) -> Channel {
    let mhz = |hz: u64| hz as f64 / 1_000_000.0;
    let (duplex, offset, tx_freq) = match m.shift {
        Shift::Plus => (Some("+"), Some(mhz(m.offset_hz)), None),
        Shift::Minus => (Some("-"), Some(mhz(m.offset_hz)), None),
        // Field 4 has no split value — an absolute TX frequency in field 14 is
        // how this radio expresses one. See the module doc.
        _ if m.tx_hz != 0 => (Some("split"), None, Some(mhz(m.tx_hz))),
        _ => (None, None, None),
    };
    Channel {
        rx_freq: mhz(m.rx_hz),
        duplex: duplex.map(str::to_string),
        offset,
        tx_freq,
        mode: Some(
            match m.mode.as_str() {
                "1" => "NFM",
                "2" => "AM",
                _ => "FM",
            }
            .into(),
        ),
        tone_mode: if m.tone_on == "1" {
            Some("Tone".into())
        } else if m.ctcss_on == "1" {
            Some("TSQL".into())
        } else if m.dcs_on == "1" {
            Some("DTCS".into())
        } else {
            None
        },
        ctcss_uplink: Some(tone::tone_hz(&m.tone_idx).expect("tone index off the table")),
        ctcss_downlink: Some(tone::tone_hz(&m.ctcss_idx).expect("ctcss index off the table")),
        dcs_code: Some(tone::dcs_code(&m.dcs_idx).expect("dcs index off the table")),
        ..Default::default()
    }
}

/// The name to send with `MN`, sanitised and cut to what the radio keeps.
///
/// Both limits are measured rather than read off Menu 200: a ninth character is
/// **silently truncated**, not refused, and a comma is silently rewritten. Doing
/// both here means [`write_name`](super::write_name)'s read-back check stays a
/// real check — otherwise every name over eight characters would fail it.
pub(crate) fn encode_name(slot: u16, c: &Channel) -> MemoryName {
    let source = c
        .name_short
        .as_deref()
        .or(c.name_long.as_deref())
        .or(c.callsign.as_deref())
        .unwrap_or("");
    MemoryName {
        slot,
        text: sanitize_name(source),
    }
}

/// Cut and clean any string into what the radio will keep verbatim.
///
/// Split out because the program path names channels with `expanded_name` — the
/// app's own disambiguated name, which is what the export preview shows — and
/// that string needs exactly the same treatment. Two places doing this
/// differently is two different names on the radio for the same channel.
pub(crate) fn sanitize_name(source: &str) -> String {
    source
        .chars()
        .map(|ch| if ch == ',' { NAME_COMMA_REPLACEMENT } else { ch })
        .take(MAX_NAME)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn channel(rx: f64) -> Channel {
        Channel {
            rx_freq: rx,
            ..Default::default()
        }
    }

    /// ★ The step rule, stated as the radio stated it. These are the exact
    /// frequencies `d710_field_bounds` was run at, and the exact sets it came
    /// back with — so a change to `STEPS_HZ` that broke the table would fail
    /// here rather than on the radio.
    #[test]
    fn the_step_table_reproduces_what_the_radio_accepted() {
        let accepted = |hz: u64| -> Vec<usize> {
            STEPS_HZ
                .iter()
                .enumerate()
                .filter(|&(_, &s)| hz.is_multiple_of(s))
                .map(|(i, _)| i)
                .collect()
        };
        assert_eq!(accepted(146_520_000), vec![0, 3, 5, 6, 8]);
        assert_eq!(accepted(145_000_000), vec![0, 1, 3, 4, 6, 7, 9]);
        assert_eq!(accepted(145_050_000), vec![0, 1, 3, 4, 5, 7, 8, 9]);
        assert_eq!(accepted(146_820_000), vec![0, 3, 5, 6, 8]);
    }

    /// A step is chosen, never assumed: 5 kHz where it divides, and the first
    /// one that does otherwise. 8.33 is skipped even where it would divide.
    #[test]
    fn a_step_is_picked_that_actually_divides_the_frequency() {
        assert_eq!(step_field(146_520_000).unwrap(), "0");
        assert_eq!(step_field(145_006_250).unwrap(), "1");
        assert!(step_field(8_330).unwrap_err().contains("tuning step"));
    }

    /// A plain minus-shift repeater, field by field, against a line shaped like
    /// the ones the radio itself prints.
    #[test]
    fn a_minus_shift_repeater_encodes_the_way_the_radio_writes_one() {
        let mut c = channel(447.275);
        c.duplex = Some("-".into());
        c.offset = Some(5.0);
        c.tone_mode = Some("TSQL".into());
        c.ctcss_uplink = Some(100.0);
        c.ctcss_downlink = Some(100.0);
        let m = encode_channel(0, &c).unwrap();
        assert_eq!(
            m.to_line(),
            "ME 000,0447275000,0,2,0,0,1,0,12,12,000,05000000,0,0000000000,0,0"
        );
    }

    /// ★ An odd split is field 14 alone. The shift stays `0` and the offset
    /// stays zero, because the radio refuses any combination of the two with a
    /// TX frequency present — and refuses shift `3` outright.
    #[test]
    fn a_split_puts_the_tx_frequency_in_field_14_and_nothing_in_the_shift() {
        let mut c = channel(146.520);
        c.duplex = Some("split".into());
        c.tx_freq = Some(146.820);
        let m = encode_channel(7, &c).unwrap();
        assert_eq!(m.shift, Shift::Simplex);
        assert_eq!(m.offset_hz, 0);
        assert_eq!(m.tx_hz, 146_820_000);
        // 5 kHz divides 146.820, so the lowest listed step wins — the same
        // rule field 3 follows, applied to field 14's frequency.
        assert_eq!(m.tx_step, "0");
        assert_eq!(
            m.to_line(),
            "ME 007,0146520000,0,0,0,0,0,0,08,08,000,00000000,0,0146820000,0,0"
        );
    }

    /// Nothing the encoder can be handed may produce field 4 = `3`. The variant
    /// still exists so a line carrying one can be *read*, but the radio refused
    /// it in every base tried and this driver must never emit it.
    #[test]
    fn no_channel_shape_encodes_the_split_shift_the_radio_refuses() {
        for (duplex, tx) in [
            (Some("split"), Some(146.820)),
            (Some("+"), None),
            (Some("-"), None),
            (None, None),
            (Some("split"), Some(146.520)),
        ] {
            let mut c = channel(146.520);
            c.duplex = duplex.map(str::to_string);
            c.tx_freq = tx;
            c.offset = Some(0.6);
            let m = encode_channel(0, &c).unwrap();
            assert_ne!(m.shift, Shift::Split, "{duplex:?}/{tx:?} produced shift 3");
            assert!(
                m.tx_hz == 0 || (m.offset_hz == 0 && m.shift == Shift::Simplex),
                "{duplex:?}/{tx:?} set a TX frequency and a shift/offset together, which the \
                 radio refuses: {}",
                m.to_line()
            );
        }
    }

    /// The measured ceiling, and the reason it is an error rather than a clamp:
    /// a clamped shift transmits on the wrong frequency.
    #[test]
    fn an_offset_past_the_radios_ceiling_is_refused_not_clamped() {
        let mut c = channel(146.520);
        c.duplex = Some("+".into());
        c.offset = Some(30.0);
        let err = encode_channel(0, &c).unwrap_err();
        assert!(err.contains("29.95"), "{err}");

        let mut ok = channel(146.520);
        ok.duplex = Some("+".into());
        ok.offset = Some(29.95);
        assert_eq!(encode_channel(0, &ok).unwrap().offset_hz, 29_950_000);
    }

    /// A toneless memory still carries both tone fields, filled the way the
    /// radio fills them.
    #[test]
    fn a_channel_with_no_tone_gets_the_radios_own_default_indices() {
        let m = encode_channel(0, &channel(146.520)).unwrap();
        assert_eq!((m.tone_on.as_str(), m.ctcss_on.as_str(), m.dcs_on.as_str()), ("0", "0", "0"));
        assert_eq!(m.tone_idx, "08");
        assert_eq!(m.ctcss_idx, "08");
        assert_eq!(m.dcs_idx, "000");
    }

    /// Exactly one flag, never a combination — the radio accepts all eight and
    /// enforces none, so this is the encoder's job alone.
    #[test]
    fn exactly_one_tone_flag_is_ever_set() {
        for mode in ["Tone", "TSQL", "DTCS", "Cross", "", "nonsense"] {
            let mut c = channel(146.520);
            c.tone_mode = Some(mode.into());
            c.ctcss_uplink = Some(100.0);
            c.dcs_code = Some("023".into());
            let m = encode_channel(0, &c).unwrap();
            let on = [&m.tone_on, &m.ctcss_on, &m.dcs_on]
                .iter()
                .filter(|f| f.as_str() == "1")
                .count();
            assert!(on <= 1, "{mode:?} set {on} flags: {}", m.to_line());
        }
    }

    /// Cross falls back to the transmit tone rather than guessing a flag pair.
    #[test]
    fn cross_keeps_the_transmit_tone_instead_of_guessing() {
        let mut c = channel(146.520);
        c.tone_mode = Some("Cross".into());
        c.ctcss_uplink = Some(123.0);
        c.ctcss_downlink = Some(100.0);
        let m = encode_channel(0, &c).unwrap();
        assert_eq!(m.tone_on, "1");
        assert_eq!(m.tone_idx, tone::tone_field(123.0).unwrap());
    }

    /// A tone the radio does not have stops this channel and names it, so the
    /// caller can skip it. It must not become the nearest tone it does have.
    #[test]
    fn a_tone_off_the_radios_table_stops_the_channel_with_a_reason() {
        let mut c = channel(146.520);
        c.tone_mode = Some("Tone".into());
        c.ctcss_uplink = Some(159.8);
        let err = encode_channel(0, &c).unwrap_err();
        assert!(err.contains("159.8"), "{err}");
    }

    /// Both name limits, both measured: eight characters, and the one character
    /// the radio rewrites instead of refusing.
    #[test]
    fn a_name_is_cut_and_the_comma_replaced_before_the_radio_does_it() {
        let mut c = channel(146.520);
        c.name_short = Some("DENVER, CO".into());
        assert_eq!(encode_name(3, &c).text, "DENVER+ ");
        assert_eq!(encode_name(3, &c).to_line(), "MN 003,DENVER+ ");

        c.name_short = None;
        c.name_long = None;
        c.callsign = Some("W0UPS".into());
        assert_eq!(encode_name(3, &c).text, "W0UPS");

        c.callsign = None;
        assert_eq!(encode_name(3, &c).text, "");
    }

    /// Whatever the encoder builds must survive the round trip the radio's own
    /// lines are held to — widths included, since `0` and `000` are different
    /// lines.
    #[test]
    fn everything_encoded_re_parses_to_itself() {
        for (rx, duplex, offset, mode, tone) in [
            (146.520, None, None, None, None),
            (447.275, Some("-"), Some(5.0), Some("FM"), Some("TSQL")),
            (145.006_25, Some("+"), Some(0.6), Some("NFM"), Some("Tone")),
            (118.400, None, None, Some("AM"), None),
            (146.520, Some("split"), None, None, Some("DTCS")),
        ] {
            let mut c = channel(rx);
            c.duplex = duplex.map(str::to_string);
            c.offset = offset;
            c.mode = mode.map(str::to_string);
            c.tone_mode = tone.map(str::to_string);
            c.ctcss_uplink = Some(100.0);
            c.dcs_code = Some("023".into());
            if duplex == Some("split") {
                c.tx_freq = Some(146.820);
            }
            let m = encode_channel(42, &c).unwrap();
            let line = m.to_line();
            assert_eq!(Memory::parse(&line).unwrap().to_line(), line, "{line}");
        }
    }
}

#[cfg(test)]
mod round_trip {
    use super::*;
    use crate::radios::kenwood_tmd710::memory::Memory;

    /// ★ **The Phase 2 gate, in the direction that matters.**
    ///
    /// `memory.rs` proves a line the radio printed comes back out unchanged.
    /// That tests the text, not the meaning. This decodes each real memory into
    /// app terms and asks the *encoder* to rebuild it — so a field mapped the
    /// wrong way round, a tone table off by one, or a shift encoded as an
    /// offset all fail here rather than on the radio.
    ///
    /// Three fields are excluded, each named rather than smoothed over:
    ///
    /// - **field 3, the step.** The radio accepts any step that divides the
    ///   frequency, so the one it happens to hold is not the only right answer:
    ///   two of Tim's memories carry 25 kHz where the encoder picks 5 kHz, and
    ///   both are legal. Field 15 goes with it.
    /// - **field 16, the lockout.** The app has no per-channel scan lockout to
    ///   round-trip through.
    /// - **field 12 on a simplex memory.** ★ This one the gate found. Memory
    ///   040 — 144.390, the APRS calling frequency — is shift `0` and still
    ///   carries a 600 kHz offset, so the radio keeps the offset field
    ///   independently of whether the shift uses it. It is residue from an
    ///   earlier edit, there is nothing in a channel record that could hold it,
    ///   and the encoder writing `00000000` there is correct: measured, the
    ///   radio accepts a zero offset with a zero shift.
    fn rebuild_diff(me: &str) -> Option<String> {
        let original = Memory::parse(me).unwrap_or_else(|e| panic!("{me}: {e}"));
        let rebuilt = match encode_channel(original.slot, &decode_channel(&original)) {
            Ok(m) => m,
            Err(e) => return Some(format!("{me}\n    encoder refused it: {e}")),
        };

        let simplex = original.shift == Shift::Simplex && original.tx_hz == 0;
        let normalise = |m: &Memory| Memory {
            step: "-".into(),
            tx_step: "-".into(),
            lockout: "-".into(),
            offset_hz: if simplex { 0 } else { m.offset_hz },
            ..m.clone()
        };
        (normalise(&rebuilt).to_line() != normalise(&original).to_line()).then(|| {
            format!("{me}\n    rebuilt: {}", rebuilt.to_line())
        })
    }

    fn assert_all_rebuild(lines: impl Iterator<Item = String>) -> usize {
        let (mut checked, mut bad) = (0, Vec::new());
        for me in lines {
            checked += 1;
            if let Some(d) = rebuild_diff(&me) {
                bad.push(d);
            }
        }
        assert!(
            bad.is_empty(),
            "{} of {checked} memories did not rebuild:\n  {}",
            bad.len(),
            bad.join("\n  ")
        );
        checked
    }

    /// The four lines that always run, matching `memory.rs`'s own sample: a
    /// UHF minus, a VHF plus, a memory whose two tone fields differ, and a
    /// 220 MHz repeater.
    #[test]
    fn the_sample_memories_rebuild_from_their_own_contents() {
        let n = assert_all_rebuild(
            [
                "ME 000,0447275000,0,2,0,0,1,0,12,12,000,05000000,0,0000000000,0,0",
                "ME 007,0147360000,0,1,0,0,1,0,12,12,000,00600000,0,0000000000,0,0",
                "ME 009,0145310000,0,2,0,0,1,0,08,18,000,00600000,0,0000000000,0,0",
                "ME 005,0224840000,0,2,0,0,1,0,12,12,000,01600000,0,0000000000,0,0",
            ]
            .iter()
            .map(|s| s.to_string()),
        );
        assert_eq!(n, 4);
    }

    /// The same, against every memory on the radio, when the gitignored capture
    /// is on this machine — 38 real ones including the 118.400 MHz AM air-band
    /// memory and the two on a 25 kHz step. A no-op in CI.
    #[test]
    fn every_captured_memory_rebuilds_from_its_own_contents() {
        let Ok(text) = std::fs::read_to_string("../scratchpad/kenwood_tmd710/memories.txt") else {
            return;
        };
        let checked =
            assert_all_rebuild(text.lines().filter(|l| l.starts_with("ME ")).map(str::to_string));
        assert!(checked >= 30, "only {checked} memories in the capture");
    }
}
