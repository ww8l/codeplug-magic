// Shared option lists and small formatters for the channel UI.

export const BANDS = ["HF", "VHF", "UHF", "220", "900"];
export const MODES = [
  "FM",
  "NFM",
  "AM",
  "USB",
  "LSB",
  "CW",
  "DMR",
  "DSTAR",
  "YSF",
  "NXDN",
  "P25",
  "M17",
];
// Mode order a freshly created zone/group is sorted into: analog FM first, then
// the digital modes in the order most operators want them, then everything else
// in MODES order. NFM rides with FM — on the air they're the same thing, so a
// mixed analog batch stays together at the top.
const ZONE_MODE_ORDER = ["FM", "NFM", "DMR", "YSF", "DSTAR"];

export const OP_STATUSES = ["On Air", "Off Air", "Unknown"];
export const DUPLEX_OPTIONS = ["+", "-", "none", "split"];
// CHIRP's universal tone scheme (maps cleanly to every radio):
//   off  - no tone        Tone - TX CTCSS only      TSQL - TX+RX CTCSS
//   DTCS - digital code    Cross - per-side mix (see CROSS_MODES)
export const TONE_MODES = ["off", "Tone", "TSQL", "DTCS", "Cross"];

export const CROSS_MODES = [
  "Tone->Tone",
  "Tone->DTCS",
  "DTCS->Tone",
  "DTCS->DTCS",
  "->Tone",
  "->DTCS",
  "DTCS->",
  "Tone->",
];

export const DTCS_POLARITIES = ["NN", "NR", "RN", "RR"];

/** The transmit and receive halves a tone scheme uses: "Tone", "DTCS", or "". */
export function toneSides(
  tone_mode: string | null,
  cross_mode: string | null,
): { tmode: string; txSide: string; rxSide: string } {
  const tmode = tone_mode ?? "off";
  let txSide = "";
  let rxSide = "";
  if (tmode === "Tone") {
    txSide = "Tone";
  } else if (tmode === "TSQL") {
    txSide = rxSide = "Tone";
  } else if (tmode === "DTCS") {
    txSide = rxSide = "DTCS";
  } else if (tmode === "Cross") {
    [txSide, rxSide] = (cross_mode || "Tone->Tone").split("->");
  }
  return { tmode, txSide, rxSide };
}

/**
 * Which of the four tone columns the chosen scheme actually reads — the single
 * source for both the editor's field visibility and what gets saved.
 */
export function toneFieldsInUse(
  tone_mode: string | null,
  cross_mode: string | null,
): { ctcss_uplink: boolean; ctcss_downlink: boolean; dcs_code: boolean; dcs_rx_code: boolean } {
  const { tmode, txSide, rxSide } = toneSides(tone_mode, cross_mode);
  return {
    ctcss_uplink: txSide === "Tone" && tmode !== "TSQL",
    ctcss_downlink: rxSide === "Tone" || tmode === "TSQL",
    dcs_code: txSide === "DTCS",
    dcs_rx_code: rxSide === "DTCS" && tmode === "Cross",
  };
}

/**
 * Null out every tone column the chosen scheme does not use, so what is stored
 * says what the operator sees.
 *
 * Setting Tone Mode to "off" used to leave the CTCSS values sitting in the row.
 * Nothing had changed against the RepeaterBook snapshot, so no override was
 * recorded — and the next re-import re-derived the scheme straight back to TSQL,
 * silently undoing "this machine is carrier access" (issue #86).
 */
export function clearUnusedToneFields<
  T extends {
    tone_mode: string | null;
    cross_mode: string;
    ctcss_uplink: number | null;
    ctcss_downlink: number | null;
    dcs_code: string | null;
    dcs_rx_code: string | null;
  },
>(input: T): T {
  const use = toneFieldsInUse(input.tone_mode, input.cross_mode);
  return {
    ...input,
    ctcss_uplink: use.ctcss_uplink ? input.ctcss_uplink : null,
    ctcss_downlink: use.ctcss_downlink ? input.ctcss_downlink : null,
    dcs_code: use.dcs_code ? input.dcs_code : null,
    dcs_rx_code: use.dcs_rx_code ? input.dcs_rx_code : null,
  };
}

// Radio-agnostic per-channel power. Mapped to each radio's levels on export.
export const POWER_LEVELS = ["High", "Med", "Low"];

// Standard CTCSS (PL) tone frequencies in Hz.
export const CTCSS_TONES = [
  67.0, 69.3, 71.9, 74.4, 77.0, 79.7, 82.5, 85.4, 88.5, 91.5, 94.8, 97.4,
  100.0, 103.5, 107.2, 110.9, 114.8, 118.8, 123.0, 127.3, 131.8, 136.5, 141.3,
  146.2, 151.4, 156.7, 159.8, 162.2, 165.5, 167.9, 171.3, 173.8, 177.3, 179.9,
  183.5, 186.2, 189.9, 192.8, 196.6, 199.5, 203.5, 206.5, 210.7, 218.1, 225.7,
  229.1, 233.6, 241.8, 250.3, 254.1,
];

// Standard DTCS (DCS) codes (octal), matching CHIRP's UV5R_DTCS set.
export const DTCS_CODES = [
  "023", "025", "026", "031", "032", "036", "043", "047", "051", "053", "054",
  "065", "071", "072", "073", "074", "114", "115", "116", "122", "125", "131",
  "132", "134", "143", "145", "152", "155", "156", "162", "165", "172", "174",
  "205", "212", "223", "225", "226", "243", "244", "245", "246", "251", "252",
  "255", "261", "263", "265", "266", "271", "274", "306", "311", "315", "325",
  "331", "332", "343", "346", "351", "356", "364", "365", "371", "411", "412",
  "413", "423", "431", "432", "445", "446", "452", "454", "455", "462", "464",
  "465", "466", "503", "506", "516", "523", "526", "532", "546", "565", "606",
  "612", "624", "627", "631", "632", "645", "654", "662", "664", "703", "712",
  "723", "731", "732", "734", "743", "754",
];
export const USE_TYPES = ["Open", "Closed", "Members"];
// Radio services a channel can belong to. Everything past "Amateur" is a
// regulated service with a fixed channel plan — see the standard lists.
export const SERVICE_TYPES = [
  "Amateur",
  "GMRS",
  "FRS",
  "MURS",
  "CB",
  "Marine",
  "Weather",
];

// DMR talkgroup library
export const DMR_NETWORKS = ["Brandmeister", "TGIF", "DMR-MARC", "Other"];
export const CALL_TYPES = ["Group", "Private"];
export const TIMESLOTS = [1, 2];

// Keep only the canonical options actually present in the data, preserving the
// canonical order; append any non-canonical values (sorted) so nothing is lost.
export function presentFacet(
  values: (string | null | undefined)[],
  canonical: readonly string[],
): string[] {
  const present = new Set<string>();
  for (const v of values) if (v != null && v !== "") present.add(v);
  const ordered = canonical.filter((c) => present.has(c));
  const extras = [...present].filter((v) => !canonical.includes(v)).sort();
  return [...ordered, ...extras];
}

/** Format a frequency in MHz, trimming trailing zeros (146.94000 -> "146.94"). */
export function fmtFreq(n: number | null | undefined): string {
  if (n == null) return "";
  return n.toFixed(5).replace(/0+$/, "").replace(/\.$/, "");
}

/** Format an offset (always positive in storage). */
export function fmtOffset(n: number | null | undefined): string {
  if (n == null || n === 0) return "";
  return n.toFixed(4).replace(/0+$/, "").replace(/\.$/, "");
}

/** Round to 4 decimals (100 Hz), the resolution of an amateur frequency. */
export function round4(n: number): number {
  return Math.round(n * 10_000) / 10_000;
}

/**
 * The customary repeater offset (MHz) for the band an RX/output frequency
 * falls in — what a "+" or "-" means when the operator doesn't say. Mirrors
 * `standard_offsets` in src-tauri/src/util.rs, taking the common choice where a
 * band has more than one. Returns null outside the repeater bands.
 */
export function defaultOffset(rx: number): number | null {
  if (!rx || rx <= 0) return null;
  if (rx < 30) return 0.1; // 10m
  if (rx < 54) return 0.5; // 6m
  if (rx < 148) return 0.6; // 2m
  if (rx < 225) return 1.6; // 1.25m
  if (rx < 470) return 5.0; // 70cm / GMRS
  if (rx < 928) return 12.0; // 33cm
  if (rx < 1300) return 12.0; // 23cm
  return null;
}

/**
 * Duplex direction and positive offset implied by a pair of frequencies — the
 * same rule the backend applies on save (`derive_duplex` in src-tauri/src/util.rs),
 * mirrored here so the panel can show what will be stored as the user types.
 */
export function deriveDuplex(
  rx: number,
  tx: number | null | undefined,
): { duplex: string; offset: number } {
  if (tx == null) return { duplex: "none", offset: 0 };
  const diff = tx - rx;
  if (Math.abs(diff) < 0.0001) return { duplex: "none", offset: 0 };
  // Unusually large separation -> treat as an odd/cross-band split.
  if (Math.abs(diff) > 15.0) return { duplex: "split", offset: round4(Math.abs(diff)) };
  return { duplex: diff > 0 ? "+" : "-", offset: round4(Math.abs(diff)) };
}

/** Sort rank for a channel's mode; lower sorts first, blank/unknown modes last. */
function zoneModeRank(mode: string | null | undefined): number {
  const m = (mode ?? "").trim().toUpperCase();
  if (!m) return Number.MAX_SAFE_INTEGER;
  const primary = ZONE_MODE_ORDER.indexOf(m);
  if (primary !== -1) return primary;
  // Everything else lands behind the primaries, keeping MODES order; values not
  // in MODES at all go to the back of that trailing group.
  const other = MODES.indexOf(m);
  return ZONE_MODE_ORDER.length + (other === -1 ? MODES.length : other);
}

/** Callsign sort key — channels without one sort after those that have one. */
function callsignKey(callsign: string | null | undefined): string {
  const c = (callsign ?? "").trim().toUpperCase();
  return c || "￿";
}

/**
 * Default ordering applied to the channels of a newly created zone/group:
 * by mode (see ZONE_MODE_ORDER), then callsign, then frequency so a station
 * with channels on several bands stays in band order.
 */
export function compareForNewZone(
  a: { mode: string | null; callsign: string | null; rx_freq: number },
  b: { mode: string | null; callsign: string | null; rx_freq: number },
): number {
  const rank = zoneModeRank(a.mode) - zoneModeRank(b.mode);
  if (rank !== 0) return rank;
  // Unknown modes share one rank; group them by name before falling to
  // callsign. Compare normalized so "fm" and "FM" tie rather than splitting.
  const mode = (a.mode ?? "")
    .trim()
    .toUpperCase()
    .localeCompare((b.mode ?? "").trim().toUpperCase());
  if (mode !== 0) return mode;
  const call = callsignKey(a.callsign).localeCompare(
    callsignKey(b.callsign),
    undefined,
    { numeric: true },
  );
  if (call !== 0) return call;
  return a.rx_freq - b.rx_freq;
}
