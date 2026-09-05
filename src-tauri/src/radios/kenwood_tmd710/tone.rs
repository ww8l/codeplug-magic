//! The TM-D710's CTCSS and DCS tables, and the conversions in and out of them.
//!
//! Fields 9, 10 and 11 of an `ME` line (`tone_idx`, `ctcss_idx`, `dcs_idx`) are
//! **indices**, not values. That was the open question at the end of the Phase 1
//! campaign — the field is three characters wide and `023` is both a plausible
//! index and a real DCS code — and the radio settled it without anyone reading
//! its screen.
//!
//! ## How the index question was answered, over the cable
//!
//! This radio **validates a write and refuses it whole**: a rejected `ME` line
//! leaves the slot exactly as it was, which makes acceptance a measurement.
//! Written to a slot that was empty, and read back (session 126):
//!
//! | `dcs_idx` written | valid DCS code? | valid index? | radio |
//! |---|---|---|---|
//! | `754` | yes — the last one | no, 754 > 103 | **refused**, slot stayed `N` |
//! | `103` | no | yes | **accepted** |
//! | `104` | no | no | **refused**, slot kept `103` |
//!
//! A field that takes `103` and refuses `754` is an index. The same pair run on
//! fields 9 and 10 puts both at `0..=41`: `41` accepted, `42` refused, on each
//! independently. So the counts are exactly **42 tones and 104 DCS codes**, and
//! the indices are **0-based** — `00` is accepted, which a 1-based field could
//! not do.
//!
//! ## Where the tables themselves come from
//!
//! The lists below are the manual's own (TM-D710GA/GE Instruction Manual
//! V1.01, SIGNALING-1 and SIGNALING-2). ⚠ That manual covers the **G**; Tim's
//! radio is the non-G TM-D710A. The two share this table — the cable-measured
//! lengths above match it exactly, 42 and 104, which is the check that matters —
//! but see the `research-before-reverse-engineering` note: a manual for the
//! wrong model has bitten this project before.
//!
//! The manual prints the CTCSS list with keypad reference numbers `01`~`42`,
//! which is **display numbering, not the stored index** (the TH-D75 taught this
//! the hard way). The 0-based offset is not taken from the manual: session 120
//! joined the radio's own 38 memories to Tim's channel library on frequency
//! **and** callsign and found field 9 predicting the library's TX tone **33
//! right, 0 wrong** under "0-based index into this list". That pins the offset
//! and the interior of the table independently of anything printed.
//!
//! ## ⚠ Nothing here is called yet
//!
//! Every item below carries `allow(dead_code)` because the Phase 2 encoder —
//! the thing that builds an `ME` line from a library channel — does not exist
//! yet. The attribute goes away with it. It is spelled out because a
//! `never used` warning on an encoder is normally a **bug report**: on the
//! D890UV a whole settings write path sat unreferenced behind a working read
//! path and nobody noticed. See `read-path-working-hides-a-dead-write-path`.
//!
//! ## What is still unconfirmed
//!
//! Not one of the radio's 38 memories uses DCS, so **the DCS list has no
//! cross-check** — its order is the manual's reading order, and the fact that it
//! is byte-identical to the 104-code list the ID-52 driver already ships. The
//! decisive screen reading is memory 503, which holds `dcs_idx = 023`: as an
//! index that is the 24th code, **D134**. Any other displayed value means this
//! table is not the radio's.

/// CTCSS tones in tenths of a hertz, in the order the radio indexes them.
///
/// Index 0 is 67.0 Hz and index 41 is 254.1 Hz — the classic 42-tone list, with
/// none of the extra tones the ID-52's 50-entry table carries.
#[cfg_attr(not(test), allow(dead_code))] // see the module doc
pub(crate) const TONES_DHZ: [u16; 42] = [
    670, 693, 719, 744, 770, 797, 825, 854, 885, 915, 948, 974, 1000, 1035, 1072, 1109, 1148, 1188,
    1230, 1273, 1318, 1365, 1413, 1462, 1514, 1567, 1622, 1679, 1738, 1799, 1862, 1928, 2035, 2065,
    2107, 2181, 2257, 2291, 2336, 2418, 2503, 2541,
];

/// DCS codes as the radio indexes them, written in octal the way the front
/// panel shows them — which is also how the channel database stores them.
///
/// Index 0 is `023` and index 103 is `754`.
#[cfg_attr(not(test), allow(dead_code))] // see the module doc
pub(crate) const DCS_CODES: [u16; 104] = [
    23, 25, 26, 31, 32, 36, 43, 47, 51, 53, 54, 65, 71, 72, 73, 74, 114, 115, 116, 122, 125, 131,
    132, 134, 143, 145, 152, 155, 156, 162, 165, 172, 174, 205, 212, 223, 225, 226, 243, 244, 245,
    246, 251, 252, 255, 261, 263, 265, 266, 271, 274, 306, 311, 315, 325, 331, 332, 343, 346, 351,
    356, 364, 365, 371, 411, 412, 413, 423, 431, 432, 445, 446, 452, 454, 455, 462, 464, 465, 466,
    503, 506, 516, 523, 526, 532, 546, 565, 606, 612, 624, 627, 631, 632, 654, 662, 664, 703, 712,
    723, 731, 732, 734, 743, 754,
];

/// The `ME` field for a tone this radio does not have.
///
/// Deliberately an error rather than a nearest-match: the channel library is
/// radio-agnostic and holds tones from radios with longer tables, and silently
/// moving an operator's 159.8 Hz to 156.7 Hz would program a channel that
/// cannot open the repeater it names. The caller decides — drop the tone, or
/// refuse the channel — the way `ChannelFit` already decides about bands.
#[cfg_attr(not(test), allow(dead_code))] // see the module doc
pub(crate) fn tone_field(hz: f64) -> Result<String, String> {
    let dhz = (hz * 10.0).round() as u16;
    let idx = TONES_DHZ
        .iter()
        .position(|&t| t == dhz)
        .ok_or_else(|| format!("the TM-D710 has no CTCSS tone {hz:.1} Hz"))?;
    Ok(format!("{idx:02}"))
}

/// The tone an `ME` field names, in hertz.
#[cfg_attr(not(test), allow(dead_code))] // see the module doc
pub(crate) fn tone_hz(field: &str) -> Result<f64, String> {
    let idx: usize = field
        .parse()
        .map_err(|_| format!("tone index {field:?} is not a number"))?;
    TONES_DHZ
        .get(idx)
        .map(|&d| f64::from(d) / 10.0)
        .ok_or_else(|| format!("tone index {idx} is past the radio's 42-tone table"))
}

/// The `ME` field for a DCS code, given as the octal digits the database holds.
#[cfg_attr(not(test), allow(dead_code))] // see the module doc
pub(crate) fn dcs_field(code: &str) -> Result<String, String> {
    let n: u16 = code
        .parse()
        .map_err(|_| format!("DCS code {code:?} is not a number"))?;
    let idx = DCS_CODES
        .iter()
        .position(|&c| c == n)
        .ok_or_else(|| format!("{code} is not a DCS code the TM-D710 has"))?;
    Ok(format!("{idx:03}"))
}

/// The DCS code an `ME` field names, zero-padded the way the panel shows it.
#[cfg_attr(not(test), allow(dead_code))] // see the module doc
pub(crate) fn dcs_code(field: &str) -> Result<String, String> {
    let idx: usize = field
        .parse()
        .map_err(|_| format!("DCS index {field:?} is not a number"))?;
    DCS_CODES
        .get(idx)
        .map(|&c| format!("{c:03}"))
        .ok_or_else(|| format!("DCS index {idx} is past the radio's 104-code table"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The lengths are not a style choice — they are what the radio accepted and
    /// refused. `42` and `104` are the first index each field rejected, so a
    /// table that grew or shrank would be writing a value the radio bounces.
    #[test]
    fn the_tables_are_exactly_as_long_as_the_radio_allows() {
        assert_eq!(TONES_DHZ.len(), 42, "field 9 refused index 42 on hardware");
        assert_eq!(DCS_CODES.len(), 104, "field 11 refused index 104 on hardware");
    }

    /// Both lists ascend, and a duplicate would make `position()` unreachable
    /// for the later copy — a value that encodes to one index and decodes to
    /// another.
    #[test]
    fn both_tables_ascend_with_no_repeats() {
        assert!(
            TONES_DHZ.windows(2).all(|w| w[0] < w[1]),
            "the CTCSS table is not strictly ascending"
        );
        assert!(
            DCS_CODES.windows(2).all(|w| w[0] < w[1]),
            "the DCS table is not strictly ascending"
        );
    }

    /// The four boundary values that were measured on the radio, written the way
    /// an `ME` line carries them — two characters for a tone, three for DCS.
    #[test]
    fn the_measured_boundaries_encode_to_the_fields_the_radio_took() {
        assert_eq!(tone_field(67.0).unwrap(), "00");
        assert_eq!(tone_field(254.1).unwrap(), "41");
        assert_eq!(dcs_field("023").unwrap(), "000");
        assert_eq!(dcs_field("754").unwrap(), "103");
    }

    /// ★ The one claim a cable cannot check. Memory 503 holds `dcs_idx = 023`;
    /// read as an index that is the 24th code. If the radio's screen shows
    /// anything but D134 for that memory, this table is wrong — see the module
    /// doc.
    #[test]
    fn dcs_index_023_is_the_code_134() {
        assert_eq!(dcs_code("023").unwrap(), "134");
    }

    /// Round-tripping is what the encoder relies on: a tone read off the radio
    /// and written straight back must land on the same field.
    #[test]
    fn every_entry_round_trips_through_its_field() {
        for (i, &dhz) in TONES_DHZ.iter().enumerate() {
            let hz = f64::from(dhz) / 10.0;
            let field = tone_field(hz).expect("encode");
            assert_eq!(field, format!("{i:02}"));
            assert_eq!(tone_hz(&field).expect("decode"), hz);
        }
        for (i, &code) in DCS_CODES.iter().enumerate() {
            let text = format!("{code:03}");
            let field = dcs_field(&text).expect("encode");
            assert_eq!(field, format!("{i:03}"));
            assert_eq!(dcs_code(&field).expect("decode"), text);
        }
    }

    /// A tone this radio does not have is an error, not the nearest one it does.
    /// 159.8 Hz is in the ID-52's table and not in this one, so it is exactly
    /// the case a shared channel library produces.
    #[test]
    fn a_tone_the_radio_lacks_is_refused_rather_than_rounded() {
        let err = tone_field(159.8).unwrap_err();
        assert!(err.contains("159.8"), "{err}");
        assert!(dcs_field("024").is_err(), "024 is not a DCS code");
        assert!(tone_hz("42").is_err(), "the radio refused index 42");
        assert!(dcs_code("104").is_err(), "the radio refused index 104");
    }
}
