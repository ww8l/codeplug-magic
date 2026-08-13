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

/**
 * Seed a values object for a schema, preferring saved values over defaults.
 *
 * `defaults: false` leaves a field the profile has never held **blank** instead
 * of inventing a value for it. That matters for the radios whose settings are
 * patched into a file the radio itself wrote: a schema default is this app's
 * guess, not the radio's setting, and saving a profile full of guesses would
 * push all of them onto the radio the next time a codeplug is written. A blank
 * is inert — the writers skip a value they cannot read — so the operator's own
 * settings survive until they load them in and change one deliberately.
 */
export function seedValues(
  fields: SettingField[],
  saved: SettingsValues,
  defaults = true,
): SettingsValues {
  const out: SettingsValues = {};
  for (const f of fields) {
    if (f.type === "section") continue; // headings hold no value
    if (f.key in saved) {
      out[f.key] = saved[f.key];
    } else if (defaults) {
      out[f.key] = fieldDefault(f);
    }
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

/**
 * The transmit or receive range as an operator reads it. Prefers the real band
 * list (`tx_bands`/`rx_bands`, JSON `[[min,max], …]`) so a radio with disjoint
 * bands shows both — "144–148, 430–450 MHz" — and falls back to the single
 * freq_min/freq_max span for models that have not been surveyed. Returns null
 * when neither is known.
 */
export function modelRange(m: RadioModel, which: "tx" | "rx"): string | null {
  const raw = which === "tx" ? m.tx_bands : m.rx_bands;
  const spans = parseBands(raw);
  if (spans) {
    return `${spans.map(([lo, hi]) => `${trim(lo)}–${trim(hi)}`).join(", ")} MHz`;
  }
  // No rx_bands means the receiver was never surveyed separately; it is not the
  // same claim as "it receives only what it transmits on", so say nothing.
  if (which === "rx") return null;
  if (m.freq_min == null || m.freq_max == null) return null;
  return `${trim(m.freq_min)}–${trim(m.freq_max)} MHz`;
}

function parseBands(raw: string | null): [number, number][] | null {
  if (!raw) return null;
  try {
    const parsed = JSON.parse(raw);
    if (!Array.isArray(parsed)) return null;
    const spans = parsed.filter(
      (b): b is [number, number] =>
        Array.isArray(b) && b.length === 2 && b.every((n) => typeof n === "number"),
    );
    return spans.length > 0 ? spans : null;
  } catch {
    return null;
  }
}

/** 148 → "148", 462.5625 → "462.5625". Band edges should read like band edges. */
function trim(mhz: number): string {
  return String(Number(mhz.toFixed(4)));
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
