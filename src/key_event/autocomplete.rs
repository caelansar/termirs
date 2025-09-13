use std::env;
use std::fs;
use std::path::{Path, PathBuf};

/// Auto-complete local file paths for SCP form
pub fn autocomplete_local_path(input: &str) -> Option<String> {
    // Handle empty input
    if input.is_empty() {
        return Some("./".to_string());
    }

    // Expand tilde to home directory
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

    // If path exists and is a directory, add trailing slash if missing
    if path.is_dir() && !expanded.ends_with('/') {
        return Some(expanded + "/");
    }

    // If path exists and is a file, return as-is
    if path.is_file() {
        return Some(expanded);
    }

    // Try to complete based on parent directory
    let (parent_dir, prefix) = if let Some(parent) = path.parent() {
        let filename = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        (parent.to_path_buf(), filename)
    } else {
        (PathBuf::from("."), expanded.clone())
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
            let path_str = if full_path.is_dir() {
                full_path.to_string_lossy().to_string() + "/"
            } else {
                full_path.to_string_lossy().to_string()
            };
            matches.push(path_str);
        }
    }

    match matches.len() {
        0 => None,
        1 => Some(matches.into_iter().next().unwrap()),
        _ => {
            // Find common prefix among matches
            let common = find_common_prefix(&matches);
            if common.len() > expanded.len() {
                Some(common)
            } else {
                // Return the first match if no common prefix extension
                Some(matches.into_iter().next().unwrap())
            }
        }
    }
}

/// Find the longest common prefix among a list of strings
pub fn find_common_prefix(strings: &[String]) -> String {
    if strings.is_empty() {
        return String::new();
    }

    let first = &strings[0];
    let mut common_len = first.len();

    for s in strings.iter().skip(1) {
        let mut len = 0;
        for (c1, c2) in first.chars().zip(s.chars()) {
            if c1 == c2 {
                len += c1.len_utf8();
            } else {
                break;
            }
        }
        common_len = common_len.min(len);
    }

    first[..common_len].to_string()
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
        (path.to_path_buf(), String::new())
    } else if let Some(parent) = path.parent() {
        let filename = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        (parent.to_path_buf(), filename)
    } else {
        (PathBuf::from("."), expanded.clone())
    };

    let entries = fs::read_dir(&parent_dir).ok()?;
    let mut options = Vec::new();

    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with(&prefix) && !name.starts_with('.') {
            options.push(name);
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

    // Expand tilde to home directory
    let expanded = if current_input.starts_with("~") {
        if let Ok(home) = env::var("HOME") {
            let home_path = PathBuf::from(home);
            let tail = &current_input[1..];
            if tail.is_empty() {
                home_path.to_string_lossy().to_string() + "/"
            } else {
                let tail = tail.strip_prefix('/').unwrap_or(tail);
                home_path.join(tail).to_string_lossy().to_string()
            }
        } else {
            current_input.to_string()
        }
    } else {
        current_input.to_string()
    };

    let path = Path::new(&expanded);

    // If the current path is a directory and ends with '/', append the selected option
    if path.is_dir() && expanded.ends_with('/') {
        return format!("{}{}", expanded, selected_option);
    }

    // If the current path has a parent directory, replace the filename with the selected option
    if let Some(parent) = path.parent() {
        let parent_str = parent.to_string_lossy();
        if parent_str.is_empty() || parent_str == "." {
            selected_option.to_string()
        } else {
            format!("{}/{}", parent_str, selected_option)
        }
    } else {
        // No parent directory, just use the selected option
        selected_option.to_string()
    }
}
