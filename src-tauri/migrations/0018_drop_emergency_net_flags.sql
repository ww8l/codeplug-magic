-- Drop the emergency-net flags: ares, races, skywarn, canwarn.
--
-- RepeaterBook publishes whether a repeater is an ARES, RACES or SKYWARN
-- machine, and the premium "Full Data" importer has been storing all four since
-- 0001. Nothing ever read them back: they appeared in one column of the import
-- preview table and nowhere else — not editable, not filterable, not consulted
-- by a single radio driver or exporter. Four columns of write-only data.
--
-- Dropped on request rather than surfaced. The information is a query away on
-- RepeaterBook for anyone who wants it, and carrying a field no code consumes
-- costs more than it is worth: it has to be threaded through every INSERT, the
-- duplicate-channel copy, the native backup format and the re-import merge, and
-- each of those is a place to get it silently wrong.
--
-- No index, view or trigger references them, so a plain DROP COLUMN is enough
-- (SQLite 3.35+). Existing backup files that still carry the keys import fine:
-- nothing in the backup reader denies unknown fields.

ALTER TABLE channels DROP COLUMN ares;
ALTER TABLE channels DROP COLUMN races;
ALTER TABLE channels DROP COLUMN skywarn;
ALTER TABLE channels DROP COLUMN canwarn;
