use thiserror::Error;

#[allow(clippy::enum_variant_names)]
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

    #[error("Sftp error: {0}")]
    SftpError(String),

    #[error("SSH public key validation error: {0}")]
    SshPublicKeyValidationError(String),

    #[error("Port forwarding error: {0}")]
    PortForwardingError(String),
}

/// Application result type alias
pub type Result<T> = std::result::Result<T, AppError>;
