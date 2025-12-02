use std::{io::Read, path::PathBuf};

use ssh2_config::SshConfig;

use crate::error::{AppError, Result};

/// SSH config host information extracted from SSH config file
#[derive(Debug, Clone)]
pub struct SshConfigHost {
    pub hostname: String,
    pub port: Option<u16>,
    pub user: Option<String>,
    pub identity_file: Option<Vec<PathBuf>>,
}

/// Open SSH config from default location (~/.ssh/config)
fn open_default_ssh_config() -> Result<std::fs::File> {
    let home_dir = dirs::home_dir()
        .ok_or_else(|| AppError::ConfigError("Could not determine home directory".to_string()))?;

    let config_path = home_dir.join(".ssh").join("config");

    if !config_path.exists() {
        return Err(AppError::ConfigError(
            "SSH config file not found at ~/.ssh/config".to_string(),
        ));
    }

    // Open main config file
    let reader = std::fs::File::open(&config_path)
        .map_err(|e| AppError::ConfigError(format!("Failed to read SSH config file: {e}")))?;

    Ok(reader)
}

/// Parse SSH config from reader
fn parse(reader: impl Read) -> Result<SshConfig> {
    let mut reader = std::io::BufReader::new(reader);

    SshConfig::default()
        .parse(&mut reader, ssh2_config::ParseRule::STRICT)
        .map_err(|e| AppError::ConfigError(format!("Failed to parse SSH config: {e}")))
}

/// Query SSH config for a specific host and return connection details
pub fn query_ssh_config(host_pattern: &str) -> Result<SshConfigHost> {
    let reader = open_default_ssh_config()?;
    query_ssh_config_from_reader(host_pattern, reader)
}

/// Query SSH config for a specific host using a reader
pub fn query_ssh_config_from_reader<R: Read>(
    host_pattern: &str,
    reader: R,
) -> Result<SshConfigHost> {
    let config = parse(reader)?;

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
    let identity_file = params.identity_file.map(|paths| {
        paths
            .iter()
            .map(|path| {
                // Convert PathBuf to string and expand tilde
                let path_str = path.to_string_lossy().to_string();
                crate::expand_tilde(&path_str)
            })
            .collect()
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
    use std::io::Cursor;

    #[test]
    #[ignore]
    fn test_query_ssh_config_real() {
        // This test verifies against your actual SSH config*
        // Make sure you have "orb" configured in ~/.ssh/config*
        let result = query_ssh_config("orb");
        match result {
            Ok(host) => {
                println!("Successfully parsed SSH config for 'orb':");
                println!(" Hostname: {}", host.hostname);
                println!(" Port: {:?}", host.port);
                println!(" User: {:?}", host.user);
                println!(" Identity file: {:?}", host.identity_file);

                // Assertions based on your config*
                assert_eq!(host.hostname, "127.0.0.1");
                assert_eq!(host.port, Some(32222));
                assert_eq!(host.user, Some("default".to_string()));
                // Note: identity_file should be expanded from ~ to full path*
                assert!(host.identity_file.is_some());
                let identity = host.identity_file.unwrap();
                assert!(
                    identity
                        .first()
                        .unwrap()
                        .to_string_lossy()
                        .contains(".orbstack/ssh/id_ed25519")
                );
            }
            Err(e) => {
                panic!("Failed to query SSH config for 'orb': {:?}", e);
            }
        }
    }

    #[test]
    fn query_ssh_config_from_reader_parses_host_information() {
        let config = r#"
Host demo
    HostName example.com
Port 2222
    User testuser
    IdentityFile ~/.ssh/id_ed25519
"#;
        let cursor = Cursor::new(config);

        let host = query_ssh_config_from_reader("demo", cursor).expect("should parse host");

        assert_eq!(host.hostname, "example.com");
        assert_eq!(host.port, Some(2222));
        assert_eq!(host.user.as_deref(), Some("testuser"));
        assert!(host.identity_file.is_some());
        let identity = host.identity_file.unwrap();
        assert!(
            identity
                .first()
                .unwrap()
                .to_string_lossy()
                .contains(".ssh/id_ed25519")
        );
    }

    #[test]
    fn query_ssh_config_multiple_hosts() {
        let config = r#"
Host a.test_host
    Port 23
    IdentityFile /path/to/id_ed25519
    IdentityFile /path/to/id_ed25519_bak

Host *.test_host
    Hostname cae.com

Host *.test_host !a.test_host
    User invalid

Host *
    User cae
    Hostname invalid
    IdentityFile /path/to/id_rsa
"#;
        let cursor = Cursor::new(config);

        let host = query_ssh_config_from_reader("a.test_host", cursor).expect("should parse host");

        assert_eq!(host.hostname, "cae.com");
        assert_eq!(host.port, Some(23));
        assert_eq!(host.user.as_deref(), Some("cae"));
        assert_eq!(
            Some(vec![
                PathBuf::from("/path/to/id_ed25519"),
                PathBuf::from("/path/to/id_ed25519_bak")
            ]),
            host.identity_file
        );
    }
}
