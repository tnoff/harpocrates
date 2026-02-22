# Vault

A cross-platform desktop application for encrypted, deduplicated file backup to any S3-compatible object store.

Files are encrypted **before** they leave your machine. The S3 bucket contains only ciphertext. Only someone with your encryption key can read anything stored there.

---

## Features

- **Client-side AES-256-GCM encryption** — files are encrypted locally before upload; the server never sees plaintext
- **Any S3-compatible backend** — AWS S3, Backblaze B2, Cloudflare R2, MinIO, or any provider with an S3 API
- **Content-addressed deduplication** — identical files are stored once regardless of where they live on disk
- **Change detection** — skips files whose size and modification time haven't changed; optionally force a full checksum comparison
- **Multiple profiles** — separate configurations for different S3 buckets or encryption keys
- **Read-only profiles** — mount a bucket as read-only for safe browsing and restore without risking writes
- **Share manifests** — generate a UUID token that lets another Vault user download specific files without exposing your full bucket
- **Integrity verification** — re-download and re-hash any file to confirm it hasn't been tampered with
- **Scramble (re-encrypt)** — assign new random UUIDs to selected files; invalidates any share tokens referencing them
- **Bandwidth throttling** — set upload and download limits in KB/s; limits apply to the next chunk during active transfers
- **Orphan cleanup** — find and remove dangling database entries or S3 objects that have lost their counterpart
- **Database export / import** — portable JSON export of the local metadata database
- **Real-time progress** — backup, restore, verify, and scramble all report per-file progress and running totals

---

## Security Model

See [SECURITY.md](SECURITY.md) for full details. The short version:

| What is protected | How |
|-------------------|-----|
| File contents | AES-256-GCM with a per-file random salt and nonce derived via Argon2id |
| S3 credentials | OS keychain (Keychain on macOS, Credential Manager on Windows, Secret Service on Linux) |
| Encryption key | Held only by you — shown once at profile creation, never stored by Vault |

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
chmod +x vault_*.AppImage
./vault_*.AppImage
```

### Linux — .deb

```bash
sudo dpkg -i vault_*.deb
vault
```

---

## First-time Setup

1. Launch Vault. You will be taken to the **Setup** screen.
2. Fill in your S3 connection details:
   - **Profile Name** — a label for this configuration (e.g. "Personal Backups")
   - **S3 Endpoint** — e.g. `https://s3.amazonaws.com` for AWS, or your provider's URL
   - **Region** — e.g. `us-east-1` (leave blank if your provider doesn't require it)
   - **Bucket** — the S3 bucket name
   - **Access Key / Secret Key** — your S3 credentials
3. Click **Create Profile**.
4. **Copy and save your encryption key.** This key is generated fresh and displayed once. Vault does not store it anywhere. If you lose it, your files cannot be decrypted.
5. Click **I've saved my key — Continue**.

---

## Usage

### Backing up files

**Single file:** In the Files tab, click **Upload** and select a file.

**Directory:** Click **Backup Directory**, select a folder, optionally add skip patterns (regular expressions matched against full paths, e.g. `\.log$`), then click **Start Backup**. A live progress bar shows per-file status with running totals for uploaded, deduped, skipped, and failed files. You can cancel at any time.

### Restoring files

Select one or more files in the Files tab, then click **Restore**. Choose to restore to original paths or a custom directory.

### Verifying integrity

Select files and click **Verify**. Vault re-downloads and re-hashes each file against the stored checksum. Any mismatch is flagged with details.

### Sharing files

Go to the **Share** tab. Select files first in the **Files** tab, then:

1. **Create** a share manifest — generates a UUID token.
2. Give the UUID to the recipient.
3. The recipient opens the **Receive** tab in their own Vault, pastes the UUID, selects which files to download, and picks a save directory.

> The recipient needs a Vault profile pointing at the same S3 bucket. They do not need your encryption key — the manifest embeds the necessary metadata.

### Scramble

Re-assigns new random S3 UUIDs to selected files (or all files). This invalidates any active share manifests that reference those files. Use this if you want to revoke access for someone who had your bucket credentials but not your encryption key.

### Cleanup

The **Cleanup** tab finds:
- **Orphaned local entries** — files tracked in the database that no longer exist on disk
- **Orphaned S3 objects** — objects in S3 that have no corresponding database entry

Both tabs support a dry-run mode that shows what would be deleted without making any changes.

---

## Data Layout on S3

Every file is stored as a randomly generated UUID with no extension or readable metadata.

```
s3://your-bucket/
  550e8400-e29b-41d4-a716-446655440000
  6ba7b810-9dad-11d1-80b4-00c04fd430c8
  ...
```

If you configure a **Relative Path** prefix in your profile, all objects are stored under it:

```
s3://your-bucket/vault/
  vault/550e8400-...
```

---

## Local Data

| Platform | Path |
|----------|------|
| Linux / macOS | `~/.vault/` |
| Windows | `%USERPROFILE%\.vault\` |

| File | Contents |
|------|----------|
| `vault.db` | SQLite — file index, profiles, share manifests |
| `config.json` | App config (database path) |

S3 credentials are stored in the OS keychain, not in `vault.db`. The encryption key is stored nowhere by Vault.
