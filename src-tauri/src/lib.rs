pub mod commands;
mod db;
mod error;
mod models;
mod radios;
mod seed;
mod util;

use db::AppState;
use tauri::Manager;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            // Database lives in the platform app-data directory.
            let data_dir = app
                .path()
                .app_data_dir()
                .expect("failed to resolve app data dir");
            let db_path = data_dir.join(db::DB_FILENAME);

            // Block on pool init during setup so commands always have state.
            let pool = tauri::async_runtime::block_on(db::init_pool(&db_path))
                .expect("failed to initialize database");

            app.manage(AppState { pool });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            // dashboard
            commands::dashboard::get_dashboard_stats,
            commands::dashboard::get_recent_codeplugs,
            // channels
            commands::channels::list_channels,
            commands::channels::list_cities,
            commands::channels::geocode_city,
            commands::channels::backfill_channel_coordinates,
            commands::channels::get_channel,
            commands::channels::create_channel,
            commands::channels::update_channel,
            commands::channels::delete_channel,
            commands::channels::delete_channels,
            commands::channels::accept_rb_value,
            // csv / json import
            commands::import::preview_csv_import,
            commands::import::import_csv,
            commands::import::preview_json_import,
            commands::import::import_json,
            // native channel backup (export / re-import selected channels)
            commands::channel_io::export_channels,
            commands::channel_io::is_channel_backup,
            commands::channel_io::preview_channel_import,
            commands::channel_io::import_channels,
            // built-in standard channel lists (GMRS, FRS, MURS, marine, …)
            commands::standard_lists::list_standard_lists,
            commands::standard_lists::import_standard_list,
            // channel lists
            commands::lists::list_channel_lists,
            commands::lists::create_channel_list,
            commands::lists::update_channel_list,
            commands::lists::delete_channel_list,
            commands::lists::get_channel_list_channels,
            commands::lists::add_channel_to_list,
            commands::lists::add_channels_to_list,
            commands::lists::remove_channel_from_list,
            commands::lists::reorder_channel_list,
            // scan lists
            commands::lists::list_scan_lists,
            commands::lists::create_scan_list,
            commands::lists::update_scan_list,
            commands::lists::delete_scan_list,
            commands::lists::get_scan_list_channels,
            commands::lists::add_channel_to_scan_list,
            commands::lists::add_channels_to_scan_list,
            commands::lists::remove_channel_from_scan_list,
            commands::lists::reorder_scan_list,
            // talkgroups
            commands::talkgroups::list_talkgroups,
            commands::talkgroups::create_talkgroup,
            commands::talkgroups::update_talkgroup,
            commands::talkgroups::delete_talkgroup,
            commands::talkgroups::get_repeater_talkgroups,
            commands::talkgroups::assign_talkgroup,
            commands::talkgroups::remove_talkgroup_assignment,
            commands::talkgroups::reorder_repeater_talkgroups,
            commands::talkgroups::preview_talkgroup_import,
            commands::talkgroups::import_talkgroups,
            commands::talkgroups::export_talkgroups,
            commands::talkgroups::is_talkgroup_backup,
            commands::talkgroups::preview_talkgroup_backup,
            commands::talkgroups::import_talkgroup_backup,
            // dmr users (DMR-ID -> callsign/name lookup library)
            commands::dmr_users::refresh_dmr_users,
            commands::dmr_users::dmr_users_status,
            commands::dmr_users::list_dmr_users,
            commands::dmr_users::list_dmr_user_countries,
            commands::dmr_users::list_dmr_continents,
            commands::dmr_users::preview_dmr_export,
            commands::dmr_users::export_dmr_users,
            // radio models & profiles
            commands::profiles::list_radio_models,
            commands::profiles::get_radio_model,
            commands::profiles::list_radio_profiles,
            commands::profiles::get_radio_profile,
            commands::profiles::create_radio_profile,
            commands::profiles::update_radio_profile,
            commands::profiles::delete_radio_profile,
            // codeplugs
            commands::codeplugs::list_codeplugs,
            commands::codeplugs::get_codeplug,
            commands::codeplugs::create_codeplug,
            commands::codeplugs::update_codeplug,
            commands::codeplugs::delete_codeplug,
            commands::codeplugs::get_codeplug_channel_lists,
            commands::codeplugs::add_channel_list_to_codeplug,
            commands::codeplugs::remove_channel_list_from_codeplug,
            commands::codeplugs::reorder_codeplug_channel_lists,
            commands::codeplugs::get_codeplug_scan_lists,
            commands::codeplugs::add_scan_list_to_codeplug,
            commands::codeplugs::remove_scan_list_from_codeplug,
            commands::codeplugs::get_codeplug_channel_scan_lists,
            commands::codeplugs::set_channel_scan_list,
            commands::codeplugs::clear_channel_scan_list,
            // export
            commands::export::export_preview,
            commands::export::generate_codeplug,
            // direct radio programming — registry-dispatched, all radios
            // (3.6c: identify/download_image; 3.6d: settings + call-sign DB)
            commands::program::list_serial_ports,
            commands::program::driver_capabilities,
            commands::program::identify_radio,
            commands::program::download_image,
            commands::program::read_radio_settings,
            commands::program::read_ft5d_settings_from_backup,
            commands::program::find_ft5d_memory_cards,
            commands::program::write_radio_settings,
            commands::program::write_callsign_db,
            commands::program::program_radio,
            // direct radio programming (UV-5R)
            commands::program::restore_image,
            commands::program::backups_dir,
            // direct radio programming (AnyTone AT-D890UV) — Stage 1: read-only
            commands::anytone::download_anytone_image,
            // radio download → library import (channels / zones→lists / contacts→TGs)
            commands::import::import_anytone_download,
            // "Program radio" from the DB: full-replace channel set (gated, backup)
            commands::anytone_program::program_anytone_preview,
            commands::anytone_program::verify_anytone_program,
            commands::anytone_program::restore_anytone_backup,
            commands::anytone_program::latest_anytone_program,
            // whole-database master backup & restore
            commands::backup::export_database,
            commands::backup::import_database,
            commands::backup::is_database_backup,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
