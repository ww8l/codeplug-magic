-- A digest of what a codeplug resolved to when it was last programmed or
-- exported, so the dashboard can tell when the radio has fallen behind the
-- database. A digest rather than change timestamps: a restore re-inserts every
-- row, and an edit that is put back is no change at all. NULL for a codeplug
-- not programmed since this column existed.
ALTER TABLE codeplugs ADD COLUMN programmed_fingerprint TEXT;
