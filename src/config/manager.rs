use std::{
    fs,
    path::{Path, PathBuf},
};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::{AppError, Result};

/// Application settings
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct AppSettings {
    pub default_port: u16,
    pub connection_timeout: u64,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            default_port: 22,
            connection_timeout: 20,
        }
    }
}

fn serialize_password<S>(plain: &String, serializer: S) -> std::result::Result<S::Ok, S::Error>
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

/// Represents an SSH connection configuration
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Connection {
    pub id: String,
    pub display_name: String,
    pub host: String,
    pub port: u16,
    pub username: String,
    #[serde(
        alias = "encrypted_password",
        serialize_with = "serialize_password",
        deserialize_with = "deserialize_password"
    )]
    pub password: String,
    pub created_at: DateTime<Utc>,
    pub last_used: Option<DateTime<Utc>>,
}

impl Connection {
    /// Creates a new connection with the given parameters
    pub fn new(host: String, port: u16, username: String, password: String) -> Self {
        let display_name = host.clone(); // Default display name is the host
        Self {
            id: Uuid::new_v4().to_string(),
            display_name,
            host,
            port,
            username,
            password,
            created_at: Utc::now(),
            last_used: None,
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

        if self.password.trim().is_empty() {
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
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Config {
    pub connections: Vec<Connection>,
    pub settings: AppSettings,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            connections: Vec::new(),
            settings: AppSettings::default(),
        }
    }
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
        let config = Self::load_config_from_path(&config_path)?;

        Ok(Self {
            config_path,
            config,
        })
    }

    /// Create a configuration manager with a custom config path (useful for testing)
    #[allow(dead_code)]
    pub fn with_path<P: AsRef<Path>>(config_path: P) -> Result<Self> {
        let config_path = config_path.as_ref().to_path_buf();
        let config = Self::load_config_from_path(&config_path)?;

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
                AppError::ConfigError(format!("Failed to create config directory: {}", e))
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
            .map_err(|e| AppError::ConfigError(format!("Failed to read config file: {}", e)))?;

        let config: Config = toml::from_str(&config_content)
            .map_err(|e| AppError::ConfigError(format!("Failed to parse config file: {}", e)))?;

        Ok(config)
    }

    /// Persist current config to disk
    pub fn save(&self) -> Result<()> {
        let toml = toml::to_string_pretty(&self.config)
            .map_err(|e| AppError::ConfigError(format!("Failed to serialize config: {}", e)))?;
        fs::write(&self.config_path, toml)
            .map_err(|e| AppError::ConfigError(format!("Failed to write config: {}", e)))?;
        Ok(())
    }

    /// Return immutable slice of connections
    pub fn connections(&self) -> &[Connection] {
        &self.config.connections
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
        }) {
            self.config.connections.push(connection);
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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_serialize_deserialize_connection() {
        let conn = Connection::new(
            "test".to_string(),
            22,
            "root".to_string(),
            "password".to_string(),
        );
        let serialized = toml::to_string(&conn).unwrap();
        println!("serialized: {}", serialized);

        let deserialized: Connection = toml::from_str(&serialized).unwrap();
        println!("deserialized: {:?}", deserialized);
        assert_eq!(conn.password, deserialized.password);
    }
}
