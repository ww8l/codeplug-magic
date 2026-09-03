// TypeScript mirrors of the Rust models. Field names are snake_case to match
// the JSON returned by the Tauri commands (serde serializes struct fields as-is).

export interface Channel {
  id: number;
  rb_name: string | null;
  name_long: string | null;
  name_short: string | null;
  callsign: string | null;
  rx_freq: number;
  tx_freq: number | null;
  offset: number | null;
  duplex: string | null;
  band: string | null;
  mode: string | null;
  tone_mode: string | null;
  ctcss_uplink: number | null;
  ctcss_downlink: number | null;
  dcs_code: string | null;
  dcs_rx_code: string | null;
  dcs_polarity: string;
  cross_mode: string;
  power: string | null;
  dmr_color_code: number | null;
  dmr_timeslot: number | null;
  dmr_talkgroup: number | null;
  dstar_capable: boolean;
  /// The three D-STAR call signs (issue #41). Null means "let the radio's
  /// driver derive it" — every D-STAR repeater imported from RepeaterBook
  /// arrives with a call sign and no module letter, so blank has to keep
  /// working. Stored as typed ("W0QEY B"); each driver packs it to the eight
  /// characters its radio wants.
  dstar_ur_call: string | null;
  dstar_rpt1: string | null;
  dstar_rpt2: string | null;
  ysf_capable: boolean;
  nxdn_capable: boolean;
  p25_capable: boolean;
  p25_nac: string | null;
  m17_capable: boolean;
  m17_can: number | null;
  tetra_capable: boolean;
  allstar_node: string | null;
  echolink_node: string | null;
  irlp_node: string | null;
  wires_node: string | null;
  use_type: string | null;
  operational_status: string | null;
  service_type: string | null;
  city: string | null;
  county: string | null;
  state: string | null;
  country: string | null;
  latitude: number | null;
  longitude: number | null;
  notes: string | null;
  source: string;
  repeaterbook_id: string | null;
  rb_ctcss_uplink: number | null;
  rb_ctcss_downlink: number | null;
  rb_operational_status: string | null;
  rb_notes: string | null;
  ctcss_uplink_overridden: boolean;
  ctcss_downlink_overridden: boolean;
  operational_status_overridden: boolean;
  notes_overridden: boolean;
  has_overrides: boolean;
  last_rb_update: string | null;
  last_user_edit: string | null;
  created_at: string | null;
}

// User-editable subset for create/update.
export interface ChannelInput {
  name_long: string | null;
  name_short: string | null;
  callsign: string | null;
  rx_freq: number;
  tx_freq: number | null;
  offset: number | null;
  duplex: string | null;
  mode: string | null;
  tone_mode: string | null;
  ctcss_uplink: number | null;
  ctcss_downlink: number | null;
  dcs_code: string | null;
  dcs_rx_code: string | null;
  dcs_polarity: string;
  cross_mode: string;
  power: string | null;
  dmr_color_code: number | null;
  dmr_timeslot: number | null;
  dmr_talkgroup: number | null;
  dstar_capable: boolean;
  /// The three D-STAR call signs (issue #41). Null means "let the radio's
  /// driver derive it" — every D-STAR repeater imported from RepeaterBook
  /// arrives with a call sign and no module letter, so blank has to keep
  /// working. Stored as typed ("W0QEY B"); each driver packs it to the eight
  /// characters its radio wants.
  dstar_ur_call: string | null;
  dstar_rpt1: string | null;
  dstar_rpt2: string | null;
  ysf_capable: boolean;
  nxdn_capable: boolean;
  p25_capable: boolean;
  p25_nac: string | null;
  m17_capable: boolean;
  m17_can: number | null;
  use_type: string | null;
  operational_status: string | null;
  service_type: string | null;
  city: string | null;
  county: string | null;
  state: string | null;
  country: string | null;
  latitude: number | null;
  longitude: number | null;
  notes: string | null;
  source: string | null;
}

export interface ChannelFilter {
  search?: string | null;
  band?: string | null;
  mode?: string | null;
  state?: string | null;
  operational_status?: string | null;
  source?: string | null;
}

export interface RadioModel {
  id: number;
  manufacturer: string;
  model: string;
  display_name: string;
  analog_capable: boolean;
  dmr_capable: boolean;
  dstar_capable: boolean;
  ysf_capable: boolean;
  nxdn_capable: boolean;
  p25_capable: boolean;
  m17_capable: boolean;
  aprs_capable: boolean;
  covers_hf: boolean;
  covers_vhf: boolean;
  covers_uhf: boolean;
  covers_220: boolean;
  covers_900: boolean;
  freq_min: number | null;
  freq_max: number | null;
  /// JSON `[[min_mhz, max_mhz], …]`: the bands the radio can transmit on, for
  /// radios whose TX coverage is not one contiguous span. Null = unsurveyed,
  /// and freq_min/freq_max plus the covers_* flags decide (migration 0016).
  tx_bands: string | null;
  /// Same shape, for the receiver. Null means "the same as transmit".
  rx_bands: string | null;
  memory_channels: number | null;
  zones_supported: boolean;
  max_zones: number | null;
  channels_per_zone: number | null;
  scan_lists_supported: boolean;
  max_scan_lists: number | null;
  banks_supported: boolean;
  max_name_length: number | null;
  export_format: string | null;
  connection_type: string | null;
  non_channel_settings_schema: string | null;
  /// Which compiled-in driver programs this radio, or null for export-only
  /// models. Every command that talks to a radio dispatches on this (Chunk 3.6).
  driver_key: string | null;
  /// Which "Program radio" dialog to render — see `lib/radioProgramming.ts`.
  /// Null falls back to the generic dialog (Chunk 3.7).
  programming_ui: string | null;
}

/// What a driver can actually do, derived on the Rust side from its trait impls
/// (`driver_capabilities`). Gate UI on this rather than on model names.
export interface DriverCapabilities {
  program_image: boolean;
  /// Can put one of its own backups back on the radio. NOT implied by
  /// `program_image`, which only means a backup can be taken.
  restore_image: boolean;
  read_settings: boolean;
  write_settings: boolean;
  write_channels: boolean;
  program_codeplug: boolean;
  /// Whether a codeplug program ALSO writes the profile's settings. Only the
  /// UV-5R does; every other radio here treats settings as a separate,
  /// explicitly-acknowledged write and leaves the radio's own alone.
  programs_settings: boolean;
  write_callsign_db: boolean;
  export: boolean;
  diagnostics: boolean;
}

// One field definition inside non_channel_settings_schema (JSON).
export interface SettingField {
  key: string;
  label: string;
  // "section" is a full-width heading used to group the fields that follow it;
  // it carries no value and is skipped when seeding/saving settings.
  type: "text" | "integer" | "select" | "boolean" | "section";
  default?: string | number | boolean;
  min?: number;
  max?: number;
  max_length?: number;
  options?: string[];
}

export interface RadioProfile {
  id: number;
  display_name: string;
  radio_model_id: number;
  non_channel_settings: string | null;
  notes: string | null;
  created_at: string | null;
  updated_at: string | null;
}

export interface RadioProfileInput {
  display_name: string;
  radio_model_id: number;
  non_channel_settings: string | null;
  notes: string | null;
}

export interface ChannelList {
  id: number;
  name: string;
  description: string | null;
  created_at: string | null;
  updated_at: string | null;
}

export interface ChannelListSummary {
  id: number;
  name: string;
  description: string | null;
  channel_count: number;
}

export interface CityCentroid {
  city: string;
  state: string | null;
  latitude: number;
  longitude: number;
  count: number;
}

export interface GeoPoint {
  latitude: number;
  longitude: number;
}

export interface BackfillResult {
  scanned: number;
  from_data: number;
  from_online: number;
  not_found: number;
  failed: number;
  /// Towns not in the user's own data, left alone because online lookup is off
  /// — their names were never sent anywhere.
  skipped_offline: number;
}

export interface ScanList {
  id: number;
  name: string;
  description: string | null;
  priority_channel_id: number | null;
  priority_channel_2_id: number | null;
  priority_select: number;
  look_back_a: number;
  look_back_b: number;
  dropout_delay: number;
  dwell_time: number;
  revert_channel: number;
  created_at: string | null;
}

export interface ScanListSummary {
  id: number;
  name: string;
  description: string | null;
  priority_channel_id: number | null;
  priority_channel_2_id: number | null;
  priority_select: number;
  look_back_a: number;
  look_back_b: number;
  dropout_delay: number;
  dwell_time: number;
  revert_channel: number;
  channel_count: number;
}

/** The eight editable per-scan-list settings sent to `update_scan_list`.
 *  Timers are in 0.1-s units (radio encoding). NOTE: priority_select and
 *  revert_channel enum bytes are NOT yet hardware-verified. */
export interface ScanListSettings {
  priority_channel_id: number | null;
  priority_channel_2_id: number | null;
  priority_select: number;
  look_back_a: number;
  look_back_b: number;
  dropout_delay: number;
  dwell_time: number;
  revert_channel: number;
}

/** A channel's explicit scan-list assignment within one codeplug (radio byte
 *  0x1B). Channels with no assignment program with no scan list. */
export interface ChannelScanListAssignment {
  channel_id: number;
  scan_list_id: number;
}

export interface Talkgroup {
  id: number;
  tg_number: number;
  name: string;
  network: string;
  call_type: string;
  notes: string | null;
  source: string;
  created_at: string | null;
  updated_at: string | null;
}

export interface TalkgroupInput {
  tg_number: number;
  name: string;
  network: string;
  call_type: string;
  notes: string | null;
}

export interface RepeaterTalkgroup {
  id: number;
  channel_id: number;
  talkgroup_id: number;
  timeslot: number;
  position: number;
  name_override: string | null;
  // joined from the talkgroup library
  tg_number: number;
  name: string;
  network: string;
  call_type: string;
}

// Mirrors the Rust `ParsedTalkgroup` — a talkgroup parsed from an import file.
export interface ParsedTalkgroup {
  tg_number: number;
  name: string;
  network: string;
  call_type: string;
  notes: string | null;
}

export interface TalkgroupImportPreview {
  total: number;
  rows: ParsedTalkgroup[];
}

// ============================================================
// DMR users (DMR-ID -> callsign/name lookup library, radioid.net)
// ============================================================
export interface DmrUser {
  id: number;
  dmr_id: number;
  callsign: string;
  first_name: string | null;
  last_name: string | null;
  city: string | null;
  state: string | null;
  country: string | null;
  remarks: string | null;
  source: string;
  updated_at: string;
}

export interface DmrUsersStatus {
  total: number;
  last_refreshed_at: string | null;
}

export interface DmrUsersRefreshSummary {
  fetched: number;
  added: number;
  updated: number;
  total: number;
}

export interface DmrExportCountryBreakdown {
  country: string;
  count: number;
}

export interface DmrExportPreview {
  total_available: number;
  will_export: number;
  by_country: DmrExportCountryBreakdown[];
}

export interface DmrExportSummary {
  written: number;
}

export interface Codeplug {
  id: number;
  name: string;
  radio_profile_id: number | null;
  notes: string | null;
  last_exported: string | null;
  last_export_kind: string | null;
  created_at: string | null;
  updated_at: string | null;
}

export interface CodeplugSummary {
  id: number;
  name: string;
  radio_profile_id: number | null;
  profile_name: string | null;
  model_display_name: string | null;
  last_exported: string | null;
  last_export_kind: string | null;
  channel_count: number;
}

export interface CodeplugInput {
  name: string;
  radio_profile_id: number | null;
  notes: string | null;
}

export interface DashboardStats {
  total_channels: number;
  total_channel_lists: number;
  total_scan_lists: number;
  total_codeplugs: number;
  total_radio_profiles: number;
  total_talkgroups: number;
}

export interface ExportPreviewRow {
  channel_id: number;
  name: string;
  rx_freq: number;
  mode: string | null;
  included: boolean;
  /// Programmed, but the radio cannot transmit there. `included` is still true;
  /// `reason` explains the restriction rather than an exclusion.
  receive_only: boolean;
  reason: string | null;
}

/// One zone on a radio whose zones are fixed blocks of memories (the BT-9000:
/// 15 blocks of 64). The radio stores no zone names, so this map is the only
/// place the operator can learn which list landed in which zone.
export interface PreviewZone {
  /// 1-based, as the radio's own zone selector shows it.
  number: number;
  list_name: string;
  channels: number;
}

export interface ExportPreview {
  codeplug_id: number;
  radio_model: string;
  // "chirp_csv" (single file) or "anytone_csv" (Channel + TalkGroups bundle).
  export_format: string;
  included_count: number;
  excluded_count: number;
  /// How many of `included_count` are receive-only.
  receive_only_count: number;
  rows: ExportPreviewRow[];
  /// The zone map, for fixed-zone radios; empty for every other model.
  /// ⚠ A channel in two of the codeplug's lists lands in BOTH zones and is
  /// programmed twice, so these summed can exceed `included_count`.
  zones: PreviewZone[];
  /// What to tell the operator about that layout.
  zone_notes: string[];
}

// ---- direct radio programming (UV-5R) ----
// Mirror of the Rust `program.rs` structs.
export interface PortInfo {
  name: string;
  // "usb" | "bluetooth" | "pci" | "unknown"
  kind: string;
  product: string | null;
}

// Registry-dispatched identify/download results (3.6c) — one shape for every
// radio. Mirrors `program.rs`'s `RadioIdent` / `DownloadResult`.
export interface RadioIdent {
  // A magic key like "UV5R_ORIG", or the model token the radio reported.
  matched_magic: string;
  ident_hex: string;
  // Printable ASCII ident ("P31183", "ID890UV"); null when the ident is binary.
  ident_ascii: string | null;
}

// The per-radio decoded shape, still used by the UV-5R program/verify result.
export interface DecodedChannel {
  index: number;
  name: string;
  rx_mhz: number;
  tone: string;
  power: string;
}

// The generic download sanity sample: a superset of what the image-clone radios
// decode. `shift` and `mode` are null on radios that don't report them (UV-5R).
export interface DecodedChannelSample {
  index: number;
  name: string;
  rx_mhz: number;
  shift: string | null;
  tone: string;
  power: string;
  mode: string | null;
}

export interface DownloadResult {
  matched_magic: string;
  ident_hex: string;
  ident_ascii: string | null;
  image_bytes: number;
  backup_path: string;
  channel_count: number;
  channels: DecodedChannelSample[];
}

export interface RadioSettingsRead {
  // Keyed by CHIRP setting name; values match the profile form (bool/number/label/text).
  settings: Record<string, string | number | boolean>;
  count: number;
  backup_path: string;
}

export interface RestoreResult {
  bytes: number;
  source: string;
}

// Unified outcome of a codeplug program, for every radio (Rust
// `CodeplugProgramReport`). Clone radios fill `verified`/`channels` (they read
// back in the same session) and leave the zone/scan-list/contact counts at
// zero. The AnyTone is the inverse: it reboots on commit, so it reports
// `expected_path` for a fresh-session byte-diff instead of `verified`.
export interface CodeplugProgramReport {
  channels_written: number;
  slots_cleared: number;
  // Non-channel settings written from the radio profile, or null when the
  // profile carried none and only channels/names went out.
  settings_written: number | null;
  zones_written: number;
  zones_cleared: number;
  scan_lists_written: number;
  scan_lists_cleared: number;
  contacts_written: number;
  contacts_cleared: number;
  verified: boolean | null;
  note: string | null;
  backup_path: string;
  expected_path: string | null;
  windows_written: string[];
  channels: DecodedChannelSample[];
  skipped: AnytoneSkippedChannel[];
  warnings: string[];
}

// ---- direct radio programming (TIDRADIO TD-H3) ----
// Mirror of the Rust `tdh3.rs` structs. Identify + download are generic since
// 3.6c (`RadioIdent` / `DownloadResult`); what's left here is TD-H3-specific.
export interface Tdh3DecodedChannel {
  index: number;
  name: string;
  rx_mhz: number;
  // TX shift as the radio applies it: "" (simplex), "−0.600"/"+5.000", or "RX-only".
  shift: string;
  tone: string;
  power: string;
  mode: string;
}

// ---- direct radio programming (AnyTone AT-D890UV, Stage 1: read-only) ----
// Mirror of the Rust `anytone.rs` structs.
export interface AnytoneRegionProbe {
  name: string;
  addr: string;
  requested: number;
  read: number;
  checksum_ok: boolean;
  preview: string;
  checksum_formula: string | null;
  error: string | null;
}

// One direction's sub-audible tone. Mirrors the Rust `AnytoneSubTone` enum
// (serde tag "kind" / content "value"): CTCSS carries Hz, DCS the raw stored code.
export type AnytoneSubTone =
  | { kind: "ctcss"; value: number }
  | { kind: "dcs"; value: number };

export interface AnytoneDecodedChannel {
  index: number;
  name: string;
  rx_mhz: number;
  tx_mhz: number;
  shift: string;
  mode: string;
  power: string;
  bandwidth: string;
  tone: string;
  tone_tx: AnytoneSubTone | null;
  tone_rx: AnytoneSubTone | null;
  color_code: number | null;
  time_slot: number | null;
  contact_index: number | null;
  contact_name: string | null;
}

export interface AnytoneDecodedZone {
  index: number;
  name: string;
  channels: string[];
  member_slots: number[];
}

// A decoded DMR contact (talkgroup) referenced by some channel.
export interface AnytoneDecodedContact {
  index: number;
  name: string;
  dmr_id: number;
  call_type: number; // 0 Private / 1 Group / 2 All
}

export interface AnytoneDownloadResult {
  ident_hex: string;
  ident_ascii: string;
  regions: AnytoneRegionProbe[];
  image_bytes: number;
  backup_path: string | null;
  channels: AnytoneDecodedChannel[];
  zones: AnytoneDecodedZone[];
  contacts: AnytoneDecodedContact[];
}

// Per-table added/skipped counts from `import_anytone_download`.
export interface AnytoneImportSummary {
  channels_added: number;
  channels_skipped: number;
  talkgroups_added: number;
  talkgroups_skipped: number;
  lists_added: number;
  lists_skipped: number;
}

// ---- "Program radio" from the DB (AT-D890UV full-replace channel set) ----
// Mirror of the Rust `anytone_program.rs` structs.
export interface AnytoneSkippedChannel {
  name: string;
  reason: string;
}

export interface AnytoneProgramPreview {
  radio: string;
  channels: number;
  zones: number;
  scan_lists: number;
  contacts: number;
  zone_names: string[];
  scan_list_names: string[];
  skipped: AnytoneSkippedChannel[];
  warnings: string[];
}

// One differing byte-run between two reads, used by program verification.
export interface AnytoneDumpDiff {
  addr: string;
  len: number;
  a_hex: string;
  b_hex: string;
}

export interface AnytoneVerifyResult {
  ok: boolean;
  bytes_checked: number;
  mismatches: AnytoneDumpDiff[];
}

// Result of pushing the radio profile's non-channel settings to the AT-D890UV.
// Outcome of a committed settings write, for every radio (Rust
// `SettingsWriteReport`). `verified` is null when the driver cannot read back
// in the same session — the AnyTone reboots on commit, so it verifies via a
// separate fresh-session byte-diff against `expected_path`.
export interface SettingsWriteReport {
  fields_written: number;
  verified: boolean | null;
  note: string | null;
  backup_path: string;
  expected_path: string | null;
  windows_written: string[];
}

// Outcome of a committed call-sign DB write (Rust `CallsignDbReport`). No
// backup path: the read that would make one is what corrupts a large commit.
export interface CallsignDbReport {
  entries_written: number;
  windows_written: string[];
  note: string;
}

// Newest program image pair on disk, so Verify/Restore stay reachable even if
// a program's result never made it back to the UI (USB drops on the reboot).
export interface AnytoneLatestProgram {
  backup_path: string;
  expected_path: string;
  stamp: string;
}

// Shared result shape for window-patch writes (zones/contacts/restore).
export interface AnytonePatchWriteResult {
  windows_written: string[];
  backup_path: string;
  note: string;
}

// Mirrors the Rust `ParsedChannel` — every field that will be imported, shown
// in the preview table before confirming.
export interface ImportPreviewRow {
  repeaterbook_id: string;
  rb_name: string;
  name_long: string;
  name_short: string;
  callsign: string;
  rx_freq: number;
  tx_freq: number | null;
  offset: number;
  duplex: string;
  band: string;
  mode: string;
  tone_mode: string;
  ctcss_uplink: number | null;
  ctcss_downlink: number | null;
  /// TX/RX DCS codes, 3-digit octal. Only the free-tier CSV supplies these;
  /// the premium "Full Data" JSON has no DCS field.
  dcs_code: string | null;
  dcs_rx_code: string | null;
  dmr_color_code: number | null;
  /// Only a mapped CSV import (#115) fills these; every RepeaterBook path
  /// leaves them null.
  dmr_timeslot: number | null;
  dmr_talkgroup: number | null;
  power: string | null;
  dstar_capable: boolean;
  ysf_capable: boolean;
  nxdn_capable: boolean;
  p25_capable: boolean;
  p25_nac: string | null;
  m17_capable: boolean;
  tetra_capable: boolean;
  allstar_node: string | null;
  echolink_node: string | null;
  irlp_node: string | null;
  wires_node: string | null;
  use_type: string | null;
  operational_status: string | null;
  city: string | null;
  county: string | null;
  state: string | null;
  country: string | null;
  latitude: number | null;
  longitude: number | null;
  notes: string | null;
}

export interface ImportPreview {
  total: number;
  rows: ImportPreviewRow[];
}

// ============================================================
// Mapping an arbitrary CSV's columns onto channel fields (#115)
// ============================================================

/** Mirrors the Rust `FieldKind`. */
export type CsvFieldKind = "text" | "freq" | "tone" | "dcs" | "number" | "enum";

/** A channel field a CSV column can be mapped onto. Mirrors `MappableField`. */
export interface CsvMappableField {
  key: string;
  label: string;
  group: string;
  kind: CsvFieldKind;
  required: boolean;
  help: string;
}

export interface CsvColumn {
  index: number;
  header: string;
  /** A few non-empty values from the top of the file. */
  samples: string[];
}

/** Field key -> zero-based column index. A field absent here is not mapped. */
export type ColumnMapping = Record<string, number>;

export interface CsvInspection {
  /** Set when the file is a RepeaterBook export and has its own importer. */
  recognized: string | null;
  columns: CsvColumn[];
  row_count: number;
  guess: ColumnMapping;
  fields: CsvMappableField[];
}

export interface ImportSummary {
  added: number;
  // RepeaterBook re-import: existing channels (matched by repeaterbook_id)
  // refreshed from the export; user overrides and custom names are preserved.
  updated: number;
  // Native-backup restore: existing channels left untouched (deduped).
  skipped: number;
}

// ============================================================
// Standard (built-in) channel lists — GMRS, FRS, MURS, marine, …
// ============================================================

export interface StandardListChannel {
  name: string;
  name_short: string;
  rx_freq: number;
  // null marks a receive-only channel (NOAA weather, marine 15 and 70).
  tx_freq: number | null;
  band: string;
  duplex: string;
  offset: number;
  mode: string;
  power: string | null;
  notes: string;
}

export interface StandardListInfo {
  id: string;
  name: string;
  full_name: string;
  description: string;
  service_type: string;
  bands: string[];
  channel_count: number;
  channels: StandardListChannel[];
}

export interface StandardImportSummary {
  added: number;
  // Already in the library (matched on frequency pair + name) and left alone.
  skipped: number;
  list_id: number | null;
  list_name: string | null;
  list_added: number;
}

// Native channel-backup preview. The backend projects each backed-up channel
// down to the same shape as an import row, so the import dialog can render
// either source with one table. (Full fidelity is preserved in the file and
// applied on import, not via the preview.)
export type ChannelBackupPreview = ImportPreview;

/// One table's contribution to a master backup, from `backup_contents`. The
/// backend derives this from the LIVE schema rather than a fixed list, so a
/// table added later shows up here on its own — `label` falls back to the raw
/// table name when nothing is known about it.
export interface BackupTable {
  table: string;
  label: string;
  rows: number;
  /// Why this line deserves a second look before the file is shared, or null
  /// for the user's own data.
  caution: string | null;
}

/// What an export produced. `path` is not always the path the UI asked for: a
/// card exporter handed a FOLDER creates a file in it and names it the way the
/// radio names its own, and the operator then picks that file by name on the
/// radio — so the name has to come back for the UI to say it out loud.
export interface CodeplugWritten {
  channels: number;
  path: string;
  /// Settings the export did NOT write because they sit outside the range the
  /// model's schema declares — null when there is nothing to say (#87).
  note: string | null;
}

/// A mounted memory card already holding a file the radio wrote for itself,
/// from `find_memory_cards`. Offering these beats asking the operator to
/// navigate to `FT5D/BACKUP/BACKUP.dat` or `ID-52/Setting/` themselves —
/// picking the wrong file is how you patch something that never reaches the
/// radio. A radio that names its own files can contribute several.
export interface MemoryCard {
  path: string;
  volume: string;
  /// A pristine `.orig` from an earlier write already sits beside it.
  has_original: boolean;
}

/// One radio's worth of files in `radio-backups/`, from `radio_backups_summary`.
export interface RadioBackupGroup {
  /// The filename token they share — a driver key on newer paths, a short radio
  /// name on the ones that predate them.
  key: string;
  label: string;
  files: number;
  bytes: number;
  /// Newest backup in the group as `YYYY-MM-DD`, or null if the file carries no
  /// readable time.
  newest: string | null;
  /// What a prune to `keep` per radio would remove from this group.
  prunable_files: number;
  prunable_bytes: number;
}

/// The whole folder. Reported so the operator can see what has accumulated;
/// nothing is ever deleted without them asking (#77).
export interface RadioBackupsSummary {
  dir: string;
  files: number;
  bytes: number;
  groups: RadioBackupGroup[];
  keep: number;
}

export interface BackupPruneResult {
  files_deleted: number;
  bytes_freed: number;
}

/// Result of downloading the BrandMeister talkgroup list. The list is not
/// bundled with the app — it is fetched when the operator asks, so nothing of
/// BrandMeister's is redistributed.
export interface TalkgroupRefreshSummary {
  fetched: number;
  /// Rows inserted. Talkgroups already in the library are left alone, so this
  /// is smaller than `fetched` on every run after the first.
  added: number;
  total: number;
}
