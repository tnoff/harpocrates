use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use rusqlite::OptionalExtension;
use tauri::{Emitter, State};

use crate::backup;
use crate::credentials;
use crate::crypto;
use crate::db::{self, DbState};
use crate::error::AppError;
use crate::profiles;
use crate::s3::S3Client;

// ── Backup cancel & progress ─────────────────────────────────────────────────

pub struct BackupCancelState(Arc<AtomicBool>);

impl BackupCancelState {
    pub fn new() -> Self { Self(Arc::new(AtomicBool::new(false))) }
    pub fn reset(&self)  { self.0.store(false, Ordering::Relaxed); }
    pub fn cancel(&self) { self.0.store(true,  Ordering::Relaxed); }
    pub fn is_cancelled(&self) -> bool { self.0.load(Ordering::Relaxed) }
}

#[derive(serde::Serialize, Clone)]
struct BackupProgressEvent {
    processed: usize,
    total: usize,
    uploaded: usize,
    deduped: usize,
    skipped: usize,
    failed: usize,
    current_file: String,
}

#[derive(serde::Serialize, Clone)]
struct VerifyProgressEvent {
    processed: usize,
    total: usize,
    current_file: String,
    passed: usize,
    failed: usize,
    errors: usize,
}

#[derive(serde::Serialize, Clone)]
struct RestoreProgressEvent {
    processed: usize,
    total: usize,
    current_file: String,
    restored: usize,
    skipped: usize,
    failed: usize,
}

#[derive(serde::Serialize, Clone)]
struct ScrambleProgressEvent {
    processed: usize,
    total: usize,
    current_file: String,
    scrambled: usize,
    failed: usize,
}

// ══════════════════════════════════════════════════════
// Helpers
// ══════════════════════════════════════════════════════

fn get_active_profile_or_err(
    conn: &rusqlite::Connection,
) -> Result<db::Profile, AppError> {
    profiles::get_active_profile(conn)?
        .ok_or_else(|| AppError::Config("No active profile set".into()))
}

fn get_profile_encryption_key(profile: &db::Profile) -> Result<String, AppError> {
    credentials::get_encryption_key(&profile.name)
}

fn get_temp_dir(profile: &db::Profile) -> PathBuf {
    profile
        .temp_directory
        .as_ref()
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
}

async fn build_s3_client(profile: &db::Profile) -> Result<S3Client, AppError> {
    let access_key = credentials::get_s3_access_key(&profile.name)?;
    let secret_key = credentials::get_s3_secret_key(&profile.name)?;
    S3Client::new(
        &profile.s3_endpoint,
        profile.s3_region.as_deref(),
        &profile.s3_bucket,
        &access_key,
        &secret_key,
        profile.extra_env.as_deref(),
        crate::throttle::global().clone(),
    )
    .await
}

// ══════════════════════════════════════════════════════
// Phase 1: Basic
// ══════════════════════════════════════════════════════

#[tauri::command]
pub fn get_table_count(db: State<DbState>) -> Result<usize, AppError> {
    let conn = db.conn()?;
    let mut stmt = conn.prepare(
        "SELECT count(*) FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%'",
    )?;
    let count: usize = stmt.query_row([], |row: &rusqlite::Row| row.get(0))?;
    Ok(count)
}

// ══════════════════════════════════════════════════════
// Phase 2: Profiles
// ══════════════════════════════════════════════════════

#[derive(serde::Deserialize)]
pub struct CreateProfileInput {
    pub name: String,
    pub mode: String,
    pub s3_endpoint: String,
    pub s3_region: Option<String>,
    pub s3_bucket: String,
    pub s3_access_key: String,
    pub s3_secret_key: String,
    pub extra_env: Option<String>,
    pub relative_path: Option<String>,
    pub temp_directory: Option<String>,
    pub import_encryption_key: Option<String>,
}

#[tauri::command]
pub fn create_profile(
    db: State<DbState>,
    input: CreateProfileInput,
) -> Result<profiles::CreateProfileResult, AppError> {
    let conn = db.conn()?;
    profiles::create_profile(
        &conn,
        &input.name,
        &input.mode,
        &input.s3_endpoint,
        input.s3_region.as_deref(),
        &input.s3_bucket,
        &input.s3_access_key,
        &input.s3_secret_key,
        input.extra_env.as_deref(),
        input.relative_path.as_deref(),
        input.temp_directory.as_deref(),
        input.import_encryption_key.as_deref(),
    )
}

#[derive(serde::Serialize)]
pub struct ProfileCredentials {
    pub s3_access_key: String,
    pub s3_secret_key: String,
}

/// Retrieve the S3 access key and secret key for a profile from the OS keychain.
/// Used to pre-populate credential fields when editing an existing profile.
#[tauri::command]
pub fn get_profile_credentials(
    db: State<DbState>,
    profile_id: i64,
) -> Result<ProfileCredentials, AppError> {
    let conn = db.conn()?;
    let profile = db::get_profile_by_id(&conn, profile_id)?
        .ok_or_else(|| AppError::NotFound(format!("Profile {} not found", profile_id)))?;
    Ok(ProfileCredentials {
        s3_access_key: credentials::get_s3_access_key(&profile.name)?,
        s3_secret_key: credentials::get_s3_secret_key(&profile.name)?,
    })
}

#[tauri::command]
pub fn list_profiles(db: State<DbState>) -> Result<Vec<db::Profile>, AppError> {
    let conn = db.conn()?;
    db::list_profiles(&conn)
}

#[tauri::command]
pub fn get_active_profile(db: State<DbState>) -> Result<Option<db::Profile>, AppError> {
    let conn = db.conn()?;
    profiles::get_active_profile(&conn)
}

#[tauri::command]
pub fn switch_profile(db: State<DbState>, profile_id: i64) -> Result<db::Profile, AppError> {
    let conn = db.conn()?;
    profiles::switch_profile(&conn, profile_id)
}

#[derive(serde::Deserialize)]
pub struct UpdateProfileInput {
    pub id: i64,
    pub name: Option<String>,
    pub mode: Option<String>,
    pub s3_endpoint: Option<String>,
    pub s3_region: Option<Option<String>>,
    pub s3_bucket: Option<String>,
    pub s3_access_key: Option<String>,
    pub s3_secret_key: Option<String>,
    pub extra_env: Option<Option<String>>,
    pub relative_path: Option<Option<String>>,
    pub temp_directory: Option<Option<String>>,
}

#[tauri::command]
pub fn update_profile(
    db: State<DbState>,
    input: UpdateProfileInput,
) -> Result<db::Profile, AppError> {
    let conn = db.conn()?;
    profiles::update_profile(
        &conn,
        input.id,
        input.name.as_deref(),
        input.mode.as_deref(),
        input.s3_endpoint.as_deref(),
        input.s3_region.as_ref().map(|o| o.as_deref()),
        input.s3_bucket.as_deref(),
        input.s3_access_key.as_deref(),
        input.s3_secret_key.as_deref(),
        input.extra_env.as_ref().map(|o| o.as_deref()),
        input.relative_path.as_ref().map(|o| o.as_deref()),
        input.temp_directory.as_ref().map(|o| o.as_deref()),
    )
}

#[tauri::command]
pub fn delete_profile(db: State<DbState>, profile_id: i64) -> Result<(), AppError> {
    let conn = db.conn()?;
    profiles::delete_profile(&conn, profile_id)
}

#[tauri::command]
pub async fn test_connection(db: State<'_, DbState>) -> Result<String, AppError> {
    let profile = {
        let conn = db.conn()?;
        get_active_profile_or_err(&conn)?
    };
    let s3 = build_s3_client(&profile).await?;
    s3.head_bucket().await?;
    Ok("Connection successful".into())
}

#[tauri::command]
pub async fn test_connection_params(
    endpoint: String,
    region: Option<String>,
    bucket: String,
    access_key: String,
    secret_key: String,
    extra_env: Option<String>,
) -> Result<String, AppError> {
    let s3 = crate::s3::S3Client::new(
        &endpoint,
        region.as_deref(),
        &bucket,
        &access_key,
        &secret_key,
        extra_env.as_deref(),
        crate::throttle::global().clone(),
    )
    .await?;
    s3.head_bucket().await?;
    Ok("Connection successful".into())
}

// ══════════════════════════════════════════════════════
// Phase 5: Backup
// ══════════════════════════════════════════════════════

#[tauri::command]
pub async fn backup_file(db: State<'_, DbState>, file_path: String) -> Result<backup::BackupResult, AppError> {
    // Gather everything we need from DB before async work
    let (profile, existing_entry, encryption_key, temp_dir) = {
        let conn = db.conn()?;
        let profile = get_active_profile_or_err(&conn)?;
        let encryption_key = get_profile_encryption_key(&profile)?;
        let temp_dir = get_temp_dir(&profile);

        let full_path_str = file_path.clone();
        let stored_path = backup::strip_relative_path(&full_path_str, profile.relative_path.as_deref());

        // Compute MD5 and check for dedup
        let original_md5 = crypto::compute_file_md5(Path::new(&file_path))?;

        let existing: Option<db::BackupEntry> = {
            let mut stmt = conn.prepare(
                "SELECT id, profile_id, object_uuid, original_md5, encrypted_md5, file_size, created_at
                 FROM backup_entry WHERE profile_id = ?1 AND original_md5 = ?2 LIMIT 1",
            )?;
            stmt.query_row(rusqlite::params![profile.id, original_md5], |row: &rusqlite::Row| {
                Ok(db::BackupEntry {
                    id: row.get(0)?,
                    profile_id: row.get(1)?,
                    object_uuid: row.get(2)?,
                    original_md5: row.get(3)?,
                    encrypted_md5: row.get(4)?,
                    file_size: row.get(5)?,
                    created_at: row.get(6)?,
                })
            }).optional()?
        };

        if let Some(ref entry) = existing {
            // Dedup: just create local_file link
            let metadata = std::fs::metadata(Path::new(&file_path))?;
            let mtime = metadata.modified().ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_secs_f64());
            let size = metadata.len() as i64;
            db::insert_local_file(&conn, entry.id, &stored_path, mtime, Some(size))?;

            return Ok(backup::BackupResult {
                backup_entry_id: entry.id,
                object_uuid: entry.object_uuid.clone(),
                original_md5: entry.original_md5.clone(),
                encrypted_md5: entry.encrypted_md5.clone(),
                file_size: entry.file_size as u64,
                was_dedup: true,
            });
        }

        (profile, existing, encryption_key, temp_dir)
    };
    let _ = existing_entry;

    // Encrypt file
    let temp_path = crypto::generate_temp_path(&temp_dir);
    let encrypt_result = crypto::encrypt_file(Path::new(&file_path), &temp_path, &encryption_key)?;

    let object_uuid = uuid::Uuid::new_v4().to_string();

    // Upload to S3 (async)
    let s3 = build_s3_client(&profile).await?;
    let upload_result = s3.upload_object(&object_uuid, &temp_path).await;
    let _ = std::fs::remove_file(&temp_path);
    upload_result?;

    // Write DB records (re-lock)
    let stored_path = backup::strip_relative_path(&file_path, profile.relative_path.as_deref());
    let conn = db.conn()?;
    let entry_id = db::insert_backup_entry(
        &conn,
        profile.id,
        &object_uuid,
        &encrypt_result.original_md5,
        &encrypt_result.encrypted_md5,
        encrypt_result.file_size as i64,
    )?;

    let metadata = std::fs::metadata(Path::new(&file_path))?;
    let mtime = metadata.modified().ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs_f64());
    let size = metadata.len() as i64;
    db::insert_local_file(&conn, entry_id, &stored_path, mtime, Some(size))?;

    Ok(backup::BackupResult {
        backup_entry_id: entry_id,
        object_uuid,
        original_md5: encrypt_result.original_md5,
        encrypted_md5: encrypt_result.encrypted_md5,
        file_size: encrypt_result.file_size,
        was_dedup: false,
    })
}

#[tauri::command]
pub async fn backup_directory(
    app: tauri::AppHandle,
    db: State<'_, DbState>,
    cancel: State<'_, BackupCancelState>,
    dir_path: String,
    skip_patterns: Vec<String>,
    force_checksum: bool,
) -> Result<backup::BackupSummary, AppError> {
    cancel.reset();

    let profile = {
        let conn = db.conn()?;
        get_active_profile_or_err(&conn)?
    };

    let encryption_key = get_profile_encryption_key(&profile)?;
    let temp_dir = get_temp_dir(&profile);
    let s3 = build_s3_client(&profile).await?;

    let regexes: Vec<regex::Regex> = skip_patterns
        .iter()
        .map(|p| regex::Regex::new(p))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| AppError::Config(format!("Invalid skip pattern: {}", e)))?;

    // Scan directory (sync)
    let files = backup::scan_directory(Path::new(&dir_path), &regexes)?;
    let total_files = files.len();
    let mut summary = backup::BackupSummary {
        total_files,
        ..Default::default()
    };

    for file_path in &files {
        if cancel.is_cancelled() {
            break;
        }

        let full_path_str = file_path.to_string_lossy().to_string();
        let stored_path = backup::strip_relative_path(&full_path_str, profile.relative_path.as_deref());

        // Emit progress at start of each file so the UI stays responsive
        let _ = app.emit("backup:progress", BackupProgressEvent {
            processed: summary.processed,
            total: summary.total_files,
            uploaded: summary.uploaded,
            deduped: summary.deduped,
            skipped: summary.skipped,
            failed: summary.failed,
            current_file: full_path_str.clone(),
        });

        let metadata = match std::fs::metadata(file_path) {
            Ok(m) => m,
            Err(e) => {
                summary.failed += 1;
                summary.failures.push(backup::BackupFailure { path: full_path_str, error: e.to_string() });
                summary.processed += 1;
                continue;
            }
        };
        let current_mtime = metadata.modified().ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs_f64());
        let current_size = Some(metadata.len() as i64);

        // Change detection — needs DB lock (short scope)
        if !force_checksum {
            let skip = {
                let conn = db.conn()?;
                let mut stmt = conn.prepare(
                    "SELECT lf.cached_mtime, lf.cached_size
                     FROM local_file lf JOIN backup_entry be ON lf.backup_entry_id = be.id
                     WHERE be.profile_id = ?1 AND lf.local_path = ?2 LIMIT 1",
                )?;
                stmt.query_row(rusqlite::params![profile.id, stored_path], |row: &rusqlite::Row| {
                    let cached_mtime: Option<f64> = row.get(0)?;
                    let cached_size: Option<i64> = row.get(1)?;
                    let mtime_match = match (cached_mtime, current_mtime) {
                        (Some(c), Some(n)) => (c - n).abs() < 0.001,
                        _ => false,
                    };
                    Ok(mtime_match && cached_size == current_size)
                }).optional()?.unwrap_or(false)
            };

            if skip {
                summary.skipped += 1;
                summary.processed += 1;
                continue;
            }
        }

        // Compute MD5
        let original_md5 = match crypto::compute_file_md5(file_path) {
            Ok(md5) => md5,
            Err(e) => {
                summary.failed += 1;
                summary.failures.push(backup::BackupFailure { path: full_path_str, error: e.to_string() });
                summary.processed += 1;
                continue;
            }
        };

        // Dedup check (short DB lock)
        let existing_entry = {
            let conn = db.conn()?;
            let mut stmt = conn.prepare(
                "SELECT id, profile_id, object_uuid, original_md5, encrypted_md5, file_size, created_at
                 FROM backup_entry WHERE profile_id = ?1 AND original_md5 = ?2 LIMIT 1",
            )?;
            stmt.query_row(rusqlite::params![profile.id, original_md5], |row: &rusqlite::Row| {
                Ok(db::BackupEntry {
                    id: row.get(0)?,
                    profile_id: row.get(1)?,
                    object_uuid: row.get(2)?,
                    original_md5: row.get(3)?,
                    encrypted_md5: row.get(4)?,
                    file_size: row.get(5)?,
                    created_at: row.get(6)?,
                })
            }).optional()?
        };

        if let Some(entry) = existing_entry {
            // Dedup — just insert local_file
            let conn = db.conn()?;
            db::insert_local_file(&conn, entry.id, &stored_path, current_mtime, current_size)?;
            summary.deduped += 1;
            summary.processed += 1;
            continue;
        }

        // Encrypt + upload
        let temp_path = crypto::generate_temp_path(&temp_dir);
        let encrypt_result = match crypto::encrypt_file(file_path, &temp_path, &encryption_key) {
            Ok(r) => r,
            Err(e) => {
                let _ = std::fs::remove_file(&temp_path);
                summary.failed += 1;
                summary.failures.push(backup::BackupFailure { path: full_path_str, error: e.to_string() });
                summary.processed += 1;
                continue;
            }
        };

        let object_uuid = uuid::Uuid::new_v4().to_string();
        let upload_result = s3.upload_object(&object_uuid, &temp_path).await;
        let _ = std::fs::remove_file(&temp_path);

        match upload_result {
            Ok(()) => {
                let conn = db.conn()?;
                let entry_id = db::insert_backup_entry(
                    &conn, profile.id, &object_uuid,
                    &encrypt_result.original_md5, &encrypt_result.encrypted_md5,
                    encrypt_result.file_size as i64,
                )?;
                db::insert_local_file(&conn, entry_id, &stored_path, current_mtime, current_size)?;
                summary.uploaded += 1;
            }
            Err(e) => {
                summary.failed += 1;
                summary.failures.push(backup::BackupFailure { path: full_path_str, error: e.to_string() });
            }
        }
        summary.processed += 1;
    }

    Ok(summary)
}

#[tauri::command]
pub fn cancel_backup(cancel: State<BackupCancelState>) -> Result<(), AppError> {
    cancel.cancel();
    Ok(())
}

// ══════════════════════════════════════════════════════
// Phase 6: Restore
// ══════════════════════════════════════════════════════

#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct RestoreSummary {
    pub total: usize,
    pub restored: usize,
    pub skipped: usize,
    pub failed: usize,
    pub failures: Vec<RestoreFailure>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct RestoreFailure {
    pub filename: String,
    pub error: String,
}

#[tauri::command]
pub async fn restore_files(
    app: tauri::AppHandle,
    db: State<'_, DbState>,
    backup_entry_ids: Vec<i64>,
    target_directory: Option<String>,
) -> Result<RestoreSummary, AppError> {
    let (profile, entries) = {
        let conn = db.conn()?;
        let profile = get_active_profile_or_err(&conn)?;
        let mut entries = Vec::new();
        for id in &backup_entry_ids {
            if let Some(entry) = db::get_backup_entry_by_id(&conn, *id)? {
                let local_files = db::list_local_files(&conn, entry.id)?;
                let path = local_files.first().map(|lf| lf.local_path.clone());
                entries.push((entry, path));
            }
        }
        (profile, entries)
    };

    let encryption_key = get_profile_encryption_key(&profile)?;
    let temp_dir = get_temp_dir(&profile);
    let s3 = build_s3_client(&profile).await?;

    let mut summary = RestoreSummary { total: entries.len(), ..Default::default() };

    for (entry, stored_path) in &entries {
        let current_filename = stored_path.as_ref()
            .and_then(|p| Path::new(p).file_name())
            .map(|f| f.to_string_lossy().to_string())
            .unwrap_or_else(|| entry.object_uuid.clone());
        let _ = app.emit("restore:progress", RestoreProgressEvent {
            processed: summary.restored + summary.skipped + summary.failed,
            total: summary.total,
            current_file: current_filename,
            restored: summary.restored,
            skipped: summary.skipped,
            failed: summary.failed,
        });

        let target_path = match &target_directory {
            Some(dir) => {
                let filename = stored_path.as_ref()
                    .and_then(|p| Path::new(p).file_name())
                    .map(|f| f.to_string_lossy().to_string())
                    .unwrap_or_else(|| entry.object_uuid.clone());
                PathBuf::from(dir).join(filename)
            }
            None => match stored_path {
                Some(sp) => backup::expand_relative_path(sp, profile.relative_path.as_deref()),
                None => {
                    summary.failed += 1;
                    summary.failures.push(RestoreFailure {
                        filename: entry.object_uuid.clone(),
                        error: "No local path recorded".into(),
                    });
                    continue;
                }
            },
        };

        // Check if already restored
        if target_path.exists() {
            if let Ok(md5) = crypto::compute_file_md5(&target_path) {
                if md5 == entry.original_md5 {
                    summary.skipped += 1;
                    continue;
                }
            }
        }

        if let Some(parent) = target_path.parent() {
            std::fs::create_dir_all(parent).ok();
        }

        let temp_encrypted = crypto::generate_temp_path(&temp_dir);
        if let Err(e) = s3.download_object(&entry.object_uuid, &temp_encrypted).await {
            let _ = std::fs::remove_file(&temp_encrypted);
            summary.failed += 1;
            summary.failures.push(RestoreFailure { filename: target_path.to_string_lossy().into(), error: e.to_string() });
            continue;
        }

        let result = crypto::decrypt_file(&temp_encrypted, &target_path, &encryption_key);
        let _ = std::fs::remove_file(&temp_encrypted);

        match result {
            Ok(()) => summary.restored += 1,
            Err(e) => {
                summary.failed += 1;
                summary.failures.push(RestoreFailure { filename: target_path.to_string_lossy().into(), error: e.to_string() });
            }
        }
    }

    Ok(summary)
}

// ══════════════════════════════════════════════════════
// Phase 7: Share Manifests
// ══════════════════════════════════════════════════════

#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub struct ManifestFileEntry {
    pub uuid: String,
    pub filename: String,
    pub size: i64,
}

#[derive(Debug, serde::Serialize)]
pub struct ManifestFileList {
    pub manifest_uuid: String,
    pub files: Vec<ManifestFileEntry>,
}

#[tauri::command]
pub async fn create_share_manifest(
    db: State<'_, DbState>,
    backup_entry_ids: Vec<i64>,
    label: Option<String>,
) -> Result<String, AppError> {
    let (profile, entries_with_filenames) = {
        let conn = db.conn()?;
        let profile = get_active_profile_or_err(&conn)?;
        let mut entries = Vec::new();
        for id in &backup_entry_ids {
            if let Some(entry) = db::get_backup_entry_by_id(&conn, *id)? {
                let local_files = db::list_local_files(&conn, entry.id)?;
                let filename = local_files.first()
                    .map(|lf| Path::new(&lf.local_path).file_name()
                        .map(|f| f.to_string_lossy().to_string())
                        .unwrap_or_else(|| lf.local_path.clone()))
                    .unwrap_or_else(|| entry.object_uuid.clone());
                entries.push((entry, filename));
            }
        }
        (profile, entries)
    };

    let encryption_key = get_profile_encryption_key(&profile)?;
    let temp_dir = get_temp_dir(&profile);
    let s3 = build_s3_client(&profile).await?;

    let manifest_files: Vec<ManifestFileEntry> = entries_with_filenames.iter()
        .map(|(e, filename)| ManifestFileEntry {
            uuid: e.object_uuid.clone(),
            filename: filename.clone(),
            size: e.file_size,
        })
        .collect();

    let manifest_json = serde_json::to_vec(&serde_json::json!({ "files": manifest_files }))?;

    let temp_plain = crypto::generate_temp_path(&temp_dir);
    let temp_encrypted = crypto::generate_temp_path(&temp_dir);
    std::fs::write(&temp_plain, &manifest_json)?;
    crypto::encrypt_file(&temp_plain, &temp_encrypted, &encryption_key)?;
    let _ = std::fs::remove_file(&temp_plain);

    let manifest_uuid = uuid::Uuid::new_v4().to_string();
    let upload_result = s3.upload_object(&manifest_uuid, &temp_encrypted).await;
    let _ = std::fs::remove_file(&temp_encrypted);
    upload_result?;

    // Insert DB records
    {
        let conn = db.conn()?;
        let manifest_id = db::insert_share_manifest(
            &conn, profile.id, &manifest_uuid,
            label.as_deref(), entries_with_filenames.len() as i64,
        )?;
        for (entry, filename) in &entries_with_filenames {
            db::insert_share_manifest_entry(&conn, manifest_id, entry.id, filename)?;
        }
    }

    Ok(manifest_uuid)
}

#[tauri::command]
pub async fn receive_manifest(
    db: State<'_, DbState>,
    manifest_uuid: String,
) -> Result<ManifestFileList, AppError> {
    let profile = {
        let conn = db.conn()?;
        get_active_profile_or_err(&conn)?
    };

    let encryption_key = get_profile_encryption_key(&profile)?;
    let temp_dir = get_temp_dir(&profile);
    let s3 = build_s3_client(&profile).await?;

    let temp_encrypted = crypto::generate_temp_path(&temp_dir);
    let temp_decrypted = crypto::generate_temp_path(&temp_dir);

    s3.download_object(&manifest_uuid, &temp_encrypted).await?;
    crypto::decrypt_file(&temp_encrypted, &temp_decrypted, &encryption_key)?;
    let _ = std::fs::remove_file(&temp_encrypted);

    let manifest_json = std::fs::read_to_string(&temp_decrypted)?;
    let _ = std::fs::remove_file(&temp_decrypted);

    let manifest: serde_json::Value = serde_json::from_str(&manifest_json)?;
    let files: Vec<ManifestFileEntry> = serde_json::from_value(
        manifest.get("files").ok_or_else(|| AppError::InvalidData("manifest is missing 'files' key".into()))?.clone(),
    )?;

    Ok(ManifestFileList { manifest_uuid, files })
}

#[tauri::command]
pub async fn download_from_manifest(
    app: tauri::AppHandle,
    db: State<'_, DbState>,
    manifest_uuid: String,
    selected_uuids: Vec<String>,
    save_directory: String,
) -> Result<RestoreSummary, AppError> {
    let profile = {
        let conn = db.conn()?;
        get_active_profile_or_err(&conn)?
    };

    let encryption_key = get_profile_encryption_key(&profile)?;
    let temp_dir = get_temp_dir(&profile);
    let s3 = build_s3_client(&profile).await?;

    let temp_encrypted = crypto::generate_temp_path(&temp_dir);
    let temp_decrypted = crypto::generate_temp_path(&temp_dir);

    s3.download_object(&manifest_uuid, &temp_encrypted).await?;
    crypto::decrypt_file(&temp_encrypted, &temp_decrypted, &encryption_key)?;
    let _ = std::fs::remove_file(&temp_encrypted);

    let manifest_json = std::fs::read_to_string(&temp_decrypted)?;
    let _ = std::fs::remove_file(&temp_decrypted);

    let manifest: serde_json::Value = serde_json::from_str(&manifest_json)?;
    let files: Vec<ManifestFileEntry> = serde_json::from_value(
        manifest.get("files").ok_or_else(|| AppError::InvalidData("manifest is missing 'files' key".into()))?.clone(),
    )?;

    let to_download: Vec<&ManifestFileEntry> = if selected_uuids.is_empty() {
        files.iter().collect()
    } else {
        files.iter().filter(|f| selected_uuids.contains(&f.uuid)).collect()
    };

    let save_dir = Path::new(&save_directory);
    std::fs::create_dir_all(save_dir)?;

    let mut summary = RestoreSummary { total: to_download.len(), ..Default::default() };

    for file_entry in to_download {
        let _ = app.emit("restore:progress", RestoreProgressEvent {
            processed: summary.restored + summary.skipped + summary.failed,
            total: summary.total,
            current_file: file_entry.filename.clone(),
            restored: summary.restored,
            skipped: summary.skipped,
            failed: summary.failed,
        });

        let target_path = save_dir.join(&file_entry.filename);
        let temp_enc = crypto::generate_temp_path(&temp_dir);

        match s3.download_object(&file_entry.uuid, &temp_enc).await {
            Ok(()) => {
                let result = crypto::decrypt_file(&temp_enc, &target_path, &encryption_key);
                let _ = std::fs::remove_file(&temp_enc);
                match result {
                    Ok(()) => summary.restored += 1,
                    Err(e) => {
                        summary.failed += 1;
                        summary.failures.push(RestoreFailure { filename: file_entry.filename.clone(), error: e.to_string() });
                    }
                }
            }
            Err(e) => {
                let _ = std::fs::remove_file(&temp_enc);
                summary.failed += 1;
                summary.failures.push(RestoreFailure { filename: file_entry.filename.clone(), error: e.to_string() });
            }
        }
    }

    Ok(summary)
}

#[tauri::command]
pub fn list_share_manifests_cmd(db: State<DbState>) -> Result<Vec<db::ShareManifest>, AppError> {
    let conn = db.conn()?;
    let profile = get_active_profile_or_err(&conn)?;
    db::list_share_manifests(&conn, profile.id)
}

#[tauri::command]
pub fn get_share_manifest_files(db: State<DbState>, manifest_id: i64) -> Result<Vec<ManifestFileEntry>, AppError> {
    let conn = db.conn()?;
    let entries = db::list_share_manifest_entries(&conn, manifest_id)?;
    let mut files = Vec::new();
    for entry in entries {
        if let Some(be) = db::get_backup_entry_by_id(&conn, entry.backup_entry_id)? {
            files.push(ManifestFileEntry { uuid: be.object_uuid, filename: entry.filename, size: be.file_size });
        }
    }
    Ok(files)
}

#[tauri::command]
pub async fn revoke_share_manifest(db: State<'_, DbState>, manifest_id: i64) -> Result<(), AppError> {
    let (profile, manifest) = {
        let conn = db.conn()?;
        let profile = get_active_profile_or_err(&conn)?;
        let manifest = db::get_share_manifest_by_id(&conn, manifest_id)?
            .ok_or_else(|| AppError::NotFound("manifest not found".into()))?;
        (profile, manifest)
    };

    let s3 = build_s3_client(&profile).await?;
    s3.delete_object(&manifest.manifest_uuid).await.ok();

    let conn = db.conn()?;
    db::delete_share_manifest(&conn, manifest_id)?;
    Ok(())
}

// ══════════════════════════════════════════════════════
// Phase 8: Scramble
// ══════════════════════════════════════════════════════

#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct ScrambleSummary {
    pub files_scrambled: usize,
    pub manifests_invalidated: usize,
    pub failed: usize,
    pub failures: Vec<String>,
}

#[tauri::command]
pub async fn scramble(
    app: tauri::AppHandle,
    db: State<'_, DbState>,
    backup_entry_ids: Vec<i64>,
    scramble_all: bool,
) -> Result<ScrambleSummary, AppError> {
    let (profile, entries) = {
        let conn = db.conn()?;
        let profile = get_active_profile_or_err(&conn)?;
        let entries = if scramble_all {
            db::list_backup_entries(&conn, profile.id)?
        } else {
            let mut e = Vec::new();
            for id in &backup_entry_ids {
                if let Some(entry) = db::get_backup_entry_by_id(&conn, *id)? {
                    e.push(entry);
                }
            }
            e
        };
        (profile, entries)
    };

    let s3 = build_s3_client(&profile).await?;
    let mut summary = ScrambleSummary::default();
    let mut scrambled_entry_ids = Vec::new();

    for entry in &entries {
        let _ = app.emit("scramble:progress", ScrambleProgressEvent {
            processed: summary.files_scrambled + summary.failed,
            total: entries.len(),
            current_file: entry.object_uuid[..8].to_string() + "...",
            scrambled: summary.files_scrambled,
            failed: summary.failed,
        });

        let new_uuid = uuid::Uuid::new_v4().to_string();
        match s3.copy_object(&entry.object_uuid, &new_uuid).await {
            Ok(()) => match s3.delete_object(&entry.object_uuid).await {
                Ok(()) => {
                    let conn = db.conn()?;
                    db::update_backup_entry_uuid(&conn, entry.id, &new_uuid)?;
                    summary.files_scrambled += 1;
                    scrambled_entry_ids.push(entry.id);
                }
                Err(e) => {
                    s3.delete_object(&new_uuid).await.ok();
                    summary.failed += 1;
                    summary.failures.push(format!("Delete old {}: {}", entry.object_uuid, e));
                }
            },
            Err(e) => {
                summary.failed += 1;
                summary.failures.push(format!("Copy {}: {}", entry.object_uuid, e));
            }
        }
    }

    // Invalidate affected manifests
    if !scrambled_entry_ids.is_empty() {
        let conn = db.conn()?;
        let manifests = db::list_share_manifests(&conn, profile.id)?;
        for manifest in &manifests {
            if !manifest.is_valid { continue; }
            let entries = db::list_share_manifest_entries(&conn, manifest.id)?;
            if entries.iter().any(|e| scrambled_entry_ids.contains(&e.backup_entry_id)) {
                db::invalidate_share_manifest(&conn, manifest.id)?;
                summary.manifests_invalidated += 1;
            }
        }
    }

    Ok(summary)
}

// ══════════════════════════════════════════════════════
// Phase 9: Cleanup
// ══════════════════════════════════════════════════════

#[derive(Debug, serde::Serialize)]
pub struct OrphanedLocalEntry {
    pub local_file_id: i64,
    pub backup_entry_id: i64,
    pub local_path: String,
}

#[derive(Debug, serde::Serialize)]
pub struct OrphanedS3Object {
    pub key: String,
    pub size: i64,
}

#[derive(Debug, Default, serde::Serialize)]
pub struct CleanupSummary {
    pub deleted_count: usize,
    pub details: Vec<String>,
}

#[tauri::command]
pub fn scan_orphaned_local_entries(db: State<DbState>) -> Result<Vec<OrphanedLocalEntry>, AppError> {
    let conn = db.conn()?;
    let profile = get_active_profile_or_err(&conn)?;

    let mut stmt = conn.prepare(
        "SELECT lf.id, lf.backup_entry_id, lf.local_path
         FROM local_file lf JOIN backup_entry be ON lf.backup_entry_id = be.id
         WHERE be.profile_id = ?1",
    )?;

    let rows = stmt.query_map(rusqlite::params![profile.id], |row: &rusqlite::Row| {
        Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?, row.get::<_, String>(2)?))
    })?.collect::<Result<Vec<(i64, i64, String)>, _>>()?;

    let mut orphans = Vec::new();
    for (lf_id, be_id, local_path) in &rows {
        let full_path = backup::expand_relative_path(local_path, profile.relative_path.as_deref());
        if !full_path.exists() {
            orphans.push(OrphanedLocalEntry {
                local_file_id: *lf_id,
                backup_entry_id: *be_id,
                local_path: local_path.to_string(),
            });
        }
    }
    Ok(orphans)
}

#[tauri::command]
pub async fn cleanup_orphaned_local_entries(
    db: State<'_, DbState>,
    local_file_ids: Vec<i64>,
    delete_s3: bool,
    dry_run: bool,
) -> Result<CleanupSummary, AppError> {
    let mut summary = CleanupSummary::default();

    if dry_run {
        summary.deleted_count = local_file_ids.len();
        summary.details.push(format!("Would delete {} local_file entries", local_file_ids.len()));
        return Ok(summary);
    }

    let profile = {
        let conn = db.conn()?;
        get_active_profile_or_err(&conn)?
    };

    let s3 = if delete_s3 { Some(build_s3_client(&profile).await?) } else { None };

    // Collect info about what to delete, then do it
    for lf_id in &local_file_ids {
        let (be_id, should_delete_s3, object_uuid) = {
            let conn = db.conn()?;
            let be_id: Option<i64> = conn.prepare("SELECT backup_entry_id FROM local_file WHERE id = ?1")?
                .query_row(rusqlite::params![lf_id], |row: &rusqlite::Row| row.get(0))
                .optional()?;

            db::delete_local_file(&conn, *lf_id)?;
            summary.deleted_count += 1;

            if let (Some(be_id), true) = (be_id, delete_s3) {
                let remaining: i64 = conn.prepare("SELECT count(*) FROM local_file WHERE backup_entry_id = ?1")?
                    .query_row(rusqlite::params![be_id], |row: &rusqlite::Row| row.get(0))?;
                if remaining == 0 {
                    let entry = db::get_backup_entry_by_id(&conn, be_id)?;
                    let uuid = entry.map(|e| e.object_uuid.clone());
                    db::delete_backup_entry(&conn, be_id)?;
                    (Some(be_id), true, uuid)
                } else {
                    (Some(be_id), false, None)
                }
            } else {
                (be_id, false, None)
            }
        };
        let _ = be_id;

        if should_delete_s3 {
            if let (Some(ref s3), Some(ref uuid)) = (&s3, &object_uuid) {
                s3.delete_object(uuid).await.ok();
                summary.details.push(format!("Deleted S3 object {}", uuid));
            }
        }
    }

    Ok(summary)
}

#[tauri::command]
pub async fn scan_orphaned_s3_objects(db: State<'_, DbState>) -> Result<Vec<OrphanedS3Object>, AppError> {
    let (profile, known_uuids) = {
        let conn = db.conn()?;
        let profile = get_active_profile_or_err(&conn)?;
        let mut uuids = std::collections::HashSet::new();
        for e in db::list_backup_entries(&conn, profile.id)? {
            uuids.insert(e.object_uuid);
        }
        for m in db::list_share_manifests(&conn, profile.id)? {
            uuids.insert(m.manifest_uuid);
        }
        (profile, uuids)
    };

    let s3 = build_s3_client(&profile).await?;
    let objects = s3.list_objects().await?;
    Ok(objects.into_iter()
        .filter(|obj| !known_uuids.contains(&obj.key))
        .map(|obj| OrphanedS3Object { key: obj.key, size: obj.size })
        .collect())
}

#[tauri::command]
pub async fn cleanup_orphaned_s3_objects(
    db: State<'_, DbState>,
    object_keys: Vec<String>,
    dry_run: bool,
) -> Result<CleanupSummary, AppError> {
    let mut summary = CleanupSummary::default();
    if dry_run {
        summary.deleted_count = object_keys.len();
        summary.details.push(format!("Would delete {} S3 objects", object_keys.len()));
        return Ok(summary);
    }

    let profile = {
        let conn = db.conn()?;
        get_active_profile_or_err(&conn)?
    };
    let s3 = build_s3_client(&profile).await?;
    for key in &object_keys {
        match s3.delete_object(key).await {
            Ok(()) => { summary.deleted_count += 1; summary.details.push(format!("Deleted {}", key)); }
            Err(e) => { summary.details.push(format!("Failed {}: {}", key, e)); }
        }
    }
    Ok(summary)
}

// ══════════════════════════════════════════════════════
// Phase 10: Integrity Verification
// ══════════════════════════════════════════════════════

#[derive(Debug, serde::Serialize)]
pub struct VerifyResult {
    pub backup_entry_id: i64,
    pub filename: String,
    pub status: String,
    pub detail: Option<String>,
}

#[derive(Debug, Default, serde::Serialize)]
pub struct VerifySummary {
    pub passed: usize,
    pub failed: usize,
    pub errors: usize,
    pub results: Vec<VerifyResult>,
}

#[tauri::command]
pub async fn verify_integrity(
    app: tauri::AppHandle,
    db: State<'_, DbState>,
    backup_entry_ids: Vec<i64>,
) -> Result<VerifySummary, AppError> {
    let (profile, entries) = {
        let conn = db.conn()?;
        let profile = get_active_profile_or_err(&conn)?;
        let mut entries = Vec::new();
        for id in &backup_entry_ids {
            if let Some(e) = db::get_backup_entry_by_id(&conn, *id)? {
                let local_files = db::list_local_files(&conn, e.id)?;
                let filename = local_files.first()
                    .map(|lf| Path::new(&lf.local_path).file_name()
                        .map(|f| f.to_string_lossy().to_string())
                        .unwrap_or_else(|| lf.local_path.clone()))
                    .unwrap_or_else(|| e.object_uuid.clone());
                entries.push((e, filename));
            }
        }
        (profile, entries)
    };

    let encryption_key = get_profile_encryption_key(&profile)?;
    let temp_dir = get_temp_dir(&profile);
    let s3 = build_s3_client(&profile).await?;
    let mut summary = VerifySummary::default();

    for (entry, filename) in &entries {
        let _ = app.emit("verify:progress", VerifyProgressEvent {
            processed: summary.passed + summary.failed + summary.errors,
            total: entries.len(),
            current_file: filename.clone(),
            passed: summary.passed,
            failed: summary.failed,
            errors: summary.errors,
        });

        let temp_encrypted = crypto::generate_temp_path(&temp_dir);

        match s3.download_object(&entry.object_uuid, &temp_encrypted).await {
            Ok(()) => {
                match crypto::compute_file_md5(&temp_encrypted) {
                    Ok(md5) if md5 == entry.encrypted_md5 => {
                        let temp_decrypted = crypto::generate_temp_path(&temp_dir);
                        match crypto::decrypt_file(&temp_encrypted, &temp_decrypted, &encryption_key) {
                            Ok(()) => {
                                summary.passed += 1;
                                summary.results.push(VerifyResult { backup_entry_id: entry.id, filename: filename.clone(), status: "passed".into(), detail: None });
                            }
                            Err(e) => {
                                summary.errors += 1;
                                summary.results.push(VerifyResult { backup_entry_id: entry.id, filename: filename.clone(), status: "error".into(), detail: Some(format!("GCM auth failed: {}", e)) });
                            }
                        }
                        let _ = std::fs::remove_file(&temp_decrypted);
                    }
                    Ok(md5) => {
                        summary.failed += 1;
                        summary.results.push(VerifyResult { backup_entry_id: entry.id, filename: filename.clone(), status: "failed".into(), detail: Some(format!("MD5 mismatch: expected {}, got {}", entry.encrypted_md5, md5)) });
                    }
                    Err(e) => {
                        summary.errors += 1;
                        summary.results.push(VerifyResult { backup_entry_id: entry.id, filename: filename.clone(), status: "error".into(), detail: Some(format!("MD5 error: {}", e)) });
                    }
                }
            }
            Err(e) => {
                summary.errors += 1;
                summary.results.push(VerifyResult { backup_entry_id: entry.id, filename: filename.clone(), status: "error".into(), detail: Some(format!("Download failed: {}", e)) });
            }
        }
        let _ = std::fs::remove_file(&temp_encrypted);
    }

    Ok(summary)
}

// ══════════════════════════════════════════════════════
// Phase 11: Database Export/Import
// ══════════════════════════════════════════════════════

#[derive(serde::Serialize, serde::Deserialize)]
struct DatabaseExport {
    version: i32,
    profiles: Vec<db::Profile>,
    backup_entries: Vec<db::BackupEntry>,
    share_manifests: Vec<db::ShareManifest>,
    share_manifest_entries: Vec<db::ShareManifestEntry>,
    local_files: Vec<db::LocalFile>,
}

#[tauri::command]
pub fn export_database(db: State<DbState>, file_path: String) -> Result<(), AppError> {
    let conn = db.conn()?;
    let profiles = db::list_profiles(&conn)?;
    let mut all_entries = Vec::new();
    let mut all_manifests = Vec::new();
    let mut all_manifest_entries = Vec::new();
    let mut all_local_files = Vec::new();

    for profile in &profiles {
        let entries = db::list_backup_entries(&conn, profile.id)?;
        for entry in &entries {
            all_local_files.extend(db::list_local_files(&conn, entry.id)?);
        }
        all_entries.extend(entries);
        let manifests = db::list_share_manifests(&conn, profile.id)?;
        for manifest in &manifests {
            all_manifest_entries.extend(db::list_share_manifest_entries(&conn, manifest.id)?);
        }
        all_manifests.extend(manifests);
    }

    let export = DatabaseExport {
        version: 1, profiles, backup_entries: all_entries,
        share_manifests: all_manifests, share_manifest_entries: all_manifest_entries,
        local_files: all_local_files,
    };
    std::fs::write(&file_path, serde_json::to_string_pretty(&export)?)?;
    Ok(())
}

#[tauri::command]
pub fn import_database(db: State<DbState>, file_path: String) -> Result<(), AppError> {
    let json = std::fs::read_to_string(&file_path)?;
    let import: DatabaseExport = serde_json::from_str(&json)?;
    let conn = db.conn()?;

    conn.execute_batch(
        "DELETE FROM share_manifest_entry; DELETE FROM local_file;
         DELETE FROM share_manifest; DELETE FROM backup_entry; DELETE FROM profile;",
    )?;

    for p in &import.profiles {
        conn.execute(
            "INSERT INTO profile (id,name,mode,s3_endpoint,s3_region,s3_bucket,extra_env,relative_path,temp_directory,is_active,created_at) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)",
            rusqlite::params![p.id,p.name,p.mode,p.s3_endpoint,p.s3_region,p.s3_bucket,p.extra_env,p.relative_path,p.temp_directory,p.is_active,p.created_at],
        )?;
    }
    for e in &import.backup_entries {
        conn.execute(
            "INSERT INTO backup_entry (id,profile_id,object_uuid,original_md5,encrypted_md5,file_size,created_at) VALUES (?1,?2,?3,?4,?5,?6,?7)",
            rusqlite::params![e.id,e.profile_id,e.object_uuid,e.original_md5,e.encrypted_md5,e.file_size,e.created_at],
        )?;
    }
    for m in &import.share_manifests {
        conn.execute(
            "INSERT INTO share_manifest (id,profile_id,manifest_uuid,label,file_count,is_valid,created_at) VALUES (?1,?2,?3,?4,?5,?6,?7)",
            rusqlite::params![m.id,m.profile_id,m.manifest_uuid,m.label,m.file_count,m.is_valid,m.created_at],
        )?;
    }
    for e in &import.share_manifest_entries {
        conn.execute(
            "INSERT INTO share_manifest_entry (id,share_manifest_id,backup_entry_id,filename) VALUES (?1,?2,?3,?4)",
            rusqlite::params![e.id,e.share_manifest_id,e.backup_entry_id,e.filename],
        )?;
    }
    for f in &import.local_files {
        conn.execute(
            "INSERT INTO local_file (id,backup_entry_id,local_path,cached_mtime,cached_size,updated_at) VALUES (?1,?2,?3,?4,?5,?6)",
            rusqlite::params![f.id,f.backup_entry_id,f.local_path,f.cached_mtime,f.cached_size,f.updated_at],
        )?;
    }
    Ok(())
}

// ══════════════════════════════════════════════════════
// File browser
// ══════════════════════════════════════════════════════

#[derive(Debug, serde::Serialize)]
pub struct FileEntry {
    pub id: i64,
    pub object_uuid: String,
    pub filename: String,
    pub local_path: String,
    pub file_size: i64,
    pub original_md5: String,
    pub created_at: String,
}

#[tauri::command]
pub fn list_files(db: State<DbState>, search: Option<String>) -> Result<Vec<FileEntry>, AppError> {
    let conn = db.conn()?;
    let profile = get_active_profile_or_err(&conn)?;

    let query = if search.is_some() {
        "SELECT be.id, be.object_uuid, lf.local_path, be.file_size, be.original_md5, be.created_at
         FROM backup_entry be JOIN local_file lf ON lf.backup_entry_id = be.id
         WHERE be.profile_id = ?1 AND lf.local_path LIKE ?2 ORDER BY lf.local_path"
    } else {
        "SELECT be.id, be.object_uuid, lf.local_path, be.file_size, be.original_md5, be.created_at
         FROM backup_entry be JOIN local_file lf ON lf.backup_entry_id = be.id
         WHERE be.profile_id = ?1 ORDER BY lf.local_path"
    };

    let mut stmt = conn.prepare(query)?;
    let search_pattern = search.map(|s| format!("%{}%", s));

    let map_row = |row: &rusqlite::Row| -> rusqlite::Result<FileEntry> {
        let local_path: String = row.get(2)?;
        let filename = Path::new(&local_path).file_name()
            .map(|f| f.to_string_lossy().to_string())
            .unwrap_or_else(|| local_path.clone());
        Ok(FileEntry {
            id: row.get(0)?, object_uuid: row.get(1)?, filename, local_path,
            file_size: row.get(3)?, original_md5: row.get(4)?, created_at: row.get(5)?,
        })
    };

    let rows: Vec<FileEntry> = if let Some(ref pattern) = search_pattern {
        stmt.query_map(rusqlite::params![profile.id, pattern], map_row)?
            .collect::<Result<Vec<_>, _>>()?
    } else {
        stmt.query_map(rusqlite::params![profile.id], map_row)?
            .collect::<Result<Vec<_>, _>>()?
    };

    Ok(rows)
}

#[tauri::command]
pub async fn delete_backup_entries(
    db: State<'_, DbState>,
    backup_entry_ids: Vec<i64>,
) -> Result<usize, AppError> {
    let (profile, entries) = {
        let conn = db.conn()?;
        let profile = get_active_profile_or_err(&conn)?;
        let mut entries = Vec::new();
        for id in &backup_entry_ids {
            if let Some(e) = db::get_backup_entry_by_id(&conn, *id)? {
                entries.push(e);
            }
        }
        (profile, entries)
    };

    let s3 = build_s3_client(&profile).await?;
    let mut deleted = 0;

    for entry in &entries {
        s3.delete_object(&entry.object_uuid).await.ok();
        let conn = db.conn()?;
        conn.execute("DELETE FROM share_manifest_entry WHERE backup_entry_id = ?1", rusqlite::params![entry.id])?;
        conn.execute("DELETE FROM local_file WHERE backup_entry_id = ?1", rusqlite::params![entry.id])?;
        db::delete_backup_entry(&conn, entry.id)?;
        deleted += 1;
    }

    Ok(deleted)
}

// ══════════════════════════════════════════════════════
// Config
// ══════════════════════════════════════════════════════

/// Return the current app configuration.
#[tauri::command]
pub fn get_config() -> Result<crate::config::AppConfig, AppError> {
    crate::config::load_or_create_config()
}

/// Change the database file path saved in config.json.
/// If `copy_existing` is true and the current database file exists, it is
/// copied to the new location first. The change takes effect on next launch.
#[tauri::command]
pub fn set_database_path(new_path: String, copy_existing: bool) -> Result<(), AppError> {
    let current = crate::config::load_or_create_config()?;

    if copy_existing {
        let src = std::path::Path::new(&current.database_path);
        if src.exists() {
            let dest = std::path::Path::new(&new_path);
            if let Some(parent) = dest.parent() {
                if !parent.as_os_str().is_empty() {
                    std::fs::create_dir_all(parent)?;
                }
            }
            std::fs::copy(src, dest)?;
        }
    }

    let new_config = crate::config::AppConfig { database_path: new_path };
    crate::config::save_config(&new_config)?;
    Ok(())
}

// Throttle
// ══════════════════════════════════════════════════════

#[derive(serde::Serialize)]
pub struct ThrottleLimits {
    pub upload_bps: u64,
    pub download_bps: u64,
}

/// Update the global upload and/or download rate limits.
/// Pass 0 for unlimited.
#[tauri::command]
pub fn set_throttle_limits(
    upload_bps: u64,
    download_bps: u64,
    throttle: State<crate::throttle::ThrottleState>,
) -> Result<(), AppError> {
    throttle.set_upload_bps(upload_bps);
    throttle.set_download_bps(download_bps);
    Ok(())
}

/// Get the current upload and download rate limits.
#[tauri::command]
pub fn get_throttle_limits(
    throttle: State<crate::throttle::ThrottleState>,
) -> Result<ThrottleLimits, AppError> {
    Ok(ThrottleLimits {
        upload_bps: throttle.get_upload_bps(),
        download_bps: throttle.get_download_bps(),
    })
}
