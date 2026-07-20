import { invoke } from "@tauri-apps/api/core";
import type {
  Channel,
  ChannelFilter,
  ChannelInput,
  ChannelList,
  ChannelListSummary,
  CityCentroid,
  BackfillResult,
  Codeplug,
  CodeplugInput,
  CodeplugSummary,
  DashboardStats,
  GeoPoint,
  ExportPreview,
  ImportPreview,
  ImportSummary,
  RadioModel,
  RadioProfile,
  RadioProfileInput,
  RepeaterTalkgroup,
  DownloadResult,
  PortInfo,
  ProgramResult,
  RadioIdent,
  RadioSettingsRead,
  RestoreResult,
  Tdh3ProgramResult,
  Tdh3Settings,
  Tdh3ProfileApplyResult,
  AnytoneDownloadResult,
  AnytoneDecodedChannel,
  AnytoneDecodedContact,
  AnytoneDecodedZone,
  AnytoneImportSummary,
  AnytoneProgramPreview,
  AnytoneProgramResult,
  AnytoneSettingsProgramResult,
  AnytoneVerifyResult,
  AnytoneLatestProgram,
  AnytonePatchWriteResult,
  ScanList,
  ScanListSummary,
  ScanListSettings,
  ChannelScanListAssignment,
  Talkgroup,
  TalkgroupImportPreview,
  TalkgroupInput,
  DmrUser,
  DmrUsersStatus,
  DmrUsersRefreshSummary,
  DmrExportPreview,
  DmrExportSummary,
} from "./types";

// Tauri converts camelCase JS argument keys to the snake_case Rust parameter
// names automatically, so we pass camelCase here.

export const api = {
  // ---- dashboard ----
  getDashboardStats: () => invoke<DashboardStats>("get_dashboard_stats"),
  getRecentCodeplugs: () => invoke<CodeplugSummary[]>("get_recent_codeplugs"),

  // ---- channels ----
  listChannels: (filter?: ChannelFilter) =>
    invoke<Channel[]>("list_channels", { filter: filter ?? null }),
  listCities: () => invoke<CityCentroid[]>("list_cities"),
  geocodeCity: (
    city: string,
    state?: string | null,
    country?: string | null,
  ) =>
    invoke<GeoPoint | null>("geocode_city", {
      city,
      state: state ?? null,
      country: country ?? null,
    }),
  backfillChannelCoordinates: () =>
    invoke<BackfillResult>("backfill_channel_coordinates"),
  getChannel: (id: number) => invoke<Channel>("get_channel", { id }),
  createChannel: (input: ChannelInput) =>
    invoke<Channel>("create_channel", { input }),
  updateChannel: (id: number, input: ChannelInput) =>
    invoke<Channel>("update_channel", { id, input }),
  deleteChannel: (id: number) => invoke<void>("delete_channel", { id }),
  deleteChannels: (ids: number[]) =>
    invoke<number>("delete_channels", { ids }),
  acceptRbValue: (id: number, field: string) =>
    invoke<Channel>("accept_rb_value", { id, field }),

  // ---- csv / json import ----
  previewCsvImport: (path: string) =>
    invoke<ImportPreview>("preview_csv_import", { path }),
  importCsv: (path: string) => invoke<ImportSummary>("import_csv", { path }),
  previewJsonImport: (path: string) =>
    invoke<ImportPreview>("preview_json_import", { path }),
  importJson: (path: string) => invoke<ImportSummary>("import_json", { path }),

  // ---- native channel backup (export / re-import selected channels) ----
  exportChannels: (ids: number[], path: string) =>
    invoke<number>("export_channels", { ids, path }),
  isChannelBackup: (path: string) =>
    invoke<boolean>("is_channel_backup", { path }),
  previewChannelImport: (path: string) =>
    invoke<ImportPreview>("preview_channel_import", { path }),
  importChannels: (path: string) =>
    invoke<ImportSummary>("import_channels", { path }),

  // ---- whole-database master backup & restore ----
  exportDatabase: (path: string) =>
    invoke<string>("export_database", { path }),
  isDatabaseBackup: (path: string) =>
    invoke<boolean>("is_database_backup", { path }),
  importDatabase: (path: string) =>
    invoke<void>("import_database", { path }),

  // ---- channel lists ----
  listChannelLists: () =>
    invoke<ChannelListSummary[]>("list_channel_lists"),
  createChannelList: (name: string, description: string | null) =>
    invoke<ChannelList>("create_channel_list", { name, description }),
  updateChannelList: (id: number, name: string, description: string | null) =>
    invoke<void>("update_channel_list", { id, name, description }),
  deleteChannelList: (id: number) =>
    invoke<void>("delete_channel_list", { id }),
  getChannelListChannels: (listId: number) =>
    invoke<Channel[]>("get_channel_list_channels", { listId }),
  addChannelToList: (listId: number, channelId: number) =>
    invoke<void>("add_channel_to_list", { listId, channelId }),
  addChannelsToList: (listId: number, channelIds: number[]) =>
    invoke<number>("add_channels_to_list", { listId, channelIds }),
  removeChannelFromList: (listId: number, channelId: number) =>
    invoke<void>("remove_channel_from_list", { listId, channelId }),
  reorderChannelList: (listId: number, orderedChannelIds: number[]) =>
    invoke<void>("reorder_channel_list", { listId, orderedChannelIds }),

  // ---- scan lists ----
  listScanLists: () => invoke<ScanListSummary[]>("list_scan_lists"),
  createScanList: (name: string, description: string | null) =>
    invoke<ScanList>("create_scan_list", { name, description }),
  updateScanList: (
    id: number,
    name: string,
    description: string | null,
    settings: ScanListSettings,
  ) =>
    invoke<void>("update_scan_list", {
      id,
      name,
      description,
      settings,
    }),
  deleteScanList: (id: number) => invoke<void>("delete_scan_list", { id }),
  getScanListChannels: (listId: number) =>
    invoke<Channel[]>("get_scan_list_channels", { listId }),
  addChannelToScanList: (listId: number, channelId: number) =>
    invoke<void>("add_channel_to_scan_list", { listId, channelId }),
  addChannelsToScanList: (listId: number, channelIds: number[]) =>
    invoke<number>("add_channels_to_scan_list", { listId, channelIds }),
  removeChannelFromScanList: (listId: number, channelId: number) =>
    invoke<void>("remove_channel_from_scan_list", { listId, channelId }),
  reorderScanList: (listId: number, orderedChannelIds: number[]) =>
    invoke<void>("reorder_scan_list", { listId, orderedChannelIds }),

  // ---- talkgroups ----
  listTalkgroups: (search?: string) =>
    invoke<Talkgroup[]>("list_talkgroups", { search: search ?? null }),
  createTalkgroup: (input: TalkgroupInput) =>
    invoke<Talkgroup>("create_talkgroup", { input }),
  updateTalkgroup: (id: number, input: TalkgroupInput) =>
    invoke<Talkgroup>("update_talkgroup", { id, input }),
  deleteTalkgroup: (id: number) => invoke<void>("delete_talkgroup", { id }),
  getRepeaterTalkgroups: (channelId: number) =>
    invoke<RepeaterTalkgroup[]>("get_repeater_talkgroups", { channelId }),
  assignTalkgroup: (channelId: number, talkgroupId: number, timeslot: number) =>
    invoke<void>("assign_talkgroup", { channelId, talkgroupId, timeslot }),
  removeTalkgroupAssignment: (assignmentId: number) =>
    invoke<void>("remove_talkgroup_assignment", { assignmentId }),
  reorderRepeaterTalkgroups: (
    channelId: number,
    orderedAssignmentIds: number[],
  ) =>
    invoke<void>("reorder_repeater_talkgroups", {
      channelId,
      orderedAssignmentIds,
    }),
  previewTalkgroupImport: (path: string, network: string) =>
    invoke<TalkgroupImportPreview>("preview_talkgroup_import", { path, network }),
  importTalkgroups: (path: string, network: string) =>
    invoke<ImportSummary>("import_talkgroups", { path, network }),
  exportTalkgroups: (path: string) =>
    invoke<number>("export_talkgroups", { path }),
  isTalkgroupBackup: (path: string) =>
    invoke<boolean>("is_talkgroup_backup", { path }),
  previewTalkgroupBackup: (path: string) =>
    invoke<TalkgroupImportPreview>("preview_talkgroup_backup", { path }),
  importTalkgroupBackup: (path: string) =>
    invoke<ImportSummary>("import_talkgroup_backup", { path }),

  // ---- dmr users (DMR-ID -> callsign/name lookup library) ----
  refreshDmrUsers: () => invoke<DmrUsersRefreshSummary>("refresh_dmr_users"),
  dmrUsersStatus: () => invoke<DmrUsersStatus>("dmr_users_status"),
  listDmrUsers: (
    search: string | undefined,
    country: string | undefined,
    limit: number,
    offset: number,
  ) =>
    invoke<DmrUser[]>("list_dmr_users", {
      search: search ?? null,
      country: country ?? null,
      limit,
      offset,
    }),
  listDmrUserCountries: () => invoke<string[]>("list_dmr_user_countries"),
  listDmrContinents: () => invoke<string[]>("list_dmr_continents"),
  previewDmrExport: (maxCount: number | null, priorityCountries: string[]) =>
    invoke<DmrExportPreview>("preview_dmr_export", {
      maxCount,
      priorityCountries,
    }),
  exportDmrUsers: (
    path: string,
    maxCount: number | null,
    priorityCountries: string[],
  ) =>
    invoke<DmrExportSummary>("export_dmr_users", {
      path,
      maxCount,
      priorityCountries,
    }),

  // ---- radio models & profiles ----
  listRadioModels: () => invoke<RadioModel[]>("list_radio_models"),
  getRadioModel: (id: number) => invoke<RadioModel>("get_radio_model", { id }),
  listRadioProfiles: () => invoke<RadioProfile[]>("list_radio_profiles"),
  getRadioProfile: (id: number) =>
    invoke<RadioProfile>("get_radio_profile", { id }),
  createRadioProfile: (input: RadioProfileInput) =>
    invoke<RadioProfile>("create_radio_profile", { input }),
  updateRadioProfile: (id: number, input: RadioProfileInput) =>
    invoke<RadioProfile>("update_radio_profile", { id, input }),
  deleteRadioProfile: (id: number) =>
    invoke<void>("delete_radio_profile", { id }),

  // ---- codeplugs ----
  listCodeplugs: () => invoke<CodeplugSummary[]>("list_codeplugs"),
  getCodeplug: (id: number) => invoke<Codeplug>("get_codeplug", { id }),
  createCodeplug: (input: CodeplugInput) =>
    invoke<Codeplug>("create_codeplug", { input }),
  updateCodeplug: (id: number, input: CodeplugInput) =>
    invoke<Codeplug>("update_codeplug", { id, input }),
  deleteCodeplug: (id: number) => invoke<void>("delete_codeplug", { id }),
  getCodeplugChannelLists: (codeplugId: number) =>
    invoke<ChannelListSummary[]>("get_codeplug_channel_lists", { codeplugId }),
  addChannelListToCodeplug: (codeplugId: number, channelListId: number) =>
    invoke<void>("add_channel_list_to_codeplug", {
      codeplugId,
      channelListId,
    }),
  removeChannelListFromCodeplug: (codeplugId: number, channelListId: number) =>
    invoke<void>("remove_channel_list_from_codeplug", {
      codeplugId,
      channelListId,
    }),
  reorderCodeplugChannelLists: (codeplugId: number, orderedListIds: number[]) =>
    invoke<void>("reorder_codeplug_channel_lists", {
      codeplugId,
      orderedListIds,
    }),
  getCodeplugScanLists: (codeplugId: number) =>
    invoke<ScanListSummary[]>("get_codeplug_scan_lists", { codeplugId }),
  addScanListToCodeplug: (codeplugId: number, scanListId: number) =>
    invoke<void>("add_scan_list_to_codeplug", { codeplugId, scanListId }),
  removeScanListFromCodeplug: (codeplugId: number, scanListId: number) =>
    invoke<void>("remove_scan_list_from_codeplug", { codeplugId, scanListId }),
  getCodeplugChannelScanLists: (codeplugId: number) =>
    invoke<ChannelScanListAssignment[]>("get_codeplug_channel_scan_lists", {
      codeplugId,
    }),
  setChannelScanList: (
    codeplugId: number,
    channelId: number,
    scanListId: number,
  ) =>
    invoke<void>("set_channel_scan_list", { codeplugId, channelId, scanListId }),
  clearChannelScanList: (codeplugId: number, channelId: number) =>
    invoke<void>("clear_channel_scan_list", { codeplugId, channelId }),

  // ---- export ----
  exportPreview: (codeplugId: number) =>
    invoke<ExportPreview>("export_preview", { codeplugId }),
  generateCodeplug: (codeplugId: number, path: string) =>
    invoke<number>("generate_codeplug", { codeplugId, path }),

  // ---- direct radio programming: registry-dispatched, all radios (3.6c) ----
  // `driverKey` is the radio's `radio_models.driver_key`; the port alone carries
  // no model hint, so the caller says which radio it expects to find.
  listSerialPorts: () => invoke<PortInfo[]>("list_serial_ports"),
  identifyRadio: (driverKey: string, port: string) =>
    invoke<RadioIdent>("identify_radio", { driverKey, port }),
  downloadImage: (driverKey: string, port: string) =>
    invoke<DownloadResult>("download_image", { driverKey, port }),

  // ---- direct radio programming (UV-5R) ----
  programCodeplug: (codeplugId: number, port: string) =>
    invoke<ProgramResult>("program_codeplug", { codeplugId, port }),
  readRadioSettings: (port: string, profileId: number) =>
    invoke<RadioSettingsRead>("read_radio_settings", { port, profileId }),
  restoreImage: (port: string, path: string) =>
    invoke<RestoreResult>("restore_image", { port, path }),
  backupsDir: () => invoke<string>("backups_dir"),

  // ---- direct radio programming (TIDRADIO TD-H3) ----
  programTdh3Codeplug: (codeplugId: number, port: string) =>
    invoke<Tdh3ProgramResult>("program_tdh3_codeplug", { codeplugId, port }),
  readTdh3SettingsForProfile: (port: string, profileId: number) =>
    invoke<RadioSettingsRead>("read_tdh3_settings_for_profile", { port, profileId }),
  readAnytoneSettingsForProfile: (port: string, profileId: number) =>
    invoke<RadioSettingsRead>("read_anytone_settings_for_profile", { port, profileId }),
  applyTdh3Profile: (port: string, profileId: number) =>
    invoke<Tdh3ProfileApplyResult>("apply_tdh3_profile", { port, profileId }),
  saveTdh3SettingsToProfile: (
    settings: Tdh3Settings,
    modelId: number,
    profileId: number | null,
    newName: string | null,
  ) =>
    invoke<RadioProfile>("save_tdh3_settings_to_profile", {
      settings,
      modelId,
      profileId,
      newName,
    }),

  // ---- direct radio programming (AnyTone AT-D890UV, Stage 1: read-only) ----
  downloadAnytoneImage: (port: string) =>
    invoke<AnytoneDownloadResult>("download_anytone_image", { port }),
  importAnytoneDownload: (
    channels: AnytoneDecodedChannel[],
    zones: AnytoneDecodedZone[],
    contacts: AnytoneDecodedContact[],
  ) =>
    invoke<AnytoneImportSummary>("import_anytone_download", {
      channels,
      zones,
      contacts,
    }),

  // ---- "Program radio" from the DB (AT-D890UV full-replace channel set) ----
  programAnytonePreview: (codeplugId: number) =>
    invoke<AnytoneProgramPreview>("program_anytone_preview", { codeplugId }),
  programAnytoneCodeplug: (codeplugId: number, port: string) =>
    invoke<AnytoneProgramResult>("program_anytone_codeplug", { codeplugId, port }),
  programAnytoneSettings: (codeplugId: number, port: string) =>
    invoke<AnytoneSettingsProgramResult>("program_anytone_settings", { codeplugId, port }),
  programAnytoneCallsignDb: (
    codeplugId: number,
    port: string,
    maxCount: number | null,
    priorityCountries: string[],
  ) =>
    invoke<AnytonePatchWriteResult>("program_anytone_callsign_db", {
      codeplugId,
      port,
      maxCount,
      priorityCountries,
    }),
  verifyAnytoneProgram: (port: string, expectedPath: string) =>
    invoke<AnytoneVerifyResult>("verify_anytone_program", { port, expectedPath }),
  restoreAnytoneBackup: (port: string, backupPath: string) =>
    invoke<AnytonePatchWriteResult>("restore_anytone_backup", { port, backupPath }),
  latestAnytoneProgram: () =>
    invoke<AnytoneLatestProgram | null>("latest_anytone_program"),
};

// Wrap an async action so any rejection surfaces as a toast. Returns the
// resolved value, or undefined on error.
import { toast } from "sonner";

export async function withToast<T>(
  promise: Promise<T>,
  opts?: { success?: string; error?: string },
): Promise<T | undefined> {
  try {
    const result = await promise;
    if (opts?.success) toast.success(opts.success);
    return result;
  } catch (e) {
    const message = typeof e === "string" ? e : String(e);
    toast.error(opts?.error ?? message);
    return undefined;
  }
}
