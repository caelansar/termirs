//! SFTP filesystem implementation.

use ratatui_explorer::{FileEntry, FileSystem};
use russh_sftp::client::SftpSession;
use std::io::{Error, ErrorKind, Result};
use std::sync::Arc;

/// A filesystem implementation for SFTP operations using `russh_sftp`.
#[derive(Clone)]
pub struct SftpFileSystem {
    session: Arc<SftpSession>,
}

impl SftpFileSystem {
    /// Create a new `SftpFileSystem` with the given SFTP session.
    pub fn new(session: Arc<SftpSession>) -> Self {
        Self { session }
    }
}

impl FileSystem for SftpFileSystem {
    async fn read_dir(&self, path: &str) -> Result<Vec<FileEntry>> {
        let mut entries = Vec::new();

        // Normalize the path (remove trailing slashes, handle empty path)
        let normalized_path = if path.is_empty() || path == "." {
            ".".to_string()
        } else {
            path.trim_end_matches('/').to_string()
        };

        // Read directory from SFTP
        let mut read_dir = self.session.read_dir(&normalized_path).await.map_err(|e| {
            Error::new(
                ErrorKind::Other,
                format!("SFTP read_dir failed for '{}': {}", normalized_path, e),
            )
        })?;

        while let Some(entry_result) = read_dir.next() {
            let entry = entry_result;

            let filename = entry.file_name().to_string();
            let is_dir = entry.file_type().is_dir();
            let is_hidden = filename.starts_with('.');

            // Get file size from metadata
            let size = if !is_dir { entry.metadata().size } else { None };

            // Construct the full path carefully to avoid double slashes
            let full_path = if normalized_path == "/" {
                format!("/{}", filename)
            } else if normalized_path == "." {
                filename.clone()
            } else {
                format!("{}/{}", normalized_path, filename)
            };

            entries.push(FileEntry {
                name: if is_dir {
                    format!("{}/", filename)
                } else {
                    filename.clone()
                },
                path: full_path,
                is_dir,
                is_hidden,
                size,
                modified: None, // SFTP metadata could provide this, but we'll skip for now
            });
        }

        // Sort: directories first, then alphabetically
        entries.sort_by(|a, b| match (a.is_dir, b.is_dir) {
            (true, false) => std::cmp::Ordering::Less,
            (false, true) => std::cmp::Ordering::Greater,
            _ => a.name.cmp(&b.name),
        });

        Ok(entries)
    }

    async fn exists(&self, path: &str) -> Result<bool> {
        match self.session.metadata(path).await {
            Ok(_) => Ok(true),
            Err(_) => Ok(false),
        }
    }

    async fn is_dir(&self, path: &str) -> Result<bool> {
        let metadata =
            self.session.metadata(path).await.map_err(|e| {
                Error::new(ErrorKind::Other, format!("SFTP metadata failed: {}", e))
            })?;

        Ok(metadata.is_dir())
    }

    async fn canonicalize(&self, path: &str) -> Result<String> {
        self.session
            .canonicalize(path)
            .await
            .map_err(|e| Error::new(ErrorKind::Other, format!("SFTP canonicalize failed: {}", e)))
    }

    fn parent(&self, path: &str) -> Option<String> {
        let path = path.trim_end_matches('/');
        if path.is_empty() || path == "/" {
            return None;
        }

        path.rsplit_once('/').map(|(parent, _)| {
            if parent.is_empty() {
                "/".to_string()
            } else {
                parent.to_string()
            }
        })
    }
}
