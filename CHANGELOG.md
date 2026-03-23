# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.2.0] - 2026-03-23

### Added
- Content-addressed chunk storage: files are split into fixed-size chunks (default 10 MiB),
  each encrypted with ChaCha20-Poly1305 and identified by HMAC-SHA256 of the plaintext
- Cross-file deduplication: identical chunk content is stored only once per S3 bucket
- Resume support: interrupted backups automatically skip already-uploaded chunks on retry
- Parallel upload worker pool (4 concurrent PutObject requests) with backpressure
- S3 key prefix support for namespacing objects within a shared bucket
- Configurable chunk size per profile
- Frontend tests for scramble, settings, setup, files, and cleanup pages
- `.nvmrc` pinning Node.js 22

### Changed
- Encryption algorithm changed from AES-256-GCM to ChaCha20-Poly1305
- Chunk identity is now HMAC-SHA256(key, plaintext) — content-addressed and profile-scoped
- Share manifests are encrypted with ChaCha20-Poly1305 and stored at `m/{uuid}` in S3
- Scramble now rotates chunk S3 keys (moves exclusively-owned chunks to new random keys)
  instead of renaming whole-file objects
- Database schema updated to v5 (new tables: `chunk`, `file_entry`, `file_chunk`, `local_file`)

### Removed
- AES-256-GCM and Argon2id dependencies
- Temporary file usage during backup (chunks are encrypted fully in memory)

### Breaking Changes
- **Database schema is incompatible with v0.1.x.** The local database is wiped and
  recreated on first launch. Re-backup all directories after upgrading.
- Existing S3 objects from v0.1.x are not removed automatically and become orphans.

## [0.1.1] - 2026-03-11

### Added
- Initial release with AES-256-GCM file encryption
- S3-compatible backend support (AWS, Backblaze B2, Cloudflare R2, MinIO)
- Backup and restore for files and directories
- Share manifests for sharing files without exposing credentials
- Scramble to invalidate share tokens
- Bandwidth throttling (upload and download)
- Multiple profiles with keychain credential storage
- Local SQLite database with JSON export/import
