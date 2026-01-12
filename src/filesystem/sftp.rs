//! SFTP filesystem implementation.

use ratatui_explorer::{FileEntry, FilePermissions, FileSystem};
use russh_sftp::client::SftpSession;
use std::io::{Error, Result};
use std::path::PathBuf;
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

        // Normalize the path using PathBuf for safer path manipulation
        let path_buf = PathBuf::from(path);
        let normalized_path = if path.is_empty() || path == "." {
            PathBuf::from(".")
        } else if path == "/" {
            PathBuf::from("/")
        } else {
            // PathBuf automatically handles trailing slashes
            let mut normalized = PathBuf::new();
            for component in path_buf.components() {
                normalized.push(component);
            }
            // Ensure we don't end up with empty path
            if normalized.as_os_str().is_empty() {
                PathBuf::from("/")
            } else {
                normalized
            }
        };

        // Convert PathBuf to string for SFTP API
        let normalized_str = normalized_path.to_string_lossy();

        // Read directory from SFTP
        let read_dir = self
            .session
            .read_dir(normalized_str.as_ref())
            .await
            .map_err(|e| {
                error!("SFTP read_dir failed for '{}': {}", normalized_str, e);
                Error::other(format!("SFTP read_dir failed for '{normalized_str}': {e}"))
            })?;

        for entry_result in read_dir {
            let entry = entry_result;

            let filename = entry.file_name();
            let is_hidden = filename.starts_with('.');

            // Construct full path using PathBuf's join method
            let full_path = normalized_path.join(&filename);
            let full_path_str = full_path.to_string_lossy();

            // Determine if this is a directory
            // For symlinks, we need to follow them to check if they point to directories
            let is_dir = if entry.file_type().is_symlink() {
                // Try to follow the symlink to get the target's metadata
                match self.session.metadata(full_path_str.as_ref()).await {
                    Ok(target_metadata) => target_metadata.is_dir(),
                    Err(_) => false, // If we can't follow the symlink, treat it as a file
                }
            } else {
                entry.file_type().is_dir()
            };

            // Get file size from metadata
            let size = if !is_dir { entry.metadata().size } else { None };

            // Get modified time from SFTP metadata
            let modified = entry.metadata().modified().ok();

            // Check if it's a symlink
            let is_symlink = entry.file_type().is_symlink();

            // Get permissions and convert to ratatui_explorer::FilePermissions
            let sftp_perms = entry.metadata().permissions();
            let permissions = Some(FilePermissions {
                user_read: sftp_perms.owner_read,
                user_write: sftp_perms.owner_write,
                user_execute: sftp_perms.owner_exec,
                group_read: sftp_perms.group_read,
                group_write: sftp_perms.group_write,
                group_execute: sftp_perms.group_exec,
                others_read: sftp_perms.other_read,
                others_write: sftp_perms.other_write,
                others_execute: sftp_perms.other_exec,
                is_symlink,
            });

            entries.push(FileEntry {
                name: if is_dir {
                    format!("{filename}/")
                } else {
                    filename
                },
                path: full_path_str.to_string(),
                is_dir,
                is_hidden,
                size,
                modified,
                permissions,
            });
        }

        // Sort: directories first, then alphabetically
        entries.sort_by(|a, b| match (a.is_dir, b.is_dir) {
            (true, false) => std::cmp::Ordering::Less,
            (false, true) => std::cmp::Ordering::Greater,
            _ => a.name.cmp(&b.name),
        });

        debug!(
            "SFTP read_dir completed for '{}': {} entries",
            normalized_path.display(),
            entries.len()
        );
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
        let metadata = self.session.metadata(path).await.map_err(|e| {
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
        self.session.remove_file(path).await.map_err(|e| {
            error!("SFTP delete failed for '{}': {}", path, e);
            Error::other(format!("SFTP delete failed: {e}"))
        })?;
        debug!("SFTP file deleted successfully: {}", path);
        Ok(())
    }
}
