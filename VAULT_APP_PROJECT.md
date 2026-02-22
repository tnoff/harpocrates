# Project: Vault — Encrypted S3 File Manager

## Overview

A cross-platform desktop app (Tauri + Rust backend, HTML/JS frontend) that provides encrypted file backup and sharing via S3-compatible object storage.

All files are encrypted locally before upload. Object names are obfuscated as UUIDs in the bucket. A local SQLite database maps UUIDs to real filenames, metadata, and encryption info.

## Core Concepts

### Profiles

A profile is a self-contained configuration unit that bundles storage credentials and an encryption key together. Each profile has:

- A user-defined name (e.g., `personal-oci`, `shared-with-john`)
- Its own S3 connection info (endpoint, region, bucket, access key, secret key)
- Its own extra environment variables
- Exactly one encryption key
- Its own `relative_path` and `temp_directory` settings
- A **mode**: either `read-write` or `read-only`

One profile is marked as the active profile. The app operates within the active profile — switching profiles changes the entire context.

Profiles are fully isolated: the encryption key belongs to a single profile, backup entries are scoped to a profile, and there is no cross-profile sharing of keys or data. Each profile must have a unique bucket/endpoint pair — no two profiles can point at the same bucket on the same endpoint. This is enforced at profile creation time and prevents cross-profile conflicts during cleanup, scramble, and other bucket-level operations.

**Profile modes:**
- **Read-write**: Full access. The user can upload, backup, restore, share (create manifests), scramble, and manage files. Used for your own backup profiles and for the "sender" side of sharing.
- **Read-only**: Download only. The user can receive share manifests and download files, but cannot upload, delete, or modify anything in the bucket. Used for the "receiver" side of sharing. The UI hides upload/backup/share/scramble/cleanup actions — only the Receive tab is available.

**Use cases:**
- `personal-oci` (read-write) — your own backup profile
- `shared-with-john` (read-write) — you upload files here and create manifests for John
- `from-tyler` (read-only) — John's profile for receiving files Tyler shares

### Encryption Model

Each profile has exactly **one encryption key**. The key is generated (or imported) when the profile is created. The key value is displayed **once** at creation time — the user is prompted to save it in a password manager or other secure method. After that, the key is stored in the OS keychain and never displayed again.

All files uploaded through a profile are encrypted with that profile's key. There are no key dropdowns or selection — it's always the profile's key.

If you need to share files with someone using a different key, you create a new profile with that key and a separate bucket, then upload the files through that profile.

### Storage Model

- All files stored in an S3-compatible bucket with UUID object names (e.g., `d70a05bc-db77-9d47-67ce-fef4a8ae11b7`)
- No directory hierarchy in the bucket — flat structure
- Real filenames, paths, and metadata tracked only in the local SQLite database
- Recipients don't need the database — they receive a manifest UUID and use their profile's key to decrypt it, which reveals the file list

### Sharing Flow

1. **Sender setup (one-time)**: Create a **read-write** profile for sharing (e.g., `shared-with-john`). The encryption key is shown once at creation; give it to the recipient out of band. Also share the S3 credentials (scoped to read-only access on the bucket if possible).
2. **Recipient setup (one-time)**: Recipient creates a **read-only** profile (e.g., `from-tyler`) with the same S3 connection info and imports the encryption key.
3. **To share files**: Sender switches to the sharing profile, uploads files, selects them, and creates a share manifest. Sends the manifest UUID to the recipient.
4. **Recipient downloads**: Switches to their read-only profile, enters the manifest UUID in the Receive tab, sees the file list, downloads.

The read-write/read-only split ensures the sender is the only one who can modify the bucket. The recipient can only download. This prevents accidental overwrites or state conflicts when two people share a bucket.

## Architecture

### Tech Stack

- **Backend**: Rust (Tauri)
  - S3 client: `aws-sdk-s3` or `rust-s3` crate
  - Encryption: `aes-gcm` crate (AES-256-GCM), `argon2` crate for key derivation
  - Database: `rusqlite` (SQLite)
  - Credential store: OS keychain via `keyring` crate, or encrypted local file
- **Frontend**: Plain HTML/JS/CSS (no framework)
  - Served by Tauri's built-in webview
  - Communicates with Rust backend via Tauri's IPC commands

### App Structure

```
vault/
├── src-tauri/
│   ├── src/
│   │   ├── main.rs              # Tauri entry point
│   │   ├── profiles.rs          # Profile CRUD and switching
│   │   ├── s3.rs                # S3 client wrapper (initialized from active profile)
│   │   ├── crypto.rs            # Encryption/decryption
│   │   ├── db.rs                # SQLite operations
│   │   ├── credentials.rs       # Credential storage (keychain ops keyed by profile name)
│   │   ├── backup.rs            # Backup logic (directory scan, dedup, scheduling)
│   │   └── commands.rs          # Tauri IPC command handlers
│   ├── Cargo.toml
│   └── tauri.conf.json
├── src/
│   ├── index.html               # Main UI
│   ├── style.css
│   └── app.js                   # Frontend logic, Tauri IPC calls
└── README.md
```

## Features

### First-Time Setup

- Create the first profile: prompt for a profile name, S3 credentials (endpoint URL, access key, secret key, bucket name, region), and extra environment variables
- Generate or import the profile's encryption key
- Store all credentials in encrypted local store, scoped to the profile
- This first profile is set as the active/default profile
- Additional profiles can be created anytime via settings

### File Browser / Management

- List all backed-up files from local database (not from S3 — the bucket only has UUIDs)
- Show: original filename, path, size, backup date, MD5
- Search/filter by filename
- Download and decrypt any file using the profile's key

### Backup (Upload)

- Single file upload: select file, encrypt with the profile's key, upload as UUID
- Directory backup: scan directory, skip symlinks, apply regex filters for skipping files

**Path prefix stripping:**
- Configurable `relative_path` in settings (e.g., `/home/user`)
- When backing up `/home/user/Music/album/song.mp3`, the stored path in the database becomes `Music/album/song.mp3`
- On restore, the file is placed relative to the local machine's `relative_path` setting — so a file backed up from `/home/tyler/Music/album/song.mp3` restores to `/home/john/Music/album/song.mp3` on another machine with `relative_path: /home/john`
- Enables directory backup/restore across machines with different home directories or mount points

**Change detection (fast path):**
- For each file, check `cached_mtime` and `cached_size` from the `local_file` table
- If both match AND the file already has a `backup_entry_id` → skip entirely (no MD5, no upload)
- If either changed → proceed to MD5 check and re-upload
- Force checksum flag: bypasses the mtime/size check and always calculates MD5

**Deduplication (MD5-based):**
- Calculate MD5 of source file
- Check local database for existing `backup_entry` with same `original_md5`
- If found: link new file path to existing backup entry (no upload)
- If not found: encrypt and upload as new entry
- Update `cached_mtime` and `cached_size` after processing

**Database write batching:**
- During directory backup, database writes are accumulated in memory rather than written per-file (avoids slamming SQLite IOPS on large backups)
- Pending writes are committed in batches (e.g., every N files or every M seconds, whichever comes first)
- The **Stop/Cancel** button commits all pending transactions before halting — no data is lost
- If the app crashes mid-batch, uncommitted files have no database entry and will be retried on the next run (same behavior as a file that failed mid-upload)

**Directory backup progress and resume:**
- UI shows overall progress: files processed / total files, files skipped (unchanged), files uploaded, files failed
- Per-file progress bar for the currently uploading file (bytes uploaded / total bytes)
- Stop button: commits all pending database writes, then stops processing. Already-recorded files are safe.
- Resume: re-scanning the directory applies change detection — files already in the database with matching mtime/size are skipped, so only remaining files are processed
- Failed files: logged with error details, skipped, and the backup continues with the next file. Summary of failures shown at the end

**Bandwidth throttling:**
- Configurable upload and download speed limits (e.g., 5 MB/s, 50 MB/s, unlimited)
- Set via settings page, adjustable while a transfer is in progress
- Implemented as a rate-limited byte stream wrapper around the S3 read/write operations
- Useful for not saturating the user's internet connection during large directory backups

**Large file uploads (multipart):**
- Files above a configurable threshold (default 100 MB) use S3 multipart upload
- If the app is paused or crashes mid-upload, the incomplete multipart upload is abandoned (S3 will clean it up via lifecycle rules or the next attempt starts fresh)
- The database entry is only created after the upload fully completes, so a partial upload is never recorded as backed up — the next run will retry the file from scratch

### Share

**Creating a share:**
- Select one or more files from the file browser in the active profile and choose "Share"
- Files must already be uploaded in the active profile (encrypted with that profile's key)
- App builds a **share manifest** — a JSON document listing the shared files:
  ```json
  {
    "files": [
      {"uuid": "d70a05bc-...", "filename": "song.mp3", "size": 4821033},
      {"uuid": "a3f1c9e2-...", "filename": "photo.jpg", "size": 2104811}
    ]
  }
  ```
- Manifest is encrypted with the profile's key and uploaded as its own UUID
- App displays the **manifest UUID** for the user to copy and send to the recipient
- The recipient must already have the profile's encryption key (exchanged once during profile setup) and S3 credentials
- Share is recorded in the local database (manifest UUID, label, creation date)

**Managing shares:**
- Share Manager view lists all active share manifests for the active profile
- Each entry shows: label (user-defined or auto-generated), number of files, creation date, status (valid/invalidated)
- Actions per share: view file list, copy manifest UUID, revoke (delete manifest object from S3 + remove from database)
- Revoking a share only deletes the manifest — the underlying files remain in the bucket

### Download (Recipient)

- Recipient switches to their **read-only** profile configured with the shared S3 credentials and encryption key
- Enter manifest UUID in the Receive tab
- App downloads and decrypts the manifest using the profile's key, displays the file list with real filenames and sizes
- Recipient can download all files or select individual files
- Each file is downloaded from S3 by its UUID, decrypted, and saved with its original filename
- Save to user-selected directory
- **No local database entries are created** — the recipient's DB does not track backup_entries for downloaded files. The files are simply written to disk. This keeps the sender as the sole owner of the S3 state and avoids sync conflicts.

### Scramble (UUID Rotation)

Invalidates all previously shared UUIDs by renaming objects in S3 server-side. Useful if a share manifest or UUID is leaked, or as a precautionary measure after a security concern.

**Flow:**
- User selects files (or "scramble all") from the file browser
- For each selected file:
  1. Generate a new UUID
  2. S3 `CopyObject(old-uuid → new-uuid)` — server-side copy, no data leaves S3
  3. S3 `DeleteObject(old-uuid)` — remove the old key
  4. Update `backup_entry.object_uuid` in the database
- For large files (above multipart threshold), uses server-side multipart copy
- Any existing share manifests referencing old UUIDs become stale — recipients will get "not found" errors
- Active share manifests in the database that referenced scrambled files are marked as invalidated
- Progress shown in the UI (similar to backup progress — files processed / total)

**Scope options:**
- Scramble selected files only
- Scramble all files in the active profile

### Restore (Owner)

- Select files or directory subtree from local database file list
- Download from S3 using stored UUID
- Decrypt with the profile's key
- Save to original path (using `relative_path` mapping) or user-selected path

**Restore resume:**
- Before downloading each file, check if the local file already exists at the target path
- If the file exists and its MD5 matches the `original_md5` in the database → skip (already restored)
- If the file exists but MD5 differs → re-download and overwrite (local file is stale or corrupted)
- If the file does not exist → download and decrypt as normal
- Stop button works the same as backup — already-restored files are on disk, unrestored files will be picked up on the next run

### Cleanup

**Orphaned local entries** (local file deleted, DB entry remains):
- List `local_file` entries where the source file no longer exists on disk
- Option to unlink the local_file entry from its backup_entry
- If a backup_entry has no remaining local_file references, offer to delete the S3 object too

**Orphaned S3 objects** (S3 object exists, no DB reference):
- Lists all objects in the profile's bucket via `ListObjectsV2`
- Compares against all `backup_entry.object_uuid` and `share_manifest.manifest_uuid` in the DB for the active profile
- Any S3 object with no matching DB entry is flagged as orphaned (e.g., from a crash mid-upload, a scramble that deleted the old DB entry but failed to delete the old S3 object, etc.)
- User selects which orphaned objects to delete

**Common to both:**
- Dry-run toggle (show what would be deleted without deleting)
- Confirmation before any deletions
- Summary of actions taken

## Database Schema (SQLite)

```sql
-- Profiles: self-contained storage + encryption configurations
-- Each profile has exactly one encryption key
CREATE TABLE profile (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    name TEXT UNIQUE NOT NULL,                 -- User-defined name (e.g., 'personal-oci', 'shared-with-john')
    mode TEXT NOT NULL DEFAULT 'read-write',   -- 'read-write' or 'read-only'
    s3_endpoint TEXT NOT NULL,
    s3_region TEXT,
    s3_bucket TEXT NOT NULL,
    extra_env TEXT,                            -- JSON object of extra environment variables
    relative_path TEXT,                        -- Path prefix stripping base (e.g., '/home/user')
    temp_directory TEXT,                       -- Scratch dir for encrypt/decrypt temp files
    is_active BOOLEAN NOT NULL DEFAULT 0,     -- Only one profile active at a time
    created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
    UNIQUE(s3_endpoint, s3_bucket)            -- Each profile must target a distinct bucket/endpoint pair
);
-- S3 credentials stored in OS keychain as vault:<profile-name>:s3-access-key / s3-secret-key
-- Encryption key stored in OS keychain as vault:<profile-name>:encryption-key

-- Unique encrypted objects in S3, scoped to a profile
CREATE TABLE backup_entry (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    profile_id INTEGER NOT NULL REFERENCES profile(id),
    object_uuid TEXT NOT NULL,                 -- UUID used as S3 object key
    original_md5 TEXT NOT NULL,                -- MD5 of original unencrypted file
    encrypted_md5 TEXT NOT NULL,               -- MD5 of encrypted file
    file_size INTEGER NOT NULL,               -- Original file size in bytes
    created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
    UNIQUE(profile_id, object_uuid)           -- UUIDs unique within a profile
);

-- Share manifests: encrypted JSON objects in S3 that list files for sharing
CREATE TABLE share_manifest (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    profile_id INTEGER NOT NULL REFERENCES profile(id),
    manifest_uuid TEXT NOT NULL,              -- UUID of the manifest object in S3
    label TEXT,                               -- User-defined label (e.g., 'photos for john', 'march playlist')
    file_count INTEGER NOT NULL,             -- Number of files in the manifest
    is_valid BOOLEAN NOT NULL DEFAULT 1,     -- Set to 0 when scramble invalidates referenced UUIDs
    created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
    UNIQUE(profile_id, manifest_uuid)
);

-- Junction table: which backup entries are included in which manifests
CREATE TABLE share_manifest_entry (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    share_manifest_id INTEGER NOT NULL REFERENCES share_manifest(id),
    backup_entry_id INTEGER NOT NULL REFERENCES backup_entry(id),
    filename TEXT NOT NULL                   -- Original filename (stored here so manifest can be rebuilt)
);

-- Local file paths that map to backup entries (many-to-one for dedup)
CREATE TABLE local_file (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    backup_entry_id INTEGER NOT NULL REFERENCES backup_entry(id),
    local_path TEXT NOT NULL,                  -- Relative file path
    cached_mtime REAL,                         -- Last known modification time
    cached_size INTEGER,                       -- Last known file size
    updated_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
    UNIQUE(backup_entry_id, local_path)       -- Same path can exist in different profiles via different backup entries
);
```

## Credential Storage

- First-time: user creates their first profile (S3 creds + encryption key)
- Additional profiles can be created anytime via settings
- Sensitive values stored using OS keychain (`keyring` crate) where available, fallback to AES-encrypted local file:
  - S3 credentials: `vault:<profile-name>:s3-access-key` / `vault:<profile-name>:s3-secret-key`
  - Encryption key: `vault:<profile-name>:encryption-key`
- Encryption key is shown **once** at profile creation with a warning to save it externally (password manager, etc.). It is never displayed again in the app.
- Profile metadata and non-sensitive settings stored in SQLite (`profile` table)
- Global config file (unencrypted) for app-level settings:
  ```json
  {
    "database_path": "~/.vault/vault.db"
  }
  ```

## Encryption Format

**AES-256-GCM**

```
[8 bytes: original filesize (little-endian uint64)]
[16 bytes: Argon2id salt]
[12 bytes: random nonce]
[encrypted data + 16-byte GCM auth tag]
```

AES-GCM provides both encryption and authentication — if the encrypted file is tampered with in the bucket, decryption will fail with an auth error rather than silently producing garbage. Keys derived from passphrases using Argon2id (arbitrary-length passphrases supported).

## Security Considerations

- **Temp directory**: User-configurable via settings (`temp_directory`). Defaults to OS temp directory if not set. Useful for directing temp file I/O to a spinning disk instead of wearing out an SSD with large media file encryption/decryption. All encrypt/decrypt operations write temp files here.
- **Temp file cleanup**: On startup, purge any leftover temp files in the configured temp directory. Delete temp files immediately after each successful encrypt/decrypt operation.
- **Database is critical**: If the SQLite database is lost, UUID-to-filename mappings are gone. The encrypted files remain in S3 but are unidentifiable. See the Database Backup section for the backup strategy.
- **S3 credentials scope**: Credentials distributed to recipients should be scoped to read-only on the sharing bucket. Since each profile has its own bucket, a recipient's read-only credentials only grant access to the shared bucket — not the sender's personal backup bucket.
- **Encryption key display**: The encryption key for a profile is shown exactly once at profile creation time. The user is warned to save it in a password manager or other secure method. The app never displays the key again — it remains in the OS keychain for operational use only.
- **Key rotation**: No built-in re-encryption with a new key. If a key is compromised, the user would need to create a new profile with a new key, re-upload affected files, and delete the old profile's data. Could be streamlined in a future version.
- **UUID scramble**: If a share manifest or individual UUIDs are leaked, the scramble feature rotates object keys server-side via S3 `CopyObject` + `DeleteObject`. This invalidates all previously shared references without re-encrypting. Note: scramble does not protect against someone who already downloaded the files — it only prevents future downloads using old UUIDs. If the encryption key itself is compromised, scramble alone is insufficient — the files need re-encryption with a new key (i.e., a new profile).

## S3 Compatibility

- Use standard S3 API (PutObject, GetObject, DeleteObject, ListObjectsV2)
- Endpoint URL is user-configurable (works with AWS, OCI, MinIO, Backblaze, Wasabi, etc.)
- Extra environment variables passable via config for backends that need them
- Multipart upload for large files (configurable threshold, default 100 MB)
- MD5 integrity check on upload

## UI Pages

### 1. Setup / Settings

**Profile switcher** (visible in the app header/sidebar at all times):
- Dropdown of all profiles, showing the active profile
- Switching profiles changes the entire app context (file list, keys, storage target)

**Profile management:**
- Create new profile: name, mode (read-write or read-only), S3 connection fields, extra env vars, relative_path, temp_directory
- Import encryption key (paste value) or generate a new one. Key shown once with warning to save it externally.
- Edit existing profile
- Delete profile (with confirmation — warns that all backup entries for this profile become orphaned)
- Test connection button (per profile)
- Set active profile

**Mode-dependent UI:**
- Read-write profiles show all tabs: Files, Restore, Share (Create + Manager), Scramble, Cleanup
- Read-only profiles show only: Receive tab (enter manifest UUID, browse files, download)

**Global settings:**
- Database path

### 2. Files (main view)
- Table of backed-up files: name, path, size, date, status
- Search bar
- Buttons: Backup File, Backup Directory, Restore, Share, Verify Integrity, Delete
- Directory backup: path input, skip patterns (regex), force checksum toggle, start/stop button
- Progress panel (visible during directory backup):
  - Overall: `142 / 1,203 files — 98 skipped, 44 uploaded, 0 failed`
  - Current file: filename + progress bar (bytes)
  - Stop button (commits pending DB writes, then halts)
  - Summary of failures at completion

### 3. Share
- **Create share tab**: select files (single or multi-select from file browser), optional label, create share manifest
  - Displays manifest UUID (with copy button)
  - All files encrypted with the active profile's key — recipient must already have this key
- **Receive tab**: enter manifest UUID, app fetches and decrypts manifest using the profile's key, shows file list with filenames and sizes, select save directory, download + decrypt all or individual files
- **Share Manager tab**: list of all active share manifests for the active profile
  - Columns: label, file count, created date, status (valid/invalidated)
  - Actions: view file list, copy manifest UUID, revoke (deletes manifest from S3)
  - Invalidated manifests (from scramble) shown with a visual indicator and option to delete from database
- Share button on files in the Files view: opens share creation popup pre-populated with selected files

### 4. Scramble
- Select scope: selected files or all files in the profile
- Preview: shows count of files to be scrambled and number of share manifests that will be invalidated
- Confirmation dialog with warning about breaking existing shares
- Progress panel during scramble operation
- Summary at completion: files scrambled, manifests invalidated, any failures

### 5. Cleanup
- **Orphaned local entries tab**: local_file entries where source is gone. Option to unlink from DB and optionally delete from S3.
- **Orphaned S3 objects tab**: S3 objects with no matching DB entry. Lists object UUID and size. Checkboxes to select for deletion.
- Dry-run toggle (both tabs)
- Delete button with confirmation

## Integrity Verification

- Accessible from the Files view: select files and click "Verify Integrity"
- Downloads the encrypted file from S3, verifies `encrypted_md5` matches the database
- Decryption itself also verifies integrity via the GCM auth tag
- Reports any mismatches (corrupted or tampered files in S3)
- Progress shown per-file; results summarized at completion (passed / failed / errors)

## Database Backup

- The SQLite database is a critical file — if lost, UUID-to-filename mappings are gone and the bucket becomes a pile of unidentifiable encrypted blobs
- The app does **not** auto-backup the database to S3
- **Export**: Settings page has an "Export Database" button that writes the database contents to a JSON file at a user-selected path. A warning is shown: "This file contains sensitive metadata including filenames, file paths, and storage configuration. Store it securely."
- **Import**: Settings page has an "Import Database" button that reads a JSON export and **replaces** the local database entirely. Used for migrating to a new machine or recovering from a lost database. A confirmation dialog warns that this will overwrite all existing data.
- The app shows a reminder in settings with the current database file path, encouraging the user to back it up regularly

## User Flows

### Flow 1: First-time setup

1. User opens the app for the first time — no profiles exist.
2. Prompted to create a profile: name (e.g., `personal`), mode (`read-write`), S3 endpoint, region, bucket, access key, secret key, any extra env vars.
3. App generates an encryption key.
4. Key is displayed **once** with a warning: "Save this key securely (e.g., password manager). It will not be shown again."
5. User copies it, stores it externally.
6. Profile saved as active. App opens to an empty Files view.

### Flow 2: Backup a directory

1. Active profile is `personal` (read-write). User clicks "Backup Directory."
2. Enters/browses to a path (e.g., `/home/tyler/Music`), optionally sets skip patterns (regex).
3. App scans the directory. For each file:
   - **Fast path**: checks `cached_mtime` + `cached_size` from `local_file` table. If both match and a `backup_entry` exists → skip entirely.
   - **Changed file**: mtime or size differs → calculate MD5 → check for dedup → encrypt + upload if new, or link to existing entry if MD5 matches.
4. DB writes are batched in memory, committed every N files or M seconds.
5. Progress panel shows: `142 / 1,203 files — 98 skipped, 44 uploaded, 0 failed`
6. Per-file progress bar for the current upload (bytes uploaded / total).
7. Failures are logged and skipped; backup continues with the next file.

**If interrupted (Stop button or crash):**
- **Stop button**: commits all pending DB writes, then halts. Every file that was uploaded + committed is safe.
- **Crash**: uncommitted files have no DB entry. The uploaded-but-uncommitted S3 objects are orphaned (cleaned up via Cleanup → Orphaned S3 Objects).
- **To resume**: run the same directory backup again. Change detection skips everything already in the DB. Files that were mid-upload or uncommitted are retried from scratch.

### Flow 3: Restore files to local disk

1. Active profile is `personal` (read-write). User goes to Files view.
2. Browses/searches for files, selects files or a directory subtree, clicks "Restore."
3. Chooses: restore to original path (using `relative_path` mapping) or pick a custom directory.
4. For each file:
   - Check if local file already exists at the target path with matching MD5 → **skip** (already restored).
   - Exists but MD5 differs → re-download and overwrite.
   - Doesn't exist → download from S3 by UUID, decrypt, write to disk.
5. Progress shown per-file, similar to backup.

**If interrupted:**
- Stop button or crash — already-restored files are on disk.
- **To resume**: run the same restore again. Files already on disk with correct MD5 are skipped. Partial/missing files are re-downloaded.

### Flow 4: Set up sharing with John (one-time, Tyler's side)

1. Tyler creates a new profile: name `shared-with-john`, mode **read-write**.
2. S3 connection: a **separate bucket** (e.g., `tyler-john-shared`) — each profile requires a distinct bucket/endpoint pair. Tyler sets up S3 creds with write access.
3. App generates (or Tyler imports) an encryption key. Shown once.
4. Tyler gives John **out of band**: the encryption key + S3 credentials (endpoint, region, bucket, access key, secret key). The S3 creds given to John should be scoped to read-only access.

### Flow 5: Set up sharing with Tyler (one-time, John's side)

1. John creates a profile: name `from-tyler`, mode **read-only**.
2. Enters the S3 connection info Tyler gave him (read-only creds).
3. Imports the encryption key Tyler gave him (paste value).
4. Key is stored in keychain, never shown again.
5. Profile saved. Since it's read-only, the UI shows only the Receive tab.

### Flow 6: Share files (Tyler sends to John)

1. Tyler switches to `shared-with-john` profile (read-write).
2. Uploads the files he wants to share — "Backup File" or "Backup Directory." Files are encrypted with this profile's key and uploaded as UUIDs.
3. Goes to Files view, selects the files, clicks "Share."
4. Optionally adds a label (e.g., "march playlist").
5. App creates a share manifest (encrypted JSON listing UUIDs + filenames), uploads it as its own UUID.
6. App displays the **manifest UUID**. Tyler copies it.
7. Tyler sends the manifest UUID to John (text, email, etc.).

### Flow 7: Receive files (John downloads)

1. John switches to `from-tyler` profile (read-only) — UI shows only the Receive tab.
2. Enters the manifest UUID Tyler sent him.
3. App downloads the manifest from S3, decrypts it with the profile's key.
4. Displays the file list: `song.mp3 (4.8 MB)`, `photo.jpg (2.1 MB)`.
5. John selects files (or all), picks a save directory.
6. Files downloaded by UUID, decrypted, saved with original filenames.
7. **Nothing written to John's database** — files just go to disk. No sync conflicts.

### Flow 8: Ongoing sharing (after initial setup)

1. Tyler switches to `shared-with-john`, uploads new files, creates a new manifest.
2. Sends John the manifest UUID.
3. John switches to `from-tyler`, enters UUID, downloads.
4. No key exchange, no profile setup — just manifest UUIDs going back and forth.

### Flow 9: Revoke access / respond to a leak

1. Tyler suspects a manifest UUID or the bucket was exposed to someone unintended.
2. **Revoke specific manifest**: Share Manager → revoke → deletes manifest from S3. Anyone with that manifest UUID gets "not found."
3. **Scramble UUIDs** (if worried about direct UUID access): selects affected files or "all files in profile" → scramble rotates UUIDs server-side → old references are dead.
4. Tyler creates a new manifest with the new UUIDs, sends it to John.
5. **If the encryption key itself is compromised**: Tyler needs to create an entirely new profile with a new key and a new bucket, re-upload everything, and share the new key with John. The old profile's data is considered exposed.

### Flow 10: Export/import database

1. User goes to Settings → "Export Database" → picks a save location.
2. App writes a JSON export with a warning: "This file contains sensitive metadata including filenames, file paths, and storage configuration. Store it securely."
3. On a new machine: Settings → "Import Database" → select the JSON file → confirmation that this replaces all existing data → database is restored.
4. User still needs to re-enter keychain credentials (S3 keys, encryption keys) since those don't travel with the JSON export.

### Flow 11: Verify file integrity

1. User goes to Files view, selects one or more files, clicks "Verify Integrity."
2. For each file, app downloads the encrypted object from S3 and compares its MD5 against `encrypted_md5` in the database.
3. If MD5 matches, app also attempts decryption — the GCM auth tag verifies the file hasn't been tampered with.
4. Results shown per-file: passed, failed (MD5 mismatch), or error (GCM auth failure, download error).
5. Summary at completion: `48 passed, 0 failed, 1 error`.

### Flow 12: Cleanup orphaned objects

1. User goes to Cleanup tab (read-write profiles only).
2. **Orphaned local entries**: app scans `local_file` entries and checks if source files still exist on disk. Missing files are listed. User can unlink from DB and optionally delete from S3 if no other local_file entries reference that backup_entry.
3. **Orphaned S3 objects**: app lists all objects in the bucket via `ListObjectsV2`, compares against all `backup_entry` and `share_manifest` UUIDs in the DB. Unrecognized objects are listed with their size. User selects which to delete.
4. Dry-run toggle available for both — preview what would be deleted without deleting.
5. Confirmation before any deletions. Summary of actions taken.

## Build / Distribution

- Tauri builds for Windows (.msi), macOS (.dmg), Linux (.AppImage / .deb)
- GitHub Actions CI/CD for cross-platform builds
- Single binary + webview, no runtime dependencies
