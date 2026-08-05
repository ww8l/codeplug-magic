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
                freq_min, freq_max, memory_channels,
                zones_supported, max_zones, channels_per_zone,
                scan_lists_supported, max_scan_lists, banks_supported,
                max_name_length, export_format, connection_type,
                non_channel_settings_schema, driver_key, programming_ui
            ) VALUES (
                ?1, ?2, ?3,
                ?4, ?5, ?6, ?7,
                ?8, ?9, ?10, ?11,
                ?12, ?13, ?14, ?15, ?16,
                ?17, ?18, ?19,
                ?20, ?21, ?22,
                ?23, ?24, ?25,
                ?26, ?27, ?28,
                ?29, ?30, ?31
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

/// The full official Brandmeister talkgroup list, embedded at compile time as a
/// `{ "<tg_number>": "<name>" }` JSON map (from api.brandmeister.network,
/// snapshot 2026-06-18). ~1,750 entries; refresh by re-downloading the file.
const BRANDMEISTER_TALKGROUPS: &str = include_str!("../data/brandmeister-talkgroups.json");

/// Seed the entire Brandmeister talkgroup library. Idempotent on
/// (network, tg_number); existing rows (including user edits) are left
/// untouched. All entries are Brandmeister group calls; source is 'seed'.
/// Done in one transaction since it inserts ~1,750 rows.
pub async fn seed_talkgroups(pool: &SqlitePool) -> Result<(), sqlx::Error> {
    let map: std::collections::HashMap<String, String> =
        serde_json::from_str(BRANDMEISTER_TALKGROUPS)
            .expect("bundled brandmeister-talkgroups.json is valid");

    let mut tx = pool.begin().await?;
    for (number, name) in &map {
        let Ok(tg_number) = number.parse::<i64>() else {
            continue;
        };
        // Override unhelpful canonical names. BM lists 9990 (the echo/parrot
        // test) as "None"; give it a meaningful label. Applied here (not in the
        // data file) so it survives re-downloading the list.
        let name = match tg_number {
            9990 => "Parrot (Echo Test)",
            _ => name.as_str(),
        };
        sqlx::query(
            "INSERT INTO talkgroups (tg_number, name, network, call_type, source)
             VALUES (?1, ?2, 'Brandmeister', 'Group', 'seed')
             ON CONFLICT(network, tg_number) DO NOTHING",
        )
        .bind(tg_number)
        .bind(name)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    Ok(())
}

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
            covers_hf: false,
            covers_vhf: true,
            covers_uhf: true,
            covers_220: false,
            covers_900: false,
            freq_min: 136.0,
            freq_max: 520.0,
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
        //    FT5D/BACKUP/BACKUP.dat in place, so "export" here writes
        //    a codeplug the radio restores directly — no cable. The
        //    settings schema stays empty because reading settings
        //    means live USB, which is still unproven; see
        //    radios/yaesu_ft5d/.
        //    NOTES: (a) banks_supported is on (24 banks), so channel
        //    lists map to banks once a programming path exists; the
        //    FT5D has no zones and no scan lists. (b) freq_min/max is
        //    the US TX span. Like every model here it is ONE
        //    contiguous range, so it cannot express the FT5D's two
        //    disjoint TX bands (144-148 + 430-450) — a 155 MHz
        //    channel passes the span and the covers_vhf flag even
        //    though the radio cannot transmit on it. RX is far wider
        //    (0.52-999.995 MHz), but we only program TX-capable
        //    channels, so the filter uses TX. (c) covers_220/900 stay
        //    false, which is what keeps a 224 MHz channel out.
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
}
