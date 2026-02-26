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

const SCHEMA_VERSION: i32 = 2;

pub fn init_database(path: &str) -> Result<Connection, AppError> {
    let conn = Connection::open(path)?;

    conn.execute_batch("PRAGMA foreign_keys = ON;")?;

    let version: i32 = conn.pragma_query_value(None, "user_version", |row| row.get(0))?;

    if version < 1 {
        create_schema(&conn)?;
    }
    if version < 2 {
        migrate_v1_to_v2(&conn)?;
    }
    if version < SCHEMA_VERSION {
        conn.pragma_update(None, "user_version", SCHEMA_VERSION)?;
    }

    Ok(conn)
}

fn create_schema(conn: &Connection) -> Result<(), AppError> {
    conn.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS profile (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            name TEXT UNIQUE NOT NULL,
            mode TEXT NOT NULL DEFAULT 'read-write',
            s3_endpoint TEXT NOT NULL,
            s3_region TEXT,
            s3_bucket TEXT NOT NULL,
            extra_env TEXT,
            relative_path TEXT,
            temp_directory TEXT,
            is_active BOOLEAN NOT NULL DEFAULT 0,
            created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
        );

        CREATE TABLE IF NOT EXISTS backup_entry (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            profile_id INTEGER NOT NULL REFERENCES profile(id),
            object_uuid TEXT NOT NULL,
            original_md5 TEXT NOT NULL,
            encrypted_md5 TEXT NOT NULL,
            file_size INTEGER NOT NULL,
            created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
            UNIQUE(profile_id, object_uuid)
        );

        CREATE TABLE IF NOT EXISTS share_manifest (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            profile_id INTEGER NOT NULL REFERENCES profile(id),
            manifest_uuid TEXT NOT NULL,
            label TEXT,
            file_count INTEGER NOT NULL,
            is_valid BOOLEAN NOT NULL DEFAULT 1,
            created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
            UNIQUE(profile_id, manifest_uuid)
        );

        CREATE TABLE IF NOT EXISTS share_manifest_entry (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            share_manifest_id INTEGER NOT NULL REFERENCES share_manifest(id),
            backup_entry_id INTEGER NOT NULL REFERENCES backup_entry(id),
            filename TEXT NOT NULL
        );

        CREATE TABLE IF NOT EXISTS local_file (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            backup_entry_id INTEGER NOT NULL REFERENCES backup_entry(id),
            local_path TEXT NOT NULL,
            cached_mtime REAL,
            cached_size INTEGER,
            updated_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
            UNIQUE(backup_entry_id, local_path)
        );
        ",
    )?;
    Ok(())
}

/// Remove the UNIQUE(s3_endpoint, s3_bucket) constraint from the profile table.
/// SQLite doesn't support DROP CONSTRAINT, so we recreate the table without it.
fn migrate_v1_to_v2(conn: &Connection) -> Result<(), AppError> {
    conn.execute_batch(
        "
        PRAGMA foreign_keys = OFF;
        BEGIN;

        CREATE TABLE IF NOT EXISTS profile_v2 (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            name TEXT UNIQUE NOT NULL,
            mode TEXT NOT NULL DEFAULT 'read-write',
            s3_endpoint TEXT NOT NULL,
            s3_region TEXT,
            s3_bucket TEXT NOT NULL,
            extra_env TEXT,
            relative_path TEXT,
            temp_directory TEXT,
            is_active BOOLEAN NOT NULL DEFAULT 0,
            created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
        );

        INSERT OR IGNORE INTO profile_v2
            SELECT id, name, mode, s3_endpoint, s3_region, s3_bucket,
                   extra_env, relative_path, temp_directory, is_active, created_at
            FROM profile;

        DROP TABLE profile;
        ALTER TABLE profile_v2 RENAME TO profile;

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
) -> Result<i64, AppError> {
    conn.execute(
        "INSERT INTO profile (name, mode, s3_endpoint, s3_region, s3_bucket, extra_env, relative_path, temp_directory)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![name, mode, s3_endpoint, s3_region, s3_bucket, extra_env, relative_path, temp_directory],
    )?;
    Ok(conn.last_insert_rowid())
}

pub fn get_profile_by_id(conn: &Connection, id: i64) -> Result<Option<Profile>, AppError> {
    let mut stmt = conn.prepare(
        "SELECT id, name, mode, s3_endpoint, s3_region, s3_bucket, extra_env, relative_path, temp_directory, is_active, created_at
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
            })
        })
        .optional()?;
    Ok(profile)
}

pub fn list_profiles(conn: &Connection) -> Result<Vec<Profile>, AppError> {
    let mut stmt = conn.prepare(
        "SELECT id, name, mode, s3_endpoint, s3_region, s3_bucket, extra_env, relative_path, temp_directory, is_active, created_at
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

// ── BackupEntry CRUD ──

#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub struct BackupEntry {
    pub id: i64,
    pub profile_id: i64,
    pub object_uuid: String,
    pub original_md5: String,
    pub encrypted_md5: String,
    pub file_size: i64,
    pub created_at: String,
}

pub fn insert_backup_entry(
    conn: &Connection,
    profile_id: i64,
    object_uuid: &str,
    original_md5: &str,
    encrypted_md5: &str,
    file_size: i64,
) -> Result<i64, AppError> {
    conn.execute(
        "INSERT INTO backup_entry (profile_id, object_uuid, original_md5, encrypted_md5, file_size)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        params![profile_id, object_uuid, original_md5, encrypted_md5, file_size],
    )?;
    Ok(conn.last_insert_rowid())
}

pub fn get_backup_entry_by_id(conn: &Connection, id: i64) -> Result<Option<BackupEntry>, AppError> {
    let mut stmt = conn.prepare(
        "SELECT id, profile_id, object_uuid, original_md5, encrypted_md5, file_size, created_at
         FROM backup_entry WHERE id = ?1",
    )?;
    let entry = stmt
        .query_row(params![id], |row| {
            Ok(BackupEntry {
                id: row.get(0)?,
                profile_id: row.get(1)?,
                object_uuid: row.get(2)?,
                original_md5: row.get(3)?,
                encrypted_md5: row.get(4)?,
                file_size: row.get(5)?,
                created_at: row.get(6)?,
            })
        })
        .optional()?;
    Ok(entry)
}

pub fn list_backup_entries(conn: &Connection, profile_id: i64) -> Result<Vec<BackupEntry>, AppError> {
    let mut stmt = conn.prepare(
        "SELECT id, profile_id, object_uuid, original_md5, encrypted_md5, file_size, created_at
         FROM backup_entry WHERE profile_id = ?1 ORDER BY id",
    )?;
    let entries = stmt
        .query_map(params![profile_id], |row| {
            Ok(BackupEntry {
                id: row.get(0)?,
                profile_id: row.get(1)?,
                object_uuid: row.get(2)?,
                original_md5: row.get(3)?,
                encrypted_md5: row.get(4)?,
                file_size: row.get(5)?,
                created_at: row.get(6)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(entries)
}

pub fn delete_backup_entry(conn: &Connection, id: i64) -> Result<(), AppError> {
    conn.execute("DELETE FROM backup_entry WHERE id = ?1", params![id])?;
    Ok(())
}

pub fn update_backup_entry_uuid(conn: &Connection, id: i64, new_uuid: &str) -> Result<(), AppError> {
    conn.execute(
        "UPDATE backup_entry SET object_uuid = ?1 WHERE id = ?2",
        params![new_uuid, id],
    )?;
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
    pub backup_entry_id: i64,
    pub filename: String,
}

pub fn insert_share_manifest_entry(
    conn: &Connection,
    share_manifest_id: i64,
    backup_entry_id: i64,
    filename: &str,
) -> Result<i64, AppError> {
    conn.execute(
        "INSERT INTO share_manifest_entry (share_manifest_id, backup_entry_id, filename)
         VALUES (?1, ?2, ?3)",
        params![share_manifest_id, backup_entry_id, filename],
    )?;
    Ok(conn.last_insert_rowid())
}

pub fn list_share_manifest_entries(
    conn: &Connection,
    share_manifest_id: i64,
) -> Result<Vec<ShareManifestEntry>, AppError> {
    let mut stmt = conn.prepare(
        "SELECT id, share_manifest_id, backup_entry_id, filename
         FROM share_manifest_entry WHERE share_manifest_id = ?1 ORDER BY id",
    )?;
    let entries = stmt
        .query_map(params![share_manifest_id], |row| {
            Ok(ShareManifestEntry {
                id: row.get(0)?,
                share_manifest_id: row.get(1)?,
                backup_entry_id: row.get(2)?,
                filename: row.get(3)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(entries)
}

// ── LocalFile CRUD ──

#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub struct LocalFile {
    pub id: i64,
    pub backup_entry_id: i64,
    pub local_path: String,
    pub cached_mtime: Option<f64>,
    pub cached_size: Option<i64>,
    pub updated_at: String,
}

pub fn insert_local_file(
    conn: &Connection,
    backup_entry_id: i64,
    local_path: &str,
    cached_mtime: Option<f64>,
    cached_size: Option<i64>,
) -> Result<i64, AppError> {
    conn.execute(
        "INSERT INTO local_file (backup_entry_id, local_path, cached_mtime, cached_size)
         VALUES (?1, ?2, ?3, ?4)",
        params![backup_entry_id, local_path, cached_mtime, cached_size],
    )?;
    Ok(conn.last_insert_rowid())
}

#[cfg(test)]
pub fn get_local_file_by_path(
    conn: &Connection,
    backup_entry_id: i64,
    local_path: &str,
) -> Result<Option<LocalFile>, AppError> {
    let mut stmt = conn.prepare(
        "SELECT id, backup_entry_id, local_path, cached_mtime, cached_size, updated_at
         FROM local_file WHERE backup_entry_id = ?1 AND local_path = ?2",
    )?;
    let file = stmt
        .query_row(params![backup_entry_id, local_path], |row| {
            Ok(LocalFile {
                id: row.get(0)?,
                backup_entry_id: row.get(1)?,
                local_path: row.get(2)?,
                cached_mtime: row.get(3)?,
                cached_size: row.get(4)?,
                updated_at: row.get(5)?,
            })
        })
        .optional()?;
    Ok(file)
}

pub fn list_local_files(conn: &Connection, backup_entry_id: i64) -> Result<Vec<LocalFile>, AppError> {
    let mut stmt = conn.prepare(
        "SELECT id, backup_entry_id, local_path, cached_mtime, cached_size, updated_at
         FROM local_file WHERE backup_entry_id = ?1 ORDER BY id",
    )?;
    let files = stmt
        .query_map(params![backup_entry_id], |row| {
            Ok(LocalFile {
                id: row.get(0)?,
                backup_entry_id: row.get(1)?,
                local_path: row.get(2)?,
                cached_mtime: row.get(3)?,
                cached_size: row.get(4)?,
                updated_at: row.get(5)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(files)
}

#[cfg(test)]
pub fn update_local_file_cache(
    conn: &Connection,
    id: i64,
    cached_mtime: Option<f64>,
    cached_size: Option<i64>,
) -> Result<(), AppError> {
    conn.execute(
        "UPDATE local_file SET cached_mtime = ?1, cached_size = ?2, updated_at = CURRENT_TIMESTAMP WHERE id = ?3",
        params![cached_mtime, cached_size, id],
    )?;
    Ok(())
}

pub fn delete_local_file(conn: &Connection, id: i64) -> Result<(), AppError> {
    conn.execute("DELETE FROM local_file WHERE id = ?1", params![id])?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup_db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
        super::create_schema(&conn).unwrap();
        conn
    }

    // ── Profile tests ──

    #[test]
    fn test_insert_and_get_profile() {
        let conn = setup_db();
        let id = insert_profile(&conn, "test", "read-write", "https://s3.example.com", Some("us-east-1"), "my-bucket", None, None, None).unwrap();
        let profile = get_profile_by_id(&conn, id).unwrap().unwrap();
        assert_eq!(profile.name, "test");
        assert_eq!(profile.mode, "read-write");
        assert_eq!(profile.s3_endpoint, "https://s3.example.com");
        assert_eq!(profile.s3_region, Some("us-east-1".into()));
        assert_eq!(profile.s3_bucket, "my-bucket");
        assert!(!profile.is_active);
    }

    #[test]
    fn test_list_profiles() {
        let conn = setup_db();
        insert_profile(&conn, "p1", "read-write", "https://s3.a.com", None, "b1", None, None, None).unwrap();
        insert_profile(&conn, "p2", "read-only", "https://s3.b.com", None, "b2", None, None, None).unwrap();
        let profiles = list_profiles(&conn).unwrap();
        assert_eq!(profiles.len(), 2);
        assert_eq!(profiles[0].name, "p1");
        assert_eq!(profiles[1].name, "p2");
    }

    #[test]
    fn test_set_active_profile() {
        let conn = setup_db();
        let id1 = insert_profile(&conn, "p1", "read-write", "https://a.com", None, "b1", None, None, None).unwrap();
        let id2 = insert_profile(&conn, "p2", "read-write", "https://b.com", None, "b2", None, None, None).unwrap();
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
        let id = insert_profile(&conn, "p1", "read-write", "https://a.com", None, "b1", None, None, None).unwrap();
        delete_profile(&conn, id).unwrap();
        assert!(get_profile_by_id(&conn, id).unwrap().is_none());
    }

    #[test]
    fn test_unique_profile_name() {
        let conn = setup_db();
        insert_profile(&conn, "same-name", "read-write", "https://a.com", None, "b1", None, None, None).unwrap();
        let result = insert_profile(&conn, "same-name", "read-write", "https://b.com", None, "b2", None, None, None);
        assert!(result.is_err());
    }

    // ── BackupEntry tests ──

    #[test]
    fn test_backup_entry_crud() {
        let conn = setup_db();
        let pid = insert_profile(&conn, "p", "read-write", "https://a.com", None, "b", None, None, None).unwrap();

        let eid = insert_backup_entry(&conn, pid, "uuid-1", "md5orig", "md5enc", 1024).unwrap();
        let entry = get_backup_entry_by_id(&conn, eid).unwrap().unwrap();
        assert_eq!(entry.object_uuid, "uuid-1");
        assert_eq!(entry.file_size, 1024);

        let entries = list_backup_entries(&conn, pid).unwrap();
        assert_eq!(entries.len(), 1);

        update_backup_entry_uuid(&conn, eid, "uuid-2").unwrap();
        let entry = get_backup_entry_by_id(&conn, eid).unwrap().unwrap();
        assert_eq!(entry.object_uuid, "uuid-2");

        delete_backup_entry(&conn, eid).unwrap();
        assert!(get_backup_entry_by_id(&conn, eid).unwrap().is_none());
    }

    #[test]
    fn test_backup_entry_unique_uuid_per_profile() {
        let conn = setup_db();
        let pid = insert_profile(&conn, "p", "read-write", "https://a.com", None, "b", None, None, None).unwrap();
        insert_backup_entry(&conn, pid, "uuid-1", "md5a", "md5b", 100).unwrap();
        let result = insert_backup_entry(&conn, pid, "uuid-1", "md5c", "md5d", 200);
        assert!(result.is_err());
    }

    // ── ShareManifest tests ──

    #[test]
    fn test_share_manifest_crud() {
        let conn = setup_db();
        let pid = insert_profile(&conn, "p", "read-write", "https://a.com", None, "b", None, None, None).unwrap();

        let mid = insert_share_manifest(&conn, pid, "manifest-uuid", Some("my share"), 3).unwrap();
        let manifest = get_share_manifest_by_id(&conn, mid).unwrap().unwrap();
        assert_eq!(manifest.manifest_uuid, "manifest-uuid");
        assert_eq!(manifest.label, Some("my share".into()));
        assert_eq!(manifest.file_count, 3);
        assert!(manifest.is_valid);

        invalidate_share_manifest(&conn, mid).unwrap();
        let manifest = get_share_manifest_by_id(&conn, mid).unwrap().unwrap();
        assert!(!manifest.is_valid);

        let manifests = list_share_manifests(&conn, pid).unwrap();
        assert_eq!(manifests.len(), 1);

        delete_share_manifest(&conn, mid).unwrap();
        assert!(get_share_manifest_by_id(&conn, mid).unwrap().is_none());
    }

    // ── ShareManifestEntry tests ──

    #[test]
    fn test_share_manifest_entries() {
        let conn = setup_db();
        let pid = insert_profile(&conn, "p", "read-write", "https://a.com", None, "b", None, None, None).unwrap();
        let eid1 = insert_backup_entry(&conn, pid, "uuid-1", "md5a", "md5b", 100).unwrap();
        let eid2 = insert_backup_entry(&conn, pid, "uuid-2", "md5c", "md5d", 200).unwrap();
        let mid = insert_share_manifest(&conn, pid, "manifest-1", None, 2).unwrap();

        insert_share_manifest_entry(&conn, mid, eid1, "file1.txt").unwrap();
        insert_share_manifest_entry(&conn, mid, eid2, "file2.txt").unwrap();

        let entries = list_share_manifest_entries(&conn, mid).unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].filename, "file1.txt");
        assert_eq!(entries[1].filename, "file2.txt");
    }

    // ── LocalFile tests ──

    #[test]
    fn test_local_file_crud() {
        let conn = setup_db();
        let pid = insert_profile(&conn, "p", "read-write", "https://a.com", None, "b", None, None, None).unwrap();
        let eid = insert_backup_entry(&conn, pid, "uuid-1", "md5a", "md5b", 100).unwrap();

        let lfid = insert_local_file(&conn, eid, "/home/user/file.txt", Some(1234.5), Some(100)).unwrap();
        let lf = get_local_file_by_path(&conn, eid, "/home/user/file.txt").unwrap().unwrap();
        assert_eq!(lf.local_path, "/home/user/file.txt");
        assert_eq!(lf.cached_mtime, Some(1234.5));
        assert_eq!(lf.cached_size, Some(100));

        update_local_file_cache(&conn, lfid, Some(5678.0), Some(200)).unwrap();
        let lf = get_local_file_by_path(&conn, eid, "/home/user/file.txt").unwrap().unwrap();
        assert_eq!(lf.cached_mtime, Some(5678.0));
        assert_eq!(lf.cached_size, Some(200));

        let files = list_local_files(&conn, eid).unwrap();
        assert_eq!(files.len(), 1);

        delete_local_file(&conn, lfid).unwrap();
        assert!(get_local_file_by_path(&conn, eid, "/home/user/file.txt").unwrap().is_none());
    }

    #[test]
    fn test_local_file_unique_constraint() {
        let conn = setup_db();
        let pid = insert_profile(&conn, "p", "read-write", "https://a.com", None, "b", None, None, None).unwrap();
        let eid = insert_backup_entry(&conn, pid, "uuid-1", "md5a", "md5b", 100).unwrap();
        insert_local_file(&conn, eid, "/path/file.txt", None, None).unwrap();
        let result = insert_local_file(&conn, eid, "/path/file.txt", None, None);
        assert!(result.is_err());
    }

    // ── Schema versioning ──

    #[test]
    fn test_init_database_sets_version() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("test.db");
        let conn = init_database(db_path.to_str().unwrap()).unwrap();
        let version: i32 = conn.pragma_query_value(None, "user_version", |row| row.get(0)).unwrap();
        assert_eq!(version, 2);
    }

    #[test]
    fn test_init_database_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("test.db");

        // First init
        {
            let conn = init_database(db_path.to_str().unwrap()).unwrap();
            insert_profile(&conn, "p1", "read-write", "https://a.com", None, "b", None, None, None).unwrap();
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
        // Insert backup_entry referencing non-existent profile
        let result = insert_backup_entry(&conn, 999, "uuid", "md5a", "md5b", 100);
        assert!(result.is_err());
    }

    // ── Cascade-like behavior through delete_share_manifest ──

    #[test]
    fn test_delete_share_manifest_cascades_entries() {
        let conn = setup_db();
        let pid = insert_profile(&conn, "p", "read-write", "https://a.com", None, "b", None, None, None).unwrap();
        let eid = insert_backup_entry(&conn, pid, "uuid-1", "md5a", "md5b", 100).unwrap();
        let mid = insert_share_manifest(&conn, pid, "manifest-1", None, 1).unwrap();
        insert_share_manifest_entry(&conn, mid, eid, "file.txt").unwrap();

        delete_share_manifest(&conn, mid).unwrap();
        let entries = list_share_manifest_entries(&conn, mid).unwrap();
        assert!(entries.is_empty());
    }
}
