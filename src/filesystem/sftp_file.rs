//! SFTP file handle using russh_sftp's built-in File type.
//!
//! This module provides a thin wrapper around `russh_sftp::client::fs::File`
//! to implement our `HostFile` trait, enabling SSH-to-SSH file transfers.

use crate::async_ssh_client::{HostFile, HostFileMetadata};
use crate::error::{AppError, Result};
use russh_sftp::client::SftpSession;
use russh_sftp::protocol::OpenFlags;

/// Re-export the built-in File type which already implements AsyncRead/AsyncWrite
pub type SftpFile = russh_sftp::client::fs::File;

impl HostFile for SftpFile {
    async fn file_metadata(&self) -> Result<HostFileMetadata> {
        let metadata = self
            .metadata()
            .await
            .map_err(|e| AppError::SftpError(format!("Failed to get file metadata: {e}")))?;
        Ok(HostFileMetadata {
            size: metadata.len(),
            permissions: metadata.permissions,
        })
    }
}

/// Open an SFTP file for reading
pub async fn open_for_read(session: &SftpSession, path: &str) -> Result<SftpFile> {
    session
        .open_with_flags(path, OpenFlags::READ)
        .await
        .map_err(|e| AppError::SftpError(format!("Failed to open file for reading: {e}")))
}

/// Open an SFTP file for writing (creates new file or truncates existing)
pub async fn open_for_write(session: &SftpSession, path: &str) -> Result<SftpFile> {
    session
        .open_with_flags(
            path,
            OpenFlags::WRITE | OpenFlags::CREATE | OpenFlags::TRUNCATE,
        )
        .await
        .map_err(|e| AppError::SftpError(format!("Failed to open file for writing: {e}")))
}
