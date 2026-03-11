use std::fs;
use std::io::{BufReader, BufWriter, Read, Write};
use std::path::Path;

use aes_gcm::aead::Aead;
use aes_gcm::{Aes256Gcm, KeyInit, Nonce};
use argon2::Argon2;
use md5::{Digest, Md5};
use rand::RngExt;

use crate::error::AppError;

const SALT_LEN: usize = 16;
const NONCE_LEN: usize = 12;

// ── V1 (legacy) format constants ──────────────────────────────────────────────
// Format: [8 bytes filesize LE][16 bytes salt][12 bytes nonce][ciphertext + 16 byte GCM tag]
const FILESIZE_HEADER_LEN: usize = 8;
const V1_HEADER_LEN: usize = FILESIZE_HEADER_LEN + SALT_LEN + NONCE_LEN; // 36 bytes

// ── V2 (streaming) format constants ───────────────────────────────────────────
// Format: [4 bytes magic "HRPT"][1 byte version=2][8 bytes filesize LE]
//         [16 bytes salt][12 bytes nonce][chunks...]
// Each chunk: [4 bytes u32 LE plaintext_len][plaintext_len + 16 bytes ciphertext+tag]
const MAGIC: &[u8; 4] = b"HRPT";
const FORMAT_VERSION: u8 = 2;
const V2_HEADER_LEN: usize = 4 + 1 + 8 + SALT_LEN + NONCE_LEN; // 41 bytes


#[derive(Debug, serde::Serialize)]
pub struct EncryptedFileMeta {
    pub original_md5: String,
    pub encrypted_md5: String,
    pub file_size: u64,
}

fn derive_key(passphrase: &[u8], salt: &[u8]) -> Result<[u8; 32], AppError> {
    let mut key = [0u8; 32];
    Argon2::default()
        .hash_password_into(passphrase, salt, &mut key)
        .map_err(|e| AppError::Crypto(format!("Argon2 key derivation failed: {}", e)))?;
    Ok(key)
}

/// Derive a per-chunk nonce by treating the last 8 bytes of the base nonce
/// as a LE u64 counter and adding chunk_idx.  The first 4 bytes stay fixed
/// as a random prefix, guaranteeing uniqueness across chunks.
fn nonce_for_chunk(base: &[u8; 12], chunk_idx: u64) -> [u8; 12] {
    let mut nonce = *base;
    let counter = u64::from_le_bytes(nonce[4..12].try_into().unwrap()).wrapping_add(chunk_idx);
    nonce[4..12].copy_from_slice(&counter.to_le_bytes());
    nonce
}

pub fn generate_temp_path(temp_dir: &Path) -> std::path::PathBuf {
    let id = uuid::Uuid::new_v4();
    temp_dir.join(format!("harpocrates-tmp-{}", id))
}

/// Encrypt a file using AES-256-GCM with Argon2id key derivation.
///
/// Uses V2 streaming format: file is processed one chunk at a time
/// so peak memory is ~2× chunk_size.  The encrypted output is
/// written to `output_path` (a temp file); the caller is responsible for
/// cleanup on error.
/// Like `encrypt_file` but calls `on_progress(bytes_done, bytes_total)` after each
/// encrypted chunk so callers can report progress for large files.
/// Default S3 multipart part size (256 MiB — fewer HTTP requests for large files).
pub const DEFAULT_CHUNK_SIZE: usize = 256 * 1024 * 1024;

/// Encryption chunk size: independent of S3 part size.
/// Smaller = more frequent progress updates, less RAM pressure per chunk.
/// AES-GCM throughput is identical regardless of chunk size.
pub const ENCRYPT_CHUNK_SIZE: usize = 16 * 1024 * 1024; // 16 MiB

pub fn encrypt_file_with_progress(
    input_path: &Path,
    output_path: &Path,
    passphrase: &str,
    on_progress: impl Fn(u64, u64),
) -> Result<EncryptedFileMeta, AppError> {
    encrypt_file_impl(input_path, output_path, passphrase, ENCRYPT_CHUNK_SIZE, on_progress)
}

pub fn encrypt_file(
    input_path: &Path,
    output_path: &Path,
    passphrase: &str,
) -> Result<EncryptedFileMeta, AppError> {
    encrypt_file_impl(input_path, output_path, passphrase, ENCRYPT_CHUNK_SIZE, |_, _| {})
}

fn encrypt_file_impl(
    input_path: &Path,
    output_path: &Path,
    passphrase: &str,
    chunk_size: usize,
    on_progress: impl Fn(u64, u64),
) -> Result<EncryptedFileMeta, AppError> {
    let file_size = fs::metadata(input_path)?.len();

    let mut salt = [0u8; SALT_LEN];
    let mut base_nonce = [0u8; NONCE_LEN];
    rand::rng().fill(&mut salt);
    rand::rng().fill(&mut base_nonce);

    let key = derive_key(passphrase.as_bytes(), &salt)?;
    let cipher = Aes256Gcm::new_from_slice(&key)
        .map_err(|e| AppError::Crypto(format!("Failed to create cipher: {}", e)))?;

    let mut reader = BufReader::new(fs::File::open(input_path)?);
    let mut writer = BufWriter::new(fs::File::create(output_path)?);

    let mut plain_md5 = Md5::new();
    let mut enc_md5 = Md5::new();

    // V2 header: MAGIC(4) + VERSION(1) + FILESIZE(8) + SALT(16) + NONCE(12)
    let mut header = [0u8; V2_HEADER_LEN];
    header[..4].copy_from_slice(MAGIC);
    header[4] = FORMAT_VERSION;
    header[5..13].copy_from_slice(&file_size.to_le_bytes());
    header[13..29].copy_from_slice(&salt);
    header[29..41].copy_from_slice(&base_nonce);
    writer.write_all(&header)?;
    enc_md5.update(header);

    let mut buf = vec![0u8; chunk_size.max(4096)];
    let mut chunk_idx: u64 = 0;
    let mut buf_pos: u64 = 0;

    loop {
        let n = reader.read(&mut buf)?;
        if n == 0 {
            break;
        }

        let chunk = &buf[..n];
        plain_md5.update(chunk);

        let nonce_arr = nonce_for_chunk(&base_nonce, chunk_idx);
        let nonce = Nonce::from_slice(&nonce_arr);
        let ciphertext = cipher
            .encrypt(nonce, chunk)
            .map_err(|e| AppError::Crypto(format!("Encryption failed on chunk {}: {}", chunk_idx, e)))?;

        let len_bytes = (n as u32).to_le_bytes();
        writer.write_all(&len_bytes)?;
        writer.write_all(&ciphertext)?;
        enc_md5.update(len_bytes);
        enc_md5.update(&ciphertext);

        on_progress(buf_pos + n as u64, file_size);
        buf_pos += n as u64;
        chunk_idx += 1;
    }

    writer.flush()?;
    writer
        .into_inner()
        .map_err(|e| std::io::Error::other(e.to_string()))?
        .sync_all()?;

    Ok(EncryptedFileMeta {
        original_md5: hex::encode(plain_md5.finalize()),
        encrypted_md5: hex::encode(enc_md5.finalize()),
        file_size,
    })
}

/// Decrypt a file previously encrypted with `encrypt_file`.
///
/// Detects V1 (legacy one-shot) vs V2 (streaming) format automatically.
/// V2 files are decrypted one chunk at a time; peak memory ≈ 2 MiB.
/// V1 files are still supported for backward compatibility but require
/// loading the entire encrypted file into memory.
pub fn decrypt_file(
    input_path: &Path,
    output_path: &Path,
    passphrase: &str,
) -> Result<(), AppError> {
    decrypt_file_impl(input_path, output_path, passphrase, |_, _| {})
}

/// Like `decrypt_file` but calls `on_progress(bytes_done, bytes_total)` after each
/// decrypted chunk so callers can report progress for large files.
/// `bytes_total` is the size of the encrypted input file.
pub fn decrypt_file_with_progress(
    input_path: &Path,
    output_path: &Path,
    passphrase: &str,
    on_progress: impl Fn(u64, u64),
) -> Result<(), AppError> {
    decrypt_file_impl(input_path, output_path, passphrase, on_progress)
}

fn decrypt_file_impl(
    input_path: &Path,
    output_path: &Path,
    passphrase: &str,
    on_progress: impl Fn(u64, u64),
) -> Result<(), AppError> {
    let bytes_total = fs::metadata(input_path)?.len();

    // Peek at the first 4 bytes to determine format.
    let mut magic_buf = [0u8; 4];
    {
        let mut f = fs::File::open(input_path)?;
        f.read_exact(&mut magic_buf)?;
    }

    if &magic_buf == MAGIC {
        decrypt_v2_streaming(input_path, output_path, passphrase, bytes_total, on_progress)
    } else {
        let data = fs::read(input_path)?;
        decrypt_v1(&data, output_path, passphrase)?;
        on_progress(bytes_total, bytes_total);
        Ok(())
    }
}

fn decrypt_v2_streaming(
    input_path: &Path,
    output_path: &Path,
    passphrase: &str,
    bytes_total: u64,
    on_progress: impl Fn(u64, u64),
) -> Result<(), AppError> {
    let mut reader = BufReader::new(fs::File::open(input_path)?);

    // Read full V2 header.
    let mut header = [0u8; V2_HEADER_LEN];
    reader.read_exact(&mut header).map_err(|_| {
        AppError::Crypto("Encrypted file is too small to contain a valid header".into())
    })?;

    let file_size = u64::from_le_bytes(header[5..13].try_into().unwrap());
    let salt = &header[13..29];
    let base_nonce: [u8; NONCE_LEN] = header[29..41].try_into().unwrap();

    let key = derive_key(passphrase.as_bytes(), salt)?;
    let cipher = Aes256Gcm::new_from_slice(&key)
        .map_err(|e| AppError::Crypto(format!("Failed to create cipher: {}", e)))?;

    let mut writer = BufWriter::new(fs::File::create(output_path)?);
    let mut chunk_idx: u64 = 0;
    let mut total_written: u64 = 0;
    let mut bytes_done: u64 = V2_HEADER_LEN as u64;

    loop {
        // Read 4-byte plaintext-length prefix; clean EOF here means we're done.
        let mut len_buf = [0u8; 4];
        match reader.read_exact(&mut len_buf) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
            Err(e) => return Err(e.into()),
        }
        let plain_len = u32::from_le_bytes(len_buf) as usize;
        let ct_len = plain_len + 16; // GCM auth tag

        let mut ct_buf = vec![0u8; ct_len];
        reader.read_exact(&mut ct_buf).map_err(|_| {
            AppError::Crypto(format!("Truncated ciphertext in chunk {}", chunk_idx))
        })?;

        let nonce_arr = nonce_for_chunk(&base_nonce, chunk_idx);
        let nonce = Nonce::from_slice(&nonce_arr);
        let plaintext = cipher
            .decrypt(nonce, ct_buf.as_ref())
            .map_err(|e| AppError::Crypto(format!("Decryption failed on chunk {} (wrong key or tampered data): {}", chunk_idx, e)))?;

        writer.write_all(&plaintext)?;
        total_written += plaintext.len() as u64;
        bytes_done += 4 + ct_len as u64;
        on_progress(bytes_done, bytes_total);
        chunk_idx += 1;
    }

    writer.flush()?;

    if total_written != file_size {
        return Err(AppError::Crypto(format!(
            "Decrypted size {} does not match expected size {}",
            total_written, file_size
        )));
    }

    Ok(())
}

fn decrypt_v1(data: &[u8], output_path: &Path, passphrase: &str) -> Result<(), AppError> {
    if data.len() < V1_HEADER_LEN {
        return Err(AppError::Crypto(
            "Encrypted file is too small to contain a valid header".into(),
        ));
    }

    let file_size_bytes: [u8; 8] = data[..FILESIZE_HEADER_LEN].try_into().unwrap();
    let expected_size = u64::from_le_bytes(file_size_bytes);
    let salt = &data[FILESIZE_HEADER_LEN..FILESIZE_HEADER_LEN + SALT_LEN];
    let nonce_bytes = &data[FILESIZE_HEADER_LEN + SALT_LEN..V1_HEADER_LEN];
    let ciphertext = &data[V1_HEADER_LEN..];

    let key = derive_key(passphrase.as_bytes(), salt)?;
    let cipher = Aes256Gcm::new_from_slice(&key)
        .map_err(|e| AppError::Crypto(format!("Failed to create cipher: {}", e)))?;
    let nonce = Nonce::from_slice(nonce_bytes);

    let plaintext = cipher
        .decrypt(nonce, ciphertext)
        .map_err(|e| AppError::Crypto(format!("Decryption failed (wrong key or tampered data): {}", e)))?;

    if plaintext.len() as u64 != expected_size {
        return Err(AppError::Crypto(format!(
            "Decrypted size {} does not match expected size {}",
            plaintext.len(),
            expected_size
        )));
    }

    let mut file = fs::File::create(output_path)?;
    file.write_all(&plaintext)?;
    file.sync_all()?;
    Ok(())
}

/// Compute the MD5 hash of a file on disk without loading it fully into memory.
pub fn compute_file_md5(path: &Path) -> Result<String, AppError> {
    let mut reader = BufReader::new(fs::File::open(path)?);
    let mut hasher = Md5::new();
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = reader.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hex::encode(hasher.finalize()))
}

/// Clean up harpocrates temp files from a directory.
pub fn cleanup_temp_files(temp_dir: &Path) -> Result<usize, AppError> {
    let mut count = 0;
    if !temp_dir.exists() {
        return Ok(0);
    }
    for entry in fs::read_dir(temp_dir)? {
        let entry = entry?;
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        if name_str.starts_with("harpocrates-tmp-") {
            if let Err(e) = fs::remove_file(entry.path()) {
                eprintln!("Warning: failed to remove temp file {:?}: {}", entry.path(), e);
            } else {
                count += 1;
            }
        }
    }
    Ok(count)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn test_encrypt_decrypt_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let input = dir.path().join("plain.txt");
        let encrypted = dir.path().join("encrypted.bin");
        let decrypted = dir.path().join("decrypted.txt");

        let original_data = b"Hello, Harpocrates! This is a test file for encryption.";
        fs::write(&input, original_data).unwrap();

        let passphrase = "test-passphrase-123";
        let meta = encrypt_file(&input, &encrypted, passphrase).unwrap();

        assert_eq!(meta.file_size, original_data.len() as u64);
        assert!(!meta.original_md5.is_empty());
        assert!(!meta.encrypted_md5.is_empty());
        assert_ne!(meta.original_md5, meta.encrypted_md5);

        // Encrypted file should be larger than original (header + tag per chunk)
        let encrypted_size = fs::metadata(&encrypted).unwrap().len();
        assert!(encrypted_size > original_data.len() as u64);

        decrypt_file(&encrypted, &decrypted, passphrase).unwrap();
        let result = fs::read(&decrypted).unwrap();
        assert_eq!(result, original_data);
    }

    #[test]
    fn test_encrypt_decrypt_empty_file() {
        let dir = tempfile::tempdir().unwrap();
        let input = dir.path().join("empty.txt");
        let encrypted = dir.path().join("encrypted.bin");
        let decrypted = dir.path().join("decrypted.txt");

        fs::write(&input, b"").unwrap();

        let meta = encrypt_file(&input, &encrypted, "pass").unwrap();
        assert_eq!(meta.file_size, 0);

        decrypt_file(&encrypted, &decrypted, "pass").unwrap();
        let result = fs::read(&decrypted).unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn test_decrypt_wrong_passphrase_fails() {
        let dir = tempfile::tempdir().unwrap();
        let input = dir.path().join("plain.txt");
        let encrypted = dir.path().join("encrypted.bin");
        let decrypted = dir.path().join("decrypted.txt");

        fs::write(&input, b"secret data").unwrap();
        encrypt_file(&input, &encrypted, "correct-pass").unwrap();

        let result = decrypt_file(&encrypted, &decrypted, "wrong-pass");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Decryption failed"));
    }

    #[test]
    fn test_decrypt_truncated_file_fails() {
        let dir = tempfile::tempdir().unwrap();
        let encrypted = dir.path().join("truncated.bin");
        let decrypted = dir.path().join("decrypted.txt");

        // Write a file too small to have a valid header (no HRPT magic → V1 path)
        fs::write(&encrypted, b"short").unwrap();

        let result = decrypt_file(&encrypted, &decrypted, "pass");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("too small"));
    }

    #[test]
    fn test_decrypt_tampered_data_fails() {
        let dir = tempfile::tempdir().unwrap();
        let input = dir.path().join("plain.txt");
        let encrypted = dir.path().join("encrypted.bin");
        let decrypted = dir.path().join("decrypted.txt");

        fs::write(&input, b"important data").unwrap();
        encrypt_file(&input, &encrypted, "pass").unwrap();

        // Flip a byte in the ciphertext area (after the V2 header + chunk length prefix)
        let mut data = fs::read(&encrypted).unwrap();
        let last = data.len() - 1;
        data[last] ^= 0xFF;
        fs::write(&encrypted, &data).unwrap();

        let result = decrypt_file(&encrypted, &decrypted, "pass");
        assert!(result.is_err());
    }

    #[test]
    fn test_compute_file_md5() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("test.txt");
        fs::write(&file, b"hello world").unwrap();

        let md5 = compute_file_md5(&file).unwrap();
        assert_eq!(md5, "5eb63bbbe01eeed093cb22bb8f5acdc3");
    }

    #[test]
    fn test_generate_temp_path() {
        let dir = tempfile::tempdir().unwrap();
        let path = generate_temp_path(dir.path());
        let filename = path.file_name().unwrap().to_string_lossy();
        assert!(filename.starts_with("harpocrates-tmp-"));
    }

    #[test]
    fn test_cleanup_temp_files() {
        let dir = tempfile::tempdir().unwrap();

        fs::write(dir.path().join("harpocrates-tmp-abc"), "tmp1").unwrap();
        fs::write(dir.path().join("harpocrates-tmp-def"), "tmp2").unwrap();
        fs::write(dir.path().join("other-file.txt"), "keep").unwrap();

        let cleaned = cleanup_temp_files(dir.path()).unwrap();
        assert_eq!(cleaned, 2);

        assert!(dir.path().join("other-file.txt").exists());
        assert!(!dir.path().join("harpocrates-tmp-abc").exists());
        assert!(!dir.path().join("harpocrates-tmp-def").exists());
    }

    #[test]
    fn test_cleanup_nonexistent_dir() {
        let result = cleanup_temp_files(Path::new("/nonexistent/path"));
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), 0);
    }

    #[test]
    fn test_encrypt_produces_different_output_each_time() {
        let dir = tempfile::tempdir().unwrap();
        let input = dir.path().join("plain.txt");
        let enc1 = dir.path().join("enc1.bin");
        let enc2 = dir.path().join("enc2.bin");

        fs::write(&input, b"same data").unwrap();
        encrypt_file(&input, &enc1, "pass").unwrap();
        encrypt_file(&input, &enc2, "pass").unwrap();

        let data1 = fs::read(&enc1).unwrap();
        let data2 = fs::read(&enc2).unwrap();
        assert_ne!(data1, data2);
    }

    #[test]
    fn test_encrypted_file_format_header() {
        let dir = tempfile::tempdir().unwrap();
        let input = dir.path().join("plain.txt");
        let encrypted = dir.path().join("encrypted.bin");

        let data = b"test data for header check";
        fs::write(&input, data).unwrap();
        encrypt_file(&input, &encrypted, "pass").unwrap();

        let enc_data = fs::read(&encrypted).unwrap();

        // V2 format: MAGIC(4) + VERSION(1) + FILESIZE(8) + SALT(16) + NONCE(12) = 41 bytes
        assert_eq!(&enc_data[..4], b"HRPT");
        assert_eq!(enc_data[4], FORMAT_VERSION);
        let size_bytes: [u8; 8] = enc_data[5..13].try_into().unwrap();
        let stored_size = u64::from_le_bytes(size_bytes);
        assert_eq!(stored_size, data.len() as u64);
        assert!(enc_data.len() >= V2_HEADER_LEN);
    }

    #[test]
    fn test_v1_backward_compat() {
        // Manually construct a V1-format encrypted file and verify it decrypts correctly.
        use aes_gcm::aead::Aead;
        use aes_gcm::{Aes256Gcm, KeyInit, Nonce};

        let dir = tempfile::tempdir().unwrap();
        let encrypted = dir.path().join("v1.bin");
        let decrypted = dir.path().join("decrypted.txt");

        let passphrase = "legacy-pass";
        let plaintext = b"legacy data";

        let mut salt = [0u8; SALT_LEN];
        let mut nonce_bytes = [0u8; NONCE_LEN];
        rand::rng().fill(&mut salt);
        rand::rng().fill(&mut nonce_bytes);

        let key = derive_key(passphrase.as_bytes(), &salt).unwrap();
        let cipher = Aes256Gcm::new_from_slice(&key).unwrap();
        let nonce = Nonce::from_slice(&nonce_bytes);
        let ciphertext = cipher.encrypt(nonce, plaintext.as_ref()).unwrap();

        // Build V1 format manually
        let mut v1_data = Vec::new();
        v1_data.extend_from_slice(&(plaintext.len() as u64).to_le_bytes());
        v1_data.extend_from_slice(&salt);
        v1_data.extend_from_slice(&nonce_bytes);
        v1_data.extend_from_slice(&ciphertext);
        fs::write(&encrypted, &v1_data).unwrap();

        decrypt_file(&encrypted, &decrypted, passphrase).unwrap();
        assert_eq!(fs::read(&decrypted).unwrap(), plaintext);
    }

    #[test]
    fn test_encrypt_decrypt_large_binary() {
        // Encrypt a moderately large binary payload (2.5 MiB) to exercise streaming read/write.
        let dir = tempfile::tempdir().unwrap();
        let input = dir.path().join("big.bin");
        let encrypted = dir.path().join("encrypted.bin");
        let decrypted = dir.path().join("decrypted.bin");

        let original: Vec<u8> = (0u8..=255).cycle().take(2 * 1024 * 1024 + 512 * 1024).collect();
        fs::write(&input, &original).unwrap();

        let meta = encrypt_file(&input, &encrypted, "multi-chunk-pass").unwrap();
        assert_eq!(meta.file_size, original.len() as u64);

        decrypt_file(&encrypted, &decrypted, "multi-chunk-pass").unwrap();
        assert_eq!(fs::read(&decrypted).unwrap(), original);
    }
}
