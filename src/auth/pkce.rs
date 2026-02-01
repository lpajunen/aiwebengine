/// PKCE (Proof Key for Code Exchange) Implementation
///
/// Implements RFC 7636 - Proof Key for Code Exchange by OAuth Public Clients
/// Provides code_verifier generation and code_challenge calculation
use crate::auth::error::AuthError;
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use sha2::{Digest, Sha256};

/// Generate a cryptographically random code_verifier
///
/// Per RFC 7636: code_verifier = high-entropy cryptographic random STRING using the
/// unreserved characters [A-Z] / [a-z] / [0-9] / "-" / "." / "_" / "~"
/// with a minimum length of 43 characters and a maximum length of 128 characters.
///
/// # Returns
/// A random 43-128 character string suitable for use as a PKCE code_verifier
pub fn generate_code_verifier() -> String {
    // Generate length between 43 and 128
    let length = 43 + (rand::random::<u8>() % 86) as usize;

    (0..length)
        .map(|_| {
            // Use unreserved characters: A-Z, a-z, 0-9, -, ., _, ~
            let chars = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-._~";
            chars[rand::random::<u8>() as usize % chars.len()] as char
        })
        .collect()
}

/// Calculate code_challenge from code_verifier using S256 method
///
/// Per RFC 7636: code_challenge = BASE64URL(SHA256(ASCII(code_verifier)))
///
/// # Arguments
/// * `code_verifier` - The code verifier string
///
/// # Returns
/// Base64URL-encoded SHA256 hash of the code_verifier
pub fn generate_code_challenge(code_verifier: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(code_verifier.as_bytes());
    let hash = hasher.finalize();
    URL_SAFE_NO_PAD.encode(hash)
}

/// PKCE pair containing both verifier and challenge
#[derive(Debug, Clone)]
pub struct PkcePair {
    /// The code verifier (keep secret, send to token endpoint)
    pub code_verifier: String,

    /// The code challenge (send to authorization endpoint)
    pub code_challenge: String,
}

impl PkcePair {
    /// Generate a new PKCE verifier/challenge pair
    pub fn generate() -> Self {
        let code_verifier = generate_code_verifier();
        let code_challenge = generate_code_challenge(&code_verifier);

        Self {
            code_verifier,
            code_challenge,
        }
    }

    /// Verify that a code_verifier matches this challenge
    pub fn verify(&self, verifier: &str) -> Result<(), AuthError> {
        let challenge = generate_code_challenge(verifier);
        if challenge == self.code_challenge {
            Ok(())
        } else {
            Err(AuthError::InvalidState) // PKCE verification failed
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_code_verifier_generation() {
        let verifier = generate_code_verifier();
        assert!(verifier.len() >= 43);
        assert!(verifier.len() <= 128);

        // Verify it only contains allowed characters
        for c in verifier.chars() {
            assert!(
                c.is_ascii_alphanumeric() || c == '-' || c == '.' || c == '_' || c == '~',
                "Invalid character in code_verifier: {}",
                c
            );
        }
    }

    #[test]
    fn test_code_challenge_generation() {
        let verifier = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
        let challenge = generate_code_challenge(verifier);

        // Expected value from RFC 7636 example
        assert_eq!(challenge, "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM");
    }

    #[test]
    fn test_pkce_pair_verify() {
        let pair = PkcePair::generate();

        // Should verify successfully with correct verifier
        assert!(pair.verify(&pair.code_verifier).is_ok());

        // Should fail with wrong verifier
        assert!(pair.verify("wrong_verifier").is_err());
    }

    #[test]
    fn test_pkce_pair_deterministic() {
        let verifier = "test_verifier_12345";
        let challenge1 = generate_code_challenge(verifier);
        let challenge2 = generate_code_challenge(verifier);

        // Same verifier should always produce same challenge
        assert_eq!(challenge1, challenge2);
    }
}
