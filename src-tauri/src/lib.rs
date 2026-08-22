pub mod commands;
mod db;
mod error;
mod models;
mod radios;
mod seed;
mod util;

use std::path::Path;

use db::AppState;
use tauri::Manager;
use tauri_plugin_dialog::{DialogExt, MessageDialogKind};

/// Turn a database startup failure into something the user can act on.
///
/// The one that matters is a DOWNGRADE, which is a normal thing to do after a
/// bad release: the older binary embeds fewer migrations than the database has
/// applied, and the app used to panic before any window existed (#73).
///
/// **The error is `VersionMissing`, not `VersionNotPresent`.** The issue named
/// the latter; simulating a real downgrade against the dev database showed sqlx
/// returns "migration N was previously applied but is missing in the resolved
/// migrations", so the downgrade branch never fired and the dialog offered
/// read-only/disk-full advice for a downgrade. `VersionNotPresent` belongs to a
/// different path (`undo`).
fn explain_startup_failure(db_path: &Path, e: &sqlx::Error) -> String {
    use sqlx::migrate::MigrateError;
    let path = db_path.display();
    if let sqlx::Error::Migrate(m) = e {
        match **m {
            MigrateError::VersionMissing(v) => {
                return format!(
                    "Your database was last used by a NEWER version of WW8L Codeplug Magic \
                     (it records update {v}, which this version does not have).\n\n\
                     Nothing has been changed and your data is intact at:\n{path}\n\n\
                     Install the newer version again to keep using it. To start over with an \
                     empty database instead, move that file somewhere safe first."
                );
            }
            MigrateError::VersionMismatch(v) => {
                return format!(
                    "One of this app's database updates (number {v}) does not match the one \
                     already applied to your database.\n\n\
                     Nothing has been changed and your data is intact at:\n{path}\n\n\
                     This usually means a damaged install — reinstalling WW8L Codeplug Magic \
                     is the fix."
                );
            }
            _ => {}
        }
    }
    format!(
        "WW8L Codeplug Magic could not open its database.\n\n{path}\n\n{e}\n\n\
         No data has been changed. If that folder is read-only or the disk is full, \
         fixing that and reopening the app is all that is needed."
    )
}

/// Report a startup failure the user can see, then exit.
///
/// `setup` runs BEFORE the event loop, so a dialog cannot be shown
/// synchronously from here — the plugin's `run_on_main_thread` queues it and it
/// appears once the loop starts. So the windows are hidden (a window with no
/// database can only show errors), the dialog is queued, and the process exits
/// when it is dismissed. `.expect` here instead killed the process before any
/// window existed: the app bounced in the Dock and vanished, with no dialog and
/// no log the user could find (#73).
fn fatal_startup_error(app: &tauri::App, message: String) {
    for (_, window) in app.webview_windows() {
        let _ = window.hide();
    }
    app.dialog()
        .message(message)
        .kind(MessageDialogKind::Error)
        .title("WW8L Codeplug Magic could not start")
        .show(|_| std::process::exit(1));
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            // Database lives in the platform app-data directory.
            let data_dir = match app.path().app_data_dir() {
                Ok(dir) => dir,
                Err(e) => {
                    fatal_startup_error(
                        app,
                        format!(
                            "WW8L Codeplug Magic could not work out where to keep its data.\n\n{e}"
                        ),
                    );
                    return Ok(());
                }
            };
            let db_path = data_dir.join(db::DB_FILENAME);

            // Block on pool init during setup so commands always have state.
            match tauri::async_runtime::block_on(db::init_pool(&db_path)) {
                Ok(pool) => {
                    app.manage(AppState { pool });
                }
                Err(e) => fatal_startup_error(app, explain_startup_failure(&db_path, &e)),
            }
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
            commands::channels::duplicate_channel,
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
            commands::program::read_id52_settings_from_card,
            commands::program::read_thd75_settings_from_card,
            commands::program::find_memory_cards,
            commands::program::write_radio_settings,
            commands::program::write_callsign_db,
            commands::program::program_radio,
            // direct radio programming (UV-5R)
            commands::program::restore_image,
            commands::radio_backups::backups_dir,
            commands::radio_backups::radio_backups_summary,
            commands::radio_backups::prune_radio_backups,
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Issue #73: a downgrade is a normal thing to do after a bad release, and
    /// it used to kill the process before any window existed. The dialog itself
    /// needs a running event loop, but the message it carries does not — and
    /// the message is the whole point: WHERE the data is and WHAT to do.
    #[test]
    fn a_downgrade_is_explained_as_a_downgrade() {
        let path = Path::new("/Users/op/Library/Application Support/x/codeplug_manager.sqlite3");
        // The variant a REAL downgrade produces, confirmed by putting a version
        // this binary does not have into the dev database's ledger and
        // launching the app. The issue named `VersionNotPresent`, which belongs
        // to a different path and never fires here — with that in the match,
        // the dialog fell through to the generic message and told the operator
        // to check their disk space.
        let err = sqlx::Error::Migrate(Box::new(sqlx::migrate::MigrateError::VersionMissing(99)));
        let msg = explain_startup_failure(path, &err);

        assert!(msg.contains("NEWER version"), "must name the cause: {msg}");
        assert!(msg.contains("99"), "must name the version it found: {msg}");
        assert!(msg.contains("codeplug_manager.sqlite3"), "must name the file: {msg}");
        assert!(msg.contains("intact"), "must say the data is safe: {msg}");
        assert!(!msg.contains("disk is full"), "a downgrade is not a disk problem: {msg}");
    }

    /// Anything else still names the path and the underlying error rather than
    /// disappearing — trigger 2 is an unwritable app-data directory.
    #[test]
    fn any_other_failure_still_names_the_path_and_the_error() {
        let path = Path::new("/locked/codeplug_manager.sqlite3");
        let err = sqlx::Error::Io(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "permission denied",
        ));
        let msg = explain_startup_failure(path, &err);
        assert!(msg.contains("/locked/codeplug_manager.sqlite3"));
        assert!(msg.contains("permission denied"));
    }

    /// `create_dir_all` failing used to be swallowed with `.ok()`, so an
    /// unwritable location surfaced as an opaque connect error instead of the
    /// real one.
    #[tokio::test]
    async fn an_unusable_data_directory_reports_the_real_error() {
        // A regular file where the parent directory should be: create_dir_all
        // cannot make a directory there.
        let blocker = std::env::temp_dir().join(format!("cpm_notadir_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&blocker);
        std::fs::write(&blocker, b"not a directory").unwrap();

        let err = db::init_pool(&blocker.join("codeplug_manager.sqlite3"))
            .await
            .expect_err("a file cannot be a parent directory");
        assert!(
            matches!(err, sqlx::Error::Io(_)),
            "the directory error must surface as itself, not as a connect failure: {err}"
        );

        let _ = std::fs::remove_file(&blocker);
    }
}
