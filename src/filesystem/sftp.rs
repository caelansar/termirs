//! SFTP filesystem implementation.

use futures::{StreamExt, TryStreamExt};
use ratatui_explorer::{FileEntry, FilePermissions, FileSystem};
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

    /// Expose the inner SFTP session.
    pub fn session(&self) -> &Arc<SftpSession> {
        &self.session
    }

    /// Create a single directory on the remote.
    pub async fn create_dir(&self, path: &str) -> Result<()> {
        self.session.create_dir(path).await.map_err(|e| {
            error!("SFTP mkdir failed for '{}': {}", path, e);
            Error::other(format!("SFTP mkdir failed for '{path}': {e}"))
        })
    }

    /// Create directory and all parents (like `mkdir -p`).
    /// Ignores "already exists" errors for intermediate components.
    pub async fn create_dir_all(&self, path: &str) -> Result<()> {
        let components: Vec<&str> = path.split('/').filter(|c| !c.is_empty()).collect();
        let mut current = if path.starts_with('/') {
            String::from("/")
        } else {
            String::new()
        };

        for component in components {
            if current.len() > 1 || (!current.is_empty() && !current.ends_with('/')) {
                current.push('/');
            }
            current.push_str(component);

            match self.session.create_dir(&current).await {
                Ok(()) => {}
                Err(_) => {
                    // Check if it already exists as a directory
                    match self.is_dir(&current).await {
                        Ok(true) => {} // Already exists
                        _ => {
                            return Err(Error::other(format!(
                                "Failed to create directory '{current}'"
                            )));
                        }
                    }
                }
            }
        }
        Ok(())
    }
}

impl FileSystem for SftpFileSystem {
    async fn read_dir(&self, path: &str) -> Result<Vec<FileEntry>> {
        debug!("SFTP read_dir: {}", path);

        // Normalize path using string operations
        let normalized_path = if path.is_empty() || path == "." {
            ".".to_string()
        } else if path == "/" {
            "/".to_string()
        } else {
            let trimmed = path.trim_end_matches('/');
            if trimmed.is_empty() {
                "/".to_string()
            } else {
                trimmed.to_string()
            }
        };

        // Read directory from SFTP using streaming, processing entries concurrently
        let stream = self
            .session
            .read_dir_stream(&normalized_path)
            .await
            .map_err(|e| {
                error!("SFTP read_dir failed for '{}': {}", normalized_path, e);
                Error::other(format!("SFTP read_dir failed for '{normalized_path}': {e}"))
            })?;

        let session = &self.session;
        let mut entries: Vec<FileEntry> = stream
            .map(|entry_result| {
                let normalized_path = &normalized_path;
                async move {
                    let entry = entry_result.map_err(|e| {
                        error!("SFTP read_dir entry error for '{}': {}", normalized_path, e);
                        Error::other(format!(
                            "SFTP read_dir entry failed for '{normalized_path}': {e}"
                        ))
                    })?;

                    let filename = entry.file_name();
                    let is_hidden = filename.starts_with('.');

                    let full_path = if *normalized_path == "/" {
                        format!("/{filename}")
                    } else if *normalized_path == "." {
                        filename.clone()
                    } else {
                        format!("{normalized_path}/{filename}")
                    };

                    let file_type = entry.file_type();

                    // For symlinks, follow once to resolve the target's type
                    let (is_dir, is_file) = if file_type.is_symlink() {
                        match session.metadata(&full_path).await {
                            Ok(meta) => (meta.is_dir(), meta.is_regular()),
                            Err(_) => (false, false),
                        }
                    } else {
                        (file_type.is_dir(), file_type.is_file())
                    };

                    let size = if !is_dir { entry.metadata().size } else { None };
                    let modified = entry.metadata().modified().ok();
                    let is_symlink = file_type.is_symlink();

                    let symlink_target = if is_symlink {
                        session.read_link(&full_path).await.ok()
                    } else {
                        None
                    };

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
                    });

                    Ok::<FileEntry, Error>(FileEntry {
                        name: if is_dir {
                            format!("{filename}/")
                        } else {
                            filename
                        },
                        path: full_path,
                        is_file,
                        is_dir,
                        is_hidden,
                        size,
                        modified,
                        permissions,
                        is_symlink,
                        symlink_target,
                    })
                }
            })
            .buffer_unordered(16)
            .try_collect()
            .await?;

        // Sort: directories first, then alphabetically
        entries.sort_by(|a, b| match (a.is_dir, b.is_dir) {
            (true, false) => std::cmp::Ordering::Less,
            (false, true) => std::cmp::Ordering::Greater,
            _ => a.name.cmp(&b.name),
        });

        debug!(
            "SFTP read_dir completed for '{}': {} entries",
            normalized_path,
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
