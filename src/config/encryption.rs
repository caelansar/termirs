//! Password encryption utilities
//! Implements AES-256-GCM encryption for password storage

use crate::error::{AppError, Result};
use ring::aead::{AES_256_GCM, Aad, LessSafeKey, NONCE_LEN, Nonce, UnboundKey};
use ring::pbkdf2::{PBKDF2_HMAC_SHA256, derive};
use ring::rand::{SecureRandom, SystemRandom};
use std::num::NonZeroU32;

const SALT_LEN: usize = 16;
const KEY_LEN: usize = 32;
const PBKDF2_ITERATIONS: u32 = 100_000;

/// Encryption utilities for password storage
pub struct PasswordEncryption {
    rng: SystemRandom,
}

impl PasswordEncryption {
    /// Create a new password encryption instance
    pub fn new() -> Self {
        Self {
            rng: SystemRandom::new(),
        }
    }

    /// Derive encryption key from system-specific information
    fn derive_key(&self, salt: &[u8]) -> Result<[u8; KEY_LEN]> {
        let mut key = [0u8; KEY_LEN];

        // Use system-specific information for key derivation
        let system_info = self.get_system_info()?;
        let iterations = NonZeroU32::new(PBKDF2_ITERATIONS)
            .ok_or_else(|| AppError::EncryptionError("Invalid iteration count".to_string()))?;

        derive(
            PBKDF2_HMAC_SHA256,
            iterations,
            salt,
            system_info.as_bytes(),
            &mut key,
        );

        Ok(key)
    }

    /// Get system-specific information for key derivation
    fn get_system_info(&self) -> Result<String> {
        // Use a combination of system information for key derivation
        // This is a simple approach - in production, you might want to use
        // more sophisticated system fingerprinting
        let hostname = std::env::var("HOSTNAME")
            .or_else(|_| std::env::var("COMPUTERNAME"))
            .unwrap_or_else(|_| "default_host".to_string());

        let username = std::env::var("USER")
            .or_else(|_| std::env::var("USERNAME"))
            .unwrap_or_else(|_| "default_user".to_string());

        Ok(format!("termirs_{}_{}", hostname, username))
    }

    /// Encrypt a password using AES-256-GCM
    pub fn encrypt_password(&self, password: &str) -> Result<String> {
        // Generate random salt
        let mut salt = [0u8; SALT_LEN];
        self.rng
            .fill(&mut salt)
            .map_err(|_| AppError::EncryptionError("Failed to generate salt".to_string()))?;

        // Generate random nonce
        let mut nonce_bytes = [0u8; NONCE_LEN];
        self.rng
            .fill(&mut nonce_bytes)
            .map_err(|_| AppError::EncryptionError("Failed to generate nonce".to_string()))?;

        // Derive key from salt
        let key_bytes = self.derive_key(&salt)?;

        // Create encryption key
        let unbound_key = UnboundKey::new(&AES_256_GCM, &key_bytes).map_err(|_| {
            AppError::EncryptionError("Failed to create encryption key".to_string())
        })?;
        let key = LessSafeKey::new(unbound_key);

        // Create nonce
        let nonce = Nonce::assume_unique_for_key(nonce_bytes);

        // Encrypt password
        let mut password_bytes = password.as_bytes().to_vec();
        key.seal_in_place_append_tag(nonce, Aad::empty(), &mut password_bytes)
            .map_err(|_| AppError::EncryptionError("Failed to encrypt password".to_string()))?;

        // Combine salt + nonce + encrypted_data and encode as base64
        let mut result = Vec::new();
        result.extend_from_slice(&salt);
        result.extend_from_slice(&nonce_bytes);
        result.extend_from_slice(&password_bytes);

        use base64::{Engine as _, engine::general_purpose};
        Ok(general_purpose::STANDARD.encode(&result))
    }

    /// Decrypt a password using AES-256-GCM
    pub fn decrypt_password(&self, encrypted_password: &str) -> Result<String> {
        // Decode from base64
        use base64::{Engine as _, engine::general_purpose};
        let encrypted_data = general_purpose::STANDARD
            .decode(encrypted_password)
            .map_err(|_| AppError::EncryptionError("Invalid base64 encoding".to_string()))?;

        // Check minimum length (salt + nonce + at least some encrypted data)
        if encrypted_data.len() < SALT_LEN + NONCE_LEN + 16 {
            return Err(AppError::EncryptionError(
                "Invalid encrypted data length".to_string(),
            ));
        }

        // Extract salt, nonce, and encrypted password
        let salt = &encrypted_data[0..SALT_LEN];
        let nonce_bytes = &encrypted_data[SALT_LEN..SALT_LEN + NONCE_LEN];
        let mut encrypted_password_bytes = encrypted_data[SALT_LEN + NONCE_LEN..].to_vec();

        // Derive key from salt
        let key_bytes = self.derive_key(salt)?;

        // Create decryption key
        let unbound_key = UnboundKey::new(&AES_256_GCM, &key_bytes).map_err(|_| {
            AppError::EncryptionError("Failed to create decryption key".to_string())
        })?;
        let key = LessSafeKey::new(unbound_key);

        // Create nonce
        let nonce_array: [u8; NONCE_LEN] = nonce_bytes
            .try_into()
            .map_err(|_| AppError::EncryptionError("Invalid nonce length".to_string()))?;
        let nonce = Nonce::assume_unique_for_key(nonce_array);

        // Decrypt password
        let decrypted_bytes = key
            .open_in_place(nonce, Aad::empty(), &mut encrypted_password_bytes)
            .map_err(|_| AppError::EncryptionError("Failed to decrypt password".to_string()))?;

        // Convert to string
        String::from_utf8(decrypted_bytes.to_vec()).map_err(|_| {
            AppError::EncryptionError("Invalid UTF-8 in decrypted password".to_string())
        })
    }
}

impl Default for PasswordEncryption {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_encrypt_decrypt_password() {
        let encryption = PasswordEncryption::new();
        let original_password = "test_password_123";

        // Encrypt the password
        let encrypted = encryption
            .encrypt_password(original_password)
            .expect("Failed to encrypt password");

        // Verify encrypted password is different from original
        assert_ne!(encrypted, original_password);
        assert!(!encrypted.is_empty());

        // Decrypt the password
        let decrypted = encryption
            .decrypt_password(&encrypted)
            .expect("Failed to decrypt password");

        // Verify decrypted password matches original
        assert_eq!(decrypted, original_password);
    }

    #[test]
    fn test_encrypt_decrypt_empty_password() {
        let encryption = PasswordEncryption::new();
        let original_password = "";

        let encrypted = encryption
            .encrypt_password(original_password)
            .expect("Failed to encrypt empty password");
        let decrypted = encryption
            .decrypt_password(&encrypted)
            .expect("Failed to decrypt empty password");

        assert_eq!(decrypted, original_password);
    }

    #[test]
    fn test_encrypt_decrypt_unicode_password() {
        let encryption = PasswordEncryption::new();
        let original_password = "пароль_测试_🔐";

        let encrypted = encryption
            .encrypt_password(original_password)
            .expect("Failed to encrypt unicode password");
        let decrypted = encryption
            .decrypt_password(&encrypted)
            .expect("Failed to decrypt unicode password");

        assert_eq!(decrypted, original_password);
    }

    #[test]
    fn test_different_encryptions_produce_different_results() {
        let encryption = PasswordEncryption::new();
        let password = "same_password";

        let encrypted1 = encryption
            .encrypt_password(password)
            .expect("Failed to encrypt password first time");
        let encrypted2 = encryption
            .encrypt_password(password)
            .expect("Failed to encrypt password second time");

        // Different encryptions should produce different results due to random salt/nonce
        assert_ne!(encrypted1, encrypted2);

        // But both should decrypt to the same original password
        let decrypted1 = encryption
            .decrypt_password(&encrypted1)
            .expect("Failed to decrypt first encryption");
        let decrypted2 = encryption
            .decrypt_password(&encrypted2)
            .expect("Failed to decrypt second encryption");

        assert_eq!(decrypted1, password);
        assert_eq!(decrypted2, password);
    }

    #[test]
    fn test_decrypt_invalid_base64() {
        let encryption = PasswordEncryption::new();
        let invalid_base64 = "not_valid_base64!@#";

        let result = encryption.decrypt_password(invalid_base64);
        assert!(result.is_err());

        if let Err(AppError::EncryptionError(msg)) = result {
            assert!(msg.contains("Invalid base64 encoding"));
        } else {
            panic!("Expected EncryptionError with base64 message");
        }
    }

    #[test]
    fn test_decrypt_too_short_data() {
        let encryption = PasswordEncryption::new();
        // Create base64 data that's too short (less than salt + nonce + minimum encrypted data)
        use base64::{Engine as _, engine::general_purpose};
        let short_data = general_purpose::STANDARD.encode(&[1, 2, 3, 4, 5]);

        let result = encryption.decrypt_password(&short_data);
        assert!(result.is_err());

        if let Err(AppError::EncryptionError(msg)) = result {
            assert!(msg.contains("Invalid encrypted data length"));
        } else {
            panic!("Expected EncryptionError with length message");
        }
    }

    #[test]
    fn test_decrypt_corrupted_data() {
        let encryption = PasswordEncryption::new();
        let password = "test_password";

        // First encrypt a valid password
        let encrypted = encryption
            .encrypt_password(password)
            .expect("Failed to encrypt password");

        // Corrupt the encrypted data by changing some bytes
        use base64::{Engine as _, engine::general_purpose};
        let mut corrupted_data = general_purpose::STANDARD.decode(&encrypted).unwrap();
        let len = corrupted_data.len();
        corrupted_data[len - 1] ^= 0xFF; // Flip bits in last byte
        let corrupted_encrypted = general_purpose::STANDARD.encode(&corrupted_data);

        // Try to decrypt corrupted data
        let result = encryption.decrypt_password(&corrupted_encrypted);
        assert!(result.is_err());

        if let Err(AppError::EncryptionError(msg)) = result {
            assert!(msg.contains("Failed to decrypt password"));
        } else {
            panic!("Expected EncryptionError with decrypt failure message");
        }
    }

    #[test]
    fn test_system_info_generation() {
        let encryption = PasswordEncryption::new();
        let system_info = encryption
            .get_system_info()
            .expect("Failed to get system info");

        // System info should not be empty and should contain the prefix
        assert!(!system_info.is_empty());
        assert!(system_info.starts_with("termirs_"));
    }

    #[test]
    fn test_key_derivation_consistency() {
        let encryption = PasswordEncryption::new();
        let salt = [1u8; SALT_LEN];

        // Derive key twice with same salt
        let key1 = encryption
            .derive_key(&salt)
            .expect("Failed to derive key first time");
        let key2 = encryption
            .derive_key(&salt)
            .expect("Failed to derive key second time");

        // Keys should be identical for same salt
        assert_eq!(key1, key2);
    }

    #[test]
    fn test_key_derivation_different_salts() {
        let encryption = PasswordEncryption::new();
        let salt1 = [1u8; SALT_LEN];
        let salt2 = [2u8; SALT_LEN];

        let key1 = encryption
            .derive_key(&salt1)
            .expect("Failed to derive key with salt1");
        let key2 = encryption
            .derive_key(&salt2)
            .expect("Failed to derive key with salt2");

        // Keys should be different for different salts
        assert_ne!(key1, key2);
    }
}
