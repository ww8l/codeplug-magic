-- DMR-ID -> callsign/name lookup database ("caller ID" database), sourced
-- from radioid.net's public dump. This is a separate, much larger library
-- from the small per-codeplug talkgroup "contacts" list (see
-- 0002_talkgroups.sql) and is not user-edited: a refresh always overwrites.

CREATE TABLE dmr_users (
    id          INTEGER PRIMARY KEY,
    dmr_id      INTEGER NOT NULL,
    callsign    TEXT NOT NULL,
    first_name  TEXT,
    last_name   TEXT,
    city        TEXT,
    state       TEXT,
    country     TEXT,
    remarks     TEXT,
    source      TEXT NOT NULL DEFAULT 'radioid.net',
    updated_at  DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP
);

CREATE UNIQUE INDEX idx_dmr_users_dmr_id   ON dmr_users(dmr_id);
CREATE INDEX        idx_dmr_users_country  ON dmr_users(country);
CREATE INDEX        idx_dmr_users_callsign ON dmr_users(callsign);
