//! Recursive directory traversal for SFTP directory copy.
//!
//! Produces a [`DirectoryManifest`] containing an ordered list of directories to create
//! and a flat list of files to transfer, suitable for feeding into the existing
//! `ScpTransferSpec` pipeline.

use std::collections::VecDeque;
use std::io::{Error, Result};

use ratatui_explorer::FileSystem;

use super::SftpFileSystem;

/// Result of walking a directory tree.
pub struct DirectoryManifest {
    /// Directories to create on the destination, in BFS order (parents before children).
    pub directories: Vec<String>,
    /// Files to transfer: `(source_absolute_path, dest_absolute_path, file_size)`.
    pub files: Vec<(String, String, Option<u64>)>,
}

/// Recursively walk a local directory tree using BFS.
///
/// `source_root` is the absolute path of the directory being copied.
/// `dest_root` is the absolute path where it should be created on the destination.
pub async fn walk_local_dir(source_root: &str, dest_root: &str) -> Result<DirectoryManifest> {
    let source_root = source_root.trim_end_matches('/');
    let dest_root = dest_root.trim_end_matches('/');

    let mut directories = Vec::new();
    let mut files = Vec::new();

    // The root destination directory itself
    directories.push(dest_root.to_string());

    // BFS queue: (source_dir_path, dest_dir_path)
    let mut queue: VecDeque<(String, String)> =
        VecDeque::from([(source_root.to_string(), dest_root.to_string())]);

    while let Some((src_dir, dst_dir)) = queue.pop_front() {
        let mut read_dir = tokio::fs::read_dir(&src_dir).await.map_err(|e| {
            Error::other(format!("Failed to read local directory '{src_dir}': {e}"))
        })?;

        while let Some(entry) = read_dir
            .next_entry()
            .await
            .map_err(|e| Error::other(format!("Failed to read entry in '{src_dir}': {e}")))?
        {
            let file_name = entry.file_name().to_string_lossy().into_owned();
            let src_path = format!("{}/{}", src_dir.trim_end_matches('/'), file_name);
            let dst_path = format!("{}/{}", dst_dir.trim_end_matches('/'), file_name);

            // Follow symlinks to determine actual type
            let metadata = tokio::fs::metadata(&src_path).await.map_err(|e| {
                Error::other(format!("Failed to get metadata for '{src_path}': {e}"))
            })?;

            if metadata.is_dir() {
                directories.push(dst_path.clone());
                queue.push_back((src_path, dst_path));
            } else if metadata.is_file() {
                files.push((src_path, dst_path, Some(metadata.len())));
            }
            // Skip other file types (sockets, etc.)
        }
    }

    Ok(DirectoryManifest { directories, files })
}

/// Recursively walk a remote SFTP directory tree using BFS.
///
/// `source_root` is the absolute path of the remote directory being copied.
/// `dest_root` is the absolute path where it should be created on the destination.
pub async fn walk_remote_dir(
    sftp: &SftpFileSystem,
    source_root: &str,
    dest_root: &str,
) -> Result<DirectoryManifest> {
    let source_root = source_root.trim_end_matches('/');
    let dest_root = dest_root.trim_end_matches('/');

    let mut directories = Vec::new();
    let mut files = Vec::new();

    // The root destination directory itself
    directories.push(dest_root.to_string());

    // BFS queue: (source_dir_path, dest_dir_path)
    let mut queue: VecDeque<(String, String)> =
        VecDeque::from([(source_root.to_string(), dest_root.to_string())]);

    while let Some((src_dir, dst_dir)) = queue.pop_front() {
        let entries = sftp.read_dir(&src_dir).await?;

        for entry in entries {
            let name = entry.name.trim_end_matches('/');
            let src_path = format!("{}/{}", src_dir.trim_end_matches('/'), name);
            let dst_path = format!("{}/{}", dst_dir.trim_end_matches('/'), name);

            if entry.is_dir {
                directories.push(dst_path.clone());
                queue.push_back((src_path, dst_path));
            } else if entry.is_file {
                files.push((src_path, dst_path, entry.size));
            }
        }
    }

    Ok(DirectoryManifest { directories, files })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn test_dir(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("termirs_test_{name}_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[tokio::test]
    async fn test_walk_local_empty_dir() {
        let tmp = test_dir("empty");
        let src = tmp.join("src_dir");
        fs::create_dir(&src).unwrap();

        let manifest = walk_local_dir(src.to_str().unwrap(), "/dest/src_dir")
            .await
            .unwrap();

        assert_eq!(manifest.directories, vec!["/dest/src_dir"]);
        assert!(manifest.files.is_empty());

        let _ = fs::remove_dir_all(&tmp);
    }

    #[tokio::test]
    async fn test_walk_local_nested() {
        let tmp = test_dir("nested");
        let src = tmp.join("root");
        fs::create_dir_all(src.join("sub1/sub2")).unwrap();
        fs::write(src.join("file1.txt"), "hello").unwrap();
        fs::write(src.join("sub1/file2.txt"), "world").unwrap();

        let manifest = walk_local_dir(src.to_str().unwrap(), "/dest/root")
            .await
            .unwrap();

        // Root + sub1 + sub2
        assert_eq!(manifest.directories.len(), 3);
        assert_eq!(manifest.directories[0], "/dest/root");
        assert!(
            manifest
                .directories
                .contains(&"/dest/root/sub1".to_string())
        );
        assert!(
            manifest
                .directories
                .contains(&"/dest/root/sub1/sub2".to_string())
        );

        assert_eq!(manifest.files.len(), 2);

        let file_dests: Vec<&str> = manifest.files.iter().map(|(_, d, _)| d.as_str()).collect();
        assert!(file_dests.contains(&"/dest/root/file1.txt"));
        assert!(file_dests.contains(&"/dest/root/sub1/file2.txt"));

        let _ = fs::remove_dir_all(&tmp);
    }

    #[tokio::test]
    async fn test_walk_local_preserves_parent_order() {
        let tmp = test_dir("order");
        let src = tmp.join("a");
        fs::create_dir_all(src.join("b/c/d")).unwrap();

        let manifest = walk_local_dir(src.to_str().unwrap(), "/dest/a")
            .await
            .unwrap();

        let pos_a = manifest
            .directories
            .iter()
            .position(|d| d == "/dest/a")
            .unwrap();
        let pos_b = manifest
            .directories
            .iter()
            .position(|d| d == "/dest/a/b")
            .unwrap();
        let pos_c = manifest
            .directories
            .iter()
            .position(|d| d == "/dest/a/b/c")
            .unwrap();
        let pos_d = manifest
            .directories
            .iter()
            .position(|d| d == "/dest/a/b/c/d")
            .unwrap();

        assert!(pos_a < pos_b);
        assert!(pos_b < pos_c);
        assert!(pos_c < pos_d);

        let _ = fs::remove_dir_all(&tmp);
    }
}
