-- Remove the Vero VR-N76. It was seeded as an export-only placeholder
-- (Bluetooth/app-only, no serial protocol) and its driver was never finished;
-- with no access to the hardware there is no path to completing it, so the
-- model is dropped rather than left as a half-supported entry.
--
-- Mirrors migration 0005's ordering: radio_profiles -> radio_models has no
-- ON DELETE cascade and codeplugs reference radio_profiles, so detach/delete
-- dependents first. On a fresh DB this runs before seeding (empty tables) and
-- is a harmless no-op.

UPDATE codeplugs SET radio_profile_id = NULL
WHERE radio_profile_id IN (
    SELECT rp.id FROM radio_profiles rp
    JOIN radio_models rm ON rm.id = rp.radio_model_id
    WHERE rm.manufacturer = 'Vero' AND rm.model = 'VR-N76'
);

DELETE FROM radio_profiles
WHERE radio_model_id IN (
    SELECT id FROM radio_models
    WHERE manufacturer = 'Vero' AND model = 'VR-N76'
);

DELETE FROM radio_models
WHERE manufacturer = 'Vero' AND model = 'VR-N76';
