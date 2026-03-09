/// Integration tests that exercise multiple modules together.
///
/// These tests use real files (via `tempfile`), real SQLite databases, and the actual
/// crypto primitives — but they do NOT touch the OS keychain or S3. Tests that
/// require credentials or network access belong in a separate test harness with
/// a live MinIO instance.
#[cfg(test)]
mod tests {
    use std::io::Write;

    use rusqlite::{Connection, OptionalExtension};
    use tempfile::{NamedTempFile, TempDir};

    use crate::backup;
    use crate::crypto;
    use crate::db;

    // ── DB helpers ────────────────────────────────────────────────────────────

    /// Open an in-memory SQLite connection with the full vault schema.
    fn open_db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
        // Reuse init_database's schema creation by opening a file then closing it,
        // or just inline the schema. We inline to stay self-contained.
        conn.execute_batch(
            "
            CREATE TABLE profile (
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
            CREATE TABLE backup_entry (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                profile_id INTEGER NOT NULL REFERENCES profile(id),
                object_uuid TEXT NOT NULL,
                original_md5 TEXT NOT NULL,
                encrypted_md5 TEXT NOT NULL,
                file_size INTEGER NOT NULL,
                created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
                UNIQUE(profile_id, object_uuid)
            );
            CREATE TABLE share_manifest (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                profile_id INTEGER NOT NULL REFERENCES profile(id),
                manifest_uuid TEXT NOT NULL,
                label TEXT,
                file_count INTEGER NOT NULL,
                is_valid BOOLEAN NOT NULL DEFAULT 1,
                created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
                UNIQUE(profile_id, manifest_uuid)
            );
            CREATE TABLE share_manifest_entry (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                share_manifest_id INTEGER NOT NULL REFERENCES share_manifest(id),
                backup_entry_id INTEGER NOT NULL REFERENCES backup_entry(id),
                filename TEXT NOT NULL
            );
            CREATE TABLE local_file (
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

    /// Insert a minimal profile and return its id.
    fn insert_test_profile(conn: &Connection, name: &str) -> i64 {
        db::insert_profile(
            conn,
            name,
            "read-write",
            &format!("https://s3.example.com/{}", name),
            None,
            "test-bucket",
            None,
            None,
            None,
            None,
        )
        .unwrap()
    }

    // ── 1. Database schema ────────────────────────────────────────────────────

    #[test]
    fn test_db_init_creates_schema_on_disk() {
        let tmp = NamedTempFile::new().unwrap();
        let path = tmp.path().to_str().unwrap().to_string();
        let conn = db::init_database(&path).unwrap();

        // Verify each expected table exists
        for table in &["profile", "backup_entry", "local_file", "share_manifest", "share_manifest_entry"] {
            let count: i64 = conn
                .query_row(
                    "SELECT count(*) FROM sqlite_master WHERE type='table' AND name=?1",
                    rusqlite::params![table],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(count, 1, "table '{}' should exist", table);
        }
    }

    #[test]
    fn test_db_init_is_idempotent() {
        // Opening the same file twice should not fail (schema already exists).
        let tmp = NamedTempFile::new().unwrap();
        let path = tmp.path().to_str().unwrap().to_string();
        db::init_database(&path).unwrap();
        db::init_database(&path).unwrap(); // second open — must not error
    }

    // ── 2. Profile CRUD ───────────────────────────────────────────────────────

    #[test]
    fn test_profile_insert_and_list() {
        let conn = open_db();
        let id = insert_test_profile(&conn, "alpha");
        let profiles = db::list_profiles(&conn).unwrap();
        assert_eq!(profiles.len(), 1);
        assert_eq!(profiles[0].id, id);
        assert_eq!(profiles[0].name, "alpha");
        assert_eq!(profiles[0].mode, "read-write");
    }

    #[test]
    fn test_profile_set_active() {
        let conn = open_db();
        let id1 = insert_test_profile(&conn, "p1");
        let id2 = insert_test_profile(&conn, "p2");

        db::set_active_profile(&conn, id1).unwrap();
        let p1 = db::get_profile_by_id(&conn, id1).unwrap().unwrap();
        let p2 = db::get_profile_by_id(&conn, id2).unwrap().unwrap();
        assert!(p1.is_active);
        assert!(!p2.is_active);

        // Switch to p2 — p1 should become inactive
        db::set_active_profile(&conn, id2).unwrap();
        let p1 = db::get_profile_by_id(&conn, id1).unwrap().unwrap();
        let p2 = db::get_profile_by_id(&conn, id2).unwrap().unwrap();
        assert!(!p1.is_active);
        assert!(p2.is_active);
    }

    #[test]
    fn test_profile_delete_requires_child_cleanup_first() {
        // db::delete_profile is a single-row delete; the caller (commands layer) is
        // responsible for deleting backup_entries and local_files first.
        // This test verifies that pattern works end-to-end.
        let conn = open_db();
        let pid = insert_test_profile(&conn, "doomed");

        let entry_id = db::insert_backup_entry(
            &conn, pid, "some-uuid", "aabbcc", "ddeeff", 42,
        )
        .unwrap();
        let lf_id = db::insert_local_file(&conn, entry_id, "/tmp/file.txt", None, Some(42)).unwrap();

        assert_eq!(db::list_backup_entries(&conn, pid).unwrap().len(), 1);

        // Correct deletion order: local_file → backup_entry → profile
        db::delete_local_file(&conn, lf_id).unwrap();
        db::delete_backup_entry(&conn, entry_id).unwrap();
        db::delete_profile(&conn, pid).unwrap();

        assert!(db::get_profile_by_id(&conn, pid).unwrap().is_none());
        assert!(db::list_backup_entries(&conn, pid).unwrap().is_empty());
    }

    // ── 3. BackupEntry + LocalFile round-trip ─────────────────────────────────

    #[test]
    fn test_backup_entry_and_local_file_round_trip() {
        let conn = open_db();
        let pid = insert_test_profile(&conn, "rw");

        let entry_id = db::insert_backup_entry(
            &conn, pid, "uuid-abc", "md5-plain", "md5-enc", 1024,
        )
        .unwrap();

        let lf_id = db::insert_local_file(
            &conn, entry_id, "/home/user/file.txt", Some(1_700_000_000.0), Some(1024),
        )
        .unwrap();
        assert!(lf_id > 0);

        let entries = db::list_backup_entries(&conn, pid).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].object_uuid, "uuid-abc");
        assert_eq!(entries[0].original_md5, "md5-plain");
        assert_eq!(entries[0].file_size, 1024);

        let local_files = db::list_local_files(&conn, entry_id).unwrap();
        assert_eq!(local_files.len(), 1);
        assert_eq!(local_files[0].local_path, "/home/user/file.txt");
        assert!((local_files[0].cached_mtime.unwrap() - 1_700_000_000.0).abs() < 0.001);
    }

    #[test]
    fn test_dedup_same_md5_reuses_entry() {
        let conn = open_db();
        let pid = insert_test_profile(&conn, "dedup");
        let md5 = "deadbeef01234567";

        // First file
        let entry_id = db::insert_backup_entry(&conn, pid, "uuid-1", md5, "enc-1", 100).unwrap();
        db::insert_local_file(&conn, entry_id, "/home/a.txt", None, None).unwrap();

        // Simulate the dedup lookup used in commands.rs
        let found: Option<db::BackupEntry> = {
            let mut stmt = conn
                .prepare(
                    "SELECT id, profile_id, object_uuid, original_md5, encrypted_md5, file_size, created_at
                     FROM backup_entry WHERE profile_id = ?1 AND original_md5 = ?2 LIMIT 1",
                )
                .unwrap();
            stmt.query_row(rusqlite::params![pid, md5], |row| {
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
            .optional()
            .unwrap()
        };

        assert!(found.is_some(), "dedup lookup should find existing entry");
        assert_eq!(found.unwrap().id, entry_id);

        // Attach a second local path to the same entry (dedup)
        db::insert_local_file(&conn, entry_id, "/home/b.txt", None, None).unwrap();
        let lfs = db::list_local_files(&conn, entry_id).unwrap();
        assert_eq!(lfs.len(), 2);
    }

    #[test]
    fn test_backup_entry_delete_after_local_file_cleanup() {
        // Must delete local_file rows before deleting backup_entry (FK constraint).
        let conn = open_db();
        let pid = insert_test_profile(&conn, "del");
        let entry_id = db::insert_backup_entry(&conn, pid, "uuid-del", "md5", "enc", 10).unwrap();
        let lf_id = db::insert_local_file(&conn, entry_id, "/tmp/x.txt", None, None).unwrap();

        db::delete_local_file(&conn, lf_id).unwrap();
        db::delete_backup_entry(&conn, entry_id).unwrap();

        let entries = db::list_backup_entries(&conn, pid).unwrap();
        assert!(entries.is_empty());
    }

    // ── 4. Share manifest lifecycle ───────────────────────────────────────────

    #[test]
    fn test_share_manifest_lifecycle() {
        let conn = open_db();
        let pid = insert_test_profile(&conn, "sharer");
        let entry_id = db::insert_backup_entry(&conn, pid, "uuid-sh", "md5", "enc", 50).unwrap();

        // Create manifest
        let manifest_id =
            db::insert_share_manifest(&conn, pid, "mfst-uuid-1", Some("vacation pics"), 1).unwrap();
        db::insert_share_manifest_entry(&conn, manifest_id, entry_id, "photo.jpg").unwrap();

        // List
        let manifests = db::list_share_manifests(&conn, pid).unwrap();
        assert_eq!(manifests.len(), 1);
        assert_eq!(manifests[0].manifest_uuid, "mfst-uuid-1");
        assert_eq!(manifests[0].label.as_deref(), Some("vacation pics"));
        assert!(manifests[0].is_valid);

        // Entries
        let entries = db::list_share_manifest_entries(&conn, manifest_id).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].filename, "photo.jpg");
        assert_eq!(entries[0].backup_entry_id, entry_id);

        // Invalidate
        db::invalidate_share_manifest(&conn, manifest_id).unwrap();
        let m = db::get_share_manifest_by_id(&conn, manifest_id).unwrap().unwrap();
        assert!(!m.is_valid);

        // Delete
        db::delete_share_manifest(&conn, manifest_id).unwrap();
        let manifests_after = db::list_share_manifests(&conn, pid).unwrap();
        assert!(manifests_after.is_empty());
        let entries_after = db::list_share_manifest_entries(&conn, manifest_id).unwrap();
        assert!(entries_after.is_empty());
    }

    // ── 5. Crypto end-to-end ──────────────────────────────────────────────────

    const TEST_KEY: &str =
        "0102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f20";

    fn write_temp_file(content: &[u8]) -> NamedTempFile {
        let mut f = NamedTempFile::new().unwrap();
        f.write_all(content).unwrap();
        f.flush().unwrap();
        f
    }

    #[test]
    fn test_encrypt_decrypt_round_trip_small_file() {
        let plain = b"Hello, Vault! This is a small plaintext message.";
        let src = write_temp_file(plain);
        let encrypted = NamedTempFile::new().unwrap();
        let decrypted = NamedTempFile::new().unwrap();

        crypto::encrypt_file(src.path(), encrypted.path(), TEST_KEY).unwrap();
        crypto::decrypt_file(encrypted.path(), decrypted.path(), TEST_KEY).unwrap();

        let recovered = std::fs::read(decrypted.path()).unwrap();
        assert_eq!(recovered, plain);
    }

    #[test]
    fn test_encrypt_decrypt_round_trip_empty_file() {
        let src = write_temp_file(b"");
        let encrypted = NamedTempFile::new().unwrap();
        let decrypted = NamedTempFile::new().unwrap();

        crypto::encrypt_file(src.path(), encrypted.path(), TEST_KEY).unwrap();
        crypto::decrypt_file(encrypted.path(), decrypted.path(), TEST_KEY).unwrap();

        let recovered = std::fs::read(decrypted.path()).unwrap();
        assert!(recovered.is_empty());
    }

    #[test]
    fn test_encrypt_decrypt_round_trip_binary_content() {
        // 4 KiB of pseudo-random bytes (0x00..0xff cycling)
        let plain: Vec<u8> = (0u16..4096).map(|i| (i % 256) as u8).collect();
        let src = write_temp_file(&plain);
        let encrypted = NamedTempFile::new().unwrap();
        let decrypted = NamedTempFile::new().unwrap();

        crypto::encrypt_file(src.path(), encrypted.path(), TEST_KEY).unwrap();
        crypto::decrypt_file(encrypted.path(), decrypted.path(), TEST_KEY).unwrap();

        assert_eq!(std::fs::read(decrypted.path()).unwrap(), plain);
    }

    #[test]
    fn test_encrypted_file_is_different_from_plaintext() {
        let plain = b"sensitive data";
        let src = write_temp_file(plain);
        let encrypted = NamedTempFile::new().unwrap();

        crypto::encrypt_file(src.path(), encrypted.path(), TEST_KEY).unwrap();

        let cipher_bytes = std::fs::read(encrypted.path()).unwrap();
        // The ciphertext must not contain the plaintext verbatim
        assert!(
            !cipher_bytes.windows(plain.len()).any(|w| w == plain),
            "plaintext must not appear verbatim in ciphertext"
        );
    }

    #[test]
    fn test_two_encryptions_produce_different_ciphertext() {
        // Different random salt + nonce each time, so identical plaintext → different ciphertext
        let plain = b"same content";
        let src = write_temp_file(plain);
        let enc1 = NamedTempFile::new().unwrap();
        let enc2 = NamedTempFile::new().unwrap();

        crypto::encrypt_file(src.path(), enc1.path(), TEST_KEY).unwrap();
        crypto::encrypt_file(src.path(), enc2.path(), TEST_KEY).unwrap();

        assert_ne!(
            std::fs::read(enc1.path()).unwrap(),
            std::fs::read(enc2.path()).unwrap(),
            "two encryptions of the same plaintext must not be identical"
        );
    }

    #[test]
    fn test_decrypt_with_wrong_key_fails() {
        let src = write_temp_file(b"top secret");
        let encrypted = NamedTempFile::new().unwrap();
        let decrypted = NamedTempFile::new().unwrap();

        let wrong_key = "ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff";
        crypto::encrypt_file(src.path(), encrypted.path(), TEST_KEY).unwrap();
        let result = crypto::decrypt_file(encrypted.path(), decrypted.path(), wrong_key);
        assert!(result.is_err(), "decryption with wrong key must fail");
    }

    #[test]
    fn test_decrypt_tampered_ciphertext_fails() {
        let src = write_temp_file(b"important document");
        let encrypted = NamedTempFile::new().unwrap();
        let decrypted = NamedTempFile::new().unwrap();

        crypto::encrypt_file(src.path(), encrypted.path(), TEST_KEY).unwrap();

        // Flip a byte in the ciphertext portion (after the 36-byte header)
        let mut bytes = std::fs::read(encrypted.path()).unwrap();
        if bytes.len() > 40 {
            bytes[40] ^= 0xFF;
        }
        std::fs::write(encrypted.path(), &bytes).unwrap();

        let result = crypto::decrypt_file(encrypted.path(), decrypted.path(), TEST_KEY);
        assert!(result.is_err(), "GCM auth must reject tampered ciphertext");
    }

    #[test]
    fn test_md5_is_consistent() {
        let content = b"deterministic content";
        let f = write_temp_file(content);
        let md5_a = crypto::compute_file_md5(f.path()).unwrap();
        let md5_b = crypto::compute_file_md5(f.path()).unwrap();
        assert_eq!(md5_a, md5_b, "MD5 of the same file must be stable");
        assert_eq!(md5_a.len(), 32, "MD5 should be 32 hex characters");
    }

    // ── 6. Backup scan ────────────────────────────────────────────────────────

    fn make_dir_with_files(files: &[(&str, &[u8])]) -> TempDir {
        let dir = TempDir::new().unwrap();
        for (name, content) in files {
            let path = dir.path().join(name);
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).unwrap();
            }
            std::fs::write(&path, content).unwrap();
        }
        dir
    }

    #[test]
    fn test_scan_directory_finds_all_files() {
        let dir = make_dir_with_files(&[
            ("a.txt", b"alpha"),
            ("b.log", b"bravo"),
            ("sub/c.txt", b"charlie"),
        ]);

        let found = backup::scan_directory(dir.path(), &[]).unwrap();
        assert_eq!(found.len(), 3, "should find all 3 files");
    }

    #[test]
    fn test_scan_directory_respects_skip_patterns() {
        let dir = make_dir_with_files(&[
            ("keep.txt", b"keep"),
            ("skip.log", b"skip"),
            ("also_skip.log", b"skip too"),
        ]);

        let pattern = regex::Regex::new(r"\.log$").unwrap();
        let found = backup::scan_directory(dir.path(), &[pattern]).unwrap();
        assert_eq!(found.len(), 1, "only keep.txt should remain");
        assert!(found[0].to_string_lossy().ends_with("keep.txt"));
    }

    #[test]
    fn test_scan_directory_recursive() {
        let dir = make_dir_with_files(&[
            ("root.txt", b"root"),
            ("level1/file.txt", b"l1"),
            ("level1/level2/deep.txt", b"deep"),
        ]);

        let found = backup::scan_directory(dir.path(), &[]).unwrap();
        assert_eq!(found.len(), 3);
    }

    #[test]
    fn test_scan_empty_directory_returns_empty() {
        let dir = TempDir::new().unwrap();
        let found = backup::scan_directory(dir.path(), &[]).unwrap();
        assert!(found.is_empty());
    }

    // ── 7. Crypto + DB integration ────────────────────────────────────────────

    #[test]
    fn test_encrypt_then_record_in_db() {
        // Simulates the core backup pipeline:
        // 1. Write a file, compute MD5, encrypt it.
        // 2. Record the backup_entry + local_file in SQLite.
        // 3. Verify the DB state matches the encrypt result.
        let conn = open_db();
        let pid = insert_test_profile(&conn, "pipeline");

        let plain = b"file content for pipeline test";
        let src = write_temp_file(plain);
        let encrypted = NamedTempFile::new().unwrap();

        let original_md5 = crypto::compute_file_md5(src.path()).unwrap();
        let result = crypto::encrypt_file(src.path(), encrypted.path(), TEST_KEY).unwrap();

        assert_eq!(result.original_md5, original_md5);
        assert_ne!(result.encrypted_md5, original_md5, "encrypted MD5 should differ");
        assert_eq!(result.file_size, plain.len() as u64);

        let uuid = "test-uuid-pipeline";
        let entry_id = db::insert_backup_entry(
            &conn,
            pid,
            uuid,
            &result.original_md5,
            &result.encrypted_md5,
            result.file_size as i64,
        )
        .unwrap();

        db::insert_local_file(
            &conn,
            entry_id,
            src.path().to_str().unwrap(),
            None,
            Some(plain.len() as i64),
        )
        .unwrap();

        let entries = db::list_backup_entries(&conn, pid).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].original_md5, original_md5);
        assert_eq!(entries[0].file_size, plain.len() as i64);

        let lfs = db::list_local_files(&conn, entry_id).unwrap();
        assert_eq!(lfs.len(), 1);
        assert_eq!(lfs[0].cached_size, Some(plain.len() as i64));
    }

    #[test]
    fn test_full_backup_and_restore_pipeline() {
        // Encrypt a file, record it in DB, then decrypt it and verify the content
        // matches the original — simulating the full backup + restore cycle
        // without S3.
        let conn = open_db();
        let pid = insert_test_profile(&conn, "full-cycle");

        let plain = b"The quick brown fox jumps over the lazy dog. Vault integration test.";
        let src = write_temp_file(plain);
        let encrypted = NamedTempFile::new().unwrap();
        let restored = NamedTempFile::new().unwrap();

        // ── Backup phase ──
        let enc_result = crypto::encrypt_file(src.path(), encrypted.path(), TEST_KEY).unwrap();
        let uuid = uuid::Uuid::new_v4().to_string();
        let entry_id = db::insert_backup_entry(
            &conn,
            pid,
            &uuid,
            &enc_result.original_md5,
            &enc_result.encrypted_md5,
            enc_result.file_size as i64,
        )
        .unwrap();
        db::insert_local_file(&conn, entry_id, "/original/path/fox.txt", None, None).unwrap();

        // ── Restore phase ──
        // Fetch the entry from DB as the restore command would
        let fetched_entry = db::get_backup_entry_by_id(&conn, entry_id).unwrap().unwrap();
        assert_eq!(fetched_entry.object_uuid, uuid);

        // Decrypt the "downloaded" (in our case still local) file
        crypto::decrypt_file(encrypted.path(), restored.path(), TEST_KEY).unwrap();

        let recovered = std::fs::read(restored.path()).unwrap();
        assert_eq!(recovered, plain, "restored content must match original");

        // ── Verify phase (MD5 check) ──
        let restored_md5 = crypto::compute_file_md5(restored.path()).unwrap();
        assert_eq!(restored_md5, fetched_entry.original_md5);
    }
}
