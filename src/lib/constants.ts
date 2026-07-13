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
