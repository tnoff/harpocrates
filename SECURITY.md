# Security

This document describes Harpocrates's security model, the cryptographic primitives used, and the threat model it is designed to protect against.

---

## Encryption

### Algorithm

Every file is encrypted with **AES-256-GCM** (Advanced Encryption Standard in Galois/Counter Mode).

- **AES-256** provides 256-bit symmetric encryption, considered computationally infeasible to brute-force.
- **GCM** is an authenticated encryption mode. It produces a 128-bit authentication tag alongside the ciphertext. If the ciphertext or tag is tampered with in any way, decryption will fail with an authentication error before any plaintext is returned. This means Harpocrates can detect corrupted or modified files.

### Key Derivation

Harpocrates accepts a passphrase/key string and derives the actual AES key using **Argon2id**:

- Argon2id is the winner of the Password Hashing Competition and is recommended by NIST.
- A **16-byte random salt** is generated fresh for every file encryption. This means even if you encrypt the same file twice with the same key, the two ciphertexts are completely different and reveal nothing about each other.

### Encrypted File Format

Each file stored in S3 uses the following binary format:

```
Offset   Size   Field
──────   ────   ─────
0        8      Original plaintext file size (little-endian u64)
8        16     Random salt (used for Argon2id key derivation)
24       12     Random nonce (used for AES-GCM)
36       N      Ciphertext (same length as plaintext)
36+N     16     GCM authentication tag
```

Total overhead per file: **52 bytes** (36-byte header + 16-byte tag).

The nonce is unique per file (randomly generated), preventing nonce reuse which would be catastrophic for GCM security.

---

## Credential Storage

### Encryption Key

The encryption key (derived from your passphrase) is:

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
- Read any file contents (all objects are AES-256-GCM encrypted)
- Determine filenames, directory structure, or file counts beyond object count
- Know the original file sizes (sizes are encrypted inside the ciphertext)
- Forge or undetectably modify files (GCM authentication will reject tampered objects)

**An attacker with access to your local `harpocrates.db`** can see:
- Filenames and local paths
- File sizes
- MD5 hashes of original and encrypted files
- Profile names, S3 endpoints, and bucket names
- Share manifest UUIDs and associated filenames

They cannot read S3 credentials (in the OS keychain) or decrypt any files (encryption key not stored).

**An attacker who only has the S3 bucket credentials** (access key + secret key, but not the encryption key) can:
- List, download, upload, and delete objects
- Observe object UUIDs, sizes, and creation timestamps

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

1. Harpocrates constructs a JSON list of file UUIDs and their filenames.
2. This list is encrypted with **your encryption key** using the same AES-256-GCM scheme.
3. The encrypted manifest is uploaded to S3 as a new UUID object.
4. The manifest UUID is given to the recipient.

The recipient:
- Must have a Harpocrates profile pointing at the same S3 bucket.
- Downloads the encrypted manifest using the UUID.
- **Decrypts it using your encryption key** — the recipient needs your encryption key to access a share manifest.

> **Important:** Sharing a manifest UUID with someone gives them your encryption key access to the manifest and the files it references. They do not, however, get your S3 credentials or access to any other files.

### Revoking a Share

Revoking a manifest deletes the manifest object from S3. This prevents future access. If the recipient has already downloaded the files, revocation does not undo that.

### Scramble

The Scramble feature re-encrypts selected files under new random S3 UUIDs. It does **not** change the encryption key or re-derive it. Scrambling is useful if:

- You want to invalidate existing share manifests (the old UUIDs no longer exist in S3).
- You want to sever the link between an old object UUID and a file in case someone observed your S3 access logs.

---

## Deduplication and Information Leakage

Harpocrates deduplicates files by MD5 hash of the **plaintext**. If two files have the same content, they share one S3 object. This means:

- An attacker with access to `harpocrates.db` can see which local paths share the same content (same `backup_entry_id` in `local_file`).
- An attacker cannot use deduplication to determine whether two Harpocrates instances (different users) have the same file, because each instance has an independent encryption key and independent object UUIDs.

---

## Reporting Security Issues

Please do not report security vulnerabilities via public GitHub issues. Instead, contact the maintainer directly.
