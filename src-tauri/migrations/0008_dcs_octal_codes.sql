-- DCS codes are octal notation, everywhere.
--
-- The convention across the app (CHIRP CSVs, the channel editor's DTCS list,
-- the TD-H3 codec's digit-wise BCD) is the printed OCTAL code, e.g. "411".
-- The AnyTone AT-D890UV radio import (sessions 26-28) instead stored the
-- radio's raw binary u16 as a DECIMAL string (raw 265 for the code the radio
-- and every repeater list print as D411). HW-confirmed 2026-07-05: the radio
-- stores raw binary and displays it in octal.
--
-- Convert the radio-imported rows to the octal convention. Only rows the
-- AT-D890UV import created are affected (identified by the notes marker the
-- import stamps); curated/CSV rows already hold octal codes.

UPDATE channels
   SET dcs_code = printf('%03o', CAST(dcs_code AS INTEGER))
 WHERE notes LIKE 'Imported from AT-D890UV slot %'
   AND dcs_code IS NOT NULL
   AND dcs_code GLOB '[0-9]*';

UPDATE channels
   SET dcs_rx_code = printf('%03o', CAST(dcs_rx_code AS INTEGER))
 WHERE notes LIKE 'Imported from AT-D890UV slot %'
   AND dcs_rx_code IS NOT NULL
   AND dcs_rx_code GLOB '[0-9]*';
