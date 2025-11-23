//! SFTP filesystem implementation.

use ratatui_explorer::{FileEntry, FileSystem};
use russh_sftp::client::SftpSession;
use std::io::{Error, Result};
use std::sync::Arc;
use tracing::{debug, error};

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
        debug!("SFTP read_dir: {}", path);
        let mut entries = Vec::new();

        // Normalize the path (remove trailing slashes, handle empty path and root)
        let normalized_path = if path.is_empty() || path == "." {
            ".".to_string()
        } else if path == "/" {
            // Special case: root directory should remain as "/"
            "/".to_string()
        } else {
            // Remove trailing slashes, but check if we end up with empty string (which means it was "/")
            let trimmed = path.trim_end_matches('/');
            if trimmed.is_empty() {
                "/".to_string()
            } else {
                trimmed.to_string()
            }
        };

        // Read directory from SFTP
        let read_dir = self.session.read_dir(&normalized_path).await.map_err(|e| {
            error!("SFTP read_dir failed for '{}': {}", normalized_path, e);
            Error::other(format!("SFTP read_dir failed for '{normalized_path}': {e}"))
        })?;

        for entry_result in read_dir {
            let entry = entry_result;

            let filename = entry.file_name().to_string();
            let is_hidden = filename.starts_with('.');

            // Construct the full path carefully to avoid double slashes
            let full_path = if normalized_path == "/" {
                format!("/{filename}")
            } else if normalized_path == "." {
                filename.clone()
            } else {
                format!("{normalized_path}/{filename}")
            };

            // Determine if this is a directory
            // For symlinks, we need to follow them to check if they point to directories
            let is_dir = if entry.file_type().is_symlink() {
                // Try to follow the symlink to get the target's metadata
                match self.session.metadata(&full_path).await {
                    Ok(target_metadata) => target_metadata.is_dir(),
                    Err(_) => false, // If we can't follow the symlink, treat it as a file
                }
            } else {
                entry.file_type().is_dir()
            };

            // Get file size from metadata
            let size = if !is_dir { entry.metadata().size } else { None };

            entries.push(FileEntry {
                name: if is_dir {
                    format!("{filename}/")
                } else {
                    filename
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

        debug!("SFTP read_dir completed for '{}': {} entries", normalized_path, entries.len());
        Ok(entries)
    }

    async fn exists(&self, path: &str) -> Result<bool> {
        match self.session.metadata(path).await {
            Ok(_) => Ok(true),
            Err(_) => Ok(false),
        }
    }

    async fn is_dir(&self, path: &str) -> Result<bool> {
        debug!("SFTP is_dir check: {}", path);
        let metadata = self
            .session
            .metadata(path)
            .await
            .map_err(|e| {
                debug!("SFTP metadata failed for '{}': {}", path, e);
                Error::other(format!("SFTP metadata failed: {e}"))
            })?;

        Ok(metadata.is_dir())
    }

    async fn canonicalize(&self, path: &str) -> Result<String> {
        self.session
            .canonicalize(path)
            .await
            .map_err(|e| Error::other(format!("SFTP canonicalize failed: {e}")))
    }

    fn parent(&self, path: &str) -> Option<String> {
        // Handle root directory - it has no parent
        if path == "/" {
            return None;
        }

        let trimmed = path.trim_end_matches('/');

        // If after trimming we get empty or just "/", no parent exists
        if trimmed.is_empty() || trimmed == "/" {
            return None;
        }

        // Find the last slash to get the parent
        trimmed.rsplit_once('/').map(|(parent, _)| {
            if parent.is_empty() {
                // Parent is root
                "/".to_string()
            } else {
                parent.to_string()
            }
        })
    }

    async fn delete(&self, path: &str) -> Result<()> {
        debug!("SFTP deleting file: {}", path);
        self.session
            .remove_file(path)
            .await
            .map_err(|e| {
                error!("SFTP delete failed for '{}': {}", path, e);
                Error::other(format!("SFTP delete failed: {e}"))
            })?;
        debug!("SFTP file deleted successfully: {}", path);
        Ok(())
    }
}
