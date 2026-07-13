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
fn standard_offsets(rx: f64) -> &'static [f64] {
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
