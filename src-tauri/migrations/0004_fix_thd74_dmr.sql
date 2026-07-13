-- Data fix: the Kenwood TH-D74A was seeded with dmr_capable=true, but it is a
-- D-STAR (and analog) handheld with no DMR support. Correct existing DBs.
-- Guarded so a deliberate user override is left alone.
UPDATE radio_models
SET dmr_capable = 0
WHERE manufacturer = 'Kenwood'
  AND model = 'TH-D74A'
  AND dmr_capable = 1;
