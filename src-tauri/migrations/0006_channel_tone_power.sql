-- Full per-channel tone control + power level.
--
-- Tone model follows CHIRP's universal scheme so it maps to every radio:
--   tone_mode ∈ off | Tone | TSQL | DTCS | Cross
--   ctcss_uplink   = TX CTCSS (rtone)      ctcss_downlink = RX CTCSS (ctone)
--   dcs_code       = TX DCS   (dtcs)       dcs_rx_code    = RX DCS   (rx_dtcs)
--   dcs_polarity   = 2 chars NN/NR/RN/RR   cross_mode     = e.g. "DTCS->Tone"
-- power is a radio-agnostic level (High/Med/Low); NULL = radio default (max).
-- Each radio maps these to its own capabilities on program/export.

ALTER TABLE channels ADD COLUMN dcs_rx_code  TEXT;
ALTER TABLE channels ADD COLUMN dcs_polarity TEXT NOT NULL DEFAULT 'NN';
ALTER TABLE channels ADD COLUMN cross_mode   TEXT NOT NULL DEFAULT 'Tone->Tone';
ALTER TABLE channels ADD COLUMN power        TEXT;

-- Migrate the old tone_mode vocabulary (CTCSS/DCS/none) to CHIRP's.
UPDATE channels SET tone_mode = 'Tone' WHERE tone_mode = 'CTCSS';
UPDATE channels SET tone_mode = 'DTCS' WHERE tone_mode = 'DCS';
UPDATE channels SET tone_mode = 'off'
    WHERE tone_mode IS NULL OR tone_mode = '' OR tone_mode = 'none';

-- Seed RX DCS from TX DCS so existing DTCS channels round-trip unchanged.
UPDATE channels SET dcs_rx_code = dcs_code WHERE dcs_code IS NOT NULL;
