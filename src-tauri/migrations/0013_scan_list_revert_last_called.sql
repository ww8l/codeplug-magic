-- Default the scan-list Revert Channel to "Last Called" ("last channel heard",
-- byte 4 @0xF8) per user preference / RT-Systems default.
--
-- 0012 shipped with column DEFAULT 0 (Selected Channel) and was already applied
-- to live databases, so its checksum is frozen — the default change lives here
-- instead. Every existing row was set to 0 by 0012 and the Revert field had no
-- UI before this, so none were intentionally chosen: safe to bump them all to 4.
-- New rows get 4 explicitly in create_scan_list (SQLite can't alter a column
-- default without a full table rebuild, which the FKs here make risky).
--
-- NOTE: byte 4 == "Last Called" is qdmr-sourced and NOT yet hardware-verified.

UPDATE scan_lists SET revert_channel = 4 WHERE revert_channel = 0;
