/// Integration tests that exercise multiple modules together.
///
/// These tests use real files (via `tempfile`), real SQLite databases, and the actual
/// crypto primitives — but they do NOT touch the OS keychain or S3. Tests that
/// require credentials or network access belong in a separate test harness with
/// a live MinIO instance.
#[cfg(test)]
mod tests {
    use std::io::{Read, Write};
    use std::sync::Mutex;

    use tempfile::{NamedTempFile, TempDir};

    use crate::backup;
    use crate::crypto;
    use crate::db;
    use crate::profiles;
    use crate::s3::S3Client;

    const TEST_KEY_HEX: &str =
        "0102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f20";

    fn key_bytes() -> [u8; 32] {
        crypto::decode_encryption_key(TEST_KEY_HEX).unwrap()
    }

    fn write_temp_file(content: &[u8]) -> NamedTempFile {
        let mut f = NamedTempFile::new().unwrap();
        f.write_all(content).unwrap();
        f.flush().unwrap();
        f
    }

    // ── 1. Database schema ────────────────────────────────────────────────────

    #[test]
    fn test_db_init_creates_v5_schema_on_disk() {
        let tmp = NamedTempFile::new().unwrap();
        let path = tmp.path().to_str().unwrap().to_string();
        let conn = db::init_database(&path).unwrap();

        for table in &[
            "profile", "chunk", "file_entry", "file_chunk",
            "local_file", "share_manifest", "share_manifest_entry",
        ] {
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
        let tmp = NamedTempFile::new().unwrap();
        let path = tmp.path().to_str().unwrap().to_string();
        db::init_database(&path).unwrap();
        db::init_database(&path).unwrap(); // second open — must not error
    }

    // ── 2. Profile CRUD ───────────────────────────────────────────────────────

    fn insert_test_profile(conn: &rusqlite::Connection, name: &str) -> i64 {
        db::insert_profile(
            conn,
            name,
            "read-write",
            "https://s3.example.com",
            None,
            "test-bucket",
            None,
            None,
            None,
            None,
            profiles::DEFAULT_CHUNK_SIZE_BYTES,
        )
        .unwrap()
    }

    #[test]
    fn test_profile_insert_and_list() {
        let conn = db::open_test_db();
        let id = insert_test_profile(&conn, "alpha");
        let profiles = db::list_profiles(&conn).unwrap();
        assert_eq!(profiles.len(), 1);
        assert_eq!(profiles[0].id, id);
        assert_eq!(profiles[0].name, "alpha");
        assert_eq!(profiles[0].mode, "read-write");
        assert_eq!(profiles[0].chunk_size_bytes, profiles::DEFAULT_CHUNK_SIZE_BYTES);
    }

    #[test]
    fn test_profile_set_active() {
        let conn = db::open_test_db();
        let id1 = insert_test_profile(&conn, "p1");
        let id2 = insert_test_profile(&conn, "p2");

        db::set_active_profile(&conn, id1).unwrap();
        assert!(db::get_profile_by_id(&conn, id1).unwrap().unwrap().is_active);
        assert!(!db::get_profile_by_id(&conn, id2).unwrap().unwrap().is_active);

        db::set_active_profile(&conn, id2).unwrap();
        assert!(!db::get_profile_by_id(&conn, id1).unwrap().unwrap().is_active);
        assert!(db::get_profile_by_id(&conn, id2).unwrap().unwrap().is_active);
    }

    // ── 3. Chunk + FileEntry + FileChunk round-trip ───────────────────────────

    #[test]
    fn test_chunk_insert_and_lookup_by_hash() {
        let conn = db::open_test_db();
        let pid = insert_test_profile(&conn, "chunk-test");

        let chunk_id =
            db::insert_chunk(&conn, pid, "abc123hash", "c/abc123hash", 1024).unwrap();
        assert!(chunk_id > 0);

        let found = db::get_chunk_id_by_hash(&conn, pid, "abc123hash").unwrap();
        assert_eq!(found, Some(chunk_id));

        let not_found = db::get_chunk_id_by_hash(&conn, pid, "nonexistent").unwrap();
        assert!(not_found.is_none());
    }

    #[test]
    fn test_chunk_update_s3_key() {
        let conn = db::open_test_db();
        let pid = insert_test_profile(&conn, "scramble-test");
        let chunk_id = db::insert_chunk(&conn, pid, "hash1", "c/hash1", 512).unwrap();

        db::update_chunk_s3_key(&conn, chunk_id, "c/new-scrambled-key").unwrap();
        let chunk = db::get_chunk_by_id(&conn, chunk_id).unwrap().unwrap();
        assert_eq!(chunk.s3_key, "c/new-scrambled-key");
    }

    #[test]
    fn test_file_entry_insert_and_lookup() {
        let conn = db::open_test_db();
        let pid = insert_test_profile(&conn, "fe-test");

        let fe_id = db::insert_file_entry(&conn, pid, "md5abc", 2048, 1).unwrap();
        let entry = db::get_file_entry_by_id(&conn, fe_id).unwrap().unwrap();
        assert_eq!(entry.original_md5, "md5abc");
        assert_eq!(entry.total_size, 2048);
        assert_eq!(entry.chunk_count, 1);

        let by_md5 = db::get_file_entry_by_md5(&conn, pid, "md5abc").unwrap();
        assert_eq!(by_md5.unwrap().id, fe_id);
    }

    #[test]
    fn test_file_chunk_ordering() {
        let conn = db::open_test_db();
        let pid = insert_test_profile(&conn, "order-test");

        let c0 = db::insert_chunk(&conn, pid, "hash0", "c/hash0", 100).unwrap();
        let c1 = db::insert_chunk(&conn, pid, "hash1", "c/hash1", 100).unwrap();
        let c2 = db::insert_chunk(&conn, pid, "hash2", "c/hash2", 80).unwrap();
        let fe_id = db::insert_file_entry(&conn, pid, "fmd5", 280, 3).unwrap();

        // Insert out of order intentionally
        db::insert_file_chunk(&conn, fe_id, 2, c2).unwrap();
        db::insert_file_chunk(&conn, fe_id, 0, c0).unwrap();
        db::insert_file_chunk(&conn, fe_id, 1, c1).unwrap();

        let keys = db::get_chunk_keys_for_file(&conn, fe_id).unwrap();
        assert_eq!(keys.len(), 3);
        assert_eq!(keys[0], (0, "c/hash0".to_string()));
        assert_eq!(keys[1], (1, "c/hash1".to_string()));
        assert_eq!(keys[2], (2, "c/hash2".to_string()));
    }

    #[test]
    fn test_count_file_entries_for_chunk() {
        let conn = db::open_test_db();
        let pid = insert_test_profile(&conn, "refcount-test");

        let chunk_id = db::insert_chunk(&conn, pid, "shared-hash", "c/shared", 256).unwrap();

        // Add chunk to two different file entries
        let fe1 = db::insert_file_entry(&conn, pid, "md5-fe1", 256, 1).unwrap();
        let fe2 = db::insert_file_entry(&conn, pid, "md5-fe2", 256, 1).unwrap();
        db::insert_file_chunk(&conn, fe1, 0, chunk_id).unwrap();
        db::insert_file_chunk(&conn, fe2, 0, chunk_id).unwrap();

        let count = db::count_file_entries_for_chunk(&conn, chunk_id).unwrap();
        assert_eq!(count, 2);
    }

    #[test]
    fn test_local_file_upsert() {
        let conn = db::open_test_db();
        let pid = insert_test_profile(&conn, "lf-test");
        let fe_id = db::insert_file_entry(&conn, pid, "md5lf", 100, 1).unwrap();

        db::upsert_local_file(&conn, fe_id, "/home/user/doc.txt", Some(1_700_000_000.0), Some(100))
            .unwrap();

        let lf = db::get_local_file_by_path(&conn, "/home/user/doc.txt")
            .unwrap()
            .unwrap();
        assert_eq!(lf.file_entry_id, fe_id);
        assert!((lf.cached_mtime.unwrap() - 1_700_000_000.0).abs() < 0.001);
        assert_eq!(lf.cached_size, Some(100));

        // Upsert again with new mtime (simulates file update)
        db::upsert_local_file(&conn, fe_id, "/home/user/doc.txt", Some(1_800_000_000.0), Some(100))
            .unwrap();

        let lf2 = db::get_local_file_by_path(&conn, "/home/user/doc.txt")
            .unwrap()
            .unwrap();
        assert!((lf2.cached_mtime.unwrap() - 1_800_000_000.0).abs() < 0.001);
    }

    // ── 4. Share manifest lifecycle ───────────────────────────────────────────

    #[test]
    fn test_share_manifest_lifecycle() {
        let conn = db::open_test_db();
        let pid = insert_test_profile(&conn, "sharer");
        let fe_id = db::insert_file_entry(&conn, pid, "md5photo", 1024, 1).unwrap();

        let manifest_id =
            db::insert_share_manifest(&conn, pid, "mfst-uuid-1", Some("vacation pics"), 1)
                .unwrap();
        db::insert_share_manifest_entry(&conn, manifest_id, fe_id, "photo.jpg").unwrap();

        let manifests = db::list_share_manifests(&conn, pid).unwrap();
        assert_eq!(manifests.len(), 1);
        assert_eq!(manifests[0].manifest_uuid, "mfst-uuid-1");
        assert!(manifests[0].is_valid);

        let entries = db::list_share_manifest_entries(&conn, manifest_id).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].filename, "photo.jpg");
        assert_eq!(entries[0].file_entry_id, fe_id);

        db::invalidate_share_manifest(&conn, manifest_id).unwrap();
        assert!(!db::get_share_manifest_by_id(&conn, manifest_id).unwrap().unwrap().is_valid);

        db::delete_share_manifest(&conn, manifest_id).unwrap();
        assert!(db::list_share_manifests(&conn, pid).unwrap().is_empty());
        assert!(db::list_share_manifest_entries(&conn, manifest_id).unwrap().is_empty());
    }

    // ── 5. Crypto: chunk encryption ───────────────────────────────────────────

    #[test]
    fn test_decode_encryption_key_valid() {
        let key = crypto::decode_encryption_key(TEST_KEY_HEX).unwrap();
        assert_eq!(key.len(), 32);
        assert_eq!(key[0], 0x01);
        assert_eq!(key[31], 0x20);
    }

    #[test]
    fn test_decode_encryption_key_invalid_hex() {
        assert!(crypto::decode_encryption_key("not-hex!").is_err());
    }

    #[test]
    fn test_decode_encryption_key_wrong_length() {
        assert!(crypto::decode_encryption_key("aabb").is_err());
    }

    #[test]
    fn test_encrypt_decrypt_chunk_round_trip() {
        let key = key_bytes();
        let plain = b"Hello, chunk storage!";
        let encrypted = crypto::encrypt_chunk(&key, plain).unwrap();
        let decrypted = crypto::decrypt_chunk(&key, &encrypted).unwrap();
        assert_eq!(decrypted, plain);
    }

    #[test]
    fn test_encrypt_chunk_empty() {
        let key = key_bytes();
        let encrypted = crypto::encrypt_chunk(&key, b"").unwrap();
        let decrypted = crypto::decrypt_chunk(&key, &encrypted).unwrap();
        assert!(decrypted.is_empty());
    }

    #[test]
    fn test_encrypt_chunk_ciphertext_differs_from_plaintext() {
        let key = key_bytes();
        let plain = b"sensitive data";
        let encrypted = crypto::encrypt_chunk(&key, plain).unwrap();
        assert!(!encrypted.windows(plain.len()).any(|w| w == plain));
    }

    #[test]
    fn test_two_encryptions_produce_different_ciphertext() {
        let key = key_bytes();
        let plain = b"same content";
        let enc1 = crypto::encrypt_chunk(&key, plain).unwrap();
        let enc2 = crypto::encrypt_chunk(&key, plain).unwrap();
        assert_ne!(enc1, enc2, "random nonces must produce distinct ciphertexts");
    }

    #[test]
    fn test_decrypt_with_wrong_key_fails() {
        let key = key_bytes();
        let wrong_key_hex = "ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff";
        let wrong_key = crypto::decode_encryption_key(wrong_key_hex).unwrap();
        let encrypted = crypto::encrypt_chunk(&key, b"top secret").unwrap();
        assert!(crypto::decrypt_chunk(&wrong_key, &encrypted).is_err());
    }

    #[test]
    fn test_decrypt_tampered_chunk_fails() {
        let key = key_bytes();
        let mut encrypted = crypto::encrypt_chunk(&key, b"important").unwrap();
        if encrypted.len() > 15 {
            encrypted[15] ^= 0xFF; // flip a byte in the ciphertext
        }
        assert!(crypto::decrypt_chunk(&key, &encrypted).is_err());
    }

    #[test]
    fn test_compute_chunk_hmac_deterministic() {
        let key = key_bytes();
        let data = b"some chunk data";
        let h1 = crypto::compute_chunk_hmac(&key, data);
        let h2 = crypto::compute_chunk_hmac(&key, data);
        assert_eq!(h1, h2);
        assert_eq!(h1.len(), 64); // 32-byte HMAC → 64 hex chars
    }

    #[test]
    fn test_compute_chunk_hmac_different_keys_differ() {
        let key1 = key_bytes();
        let key2 = crypto::decode_encryption_key(
            "ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff",
        )
        .unwrap();
        let data = b"same data";
        assert_ne!(
            crypto::compute_chunk_hmac(&key1, data),
            crypto::compute_chunk_hmac(&key2, data),
        );
    }

    #[test]
    fn test_md5_is_consistent() {
        let content = b"deterministic content";
        let f = write_temp_file(content);
        let md5_a = crypto::compute_file_md5(f.path()).unwrap();
        let md5_b = crypto::compute_file_md5(f.path()).unwrap();
        assert_eq!(md5_a, md5_b);
        assert_eq!(md5_a.len(), 32);
    }

    // ── 6. Backup directory scan ──────────────────────────────────────────────

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
        assert_eq!(backup::scan_directory(dir.path(), &[]).unwrap().len(), 3);
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
        assert_eq!(found.len(), 1);
        assert!(found[0].to_string_lossy().ends_with("keep.txt"));
    }

    #[test]
    fn test_scan_directory_recursive() {
        let dir = make_dir_with_files(&[
            ("root.txt", b"root"),
            ("level1/file.txt", b"l1"),
            ("level1/level2/deep.txt", b"deep"),
        ]);
        assert_eq!(backup::scan_directory(dir.path(), &[]).unwrap().len(), 3);
    }

    #[test]
    fn test_scan_empty_directory_returns_empty() {
        let dir = TempDir::new().unwrap();
        assert!(backup::scan_directory(dir.path(), &[]).unwrap().is_empty());
    }

    // ── 7. Chunk pipeline end-to-end (no S3) ─────────────────────────────────

    /// Simulates the full backup → restore cycle using the DB and crypto
    /// primitives directly, without S3. Exercises chunk splitting, HMAC
    /// identity, encryption, DB recording, and reassembly.
    #[test]
    fn test_chunk_pipeline_backup_and_restore() {
        use md5::{Digest, Md5};

        let key = key_bytes();
        let conn = db::open_test_db();
        let pid = insert_test_profile(&conn, "pipeline");

        // Build a 3-chunk file (chunk_size = 16 bytes, file = 40 bytes)
        let chunk_size: usize = 16;
        let plain: Vec<u8> = (0u8..40).collect();
        let src = write_temp_file(&plain);

        // ── Backup phase ──
        let mut file = std::fs::File::open(src.path()).unwrap();
        let mut rolling_md5 = Md5::new();
        let mut all_chunk_ids: Vec<(usize, i64)> = Vec::new();
        let mut chunk_index = 0;
        let mut total_size: i64 = 0;

        loop {
            let mut buf = Vec::with_capacity(chunk_size);
            let n = (&mut file)
                .take(chunk_size as u64)
                .read_to_end(&mut buf)
                .unwrap();
            if n == 0 {
                break;
            }
            rolling_md5.update(&buf);
            total_size += n as i64;

            let chunk_hash = crypto::compute_chunk_hmac(&key, &buf);

            // "Upload" is skipped (no S3); insert chunk record directly.
            let encrypted = crypto::encrypt_chunk(&key, &buf).unwrap();
            let enc_size = encrypted.len() as i64;
            let s3_key = format!("c/{}", chunk_hash);
            let chunk_id =
                db::insert_chunk(&conn, pid, &chunk_hash, &s3_key, enc_size).unwrap();

            all_chunk_ids.push((chunk_index, chunk_id));
            chunk_index += 1;
        }

        let original_md5 = hex::encode(rolling_md5.finalize());
        let total_chunks = chunk_index;

        let fe_id =
            db::insert_file_entry(&conn, pid, &original_md5, total_size, total_chunks as i64)
                .unwrap();
        for (idx, chunk_id) in &all_chunk_ids {
            db::insert_file_chunk(&conn, fe_id, *idx as i64, *chunk_id).unwrap();
        }
        db::upsert_local_file(&conn, fe_id, "/tmp/test-file.bin", None, Some(total_size))
            .unwrap();

        // ── Verify DB state ──
        let entry = db::get_file_entry_by_id(&conn, fe_id).unwrap().unwrap();
        assert_eq!(entry.original_md5, original_md5);
        assert_eq!(entry.chunk_count, 3); // 40 bytes / 16 = 3 chunks
        assert_eq!(entry.total_size, 40);

        let chunk_keys = db::get_chunk_keys_for_file(&conn, fe_id).unwrap();
        assert_eq!(chunk_keys.len(), 3);

        // ── Restore phase ──
        // Re-encrypt chunks from original data (simulates what S3 stores)
        let chunks_data: Vec<Vec<u8>> = plain.chunks(chunk_size).map(|c| c.to_vec()).collect();

        let mut restored = Vec::new();
        for (idx, s3_key) in &chunk_keys {
            // "Download" = re-encrypt the original chunk (simulates S3 download)
            let encrypted = crypto::encrypt_chunk(&key, &chunks_data[*idx]).unwrap();
            let decrypted = crypto::decrypt_chunk(&key, &encrypted).unwrap();
            restored.extend_from_slice(&decrypted);
            // Verify s3_key matches expected pattern
            assert!(s3_key.starts_with("c/"));
        }

        assert_eq!(restored, plain, "restored data must match original");

        // ── MD5 verification ──
        let restored_md5 = hex::encode(Md5::digest(&restored));
        assert_eq!(restored_md5, original_md5);
    }

    #[test]
    fn test_dedup_identical_chunks() {
        let key = key_bytes();
        let conn = db::open_test_db();
        let pid = insert_test_profile(&conn, "dedup");

        let chunk_data = b"identical chunk content";
        let hash = crypto::compute_chunk_hmac(&key, chunk_data);

        // Insert once
        let chunk_id =
            db::insert_chunk(&conn, pid, &hash, &format!("c/{}", hash), 64).unwrap();

        // Same hash should be found without inserting again
        let found = db::get_chunk_id_by_hash(&conn, pid, &hash).unwrap();
        assert_eq!(found, Some(chunk_id));

        // Two file_entries pointing to the same chunk
        let fe1 = db::insert_file_entry(&conn, pid, "md5-file1", 23, 1).unwrap();
        let fe2 = db::insert_file_entry(&conn, pid, "md5-file2", 23, 1).unwrap();
        db::insert_file_chunk(&conn, fe1, 0, chunk_id).unwrap();
        db::insert_file_chunk(&conn, fe2, 0, chunk_id).unwrap();

        let ref_count = db::count_file_entries_for_chunk(&conn, chunk_id).unwrap();
        assert_eq!(ref_count, 2, "chunk should be referenced by both file_entries");
    }

    // ── 8. backup_file outcomes (no S3) ──────────────────────────────────────

    /// Returns `Skipped` when local_file already has a matching mtime and size.
    /// The S3 client is never called on this path.
    #[tokio::test]
    async fn test_backup_file_skipped_on_cache_hit() {
        let key = key_bytes();
        let f = write_temp_file(b"some file content that will be skipped");
        let path = f.path().to_path_buf();

        let metadata = std::fs::metadata(&path).unwrap();
        let mtime = metadata
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs_f64());
        let size = metadata.len() as i64;

        let conn = db::open_test_db();
        let pid = insert_test_profile(&conn, "skip-test");

        // Seed local_file with the current mtime + size so the cache check hits.
        let fe_id = db::insert_file_entry(&conn, pid, "dummy-md5", size, 1).unwrap();
        db::upsert_local_file(&conn, fe_id, "skip/file.bin", mtime, Some(size)).unwrap();

        let db_state = db::DbState(Mutex::new(conn));
        let s3 = S3Client::new_for_test();

        let result = backup::backup_file(
            &db_state,
            &s3,
            pid,
            &path,
            "skip/file.bin",
            &key,
            None,
            1024 * 1024,
        )
        .await
        .unwrap();

        assert!(
            matches!(result, backup::FileOutcome::Skipped),
            "expected Skipped when mtime+size match"
        );
    }

    /// Returns `Deduped` when all chunk hashes AND the whole-file MD5 already
    /// exist in the DB.  No S3 operations are attempted because every chunk is
    /// found via `get_chunk_id_by_hash` and the `join_set` stays empty.
    #[tokio::test]
    async fn test_backup_file_deduped_when_md5_exists() {
        use md5::{Digest, Md5};

        let key = key_bytes();
        let chunk_size: usize = 16;
        // Exactly two full chunks of 16 bytes each.
        let content: Vec<u8> = (0u8..32).collect();
        let f = write_temp_file(&content);
        let path = f.path().to_path_buf();

        // Reproduce the rolling MD5 and per-chunk HMACs that backup_file will compute.
        let mut md5_hasher = Md5::new();
        let mut chunk_hashes: Vec<String> = Vec::new();
        for chunk in content.chunks(chunk_size) {
            md5_hasher.update(chunk);
            chunk_hashes.push(crypto::compute_chunk_hmac(&key, chunk));
        }
        let original_md5 = hex::encode(md5_hasher.finalize());

        let conn = db::open_test_db();
        let pid = insert_test_profile(&conn, "dedup-test");

        // Pre-seed every chunk so get_chunk_id_by_hash returns Some for each one,
        // keeping the join_set empty and preventing any S3 call.
        let mut chunk_ids: Vec<i64> = Vec::new();
        for (i, hash) in chunk_hashes.iter().enumerate() {
            let chunk_data = &content[i * chunk_size..(i + 1) * chunk_size];
            let enc_len =
                crypto::encrypt_chunk(&key, chunk_data).unwrap().len() as i64;
            let cid =
                db::insert_chunk(&conn, pid, hash, &format!("c/{}", hash), enc_len)
                    .unwrap();
            chunk_ids.push(cid);
        }

        // Pre-seed file_entry with the matching MD5.
        let fe_id = db::insert_file_entry(
            &conn,
            pid,
            &original_md5,
            content.len() as i64,
            chunk_hashes.len() as i64,
        )
        .unwrap();
        for (i, cid) in chunk_ids.iter().enumerate() {
            db::insert_file_chunk(&conn, fe_id, i as i64, *cid).unwrap();
        }

        let db_state = db::DbState(Mutex::new(conn));
        let s3 = S3Client::new_for_test();

        let result = backup::backup_file(
            &db_state,
            &s3,
            pid,
            &path,
            "dedup/file.bin",
            &key,
            None,
            chunk_size,
        )
        .await
        .unwrap();

        assert!(
            matches!(result, backup::FileOutcome::Deduped),
            "expected Deduped when all chunks and MD5 are already in DB"
        );

        // local_file should now map the stored path to the existing file_entry.
        let lf = {
            let conn = db_state.conn().unwrap();
            db::get_local_file_by_path(&conn, "dedup/file.bin").unwrap()
        };
        assert!(lf.is_some(), "local_file should have been upserted");
        assert_eq!(
            lf.unwrap().file_entry_id,
            fe_id,
            "local_file should reference the pre-existing file_entry"
        );
    }
}
