use sqlx::SqlitePool;

/// One row in the built-in radio model library.
struct ModelSeed {
    manufacturer: &'static str,
    model: &'static str,
    display_name: &'static str,
    analog_capable: bool,
    dmr_capable: bool,
    dstar_capable: bool,
    ysf_capable: bool,
    nxdn_capable: bool,
    p25_capable: bool,
    m17_capable: bool,
    aprs_capable: bool,
    covers_hf: bool,
    covers_vhf: bool,
    covers_uhf: bool,
    covers_220: bool,
    covers_900: bool,
    freq_min: f64,
    freq_max: f64,
    /// JSON `[[min_mhz, max_mhz], …]` — the bands the radio transmits on, for
    /// radios whose TX coverage is not one contiguous span (migration 0016).
    /// None leaves freq_min/freq_max + the covers_* flags in charge.
    tx_bands: Option<&'static str>,
    /// JSON `[[min_mhz, max_mhz], …]` — the bands the radio *receives* on.
    /// None means "same as transmit". Channels inside this but outside the TX
    /// bands are programmed receive-only rather than dropped.
    rx_bands: Option<&'static str>,
    memory_channels: i64,
    zones_supported: bool,
    max_zones: Option<i64>,
    channels_per_zone: Option<i64>,
    scan_lists_supported: bool,
    max_scan_lists: Option<i64>,
    banks_supported: bool,
    max_name_length: i64,
    export_format: &'static str,
    connection_type: &'static str,
    non_channel_settings_schema: &'static str,
    /// Live-USB driver key (`radios/<driver_key>/`); None for export-only models.
    driver_key: Option<&'static str>,
    /// Frontend "Program radio" dialog key; None for no live-programming UI.
    programming_ui: Option<&'static str>,
}

/// Seed (and refresh) the built-in radio model library. Keyed on
/// (manufacturer, model): on conflict it UPDATES the spec/capabilities/schema
/// from the bundled definitions, so changes here (e.g. a new settings schema)
/// propagate to existing databases on the next startup. The row id is preserved,
/// so user-created profiles keep their references intact. Built-in models are
/// reference data; users edit profiles, not models.
pub async fn seed_radio_models(pool: &SqlitePool) -> Result<(), sqlx::Error> {
    for m in models() {
        sqlx::query(
            r#"
            INSERT INTO radio_models (
                manufacturer, model, display_name,
                analog_capable, dmr_capable, dstar_capable, ysf_capable,
                nxdn_capable, p25_capable, m17_capable, aprs_capable,
                covers_hf, covers_vhf, covers_uhf, covers_220, covers_900,
                freq_min, freq_max, tx_bands, rx_bands, memory_channels,
                zones_supported, max_zones, channels_per_zone,
                scan_lists_supported, max_scan_lists, banks_supported,
                max_name_length, export_format, connection_type,
                non_channel_settings_schema, driver_key, programming_ui
            ) VALUES (
                ?1, ?2, ?3,
                ?4, ?5, ?6, ?7,
                ?8, ?9, ?10, ?11,
                ?12, ?13, ?14, ?15, ?16,
                ?17, ?18, ?19, ?20, ?21,
                ?22, ?23, ?24,
                ?25, ?26, ?27,
                ?28, ?29, ?30,
                ?31, ?32, ?33
            )
            ON CONFLICT(manufacturer, model) DO UPDATE SET
                display_name = excluded.display_name,
                analog_capable = excluded.analog_capable,
                dmr_capable = excluded.dmr_capable,
                dstar_capable = excluded.dstar_capable,
                ysf_capable = excluded.ysf_capable,
                nxdn_capable = excluded.nxdn_capable,
                p25_capable = excluded.p25_capable,
                m17_capable = excluded.m17_capable,
                aprs_capable = excluded.aprs_capable,
                covers_hf = excluded.covers_hf,
                covers_vhf = excluded.covers_vhf,
                covers_uhf = excluded.covers_uhf,
                covers_220 = excluded.covers_220,
                covers_900 = excluded.covers_900,
                freq_min = excluded.freq_min,
                freq_max = excluded.freq_max,
                tx_bands = excluded.tx_bands,
                rx_bands = excluded.rx_bands,
                memory_channels = excluded.memory_channels,
                zones_supported = excluded.zones_supported,
                max_zones = excluded.max_zones,
                channels_per_zone = excluded.channels_per_zone,
                scan_lists_supported = excluded.scan_lists_supported,
                max_scan_lists = excluded.max_scan_lists,
                banks_supported = excluded.banks_supported,
                max_name_length = excluded.max_name_length,
                export_format = excluded.export_format,
                connection_type = excluded.connection_type,
                non_channel_settings_schema = excluded.non_channel_settings_schema,
                driver_key = excluded.driver_key,
                programming_ui = excluded.programming_ui
            "#,
        )
        .bind(m.manufacturer)
        .bind(m.model)
        .bind(m.display_name)
        .bind(m.analog_capable)
        .bind(m.dmr_capable)
        .bind(m.dstar_capable)
        .bind(m.ysf_capable)
        .bind(m.nxdn_capable)
        .bind(m.p25_capable)
        .bind(m.m17_capable)
        .bind(m.aprs_capable)
        .bind(m.covers_hf)
        .bind(m.covers_vhf)
        .bind(m.covers_uhf)
        .bind(m.covers_220)
        .bind(m.covers_900)
        .bind(m.freq_min)
        .bind(m.freq_max)
        .bind(m.tx_bands)
        .bind(m.rx_bands)
        .bind(m.memory_channels)
        .bind(m.zones_supported)
        .bind(m.max_zones)
        .bind(m.channels_per_zone)
        .bind(m.scan_lists_supported)
        .bind(m.max_scan_lists)
        .bind(m.banks_supported)
        .bind(m.max_name_length)
        .bind(m.export_format)
        .bind(m.connection_type)
        .bind(m.non_channel_settings_schema)
        .bind(m.driver_key)
        .bind(m.programming_ui)
        .execute(pool)
        .await?;
    }
    Ok(())
}

/// The FT5D profile-settings schema, GENERATED alongside the Rust decode table
/// by `scratchpad/ft5d/gen_ft5d_settings.py` from CHIRP's `ft1d.py`. Exposed so
/// the settings module can assert the two halves still describe the same fields.
pub const FT5D_SETTINGS_SCHEMA: &str = include_str!("ft5d_settings_schema.json");

/// The ID-52 profile-settings schema, GENERATED alongside the Rust decode table
/// by `scratchpad/id52/gen_id52_table.py` from the measurement sheet. Exposed
/// for the same reason as the FT5D's: the settings module asserts the two halves
/// still describe the same fields.
pub const ID52_SETTINGS_SCHEMA: &str = include_str!("id52_settings_schema.json");

/// The TH-D75 profile-settings schema, GENERATED alongside the Rust decode
/// table by `scratchpad/thd75/gen_thd75_table.py` from `rts/MEASURED.md`.
/// Exposed for the same reason as the other two: the settings module asserts
/// the two halves still describe the same fields.
pub const THD75_SETTINGS_SCHEMA: &str = include_str!("thd75_settings_schema.json");

/// The TH-D72 profile-settings schema, GENERATED alongside the Rust field table
/// by `scratchpad/kenwood_thd72/gen_thd72_settings.py` from
/// `scratchpad/kenwood_thd72/MEASURED.md`. One parse emits both, so a field
/// cannot exist in the form and not in the encoder.
///
/// These are the 19 parameters of the radio's own `MU` menu command, not offsets
/// into the clone image — see `radios/kenwood_thd72/settings.rs` for why, and
/// for the six-of-six cross-check that showed the two carry the same values.
pub const THD72_SETTINGS_SCHEMA: &str = include_str!("thd72_settings_schema.json");

/// The BT-9000 profile-settings schema, GENERATED alongside the Rust field
/// table by `scratchpad/binteradio_bt9000/gen_bt9000_settings.py` from that
/// folder's `MEASURED.md`. `radios/binteradio_bt9000/settings.rs` asserts the
/// two halves still describe the same fields.
///
/// ⚠ It is SHORT on purpose. The sheet grades ~43 candidate fields and this
/// carries only the ones whose encoding was settled on the radio's own screen.
/// This radio validates nothing — it stored 127 in fields whose maxima are 9,
/// 2, 3 and 1 — so an unsettled encoding would be stored rather than refused,
/// and the schema is where that decision is enforced.
pub const BT9000_SETTINGS_SCHEMA: &str =
    include_str!("bt9000_settings_schema.json");


fn models() -> Vec<ModelSeed> {
    vec![
        // --------------------------------------------------------
        // 1. Baofeng UV-5R — analog only, 128 ch, no zones, 7 char
        // --------------------------------------------------------
        ModelSeed {
            manufacturer: "Baofeng",
            model: "UV-5R",
            driver_key: Some("baofeng_uv5r"),
            programming_ui: Some("generic"),
            display_name: "Baofeng UV-5R",
            analog_capable: true,
            dmr_capable: false,
            dstar_capable: false,
            ysf_capable: false,
            nxdn_capable: false,
            p25_capable: false,
            m17_capable: false,
            aprs_capable: false,
            // Updated with the measured bands: `tx_bands` now reaches 18-64 MHz
            // (27.500 and 50.125 both keyed, so part of it is HF) and 200-260
            // (223.500 keyed, which is 1.25 m). These flags are display only —
            // `tx_capable` reads `tx_bands` — but they are what the band chips
            // in the codeplug and profile screens show, and therefore what an
            // operator picks a radio by.
            covers_hf: true,
            covers_vhf: true,
            covers_uhf: true,
            covers_220: true,
            covers_900: false,
            freq_min: 18.0,
            freq_max: 520.0,
            tx_bands: None,
            rx_bands: None,
            memory_channels: 128,
            zones_supported: false,
            max_zones: None,
            channels_per_zone: None,
            scan_lists_supported: false,
            max_scan_lists: None,
            banks_supported: false,
            max_name_length: 7,
            export_format: "chirp_csv",
            connection_type: "Kenwood K1 (2-pin)",
            // Global (non-channel) settings derived from the CHIRP uv5r.py
            // driver: the full Basic, Advanced, Work Mode (VFO A/B), DTMF, and
            // Other (power-on message + band TX limits) setting groups (74
            // fields). Read-only firmware/6+power-on messages and the Service
            // calibration group are intentionally omitted. Keys match CHIRP's
            // RadioSetting names so a future CHIRP-.img exporter can map directly.
            non_channel_settings_schema: r#"[
                {"key": "squelch", "label": "Carrier Squelch Level", "type": "integer", "min": 0, "max": 9, "default": 5},
                {"key": "save", "label": "Battery Saver", "type": "select", "options": ["Off", "1:1", "1:2", "1:3", "1:4"], "default": "1:3"},
                {"key": "abr", "label": "Backlight Timeout (s)", "type": "integer", "min": 0, "max": 24, "default": 5},
                {"key": "beep", "label": "Key Beep", "type": "boolean", "default": true},
                {"key": "timeout", "label": "Timeout Timer", "type": "select", "options": ["15 sec", "30 sec", "45 sec", "60 sec", "75 sec", "90 sec", "105 sec", "120 sec", "135 sec", "150 sec", "165 sec", "180 sec", "195 sec", "210 sec", "225 sec", "240 sec", "255 sec", "270 sec", "285 sec", "300 sec", "315 sec", "330 sec", "345 sec", "360 sec", "375 sec", "390 sec", "405 sec", "420 sec", "435 sec", "450 sec", "465 sec", "480 sec", "495 sec", "510 sec", "525 sec", "540 sec", "555 sec", "570 sec", "585 sec", "600 sec", "Off (if supported by radio)"], "default": "120 sec"},
                {"key": "mdfa", "label": "Display Mode (A)", "type": "select", "options": ["Channel", "Name", "Frequency"], "default": "Name"},
                {"key": "mdfb", "label": "Display Mode (B)", "type": "select", "options": ["Channel", "Name", "Frequency"], "default": "Frequency"},
                {"key": "wtled", "label": "Standby LED Color", "type": "select", "options": ["Off", "Blue", "Orange", "Purple"], "default": "Purple"},
                {"key": "rxled", "label": "RX LED Color", "type": "select", "options": ["Off", "Blue", "Orange", "Purple"], "default": "Blue"},
                {"key": "txled", "label": "TX LED Color", "type": "select", "options": ["Off", "Blue", "Orange", "Purple"], "default": "Orange"},
                {"key": "roger", "label": "Roger Beep", "type": "boolean", "default": false},
                {"key": "vox", "label": "VOX Sensitivity", "type": "select", "options": ["OFF", "1", "2", "3", "4", "5", "6", "7", "8", "9", "10"], "default": "OFF"},
                {"key": "tdr", "label": "Dual Watch", "type": "boolean", "default": true},
                {"key": "tdrab", "label": "Dual Watch TX Priority", "type": "select", "options": ["Off", "A", "B"], "default": "Off"},
                {"key": "almod", "label": "Alarm Mode", "type": "select", "options": ["Site", "Tone", "Code"], "default": "Site"},
                {"key": "voice", "label": "Voice Prompt", "type": "select", "options": ["Off", "English", "Chinese"], "default": "English"},
                {"key": "screv", "label": "Scan Resume", "type": "select", "options": ["TO", "CO", "SE"], "default": "CO"},
                {"key": "bcl", "label": "Busy Channel Lockout", "type": "boolean", "default": false},
                {"key": "autolk", "label": "Automatic Key Lock", "type": "boolean", "default": false},
                {"key": "fmradio", "label": "Broadcast FM Radio", "type": "boolean", "default": true},
                {"key": "ste", "label": "Squelch Tail Eliminate (HT to HT)", "type": "boolean", "default": true},
                {"key": "rpste", "label": "Squelch Tail Eliminate (repeater)", "type": "select", "options": ["OFF", "1", "2", "3", "4", "5", "6", "7", "8", "9", "10"], "default": "OFF"},
                {"key": "rptrl", "label": "STE Repeater Delay", "type": "select", "options": ["OFF", "100 ms", "200 ms", "300 ms", "400 ms", "500 ms", "600 ms", "700 ms", "800 ms", "900 ms", "1000 ms"], "default": "OFF"},
                {"key": "reset", "label": "RESET Menu Enabled", "type": "boolean", "default": true},
                {"key": "menu", "label": "All Menus Enabled", "type": "boolean", "default": true},
                {"key": "displayab", "label": "Display Side", "type": "select", "options": ["A", "B"], "default": "A"},
                {"key": "workmode", "label": "VFO/MR Mode", "type": "select", "options": ["Frequency", "Channel"], "default": "Channel"},
                {"key": "keylock", "label": "Keypad Lock", "type": "boolean", "default": false},
                {"key": "wmchannel.mrcha", "label": "MR A Channel", "type": "integer", "min": 0, "max": 127, "default": 0},
                {"key": "wmchannel.mrchb", "label": "MR B Channel", "type": "integer", "min": 0, "max": 127, "default": 0},
                {"key": "vfoa.freq", "label": "VFO A Frequency (MHz)", "type": "text", "max_length": 10, "default": ""},
                {"key": "vfob.freq", "label": "VFO B Frequency (MHz)", "type": "text", "max_length": 10, "default": ""},
                {"key": "vfoa.sftd", "label": "VFO A Shift", "type": "select", "options": ["Off", "+", "-"], "default": "Off"},
                {"key": "vfob.sftd", "label": "VFO B Shift", "type": "select", "options": ["Off", "+", "-"], "default": "Off"},
                {"key": "vfoa.offset", "label": "VFO A Offset (0.0-999.999)", "type": "text", "max_length": 10, "default": ""},
                {"key": "vfob.offset", "label": "VFO B Offset (0.0-999.999)", "type": "text", "max_length": 10, "default": ""},
                {"key": "vfoa.txpower", "label": "VFO A Power", "type": "select", "options": ["High", "Low"], "default": "High"},
                {"key": "vfob.txpower", "label": "VFO B Power", "type": "select", "options": ["High", "Low"], "default": "High"},
                {"key": "vfoa.widenarr", "label": "VFO A Bandwidth", "type": "select", "options": ["Wide", "Narrow"], "default": "Wide"},
                {"key": "vfob.widenarr", "label": "VFO B Bandwidth", "type": "select", "options": ["Wide", "Narrow"], "default": "Wide"},
                {"key": "vfoa.scode", "label": "VFO A PTT-ID", "type": "select", "options": ["1", "2", "3", "4", "5", "6", "7", "8", "9", "10", "11", "12", "13", "14", "15"], "default": "1"},
                {"key": "vfob.scode", "label": "VFO B PTT-ID", "type": "select", "options": ["1", "2", "3", "4", "5", "6", "7", "8", "9", "10", "11", "12", "13", "14", "15"], "default": "1"},
                {"key": "vfoa.step", "label": "VFO A Tuning Step", "type": "select", "options": ["2.5", "5.0", "6.25", "10.0", "12.5", "25.0"], "default": "5.0"},
                {"key": "vfob.step", "label": "VFO B Tuning Step", "type": "select", "options": ["2.5", "5.0", "6.25", "10.0", "12.5", "25.0"], "default": "5.0"},
                {"key": "pttid/0.code", "label": "PTT ID Code 1", "type": "text", "max_length": 5, "default": ""},
                {"key": "pttid/1.code", "label": "PTT ID Code 2", "type": "text", "max_length": 5, "default": ""},
                {"key": "pttid/2.code", "label": "PTT ID Code 3", "type": "text", "max_length": 5, "default": ""},
                {"key": "pttid/3.code", "label": "PTT ID Code 4", "type": "text", "max_length": 5, "default": ""},
                {"key": "pttid/4.code", "label": "PTT ID Code 5", "type": "text", "max_length": 5, "default": ""},
                {"key": "pttid/5.code", "label": "PTT ID Code 6", "type": "text", "max_length": 5, "default": ""},
                {"key": "pttid/6.code", "label": "PTT ID Code 7", "type": "text", "max_length": 5, "default": ""},
                {"key": "pttid/7.code", "label": "PTT ID Code 8", "type": "text", "max_length": 5, "default": ""},
                {"key": "pttid/8.code", "label": "PTT ID Code 9", "type": "text", "max_length": 5, "default": ""},
                {"key": "pttid/9.code", "label": "PTT ID Code 10", "type": "text", "max_length": 5, "default": ""},
                {"key": "pttid/10.code", "label": "PTT ID Code 11", "type": "text", "max_length": 5, "default": ""},
                {"key": "pttid/11.code", "label": "PTT ID Code 12", "type": "text", "max_length": 5, "default": ""},
                {"key": "pttid/12.code", "label": "PTT ID Code 13", "type": "text", "max_length": 5, "default": ""},
                {"key": "pttid/13.code", "label": "PTT ID Code 14", "type": "text", "max_length": 5, "default": ""},
                {"key": "pttid/14.code", "label": "PTT ID Code 15", "type": "text", "max_length": 5, "default": ""},
                {"key": "ani.code", "label": "ANI Code", "type": "text", "max_length": 5, "default": ""},
                {"key": "ani.aniid", "label": "ANI ID", "type": "select", "options": ["Off", "BOT", "EOT", "Both"], "default": "Off"},
                {"key": "ani.alarmcode", "label": "Alarm Code", "type": "text", "max_length": 3, "default": ""},
                {"key": "dtmfst", "label": "DTMF Sidetone", "type": "select", "options": ["OFF", "DT-ST", "ANI-ST", "DT+ANI"], "default": "DT-ST"},
                {"key": "ani.dtmfon", "label": "DTMF Speed (on)", "type": "select", "options": ["50 ms", "60 ms", "70 ms", "80 ms", "90 ms", "100 ms", "110 ms", "120 ms", "130 ms", "140 ms", "150 ms", "160 ms", "170 ms", "180 ms", "190 ms", "200 ms", "210 ms", "220 ms", "230 ms", "240 ms", "250 ms", "260 ms", "270 ms", "280 ms", "290 ms", "300 ms", "310 ms", "320 ms", "330 ms", "340 ms", "350 ms", "360 ms", "370 ms", "380 ms", "390 ms", "400 ms", "410 ms", "420 ms", "430 ms", "440 ms", "450 ms", "460 ms", "470 ms", "480 ms", "490 ms", "500 ms", "510 ms", "520 ms", "530 ms", "540 ms", "550 ms", "560 ms", "570 ms", "580 ms", "590 ms", "600 ms", "610 ms", "620 ms", "630 ms", "640 ms", "650 ms", "660 ms", "670 ms", "680 ms", "690 ms", "700 ms", "710 ms", "720 ms", "730 ms", "740 ms", "750 ms", "760 ms", "770 ms", "780 ms", "790 ms", "800 ms", "810 ms", "820 ms", "830 ms", "840 ms", "850 ms", "860 ms", "870 ms", "880 ms", "890 ms", "900 ms", "910 ms", "920 ms", "930 ms", "940 ms", "950 ms", "960 ms", "970 ms", "980 ms", "990 ms", "1000 ms", "1010 ms", "1020 ms", "1030 ms", "1040 ms", "1050 ms", "1060 ms", "1070 ms", "1080 ms", "1090 ms", "1100 ms", "1110 ms", "1120 ms", "1130 ms", "1140 ms", "1150 ms", "1160 ms", "1170 ms", "1180 ms", "1190 ms", "1200 ms", "1210 ms", "1220 ms", "1230 ms", "1240 ms", "1250 ms", "1260 ms", "1270 ms", "1280 ms", "1290 ms", "1300 ms", "1310 ms", "1320 ms", "1330 ms", "1340 ms", "1350 ms", "1360 ms", "1370 ms", "1380 ms", "1390 ms", "1400 ms", "1410 ms", "1420 ms", "1430 ms", "1440 ms", "1450 ms", "1460 ms", "1470 ms", "1480 ms", "1490 ms", "1500 ms", "1510 ms", "1520 ms", "1530 ms", "1540 ms", "1550 ms", "1560 ms", "1570 ms", "1580 ms", "1590 ms", "1600 ms", "1610 ms", "1620 ms", "1630 ms", "1640 ms", "1650 ms", "1660 ms", "1670 ms", "1680 ms", "1690 ms", "1700 ms", "1710 ms", "1720 ms", "1730 ms", "1740 ms", "1750 ms", "1760 ms", "1770 ms", "1780 ms", "1790 ms", "1800 ms", "1810 ms", "1820 ms", "1830 ms", "1840 ms", "1850 ms", "1860 ms", "1870 ms", "1880 ms", "1890 ms", "1900 ms", "1910 ms", "1920 ms", "1930 ms", "1940 ms", "1950 ms", "1960 ms", "1970 ms", "1980 ms", "1990 ms", "2000 ms"], "default": "100 ms"},
                {"key": "ani.dtmfoff", "label": "DTMF Speed (off)", "type": "select", "options": ["50 ms", "60 ms", "70 ms", "80 ms", "90 ms", "100 ms", "110 ms", "120 ms", "130 ms", "140 ms", "150 ms", "160 ms", "170 ms", "180 ms", "190 ms", "200 ms", "210 ms", "220 ms", "230 ms", "240 ms", "250 ms", "260 ms", "270 ms", "280 ms", "290 ms", "300 ms", "310 ms", "320 ms", "330 ms", "340 ms", "350 ms", "360 ms", "370 ms", "380 ms", "390 ms", "400 ms", "410 ms", "420 ms", "430 ms", "440 ms", "450 ms", "460 ms", "470 ms", "480 ms", "490 ms", "500 ms", "510 ms", "520 ms", "530 ms", "540 ms", "550 ms", "560 ms", "570 ms", "580 ms", "590 ms", "600 ms", "610 ms", "620 ms", "630 ms", "640 ms", "650 ms", "660 ms", "670 ms", "680 ms", "690 ms", "700 ms", "710 ms", "720 ms", "730 ms", "740 ms", "750 ms", "760 ms", "770 ms", "780 ms", "790 ms", "800 ms", "810 ms", "820 ms", "830 ms", "840 ms", "850 ms", "860 ms", "870 ms", "880 ms", "890 ms", "900 ms", "910 ms", "920 ms", "930 ms", "940 ms", "950 ms", "960 ms", "970 ms", "980 ms", "990 ms", "1000 ms", "1010 ms", "1020 ms", "1030 ms", "1040 ms", "1050 ms", "1060 ms", "1070 ms", "1080 ms", "1090 ms", "1100 ms", "1110 ms", "1120 ms", "1130 ms", "1140 ms", "1150 ms", "1160 ms", "1170 ms", "1180 ms", "1190 ms", "1200 ms", "1210 ms", "1220 ms", "1230 ms", "1240 ms", "1250 ms", "1260 ms", "1270 ms", "1280 ms", "1290 ms", "1300 ms", "1310 ms", "1320 ms", "1330 ms", "1340 ms", "1350 ms", "1360 ms", "1370 ms", "1380 ms", "1390 ms", "1400 ms", "1410 ms", "1420 ms", "1430 ms", "1440 ms", "1450 ms", "1460 ms", "1470 ms", "1480 ms", "1490 ms", "1500 ms", "1510 ms", "1520 ms", "1530 ms", "1540 ms", "1550 ms", "1560 ms", "1570 ms", "1580 ms", "1590 ms", "1600 ms", "1610 ms", "1620 ms", "1630 ms", "1640 ms", "1650 ms", "1660 ms", "1670 ms", "1680 ms", "1690 ms", "1700 ms", "1710 ms", "1720 ms", "1730 ms", "1740 ms", "1750 ms", "1760 ms", "1770 ms", "1780 ms", "1790 ms", "1800 ms", "1810 ms", "1820 ms", "1830 ms", "1840 ms", "1850 ms", "1860 ms", "1870 ms", "1880 ms", "1890 ms", "1900 ms", "1910 ms", "1920 ms", "1930 ms", "1940 ms", "1950 ms", "1960 ms", "1970 ms", "1980 ms", "1990 ms", "2000 ms"], "default": "100 ms"},
                {"key": "pttlt", "label": "PTT ID Delay", "type": "integer", "min": 0, "max": 50, "default": 0},
                {"key": "poweron_msg.line1", "label": "Power-On Message Line 1", "type": "text", "max_length": 7, "default": ""},
                {"key": "poweron_msg.line2", "label": "Power-On Message Line 2", "type": "text", "max_length": 7, "default": ""},
                {"key": "limits.vhf.lower", "label": "VHF Lower Limit (MHz)", "type": "integer", "min": 1, "max": 1000, "default": 136},
                {"key": "limits.vhf.upper", "label": "VHF Upper Limit (MHz)", "type": "integer", "min": 1, "max": 1000, "default": 174},
                {"key": "limits.vhf.enable", "label": "VHF TX Enabled", "type": "boolean", "default": true},
                {"key": "limits.uhf.lower", "label": "UHF Lower Limit (MHz)", "type": "integer", "min": 1, "max": 1000, "default": 400},
                {"key": "limits.uhf.upper", "label": "UHF Upper Limit (MHz)", "type": "integer", "min": 1, "max": 1000, "default": 520},
                {"key": "limits.uhf.enable", "label": "UHF TX Enabled", "type": "boolean", "default": true}
            ]"#,
        },
        // --------------------------------------------------------
        // 9. Binteradio BT-9000 (issue #43) — analog FM/NFM/AM clone-mode HT.
        //    One badge on an OEM platform also sold as the Radtel RT-950 Pro,
        //    Bajeton BJ-9000 and Tenway TP-900 Pro; the radio reports its model
        //    as "RT-950" whatever the case says.
        //
        //    960 channels in FIXED zones of 99: nine full ones and a tenth
        //    holding 69. Zones carry no names in the radio — membership is
        //    index/99 and the vendor CPS keeps labels only in its own file.
        //
        //    ⚠⚠ NOT the manual's "15 zones ... 64 channels per zone", and not
        //    the RT-950 Pro reference driver's `index // 64` either. Both are
        //    wrong, and this app shipped a codeplug on them: the second channel
        //    list went to memory 64, which the radio calls zone 1 CHANNEL 65,
        //    so the operator got one zone. Eleven memories were written and
        //    read back off the radio's own screen to settle it — three of them
        //    as predictions. See `CHANNELS_PER_ZONE` in the driver.
        //
        //    ⚠ `zones_supported` was false for that reason, on the argument
        //    that this app would be offering a name the radio cannot store.
        //    That confused the LABEL with the ZONE. The radio has fifteen of
        //    them and the operator switches between them; declaring otherwise
        //    did not withhold a name, it dumped every channel list into one
        //    undifferentiated run of memories. The flag now says what is true —
        //    fifteen zones of sixty-four — and `commands/export.rs`
        //    (`fixed_zone_layout`) reads those two numbers to lay one channel
        //    list into each zone. The names stay ours: the Program dialog shows
        //    the zone map, because the radio cannot.
        // --------------------------------------------------------
        ModelSeed {
            manufacturer: "Binteradio",
            model: "BT-9000",
            driver_key: Some("binteradio_bt9000"),
            programming_ui: Some("generic"),
            display_name: "Binteradio BT-9000",
            analog_capable: true,
            dmr_capable: false,
            dstar_capable: false,
            ysf_capable: false,
            nxdn_capable: false,
            p25_capable: false,
            m17_capable: false,
            // The radio HAS APRS and GPS, and its APRS block decodes correctly.
            // The flag stays false because the block cannot be WRITTEN: 0x55
            // answers 0x06 and never commits (issue #43, measured four times).
            // Claiming the capability would put an APRS form in front of the
            // operator that silently does nothing.
            aprs_capable: false,
            // Updated with the measured bands: `tx_bands` now reaches 18-64 MHz
            // (27.500 and 50.125 both keyed, so part of it is HF) and 200-260
            // (223.500 keyed, which is 1.25 m). These flags are display only —
            // `tx_capable` reads `tx_bands` — but they are what the band chips
            // in the codeplug and profile screens show, and therefore what an
            // operator picks a radio by.
            covers_hf: true,
            covers_vhf: true,
            covers_uhf: true,
            covers_220: true,
            covers_900: false,
            freq_min: 18.0,
            freq_max: 520.0,
            // MEASURED on the radio (s128), one channel per band edge and the
            // PTT pressed on each. The image could not answer this — it stores
            // every frequency it is given — so each of these was keyed.
            //
            // | probe | keys? |
            // |---|---|
            // | 27.500, 50.125 | yes (27.5 needs the 18-64 Work Band selected) |
            // | 108.000 | **NO** — receives AM, refuses to transmit |
            // | 136.000, 145.100, 174.000 | yes |
            // | 200.000, 223.500, 260.000 | yes — the `F` blob's third pair is REAL |
            // | 400.000, 431.100, 520.000 | yes |
            // | 580.000 | not settable on the VFO at all |
            //
            // So the radio DOES gate transmit — 108 refusing is what proves the
            // other twelve "yes" answers mean something — and 220 MHz is real,
            // which the manual never mentions.
            //
            // ⚠ The spans between tested points are NOT measured: 64-108,
            // 108-136, 174-200, 260-400 and 520-580 are excluded because
            // nothing was keyed there, not because anything refused. The three
            // narrow spans take both of their own edges from a successful key;
            // 18-64 takes the range the radio's own Work Band menu declares,
            // with 27.5 and 50.125 confirmed inside it.
            tx_bands: Some("[[18.0,64.0],[136.0,174.0],[200.0,260.0],[400.0,520.0]]"),
            // The receiver is wider than the transmitter, which is why these now
            // differ. A channel that lands here but not in `tx_bands` is
            // programmed receive-only — byte 15 bit 1 clear — which is measured,
            // not assumed: such a channel refuses the PTT while its neighbour
            // keys normally.
            //
            // ⚠ CAPPED AT 520, not at the 999 the Work Band menu advertises.
            // The highest frequency anything was confirmed at is 520.000, and
            // the next probe up — 580.000 — could not be dialled on the VFO at
            // all. Claiming 999 on the strength of a menu label would make a 902
            // MHz repeater (RepeaterBook carries them) `ReceiveOnly` instead of
            // `Excluded`: it would take a memory slot and be reported as
            // written, on a radio that cannot tune it. That is the
            // silently-empty-slot failure this project already recorded once on
            // the ID-52, and the old conservative seed did not have it.
            rx_bands: Some("[[18.0,520.0]]"),
            memory_channels: 960,
            zones_supported: true,
            max_zones: Some(10),
            channels_per_zone: Some(99),
            // Per-channel scan-add flag (byte 15 bit 2), not named scan lists.
            scan_lists_supported: false,
            max_scan_lists: None,
            banks_supported: false,
            max_name_length: 12,
            export_format: "chirp_csv",
            connection_type: "Kenwood K1 (2-pin)",
            // Generated from scratchpad/binteradio_bt9000/MEASURED.md, and
            // carrying only the fields whose encoding was settled on the
            // radio's own screen. The sheet names ~43 candidates; the rest are
            // measured but not settled, and are listed by the generator on
            // stderr rather than emitted. SCREEN-CHECK.md is the runbook that
            // closes the gap.
            non_channel_settings_schema: BT9000_SETTINGS_SCHEMA,
        },
        // --------------------------------------------------------
        // 2. TIDRADIO TD-H3 — analog FM/NFM/AM, 200 ch, no zones, 8 char.
        //    Wide-RX (18-600) dual/tri-band HT; TX 136-600 on the unlocked
        //    variant (220 MHz behind the TX-220 toggle). Export only (CHIRP
        //    CSV); no direct serial codec yet, so the Program/Download-settings
        //    buttons stay hidden (those gate on model == "UV-5R").
        // --------------------------------------------------------
        ModelSeed {
            manufacturer: "TIDRADIO",
            model: "TD-H3",
            driver_key: Some("tidradio_tdh3"),
            programming_ui: Some("tdh3"),
            display_name: "TIDRADIO TD-H3",
            analog_capable: true,
            dmr_capable: false,
            dstar_capable: false,
            ysf_capable: false,
            nxdn_capable: false,
            p25_capable: false,
            m17_capable: false,
            aprs_capable: false,
            covers_hf: false,
            covers_vhf: true,
            covers_uhf: true,
            covers_220: true,
            covers_900: false,
            // TX span of the unlocked TD-H3 (CHIRP _txbands 136-600). It RX
            // down to 18 MHz, but channels we program are TX-capable repeaters,
            // so the export band filter uses the TX range.
            freq_min: 136.0,
            freq_max: 600.0,
            tx_bands: None,
            rx_bands: None,
            memory_channels: 200,
            zones_supported: false,
            max_zones: None,
            channels_per_zone: None,
            // Per-channel scan-add (skip) flag, not named scan lists — flat
            // memory pool, same as the UV-5R.
            scan_lists_supported: false,
            max_scan_lists: None,
            banks_supported: false,
            max_name_length: 8,
            export_format: "chirp_csv",
            connection_type: "Kenwood K1 (2-pin)",
            // Global (non-channel) settings sourced from the CHIRP tdh8.py
            // driver's H3 branch (Basic, A/B VFO, FM broadcast, DTMF groups).
            // Keys match CHIRP's RadioSetting names so a future TD-H3 .img
            // exporter can map directly. The 25 FM-broadcast memory presets are
            // omitted (channel-like data, not a global setting). Defaults are
            // sensible factory-ish values (CHIRP reads them from the radio).
            non_channel_settings_schema: r##"[
                {"key": "squelch", "label": "Squelch Level", "type": "select", "options": ["0", "1", "2", "3", "4", "5", "6", "7", "8", "9"], "default": "3"},
                {"key": "ligcon", "label": "Backlight Timeout", "type": "select", "options": ["CONT", "5s", "10s", "15s", "30s"], "default": "5s"},
                {"key": "voiceprompt", "label": "Voice Prompt", "type": "boolean", "default": true},
                {"key": "keyautolock", "label": "Auto Keypad Lock", "type": "boolean", "default": false},
                {"key": "mdfa", "label": "Display Mode (A)", "type": "select", "options": ["Frequency", "Name"], "default": "Name"},
                {"key": "mdfb", "label": "Display Mode (B)", "type": "select", "options": ["Frequency", "Name"], "default": "Name"},
                {"key": "sync", "label": "A/B Sync", "type": "boolean", "default": false},
                {"key": "save", "label": "Power Save", "type": "select", "options": ["Off", "1:1", "1:2", "1:3", "1:4", "1:8"], "default": "1:2"},
                {"key": "dbrx", "label": "Dual Receive", "type": "boolean", "default": true},
                {"key": "astep", "label": "Tuning Step (A)", "type": "select", "options": ["2.5", "5.0", "6.25", "10.0", "12.5", "25.0"], "default": "5.0"},
                {"key": "bstep", "label": "Tuning Step (B)", "type": "select", "options": ["2.5", "5.0", "6.25", "10.0", "12.5", "25.0"], "default": "5.0"},
                {"key": "scanmode", "label": "Scan Resume", "type": "select", "options": ["TO", "CO", "SE"], "default": "CO"},
                {"key": "pritx", "label": "Priority TX", "type": "select", "options": ["Edit", "Busy"], "default": "Edit"},
                {"key": "btnvoice", "label": "Key Beep", "type": "boolean", "default": true},
                {"key": "rogerprompt", "label": "Roger Beep", "type": "select", "options": ["Off", "TONE1", "TONE2"], "default": "Off"},
                {"key": "brightness", "label": "Brightness", "type": "select", "options": ["1", "2", "3", "4", "5"], "default": "3"},
                {"key": "txled", "label": "Display LCD (TX)", "type": "boolean", "default": true},
                {"key": "rxled", "label": "Display LCD (RX)", "type": "boolean", "default": true},
                {"key": "onlychmode", "label": "Channel Mode Only", "type": "boolean", "default": false},
                {"key": "ssidekey1", "label": "PF1 Key (Short Press)", "type": "select", "options": ["None", "FM Radio", "Lamp", "Monitor", "TONE", "Alarm", "Weather"], "default": "Monitor"},
                {"key": "lsidekey3", "label": "PF1 Key (Long Press)", "type": "select", "options": ["None", "FM Radio", "Lamp", "Monitor", "TONE", "Alarm", "Weather"], "default": "FM Radio"},
                {"key": "tonevoice", "label": "Repeater Tone Burst", "type": "select", "options": ["1000 Hz", "1450 Hz", "1750 Hz", "2100 Hz"], "default": "1750 Hz"},
                {"key": "tailclean", "label": "QT/DQT Tail Elimination", "type": "boolean", "default": true},
                {"key": "fmrec", "label": "Default Bandwidth", "type": "select", "options": ["Wide", "Narrow"], "default": "Wide"},
                {"key": "lang", "label": "Language", "type": "select", "options": ["Chinese", "English"], "default": "English"},
                {"key": "alarm", "label": "Alarm Mode", "type": "select", "options": ["On site", "Alarm"], "default": "On site"},
                {"key": "amband", "label": "AM Band Enable", "type": "boolean", "default": false},
                {"key": "tot", "label": "Time-Out Timer", "type": "select", "options": ["Off", "30S", "60S", "90S", "120S", "150S", "180S", "210S"], "default": "120S"},
                {"key": "tx220", "label": "TX 220 MHz Enable", "type": "boolean", "default": false},
                {"key": "tx350", "label": "TX 350 MHz Enable", "type": "boolean", "default": false},
                {"key": "tx500", "label": "TX 500 MHz Enable", "type": "boolean", "default": false},
                {"key": "scanband", "label": "Scan Band", "type": "select", "options": ["All", "0.5M", "1.0M", "1.5M", "2.0M", "2.5M", "3.0M", "3.5M", "4.0M", "4.5M", "5.0M"], "default": "All"},
                {"key": "voxgain", "label": "VOX Gain", "type": "select", "options": ["Off", "1", "2", "3", "4", "5"], "default": "Off"},
                {"key": "voxdelay", "label": "VOX Delay", "type": "select", "options": ["1.05s", "2.0s", "3.0s"], "default": "1.05s"},
                {"key": "breathled", "label": "Breathing LED", "type": "select", "options": ["Off", "5S", "10S", "15S", "30S"], "default": "Off"},
                {"key": "ponmsg", "label": "Power-On Display", "type": "select", "options": ["Off", "Msg", "Icon"], "default": "Msg"},
                {"key": "micgain", "label": "Mic Gain", "type": "select", "options": ["0", "1", "2", "3", "4", "5", "6", "7", "8", "9"], "default": "5"},
                {"key": "kill", "label": "Remote Kill Enable", "type": "boolean", "default": false},
                {"key": "stun", "label": "Remote Stun Enable", "type": "boolean", "default": false},
                {"key": "poweron_msg.msg1", "label": "Power-On Message 1", "type": "text", "max_length": 16, "default": ""},
                {"key": "poweron_msg.msg2", "label": "Power-On Message 2", "type": "text", "max_length": 16, "default": ""},
                {"key": "poweron_msg.msg3", "label": "Power-On Message 3", "type": "text", "max_length": 16, "default": ""},
                {"key": "rxfreqa", "label": "VFO A Frequency (MHz)", "type": "text", "max_length": 10, "default": ""},
                {"key": "a", "label": "VFO A Offset (MHz)", "type": "text", "max_length": 10, "default": ""},
                {"key": "dira", "label": "VFO A Offset Direction", "type": "select", "options": ["Off", "-", "+"], "default": "Off"},
                {"key": "lowpower", "label": "VFO A Power", "type": "select", "options": ["Low", "High"], "default": "High"},
                {"key": "wide", "label": "VFO A Bandwidth", "type": "select", "options": ["Wide", "Narrow"], "default": "Wide"},
                {"key": "bcl", "label": "VFO A Busy Lockout", "type": "boolean", "default": false},
                {"key": "specialqta", "label": "VFO A Special QT/DQT", "type": "boolean", "default": false},
                {"key": "aworkmode", "label": "VFO A Work Mode", "type": "select", "options": ["VFO", "VFO+CH", "CH Mode"], "default": "CH Mode"},
                {"key": "rxfreqb", "label": "VFO B Frequency (MHz)", "type": "text", "max_length": 10, "default": ""},
                {"key": "b", "label": "VFO B Offset (MHz)", "type": "text", "max_length": 10, "default": ""},
                {"key": "dirb", "label": "VFO B Offset Direction", "type": "select", "options": ["Off", "-", "+"], "default": "Off"},
                {"key": "lowpowerb", "label": "VFO B Power", "type": "select", "options": ["Low", "High"], "default": "High"},
                {"key": "wideb", "label": "VFO B Bandwidth", "type": "select", "options": ["Wide", "Narrow"], "default": "Wide"},
                {"key": "bclb", "label": "VFO B Busy Lockout", "type": "boolean", "default": false},
                {"key": "specialqtb", "label": "VFO B Special QT/DQT", "type": "boolean", "default": false},
                {"key": "bworkmode", "label": "VFO B Work Mode", "type": "select", "options": ["VFO", "VFO+CH", "CH Mode"], "default": "CH Mode"},
                {"key": "fmworkmode", "label": "FM Radio Work Mode", "type": "select", "options": ["VFO", "CH"], "default": "VFO"},
                {"key": "fmroad", "label": "FM Radio Channel", "type": "select", "options": ["0", "1", "2", "3", "4", "5", "6", "7", "8", "9", "10", "11", "12", "13", "14", "15", "16", "17", "18", "19", "20", "21", "22", "23", "24", "25"], "default": "0"},
                {"key": "fmvfo", "label": "FM Radio VFO (76.0-108.0 MHz)", "type": "text", "max_length": 6, "default": ""},
                {"key": "gcode", "label": "DTMF Group Code", "type": "select", "options": ["Off", "*", "#", "A", "B", "C", "D"], "default": "Off"},
                {"key": "icode", "label": "DTMF ID Code", "type": "text", "max_length": 3, "default": ""},
                {"key": "group1", "label": "DTMF Group 1", "type": "text", "max_length": 7, "default": ""},
                {"key": "group2", "label": "DTMF Group 2", "type": "text", "max_length": 7, "default": ""},
                {"key": "group3", "label": "DTMF Group 3", "type": "text", "max_length": 7, "default": ""},
                {"key": "group4", "label": "DTMF Group 4", "type": "text", "max_length": 7, "default": ""},
                {"key": "group5", "label": "DTMF Group 5", "type": "text", "max_length": 7, "default": ""},
                {"key": "group6", "label": "DTMF Group 6", "type": "text", "max_length": 7, "default": ""},
                {"key": "group7", "label": "DTMF Group 7", "type": "text", "max_length": 7, "default": ""},
                {"key": "group8", "label": "DTMF Group 8", "type": "text", "max_length": 7, "default": ""},
                {"key": "scode", "label": "PTT ID Start Code (BOT)", "type": "text", "max_length": 7, "default": ""},
                {"key": "ecode", "label": "PTT ID End Code (EOT)", "type": "text", "max_length": 7, "default": ""},
                {"key": "stuncode", "label": "Stun Code", "type": "text", "max_length": 16, "default": ""},
                {"key": "killcode", "label": "Kill Code", "type": "text", "max_length": 16, "default": ""},
                {"key": "dtmfst", "label": "DTMF Side Tone", "type": "boolean", "default": true},
                {"key": "dtmfdecode", "label": "DTMF Decode Enable", "type": "boolean", "default": false},
                {"key": "dtmfautorst", "label": "DTMF Auto Reset Time", "type": "select", "options": ["Off", "5S", "10S", "15S"], "default": "Off"},
                {"key": "dtmfdecoderesp", "label": "DTMF Decode Response", "type": "select", "options": ["NULL", "RING", "REPLY", "BOTH"], "default": "NULL"},
                {"key": "dtmfspeed", "label": "DTMF Speed", "type": "select", "options": ["80ms", "90ms", "100ms", "110ms", "120ms", "130ms", "140ms", "150ms"], "default": "100ms"}
            ]"##,
        },
        // --------------------------------------------------------
        // 3. AnyTone AT-D890UV — DMR Tier I/II + analog FM dual-band
        //    HT, 4000 channels in up to 250 zones (160 ch/zone), 250
        //    scan lists, 16-char alpha tags. APRS (analog + DMR) TX/RX,
        //    GPS, Bluetooth, Air-band AM RX, USB-C. This is the FIRST
        //    real model to use the DMR-native `anytone_csv` exporter
        //    (already built + tested in commands/export.rs): it emits an
        //    Anytone CPS Channel CSV plus a Digital Contacts/TalkGroups
        //    CSV, so per-talkgroup color-code/timeslot/contact data
        //    survives the round-trip (unlike the analog chirp_csv path).
        //    NOTES/FOLLOW-UPS: (a) zones_supported is on, so channel
        //    lists map to zones (the list→zone foundation), but the
        //    anytone exporter does not yet emit a Zone column — channels
        //    still export, just not zone-segregated; a Zone CSV is the
        //    follow-up. (b) nxdn_capable is left false: retailers market
        //    it as "DMR/NXDN" but NXDN may require a firmware update and
        //    isn't part of the export pipeline — flip it on if wanted.
        //    (c) freq_max uses the US TX ceiling (480); non-US units TX
        //    to 520 and a 220-225 RX band exists on band 14.
        // --------------------------------------------------------
        ModelSeed {
            manufacturer: "AnyTone",
            model: "AT-D890UV",
            driver_key: Some("anytone_atd890uv"),
            programming_ui: Some("anytone"),
            display_name: "AnyTone AT-D890UV",
            analog_capable: true,
            dmr_capable: true,
            dstar_capable: false,
            ysf_capable: false,
            nxdn_capable: false,
            p25_capable: false,
            m17_capable: false,
            aprs_capable: true,
            covers_hf: false,
            covers_vhf: true,
            covers_uhf: true,
            covers_220: false,
            covers_900: false,
            // US TX span: 2 m (136-174) + 70 cm (400-480). RX is wider
            // (FM bcast 87.5-108, air 108-136); channels we program are
            // TX-capable, so the export band filter uses the TX range.
            freq_min: 136.0,
            freq_max: 480.0,
            tx_bands: None,
            rx_bands: None,
            memory_channels: 4000,
            zones_supported: true,
            max_zones: Some(250),
            // Manual: a zone holds max 160 analog and/or digital channels
            // (the 0x200 zone-list stride in the memory map is a fixed
            // allocation, only ~160×2 bytes of it are used).
            channels_per_zone: Some(160),
            scan_lists_supported: true,
            max_scan_lists: Some(250),
            banks_supported: false,
            max_name_length: 16,
            export_format: "anytone_csv",
            connection_type: "USB-C (CPS cable)",
            // Full General Settings block (0x03500000, 204 fields), GENERATED
            // from the dmr-tools d890uv v1.05 definition by
            // scratchpad/gen_anytone_settings.py — same source as the Rust
            // decode table (commands/anytone_settings_table.rs) so UI + decoder
            // can't drift. Read via the generic `read_radio_settings` command.
            non_channel_settings_schema: include_str!("anytone_settings_schema.json"),
        },
        // --------------------------------------------------------
        // 4. Yaesu FT5D — C4FM (System Fusion) + analog FM dual-band
        //    HT, 900 memories in 24 banks, 16-char alpha tags, 5 W.
        //    Covers the FT5DR (US/Asia/AU) and FT5DE (EU) variants;
        //    "FT5D" unhyphenated follows Yaesu's own documents and
        //    CHIRP's naming for the family (FT1D/FT2D/FT3D).
        //    Programmed via the microSD card (issue #32):
        //    `yaesu_ft5d_sd` patches the radio's own
        //    FT5D/BACKUP/BACKUP.dat in place, so this "export" writes
        //    a codeplug the radio restores directly — no cable. The
        //    settings come out of and go back into that same file, so
        //    the profile is populated too; see radios/yaesu_ft5d/.
        //    `programming_ui` is 'generic' because the generic dialog
        //    is what carries the "Write to SD card…" action: it drops
        //    the port picker and the cable buttons on its own once the
        //    driver reports no cable capability, and picks the media
        //    path up from `export_format` (lib/radioProgramming.ts).
        //    NOTES: (a) banks_supported is on (24 banks), so channel
        //    lists map to banks once a programming path exists; the
        //    FT5D has no zones and no scan lists. (b) tx_bands says
        //    what freq_min/freq_max cannot: the FT5D's two disjoint
        //    US TX bands, so a 155 MHz or 462 MHz channel is no
        //    longer mistaken for one the radio can talk on.
        //    freq_min/freq_max stay as the enclosing span for the UI
        //    and for anything that has not learned about tx_bands.
        //    (c) rx_bands is the receiver, which is enormous by
        //    comparison — GMRS, marine, NOAA, air, 220 and 900 all
        //    land inside it, so those channels are programmed
        //    receive-only instead of being dropped (issue #32).
        //    covers_220/900 stay false: they describe transmit.
        // --------------------------------------------------------
        ModelSeed {
            manufacturer: "Yaesu",
            model: "FT5D",
            driver_key: Some("yaesu_ft5d"),
            programming_ui: Some("generic"),
            display_name: "Yaesu FT5D",
            analog_capable: true,
            dmr_capable: false,
            dstar_capable: false,
            ysf_capable: true,
            nxdn_capable: false,
            p25_capable: false,
            m17_capable: false,
            aprs_capable: true,
            covers_hf: false,
            covers_vhf: true,
            covers_uhf: true,
            covers_220: false,
            covers_900: false,
            freq_min: 144.0,
            freq_max: 450.0,
            tx_bands: Some("[[144.0,148.0],[430.0,450.0]]"),
            rx_bands: Some("[[0.5,999.995]]"),
            memory_channels: 900,
            zones_supported: false,
            max_zones: None,
            channels_per_zone: None,
            scan_lists_supported: false,
            max_scan_lists: None,
            banks_supported: true,
            max_name_length: 16,
            export_format: "yaesu_ft5d_sd",
            connection_type: "microSD or USB (SCU-19/39/57)",
            // 67 settings in 9 groups, GENERATED from CHIRP's ft1d.py by
            // scratchpad/ft5d/gen_ft5d_settings.py — the same parse that emits
            // the Rust decode table (radios/yaesu_ft5d/ft5d_settings_table.rs),
            // so the form and the decoder cannot drift. Read out of and written
            // back into the microSD backup, not over a cable.
            non_channel_settings_schema: FT5D_SETTINGS_SCHEMA,
        },
        // --------------------------------------------------------
        // 5. Icom ID-52 — D-STAR + analog FM dual-band HT, 1000
        //    memories in 100 groups, 16-char names, 5 W.
        //    Covers the ID-52A (US) and ID-52E (EU); the TX bands
        //    below are the A's.
        //    Programmed from its own microSD card (issue #38):
        //    `icom_id52_icf` patches the `.icf` the radio writes with
        //    SET > SD Card > Save Setting, and the radio reads that
        //    same file back with Load Setting > ALL — which restores
        //    memories AND every MENU setting in one operation, so the
        //    codeplug and the radio profile travel together in one
        //    file. Picking a `.csv` instead writes the Memory CH
        //    export (memories only); see radios/icom_id52/.
        //    NOTES: (a) the ID-52's "groups" are banks, not zones:
        //    exactly one per memory, stored in the memory itself, so
        //    a channel in two lists lands in the first. (b) tx_bands
        //    carries the two ham bands; rx_bands is the A band's real
        //    coverage from Icom's own specification — 108–174 and
        //    225–479 MHz — so air band, marine, NOAA and GMRS are
        //    programmed receive-only, and anything outside is
        //    excluded. ⚠ THE GAPS ARE NOT COSMETIC. This started as
        //    one span, 0.495–1310.995, on the reasoning that an
        //    unusable receive-only memory was harmless. It is not:
        //    Tim's radio was programmed with three 220 MHz repeaters
        //    and showed those three memories as EMPTY SLOTS, having
        //    silently refused frequencies it cannot tune, while the
        //    app reported them as written. A frequency this radio
        //    does not cover has to be excluded here, where the
        //    operator is told why. (c) covers_220 / covers_900 stay
        //    false, and now they are true of the receiver too: the
        //    ID-52A has no 220 and no 800–1300 coverage at all.
        // --------------------------------------------------------
        ModelSeed {
            manufacturer: "Icom",
            model: "ID-52",
            driver_key: Some("icom_id52"),
            programming_ui: Some("generic"),
            display_name: "Icom ID-52",
            analog_capable: true,
            dmr_capable: false,
            dstar_capable: true,
            ysf_capable: false,
            nxdn_capable: false,
            p25_capable: false,
            m17_capable: false,
            aprs_capable: true,
            covers_hf: false,
            covers_vhf: true,
            covers_uhf: true,
            covers_220: false,
            covers_900: false,
            freq_min: 144.0,
            freq_max: 450.0,
            tx_bands: Some("[[144.0,148.0],[430.0,450.0]]"),
            rx_bands: Some("[[108.0,174.0],[225.0,479.0]]"),
            memory_channels: 1000,
            zones_supported: false,
            max_zones: None,
            channels_per_zone: None,
            scan_lists_supported: false,
            max_scan_lists: None,
            banks_supported: true,
            max_name_length: 16,
            export_format: "icom_id52_icf",
            connection_type: "microSD or USB (SD Card Mode)",
            // 160 settings in 12 sections, GENERATED from the measurement sheet
            // by scratchpad/id52/gen_id52_table.py — the same parse that emits
            // the Rust decode table (radios/icom_id52/id52_settings_table.rs),
            // so the form and the decoder cannot drift. Every offset in it was
            // measured by diffing two real `.icf` saves; none is inferred from
            // another radio's driver.
            non_channel_settings_schema: ID52_SETTINGS_SCHEMA,
        },
        // --------------------------------------------------------
        // 6. Kenwood TH-D75 — D-STAR + analog FM TRI-band HT, 1000
        //    memories in 30 groups, 16-char names, 5 W. The A model
        //    (US); the E's TX bands differ.
        //    Programmed from its own microSD card (issue #40):
        //    `kenwood_thd75_sd` patches the `.d75` the radio writes
        //    with Menu > Configuration > SD Card > Save Setting, and
        //    the radio reads it back with Load Setting. The card
        //    mounts over USB-C, so it never has to come out. See
        //    radios/kenwood_thd75/.
        //    NOTES: (a) covers_220 is TRUE, a first for this app —
        //    the 224 MHz repeaters that came back as EMPTY SLOTS on
        //    the ID-52 are a band this radio genuinely TRANSMITS on,
        //    and tx_bands carries all three. (b) rx_bands is Band B's
        //    continuous 0.1-524 MHz from Kenwood's own TH-D75A
        //    specification, so HF, FM broadcast, air, marine, NOAA
        //    and GMRS are all programmed receive-only. Band A is
        //    narrower (136-174 / 216-260 / 410-470), but that
        //    asymmetry is carried in the memory itself: the driver
        //    writes the band code the radio uses to file a memory as
        //    Band-A-or-Band-B, so an out-of-Band-A frequency is a
        //    memory the radio tunes on B rather than one it silently
        //    drops. ⚠ That mechanism is measured off the radio's own
        //    saves but NOT yet confirmed by loading a file into a
        //    radio — Phase 4 of scratchpad/thd75/PLAN.md programs a
        //    boundary channel deliberately, and if the radio drops
        //    it, rx_bands narrows to Band A's ranges.
        //    (c) the D75's "groups" are banks, not zones: exactly one
        //    per memory, stored in the memory itself, so a channel in
        //    two lists lands in the first. It has program-scan L/U
        //    limit pairs rather than named scan lists.
        // --------------------------------------------------------
        ModelSeed {
            manufacturer: "Kenwood",
            model: "TH-D75",
            driver_key: Some("kenwood_thd75"),
            programming_ui: Some("generic"),
            display_name: "Kenwood TH-D75",
            analog_capable: true,
            dmr_capable: false,
            dstar_capable: true,
            ysf_capable: false,
            nxdn_capable: false,
            p25_capable: false,
            m17_capable: false,
            aprs_capable: true,
            covers_hf: false,
            covers_vhf: true,
            covers_uhf: true,
            covers_220: true,
            covers_900: false,
            freq_min: 144.0,
            freq_max: 450.0,
            tx_bands: Some("[[144.0,148.0],[222.0,225.0],[430.0,450.0]]"),
            rx_bands: Some("[[0.1,524.0]]"),
            memory_channels: 1000,
            zones_supported: false,
            max_zones: None,
            channels_per_zone: None,
            scan_lists_supported: false,
            max_scan_lists: None,
            banks_supported: true,
            max_name_length: 16,
            export_format: "kenwood_thd75_sd",
            connection_type: "microSD or USB-C (SD card mode)",
            // Filled in Phase 3. The MENU offsets are the one genuinely
            // unpublished part of this format (CHIRP's D74 driver has no
            // settings support at all), so all 185 were measured through the
            // RT Systems bridge and generated into `thd75_settings_schema.json`
            // alongside the Rust decode table, the way the FT5D's and the
            // ID-52's are.
            non_channel_settings_schema: THD75_SETTINGS_SCHEMA,
        },
        // --------------------------------------------------------
        // 7. Kenwood TH-D72 — analog FM dual-band HT with APRS and a
        //    built-in GPS and TNC, 1000 memories in 10 groups,
        //    8-char names, 5 W. A 2010 radio: no D-STAR, no DMR, no
        //    C4FM. The A model (US); the E and K differ, and a radio
        //    whose TX has been hardware-extended differs again.
        //    Programmed over a CABLE (issue #55) — the built-in
        //    mini-USB port, cloning a flat 64 KiB image, which is the
        //    UV-5R/TD-H3 modality rather than the microSD patching
        //    the TH-D75 above uses. See radios/kenwood_thd72/.
        //    NOTES:
        //    (a) rx_bands has a REAL GAP and it matters. 118-174 and
        //    320-524, with nothing between: this radio cannot hear
        //    220 MHz at all, where the TH-D75 directly above it
        //    TRANSMITS there. Three sources agree — CHIRP's
        //    valid_bands, the union of the radio's own programmable-
        //    VFO table read out of a factory-reset image, and the
        //    manual naming the 118/144/300/430-440 bands. A 224 MHz
        //    repeater must be excluded here, not handed to the
        //    encoder: see the test below.
        //    (b) the D72's "groups" are POSITIONAL, not a flag —
        //    group n is memories n*100..n*100+99 and only the ten
        //    group NAMES are stored. So banks_supported is true, but
        //    a channel list of more than 100 channels cannot be one
        //    group, and a channel in two lists lands in the first.
        //    (c) program-scan L/U limit pairs, not named scan lists,
        //    so scan_lists_supported is false — same as the D75.
        //    (d) ★ tx_bands is MEASURED, not assumed. Phase 1 asked
        //    the radio itself on 2026-08-26: `TY` answered A,M,B,1 —
        //    a stock TH-D72A whose TX is NOT hardware-extended — so
        //    144-148 / 430-450 is this radio's own allocation. `FV 0`
        //    reports firmware 1.08. An E/K or a TX-modified radio
        //    answers TY differently and would need its own row; such
        //    a radio is under-served by this one, which is the safe
        //    direction.
        //    (e) settings are the radio's own 19 MU menu parameters,
        //    read and written over the ASCII command rather than
        //    through the clone image: MU covers all 19 where CHIRP
        //    names offsets for only 6, and the two agree on all six
        //    they share. p2 contrast is deliberately NOT in the
        //    schema — the manual, CHIRP and LA3QMA give three
        //    different ranges for it and the radio has not settled
        //    which. See radios/kenwood_thd72/settings.rs.
        // --------------------------------------------------------
        ModelSeed {
            manufacturer: "Kenwood",
            model: "TH-D72",
            driver_key: Some("kenwood_thd72"),
            programming_ui: Some("generic"),
            display_name: "Kenwood TH-D72",
            analog_capable: true,
            dmr_capable: false,
            dstar_capable: false,
            ysf_capable: false,
            nxdn_capable: false,
            p25_capable: false,
            m17_capable: false,
            aprs_capable: true,
            covers_hf: false,
            covers_vhf: true,
            covers_uhf: true,
            covers_220: false,
            covers_900: false,
            freq_min: 144.0,
            freq_max: 450.0,
            tx_bands: Some("[[144.0,148.0],[430.0,450.0]]"),
            rx_bands: Some("[[118.0,174.0],[320.0,524.0]]"),
            memory_channels: 1000,
            zones_supported: false,
            max_zones: None,
            channels_per_zone: None,
            scan_lists_supported: false,
            max_scan_lists: None,
            banks_supported: true,
            max_name_length: 8,
            // chirp_csv, like the UV-5R and TD-H3 above: an analog radio
            // programmed over a cable, with no file workflow of its own. The
            // .mc4 the container can write is a measuring convenience, not a
            // format any user is asked to hand around.
            export_format: "chirp_csv",
            connection_type: "USB cable (built-in mini-USB)",
            non_channel_settings_schema: THD72_SETTINGS_SCHEMA,
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::radios::registry;

    /// The two registry columns must agree with each other and with what is
    /// actually compiled in. A typo here fails SILENTLY at runtime — a bad
    /// `driver_key` only surfaces when the user plugs a radio in, and a bad
    /// `programming_ui` just falls back to the generic dialog (Chunk 3.7).
    ///
    /// ⚠️ `UIS` mirrors the keys of `PROGRAM_DIALOGS` in
    /// `src/components/codeplugs/programDialogs.ts`; adding a bespoke dialog
    /// means adding its key in both places.
    /// The tripwire this replaces has done its job.
    ///
    /// It fired if the BT-9000's `rx_bands` and `tx_bands` ever diverged, because
    /// the encoder set the per-channel TX-enable bit unconditionally and a
    /// receive-only channel would therefore have been programmed able to
    /// transmit — on a radio that validates nothing and keys wherever it is
    /// told. The bit it was waiting on (byte 15 bit 1) was measured on the radio
    /// in s128: a channel written with it clear refuses the PTT while its
    /// neighbour with it set keys normally.
    ///
    /// So the bands now legitimately differ, and what needs guarding is the
    /// other half: that the encoder actually honours the distinction.
    #[test]
    fn bt9000_receive_only_channels_are_possible_and_handled() {
        let bt = models()
            .into_iter()
            .find(|m| m.driver_key == Some("binteradio_bt9000"))
            .expect("the BT-9000 is seeded");
        assert_ne!(
            bt.rx_bands, bt.tx_bands,
            "the BT-9000 receives wider than it transmits — 108 MHz takes AM and \
             refuses the PTT — so these are expected to differ"
        );
        // The encoder's own test proves the bit; this proves the schema still
        // asks it to be used.
        let rx: Vec<[f64; 2]> = serde_json::from_str(bt.rx_bands.unwrap()).unwrap();
        let tx: Vec<[f64; 2]> = serde_json::from_str(bt.tx_bands.unwrap()).unwrap();
        // ⚠ "Some rx band is not fully covered by the UNION of the tx bands."
        //
        // The first version of this asserted `t != r` among its disjuncts, which
        // made it true for any bands that merely differ — so `rx = [[137,173]]`
        // strictly INSIDE `tx = [[136,174]]` would have passed while receive-only
        // was dead code, which is the exact thing it claims to rule out. A guard
        // that cannot fail proves nothing.
        let uncovered = rx.iter().any(|r| {
            // Walk the rx span, swallowing any tx span that overlaps it. If a gap
            // survives, the receiver reaches somewhere the transmitter does not.
            let mut lo = r[0];
            let mut spans: Vec<[f64; 2]> = tx.iter().copied().filter(|t| t[1] > r[0] && t[0] < r[1]).collect();
            spans.sort_by(|a, b| a[0].partial_cmp(&b[0]).unwrap());
            for t in spans {
                if t[0] > lo {
                    return true;
                }
                lo = lo.max(t[1]);
            }
            lo < r[1]
        });
        assert!(
            uncovered,
            "rx_bands must cover ground tx_bands does not, or receive-only is dead code"
        );
    }

    #[test]
    fn every_seeded_driver_key_resolves_and_pairs_with_a_known_ui() {
        const UIS: [&str; 3] = ["generic", "tdh3", "anytone"];
        for m in models() {
            match (m.driver_key, m.programming_ui) {
                (Some(key), Some(ui)) => {
                    assert!(
                        registry::driver_for_key(key).is_some(),
                        "{}: no compiled-in driver for '{key}'",
                        m.model
                    );
                    assert!(UIS.contains(&ui), "{}: unknown programming_ui '{ui}'", m.model);
                }
                // Export-only models carry neither; one without the other is a
                // half-migrated row.
                (None, None) => {}
                _ => panic!(
                    "{}: driver_key and programming_ui must be set together",
                    m.model
                ),
            }
        }
    }

    /// Every seeded `export_format` must be one the export command can act on:
    /// either a registered exporter's key, or `chirp_csv`, the shared analog
    /// fallback. A typo also fails silently — the model just exports CHIRP CSV
    /// instead of its native format (Chunk 3.8).
    #[test]
    fn every_seeded_export_format_resolves_or_is_the_chirp_fallback() {
        for m in models() {
            assert!(
                m.export_format == "chirp_csv"
                    || registry::exporter_for_format(m.export_format).is_some(),
                "{}: unknown export_format '{}'",
                m.model,
                m.export_format
            );
        }
    }

    /// The TH-D72 is the counter-example that sits directly under the TH-D75 in
    /// `models()`, and getting the two confused would put a channel on a radio
    /// that cannot hear it.
    ///
    /// The D75 TRANSMITS on 220 MHz. The D72 does not receive there at all —
    /// its coverage is 118-174 and 320-524 with a real hole between, agreed on
    /// by CHIRP's `valid_bands`, by the union of the radio's own
    /// programmable-VFO table read out of a factory-reset image, and by the
    /// manual naming the 118/144/300/430-440 bands. A 224 MHz repeater has to
    /// be excluded here rather than reaching the encoder.
    ///
    /// It also hears far more than it can key: the air band at 118-136 is AM
    /// receive-only, and so is the whole 320-430 stretch.
    #[test]
    fn the_thd72_cannot_hear_220_and_hears_two_disjoint_spans() {
        let d72 = models()
            .into_iter()
            .find(|m| m.model == "TH-D72")
            .expect("the TH-D72 is seeded");

        let bands = |json: &str| -> Vec<Vec<f64>> { serde_json::from_str(json).unwrap() };
        let tx = bands(d72.tx_bands.expect("TH-D72 has tx_bands"));
        let rx = bands(d72.rx_bands.expect("TH-D72 has rx_bands"));
        let covers = |bs: &[Vec<f64>], mhz: f64| bs.iter().any(|b| mhz >= b[0] && mhz <= b[1]);

        assert!(!d72.covers_220, "the TH-D72 has no 220 MHz coverage of any kind");

        // The failure the ID-52 shipped: a repeater in a band the radio cannot
        // tune, reported as written.
        assert!(!covers(&rx, 224.5), "224.5 MHz must not be receivable");
        assert!(!covers(&tx, 224.5), "224.5 MHz must not be transmittable");

        // The gap is a gap, not a rounding artefact.
        assert!(covers(&rx, 174.0) || covers(&rx, 173.9));
        assert!(!covers(&rx, 200.0), "nothing between 174 and 320 MHz is receivable");
        assert!(covers(&rx, 320.0));

        // Heard but not keyed.
        assert!(covers(&rx, 121.5) && !covers(&tx, 121.5), "air band is receive-only");
        assert!(covers(&rx, 350.0) && !covers(&tx, 350.0), "300 MHz band is receive-only");
        assert!(covers(&rx, 462.6) && !covers(&tx, 462.6), "GMRS is receive-only");

        // The two ham bands it does key on.
        assert!(covers(&tx, 146.52) && covers(&tx, 446.0));
    }

    /// ★★★ The BT-9000's zone geometry, seeded from what the RADIO says.
    ///
    /// These three columns are what `commands/export.rs::fixed_zone_layout`
    /// reads, so they alone decide which memory each channel lands in — the
    /// driver writes by slot and never thinks in zones at all.
    ///
    /// They read 15 and 64 until s130, matching the manual and the RT-950 Pro
    /// reference driver, and the driver's own constants agreed with them. Being
    /// wrong TOGETHER is why nothing caught it: a codeplug's second channel
    /// list went to memory 64, the radio calls that memory zone 1 channel 65,
    /// and the operator got one zone. Eleven memories were then written and
    /// read back off the radio's screen — three of them as predictions — and
    /// they give 99 and 10.
    #[test]
    fn the_bt9000_zone_geometry_is_the_radios_own() {
        let m = models().into_iter().find(|m| m.model == "BT-9000").unwrap();
        assert!(m.zones_supported);
        assert_eq!(m.channels_per_zone, Some(99), "zones are 99 wide, NOT the manual's 64");
        assert_eq!(m.max_zones, Some(10), "ten zones, NOT the manual's 15");
        assert_eq!(m.memory_channels, 960);
        // ⚠ 99 does not divide 960. Nine full zones and a tenth of 69 — which
        // is the radio's own answer for its last memory, 959: zone 10 ch 69.
        let per = m.channels_per_zone.unwrap();
        let last = m.memory_channels - (m.max_zones.unwrap() - 1) * per;
        assert_eq!(last, 69, "the last zone is short, and anything that multiplies loses it");
        assert_eq!(
            crate::radios::binteradio_bt9000::CHANNELS_PER_ZONE as i64, per,
            "the driver decodes a memory's zone with this too; the two must not drift"
        );
    }

    /// A gap in a seeded receiver is not a detail — it decides whether a channel
    /// is programmed or refused, and the radio does NOT refuse it politely.
    ///
    /// Tim's ID-52 was programmed with three 220 MHz repeaters inside a group of
    /// 31. The app reported 31 channels written; the radio showed positions 4, 7
    /// and 21 as EMPTY, having silently dropped the frequencies it cannot tune.
    /// That is the worst possible failure — the operator is told the channel is
    /// on the radio and it is not — and it happened because `rx_bands` was
    /// written as one span from 0.495 to 1310.995 MHz on the assumption that an
    /// unusable receive-only memory would be harmless.
    ///
    /// So: every seeded receiver must exclude what its radio genuinely cannot
    /// hear. The ID-52A's coverage is Icom's published A band, 108–174 and
    /// 225–479 MHz.
    #[test]
    fn a_seeded_receiver_excludes_what_the_radio_cannot_tune() {
        let id52 = models()
            .into_iter()
            .find(|m| m.model == "ID-52")
            .expect("the ID-52 is seeded");
        let rx: Vec<Vec<f64>> =
            serde_json::from_str(id52.rx_bands.expect("ID-52 has rx_bands")).unwrap();
        let hears = |f: f64| rx.iter().any(|b| f >= b[0] && f <= b[1]);

        // The three frequencies the radio itself rejected.
        for f in [224.320, 224.520, 224.600] {
            assert!(!hears(f), "{f} MHz: the radio showed this as an empty memory");
        }
        // ...and the rest of the 220 band with them, plus the regions this
        // radio has no receiver for at all.
        for f in [222.0, 225.0_f64 - 0.001, 0.5, 76.0, 107.9, 800.0, 900.0, 1296.0] {
            assert!(!hears(f), "{f} MHz is outside the ID-52A's coverage");
        }
        // What it genuinely does hear has to survive: air band, 2 m, marine,
        // NOAA, GMRS and 70 cm.
        for f in [118.4, 146.94, 156.8, 162.55, 462.5625, 467.7125, 446.8125] {
            assert!(hears(f), "{f} MHz is inside the ID-52A's coverage");
        }
    }

    /// The TH-D75 is the other half of that lesson: the same 220 MHz repeaters
    /// the ID-52 threw away are a band this radio TRANSMITS on, so seeding it
    /// too conservatively would drop channels the operator can legally work.
    ///
    /// The numbers are Kenwood's own TH-D75A specification — TX 144–148 /
    /// 222–225 / 430–450, Band-B RX 0.1–524 — not CHIRP's and not RT Systems'.
    #[test]
    fn the_thd75_transmits_on_220_and_hears_the_whole_of_band_b() {
        let d75 = models()
            .into_iter()
            .find(|m| m.model == "TH-D75")
            .expect("the TH-D75 is seeded");
        let bands = |json: &str| -> Vec<Vec<f64>> { serde_json::from_str(json).unwrap() };
        let tx = bands(d75.tx_bands.expect("TH-D75 has tx_bands"));
        let rx = bands(d75.rx_bands.expect("TH-D75 has rx_bands"));
        let within = |bs: &[Vec<f64>], f: f64| bs.iter().any(|b| f >= b[0] && f <= b[1]);

        assert!(d75.covers_220, "the TH-D75 is a tri-band radio");
        // The three the ID-52 showed as empty slots — transmit-capable here.
        for f in [224.320, 224.520, 224.600] {
            assert!(within(&tx, f), "{f} MHz: the TH-D75 transmits on 220");
        }
        for f in [146.94, 446.8125] {
            assert!(within(&tx, f), "{f} MHz is a TH-D75 transmit band");
        }
        // Receive-only, all inside Band B: HF, FM broadcast, air, marine, NOAA,
        // GMRS. Each is a memory the radio tunes rather than one it drops.
        for f in [3.573, 14.074, 88.5, 118.4, 156.8, 162.55, 462.5625] {
            assert!(within(&rx, f), "{f} MHz is inside Band B");
            assert!(!within(&tx, f), "{f} MHz is receive-only on a TH-D75");
        }
        // Band B stops at 524 MHz; there is no 800/900 or 23 cm receiver.
        for f in [524.001, 800.0, 900.0, 1296.0] {
            assert!(!within(&rx, f), "{f} MHz is outside the TH-D75's coverage");
        }
    }

    /// `tx_bands`/`rx_bands` are hand-written JSON that the exporter parses
    /// leniently — a typo would not fail loudly, it would quietly fall back to
    /// the old contiguous-span rules and drop channels again. Check the shape
    /// here, where it is cheap.
    #[test]
    fn seeded_band_lists_are_well_formed() {
        let parse = |json: &str| -> Vec<Vec<f64>> {
            serde_json::from_str(json).unwrap_or_else(|e| panic!("bad band JSON {json}: {e}"))
        };
        for m in models() {
            for (label, json) in [("tx_bands", m.tx_bands), ("rx_bands", m.rx_bands)] {
                let Some(json) = json else { continue };
                let bands = parse(json);
                assert!(!bands.is_empty(), "{}: empty {label}", m.model);
                for b in &bands {
                    assert_eq!(b.len(), 2, "{}: {label} entry is not a pair: {b:?}", m.model);
                    assert!(b[0] < b[1], "{}: {label} span is inverted: {b:?}", m.model);
                }
            }
            // A radio that hears less than it transmits on would be news; catch
            // a swapped pair of columns.
            if let (Some(tx), Some(rx)) = (m.tx_bands, m.rx_bands) {
                let (tx, rx) = (parse(tx), parse(rx));
                for t in &tx {
                    assert!(
                        rx.iter().any(|r| r[0] <= t[0] && r[1] >= t[1]),
                        "{}: transmit band {t:?} is outside every receive band",
                        m.model
                    );
                }
            }
        }
    }
}
