-- Talkgroup support (Phase 1)
-- A DMR repeater carries many talkgroups across two timeslots; in a radio's
-- codeplug each (talkgroup x timeslot) becomes its own channel. Talkgroups are
-- defined once here and reused across repeaters via repeater_talkgroups.

-- ============================================================
-- talkgroups : reusable library of DMR talkgroups ("contacts")
-- ============================================================
CREATE TABLE talkgroups (
    id          INTEGER PRIMARY KEY,
    tg_number   INTEGER NOT NULL,            -- the DMR talkgroup ID, e.g. 3108
    name        TEXT NOT NULL,
    network     TEXT NOT NULL DEFAULT 'Brandmeister', -- Brandmeister/TGIF/DMR-MARC/Other
    call_type   TEXT NOT NULL DEFAULT 'Group',         -- Group | Private
    notes       TEXT,
    source      TEXT NOT NULL DEFAULT 'manual',        -- manual | import | seed
    created_at  DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at  DATETIME
);

-- The same numeric talkgroup can exist on different networks, so identity is
-- (network, tg_number).
CREATE UNIQUE INDEX idx_talkgroups_network_number
    ON talkgroups(network, tg_number);

-- ============================================================
-- repeater_talkgroups : which talkgroups are carried on which repeater
-- channel, and on which timeslot. Ordered for export expansion.
-- ============================================================
CREATE TABLE repeater_talkgroups (
    id              INTEGER PRIMARY KEY,
    channel_id      INTEGER NOT NULL REFERENCES channels(id) ON DELETE CASCADE,
    talkgroup_id    INTEGER NOT NULL REFERENCES talkgroups(id) ON DELETE CASCADE,
    timeslot        INTEGER NOT NULL DEFAULT 1,  -- 1 or 2
    position        INTEGER NOT NULL DEFAULT 0,
    name_override   TEXT,
    UNIQUE(channel_id, talkgroup_id, timeslot)
);
CREATE INDEX idx_rtg_channel ON repeater_talkgroups(channel_id);
CREATE INDEX idx_rtg_talkgroup ON repeater_talkgroups(talkgroup_id);
