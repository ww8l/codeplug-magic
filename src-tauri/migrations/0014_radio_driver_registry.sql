-- Chunk 3.2: the radio-driver registry boundary.
--
-- `driver_key`   ties a model to a live-USB driver in `radios/<driver_key>/`
--                (Rust side: radios::registry::driver_for_key). NULL means the
--                model has no serial driver — CSV/export-only, e.g. the Vero
--                VR-N76 (Bluetooth-app-only).
-- `programming_ui` selects which frontend "Program radio" dialog to render
--                (src/lib/radioProgramming.ts, wired in 3.7): "generic" is the
--                capability-driven default; "tdh3"/"anytone" are bespoke dialogs.
--                NULL means no live-programming UI.
--
-- Both are additive, nullable, no DEFAULT — safe under the frozen-checksum rule.
-- seed.rs also sets these on every startup (ON CONFLICT DO UPDATE), so new
-- installs get them declaratively; this backfill keeps the CURRENT live DB
-- correct without depending on a reseed.

ALTER TABLE radio_models ADD COLUMN driver_key TEXT;
ALTER TABLE radio_models ADD COLUMN programming_ui TEXT;

UPDATE radio_models SET driver_key = 'baofeng_uv5r',    programming_ui = 'generic'
    WHERE manufacturer = 'Baofeng'  AND model = 'UV-5R';
UPDATE radio_models SET driver_key = 'tidradio_tdh3',   programming_ui = 'tdh3'
    WHERE manufacturer = 'TIDRADIO' AND model = 'TD-H3';
UPDATE radio_models SET driver_key = 'anytone_atd890uv', programming_ui = 'anytone'
    WHERE manufacturer = 'AnyTone'  AND model = 'AT-D890UV';
-- Vero VR-N76 intentionally left NULL/NULL: no serial driver, CSV export only.
