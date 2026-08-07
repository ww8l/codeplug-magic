-- Issue #32: a radio hears far more than it can talk on.
--
-- `freq_min`/`freq_max` is ONE contiguous span doing two jobs at once: it is
-- the only transmit test we have, and the only receive test we have. That is
-- why an FT5D refused GMRS — 462 MHz falls outside its 144-450 TX span, so a
-- channel it receives perfectly well was dropped from the codeplug entirely.
--
-- `tx_bands` / `rx_bands` are JSON arrays of [min_mhz, max_mhz] pairs, so a
-- radio with disjoint bands can finally say so (the FT5D transmits on
-- 144-148 and 430-450, nothing between). Both are NULL for every model that
-- has not been surveyed; NULL means "fall back to freq_min/freq_max plus the
-- covers_* flags", which is exactly the old behaviour. A channel inside
-- `rx_bands` but outside `tx_bands` is still programmed — as receive-only.
--
-- Additive, nullable, no DEFAULT — safe under the frozen-checksum rule. seed.rs
-- sets these on every startup (ON CONFLICT DO UPDATE) so new installs get them
-- declaratively; this backfill keeps the CURRENT live DB correct without
-- depending on a reseed.

ALTER TABLE radio_models ADD COLUMN tx_bands TEXT;
ALTER TABLE radio_models ADD COLUMN rx_bands TEXT;

-- FT5D: US TX bands, and a receiver that spans 0.5-999.995 MHz in one piece
-- (the US cellular block, 824-849/869-894, is not carved out — nothing legal
-- to program lives there anyway).
UPDATE radio_models
   SET tx_bands = '[[144.0,148.0],[430.0,450.0]]',
       rx_bands = '[[0.5,999.995]]'
 WHERE manufacturer = 'Yaesu' AND model = 'FT5D';
