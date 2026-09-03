//! Credentials the engine holds itself.
//!
//! Everything here exists so that a person can have an account on this engine
//! without handing an address to Google, Microsoft or Apple — and, for a
//! personal install, without registering an OAuth client at all.
//!
//! Two kinds of identity share the machinery:
//!
//! - a **guest** has no credential. The engine mints an identity and issues a
//!   session, so someone using a solution gets stable storage, ownership and a
//!   display name while surrendering nothing. They cannot sign in again from
//!   another browser, which is the honest cost of holding no secret.
//! - a **local account** has a username and an Argon2id password hash stored
//!   here.
//!
//! A guest becomes a local account by attaching a credential to the same
//! `user_id` — the reason the credential lives in its own table. Whatever the
//! guest accumulated is still theirs afterwards, which is the whole point:
//! without it, "sign up properly" means "start again".
//!
//! Both kinds are ordinary authenticated users. Neither can reach the editor
//! or administrator tier without an administrator granting it
//! ([`crate::security::UserContext`] describes what each tier holds).

use crate::auth::error::AuthError;
use crate::database::get_global_database;
use argon2::Argon2;
use argon2::password_hash::{
    PasswordHash, PasswordHasher, PasswordVerifier, SaltString, rand_core::OsRng,
};
use sqlx::{PgPool, Row};

/// Provider name recorded on accounts holding an engine-issued credential.
pub const LOCAL_PROVIDER: &str = "local";

/// Provider name recorded on accounts with no credential at all.
pub const GUEST_PROVIDER: &str = "guest";

/// Floor on password length, independent of configuration. Configuration can
/// demand more, never less.
pub const MIN_PASSWORD_LENGTH: usize = 8;

/// Ceiling on password length. Argon2 is happy with far more, but an unbounded
/// input is work an unauthenticated caller gets to ask for.
pub const MAX_PASSWORD_LENGTH: usize = 1024;

const MIN_USERNAME_LENGTH: usize = 3;
const MAX_USERNAME_LENGTH: usize = 32;

fn pool() -> Result<PgPool, AuthError> {
    get_global_database()
        .map(|db| db.pool().clone())
        .ok_or_else(|| AuthError::Internal("database is not initialised".to_string()))
}

/// Fold a username to the one spelling stored, looked up and made unique.
///
/// Case-insensitive on purpose: two accounts differing only in capitalisation
/// are two accounts a person cannot tell apart, which is a phishing tool rather
/// than a feature.
pub fn normalize_username(username: &str) -> String {
    username.trim().to_lowercase()
}

/// Check a username against the rules before it reaches the database.
///
/// The character set is narrow deliberately. A username is displayed next to
/// other people's names, so anything that lets one account impersonate another
/// — whitespace, direction marks, lookalike scripts — is worth more than the
/// expressiveness it costs.
pub fn validate_username(username: &str) -> Result<String, AuthError> {
    let normalized = normalize_username(username);

    if normalized.len() < MIN_USERNAME_LENGTH {
        return Err(AuthError::InvalidUsername(format!(
            "must be at least {} characters",
            MIN_USERNAME_LENGTH
        )));
    }
    if normalized.len() > MAX_USERNAME_LENGTH {
        return Err(AuthError::InvalidUsername(format!(
            "must be at most {} characters",
            MAX_USERNAME_LENGTH
        )));
    }

    let mut chars = normalized.chars();
    let first = chars.next().unwrap_or_default();
    if !first.is_ascii_alphanumeric() {
        return Err(AuthError::InvalidUsername(
            "must start with a letter or digit".to_string(),
        ));
    }
    if !normalized
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-'))
    {
        return Err(AuthError::InvalidUsername(
            "may contain only letters, digits, and _ . -".to_string(),
        ));
    }

    Ok(normalized)
}

/// Check a password against the floor and the configured minimum.
pub fn validate_password(password: &str, min_length: usize) -> Result<(), AuthError> {
    let required = min_length.max(MIN_PASSWORD_LENGTH);
    if password.chars().count() < required {
        return Err(AuthError::WeakPassword(format!(
            "must be at least {} characters",
            required
        )));
    }
    if password.len() > MAX_PASSWORD_LENGTH {
        return Err(AuthError::WeakPassword(format!(
            "must be at most {} bytes",
            MAX_PASSWORD_LENGTH
        )));
    }
    Ok(())
}

/// Hash a password for storage, as an Argon2id PHC string.
///
/// The parameters and salt travel inside the string, so raising the cost later
/// is a change to this function and not a migration.
pub fn hash_password(password: &str) -> Result<String, AuthError> {
    let salt = SaltString::generate(&mut OsRng);
    Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map(|hash| hash.to_string())
        .map_err(|e| AuthError::Internal(format!("password hashing failed: {}", e)))
}

/// Verify a password against a stored PHC string.
///
/// A hash that will not parse verifies as false rather than erroring: a
/// corrupt row must not become a way to tell one account from another.
pub fn verify_password(password: &str, stored_hash: &str) -> bool {
    match PasswordHash::new(stored_hash) {
        Ok(parsed) => Argon2::default()
            .verify_password(password.as_bytes(), &parsed)
            .is_ok(),
        Err(e) => {
            tracing::error!("stored password hash could not be parsed: {}", e);
            false
        }
    }
}

/// Whether a username is already spoken for.
pub async fn username_exists(username: &str) -> Result<bool, AuthError> {
    let normalized = normalize_username(username);
    let pool = pool()?;
    let row = sqlx::query("SELECT 1 FROM local_credentials WHERE username = $1")
        .bind(&normalized)
        .fetch_optional(&pool)
        .await
        .map_err(|e| AuthError::Internal(format!("credential lookup failed: {}", e)))?;
    Ok(row.is_some())
}

/// Whether this user already holds an engine-issued credential.
pub async fn user_has_credential(user_id: &str) -> Result<bool, AuthError> {
    let pool = pool()?;
    let row = sqlx::query("SELECT 1 FROM local_credentials WHERE user_id = $1")
        .bind(user_id)
        .fetch_optional(&pool)
        .await
        .map_err(|e| AuthError::Internal(format!("credential lookup failed: {}", e)))?;
    Ok(row.is_some())
}

/// Attach a credential to an existing user.
///
/// This is both halves of the claim path: a guest who wants to keep their
/// account, and an administrator adding a way in that does not depend on an
/// external provider being reachable. One credential per user — changing a
/// password is a separate operation, and this must not become a way to
/// overwrite one.
pub async fn attach_credential(
    user_id: &str,
    username: &str,
    password: &str,
    min_password_length: usize,
) -> Result<String, AuthError> {
    let normalized = validate_username(username)?;
    validate_password(password, min_password_length)?;

    if user_has_credential(user_id).await? {
        return Err(AuthError::CredentialAlreadySet);
    }

    let hash = hash_password(password)?;
    let pool = pool()?;

    let result = sqlx::query(
        r#"
        INSERT INTO local_credentials (user_id, username, password_hash)
        VALUES ($1, $2, $3)
        ON CONFLICT DO NOTHING
        "#,
    )
    .bind(user_id)
    .bind(&normalized)
    .bind(&hash)
    .execute(&pool)
    .await
    .map_err(|e| AuthError::Internal(format!("storing credential failed: {}", e)))?;

    // `ON CONFLICT DO NOTHING` covers both unique indexes, so a zero row count
    // means the username was taken or the user gained a credential between the
    // check above and this insert. The database is what decides, not the check.
    if result.rows_affected() == 0 {
        return Err(AuthError::UsernameTaken);
    }

    Ok(normalized)
}

/// Check a password against the one an account holds.
///
/// The authorization for anything that acts on a credential from inside a
/// session: changing the password, and issuing recovery codes. The session says
/// which account, and this says it is really them — a session someone else got
/// hold of must not be enough on its own, or the two things that can lock an
/// owner out of their own account are both reachable from a stolen cookie.
///
/// An account with no credential answers the same as a wrong password, after
/// hashing against the dummy, so this is not a way to learn which accounts have
/// one.
pub async fn verify_user_password(user_id: &str, password: &str) -> Result<(), AuthError> {
    let pool = pool()?;
    let row = sqlx::query("SELECT password_hash FROM local_credentials WHERE user_id = $1")
        .bind(user_id)
        .fetch_optional(&pool)
        .await
        .map_err(|e| AuthError::Internal(format!("credential lookup failed: {}", e)))?;

    let Some(row) = row else {
        // A guest, or a federated identity: there is no password here, and
        // treating its absence as a pass would be a way to take an account over
        // from whatever session happened to be open.
        let _ = verify_password(password, DUMMY_HASH);
        return Err(AuthError::InvalidCredentials);
    };

    let stored_hash: String = row
        .try_get("password_hash")
        .map_err(|e| AuthError::Internal(format!("credential row is malformed: {}", e)))?;

    if !verify_password(password, &stored_hash) {
        return Err(AuthError::InvalidCredentials);
    }

    Ok(())
}

/// Replace an account's password with another, given the one it has now.
///
/// The lifecycle piece a personal install cannot do without: the credential is
/// not one way in among several there, it is the only one, and a password
/// suspected of exposure had no way to be changed at all — [`attach_credential`]
/// refuses when a credential exists, by design, and nothing else wrote to the
/// table.
///
/// Takes the current password rather than trusting the session, so a session
/// someone else has hold of cannot be used to lock the owner out of their own
/// account.
pub async fn change_password(
    user_id: &str,
    current_password: &str,
    new_password: &str,
    min_password_length: usize,
) -> Result<(), AuthError> {
    validate_password(new_password, min_password_length)?;
    verify_user_password(user_id, current_password).await?;

    let pool = pool()?;
    let hash = hash_password(new_password)?;
    sqlx::query("UPDATE local_credentials SET password_hash = $1 WHERE user_id = $2")
        .bind(&hash)
        .bind(user_id)
        .execute(&pool)
        .await
        .map_err(|e| AuthError::Internal(format!("storing credential failed: {}", e)))?;

    Ok(())
}

/// Replace an account's password without presenting the one it has now.
///
/// The break-glass half of [`change_password`], and the reason a forgotten
/// password no longer means editing the database by hand. Nothing reachable
/// over HTTP calls this: the only caller is `--set-password`, which runs with
/// no server up and is authorized by holding the configuration file and the
/// database it points at — the same authority `--grant-role` and
/// `auth.bootstrap_admins` already run on.
///
/// The configured minimum still applies. An operator resetting a password is
/// not a reason to write one the sign-in page would refuse.
///
/// Ending the account's sessions is the caller's job, not this function's: a
/// password is reset because the old one may be known, and a session minted
/// under it otherwise keeps working for up to `max_session_age`.
pub async fn set_password(
    user_id: &str,
    new_password: &str,
    min_password_length: usize,
) -> Result<(), AuthError> {
    validate_password(new_password, min_password_length)?;

    let hash = hash_password(new_password)?;
    let pool = pool()?;

    let result = sqlx::query("UPDATE local_credentials SET password_hash = $1 WHERE user_id = $2")
        .bind(&hash)
        .bind(user_id)
        .execute(&pool)
        .await
        .map_err(|e| AuthError::Internal(format!("storing credential failed: {}", e)))?;

    // No row means the account holds no credential to replace. Inventing one
    // here would need a username, and choosing somebody's username on their
    // behalf is not a thing a password reset should do.
    if result.rows_affected() == 0 {
        return Err(AuthError::InvalidCredentials);
    }

    Ok(())
}

/// The account a username belongs to, if any.
///
/// Reads the credential table rather than the `users` row, because that is
/// where a username is unique: an account that claimed a name after starting
/// as a guest still carries `guest` as its provider.
pub async fn user_id_for_username(username: &str) -> Result<Option<String>, AuthError> {
    let normalized = normalize_username(username);
    let pool = pool()?;
    let row = sqlx::query("SELECT user_id FROM local_credentials WHERE username = $1")
        .bind(&normalized)
        .fetch_optional(&pool)
        .await
        .map_err(|e| AuthError::Internal(format!("credential lookup failed: {}", e)))?;

    match row {
        Some(row) => row
            .try_get::<String, _>("user_id")
            .map(Some)
            .map_err(|e| AuthError::Internal(format!("credential row is malformed: {}", e))),
        None => Ok(None),
    }
}

/// Look up the user a username and password belong to.
///
/// Returns [`AuthError::InvalidCredentials`] for an unknown username and for a
/// wrong password alike. The unknown-username branch still runs a verification
/// against a dummy hash, so the two answers take comparable time and the
/// endpoint does not become a username oracle.
pub async fn verify_login(username: &str, password: &str) -> Result<String, AuthError> {
    let normalized = normalize_username(username);
    let pool = pool()?;

    let row =
        sqlx::query("SELECT user_id, password_hash FROM local_credentials WHERE username = $1")
            .bind(&normalized)
            .fetch_optional(&pool)
            .await
            .map_err(|e| AuthError::Internal(format!("credential lookup failed: {}", e)))?;

    let Some(row) = row else {
        // Hash something anyway. Skipping the work here is what turns a login
        // endpoint into a way to enumerate accounts by timing.
        let _ = verify_password(password, DUMMY_HASH);
        return Err(AuthError::InvalidCredentials);
    };

    let user_id: String = row
        .try_get("user_id")
        .map_err(|e| AuthError::Internal(format!("credential row is malformed: {}", e)))?;
    let stored_hash: String = row
        .try_get("password_hash")
        .map_err(|e| AuthError::Internal(format!("credential row is malformed: {}", e)))?;

    if verify_password(password, &stored_hash) {
        Ok(user_id)
    } else {
        Err(AuthError::InvalidCredentials)
    }
}

/// The username attached to a user, if any. Used to show someone who they are
/// signed in as when there is no email address to show.
pub async fn username_for_user(user_id: &str) -> Result<Option<String>, AuthError> {
    let pool = pool()?;
    let row = sqlx::query("SELECT username FROM local_credentials WHERE user_id = $1")
        .bind(user_id)
        .fetch_optional(&pool)
        .await
        .map_err(|e| AuthError::Internal(format!("credential lookup failed: {}", e)))?;

    match row {
        Some(row) => row
            .try_get::<String, _>("username")
            .map(Some)
            .map_err(|e| AuthError::Internal(format!("credential row is malformed: {}", e))),
        None => Ok(None),
    }
}

/// How many codes an account is issued at once.
///
/// Ten because a set is written down once and used one at a time, and because
/// the cost of a spare is nothing while the cost of running out is the thing
/// this feature exists to prevent.
pub const RECOVERY_CODE_COUNT: usize = 10;

/// Characters a recovery code is built from.
///
/// No `i`, `l`, `o` or `u`: a code is copied onto paper and typed back months
/// later, and a character somebody has to guess the identity of is a code that
/// fails when it is needed. Lower case only, since the redeem path folds case
/// anyway and a mixed-case alphabet would only add ways to mistype.
const RECOVERY_ALPHABET: &[u8] = b"abcdefghjkmnpqrstvwxyz23456789";

/// Characters per code, before the grouping hyphens.
///
/// Twenty of a thirty-character alphabet is a little over 98 bits. It does not
/// need to be less guessable than that, and a code nobody can transcribe is
/// worse than a shorter one.
const RECOVERY_CODE_LENGTH: usize = 20;

/// Fold a code to the one spelling that is looked up.
///
/// Everything that is not part of the alphabet goes: the hyphens this engine
/// writes, the spaces a person types, the case a phone capitalised. What is
/// left is compared, so a code read off paper matches however it was entered.
pub fn normalize_recovery_code(code: &str) -> String {
    code.chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .map(|c| c.to_ascii_lowercase())
        .collect()
}

/// Hash a code for storage.
///
/// SHA-256, no salt and no work factor — the same reasoning as
/// [`crate::auth::refresh_tokens`]. This is a string the engine generated with
/// about a hundred bits of entropy, not a password somebody chose, so there is
/// nothing to guess and nothing to pre-compute; and it means redeeming a code
/// is one indexed lookup rather than an Argon2 verification against every row
/// the account holds.
fn hash_recovery_code(code: &str) -> String {
    use sha2::{Digest, Sha256};
    hex::encode(Sha256::digest(normalize_recovery_code(code).as_bytes()))
}

/// One code, in the shape it is shown to a person: four groups of five.
fn generate_recovery_code() -> String {
    let modulus = RECOVERY_ALPHABET.len() as u8;
    // The largest multiple of the alphabet that fits in a byte. Bytes at or
    // above it are drawn again rather than folded, so every character is
    // equally likely — a bias here is small, but it is free to not have one.
    let ceiling = u8::MAX - (u8::MAX % modulus);

    let mut code = String::with_capacity(RECOVERY_CODE_LENGTH + 3);
    for position in 0..RECOVERY_CODE_LENGTH {
        if position > 0 && position % 5 == 0 {
            code.push('-');
        }
        let byte = loop {
            let candidate: u8 = rand::random();
            if candidate < ceiling {
                break candidate;
            }
        };
        code.push(RECOVERY_ALPHABET[(byte % modulus) as usize] as char);
    }
    code
}

/// Issue a fresh set of recovery codes, returning the only copy of them.
///
/// Replaces whatever the account held: a set is a set, and leaving the previous
/// one working would mean a person who reissues because the old codes were seen
/// by somebody has not actually taken them away.
///
/// The account must hold a credential. A code's whole power is to set a
/// password, so issuing one to an account with no password would be issuing a
/// credential that can do nothing — and to a guest, whose account has no way in
/// by design, it would be a way in.
pub async fn issue_recovery_codes(user_id: &str) -> Result<Vec<String>, AuthError> {
    if !user_has_credential(user_id).await? {
        return Err(AuthError::InvalidCredentials);
    }

    let codes: Vec<String> = (0..RECOVERY_CODE_COUNT)
        .map(|_| generate_recovery_code())
        .collect();
    let hashes: Vec<String> = codes.iter().map(|code| hash_recovery_code(code)).collect();

    let pool = pool()?;
    let mut tx = pool
        .begin()
        .await
        .map_err(|e| AuthError::Internal(format!("storing recovery codes failed: {}", e)))?;

    sqlx::query("DELETE FROM local_recovery_codes WHERE user_id = $1")
        .bind(user_id)
        .execute(&mut *tx)
        .await
        .map_err(|e| AuthError::Internal(format!("storing recovery codes failed: {}", e)))?;

    // One statement rather than ten round trips, and inside the transaction
    // that removed the previous set, so an account is never briefly holding no
    // codes at all.
    sqlx::query(
        r#"
        INSERT INTO local_recovery_codes (code_hash, user_id)
        SELECT hash, $2 FROM UNNEST($1::text[]) AS hash
        "#,
    )
    .bind(&hashes)
    .bind(user_id)
    .execute(&mut *tx)
    .await
    .map_err(|e| AuthError::Internal(format!("storing recovery codes failed: {}", e)))?;

    tx.commit()
        .await
        .map_err(|e| AuthError::Internal(format!("storing recovery codes failed: {}", e)))?;

    Ok(codes)
}

/// How many of an account's codes are still good, for the account page to
/// report.
pub async fn unused_recovery_code_count(user_id: &str) -> Result<i64, AuthError> {
    let pool = pool()?;
    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM local_recovery_codes WHERE user_id = $1 AND used_at IS NULL",
    )
    .bind(user_id)
    .fetch_one(&pool)
    .await
    .map_err(|e| AuthError::Internal(format!("recovery code lookup failed: {}", e)))?;

    Ok(count)
}

/// Spend a recovery code and set the password it was held against.
///
/// The username is asked for as well as the code, and both have to name the
/// same account. Not because the code is ambiguous — it identifies an account
/// on its own — but because it makes the form read like a sign-in, and because
/// a code typed against the wrong account should fail rather than quietly reset
/// whichever one it belongs to.
///
/// The code is spent in the same transaction that writes the password, and
/// spending it is conditional on it still being unused, so two requests racing
/// with the same code cannot both set a password. Every other code the account
/// holds survives: a set of ten is ten emergencies, not one.
///
/// Returns the same error for an unknown username, a wrong code and a spent
/// code. There is nothing to be learned here about which accounts exist or
/// which codes were once real.
pub async fn redeem_recovery_code(
    username: &str,
    code: &str,
    new_password: &str,
    min_password_length: usize,
) -> Result<String, AuthError> {
    validate_password(new_password, min_password_length)?;

    // Hashed before the username is looked up, so an unknown name costs what a
    // known one costs. Doing it after the lookup would make the whole Argon2
    // hash the difference between "no such account" and "wrong code", which is
    // the timing oracle `verify_login` hashes against a dummy to avoid.
    let password_hash = hash_password(new_password)?;
    let hash = hash_recovery_code(code);

    let Some(user_id) = user_id_for_username(username).await? else {
        return Err(AuthError::InvalidCredentials);
    };

    let pool = pool()?;

    let mut tx = pool
        .begin()
        .await
        .map_err(|e| AuthError::Internal(format!("redeeming a recovery code failed: {}", e)))?;

    let spent = sqlx::query(
        r#"
        UPDATE local_recovery_codes
        SET used_at = NOW()
        WHERE code_hash = $1 AND user_id = $2 AND used_at IS NULL
        "#,
    )
    .bind(&hash)
    .bind(&user_id)
    .execute(&mut *tx)
    .await
    .map_err(|e| AuthError::Internal(format!("redeeming a recovery code failed: {}", e)))?;

    if spent.rows_affected() == 0 {
        return Err(AuthError::InvalidCredentials);
    }

    sqlx::query("UPDATE local_credentials SET password_hash = $1 WHERE user_id = $2")
        .bind(&password_hash)
        .bind(&user_id)
        .execute(&mut *tx)
        .await
        .map_err(|e| AuthError::Internal(format!("storing credential failed: {}", e)))?;

    tx.commit()
        .await
        .map_err(|e| AuthError::Internal(format!("redeeming a recovery code failed: {}", e)))?;

    Ok(user_id)
}

/// An Argon2id hash of a value no one holds, verified against when the username
/// is unknown so that branch costs what a real verification costs.
const DUMMY_HASH: &str = "$argon2id$v=19$m=19456,t=2,p=1$c29tZXNhbHRzb21lc2FsdA$Rk9SIFRJTUlORyBPTkxZIC0gbmV2ZXIgbWF0Y2hlcw";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn usernames_are_folded_to_one_spelling() {
        assert_eq!(normalize_username("  PlayerOne  "), "playerone");
        assert_eq!(
            validate_username("PlayerOne").expect("valid"),
            "playerone",
            "validation returns the stored spelling, not the typed one"
        );
    }

    #[test]
    fn usernames_that_could_impersonate_are_rejected() {
        for candidate in [
            "ab",                    // too short
            "a".repeat(33).as_str(), // too long
            "_leading",              // must start alphanumeric
            "with space",            // whitespace
            "emoji\u{1F600}",        // outside the allowed set
            "player\u{200E}one",     // direction mark
        ] {
            assert!(
                validate_username(candidate).is_err(),
                "{:?} should be rejected",
                candidate
            );
        }
    }

    #[test]
    fn password_floor_holds_against_a_lower_configured_minimum() {
        assert!(
            validate_password("short", 1).is_err(),
            "configuration must not be able to lower the floor"
        );
        assert!(validate_password("a-fine-password", 0).is_ok());
        assert!(
            validate_password("a-fine-password", 32).is_err(),
            "but it can raise it"
        );
    }

    #[test]
    fn a_hash_verifies_only_against_its_own_password() {
        let hash = hash_password("correct horse battery staple").expect("hashing should succeed");
        assert!(verify_password("correct horse battery staple", &hash));
        assert!(!verify_password("Correct horse battery staple", &hash));
        assert!(!verify_password("", &hash));
    }

    #[test]
    fn hashing_the_same_password_twice_gives_different_hashes() {
        let first = hash_password("same password").expect("hashing should succeed");
        let second = hash_password("same password").expect("hashing should succeed");
        assert_ne!(first, second, "each hash carries its own salt");
    }

    #[test]
    fn a_recovery_code_is_readable_and_unambiguous() {
        let code = generate_recovery_code();

        assert_eq!(code, code.to_lowercase());
        assert_eq!(
            code.chars().filter(|c| *c == '-').count(),
            3,
            "four groups of five, so it can be read off paper"
        );
        assert_eq!(
            normalize_recovery_code(&code).len(),
            RECOVERY_CODE_LENGTH,
            "the hyphens are decoration, not part of the secret"
        );
        assert!(
            !code.contains(['i', 'l', 'o', 'u', '0', '1']),
            "no character anybody has to guess the identity of: {}",
            code
        );
    }

    #[test]
    fn recovery_codes_do_not_repeat() {
        let codes: std::collections::HashSet<String> =
            (0..64).map(|_| generate_recovery_code()).collect();
        assert_eq!(codes.len(), 64, "each draw should be its own code");
    }

    /// A code is transcribed by hand, so the spelling that comes back is not
    /// the spelling that went out. What is compared is what is left after the
    /// decoration.
    #[test]
    fn a_code_matches_however_it_was_typed_back() {
        let code = generate_recovery_code();
        let typed = format!("  {}  ", code.to_uppercase().replace('-', " "));

        assert_eq!(
            normalize_recovery_code(&typed),
            normalize_recovery_code(&code)
        );
        assert_eq!(hash_recovery_code(&typed), hash_recovery_code(&code));
    }

    #[test]
    fn two_codes_hash_to_two_hashes() {
        assert_ne!(
            hash_recovery_code(&generate_recovery_code()),
            hash_recovery_code(&generate_recovery_code())
        );
    }

    #[test]
    fn an_unparseable_stored_hash_verifies_as_false() {
        assert!(!verify_password("anything", "not-a-phc-string"));
    }

    #[test]
    fn the_dummy_hash_parses_so_the_timing_branch_does_real_work() {
        assert!(
            PasswordHash::new(DUMMY_HASH).is_ok(),
            "an unparseable dummy would return early and defeat its purpose"
        );
        assert!(!verify_password("anything at all", DUMMY_HASH));
    }
}
