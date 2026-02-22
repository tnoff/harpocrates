use thiserror::Error;

#[derive(Debug, Error)]
pub enum AppError {
    #[error("Database error: {0}")]
    Database(#[from] rusqlite::Error),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Config error: {0}")]
    Config(String),

    #[error("Serialization error: {0}")]
    Serialization(#[from] serde_json::Error),

    #[error("Crypto error: {0}")]
    Crypto(String),

    #[error("S3 error: {0}")]
    S3(String),

    #[error("Credential error: {0}")]
    Credential(String),

    /// Returned when a mutex is poisoned (a thread panicked while holding the lock).
    #[error("Lock error: {0}")]
    Lock(String),

    /// Returned when a requested resource does not exist.
    #[error("Not found: {0}")]
    NotFound(String),

    /// Returned when data is structurally invalid or corrupt.
    #[error("Invalid data: {0}")]
    InvalidData(String),

}

impl serde::Serialize for AppError {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_error_display() {
        let err = AppError::Config("bad config".into());
        assert_eq!(err.to_string(), "Config error: bad config");

        let err = AppError::Crypto("decrypt failed".into());
        assert_eq!(err.to_string(), "Crypto error: decrypt failed");

        let err = AppError::S3("connection refused".into());
        assert_eq!(err.to_string(), "S3 error: connection refused");

        let err = AppError::Lock("mutex poisoned".into());
        assert_eq!(err.to_string(), "Lock error: mutex poisoned");

        let err = AppError::NotFound("manifest 42".into());
        assert_eq!(err.to_string(), "Not found: manifest 42");

        let err = AppError::InvalidData("missing 'files' key".into());
        assert_eq!(err.to_string(), "Invalid data: missing 'files' key");
    }

    #[test]
    fn test_error_serialize() {
        let err = AppError::Config("test".into());
        let json = serde_json::to_string(&err).unwrap();
        assert_eq!(json, "\"Config error: test\"");
    }

    #[test]
    fn test_error_from_io() {
        let io_err = std::io::Error::new(std::io::ErrorKind::NotFound, "file not found");
        let app_err: AppError = io_err.into();
        assert!(app_err.to_string().contains("file not found"));
    }
}
