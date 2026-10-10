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

/**
 * Seed a values object for a schema from what the profile has saved.
 *
 * A field the profile has never held stays **blank** — absent from the result —
 * for every radio, card or cable. A schema default is this app's guess, not the
 * radio's setting, and every writer patches the profile's values over bytes it
 * has just read off the radio (or out of the radio's own file): a seeded guess
 * would be WRITTEN, replacing the operator's real setting, the first time a
 * profile nobody filled in was programmed (#90, #49, #128). An absent key is
 * inert — every writer skips it — so the radio's own settings survive until the
 * operator reads them in or changes one deliberately.
 */
export function seedValues(
  fields: SettingField[],
  saved: SettingsValues,
): SettingsValues {
  const out: SettingsValues = {};
  for (const f of fields) {
    if (f.type === "section") continue; // headings hold no value
    if (f.key in saved) out[f.key] = saved[f.key];
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

/** One sub-tab of the settings form: a section heading and the fields under it. */
export interface SettingsTab {
  key: string;
  label: string;
  fields: SettingField[];
}

/**
 * Split a schema into sub-tabs on its section headings, or return null to leave
 * it as the single scroll it has always been.
 *
 * Only schemas carrying an APRS section are split. Those radios keep their APRS
 * settings in tables — status texts, canned phrases, beacon objects — so their
 * field count runs away from the rest, and their own programming software
 * already presents APRS on tabs of its own. Splitting there matches what the
 * operator has seen before; splitting a radio with no APRS would just add a
 * click to a list that reads fine as one column.
 *
 * Tabs are presentation only. The form holds every field's value whether or not
 * its tab is on screen, so saving is unaffected by which one is open.
 */
export function settingsTabs(fields: SettingField[]): SettingsTab[] | null {
  const hasAprs = fields.some(
    (f) => f.type === "section" && /^APRS\b/i.test(f.label),
  );
  if (!hasAprs) return null;

  const tabs: SettingsTab[] = [];
  for (const f of fields) {
    if (f.type === "section") {
      tabs.push({ key: f.key, label: f.label, fields: [] });
    } else if (tabs.length > 0) {
      tabs[tabs.length - 1].fields.push(f);
    } else {
      // A field ahead of the first heading. No schema does this today, but
      // dropping it would hide a real setting, so it gets a tab of its own.
      tabs.push({ key: "section-general", label: "General", fields: [f] });
    }
  }
  // A heading with nothing under it would be an empty tab.
  return tabs.filter((t) => t.fields.length > 0);
}

/**
 * Why a value is one the radio cannot take, or null if it is fine.
 *
 * A schema's `min`/`max` reached the screen as HTML attributes and nothing
 * else, and `<input type="number">` flags an out-of-range value without
 * preventing one — so 300 in a 0–24 field was saved, handed to the encoder and
 * cast down to a byte, landing on the radio as 44 (#87). The Rust side refuses
 * the same values on the way to the radio; this is what puts the reason next to
 * the field, before the profile is stored.
 *
 * Numbers only, deliberately: a value stored as text is skipped here exactly as
 * the encoders skip it, and a `select` holding a label its option list does not
 * name is how a setting read off a radio survives a round trip.
 */
export function settingRangeError(
  field: SettingField,
  value: SettingsValue | undefined,
): string | null {
  if (field.type !== "integer" || typeof value !== "number") return null;
  if (!Number.isFinite(value)) return null;
  if (!Number.isInteger(value)) return "Whole numbers only.";
  if (field.min != null && field.max != null && (value < field.min || value > field.max))
    return `Must be between ${field.min} and ${field.max}.`;
  if (field.min != null && value < field.min) return `Must be ${field.min} or more.`;
  if (field.max != null && value > field.max) return `Must be ${field.max} or less.`;
  return null;
}

/** Every out-of-range value in `values`, keyed by field key. */
export function settingsRangeErrors(
  fields: SettingField[],
  values: SettingsValues,
): Record<string, string> {
  const errors: Record<string, string> = {};
  for (const f of fields) {
    const message = settingRangeError(f, values[f.key]);
    if (message) errors[f.key] = message;
  }
  return errors;
}
