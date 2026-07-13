-- Add CITY to the synthetic RepeaterBook dedupe id.
--
-- The "Full Data" JSON/CSV export has no unique repeater id, so we derive one
-- from callsign + frequency + state. That collapses distinct repeaters that
-- share all three: e.g. N2SKY 448.400 in CO exists in BOTH Fort Collins and
-- Buena Vista, so importing kept only one of them. Adding the city
-- disambiguates them.
--
-- Recompute the id for already-imported RepeaterBook rows so a re-import still
-- matches (and merges) them instead of inserting duplicates. This MUST match the
-- Rust format in finalize(): UPPER(callsign)|%.4f freq|UPPER(state)|UPPER(city),
-- with NULL state/city rendered as an empty segment.
--
-- Safe against the UNIQUE index on repeaterbook_id: the new key is strictly more
-- specific than the old one, so it cannot introduce a collision the old unique
-- constraint didn't already forbid.
UPDATE channels
SET repeaterbook_id =
        UPPER(callsign) || '|' ||
        printf('%.4f', rx_freq) || '|' ||
        COALESCE(UPPER(state), '') || '|' ||
        COALESCE(UPPER(city), '')
WHERE source = 'repeaterbook'
  AND repeaterbook_id IS NOT NULL;
