//! SFTP file handle using russh_sftp's built-in File type.
//!
//! This module provides a thin wrapper around `russh_sftp::client::fs::File`
//! to implement our `HostFile` trait, enabling SSH-to-SSH file transfers.

use russh_sftp::client::SftpSession;
use russh_sftp::protocol::OpenFlags;
use std::sync::Arc;

use crate::async_ssh_client::HostFile;
use crate::error::{AppError, Result};

/// Re-export the built-in File type which already implements AsyncRead/AsyncWrite
pub type SftpFile = russh_sftp::client::fs::File;

impl HostFile for SftpFile {
    async fn file_size(&self) -> Result<u64> {
        let metadata = self
            .metadata()
            .await
            .map_err(|e| AppError::SftpError(format!("Failed to get file metadata: {}", e)))?;
        Ok(metadata.len())
    }
}

/// Open an SFTP file for reading
pub async fn open_for_read(session: Arc<SftpSession>, path: &str) -> Result<SftpFile> {
    session
        .open_with_flags(path, OpenFlags::READ)
        .await
        .map_err(|e| AppError::SftpError(format!("Failed to open file for reading: {}", e)))
}

/// Open an SFTP file for writing (creates new file or truncates existing)
pub async fn open_for_write(session: Arc<SftpSession>, path: &str) -> Result<SftpFile> {
    session
        .open_with_flags(
            path,
            OpenFlags::WRITE | OpenFlags::CREATE | OpenFlags::TRUNCATE,
        )
        .await
        .map_err(|e| AppError::SftpError(format!("Failed to open file for writing: {}", e)))
}
