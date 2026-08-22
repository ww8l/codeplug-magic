-- Snapshot the DCS codes RepeaterBook supplied, the way rb_ctcss_uplink and
-- rb_ctcss_downlink already snapshot the tones.
--
-- Until the free-tier CSV importer landed, RepeaterBook could not describe DCS
-- at all, so `keeps_dcs` could treat any stored DCS scheme as proof of an
-- operator edit and freeze it (issue #71). That premise is now false: the free
-- CSV's tone columns carry values like D073. Without a snapshot there is no
-- longer any way to tell an operator's curated DCS from one an import wrote,
-- and the two need opposite handling on a re-import — keep the first, refresh
-- the second.
--
-- NULL for every existing row, which reads as "differs from what RB last said"
-- and so preserves any DCS already stored. That is the safe direction: a
-- channel keeps working and the snapshot fills in on the next import.

ALTER TABLE channels ADD COLUMN rb_dcs_code    TEXT;
ALTER TABLE channels ADD COLUMN rb_dcs_rx_code TEXT;
