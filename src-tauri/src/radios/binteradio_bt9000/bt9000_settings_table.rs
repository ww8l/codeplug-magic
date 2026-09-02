//! BT-9000 settings field table — GENERATED, do not edit.
//!
//! Source: `scratchpad/binteradio_bt9000/MEASURED.md`, via
//! `gen_bt9000_settings.py`. The profile-form schema at
//! `src/bt9000_settings_schema.json` comes from the same parse, and
//! `settings.rs` asserts the two still describe the same fields.
//!
//! Every field here is graded `screen` in the sheet: its encoding was
//! settled on the radio's own screen, not taken from the source
//! inventory or the manual. That bar is higher here than on other
//! radios in this crate because THIS RADIO VALIDATES NOTHING — it
//! stored 127 in four fields whose maxima are 9, 2, 3 and 1 — so a
//! wrong encoding is stored rather than refused, and this table is the
//! only thing standing between the operator and a bad value.
//!
//! 42 field(s). The sheet's other rows are measured but not
//! settled; see its Tally section for what is still owed.

/// How a field's value is carried in the byte.
#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) enum Enc {
    /// Stored byte is the value: an enum index, or the displayed number.
    Direct,
    /// Stored byte is the displayed number minus one. Real on this radio:
    /// SQL at 0x00 stores the level, VOX Level at 0x02 stores level − 1,
    /// and they sit two bytes apart with the same printed "Level 1-9".
    Minus1,
}

/// What the form draws, and what the encoder may write.
#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) enum Kind {
    /// `0 = OFF`, `1 = ON`.
    Bool,
    /// Zero-based index into `options`, which is in STORED order.
    Enum,
    /// Displayed number, inclusive of both bounds.
    Int { lo: u8, hi: u8 },
}

/// One settings field.
pub(crate) struct SF {
    pub key: &'static str,
    pub label: &'static str,
    /// This radio's own menu path. NOT the RT-950 Pro manual's: every
    /// item in the Radio group is numbered one higher here, because the
    /// BT-9000 inserts Work Band at Radio → 1.
    pub menu: &'static str,
    /// Offset within the function block (file offset = 0x7900 + addr).
    pub addr: usize,
    pub kind: Kind,
    pub enc: Enc,
    /// Enum labels in stored-index order; empty for the other kinds.
    pub options: &'static [&'static str],
}

pub(crate) const FIELDS: [SF; 42] = [
    SF {
        key: "vox-level",
        label: "VOX Level",
        menu: "VOX → 2. VOX Level",
        addr: 0x02,
        kind: Kind::Int { lo: 1, hi: 9 },
        enc: Enc::Minus1,
        options: &[],
    },
    SF {
        key: "vox-delay",
        label: "VOX Delay",
        menu: "VOX → 3. VOX Delay",
        addr: 0x20,
        kind: Kind::Enum,
        enc: Enc::Direct,
        options: &["0.5 sec", "0.6 sec", "0.7 sec", "0.8 sec", "0.9 sec", "1.0 sec", "1.1 sec", "1.2 sec", "1.3 sec", "1.4 sec", "1.5 sec", "1.6 sec", "1.7 sec", "1.8 sec", "1.9 sec", "2.0 sec"],
    },
    SF {
        key: "vox-switch",
        label: "VOX Switch",
        menu: "VOX → 1. VOX Switch",
        addr: 0x28,
        kind: Kind::Bool,
        enc: Enc::Direct,
        options: &[],
    },
    SF {
        key: "squelch",
        label: "SQL",
        menu: "Radio → 2. SQL",
        addr: 0x00,
        kind: Kind::Int { lo: 1, hi: 9 },
        enc: Enc::Direct,
        options: &[],
    },
    SF {
        key: "battery-save",
        label: "Battery Save",
        menu: "Radio → 7. Battery Save",
        addr: 0x01,
        kind: Kind::Enum,
        enc: Enc::Direct,
        options: &["OFF", "Normal", "Super", "DEEP"],
    },
    SF {
        key: "standby-set",
        label: "Standby Set",
        menu: "Radio → 6. Standby Set",
        addr: 0x04,
        kind: Kind::Bool,
        enc: Enc::Direct,
        options: &[],
    },
    SF {
        key: "tot",
        label: "TOT",
        menu: "Radio → 9. TOT",
        addr: 0x05,
        kind: Kind::Enum,
        enc: Enc::Direct,
        options: &["OFF", "30sec", "60sec", "90sec", "120sec", "150sec", "180sec", "210sec", "240sec"],
    },
    SF {
        key: "scan-mode",
        label: "Scan Mode",
        menu: "Radio → 12. Scan Mode",
        addr: 0x0A,
        kind: Kind::Enum,
        enc: Enc::Direct,
        options: &["Time", "Carrier", "Search"],
    },
    SF {
        key: "sos-mode",
        label: "SOS Mode",
        menu: "Radio → 18. SOS Mode",
        addr: 0x11,
        kind: Kind::Enum,
        enc: Enc::Direct,
        options: &["On Site", "Send Sound", "Send Code"],
    },
    SF {
        key: "tall",
        label: "TALL",
        menu: "Radio → 10. TALL",
        addr: 0x14,
        kind: Kind::Bool,
        enc: Enc::Direct,
        options: &[],
    },
    SF {
        key: "rp-ste",
        label: "RP-STE",
        menu: "Radio → 14. RP-STE",
        addr: 0x15,
        kind: Kind::Enum,
        enc: Enc::Direct,
        options: &["OFF", "100ms", "200ms", "300ms", "400ms", "500ms", "600ms", "700ms", "800ms", "900ms", "1000ms"],
    },
    SF {
        key: "rpt-rl",
        label: "RPT-RL",
        menu: "Radio → 15. RPT-RL",
        addr: 0x16,
        kind: Kind::Enum,
        enc: Enc::Direct,
        options: &["OFF", "100ms", "200ms", "300ms", "400ms", "500ms", "600ms", "700ms", "800ms", "900ms", "1000ms"],
    },
    SF {
        key: "roger",
        label: "ROGRE",
        menu: "Radio → 13. ROGRE",
        addr: 0x17,
        kind: Kind::Enum,
        enc: Enc::Direct,
        options: &["OFF", "Beep", "Tone 1200"],
    },
    SF {
        key: "r-tone",
        label: "R-TONE",
        menu: "Radio → 11. R-TONE",
        addr: 0x1E,
        kind: Kind::Enum,
        enc: Enc::Direct,
        options: &["1000hz", "1450hz", "1750hz", "2100hz"],
    },
    SF {
        key: "ab-rpt-mode",
        label: "AB RPT-Mode",
        menu: "Radio → 16. AB RPT-Mode",
        addr: 0x26,
        kind: Kind::Bool,
        enc: Enc::Direct,
        options: &[],
    },
    SF {
        key: "rpt-speaker",
        label: "RPT-Speaker",
        menu: "Radio → 17. RPT-Speaker",
        addr: 0x3A,
        kind: Kind::Bool,
        enc: Enc::Direct,
        options: &[],
    },
    SF {
        key: "mdf-a",
        label: "MDF-A",
        menu: "VFO&CH → 1. MDF-A",
        addr: 0x0D,
        kind: Kind::Enum,
        enc: Enc::Direct,
        options: &["NAME", "FREQUENCY", "CHANNEL NUM."],
    },
    SF {
        key: "mdf-b",
        label: "MDF-B",
        menu: "VFO&CH → 2. MDF-B",
        addr: 0x0E,
        kind: Kind::Enum,
        enc: Enc::Direct,
        options: &["NAME", "FREQUENCY", "CHANNEL NUM."],
    },
    SF {
        key: "mdf-c",
        label: "MDF-C",
        menu: "VFO&CH → 3. MDF-C",
        addr: 0x0F,
        kind: Kind::Enum,
        enc: Enc::Direct,
        options: &["NAME", "FREQUENCY", "CHANNEL NUM."],
    },
    SF {
        key: "backlight",
        label: "Back Light",
        menu: "Setting → 4. Back Light",
        addr: 0x03,
        kind: Kind::Enum,
        enc: Enc::Direct,
        options: &["Bright", "5 sec", "10 sec", "15 sec", "20 sec", "30 sec", "1 min", "2 min", "3 min"],
    },
    SF {
        key: "beep",
        label: "Beep Prompt",
        menu: "Setting → 1. Beep Prompt",
        addr: 0x06,
        kind: Kind::Bool,
        enc: Enc::Direct,
        options: &[],
    },
    SF {
        key: "voice",
        label: "Voice",
        menu: "Setting → 2. Voice",
        addr: 0x07,
        kind: Kind::Bool,
        enc: Enc::Direct,
        options: &[],
    },
    SF {
        key: "language",
        label: "Language",
        menu: "Setting → 7. Language",
        addr: 0x08,
        kind: Kind::Enum,
        enc: Enc::Direct,
        options: &["English", "Chinese"],
    },
    SF {
        key: "keypad-lock",
        label: "Keypad Lock",
        menu: "Setting → 3. Keypad Lock",
        addr: 0x10,
        kind: Kind::Enum,
        enc: Enc::Direct,
        options: &["OFF", "5 sec", "10 sec", "15 sec"],
    },
    SF {
        key: "power-on-display",
        label: "Power On Display",
        menu: "Setting → 6. Power On Display",
        addr: 0x1C,
        kind: Kind::Enum,
        enc: Enc::Direct,
        options: &["Picture", "Voltage"],
    },
    SF {
        key: "menu-timeout",
        label: "Menu OutTime",
        menu: "Setting → 5. Menu OutTime",
        addr: 0x21,
        kind: Kind::Enum,
        enc: Enc::Direct,
        options: &["5 sec", "10 sec", "15 sec", "20 sec", "25 sec", "30 sec", "35 sec", "40 sec", "45 sec", "50 sec", "60 sec"],
    },
    SF {
        key: "bluetooth",
        label: "Bluetooth",
        menu: "Bluetooth",
        addr: 0x1D,
        kind: Kind::Bool,
        enc: Enc::Direct,
        options: &[],
    },
    SF {
        key: "dtmfst",
        label: "DTMFST",
        menu: "Signaling → 6. DTMFST",
        addr: 0x09,
        kind: Kind::Enum,
        enc: Enc::Direct,
        options: &["OFF", "DT-ST", "ANI-ST", "DT+ANI"],
    },
    SF {
        key: "pf1-short",
        label: "PF1",
        menu: "User Key → 1. PF1",
        addr: 0x29,
        kind: Kind::Enum,
        enc: Enc::Direct,
        options: &["RADIO", "MONI", "SCAN", "SEARCH", "SOS", "SPECTRUM", "Beacon TX"],
    },
    SF {
        key: "pf1-long",
        label: "Long press PF1",
        menu: "User Key → 2. Long press PF1",
        addr: 0x2A,
        kind: Kind::Enum,
        enc: Enc::Direct,
        options: &["RADIO", "MONI", "SCAN", "SEARCH", "SOS", "SPECTRUM", "Beacon TX"],
    },
    SF {
        key: "pf2-short",
        label: "PF2",
        menu: "User Key → 3. PF2",
        addr: 0x2B,
        kind: Kind::Enum,
        enc: Enc::Direct,
        options: &["RADIO", "MONI", "SCAN", "SEARCH", "SOS", "SPECTRUM", "Beacon TX"],
    },
    SF {
        key: "pf2-long",
        label: "Long press PF2",
        menu: "User Key → 4. Long press PF2",
        addr: 0x2C,
        kind: Kind::Enum,
        enc: Enc::Direct,
        options: &["RADIO", "MONI", "SCAN", "SEARCH", "SOS", "SPECTRUM", "Beacon TX"],
    },
    SF {
        key: "key0-long",
        label: "key [0] long press",
        menu: "User Key → key [0] long press",
        addr: 0x3B,
        kind: Kind::Enum,
        enc: Enc::Direct,
        options: &["NONE", "RADIO", "VOX", "SEARCH", "SPECTRUM", "NOAA", "SCAN QT", "SQUELCH", "FREQ STEP", "TX POWER", "CH-MEMORY", "ZONE SELECT", "STANDBY SET", "CTCSS DCS", "FREQ OFFSET", "FREQ DIR", "RX MODULATION", "TONE TX", "TRANSFER", "GPS SWITCH", "APRS SWITCH", "ROGER"],
    },
    SF {
        key: "key1-long",
        label: "key [1] long press",
        menu: "User Key → key [1] long press",
        addr: 0x3C,
        kind: Kind::Enum,
        enc: Enc::Direct,
        options: &["NONE", "RADIO", "VOX", "SEARCH", "SPECTRUM", "NOAA", "SCAN QT", "SQUELCH", "FREQ STEP", "TX POWER", "CH-MEMORY", "ZONE SELECT", "STANDBY SET", "CTCSS DCS", "FREQ OFFSET", "FREQ DIR", "RX MODULATION", "TONE TX", "TRANSFER", "GPS SWITCH", "APRS SWITCH", "ROGER"],
    },
    SF {
        key: "key2-long",
        label: "key [2] long press",
        menu: "User Key → key [2] long press",
        addr: 0x3D,
        kind: Kind::Enum,
        enc: Enc::Direct,
        options: &["NONE", "RADIO", "VOX", "SEARCH", "SPECTRUM", "NOAA", "SCAN QT", "SQUELCH", "FREQ STEP", "TX POWER", "CH-MEMORY", "ZONE SELECT", "STANDBY SET", "CTCSS DCS", "FREQ OFFSET", "FREQ DIR", "RX MODULATION", "TONE TX", "TRANSFER", "GPS SWITCH", "APRS SWITCH", "ROGER"],
    },
    SF {
        key: "key3-long",
        label: "key [3] long press",
        menu: "User Key → key [3] long press",
        addr: 0x3E,
        kind: Kind::Enum,
        enc: Enc::Direct,
        options: &["NONE", "RADIO", "VOX", "SEARCH", "SPECTRUM", "NOAA", "SCAN QT", "SQUELCH", "FREQ STEP", "TX POWER", "CH-MEMORY", "ZONE SELECT", "STANDBY SET", "CTCSS DCS", "FREQ OFFSET", "FREQ DIR", "RX MODULATION", "TONE TX", "TRANSFER", "GPS SWITCH", "APRS SWITCH", "ROGER"],
    },
    SF {
        key: "key4-long",
        label: "key [4] long press",
        menu: "User Key → key [4] long press",
        addr: 0x3F,
        kind: Kind::Enum,
        enc: Enc::Direct,
        options: &["NONE", "RADIO", "VOX", "SEARCH", "SPECTRUM", "NOAA", "SCAN QT", "SQUELCH", "FREQ STEP", "TX POWER", "CH-MEMORY", "ZONE SELECT", "STANDBY SET", "CTCSS DCS", "FREQ OFFSET", "FREQ DIR", "RX MODULATION", "TONE TX", "TRANSFER", "GPS SWITCH", "APRS SWITCH", "ROGER"],
    },
    SF {
        key: "key5-long",
        label: "key [5] long press",
        menu: "User Key → key [5] long press",
        addr: 0x40,
        kind: Kind::Enum,
        enc: Enc::Direct,
        options: &["NONE", "RADIO", "VOX", "SEARCH", "SPECTRUM", "NOAA", "SCAN QT", "SQUELCH", "FREQ STEP", "TX POWER", "CH-MEMORY", "ZONE SELECT", "STANDBY SET", "CTCSS DCS", "FREQ OFFSET", "FREQ DIR", "RX MODULATION", "TONE TX", "TRANSFER", "GPS SWITCH", "APRS SWITCH", "ROGER"],
    },
    SF {
        key: "key6-long",
        label: "key [6] long press",
        menu: "User Key → key [6] long press",
        addr: 0x41,
        kind: Kind::Enum,
        enc: Enc::Direct,
        options: &["NONE", "RADIO", "VOX", "SEARCH", "SPECTRUM", "NOAA", "SCAN QT", "SQUELCH", "FREQ STEP", "TX POWER", "CH-MEMORY", "ZONE SELECT", "STANDBY SET", "CTCSS DCS", "FREQ OFFSET", "FREQ DIR", "RX MODULATION", "TONE TX", "TRANSFER", "GPS SWITCH", "APRS SWITCH", "ROGER"],
    },
    SF {
        key: "key7-long",
        label: "key [7] long press",
        menu: "User Key → key [7] long press",
        addr: 0x42,
        kind: Kind::Enum,
        enc: Enc::Direct,
        options: &["NONE", "RADIO", "VOX", "SEARCH", "SPECTRUM", "NOAA", "SCAN QT", "SQUELCH", "FREQ STEP", "TX POWER", "CH-MEMORY", "ZONE SELECT", "STANDBY SET", "CTCSS DCS", "FREQ OFFSET", "FREQ DIR", "RX MODULATION", "TONE TX", "TRANSFER", "GPS SWITCH", "APRS SWITCH", "ROGER"],
    },
    SF {
        key: "key8-long",
        label: "key [8] long press",
        menu: "User Key → key [8] long press",
        addr: 0x43,
        kind: Kind::Enum,
        enc: Enc::Direct,
        options: &["NONE", "RADIO", "VOX", "SEARCH", "SPECTRUM", "NOAA", "SCAN QT", "SQUELCH", "FREQ STEP", "TX POWER", "CH-MEMORY", "ZONE SELECT", "STANDBY SET", "CTCSS DCS", "FREQ OFFSET", "FREQ DIR", "RX MODULATION", "TONE TX", "TRANSFER", "GPS SWITCH", "APRS SWITCH", "ROGER"],
    },
    SF {
        key: "key9-long",
        label: "key [9] long press",
        menu: "User Key → key [9] long press",
        addr: 0x44,
        kind: Kind::Enum,
        enc: Enc::Direct,
        options: &["NONE", "RADIO", "VOX", "SEARCH", "SPECTRUM", "NOAA", "SCAN QT", "SQUELCH", "FREQ STEP", "TX POWER", "CH-MEMORY", "ZONE SELECT", "STANDBY SET", "CTCSS DCS", "FREQ OFFSET", "FREQ DIR", "RX MODULATION", "TONE TX", "TRANSFER", "GPS SWITCH", "APRS SWITCH", "ROGER"],
    },
];
