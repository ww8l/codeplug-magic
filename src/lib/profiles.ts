// Helpers for the schema-driven radio-profile settings form.
import type { RadioModel, SettingField } from "./types";

export type SettingsValue = string | number | boolean;
export type SettingsValues = Record<string, SettingsValue>;

/** Parse a model's `non_channel_settings_schema` JSON into field definitions. */
export function parseSchema(model: RadioModel | null | undefined): SettingField[] {
  if (!model?.non_channel_settings_schema) return [];
  try {
    const parsed = JSON.parse(model.non_channel_settings_schema);
    return Array.isArray(parsed) ? (parsed as SettingField[]) : [];
  } catch {
    return [];
  }
}

/** Parse a profile's saved `non_channel_settings` JSON object. */
export function parseSettings(json: string | null | undefined): SettingsValues {
  if (!json) return {};
  try {
    const parsed = JSON.parse(json);
    return parsed && typeof parsed === "object" ? (parsed as SettingsValues) : {};
  } catch {
    return {};
  }
}

/** The default value for a field, falling back to a sensible per-type blank. */
export function fieldDefault(field: SettingField): SettingsValue {
  if (field.default !== undefined) return field.default;
  switch (field.type) {
    case "boolean":
      return false;
    case "integer":
      return field.min ?? 0;
    case "select":
      return field.options?.[0] ?? "";
    default:
      return "";
  }
}

/** Seed a values object for a schema, preferring saved values over defaults. */
export function seedValues(
  fields: SettingField[],
  saved: SettingsValues,
): SettingsValues {
  const out: SettingsValues = {};
  for (const f of fields) {
    if (f.type === "section") continue; // headings hold no value
    out[f.key] = f.key in saved ? saved[f.key] : fieldDefault(f);
  }
  return out;
}

/** Human-readable list of the bands a model covers. */
export function modelBands(m: RadioModel): string[] {
  const bands: string[] = [];
  if (m.covers_hf) bands.push("HF");
  if (m.covers_vhf) bands.push("VHF");
  if (m.covers_220) bands.push("220");
  if (m.covers_uhf) bands.push("UHF");
  if (m.covers_900) bands.push("900");
  return bands;
}

/** Human-readable list of the modes a model supports. */
export function modelModes(m: RadioModel): string[] {
  const modes: string[] = [];
  if (m.analog_capable) modes.push("Analog");
  if (m.dmr_capable) modes.push("DMR");
  if (m.dstar_capable) modes.push("D-STAR");
  if (m.ysf_capable) modes.push("System Fusion");
  if (m.nxdn_capable) modes.push("NXDN");
  if (m.p25_capable) modes.push("P25");
  if (m.m17_capable) modes.push("M17");
  return modes;
}
