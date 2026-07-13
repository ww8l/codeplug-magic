-- Trim the seeded radio model library down to the Baofeng UV-5R. Other models
-- will be reintroduced one at a time with properly-sourced settings schemas
-- (the UV-5R schema is now derived from the CHIRP uv5r.py driver).
--
-- radio_profiles -> radio_models has no ON DELETE cascade, and codeplugs
-- reference radio_profiles, so detach/delete dependents first. On a fresh DB
-- this runs before seeding (empty tables) and is a harmless no-op.

UPDATE codeplugs SET radio_profile_id = NULL
WHERE radio_profile_id IN (
    SELECT rp.id FROM radio_profiles rp
    JOIN radio_models rm ON rm.id = rp.radio_model_id
    WHERE NOT (rm.manufacturer = 'Baofeng' AND rm.model = 'UV-5R')
);

DELETE FROM radio_profiles
WHERE radio_model_id IN (
    SELECT id FROM radio_models
    WHERE NOT (manufacturer = 'Baofeng' AND model = 'UV-5R')
);

DELETE FROM radio_models
WHERE NOT (manufacturer = 'Baofeng' AND model = 'UV-5R');
