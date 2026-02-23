use std::fs;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::error::AppError;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    pub database_path: String,
}

impl Default for AppConfig {
    fn default() -> Self {
        let dir = app_dir();
        Self {
            database_path: dir.join("harpocrates.db").to_string_lossy().into_owned(),
        }
    }
}

pub fn app_dir() -> PathBuf {
    dirs::home_dir()
        .expect("Could not determine home directory")
        .join(".harpocrates")
}

pub fn config_path() -> PathBuf {
    app_dir().join("config.json")
}

pub fn load_or_create_config() -> Result<AppConfig, AppError> {
    let dir = app_dir();
    if !dir.exists() {
        fs::create_dir_all(&dir)?;
    }

    let path = config_path();
    if path.exists() {
        let contents = fs::read_to_string(&path)?;
        let config: AppConfig = serde_json::from_str(&contents)?;
        Ok(config)
    } else {
        let config = AppConfig::default();
        let json = serde_json::to_string_pretty(&config)?;
        fs::write(&path, json)?;
        Ok(config)
    }
}

/// Save config to the default config path.
pub fn save_config(config: &AppConfig) -> Result<(), AppError> {
    let json = serde_json::to_string_pretty(config)?;
    fs::write(config_path(), json)?;
    Ok(())
}

/// Load config from a specific path (used for testing)
pub fn load_config_from(path: &std::path::Path) -> Result<AppConfig, AppError> {
    let contents = fs::read_to_string(path)?;
    let config: AppConfig = serde_json::from_str(&contents)?;
    Ok(config)
}

/// Save config to a specific path (used for testing)
pub fn save_config_to(config: &AppConfig, path: &std::path::Path) -> Result<(), AppError> {
    let json = serde_json::to_string_pretty(config)?;
    fs::write(path, json)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = AppConfig::default();
        assert!(config.database_path.contains("harpocrates.db"));
        assert!(config.database_path.contains(".harpocrates"));
    }

    #[test]
    fn test_config_serialize_deserialize() {
        let config = AppConfig {
            database_path: "/tmp/test.db".into(),
        };
        let json = serde_json::to_string(&config).unwrap();
        let parsed: AppConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.database_path, "/tmp/test.db");
    }

    #[test]
    fn test_save_and_load_config() {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("config.json");

        let config = AppConfig {
            database_path: "/custom/path/harpocrates.db".into(),
        };
        save_config_to(&config, &config_path).unwrap();

        let loaded = load_config_from(&config_path).unwrap();
        assert_eq!(loaded.database_path, "/custom/path/harpocrates.db");
    }

    #[test]
    fn test_app_dir() {
        let dir = app_dir();
        assert!(dir.to_string_lossy().contains(".harpocrates"));
    }
}
