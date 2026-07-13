-- Per-channel scan-list assignment (the radio's channel byte 0x1B / "scan list"
-- field). On DMR radios each channel points at exactly ONE scan list to launch
-- when Scan is pressed on that channel — an explicit per-channel setting, NOT
-- derived from scan-list membership. The assignment is PER-CODEPLUG because
-- channels are a shared library while scan lists belong to a codeplug, so the
-- same channel can point at different lists in different codeplugs.
--
-- PK (codeplug_id, channel_id): a channel resolves to at most one scan list in
-- a given codeplug. Channels with no row here fall back to membership-derivation
-- ("first assigned scan list containing the channel") when programming.
CREATE TABLE codeplug_channel_scan_lists (
    codeplug_id  INTEGER NOT NULL REFERENCES codeplugs(id)   ON DELETE CASCADE,
    channel_id   INTEGER NOT NULL REFERENCES channels(id)    ON DELETE CASCADE,
    scan_list_id INTEGER NOT NULL REFERENCES scan_lists(id)  ON DELETE CASCADE,
    PRIMARY KEY(codeplug_id, channel_id)
);
CREATE INDEX idx_ccsl_list ON codeplug_channel_scan_lists(scan_list_id);
