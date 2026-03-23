//! Serial, in-memory FIFO queue for all S3-touching operations.
//!
//! One worker task runs operations one at a time. Each op carries its own
//! cancellation flag. The queue emits `queue:updated`, `op:complete`, and
//! `op:failed` events to the frontend, plus per-op progress events
//! (`backup:progress`, `restore:progress`, `scramble:progress`, `verify:progress`).

use std::collections::{HashMap, HashSet, VecDeque};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use md5::{Digest, Md5};
use tauri::{AppHandle, Emitter, Manager};
use tokio::sync::mpsc;

use crate::backup;
use crate::credentials;
use crate::crypto;
use crate::db::{self, DbState};
use crate::error::AppError;
use crate::profiles;
use crate::s3::S3Client;

// ── Public IPC types ──────────────────────────────────────────────────────────

#[derive(Debug, Clone, serde::Serialize)]
pub struct QueueEntry {
    pub id: String,
    pub label: String,
    pub op_type: String,
}

#[derive(Clone, serde::Serialize)]
pub struct QueueSnapshot {
    pub pending: Vec<QueueEntry>,
    pub active: Option<QueueEntry>,
}

// ── Completion events ─────────────────────────────────────────────────────────

#[derive(Clone, serde::Serialize)]
pub struct OpCompleteEvent {
    pub id: String,
    pub message: String,
}

#[derive(Clone, serde::Serialize)]
pub struct OpFailedEvent {
    pub id: String,
    pub error: String,
}

// ── Progress events ───────────────────────────────────────────────────────────

#[derive(serde::Serialize, Clone)]
struct BackupProgressEvent {
    op_id: String,
    processed: usize,
    total: usize,
    uploaded: usize,
    deduped: usize,
    skipped: usize,
    failed: usize,
    current_file: String,
}

#[derive(serde::Serialize, Clone)]
struct RestoreProgressEvent {
    op_id: String,
    processed: usize,
    total: usize,
    current_file: String,
    restored: usize,
    skipped: usize,
    failed: usize,
}

#[derive(serde::Serialize, Clone)]
struct ScrambleProgressEvent {
    op_id: String,
    processed: usize,
    total: usize,
    current_file: String,
    scrambled: usize,
    failed: usize,
}

#[derive(serde::Serialize, Clone)]
struct VerifyProgressEvent {
    op_id: String,
    processed: usize,
    total: usize,
    current_file: String,
    passed: usize,
    failed: usize,
    errors: usize,
}

#[derive(serde::Serialize, Clone)]
struct CleanupProgressEvent {
    op_id: String,
    processed: usize,
    total: usize,
    current_item: String,
    deleted: usize,
    failed: usize,
}

/// Emitted repeatedly during an upload or restore to show byte-level progress.
/// `phase` is one of "Uploading", "Downloading".
#[derive(serde::Serialize, Clone)]
struct UploadProgressEvent {
    op_id: String,
    bytes_done: u64,
    bytes_total: u64,
    phase: String,
    phase_done: u64,
    phase_total: u64,
}

/// Emitted once at the start of an operation with the full list of items
/// that will be processed, so the frontend can show a "remaining" list.
#[derive(serde::Serialize, Clone)]
struct OpPendingFilesEvent {
    op_id: String,
    files: Vec<String>,
}

// Sent when verify completes so the modal can show per-file results.
#[derive(serde::Serialize, Clone)]
pub struct VerifyFileResult {
    backup_entry_id: i64,
    filename: String,
    status: String,
    detail: Option<String>,
}

#[derive(serde::Serialize, Clone)]
pub struct VerifyCompleteEvent {
    pub op_id: String,
    pub passed: usize,
    pub failed: usize,
    pub errors: usize,
    pub results: Vec<VerifyFileResult>,
}

// ── Op parameter variants ─────────────────────────────────────────────────────

pub enum OpParams {
    BackupDirectory {
        dir_path: String,
        skip_patterns: Vec<String>,
        force_checksum: bool,
    },
    RestoreFiles {
        backup_entry_ids: Vec<i64>,
        target_directory: Option<String>,
    },
    DownloadManifest {
        manifest_uuid: String,
        selected_uuids: Vec<String>,
        save_directory: String,
    },
    Scramble {
        backup_entry_ids: Vec<i64>,
        scramble_all: bool,
    },
    CleanupOrphanedLocal {
        local_file_ids: Vec<i64>,
        delete_s3: bool,
        dry_run: bool,
    },
    CleanupOrphanedS3 {
        object_keys: Vec<String>,
        dry_run: bool,
    },
    VerifyIntegrity {
        backup_entry_ids: Vec<i64>,
    },
    BackupFile {
        file_path: String,
    },
}

// ── Internal pending op ───────────────────────────────────────────────────────

struct PendingOp {
    entry: QueueEntry,
    params: OpParams,
    cancel: Arc<AtomicBool>,
}

// ── OperationQueue ────────────────────────────────────────────────────────────

pub struct OperationQueue {
    tx: mpsc::UnboundedSender<PendingOp>,
    /// Receiver is taken out once by `start_worker`; wrapped in Option so we
    /// can `take()` it without needing &mut self.
    rx: Mutex<Option<mpsc::UnboundedReceiver<PendingOp>>>,
    /// Mirror of pending entries in channel order, for snapshot queries.
    pending: Arc<Mutex<VecDeque<QueueEntry>>>,
    /// Cancel flags for ops still waiting in the channel, keyed by op id.
    pending_cancels: Arc<Mutex<HashMap<String, Arc<AtomicBool>>>>,
    /// Currently executing op.
    active: Arc<Mutex<Option<QueueEntry>>>,
    /// Cancel flag for the currently executing op.
    active_cancel: Arc<Mutex<Option<Arc<AtomicBool>>>>,
    /// App handle stored after start_worker so enqueue can emit events.
    app: Mutex<Option<AppHandle>>,
    /// Dir paths currently in-flight (pending or active) for BackupDirectory ops.
    /// Used to reject duplicate submissions before they reach the queue.
    backup_dir_paths: Arc<Mutex<HashSet<String>>>,
    /// Maps op_id → dir_path for in-flight BackupDirectory ops so that
    /// `cancel` can free the slot immediately without waiting for the worker.
    backup_dir_op_ids: Arc<Mutex<HashMap<String, String>>>,
}

impl OperationQueue {
    pub fn new() -> Self {
        let (tx, rx) = mpsc::unbounded_channel();
        Self {
            tx,
            rx: Mutex::new(Some(rx)),
            pending: Arc::new(Mutex::new(VecDeque::new())),
            pending_cancels: Arc::new(Mutex::new(HashMap::new())),
            active: Arc::new(Mutex::new(None)),
            active_cancel: Arc::new(Mutex::new(None)),
            app: Mutex::new(None),
            backup_dir_paths: Arc::new(Mutex::new(HashSet::new())),
            backup_dir_op_ids: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Atomically checks whether `dir_path` is already queued/active and, if
    /// not, registers it. Returns `true` if registration succeeded (caller may
    /// proceed), `false` if the path is already in-flight (caller should reject).
    pub fn try_register_backup_dir(&self, dir_path: &str) -> bool {
        self.backup_dir_paths.lock().unwrap().insert(dir_path.to_string())
    }

    /// Bind `op_id` to a previously-registered `dir_path` so that `cancel`
    /// can free the slot immediately. Call right after `enqueue` returns.
    pub fn bind_backup_dir_op(&self, op_id: &str, dir_path: &str) {
        self.backup_dir_op_ids.lock().unwrap().insert(op_id.to_string(), dir_path.to_string());
    }

    fn unregister_backup_dir_op(&self, op_id: &str) {
        if let Some(dir_path) = self.backup_dir_op_ids.lock().unwrap().remove(op_id) {
            self.backup_dir_paths.lock().unwrap().remove(&dir_path);
        }
    }

    /// Enqueue an operation. Returns the op ID immediately.
    pub fn enqueue(
        &self,
        label: impl Into<String>,
        op_type: impl Into<String>,
        params: OpParams,
    ) -> String {
        let id = uuid::Uuid::new_v4().to_string();
        let entry = QueueEntry {
            id: id.clone(),
            label: label.into(),
            op_type: op_type.into(),
        };
        let cancel = Arc::new(AtomicBool::new(false));
        self.pending.lock().unwrap().push_back(entry.clone());
        self.pending_cancels
            .lock()
            .unwrap()
            .insert(id.clone(), Arc::clone(&cancel));
        let _ = self.tx.send(PendingOp { entry, params, cancel });

        // Notify the frontend so it can show the new pending op immediately.
        if let Some(app) = self.app.lock().unwrap().as_ref() {
            emit_snapshot(app, &self.pending, &self.active);
        }

        id
    }

    /// Cancel a queued or active operation by ID.
    /// - Pending: removed from queue immediately; emits `queue:updated` right away.
    /// - Active: cancellation flag is set; current file finishes then op stops.
    pub fn cancel(&self, op_id: &str) {
        let was_pending = {
            let mut pending = self.pending.lock().unwrap();
            let mut cancels = self.pending_cancels.lock().unwrap();
            if let Some(flag) = cancels.remove(op_id) {
                flag.store(true, Ordering::Relaxed);
                pending.retain(|e| e.id != op_id);
                true
            } else {
                false
            }
        };

        if was_pending {
            // Free the dir-path slot immediately so the same directory can be re-queued.
            self.unregister_backup_dir_op(op_id);
            if let Some(app) = self.app.lock().unwrap().as_ref() {
                emit_snapshot(app, &self.pending, &self.active);
            }
            return;
        }

        // Not pending — try active
        let active = self.active.lock().unwrap();
        if let Some(ref entry) = *active {
            if entry.id == op_id {
                let ac = self.active_cancel.lock().unwrap();
                if let Some(flag) = ac.as_ref() {
                    flag.store(true, Ordering::Relaxed);
                }
            }
        }
    }

    /// Return a snapshot of current queue state for the frontend.
    pub fn snapshot(&self) -> QueueSnapshot {
        QueueSnapshot {
            pending: self.pending.lock().unwrap().iter().cloned().collect(),
            active: self.active.lock().unwrap().clone(),
        }
    }

    /// Start the background worker. Must be called exactly once during app setup.
    pub fn start_worker(&self, app: AppHandle) {
        *self.app.lock().unwrap() = Some(app.clone());

        let rx = self
            .rx
            .lock()
            .unwrap()
            .take()
            .expect("start_worker called twice");

        let pending = Arc::clone(&self.pending);
        let pending_cancels = Arc::clone(&self.pending_cancels);
        let active = Arc::clone(&self.active);
        let active_cancel_store = Arc::clone(&self.active_cancel);
        let backup_dir_op_ids = Arc::clone(&self.backup_dir_op_ids);
        let backup_dir_paths = Arc::clone(&self.backup_dir_paths);

        tauri::async_runtime::spawn(async move {
            let mut rx = rx;
            while let Some(op) = rx.recv().await {
                // Remove from pending tracking (it's now "in flight")
                {
                    pending.lock().unwrap().retain(|e| e.id != op.entry.id);
                    pending_cancels.lock().unwrap().remove(&op.entry.id);
                }

                let op_id = op.entry.id.clone();

                // Skip if cancelled while waiting in queue
                if op.cancel.load(Ordering::Relaxed) {
                    // Free dir-path slot (cancel() may have done this already; no-op if so).
                    if let Some(dir) = backup_dir_op_ids.lock().unwrap().remove(&op_id) {
                        backup_dir_paths.lock().unwrap().remove(&dir);
                    }
                    emit_snapshot(&app, &pending, &active);
                    continue;
                }

                // Mark as active
                {
                    *active.lock().unwrap() = Some(op.entry.clone());
                    *active_cancel_store.lock().unwrap() = Some(Arc::clone(&op.cancel));
                }
                emit_snapshot(&app, &pending, &active);

                // Execute
                let result = run_op(&app, &op_id, op.params, op.cancel).await;

                // Free dir-path slot for BackupDirectory ops.
                if let Some(dir) = backup_dir_op_ids.lock().unwrap().remove(&op_id) {
                    backup_dir_paths.lock().unwrap().remove(&dir);
                }

                // Clear active
                {
                    *active.lock().unwrap() = None;
                    *active_cancel_store.lock().unwrap() = None;
                }

                // Emit result
                match result {
                    Ok(message) => {
                        let _ = app.emit("op:complete", OpCompleteEvent { id: op_id, message });
                    }
                    Err(e) => {
                        let _ =
                            app.emit("op:failed", OpFailedEvent { id: op_id, error: e.to_string() });
                    }
                }

                emit_snapshot(&app, &pending, &active);
            }
        });
    }
}

fn emit_snapshot(
    app: &AppHandle,
    pending: &Arc<Mutex<VecDeque<QueueEntry>>>,
    active: &Arc<Mutex<Option<QueueEntry>>>,
) {
    let snapshot = QueueSnapshot {
        pending: pending.lock().unwrap().iter().cloned().collect(),
        active: active.lock().unwrap().clone(),
    };
    let _ = app.emit("queue:updated", snapshot);
}

// ── Private helpers ────────────────────────────────────────────────────────────

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

// ── Op dispatcher ─────────────────────────────────────────────────────────────

async fn run_op(
    app: &AppHandle,
    op_id: &str,
    params: OpParams,
    cancel: Arc<AtomicBool>,
) -> Result<String, AppError> {
    match params {
        OpParams::BackupDirectory { dir_path, skip_patterns, force_checksum } => {
            run_backup_directory(app, op_id, &dir_path, skip_patterns, force_checksum, cancel)
                .await
        }
        OpParams::RestoreFiles { backup_entry_ids, target_directory } => {
            run_restore_files(app, op_id, backup_entry_ids, target_directory, cancel).await
        }
        OpParams::DownloadManifest { manifest_uuid, selected_uuids, save_directory } => {
            run_download_manifest(app, op_id, manifest_uuid, selected_uuids, save_directory, cancel)
                .await
        }
        OpParams::Scramble { backup_entry_ids, scramble_all } => {
            run_scramble(app, op_id, backup_entry_ids, scramble_all, cancel).await
        }
        OpParams::CleanupOrphanedLocal { local_file_ids, delete_s3, dry_run } => {
            run_cleanup_local(app, op_id, local_file_ids, delete_s3, dry_run).await
        }
        OpParams::CleanupOrphanedS3 { object_keys, dry_run } => {
            run_cleanup_s3(app, op_id, object_keys, dry_run).await
        }
        OpParams::VerifyIntegrity { backup_entry_ids } => {
            run_verify_integrity(app, op_id, backup_entry_ids).await
        }
        OpParams::BackupFile { file_path } => {
            run_backup_file(app, op_id, &file_path).await
        }
    }
}

// ── BackupFile ────────────────────────────────────────────────────────────────

async fn run_backup_file(
    app: &AppHandle,
    op_id: &str,
    file_path: &str,
) -> Result<String, AppError> {
    let name = Path::new(file_path)
        .file_name()
        .map(|f| f.to_string_lossy().into_owned())
        .unwrap_or_else(|| file_path.to_string());

    let _ = app.emit(
        "op:pending_files",
        OpPendingFilesEvent { op_id: op_id.to_string(), files: vec![name.clone()] },
    );
    let _ = app.emit(
        "backup:progress",
        BackupProgressEvent {
            op_id: op_id.to_string(),
            processed: 0,
            total: 1,
            uploaded: 0,
            deduped: 0,
            skipped: 0,
            failed: 0,
            current_file: name.clone(),
        },
    );
    let _ = app.emit(
        "upload:progress",
        UploadProgressEvent {
            op_id: op_id.to_string(),
            bytes_done: 0,
            bytes_total: 1,
            phase: "Uploading".into(),
            phase_done: 0,
            phase_total: 1,
        },
    );

    let profile = {
        let db = app.state::<DbState>();
        let conn = db.conn()?;
        get_active_profile_or_err(&conn)?
    };
    let key_hex = get_profile_encryption_key(&profile)?;
    let key_bytes = crypto::decode_encryption_key(&key_hex)?;
    let s3 = build_s3_client(&profile).await?;
    let db = app.state::<DbState>();
    let stored_path = backup::strip_relative_path(file_path, profile.relative_path.as_deref());
    let chunk_size = profile.chunk_size_bytes as usize;

    let outcome = backup::backup_file(
        &db,
        &s3,
        profile.id,
        Path::new(file_path),
        &stored_path,
        &key_bytes,
        profile.s3_key_prefix.as_deref(),
        chunk_size,
    )
    .await?;

    let (uploaded, deduped, skipped, message) = match outcome {
        backup::FileOutcome::Skipped => (0usize, 0usize, 1usize, "1 skipped".to_string()),
        backup::FileOutcome::Deduped => (0, 1, 0, "1 deduped".to_string()),
        backup::FileOutcome::Uploaded { .. } => (1, 0, 0, "1 uploaded".to_string()),
    };

    let _ = app.emit(
        "backup:progress",
        BackupProgressEvent {
            op_id: op_id.to_string(),
            processed: 1,
            total: 1,
            uploaded,
            deduped,
            skipped,
            failed: 0,
            current_file: name,
        },
    );

    Ok(message)
}

// ── BackupDirectory ───────────────────────────────────────────────────────────

async fn run_backup_directory(
    app: &AppHandle,
    op_id: &str,
    dir_path: &str,
    skip_patterns: Vec<String>,
    force_checksum: bool,
    cancel: Arc<AtomicBool>,
) -> Result<String, AppError> {
    let regexes: Vec<regex::Regex> = skip_patterns
        .iter()
        .map(|p| regex::Regex::new(p))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| AppError::Config(format!("Invalid skip pattern: {}", e)))?;

    let (profile, encryption_key) = {
        let db = app.state::<DbState>();
        let conn = db.conn()?;
        let profile = get_active_profile_or_err(&conn)?;
        let key = get_profile_encryption_key(&profile)?;
        (profile, key)
    };

    let s3 = build_s3_client(&profile).await?;

    let op_id_owned = op_id.to_string();
    let app_clone = app.clone();

    // Pre-scan to let the frontend show the full pending file list.
    // Send full paths so the frontend can remove the correct entry when two
    // files in different subdirectories share the same basename.
    let pending_names: Vec<String> = backup::scan_directory(Path::new(dir_path), &regexes)?
        .into_iter()
        .map(|p| p.to_string_lossy().into_owned())
        .collect();
    let _ = app.emit(
        "op:pending_files",
        OpPendingFilesEvent { op_id: op_id.to_string(), files: pending_names },
    );

    let db = app.state::<DbState>();
    let summary = backup::backup_directory(
        &db,
        &s3,
        profile.id,
        Path::new(dir_path),
        &encryption_key,
        profile.s3_key_prefix.as_deref(),
        profile.relative_path.as_deref(),
        profile.chunk_size_bytes as usize,
        &regexes,
        force_checksum,
        cancel,
        move |s: &backup::BackupSummary, current_file: &str| {
            let _ = app_clone.emit(
                "backup:progress",
                BackupProgressEvent {
                    op_id: op_id_owned.clone(),
                    processed: s.processed,
                    total: s.total_files,
                    uploaded: s.uploaded,
                    deduped: s.deduped,
                    skipped: s.skipped,
                    failed: s.failed,
                    current_file: current_file.to_string(),
                },
            );
        },
    )
    .await?;

    let mut parts = vec![
        format!("{} uploaded", summary.uploaded),
        format!("{} deduped", summary.deduped),
        format!("{} skipped", summary.skipped),
    ];
    if summary.failed > 0 {
        parts.push(format!("{} failed", summary.failed));
    }
    Ok(parts.join(", "))
}

// ── RestoreFiles ──────────────────────────────────────────────────────────────

async fn run_restore_files(
    app: &AppHandle,
    op_id: &str,
    backup_entry_ids: Vec<i64>,
    target_directory: Option<String>,
    _cancel: Arc<AtomicBool>,
) -> Result<String, AppError> {
    let (profile, entries) = {
        let db = app.state::<DbState>();
        let conn = db.conn()?;
        let profile = get_active_profile_or_err(&conn)?;
        let mut entries = Vec::new();
        for id in &backup_entry_ids {
            if let Some(entry) = db::get_file_entry_by_id(&conn, *id)? {
                let local_files = db::list_local_files_for_entry(&conn, entry.id)?;
                let path = local_files.first().map(|lf| lf.local_path.clone());
                let chunk_keys = db::get_chunk_keys_for_file(&conn, entry.id)?;
                entries.push((entry, path, chunk_keys));
            }
        }
        (profile, entries)
    };

    let key_hex = get_profile_encryption_key(&profile)?;
    let key_bytes = crypto::decode_encryption_key(&key_hex)?;
    let s3 = build_s3_client(&profile).await?;

    let total = entries.len();
    let mut restored = 0usize;
    let mut skipped = 0usize;
    let mut failed = 0usize;

    let pending_names: Vec<String> = entries
        .iter()
        .map(|(entry, stored_path, _)| {
            stored_path
                .as_ref()
                .and_then(|p| Path::new(p).file_name())
                .map(|f| f.to_string_lossy().into_owned())
                .unwrap_or_else(|| entry.original_md5.clone())
        })
        .collect();
    let _ = app.emit(
        "op:pending_files",
        OpPendingFilesEvent { op_id: op_id.to_string(), files: pending_names },
    );

    for (entry, stored_path, chunk_keys) in &entries {
        let current_filename = stored_path
            .as_ref()
            .and_then(|p| Path::new(p).file_name())
            .map(|f| f.to_string_lossy().to_string())
            .unwrap_or_else(|| entry.original_md5.clone());

        let _ = app.emit(
            "restore:progress",
            RestoreProgressEvent {
                op_id: op_id.to_string(),
                processed: restored + skipped + failed,
                total,
                current_file: current_filename.clone(),
                restored,
                skipped,
                failed,
            },
        );

        let target_path = match &target_directory {
            Some(dir) => {
                let filename = stored_path
                    .as_ref()
                    .and_then(|p| Path::new(p).file_name())
                    .map(|f| f.to_string_lossy().to_string())
                    .unwrap_or_else(|| entry.original_md5.clone());
                PathBuf::from(dir).join(filename)
            }
            None => match stored_path {
                Some(sp) => backup::expand_relative_path(sp, profile.relative_path.as_deref()),
                None => {
                    failed += 1;
                    continue;
                }
            },
        };

        // Skip if the existing file already has matching content.
        if target_path.exists() {
            if let Ok(md5) = crypto::compute_file_md5(&target_path) {
                if md5 == entry.original_md5 {
                    skipped += 1;
                    continue;
                }
            }
        }

        if let Some(parent) = target_path.parent() {
            std::fs::create_dir_all(parent).ok();
        }

        match restore_file_from_chunks(
            &s3,
            chunk_keys,
            &key_bytes,
            &target_path,
            app,
            op_id,
            entry.total_size as u64,
        )
        .await
        {
            Ok(()) => {
                // Verify final MD5
                match crypto::compute_file_md5(&target_path) {
                    Ok(md5) if md5 == entry.original_md5 => restored += 1,
                    Ok(_) => {
                        let _ = std::fs::remove_file(&target_path);
                        failed += 1;
                    }
                    Err(_) => failed += 1,
                }
            }
            Err(_) => {
                let _ = std::fs::remove_file(&target_path);
                failed += 1;
            }
        }
    }

    let mut parts = vec![format!("{} restored", restored), format!("{} skipped", skipped)];
    if failed > 0 {
        parts.push(format!("{} failed", failed));
    }
    Ok(parts.join(", "))
}

/// Download and decrypt all chunks for a file, writing plaintext to `target_path`.
async fn restore_file_from_chunks(
    s3: &S3Client,
    chunk_keys: &[(usize, String)],
    key_bytes: &[u8; 32],
    target_path: &Path,
    app: &AppHandle,
    op_id: &str,
    total_size: u64,
) -> Result<(), AppError> {
    let mut out_file = std::fs::File::create(target_path)?;
    let mut bytes_done: u64 = 0;
    let bytes_total = total_size.max(1);

    let _ = app.emit(
        "upload:progress",
        UploadProgressEvent {
            op_id: op_id.to_string(),
            bytes_done: 0,
            bytes_total,
            phase: "Downloading".into(),
            phase_done: 0,
            phase_total: bytes_total,
        },
    );

    for (_, s3_key) in chunk_keys {
        let encrypted = s3.download_chunk(s3_key).await?;
        let plaintext = crypto::decrypt_chunk(key_bytes, &encrypted)?;
        bytes_done += plaintext.len() as u64;
        out_file.write_all(&plaintext)?;

        let _ = app.emit(
            "upload:progress",
            UploadProgressEvent {
                op_id: op_id.to_string(),
                bytes_done,
                bytes_total,
                phase: "Downloading".into(),
                phase_done: bytes_done,
                phase_total: bytes_total,
            },
        );
    }

    Ok(())
}

// ── DownloadManifest ──────────────────────────────────────────────────────────

/// Internal manifest JSON format (mirrors ManifestJsonFile in commands.rs).
#[derive(serde::Deserialize)]
struct ManifestJsonFile {
    filename: String,
    original_md5: String,
    total_size: i64,
    /// Ordered list of S3 keys for restoring this file.
    chunks: Vec<String>,
}

#[derive(serde::Deserialize)]
struct ManifestJson {
    #[allow(dead_code)]
    version: i32,
    files: Vec<ManifestJsonFile>,
}

async fn run_download_manifest(
    app: &AppHandle,
    op_id: &str,
    manifest_uuid: String,
    selected_uuids: Vec<String>,
    save_directory: String,
    _cancel: Arc<AtomicBool>,
) -> Result<String, AppError> {
    let profile = {
        let db = app.state::<DbState>();
        let conn = db.conn()?;
        get_active_profile_or_err(&conn)?
    };

    let key_hex = get_profile_encryption_key(&profile)?;
    let key_bytes = crypto::decode_encryption_key(&key_hex)?;
    let s3 = build_s3_client(&profile).await?;

    // manifest_uuid is the full S3 key (e.g. "{prefix}/m/{uuid}")
    let encrypted = s3.download_chunk(&manifest_uuid).await?;
    let manifest_bytes = crypto::decrypt_chunk(&key_bytes, &encrypted)?;
    let manifest: ManifestJson = serde_json::from_slice(&manifest_bytes)
        .map_err(|e| AppError::InvalidData(format!("Invalid manifest format: {}", e)))?;

    let to_download: Vec<&ManifestJsonFile> = if selected_uuids.is_empty() {
        manifest.files.iter().collect()
    } else {
        manifest.files.iter().filter(|f| selected_uuids.contains(&f.original_md5)).collect()
    };

    let save_dir = Path::new(&save_directory);
    std::fs::create_dir_all(save_dir)?;

    let total = to_download.len();
    let mut restored = 0usize;
    let mut failed = 0usize;

    let pending_names: Vec<String> =
        to_download.iter().map(|f| f.filename.clone()).collect();
    let _ = app.emit(
        "op:pending_files",
        OpPendingFilesEvent { op_id: op_id.to_string(), files: pending_names },
    );

    for file_entry in &to_download {
        let _ = app.emit(
            "restore:progress",
            RestoreProgressEvent {
                op_id: op_id.to_string(),
                processed: restored + failed,
                total,
                current_file: file_entry.filename.clone(),
                restored,
                skipped: 0,
                failed,
            },
        );

        let target_path = save_dir.join(&file_entry.filename);
        let chunk_keys: Vec<(usize, String)> =
            file_entry.chunks.iter().cloned().enumerate().collect();

        match restore_file_from_chunks(
            &s3,
            &chunk_keys,
            &key_bytes,
            &target_path,
            app,
            op_id,
            file_entry.total_size as u64,
        )
        .await
        {
            Ok(()) => {
                match crypto::compute_file_md5(&target_path) {
                    Ok(md5) if md5 == file_entry.original_md5 => restored += 1,
                    Ok(_) => {
                        let _ = std::fs::remove_file(&target_path);
                        failed += 1;
                    }
                    Err(_) => failed += 1,
                }
            }
            Err(_) => {
                let _ = std::fs::remove_file(&target_path);
                failed += 1;
            }
        }
    }

    let mut parts = vec![format!("{} downloaded", restored)];
    if failed > 0 {
        parts.push(format!("{} failed", failed));
    }
    Ok(parts.join(", "))
}

// ── Scramble ──────────────────────────────────────────────────────────────────

async fn run_scramble(
    app: &AppHandle,
    op_id: &str,
    backup_entry_ids: Vec<i64>,
    scramble_all: bool,
    _cancel: Arc<AtomicBool>,
) -> Result<String, AppError> {
    let (profile, file_entries) = {
        let db = app.state::<DbState>();
        let conn = db.conn()?;
        let profile = get_active_profile_or_err(&conn)?;
        let entries = if scramble_all {
            db::list_file_entries(&conn, profile.id)?
        } else {
            let mut e = Vec::new();
            for id in &backup_entry_ids {
                if let Some(entry) = db::get_file_entry_by_id(&conn, *id)? {
                    e.push(entry);
                }
            }
            e
        };
        (profile, entries)
    };

    let s3 = build_s3_client(&profile).await?;
    let total = file_entries.len();
    let mut scrambled = 0usize;
    let mut failed = 0usize;
    let mut scrambled_entry_ids: Vec<i64> = Vec::new();

    let pending_names: Vec<String> = file_entries
        .iter()
        .map(|e| format!("{}…", &e.original_md5[..8.min(e.original_md5.len())]))
        .collect();
    let _ = app.emit(
        "op:pending_files",
        OpPendingFilesEvent { op_id: op_id.to_string(), files: pending_names },
    );

    for entry in &file_entries {
        let display = format!("{}…", &entry.original_md5[..8.min(entry.original_md5.len())]);
        let _ = app.emit(
            "scramble:progress",
            ScrambleProgressEvent {
                op_id: op_id.to_string(),
                processed: scrambled + failed,
                total,
                current_file: display,
                scrambled,
                failed,
            },
        );

        // Collect chunks exclusively owned by this file_entry (count == 1).
        let exclusive_chunks: Vec<(i64, String)> = {
            let db = app.state::<DbState>();
            let conn = db.conn()?;
            let chunk_ids = db::get_chunk_ids_for_file(&conn, entry.id)?;
            let mut result = Vec::new();
            for chunk_id in chunk_ids {
                let count = db::count_file_entries_for_chunk(&conn, chunk_id)?;
                if count == 1 {
                    if let Some(c) = db::get_chunk_by_id(&conn, chunk_id)? {
                        result.push((chunk_id, c.s3_key));
                    }
                }
            }
            result
        };

        let mut entry_ok = true;
        for (chunk_id, old_key) in &exclusive_chunks {
            let new_key = backup::make_chunk_s3_key(
                profile.s3_key_prefix.as_deref(),
                &uuid::Uuid::new_v4().to_string(),
            );
            match s3.copy_object(old_key, &new_key).await {
                Ok(()) => match s3.delete_object(old_key).await {
                    Ok(()) => {
                        let db = app.state::<DbState>();
                        let conn = db.conn()?;
                        db::update_chunk_s3_key(&conn, *chunk_id, &new_key)?;
                    }
                    Err(_) => {
                        s3.delete_object(&new_key).await.ok();
                        entry_ok = false;
                    }
                },
                Err(_) => {
                    entry_ok = false;
                }
            }
        }

        if entry_ok {
            scrambled += 1;
            scrambled_entry_ids.push(entry.id);
        } else {
            failed += 1;
        }
    }

    // Invalidate share manifests that reference scrambled file_entries.
    if !scrambled_entry_ids.is_empty() {
        let db = app.state::<DbState>();
        let conn = db.conn()?;
        let manifests = db::list_share_manifests(&conn, profile.id)?;
        for manifest in &manifests {
            if !manifest.is_valid {
                continue;
            }
            let entries = db::list_share_manifest_entries(&conn, manifest.id)?;
            if entries.iter().any(|e| scrambled_entry_ids.contains(&e.file_entry_id)) {
                db::invalidate_share_manifest(&conn, manifest.id)?;
            }
        }
    }

    let mut parts = vec![format!("{} scrambled", scrambled)];
    if failed > 0 {
        parts.push(format!("{} failed", failed));
    }
    Ok(parts.join(", "))
}

// ── CleanupOrphanedLocal ──────────────────────────────────────────────────────

async fn run_cleanup_local(
    app: &AppHandle,
    op_id: &str,
    local_file_ids: Vec<i64>,
    delete_s3: bool,
    dry_run: bool,
) -> Result<String, AppError> {
    if dry_run {
        return Ok(format!("Would delete {} local entries", local_file_ids.len()));
    }

    let profile = {
        let db = app.state::<DbState>();
        let conn = db.conn()?;
        get_active_profile_or_err(&conn)?
    };

    let s3 = if delete_s3 { Some(build_s3_client(&profile).await?) } else { None };
    let total = local_file_ids.len();
    let mut deleted = 0usize;
    let mut failed = 0usize;

    // Collect display names for the pending list.
    {
        let db = app.state::<DbState>();
        let conn = db.conn()?;
        let pending_names: Vec<String> = local_file_ids
            .iter()
            .filter_map(|lf_id| db::get_local_file_by_id(&conn, *lf_id).ok().flatten())
            .map(|lf| {
                Path::new(&lf.local_path)
                    .file_name()
                    .map(|f| f.to_string_lossy().into_owned())
                    .unwrap_or_else(|| lf.local_path.clone())
            })
            .collect();
        let _ = app.emit(
            "op:pending_files",
            OpPendingFilesEvent { op_id: op_id.to_string(), files: pending_names },
        );
    }

    for (idx, lf_id) in local_file_ids.iter().enumerate() {
        // Fetch the local_file row.
        let lf = {
            let db = app.state::<DbState>();
            let conn = db.conn()?;
            db::get_local_file_by_id(&conn, *lf_id)?
        };
        let lf = match lf {
            Some(lf) => lf,
            None => {
                deleted += 1;
                continue;
            }
        };

        let current_item = Path::new(&lf.local_path)
            .file_name()
            .map(|f| f.to_string_lossy().into_owned())
            .unwrap_or_else(|| lf.local_path.clone());

        let _ = app.emit(
            "cleanup:progress",
            CleanupProgressEvent {
                op_id: op_id.to_string(),
                processed: idx,
                total,
                current_item: current_item.clone(),
                deleted,
                failed,
            },
        );

        let file_entry_id = lf.file_entry_id;

        // If deleting S3, record which chunks are exclusively owned BEFORE
        // removing file_chunk rows.
        let exclusive_chunks: Vec<(i64, String)> = if delete_s3 {
            let db = app.state::<DbState>();
            let conn = db.conn()?;
            let chunk_ids = db::get_chunk_ids_for_file(&conn, file_entry_id)?;
            let mut result = Vec::new();
            for chunk_id in chunk_ids {
                let count = db::count_file_entries_for_chunk(&conn, chunk_id)?;
                if count == 1 {
                    if let Some(c) = db::get_chunk_by_id(&conn, chunk_id)? {
                        result.push((chunk_id, c.s3_key));
                    }
                }
            }
            result
        } else {
            Vec::new()
        };

        // Delete local_file; if no more local_files for this file_entry, clean up.
        let orphaned_entry = {
            let db = app.state::<DbState>();
            let conn = db.conn()?;
            db::delete_local_file(&conn, *lf_id)?;
            deleted += 1;

            let remaining: i64 = conn
                .prepare("SELECT COUNT(*) FROM local_file WHERE file_entry_id = ?1")?
                .query_row(rusqlite::params![file_entry_id], |r| r.get(0))?;

            if remaining == 0 {
                conn.execute(
                    "DELETE FROM file_chunk WHERE file_entry_id = ?1",
                    rusqlite::params![file_entry_id],
                )?;
                conn.execute(
                    "DELETE FROM share_manifest_entry WHERE file_entry_id = ?1",
                    rusqlite::params![file_entry_id],
                )?;
                db::delete_file_entry(&conn, file_entry_id)?;
                true
            } else {
                false
            }
        };

        // Delete orphaned S3 chunks.
        if orphaned_entry && delete_s3 {
            if let Some(ref s3_client) = s3 {
                for (chunk_id, s3_key) in &exclusive_chunks {
                    if s3_client.delete_object(s3_key).await.is_err() {
                        failed += 1;
                    } else {
                        let db = app.state::<DbState>();
                        let conn = db.conn()?;
                        db::delete_chunk(&conn, *chunk_id)?;
                    }
                }
            }
        }
    }

    let mut parts = vec![format!("{} entries removed", deleted)];
    if failed > 0 {
        parts.push(format!("{} failed", failed));
    }
    Ok(parts.join(", "))
}

// ── CleanupOrphanedS3 ─────────────────────────────────────────────────────────

async fn run_cleanup_s3(
    app: &AppHandle,
    op_id: &str,
    object_keys: Vec<String>,
    dry_run: bool,
) -> Result<String, AppError> {
    if dry_run {
        return Ok(format!("Would delete {} S3 objects", object_keys.len()));
    }

    let profile = {
        let db = app.state::<DbState>();
        let conn = db.conn()?;
        get_active_profile_or_err(&conn)?
    };

    let s3 = build_s3_client(&profile).await?;
    let total = object_keys.len();
    let mut deleted = 0usize;
    let mut failed = 0usize;

    let pending_names: Vec<String> = object_keys
        .iter()
        .map(|k| k.split('/').next_back().unwrap_or(k).to_string())
        .collect();
    let _ = app.emit(
        "op:pending_files",
        OpPendingFilesEvent { op_id: op_id.to_string(), files: pending_names },
    );

    for (idx, key) in object_keys.iter().enumerate() {
        let _ = app.emit(
            "cleanup:progress",
            CleanupProgressEvent {
                op_id: op_id.to_string(),
                processed: idx,
                total,
                current_item: key.split('/').next_back().unwrap_or(key).to_string(),
                deleted,
                failed,
            },
        );

        match s3.delete_object(key).await {
            Ok(()) => deleted += 1,
            Err(_) => failed += 1,
        }
    }

    let mut parts = vec![format!("{} deleted", deleted)];
    if failed > 0 {
        parts.push(format!("{} failed", failed));
    }
    Ok(parts.join(", "))
}

// ── VerifyIntegrity ───────────────────────────────────────────────────────────

async fn run_verify_integrity(
    app: &AppHandle,
    op_id: &str,
    backup_entry_ids: Vec<i64>,
) -> Result<String, AppError> {
    let (profile, entries) = {
        let db = app.state::<DbState>();
        let conn = db.conn()?;
        let profile = get_active_profile_or_err(&conn)?;
        let mut entries = Vec::new();
        for id in &backup_entry_ids {
            if let Some(e) = db::get_file_entry_by_id(&conn, *id)? {
                let local_files = db::list_local_files_for_entry(&conn, e.id)?;
                let filename = local_files
                    .first()
                    .map(|lf| {
                        Path::new(&lf.local_path)
                            .file_name()
                            .map(|f| f.to_string_lossy().to_string())
                            .unwrap_or_else(|| lf.local_path.clone())
                    })
                    .unwrap_or_else(|| e.original_md5.clone());
                let chunk_keys = db::get_chunk_keys_for_file(&conn, e.id)?;
                entries.push((e, filename, chunk_keys));
            }
        }
        (profile, entries)
    };

    let key_hex = get_profile_encryption_key(&profile)?;
    let key_bytes = crypto::decode_encryption_key(&key_hex)?;
    let s3 = build_s3_client(&profile).await?;

    let total = entries.len();
    let mut passed = 0usize;
    let mut failed = 0usize;
    let mut errors = 0usize;
    let mut results: Vec<VerifyFileResult> = Vec::new();

    let pending_names: Vec<String> = entries.iter().map(|(_, name, _)| name.clone()).collect();
    let _ = app.emit(
        "op:pending_files",
        OpPendingFilesEvent { op_id: op_id.to_string(), files: pending_names },
    );

    for (entry, filename, chunk_keys) in &entries {
        let _ = app.emit(
            "verify:progress",
            VerifyProgressEvent {
                op_id: op_id.to_string(),
                processed: passed + failed + errors,
                total,
                current_file: filename.clone(),
                passed,
                failed,
                errors,
            },
        );

        // Download and decrypt all chunks, accumulating a rolling MD5.
        // No temp files needed — each chunk is processed in memory and dropped.
        let mut hasher = Md5::new();
        let mut error_detail: Option<String> = None;

        'chunks: for (_, s3_key) in chunk_keys {
            match s3.download_chunk(s3_key).await {
                Ok(encrypted) => match crypto::decrypt_chunk(&key_bytes, &encrypted) {
                    Ok(plaintext) => hasher.update(&plaintext),
                    Err(e) => {
                        error_detail = Some(format!("Decrypt failed ({}): {}", s3_key, e));
                        break 'chunks;
                    }
                },
                Err(e) => {
                    error_detail = Some(format!("Download failed ({}): {}", s3_key, e));
                    break 'chunks;
                }
            }
        }

        if let Some(detail) = error_detail {
            errors += 1;
            results.push(VerifyFileResult {
                backup_entry_id: entry.id,
                filename: filename.clone(),
                status: "error".into(),
                detail: Some(detail),
            });
        } else {
            let computed_md5 = hex::encode(hasher.finalize());
            if computed_md5 == entry.original_md5 {
                passed += 1;
                results.push(VerifyFileResult {
                    backup_entry_id: entry.id,
                    filename: filename.clone(),
                    status: "passed".into(),
                    detail: None,
                });
            } else {
                failed += 1;
                results.push(VerifyFileResult {
                    backup_entry_id: entry.id,
                    filename: filename.clone(),
                    status: "failed".into(),
                    detail: Some(format!(
                        "MD5 mismatch: expected {}, got {}",
                        entry.original_md5, computed_md5
                    )),
                });
            }
        }
    }

    let _ = app.emit(
        "verify:complete",
        VerifyCompleteEvent {
            op_id: op_id.to_string(),
            passed,
            failed,
            errors,
            results,
        },
    );

    let mut parts = vec![format!("{} passed", passed)];
    if failed > 0 {
        parts.push(format!("{} failed", failed));
    }
    if errors > 0 {
        parts.push(format!("{} errors", errors));
    }
    Ok(parts.join(", "))
}
