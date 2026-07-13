-- Distinguish how a codeplug was last pushed out, so the Codeplugs screen can
-- show "Programmed <date>" (written to a radio over the cable) vs
-- "Exported <date>" (written to a CSV/file). NULL = never exported.
--   last_export_kind ∈ NULL | 'radio' | 'file'
ALTER TABLE codeplugs ADD COLUMN last_export_kind TEXT;

-- Existing rows that already carry a last_exported timestamp predate this
-- column; we can't tell which path set them, so leave the kind NULL and let the
-- UI fall back to the generic "Exported <date>" wording for those.
