use crate::error::AppError;

const SERVICE_PREFIX: &str = "harpocrates";

pub fn register_default_store() -> Result<(), AppError> {
    #[cfg(target_os = "macos")]
    let store = apple_native_keyring_store::keychain::Store::new()
        .map_err(|e| AppError::Credential(format!("Failed to init macOS keychain store: {}", e)))?;

    #[cfg(target_os = "windows")]
    let store = windows_native_keyring_store::Store::new()
        .map_err(|e| AppError::Credential(format!("Failed to init Windows credential store: {}", e)))?;

    #[cfg(all(unix, not(target_os = "macos")))]
    let store = dbus_secret_service_keyring_store::Store::new()
        .map_err(|e| AppError::Credential(format!("Failed to init Secret Service store: {}", e)))?;

    keyring_core::set_default_store(store);
    Ok(())
}

fn service_name(profile_name: &str, key_type: &str) -> String {
    format!("{}:{}:{}", SERVICE_PREFIX, profile_name, key_type)
}

pub fn store_credential(profile_name: &str, key_type: &str, value: &str) -> Result<(), AppError> {
    let service = service_name(profile_name, key_type);
    let entry = keyring_core::Entry::new(&service, profile_name)
        .map_err(|e| AppError::Credential(format!("Failed to create keyring entry: {}", e)))?;
    entry
        .set_password(value)
        .map_err(|e| AppError::Credential(format!("Failed to store credential '{}': {}", service, e)))?;
    Ok(())
}

pub fn get_credential(profile_name: &str, key_type: &str) -> Result<String, AppError> {
    let service = service_name(profile_name, key_type);
    let entry = keyring_core::Entry::new(&service, profile_name)
        .map_err(|e| AppError::Credential(format!("Failed to create keyring entry: {}", e)))?;
    entry
        .get_password()
        .map_err(|e| AppError::Credential(format!("Failed to retrieve credential '{}': {}", service, e)))
}

pub fn delete_credential(profile_name: &str, key_type: &str) -> Result<(), AppError> {
    let service = service_name(profile_name, key_type);
    let entry = keyring_core::Entry::new(&service, profile_name)
        .map_err(|e| AppError::Credential(format!("Failed to create keyring entry: {}", e)))?;
    match entry.delete_credential() {
        Ok(()) => Ok(()),
        Err(keyring_core::Error::NoEntry) => Ok(()), // Already deleted, not an error
        Err(e) => Err(AppError::Credential(format!(
            "Failed to delete credential '{}': {}",
            service, e
        ))),
    }
}

pub fn store_s3_access_key(profile_name: &str, value: &str) -> Result<(), AppError> {
    store_credential(profile_name, "s3-access-key", value)
}

pub fn store_s3_secret_key(profile_name: &str, value: &str) -> Result<(), AppError> {
    store_credential(profile_name, "s3-secret-key", value)
}

pub fn store_encryption_key(profile_name: &str, value: &str) -> Result<(), AppError> {
    store_credential(profile_name, "encryption-key", value)
}

pub fn get_s3_access_key(profile_name: &str) -> Result<String, AppError> {
    get_credential(profile_name, "s3-access-key")
}

pub fn get_s3_secret_key(profile_name: &str) -> Result<String, AppError> {
    get_credential(profile_name, "s3-secret-key")
}

pub fn get_encryption_key(profile_name: &str) -> Result<String, AppError> {
    get_credential(profile_name, "encryption-key")
}

pub fn delete_all_credentials(profile_name: &str) -> Result<(), AppError> {
    delete_credential(profile_name, "s3-access-key")?;
    delete_credential(profile_name, "s3-secret-key")?;
    delete_credential(profile_name, "encryption-key")?;
    Ok(())
}

#[cfg(test)]
pub fn init_test_store() {
    use std::sync::Once;
    static INIT: Once = Once::new();
    INIT.call_once(|| {
        let store = keyring_core::mock::Store::new()
            .expect("failed to build mock keyring store");
        keyring_core::set_default_store(store);
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_service_name_format() {
        let name = service_name("my-profile", "s3-access-key");
        assert_eq!(name, "harpocrates:my-profile:s3-access-key");
    }

    #[test]
    fn test_service_name_encryption_key() {
        let name = service_name("prod", "encryption-key");
        assert_eq!(name, "harpocrates:prod:encryption-key");
    }

    #[test]
    fn test_service_name_with_special_chars() {
        let name = service_name("profile-with-dashes", "s3-secret-key");
        assert_eq!(name, "harpocrates:profile-with-dashes:s3-secret-key");
    }
}
