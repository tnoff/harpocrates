# Vault App — Implementation Tasks

Reference: `VAULT_APP_PROJECT.md` for full specifications, schema, and user flows.

---

## Phase 1: Project Scaffolding

### Task 1.1: Initialize Tauri project
- Create the `vault/` directory structure per the App Structure section
- Initialize Tauri with Rust backend (`cargo init` inside `src-tauri/`)
- Set up `tauri.conf.json` with app name, window config, and IPC permissions
- Create placeholder `index.html`, `style.css`, `app.js` in `src/`
- Add Rust dependencies to `Cargo.toml`: `rusqlite`, `aes-gcm`, `argon2`, `keyring`, `aws-sdk-s3` (or `rust-s3`), `uuid`, `serde`, `serde_json`, `md-5`, `tokio`
- Verify the app builds and opens a blank window
- **Output**: Skeleton Tauri app that compiles and launches

### Task 1.2: Set up SQLite database module (`db.rs`)
- Implement database initialization: create the SQLite file at the configured `database_path` (default `~/.vault/vault.db`)
- Create all tables from the schema section: `profile`, `backup_entry`, `share_manifest`, `share_manifest_entry`, `local_file`
- Include all constraints: `UNIQUE(s3_endpoint, s3_bucket)` on profile, `UNIQUE(profile_id, object_uuid)` on backup_entry, etc.
- Implement migration/versioning strategy (embed schema version in a `pragma user_version` or metadata table)
- Write helper functions for common queries: insert/update/delete/get for each table
- **Output**: `db.rs` with schema creation and CRUD helpers, tested with unit tests

### Task 1.3: Set up global config file
- On first launch, create `~/.vault/` directory if it doesn't exist
- Create or load global config JSON file with `database_path` setting
- Parse config on startup, pass to database module
- **Output**: App reads/writes `~/.vault/config.json`

---

## Phase 2: Credential Storage & Profiles

### Task 2.1: Implement credential storage (`credentials.rs`)
- Integrate `keyring` crate for OS keychain access
- Implement fallback to AES-encrypted local file if keychain is unavailable
- Functions to store/retrieve/delete:
  - S3 credentials: `vault:<profile-name>:s3-access-key`, `vault:<profile-name>:s3-secret-key`
  - Encryption key: `vault:<profile-name>:encryption-key`
- **Output**: `credentials.rs` with store/retrieve/delete for all credential types

### Task 2.2: Implement profile management (`profiles.rs`)
- Profile CRUD operations using `db.rs` helpers:
  - Create profile (validate unique bucket/endpoint pair, set mode)
  - Read profile (by id, by name, get active)
  - Update profile (edit settings, switch active profile)
  - Delete profile (cascade-delete backup_entries, share_manifests, local_files; remove keychain entries)
- Enforce `UNIQUE(s3_endpoint, s3_bucket)` at the application level with a clear error message
- Enforce only one `is_active = 1` profile at a time
- On create: generate encryption key (random 256-bit) or accept imported key value
- Display key **once** — return it to the frontend for display, then store in keychain only
- **Output**: `profiles.rs` with full CRUD, tested

### Task 2.3: Implement Tauri IPC commands for profiles (`commands.rs` — profiles subset)
- Expose profile CRUD as Tauri commands callable from frontend:
  - `create_profile`, `list_profiles`, `get_active_profile`, `switch_profile`, `update_profile`, `delete_profile`, `test_connection`
- `create_profile` returns the generated encryption key value (one-time display)
- `test_connection` uses S3 client to verify credentials work (e.g., `HeadBucket` or `ListObjectsV2` with max-keys=0)
- **Output**: Tauri commands wired up, callable from JS

---

## Phase 3: Encryption

### Task 3.1: Implement encryption/decryption (`crypto.rs`)
- Implement the encryption format from the spec:
  - `[8 bytes: filesize LE u64][16 bytes: Argon2id salt][12 bytes: nonce][encrypted data + 16-byte GCM tag]`
- `encrypt_file(input_path, output_path, key) -> Result<EncryptedFileMeta>`
  - Read source file, derive key via Argon2id, generate random salt + nonce, encrypt with AES-256-GCM
  - Write to output path in the specified format
  - Return metadata: original MD5, encrypted MD5, original file size
- `decrypt_file(input_path, output_path, key) -> Result<()>`
  - Parse header (filesize, salt, nonce), derive key, decrypt, verify GCM tag
  - Write decrypted data to output path
- Use the profile's `temp_directory` setting for intermediate files (fall back to OS temp dir)
- Delete temp files immediately after each operation
- **Output**: `crypto.rs` with encrypt/decrypt functions, tested with round-trip unit tests

### Task 3.2: Implement temp file cleanup on startup
- On app launch, scan the active profile's `temp_directory` (and OS temp dir) for leftover vault temp files
- Delete any found (use a naming convention like `vault-tmp-*` to identify them)
- **Output**: Startup cleanup logic in `main.rs`

---

## Phase 4: S3 Client

### Task 4.1: Implement S3 client wrapper (`s3.rs`)
- Initialize S3 client from active profile settings (endpoint, region, bucket, access key, secret key, extra env vars)
- Re-initialize when the active profile is switched
- Implement core operations:
  - `upload_object(uuid, file_path) -> Result<()>` — PutObject with MD5 integrity check
  - `download_object(uuid, file_path) -> Result<()>` — GetObject
  - `delete_object(uuid) -> Result<()>` — DeleteObject
  - `copy_object(old_uuid, new_uuid) -> Result<()>` — CopyObject (server-side copy for scramble)
  - `list_objects() -> Result<Vec<S3Object>>` — ListObjectsV2 (for cleanup)
  - `head_bucket() -> Result<()>` — for connection testing
- Apply extra environment variables from profile config before each operation
- **Output**: `s3.rs` with all S3 operations, tested against a real or mocked S3 endpoint

### Task 4.2: Implement multipart upload for large files
- Files above configurable threshold (default 100 MB) use S3 multipart upload
- Split encrypted file into parts, upload each, complete multipart upload
- On failure/crash, abandon the multipart upload (S3 lifecycle rules clean up)
- **Output**: Multipart upload integrated into `upload_object`, transparent to callers

### Task 4.3: Implement bandwidth throttling
- Rate-limited byte stream wrapper around S3 read/write operations
- Configurable upload and download speed limits (stored per-profile or global)
- Adjustable while a transfer is in progress (via Tauri command from frontend)
- **Output**: Throttled stream wrapper used by `s3.rs` upload/download

---

## Phase 5: Backup

### Task 5.1: Implement single file backup
- Tauri command: `backup_file(file_path) -> Result<BackupResult>`
- Flow: read file → compute MD5 → check dedup (existing `backup_entry` with same `original_md5`) → if new: encrypt → upload as UUID → insert `backup_entry` + `local_file` → return result
- If dedup match: just create `local_file` link to existing `backup_entry`
- Apply `relative_path` stripping to store the relative path in `local_file.local_path`
- **Output**: Single file backup working end-to-end

### Task 5.2: Implement directory backup with scanning
- Tauri command: `backup_directory(dir_path, skip_patterns, force_checksum) -> Result<BackupSummary>`
- Recursive directory scan: enumerate all files, skip symlinks, apply regex skip patterns
- For each file, run the change detection fast path:
  - Check `cached_mtime` + `cached_size` in `local_file` table
  - If both match and `backup_entry_id` exists → skip
  - If changed → compute MD5 → dedup check → encrypt + upload if needed
- Update `cached_mtime` and `cached_size` after processing each file
- **Output**: Directory scanning with change detection and dedup

### Task 5.3: Implement database write batching
- Accumulate DB writes (new `backup_entry` + `local_file` inserts, `local_file` updates) in an in-memory buffer
- Commit in batches: every N files (e.g., 50) or every M seconds (e.g., 30), whichever comes first
- Expose a `flush()` function that commits all pending writes (called by Stop button)
- On crash: uncommitted writes are lost — those files will be retried on next run
- **Output**: Batched write layer in `db.rs` or `backup.rs`

### Task 5.4: Implement backup progress reporting and stop
- Send progress events from Rust to frontend via Tauri event system:
  - Overall: files processed, skipped, uploaded, failed, total
  - Per-file: current filename, bytes uploaded, total bytes
- Stop command: set a cancellation flag, wait for current file to finish, flush pending DB writes, halt
- Failed files: log error details, skip, continue with next file
- Return `BackupSummary` at completion with counts and failure details
- **Output**: Real-time progress events + stop/resume behavior

---

## Phase 6: Restore

### Task 6.1: Implement file restore
- Tauri command: `restore_files(backup_entry_ids, target_directory_or_null) -> Result<RestoreSummary>`
- If `target_directory` is null, use `relative_path` mapping to restore to original location
- For each file:
  - Compute target path
  - If file exists at target and MD5 matches `original_md5` → skip
  - If file exists but MD5 differs → download, decrypt, overwrite
  - If file doesn't exist → download from S3 by UUID, decrypt, write to target
- Create parent directories as needed
- Progress events similar to backup (per-file progress, overall counts)
- Stop button: halts after current file, already-restored files are on disk
- Return `RestoreSummary` with counts (restored, skipped, failed)
- **Output**: Full restore with resume support

---

## Phase 7: Share Manifests

### Task 7.1: Implement share manifest creation
- Tauri command: `create_share_manifest(backup_entry_ids, label) -> Result<String>` (returns manifest UUID)
- Build manifest JSON: `{"files": [{"uuid": "...", "filename": "...", "size": ...}, ...]}`
- Encrypt manifest JSON with the profile's key (use same `crypto.rs` encrypt, but for a small in-memory blob rather than a file)
- Upload encrypted manifest as a new UUID to S3
- Insert `share_manifest` row + `share_manifest_entry` rows in DB
- Return the manifest UUID for the user to copy
- **Output**: Manifest creation working end-to-end

### Task 7.2: Implement share manifest receiving (download)
- Tauri command: `receive_manifest(manifest_uuid) -> Result<ManifestFileList>`
- Download the manifest object from S3 by UUID
- Decrypt with the profile's key
- Parse JSON, return file list (uuid, filename, size) to frontend for display
- **Output**: Manifest download + decryption + display

### Task 7.3: Implement file download from manifest
- Tauri command: `download_from_manifest(manifest_uuid, selected_uuids, save_directory) -> Result<DownloadSummary>`
- For each selected file in the manifest:
  - Download from S3 by UUID
  - Decrypt with the profile's key
  - Save to `save_directory/filename`
- No database writes on the recipient side
- Progress events per-file
- **Output**: Full receive flow working

### Task 7.4: Implement share manager (list, revoke)
- Tauri commands:
  - `list_share_manifests() -> Result<Vec<ShareManifestSummary>>` — list all manifests for active profile
  - `get_share_manifest_files(manifest_id) -> Result<Vec<ManifestFile>>` — list files in a manifest
  - `revoke_share_manifest(manifest_id) -> Result<()>` — delete manifest object from S3 + remove DB rows
- **Output**: Share management CRUD

---

## Phase 8: Scramble

### Task 8.1: Implement UUID scramble
- Tauri command: `scramble(backup_entry_ids_or_all) -> Result<ScrambleSummary>`
- For each selected backup entry:
  1. Generate new UUID
  2. `CopyObject(old_uuid → new_uuid)` — server-side copy
  3. `DeleteObject(old_uuid)`
  4. Update `backup_entry.object_uuid` in DB
- For large files, use server-side multipart copy
- After scrambling, find all `share_manifest` entries that reference any scrambled `backup_entry` via `share_manifest_entry` and set `is_valid = 0`
- Progress events per-file
- Return summary: files scrambled, manifests invalidated, failures
- **Output**: Scramble working end-to-end

---

## Phase 9: Cleanup

### Task 9.1: Implement orphaned local entry detection
- Tauri command: `scan_orphaned_local_entries() -> Result<Vec<OrphanedLocalEntry>>`
- Query all `local_file` entries for the active profile (via join to `backup_entry`)
- Check if each `local_path` (expanded with `relative_path`) exists on disk
- Return list of entries where the source file is missing
- **Output**: Orphan detection for local entries

### Task 9.2: Implement orphaned local entry cleanup
- Tauri command: `cleanup_orphaned_local_entries(local_file_ids, delete_s3) -> Result<CleanupSummary>`
- Delete the `local_file` rows
- If `delete_s3` is true and the `backup_entry` has no remaining `local_file` references: delete the S3 object + `backup_entry` row
- Support dry-run flag (return what would be deleted without deleting)
- **Output**: Local orphan cleanup with optional S3 deletion

### Task 9.3: Implement orphaned S3 object detection
- Tauri command: `scan_orphaned_s3_objects() -> Result<Vec<OrphanedS3Object>>`
- `ListObjectsV2` on the active profile's bucket
- Compare each object key against all `backup_entry.object_uuid` and `share_manifest.manifest_uuid` for the active profile
- Return unmatched objects (UUID + size)
- **Output**: S3 orphan detection

### Task 9.4: Implement orphaned S3 object cleanup
- Tauri command: `cleanup_orphaned_s3_objects(object_uuids, dry_run) -> Result<CleanupSummary>`
- Delete selected objects from S3
- Support dry-run flag
- **Output**: S3 orphan cleanup

---

## Phase 10: Integrity Verification

### Task 10.1: Implement integrity verification
- Tauri command: `verify_integrity(backup_entry_ids) -> Result<VerifySummary>`
- For each file:
  1. Download encrypted object from S3
  2. Compute MD5 of downloaded data, compare against `encrypted_md5` in DB
  3. Attempt decryption — GCM auth tag verifies tampering
  4. Record result: passed, failed (MD5 mismatch), error (GCM failure, download error)
- Progress events per-file
- Return summary: passed count, failed count, error count, details
- **Output**: Integrity verification working

---

## Phase 11: Database Export/Import

### Task 11.1: Implement database export to JSON
- Tauri command: `export_database(file_path) -> Result<()>`
- Serialize all tables (profile, backup_entry, share_manifest, share_manifest_entry, local_file) to a JSON structure
- Exclude sensitive data (keychain credentials are NOT included)
- Write to user-selected file path
- **Output**: JSON export of full database

### Task 11.2: Implement database import from JSON
- Tauri command: `import_database(file_path) -> Result<()>`
- Read JSON file, validate structure
- **Replace** entire local database (drop all tables, recreate, insert imported data)
- Show confirmation warning before proceeding: "This will overwrite all existing data"
- After import, user must re-enter keychain credentials for each profile
- **Output**: JSON import with full replacement

---

## Phase 12: Frontend UI

### Task 12.1: Implement app shell and navigation
- Build the main app layout: header with profile switcher dropdown, sidebar/tab navigation
- Implement mode-dependent navigation:
  - Read-write profiles: Files, Share (Create + Manager), Scramble, Cleanup tabs
  - Read-only profiles: Receive tab only
- Settings page accessible from header/sidebar regardless of mode
- Wire up profile switching: dropdown calls `switch_profile`, reloads all views
- **Output**: App shell with navigation, profile switcher, mode-dependent tabs

### Task 12.2: Implement first-time setup / profile creation UI
- If no profiles exist on launch, show setup wizard
- Profile creation form: name, mode (read-write/read-only), S3 fields (endpoint, region, bucket, access key, secret key), extra env vars (key-value pairs), relative_path, temp_directory
- Generate or import encryption key toggle
- On submit: call `create_profile`, display the returned encryption key **once** with copy button and warning: "Save this key securely (e.g., password manager). It will not be shown again."
- Test connection button
- **Output**: Profile creation wizard and settings form

### Task 12.3: Implement Settings page
- Profile management: list all profiles, edit, delete (with confirmation dialog), test connection
- Database path display with reminder to back up
- Export Database button → file picker → calls `export_database` → shows sensitivity warning
- Import Database button → file picker → confirmation dialog → calls `import_database`
- Bandwidth throttling controls (upload/download speed limits)
- **Output**: Full settings page

### Task 12.4: Implement Files view (main view)
- Table showing backed-up files from local DB: filename, path, size, backup date, MD5
- Search/filter bar
- Multi-select support (checkboxes or shift-click)
- Action buttons: Backup File, Backup Directory, Restore, Share, Verify Integrity, Delete
- Backup File: file picker → calls `backup_file`
- Delete: confirmation dialog → delete `backup_entry` + S3 object
- **Output**: File browser with search and action buttons

### Task 12.5: Implement directory backup UI
- Directory backup dialog: path input (with browse button), skip patterns (regex, add/remove), force checksum toggle
- Start button → calls `backup_directory`
- Progress panel:
  - Overall progress: `142 / 1,203 files — 98 skipped, 44 uploaded, 0 failed`
  - Current file: filename + progress bar (bytes uploaded / total)
  - Stop button
- Listen to Tauri progress events, update UI in real-time
- Summary of failures at completion
- **Output**: Directory backup UI with progress and stop

### Task 12.6: Implement Restore UI
- Triggered from Files view: select files → click Restore
- Dialog: restore to original path (default) or pick custom directory
- Progress panel per-file (similar to backup)
- Stop button
- **Output**: Restore dialog with progress

### Task 12.7: Implement Share tab (Create + Receive + Manager)
- **Create share**: pre-populated with selected files from Files view, optional label input, Create button → calls `create_share_manifest` → displays manifest UUID with copy button
- **Receive**: manifest UUID input field, Fetch button → calls `receive_manifest` → displays file list table (filename, size) → select files → pick save directory → Download button → calls `download_from_manifest` → progress per-file
- **Share Manager**: table of manifests (label, file count, created date, status), actions per row: view files, copy UUID, revoke (with confirmation)
- **Output**: Full share UI (create, receive, manage)

### Task 12.8: Implement Scramble UI
- Scope selection: selected files (passed from Files view) or all files in profile
- Preview: calls backend to count affected files and manifests that will be invalidated
- Confirmation dialog: "This will rotate UUIDs for X files and invalidate Y share manifests. Existing share links will stop working."
- Progress panel during scramble
- Summary at completion
- **Output**: Scramble UI with preview and progress

### Task 12.9: Implement Cleanup UI
- Two tabs: Orphaned Local Entries, Orphaned S3 Objects
- **Orphaned Local Entries**: Scan button → calls `scan_orphaned_local_entries` → table of results → checkboxes → "Also delete from S3" toggle → Delete button → confirmation
- **Orphaned S3 Objects**: Scan button → calls `scan_orphaned_s3_objects` → table (UUID, size) → checkboxes → Delete button → confirmation
- Dry-run toggle for both tabs
- Summary of actions taken
- **Output**: Cleanup UI with both orphan types

### Task 12.10: Implement Verify Integrity UI
- Triggered from Files view: select files → click Verify Integrity
- Progress per-file
- Results table: filename, status (passed/failed/error), details
- Summary: `48 passed, 0 failed, 1 error`
- **Output**: Integrity verification UI

---

## Phase 13: Polish & Build

### Task 13.1: Error handling and user feedback
- Consistent error handling across all Tauri commands — return structured errors to frontend
- User-facing error messages (not raw Rust errors) for common failures: S3 connection refused, auth failed, file not found, decryption failed (wrong key), bucket/endpoint uniqueness violation
- Toast/notification system in the frontend for success/error feedback
- **Output**: Consistent error handling throughout

### Task 13.2: Cross-platform build setup
- GitHub Actions CI/CD pipeline for:
  - Windows (.msi)
  - macOS (.dmg)
  - Linux (.AppImage / .deb)
- Tauri build configuration for each platform
- Test keychain integration works on each platform (or fallback to encrypted file)
- **Output**: CI/CD pipeline producing platform builds
