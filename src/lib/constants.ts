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
export const SERVICE_TYPES = ["Amateur", "GMRS"];

// DMR talkgroup library
export const DMR_NETWORKS = ["Brandmeister", "TGIF", "DMR-MARC", "Other"];
export const CALL_TYPES = ["Group", "Private"];
export const TIMESLOTS = [1, 2];

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
