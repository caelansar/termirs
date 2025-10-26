use std::{
    fs,
    path::{Path, PathBuf},
};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::{AppError, Result};

pub const DEFAULT_TERMINAL_SCROLLBACK_LINES: usize = 2000;
pub const MAX_TERMINAL_SCROLLBACK_LINES: usize = 5000;

fn default_terminal_scrollback_lines() -> usize {
    DEFAULT_TERMINAL_SCROLLBACK_LINES
}

/// Application settings
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct AppSettings {
    pub default_port: u16,
    pub connection_timeout: u64,
    #[serde(default = "default_terminal_scrollback_lines")]
    pub terminal_scrollback_lines: usize,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            default_port: 22,
            connection_timeout: 20,
            terminal_scrollback_lines: DEFAULT_TERMINAL_SCROLLBACK_LINES,
        }
    }
}

fn serialize_password<S>(plain: &str, serializer: S) -> std::result::Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    let enc = crate::config::encryption::PasswordEncryption::new();
    let encrypted = enc
        .encrypt_password(plain)
        .map_err(serde::ser::Error::custom)?;
    serializer.serialize_str(&encrypted)
}

fn deserialize_password<'de, D>(deserializer: D) -> std::result::Result<String, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let encrypted = String::deserialize(deserializer)?;
    let enc = crate::config::encryption::PasswordEncryption::new();
    enc.decrypt_password(&encrypted)
        .map_err(serde::de::Error::custom)
}

fn serialize_password_option<S>(
    plain: &Option<String>,
    serializer: S,
) -> std::result::Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    if let Some(plain) = plain {
        let enc = crate::config::encryption::PasswordEncryption::new();
        let encrypted = enc
            .encrypt_password(plain)
            .map_err(serde::ser::Error::custom)?;
        serializer.serialize_str(&encrypted)
    } else {
        serializer.serialize_none()
    }
}

fn deserialize_password_option<'de, D>(
    deserializer: D,
) -> std::result::Result<Option<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let maybe_encrypted = Option::<String>::deserialize(deserializer)?;
    match maybe_encrypted {
        Some(encrypted) => {
            let enc = crate::config::encryption::PasswordEncryption::new();
            enc.decrypt_password(&encrypted)
                .map(Some)
                .map_err(serde::de::Error::custom)
        }
        None => Ok(None),
    }
}

/// Represents an SSH connection configuration
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Connection {
    pub id: String,
    pub display_name: String,
    pub host: String,
    pub port: u16,
    pub username: String,
    pub auth_method: AuthMethod,
    pub created_at: DateTime<Utc>,
    pub last_used: Option<DateTime<Utc>>,
    pub public_key: Option<String>,
}

/// Status of a port forwarding session
#[derive(Clone, Debug, PartialEq, Default, Hash)]
pub enum PortForwardStatus {
    #[default]
    Stopped,
    Running,
    Failed(String),
}

/// Represents a port forwarding configuration
#[derive(Serialize, Deserialize, Clone, Debug, Hash)]
pub struct PortForward {
    pub id: String,
    pub connection_id: String,
    pub local_addr: String,
    pub local_port: u16,
    pub service_host: String,
    pub service_port: u16,
    pub display_name: Option<String>,
    pub created_at: DateTime<Utc>,
    #[serde(skip)] // Runtime status, not persisted
    pub status: PortForwardStatus,
}

impl PortForward {
    /// Creates a new port forward with the given parameters
    pub fn new(
        connection_id: String,
        local_addr: String,
        local_port: u16,
        service_host: String,
        service_port: u16,
        display_name: Option<String>,
    ) -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            connection_id,
            local_addr,
            local_port,
            service_host,
            service_port,
            display_name,
            created_at: Utc::now(),
            status: PortForwardStatus::Stopped,
        }
    }

    /// Validates the port forward parameters
    pub fn validate(&self) -> Result<()> {
        if self.local_addr.trim().is_empty() {
            return Err(AppError::ValidationError(
                "Local address cannot be empty".to_string(),
            ));
        }

        if self.local_port == 0 {
            return Err(AppError::ValidationError(
                "Local port must be greater than 0".to_string(),
            ));
        }

        if self.service_host.trim().is_empty() {
            return Err(AppError::ValidationError(
                "Service host cannot be empty".to_string(),
            ));
        }

        if self.service_port == 0 {
            return Err(AppError::ValidationError(
                "Service port must be greater than 0".to_string(),
            ));
        }

        if self.connection_id.trim().is_empty() {
            return Err(AppError::ValidationError(
                "Connection ID cannot be empty".to_string(),
            ));
        }

        Ok(())
    }

    /// Gets the display name or generates a default one
    pub fn get_display_name(&self) -> String {
        self.display_name.clone().unwrap_or_else(|| {
            format!(
                "{}:{} -> {}:{}",
                self.local_addr, self.local_port, self.service_host, self.service_port
            )
        })
    }

    /// Gets the local address and port as a string
    pub fn local_address(&self) -> String {
        format!("{}:{}", self.local_addr, self.local_port)
    }

    /// Gets the service address and port as a string
    pub fn service_address(&self) -> String {
        format!("{}:{}", self.service_host, self.service_port)
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub enum AuthMethod {
    #[serde(rename = "password")]
    Password(
        #[serde(
            serialize_with = "serialize_password",
            deserialize_with = "deserialize_password"
        )]
        String,
    ),
    #[serde(rename = "public_key")]
    PublicKey {
        private_key_path: String,
        #[serde(
            default,
            serialize_with = "serialize_password_option",
            deserialize_with = "deserialize_password_option"
        )]
        passphrase: Option<String>,
    },
    #[serde(rename = "auto_load_key")]
    AutoLoadKey,
}

impl Connection {
    /// Creates a new connection with the given parameters
    pub fn new(host: String, port: u16, username: String, auth_method: AuthMethod) -> Self {
        let display_name = host.clone(); // Default display name is the host
        Self {
            id: Uuid::new_v4().to_string(),
            display_name,
            host,
            port,
            username,
            auth_method,
            created_at: Utc::now(),
            last_used: None,
            public_key: None,
        }
    }

    pub fn host_port(&self) -> String {
        format!("{}:{}", self.host, self.port)
    }

    /// Validates the connection parameters
    pub fn validate(&self) -> Result<()> {
        if self.host.trim().is_empty() {
            return Err(AppError::ValidationError(
                "Host cannot be empty".to_string(),
            ));
        }

        if self.port == 0 {
            return Err(AppError::ValidationError(
                "Port must be greater than 0".to_string(),
            ));
        }

        if self.username.trim().is_empty() {
            return Err(AppError::ValidationError(
                "Username cannot be empty".to_string(),
            ));
        }

        if let AuthMethod::Password(password) = &self.auth_method
            && password.trim().is_empty()
        {
            return Err(AppError::ValidationError(
                "Password cannot be empty".to_string(),
            ));
        }

        Ok(())
    }

    /// Updates the last used timestamp
    pub fn update_last_used(&mut self) {
        self.last_used = Some(Utc::now());
    }

    /// Sets a custom display name
    pub fn set_display_name(&mut self, name: String) {
        self.display_name = name;
    }
}

/// Main configuration structure
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct Config {
    pub connections: Vec<Connection>,
    #[serde(default)]
    pub port_forwards: Vec<PortForward>,
    pub settings: AppSettings,
}
/// Configuration manager for handling application settings and connection storage
pub struct ConfigManager {
    config_path: PathBuf,
    config: Config,
}

impl ConfigManager {
    /// Create a new configuration manager
    pub fn new() -> Result<Self> {
        let config_path = Self::get_config_path()?;
        let mut config = Self::load_config_from_path(&config_path)?;
        Self::normalize_settings(&mut config);

        Ok(Self {
            config_path,
            config,
        })
    }

    pub fn default_port(&self) -> u16 {
        self.config.settings.default_port
    }

    /// Create a configuration manager with a custom config path (useful for testing)
    #[allow(dead_code)]
    pub fn with_path<P: AsRef<Path>>(config_path: P) -> Result<Self> {
        let config_path = config_path.as_ref().to_path_buf();
        let mut config = Self::load_config_from_path(&config_path)?;
        Self::normalize_settings(&mut config);

        Ok(Self {
            config_path,
            config,
        })
    }

    /// Get the default configuration file path
    fn get_config_path() -> Result<PathBuf> {
        let home_dir = std::env::var("HOME")
            .map_err(|_| AppError::ConfigError("HOME environment variable not set".to_string()))?;

        let config_dir = Path::new(&home_dir).join(".config").join("termirs");

        // Create config directory if it doesn't exist
        if !config_dir.exists() {
            fs::create_dir_all(&config_dir).map_err(|e| {
                AppError::ConfigError(format!("Failed to create config directory: {e}"))
            })?;
        }

        Ok(config_dir.join("config.toml"))
    }

    /// Load configuration from the specified path
    fn load_config_from_path(config_path: &Path) -> Result<Config> {
        if !config_path.exists() {
            // Return default config if file doesn't exist
            return Ok(Config::default());
        }

        let config_content = fs::read_to_string(config_path)
            .map_err(|e| AppError::ConfigError(format!("Failed to read config file: {e}")))?;

        let config: Config = toml::from_str(&config_content)
            .map_err(|e| AppError::ConfigError(format!("Failed to parse config file: {e}")))?;

        Ok(config)
    }

    /// Persist current config to disk
    pub fn save(&self) -> Result<()> {
        let toml = toml::to_string_pretty(&self.config)
            .map_err(|e| AppError::ConfigError(format!("Failed to serialize config: {e}")))?;
        fs::write(&self.config_path, toml)
            .map_err(|e| AppError::ConfigError(format!("Failed to write config: {e}")))?;
        Ok(())
    }

    /// Return immutable slice of connections
    pub fn connections(&self) -> &[Connection] {
        &self.config.connections
    }

    /// Return mutable slice of connections
    pub fn connections_mut(&mut self) -> &mut Vec<Connection> {
        &mut self.config.connections
    }

    pub fn terminal_scrollback_lines(&self) -> usize {
        self.config.settings.terminal_scrollback_lines
    }

    /// Find a connection by ID
    pub fn find_connection(&self, id: &str) -> Option<&Connection> {
        self.config.connections.iter().find(|c| c.id == id)
    }

    /// Add a new connection and persist it
    pub fn add_connection(&mut self, connection: Connection) -> Result<()> {
        // Validate the connection before adding
        connection.validate()?;

        // Best-effort dedup: same host/port/username
        if !self.config.connections.iter().any(|c| {
            c.host == connection.host
                && c.port == connection.port
                && c.username == connection.username
                && c.display_name == connection.display_name
        }) {
            self.config.connections.push(connection);
        } else {
            return Err(AppError::ConfigError(
                "Connection already exists".to_string(),
            ));
        }
        self.save()
    }

    /// Update an existing connection
    pub fn update_connection(&mut self, connection: Connection) -> Result<()> {
        // Validate the connection before updating
        connection.validate()?;

        // Find and update the connection
        if let Some(existing_conn) = self
            .config
            .connections
            .iter_mut()
            .find(|conn| conn.id == connection.id)
        {
            *existing_conn = connection;
            Ok(())
        } else {
            Err(AppError::ConfigError("Connection not found".to_string()))
        }
    }

    /// Remove a connection by ID
    pub fn remove_connection(&mut self, id: &str) -> Result<()> {
        let initial_len = self.config.connections.len();
        self.config.connections.retain(|conn| conn.id != id);

        if self.config.connections.len() == initial_len {
            Err(AppError::ConfigError("Connection not found".to_string()))
        } else {
            Ok(())
        }
    }

    /// Update last_used for a connection by id and persist
    pub fn touch_last_used(&mut self, id: &str) -> Result<()> {
        if let Some(c) = self.config.connections.iter_mut().find(|c| c.id == id) {
            c.update_last_used();
            self.save()?;
        }
        Ok(())
    }

    /// Return immutable slice of port forwards
    pub fn port_forwards(&self) -> &[PortForward] {
        &self.config.port_forwards
    }

    /// Return mutable slice of port forwards
    pub fn port_forwards_mut(&mut self) -> &mut Vec<PortForward> {
        &mut self.config.port_forwards
    }

    /// Add a new port forward and persist it
    pub fn add_port_forward(&mut self, port_forward: PortForward) -> Result<()> {
        // Validate the port forward before adding
        port_forward.validate()?;

        // Check for duplicates based on connection_id, local_addr, local_port, service_host, service_port
        if self.config.port_forwards.iter().any(|pf| {
            pf.connection_id == port_forward.connection_id
                && pf.local_addr == port_forward.local_addr
                && pf.local_port == port_forward.local_port
                && pf.service_host == port_forward.service_host
                && pf.service_port == port_forward.service_port
        }) {
            return Err(AppError::ConfigError(
                "Port forward already exists with the same configuration".to_string(),
            ));
        }

        self.config.port_forwards.push(port_forward);
        self.save()
    }

    /// Update an existing port forward
    pub fn update_port_forward(&mut self, port_forward: PortForward) -> Result<()> {
        // Validate the port forward before updating
        port_forward.validate()?;

        // Find and update the port forward
        if let Some(existing_pf) = self
            .config
            .port_forwards
            .iter_mut()
            .find(|pf| pf.id == port_forward.id)
        {
            *existing_pf = port_forward;
            Ok(())
        } else {
            Err(AppError::ConfigError("Port forward not found".to_string()))
        }
    }

    /// Remove a port forward by ID
    pub fn remove_port_forward(&mut self, id: &str) -> Result<()> {
        let initial_len = self.config.port_forwards.len();
        self.config.port_forwards.retain(|pf| pf.id != id);

        if self.config.port_forwards.len() == initial_len {
            Err(AppError::ConfigError("Port forward not found".to_string()))
        } else {
            Ok(())
        }
    }

    /// Find a port forward by ID (mutable)
    pub fn find_port_forward_mut(&mut self, id: &str) -> Option<&mut PortForward> {
        self.config.port_forwards.iter_mut().find(|pf| pf.id == id)
    }

    fn normalize_settings(config: &mut Config) {
        if config.settings.terminal_scrollback_lines == 0 {
            config.settings.terminal_scrollback_lines = DEFAULT_TERMINAL_SCROLLBACK_LINES;
        }
        if config.settings.terminal_scrollback_lines > MAX_TERMINAL_SCROLLBACK_LINES {
            config.settings.terminal_scrollback_lines = MAX_TERMINAL_SCROLLBACK_LINES;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_serialize_deserialize_connection_public_key() {
        let conn = Connection::new(
            "test".to_string(),
            22,
            "root".to_string(),
            AuthMethod::PublicKey {
                private_key_path: "path".to_string(),
                passphrase: None,
            },
        );
        let serialized = toml::to_string(&conn).unwrap();
        println!("serialized: {}", serialized);

        let deserialized: Connection = toml::from_str(&serialized).unwrap();
        println!("deserialized: {:?}", deserialized);
        assert_eq!(conn.auth_method, deserialized.auth_method);
    }

    #[test]
    fn test_serialize_deserialize_connection_password() {
        let conn = Connection::new(
            "test".to_string(),
            22,
            "root".to_string(),
            AuthMethod::Password("password".to_string()),
        );
        let serialized = toml::to_string(&conn).unwrap();
        println!("serialized: {}", serialized);

        let deserialized: Connection = toml::from_str(&serialized).unwrap();
        println!("deserialized: {:?}", deserialized);
        assert_eq!(conn.auth_method, deserialized.auth_method);
    }

    #[test]
    fn test_serialize_config() {
        let conn = Connection::new(
            "test".to_string(),
            22,
            "root".to_string(),
            AuthMethod::PublicKey {
                private_key_path: "path".to_string(),
                passphrase: None,
            },
        );
        let conn1 = Connection::new(
            "test1".to_string(),
            23,
            "root".to_string(),
            AuthMethod::Password("password".to_string()),
        );
        let config = Config {
            connections: vec![conn, conn1],
            port_forwards: vec![],
            settings: AppSettings::default(),
        };
        let serialized = toml::to_string(&config).unwrap();
        println!("serialized: {}", serialized);
    }
}
