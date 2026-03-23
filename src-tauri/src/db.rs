use std::sync::Mutex;

use rusqlite::{params, Connection, OptionalExtension};

use crate::error::AppError;

pub struct DbState(pub Mutex<Connection>);

impl DbState {
    /// Acquire the database connection lock, converting a poison error into `AppError::Lock`.
    pub fn conn(&self) -> Result<std::sync::MutexGuard<'_, Connection>, AppError> {
        self.0.lock().map_err(|e| AppError::Lock(e.to_string()))
    }
}

const SCHEMA_VERSION: i32 = 5;

pub fn init_database(path: &str) -> Result<Connection, AppError> {
    let conn = Connection::open(path)?;
    conn.execute_batch("PRAGMA foreign_keys = ON;")?;

    let version: i32 = conn.pragma_query_value(None, "user_version", |row| row.get(0))?;

    if version < SCHEMA_VERSION {
        migrate_to_v5(&conn)?;
        conn.pragma_update(None, "user_version", SCHEMA_VERSION)?;
    }

    Ok(conn)
}

/// Drop all old tables and create the new v5 schema from scratch.
fn migrate_to_v5(conn: &Connection) -> Result<(), AppError> {
    conn.execute_batch(
        "
        PRAGMA foreign_keys = OFF;
        BEGIN;

        DROP TABLE IF EXISTS share_manifest_entry;
        DROP TABLE IF EXISTS local_file;
        DROP TABLE IF EXISTS share_manifest;
        DROP TABLE IF EXISTS file_chunk;
        DROP TABLE IF EXISTS file_entry;
        DROP TABLE IF EXISTS chunk;
        DROP TABLE IF EXISTS backup_entry;
        DROP TABLE IF EXISTS profile;

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
            chunk_size_bytes INTEGER NOT NULL DEFAULT 10485760,
            is_active BOOLEAN NOT NULL DEFAULT 0,
            created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
        );

        -- One row per unique chunk stored in S3 (content-addressed)
        CREATE TABLE chunk (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            profile_id INTEGER NOT NULL REFERENCES profile(id),
            chunk_hash TEXT NOT NULL,
            s3_key TEXT NOT NULL,
            encrypted_size INTEGER NOT NULL,
            created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
            UNIQUE(profile_id, chunk_hash)
        );

        -- One row per unique file content (whole-file dedup by MD5)
        CREATE TABLE file_entry (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            profile_id INTEGER NOT NULL REFERENCES profile(id),
            original_md5 TEXT NOT NULL,
            total_size INTEGER NOT NULL,
            chunk_count INTEGER NOT NULL,
            created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
            UNIQUE(profile_id, original_md5)
        );

        -- Ordered chunk list for each file_entry
        CREATE TABLE file_chunk (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            file_entry_id INTEGER NOT NULL REFERENCES file_entry(id),
            chunk_index INTEGER NOT NULL,
            chunk_id INTEGER NOT NULL REFERENCES chunk(id),
            UNIQUE(file_entry_id, chunk_index)
        );

        -- Local paths mapped to file_entries
        CREATE TABLE local_file (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            file_entry_id INTEGER NOT NULL REFERENCES file_entry(id),
            local_path TEXT NOT NULL UNIQUE,
            cached_mtime REAL,
            cached_size INTEGER,
            updated_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
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
            file_entry_id INTEGER NOT NULL REFERENCES file_entry(id),
            filename TEXT NOT NULL
        );

        COMMIT;
        PRAGMA foreign_keys = ON;
        ",
    )?;
    Ok(())
}

// ── Profile CRUD ──

#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub struct Profile {
    pub id: i64,
    pub name: String,
    pub mode: String,
    pub s3_endpoint: String,
    pub s3_region: Option<String>,
    pub s3_bucket: String,
    pub extra_env: Option<String>,
    pub relative_path: Option<String>,
    pub temp_directory: Option<String>,
    pub s3_key_prefix: Option<String>,
    pub chunk_size_bytes: i64,
    pub is_active: bool,
    pub created_at: String,
}

#[allow(clippy::too_many_arguments)]
pub fn insert_profile(
    conn: &Connection,
    name: &str,
    mode: &str,
    s3_endpoint: &str,
    s3_region: Option<&str>,
    s3_bucket: &str,
    extra_env: Option<&str>,
    relative_path: Option<&str>,
    temp_directory: Option<&str>,
    s3_key_prefix: Option<&str>,
    chunk_size_bytes: i64,
) -> Result<i64, AppError> {
    conn.execute(
        "INSERT INTO profile (name, mode, s3_endpoint, s3_region, s3_bucket, extra_env, relative_path, temp_directory, s3_key_prefix, chunk_size_bytes)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        params![name, mode, s3_endpoint, s3_region, s3_bucket, extra_env, relative_path, temp_directory, s3_key_prefix, chunk_size_bytes],
    )?;
    Ok(conn.last_insert_rowid())
}

pub fn get_profile_by_id(conn: &Connection, id: i64) -> Result<Option<Profile>, AppError> {
    let mut stmt = conn.prepare(
        "SELECT id, name, mode, s3_endpoint, s3_region, s3_bucket, extra_env, relative_path, temp_directory, is_active, created_at, s3_key_prefix, chunk_size_bytes
         FROM profile WHERE id = ?1",
    )?;
    let profile = stmt
        .query_row(params![id], |row| {
            Ok(Profile {
                id: row.get(0)?,
                name: row.get(1)?,
                mode: row.get(2)?,
                s3_endpoint: row.get(3)?,
                s3_region: row.get(4)?,
                s3_bucket: row.get(5)?,
                extra_env: row.get(6)?,
                relative_path: row.get(7)?,
                temp_directory: row.get(8)?,
                is_active: row.get(9)?,
                created_at: row.get(10)?,
                s3_key_prefix: row.get(11)?,
                chunk_size_bytes: row.get(12)?,
            })
        })
        .optional()?;
    Ok(profile)
}

pub fn list_profiles(conn: &Connection) -> Result<Vec<Profile>, AppError> {
    let mut stmt = conn.prepare(
        "SELECT id, name, mode, s3_endpoint, s3_region, s3_bucket, extra_env, relative_path, temp_directory, is_active, created_at, s3_key_prefix, chunk_size_bytes
         FROM profile ORDER BY id",
    )?;
    let profiles = stmt
        .query_map([], |row| {
            Ok(Profile {
                id: row.get(0)?,
                name: row.get(1)?,
                mode: row.get(2)?,
                s3_endpoint: row.get(3)?,
                s3_region: row.get(4)?,
                s3_bucket: row.get(5)?,
                extra_env: row.get(6)?,
                relative_path: row.get(7)?,
                temp_directory: row.get(8)?,
                is_active: row.get(9)?,
                created_at: row.get(10)?,
                s3_key_prefix: row.get(11)?,
                chunk_size_bytes: row.get(12)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(profiles)
}

pub fn delete_profile(conn: &Connection, id: i64) -> Result<(), AppError> {
    conn.execute("DELETE FROM profile WHERE id = ?1", params![id])?;
    Ok(())
}

pub fn set_active_profile(conn: &Connection, id: i64) -> Result<(), AppError> {
    conn.execute("UPDATE profile SET is_active = 0 WHERE is_active = 1", [])?;
    conn.execute("UPDATE profile SET is_active = 1 WHERE id = ?1", params![id])?;
    Ok(())
}

// ── Chunk CRUD ──

#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub struct Chunk {
    pub id: i64,
    pub profile_id: i64,
    pub chunk_hash: String,
    pub s3_key: String,
    pub encrypted_size: i64,
    pub created_at: String,
}

pub fn insert_chunk(
    conn: &Connection,
    profile_id: i64,
    chunk_hash: &str,
    s3_key: &str,
    encrypted_size: i64,
) -> Result<i64, AppError> {
    conn.execute(
        "INSERT INTO chunk (profile_id, chunk_hash, s3_key, encrypted_size)
         VALUES (?1, ?2, ?3, ?4)",
        params![profile_id, chunk_hash, s3_key, encrypted_size],
    )?;
    Ok(conn.last_insert_rowid())
}

pub fn get_chunk_id_by_hash(
    conn: &Connection,
    profile_id: i64,
    chunk_hash: &str,
) -> Result<Option<i64>, AppError> {
    let mut stmt = conn.prepare(
        "SELECT id FROM chunk WHERE profile_id = ?1 AND chunk_hash = ?2",
    )?;
    let id = stmt
        .query_row(params![profile_id, chunk_hash], |row| row.get(0))
        .optional()?;
    Ok(id)
}

pub fn get_chunk_by_id(conn: &Connection, id: i64) -> Result<Option<Chunk>, AppError> {
    let mut stmt = conn.prepare(
        "SELECT id, profile_id, chunk_hash, s3_key, encrypted_size, created_at
         FROM chunk WHERE id = ?1",
    )?;
    let chunk = stmt
        .query_row(params![id], |row| {
            Ok(Chunk {
                id: row.get(0)?,
                profile_id: row.get(1)?,
                chunk_hash: row.get(2)?,
                s3_key: row.get(3)?,
                encrypted_size: row.get(4)?,
                created_at: row.get(5)?,
            })
        })
        .optional()?;
    Ok(chunk)
}

pub fn update_chunk_s3_key(conn: &Connection, id: i64, new_s3_key: &str) -> Result<(), AppError> {
    conn.execute(
        "UPDATE chunk SET s3_key = ?1 WHERE id = ?2",
        params![new_s3_key, id],
    )?;
    Ok(())
}

pub fn delete_chunk(conn: &Connection, id: i64) -> Result<(), AppError> {
    conn.execute("DELETE FROM chunk WHERE id = ?1", params![id])?;
    Ok(())
}

/// List all chunks for a profile (used by orphan detection).
pub fn list_chunks(conn: &Connection, profile_id: i64) -> Result<Vec<Chunk>, AppError> {
    let mut stmt = conn.prepare(
        "SELECT id, profile_id, chunk_hash, s3_key, encrypted_size, created_at
         FROM chunk WHERE profile_id = ?1",
    )?;
    let chunks = stmt
        .query_map(params![profile_id], |row| {
            Ok(Chunk {
                id: row.get(0)?,
                profile_id: row.get(1)?,
                chunk_hash: row.get(2)?,
                s3_key: row.get(3)?,
                encrypted_size: row.get(4)?,
                created_at: row.get(5)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(chunks)
}

// ── FileEntry CRUD ──

#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub struct FileEntry {
    pub id: i64,
    pub profile_id: i64,
    pub original_md5: String,
    pub total_size: i64,
    pub chunk_count: i64,
    pub created_at: String,
}

pub fn insert_file_entry(
    conn: &Connection,
    profile_id: i64,
    original_md5: &str,
    total_size: i64,
    chunk_count: i64,
) -> Result<i64, AppError> {
    conn.execute(
        "INSERT INTO file_entry (profile_id, original_md5, total_size, chunk_count)
         VALUES (?1, ?2, ?3, ?4)",
        params![profile_id, original_md5, total_size, chunk_count],
    )?;
    Ok(conn.last_insert_rowid())
}

pub fn get_file_entry_by_id(conn: &Connection, id: i64) -> Result<Option<FileEntry>, AppError> {
    let mut stmt = conn.prepare(
        "SELECT id, profile_id, original_md5, total_size, chunk_count, created_at
         FROM file_entry WHERE id = ?1",
    )?;
    let entry = stmt
        .query_row(params![id], |row| {
            Ok(FileEntry {
                id: row.get(0)?,
                profile_id: row.get(1)?,
                original_md5: row.get(2)?,
                total_size: row.get(3)?,
                chunk_count: row.get(4)?,
                created_at: row.get(5)?,
            })
        })
        .optional()?;
    Ok(entry)
}

pub fn get_file_entry_by_md5(
    conn: &Connection,
    profile_id: i64,
    md5: &str,
) -> Result<Option<FileEntry>, AppError> {
    let mut stmt = conn.prepare(
        "SELECT id, profile_id, original_md5, total_size, chunk_count, created_at
         FROM file_entry WHERE profile_id = ?1 AND original_md5 = ?2",
    )?;
    let entry = stmt
        .query_row(params![profile_id, md5], |row| {
            Ok(FileEntry {
                id: row.get(0)?,
                profile_id: row.get(1)?,
                original_md5: row.get(2)?,
                total_size: row.get(3)?,
                chunk_count: row.get(4)?,
                created_at: row.get(5)?,
            })
        })
        .optional()?;
    Ok(entry)
}

pub fn list_file_entries(conn: &Connection, profile_id: i64) -> Result<Vec<FileEntry>, AppError> {
    let mut stmt = conn.prepare(
        "SELECT id, profile_id, original_md5, total_size, chunk_count, created_at
         FROM file_entry WHERE profile_id = ?1 ORDER BY id",
    )?;
    let entries = stmt
        .query_map(params![profile_id], |row| {
            Ok(FileEntry {
                id: row.get(0)?,
                profile_id: row.get(1)?,
                original_md5: row.get(2)?,
                total_size: row.get(3)?,
                chunk_count: row.get(4)?,
                created_at: row.get(5)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(entries)
}

pub fn delete_file_entry(conn: &Connection, id: i64) -> Result<(), AppError> {
    conn.execute("DELETE FROM file_entry WHERE id = ?1", params![id])?;
    Ok(())
}

// ── FileChunk CRUD ──

#[allow(dead_code)]
#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub struct FileChunk {
    pub id: i64,
    pub file_entry_id: i64,
    pub chunk_index: i64,
    pub chunk_id: i64,
}

pub fn insert_file_chunk(
    conn: &Connection,
    file_entry_id: i64,
    chunk_index: i64,
    chunk_id: i64,
) -> Result<i64, AppError> {
    conn.execute(
        "INSERT INTO file_chunk (file_entry_id, chunk_index, chunk_id)
         VALUES (?1, ?2, ?3)",
        params![file_entry_id, chunk_index, chunk_id],
    )?;
    Ok(conn.last_insert_rowid())
}

/// Returns (chunk_index, s3_key) ordered by chunk_index for restoring a file.
pub fn get_chunk_keys_for_file(
    conn: &Connection,
    file_entry_id: i64,
) -> Result<Vec<(usize, String)>, AppError> {
    let mut stmt = conn.prepare(
        "SELECT fc.chunk_index, c.s3_key
         FROM file_chunk fc JOIN chunk c ON fc.chunk_id = c.id
         WHERE fc.file_entry_id = ?1
         ORDER BY fc.chunk_index",
    )?;
    let keys = stmt
        .query_map(params![file_entry_id], |row| {
            Ok((row.get::<_, i64>(0)? as usize, row.get::<_, String>(1)?))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(keys)
}

/// Returns the chunk_ids for a file entry (for scramble deduplication check).
pub fn get_chunk_ids_for_file(
    conn: &Connection,
    file_entry_id: i64,
) -> Result<Vec<i64>, AppError> {
    let mut stmt = conn.prepare(
        "SELECT chunk_id FROM file_chunk WHERE file_entry_id = ?1 ORDER BY chunk_index",
    )?;
    let ids = stmt
        .query_map(params![file_entry_id], |row| row.get(0))?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(ids)
}

/// Count how many distinct file_entries use a given chunk_id.
pub fn count_file_entries_for_chunk(conn: &Connection, chunk_id: i64) -> Result<i64, AppError> {
    let mut stmt = conn.prepare(
        "SELECT COUNT(DISTINCT file_entry_id) FROM file_chunk WHERE chunk_id = ?1",
    )?;
    let count: i64 = stmt.query_row(params![chunk_id], |row| row.get(0))?;
    Ok(count)
}

// ── LocalFile CRUD ──

#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub struct LocalFile {
    pub id: i64,
    pub file_entry_id: i64,
    pub local_path: String,
    pub cached_mtime: Option<f64>,
    pub cached_size: Option<i64>,
    pub updated_at: String,
}

/// Insert or update a local_file record. Updates file_entry_id, mtime, size if path already exists.
pub fn upsert_local_file(
    conn: &Connection,
    file_entry_id: i64,
    local_path: &str,
    cached_mtime: Option<f64>,
    cached_size: Option<i64>,
) -> Result<i64, AppError> {
    conn.execute(
        "INSERT INTO local_file (file_entry_id, local_path, cached_mtime, cached_size)
         VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(local_path) DO UPDATE SET
             file_entry_id = excluded.file_entry_id,
             cached_mtime = excluded.cached_mtime,
             cached_size = excluded.cached_size,
             updated_at = CURRENT_TIMESTAMP",
        params![file_entry_id, local_path, cached_mtime, cached_size],
    )?;
    Ok(conn.last_insert_rowid())
}

pub fn get_local_file_by_path(
    conn: &Connection,
    local_path: &str,
) -> Result<Option<LocalFile>, AppError> {
    let mut stmt = conn.prepare(
        "SELECT id, file_entry_id, local_path, cached_mtime, cached_size, updated_at
         FROM local_file WHERE local_path = ?1",
    )?;
    let file = stmt
        .query_row(params![local_path], |row| {
            Ok(LocalFile {
                id: row.get(0)?,
                file_entry_id: row.get(1)?,
                local_path: row.get(2)?,
                cached_mtime: row.get(3)?,
                cached_size: row.get(4)?,
                updated_at: row.get(5)?,
            })
        })
        .optional()?;
    Ok(file)
}

pub fn get_local_file_by_id(conn: &Connection, id: i64) -> Result<Option<LocalFile>, AppError> {
    let mut stmt = conn.prepare(
        "SELECT id, file_entry_id, local_path, cached_mtime, cached_size, updated_at
         FROM local_file WHERE id = ?1",
    )?;
    let file = stmt
        .query_row(params![id], |row| {
            Ok(LocalFile {
                id: row.get(0)?,
                file_entry_id: row.get(1)?,
                local_path: row.get(2)?,
                cached_mtime: row.get(3)?,
                cached_size: row.get(4)?,
                updated_at: row.get(5)?,
            })
        })
        .optional()?;
    Ok(file)
}

pub fn list_local_files_for_entry(
    conn: &Connection,
    file_entry_id: i64,
) -> Result<Vec<LocalFile>, AppError> {
    let mut stmt = conn.prepare(
        "SELECT id, file_entry_id, local_path, cached_mtime, cached_size, updated_at
         FROM local_file WHERE file_entry_id = ?1 ORDER BY id",
    )?;
    let files = stmt
        .query_map(params![file_entry_id], |row| {
            Ok(LocalFile {
                id: row.get(0)?,
                file_entry_id: row.get(1)?,
                local_path: row.get(2)?,
                cached_mtime: row.get(3)?,
                cached_size: row.get(4)?,
                updated_at: row.get(5)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(files)
}

pub fn delete_local_file(conn: &Connection, id: i64) -> Result<(), AppError> {
    conn.execute("DELETE FROM local_file WHERE id = ?1", params![id])?;
    Ok(())
}

// ── ShareManifest CRUD ──

#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub struct ShareManifest {
    pub id: i64,
    pub profile_id: i64,
    pub manifest_uuid: String,
    pub label: Option<String>,
    pub file_count: i64,
    pub is_valid: bool,
    pub created_at: String,
}

pub fn insert_share_manifest(
    conn: &Connection,
    profile_id: i64,
    manifest_uuid: &str,
    label: Option<&str>,
    file_count: i64,
) -> Result<i64, AppError> {
    conn.execute(
        "INSERT INTO share_manifest (profile_id, manifest_uuid, label, file_count)
         VALUES (?1, ?2, ?3, ?4)",
        params![profile_id, manifest_uuid, label, file_count],
    )?;
    Ok(conn.last_insert_rowid())
}

pub fn get_share_manifest_by_id(conn: &Connection, id: i64) -> Result<Option<ShareManifest>, AppError> {
    let mut stmt = conn.prepare(
        "SELECT id, profile_id, manifest_uuid, label, file_count, is_valid, created_at
         FROM share_manifest WHERE id = ?1",
    )?;
    let manifest = stmt
        .query_row(params![id], |row| {
            Ok(ShareManifest {
                id: row.get(0)?,
                profile_id: row.get(1)?,
                manifest_uuid: row.get(2)?,
                label: row.get(3)?,
                file_count: row.get(4)?,
                is_valid: row.get(5)?,
                created_at: row.get(6)?,
            })
        })
        .optional()?;
    Ok(manifest)
}

pub fn list_share_manifests(conn: &Connection, profile_id: i64) -> Result<Vec<ShareManifest>, AppError> {
    let mut stmt = conn.prepare(
        "SELECT id, profile_id, manifest_uuid, label, file_count, is_valid, created_at
         FROM share_manifest WHERE profile_id = ?1 ORDER BY id",
    )?;
    let manifests = stmt
        .query_map(params![profile_id], |row| {
            Ok(ShareManifest {
                id: row.get(0)?,
                profile_id: row.get(1)?,
                manifest_uuid: row.get(2)?,
                label: row.get(3)?,
                file_count: row.get(4)?,
                is_valid: row.get(5)?,
                created_at: row.get(6)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(manifests)
}

pub fn delete_share_manifest(conn: &Connection, id: i64) -> Result<(), AppError> {
    conn.execute("DELETE FROM share_manifest_entry WHERE share_manifest_id = ?1", params![id])?;
    conn.execute("DELETE FROM share_manifest WHERE id = ?1", params![id])?;
    Ok(())
}

pub fn invalidate_share_manifest(conn: &Connection, id: i64) -> Result<(), AppError> {
    conn.execute(
        "UPDATE share_manifest SET is_valid = 0 WHERE id = ?1",
        params![id],
    )?;
    Ok(())
}

// ── ShareManifestEntry CRUD ──

#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub struct ShareManifestEntry {
    pub id: i64,
    pub share_manifest_id: i64,
    pub file_entry_id: i64,
    pub filename: String,
}

pub fn insert_share_manifest_entry(
    conn: &Connection,
    share_manifest_id: i64,
    file_entry_id: i64,
    filename: &str,
) -> Result<i64, AppError> {
    conn.execute(
        "INSERT INTO share_manifest_entry (share_manifest_id, file_entry_id, filename)
         VALUES (?1, ?2, ?3)",
        params![share_manifest_id, file_entry_id, filename],
    )?;
    Ok(conn.last_insert_rowid())
}

pub fn list_share_manifest_entries(
    conn: &Connection,
    share_manifest_id: i64,
) -> Result<Vec<ShareManifestEntry>, AppError> {
    let mut stmt = conn.prepare(
        "SELECT id, share_manifest_id, file_entry_id, filename
         FROM share_manifest_entry WHERE share_manifest_id = ?1 ORDER BY id",
    )?;
    let entries = stmt
        .query_map(params![share_manifest_id], |row| {
            Ok(ShareManifestEntry {
                id: row.get(0)?,
                share_manifest_id: row.get(1)?,
                file_entry_id: row.get(2)?,
                filename: row.get(3)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(entries)
}

#[cfg(test)]
pub fn open_test_db() -> Connection {
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
    migrate_to_v5(&conn).unwrap();
    conn
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup_db() -> Connection {
        open_test_db()
    }

    // ── Profile tests ──

    #[test]
    fn test_insert_and_get_profile() {
        let conn = setup_db();
        let id = insert_profile(&conn, "test", "read-write", "https://s3.example.com", Some("us-east-1"), "my-bucket", None, None, None, None, 10485760).unwrap();
        let profile = get_profile_by_id(&conn, id).unwrap().unwrap();
        assert_eq!(profile.name, "test");
        assert_eq!(profile.mode, "read-write");
        assert_eq!(profile.s3_endpoint, "https://s3.example.com");
        assert_eq!(profile.s3_region, Some("us-east-1".into()));
        assert_eq!(profile.s3_bucket, "my-bucket");
        assert_eq!(profile.chunk_size_bytes, 10485760);
        assert!(!profile.is_active);
    }

    #[test]
    fn test_list_profiles() {
        let conn = setup_db();
        insert_profile(&conn, "p1", "read-write", "https://s3.a.com", None, "b1", None, None, None, None, 10485760).unwrap();
        insert_profile(&conn, "p2", "read-only", "https://s3.b.com", None, "b2", None, None, None, None, 10485760).unwrap();
        let profiles = list_profiles(&conn).unwrap();
        assert_eq!(profiles.len(), 2);
        assert_eq!(profiles[0].name, "p1");
        assert_eq!(profiles[1].name, "p2");
    }

    #[test]
    fn test_set_active_profile() {
        let conn = setup_db();
        let id1 = insert_profile(&conn, "p1", "read-write", "https://a.com", None, "b1", None, None, None, None, 10485760).unwrap();
        let id2 = insert_profile(&conn, "p2", "read-write", "https://b.com", None, "b2", None, None, None, None, 10485760).unwrap();
        set_active_profile(&conn, id1).unwrap();
        assert!(get_profile_by_id(&conn, id1).unwrap().unwrap().is_active);
        assert!(!get_profile_by_id(&conn, id2).unwrap().unwrap().is_active);

        set_active_profile(&conn, id2).unwrap();
        assert!(!get_profile_by_id(&conn, id1).unwrap().unwrap().is_active);
        assert!(get_profile_by_id(&conn, id2).unwrap().unwrap().is_active);
    }

    #[test]
    fn test_delete_profile() {
        let conn = setup_db();
        let id = insert_profile(&conn, "p1", "read-write", "https://a.com", None, "b1", None, None, None, None, 10485760).unwrap();
        delete_profile(&conn, id).unwrap();
        assert!(get_profile_by_id(&conn, id).unwrap().is_none());
    }

    // ── Chunk tests ──

    #[test]
    fn test_chunk_crud() {
        let conn = setup_db();
        let pid = insert_profile(&conn, "p", "read-write", "https://a.com", None, "b", None, None, None, None, 10485760).unwrap();

        let cid = insert_chunk(&conn, pid, "hash-abc", "prefix/c/hash-abc", 1024).unwrap();
        let chunk = get_chunk_by_id(&conn, cid).unwrap().unwrap();
        assert_eq!(chunk.chunk_hash, "hash-abc");
        assert_eq!(chunk.s3_key, "prefix/c/hash-abc");
        assert_eq!(chunk.encrypted_size, 1024);

        let found_id = get_chunk_id_by_hash(&conn, pid, "hash-abc").unwrap();
        assert_eq!(found_id, Some(cid));

        let not_found = get_chunk_id_by_hash(&conn, pid, "missing").unwrap();
        assert!(not_found.is_none());
    }

    #[test]
    fn test_chunk_unique_constraint() {
        let conn = setup_db();
        let pid = insert_profile(&conn, "p", "read-write", "https://a.com", None, "b", None, None, None, None, 10485760).unwrap();
        insert_chunk(&conn, pid, "hash-abc", "prefix/c/hash-abc", 1024).unwrap();
        let result = insert_chunk(&conn, pid, "hash-abc", "prefix/c/hash-abc", 1024);
        assert!(result.is_err());
    }

    // ── FileEntry tests ──

    #[test]
    fn test_file_entry_crud() {
        let conn = setup_db();
        let pid = insert_profile(&conn, "p", "read-write", "https://a.com", None, "b", None, None, None, None, 10485760).unwrap();

        let feid = insert_file_entry(&conn, pid, "md5abc", 2048, 1).unwrap();
        let fe = get_file_entry_by_id(&conn, feid).unwrap().unwrap();
        assert_eq!(fe.original_md5, "md5abc");
        assert_eq!(fe.total_size, 2048);
        assert_eq!(fe.chunk_count, 1);

        let fe2 = get_file_entry_by_md5(&conn, pid, "md5abc").unwrap().unwrap();
        assert_eq!(fe2.id, feid);

        let entries = list_file_entries(&conn, pid).unwrap();
        assert_eq!(entries.len(), 1);

        delete_file_entry(&conn, feid).unwrap();
        assert!(get_file_entry_by_id(&conn, feid).unwrap().is_none());
    }

    #[test]
    fn test_file_entry_unique_md5_per_profile() {
        let conn = setup_db();
        let pid = insert_profile(&conn, "p", "read-write", "https://a.com", None, "b", None, None, None, None, 10485760).unwrap();
        insert_file_entry(&conn, pid, "md5abc", 100, 1).unwrap();
        let result = insert_file_entry(&conn, pid, "md5abc", 200, 2);
        assert!(result.is_err());
    }

    // ── FileChunk tests ──

    #[test]
    fn test_file_chunk_and_restore_keys() {
        let conn = setup_db();
        let pid = insert_profile(&conn, "p", "read-write", "https://a.com", None, "b", None, None, None, None, 10485760).unwrap();
        let cid0 = insert_chunk(&conn, pid, "hash0", "prefix/c/hash0", 100).unwrap();
        let cid1 = insert_chunk(&conn, pid, "hash1", "prefix/c/hash1", 100).unwrap();
        let feid = insert_file_entry(&conn, pid, "md5abc", 200, 2).unwrap();

        insert_file_chunk(&conn, feid, 0, cid0).unwrap();
        insert_file_chunk(&conn, feid, 1, cid1).unwrap();

        let keys = get_chunk_keys_for_file(&conn, feid).unwrap();
        assert_eq!(keys.len(), 2);
        assert_eq!(keys[0], (0, "prefix/c/hash0".to_string()));
        assert_eq!(keys[1], (1, "prefix/c/hash1".to_string()));
    }

    // ── LocalFile tests ──

    #[test]
    fn test_local_file_upsert() {
        let conn = setup_db();
        let pid = insert_profile(&conn, "p", "read-write", "https://a.com", None, "b", None, None, None, None, 10485760).unwrap();
        let feid1 = insert_file_entry(&conn, pid, "md5a", 100, 1).unwrap();
        let feid2 = insert_file_entry(&conn, pid, "md5b", 200, 1).unwrap();

        // Initial insert
        upsert_local_file(&conn, feid1, "/home/user/file.txt", Some(1234.5), Some(100)).unwrap();
        let lf = get_local_file_by_path(&conn, "/home/user/file.txt").unwrap().unwrap();
        assert_eq!(lf.file_entry_id, feid1);
        assert_eq!(lf.cached_mtime, Some(1234.5));

        // Upsert with new file_entry (file changed)
        upsert_local_file(&conn, feid2, "/home/user/file.txt", Some(5678.0), Some(200)).unwrap();
        let lf2 = get_local_file_by_path(&conn, "/home/user/file.txt").unwrap().unwrap();
        assert_eq!(lf2.file_entry_id, feid2);
        assert_eq!(lf2.cached_mtime, Some(5678.0));
    }

    #[test]
    fn test_local_file_list_for_entry() {
        let conn = setup_db();
        let pid = insert_profile(&conn, "p", "read-write", "https://a.com", None, "b", None, None, None, None, 10485760).unwrap();
        let feid = insert_file_entry(&conn, pid, "md5a", 100, 1).unwrap();

        upsert_local_file(&conn, feid, "/path/file1.txt", None, None).unwrap();
        upsert_local_file(&conn, feid, "/path/file2.txt", None, None).unwrap();

        let files = list_local_files_for_entry(&conn, feid).unwrap();
        assert_eq!(files.len(), 2);
    }

    // ── ShareManifest tests ──

    #[test]
    fn test_share_manifest_crud() {
        let conn = setup_db();
        let pid = insert_profile(&conn, "p", "read-write", "https://a.com", None, "b", None, None, None, None, 10485760).unwrap();
        let feid = insert_file_entry(&conn, pid, "md5a", 100, 1).unwrap();

        let mid = insert_share_manifest(&conn, pid, "manifest-uuid", Some("my share"), 1).unwrap();
        let manifest = get_share_manifest_by_id(&conn, mid).unwrap().unwrap();
        assert_eq!(manifest.manifest_uuid, "manifest-uuid");
        assert_eq!(manifest.label, Some("my share".into()));
        assert!(manifest.is_valid);

        insert_share_manifest_entry(&conn, mid, feid, "file.txt").unwrap();
        let entries = list_share_manifest_entries(&conn, mid).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].file_entry_id, feid);
        assert_eq!(entries[0].filename, "file.txt");

        invalidate_share_manifest(&conn, mid).unwrap();
        assert!(!get_share_manifest_by_id(&conn, mid).unwrap().unwrap().is_valid);

        delete_share_manifest(&conn, mid).unwrap();
        assert!(get_share_manifest_by_id(&conn, mid).unwrap().is_none());
        assert!(list_share_manifest_entries(&conn, mid).unwrap().is_empty());
    }

    // ── Schema versioning ──

    #[test]
    fn test_init_database_sets_version() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("test.db");
        let conn = init_database(db_path.to_str().unwrap()).unwrap();
        let version: i32 = conn.pragma_query_value(None, "user_version", |row| row.get(0)).unwrap();
        assert_eq!(version, 5);
    }

    #[test]
    fn test_init_database_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("test.db");

        // First init
        {
            let conn = init_database(db_path.to_str().unwrap()).unwrap();
            insert_profile(&conn, "p1", "read-write", "https://a.com", None, "b", None, None, None, None, 10485760).unwrap();
        }

        // Second init should not lose data
        {
            let conn = init_database(db_path.to_str().unwrap()).unwrap();
            let profiles = list_profiles(&conn).unwrap();
            assert_eq!(profiles.len(), 1);
            assert_eq!(profiles[0].name, "p1");
        }
    }

    // ── Foreign key tests ──

    #[test]
    fn test_foreign_key_enforcement() {
        let conn = setup_db();
        // Insert file_entry referencing non-existent profile
        let result = insert_file_entry(&conn, 999, "md5", 100, 1);
        assert!(result.is_err());
    }

    #[test]
    fn test_count_file_entries_for_chunk() {
        let conn = setup_db();
        let pid = insert_profile(&conn, "p", "read-write", "https://a.com", None, "b", None, None, None, None, 10485760).unwrap();
        let cid = insert_chunk(&conn, pid, "hash-shared", "prefix/c/hash-shared", 100).unwrap();
        let feid1 = insert_file_entry(&conn, pid, "md5a", 100, 1).unwrap();
        let feid2 = insert_file_entry(&conn, pid, "md5b", 100, 1).unwrap();
        insert_file_chunk(&conn, feid1, 0, cid).unwrap();
        insert_file_chunk(&conn, feid2, 0, cid).unwrap();

        let count = count_file_entries_for_chunk(&conn, cid).unwrap();
        assert_eq!(count, 2);
    }
}
