use std::io::{BufReader, Read};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use md5::{Digest, Md5};
use regex::Regex;
use tokio::sync::Semaphore;
use tokio::task::JoinSet;

use crate::crypto;
use crate::db;
use crate::error::AppError;
use crate::s3::S3Client;

/// Maximum concurrent S3 PutObject requests per backup operation.
const UPLOAD_CONCURRENCY: usize = 4;

/// Build the S3 key for a content-addressed chunk.
pub fn make_chunk_s3_key(prefix: Option<&str>, chunk_hash: &str) -> String {
    match prefix {
        Some(p) if !p.is_empty() => format!("{}/c/{}", p, chunk_hash),
        _ => format!("c/{}", chunk_hash),
    }
}

/// Strip the relative_path prefix from a full path to get a path for storage.
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

/// Expand a stored path back to a full path using relative_path prefix.
pub fn expand_relative_path(stored_path: &str, relative_path: Option<&str>) -> PathBuf {
    match relative_path {
        Some(prefix) => {
            let prefix = prefix.trim_end_matches('/');
            PathBuf::from(format!("{}/{}", prefix, stored_path))
        }
        None => PathBuf::from(stored_path),
    }
}

#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct BackupSummary {
    pub total_files: usize,
    pub processed: usize,
    pub skipped: usize,
    pub uploaded: usize,
    pub deduped: usize,
    pub failed: usize,
    pub chunks_uploaded: usize,
    pub chunks_deduped: usize,
    pub failures: Vec<BackupFailure>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct BackupFailure {
    pub path: String,
    pub error: String,
}

/// Scan a directory recursively, returning all file paths (skipping symlinks and skip patterns).
pub fn scan_directory(dir: &Path, skip_patterns: &[Regex]) -> Result<Vec<PathBuf>, AppError> {
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
        if metadata.file_type().is_symlink() {
            continue;
        }
        if metadata.is_dir() {
            scan_directory_recursive(&path, skip_patterns, files)?;
        } else if metadata.is_file() {
            let path_str = path.to_string_lossy();
            if !skip_patterns.iter().any(|p| p.is_match(&path_str)) {
                files.push(path);
            }
        }
    }
    Ok(())
}

// ── Chunk pipeline ─────────────────────────────────────────────────────────────

/// Outcome of backing up one file.
pub enum FileOutcome {
    /// mtime + size cache hit — no work done.
    Skipped,
    /// file_entry already exists for this MD5 — only `local_file` was upserted.
    Deduped,
    /// New file_entry created; carries per-file chunk counts.
    Uploaded { chunks_uploaded: usize, chunks_deduped: usize },
}

/// A chunk that was freshly uploaded to S3 and needs a DB record inserted.
struct UploadedChunk {
    chunk_index: usize,
    chunk_hash: String,
    s3_key: String,
    encrypted_size: i64,
}

/// Process a single file through the chunk pipeline.
///
/// Reads the file in fixed-size chunks, computing a rolling MD5 and a per-chunk
/// HMAC.  Chunks not already in the DB are encrypted and uploaded concurrently
/// (bounded to `UPLOAD_CONCURRENCY` in-flight PutObject requests).  All DB
/// writes happen in a single transaction *after* every upload succeeds, so the
/// DB is always consistent with S3.
#[allow(clippy::too_many_arguments)]
pub async fn backup_file(
    db_state: &crate::db::DbState,
    s3: &S3Client,
    profile_id: i64,
    file_path: &Path,
    stored_path: &str,
    key_bytes: &[u8; 32],
    s3_key_prefix: Option<&str>,
    chunk_size: usize,
) -> Result<FileOutcome, AppError> {
    // ── Change detection (fast path) ──────────────────────────────────────────
    let metadata = std::fs::metadata(file_path)?;
    let current_mtime = metadata
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs_f64());
    let current_size = metadata.len() as i64;

    {
        let conn = db_state.conn()?;
        if let Some(lf) = db::get_local_file_by_path(&conn, stored_path)? {
            let mtime_ok = match (lf.cached_mtime, current_mtime) {
                (Some(c), Some(n)) => (c - n).abs() < 0.001,
                _ => false,
            };
            if mtime_ok && lf.cached_size == Some(current_size) {
                return Ok(FileOutcome::Skipped);
            }
        }
    }

    // ── Read file in chunks ────────────────────────────────────────────────────
    // Use BufReader + take(chunk_size) to always get deterministic chunk boundaries:
    // every chunk is exactly chunk_size bytes except possibly the last one.
    let file = std::fs::File::open(file_path)?;
    let mut reader = BufReader::new(file);

    let mut rolling_md5 = Md5::new();
    let semaphore = Arc::new(Semaphore::new(UPLOAD_CONCURRENCY));
    let mut join_set: JoinSet<Result<UploadedChunk, AppError>> = JoinSet::new();
    let mut existing_chunks: Vec<(usize, i64)> = Vec::new(); // (chunk_index, chunk_id)
    let mut chunk_index: usize = 0;
    let mut total_size: i64 = 0;

    loop {
        let mut chunk_buf = Vec::with_capacity(chunk_size);
        let n = reader.by_ref().take(chunk_size as u64).read_to_end(&mut chunk_buf)?;
        if n == 0 {
            break;
        }
        rolling_md5.update(&chunk_buf);
        total_size += n as i64;

        let chunk_hash = crypto::compute_chunk_hmac(key_bytes, &chunk_buf);

        let existing_id = {
            let conn = db_state.conn()?;
            db::get_chunk_id_by_hash(&conn, profile_id, &chunk_hash)?
        };

        if let Some(id) = existing_id {
            existing_chunks.push((chunk_index, id));
        } else {
            // Acquiring the permit here provides backpressure: the read loop
            // stalls when UPLOAD_CONCURRENCY uploads are already in-flight.
            let permit = semaphore
                .clone()
                .acquire_owned()
                .await
                .map_err(|_| AppError::InvalidData("Upload semaphore was closed".into()))?;

            let encrypted = crypto::encrypt_chunk(key_bytes, &chunk_buf)?;
            let enc_size = encrypted.len() as i64;
            let s3_key = make_chunk_s3_key(s3_key_prefix, &chunk_hash);
            let s3_clone = s3.clone();
            let s3_key_clone = s3_key.clone();
            let hash_clone = chunk_hash.clone();
            let idx = chunk_index;

            join_set.spawn(async move {
                let _permit = permit; // released when this task completes
                s3_clone.upload_chunk(&s3_key_clone, encrypted).await?;
                Ok(UploadedChunk {
                    chunk_index: idx,
                    chunk_hash: hash_clone,
                    s3_key: s3_key_clone,
                    encrypted_size: enc_size,
                })
            });
        }

        chunk_index += 1;
    }

    let total_chunks = chunk_index;

    // ── Collect upload results ─────────────────────────────────────────────────
    let mut uploaded_chunks: Vec<UploadedChunk> = Vec::new();
    while let Some(result) = join_set.join_next().await {
        match result {
            Ok(Ok(uc)) => uploaded_chunks.push(uc),
            Ok(Err(e)) => {
                join_set.abort_all();
                return Err(e);
            }
            Err(join_err) => {
                join_set.abort_all();
                return Err(AppError::InvalidData(format!(
                    "Upload task panicked: {}",
                    join_err
                )));
            }
        }
    }

    let original_md5 = hex::encode(rolling_md5.finalize());
    let chunks_uploaded = uploaded_chunks.len();
    let chunks_deduped = existing_chunks.len();

    // ── DB writes in one transaction ───────────────────────────────────────────
    {
        let conn = db_state.conn()?;
        let tx = conn.unchecked_transaction()?;

        // Whole-file dedup: if file_entry with this MD5 already exists, only link local path.
        if let Some(entry) = db::get_file_entry_by_md5(&tx, profile_id, &original_md5)? {
            db::upsert_local_file(&tx, entry.id, stored_path, current_mtime, Some(current_size))?;
            tx.commit()?;
            return Ok(FileOutcome::Deduped);
        }

        // Insert chunk records for newly uploaded chunks
        let mut all_chunks: Vec<(usize, i64)> = existing_chunks;

        for uc in &uploaded_chunks {
            let chunk_id =
                db::insert_chunk(&tx, profile_id, &uc.chunk_hash, &uc.s3_key, uc.encrypted_size)?;
            all_chunks.push((uc.chunk_index, chunk_id));
        }

        // Insert file_entry
        let file_entry_id =
            db::insert_file_entry(&tx, profile_id, &original_md5, total_size, total_chunks as i64)?;

        // Insert file_chunk rows in index order
        all_chunks.sort_by_key(|(idx, _)| *idx);
        for (idx, chunk_id) in &all_chunks {
            db::insert_file_chunk(&tx, file_entry_id, *idx as i64, *chunk_id)?;
        }

        db::upsert_local_file(&tx, file_entry_id, stored_path, current_mtime, Some(current_size))?;
        tx.commit()?;
    }

    Ok(FileOutcome::Uploaded { chunks_uploaded, chunks_deduped })
}

// ── Directory backup ──────────────────────────────────────────────────────────

/// Backup a directory using content-addressed chunk storage.
///
/// - `key_hex`: 64-char hex-encoded 32-byte encryption key
/// - `chunk_size`: bytes per chunk (from profile `chunk_size_bytes`)
/// - `on_progress(summary, file_path)`: called before processing each file
#[allow(clippy::too_many_arguments)]
pub async fn backup_directory(
    db: &crate::db::DbState,
    s3: &S3Client,
    profile_id: i64,
    dir_path: &Path,
    key_hex: &str,
    s3_key_prefix: Option<&str>,
    relative_path: Option<&str>,
    chunk_size: usize,
    skip_patterns: &[Regex],
    force_checksum: bool,
    cancel_flag: Arc<AtomicBool>,
    on_progress: impl Fn(&BackupSummary, &str) + Send,
) -> Result<BackupSummary, AppError> {
    let key_bytes = crypto::decode_encryption_key(key_hex)?;
    let files = scan_directory(dir_path, skip_patterns)?;
    let total_files = files.len();

    let mut summary = BackupSummary {
        total_files,
        ..Default::default()
    };

    for file_path in &files {
        if cancel_flag.load(Ordering::Relaxed) {
            break;
        }

        let full_path_str = file_path.to_string_lossy().to_string();
        let stored_path = strip_relative_path(&full_path_str, relative_path);

        on_progress(&summary, &full_path_str);

        // force_checksum bypasses mtime/size cache by clearing the stored entry
        if force_checksum {
            let conn = db.conn()?;
            if let Some(lf) = db::get_local_file_by_path(&conn, &stored_path)? {
                db::delete_local_file(&conn, lf.id)?;
            }
        }

        match backup_file(
            db,
            s3,
            profile_id,
            file_path,
            &stored_path,
            &key_bytes,
            s3_key_prefix,
            chunk_size,
        )
        .await
        {
            Ok(FileOutcome::Skipped) => {
                summary.skipped += 1;
            }
            Ok(FileOutcome::Deduped) => {
                summary.deduped += 1;
            }
            Ok(FileOutcome::Uploaded { chunks_uploaded, chunks_deduped }) => {
                summary.uploaded += 1;
                summary.chunks_uploaded += chunks_uploaded;
                summary.chunks_deduped += chunks_deduped;
            }
            Err(e) => {
                summary.failed += 1;
                summary.failures.push(BackupFailure {
                    path: full_path_str,
                    error: e.to_string(),
                });
            }
        }
        summary.processed += 1;
    }

    Ok(summary)
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    // ── strip_relative_path ────────────────────────────────────────────────────

    #[test]
    fn test_strip_relative_path_with_prefix() {
        let result = strip_relative_path("/home/user/documents/file.txt", Some("/home/user"));
        assert_eq!(result, "documents/file.txt");
    }

    #[test]
    fn test_strip_relative_path_with_trailing_slash() {
        let result =
            strip_relative_path("/home/user/documents/file.txt", Some("/home/user/"));
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

    // ── expand_relative_path ───────────────────────────────────────────────────

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

    // ── make_chunk_s3_key ──────────────────────────────────────────────────────

    #[test]
    fn test_make_chunk_s3_key_no_prefix() {
        assert_eq!(make_chunk_s3_key(None, "abc123"), "c/abc123");
    }

    #[test]
    fn test_make_chunk_s3_key_empty_prefix() {
        assert_eq!(make_chunk_s3_key(Some(""), "abc123"), "c/abc123");
    }

    #[test]
    fn test_make_chunk_s3_key_with_prefix() {
        assert_eq!(
            make_chunk_s3_key(Some("team-alpha"), "abc123"),
            "team-alpha/c/abc123"
        );
    }

    #[test]
    fn test_make_chunk_s3_key_nested_prefix() {
        assert_eq!(
            make_chunk_s3_key(Some("org/team"), "abc123"),
            "org/team/c/abc123"
        );
    }

    // ── scan_directory ─────────────────────────────────────────────────────────

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

    // ── BackupSummary ──────────────────────────────────────────────────────────

    #[test]
    fn test_backup_summary_default() {
        let s = BackupSummary::default();
        assert_eq!(s.total_files, 0);
        assert_eq!(s.processed, 0);
        assert_eq!(s.skipped, 0);
        assert_eq!(s.uploaded, 0);
        assert_eq!(s.deduped, 0);
        assert_eq!(s.failed, 0);
        assert_eq!(s.chunks_uploaded, 0);
        assert_eq!(s.chunks_deduped, 0);
        assert!(s.failures.is_empty());
    }
}
