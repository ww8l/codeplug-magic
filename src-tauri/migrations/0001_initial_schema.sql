-- CodePlug Manager initial schema
-- All BOOLEAN columns are stored as INTEGER 0/1 (SQLite convention).
-- All DATETIME columns are stored as TEXT (ISO-8601).

-- ============================================================
-- channels : Master Channel Table (single source of truth)
-- ============================================================
CREATE TABLE channels (
    id                              INTEGER PRIMARY KEY,
    rb_name                         TEXT,
    name_long                       TEXT,
    name_short                      TEXT,
    callsign                        TEXT,
    rx_freq                         REAL NOT NULL,
    tx_freq                         REAL,
    offset                          REAL,
    duplex                          TEXT,
    band                            TEXT,
    mode                            TEXT,
    tone_mode                       TEXT,
    ctcss_uplink                    REAL,
    ctcss_downlink                  REAL,
    dcs_code                        TEXT,
    dmr_color_code                  INTEGER,
    dmr_timeslot                    INTEGER,
    dmr_talkgroup                   INTEGER,
    dstar_capable                   BOOLEAN NOT NULL DEFAULT 0,
    ysf_capable                     BOOLEAN NOT NULL DEFAULT 0,
    nxdn_capable                    BOOLEAN NOT NULL DEFAULT 0,
    p25_capable                     BOOLEAN NOT NULL DEFAULT 0,
    p25_nac                         TEXT,
    m17_capable                     BOOLEAN NOT NULL DEFAULT 0,
    m17_can                         INTEGER,
    tetra_capable                   BOOLEAN NOT NULL DEFAULT 0,
    allstar_node                    TEXT,
    echolink_node                   TEXT,
    irlp_node                       TEXT,
    wires_node                      TEXT,
    ares                            BOOLEAN NOT NULL DEFAULT 0,
    races                           BOOLEAN NOT NULL DEFAULT 0,
    skywarn                         BOOLEAN NOT NULL DEFAULT 0,
    canwarn                         BOOLEAN NOT NULL DEFAULT 0,
    use_type                        TEXT,
    operational_status              TEXT,
    service_type                    TEXT,
    city                            TEXT,
    county                          TEXT,
    state                           TEXT,
    country                         TEXT DEFAULT 'United States',
    latitude                        REAL,
    longitude                       REAL,
    notes                           TEXT,
    source                          TEXT NOT NULL DEFAULT 'manual',
    repeaterbook_id                 TEXT,
    rb_ctcss_uplink                 REAL,
    rb_ctcss_downlink               REAL,
    rb_operational_status           TEXT,
    rb_notes                        TEXT,
    ctcss_uplink_overridden         BOOLEAN NOT NULL DEFAULT 0,
    ctcss_downlink_overridden       BOOLEAN NOT NULL DEFAULT 0,
    operational_status_overridden   BOOLEAN NOT NULL DEFAULT 0,
    notes_overridden                BOOLEAN NOT NULL DEFAULT 0,
    has_overrides                   BOOLEAN NOT NULL DEFAULT 0,
    last_rb_update                  DATETIME,
    last_user_edit                  DATETIME,
    created_at                      DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP
);

-- A repeaterbook_id uniquely identifies an imported repeater so re-imports
-- can dedupe. Only enforced when present (manual channels have no id).
CREATE UNIQUE INDEX idx_channels_repeaterbook_id
    ON channels(repeaterbook_id) WHERE repeaterbook_id IS NOT NULL;
CREATE INDEX idx_channels_band   ON channels(band);
CREATE INDEX idx_channels_mode   ON channels(mode);
CREATE INDEX idx_channels_state  ON channels(state);
CREATE INDEX idx_channels_status ON channels(operational_status);

-- ============================================================
-- radio_models : built-in read-only library (seeded at startup)
-- ============================================================
CREATE TABLE radio_models (
    id                              INTEGER PRIMARY KEY,
    manufacturer                    TEXT NOT NULL,
    model                           TEXT NOT NULL,
    display_name                    TEXT NOT NULL,
    analog_capable                  BOOLEAN NOT NULL DEFAULT 0,
    dmr_capable                     BOOLEAN NOT NULL DEFAULT 0,
    dstar_capable                   BOOLEAN NOT NULL DEFAULT 0,
    ysf_capable                     BOOLEAN NOT NULL DEFAULT 0,
    nxdn_capable                    BOOLEAN NOT NULL DEFAULT 0,
    p25_capable                     BOOLEAN NOT NULL DEFAULT 0,
    m17_capable                     BOOLEAN NOT NULL DEFAULT 0,
    aprs_capable                    BOOLEAN NOT NULL DEFAULT 0,
    covers_hf                       BOOLEAN NOT NULL DEFAULT 0,
    covers_vhf                      BOOLEAN NOT NULL DEFAULT 0,
    covers_uhf                      BOOLEAN NOT NULL DEFAULT 0,
    covers_220                      BOOLEAN NOT NULL DEFAULT 0,
    covers_900                      BOOLEAN NOT NULL DEFAULT 0,
    freq_min                        REAL,
    freq_max                        REAL,
    memory_channels                 INTEGER,
    zones_supported                 BOOLEAN NOT NULL DEFAULT 0,
    max_zones                       INTEGER,
    channels_per_zone               INTEGER,
    scan_lists_supported            BOOLEAN NOT NULL DEFAULT 0,
    max_scan_lists                  INTEGER,
    banks_supported                 BOOLEAN NOT NULL DEFAULT 0,
    max_name_length                 INTEGER,
    export_format                   TEXT,
    connection_type                 TEXT,
    non_channel_settings_schema     TEXT
);

CREATE UNIQUE INDEX idx_radio_models_make_model
    ON radio_models(manufacturer, model);

-- ============================================================
-- radio_profiles : user instances of a radio model
-- ============================================================
CREATE TABLE radio_profiles (
    id                      INTEGER PRIMARY KEY,
    display_name            TEXT NOT NULL,
    radio_model_id          INTEGER NOT NULL REFERENCES radio_models(id),
    non_channel_settings    TEXT,
    notes                   TEXT,
    created_at              DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at              DATETIME
);

-- ============================================================
-- channel_lists : ordered, radio-agnostic collections
-- ============================================================
CREATE TABLE channel_lists (
    id          INTEGER PRIMARY KEY,
    name        TEXT NOT NULL,
    description TEXT,
    created_at  DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at  DATETIME
);

CREATE TABLE channel_list_entries (
    id              INTEGER PRIMARY KEY,
    channel_list_id INTEGER NOT NULL REFERENCES channel_lists(id) ON DELETE CASCADE,
    channel_id      INTEGER NOT NULL REFERENCES channels(id) ON DELETE CASCADE,
    position        INTEGER NOT NULL,
    UNIQUE(channel_list_id, position)
);
CREATE INDEX idx_cle_list ON channel_list_entries(channel_list_id);

-- ============================================================
-- scan_lists : ordered collections for scanning behavior
-- ============================================================
CREATE TABLE scan_lists (
    id                  INTEGER PRIMARY KEY,
    name                TEXT NOT NULL,
    description         TEXT,
    priority_channel_id INTEGER REFERENCES channels(id),
    created_at          DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP
);

CREATE TABLE scan_list_entries (
    id           INTEGER PRIMARY KEY,
    scan_list_id INTEGER NOT NULL REFERENCES scan_lists(id) ON DELETE CASCADE,
    channel_id   INTEGER NOT NULL REFERENCES channels(id) ON DELETE CASCADE,
    position     INTEGER NOT NULL
);
CREATE INDEX idx_sle_list ON scan_list_entries(scan_list_id);

-- ============================================================
-- codeplugs : named saved configurations
-- ============================================================
CREATE TABLE codeplugs (
    id               INTEGER PRIMARY KEY,
    name             TEXT NOT NULL,
    radio_profile_id INTEGER REFERENCES radio_profiles(id),
    notes            TEXT,
    last_exported    DATETIME,
    created_at       DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at       DATETIME
);

CREATE TABLE codeplug_channel_lists (
    codeplug_id     INTEGER NOT NULL REFERENCES codeplugs(id) ON DELETE CASCADE,
    channel_list_id INTEGER NOT NULL REFERENCES channel_lists(id) ON DELETE CASCADE,
    position        INTEGER,
    PRIMARY KEY(codeplug_id, channel_list_id)
);

CREATE TABLE codeplug_scan_lists (
    codeplug_id  INTEGER NOT NULL REFERENCES codeplugs(id) ON DELETE CASCADE,
    scan_list_id INTEGER NOT NULL REFERENCES scan_lists(id) ON DELETE CASCADE,
    PRIMARY KEY(codeplug_id, scan_list_id)
);
