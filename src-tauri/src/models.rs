use serde::{Deserialize, Serialize};
use sqlx::FromRow;

// ============================================================
// channels
// ============================================================
#[derive(Debug, Clone, Default, Serialize, Deserialize, FromRow)]
pub struct Channel {
    pub id: i64,
    pub rb_name: Option<String>,
    pub name_long: Option<String>,
    pub name_short: Option<String>,
    pub callsign: Option<String>,
    pub rx_freq: f64,
    pub tx_freq: Option<f64>,
    pub offset: Option<f64>,
    pub duplex: Option<String>,
    pub band: Option<String>,
    pub mode: Option<String>,
    pub tone_mode: Option<String>,
    pub ctcss_uplink: Option<f64>,
    pub ctcss_downlink: Option<f64>,
    pub dcs_code: Option<String>,
    pub dcs_rx_code: Option<String>,
    pub dcs_polarity: String,
    pub cross_mode: String,
    pub power: Option<String>,
    pub dmr_color_code: Option<i64>,
    pub dmr_timeslot: Option<i64>,
    pub dmr_talkgroup: Option<i64>,
    pub dstar_capable: bool,
    pub ysf_capable: bool,
    pub nxdn_capable: bool,
    pub p25_capable: bool,
    pub p25_nac: Option<String>,
    pub m17_capable: bool,
    pub m17_can: Option<i64>,
    pub tetra_capable: bool,
    pub allstar_node: Option<String>,
    pub echolink_node: Option<String>,
    pub irlp_node: Option<String>,
    pub wires_node: Option<String>,
    pub ares: bool,
    pub races: bool,
    pub skywarn: bool,
    pub canwarn: bool,
    pub use_type: Option<String>,
    pub operational_status: Option<String>,
    pub service_type: Option<String>,
    pub city: Option<String>,
    pub county: Option<String>,
    pub state: Option<String>,
    pub country: Option<String>,
    pub latitude: Option<f64>,
    pub longitude: Option<f64>,
    pub notes: Option<String>,
    pub source: String,
    pub repeaterbook_id: Option<String>,
    pub rb_ctcss_uplink: Option<f64>,
    pub rb_ctcss_downlink: Option<f64>,
    pub rb_operational_status: Option<String>,
    pub rb_notes: Option<String>,
    pub ctcss_uplink_overridden: bool,
    pub ctcss_downlink_overridden: bool,
    pub operational_status_overridden: bool,
    pub notes_overridden: bool,
    pub has_overrides: bool,
    pub last_rb_update: Option<String>,
    pub last_user_edit: Option<String>,
    pub created_at: Option<String>,
}

fn default_polarity() -> String {
    "NN".to_string()
}
fn default_cross_mode() -> String {
    "Tone->Tone".to_string()
}

/// User-editable fields for creating / updating a channel.
/// RepeaterBook-managed columns (rb_*) are not part of this input.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChannelInput {
    pub name_long: Option<String>,
    pub name_short: Option<String>,
    pub callsign: Option<String>,
    pub rx_freq: f64,
    pub tx_freq: Option<f64>,
    pub offset: Option<f64>,
    pub duplex: Option<String>,
    pub mode: Option<String>,
    pub tone_mode: Option<String>,
    pub ctcss_uplink: Option<f64>,
    pub ctcss_downlink: Option<f64>,
    pub dcs_code: Option<String>,
    pub dcs_rx_code: Option<String>,
    #[serde(default = "default_polarity")]
    pub dcs_polarity: String,
    #[serde(default = "default_cross_mode")]
    pub cross_mode: String,
    pub power: Option<String>,
    pub dmr_color_code: Option<i64>,
    pub dmr_timeslot: Option<i64>,
    pub dmr_talkgroup: Option<i64>,
    #[serde(default)]
    pub dstar_capable: bool,
    #[serde(default)]
    pub ysf_capable: bool,
    #[serde(default)]
    pub nxdn_capable: bool,
    #[serde(default)]
    pub p25_capable: bool,
    pub p25_nac: Option<String>,
    #[serde(default)]
    pub m17_capable: bool,
    pub m17_can: Option<i64>,
    pub use_type: Option<String>,
    pub operational_status: Option<String>,
    pub service_type: Option<String>,
    pub city: Option<String>,
    pub county: Option<String>,
    pub state: Option<String>,
    pub country: Option<String>,
    pub latitude: Option<f64>,
    pub longitude: Option<f64>,
    pub notes: Option<String>,
    /// Where the channel data came from. Imports set this themselves
    /// ("repeaterbook"); for a manual channel the user supplies it (defaults to
    /// "manual" when blank). Omitted/None on update preserves the existing value.
    pub source: Option<String>,
}

/// Filters for listing channels.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ChannelFilter {
    pub search: Option<String>,
    pub band: Option<String>,
    pub mode: Option<String>,
    pub state: Option<String>,
    pub operational_status: Option<String>,
    pub source: Option<String>,
}

// ============================================================
// radio_models
// ============================================================
#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct RadioModel {
    pub id: i64,
    pub manufacturer: String,
    pub model: String,
    pub display_name: String,
    pub analog_capable: bool,
    pub dmr_capable: bool,
    pub dstar_capable: bool,
    pub ysf_capable: bool,
    pub nxdn_capable: bool,
    pub p25_capable: bool,
    pub m17_capable: bool,
    pub aprs_capable: bool,
    pub covers_hf: bool,
    pub covers_vhf: bool,
    pub covers_uhf: bool,
    pub covers_220: bool,
    pub covers_900: bool,
    pub freq_min: Option<f64>,
    pub freq_max: Option<f64>,
    pub memory_channels: Option<i64>,
    pub zones_supported: bool,
    pub max_zones: Option<i64>,
    pub channels_per_zone: Option<i64>,
    pub scan_lists_supported: bool,
    pub max_scan_lists: Option<i64>,
    pub banks_supported: bool,
    pub max_name_length: Option<i64>,
    pub export_format: Option<String>,
    pub connection_type: Option<String>,
    /// Live-USB driver key → `radios/<driver_key>/` (see migration 0014).
    /// NULL for export-only models with no serial driver (e.g. VR-N76).
    pub driver_key: Option<String>,
    /// Which frontend "Program radio" dialog to render: "generic" (default),
    /// "tdh3", "anytone", or NULL for no live-programming UI.
    pub programming_ui: Option<String>,
    pub non_channel_settings_schema: Option<String>,
}

// ============================================================
// radio_profiles
// ============================================================
#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct RadioProfile {
    pub id: i64,
    pub display_name: String,
    pub radio_model_id: i64,
    pub non_channel_settings: Option<String>,
    pub notes: Option<String>,
    pub created_at: Option<String>,
    pub updated_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RadioProfileInput {
    pub display_name: String,
    pub radio_model_id: i64,
    pub non_channel_settings: Option<String>,
    pub notes: Option<String>,
}

// ============================================================
// channel_lists
// ============================================================
#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct ChannelList {
    pub id: i64,
    pub name: String,
    pub description: Option<String>,
    pub created_at: Option<String>,
    pub updated_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct ChannelListSummary {
    pub id: i64,
    pub name: String,
    pub description: Option<String>,
    pub channel_count: i64,
}

/// A distinct city present in the channel data, with a representative
/// (averaged) coordinate. Used as the "distance from" origin picker.
#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct CityCentroid {
    pub city: String,
    pub state: Option<String>,
    pub latitude: f64,
    pub longitude: f64,
    pub count: i64,
}

// ============================================================
// scan_lists
// ============================================================
#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct ScanList {
    pub id: i64,
    pub name: String,
    pub description: Option<String>,
    pub priority_channel_id: Option<i64>,
    pub priority_channel_2_id: Option<i64>,
    pub priority_select: i64,
    pub look_back_a: i64,
    pub look_back_b: i64,
    pub dropout_delay: i64,
    pub dwell_time: i64,
    pub revert_channel: i64,
    pub created_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct ScanListSummary {
    pub id: i64,
    pub name: String,
    pub description: Option<String>,
    pub priority_channel_id: Option<i64>,
    pub priority_channel_2_id: Option<i64>,
    pub priority_select: i64,
    pub look_back_a: i64,
    pub look_back_b: i64,
    pub dropout_delay: i64,
    pub dwell_time: i64,
    pub revert_channel: i64,
    pub channel_count: i64,
}

/// The eight editable per-scan-list settings, as they arrive from the frontend
/// (channel ids, not radio slots). `priority_channel_id` is "priority channel 1".
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScanListSettings {
    pub priority_channel_id: Option<i64>,
    pub priority_channel_2_id: Option<i64>,
    pub priority_select: i64,
    pub look_back_a: i64,
    pub look_back_b: i64,
    pub dropout_delay: i64,
    pub dwell_time: i64,
    pub revert_channel: i64,
}

// ============================================================
// talkgroups
// ============================================================
#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct Talkgroup {
    pub id: i64,
    pub tg_number: i64,
    pub name: String,
    pub network: String,
    pub call_type: String,
    pub notes: Option<String>,
    pub source: String,
    pub created_at: Option<String>,
    pub updated_at: Option<String>,
}

/// User-editable fields for creating / updating a talkgroup.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TalkgroupInput {
    pub tg_number: i64,
    pub name: String,
    pub network: String,
    pub call_type: String,
    pub notes: Option<String>,
}

/// A talkgroup assigned to a repeater channel, joined with the talkgroup's
/// library fields for display. Returned by `get_repeater_talkgroups`.
#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct RepeaterTalkgroup {
    pub id: i64,
    pub channel_id: i64,
    pub talkgroup_id: i64,
    pub timeslot: i64,
    pub position: i64,
    pub name_override: Option<String>,
    // joined from talkgroups
    pub tg_number: i64,
    pub name: String,
    pub network: String,
    pub call_type: String,
}

// ============================================================
// codeplugs
// ============================================================
#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct Codeplug {
    pub id: i64,
    pub name: String,
    pub radio_profile_id: Option<i64>,
    pub notes: Option<String>,
    pub last_exported: Option<String>,
    pub last_export_kind: Option<String>,
    pub created_at: Option<String>,
    pub updated_at: Option<String>,
}

/// Summary row for the codeplug card grid.
#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct CodeplugSummary {
    pub id: i64,
    pub name: String,
    pub radio_profile_id: Option<i64>,
    pub profile_name: Option<String>,
    pub model_display_name: Option<String>,
    pub last_exported: Option<String>,
    pub last_export_kind: Option<String>,
    pub channel_count: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CodeplugInput {
    pub name: String,
    pub radio_profile_id: Option<i64>,
    pub notes: Option<String>,
}

// ============================================================
// dashboard
// ============================================================
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DashboardStats {
    pub total_channels: i64,
    pub total_channel_lists: i64,
    pub total_scan_lists: i64,
    pub total_codeplugs: i64,
    pub total_radio_profiles: i64,
    pub total_talkgroups: i64,
}

// ============================================================
// export
// ============================================================
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExportPreviewRow {
    pub channel_id: i64,
    pub name: String,
    pub rx_freq: f64,
    pub mode: Option<String>,
    pub included: bool,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExportPreview {
    pub codeplug_id: i64,
    pub radio_model: String,
    /// The exporter that will run: "chirp_csv" (single file) or "anytone_csv"
    /// (a Channel + TalkGroups CSV bundle). Lets the UI explain the output.
    pub export_format: String,
    pub included_count: usize,
    pub excluded_count: usize,
    pub rows: Vec<ExportPreviewRow>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImportSummary {
    pub added: usize,
    /// Existing rows (matched by `repeaterbook_id`) merged from the fresh export:
    /// RB-authoritative fields refreshed, user overrides and custom names
    /// preserved. Used by the RepeaterBook re-import path.
    #[serde(default)]
    pub updated: usize,
    /// Existing rows left untouched. The native-backup restore skips duplicates
    /// rather than merging them, so it reports here instead of `updated`.
    #[serde(default)]
    pub skipped: usize,
}

// ============================================================
// dmr_users : DMR-ID -> callsign/name lookup library (radioid.net)
// ============================================================
#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct DmrUser {
    pub id: i64,
    pub dmr_id: i64,
    pub callsign: String,
    pub first_name: Option<String>,
    pub last_name: Option<String>,
    pub city: Option<String>,
    pub state: Option<String>,
    pub country: Option<String>,
    pub remarks: Option<String>,
    pub source: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DmrUsersStatus {
    pub total: i64,
    pub last_refreshed_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DmrUsersRefreshSummary {
    pub fetched: usize,
    pub added: usize,
    pub updated: usize,
    pub total: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DmrExportCountryBreakdown {
    pub country: String,
    pub count: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DmrExportPreview {
    pub total_available: i64,
    pub will_export: i64,
    pub by_country: Vec<DmrExportCountryBreakdown>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DmrExportSummary {
    pub written: i64,
}
