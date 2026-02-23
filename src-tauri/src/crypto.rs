use std::fs;
use std::io::Write;
use std::path::Path;

use aes_gcm::aead::Aead;
use aes_gcm::{Aes256Gcm, KeyInit, Nonce};
use argon2::Argon2;
use md5::{Digest, Md5};
use rand::RngCore;

use crate::error::AppError;

const SALT_LEN: usize = 16;
const NONCE_LEN: usize = 12;
const FILESIZE_HEADER_LEN: usize = 8;
const HEADER_LEN: usize = FILESIZE_HEADER_LEN + SALT_LEN + NONCE_LEN; // 36 bytes

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

fn md5_of_bytes(data: &[u8]) -> String {
    let mut hasher = Md5::new();
    hasher.update(data);
    hex::encode(hasher.finalize())
}

fn md5_of_file(path: &Path) -> Result<String, AppError> {
    let data = fs::read(path)?;
    Ok(md5_of_bytes(&data))
}

pub fn generate_temp_path(temp_dir: &Path) -> std::path::PathBuf {
    let id = uuid::Uuid::new_v4();
    temp_dir.join(format!("harpocrates-tmp-{}", id))
}

/// Encrypt a file using AES-256-GCM with Argon2id key derivation.
///
/// Format: [8 bytes filesize LE][16 bytes salt][12 bytes nonce][ciphertext + 16 byte GCM tag]
pub fn encrypt_file(
    input_path: &Path,
    output_path: &Path,
    passphrase: &str,
) -> Result<EncryptedFileMeta, AppError> {
    let plaintext = fs::read(input_path)?;
    let file_size = plaintext.len() as u64;

    // Compute original MD5
    let original_md5 = md5_of_bytes(&plaintext);

    // Generate random salt and nonce
    let mut salt = [0u8; SALT_LEN];
    let mut nonce_bytes = [0u8; NONCE_LEN];
    rand::thread_rng().fill_bytes(&mut salt);
    rand::thread_rng().fill_bytes(&mut nonce_bytes);

    // Derive key from passphrase + salt
    let key = derive_key(passphrase.as_bytes(), &salt)?;
    let cipher = Aes256Gcm::new_from_slice(&key)
        .map_err(|e| AppError::Crypto(format!("Failed to create cipher: {}", e)))?;

    let nonce = Nonce::from_slice(&nonce_bytes);

    // Encrypt
    let ciphertext = cipher
        .encrypt(nonce, plaintext.as_ref())
        .map_err(|e| AppError::Crypto(format!("Encryption failed: {}", e)))?;

    // Build output: header + ciphertext
    let mut output = Vec::with_capacity(HEADER_LEN + ciphertext.len());
    output.extend_from_slice(&file_size.to_le_bytes()); // 8 bytes
    output.extend_from_slice(&salt);                     // 16 bytes
    output.extend_from_slice(&nonce_bytes);              // 12 bytes
    output.extend_from_slice(&ciphertext);               // ciphertext + GCM tag

    // Compute encrypted MD5
    let encrypted_md5 = md5_of_bytes(&output);

    // Write to output file
    let mut file = fs::File::create(output_path)?;
    file.write_all(&output)?;
    file.sync_all()?;

    Ok(EncryptedFileMeta {
        original_md5,
        encrypted_md5,
        file_size,
    })
}

/// Decrypt a file that was encrypted with encrypt_file.
pub fn decrypt_file(
    input_path: &Path,
    output_path: &Path,
    passphrase: &str,
) -> Result<(), AppError> {
    let data = fs::read(input_path)?;

    if data.len() < HEADER_LEN {
        return Err(AppError::Crypto(
            "Encrypted file is too small to contain a valid header".into(),
        ));
    }

    // Parse header
    let file_size_bytes: [u8; 8] = data[..FILESIZE_HEADER_LEN].try_into().unwrap();
    let expected_size = u64::from_le_bytes(file_size_bytes);

    let salt = &data[FILESIZE_HEADER_LEN..FILESIZE_HEADER_LEN + SALT_LEN];
    let nonce_bytes = &data[FILESIZE_HEADER_LEN + SALT_LEN..HEADER_LEN];
    let ciphertext = &data[HEADER_LEN..];

    // Derive key from passphrase + salt
    let key = derive_key(passphrase.as_bytes(), salt)?;
    let cipher = Aes256Gcm::new_from_slice(&key)
        .map_err(|e| AppError::Crypto(format!("Failed to create cipher: {}", e)))?;

    let nonce = Nonce::from_slice(nonce_bytes);

    // Decrypt (GCM tag verification happens here)
    let plaintext = cipher
        .decrypt(nonce, ciphertext)
        .map_err(|e| AppError::Crypto(format!("Decryption failed (wrong key or tampered data): {}", e)))?;

    // Verify file size matches
    if plaintext.len() as u64 != expected_size {
        return Err(AppError::Crypto(format!(
            "Decrypted size {} does not match expected size {}",
            plaintext.len(),
            expected_size
        )));
    }

    // Write decrypted data
    let mut file = fs::File::create(output_path)?;
    file.write_all(&plaintext)?;
    file.sync_all()?;

    Ok(())
}

/// Compute the MD5 hash of a file on disk.
pub fn compute_file_md5(path: &Path) -> Result<String, AppError> {
    md5_of_file(path)
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

        // Encrypted file should be larger than original (header + tag)
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

        // Write a file too small to have a valid header
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

        // Tamper with the encrypted data (flip a byte in the ciphertext area)
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

        // Create some temp files and a non-temp file
        fs::write(dir.path().join("harpocrates-tmp-abc"), "tmp1").unwrap();
        fs::write(dir.path().join("harpocrates-tmp-def"), "tmp2").unwrap();
        fs::write(dir.path().join("other-file.txt"), "keep").unwrap();

        let cleaned = cleanup_temp_files(dir.path()).unwrap();
        assert_eq!(cleaned, 2);

        // Non-temp file should remain
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
        // Due to random salt and nonce, encrypting the same file twice should produce different ciphertext
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
        // Check header: first 8 bytes should be file size in LE
        let size_bytes: [u8; 8] = enc_data[..8].try_into().unwrap();
        let stored_size = u64::from_le_bytes(size_bytes);
        assert_eq!(stored_size, data.len() as u64);

        // Total header: 8 (size) + 16 (salt) + 12 (nonce) = 36 bytes
        assert!(enc_data.len() >= 36);
    }
}
