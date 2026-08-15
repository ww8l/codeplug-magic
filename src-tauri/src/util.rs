//! Pure helpers for deriving channel attributes shared by manual entry,
//! CSV import, and export.

/// Derive the amateur band label from an RX frequency in MHz.
pub fn derive_band(freq: f64) -> &'static str {
    match freq {
        f if f < 30.0 => "HF",
        f if f < 174.0 => "VHF",
        f if f < 300.0 => "220",
        f if f < 700.0 => "UHF",
        f if f < 1000.0 => "900",
        _ => "UHF",
    }
}

/// Derive duplex direction and a positive offset (MHz) from RX/TX frequencies.
/// Returns (duplex, offset) where duplex is one of "+", "-", "none", "split".
pub fn derive_duplex(rx_freq: f64, tx_freq: Option<f64>) -> (String, f64) {
    match tx_freq {
        None => ("none".to_string(), 0.0),
        Some(tx) => {
            let diff = tx - rx_freq;
            if diff.abs() < 0.0001 {
                ("none".to_string(), 0.0)
            } else if diff.abs() > 15.0 {
                // Unusually large separation -> treat as an odd/cross-band split.
                ("split".to_string(), diff.abs())
            } else if diff > 0.0 {
                ("+".to_string(), diff)
            } else {
                ("-".to_string(), diff.abs())
            }
        }
    }
}

/// Canonical whole-kHz repeater offsets (MHz) recognized for the band that
/// `rx` (an RX/output frequency in MHz) falls in. Used to undo RepeaterBook's
/// 3-decimal truncation of the input/TX frequency (see [`repair_truncated_tx`]).
pub(crate) fn standard_offsets(rx: f64) -> &'static [f64] {
    match rx {
        f if f < 30.0 => &[0.1],          // 10m
        f if f < 54.0 => &[0.5, 1.0],     // 6m
        f if f < 148.0 => &[0.6],         // 2m
        f if f < 225.0 => &[1.6],         // 1.25m
        f if f < 450.0 => &[5.0],         // 70cm
        f if f < 470.0 => &[5.0],         // GMRS / UHF
        f if f < 928.0 => &[12.0, 25.0],  // 33cm
        f if f < 1300.0 => &[12.0, 20.0], // 23cm
        _ => &[],
    }
}

/// Round to 4 decimal places (100 Hz), the resolution of an amateur frequency.
fn round_4(f: f64) -> f64 {
    (f * 10_000.0).round() / 10_000.0
}

/// Undo RepeaterBook's 3-decimal truncation of the input/TX frequency.
///
/// RepeaterBook exports the output frequency at 4 decimals (e.g. 446.8625) but
/// the input/TX frequency at only 3 (441.863 instead of 441.8625), so a
/// standard-offset repeater arrives with an offset that is up to half a kHz
/// short (4.9995 instead of 5.0). Real amateur offsets are whole kHz, so when
/// the derived offset lands within a 3-decimal rounding error of a canonical
/// band offset we snap it and rebuild the TX frequency at full precision from
/// `rx ± offset`. Anything that isn't a recognized standard offset (a genuine
/// odd split) and any non-repeater duplex are returned unchanged.
///
/// Returns the (possibly corrected) `(tx_freq, offset)`.
pub fn repair_truncated_tx(
    rx: f64,
    tx: Option<f64>,
    duplex: &str,
    offset: f64,
) -> (Option<f64>, f64) {
    // 3-decimal rounding can move a frequency by at most 0.5 kHz; the small
    // slack absorbs binary float error in the derived offset.
    const SNAP_TOL: f64 = 0.0005 + 1e-9;
    let sign = match duplex {
        "+" => 1.0,
        "-" => -1.0,
        _ => return (tx, offset),
    };
    match standard_offsets(rx)
        .iter()
        .copied()
        .find(|&std| (offset - std).abs() <= SNAP_TOL)
    {
        Some(std) => (Some(round_4(rx + sign * std)), std),
        None => (tx, offset),
    }
}

/// Truncate a string to `max` chars (char-aware), trimming trailing whitespace.
pub fn truncate(s: &str, max: usize) -> String {
    s.trim().chars().take(max).collect::<String>().trim_end().to_string()
}

/// Generate the long (<=16 char) display name from callsign + city.
pub fn gen_name_long(callsign: &str, city: &str) -> String {
    let combined = format!("{} {}", callsign.trim(), city.trim());
    truncate(combined.trim(), 16)
}

/// Generate the short (<=7 char) display name from the callsign.
pub fn gen_name_short(callsign: &str) -> String {
    truncate(callsign, 7)
}

/// Tidy a D-STAR call sign the way it will be stored: trimmed, upper case, and
/// blank collapsed to None. Kept out of the drivers so the channel library holds
/// one canonical spelling and every radio packs the same text.
pub fn normalize_dstar_call(text: Option<&str>) -> Option<String> {
    let t = text?.trim().to_uppercase();
    (!t.is_empty()).then_some(t)
}

/// Lay a D-STAR call sign out the way the radios store it: the module letter in
/// the **eighth** character, `W0QEY  B`.
///
/// This is the JARL convention, not one vendor's — Icom and Kenwood write the
/// same eight bytes — so it lives here rather than in either driver, and the
/// channel library stores what the operator typed ("W0QEY B") rather than a
/// padded string nobody wants to read in a grid.
///
/// A single trailing letter after a space is the module (or, in `Your
/// Callsign`, a reflector command: `REF030 C` and `REF030CL` are both real
/// things people type). Anything else is taken literally, which is what makes
/// `CQCQCQ`, `DIRECT`, `REF030CL` and `/W0QEYB` all come out right.
///
/// ⚠ **It pads to position 8, it does not pad to length 8.** A call with a
/// module needs the interior spaces or the module lands in the wrong byte; a
/// bare `CQCQCQ` needs nothing after it, because the radios fill the rest of
/// the field with NUL and a space-padded `CQCQCQ  ` is a shape a TH-D75 never
/// writes. Trailing padding belongs to whichever driver owns the field. Caught
/// by the ground-truth re-encode against a real radio save, which is exactly
/// the class of thing it exists for.
pub fn pack_dstar_call(text: &str) -> String {
    let t: String = text.trim().to_uppercase().chars().take(8).collect();
    let mut parts = t.split_whitespace();
    let (Some(call), Some(module)) = (parts.next(), parts.next_back()) else {
        return t;
    };
    // Two words where the second is not a lone letter is not the call+module
    // shape; hand it over as typed and let the radio judge it.
    if module.chars().count() != 1 || parts.next().is_some() {
        return t;
    }
    format!("{:<7.7}{}", call, module)
}

#[cfg(test)]
mod dstar_tests {
    use super::*;

    /// The five D-STAR memories a real TH-D75 wrote, and the two ways an
    /// operator would type each of them. Both have to reach the same bytes:
    /// the radio's own file is the only thing that decides what is correct.
    #[test]
    fn packs_a_call_sign_the_way_the_radio_writes_it() {
        for typed in ["W0QEY B", "w0qey b", "W0QEY  B"] {
            assert_eq!(pack_dstar_call(typed), "W0QEY  B", "{typed}");
        }
        assert_eq!(pack_dstar_call("KC0DS C"), "KC0DS  C");
        assert_eq!(pack_dstar_call("WW8L B"), "WW8L   B");
        assert_eq!(pack_dstar_call("W0QEY G"), "W0QEY  G");
        // ⚠ NOT padded. A TH-D75 writes `CQCQCQ` followed by NUL, so returning
        // `CQCQCQ  ` here put spaces into 91 of the radio's own memories and
        // the ground-truth re-encode failed on every one of them.
        assert_eq!(pack_dstar_call("CQCQCQ"), "CQCQCQ");
        assert_eq!(pack_dstar_call("DIRECT"), "DIRECT");
        // Reflector commands and call-sign routing are the reason `Your
        // Callsign` had to become a stored field at all.
        assert_eq!(pack_dstar_call("REF030CL"), "REF030CL");
        assert_eq!(pack_dstar_call("REF030 C"), "REF030 C");
        assert_eq!(pack_dstar_call("/W0QEYB"), "/W0QEYB");
        // A module always lands in the eighth character, and nothing ever
        // overruns the field.
        for s in ["W0QEY B", "CQCQCQ", "REF030CL", "A VERY LONG THING", ""] {
            assert!(pack_dstar_call(s).len() <= 8, "{s}");
        }
        assert_eq!(pack_dstar_call("W0QEY B").len(), 8);
    }

    #[test]
    fn normalizing_drops_blanks_and_upper_cases() {
        assert_eq!(normalize_dstar_call(Some("  w0qey b ")).as_deref(), Some("W0QEY B"));
        assert_eq!(normalize_dstar_call(Some("   ")), None);
        assert_eq!(normalize_dstar_call(None), None);
    }
}
