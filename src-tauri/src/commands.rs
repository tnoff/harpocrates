use std::path::Path;

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

fn get_active_profile_or_err(conn: &rusqlite::Connection) -> Result<db::Profile, AppError> {
    profiles::get_active_profile(conn)?
        .ok_or_else(|| AppError::Config("No active profile set".into()))
}

fn get_profile_encryption_key(profile: &db::Profile) -> Result<String, AppError> {
    credentials::get_encryption_key(&profile.name)
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

/// Build the S3 key for a share manifest (uses `m/` namespace).
fn make_manifest_s3_key(prefix: Option<&str>, uuid: &str) -> String {
    match prefix {
        Some(p) if !p.is_empty() => format!("{}/m/{}", p, uuid),
        _ => format!("m/{}", uuid),
    }
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
    Ok(count as usize)
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
    pub chunk_size_bytes: Option<i64>,
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
        input.chunk_size_bytes,
    )
}

#[derive(serde::Serialize)]
pub struct ProfileCredentials {
    pub s3_access_key: String,
    pub s3_secret_key: String,
}

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
    pub chunk_size_bytes: Option<i64>,
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
        input.chunk_size_bytes,
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
// Queue management
// ══════════════════════════════════════════════════════

#[tauri::command]
pub fn cancel_operation(
    queue: State<'_, OperationQueue>,
    op_id: String,
) -> Result<(), AppError> {
    queue.cancel(&op_id);
    Ok(())
}

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

/// Enqueue a restore of the given file_entry IDs (called backup_entry_ids for API compat).
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
    let id = queue.enqueue(
        label,
        "restore",
        OpParams::RestoreFiles { backup_entry_ids, target_directory },
    );
    Ok(id)
}

// ══════════════════════════════════════════════════════
// Phase 7: Share Manifests
// ══════════════════════════════════════════════════════

/// Describes one file inside a share manifest.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub struct ManifestFileEntry {
    pub uuid: String,    // original_md5 — used as stable identifier for download selection
    pub filename: String,
    pub size: i64,
}

#[derive(Debug, serde::Serialize)]
pub struct ManifestFileList {
    pub manifest_uuid: String,
    pub files: Vec<ManifestFileEntry>,
}

/// Internal manifest JSON format stored in the encrypted S3 object.
#[derive(serde::Serialize, serde::Deserialize)]
struct ManifestJson {
    version: i32,
    files: Vec<ManifestJsonFile>,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct ManifestJsonFile {
    filename: String,
    original_md5: String,
    total_size: i64,
    /// Ordered list of (chunk_index, s3_key) pairs for restoring this file.
    chunks: Vec<String>,
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
        let mut entries: Vec<(db::FileEntry, String, Vec<String>)> = Vec::new();
        for id in &backup_entry_ids {
            if let Some(entry) = db::get_file_entry_by_id(&conn, *id)? {
                // Get filename from local_file
                let local_files = db::list_local_files_for_entry(&conn, entry.id)?;
                let filename = local_files
                    .first()
                    .map(|lf| {
                        Path::new(&lf.local_path)
                            .file_name()
                            .map(|f| f.to_string_lossy().to_string())
                            .unwrap_or_else(|| lf.local_path.clone())
                    })
                    .unwrap_or_else(|| entry.original_md5.clone());
                // Get ordered chunk S3 keys
                let chunk_keys: Vec<String> = db::get_chunk_keys_for_file(&conn, entry.id)?
                    .into_iter()
                    .map(|(_, s3_key)| s3_key)
                    .collect();
                entries.push((entry, filename, chunk_keys));
            }
        }
        (profile, entries)
    };

    let key_hex = get_profile_encryption_key(&profile)?;
    let key_bytes = crypto::decode_encryption_key(&key_hex)?;
    let s3 = build_s3_client(&profile).await?;

    let manifest_json = ManifestJson {
        version: 2,
        files: entries_with_filenames
            .iter()
            .map(|(e, filename, chunks)| ManifestJsonFile {
                filename: filename.clone(),
                original_md5: e.original_md5.clone(),
                total_size: e.total_size,
                chunks: chunks.clone(),
            })
            .collect(),
    };

    let manifest_bytes = serde_json::to_vec(&manifest_json)?;
    let encrypted = crypto::encrypt_chunk(&key_bytes, &manifest_bytes)?;

    let manifest_uuid =
        make_manifest_s3_key(profile.s3_key_prefix.as_deref(), &uuid::Uuid::new_v4().to_string());
    s3.upload_chunk(&manifest_uuid, encrypted).await?;

    // Insert DB records
    {
        let conn = db.conn()?;
        let manifest_id = db::insert_share_manifest(
            &conn,
            profile.id,
            &manifest_uuid,
            label.as_deref(),
            entries_with_filenames.len() as i64,
        )?;
        for (entry, filename, _) in &entries_with_filenames {
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

    let key_hex = get_profile_encryption_key(&profile)?;
    let key_bytes = crypto::decode_encryption_key(&key_hex)?;
    let s3 = build_s3_client(&profile).await?;

    let encrypted = s3.download_chunk(&manifest_uuid).await?;
    let manifest_bytes = crypto::decrypt_chunk(&key_bytes, &encrypted)?;

    let manifest: ManifestJson = serde_json::from_slice(&manifest_bytes)
        .map_err(|e| AppError::InvalidData(format!("Invalid manifest format: {}", e)))?;

    let files: Vec<ManifestFileEntry> = manifest
        .files
        .into_iter()
        .map(|f| ManifestFileEntry {
            uuid: f.original_md5,
            filename: f.filename,
            size: f.total_size,
        })
        .collect();

    Ok(ManifestFileList { manifest_uuid, files })
}

/// Enqueue a manifest download. `selected_uuids` are `original_md5` values of the files to download.
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
pub fn get_share_manifest_files(
    db: State<DbState>,
    manifest_id: i64,
) -> Result<Vec<ManifestFileEntry>, AppError> {
    let conn = db.conn()?;
    let entries = db::list_share_manifest_entries(&conn, manifest_id)?;
    let mut files = Vec::new();
    for entry in entries {
        if let Some(fe) = db::get_file_entry_by_id(&conn, entry.file_entry_id)? {
            files.push(ManifestFileEntry {
                uuid: fe.original_md5,
                filename: entry.filename,
                size: fe.total_size,
            });
        }
    }
    Ok(files)
}

#[tauri::command]
pub async fn revoke_share_manifest(
    db: State<'_, DbState>,
    manifest_id: i64,
) -> Result<(), AppError> {
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
        if count == 1 {
            "Scrambling 1 file".into()
        } else {
            format!("Scrambling {} files", count)
        }
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
    pub file_entry_id: i64,
    pub local_path: String,
}

#[derive(Debug, serde::Serialize)]
pub struct OrphanedS3Object {
    pub key: String,
    pub size: i64,
}

#[tauri::command]
pub fn scan_orphaned_local_entries(
    db: State<DbState>,
) -> Result<Vec<OrphanedLocalEntry>, AppError> {
    let conn = db.conn()?;
    let profile = get_active_profile_or_err(&conn)?;

    let mut stmt = conn.prepare(
        "SELECT lf.id, lf.file_entry_id, lf.local_path
         FROM local_file lf
         JOIN file_entry fe ON lf.file_entry_id = fe.id
         WHERE fe.profile_id = ?1",
    )?;

    let rows = stmt
        .query_map(rusqlite::params![profile.id], |row: &rusqlite::Row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?, row.get::<_, String>(2)?))
        })?
        .collect::<Result<Vec<(i64, i64, String)>, _>>()?;

    let mut orphans = Vec::new();
    for (lf_id, fe_id, local_path) in &rows {
        let full_path = backup::expand_relative_path(local_path, profile.relative_path.as_deref());
        if !full_path.exists() {
            orphans.push(OrphanedLocalEntry {
                local_file_id: *lf_id,
                file_entry_id: *fe_id,
                local_path: local_path.to_string(),
            });
        }
    }
    Ok(orphans)
}

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
pub async fn scan_orphaned_s3_objects(
    db: State<'_, DbState>,
) -> Result<Vec<OrphanedS3Object>, AppError> {
    let (profile, known_keys) = {
        let conn = db.conn()?;
        let profile = get_active_profile_or_err(&conn)?;
        let mut keys = std::collections::HashSet::new();
        for chunk in db::list_chunks(&conn, profile.id)? {
            keys.insert(chunk.s3_key);
        }
        for manifest in db::list_share_manifests(&conn, profile.id)? {
            keys.insert(manifest.manifest_uuid);
        }
        (profile, keys)
    };

    let s3 = build_s3_client(&profile).await?;
    let objects = s3.list_objects(profile.s3_key_prefix.as_deref()).await?;
    Ok(objects
        .into_iter()
        .filter(|obj| !known_keys.contains(&obj.key))
        .map(|obj| OrphanedS3Object { key: obj.key, size: obj.size })
        .collect())
}

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
    file_entries: Vec<db::FileEntry>,
    chunks: Vec<db::Chunk>,
    local_files: Vec<db::LocalFile>,
    share_manifests: Vec<db::ShareManifest>,
    share_manifest_entries: Vec<db::ShareManifestEntry>,
}

#[tauri::command]
pub fn export_database(db: State<DbState>, file_path: String) -> Result<(), AppError> {
    let conn = db.conn()?;
    let profiles = db::list_profiles(&conn)?;
    let mut file_entries = Vec::new();
    let mut chunks = Vec::new();
    let mut local_files = Vec::new();
    let mut share_manifests = Vec::new();
    let mut share_manifest_entries = Vec::new();

    for profile in &profiles {
        file_entries.extend(db::list_file_entries(&conn, profile.id)?);
        chunks.extend(db::list_chunks(&conn, profile.id)?);
        let manifests = db::list_share_manifests(&conn, profile.id)?;
        for manifest in &manifests {
            share_manifest_entries.extend(db::list_share_manifest_entries(&conn, manifest.id)?);
        }
        share_manifests.extend(manifests);
    }
    // local_file is globally unique, query all
    let mut stmt = conn.prepare(
        "SELECT id, file_entry_id, local_path, cached_mtime, cached_size, updated_at FROM local_file",
    )?;
    local_files.extend(
        stmt.query_map([], |row| {
            Ok(db::LocalFile {
                id: row.get(0)?,
                file_entry_id: row.get(1)?,
                local_path: row.get(2)?,
                cached_mtime: row.get(3)?,
                cached_size: row.get(4)?,
                updated_at: row.get(5)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?,
    );

    let export = DatabaseExport {
        version: 5,
        profiles,
        file_entries,
        chunks,
        local_files,
        share_manifests,
        share_manifest_entries,
    };
    std::fs::write(&file_path, serde_json::to_string_pretty(&export)?)?;
    Ok(())
}

#[tauri::command]
pub fn import_database(db: State<DbState>, file_path: String) -> Result<(), AppError> {
    let json = std::fs::read_to_string(&file_path)?;
    let import: DatabaseExport = serde_json::from_str(&json)
        .map_err(|e| AppError::InvalidData(format!("Invalid database export: {}", e)))?;
    let conn = db.conn()?;

    conn.execute_batch(
        "DELETE FROM share_manifest_entry;
         DELETE FROM local_file;
         DELETE FROM file_chunk;
         DELETE FROM share_manifest;
         DELETE FROM chunk;
         DELETE FROM file_entry;
         DELETE FROM profile;",
    )?;

    for p in &import.profiles {
        conn.execute(
            "INSERT INTO profile (id,name,mode,s3_endpoint,s3_region,s3_bucket,extra_env,relative_path,temp_directory,is_active,created_at,s3_key_prefix,chunk_size_bytes)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13)",
            rusqlite::params![p.id,p.name,p.mode,p.s3_endpoint,p.s3_region,p.s3_bucket,p.extra_env,p.relative_path,p.temp_directory,p.is_active,p.created_at,p.s3_key_prefix,p.chunk_size_bytes],
        )?;
    }
    for e in &import.file_entries {
        conn.execute(
            "INSERT INTO file_entry (id,profile_id,original_md5,total_size,chunk_count,created_at) VALUES (?1,?2,?3,?4,?5,?6)",
            rusqlite::params![e.id,e.profile_id,e.original_md5,e.total_size,e.chunk_count,e.created_at],
        )?;
    }
    for c in &import.chunks {
        conn.execute(
            "INSERT INTO chunk (id,profile_id,chunk_hash,s3_key,encrypted_size,created_at) VALUES (?1,?2,?3,?4,?5,?6)",
            rusqlite::params![c.id,c.profile_id,c.chunk_hash,c.s3_key,c.encrypted_size,c.created_at],
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
            "INSERT INTO share_manifest_entry (id,share_manifest_id,file_entry_id,filename) VALUES (?1,?2,?3,?4)",
            rusqlite::params![e.id,e.share_manifest_id,e.file_entry_id,e.filename],
        )?;
    }
    for f in &import.local_files {
        conn.execute(
            "INSERT INTO local_file (id,file_entry_id,local_path,cached_mtime,cached_size,updated_at) VALUES (?1,?2,?3,?4,?5,?6)",
            rusqlite::params![f.id,f.file_entry_id,f.local_path,f.cached_mtime,f.cached_size,f.updated_at],
        )?;
    }
    Ok(())
}

// ══════════════════════════════════════════════════════
// Phase 12: Profile Config Export / Import
// ══════════════════════════════════════════════════════

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
    pub chunk_size_bytes: i64,
}

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
        chunk_size_bytes: profile.chunk_size_bytes,
    };
    std::fs::write(&file_path, serde_json::to_string_pretty(&export)?)?;
    Ok(())
}

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
        Some(cfg.chunk_size_bytes),
    )
}

// ══════════════════════════════════════════════════════
// File browser
// ══════════════════════════════════════════════════════

/// Frontend-facing file entry. `object_uuid` is set to `original_md5` for compatibility.
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
        "SELECT fe.id, fe.original_md5, lf.local_path, fe.total_size, fe.original_md5, fe.created_at
         FROM file_entry fe JOIN local_file lf ON lf.file_entry_id = fe.id
         WHERE fe.profile_id = ?1 AND lf.local_path LIKE ?2 ORDER BY lf.local_path"
    } else {
        "SELECT fe.id, fe.original_md5, lf.local_path, fe.total_size, fe.original_md5, fe.created_at
         FROM file_entry fe JOIN local_file lf ON lf.file_entry_id = fe.id
         WHERE fe.profile_id = ?1 ORDER BY lf.local_path"
    };

    let mut stmt = conn.prepare(query)?;
    let search_pattern = search.map(|s| format!("%{}%", s));

    let map_row = |row: &rusqlite::Row| -> rusqlite::Result<FileEntry> {
        let local_path: String = row.get(2)?;
        let filename = Path::new(&local_path)
            .file_name()
            .map(|f| f.to_string_lossy().to_string())
            .unwrap_or_else(|| local_path.clone());
        Ok(FileEntry {
            id: row.get(0)?,
            object_uuid: row.get(1)?, // original_md5 used as uuid
            filename,
            local_path,
            file_size: row.get(3)?,
            original_md5: row.get(4)?,
            created_at: row.get(5)?,
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

/// Delete file_entries (and associated chunks that become orphaned). Frontend calls this
/// `delete_backup_entries` for API compatibility.
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
            if let Some(e) = db::get_file_entry_by_id(&conn, *id)? {
                entries.push(e);
            }
        }
        (profile, entries)
    };

    let s3 = build_s3_client(&profile).await?;
    let mut deleted = 0;

    for entry in &entries {
        // Collect chunk IDs referenced by this file_entry before deleting file_chunk rows
        let chunk_ids: Vec<i64> = {
            let conn = db.conn()?;
            db::get_chunk_ids_for_file(&conn, entry.id)?
        };

        // Remove share_manifest_entry rows that reference this file_entry
        {
            let conn = db.conn()?;
            conn.execute(
                "DELETE FROM share_manifest_entry WHERE file_entry_id = ?1",
                rusqlite::params![entry.id],
            )?;
            conn.execute(
                "DELETE FROM file_chunk WHERE file_entry_id = ?1",
                rusqlite::params![entry.id],
            )?;
            conn.execute(
                "DELETE FROM local_file WHERE file_entry_id = ?1",
                rusqlite::params![entry.id],
            )?;
            db::delete_file_entry(&conn, entry.id)?;
        }

        // Delete chunks that are now orphaned (not referenced by any remaining file_entry)
        for chunk_id in &chunk_ids {
            let ref_count = {
                let conn = db.conn()?;
                db::count_file_entries_for_chunk(&conn, *chunk_id)?
            };
            if ref_count == 0 {
                let chunk = {
                    let conn = db.conn()?;
                    db::get_chunk_by_id(&conn, *chunk_id)?
                };
                if let Some(c) = chunk {
                    s3.delete_object(&c.s3_key).await.ok();
                    let conn = db.conn()?;
                    db::delete_chunk(&conn, *chunk_id)?;
                }
            }
        }

        deleted += 1;
    }

    Ok(deleted)
}

// ══════════════════════════════════════════════════════
// Config
// ══════════════════════════════════════════════════════

#[tauri::command]
pub fn get_config() -> Result<crate::config::AppConfig, AppError> {
    crate::config::load_or_create_config()
}

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

// ══════════════════════════════════════════════════════
// Throttle
// ══════════════════════════════════════════════════════

#[derive(serde::Serialize)]
pub struct ThrottleLimits {
    pub upload_bps: u64,
    pub download_bps: u64,
}

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

#[tauri::command]
pub fn get_throttle_limits(
    throttle: State<crate::throttle::ThrottleState>,
) -> Result<ThrottleLimits, AppError> {
    Ok(ThrottleLimits {
        upload_bps: throttle.get_upload_bps(),
        download_bps: throttle.get_download_bps(),
    })
}
