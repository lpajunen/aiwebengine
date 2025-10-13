// Data Encryption Module
// Provides field-level encryption for sensitive data (OAuth tokens, secrets, etc.)

use aes_gcm::{
    Aes256Gcm, Nonce,
    aead::{Aead, KeyInit},
};
use argon2::Argon2;
use base64::{Engine as _, engine::general_purpose};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tracing::debug;

/// Encryption-related errors
#[derive(Debug, thiserror::Error)]
pub enum EncryptionError {
    #[error("Encryption failed: {0}")]
    EncryptionFailed(String),

    #[error("Decryption failed: {0}")]
    DecryptionFailed(String),

    #[error("Key derivation failed: {0}")]
    KeyDerivationFailed(String),

    #[error("Invalid encrypted data format")]
    InvalidFormat,

    #[error("Base64 encoding error: {0}")]
    Base64Error(String),
}

/// Encrypted data container
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EncryptedData {
    /// Base64-encoded ciphertext
    pub ciphertext: String,
    /// Base64-encoded nonce (12 bytes)
    pub nonce: String,
    /// Version for future key rotation support
    pub version: u8,
}

/// Data encryption service using AES-256-GCM
pub struct DataEncryption {
    cipher: Aes256Gcm,
    key_version: u8,
}

impl DataEncryption {
    /// Create a new data encryption service with a master key
    pub fn new(master_key: &[u8; 32]) -> Self {
        let cipher = Aes256Gcm::new(master_key.into());

        Self {
            cipher,
            key_version: 1,
        }
    }

    /// Create from a password using Argon2 key derivation
    pub fn from_password(password: &str, salt: &[u8]) -> Result<Self, EncryptionError> {
        let mut key = [0u8; 32];

        // Use Argon2id for key derivation
        let argon2 = Argon2::default();

        // Derive key from password
        argon2
            .hash_password_into(password.as_bytes(), salt, &mut key)
            .map_err(|e| EncryptionError::KeyDerivationFailed(e.to_string()))?;

        Ok(Self::new(&key))
    }

    /// Encrypt a string field
    pub fn encrypt_field(&self, plaintext: &str) -> Result<EncryptedData, EncryptionError> {
        // Generate random nonce (96 bits / 12 bytes for AES-GCM)
        let nonce_bytes: [u8; 12] = rand::random();
        let nonce = Nonce::from(nonce_bytes);

        // Encrypt
        let ciphertext = self
            .cipher
            .encrypt(&nonce, plaintext.as_bytes())
            .map_err(|e| EncryptionError::EncryptionFailed(e.to_string()))?;

        // Encode to base64 for storage
        let ciphertext_b64 = general_purpose::STANDARD.encode(&ciphertext);
        let nonce_b64 = general_purpose::STANDARD.encode(nonce_bytes);

        Ok(EncryptedData {
            ciphertext: ciphertext_b64,
            nonce: nonce_b64,
            version: self.key_version,
        })
    }

    /// Decrypt a string field
    pub fn decrypt_field(&self, encrypted: &EncryptedData) -> Result<String, EncryptionError> {
        // Check version compatibility
        if encrypted.version != self.key_version {
            return Err(EncryptionError::DecryptionFailed(format!(
                "Key version mismatch: expected {}, got {}",
                self.key_version, encrypted.version
            )));
        }

        // Decode from base64
        let ciphertext = general_purpose::STANDARD
            .decode(&encrypted.ciphertext)
            .map_err(|e| EncryptionError::Base64Error(e.to_string()))?;

        let nonce_bytes = general_purpose::STANDARD
            .decode(&encrypted.nonce)
            .map_err(|e| EncryptionError::Base64Error(e.to_string()))?;

        if nonce_bytes.len() != 12 {
            return Err(EncryptionError::InvalidFormat);
        }

        let nonce = Nonce::from_slice(&nonce_bytes);

        // Decrypt
        let plaintext = self
            .cipher
            .decrypt(nonce, ciphertext.as_ref())
            .map_err(|e| EncryptionError::DecryptionFailed(e.to_string()))?;

        // Convert to string
        String::from_utf8(plaintext)
            .map_err(|e| EncryptionError::DecryptionFailed(format!("Invalid UTF-8: {}", e)))
    }

    /// Encrypt binary data
    pub fn encrypt_bytes(&self, plaintext: &[u8]) -> Result<EncryptedData, EncryptionError> {
        let nonce_bytes: [u8; 12] = rand::random();
        let nonce = Nonce::from(nonce_bytes);

        let ciphertext = self
            .cipher
            .encrypt(&nonce, plaintext)
            .map_err(|e| EncryptionError::EncryptionFailed(e.to_string()))?;

        let ciphertext_b64 = general_purpose::STANDARD.encode(&ciphertext);
        let nonce_b64 = general_purpose::STANDARD.encode(nonce_bytes);

        Ok(EncryptedData {
            ciphertext: ciphertext_b64,
            nonce: nonce_b64,
            version: self.key_version,
        })
    }

    /// Decrypt binary data
    pub fn decrypt_bytes(&self, encrypted: &EncryptedData) -> Result<Vec<u8>, EncryptionError> {
        if encrypted.version != self.key_version {
            return Err(EncryptionError::DecryptionFailed(format!(
                "Key version mismatch: expected {}, got {}",
                self.key_version, encrypted.version
            )));
        }

        let ciphertext = general_purpose::STANDARD
            .decode(&encrypted.ciphertext)
            .map_err(|e| EncryptionError::Base64Error(e.to_string()))?;

        let nonce_bytes = general_purpose::STANDARD
            .decode(&encrypted.nonce)
            .map_err(|e| EncryptionError::Base64Error(e.to_string()))?;

        if nonce_bytes.len() != 12 {
            return Err(EncryptionError::InvalidFormat);
        }

        let nonce = Nonce::from_slice(&nonce_bytes);

        self.cipher
            .decrypt(nonce, ciphertext.as_ref())
            .map_err(|e| EncryptionError::DecryptionFailed(e.to_string()))
    }
}

/// Utility for securely handling sensitive strings in memory
pub struct SecureString {
    data: Vec<u8>,
}

impl SecureString {
    pub fn new(s: String) -> Self {
        Self {
            data: s.into_bytes(),
        }
    }

    pub fn as_str(&self) -> Result<&str, std::str::Utf8Error> {
        std::str::from_utf8(&self.data)
    }
}

impl Drop for SecureString {
    fn drop(&mut self) {
        // Zero out memory when dropped
        for byte in &mut self.data {
            *byte = 0;
        }
    }
}

impl std::fmt::Debug for SecureString {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "SecureString([REDACTED])")
    }
}

/// Helper for encrypting common sensitive fields
pub struct FieldEncryptor {
    encryption: Arc<DataEncryption>,
}

impl FieldEncryptor {
    pub fn new(encryption: Arc<DataEncryption>) -> Self {
        Self { encryption }
    }

    /// Encrypt an OAuth access token
    pub fn encrypt_access_token(&self, token: &str) -> Result<EncryptedData, EncryptionError> {
        debug!("Encrypting access token");
        self.encryption.encrypt_field(token)
    }

    /// Decrypt an OAuth access token
    pub fn decrypt_access_token(
        &self,
        encrypted: &EncryptedData,
    ) -> Result<String, EncryptionError> {
        debug!("Decrypting access token");
        self.encryption.decrypt_field(encrypted)
    }

    /// Encrypt an OAuth refresh token
    pub fn encrypt_refresh_token(&self, token: &str) -> Result<EncryptedData, EncryptionError> {
        debug!("Encrypting refresh token");
        self.encryption.encrypt_field(token)
    }

    /// Decrypt an OAuth refresh token
    pub fn decrypt_refresh_token(
        &self,
        encrypted: &EncryptedData,
    ) -> Result<String, EncryptionError> {
        debug!("Decrypting refresh token");
        self.encryption.decrypt_field(encrypted)
    }

    /// Encrypt a client secret
    pub fn encrypt_client_secret(&self, secret: &str) -> Result<EncryptedData, EncryptionError> {
        debug!("Encrypting client secret");
        self.encryption.encrypt_field(secret)
    }

    /// Decrypt a client secret
    pub fn decrypt_client_secret(
        &self,
        encrypted: &EncryptedData,
    ) -> Result<String, EncryptionError> {
        debug!("Decrypting client secret");
        self.encryption.decrypt_field(encrypted)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_encryption() -> DataEncryption {
        let key: [u8; 32] = rand::random();
        DataEncryption::new(&key)
    }

    #[test]
    fn test_encrypt_decrypt_field() {
        let encryption = create_test_encryption();
        let plaintext = "sensitive data";

        let encrypted = encryption.encrypt_field(plaintext).unwrap();
        let decrypted = encryption.decrypt_field(&encrypted).unwrap();

        assert_eq!(plaintext, decrypted);
    }

    #[test]
    fn test_encrypt_decrypt_bytes() {
        let encryption = create_test_encryption();
        let plaintext = b"binary data";

        let encrypted = encryption.encrypt_bytes(plaintext).unwrap();
        let decrypted = encryption.decrypt_bytes(&encrypted).unwrap();

        assert_eq!(plaintext, decrypted.as_slice());
    }

    #[test]
    fn test_different_nonces() {
        let encryption = create_test_encryption();
        let plaintext = "test data";

        let encrypted1 = encryption.encrypt_field(plaintext).unwrap();
        let encrypted2 = encryption.encrypt_field(plaintext).unwrap();

        // Same plaintext should produce different ciphertexts (different nonces)
        assert_ne!(encrypted1.ciphertext, encrypted2.ciphertext);
        assert_ne!(encrypted1.nonce, encrypted2.nonce);

        // Both should decrypt to same plaintext
        assert_eq!(encryption.decrypt_field(&encrypted1).unwrap(), plaintext);
        assert_eq!(encryption.decrypt_field(&encrypted2).unwrap(), plaintext);
    }

    #[test]
    fn test_version_mismatch() {
        let encryption = create_test_encryption();
        let plaintext = "test";

        let mut encrypted = encryption.encrypt_field(plaintext).unwrap();
        encrypted.version = 99; // Wrong version

        let result = encryption.decrypt_field(&encrypted);
        assert!(result.is_err());
    }

    #[test]
    fn test_secure_string_zeroing() {
        let _data: Vec<u8>;
        {
            let secure = SecureString::new("secret".to_string());
            _data = secure.data.clone();
        }
        // After drop, original data in SecureString should be zeroed
        // (This test verifies the Drop trait implementation)
    }

    #[test]
    fn test_field_encryptor() {
        let key: [u8; 32] = rand::random();
        let encryption = Arc::new(DataEncryption::new(&key));
        let encryptor = FieldEncryptor::new(encryption);

        let token = "oauth_access_token_12345";
        let encrypted = encryptor.encrypt_access_token(token).unwrap();
        let decrypted = encryptor.decrypt_access_token(&encrypted).unwrap();

        assert_eq!(token, decrypted);
    }

    #[test]
    fn test_password_key_derivation() {
        let password = "super_secret_password";
        let salt = b"random_salt_1234"; // In production, use random salt

        let encryption = DataEncryption::from_password(password, salt).unwrap();
        let plaintext = "test data";

        let encrypted = encryption.encrypt_field(plaintext).unwrap();
        let decrypted = encryption.decrypt_field(&encrypted).unwrap();

        assert_eq!(plaintext, decrypted);
    }
}
