use thiserror::Error;

#[derive(Debug, Error)]
pub enum AppError {
    #[error("SSH connection failed: {0}")]
    SshConnectionError(String),

    #[error("IO error: {0}")]
    IOError(#[from] std::io::Error),

    #[error("Authentication error: {0}")]
    AuthenticationError(String),
}

/// Application result type alias
pub type Result<T> = std::result::Result<T, AppError>;
