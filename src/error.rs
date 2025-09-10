use thiserror::Error;

#[derive(Debug, Error)]
pub enum AppError {
    #[error("SSH connection failed: {0}")]
    SshConnectionError(String),

    #[error("IO error: {0}")]
    IOError(#[from] std::io::Error),

    #[error("Authentication error: {0}")]
    AuthenticationError(String),

    #[error("Validation error: {0}")]
    ValidationError(String),

    #[error("Encryption error: {0}")]
    EncryptionError(String),

    #[error("Config error: {0}")]
    ConfigError(String),

    #[error("SSH write error: {0}")]
    SshWriteError(String),

    #[error("Russh error: {0}")]
    RusshError(#[from] russh::Error),

    #[error("Russh Sftp error: {0}")]
    RusshSftpError(#[from] russh_sftp::client::error::Error),
}

/// Application result type alias
pub type Result<T> = std::result::Result<T, AppError>;
