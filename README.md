# Harpocrates

A cross-platform desktop application for encrypted, deduplicated file backup to any S3-compatible object store.

Files are encrypted **before** they leave your machine. The S3 bucket contains only ciphertext. Only someone with your encryption key can read anything stored there.

---

## Features

- **Client-side XChaCha20-Poly1305 encryption** — files are split into fixed-size chunks, each encrypted locally before upload; the server never sees plaintext
- **Any S3-compatible backend** — AWS S3, Backblaze B2, Cloudflare R2, MinIO, or any provider with an S3 API
- **Content-addressed deduplication** — files are split into chunks identified by an HMAC of their content; identical chunks (within a profile) are stored once, enabling both whole-file and sub-file deduplication
- **Change detection and resume** — skips files whose size and modification time haven't changed; a backup interrupted mid-run resumes automatically on the next run (the chunk deduplication check doubles as the resume check — only chunks not yet in S3 are uploaded)
- **Multiple profiles** — separate configurations for different S3 buckets or encryption keys
- **Read-only profiles** — mount a bucket as read-only for safe browsing and restore without risking writes
- **Share manifests** — generate a UUID token that lets another Harpocrates user download specific files without exposing your full bucket
- **Integrity verification** — re-download and re-hash any file to confirm it hasn't been tampered with
- **Scramble** — move selected files' chunks to new random S3 keys; invalidates any share tokens referencing them
- **Bandwidth throttling** — set upload and download limits in KB/s; limits apply to the next chunk during active transfers
- **Orphan cleanup** — find and remove dangling database entries or S3 objects that have lost their counterpart
- **Database export / import** — portable JSON export of the local metadata database
- **Real-time progress** — backup, restore, verify, and scramble all report per-file progress in the persistent status footer

---

## Upgrading from v1

> **Breaking change — existing database is not preserved.**
>
> Version 2 introduced content-addressed chunk storage with a new database schema (v5). On first launch after upgrading, Harpocrates automatically migrates the database by dropping all v1 tables (`backup_entry`, `local_file`, etc.) and creating the new schema. **Your S3 objects are not touched**, but the local file index is wiped — the Files tab will be empty after the upgrade.
>
> To rebuild the index, re-run **Backup Directory** on your folders. Files will be re-uploaded as new content-addressed chunks. Your old v1 S3 objects (stored as bare UUIDs) will appear as orphans in the **Cleanup** tab and can be deleted from there once the re-backup is complete.

---

## User workflow guide

See [docs/workflow.md](docs/workflow.md) for illustrated walkthroughs of the two main use cases:
- Backing up a folder (and restoring from it)
- Sharing files with another person

---

## Security Model

See [SECURITY.md](SECURITY.md) for full details. The short version:

| What is protected | How |
|-------------------|-----|
| File contents | XChaCha20-Poly1305 with a per-chunk random nonce; each chunk is encrypted independently in memory |
| S3 credentials | OS keychain (Keychain on macOS, Credential Manager on Windows, Secret Service on Linux) |
| Encryption key | Held only by you — shown once at profile creation, never stored by Harpocrates |

Your **encryption key** is shown once at profile creation. **It cannot be recovered if lost.** Store it somewhere safe — a password manager, printed paper backup, etc.

---

## Requirements

| Platform | Minimum |
|----------|---------|
| Linux | glibc 2.17+ (Ubuntu 22.04, Fedora 36, etc.) |
| macOS | 10.15 Catalina |
| Windows | 10 (x64) |

An S3-compatible bucket with read/write access is required. The bucket does not need to be public.

---

## Installation

Download the latest release for your platform from the [Releases page](../../releases):

| Platform | File |
|----------|------|
| Linux | `.deb` or `.AppImage` |
| macOS (Apple Silicon) | `_aarch64.dmg` |
| macOS (Intel) | `_x64.dmg` |
| Windows | `_x64_en-US.msi` |

### Linux — AppImage

```bash
chmod +x harpocrates_*.AppImage
./harpocrates_*.AppImage
```

### Linux — .deb

```bash
sudo dpkg -i harpocrates_*.deb
harpocrates
```

---

## First-time Setup

1. Launch Harpocrates. You will be taken to the **Setup** screen.
2. Fill in your S3 connection details:
   - **Profile Name** — a label for this configuration (e.g. "Personal Backups")
   - **S3 Endpoint** — e.g. `https://s3.amazonaws.com` for AWS, or your provider's URL
   - **Region** — e.g. `us-east-1` (leave blank if your provider doesn't require it)
   - **Bucket** — the S3 bucket name
   - **Access Key / Secret Key** — your S3 credentials (must be generated in your provider's IAM console beforehand; Harpocrates cannot create these for you)
   - **Key Prefix** — optional sub-path within the bucket (e.g. `team-alpha`); useful for restricting IAM policies to a specific prefix
3. Click **Create Profile**.
4. **Copy and save your encryption key.** This key is generated fresh and displayed once. Harpocrates does not store it anywhere. If you lose it, your files cannot be decrypted.
5. Click **I've saved my key — Continue**.

---

## Usage

### Backing up files

**Single file:** In the Files tab, click **Upload** and select a file.

**Directory:** Click **Backup Directory**, select a folder, optionally add skip patterns (regular expressions matched against full paths, e.g. `\.log$`), then click **Start Backup**. Progress is tracked in the status footer at the bottom of the window — the modal closes immediately and the backup continues in the background. You can cancel at any time from the footer.

### Restoring files

Select one or more files in the Files tab, then click **Restore**. Choose to restore to original paths or a custom directory.

### Verifying integrity

Select files and click **Verify**. Harpocrates re-downloads and re-hashes each file against the stored checksum. Any mismatch is flagged with details.

### Sharing files

Go to the **Share** tab. Select files first in the **Files** tab, then:

1. **Create** a share manifest — generates a UUID token.
2. Give the UUID to the recipient.
3. The recipient opens the **Receive** tab in their own Harpocrates, pastes the UUID, selects which files to download, and picks a save directory.

> The recipient needs a Harpocrates profile pointing at the same S3 bucket. They do not need your encryption key — the manifest embeds the necessary metadata.

### Scramble

Moves the S3 chunks of selected files (or all files) to new random keys. Only chunks exclusively owned by the selected files are moved — chunks shared with other files are left untouched to preserve deduplication. Any share manifests referencing the scrambled files are invalidated. Use this if you want to revoke access for someone who had your bucket credentials but not your encryption key.

### Cleanup

The **Cleanup** tab finds:
- **Orphaned local entries** — files tracked in the database that no longer exist on disk
- **Orphaned S3 objects** — objects in S3 that have no corresponding database entry

Both tabs support a dry-run mode that shows what would be deleted without making any changes.

---

## S3 Credential Management

**Harpocrates does not create, rotate, or manage S3 credentials.** You are entirely responsible for provisioning access keys and configuring permissions on your storage provider. This includes:

- Creating IAM users, roles, or API tokens with appropriate bucket permissions
- Rotating credentials when needed
- Scoping permissions to the minimum required (see below)
- Revoking credentials if they are compromised

Harpocrates only stores the credentials you provide in the OS keychain and uses them when making S3 API calls. It has no knowledge of your provider's IAM system and cannot automate any part of credential lifecycle management.

### Recommended minimum permissions

For a read-write profile, the access key needs:

```
s3:PutObject
s3:GetObject
s3:DeleteObject
s3:ListBucket
s3:HeadBucket
```

For a read-only profile:

```
s3:GetObject
s3:ListBucket
s3:HeadBucket
```

### Key Prefix and IAM scoping

Each profile can optionally set a **Key Prefix** (e.g. `team-alpha`). All chunk and manifest objects for that profile are then stored under `{prefix}/c/` and `{prefix}/m/` respectively. This lets you scope IAM policies to a sub-path of a shared bucket — for example, granting one set of credentials access only to `team-alpha/*` and another to `team-beta/*`. Harpocrates does not configure these policies; you must set them up in your provider's IAM console.

---

## Data Layout on S3

Files are split into fixed-size chunks. Each chunk is stored as a separate S3 object at a content-addressed key — the key is derived from an HMAC of the chunk's plaintext, so identical chunk content maps to the same object. Share manifests use a separate namespace.

```
s3://your-bucket/
  c/a3f1b2...   (encrypted chunk, key = HMAC of plaintext)
  c/9d4e7c...
  m/550e8400...  (encrypted share manifest)
  ...
```

If a **Key Prefix** is configured in the profile, all objects are stored under it:

```
s3://your-bucket/
  team-alpha/c/a3f1b2...
  team-alpha/c/9d4e7c...
  team-alpha/m/550e8400...
  ...
```

---

## Local Data

| Platform | Path |
|----------|------|
| Linux / macOS | `~/.harpocrates/` |
| Windows | `%USERPROFILE%\.harpocrates\` |

| File | Contents |
|------|----------|
| `harpocrates.db` | SQLite — file index, profiles, share manifests |
| `config.json` | App config (database path) |

S3 credentials are stored in the OS keychain, not in `harpocrates.db`. The encryption key is stored nowhere by Harpocrates.
