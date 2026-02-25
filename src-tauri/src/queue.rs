//! Serial, in-memory FIFO queue for all S3-touching operations.
//!
//! One worker task runs operations one at a time. Each op carries its own
//! cancellation flag. The queue emits `queue:updated`, `op:complete`, and
//! `op:failed` events to the frontend, plus per-op progress events
//! (`backup:progress`, `restore:progress`, `scramble:progress`, `verify:progress`).

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use rusqlite::OptionalExtension;
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
    /// - Pending: removed from queue immediately.
    /// - Active: cancellation flag is set; current file finishes then op stops.
    pub fn cancel(&self, op_id: &str) {
        {
            let mut pending = self.pending.lock().unwrap();
            let mut cancels = self.pending_cancels.lock().unwrap();
            if let Some(flag) = cancels.remove(op_id) {
                flag.store(true, Ordering::Relaxed);
                pending.retain(|e| e.id != op_id);
                return;
            }
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

        tauri::async_runtime::spawn(async move {
            let mut rx = rx;
            while let Some(op) = rx.recv().await {
                // Remove from pending tracking (it's now "in flight")
                {
                    pending.lock().unwrap().retain(|e| e.id != op.entry.id);
                    pending_cancels.lock().unwrap().remove(&op.entry.id);
                }

                // Skip if cancelled while waiting in queue
                if op.cancel.load(Ordering::Relaxed) {
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
                let op_id = op.entry.id.clone();
                let result = run_op(&app, &op_id, op.params, op.cancel).await;

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

// ── Private helpers (mirrors commands.rs) ────────────────────────────────────

fn get_active_profile_or_err(conn: &rusqlite::Connection) -> Result<db::Profile, AppError> {
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
    }
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

    let (profile, encryption_key, temp_dir) = {
        let db = app.state::<DbState>();
        let conn = db.conn()?;
        let profile = get_active_profile_or_err(&conn)?;
        let key = get_profile_encryption_key(&profile)?;
        let tmp = get_temp_dir(&profile);
        (profile, key, tmp)
    };

    let s3 = build_s3_client(&profile).await?;

    let op_id_owned = op_id.to_string();
    let app_clone = app.clone();

    // Pre-scan to let the frontend show the full pending file list.
    let pending_names: Vec<String> = backup::scan_directory(Path::new(dir_path), &regexes)?
        .into_iter()
        .filter_map(|p| p.file_name().map(|f| f.to_string_lossy().into_owned()))
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
        profile.relative_path.as_deref(),
        &temp_dir,
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

    let total = entries.len();
    let mut restored = 0usize;
    let mut skipped = 0usize;
    let mut failed = 0usize;

    let pending_names: Vec<String> = entries
        .iter()
        .map(|(entry, stored_path)| {
            stored_path
                .as_ref()
                .and_then(|p| Path::new(p).file_name())
                .map(|f| f.to_string_lossy().into_owned())
                .unwrap_or_else(|| entry.object_uuid.clone())
        })
        .collect();
    let _ = app.emit(
        "op:pending_files",
        OpPendingFilesEvent { op_id: op_id.to_string(), files: pending_names },
    );

    for (entry, stored_path) in &entries {
        let current_filename = stored_path
            .as_ref()
            .and_then(|p| Path::new(p).file_name())
            .map(|f| f.to_string_lossy().to_string())
            .unwrap_or_else(|| entry.object_uuid.clone());

        let _ = app.emit(
            "restore:progress",
            RestoreProgressEvent {
                op_id: op_id.to_string(),
                processed: restored + skipped + failed,
                total,
                current_file: current_filename,
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
                    .unwrap_or_else(|| entry.object_uuid.clone());
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

        let temp_enc = crypto::generate_temp_path(&temp_dir);
        if s3.download_object(&entry.object_uuid, &temp_enc).await.is_err() {
            let _ = std::fs::remove_file(&temp_enc);
            failed += 1;
            continue;
        }

        let result = crypto::decrypt_file(&temp_enc, &target_path, &encryption_key);
        let _ = std::fs::remove_file(&temp_enc);
        match result {
            Ok(()) => restored += 1,
            Err(_) => failed += 1,
        }
    }

    let mut parts = vec![format!("{} restored", restored), format!("{} skipped", skipped)];
    if failed > 0 {
        parts.push(format!("{} failed", failed));
    }
    Ok(parts.join(", "))
}

// ── DownloadManifest ──────────────────────────────────────────────────────────

#[derive(serde::Deserialize)]
struct ManifestFileEntry {
    uuid: String,
    filename: String,
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

    let encryption_key = get_profile_encryption_key(&profile)?;
    let temp_dir = get_temp_dir(&profile);
    let s3 = build_s3_client(&profile).await?;

    // Download and decrypt the manifest
    let temp_enc = crypto::generate_temp_path(&temp_dir);
    let temp_dec = crypto::generate_temp_path(&temp_dir);
    s3.download_object(&manifest_uuid, &temp_enc).await?;
    crypto::decrypt_file(&temp_enc, &temp_dec, &encryption_key)?;
    let _ = std::fs::remove_file(&temp_enc);

    let manifest_json = std::fs::read_to_string(&temp_dec)?;
    let _ = std::fs::remove_file(&temp_dec);

    let manifest: serde_json::Value = serde_json::from_str(&manifest_json)?;
    let all_files: Vec<ManifestFileEntry> = serde_json::from_value(
        manifest
            .get("files")
            .ok_or_else(|| AppError::InvalidData("manifest is missing 'files' key".into()))?
            .clone(),
    )?;

    let to_download: Vec<&ManifestFileEntry> = if selected_uuids.is_empty() {
        all_files.iter().collect()
    } else {
        all_files.iter().filter(|f| selected_uuids.contains(&f.uuid)).collect()
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
        let temp_enc = crypto::generate_temp_path(&temp_dir);

        match s3.download_object(&file_entry.uuid, &temp_enc).await {
            Ok(()) => {
                let result = crypto::decrypt_file(&temp_enc, &target_path, &encryption_key);
                let _ = std::fs::remove_file(&temp_enc);
                match result {
                    Ok(()) => restored += 1,
                    Err(_) => failed += 1,
                }
            }
            Err(_) => {
                let _ = std::fs::remove_file(&temp_enc);
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
    let (profile, entries) = {
        let db = app.state::<DbState>();
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
    let total = entries.len();
    let mut scrambled = 0usize;
    let mut failed = 0usize;
    let mut scrambled_entry_ids = Vec::new();

    let pending_names: Vec<String> = entries
        .iter()
        .map(|e| format!("{}…", &e.object_uuid[..8]))
        .collect();
    let _ = app.emit(
        "op:pending_files",
        OpPendingFilesEvent { op_id: op_id.to_string(), files: pending_names },
    );

    for entry in &entries {
        let _ = app.emit(
            "scramble:progress",
            ScrambleProgressEvent {
                op_id: op_id.to_string(),
                processed: scrambled + failed,
                total,
                current_file: format!("{}...", &entry.object_uuid[..8]),
                scrambled,
                failed,
            },
        );

        let new_uuid = uuid::Uuid::new_v4().to_string();
        match s3.copy_object(&entry.object_uuid, &new_uuid).await {
            Ok(()) => match s3.delete_object(&entry.object_uuid).await {
                Ok(()) => {
                    let db = app.state::<DbState>();
                    let conn = db.conn()?;
                    db::update_backup_entry_uuid(&conn, entry.id, &new_uuid)?;
                    scrambled += 1;
                    scrambled_entry_ids.push(entry.id);
                }
                Err(_) => {
                    s3.delete_object(&new_uuid).await.ok();
                    failed += 1;
                }
            },
            Err(_) => {
                failed += 1;
            }
        }
    }

    // Invalidate affected share manifests
    if !scrambled_entry_ids.is_empty() {
        let db = app.state::<DbState>();
        let conn = db.conn()?;
        let manifests = db::list_share_manifests(&conn, profile.id)?;
        for manifest in &manifests {
            if !manifest.is_valid {
                continue;
            }
            let entries = db::list_share_manifest_entries(&conn, manifest.id)?;
            if entries.iter().any(|e| scrambled_entry_ids.contains(&e.backup_entry_id)) {
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

    {
        let db = app.state::<DbState>();
        let conn = db.conn()?;
        let pending_names: Vec<String> = local_file_ids
            .iter()
            .map(|lf_id| {
                conn.prepare("SELECT local_path FROM local_file WHERE id = ?1")
                    .and_then(|mut s| s.query_row(rusqlite::params![lf_id], |r| r.get::<_, String>(0)))
                    .map(|p| {
                        p.split('/')
                            .last()
                            .unwrap_or(&p)
                            .to_string()
                    })
                    .unwrap_or_else(|_| lf_id.to_string())
            })
            .collect();
        let _ = app.emit(
            "op:pending_files",
            OpPendingFilesEvent { op_id: op_id.to_string(), files: pending_names },
        );
    }

    for (idx, lf_id) in local_file_ids.iter().enumerate() {
        let (local_path, object_uuid_to_delete) = {
            let db = app.state::<DbState>();
            let conn = db.conn()?;

            let row: Option<(String, Option<i64>)> = conn
                .prepare(
                    "SELECT local_path, backup_entry_id FROM local_file WHERE id = ?1",
                )?
                .query_row(rusqlite::params![lf_id], |row| {
                    Ok((row.get(0)?, row.get(1)?))
                })
                .optional()?;

            let (local_path, be_id) = row.unwrap_or_else(|| (lf_id.to_string(), None));

            let _ = app.emit(
                "cleanup:progress",
                CleanupProgressEvent {
                    op_id: op_id.to_string(),
                    processed: idx,
                    total,
                    current_item: local_path
                        .split('/')
                        .last()
                        .unwrap_or(&local_path)
                        .to_string(),
                    deleted,
                    failed,
                },
            );

            db::delete_local_file(&conn, *lf_id)?;
            deleted += 1;

            let uuid = if let (Some(be_id), true) = (be_id, delete_s3) {
                let remaining: i64 = conn
                    .prepare(
                        "SELECT count(*) FROM local_file WHERE backup_entry_id = ?1",
                    )?
                    .query_row(rusqlite::params![be_id], |row| row.get(0))?;
                if remaining == 0 {
                    let entry = db::get_backup_entry_by_id(&conn, be_id)?;
                    let uuid = entry.map(|e| e.object_uuid.clone());
                    db::delete_backup_entry(&conn, be_id)?;
                    uuid
                } else {
                    None
                }
            } else {
                None
            };

            (local_path, uuid)
        };

        let _ = local_path; // used for progress display above
        if let (Some(ref s3_client), Some(ref uuid)) = (&s3, &object_uuid_to_delete) {
            if s3_client.delete_object(uuid).await.is_err() {
                failed += 1;
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
        .map(|k| k.split('/').last().unwrap_or(k).to_string())
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
                current_item: key.split('/').last().unwrap_or(key).to_string(),
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
            if let Some(e) = db::get_backup_entry_by_id(&conn, *id)? {
                let local_files = db::list_local_files(&conn, e.id)?;
                let filename = local_files
                    .first()
                    .map(|lf| {
                        Path::new(&lf.local_path)
                            .file_name()
                            .map(|f| f.to_string_lossy().to_string())
                            .unwrap_or_else(|| lf.local_path.clone())
                    })
                    .unwrap_or_else(|| e.object_uuid.clone());
                entries.push((e, filename));
            }
        }
        (profile, entries)
    };

    let encryption_key = get_profile_encryption_key(&profile)?;
    let temp_dir = get_temp_dir(&profile);
    let s3 = build_s3_client(&profile).await?;

    let total = entries.len();
    let mut passed = 0usize;
    let mut failed = 0usize;
    let mut errors = 0usize;
    let mut results: Vec<VerifyFileResult> = Vec::new();

    let pending_names: Vec<String> = entries.iter().map(|(_, name)| name.clone()).collect();
    let _ = app.emit(
        "op:pending_files",
        OpPendingFilesEvent { op_id: op_id.to_string(), files: pending_names },
    );

    for (entry, filename) in &entries {
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

        let temp_enc = crypto::generate_temp_path(&temp_dir);

        match s3.download_object(&entry.object_uuid, &temp_enc).await {
            Ok(()) => match crypto::compute_file_md5(&temp_enc) {
                Ok(md5) if md5 == entry.encrypted_md5 => {
                    let temp_dec = crypto::generate_temp_path(&temp_dir);
                    match crypto::decrypt_file(&temp_enc, &temp_dec, &encryption_key) {
                        Ok(()) => {
                            passed += 1;
                            results.push(VerifyFileResult {
                                backup_entry_id: entry.id,
                                filename: filename.clone(),
                                status: "passed".into(),
                                detail: None,
                            });
                        }
                        Err(e) => {
                            errors += 1;
                            results.push(VerifyFileResult {
                                backup_entry_id: entry.id,
                                filename: filename.clone(),
                                status: "error".into(),
                                detail: Some(format!("Decrypt failed: {}", e)),
                            });
                        }
                    }
                    let _ = std::fs::remove_file(&temp_dec);
                }
                Ok(md5) => {
                    failed += 1;
                    results.push(VerifyFileResult {
                        backup_entry_id: entry.id,
                        filename: filename.clone(),
                        status: "failed".into(),
                        detail: Some(format!(
                            "MD5 mismatch: expected {}, got {}",
                            entry.encrypted_md5, md5
                        )),
                    });
                }
                Err(e) => {
                    errors += 1;
                    results.push(VerifyFileResult {
                        backup_entry_id: entry.id,
                        filename: filename.clone(),
                        status: "error".into(),
                        detail: Some(format!("MD5 error: {}", e)),
                    });
                }
            },
            Err(e) => {
                errors += 1;
                results.push(VerifyFileResult {
                    backup_entry_id: entry.id,
                    filename: filename.clone(),
                    status: "error".into(),
                    detail: Some(format!("Download failed: {}", e)),
                });
            }
        }
        let _ = std::fs::remove_file(&temp_enc);
    }

    // Emit the full per-file results so the modal can display them
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
