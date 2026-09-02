use crate::error::{AppError, AppResult};
use serde::{Deserialize, Serialize};
use sqlx::postgres::PgRow;
use sqlx::{PgPool, Row};
use std::sync::OnceLock;
use std::time::SystemTime;
use tracing::{debug, error, warn};

/// Global bootstrap admin configuration
static BOOTSTRAP_ADMINS: OnceLock<Vec<String>> = OnceLock::new();

/// Set the bootstrap admin emails
///
/// This should be called once at application startup with the configured admin emails.
/// Users with these emails will automatically receive Administrator role on first sign-in.
pub fn set_bootstrap_admins(admins: Vec<String>) {
    if BOOTSTRAP_ADMINS.set(admins).is_err() {
        warn!("Bootstrap admins already set, ignoring duplicate configuration");
    }
}

/// Get the bootstrap admin emails
fn get_bootstrap_admins() -> &'static [String] {
    BOOTSTRAP_ADMINS.get().map(|v| v.as_slice()).unwrap_or(&[])
}

/// Local usernames the operator declared administrators.
static BOOTSTRAP_ADMIN_USERNAMES: OnceLock<Vec<String>> = OnceLock::new();

/// Set the local usernames that are administrators.
///
/// The counterpart of [`set_bootstrap_admins`] for an engine with no OAuth
/// provider. Matching an email address is only meaningful when a provider
/// verified it, and a local account has no address at all — so on a personal
/// install the email list can never name anybody, and the account the owner
/// created for themselves has no way to reach the administrator tier: granting
/// a role takes an administrator, and there is none.
///
/// Naming a username in `auth.internal.bootstrap_admin_usernames` is the same
/// declaration by the same authority — the configuration file, which only
/// whoever runs the engine can write — for the accounts a personal install
/// actually has.
pub fn set_bootstrap_admin_usernames(usernames: Vec<String>) {
    let normalized = usernames
        .iter()
        .map(|username| username.trim().to_lowercase())
        .filter(|username| !username.is_empty())
        .collect();

    if BOOTSTRAP_ADMIN_USERNAMES.set(normalized).is_err() {
        warn!("Bootstrap admin usernames already set, ignoring duplicate configuration");
    }
}

/// Whether the operator named this local username an administrator.
fn is_bootstrap_admin_username(username: &str) -> bool {
    let username = username.trim().to_lowercase();

    BOOTSTRAP_ADMIN_USERNAMES
        .get()
        .map(|names| names.contains(&username))
        .unwrap_or(false)
}

/// Realm value meaning "a principal on every host this engine serves".
///
/// Only ever set deliberately, and only from a place that already carries the
/// authority: an administrator calling `/engine/user_realm`, or an address the
/// operator wrote into `auth.bootstrap_admins`. No sign-in *earns* it — an
/// account anyone can create must not reach every host by existing — which is
/// why [`upsert_internal_user`] never sets it and guests and local accounts
/// stay scoped to the host they were created on.
pub const GLOBAL_REALM: &str = "*";

/// Whether an account in `realm` authenticates on `host`.
///
/// An empty realm authorizes nothing. It marks a row created before realms
/// existed, and the next sign-in records the host it happened on — so this is
/// a re-authentication, not a lockout. Treating it as global instead would
/// mean a column added to bound accounts shipped defaulting to unbounded.
pub fn realm_authorizes_host(realm: &str, host: &str) -> bool {
    if realm.is_empty() {
        return false;
    }
    realm == GLOBAL_REALM || realm.eq_ignore_ascii_case(host)
}

/// Defines the types of user repository errors that can occur
#[derive(Debug, thiserror::Error)]
pub enum UserRepositoryError {
    #[error("Mutex lock failed: {0}")]
    LockError(String),
    #[error("User not found: {0}")]
    UserNotFound(String),
    #[error("Invalid data format: {0}")]
    InvalidData(String),
}

/// User roles in the system
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum UserRole {
    /// Basic authenticated user
    Authenticated,
    /// User with editor privileges
    Editor,
    /// User with administrator privileges
    Administrator,
}

impl UserRole {
    /// Check if this role has at least the privileges of another role
    pub fn has_privilege(&self, required: &UserRole) -> bool {
        matches!(
            (self, required),
            (UserRole::Administrator, _)
                | (UserRole::Editor, UserRole::Editor | UserRole::Authenticated)
                | (UserRole::Authenticated, UserRole::Authenticated)
        )
    }
}

/// Provider-specific user information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderInfo {
    /// Provider name (e.g., "google", "github")
    pub provider_name: String,
    /// Provider-specific user ID
    pub provider_user_id: String,
    /// When the user first authenticated with this provider
    pub first_auth_at: SystemTime,
    /// When the user last authenticated with this provider
    pub last_auth_at: SystemTime,
}

/// User data stored in the repository
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct User {
    /// Unique internal user ID (UUID)
    pub id: String,
    /// User's email address. `None` for identities the engine authenticates
    /// itself — a guest has no address, and a local account is named by its
    /// username rather than reachable by mail.
    pub email: Option<String>,
    /// User's display name
    pub name: Option<String>,
    /// User's roles in the system
    pub roles: Vec<UserRole>,
    /// When the user was first created
    pub created_at: SystemTime,
    /// When the user data was last updated
    pub updated_at: SystemTime,
    /// Provider information for all providers this user has authenticated with
    pub providers: Vec<ProviderInfo>,
    /// The host this account is a principal on. [`GLOBAL_REALM`] for every
    /// host; empty for a row that predates realms and has not signed in since.
    pub realm: String,
}

impl User {
    /// Create a new user with default authenticated role
    pub fn new(
        email: Option<String>,
        name: Option<String>,
        provider_name: String,
        provider_user_id: String,
        realm: String,
    ) -> Self {
        let now = SystemTime::now();
        let id = uuid::Uuid::new_v4().to_string();

        Self {
            id,
            email,
            name,
            roles: vec![UserRole::Authenticated],
            created_at: now,
            updated_at: now,
            providers: vec![ProviderInfo {
                provider_name,
                provider_user_id,
                first_auth_at: now,
                last_auth_at: now,
            }],
            realm,
        }
    }

    /// Check if user has a specific role
    pub fn has_role(&self, role: &UserRole) -> bool {
        self.roles.iter().any(|r| r == role)
    }

    /// Check if user has the required privilege level
    pub fn has_privilege(&self, required: &UserRole) -> bool {
        self.roles.iter().any(|r| r.has_privilege(required))
    }

    /// Add a role if not already present
    pub fn add_role(&mut self, role: UserRole) {
        if !self.has_role(&role) {
            self.roles.push(role);
            self.updated_at = SystemTime::now();
        }
    }

    /// Remove a role
    pub fn remove_role(&mut self, role: &UserRole) {
        self.roles.retain(|r| r != role);
        self.updated_at = SystemTime::now();
    }

    /// Update user information from a new authentication
    pub fn update_from_auth(
        &mut self,
        email: Option<String>,
        name: Option<String>,
        provider_name: String,
        provider_user_id: String,
    ) {
        // A provider that stops reporting an address must not erase the one we
        // already have; only an address actually supplied replaces it.
        if email.is_some() {
            self.email = email;
        }
        if name.is_some() {
            self.name = name;
        }
        self.updated_at = SystemTime::now();

        // Update or add provider info
        if let Some(provider_info) = self
            .providers
            .iter_mut()
            .find(|p| p.provider_name == provider_name)
        {
            provider_info.last_auth_at = SystemTime::now();
        } else {
            self.providers.push(ProviderInfo {
                provider_name,
                provider_user_id,
                first_auth_at: SystemTime::now(),
                last_auth_at: SystemTime::now(),
            });
        }
    }
}

/// Get database pool
fn get_db_pool() -> AppResult<std::sync::Arc<crate::database::Database>> {
    crate::repository::get_db_pool().ok_or_else(|| AppError::Internal {
        message: "Database not initialized".to_string(),
    })
}

/// Convert chrono::DateTime to SystemTime
fn datetime_to_system_time(dt: chrono::DateTime<chrono::Utc>) -> SystemTime {
    std::time::UNIX_EPOCH + std::time::Duration::from_secs(dt.timestamp() as u64)
}

/// Database-backed upsert user
/// The record a sign-in writes, whoever vouched for it.
struct UpsertUser<'a> {
    /// `None` for identities the engine authenticates itself, which have no
    /// address. An existing address is never erased by one.
    email: Option<&'a str>,
    name: Option<&'a str>,
    provider_name: &'a str,
    provider_user_id: &'a str,
    is_admin: bool,
    is_editor: bool,
    /// The host the sign-in happened on — or, when `realm_is_authoritative`,
    /// the realm configuration says this account has. Recorded on creation,
    /// and on an existing row filled in only when it is still empty.
    realm: &'a str,
    /// Whether `realm` is asserted by configuration rather than observed from
    /// the request. An ordinary sign-in observes the host and must not move an
    /// account that already has a realm; a realm the operator declared is not
    /// an observation and replaces what is stored.
    realm_is_authoritative: bool,
}

async fn db_upsert_user(pool: &PgPool, user: UpsertUser<'_>) -> AppResult<String> {
    let UpsertUser {
        email,
        name,
        provider_name,
        provider_user_id,
        is_admin,
        is_editor,
        realm,
        realm_is_authoritative,
    } = user;
    let now = chrono::Utc::now();

    // Try to update existing user first (preserve existing roles)
    //
    // The realm is filled in only when it is empty — a row from before realms
    // existed, recording the host of the first sign-in since. A realm observed
    // from a request is never overwritten: signing in on another host must not
    // move an account there, or re-homing an account would be as easy as
    // visiting a different URL.
    //
    // An authoritative realm is different. It comes from configuration rather
    // than from wherever the browser happened to be pointed, so it replaces
    // what is stored — including on a row stamped with a single host before the
    // operator named the address.
    let update_result = sqlx::query(
        r#"
        UPDATE users
        SET email = COALESCE($1, email),
            name = $2,
            updated_at = $3,
            last_login_at = $3,
            realm = CASE WHEN $7 THEN $6 WHEN realm = '' THEN $6 ELSE realm END
        WHERE provider = $4 AND provider_user_id = $5
        RETURNING user_id
        "#,
    )
    .bind(email)
    .bind(name)
    .bind(now)
    .bind(provider_name)
    .bind(provider_user_id)
    .bind(realm)
    .bind(realm_is_authoritative)
    .fetch_optional(pool)
    .await
    .map_err(|e| {
        error!("Database error updating user: {}", e);
        AppError::Database {
            message: format!("Database error: {}", e),
            source: None,
        }
    })?;

    if let Some(row) = update_result {
        let user_id: String = row.try_get("user_id").map_err(|e| {
            error!("Database error getting user_id: {}", e);
            AppError::Database {
                message: format!("Database error: {}", e),
                source: None,
            }
        })?;
        debug!("Updated existing user in database: {}", user_id);
        return Ok(user_id);
    }

    // User doesn't exist, create new one
    let user_id = uuid::Uuid::new_v4().to_string();

    sqlx::query(
        r#"
        INSERT INTO users (user_id, email, name, provider, provider_user_id, is_admin, is_editor, realm, created_at, updated_at, last_login_at)
        VALUES ($1, $2, $3, $4, $5, $6, $7, $9, $8, $8, $8)
        "#,
    )
    .bind(&user_id)
    .bind(email)
    .bind(name)
    .bind(provider_name)
    .bind(provider_user_id)
    .bind(is_admin)
    .bind(is_editor)
    .bind(now)
    .bind(realm)
    .execute(pool)
    .await
    .map_err(|e| {
        error!("Database error creating user: {}", e);
        AppError::Database { message: format!("Database error: {}", e), source: None }
    })?;

    debug!("Created new user in database: {}", user_id);
    Ok(user_id)
}

/// Database-backed delete user
async fn db_delete_user(pool: &PgPool, user_id: &str) -> AppResult<bool> {
    let result = sqlx::query("DELETE FROM users WHERE user_id = $1")
        .bind(user_id)
        .execute(pool)
        .await
        .map_err(|e| {
            error!("Database error deleting user: {}", e);
            AppError::Database {
                message: format!("Database error: {}", e),
                source: None,
            }
        })?;

    Ok(result.rows_affected() > 0)
}

/// Database-backed get user
async fn db_get_user(pool: &PgPool, user_id: &str) -> AppResult<User> {
    let row = sqlx::query(
        r#"
        SELECT user_id, email, name, provider, provider_user_id, is_admin, is_editor, realm, created_at, updated_at
        FROM users
        WHERE user_id = $1
        "#,
    )
    .bind(user_id)
    .fetch_optional(pool)
    .await
    .map_err(|e| {
        error!("Database error getting user: {}", e);
        AppError::Database { message: format!("Database error: {}", e), source: None }
    })?
    .ok_or_else(|| AppError::Validation { field: "user_id".to_string(), reason: format!("User not found: {}", user_id) })?;

    let db_user_id: String = row.try_get("user_id").map_err(|e| AppError::Database {
        message: e.to_string(),
        source: None,
    })?;
    let email: Option<String> = row.try_get("email").map_err(|e| AppError::Database {
        message: e.to_string(),
        source: None,
    })?;
    let name: Option<String> = row.try_get("name").map_err(|e| AppError::Database {
        message: e.to_string(),
        source: None,
    })?;
    let provider: String = row.try_get("provider").map_err(|e| AppError::Database {
        message: e.to_string(),
        source: None,
    })?;
    let provider_user_id: String =
        row.try_get("provider_user_id")
            .map_err(|e| AppError::Database {
                message: e.to_string(),
                source: None,
            })?;
    let is_admin: bool = row.try_get("is_admin").map_err(|e| AppError::Database {
        message: e.to_string(),
        source: None,
    })?;
    let is_editor: bool = row.try_get("is_editor").map_err(|e| AppError::Database {
        message: e.to_string(),
        source: None,
    })?;
    let realm: String = row.try_get("realm").map_err(|e| AppError::Database {
        message: e.to_string(),
        source: None,
    })?;
    let created_at: chrono::DateTime<chrono::Utc> =
        row.try_get("created_at").map_err(|e| AppError::Database {
            message: e.to_string(),
            source: None,
        })?;
    let updated_at: chrono::DateTime<chrono::Utc> =
        row.try_get("updated_at").map_err(|e| AppError::Database {
            message: e.to_string(),
            source: None,
        })?;

    let mut roles = vec![UserRole::Authenticated];
    if is_editor {
        roles.push(UserRole::Editor);
    }
    if is_admin {
        roles.push(UserRole::Administrator);
    }

    let providers = vec![ProviderInfo {
        provider_name: provider,
        provider_user_id,
        first_auth_at: datetime_to_system_time(created_at),
        last_auth_at: datetime_to_system_time(updated_at),
    }];

    Ok(User {
        id: db_user_id,
        email,
        name,
        roles,
        created_at: datetime_to_system_time(created_at),
        updated_at: datetime_to_system_time(updated_at),
        providers,
        realm,
    })
}

/// Database-backed find user by provider
async fn db_find_user_by_provider(
    pool: &PgPool,
    provider_name: &str,
    provider_user_id: &str,
) -> AppResult<Option<User>> {
    let row = sqlx::query(
        r#"
        SELECT user_id, email, name, provider, provider_user_id, is_admin, is_editor, realm, created_at, updated_at
        FROM users
        WHERE provider = $1 AND provider_user_id = $2
        "#,
    )
    .bind(provider_name)
    .bind(provider_user_id)
    .fetch_optional(pool)
    .await
    .map_err(|e| {
        error!("Database error finding user by provider: {}", e);
        AppError::Database { message: format!("Database error: {}", e), source: None }
    })?;

    if let Some(row) = row {
        let db_user_id: String = row.try_get("user_id").map_err(|e| AppError::Database {
            message: e.to_string(),
            source: None,
        })?;
        let email: Option<String> = row.try_get("email").map_err(|e| AppError::Database {
            message: e.to_string(),
            source: None,
        })?;
        let name: Option<String> = row.try_get("name").map_err(|e| AppError::Database {
            message: e.to_string(),
            source: None,
        })?;
        let provider: String = row.try_get("provider").map_err(|e| AppError::Database {
            message: e.to_string(),
            source: None,
        })?;
        let provider_user_id: String =
            row.try_get("provider_user_id")
                .map_err(|e| AppError::Database {
                    message: e.to_string(),
                    source: None,
                })?;
        let is_admin: bool = row.try_get("is_admin").map_err(|e| AppError::Database {
            message: e.to_string(),
            source: None,
        })?;
        let is_editor: bool = row.try_get("is_editor").map_err(|e| AppError::Database {
            message: e.to_string(),
            source: None,
        })?;
        let realm: String = row.try_get("realm").map_err(|e| AppError::Database {
            message: e.to_string(),
            source: None,
        })?;
        let created_at: chrono::DateTime<chrono::Utc> =
            row.try_get("created_at").map_err(|e| AppError::Database {
                message: e.to_string(),
                source: None,
            })?;
        let updated_at: chrono::DateTime<chrono::Utc> =
            row.try_get("updated_at").map_err(|e| AppError::Database {
                message: e.to_string(),
                source: None,
            })?;

        let mut roles = vec![UserRole::Authenticated];
        if is_editor {
            roles.push(UserRole::Editor);
        }
        if is_admin {
            roles.push(UserRole::Administrator);
        }

        let providers = vec![ProviderInfo {
            provider_name: provider,
            provider_user_id,
            first_auth_at: datetime_to_system_time(created_at),
            last_auth_at: datetime_to_system_time(updated_at),
        }];

        Ok(Some(User {
            id: db_user_id,
            email,
            name,
            roles,
            created_at: datetime_to_system_time(created_at),
            updated_at: datetime_to_system_time(updated_at),
            providers,
            realm,
        }))
    } else {
        Ok(None)
    }
}

/// Upsert a user based on provider authentication
///
/// If a user with the given provider credentials already exists, returns the existing user.
/// Otherwise, creates a new user with a generated ID.
///
/// # Arguments
/// * `email` - User's email address
/// * `name` - User's display name (optional)
/// * `provider_name` - OAuth provider name (e.g., "google", "github")
/// * `provider_user_id` - Provider-specific user ID
///
/// # Returns
/// The user ID (either existing or newly created)
pub async fn upsert_user(
    email: String,
    name: Option<String>,
    provider_name: String,
    provider_user_id: String,
    realm: String,
) -> AppResult<String> {
    let bootstrap_admins = get_bootstrap_admins();
    upsert_user_with_bootstrap(
        email,
        name,
        provider_name,
        provider_user_id,
        bootstrap_admins,
        realm,
    )
    .await
}

/// Upsert a user with bootstrap admin configuration
///
/// This is the internal implementation that supports bootstrap admins.
/// If the user's email matches one in the bootstrap_admins list, they
/// automatically get the Administrator role on creation, and the
/// [`GLOBAL_REALM`] — an engine administrator is a principal on every host the
/// engine serves, including the management host, whichever host they happened
/// to sign in on first. Both are re-applied on every sign-in, so an account
/// stamped with a single host before the operator named its address is repaired
/// the next time it logs in rather than needing the database edited.
///
/// # Arguments
/// * `email` - User's email address
/// * `name` - User's display name (optional)
/// * `provider_name` - OAuth provider name (e.g., "google", "github")
/// * `provider_user_id` - Provider-specific user ID
/// * `bootstrap_admins` - List of emails that should automatically get admin role
///
/// # Returns
/// The user ID (either existing or newly created)
pub async fn upsert_user_with_bootstrap(
    email: String,
    name: Option<String>,
    provider_name: String,
    provider_user_id: String,
    bootstrap_admins: &[String],
    realm: String,
) -> AppResult<String> {
    // Validate inputs
    if email.trim().is_empty() {
        return Err(AppError::Validation {
            field: "email".to_string(),
            reason: "Email cannot be empty".to_string(),
        });
    }

    if provider_name.trim().is_empty() {
        return Err(AppError::Validation {
            field: "provider_name".to_string(),
            reason: "Provider name cannot be empty".to_string(),
        });
    }

    if provider_user_id.trim().is_empty() {
        return Err(AppError::Validation {
            field: "provider_user_id".to_string(),
            reason: "Provider user ID cannot be empty".to_string(),
        });
    }

    // Check if this email is in the bootstrap admins list
    let email_lower = email.to_lowercase();
    let is_bootstrap_admin = bootstrap_admins
        .iter()
        .any(|admin_email| admin_email.to_lowercase() == email_lower);

    let is_admin = is_bootstrap_admin;
    let is_editor = false; // For now, only admins get editor role automatically

    if is_bootstrap_admin {
        debug!(
            "User {} will be granted Administrator role (bootstrap admin)",
            email
        );
    }

    // An address the operator named in `auth.bootstrap_admins` is a principal
    // on every host, not merely on the one it happened to sign in on first.
    //
    // Scoping it like any other account defeats the bootstrap path on a
    // multi-host deployment, and does it silently: the account that exists to
    // administer the engine gets stamped with whichever host it first touched,
    // every later request on the management host is refused as
    // [`realm_authorizes_host`] fails, and the sign-in loops. The way back —
    // `/engine/user_realm` — is served only on the management host that can no
    // longer be reached, so the only remaining recovery is editing the database
    // by hand.
    //
    // Widening here is the operator's own declaration rather than something a
    // sign-in earned: the address is written in configuration, which is exactly
    // the authority [`GLOBAL_REALM`] is meant to require. An account nobody
    // named cannot reach this branch, and self-registered identities never
    // reach this function at all — see [`upsert_internal_user`].
    let (realm, realm_is_authoritative) = if is_bootstrap_admin {
        (GLOBAL_REALM.to_string(), true)
    } else {
        (realm, false)
    };

    let db = get_db_pool()?;

    let user_id = db_upsert_user(
        db.pool(),
        UpsertUser {
            email: Some(&email),
            name: name.as_deref(),
            provider_name: &provider_name,
            provider_user_id: &provider_user_id,
            is_admin,
            is_editor,
            realm: &realm,
            realm_is_authoritative,
        },
    )
    .await?;

    // An upsert preserves the roles on a row it finds, so the flag above only
    // ever reached accounts created after the address was configured. Naming an
    // existing account has to work too — it is the ordinary case when an
    // operator adds an administrator.
    if is_bootstrap_admin {
        apply_configured_administrator(db.pool(), &user_id).await?;
    }

    Ok(user_id)
}

/// Make an account the operator named in configuration what the configuration
/// says it is: an administrator, and a principal on every host.
///
/// Run on every sign-in rather than only at creation, because the interesting
/// case is the account that already exists. Someone installs the engine, makes
/// themselves an account, then reads the documentation and writes their
/// username into the config — and an upsert preserves the roles on a row it
/// finds, so without this the declaration would apply to everybody except the
/// person who made it.
///
/// Writes only when something is actually different, so an ordinary sign-in by
/// a configured administrator costs nothing and leaves no audit noise.
async fn apply_configured_administrator(pool: &PgPool, user_id: &str) -> AppResult<()> {
    let updated = sqlx::query(
        r#"
        UPDATE users
        SET is_admin = TRUE, realm = $2, updated_at = $3
        WHERE user_id = $1 AND (is_admin = FALSE OR realm <> $2)
        "#,
    )
    .bind(user_id)
    .bind(GLOBAL_REALM)
    .bind(chrono::Utc::now())
    .execute(pool)
    .await
    .map_err(|e| AppError::Database {
        message: format!("Database error granting configured administrator: {}", e),
        source: None,
    })?;

    if updated.rows_affected() > 0 {
        debug!(
            "User {} is an administrator on every host by configuration",
            user_id
        );
    }

    Ok(())
}

/// Grant the administrator role to a local account the operator named in
/// `auth.internal.bootstrap_admin_usernames`, and do nothing for anyone else.
///
/// The way a personal install gets an owner. Every other road to the
/// administrator tier needs an administrator to already exist —
/// `/engine/user_roles` is guarded by `AdministerEngine`, and
/// [`upsert_internal_user`] deliberately grants nothing — which on a laptop
/// with no OAuth provider is a circle with no way in. The remaining workaround
/// was a development mode that handed those capabilities to *anonymous*
/// callers on every interface the engine binds; it is gone, and this is what
/// replaced it.
///
/// A username is not an identity claim the way a provider-verified address is,
/// so this grants nothing on its own: it names a local account, and reaching
/// that account still takes its password.
pub async fn apply_bootstrap_admin_username(user_id: &str, username: &str) -> AppResult<()> {
    if !is_bootstrap_admin_username(username) {
        return Ok(());
    }

    let db = get_db_pool()?;
    apply_configured_administrator(db.pool(), user_id).await
}

/// Create or refresh an identity the engine authenticates itself — a guest, or
/// a local username-and-password account.
///
/// Deliberately not a thin wrapper over [`upsert_user_with_bootstrap`]. That
/// path grants the Administrator role to any address listed in
/// `auth.bootstrap_admins`, and matching an address is only meaningful when a
/// provider has verified it. An internal identity has no verified address and
/// carries no email at all, so it is created with the Authenticated role and
/// nothing more; reaching any higher tier takes an administrator granting it.
///
/// `realm` is the host the sign-up happened on, and is never
/// [`GLOBAL_REALM`]. An account anyone can create must not become a principal
/// everywhere by being created.
pub async fn upsert_internal_user(
    name: Option<String>,
    provider_name: String,
    provider_user_id: String,
    realm: String,
) -> AppResult<String> {
    if provider_name.trim().is_empty() {
        return Err(AppError::Validation {
            field: "provider_name".to_string(),
            reason: "Provider name cannot be empty".to_string(),
        });
    }

    if provider_user_id.trim().is_empty() {
        return Err(AppError::Validation {
            field: "provider_user_id".to_string(),
            reason: "Provider user ID cannot be empty".to_string(),
        });
    }

    let db = get_db_pool()?;

    db_upsert_user(
        db.pool(),
        UpsertUser {
            email: None,
            name: name.as_deref(),
            provider_name: &provider_name,
            provider_user_id: &provider_user_id,
            is_admin: false,
            is_editor: false,
            realm: &realm,
            // Never. An identity anyone can create for themselves is a
            // principal exactly where it was created, and nothing about a
            // sign-up widens that.
            realm_is_authoritative: false,
        },
    )
    .await
}

/// Get a user by their internal ID (for blocking contexts, e.g. JS host functions)
pub fn get_user(user_id: &str) -> AppResult<User> {
    tokio::task::block_in_place(|| {
        tokio::runtime::Handle::current().block_on(get_user_async(user_id))
    })
}

/// Async variant of [`get_user`] for callers already in async context
pub async fn get_user_async(user_id: &str) -> AppResult<User> {
    let db = get_db_pool()?;
    db_get_user(db.pool(), user_id).await
}

/// Find a user by provider credentials
pub fn find_user_by_provider(
    provider_name: &str,
    provider_user_id: &str,
) -> AppResult<Option<User>> {
    let db = get_db_pool()?;
    tokio::task::block_in_place(|| {
        tokio::runtime::Handle::current().block_on(async {
            db_find_user_by_provider(db.pool(), provider_name, provider_user_id).await
        })
    })
}

/// Database-backed update user roles
async fn db_update_user_roles(
    pool: &PgPool,
    user_id: &str,
    is_admin: bool,
    is_editor: bool,
) -> AppResult<()> {
    let now = chrono::Utc::now();

    sqlx::query(
        r#"
        UPDATE users
        SET is_admin = $1, is_editor = $2, updated_at = $3
        WHERE user_id = $4
        "#,
    )
    .bind(is_admin)
    .bind(is_editor)
    .bind(now)
    .bind(user_id)
    .execute(pool)
    .await
    .map_err(|e| {
        error!("Database error updating user roles: {}", e);
        AppError::Database {
            message: format!("Database error: {}", e),
            source: None,
        }
    })?;

    debug!(
        "Updated user roles in database: {} (admin: {}, editor: {})",
        user_id, is_admin, is_editor
    );
    Ok(())
}

/// Update user roles
///
/// Completely replaces the user's role set with the provided roles.
/// Always ensures at least Authenticated role is present.
pub fn update_user_roles(user_id: &str, roles: Vec<UserRole>) -> AppResult<()> {
    // Ensure Authenticated role is always present
    let mut new_roles = roles;
    if !new_roles
        .iter()
        .any(|r| matches!(r, UserRole::Authenticated))
    {
        new_roles.push(UserRole::Authenticated);
    }

    // Calculate boolean flags from roles
    let is_admin = new_roles
        .iter()
        .any(|r| matches!(r, UserRole::Administrator));
    let is_editor = new_roles.iter().any(|r| matches!(r, UserRole::Editor));

    let db = get_db_pool()?;
    tokio::task::block_in_place(|| {
        tokio::runtime::Handle::current().block_on(async {
            db_update_user_roles(db.pool(), user_id, is_admin, is_editor).await?;

            // What was granted or taken away has to reach the sessions that
            // already exist. Roles are read from the repository once, when a
            // session is minted, and every consumer reads the stamped copy —
            // so without this, revoking an administrator revokes nothing they
            // are currently holding, and granting a role does nothing until
            // the person happens to sign in again.
            match crate::security::delete_sessions_for_user(db.pool(), user_id).await {
                Ok(0) => {}
                Ok(count) => debug!(
                    "Ended {} session(s) for {} so the new roles take effect",
                    count, user_id
                ),
                Err(e) => {
                    // The roles are already written. Reporting failure here
                    // would say the change did not happen when it did, so this
                    // is loud rather than fatal.
                    error!(
                        "Roles for {} changed but their sessions could not be ended: {}",
                        user_id, e
                    );
                }
            }

            Ok(())
        })
    })
}

/// Move a user into a realm — the host they are a principal on, or
/// [`GLOBAL_REALM`] for every host.
///
/// The only way `*` is ever set. No sign-in path produces it, because an
/// account anyone can create must not reach every host by existing; an
/// administrator granting it is a deliberate act, and the audit trail on the
/// calling side records it as one.
///
/// Takes effect immediately. Sessions carry the realm they were minted with
/// and every consumer reads that stamped copy, so the sessions the account
/// already holds are ended here — otherwise narrowing an account from `*` to
/// one host would leave it authenticating everywhere for the rest of those
/// sessions' lives (up to `max_session_age`, thirty days by default).
pub fn set_user_realm(user_id: &str, realm: &str) -> AppResult<()> {
    let realm = realm.trim().to_lowercase();
    if realm.is_empty() {
        return Err(AppError::Validation {
            field: "realm".to_string(),
            reason: "Realm cannot be empty".to_string(),
        });
    }

    let db = get_db_pool()?;
    tokio::task::block_in_place(|| {
        tokio::runtime::Handle::current().block_on(async {
            let result =
                sqlx::query("UPDATE users SET realm = $1, updated_at = $2 WHERE user_id = $3")
                    .bind(&realm)
                    .bind(chrono::Utc::now())
                    .bind(user_id)
                    .execute(db.pool())
                    .await
                    .map_err(|e| {
                        error!("Database error setting user realm: {}", e);
                        AppError::Database {
                            message: format!("Database error: {}", e),
                            source: None,
                        }
                    })?;

            if result.rows_affected() == 0 {
                return Err(AppError::from(UserRepositoryError::UserNotFound(
                    user_id.to_string(),
                )));
            }

            // The new realm has to reach the sessions that already exist, for
            // the same reason a role change does: the realm is stamped in at
            // mint time and `validate_session` compares the request's host
            // against that copy, not against the row this just wrote.
            match crate::security::delete_sessions_for_user(db.pool(), user_id).await {
                Ok(0) => {}
                Ok(count) => debug!(
                    "Ended {} session(s) for {} so the new realm takes effect",
                    count, user_id
                ),
                Err(e) => {
                    // The realm is already written. Reporting failure here
                    // would say the change did not happen when it did, so this
                    // is loud rather than fatal.
                    error!(
                        "Realm for {} changed but their sessions could not be ended: {}",
                        user_id, e
                    );
                }
            }

            Ok(())
        })
    })
}

/// Add a role to a user
pub fn add_user_role(user_id: &str, role: UserRole) -> AppResult<()> {
    // Get current user to determine new role flags
    let current_user = get_user(user_id)?;
    let mut new_roles = current_user.roles;
    new_roles.push(role);

    // Remove duplicates
    new_roles.sort_by(|a, b| format!("{:?}", a).cmp(&format!("{:?}", b)));
    new_roles.dedup();

    // Update with new roles
    update_user_roles(user_id, new_roles)
}

/// Remove a role from a user
pub fn remove_user_role(user_id: &str, role: &UserRole) -> AppResult<()> {
    // Don't allow removing Authenticated role
    if matches!(role, UserRole::Authenticated) {
        return Err(AppError::Validation {
            field: "role".to_string(),
            reason: "Cannot remove Authenticated role".to_string(),
        });
    }

    // Get current user to determine new role flags
    let current_user = get_user(user_id)?;
    let mut new_roles = current_user.roles;
    new_roles.retain(|r| r != role);

    // Update with new roles
    update_user_roles(user_id, new_roles)
}

/// Find an account by the address a provider gave it.
///
/// Only the federated accounts have one; a local account or a guest has no
/// address at all, and is found by its provider identity instead.
pub async fn find_user_by_email(email: &str) -> AppResult<Option<User>> {
    let db = get_db_pool()?;
    let user_id: Option<String> =
        sqlx::query_scalar("SELECT user_id FROM users WHERE LOWER(email) = LOWER($1) LIMIT 1")
            .bind(email.trim())
            .fetch_optional(db.pool())
            .await
            .map_err(|e| AppError::Database {
                message: format!("Database error finding user by email: {}", e),
                source: None,
            })?;

    match user_id {
        Some(user_id) => db_get_user(db.pool(), &user_id).await.map(Some),
        None => Ok(None),
    }
}

/// List all users (for admin purposes)
pub fn list_users() -> AppResult<Vec<User>> {
    let db = get_db_pool()?;
    tokio::task::block_in_place(|| {
        tokio::runtime::Handle::current().block_on(async {
            let pool = db.pool();
            let rows = sqlx::query(
                    r#"
                    SELECT user_id, email, name, provider, provider_user_id, is_admin, is_editor, realm, created_at, updated_at
                    FROM users
                    ORDER BY created_at DESC
                    "#,
                )
                .fetch_all(pool)
                .await
                .map_err(|e| {
                    error!("Database error listing users: {}", e);
                    AppError::Database { message: format!("Database error: {}", e), source: None }
                })?;

            rows.into_iter().map(|row| convert_row_to_user(&row)).collect()
        })
    })
}

/// Convert a database row into a User object.
fn convert_row_to_user(row: &PgRow) -> AppResult<User> {
    let db_user_id: String = row.try_get("user_id").map_err(|e| AppError::Database {
        message: e.to_string(),
        source: None,
    })?;
    let email: Option<String> = row.try_get("email").map_err(|e| AppError::Database {
        message: e.to_string(),
        source: None,
    })?;
    let name: Option<String> = row.try_get("name").map_err(|e| AppError::Database {
        message: e.to_string(),
        source: None,
    })?;
    let provider: String = row.try_get("provider").map_err(|e| AppError::Database {
        message: e.to_string(),
        source: None,
    })?;
    let provider_user_id: String =
        row.try_get("provider_user_id")
            .map_err(|e| AppError::Database {
                message: e.to_string(),
                source: None,
            })?;
    let is_admin: bool = row.try_get("is_admin").map_err(|e| AppError::Database {
        message: e.to_string(),
        source: None,
    })?;
    let is_editor: bool = row.try_get("is_editor").map_err(|e| AppError::Database {
        message: e.to_string(),
        source: None,
    })?;
    let realm: String = row.try_get("realm").map_err(|e| AppError::Database {
        message: e.to_string(),
        source: None,
    })?;
    let created_at: chrono::DateTime<chrono::Utc> =
        row.try_get("created_at").map_err(|e| AppError::Database {
            message: e.to_string(),
            source: None,
        })?;
    let updated_at: chrono::DateTime<chrono::Utc> =
        row.try_get("updated_at").map_err(|e| AppError::Database {
            message: e.to_string(),
            source: None,
        })?;

    let mut roles = vec![UserRole::Authenticated];
    if is_editor {
        roles.push(UserRole::Editor);
    }
    if is_admin {
        roles.push(UserRole::Administrator);
    }

    let providers = vec![ProviderInfo {
        provider_name: provider,
        provider_user_id,
        first_auth_at: datetime_to_system_time(created_at),
        last_auth_at: datetime_to_system_time(updated_at),
    }];

    Ok(User {
        id: db_user_id.clone(),
        email,
        name,
        roles,
        created_at: datetime_to_system_time(created_at),
        updated_at: datetime_to_system_time(updated_at),
        providers,
        realm,
    })
}

/// Get user count
pub fn get_user_count() -> AppResult<usize> {
    let db = get_db_pool()?;
    tokio::task::block_in_place(|| {
        tokio::runtime::Handle::current().block_on(async {
            let row = sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM users")
                .fetch_one(db.pool())
                .await
                .map_err(|e| AppError::Database {
                    message: format!("Database error counting users: {}", e),
                    source: None,
                })?;
            Ok(row as usize)
        })
    })
}

/// Delete a user (for testing/admin purposes)
pub fn delete_user(user_id: &str) -> AppResult<bool> {
    let db = get_db_pool()?;
    let deleted = tokio::task::block_in_place(|| {
        tokio::runtime::Handle::current().block_on(async {
            // Before the row goes: a session outlives the user it belongs to
            // otherwise, and it carries the roles and realm that were stamped
            // into it, so it keeps working for whatever is left of its life.
            if let Err(e) = crate::security::delete_sessions_for_user(db.pool(), user_id).await {
                error!(
                    "Could not end sessions for {} before deletion: {}",
                    user_id, e
                );
            }

            db_delete_user(db.pool(), user_id).await
        })
    })?;
    if deleted {
        debug!("Deleted user: {}", user_id);
    }
    Ok(deleted)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Once, OnceLock};
    use tokio::runtime::Runtime;

    static DB_INIT: Once = Once::new();

    fn get_runtime() -> &'static Runtime {
        static RUNTIME: OnceLock<Runtime> = OnceLock::new();
        RUNTIME.get_or_init(|| {
            tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .expect("Failed to create Tokio runtime")
        })
    }

    fn setup_db() {
        let Ok(url) = std::env::var("DATABASE_URL") else {
            return;
        };
        DB_INIT.call_once(|| {
            get_runtime().block_on(async {
                let pool = sqlx::PgPool::connect_lazy(&url).expect("Failed to create pool");
                let db = std::sync::Arc::new(crate::database::Database::from_pool(pool.clone()));
                let _ = crate::database::initialize_global_database(db);
                let server_id = crate::notifications::generate_server_id();
                let _ = crate::notifications::initialize_server_id(server_id.clone());
                let repo = crate::repository::PostgresRepository::new(pool, server_id);
                let _ = crate::repository::initialize_repository(repo);
            });
        });
    }

    fn should_skip_db_tests() -> bool {
        std::env::var("DATABASE_URL").is_err()
    }

    #[test]
    fn test_user_creation() {
        if should_skip_db_tests() {
            return;
        }
        let user = User::new(
            Some("test@example.com".to_string()),
            Some("Test User".to_string()),
            "google".to_string(),
            "google123".to_string(),
            "test.example.com".to_string(),
        );

        assert_eq!(user.email.as_deref(), Some("test@example.com"));
        assert_eq!(user.name, Some("Test User".to_string()));
        assert_eq!(user.roles.len(), 1);
        assert_eq!(user.roles[0], UserRole::Authenticated);
        assert_eq!(user.providers.len(), 1);
    }

    #[test]
    fn test_upsert_user_new() {
        if should_skip_db_tests() {
            return;
        }
        setup_db();
        let rt = get_runtime();

        rt.block_on(async {
            let user_id = upsert_user(
                "new@example.com".to_string(),
                Some("New User".to_string()),
                "github".to_string(),
                "github456".to_string(),
                "test.example.com".to_string(),
            )
            .await
            .unwrap();

            let user = get_user(&user_id).unwrap();
            assert_eq!(user.email.as_deref(), Some("new@example.com"));
            assert_eq!(user.name, Some("New User".to_string()));
        });
    }

    #[test]
    fn test_upsert_user_existing() {
        if should_skip_db_tests() {
            return;
        }
        setup_db();
        let rt = get_runtime();

        rt.block_on(async {
            // First insert
            let user_id1 = upsert_user(
                "existing@example.com".to_string(),
                Some("Existing User".to_string()),
                "google".to_string(),
                "google789".to_string(),
                "test.example.com".to_string(),
            )
            .await
            .unwrap();

            // Second insert with same provider credentials
            let user_id2 = upsert_user(
                "updated@example.com".to_string(),
                Some("Updated User".to_string()),
                "google".to_string(),
                "google789".to_string(),
                "test.example.com".to_string(),
            )
            .await
            .unwrap();

            // Should return the same user ID
            assert_eq!(user_id1, user_id2);

            // User info should be updated
            let user = get_user(&user_id1).unwrap();
            assert_eq!(user.email.as_deref(), Some("updated@example.com"));
            assert_eq!(user.name, Some("Updated User".to_string()));
        });
    }

    #[test]
    fn test_role_management() {
        if should_skip_db_tests() {
            return;
        }
        setup_db();
        let rt = get_runtime();

        rt.block_on(async {
            let user_id = upsert_user(
                "roles@example.com".to_string(),
                None,
                "google".to_string(),
                "google_roles".to_string(),
                "test.example.com".to_string(),
            )
            .await
            .unwrap();

            // Add Editor role
            add_user_role(&user_id, UserRole::Editor).unwrap();
            let user = get_user(&user_id).unwrap();
            assert!(user.has_role(&UserRole::Editor));
            assert!(user.has_role(&UserRole::Authenticated));

            // Add Administrator role
            add_user_role(&user_id, UserRole::Administrator).unwrap();
            let user = get_user(&user_id).unwrap();
            assert!(user.has_role(&UserRole::Administrator));

            // Remove Editor role
            remove_user_role(&user_id, &UserRole::Editor).unwrap();
            let user = get_user(&user_id).unwrap();
            assert!(!user.has_role(&UserRole::Editor));
            assert!(user.has_role(&UserRole::Administrator));
        });
    }

    #[test]
    fn test_role_privileges() {
        if should_skip_db_tests() {
            return;
        }
        assert!(UserRole::Administrator.has_privilege(&UserRole::Authenticated));
        assert!(UserRole::Administrator.has_privilege(&UserRole::Editor));
        assert!(UserRole::Administrator.has_privilege(&UserRole::Administrator));

        assert!(UserRole::Editor.has_privilege(&UserRole::Authenticated));
        assert!(UserRole::Editor.has_privilege(&UserRole::Editor));
        assert!(!UserRole::Editor.has_privilege(&UserRole::Administrator));

        assert!(UserRole::Authenticated.has_privilege(&UserRole::Authenticated));
        assert!(!UserRole::Authenticated.has_privilege(&UserRole::Editor));
        assert!(!UserRole::Authenticated.has_privilege(&UserRole::Administrator));
    }

    #[test]
    fn test_cannot_remove_authenticated_role() {
        if should_skip_db_tests() {
            return;
        }
        setup_db();
        let rt = get_runtime();

        rt.block_on(async {
            let user_id = upsert_user(
                "auth@example.com".to_string(),
                None,
                "google".to_string(),
                "google_auth".to_string(),
                "test.example.com".to_string(),
            )
            .await
            .unwrap();

            let result = remove_user_role(&user_id, &UserRole::Authenticated);
            assert!(result.is_err());
        });
    }

    #[test]
    fn test_find_user_by_provider() {
        if should_skip_db_tests() {
            return;
        }
        setup_db();
        let rt = get_runtime();

        rt.block_on(async {
            let user_id = upsert_user(
                "provider@example.com".to_string(),
                None,
                "github".to_string(),
                "github_provider".to_string(),
                "test.example.com".to_string(),
            )
            .await
            .unwrap();

            let found = find_user_by_provider("github", "github_provider")
                .unwrap()
                .unwrap();
            assert_eq!(found.id, user_id);

            let not_found = find_user_by_provider("github", "nonexistent").unwrap();
            assert!(not_found.is_none());
        });
    }

    #[test]
    fn test_update_user_roles() {
        if should_skip_db_tests() {
            return;
        }
        setup_db();
        let rt = get_runtime();

        rt.block_on(async {
            let user_id = upsert_user(
                "update@example.com".to_string(),
                None,
                "google".to_string(),
                "google_update".to_string(),
                "test.example.com".to_string(),
            )
            .await
            .unwrap();

            // Set roles to Editor and Administrator
            update_user_roles(&user_id, vec![UserRole::Editor, UserRole::Administrator]).unwrap();

            let user = get_user(&user_id).unwrap();
            assert_eq!(user.roles.len(), 3); // Authenticated is auto-added
            assert!(user.has_role(&UserRole::Authenticated));
            assert!(user.has_role(&UserRole::Editor));
            assert!(user.has_role(&UserRole::Administrator));
        });
    }

    #[test]
    fn test_delete_user() {
        if should_skip_db_tests() {
            return;
        }
        setup_db();
        let rt = get_runtime();

        rt.block_on(async {
            let user_id = upsert_user(
                "delete@example.com".to_string(),
                None,
                "google".to_string(),
                "google_delete".to_string(),
                "test.example.com".to_string(),
            )
            .await
            .unwrap();

            // Verify user exists
            assert!(get_user(&user_id).is_ok());

            // Delete user
            let deleted = delete_user(&user_id).unwrap();
            assert!(deleted);

            // Verify user is gone
            assert!(get_user(&user_id).is_err());

            // Verify provider index is cleaned up
            let found = find_user_by_provider("google", "google_delete").unwrap();
            assert!(found.is_none());
        });
    }

    #[test]
    fn test_validation() {
        if should_skip_db_tests() {
            return;
        }
        setup_db();
        let rt = get_runtime();
        rt.block_on(async {
            // Empty email
            assert!(
                upsert_user(
                    "".to_string(),
                    None,
                    "google".to_string(),
                    "user123".to_string(),
                    "test.example.com".to_string()
                )
                .await
                .is_err()
            );

            // Empty provider name
            assert!(
                upsert_user(
                    "user@example.com".to_string(),
                    None,
                    "".to_string(),
                    "user123".to_string(),
                    "test.example.com".to_string()
                )
                .await
                .is_err()
            );

            // Empty provider user ID
            assert!(
                upsert_user(
                    "user@example.com".to_string(),
                    None,
                    "google".to_string(),
                    "".to_string(),
                    "test.example.com".to_string()
                )
                .await
                .is_err()
            );
        });
    }

    #[test]
    fn test_bootstrap_admin() {
        if should_skip_db_tests() {
            return;
        }
        setup_db();
        let rt = get_runtime();

        rt.block_on(async {
            let bootstrap_admins = vec!["admin@example.com".to_string()];

            // Create user with bootstrap admin email
            let admin_id = upsert_user_with_bootstrap(
                "admin@example.com".to_string(),
                Some("Admin User".to_string()),
                "google".to_string(),
                "google_admin".to_string(),
                &bootstrap_admins,
                "test.example.com".to_string(),
            )
            .await
            .unwrap();

            // User should have Administrator role automatically
            let admin_user = get_user(&admin_id).unwrap();
            assert!(admin_user.has_role(&UserRole::Administrator));
            assert!(admin_user.has_role(&UserRole::Authenticated));

            // Create regular user (not in bootstrap list)
            let user_id = upsert_user_with_bootstrap(
                "regular@example.com".to_string(),
                Some("Regular User".to_string()),
                "google".to_string(),
                "google_regular".to_string(),
                &bootstrap_admins,
                "test.example.com".to_string(),
            )
            .await
            .unwrap();

            // User should NOT have Administrator role
            let regular_user = get_user(&user_id).unwrap();
            assert!(!regular_user.has_role(&UserRole::Administrator));
            assert!(regular_user.has_role(&UserRole::Authenticated));
        });
    }

    /// The lockout this closes. An administrator signs in on the main host,
    /// their account is stamped with it, and every later request on the
    /// management host is refused — while `/engine/user_realm`, the way to
    /// widen it, is served only on the host they can no longer reach.
    #[test]
    fn a_bootstrap_admin_is_a_principal_on_every_host() {
        if should_skip_db_tests() {
            return;
        }
        setup_db();
        let rt = get_runtime();

        rt.block_on(async {
            let bootstrap_admins = vec!["realm-admin@example.com".to_string()];
            let provider_user_id = format!("google_realm_admin_{}", uuid::Uuid::new_v4());

            let admin_id = upsert_user_with_bootstrap(
                "realm-admin@example.com".to_string(),
                Some("Realm Admin".to_string()),
                "google".to_string(),
                provider_user_id.clone(),
                &bootstrap_admins,
                "softagen.com".to_string(),
            )
            .await
            .unwrap();

            let admin = get_user(&admin_id).unwrap();
            assert_eq!(
                admin.realm, GLOBAL_REALM,
                "an address named in bootstrap_admins is not scoped to the host it signed in on"
            );
            assert!(realm_authorizes_host(&admin.realm, "manage.softagen.com"));

            // Signing in again elsewhere keeps it global rather than re-homing.
            upsert_user_with_bootstrap(
                "realm-admin@example.com".to_string(),
                Some("Realm Admin".to_string()),
                "google".to_string(),
                provider_user_id,
                &bootstrap_admins,
                "world.softagen.com".to_string(),
            )
            .await
            .unwrap();
            assert_eq!(get_user(&admin_id).unwrap().realm, GLOBAL_REALM);
        });
    }

    /// An account stamped with one host before the operator named its address
    /// is repaired by signing in, not by editing the database — which matters
    /// because the endpoint that would repair it is unreachable from where the
    /// locked-out admin can authenticate.
    #[test]
    fn naming_an_existing_account_as_a_bootstrap_admin_widens_it_on_next_sign_in() {
        if should_skip_db_tests() {
            return;
        }
        setup_db();
        let rt = get_runtime();

        rt.block_on(async {
            let provider_user_id = format!("google_promoted_{}", uuid::Uuid::new_v4());

            // Signs in before the operator lists them: scoped to one host.
            let user_id = upsert_user_with_bootstrap(
                "promoted@example.com".to_string(),
                None,
                "google".to_string(),
                provider_user_id.clone(),
                &[],
                "softagen.com".to_string(),
            )
            .await
            .unwrap();
            assert_eq!(get_user(&user_id).unwrap().realm, "softagen.com");
            assert!(!realm_authorizes_host(
                &get_user(&user_id).unwrap().realm,
                "manage.softagen.com"
            ));

            // Operator adds them to auth.bootstrap_admins; next sign-in repairs it.
            let bootstrap_admins = vec!["promoted@example.com".to_string()];
            upsert_user_with_bootstrap(
                "promoted@example.com".to_string(),
                None,
                "google".to_string(),
                provider_user_id,
                &bootstrap_admins,
                "softagen.com".to_string(),
            )
            .await
            .unwrap();

            let repaired = get_user(&user_id).unwrap();
            assert_eq!(repaired.realm, GLOBAL_REALM);
            assert!(realm_authorizes_host(
                &repaired.realm,
                "manage.softagen.com"
            ));
        });
    }

    /// Widening is for addresses the operator named, and nothing else. An
    /// ordinary sign-in still observes a host, and observing a second one does
    /// not move the account — otherwise re-homing an account would be as easy
    /// as visiting a different URL.
    #[test]
    fn an_ordinary_account_is_still_pinned_to_the_host_it_signed_in_on() {
        if should_skip_db_tests() {
            return;
        }
        setup_db();
        let rt = get_runtime();

        rt.block_on(async {
            let bootstrap_admins = vec!["someone-else@example.com".to_string()];
            let provider_user_id = format!("google_ordinary_{}", uuid::Uuid::new_v4());

            let user_id = upsert_user_with_bootstrap(
                "player@example.com".to_string(),
                None,
                "google".to_string(),
                provider_user_id.clone(),
                &bootstrap_admins,
                "world.softagen.com".to_string(),
            )
            .await
            .unwrap();
            assert_eq!(get_user(&user_id).unwrap().realm, "world.softagen.com");

            upsert_user_with_bootstrap(
                "player@example.com".to_string(),
                None,
                "google".to_string(),
                provider_user_id,
                &bootstrap_admins,
                "manage.softagen.com".to_string(),
            )
            .await
            .unwrap();

            let user = get_user(&user_id).unwrap();
            assert_eq!(
                user.realm, "world.softagen.com",
                "signing in on the management host must not make a player a principal there"
            );
            assert!(!realm_authorizes_host(&user.realm, "manage.softagen.com"));
        });
    }

    #[test]
    fn test_bootstrap_admin_case_insensitive() {
        if should_skip_db_tests() {
            return;
        }
        setup_db();
        let rt = get_runtime();

        rt.block_on(async {
            let bootstrap_admins = vec!["Admin@Example.COM".to_string()];

            // Create user with different case
            let admin_id = upsert_user_with_bootstrap(
                "admin@example.com".to_string(),
                Some("Admin User".to_string()),
                "google".to_string(),
                "google_admin_case".to_string(),
                &bootstrap_admins,
                "test.example.com".to_string(),
            )
            .await
            .unwrap();

            // Should still get admin role (case-insensitive comparison)
            let admin_user = get_user(&admin_id).unwrap();
            assert!(admin_user.has_role(&UserRole::Administrator));
        });
    }
}
