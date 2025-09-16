use std::env;
use std::fs;
use std::path::{Path, PathBuf};

/// Auto-complete local file paths for SCP form
pub fn autocomplete_local_path(input: &str) -> Option<String> {
    // Handle empty input
    if input.is_empty() {
        return Some("./".to_string());
    }

    // Expand tilde to home directory for filesystem operations
    let path = crate::expand_tilde(input);

    // If path exists and is a directory, add trailing slash if missing (preserve original format)
    if path.is_dir() && !input.ends_with('/') {
        return Some(input.to_string() + "/");
    }

    // If path exists and is a file, return as-is (preserve original format)
    if path.is_file() {
        return Some(input.to_string());
    }

    // Try to complete based on parent directory
    let (parent_dir, prefix) = if let Some(parent) = path.parent() {
        let filename = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        (parent.to_path_buf(), filename)
    } else {
        (PathBuf::from("."), path.to_string_lossy().to_string())
    };

    // Read directory entries
    let entries = match fs::read_dir(&parent_dir) {
        Ok(entries) => entries,
        Err(_) => return None,
    };

    let mut matches = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with(&prefix) && !name.starts_with('.') {
            let full_path = parent_dir.join(&name);

            // Preserve original format when constructing matches
            let path_str = if input.starts_with("~") {
                // For tilde paths, construct result in tilde format
                if let Ok(home) = env::var("HOME") {
                    if let Ok(relative) = full_path.strip_prefix(&home) {
                        let rel_str = relative.to_string_lossy();
                        if rel_str.is_empty() {
                            "~/".to_string()
                        } else if full_path.is_dir() {
                            format!("~/{}/", rel_str)
                        } else {
                            format!("~/{}", rel_str)
                        }
                    } else {
                        // Fallback to absolute path
                        if full_path.is_dir() {
                            full_path.to_string_lossy().to_string() + "/"
                        } else {
                            full_path.to_string_lossy().to_string()
                        }
                    }
                } else {
                    if full_path.is_dir() {
                        full_path.to_string_lossy().to_string() + "/"
                    } else {
                        full_path.to_string_lossy().to_string()
                    }
                }
            } else {
                // For non-tilde paths, use absolute paths as before
                if full_path.is_dir() {
                    full_path.to_string_lossy().to_string() + "/"
                } else {
                    full_path.to_string_lossy().to_string()
                }
            };
            matches.push(path_str);
        }
    }

    match matches.len() {
        1 => Some(matches.into_iter().next().unwrap()),
        _ => None,
    }
}

/// List available completion options for display
pub fn list_completion_options(input: &str) -> Option<Vec<String>> {
    let expanded = if input.starts_with("~") {
        if let Ok(home) = env::var("HOME") {
            let home_path = PathBuf::from(home);
            let tail = &input[1..];
            if tail.is_empty() {
                home_path.to_string_lossy().to_string() + "/"
            } else {
                let tail = tail.strip_prefix('/').unwrap_or(tail);
                home_path.join(tail).to_string_lossy().to_string()
            }
        } else {
            input.to_string()
        }
    } else {
        input.to_string()
    };

    let path = Path::new(&expanded);
    let (parent_dir, prefix) = if path.is_dir() && expanded.ends_with('/') {
        // For directories ending with '/', list all contents
        (path.to_path_buf(), String::new())
    } else if let Some(parent) = path.parent() {
        let filename = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        (parent.to_path_buf(), filename)
    } else {
        // Handle cases like "./" or "../" or current directory
        let canonical_path = if expanded == "." || expanded == "./" {
            PathBuf::from(".")
        } else if expanded == ".." || expanded == "../" {
            PathBuf::from("..")
        } else {
            PathBuf::from(".")
        };
        (canonical_path, String::new())
    };

    let entries = fs::read_dir(&parent_dir).ok()?;
    let mut options = Vec::new();

    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        // For relative paths like "./" or "../", show all files/directories
        // For other cases, filter by prefix
        if prefix.is_empty() || name.starts_with(&prefix) {
            // Skip hidden files unless specifically searching for them
            if !name.starts_with('.') || prefix.starts_with('.') {
                options.push(name);
            }
        }
    }

    if options.is_empty() {
        None
    } else {
        options.sort();
        Some(options)
    }
}

/// Construct the completed path by combining the current input with the selected option
pub fn construct_completed_path(current_input: &str, selected_option: &str) -> String {
    // Handle empty input
    if current_input.is_empty() {
        return format!("./{}", selected_option);
    }

    // Special handling for tilde paths to preserve the tilde format
    if current_input.starts_with("~") {
        let tail = &current_input[1..];
        if tail.is_empty() || tail == "/" {
            // For "~" or "~/", append the selected option
            return format!("~/{}", selected_option);
        } else {
            // For "~/something", we need to check if we're replacing the last part
            if let Some(last_slash_pos) = tail.rfind('/') {
                // There's a path after ~/, replace the last component
                let prefix = &tail[..last_slash_pos + 1]; // Include the slash
                return format!("~{}{}", prefix, selected_option);
            } else {
                // Direct child of home directory, replace the tail
                return format!("~/{}", selected_option);
            }
        }
    }

    // For non-tilde paths, expand and process normally
    let expanded = current_input.to_string();
    let path = Path::new(&expanded);

    // If the current path is a directory and ends with '/', append the selected option
    if path.is_dir() && expanded.ends_with('/') {
        return format!("{}{}", expanded, selected_option);
    }

    // Handle relative paths like "./" and "../"
    if expanded == "./" || expanded == "../" {
        return format!("{}{}", expanded, selected_option);
    }

    // If the current path has a parent directory, replace the filename with the selected option
    if let Some(parent) = path.parent() {
        let parent_str = parent.to_string_lossy();
        if parent_str.is_empty() {
            selected_option.to_string()
        } else if parent_str == "." {
            // For current directory, preserve the "./" format if it was originally there
            if current_input.starts_with("./") {
                format!("./{}", selected_option)
            } else {
                selected_option.to_string()
            }
        } else {
            format!("{}/{}", parent_str, selected_option)
        }
    } else {
        // No parent directory, just use the selected option
        selected_option.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_list_completion_options() {
        let res = list_completion_options("./");
        assert!(res.is_some());
        assert!(res.unwrap().contains(&"Cargo.toml".to_string()));
    }
}
