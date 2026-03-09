use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Instant;

use regex::Regex;
use rusqlite::Connection;

use crate::crypto;
use crate::db;
use crate::error::AppError;
use crate::s3::S3Client;

const BATCH_SIZE: usize = 50;
const BATCH_INTERVAL_SECS: u64 = 30;

/// Construct an S3 object key from an optional prefix and a UUID.
/// If prefix is Some and non-empty, returns `"{prefix}/{uuid}"`.
/// Otherwise returns just `uuid`.
pub fn make_s3_key(prefix: Option<&str>, uuid: &str) -> String {
    match prefix {
        Some(p) if !p.is_empty() => format!("{}/{}", p, uuid),
        _ => uuid.to_string(),
    }
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct BackupResult {
    pub backup_entry_id: i64,
    pub object_uuid: String,
    pub original_md5: String,
    pub encrypted_md5: String,
    pub file_size: u64,
    pub was_dedup: bool,
}

#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct BackupSummary {
    pub total_files: usize,
    pub processed: usize,
    pub skipped: usize,
    pub uploaded: usize,
    pub deduped: usize,
    pub failed: usize,
    pub failures: Vec<BackupFailure>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct BackupFailure {
    pub path: String,
    pub error: String,
}

/// Pending database write for batching
struct PendingWrite {
    profile_id: i64,
    object_uuid: String,
    original_md5: String,
    encrypted_md5: String,
    file_size: i64,
    local_path: String,
    cached_mtime: Option<f64>,
    cached_size: Option<i64>,
    is_new_entry: bool, // true = new backup_entry + local_file, false = dedup (local_file only, linked to existing)
    existing_entry_id: Option<i64>, // for dedup case
}

/// Flush pending writes to the database in a transaction
fn flush_pending_writes(conn: &Connection, writes: &[PendingWrite]) -> Result<(), AppError> {
    if writes.is_empty() {
        return Ok(());
    }
    let tx = conn.unchecked_transaction()?;
    for w in writes {
        if w.is_new_entry {
            let entry_id = db::insert_backup_entry(
                &tx,
                w.profile_id,
                &w.object_uuid,
                &w.original_md5,
                &w.encrypted_md5,
                w.file_size,
            )?;
            db::insert_local_file(&tx, entry_id, &w.local_path, w.cached_mtime, w.cached_size)?;
        } else if let Some(entry_id) = w.existing_entry_id {
            // Dedup: just link local_file to existing backup_entry
            db::insert_local_file(&tx, entry_id, &w.local_path, w.cached_mtime, w.cached_size)?;
        }
    }
    tx.commit()?;
    Ok(())
}

/// Strip the relative_path prefix from a full path to get a relative path for storage
pub fn strip_relative_path(full_path: &str, relative_path: Option<&str>) -> String {
    match relative_path {
        Some(prefix) => {
            let prefix = prefix.trim_end_matches('/');
            if let Some(stripped) = full_path.strip_prefix(prefix) {
                stripped.trim_start_matches('/').to_string()
            } else {
                full_path.to_string()
            }
        }
        None => full_path.to_string(),
    }
}

/// Expand a stored relative path back to a full path using relative_path prefix
pub fn expand_relative_path(stored_path: &str, relative_path: Option<&str>) -> PathBuf {
    match relative_path {
        Some(prefix) => {
            let prefix = prefix.trim_end_matches('/');
            PathBuf::from(format!("{}/{}", prefix, stored_path))
        }
        None => PathBuf::from(stored_path),
    }
}

#[allow(dead_code, clippy::too_many_arguments)]
pub async fn backup_single_file(
    conn: &Connection,
    s3: &S3Client,
    profile_id: i64,
    file_path: &Path,
    encryption_key: &str,
    s3_key_prefix: Option<&str>,
    relative_path: Option<&str>,
    temp_dir: &Path,
) -> Result<BackupResult, AppError> {
    let full_path_str = file_path.to_string_lossy().to_string();
    let stored_path = strip_relative_path(&full_path_str, relative_path);

    // Compute MD5 of original file
    let original_md5 = crypto::compute_file_md5(file_path)?;

    // Check for dedup: existing backup_entry with same original_md5 in this profile
    let existing = find_backup_by_md5(conn, profile_id, &original_md5)?;

    if let Some(entry) = existing {
        // Dedup: just create local_file link
        let metadata = std::fs::metadata(file_path)?;
        let mtime = metadata
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs_f64());
        let size = metadata.len() as i64;

        db::insert_local_file(conn, entry.id, &stored_path, mtime, Some(size))?;

        return Ok(BackupResult {
            backup_entry_id: entry.id,
            object_uuid: entry.object_uuid,
            original_md5: entry.original_md5,
            encrypted_md5: entry.encrypted_md5,
            file_size: entry.file_size as u64,
            was_dedup: true,
        });
    }

    // Encrypt file to temp location
    let temp_path = crypto::generate_temp_path(temp_dir);
    let encrypt_result = crypto::encrypt_file(file_path, &temp_path, encryption_key)?;

    // Generate UUID for S3 object
    let object_uuid = make_s3_key(s3_key_prefix, &uuid::Uuid::new_v4().to_string());

    // Upload to S3 (multipart is handled automatically for large files)
    let upload_result = s3.upload_object(&object_uuid, &temp_path).await;

    // Clean up temp file
    let _ = std::fs::remove_file(&temp_path);

    upload_result?;

    // Insert backup_entry and local_file
    let entry_id = db::insert_backup_entry(
        conn,
        profile_id,
        &object_uuid,
        &encrypt_result.original_md5,
        &encrypt_result.encrypted_md5,
        encrypt_result.file_size as i64,
    )?;

    let metadata = std::fs::metadata(file_path)?;
    let mtime = metadata
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs_f64());
    let size = metadata.len() as i64;

    db::insert_local_file(conn, entry_id, &stored_path, mtime, Some(size))?;

    Ok(BackupResult {
        backup_entry_id: entry_id,
        object_uuid,
        original_md5: encrypt_result.original_md5,
        encrypted_md5: encrypt_result.encrypted_md5,
        file_size: encrypt_result.file_size,
        was_dedup: false,
    })
}

/// Find an existing backup_entry by MD5 for deduplication
fn find_backup_by_md5(
    conn: &Connection,
    profile_id: i64,
    md5: &str,
) -> Result<Option<db::BackupEntry>, AppError> {
    let mut stmt = conn.prepare(
        "SELECT id, profile_id, object_uuid, original_md5, encrypted_md5, file_size, created_at
         FROM backup_entry WHERE profile_id = ?1 AND original_md5 = ?2 LIMIT 1",
    )?;
    let entry = stmt
        .query_row(rusqlite::params![profile_id, md5], |row| {
            Ok(db::BackupEntry {
                id: row.get(0)?,
                profile_id: row.get(1)?,
                object_uuid: row.get(2)?,
                original_md5: row.get(3)?,
                encrypted_md5: row.get(4)?,
                file_size: row.get(5)?,
                created_at: row.get(6)?,
            })
        })
        .optional()?;
    Ok(entry)
}

use rusqlite::OptionalExtension;

/// Scan a directory and return all file paths (skipping symlinks and matching skip patterns)
pub fn scan_directory(
    dir: &Path,
    skip_patterns: &[Regex],
) -> Result<Vec<PathBuf>, AppError> {
    let mut files = Vec::new();
    scan_directory_recursive(dir, skip_patterns, &mut files)?;
    Ok(files)
}

fn scan_directory_recursive(
    dir: &Path,
    skip_patterns: &[Regex],
    files: &mut Vec<PathBuf>,
) -> Result<(), AppError> {
    let entries = std::fs::read_dir(dir)?;
    for entry in entries {
        let entry = entry?;
        let path = entry.path();
        let metadata = match entry.metadata() {
            Ok(m) => m,
            Err(_) => continue,
        };

        // Skip symlinks
        if metadata.file_type().is_symlink() {
            continue;
        }

        if metadata.is_dir() {
            scan_directory_recursive(&path, skip_patterns, files)?;
        } else if metadata.is_file() {
            let path_str = path.to_string_lossy();
            let should_skip = skip_patterns.iter().any(|p| p.is_match(&path_str));
            if !should_skip {
                files.push(path);
            }
        }
    }
    Ok(())
}

/// Check if a file has changed based on cached mtime and size
fn file_has_changed(
    conn: &Connection,
    profile_id: i64,
    stored_path: &str,
    current_mtime: Option<f64>,
    current_size: Option<i64>,
) -> Result<(bool, Option<i64>), AppError> {
    // Find local_file entry by path, joined with backup_entry for profile scope
    let mut stmt = conn.prepare(
        "SELECT lf.cached_mtime, lf.cached_size, lf.backup_entry_id
         FROM local_file lf
         JOIN backup_entry be ON lf.backup_entry_id = be.id
         WHERE be.profile_id = ?1 AND lf.local_path = ?2
         LIMIT 1",
    )?;

    let result = stmt
        .query_row(rusqlite::params![profile_id, stored_path], |row| {
            let cached_mtime: Option<f64> = row.get(0)?;
            let cached_size: Option<i64> = row.get(1)?;
            let entry_id: i64 = row.get(2)?;
            Ok((cached_mtime, cached_size, entry_id))
        })
        .optional()?;

    match result {
        Some((cached_mtime, cached_size, entry_id)) => {
            let mtime_match = match (cached_mtime, current_mtime) {
                (Some(c), Some(n)) => (c - n).abs() < 0.001,
                _ => false,
            };
            let size_match = cached_size == current_size;

            if mtime_match && size_match {
                Ok((false, Some(entry_id))) // unchanged
            } else {
                Ok((true, Some(entry_id))) // changed
            }
        }
        None => Ok((true, None)), // new file
    }
}

/// Backup a directory with change detection, dedup, and batching.
///
/// `on_progress` is called at the start of each file with the current summary
/// snapshot and the file path string, allowing callers to emit progress events.
#[allow(clippy::too_many_arguments)]
pub async fn backup_directory(
    db: &crate::db::DbState,
    s3: &S3Client,
    profile_id: i64,
    dir_path: &Path,
    encryption_key: &str,
    s3_key_prefix: Option<&str>,
    relative_path: Option<&str>,
    temp_dir: &Path,
    skip_patterns: &[Regex],
    force_checksum: bool,
    cancel_flag: Arc<AtomicBool>,
    on_progress: impl Fn(&BackupSummary, &str) + Send,
) -> Result<BackupSummary, AppError> {
    let files = scan_directory(dir_path, skip_patterns)?;
    let total_files = files.len();

    let mut summary = BackupSummary {
        total_files,
        ..Default::default()
    };
    let mut pending_writes: Vec<PendingWrite> = Vec::new();
    let mut last_flush = Instant::now();

    for file_path in &files {
        // Check cancellation
        if cancel_flag.load(Ordering::Relaxed) {
            let conn = db.conn()?;
            flush_pending_writes(&conn, &pending_writes)?;
            pending_writes.clear();
            break;
        }

        let full_path_str = file_path.to_string_lossy().to_string();
        let stored_path = strip_relative_path(&full_path_str, relative_path);

        // Emit progress before processing this file
        on_progress(&summary, &full_path_str);

        // Get file metadata
        let metadata = match std::fs::metadata(file_path) {
            Ok(m) => m,
            Err(e) => {
                summary.failed += 1;
                summary.failures.push(BackupFailure {
                    path: full_path_str,
                    error: e.to_string(),
                });
                summary.processed += 1;
                continue;
            }
        };

        let current_mtime = metadata
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs_f64());
        let current_size = Some(metadata.len() as i64);

        // Change detection (fast path)
        if !force_checksum {
            let change_result = {
                let conn = db.conn()?;
                file_has_changed(&conn, profile_id, &stored_path, current_mtime, current_size)
            };
            match change_result {
                Ok((false, Some(_entry_id))) => {
                    summary.skipped += 1;
                    summary.processed += 1;
                    continue;
                }
                Ok(_) => {}
                Err(e) => {
                    summary.failed += 1;
                    summary.failures.push(BackupFailure {
                        path: full_path_str,
                        error: e.to_string(),
                    });
                    summary.processed += 1;
                    continue;
                }
            }
        }

        // Compute MD5
        let original_md5 = match crypto::compute_file_md5(file_path) {
            Ok(md5) => md5,
            Err(e) => {
                summary.failed += 1;
                summary.failures.push(BackupFailure {
                    path: full_path_str,
                    error: e.to_string(),
                });
                summary.processed += 1;
                continue;
            }
        };

        // Check dedup (short DB lock)
        let dedup_result = {
            let conn = db.conn()?;
            find_backup_by_md5(&conn, profile_id, &original_md5)
        };

        match dedup_result {
            Ok(Some(existing)) => {
                pending_writes.push(PendingWrite {
                    profile_id,
                    object_uuid: existing.object_uuid,
                    original_md5: existing.original_md5,
                    encrypted_md5: existing.encrypted_md5,
                    file_size: existing.file_size,
                    local_path: stored_path,
                    cached_mtime: current_mtime,
                    cached_size: current_size,
                    is_new_entry: false,
                    existing_entry_id: Some(existing.id),
                });
                summary.deduped += 1;
                summary.processed += 1;
            }
            Ok(None) => {
                let temp_path = crypto::generate_temp_path(temp_dir);
                match crypto::encrypt_file(file_path, &temp_path, encryption_key) {
                    Ok(encrypt_meta) => {
                        let object_uuid = make_s3_key(s3_key_prefix, &uuid::Uuid::new_v4().to_string());
                        let upload_result = s3.upload_object(&object_uuid, &temp_path).await;
                        let _ = std::fs::remove_file(&temp_path);

                        match upload_result {
                            Ok(()) => {
                                pending_writes.push(PendingWrite {
                                    profile_id,
                                    object_uuid,
                                    original_md5: encrypt_meta.original_md5,
                                    encrypted_md5: encrypt_meta.encrypted_md5,
                                    file_size: encrypt_meta.file_size as i64,
                                    local_path: stored_path,
                                    cached_mtime: current_mtime,
                                    cached_size: current_size,
                                    is_new_entry: true,
                                    existing_entry_id: None,
                                });
                                summary.uploaded += 1;
                                summary.processed += 1;
                            }
                            Err(e) => {
                                summary.failed += 1;
                                summary.failures.push(BackupFailure {
                                    path: full_path_str,
                                    error: e.to_string(),
                                });
                                summary.processed += 1;
                            }
                        }
                    }
                    Err(e) => {
                        let _ = std::fs::remove_file(&temp_path);
                        summary.failed += 1;
                        summary.failures.push(BackupFailure {
                            path: full_path_str,
                            error: e.to_string(),
                        });
                        summary.processed += 1;
                    }
                }
            }
            Err(e) => {
                summary.failed += 1;
                summary.failures.push(BackupFailure {
                    path: full_path_str,
                    error: e.to_string(),
                });
                summary.processed += 1;
            }
        }

        // Batch flush (short DB lock)
        if pending_writes.len() >= BATCH_SIZE
            || last_flush.elapsed().as_secs() >= BATCH_INTERVAL_SECS
        {
            let conn = db.conn()?;
            flush_pending_writes(&conn, &pending_writes)?;
            pending_writes.clear();
            last_flush = Instant::now();
        }
    }

    // Final flush
    {
        let conn = db.conn()?;
        flush_pending_writes(&conn, &pending_writes)?;
    }

    Ok(summary)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    // ── strip_relative_path tests ──

    #[test]
    fn test_strip_relative_path_with_prefix() {
        let result = strip_relative_path("/home/user/documents/file.txt", Some("/home/user"));
        assert_eq!(result, "documents/file.txt");
    }

    #[test]
    fn test_strip_relative_path_with_trailing_slash() {
        let result = strip_relative_path("/home/user/documents/file.txt", Some("/home/user/"));
        assert_eq!(result, "documents/file.txt");
    }

    #[test]
    fn test_strip_relative_path_no_match() {
        let result = strip_relative_path("/other/path/file.txt", Some("/home/user"));
        assert_eq!(result, "/other/path/file.txt");
    }

    #[test]
    fn test_strip_relative_path_none_prefix() {
        let result = strip_relative_path("/home/user/file.txt", None);
        assert_eq!(result, "/home/user/file.txt");
    }

    #[test]
    fn test_strip_relative_path_exact_match() {
        let result = strip_relative_path("/home/user", Some("/home/user"));
        assert_eq!(result, "");
    }

    // ── expand_relative_path tests ──

    #[test]
    fn test_expand_relative_path_with_prefix() {
        let result = expand_relative_path("documents/file.txt", Some("/home/user"));
        assert_eq!(result, PathBuf::from("/home/user/documents/file.txt"));
    }

    #[test]
    fn test_expand_relative_path_with_trailing_slash() {
        let result = expand_relative_path("file.txt", Some("/home/user/"));
        assert_eq!(result, PathBuf::from("/home/user/file.txt"));
    }

    #[test]
    fn test_expand_relative_path_none_prefix() {
        let result = expand_relative_path("/absolute/path/file.txt", None);
        assert_eq!(result, PathBuf::from("/absolute/path/file.txt"));
    }

    // ── scan_directory tests ──

    #[test]
    fn test_scan_directory_empty() {
        let dir = tempfile::tempdir().unwrap();
        let files = scan_directory(dir.path(), &[]).unwrap();
        assert!(files.is_empty());
    }

    #[test]
    fn test_scan_directory_finds_files() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("a.txt"), "hello").unwrap();
        fs::write(dir.path().join("b.txt"), "world").unwrap();
        let files = scan_directory(dir.path(), &[]).unwrap();
        assert_eq!(files.len(), 2);
    }

    #[test]
    fn test_scan_directory_recursive() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("sub/deep")).unwrap();
        fs::write(dir.path().join("top.txt"), "1").unwrap();
        fs::write(dir.path().join("sub/mid.txt"), "2").unwrap();
        fs::write(dir.path().join("sub/deep/bottom.txt"), "3").unwrap();
        let files = scan_directory(dir.path(), &[]).unwrap();
        assert_eq!(files.len(), 3);
    }

    #[test]
    fn test_scan_directory_with_skip_pattern() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("keep.txt"), "keep").unwrap();
        fs::write(dir.path().join("skip.log"), "skip").unwrap();
        fs::write(dir.path().join("also_keep.txt"), "keep").unwrap();

        let patterns = vec![Regex::new(r"\.log$").unwrap()];
        let files = scan_directory(dir.path(), &patterns).unwrap();
        assert_eq!(files.len(), 2);
        assert!(files.iter().all(|f| !f.to_string_lossy().ends_with(".log")));
    }

    #[test]
    fn test_scan_directory_nonexistent() {
        let result = scan_directory(Path::new("/nonexistent/dir/xyz"), &[]);
        assert!(result.is_err());
    }

    // ── flush_pending_writes tests ──

    fn setup_db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
        conn.execute_batch(
            "
            CREATE TABLE IF NOT EXISTS profile (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                name TEXT UNIQUE NOT NULL,
                mode TEXT NOT NULL DEFAULT 'read-write',
                s3_endpoint TEXT NOT NULL,
                s3_region TEXT,
                s3_bucket TEXT NOT NULL,
                extra_env TEXT,
                relative_path TEXT,
                temp_directory TEXT,
                s3_key_prefix TEXT,
                is_active BOOLEAN NOT NULL DEFAULT 0,
                created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
                UNIQUE(s3_endpoint, s3_bucket)
            );
            CREATE TABLE IF NOT EXISTS backup_entry (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                profile_id INTEGER NOT NULL REFERENCES profile(id),
                object_uuid TEXT NOT NULL,
                original_md5 TEXT NOT NULL,
                encrypted_md5 TEXT NOT NULL,
                file_size INTEGER NOT NULL,
                created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
                UNIQUE(profile_id, object_uuid)
            );
            CREATE TABLE IF NOT EXISTS local_file (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                backup_entry_id INTEGER NOT NULL REFERENCES backup_entry(id),
                local_path TEXT NOT NULL,
                cached_mtime REAL,
                cached_size INTEGER,
                updated_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
                UNIQUE(backup_entry_id, local_path)
            );
            ",
        )
        .unwrap();
        conn
    }

    #[test]
    fn test_flush_pending_writes_empty() {
        let conn = setup_db();
        let result = flush_pending_writes(&conn, &[]);
        assert!(result.is_ok());
    }

    #[test]
    fn test_flush_pending_writes_new_entry() {
        let conn = setup_db();
        let pid = db::insert_profile(
            &conn, "test", "read-write", "https://s3.test.com", None, "bucket", None, None, None, None,
        ).unwrap();

        let writes = vec![PendingWrite {
            profile_id: pid,
            object_uuid: "uuid-flush-1".into(),
            original_md5: "md5orig".into(),
            encrypted_md5: "md5enc".into(),
            file_size: 512,
            local_path: "docs/readme.txt".into(),
            cached_mtime: Some(1000.0),
            cached_size: Some(512),
            is_new_entry: true,
            existing_entry_id: None,
        }];

        flush_pending_writes(&conn, &writes).unwrap();

        let entries = db::list_backup_entries(&conn, pid).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].object_uuid, "uuid-flush-1");

        let files = db::list_local_files(&conn, entries[0].id).unwrap();
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].local_path, "docs/readme.txt");
    }

    #[test]
    fn test_flush_pending_writes_dedup() {
        let conn = setup_db();
        let pid = db::insert_profile(
            &conn, "test", "read-write", "https://s3.test.com", None, "bucket", None, None, None, None,
        ).unwrap();
        let eid = db::insert_backup_entry(&conn, pid, "existing-uuid", "md5a", "md5b", 256).unwrap();

        let writes = vec![PendingWrite {
            profile_id: pid,
            object_uuid: "existing-uuid".into(),
            original_md5: "md5a".into(),
            encrypted_md5: "md5b".into(),
            file_size: 256,
            local_path: "another/path.txt".into(),
            cached_mtime: Some(2000.0),
            cached_size: Some(256),
            is_new_entry: false,
            existing_entry_id: Some(eid),
        }];

        flush_pending_writes(&conn, &writes).unwrap();

        // Should have linked local_file to existing backup_entry
        let files = db::list_local_files(&conn, eid).unwrap();
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].local_path, "another/path.txt");

        // No new backup_entry should be created
        let entries = db::list_backup_entries(&conn, pid).unwrap();
        assert_eq!(entries.len(), 1);
    }

    // ── BackupSummary/BackupResult struct tests ──

    #[test]
    fn test_backup_summary_default() {
        let summary = BackupSummary::default();
        assert_eq!(summary.total_files, 0);
        assert_eq!(summary.processed, 0);
        assert_eq!(summary.skipped, 0);
        assert_eq!(summary.uploaded, 0);
        assert_eq!(summary.deduped, 0);
        assert_eq!(summary.failed, 0);
        assert!(summary.failures.is_empty());
    }

    // ── find_backup_by_md5 tests ──

    #[test]
    fn test_find_backup_by_md5_found() {
        let conn = setup_db();
        let pid = db::insert_profile(
            &conn, "p", "read-write", "https://a.com", None, "b", None, None, None, None,
        ).unwrap();
        db::insert_backup_entry(&conn, pid, "uuid-1", "target-md5", "enc-md5", 100).unwrap();

        let result = find_backup_by_md5(&conn, pid, "target-md5").unwrap();
        assert!(result.is_some());
        assert_eq!(result.unwrap().original_md5, "target-md5");
    }

    #[test]
    fn test_find_backup_by_md5_not_found() {
        let conn = setup_db();
        let pid = db::insert_profile(
            &conn, "p", "read-write", "https://a.com", None, "b", None, None, None, None,
        ).unwrap();

        let result = find_backup_by_md5(&conn, pid, "nonexistent-md5").unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn test_find_backup_by_md5_profile_scoped() {
        let conn = setup_db();
        let pid1 = db::insert_profile(
            &conn, "p1", "read-write", "https://a.com", None, "b1", None, None, None, None,
        ).unwrap();
        let pid2 = db::insert_profile(
            &conn, "p2", "read-write", "https://b.com", None, "b2", None, None, None, None,
        ).unwrap();

        db::insert_backup_entry(&conn, pid1, "uuid-1", "shared-md5", "enc-md5", 100).unwrap();

        // Same MD5 but different profile should not match
        let result = find_backup_by_md5(&conn, pid2, "shared-md5").unwrap();
        assert!(result.is_none());
    }

    // ── make_s3_key tests ──

    #[test]
    fn test_make_s3_key_no_prefix() {
        assert_eq!(make_s3_key(None, "my-uuid"), "my-uuid");
    }

    #[test]
    fn test_make_s3_key_empty_prefix() {
        assert_eq!(make_s3_key(Some(""), "my-uuid"), "my-uuid");
    }

    #[test]
    fn test_make_s3_key_with_prefix() {
        assert_eq!(make_s3_key(Some("team-alpha"), "my-uuid"), "team-alpha/my-uuid");
    }

    #[test]
    fn test_make_s3_key_nested_prefix() {
        assert_eq!(make_s3_key(Some("org/team"), "my-uuid"), "org/team/my-uuid");
    }
}
