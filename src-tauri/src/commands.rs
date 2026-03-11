use std::path::{Path, PathBuf};


use tauri::State;

use crate::backup;
use crate::credentials;
use crate::crypto;
use crate::db::{self, DbState};
use crate::error::AppError;
use crate::profiles;
use crate::queue::{OperationQueue, OpParams, QueueSnapshot};
use crate::s3::S3Client;

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
    let part_size = profile
        .upload_chunk_size_mb
        .filter(|&mb| mb > 0)
        .map(|mb| mb as usize * 1024 * 1024)
        .unwrap_or(crate::crypto::DEFAULT_CHUNK_SIZE);
    S3Client::new(
        &profile.s3_endpoint,
        profile.s3_region.as_deref(),
        &profile.s3_bucket,
        &access_key,
        &secret_key,
        profile.extra_env.as_deref(),
        crate::throttle::global().clone(),
        part_size,
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
    let count: i64 = stmt.query_row([], |row: &rusqlite::Row| row.get(0))?;
    let count = count as usize;
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
    pub s3_key_prefix: Option<String>,
    pub upload_chunk_size_mb: Option<i64>,
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
        input.s3_key_prefix.as_deref(),
        input.upload_chunk_size_mb,
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
    pub s3_key_prefix: Option<Option<String>>,
    pub upload_chunk_size_mb: Option<Option<i64>>,
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
        input.s3_key_prefix.as_ref().map(|o| o.as_deref()),
        input.upload_chunk_size_mb,
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
        crate::crypto::DEFAULT_CHUNK_SIZE,
    )
    .await?;
    s3.head_bucket().await?;
    Ok("Connection successful".into())
}

// ══════════════════════════════════════════════════════
// Queue management
// ══════════════════════════════════════════════════════

/// Cancel a queued or active operation.
#[tauri::command]
pub fn cancel_operation(
    queue: State<'_, OperationQueue>,
    op_id: String,
) -> Result<(), AppError> {
    queue.cancel(&op_id);
    Ok(())
}

/// Return the current queue snapshot (pending + active).
#[tauri::command]
pub fn get_queue(queue: State<'_, OperationQueue>) -> Result<QueueSnapshot, AppError> {
    Ok(queue.snapshot())
}

// ══════════════════════════════════════════════════════
// Phase 5: Backup
// ══════════════════════════════════════════════════════

#[tauri::command]
pub fn backup_file(
    queue: State<'_, OperationQueue>,
    file_path: String,
) -> Result<String, AppError> {
    let name = Path::new(&file_path)
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| file_path.clone());
    let id = queue.enqueue(
        format!("Backing up {}", name),
        "backup",
        OpParams::BackupFile { file_path },
    );
    Ok(id)
}

/// Enqueue a directory backup. Returns the op ID immediately.
/// Returns an error if the same directory is already queued or active.
#[tauri::command]
pub fn backup_directory(
    queue: State<'_, OperationQueue>,
    dir_path: String,
    skip_patterns: Vec<String>,
    force_checksum: bool,
) -> Result<String, AppError> {
    let dirname = Path::new(&dir_path)
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| dir_path.clone());

    if !queue.try_register_backup_dir(&dir_path) {
        return Err(AppError::Config(format!(
            "'{}' is already being backed up",
            dirname
        )));
    }

    let id = queue.enqueue(
        format!("Backing up {}", dirname),
        "backup",
        OpParams::BackupDirectory { dir_path: dir_path.clone(), skip_patterns, force_checksum },
    );
    queue.bind_backup_dir_op(&id, &dir_path);
    Ok(id)
}

// ══════════════════════════════════════════════════════
// Phase 6: Restore
// ══════════════════════════════════════════════════════

/// Enqueue a restore operation. Returns the op ID immediately.
#[tauri::command]
pub fn restore_files(
    queue: State<'_, OperationQueue>,
    backup_entry_ids: Vec<i64>,
    target_directory: Option<String>,
) -> Result<String, AppError> {
    let count = backup_entry_ids.len();
    let label = if count == 1 {
        "Restoring 1 file".into()
    } else {
        format!("Restoring {} files", count)
    };
    let id = queue.enqueue(label, "restore", OpParams::RestoreFiles { backup_entry_ids, target_directory });
    Ok(id)
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

    let manifest_uuid = backup::make_s3_key(profile.s3_key_prefix.as_deref(), &uuid::Uuid::new_v4().to_string());
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

/// Enqueue a manifest download. Returns the op ID immediately.
#[tauri::command]
pub fn download_from_manifest(
    queue: State<'_, OperationQueue>,
    manifest_uuid: String,
    selected_uuids: Vec<String>,
    save_directory: String,
) -> Result<String, AppError> {
    let count = selected_uuids.len();
    let label = if count == 0 {
        "Downloading shared files".into()
    } else if count == 1 {
        "Downloading 1 file".into()
    } else {
        format!("Downloading {} files", count)
    };
    let id = queue.enqueue(
        label,
        "download",
        OpParams::DownloadManifest { manifest_uuid, selected_uuids, save_directory },
    );
    Ok(id)
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

/// Enqueue a scramble operation. Returns the op ID immediately.
#[tauri::command]
pub fn scramble(
    queue: State<'_, OperationQueue>,
    backup_entry_ids: Vec<i64>,
    scramble_all: bool,
) -> Result<String, AppError> {
    let label = if scramble_all {
        "Scrambling all files".into()
    } else {
        let count = backup_entry_ids.len();
        if count == 1 { "Scrambling 1 file".into() } else { format!("Scrambling {} files", count) }
    };
    let id = queue.enqueue(label, "scramble", OpParams::Scramble { backup_entry_ids, scramble_all });
    Ok(id)
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

/// Enqueue a local orphan cleanup. Returns the op ID immediately.
#[tauri::command]
pub fn cleanup_orphaned_local_entries(
    queue: State<'_, OperationQueue>,
    local_file_ids: Vec<i64>,
    delete_s3: bool,
    dry_run: bool,
) -> Result<String, AppError> {
    let count = local_file_ids.len();
    let label = if dry_run {
        format!("Previewing cleanup of {} local entries", count)
    } else {
        format!("Cleaning up {} local entries", count)
    };
    let id = queue.enqueue(
        label,
        "cleanup",
        OpParams::CleanupOrphanedLocal { local_file_ids, delete_s3, dry_run },
    );
    Ok(id)
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
    let objects = s3.list_objects(profile.s3_key_prefix.as_deref()).await?;
    Ok(objects.into_iter()
        .filter(|obj| !known_uuids.contains(&obj.key))
        .map(|obj| OrphanedS3Object { key: obj.key, size: obj.size })
        .collect())
}

/// Enqueue an S3 orphan cleanup. Returns the op ID immediately.
#[tauri::command]
pub fn cleanup_orphaned_s3_objects(
    queue: State<'_, OperationQueue>,
    object_keys: Vec<String>,
    dry_run: bool,
) -> Result<String, AppError> {
    let count = object_keys.len();
    let label = if dry_run {
        format!("Previewing cleanup of {} S3 objects", count)
    } else {
        format!("Cleaning up {} S3 objects", count)
    };
    let id = queue.enqueue(
        label,
        "cleanup",
        OpParams::CleanupOrphanedS3 { object_keys, dry_run },
    );
    Ok(id)
}

// ══════════════════════════════════════════════════════
// Phase 10: Integrity Verification
// ══════════════════════════════════════════════════════

/// Enqueue an integrity verification. Returns the op ID immediately.
/// Results are delivered via the `verify:complete` event.
#[tauri::command]
pub fn verify_integrity(
    queue: State<'_, OperationQueue>,
    backup_entry_ids: Vec<i64>,
) -> Result<String, AppError> {
    let count = backup_entry_ids.len();
    let label = if count == 1 {
        "Verifying 1 file".into()
    } else {
        format!("Verifying {} files", count)
    };
    let id = queue.enqueue(label, "verify", OpParams::VerifyIntegrity { backup_entry_ids });
    Ok(id)
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
            "INSERT INTO profile (id,name,mode,s3_endpoint,s3_region,s3_bucket,extra_env,relative_path,temp_directory,is_active,created_at,s3_key_prefix) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12)",
            rusqlite::params![p.id,p.name,p.mode,p.s3_endpoint,p.s3_region,p.s3_bucket,p.extra_env,p.relative_path,p.temp_directory,p.is_active,p.created_at,p.s3_key_prefix],
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
// Phase 12: Profile Config Export / Import
// ══════════════════════════════════════════════════════

/// Serializable snapshot of a profile's connection config, excluding the encryption key.
/// Written to disk during export; read back during import.
#[derive(serde::Serialize, serde::Deserialize)]
pub struct ProfileConfigExport {
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
    pub s3_key_prefix: Option<String>,
    pub upload_chunk_size_mb: Option<i64>,
}

/// Export a profile's connection config (including S3 credentials) to a JSON file.
/// The encryption key is intentionally omitted; the recipient must supply it at import time.
#[tauri::command]
pub fn export_profile_config(
    db: State<DbState>,
    profile_id: i64,
    file_path: String,
) -> Result<(), AppError> {
    let conn = db.conn()?;
    let profile = db::get_profile_by_id(&conn, profile_id)?
        .ok_or_else(|| AppError::NotFound(format!("Profile {} not found", profile_id)))?;
    let s3_access_key = credentials::get_s3_access_key(&profile.name)?;
    let s3_secret_key = credentials::get_s3_secret_key(&profile.name)?;
    let export = ProfileConfigExport {
        name: profile.name,
        mode: profile.mode,
        s3_endpoint: profile.s3_endpoint,
        s3_region: profile.s3_region,
        s3_bucket: profile.s3_bucket,
        s3_access_key,
        s3_secret_key,
        extra_env: profile.extra_env,
        relative_path: profile.relative_path,
        temp_directory: profile.temp_directory,
        s3_key_prefix: profile.s3_key_prefix,
        upload_chunk_size_mb: profile.upload_chunk_size_mb,
    };
    let json = serde_json::to_string_pretty(&export)?;
    std::fs::write(&file_path, json)?;
    Ok(())
}

/// Import a profile from a previously exported JSON file.
/// `mode_override` lets the importer choose "read-only" or "read-write" regardless of what the
/// exporter had. If `None`, the mode stored in the file is used.
/// The encryption key must be supplied separately — it is never stored in the export file.
#[tauri::command]
pub fn import_profile_config(
    db: State<DbState>,
    file_path: String,
    encryption_key: String,
    mode_override: Option<String>,
) -> Result<profiles::CreateProfileResult, AppError> {
    let data = std::fs::read_to_string(&file_path)?;
    let cfg: ProfileConfigExport = serde_json::from_str(&data)
        .map_err(|e| AppError::InvalidData(format!("Invalid profile config file: {}", e)))?;
    let mode = mode_override.as_deref().unwrap_or(&cfg.mode);
    let conn = db.conn()?;
    profiles::create_profile(
        &conn,
        &cfg.name,
        mode,
        &cfg.s3_endpoint,
        cfg.s3_region.as_deref(),
        &cfg.s3_bucket,
        &cfg.s3_access_key,
        &cfg.s3_secret_key,
        cfg.extra_env.as_deref(),
        cfg.relative_path.as_deref(),
        cfg.temp_directory.as_deref(),
        Some(&encryption_key),
        cfg.s3_key_prefix.as_deref(),
        cfg.upload_chunk_size_mb,
    )
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
