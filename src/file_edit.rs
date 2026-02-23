//! File editing logic for opening files in an external editor.
//!
//! Supports both local and remote (SFTP) files. Remote files are downloaded
//! to a temporary file, edited locally, then uploaded back if modified.

use std::io;
use std::path::Path;
use std::time::SystemTime;

use crate::config::manager::Connection;
use crate::error::{AppError, Result};

/// Result of an edit operation.
pub enum EditResult {
    /// File was modified (mtime changed after editor exited).
    Modified,
    /// File was not modified (mtime unchanged).
    Unchanged,
}

const BINARY_DETECTION_BYTES: usize = 2048;

/// Read up to `BINARY_DETECTION_BYTES` from a local file for binary detection.
async fn read_file_head(path: &Path) -> io::Result<Vec<u8>> {
    use tokio::io::AsyncReadExt;
    let mut file = tokio::fs::File::open(path).await?;
    let mut buf = vec![0u8; BINARY_DETECTION_BYTES];
    let n = file.read(&mut buf).await?;
    buf.truncate(n);
    Ok(buf)
}

/// Check whether the given content looks like a binary file.
fn is_binary(content: &[u8]) -> bool {
    content_inspector::inspect(content).is_binary()
}

/// Check whether a local file looks like a binary file.
pub async fn check_binary(path: &str) -> Result<bool> {
    let file_path = Path::new(path);
    let head = read_file_head(file_path)
        .await
        .map_err(|e| AppError::SftpError(format!("Failed to read file for binary check: {e}")))?;
    Ok(is_binary(&head))
}

/// Check whether a remote file looks like a binary file by reading only the
/// first [`BINARY_DETECTION_BYTES`] bytes via SFTP.
pub async fn check_remote_binary(connection: Option<&Connection>, path: &str) -> Result<bool> {
    use crate::async_ssh_client::SshSession;
    let connection = connection.ok_or_else(|| {
        AppError::SftpError("Could not determine connection for remote file".to_string())
    })?;
    let head = SshSession::sftp_read_head(connection, path, BINARY_DETECTION_BYTES).await?;
    Ok(is_binary(&head))
}

/// Open a local file in the user's preferred editor.
///
/// Returns an error if the file is binary. Compares mtime before and after
/// the editor exits to determine whether the file was changed.
pub async fn edit_local_file(path: &str) -> Result<EditResult> {
    let file_path = Path::new(path);

    // Record mtime before editing
    let mtime_before = std::fs::metadata(file_path)
        .and_then(|m| m.modified())
        .unwrap_or(SystemTime::UNIX_EPOCH);

    // Open in editor (blocking)
    tracing::info!("Opening local file in editor: {}", file_path.display());
    edit::edit_file(file_path).map_err(|e| AppError::SftpError(format!("Editor failed: {e}")))?;

    // Compare mtime
    let mtime_after = std::fs::metadata(file_path)
        .and_then(|m| m.modified())
        .unwrap_or(SystemTime::UNIX_EPOCH);

    if mtime_after != mtime_before {
        Ok(EditResult::Modified)
    } else {
        Ok(EditResult::Unchanged)
    }
}

/// Download a remote file to a temp file, open it in the editor, and upload
/// back if modified.
///
/// This function is blocking (spawns editor) and should only be called when
/// the TUI is suspended.
pub async fn edit_remote_file(remote_path: &str, connection: &Connection) -> Result<EditResult> {
    use crate::async_ssh_client::SshSession;

    // Determine a file extension to preserve syntax highlighting in editors
    let extension = Path::new(remote_path)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("");

    // Create temp file with the same extension
    let tmp_file = if extension.is_empty() {
        tempfile::Builder::new()
            .prefix("termirs-edit-")
            .tempfile()
            .map_err(|e| AppError::SftpError(format!("Failed to create temp file: {e}")))?
    } else {
        tempfile::Builder::new()
            .prefix("termirs-edit-")
            .suffix(&format!(".{extension}"))
            .tempfile()
            .map_err(|e| AppError::SftpError(format!("Failed to create temp file: {e}")))?
    };
    let tmp_path = tmp_file.path().to_string_lossy().to_string();

    let (session, _server_key) = SshSession::new_session(connection).await?;

    let channel_recv = session.channel_open_session().await?;
    let channel_send = session.channel_open_session().await?;

    // Download remote file to temp
    SshSession::sftp_receive_file(
        Some(channel_recv),
        connection,
        remote_path,
        &tmp_path,
        0,
        None,
    )
    .await?;

    // Record mtime before editing
    let mtime_before = std::fs::metadata(tmp_file.path())
        .and_then(|m| m.modified())
        .unwrap_or(SystemTime::UNIX_EPOCH);

    // Open in editor (blocking)
    edit::edit_file(tmp_file.path())
        .map_err(|e| AppError::SftpError(format!("Editor failed: {e}")))?;

    // Compare mtime
    let mtime_after = std::fs::metadata(tmp_file.path())
        .and_then(|m| m.modified())
        .unwrap_or(SystemTime::UNIX_EPOCH);

    if mtime_after == mtime_before {
        return Ok(EditResult::Unchanged);
    }

    // Upload modified file back to remote
    SshSession::sftp_send_file(
        Some(channel_send),
        connection,
        &tmp_path,
        remote_path,
        0,
        None,
    )
    .await?;

    Ok(EditResult::Modified)
}
