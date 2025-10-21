use std::path::PathBuf;

use ssh2_config::SshConfig;

use crate::error::{AppError, Result};

/// SSH config host information extracted from SSH config file
#[derive(Debug, Clone)]
pub struct SshConfigHost {
    pub hostname: String,
    pub port: Option<u16>,
    pub user: Option<String>,
    pub identity_file: Option<String>,
}

/// Parse SSH config from default location (~/.ssh/config)
fn parse_ssh_config() -> Result<SshConfig> {
    let home_dir = std::env::var("HOME")
        .map_err(|_| AppError::ConfigError("HOME environment variable not set".to_string()))?;

    let config_path = PathBuf::from(&home_dir).join(".ssh").join("config");

    if !config_path.exists() {
        return Err(AppError::ConfigError(
            "SSH config file not found at ~/.ssh/config".to_string(),
        ));
    }

    // Read and parse main config file
    let reader = std::fs::File::open(&config_path)
        .map_err(|e| AppError::ConfigError(format!("Failed to read SSH config file: {e}")))?;

    // Parse the consolidated config
    let mut reader = std::io::BufReader::new(reader);

    SshConfig::default()
        .parse(&mut reader, ssh2_config::ParseRule::STRICT)
        .map_err(|e| AppError::ConfigError(format!("Failed to parse SSH config: {e}")))
}

/// Query SSH config for a specific host and return connection details
pub fn query_ssh_config(host_pattern: &str) -> Result<SshConfigHost> {
    let config = parse_ssh_config()?;

    // Query the config for the host pattern
    let params = config.query(host_pattern);

    // Extract hostname (use the host pattern if HostName is not specified)
    let hostname = params
        .host_name
        .clone()
        .unwrap_or_else(|| host_pattern.to_string());

    // Extract port/user
    let port = params.port;
    let user = params.user;

    // Extract identity file (prefer the first one if multiple exist)
    let identity_file = params
        .identity_file
        .as_ref()
        .and_then(|files| files.first())
        .map(|path| {
            // Convert PathBuf to string and expand tilde
            let path_str = path.to_string_lossy().to_string();
            crate::expand_tilde(&path_str).to_string_lossy().to_string()
        });

    // Validate that we have at least a hostname
    if hostname.is_empty() {
        return Err(AppError::ConfigError(format!(
            "No host found matching '{host_pattern}'"
        )));
    }

    Ok(SshConfigHost {
        hostname,
        port,
        user,
        identity_file,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "this test requires a real SSH config file in ~/.ssh/config"]
    fn test_query_ssh_config_real() {
        // This test verifies against your actual SSH config
        // Make sure you have "orb" configured in ~/.ssh/config
        let result = query_ssh_config("orb");
        match result {
            Ok(host) => {
                println!("Successfully parsed SSH config for 'orb':");
                println!("  Hostname: {}", host.hostname);
                println!("  Port: {:?}", host.port);
                println!("  User: {:?}", host.user);
                println!("  Identity file: {:?}", host.identity_file);

                // Assertions based on your config
                assert_eq!(host.hostname, "127.0.0.1");
                assert_eq!(host.port, Some(32222));
                assert_eq!(host.user, Some("default".to_string()));
                // Note: identity_file should be expanded from ~ to full path
                assert!(host.identity_file.is_some());
                let identity = host.identity_file.unwrap();
                assert!(identity.contains(".orbstack/ssh/id_ed25519"));
            }
            Err(e) => {
                panic!("Failed to query SSH config for 'orb': {:?}", e);
            }
        }
    }
}
