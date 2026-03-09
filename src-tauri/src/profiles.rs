use rand::RngCore;
use rusqlite::Connection;

use crate::credentials;
use crate::db;
use crate::error::AppError;

#[derive(Debug, serde::Serialize)]
pub struct CreateProfileResult {
    pub profile: db::Profile,
    pub encryption_key: String,
}

pub fn generate_encryption_key() -> String {
    let mut key_bytes = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut key_bytes);
    hex::encode(key_bytes)
}

/// Validate and normalize an S3 key prefix.
/// Returns `None` for empty/whitespace-only input (meaning no prefix).
/// Strips leading/trailing whitespace and slashes.
fn validate_s3_key_prefix(prefix: &str) -> Result<Option<String>, AppError> {
    let trimmed = prefix.trim().trim_matches('/').to_string();
    if trimmed.is_empty() {
        return Ok(None);
    }
    if trimmed.len() > 200 {
        return Err(AppError::Config(
            "S3 key prefix must not exceed 200 characters".into(),
        ));
    }
    if trimmed.contains("//") {
        return Err(AppError::Config(
            "S3 key prefix must not contain consecutive slashes".into(),
        ));
    }
    if trimmed.chars().any(|c| c.is_control()) {
        return Err(AppError::Config(
            "S3 key prefix contains invalid characters".into(),
        ));
    }
    Ok(Some(trimmed))
}

#[allow(clippy::too_many_arguments)]
pub fn create_profile(
    conn: &Connection,
    name: &str,
    mode: &str,
    s3_endpoint: &str,
    s3_region: Option<&str>,
    s3_bucket: &str,
    s3_access_key: &str,
    s3_secret_key: &str,
    extra_env: Option<&str>,
    relative_path: Option<&str>,
    temp_directory: Option<&str>,
    import_encryption_key: Option<&str>,
    s3_key_prefix: Option<&str>,
) -> Result<CreateProfileResult, AppError> {
    // Validate mode
    if mode != "read-write" && mode != "read-only" {
        return Err(AppError::Config(format!(
            "Invalid profile mode '{}'. Must be 'read-write' or 'read-only'.",
            mode
        )));
    }

    let validated_prefix = match s3_key_prefix {
        Some(p) => validate_s3_key_prefix(p)?,
        None => None,
    };

    // Insert profile into DB (UNIQUE constraint on endpoint+bucket will catch duplicates)
    let profile_id = db::insert_profile(
        conn,
        name,
        mode,
        s3_endpoint,
        s3_region,
        s3_bucket,
        extra_env,
        relative_path,
        temp_directory,
        validated_prefix.as_deref(),
    )?;

    // Generate or import encryption key
    let encryption_key = match import_encryption_key {
        Some(key) => key.to_string(),
        None => generate_encryption_key(),
    };

    // Store credentials in keychain
    credentials::store_s3_access_key(name, s3_access_key)?;
    credentials::store_s3_secret_key(name, s3_secret_key)?;
    credentials::store_encryption_key(name, &encryption_key)?;

    // If this is the first profile, make it active
    let profiles = db::list_profiles(conn)?;
    if profiles.len() == 1 {
        db::set_active_profile(conn, profile_id)?;
    }

    let profile = db::get_profile_by_id(conn, profile_id)?
        .ok_or_else(|| AppError::NotFound("profile not found after creation".into()))?;

    Ok(CreateProfileResult {
        profile,
        encryption_key,
    })
}

pub fn get_active_profile(conn: &Connection) -> Result<Option<db::Profile>, AppError> {
    let profiles = db::list_profiles(conn)?;
    Ok(profiles.into_iter().find(|p| p.is_active))
}

pub fn switch_profile(conn: &Connection, profile_id: i64) -> Result<db::Profile, AppError> {
    db::get_profile_by_id(conn, profile_id)?
        .ok_or_else(|| AppError::Config(format!("Profile with id {} not found", profile_id)))?;
    db::set_active_profile(conn, profile_id)?;
    // Return refreshed profile
    db::get_profile_by_id(conn, profile_id)?
        .ok_or_else(|| AppError::NotFound("profile not found after switch".into()))
}

#[allow(clippy::too_many_arguments)]
pub fn update_profile(
    conn: &Connection,
    id: i64,
    name: Option<&str>,
    mode: Option<&str>,
    s3_endpoint: Option<&str>,
    s3_region: Option<Option<&str>>,
    s3_bucket: Option<&str>,
    s3_access_key: Option<&str>,
    s3_secret_key: Option<&str>,
    extra_env: Option<Option<&str>>,
    relative_path: Option<Option<&str>>,
    temp_directory: Option<Option<&str>>,
    s3_key_prefix: Option<Option<&str>>,
) -> Result<db::Profile, AppError> {
    let existing = db::get_profile_by_id(conn, id)?
        .ok_or_else(|| AppError::Config(format!("Profile with id {} not found", id)))?;

    if let Some(m) = mode {
        if m != "read-write" && m != "read-only" {
            return Err(AppError::Config(format!(
                "Invalid profile mode '{}'. Must be 'read-write' or 'read-only'.",
                m
            )));
        }
    }

    let new_name = name.unwrap_or(&existing.name);
    let new_mode = mode.unwrap_or(&existing.mode);
    let new_endpoint = s3_endpoint.unwrap_or(&existing.s3_endpoint);
    let new_region = match s3_region {
        Some(r) => r.map(|s| s.to_string()),
        None => existing.s3_region.clone(),
    };
    let new_bucket = s3_bucket.unwrap_or(&existing.s3_bucket);
    let new_extra_env = match extra_env {
        Some(e) => e.map(|s| s.to_string()),
        None => existing.extra_env.clone(),
    };
    let new_relative_path = match relative_path {
        Some(r) => r.map(|s| s.to_string()),
        None => existing.relative_path.clone(),
    };
    let new_temp_directory = match temp_directory {
        Some(t) => t.map(|s| s.to_string()),
        None => existing.temp_directory.clone(),
    };
    let new_s3_key_prefix = match s3_key_prefix {
        Some(Some(p)) => validate_s3_key_prefix(p)?,
        Some(None) => None,
        None => existing.s3_key_prefix.clone(),
    };

    // Delete and re-insert (simple approach for updates with unique constraints)
    conn.execute(
        "UPDATE profile SET name=?1, mode=?2, s3_endpoint=?3, s3_region=?4, s3_bucket=?5, extra_env=?6, relative_path=?7, temp_directory=?8, s3_key_prefix=?9 WHERE id=?10",
        rusqlite::params![new_name, new_mode, new_endpoint, new_region, new_bucket, new_extra_env, new_relative_path, new_temp_directory, new_s3_key_prefix, id],
    )?;

    // Update keychain credentials if changed
    if let Some(access_key) = s3_access_key {
        // If profile name changed, delete old credentials and store under new name
        if name.is_some() && new_name != existing.name {
            credentials::delete_all_credentials(&existing.name)?;
            // Re-store all credentials under new name
            let secret = credentials::get_s3_secret_key(&existing.name)
                .unwrap_or_default();
            let enc_key = credentials::get_encryption_key(&existing.name)
                .unwrap_or_default();
            credentials::store_s3_access_key(new_name, access_key)?;
            if !secret.is_empty() {
                credentials::store_s3_secret_key(new_name, &secret)?;
            }
            if !enc_key.is_empty() {
                credentials::store_encryption_key(new_name, &enc_key)?;
            }
        } else {
            credentials::store_s3_access_key(new_name, access_key)?;
        }
    }
    if let Some(secret_key) = s3_secret_key {
        credentials::store_s3_secret_key(new_name, secret_key)?;
    }

    // If only the name changed (no explicit credential updates), migrate credentials
    if let Some(nn) = name {
        if nn != existing.name && s3_access_key.is_none() {
            // Migrate all credentials from old name to new name
            if let Ok(ak) = credentials::get_s3_access_key(&existing.name) {
                credentials::store_s3_access_key(nn, &ak)?;
            }
            if let Ok(sk) = credentials::get_s3_secret_key(&existing.name) {
                credentials::store_s3_secret_key(nn, &sk)?;
            }
            if let Ok(ek) = credentials::get_encryption_key(&existing.name) {
                credentials::store_encryption_key(nn, &ek)?;
            }
            credentials::delete_all_credentials(&existing.name)?;
        }
    }

    db::get_profile_by_id(conn, id)?
        .ok_or_else(|| AppError::NotFound("profile not found after update".into()))
}

pub fn delete_profile(conn: &Connection, id: i64) -> Result<(), AppError> {
    let profile = db::get_profile_by_id(conn, id)?
        .ok_or_else(|| AppError::Config(format!("Profile with id {} not found", id)))?;

    // Delete all related data (share_manifest_entries, local_files, backup_entries, share_manifests)
    conn.execute(
        "DELETE FROM share_manifest_entry WHERE share_manifest_id IN (SELECT id FROM share_manifest WHERE profile_id = ?1)",
        rusqlite::params![id],
    )?;
    conn.execute(
        "DELETE FROM local_file WHERE backup_entry_id IN (SELECT id FROM backup_entry WHERE profile_id = ?1)",
        rusqlite::params![id],
    )?;
    conn.execute(
        "DELETE FROM share_manifest WHERE profile_id = ?1",
        rusqlite::params![id],
    )?;
    conn.execute(
        "DELETE FROM backup_entry WHERE profile_id = ?1",
        rusqlite::params![id],
    )?;
    db::delete_profile(conn, id)?;

    // Remove keychain entries
    credentials::delete_all_credentials(&profile.name)?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generate_encryption_key_length() {
        let key = generate_encryption_key();
        // 32 bytes = 64 hex characters
        assert_eq!(key.len(), 64);
    }

    #[test]
    fn test_generate_encryption_key_is_hex() {
        let key = generate_encryption_key();
        assert!(key.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn test_generate_encryption_key_unique() {
        let key1 = generate_encryption_key();
        let key2 = generate_encryption_key();
        assert_ne!(key1, key2);
    }

    fn setup_db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
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
                s3_key_prefix TEXT,
                is_active BOOLEAN NOT NULL DEFAULT 0,
                created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
                UNIQUE(s3_endpoint, s3_bucket)
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
        )
        .unwrap();
        conn
    }

    #[test]
    fn test_get_active_profile_none() {
        let conn = setup_db();
        let result = get_active_profile(&conn).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn test_get_active_profile_found() {
        let conn = setup_db();
        let id = db::insert_profile(
            &conn, "test", "read-write", "https://s3.test.com", None, "bucket", None, None, None, None,
        ).unwrap();
        db::set_active_profile(&conn, id).unwrap();
        let result = get_active_profile(&conn).unwrap();
        assert!(result.is_some());
        assert_eq!(result.unwrap().name, "test");
    }

    #[test]
    fn test_switch_profile() {
        let conn = setup_db();
        let id1 = db::insert_profile(
            &conn, "p1", "read-write", "https://a.com", None, "b1", None, None, None, None,
        ).unwrap();
        let id2 = db::insert_profile(
            &conn, "p2", "read-write", "https://b.com", None, "b2", None, None, None, None,
        ).unwrap();

        let profile = switch_profile(&conn, id1).unwrap();
        assert_eq!(profile.name, "p1");
        assert!(profile.is_active);

        let profile = switch_profile(&conn, id2).unwrap();
        assert_eq!(profile.name, "p2");
        assert!(profile.is_active);

        // p1 should no longer be active
        let p1 = db::get_profile_by_id(&conn, id1).unwrap().unwrap();
        assert!(!p1.is_active);
    }

    #[test]
    fn test_switch_profile_nonexistent() {
        let conn = setup_db();
        let result = switch_profile(&conn, 999);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("not found"));
    }

    #[test]
    fn test_create_profile_invalid_mode() {
        let conn = setup_db();
        let result = create_profile(
            &conn, "test", "invalid-mode", "https://s3.test.com", None, "bucket",
            "ak", "sk", None, None, None, None, None,
        );
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Invalid profile mode"));
    }

    // ── validate_s3_key_prefix tests ──

    #[test]
    fn test_validate_prefix_none_input() {
        // None = no prefix wanted, always fine
        assert_eq!(validate_s3_key_prefix("").unwrap(), None);
        assert_eq!(validate_s3_key_prefix("   ").unwrap(), None);
    }

    #[test]
    fn test_validate_prefix_strips_slashes_and_whitespace() {
        assert_eq!(
            validate_s3_key_prefix("  /team-alpha/  ").unwrap(),
            Some("team-alpha".to_string())
        );
        assert_eq!(
            validate_s3_key_prefix("/foo/bar/").unwrap(),
            Some("foo/bar".to_string())
        );
    }

    #[test]
    fn test_validate_prefix_simple_value() {
        assert_eq!(
            validate_s3_key_prefix("team-alpha").unwrap(),
            Some("team-alpha".to_string())
        );
    }

    #[test]
    fn test_validate_prefix_nested_path() {
        assert_eq!(
            validate_s3_key_prefix("org/team/backups").unwrap(),
            Some("org/team/backups".to_string())
        );
    }

    #[test]
    fn test_validate_prefix_rejects_double_slash() {
        let result = validate_s3_key_prefix("foo//bar");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("consecutive slashes"));
    }

    #[test]
    fn test_validate_prefix_rejects_over_200_chars() {
        let long = "a".repeat(201);
        let result = validate_s3_key_prefix(&long);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("200"));
    }

    #[test]
    fn test_validate_prefix_accepts_exactly_200_chars() {
        let edge = "a".repeat(200);
        assert!(validate_s3_key_prefix(&edge).unwrap().is_some());
    }

    #[test]
    fn test_validate_prefix_rejects_control_chars() {
        let result = validate_s3_key_prefix("foo\x01bar");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("invalid characters"));
    }
}
