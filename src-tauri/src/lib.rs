use std::sync::Mutex;
use tauri::Manager;

mod backup;
mod commands;
mod config;
mod credentials;
mod crypto;
mod db;
mod error;
mod profiles;
mod queue;
mod s3;
mod throttle;

#[cfg(test)]
mod integration_tests;

/// Clean up harpocrates temp files from all known temp directories.
/// Covers the system temp dir plus any per-profile custom temp dirs from the DB.
fn cleanup_temp_dirs(conn: &rusqlite::Connection) {
    let mut dirs = vec![std::env::temp_dir()];

    // Add any custom temp dirs configured in profiles.
    if let Ok(profiles) = db::list_profiles(conn) {
        for profile in profiles {
            if let Some(ref custom) = profile.temp_directory {
                let p = std::path::PathBuf::from(custom);
                if !dirs.contains(&p) {
                    dirs.push(p);
                }
            }
        }
    }

    for dir in &dirs {
        match crypto::cleanup_temp_files(dir) {
            Ok(0) => {}
            Ok(n) => println!("Cleaned up {} temp files from {:?}", n, dir),
            Err(e) => eprintln!("Warning: temp cleanup failed for {:?}: {}", dir, e),
        }
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let app_config = config::load_or_create_config().expect("Failed to load or create config");

    println!("Harpocrates config loaded: {:?}", app_config);
    println!("Database path: {}", app_config.database_path);

    let conn = db::init_database(&app_config.database_path)
        .expect("Failed to initialize database");

    println!("Database initialized successfully");

    // Clean up leftover temp files from any previous session.
    cleanup_temp_dirs(&conn);

    let op_queue = queue::OperationQueue::new();

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .manage(db::DbState(Mutex::new(conn)))
        .manage(throttle::global().clone())
        .manage(op_queue)
        .setup(|app| {
            let q = app.state::<queue::OperationQueue>();
            q.start_worker(app.handle().clone());
            Ok(())
        })
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::Destroyed = event {
                let db = window.app_handle().state::<db::DbState>();
                if let Ok(conn) = db.conn() {
                    cleanup_temp_dirs(&conn);
                };
            }
        })
        .invoke_handler(tauri::generate_handler![
            // Phase 1
            commands::get_table_count,
            // Phase 2: Profiles
            commands::create_profile,
            commands::get_profile_credentials,
            commands::list_profiles,
            commands::get_active_profile,
            commands::switch_profile,
            commands::update_profile,
            commands::delete_profile,
            commands::test_connection,
            commands::test_connection_params,
            // Phase 5: Backup
            commands::backup_file,
            commands::backup_directory,
            // Phase 6: Restore
            commands::restore_files,
            // Phase 7: Share
            commands::create_share_manifest,
            commands::receive_manifest,
            commands::download_from_manifest,
            commands::list_share_manifests_cmd,
            commands::get_share_manifest_files,
            commands::revoke_share_manifest,
            // Phase 8: Scramble
            commands::scramble,
            // Phase 9: Cleanup
            commands::scan_orphaned_local_entries,
            commands::cleanup_orphaned_local_entries,
            commands::scan_orphaned_s3_objects,
            commands::cleanup_orphaned_s3_objects,
            // Phase 10: Integrity
            commands::verify_integrity,
            // Phase 11: Export/Import
            commands::export_database,
            commands::import_database,
            // Phase 12: Profile config export/import
            commands::export_profile_config,
            commands::import_profile_config,
            // File browser
            commands::list_files,
            commands::delete_backup_entries,
            // Config
            commands::get_config,
            commands::set_database_path,
            // Throttle
            commands::set_throttle_limits,
            commands::get_throttle_limits,
            // Queue
            commands::cancel_operation,
            commands::get_queue,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
