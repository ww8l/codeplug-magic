-- Per-scan-list settings for the AnyTone AT-D890UV (RT-Systems parity).
-- The scan-list binary record (encode_scan_list_record) already reserves bytes
-- for all of these; before this migration the encoder hardcoded CPS defaults.
--
-- priority_channel_id (from 0001) is reused as "priority channel 1".
--
-- NOTE: the priority_select (@0x01) and revert_channel (@0xF8) enum byte values
-- are sourced from qdmr and are NOT yet hardware-verified. Pin them with the
-- re_settings_diff differential-dump loop before trusting them (see plan §2).

ALTER TABLE scan_lists ADD COLUMN priority_channel_2_id INTEGER REFERENCES channels(id);
ALTER TABLE scan_lists ADD COLUMN priority_select INTEGER NOT NULL DEFAULT 0;  -- 0=Off,1=Ch1,2=Ch2,3=Ch1+Ch2
ALTER TABLE scan_lists ADD COLUMN look_back_a     INTEGER NOT NULL DEFAULT 20; -- 0.1s units => 2.0s
ALTER TABLE scan_lists ADD COLUMN look_back_b     INTEGER NOT NULL DEFAULT 30; -- 0.1s units => 3.0s
ALTER TABLE scan_lists ADD COLUMN dropout_delay   INTEGER NOT NULL DEFAULT 31; -- 0.1s units => 3.1s
ALTER TABLE scan_lists ADD COLUMN dwell_time      INTEGER NOT NULL DEFAULT 31; -- 0.1s units => 3.1s
ALTER TABLE scan_lists ADD COLUMN revert_channel  INTEGER NOT NULL DEFAULT 0;  -- 0=Selected Channel (matches prior hardcode)
