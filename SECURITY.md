# Security

This document describes Harpocrates's security model, the cryptographic primitives used, and the threat model it is designed to protect against.

---

## Encryption

### Algorithm

Every chunk is encrypted with **XChaCha20-Poly1305**, an authenticated encryption scheme.

- **XChaCha20** provides 256-bit symmetric encryption with a 192-bit nonce, making random nonce reuse essentially impossible.
- **Poly1305** is a one-time MAC providing 128-bit authentication. If any byte of the ciphertext or tag is modified, decryption fails before returning any plaintext — Harpocrates can detect corrupted or tampered chunks.

### Key

The encryption key is a **256-bit (32-byte) random value** generated once at profile creation and displayed as a 64-character hex string. Harpocrates never stores it — the user is responsible for keeping it safe. The key is placed in the OS keychain only for the duration of the session (see Credential Storage below).

There is no password-based key derivation (no KDF, no salt). The key is the user's secret directly.

### Encrypted Chunk Format

Each chunk stored in S3 uses the following binary layout:

```
Offset   Size   Field
──────   ────   ─────
0        12     Random nonce (XChaCha20-Poly1305 nonce)
12       N      Ciphertext (same length as plaintext chunk)
12+N     16     Poly1305 authentication tag
```

Total overhead per chunk: **28 bytes**.

The nonce is randomly generated fresh for every encryption call, making each ciphertext unique even for identical plaintext.

### Chunk Identity

Each chunk is identified by `HMAC-SHA256(encryption_key, plaintext_chunk)`, hex-encoded. This hash:
- Serves as the deduplication key: the same chunk content always produces the same hash within a profile, so it is stored only once.
- Is computationally indistinguishable from random to anyone without the encryption key, revealing no information about chunk content.
- Is profile-scoped: the same file content on two different profiles (different keys) produces different hashes.

---

## Credential Storage

### Encryption Key

The encryption key is:

- Generated once at profile creation and displayed to you.
- **Never stored by Harpocrates** — not in the database, not in any file, not in the OS keychain.
- Your sole responsibility to keep safe.

If you lose the encryption key, there is no recovery path. Your files remain in S3 but cannot be decrypted.

### S3 Credentials (Access Key / Secret Key)

S3 credentials are stored in the **OS keychain**:

| Platform | Keychain |
|----------|----------|
| macOS | Keychain Access |
| Windows | Windows Credential Manager |
| Linux | Secret Service API (e.g. GNOME Keyring, KWallet) |

They are never written to `harpocrates.db` or any plaintext file.

---

## What Harpocrates Protects Against

**An attacker with read access to your S3 bucket** cannot:
- Read any file contents (all chunks are XChaCha20-Poly1305 encrypted)
- Determine filenames, directory structure, or file counts beyond object count
- Know original file sizes or chunk boundaries (no size metadata in S3 objects)
- Infer chunk content from S3 keys (keys are HMACs, computationally opaque without the encryption key)
- Forge or undetectably modify chunks (Poly1305 authentication rejects tampered objects)

**An attacker with access to your local `harpocrates.db`** can see:
- Filenames and local paths
- File sizes and chunk counts
- MD5 hashes of original plaintext files (`file_entry.original_md5`)
- HMAC-SHA256 chunk hashes (opaque without the encryption key)
- Profile names, S3 endpoints, and bucket names
- Share manifest S3 keys and associated filenames

They cannot read S3 credentials (in the OS keychain) or decrypt any files (encryption key not stored).

**An attacker who only has the S3 bucket credentials** (access key + secret key, but not the encryption key) can:
- List, download, upload, and delete objects
- Observe object keys (HMAC-based for chunks, UUID-based for manifests), sizes, and creation timestamps

They cannot decrypt any file contents without the encryption key.

---

## What Harpocrates Does NOT Protect Against

- **An attacker with your encryption key** — they can decrypt everything.
- **An attacker with full local machine access** — they can observe the running process, intercept IPC calls, or access the OS keychain.
- **Traffic analysis** — an attacker monitoring your network traffic can observe the timing and size of S3 uploads/downloads, which may leak information about file access patterns even if they cannot read the content.
- **S3 provider access** — your S3 provider can see object sizes, access times, and object counts.
- **Metadata in `harpocrates.db`** — local filenames and paths are stored in plaintext. If an attacker accesses your local disk, they can see which files you have backed up (but not their contents).
- **Deletion attacks** — an attacker with write access to your bucket can delete all your objects. Harpocrates does not provide redundancy or versioning.

---

## Share Manifests

When you create a share manifest:

1. Harpocrates constructs a JSON list of file identifiers (MD5s) and their filenames.
2. This list is encrypted with **your encryption key** using XChaCha20-Poly1305 (the same scheme used for chunks).
3. The encrypted manifest is uploaded to S3 under the `m/` namespace (e.g. `m/{uuid}`).
4. The manifest S3 key is given to the recipient.

The recipient:
- Must have a Harpocrates profile pointing at the same S3 bucket.
- Downloads the encrypted manifest using the UUID.
- **Decrypts it using your encryption key** — the recipient needs your encryption key to access a share manifest.

> **Important:** Sharing a manifest UUID with someone gives them your encryption key access to the manifest and the files it references. They do not, however, get your S3 credentials or access to any other files.

### Revoking a Share

Revoking a manifest deletes the manifest object from S3. This prevents future access. If the recipient has already downloaded the files, revocation does not undo that.

### Scramble

The Scramble feature moves the S3 chunks of selected files to new random keys. Specifically:

- Only chunks **exclusively referenced** by the selected files are moved (shared chunks used by other files are left in place to preserve deduplication).
- Each exclusive chunk is copied to a new `c/{random-uuid}` key and the original is deleted.
- The DB is updated to reflect the new keys.
- Share manifests referencing the scrambled files are invalidated.

Scramble does **not** change the encryption key or re-encrypt the data. It is useful to:
- Invalidate share tokens (the old S3 keys no longer exist).
- Sever the link between old S3 access log entries and the current file location.

---

## Deduplication and Information Leakage

Harpocrates uses two levels of deduplication:

1. **Per-chunk (HMAC-based):** chunks with identical plaintext content share one S3 object, identified by `HMAC-SHA256(key, chunk)`. The HMAC reveals nothing about chunk content to anyone without the key.

2. **Per-file (MD5-based):** if a file's complete MD5 matches an existing `file_entry`, only the local path mapping is updated — no new S3 objects are written.

Information leakage implications:
- An attacker with access to `harpocrates.db` can see which local paths share the same `file_entry` (and thus the same content), and can observe `original_md5` values.
- An attacker with **both** the DB and the encryption key could reconstruct which chunks are shared across files.
- An attacker **without** the encryption key cannot interpret chunk hashes or use them to infer content.
- Cross-profile deduplication is impossible — chunk HMACs are key-scoped, so two profiles (even on the same bucket) produce independent hashes for the same content.

---

## Reporting Security Issues

Please do not report security vulnerabilities via public GitHub issues. Instead, contact the maintainer directly.
