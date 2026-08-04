//! Built-in "standard" channel lists.
//!
//! Unlike repeaters, the regulated radio services have channel plans fixed by
//! rule — GMRS, FRS and MURS by FCC Part 95, CB by Part 95 Subpart D, marine
//! VHF by Part 80, and the NOAA weather channels by NWS allocation. There is
//! nothing to look up and nothing to import from a file, so the plans live here
//! as static tables and a user seeds them with one click.
//!
//! The tables describe each *service*, not any one radio: every channel the
//! service defines is listed, with the frequency, mode and power the rules
//! prescribe. Whether a given radio may legally transmit there (a ham HT on
//! marine VHF, say) is the radio's business — `exclusion_reason` already drops
//! out-of-band channels per model at export time.

use serde::Serialize;
use sqlx::{Acquire, SqlitePool};
use tauri::State;

use crate::db::AppState;
use crate::error::MapErrString;
use crate::util::{derive_band, derive_duplex};

// ============================================================
// Catalog types
// ============================================================

/// One channel of a built-in list, as authored in the tables below.
struct StdChannel {
    /// Long, descriptive name — used when the radio has room for it.
    name: &'static str,
    /// Short name, kept to **7 characters**: the tightest `max_name_length` of
    /// any model we support, so it lands intact on every radio.
    short: &'static str,
    /// Receive frequency, MHz.
    rx: f64,
    /// Transmit frequency, MHz. `None` marks a receive-only channel.
    tx: Option<f64>,
    mode: &'static str,
    power: Option<&'static str>,
    notes: &'static str,
}

/// A whole service's channel plan.
struct StdList {
    /// Stable slug the UI passes back to `import_standard_list`.
    id: &'static str,
    /// Short label ("GMRS").
    name: &'static str,
    /// Spelled-out service name ("General Mobile Radio Service").
    full_name: &'static str,
    description: &'static str,
    /// Goes into each channel's `service_type` column.
    service_type: &'static str,
    channels: &'static [StdChannel],
}

// Power-level shorthands, matching POWER_LEVELS in the UI. `ANY` means the
// service sets no ceiling worth pinning — leave it to the radio profile.
const HIGH: Option<&'static str> = Some("High");
const MED: Option<&'static str> = Some("Med");
const LOW: Option<&'static str> = Some("Low");
const ANY: Option<&'static str> = None;

/// Simplex channel — transmit and receive on the same frequency.
const fn sx(
    name: &'static str,
    short: &'static str,
    freq: f64,
    mode: &'static str,
    power: Option<&'static str>,
    notes: &'static str,
) -> StdChannel {
    StdChannel { name, short, rx: freq, tx: Some(freq), mode, power, notes }
}

/// Duplex pair — `rx` is what the radio hears, `tx` what it sends.
const fn dx(
    name: &'static str,
    short: &'static str,
    rx: f64,
    tx: f64,
    mode: &'static str,
    power: Option<&'static str>,
    notes: &'static str,
) -> StdChannel {
    StdChannel { name, short, rx, tx: Some(tx), mode, power, notes }
}

/// Receive-only channel — the service itself forbids voice here.
const fn ro(
    name: &'static str,
    short: &'static str,
    freq: f64,
    mode: &'static str,
    notes: &'static str,
) -> StdChannel {
    StdChannel { name, short, rx: freq, tx: None, mode, power: ANY, notes }
}

// ============================================================
// FRS — FCC Part 95 Subpart B
// ============================================================

/// Channels 1-7 and 15-22 allow 2 W ERP in 20 kHz; the interstitial channels
/// 8-14 are capped at 0.5 W in 12.5 kHz, hence NFM and Low.
static FRS: &[StdChannel] = &[
    sx("FRS 1", "FRS 1", 462.5625, "FM", LOW, "2 W; shared with GMRS 1"),
    sx("FRS 2", "FRS 2", 462.5875, "FM", LOW, "2 W; shared with GMRS 2"),
    sx("FRS 3", "FRS 3", 462.6125, "FM", LOW, "2 W; shared with GMRS 3"),
    sx("FRS 4", "FRS 4", 462.6375, "FM", LOW, "2 W; shared with GMRS 4"),
    sx("FRS 5", "FRS 5", 462.6625, "FM", LOW, "2 W; shared with GMRS 5"),
    sx("FRS 6", "FRS 6", 462.6875, "FM", LOW, "2 W; shared with GMRS 6"),
    sx("FRS 7", "FRS 7", 462.7125, "FM", LOW, "2 W; shared with GMRS 7"),
    sx("FRS 8", "FRS 8", 467.5625, "NFM", LOW, "0.5 W, narrowband"),
    sx("FRS 9", "FRS 9", 467.5875, "NFM", LOW, "0.5 W, narrowband"),
    sx("FRS 10", "FRS 10", 467.6125, "NFM", LOW, "0.5 W, narrowband"),
    sx("FRS 11", "FRS 11", 467.6375, "NFM", LOW, "0.5 W, narrowband"),
    sx("FRS 12", "FRS 12", 467.6625, "NFM", LOW, "0.5 W, narrowband"),
    sx("FRS 13", "FRS 13", 467.6875, "NFM", LOW, "0.5 W, narrowband"),
    sx("FRS 14", "FRS 14", 467.7125, "NFM", LOW, "0.5 W, narrowband"),
    sx("FRS 15", "FRS 15", 462.5500, "FM", LOW, "2 W; shared with GMRS 15"),
    sx("FRS 16", "FRS 16", 462.5750, "FM", LOW, "2 W; shared with GMRS 16"),
    sx("FRS 17", "FRS 17", 462.6000, "FM", LOW, "2 W; shared with GMRS 17"),
    sx("FRS 18", "FRS 18", 462.6250, "FM", LOW, "2 W; shared with GMRS 18"),
    sx("FRS 19", "FRS 19", 462.6500, "FM", LOW, "2 W; shared with GMRS 19"),
    sx("FRS 20", "FRS 20", 462.6750, "FM", LOW, "2 W; shared with GMRS 20"),
    sx("FRS 21", "FRS 21", 462.7000, "FM", LOW, "2 W; shared with GMRS 21"),
    sx("FRS 22", "FRS 22", 462.7250, "FM", LOW, "2 W; shared with GMRS 22"),
];

// ============================================================
// GMRS — FCC Part 95 Subpart E (license required)
// ============================================================

/// 22 simplex channels plus the 8 repeater pairs. The repeater pairs reuse the
/// channel 15-22 frequencies for receive and transmit 5 MHz up, so they are
/// separate channels rather than a flag on the simplex ones.
static GMRS: &[StdChannel] = &[
    sx("GMRS 1", "GMRS 1", 462.5625, "FM", MED, "5 W; shared with FRS 1"),
    sx("GMRS 2", "GMRS 2", 462.5875, "FM", MED, "5 W; shared with FRS 2"),
    sx("GMRS 3", "GMRS 3", 462.6125, "FM", MED, "5 W; shared with FRS 3"),
    sx("GMRS 4", "GMRS 4", 462.6375, "FM", MED, "5 W; shared with FRS 4"),
    sx("GMRS 5", "GMRS 5", 462.6625, "FM", MED, "5 W; shared with FRS 5"),
    sx("GMRS 6", "GMRS 6", 462.6875, "FM", MED, "5 W; shared with FRS 6"),
    sx("GMRS 7", "GMRS 7", 462.7125, "FM", MED, "5 W; shared with FRS 7"),
    sx("GMRS 8", "GMRS 8", 467.5625, "NFM", LOW, "0.5 W, narrowband"),
    sx("GMRS 9", "GMRS 9", 467.5875, "NFM", LOW, "0.5 W, narrowband"),
    sx("GMRS 10", "GMRS 10", 467.6125, "NFM", LOW, "0.5 W, narrowband"),
    sx("GMRS 11", "GMRS 11", 467.6375, "NFM", LOW, "0.5 W, narrowband"),
    sx("GMRS 12", "GMRS 12", 467.6625, "NFM", LOW, "0.5 W, narrowband"),
    sx("GMRS 13", "GMRS 13", 467.6875, "NFM", LOW, "0.5 W, narrowband"),
    sx("GMRS 14", "GMRS 14", 467.7125, "NFM", LOW, "0.5 W, narrowband"),
    sx("GMRS 15", "GMRS 15", 462.5500, "FM", HIGH, "50 W simplex"),
    sx("GMRS 16", "GMRS 16", 462.5750, "FM", HIGH, "50 W simplex"),
    sx("GMRS 17", "GMRS 17", 462.6000, "FM", HIGH, "50 W simplex"),
    sx("GMRS 18", "GMRS 18", 462.6250, "FM", HIGH, "50 W simplex"),
    sx("GMRS 19", "GMRS 19", 462.6500, "FM", HIGH, "50 W simplex"),
    sx("GMRS 20", "GMRS 20", 462.6750, "FM", HIGH, "50 W simplex"),
    sx("GMRS 21", "GMRS 21", 462.7000, "FM", HIGH, "50 W simplex"),
    sx("GMRS 22", "GMRS 22", 462.7250, "FM", HIGH, "50 W simplex"),
    dx("GMRS Repeater 15", "RPT15", 462.5500, 467.5500, "FM", HIGH, "+5 MHz repeater pair"),
    dx("GMRS Repeater 16", "RPT16", 462.5750, 467.5750, "FM", HIGH, "+5 MHz repeater pair"),
    dx("GMRS Repeater 17", "RPT17", 462.6000, 467.6000, "FM", HIGH, "+5 MHz repeater pair"),
    dx("GMRS Repeater 18", "RPT18", 462.6250, 467.6250, "FM", HIGH, "+5 MHz repeater pair"),
    dx("GMRS Repeater 19", "RPT19", 462.6500, 467.6500, "FM", HIGH, "+5 MHz repeater pair"),
    dx("GMRS Repeater 20", "RPT20", 462.6750, 467.6750, "FM", HIGH, "+5 MHz repeater pair"),
    dx("GMRS Repeater 21", "RPT21", 462.7000, 467.7000, "FM", HIGH, "+5 MHz repeater pair"),
    dx("GMRS Repeater 22", "RPT22", 462.7250, 467.7250, "FM", HIGH, "+5 MHz repeater pair"),
];

// ============================================================
// MURS — FCC Part 95 Subpart J (no license required)
// ============================================================

/// 2 W on all five. Channels 1-3 are limited to 11.25 kHz (narrowband);
/// 4 and 5 — the "Blue Dot" and "Green Dot" business channels — allow 20 kHz.
static MURS: &[StdChannel] = &[
    sx("MURS 1", "MURS 1", 151.8200, "NFM", LOW, "2 W, narrowband"),
    sx("MURS 2", "MURS 2", 151.8800, "NFM", LOW, "2 W, narrowband"),
    sx("MURS 3", "MURS 3", 151.9400, "NFM", LOW, "2 W, narrowband"),
    sx("MURS 4 Blue Dot", "MURS 4", 154.5700, "FM", LOW, "2 W; \"Blue Dot\""),
    sx("MURS 5 Green Dot", "MURS 5", 154.6000, "FM", LOW, "2 W; \"Green Dot\""),
];

// ============================================================
// CB — FCC Part 95 Subpart D (no license required)
// ============================================================

/// The 40 AM channels, 4 W carrier. Note the well-known kink: channel 23 sits
/// at 27.255 MHz, *above* channels 24 and 25 — that is the real allocation,
/// not a typo, and radios number them this way.
static CB: &[StdChannel] = &[
    sx("CB 1", "CB 1", 26.9650, "AM", ANY, ""),
    sx("CB 2", "CB 2", 26.9750, "AM", ANY, ""),
    sx("CB 3", "CB 3", 26.9850, "AM", ANY, ""),
    sx("CB 4", "CB 4", 27.0050, "AM", ANY, "Common off-road / 4x4 channel"),
    sx("CB 5", "CB 5", 27.0150, "AM", ANY, ""),
    sx("CB 6", "CB 6", 27.0250, "AM", ANY, ""),
    sx("CB 7", "CB 7", 27.0350, "AM", ANY, ""),
    sx("CB 8", "CB 8", 27.0550, "AM", ANY, ""),
    sx("CB 9", "CB 9", 27.0650, "AM", ANY, "Emergency / traveler assistance"),
    sx("CB 10", "CB 10", 27.0750, "AM", ANY, ""),
    sx("CB 11", "CB 11", 27.0850, "AM", ANY, ""),
    sx("CB 12", "CB 12", 27.1050, "AM", ANY, ""),
    sx("CB 13", "CB 13", 27.1150, "AM", ANY, "Marine / RV common"),
    sx("CB 14", "CB 14", 27.1250, "AM", ANY, ""),
    sx("CB 15", "CB 15", 27.1350, "AM", ANY, ""),
    sx("CB 16", "CB 16", 27.1550, "AM", ANY, ""),
    sx("CB 17", "CB 17", 27.1650, "AM", ANY, ""),
    sx("CB 18", "CB 18", 27.1750, "AM", ANY, ""),
    sx("CB 19", "CB 19", 27.1850, "AM", ANY, "Highway / trucker channel"),
    sx("CB 20", "CB 20", 27.2050, "AM", ANY, ""),
    sx("CB 21", "CB 21", 27.2150, "AM", ANY, ""),
    sx("CB 22", "CB 22", 27.2250, "AM", ANY, ""),
    sx("CB 23", "CB 23", 27.2550, "AM", ANY, "Out of sequence — above 24 and 25"),
    sx("CB 24", "CB 24", 27.2350, "AM", ANY, ""),
    sx("CB 25", "CB 25", 27.2450, "AM", ANY, ""),
    sx("CB 26", "CB 26", 27.2650, "AM", ANY, ""),
    sx("CB 27", "CB 27", 27.2750, "AM", ANY, ""),
    sx("CB 28", "CB 28", 27.2850, "AM", ANY, ""),
    sx("CB 29", "CB 29", 27.2950, "AM", ANY, ""),
    sx("CB 30", "CB 30", 27.3050, "AM", ANY, ""),
    sx("CB 31", "CB 31", 27.3150, "AM", ANY, ""),
    sx("CB 32", "CB 32", 27.3250, "AM", ANY, ""),
    sx("CB 33", "CB 33", 27.3350, "AM", ANY, ""),
    sx("CB 34", "CB 34", 27.3450, "AM", ANY, ""),
    sx("CB 35", "CB 35", 27.3550, "AM", ANY, ""),
    sx("CB 36", "CB 36", 27.3650, "AM", ANY, "SSB calling (upper channels)"),
    sx("CB 37", "CB 37", 27.3750, "AM", ANY, ""),
    sx("CB 38", "CB 38", 27.3850, "AM", ANY, "LSB calling by convention"),
    sx("CB 39", "CB 39", 27.3950, "AM", ANY, ""),
    sx("CB 40", "CB 40", 27.4050, "AM", ANY, ""),
];

// ============================================================
// Marine VHF — FCC Part 80, US channel assignments
// ============================================================

/// The US marine plan. Channel numbers ending in "A" are the US simplex use of
/// an international duplex channel — the radio transmits and receives on the
/// ship half only. The handful of true duplex channels (public correspondence
/// and port operations) receive on 161-162 MHz and transmit 4.6 MHz down.
/// 1 W channels are marked Low; the rest allow the full 25 W.
static MARINE: &[StdChannel] = &[
    sx("Marine 01A Port Ops", "MAR 01A", 156.0500, "FM", HIGH, "Port operations / VTS"),
    sx("Marine 05A Port Ops", "MAR 05A", 156.2500, "FM", HIGH, "Port operations / VTS"),
    sx("Marine 06 Safety", "MAR 06", 156.3000, "FM", HIGH, "Intership safety — required on every set"),
    sx("Marine 07A Commercial", "MAR 07A", 156.3500, "FM", HIGH, "Commercial"),
    sx("Marine 08 Commercial", "MAR 08", 156.4000, "FM", HIGH, "Commercial, intership only"),
    sx("Marine 09 Calling", "MAR 09", 156.4500, "FM", HIGH, "Boater calling channel"),
    sx("Marine 10 Commercial", "MAR 10", 156.5000, "FM", HIGH, "Commercial"),
    sx("Marine 11 VTS", "MAR 11", 156.5500, "FM", HIGH, "Commercial / VTS"),
    sx("Marine 12 Port Ops", "MAR 12", 156.6000, "FM", HIGH, "Port operations / VTS"),
    sx("Marine 13 Bridge", "MAR 13", 156.6500, "FM", LOW, "Bridge-to-bridge navigation safety, 1 W"),
    sx("Marine 14 Port Ops", "MAR 14", 156.7000, "FM", HIGH, "Port operations / VTS"),
    ro("Marine 15 Environmental", "MAR 15", 156.7500, "FM", "Receive only — environmental broadcasts"),
    sx("Marine 16 Distress", "MAR 16", 156.8000, "FM", HIGH, "Distress, safety and calling"),
    sx("Marine 17 State Control", "MAR 17", 156.8500, "FM", LOW, "State and local government, 1 W"),
    sx("Marine 18A Commercial", "MAR 18A", 156.9000, "FM", HIGH, "Commercial"),
    sx("Marine 19A Commercial", "MAR 19A", 156.9500, "FM", HIGH, "Commercial"),
    dx("Marine 20 Port Ops", "MAR 20", 161.6000, 157.0000, "FM", HIGH, "Port operations, duplex"),
    sx("Marine 20A Port Ops", "MAR 20A", 157.0000, "FM", HIGH, "Port operations, simplex"),
    sx("Marine 21A Government", "MAR 21A", 157.0500, "FM", HIGH, "US Government only"),
    sx("Marine 22A Coast Guard", "MAR 22A", 157.1000, "FM", HIGH, "Coast Guard liaison and safety broadcasts"),
    sx("Marine 23A Government", "MAR 23A", 157.1500, "FM", HIGH, "US Government only"),
    dx("Marine 24 Marine Operator", "MAR 24", 161.8000, 157.2000, "FM", HIGH, "Public correspondence"),
    dx("Marine 25 Marine Operator", "MAR 25", 161.8500, 157.2500, "FM", HIGH, "Public correspondence"),
    dx("Marine 26 Marine Operator", "MAR 26", 161.9000, 157.3000, "FM", HIGH, "Public correspondence"),
    dx("Marine 27 Marine Operator", "MAR 27", 161.9500, 157.3500, "FM", HIGH, "Public correspondence"),
    dx("Marine 28 Marine Operator", "MAR 28", 162.0000, 157.4000, "FM", HIGH, "Public correspondence"),
    sx("Marine 61A Government", "MAR 61A", 156.0750, "FM", HIGH, "US Government only"),
    sx("Marine 62A Government", "MAR 62A", 156.1250, "FM", HIGH, "US Government only"),
    sx("Marine 63A Port Ops", "MAR 63A", 156.1750, "FM", HIGH, "Port operations / commercial"),
    sx("Marine 64A Government", "MAR 64A", 156.2250, "FM", HIGH, "US Government only"),
    sx("Marine 65A Port Ops", "MAR 65A", 156.2750, "FM", HIGH, "Port operations"),
    sx("Marine 66A Port Ops", "MAR 66A", 156.3250, "FM", HIGH, "Port operations"),
    sx("Marine 67 Bridge", "MAR 67", 156.3750, "FM", LOW, "Commercial; bridge-to-bridge on the lower Mississippi, 1 W"),
    sx("Marine 68 Non-Commercial", "MAR 68", 156.4250, "FM", HIGH, "Recreational working channel"),
    sx("Marine 69 Non-Commercial", "MAR 69", 156.4750, "FM", HIGH, "Recreational working channel"),
    ro("Marine 70 DSC", "MAR 70", 156.5250, "FM", "Digital selective calling — voice prohibited"),
    sx("Marine 71 Non-Commercial", "MAR 71", 156.5750, "FM", HIGH, "Recreational working channel"),
    sx("Marine 72 Non-Commercial", "MAR 72", 156.6250, "FM", HIGH, "Recreational, intership only"),
    sx("Marine 73 Port Ops", "MAR 73", 156.6750, "FM", HIGH, "Port operations"),
    sx("Marine 74 Port Ops", "MAR 74", 156.7250, "FM", HIGH, "Port operations"),
    sx("Marine 75 Port Ops", "MAR 75", 156.7750, "FM", LOW, "Port operations, guard band, 1 W"),
    sx("Marine 76 Port Ops", "MAR 76", 156.8250, "FM", LOW, "Port operations, guard band, 1 W"),
    sx("Marine 77 Port Ops", "MAR 77", 156.8750, "FM", LOW, "Port operations, intership only, 1 W"),
    sx("Marine 78A Non-Commercial", "MAR 78A", 156.9250, "FM", HIGH, "Recreational working channel"),
    sx("Marine 79A Commercial", "MAR 79A", 156.9750, "FM", HIGH, "Commercial; non-commercial on the Great Lakes"),
    sx("Marine 80A Commercial", "MAR 80A", 157.0250, "FM", HIGH, "Commercial; non-commercial on the Great Lakes"),
    sx("Marine 81A Government", "MAR 81A", 157.0750, "FM", HIGH, "US Government only — environmental protection"),
    sx("Marine 82A Government", "MAR 82A", 157.1250, "FM", HIGH, "US Government only"),
    sx("Marine 83A Government", "MAR 83A", 157.1750, "FM", HIGH, "US Government only"),
    dx("Marine 84 Marine Operator", "MAR 84", 161.8250, 157.2250, "FM", HIGH, "Public correspondence"),
    dx("Marine 85 Marine Operator", "MAR 85", 161.8750, 157.2750, "FM", HIGH, "Public correspondence"),
    dx("Marine 86 Marine Operator", "MAR 86", 161.9250, 157.3250, "FM", HIGH, "Public correspondence"),
    sx("Marine 87 Marine Operator", "MAR 87", 157.3750, "FM", HIGH, "Public correspondence"),
    sx("Marine 88 Commercial", "MAR 88", 157.4250, "FM", HIGH, "Commercial, intership only"),
];

// ============================================================
// NOAA weather — receive only
// ============================================================

/// The seven NWR frequencies. Every one is receive-only: transmitting on them
/// is a federal offence, so none carries a TX frequency.
static WEATHER: &[StdChannel] = &[
    ro("NOAA Weather 1", "WX 1", 162.5500, "FM", "Receive only"),
    ro("NOAA Weather 2", "WX 2", 162.4000, "FM", "Receive only"),
    ro("NOAA Weather 3", "WX 3", 162.4750, "FM", "Receive only"),
    ro("NOAA Weather 4", "WX 4", 162.4250, "FM", "Receive only"),
    ro("NOAA Weather 5", "WX 5", 162.4500, "FM", "Receive only"),
    ro("NOAA Weather 6", "WX 6", 162.5000, "FM", "Receive only"),
    ro("NOAA Weather 7", "WX 7", 162.5250, "FM", "Receive only"),
];

/// Everything on offer, in the order the picker shows them.
static CATALOG: &[StdList] = &[
    StdList {
        id: "gmrs",
        name: "GMRS",
        full_name: "General Mobile Radio Service",
        description:
            "22 simplex channels plus the 8 repeater pairs (+5 MHz). FCC licence required, no exam.",
        service_type: "GMRS",
        channels: GMRS,
    },
    StdList {
        id: "frs",
        name: "FRS",
        full_name: "Family Radio Service",
        description:
            "The 22 licence-free bubble-pack channels. Shares frequencies with GMRS at lower power.",
        service_type: "FRS",
        channels: FRS,
    },
    StdList {
        id: "murs",
        name: "MURS",
        full_name: "Multi-Use Radio Service",
        description: "Five licence-free 2 W VHF channels, including Blue Dot and Green Dot.",
        service_type: "MURS",
        channels: MURS,
    },
    StdList {
        id: "marine",
        name: "Marine VHF",
        full_name: "Marine VHF (US channel plan)",
        description:
            "The US Part 80 channels — distress, calling, bridge-to-bridge, port ops and the marine operator pairs.",
        service_type: "Marine",
        channels: MARINE,
    },
    StdList {
        id: "weather",
        name: "NOAA Weather",
        full_name: "NOAA Weather Radio",
        description: "The seven NWR broadcast frequencies. Receive only.",
        service_type: "Weather",
        channels: WEATHER,
    },
    StdList {
        id: "cb",
        name: "CB",
        full_name: "Citizens Band",
        description:
            "All 40 AM channels at 27 MHz. Most radios in your library can't tune here — they'll be skipped at export.",
        service_type: "CB",
        channels: CB,
    },
];

fn find_list(id: &str) -> Result<&'static StdList, String> {
    CATALOG
        .iter()
        .find(|l| l.id == id)
        .ok_or_else(|| format!("Unknown standard list \"{id}\""))
}

// ============================================================
// Wire types
// ============================================================

#[derive(Debug, Serialize)]
pub struct StandardListChannel {
    pub name: String,
    pub name_short: String,
    pub rx_freq: f64,
    pub tx_freq: Option<f64>,
    pub band: String,
    pub duplex: String,
    pub offset: f64,
    pub mode: String,
    pub power: Option<String>,
    pub notes: String,
}

#[derive(Debug, Serialize)]
pub struct StandardListInfo {
    pub id: String,
    pub name: String,
    pub full_name: String,
    pub description: String,
    pub service_type: String,
    /// Every band the list touches, e.g. `["VHF"]` or `["UHF"]`.
    pub bands: Vec<String>,
    pub channel_count: usize,
    pub channels: Vec<StandardListChannel>,
}

#[derive(Debug, Default, Serialize)]
pub struct StandardImportSummary {
    /// Channels inserted into the master table.
    pub added: usize,
    /// Channels already present (matched on frequency pair + name) and left alone.
    pub skipped: usize,
    /// The channel list the import filled, when one was asked for.
    pub list_id: Option<i64>,
    pub list_name: Option<String>,
    /// Entries appended to that list — excludes ones already in it.
    pub list_added: usize,
}

// ============================================================
// Commands
// ============================================================

/// The whole catalog, channels included. It is a few hundred rows of static
/// data, so the picker gets everything in one call and previews without a
/// round trip per selection.
#[tauri::command]
pub fn list_standard_lists() -> Vec<StandardListInfo> {
    CATALOG
        .iter()
        .map(|l| {
            let mut bands: Vec<String> = Vec::new();
            let channels = l
                .channels
                .iter()
                .map(|c| {
                    let band = derive_band(c.rx).to_string();
                    if !bands.contains(&band) {
                        bands.push(band.clone());
                    }
                    let (duplex, offset) = derive_duplex(c.rx, c.tx);
                    StandardListChannel {
                        name: c.name.to_string(),
                        name_short: c.short.to_string(),
                        rx_freq: c.rx,
                        tx_freq: c.tx,
                        band,
                        duplex,
                        offset,
                        mode: c.mode.to_string(),
                        power: c.power.map(str::to_string),
                        notes: c.notes.to_string(),
                    }
                })
                .collect();
            StandardListInfo {
                id: l.id.to_string(),
                name: l.name.to_string(),
                full_name: l.full_name.to_string(),
                description: l.description.to_string(),
                service_type: l.service_type.to_string(),
                bands,
                channel_count: l.channels.len(),
                channels,
            }
        })
        .collect()
}

/// Add a standard list's channels to the master table, optionally collecting
/// them into a channel list (which becomes a zone on the radio).
///
/// Re-running it is safe: a channel already in the library is counted as
/// skipped rather than duplicated, but it still joins the channel list — so
/// importing GMRS twice gives you one set of channels and one complete list.
#[tauri::command]
pub async fn import_standard_list(
    state: State<'_, AppState>,
    id: String,
    create_list: bool,
    list_name: Option<String>,
) -> Result<StandardImportSummary, String> {
    import_into(&state.pool, &id, create_list, list_name.as_deref()).await
}

/// The body of `import_standard_list`, against a pool rather than Tauri state.
async fn import_into(
    pool: &SqlitePool,
    id: &str,
    create_list: bool,
    list_name: Option<&str>,
) -> Result<StandardImportSummary, String> {
    let list = find_list(id)?;
    let mut conn = pool.acquire().await.estr()?;
    let mut tx = conn.begin().await.estr()?;

    let mut summary = StandardImportSummary::default();
    // Channel ids in catalog order, so a created list keeps the channel-number
    // ordering the service defines rather than whatever order rows landed in.
    let mut channel_ids: Vec<i64> = Vec::with_capacity(list.channels.len());

    for c in list.channels {
        // Same dedupe rule the native channel backup importer uses for manual
        // channels: frequency pair plus name.
        let existing: Option<(i64,)> = sqlx::query_as(
            "SELECT id FROM channels
             WHERE rx_freq = ?1
               AND IFNULL(name_long, '') = ?2
               AND IFNULL(tx_freq, -1) = IFNULL(?3, -1)",
        )
        .bind(c.rx)
        .bind(c.name)
        .bind(c.tx)
        .fetch_optional(&mut *tx)
        .await
        .estr()?;

        if let Some((existing_id,)) = existing {
            summary.skipped += 1;
            channel_ids.push(existing_id);
            continue;
        }

        let (duplex, offset) = derive_duplex(c.rx, c.tx);
        let new_id = sqlx::query(
            r#"
            INSERT INTO channels (
                name_long, name_short, rx_freq, tx_freq, offset, duplex, band,
                mode, tone_mode, dcs_polarity, cross_mode, power, service_type,
                country, notes, source, last_user_edit
            ) VALUES (
                ?1, ?2, ?3, ?4, ?5, ?6, ?7,
                ?8, 'off', 'NN', 'Tone->Tone', ?9, ?10,
                'United States', ?11, 'standard', CURRENT_TIMESTAMP
            )
            "#,
        )
        .bind(c.name)
        .bind(c.short)
        .bind(c.rx)
        .bind(c.tx)
        .bind(offset)
        .bind(&duplex)
        .bind(derive_band(c.rx))
        .bind(c.mode)
        .bind(c.power)
        .bind(list.service_type)
        .bind(if c.notes.is_empty() { None } else { Some(c.notes) })
        .execute(&mut *tx)
        .await
        .estr()?
        .last_insert_rowid();

        summary.added += 1;
        channel_ids.push(new_id);
    }

    if create_list {
        let name = list_name
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or(list.name)
            .to_string();

        // Reuse a list of that name if the user already has one, so a second
        // import tops it up instead of creating "GMRS" twice.
        let existing: Option<(i64,)> =
            sqlx::query_as("SELECT id FROM channel_lists WHERE name = ?1")
                .bind(&name)
                .fetch_optional(&mut *tx)
                .await
                .estr()?;

        let list_id = match existing {
            Some((lid,)) => lid,
            None => sqlx::query("INSERT INTO channel_lists (name, description) VALUES (?1, ?2)")
                .bind(&name)
                .bind(format!("{} — standard channel list", list.full_name))
                .execute(&mut *tx)
                .await
                .estr()?
                .last_insert_rowid(),
        };

        let mut next_pos: i64 = sqlx::query_as::<_, (i64,)>(
            "SELECT COALESCE(MAX(position), -1) + 1 FROM channel_list_entries WHERE channel_list_id = ?1",
        )
        .bind(list_id)
        .fetch_one(&mut *tx)
        .await
        .estr()?
        .0;

        for channel_id in &channel_ids {
            let already: Option<(i64,)> = sqlx::query_as(
                "SELECT id FROM channel_list_entries WHERE channel_list_id = ?1 AND channel_id = ?2",
            )
            .bind(list_id)
            .bind(channel_id)
            .fetch_optional(&mut *tx)
            .await
            .estr()?;
            if already.is_some() {
                continue;
            }
            sqlx::query(
                "INSERT INTO channel_list_entries (channel_list_id, channel_id, position) VALUES (?1, ?2, ?3)",
            )
            .bind(list_id)
            .bind(channel_id)
            .bind(next_pos)
            .execute(&mut *tx)
            .await
            .estr()?;
            next_pos += 1;
            summary.list_added += 1;
        }

        summary.list_id = Some(list_id);
        summary.list_name = Some(name);
    }

    tx.commit().await.estr()?;
    Ok(summary)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::Channel;

    /// Short names have to survive the narrowest radio we support (7 chars),
    /// and every channel needs both names — an empty one would export blank.
    #[test]
    fn names_fit_the_narrowest_radio() {
        for list in CATALOG {
            for c in list.channels {
                assert!(
                    !c.name.is_empty() && !c.short.is_empty(),
                    "{} / {} is missing a name",
                    list.id,
                    c.rx
                );
                assert!(
                    c.short.chars().count() <= 7,
                    "{} short name {:?} is {} chars, max 7",
                    list.id,
                    c.short,
                    c.short.chars().count()
                );
            }
        }
    }

    /// Names are the dedupe key alongside the frequency pair, so a repeat
    /// inside one list would make the second import skip a real channel.
    #[test]
    fn names_and_frequencies_are_unique_within_a_list() {
        for list in CATALOG {
            let mut seen_names: Vec<&str> = Vec::new();
            let mut seen_pairs: Vec<(u64, u64)> = Vec::new();
            for c in list.channels {
                assert!(
                    !seen_names.contains(&c.name),
                    "{}: duplicate name {:?}",
                    list.id,
                    c.name
                );
                seen_names.push(c.name);

                let key = (
                    (c.rx * 1_000_000.0).round() as u64,
                    (c.tx.unwrap_or(0.0) * 1_000_000.0).round() as u64,
                );
                assert!(
                    !seen_pairs.contains(&key),
                    "{}: duplicate RX/TX pair for {:?}",
                    list.id,
                    c.name
                );
                seen_pairs.push(key);
            }
        }
    }

    /// Slugs address the tables from the UI; two the same would shadow a list.
    #[test]
    fn list_ids_are_unique() {
        let mut ids: Vec<&str> = Vec::new();
        for list in CATALOG {
            assert!(!ids.contains(&list.id), "duplicate list id {:?}", list.id);
            ids.push(list.id);
        }
    }

    /// Spot-checks against the published allocations — the frequencies are the
    /// whole point of the feature, and a fat-fingered digit is invisible in
    /// the UI but wrong on the air.
    #[test]
    fn frequencies_match_the_published_plans() {
        let by_name = |list: &'static [StdChannel], name: &str| {
            list.iter().find(|c| c.name == name).unwrap_or_else(|| panic!("missing {name}"))
        };

        // FRS/GMRS share channels 1-7 and 15-22; only power and bandwidth differ.
        for n in [1usize, 2, 3, 4, 5, 6, 7, 15, 16, 17, 18, 19, 20, 21, 22] {
            let frs = by_name(FRS, &format!("FRS {n}"));
            let gmrs = by_name(GMRS, &format!("GMRS {n}"));
            assert_eq!(frs.rx, gmrs.rx, "FRS {n} and GMRS {n} must share a frequency");
        }

        // The 467 MHz interstitials are narrowband half-watt channels.
        assert_eq!(by_name(FRS, "FRS 8").rx, 467.5625);
        assert_eq!(by_name(FRS, "FRS 8").mode, "NFM");
        assert_eq!(by_name(GMRS, "GMRS 14").rx, 467.7125);

        // GMRS repeater pairs transmit exactly 5 MHz up from the output.
        for c in GMRS.iter().filter(|c| c.name.starts_with("GMRS Repeater")) {
            let tx = c.tx.expect("repeater channels transmit");
            assert!(
                (tx - c.rx - 5.0).abs() < 1e-9,
                "{} offset is {} MHz, expected +5",
                c.name,
                tx - c.rx
            );
        }

        assert_eq!(by_name(MURS, "MURS 4 Blue Dot").rx, 154.5700);
        assert_eq!(by_name(CB, "CB 19").rx, 27.1850);
        // The famous kink: 23 sits above 24 and 25.
        assert!(by_name(CB, "CB 23").rx > by_name(CB, "CB 25").rx);
        assert_eq!(CB.len(), 40);

        assert_eq!(by_name(MARINE, "Marine 16 Distress").rx, 156.8000);
        // Duplex marine channels receive high and transmit 4.6 MHz down.
        let ch24 = by_name(MARINE, "Marine 24 Marine Operator");
        assert_eq!(ch24.rx, 161.8000);
        assert_eq!(ch24.tx, Some(157.2000));

        assert_eq!(by_name(WEATHER, "NOAA Weather 1").rx, 162.5500);
        assert_eq!(WEATHER.len(), 7);
    }

    /// Receive-only channels must carry no TX frequency — that `None` is the
    /// only thing stopping an exporter from keying up on NOAA or marine 70.
    #[test]
    fn receive_only_channels_have_no_tx() {
        for c in WEATHER {
            assert!(c.tx.is_none(), "{} must be receive-only", c.name);
        }
        for name in ["Marine 15 Environmental", "Marine 70 DSC"] {
            let c = MARINE.iter().find(|c| c.name == name).expect("channel exists");
            assert!(c.tx.is_none(), "{name} must be receive-only");
        }
    }

    /// The import lands real rows, fills a channel list in channel-number
    /// order, and — the part that matters — a second run neither duplicates
    /// channels nor leaves the list short.
    #[tokio::test]
    async fn imports_once_and_is_safe_to_repeat() {
        let dir = std::env::temp_dir().join(format!("cpm_std_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let db_path = dir.join("std.sqlite3");
        let _ = std::fs::remove_file(&db_path);
        let pool = crate::db::init_pool(&db_path).await.expect("init_pool");

        let first = import_into(&pool, "gmrs", true, None).await.unwrap();
        assert_eq!(first.added, 30);
        assert_eq!(first.skipped, 0);
        assert_eq!(first.list_added, 30);
        assert_eq!(first.list_name.as_deref(), Some("GMRS"));

        // Fields the exporters actually read came through.
        let rpt = sqlx::query_as::<_, Channel>(
            "SELECT * FROM channels WHERE name_long = 'GMRS Repeater 19'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(rpt.rx_freq, 462.6500);
        assert_eq!(rpt.tx_freq, Some(467.6500));
        assert_eq!(rpt.duplex.as_deref(), Some("+"));
        assert!((rpt.offset.unwrap() - 5.0).abs() < 1e-9);
        assert_eq!(rpt.band.as_deref(), Some("UHF"));
        assert_eq!(rpt.name_short.as_deref(), Some("RPT19"));
        assert_eq!(rpt.service_type.as_deref(), Some("GMRS"));
        assert_eq!(rpt.tone_mode.as_deref(), Some("off"));
        assert_eq!(rpt.source, "standard");

        // The list is in catalog order — channel 1 first, repeater 22 last.
        let names: Vec<(String,)> = sqlx::query_as(
            "SELECT c.name_long FROM channel_list_entries e
             JOIN channels c ON c.id = e.channel_id
             JOIN channel_lists l ON l.id = e.channel_list_id
             WHERE l.name = 'GMRS' ORDER BY e.position",
        )
        .fetch_all(&pool)
        .await
        .unwrap();
        assert_eq!(names.len(), 30);
        assert_eq!(names[0].0, "GMRS 1");
        assert_eq!(names[29].0, "GMRS Repeater 22");

        // Re-importing skips every channel and adds no list entries, but does
        // not create a second "GMRS" list either.
        let again = import_into(&pool, "gmrs", true, None).await.unwrap();
        assert_eq!(again.added, 0);
        assert_eq!(again.skipped, 30);
        assert_eq!(again.list_added, 0);
        assert_eq!(again.list_id, first.list_id);

        let total: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM channels")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(total.0, 30);

        // FRS shares 15 frequencies with GMRS but names them differently, so
        // all 22 are new channels rather than dedupe collisions.
        let frs = import_into(&pool, "frs", false, None).await.unwrap();
        assert_eq!(frs.added, 22);
        assert_eq!(frs.skipped, 0);
        assert_eq!(frs.list_id, None);

        // Receive-only channels keep a NULL tx_freq through the round trip.
        import_into(&pool, "weather", true, Some(" Weather ")).await.unwrap();
        let wx = sqlx::query_as::<_, Channel>(
            "SELECT * FROM channels WHERE name_long = 'NOAA Weather 1'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(wx.tx_freq, None);
        assert_eq!(wx.duplex.as_deref(), Some("none"));
        // The list name is taken from the caller, trimmed.
        let lists: Vec<(String,)> =
            sqlx::query_as("SELECT name FROM channel_lists ORDER BY name")
                .fetch_all(&pool)
                .await
                .unwrap();
        assert_eq!(
            lists.iter().map(|l| l.0.as_str()).collect::<Vec<_>>(),
            vec!["GMRS", "Weather"]
        );

        assert!(import_into(&pool, "nope", false, None).await.is_err());

        let _ = std::fs::remove_file(&db_path);
    }

    /// The bands reported to the picker come from the real frequencies, so a
    /// list can't advertise a band it doesn't cover.
    #[test]
    fn reported_bands_come_from_the_channels() {
        let infos = list_standard_lists();
        assert_eq!(infos.len(), CATALOG.len());
        let by_id = |id: &str| infos.iter().find(|i| i.id == id).expect("list present");

        assert_eq!(by_id("gmrs").bands, vec!["UHF"]);
        assert_eq!(by_id("murs").bands, vec!["VHF"]);
        assert_eq!(by_id("cb").bands, vec!["HF"]);
        assert_eq!(by_id("weather").bands, vec!["VHF"]);
        assert_eq!(by_id("gmrs").channel_count, 30);
        assert_eq!(by_id("frs").channel_count, 22);
    }
}
