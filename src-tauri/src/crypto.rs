use std::fs;
use std::io::{BufReader, Read};
use std::path::Path;

use chacha20poly1305::aead::Aead;
use chacha20poly1305::{ChaCha20Poly1305, KeyInit, Nonce};
use hmac::{Hmac, Mac};
use md5::{Digest, Md5};
use rand::RngExt;
use sha2::Sha256;

use crate::error::AppError;

const NONCE_LEN: usize = 12;

type HmacSha256 = Hmac<Sha256>;

// ── Key utilities ─────────────────────────────────────────────────────────────

/// Decode a 64-char hex string into a 32-byte encryption key.
pub fn decode_encryption_key(hex_key: &str) -> Result<[u8; 32], AppError> {
    let bytes = hex::decode(hex_key)
        .map_err(|e| AppError::Crypto(format!("Invalid hex key: {}", e)))?;
    bytes
        .try_into()
        .map_err(|_| AppError::Crypto("Encryption key must be exactly 32 bytes (64 hex chars)".into()))
}

/// Derive a 32-byte key from an arbitrary passphrase using SHA-256.
/// Used when the user provides a non-hex import key (e.g. a memorable passphrase).
/// The same passphrase always produces the same key.
pub fn derive_key_from_passphrase(passphrase: &str) -> [u8; 32] {
    sha2::Sha256::digest(passphrase.as_bytes()).into()
}

// ── Chunk HMAC ────────────────────────────────────────────────────────────────

/// Compute HMAC-SHA256(key, data) and return the result as a lowercase hex string.
/// Used as the chunk's content-addressed identity: same content + same key → same hash.
pub fn compute_chunk_hmac(key_bytes: &[u8], data: &[u8]) -> String {
    let mut mac = <HmacSha256 as Mac>::new_from_slice(key_bytes)
        .expect("HMAC accepts any key length");
    mac.update(data);
    hex::encode(mac.finalize().into_bytes())
}

// ── Per-chunk encryption ──────────────────────────────────────────────────────

/// Encrypt a plaintext chunk with ChaCha20-Poly1305.
///
/// Output format: `[12-byte random nonce][ciphertext + 16-byte Poly1305 tag]`
/// Total overhead per chunk: 28 bytes.
pub fn encrypt_chunk(key_bytes: &[u8; 32], plaintext: &[u8]) -> Result<Vec<u8>, AppError> {
    let cipher = ChaCha20Poly1305::new_from_slice(key_bytes)
        .map_err(|e| AppError::Crypto(format!("Failed to create cipher: {}", e)))?;

    let mut nonce_bytes = [0u8; NONCE_LEN];
    rand::rng().fill(&mut nonce_bytes);
    let nonce = Nonce::from_slice(&nonce_bytes);

    let ciphertext = cipher
        .encrypt(nonce, plaintext)
        .map_err(|e| AppError::Crypto(format!("Chunk encryption failed: {}", e)))?;

    let mut output = Vec::with_capacity(NONCE_LEN + ciphertext.len());
    output.extend_from_slice(&nonce_bytes);
    output.extend_from_slice(&ciphertext);
    Ok(output)
}

/// Decrypt a chunk produced by `encrypt_chunk`.
///
/// Expects format: `[12-byte nonce][ciphertext + 16-byte tag]`
pub fn decrypt_chunk(key_bytes: &[u8; 32], encrypted: &[u8]) -> Result<Vec<u8>, AppError> {
    if encrypted.len() < NONCE_LEN + 16 {
        return Err(AppError::Crypto(
            "Encrypted chunk too small to contain nonce and auth tag".into(),
        ));
    }

    let nonce = Nonce::from_slice(&encrypted[..NONCE_LEN]);
    let ciphertext = &encrypted[NONCE_LEN..];

    let cipher = ChaCha20Poly1305::new_from_slice(key_bytes)
        .map_err(|e| AppError::Crypto(format!("Failed to create cipher: {}", e)))?;

    cipher
        .decrypt(nonce, ciphertext)
        .map_err(|e| AppError::Crypto(format!("Chunk decryption failed (wrong key or tampered data): {}", e)))
}

// ── File utilities (kept for restore verification) ────────────────────────────

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

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    // ── decode_encryption_key ──────────────────────────────────────────────────

    #[test]
    fn test_decode_encryption_key_valid() {
        let hex = "0102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f20";
        let key = decode_encryption_key(hex).unwrap();
        assert_eq!(key.len(), 32);
        assert_eq!(key[0], 0x01);
        assert_eq!(key[31], 0x20);
    }

    #[test]
    fn test_decode_encryption_key_wrong_length() {
        let hex = "deadbeef"; // only 4 bytes
        assert!(decode_encryption_key(hex).is_err());
    }

    #[test]
    fn test_decode_encryption_key_invalid_hex() {
        let hex = "zzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzz";
        assert!(decode_encryption_key(hex).is_err());
    }

    // ── compute_chunk_hmac ─────────────────────────────────────────────────────

    #[test]
    fn test_hmac_deterministic() {
        let key = b"test-key-32-bytes-padded-to-len!";
        let data = b"hello world";
        let h1 = compute_chunk_hmac(key, data);
        let h2 = compute_chunk_hmac(key, data);
        assert_eq!(h1, h2);
    }

    #[test]
    fn test_hmac_different_data_different_hash() {
        let key = b"test-key-32-bytes-padded-to-len!";
        let h1 = compute_chunk_hmac(key, b"chunk A");
        let h2 = compute_chunk_hmac(key, b"chunk B");
        assert_ne!(h1, h2);
    }

    #[test]
    fn test_hmac_different_key_different_hash() {
        let h1 = compute_chunk_hmac(b"key-one", b"same data");
        let h2 = compute_chunk_hmac(b"key-two", b"same data");
        assert_ne!(h1, h2);
    }

    #[test]
    fn test_hmac_output_is_hex_string() {
        let hash = compute_chunk_hmac(b"any-key", b"any-data");
        assert_eq!(hash.len(), 64); // 32 bytes → 64 hex chars
        assert!(hash.chars().all(|c| c.is_ascii_hexdigit()));
    }

    // ── encrypt_chunk / decrypt_chunk ──────────────────────────────────────────

    fn test_key() -> [u8; 32] {
        let mut k = [0u8; 32];
        for (i, b) in k.iter_mut().enumerate() {
            *b = i as u8;
        }
        k
    }

    #[test]
    fn test_encrypt_decrypt_roundtrip() {
        let key = test_key();
        let plaintext = b"Hello, Harpocrates! Chunk encryption test.";
        let encrypted = encrypt_chunk(&key, plaintext).unwrap();
        let decrypted = decrypt_chunk(&key, &encrypted).unwrap();
        assert_eq!(decrypted, plaintext);
    }

    #[test]
    fn test_encrypt_empty_chunk() {
        let key = test_key();
        let encrypted = encrypt_chunk(&key, b"").unwrap();
        let decrypted = decrypt_chunk(&key, &encrypted).unwrap();
        assert!(decrypted.is_empty());
    }

    #[test]
    fn test_encrypted_size() {
        let key = test_key();
        let plaintext = b"test data";
        let encrypted = encrypt_chunk(&key, plaintext).unwrap();
        // nonce(12) + ciphertext(n) + tag(16)
        assert_eq!(encrypted.len(), NONCE_LEN + plaintext.len() + 16);
    }

    #[test]
    fn test_encrypt_produces_different_output_each_time() {
        let key = test_key();
        let data = b"same data every time";
        let enc1 = encrypt_chunk(&key, data).unwrap();
        let enc2 = encrypt_chunk(&key, data).unwrap();
        // Different random nonces → different ciphertext
        assert_ne!(enc1, enc2);
    }

    #[test]
    fn test_decrypt_wrong_key_fails() {
        let key1 = test_key();
        let mut key2 = test_key();
        key2[0] ^= 0xFF;

        let encrypted = encrypt_chunk(&key1, b"secret").unwrap();
        let result = decrypt_chunk(&key2, &encrypted);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("wrong key") || err.contains("tampered"));
    }

    #[test]
    fn test_decrypt_tampered_ciphertext_fails() {
        let key = test_key();
        let mut encrypted = encrypt_chunk(&key, b"important data").unwrap();
        // Flip a byte in the ciphertext (after the nonce)
        let last = encrypted.len() - 1;
        encrypted[last] ^= 0xFF;
        assert!(decrypt_chunk(&key, &encrypted).is_err());
    }

    #[test]
    fn test_decrypt_too_short_fails() {
        let key = test_key();
        let short = vec![0u8; 20]; // < 12 + 16 = 28
        assert!(decrypt_chunk(&key, &short).is_err());
    }

    #[test]
    fn test_large_chunk_roundtrip() {
        let key = test_key();
        // 10 MiB chunk (default chunk size)
        let plaintext: Vec<u8> = (0u8..=255).cycle().take(10 * 1024 * 1024).collect();
        let encrypted = encrypt_chunk(&key, &plaintext).unwrap();
        let decrypted = decrypt_chunk(&key, &encrypted).unwrap();
        assert_eq!(decrypted, plaintext);
    }

    // ── compute_file_md5 ───────────────────────────────────────────────────────

    #[test]
    fn test_compute_file_md5() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("test.txt");
        fs::write(&file, b"hello world").unwrap();
        let md5 = compute_file_md5(&file).unwrap();
        assert_eq!(md5, "5eb63bbbe01eeed093cb22bb8f5acdc3");
    }


}
