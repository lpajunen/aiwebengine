//! The cryptography a solution should not be writing for itself.
//!
//! The engine tells scripts to verify their own webhook signatures — Telegram
//! signs a delivery with a shared secret it echoes in a header, Slack and
//! GitHub sign the body with HMAC — and until now it handed them nothing to do
//! it with. No HMAC, no constant-time comparison, no source of randomness. So
//! every solution that followed the documentation wrote the same three
//! mistakes: it fetched the secret into JavaScript, compared it with `===`,
//! and invented its own webhook secret by typing one.
//!
//! Each of those is the [`crate::repository`] `personalStorage` argument in a
//! different costume. The engine owns the key, so the script cannot get it
//! wrong:
//!
//! - **The secret is named, not passed.** [`verify_hmac`] and the
//!   `secretEquals` binding above it take the *name* of a secret and resolve
//!   it host-side, exactly as `fetch` resolves `{{secret:...}}`. A script
//!   cannot leak a value it is never given, which is the property
//!   `secretStorage` has had since it shipped and which a hand-rolled
//!   comparison threw away — the agent's own Telegram webhook keeps its shared
//!   secret in `scriptStorage` rather than `secretStorage` for exactly this
//!   reason, "because checking it means comparing it".
//!
//! - **The comparison is constant-time.** `===` on a secret leaks it a byte at
//!   a time to anyone who can time the endpoint. This is the one mistake that
//!   looks like working code forever.
//!
//! - **The randomness is the engine's.** A webhook secret somebody typed is a
//!   webhook secret somebody can guess.
//!
//! There is deliberately no signing half. Verification answers a question
//! about something that arrived; signing produces a credential, and the
//! outbound cases the engine has (a bearer token, a key in a path) are already
//! served by `{{secret:...}}` without the script holding anything. When an API
//! that wants a signed request turns up, that is the moment to design it.

use base64::Engine as _;
use hmac::{Hmac, KeyInit, Mac};
use sha1::Sha1;
use sha2::{Sha256, Sha512};
use subtle::ConstantTimeEq;

/// Shortest token [`random_token`] will mint, in bytes.
///
/// 128 bits. A script asking for fewer has misunderstood what the value is
/// for, and the refusal is cheaper than the webhook secret nobody can rotate
/// once it is deployed.
pub const MIN_TOKEN_BYTES: usize = 16;

/// Longest token [`random_token`] will mint, in bytes.
///
/// 512 bits, which is longer than any shared secret has cause to be. The cap
/// exists so that a mistyped length is an error rather than an allocation.
pub const MAX_TOKEN_BYTES: usize = 64;

/// Which hash an HMAC is built on.
///
/// `Sha1` is here because webhooks still send it — GitHub's original
/// `X-Hub-Signature` is HMAC-SHA1 — and a verifier that cannot speak it simply
/// cannot verify those deliveries. It is not a choice to make for something
/// new, and the type declarations say so.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Digest {
    Sha1,
    Sha256,
    Sha512,
}

impl Digest {
    /// The name this is asked for by, from JavaScript. Snake-free lower case,
    /// as every other vocabulary crossing that boundary is.
    pub fn as_str(self) -> &'static str {
        match self {
            Digest::Sha1 => "sha1",
            Digest::Sha256 => "sha256",
            Digest::Sha512 => "sha512",
        }
    }

    /// An unknown name is refused rather than defaulted.
    ///
    /// The rule [`crate::security::Capability::parse`] states: a caller who
    /// asked for `sha-256` and silently got `sha256` learned nothing, and one
    /// who asked for `md5` and silently got `sha256` would be told their
    /// signatures verify when they are checking the wrong thing entirely.
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "sha1" => Some(Digest::Sha1),
            "sha256" => Some(Digest::Sha256),
            "sha512" => Some(Digest::Sha512),
            _ => None,
        }
    }

    pub fn all() -> [Digest; 3] {
        [Digest::Sha1, Digest::Sha256, Digest::Sha512]
    }
}

/// How a signature is written down on the wire.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Encoding {
    #[default]
    Hex,
    Base64,
}

impl Encoding {
    pub fn as_str(self) -> &'static str {
        match self {
            Encoding::Hex => "hex",
            Encoding::Base64 => "base64",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "hex" => Some(Encoding::Hex),
            "base64" => Some(Encoding::Base64),
            _ => None,
        }
    }

    /// Read a signature written this way.
    ///
    /// `None` for anything that is not, which a caller turns into "did not
    /// verify" rather than into an error: the signature came from whoever sent
    /// the request, so a malformed one is a failed delivery and not a bug in
    /// the script checking it.
    fn decode(self, value: &str) -> Option<Vec<u8>> {
        let value = value.trim();
        match self {
            Encoding::Hex => hex::decode(value).ok(),
            // Both alphabets, because a sender chooses and the receiver does
            // not. Standard first, since it is what every webhook using base64
            // emits; URL-safe as a fallback rather than a second option for
            // the script to get wrong.
            Encoding::Base64 => base64::engine::general_purpose::STANDARD
                .decode(value)
                .or_else(|_| base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(value))
                .ok(),
        }
    }

    fn encode(self, bytes: &[u8]) -> String {
        match self {
            Encoding::Hex => hex::encode(bytes),
            Encoding::Base64 => base64::engine::general_purpose::STANDARD.encode(bytes),
        }
    }
}

/// Compare two byte strings without telling the caller where they differ.
///
/// Length is not secret — it is visible in the encoding of anything that
/// carries one — so returning early on a length mismatch leaks nothing the
/// wire did not. What must not leak is the position of the first differing
/// byte, which `subtle` is what keeps quiet.
pub fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.ct_eq(b).into()
}

/// The MAC of `message` under `key`.
fn mac(digest: Digest, key: &[u8], message: &[u8]) -> Vec<u8> {
    // `new_from_slice` fails only for a key length the algorithm refuses, and
    // HMAC accepts any: a key shorter than the block size is zero-padded and a
    // longer one is hashed. So there is no error case to propagate, which is
    // the same reasoning `security::csrf` records at its own call.
    match digest {
        Digest::Sha1 => {
            let mut mac = <Hmac<Sha1> as KeyInit>::new_from_slice(key)
                .expect("HMAC accepts a key of any length");
            mac.update(message);
            mac.finalize().into_bytes().to_vec()
        }
        Digest::Sha256 => {
            let mut mac = <Hmac<Sha256> as KeyInit>::new_from_slice(key)
                .expect("HMAC accepts a key of any length");
            mac.update(message);
            mac.finalize().into_bytes().to_vec()
        }
        Digest::Sha512 => {
            let mut mac = <Hmac<Sha512> as KeyInit>::new_from_slice(key)
                .expect("HMAC accepts a key of any length");
            mac.update(message);
            mac.finalize().into_bytes().to_vec()
        }
    }
}

/// Whether `signature` is the MAC of `message` under `key`.
///
/// Every way of answering no answers the same way. A signature that is not
/// valid hex, one of the right shape over the wrong bytes, and one of the
/// wrong length are all simply `false`, because they are all the same event
/// from the script's side — something arrived that this key did not sign — and
/// distinguishing them in the return value would put a decoding oracle where a
/// yes-or-no belongs.
pub fn verify_hmac(
    digest: Digest,
    key: &[u8],
    message: &[u8],
    signature: &str,
    encoding: Encoding,
) -> bool {
    let Some(offered) = encoding.decode(signature) else {
        return false;
    };
    constant_time_eq(&mac(digest, key, message), &offered)
}

/// The MAC as the wire writes it. For tests and for nothing else yet — see the
/// note about signing in this module's documentation.
pub fn hmac_encoded(digest: Digest, key: &[u8], message: &[u8], encoding: Encoding) -> String {
    encoding.encode(&mac(digest, key, message))
}

/// Why a token could not be minted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TokenRefusal {
    TooShort { asked: usize },
    TooLong { asked: usize },
}

impl std::fmt::Display for TokenRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TokenRefusal::TooShort { asked } => write!(
                f,
                "a token of {} bytes is guessable; ask for at least {}",
                asked, MIN_TOKEN_BYTES
            ),
            TokenRefusal::TooLong { asked } => write!(
                f,
                "a token of {} bytes is longer than any shared secret needs; the most is {}",
                asked, MAX_TOKEN_BYTES
            ),
        }
    }
}

/// A fresh random token of `bytes` bytes, written in `encoding`.
///
/// Refused outside [`MIN_TOKEN_BYTES`]..=[`MAX_TOKEN_BYTES`] rather than
/// clamped. Clamping is right for [`crate::script_limits`], where a stored
/// value takes effect without a restart and a mistyped one would hold a slot;
/// it is wrong here, because the caller would go on believing it had asked for
/// something it did not get, and the thing it did not get is the entropy.
pub fn random_token(bytes: usize, encoding: Encoding) -> Result<String, TokenRefusal> {
    if bytes < MIN_TOKEN_BYTES {
        return Err(TokenRefusal::TooShort { asked: bytes });
    }
    if bytes > MAX_TOKEN_BYTES {
        return Err(TokenRefusal::TooLong { asked: bytes });
    }

    let raw: Vec<u8> = std::iter::repeat_with(rand::random::<u8>)
        .take(bytes)
        .collect();
    Ok(encoding.encode(&raw))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RFC 4231 case 2, which `security::csrf` also pins. A wrong HMAC that
    /// agrees with itself verifies everything and protects nothing, so the
    /// implementation is checked against the standard's own vector rather than
    /// against its own output.
    #[test]
    fn hmac_sha256_matches_rfc_4231() {
        assert_eq!(
            hmac_encoded(
                Digest::Sha256,
                b"Jefe",
                b"what do ya want for nothing?",
                Encoding::Hex
            ),
            "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843"
        );
    }

    /// RFC 2202 case 2, the same message and key one algorithm down.
    #[test]
    fn hmac_sha1_matches_rfc_2202() {
        assert_eq!(
            hmac_encoded(
                Digest::Sha1,
                b"Jefe",
                b"what do ya want for nothing?",
                Encoding::Hex
            ),
            "effcdf6ae5eb2fa2d27416d5f184df9c259a7c79"
        );
    }

    /// RFC 4231 case 2 again, at 512.
    #[test]
    fn hmac_sha512_matches_rfc_4231() {
        assert_eq!(
            hmac_encoded(
                Digest::Sha512,
                b"Jefe",
                b"what do ya want for nothing?",
                Encoding::Hex
            ),
            "164b7a7bfcf819e2e395fbe73b56e0a387bd64222e831fd610270cd7ea250554\
             9758bf75c05a994a6d034f65f8f0e6fdcaeab1a34d4a6b4b636e070a38bce737"
        );
    }

    /// A signature this key did not produce does not verify, and one it did
    /// does. The whole of what the function is for.
    #[test]
    fn a_signature_verifies_only_under_the_key_that_made_it() {
        let message = b"payload";
        let good = hmac_encoded(Digest::Sha256, b"key", message, Encoding::Hex);

        assert!(verify_hmac(
            Digest::Sha256,
            b"key",
            message,
            &good,
            Encoding::Hex
        ));
        assert!(!verify_hmac(
            Digest::Sha256,
            b"another key",
            message,
            &good,
            Encoding::Hex
        ));
        assert!(!verify_hmac(
            Digest::Sha256,
            b"key",
            b"a different payload",
            &good,
            Encoding::Hex
        ));
    }

    /// The same MAC written the other way round still verifies, and only under
    /// the encoding it was written in.
    #[test]
    fn a_signature_is_read_in_the_encoding_it_was_written_in() {
        let signature = hmac_encoded(Digest::Sha256, b"key", b"payload", Encoding::Base64);

        assert!(verify_hmac(
            Digest::Sha256,
            b"key",
            b"payload",
            &signature,
            Encoding::Base64
        ));
        assert!(!verify_hmac(
            Digest::Sha256,
            b"key",
            b"payload",
            &signature,
            Encoding::Hex
        ));
    }

    /// A sender that writes base64 the URL-safe way is still understood. The
    /// receiver does not choose, so it accepts both rather than making the
    /// script pick.
    #[test]
    fn url_safe_base64_is_understood_too() {
        let raw = mac(Digest::Sha256, b"key", b"payload");
        let url_safe = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(&raw);

        assert!(verify_hmac(
            Digest::Sha256,
            b"key",
            b"payload",
            &url_safe,
            Encoding::Base64
        ));
    }

    /// Anything that is not a signature is a failed verification, not an
    /// error: it came from whoever sent the request.
    #[test]
    fn a_malformed_signature_simply_does_not_verify() {
        for offered in ["", "not hex at all", "abc", "zz", "  "] {
            assert!(
                !verify_hmac(Digest::Sha256, b"key", b"payload", offered, Encoding::Hex),
                "{:?} should not verify",
                offered
            );
        }
    }

    /// Whitespace around a signature is a header that was pretty-printed, not
    /// an attack, and every webhook sender eventually emits one.
    #[test]
    fn a_signature_may_arrive_with_whitespace_around_it() {
        let signature = hmac_encoded(Digest::Sha256, b"key", b"payload", Encoding::Hex);

        assert!(verify_hmac(
            Digest::Sha256,
            b"key",
            b"payload",
            &format!("  {}\n", signature),
            Encoding::Hex
        ));
    }

    #[test]
    fn equal_and_unequal_strings_compare_as_they_should() {
        assert!(constant_time_eq(b"abc", b"abc"));
        assert!(!constant_time_eq(b"abc", b"abd"));
        assert!(!constant_time_eq(b"abc", b"abcd"));
        assert!(constant_time_eq(b"", b""));
    }

    /// Every name round-trips, so a vocabulary added to one half cannot be
    /// missing from the other.
    #[test]
    fn digest_and_encoding_names_round_trip() {
        for digest in Digest::all() {
            assert_eq!(Digest::parse(digest.as_str()), Some(digest));
        }
        for encoding in [Encoding::Hex, Encoding::Base64] {
            assert_eq!(Encoding::parse(encoding.as_str()), Some(encoding));
        }
        assert_eq!(Digest::parse("SHA256"), Some(Digest::Sha256));
        assert_eq!(Digest::parse("md5"), None);
        assert_eq!(Digest::parse("sha-256"), None);
        assert_eq!(Encoding::parse("base64url"), None);
    }

    /// A token is the length that was asked for and is not the same twice.
    #[test]
    fn a_token_is_as_long_as_asked_and_never_repeats() {
        let one = random_token(32, Encoding::Hex).expect("32 bytes is allowed");
        let two = random_token(32, Encoding::Hex).expect("32 bytes is allowed");

        assert_eq!(one.len(), 64, "32 bytes is 64 hex characters");
        assert_ne!(one, two);
    }

    /// Refused rather than quietly made safe, in both directions.
    #[test]
    fn a_token_outside_the_bounds_is_refused() {
        assert_eq!(
            random_token(8, Encoding::Hex),
            Err(TokenRefusal::TooShort { asked: 8 })
        );
        assert_eq!(
            random_token(0, Encoding::Hex),
            Err(TokenRefusal::TooShort { asked: 0 })
        );
        assert_eq!(
            random_token(1024, Encoding::Hex),
            Err(TokenRefusal::TooLong { asked: 1024 })
        );
    }
}
