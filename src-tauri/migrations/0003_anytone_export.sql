-- DMR-native export (Phase 3). The Anytone AT-D878UVII was seeded with
-- export_format 'chirp_csv', which stuffed DMR programming into the CHIRP
-- Comment column. Switch it to the Anytone CSV bundle exporter (a Channel CSV
-- with real DMR columns plus a companion Digital Contacts / TalkGroups CSV).
-- Only touches the chirp_csv default so a user override is left alone.
UPDATE radio_models
SET export_format = 'anytone_csv'
WHERE manufacturer = 'Anytone'
  AND model = 'AT-D878UVII'
  AND export_format = 'chirp_csv';
