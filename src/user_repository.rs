use anyhow::Result;
use serde::{Deserialize, Serialize};
use sqlx::{PgPool, Row};
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock, PoisonError};
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
    /// User's email address
    pub email: String,
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
}

impl User {
    /// Create a new user with default authenticated role
    pub fn new(
        email: String,
        name: Option<String>,
        provider_name: String,
        provider_user_id: String,
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
        email: String,
        name: Option<String>,
        provider_name: String,
        provider_user_id: String,
    ) {
        // Update email and name if provided
        self.email = email;
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

/// Lookup key for finding users by provider
#[derive(Debug, Clone, Hash, Eq, PartialEq)]
struct ProviderKey {
    provider_name: String,
    provider_user_id: String,
}

/// In-memory user repository
static USERS: OnceLock<Mutex<HashMap<String, User>>> = OnceLock::new();
static USER_PROVIDER_INDEX: OnceLock<Mutex<HashMap<ProviderKey, String>>> = OnceLock::new();

/// Safe mutex access with recovery from poisoned state
fn safe_lock_users()
-> Result<std::sync::MutexGuard<'static, HashMap<String, User>>, UserRepositoryError> {
    let store = USERS.get_or_init(|| Mutex::new(HashMap::new()));

    match store.lock() {
        Ok(guard) => Ok(guard),
        Err(PoisonError { .. }) => {
            warn!("Users mutex was poisoned, recovering with new data");
            store.lock().map_err(|e| {
                error!("Failed to recover from poisoned mutex: {}", e);
                UserRepositoryError::LockError(format!("Unrecoverable mutex poisoning: {}", e))
            })
        }
    }
}

fn safe_lock_provider_index()
-> Result<std::sync::MutexGuard<'static, HashMap<ProviderKey, String>>, UserRepositoryError> {
    let store = USER_PROVIDER_INDEX.get_or_init(|| Mutex::new(HashMap::new()));

    match store.lock() {
        Ok(guard) => Ok(guard),
        Err(PoisonError { .. }) => {
            warn!("User provider index mutex was poisoned, recovering");
            store.lock().map_err(|e| {
                error!(
                    "Failed to recover from poisoned provider index mutex: {}",
                    e
                );
                UserRepositoryError::LockError(format!("Unrecoverable mutex poisoning: {}", e))
            })
        }
    }
}

/// Get database pool if available
fn get_db_pool() -> Option<std::sync::Arc<crate::database::Database>> {
    crate::database::get_global_database()
}

/// Convert chrono::DateTime to SystemTime
fn datetime_to_system_time(dt: chrono::DateTime<chrono::Utc>) -> SystemTime {
    std::time::UNIX_EPOCH + std::time::Duration::from_secs(dt.timestamp() as u64)
}

/// Database-backed upsert user
async fn db_upsert_user(
    pool: &PgPool,
    email: &str,
    name: Option<&str>,
    provider_name: &str,
    provider_user_id: &str,
    is_admin: bool,
    is_editor: bool,
) -> Result<String, UserRepositoryError> {
    let now = chrono::Utc::now();

    // Try to update existing user first
    let update_result = sqlx::query(
        r#"
        UPDATE users
        SET email = $1, name = $2, is_admin = $3, is_editor = $4, updated_at = $5, last_login_at = $5
        WHERE provider = $6 AND provider_user_id = $7
        RETURNING user_id
        "#,
    )
    .bind(email)
    .bind(name)
    .bind(is_admin)
    .bind(is_editor)
    .bind(now)
    .bind(provider_name)
    .bind(provider_user_id)
    .fetch_optional(pool)
    .await
    .map_err(|e| {
        error!("Database error updating user: {}", e);
        UserRepositoryError::InvalidData(format!("Database error: {}", e))
    })?;

    if let Some(row) = update_result {
        let user_id: String = row.try_get("user_id").map_err(|e| {
            error!("Database error getting user_id: {}", e);
            UserRepositoryError::InvalidData(format!("Database error: {}", e))
        })?;
        debug!("Updated existing user in database: {}", user_id);
        return Ok(user_id);
    }

    // User doesn't exist, create new one
    let user_id = uuid::Uuid::new_v4().to_string();

    sqlx::query(
        r#"
        INSERT INTO users (user_id, email, name, provider, provider_user_id, is_admin, is_editor, created_at, updated_at, last_login_at)
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $8, $8)
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
    .execute(pool)
    .await
    .map_err(|e| {
        error!("Database error creating user: {}", e);
        UserRepositoryError::InvalidData(format!("Database error: {}", e))
    })?;

    debug!("Created new user in database: {}", user_id);
    Ok(user_id)
}

/// Database-backed get user
async fn db_get_user(pool: &PgPool, user_id: &str) -> Result<User, UserRepositoryError> {
    let row = sqlx::query(
        r#"
        SELECT user_id, email, name, provider, provider_user_id, is_admin, is_editor, created_at, updated_at
        FROM users
        WHERE user_id = $1
        "#,
    )
    .bind(user_id)
    .fetch_optional(pool)
    .await
    .map_err(|e| {
        error!("Database error getting user: {}", e);
        UserRepositoryError::InvalidData(format!("Database error: {}", e))
    })?
    .ok_or_else(|| UserRepositoryError::UserNotFound(user_id.to_string()))?;

    let db_user_id: String = row
        .try_get("user_id")
        .map_err(|e| UserRepositoryError::InvalidData(e.to_string()))?;
    let email: String = row
        .try_get("email")
        .map_err(|e| UserRepositoryError::InvalidData(e.to_string()))?;
    let name: Option<String> = row
        .try_get("name")
        .map_err(|e| UserRepositoryError::InvalidData(e.to_string()))?;
    let provider: String = row
        .try_get("provider")
        .map_err(|e| UserRepositoryError::InvalidData(e.to_string()))?;
    let provider_user_id: String = row
        .try_get("provider_user_id")
        .map_err(|e| UserRepositoryError::InvalidData(e.to_string()))?;
    let is_admin: bool = row
        .try_get("is_admin")
        .map_err(|e| UserRepositoryError::InvalidData(e.to_string()))?;
    let is_editor: bool = row
        .try_get("is_editor")
        .map_err(|e| UserRepositoryError::InvalidData(e.to_string()))?;
    let created_at: chrono::DateTime<chrono::Utc> = row
        .try_get("created_at")
        .map_err(|e| UserRepositoryError::InvalidData(e.to_string()))?;
    let updated_at: chrono::DateTime<chrono::Utc> = row
        .try_get("updated_at")
        .map_err(|e| UserRepositoryError::InvalidData(e.to_string()))?;

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
    })
}

/// Database-backed find user by provider
async fn db_find_user_by_provider(
    pool: &PgPool,
    provider_name: &str,
    provider_user_id: &str,
) -> Result<Option<User>, UserRepositoryError> {
    let row = sqlx::query(
        r#"
        SELECT user_id, email, name, provider, provider_user_id, is_admin, is_editor, created_at, updated_at
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
        UserRepositoryError::InvalidData(format!("Database error: {}", e))
    })?;

    if let Some(row) = row {
        let db_user_id: String = row
            .try_get("user_id")
            .map_err(|e| UserRepositoryError::InvalidData(e.to_string()))?;
        let email: String = row
            .try_get("email")
            .map_err(|e| UserRepositoryError::InvalidData(e.to_string()))?;
        let name: Option<String> = row
            .try_get("name")
            .map_err(|e| UserRepositoryError::InvalidData(e.to_string()))?;
        let provider: String = row
            .try_get("provider")
            .map_err(|e| UserRepositoryError::InvalidData(e.to_string()))?;
        let provider_user_id: String = row
            .try_get("provider_user_id")
            .map_err(|e| UserRepositoryError::InvalidData(e.to_string()))?;
        let is_admin: bool = row
            .try_get("is_admin")
            .map_err(|e| UserRepositoryError::InvalidData(e.to_string()))?;
        let is_editor: bool = row
            .try_get("is_editor")
            .map_err(|e| UserRepositoryError::InvalidData(e.to_string()))?;
        let created_at: chrono::DateTime<chrono::Utc> = row
            .try_get("created_at")
            .map_err(|e| UserRepositoryError::InvalidData(e.to_string()))?;
        let updated_at: chrono::DateTime<chrono::Utc> = row
            .try_get("updated_at")
            .map_err(|e| UserRepositoryError::InvalidData(e.to_string()))?;

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
pub fn upsert_user(
    email: String,
    name: Option<String>,
    provider_name: String,
    provider_user_id: String,
) -> Result<String, UserRepositoryError> {
    let bootstrap_admins = get_bootstrap_admins();
    upsert_user_with_bootstrap(
        email,
        name,
        provider_name,
        provider_user_id,
        bootstrap_admins,
    )
}

/// Upsert a user with bootstrap admin configuration
///
/// This is the internal implementation that supports bootstrap admins.
/// If the user's email matches one in the bootstrap_admins list, they automatically
/// get the Administrator role on creation.
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
pub fn upsert_user_with_bootstrap(
    email: String,
    name: Option<String>,
    provider_name: String,
    provider_user_id: String,
    bootstrap_admins: &[String],
) -> Result<String, UserRepositoryError> {
    // Validate inputs
    if email.trim().is_empty() {
        return Err(UserRepositoryError::InvalidData(
            "Email cannot be empty".to_string(),
        ));
    }

    if provider_name.trim().is_empty() {
        return Err(UserRepositoryError::InvalidData(
            "Provider name cannot be empty".to_string(),
        ));
    }

    if provider_user_id.trim().is_empty() {
        return Err(UserRepositoryError::InvalidData(
            "Provider user ID cannot be empty".to_string(),
        ));
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

    // Try database first
    if let Some(db) = get_db_pool() {
        // Use tokio::task::block_in_place to run async code in sync context
        let result = tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(async {
                db_upsert_user(
                    db.pool(),
                    &email,
                    name.as_deref(),
                    &provider_name,
                    &provider_user_id,
                    is_admin,
                    is_editor,
                )
                .await
            })
        });

        match result {
            Ok(user_id) => {
                // Also update in-memory cache for consistency
                let _ = upsert_user_in_memory(
                    email.clone(),
                    name.clone(),
                    provider_name.clone(),
                    provider_user_id.clone(),
                    bootstrap_admins,
                );
                return Ok(user_id);
            }
            Err(e) => {
                warn!("Database upsert failed, falling back to in-memory: {}", e);
                // Fall through to in-memory implementation
            }
        }
    }

    // Fall back to in-memory implementation
    upsert_user_in_memory(
        email,
        name,
        provider_name,
        provider_user_id,
        bootstrap_admins,
    )
}

/// In-memory implementation of upsert user (existing logic)
fn upsert_user_in_memory(
    email: String,
    name: Option<String>,
    provider_name: String,
    provider_user_id: String,
    bootstrap_admins: &[String],
) -> Result<String, UserRepositoryError> {
    let provider_key = ProviderKey {
        provider_name: provider_name.clone(),
        provider_user_id: provider_user_id.clone(),
    };

    // Check if user already exists for this provider
    let mut provider_index = safe_lock_provider_index()?;
    let mut users = safe_lock_users()?;

    if let Some(user_id) = provider_index.get(&provider_key) {
        // User exists - update their information
        if let Some(user) = users.get_mut(user_id) {
            user.update_from_auth(email, name, provider_name, provider_user_id);
            debug!("Updated existing user: {}", user_id);
            return Ok(user_id.clone());
        } else {
            // Index is out of sync - clean it up
            warn!("Provider index out of sync, removing stale entry");
            provider_index.remove(&provider_key);
        }
    }

    // Create new user
    let mut user = User::new(email.clone(), name, provider_name, provider_user_id);

    // Check if this email is in the bootstrap admins list
    let email_lower = email.to_lowercase();
    let is_bootstrap_admin = bootstrap_admins
        .iter()
        .any(|admin_email| admin_email.to_lowercase() == email_lower);

    if is_bootstrap_admin {
        // Automatically grant Administrator role to bootstrap admins
        user.add_role(UserRole::Administrator);
        debug!(
            "Granted Administrator role to bootstrap admin: {} ({})",
            user.id, email
        );
    }

    let user_id = user.id.clone();

    // Store user
    users.insert(user_id.clone(), user);
    provider_index.insert(provider_key, user_id.clone());

    debug!("Created new user: {} ({})", user_id, email);
    Ok(user_id)
}

/// Get a user by their internal ID
pub fn get_user(user_id: &str) -> Result<User, UserRepositoryError> {
    // Try database first
    if let Some(db) = get_db_pool() {
        let result = tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current()
                .block_on(async { db_get_user(db.pool(), user_id).await })
        });

        match result {
            Ok(user) => return Ok(user),
            Err(UserRepositoryError::UserNotFound(_)) => {
                // User not found in database, fall back to in-memory
            }
            Err(e) => {
                warn!("Database get_user failed, falling back to in-memory: {}", e);
            }
        }
    }

    // Fall back to in-memory
    let users = safe_lock_users()?;
    users
        .get(user_id)
        .cloned()
        .ok_or_else(|| UserRepositoryError::UserNotFound(user_id.to_string()))
}

/// Find a user by provider credentials
pub fn find_user_by_provider(
    provider_name: &str,
    provider_user_id: &str,
) -> Result<Option<User>, UserRepositoryError> {
    // Try database first
    if let Some(db) = get_db_pool() {
        let result = tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(async {
                db_find_user_by_provider(db.pool(), provider_name, provider_user_id).await
            })
        });

        match result {
            Ok(user) => return Ok(user),
            Err(e) => {
                warn!(
                    "Database find_user_by_provider failed, falling back to in-memory: {}",
                    e
                );
            }
        }
    }

    // Fall back to in-memory
    let provider_key = ProviderKey {
        provider_name: provider_name.to_string(),
        provider_user_id: provider_user_id.to_string(),
    };

    let provider_index = safe_lock_provider_index()?;
    let users = safe_lock_users()?;

    if let Some(user_id) = provider_index.get(&provider_key) {
        Ok(users.get(user_id).cloned())
    } else {
        Ok(None)
    }
}

/// Database-backed update user roles
async fn db_update_user_roles(
    pool: &PgPool,
    user_id: &str,
    is_admin: bool,
    is_editor: bool,
) -> Result<(), UserRepositoryError> {
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
        UserRepositoryError::InvalidData(format!("Database error: {}", e))
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
pub fn update_user_roles(user_id: &str, roles: Vec<UserRole>) -> Result<(), UserRepositoryError> {
    // Calculate boolean flags from roles
    let is_admin = roles.iter().any(|r| matches!(r, UserRole::Administrator));
    let is_editor = roles.iter().any(|r| matches!(r, UserRole::Editor));

    // Update database first
    if let Some(db) = get_db_pool() {
        let result = tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(async {
                db_update_user_roles(db.pool(), user_id, is_admin, is_editor).await
            })
        });

        match result {
            Ok(()) => {
                // Also update in-memory cache for consistency
                let mut users = safe_lock_users()?;
                if let Some(user) = users.get_mut(user_id) {
                    // Ensure Authenticated role is always present
                    let mut new_roles = roles;
                    if !new_roles
                        .iter()
                        .any(|r| matches!(r, UserRole::Authenticated))
                    {
                        new_roles.push(UserRole::Authenticated);
                    }

                    user.roles = new_roles;
                    user.updated_at = SystemTime::now();
                    debug!("Updated roles for user: {}", user_id);
                    return Ok(());
                } else {
                    warn!(
                        "User {} not found in memory cache after database update",
                        user_id
                    );
                }
            }
            Err(e) => {
                warn!(
                    "Database update_user_roles failed, falling back to in-memory: {}",
                    e
                );
            }
        }
    }

    // Fall back to in-memory only
    let mut users = safe_lock_users()?;

    let user = users
        .get_mut(user_id)
        .ok_or_else(|| UserRepositoryError::UserNotFound(user_id.to_string()))?;

    // Ensure Authenticated role is always present
    let mut new_roles = roles;
    if !new_roles
        .iter()
        .any(|r| matches!(r, UserRole::Authenticated))
    {
        new_roles.push(UserRole::Authenticated);
    }

    user.roles = new_roles;
    user.updated_at = SystemTime::now();

    debug!("Updated roles for user (in-memory only): {}", user_id);
    Ok(())
}

/// Add a role to a user
pub fn add_user_role(user_id: &str, role: UserRole) -> Result<(), UserRepositoryError> {
    // Get current user to determine new role flags
    let current_user = get_user(user_id)?;
    let mut new_roles = current_user.roles.clone();
    new_roles.push(role);

    // Remove duplicates
    new_roles.sort_by(|a, b| format!("{:?}", a).cmp(&format!("{:?}", b)));
    new_roles.dedup();

    // Update with new roles
    update_user_roles(user_id, new_roles)
}

/// Remove a role from a user
pub fn remove_user_role(user_id: &str, role: &UserRole) -> Result<(), UserRepositoryError> {
    // Don't allow removing Authenticated role
    if matches!(role, UserRole::Authenticated) {
        return Err(UserRepositoryError::InvalidData(
            "Cannot remove Authenticated role".to_string(),
        ));
    }

    // Get current user to determine new role flags
    let current_user = get_user(user_id)?;
    let mut new_roles = current_user.roles.clone();
    new_roles.retain(|r| r != role);

    // Update with new roles
    update_user_roles(user_id, new_roles)
}

/// List all users (for admin purposes)
pub fn list_users() -> Result<Vec<User>, UserRepositoryError> {
    // Try database first
    if let Some(db) = get_db_pool() {
        let result: Result<Vec<User>, UserRepositoryError> = tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(async {
                let pool = db.pool();
                let rows = sqlx::query(
                    r#"
                    SELECT user_id, email, name, provider, provider_user_id, is_admin, is_editor, created_at, updated_at
                    FROM users
                    ORDER BY created_at DESC
                    "#,
                )
                .fetch_all(pool)
                .await
                .map_err(|e| {
                    error!("Database error listing users: {}", e);
                    UserRepositoryError::InvalidData(format!("Database error: {}", e))
                })?;

                let mut users = Vec::new();
                for row in rows {
                    let db_user_id: String = row
                        .try_get("user_id")
                        .map_err(|e| UserRepositoryError::InvalidData(e.to_string()))?;
                    let email: String = row
                        .try_get("email")
                        .map_err(|e| UserRepositoryError::InvalidData(e.to_string()))?;
                    let name: Option<String> = row
                        .try_get("name")
                        .map_err(|e| UserRepositoryError::InvalidData(e.to_string()))?;
                    let provider: String = row
                        .try_get("provider")
                        .map_err(|e| UserRepositoryError::InvalidData(e.to_string()))?;
                    let provider_user_id: String = row
                        .try_get("provider_user_id")
                        .map_err(|e| UserRepositoryError::InvalidData(e.to_string()))?;
                    let is_admin: bool = row
                        .try_get("is_admin")
                        .map_err(|e| UserRepositoryError::InvalidData(e.to_string()))?;
                    let is_editor: bool = row
                        .try_get("is_editor")
                        .map_err(|e| UserRepositoryError::InvalidData(e.to_string()))?;
                    let created_at: chrono::DateTime<chrono::Utc> = row
                        .try_get("created_at")
                        .map_err(|e| UserRepositoryError::InvalidData(e.to_string()))?;
                    let updated_at: chrono::DateTime<chrono::Utc> = row
                        .try_get("updated_at")
                        .map_err(|e| UserRepositoryError::InvalidData(e.to_string()))?;

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

                    let user = User {
                        id: db_user_id.clone(),
                        email,
                        name,
                        roles,
                        created_at: datetime_to_system_time(created_at),
                        updated_at: datetime_to_system_time(updated_at),
                        providers,
                    };

                    // Also update in-memory cache for consistency
                    let mut users_cache = safe_lock_users()?;
                    let mut provider_index = safe_lock_provider_index()?;
                    users_cache.insert(db_user_id, user.clone());
                    provider_index.insert(
                        ProviderKey {
                            provider_name: user.providers[0].provider_name.clone(),
                            provider_user_id: user.providers[0].provider_user_id.clone(),
                        },
                        user.id.clone(),
                    );

                    users.push(user);
                }

                Ok(users)
            })
        });

        match result {
            Ok(users) => return Ok(users),
            Err(e) => {
                warn!(
                    "Database list_users failed, falling back to in-memory: {}",
                    e
                );
            }
        }
    }

    // Fall back to in-memory
    let users = safe_lock_users()?;
    Ok(users.values().cloned().collect())
}

/// Get user count
pub fn get_user_count() -> Result<usize, UserRepositoryError> {
    let users = safe_lock_users()?;
    Ok(users.len())
}

/// Delete a user (for testing/admin purposes)
pub fn delete_user(user_id: &str) -> Result<bool, UserRepositoryError> {
    let mut users = safe_lock_users()?;
    let mut provider_index = safe_lock_provider_index()?;

    // Remove user
    if let Some(user) = users.remove(user_id) {
        // Remove all provider index entries for this user
        for provider_info in user.providers {
            let provider_key = ProviderKey {
                provider_name: provider_info.provider_name,
                provider_user_id: provider_info.provider_user_id,
            };
            provider_index.remove(&provider_key);
        }

        debug!("Deleted user: {}", user_id);
        Ok(true)
    } else {
        Ok(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    // Use a global mutex to serialize tests that access global state
    static TEST_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn test_user_creation() {
        let user = User::new(
            "test@example.com".to_string(),
            Some("Test User".to_string()),
            "google".to_string(),
            "google123".to_string(),
        );

        assert_eq!(user.email, "test@example.com");
        assert_eq!(user.name, Some("Test User".to_string()));
        assert_eq!(user.roles.len(), 1);
        assert_eq!(user.roles[0], UserRole::Authenticated);
        assert_eq!(user.providers.len(), 1);
    }

    #[test]
    fn test_upsert_user_new() {
        let _lock = TEST_LOCK.lock().unwrap();

        let user_id = upsert_user(
            "new@example.com".to_string(),
            Some("New User".to_string()),
            "github".to_string(),
            "github456".to_string(),
        )
        .unwrap();

        let user = get_user(&user_id).unwrap();
        assert_eq!(user.email, "new@example.com");
        assert_eq!(user.name, Some("New User".to_string()));
    }

    #[test]
    fn test_upsert_user_existing() {
        let _lock = TEST_LOCK.lock().unwrap();

        // First insert
        let user_id1 = upsert_user(
            "existing@example.com".to_string(),
            Some("Existing User".to_string()),
            "google".to_string(),
            "google789".to_string(),
        )
        .unwrap();

        // Second insert with same provider credentials
        let user_id2 = upsert_user(
            "updated@example.com".to_string(),
            Some("Updated User".to_string()),
            "google".to_string(),
            "google789".to_string(),
        )
        .unwrap();

        // Should return the same user ID
        assert_eq!(user_id1, user_id2);

        // User info should be updated
        let user = get_user(&user_id1).unwrap();
        assert_eq!(user.email, "updated@example.com");
        assert_eq!(user.name, Some("Updated User".to_string()));
    }

    #[test]
    fn test_role_management() {
        let _lock = TEST_LOCK.lock().unwrap();

        let user_id = upsert_user(
            "roles@example.com".to_string(),
            None,
            "google".to_string(),
            "google_roles".to_string(),
        )
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
    }

    #[test]
    fn test_role_privileges() {
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
        let _lock = TEST_LOCK.lock().unwrap();

        let user_id = upsert_user(
            "auth@example.com".to_string(),
            None,
            "google".to_string(),
            "google_auth".to_string(),
        )
        .unwrap();

        let result = remove_user_role(&user_id, &UserRole::Authenticated);
        assert!(result.is_err());
    }

    #[test]
    fn test_find_user_by_provider() {
        let _lock = TEST_LOCK.lock().unwrap();

        let user_id = upsert_user(
            "provider@example.com".to_string(),
            None,
            "github".to_string(),
            "github_provider".to_string(),
        )
        .unwrap();

        let found = find_user_by_provider("github", "github_provider")
            .unwrap()
            .unwrap();
        assert_eq!(found.id, user_id);

        let not_found = find_user_by_provider("github", "nonexistent").unwrap();
        assert!(not_found.is_none());
    }

    #[test]
    fn test_update_user_roles() {
        let _lock = TEST_LOCK.lock().unwrap();

        let user_id = upsert_user(
            "update@example.com".to_string(),
            None,
            "google".to_string(),
            "google_update".to_string(),
        )
        .unwrap();

        // Set roles to Editor and Administrator
        update_user_roles(&user_id, vec![UserRole::Editor, UserRole::Administrator]).unwrap();

        let user = get_user(&user_id).unwrap();
        assert_eq!(user.roles.len(), 3); // Authenticated is auto-added
        assert!(user.has_role(&UserRole::Authenticated));
        assert!(user.has_role(&UserRole::Editor));
        assert!(user.has_role(&UserRole::Administrator));
    }

    #[test]
    fn test_delete_user() {
        let _lock = TEST_LOCK.lock().unwrap();

        let user_id = upsert_user(
            "delete@example.com".to_string(),
            None,
            "google".to_string(),
            "google_delete".to_string(),
        )
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
    }

    #[test]
    fn test_validation() {
        // Empty email
        assert!(
            upsert_user(
                "".to_string(),
                None,
                "google".to_string(),
                "user123".to_string()
            )
            .is_err()
        );

        // Empty provider name
        assert!(
            upsert_user(
                "user@example.com".to_string(),
                None,
                "".to_string(),
                "user123".to_string()
            )
            .is_err()
        );

        // Empty provider user ID
        assert!(
            upsert_user(
                "user@example.com".to_string(),
                None,
                "google".to_string(),
                "".to_string()
            )
            .is_err()
        );
    }

    #[test]
    fn test_bootstrap_admin() {
        let _lock = TEST_LOCK.lock().unwrap();

        let bootstrap_admins = vec!["admin@example.com".to_string()];

        // Create user with bootstrap admin email
        let admin_id = upsert_user_with_bootstrap(
            "admin@example.com".to_string(),
            Some("Admin User".to_string()),
            "google".to_string(),
            "google_admin".to_string(),
            &bootstrap_admins,
        )
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
        )
        .unwrap();

        // User should NOT have Administrator role
        let regular_user = get_user(&user_id).unwrap();
        assert!(!regular_user.has_role(&UserRole::Administrator));
        assert!(regular_user.has_role(&UserRole::Authenticated));
    }

    #[test]
    fn test_bootstrap_admin_case_insensitive() {
        let _lock = TEST_LOCK.lock().unwrap();

        let bootstrap_admins = vec!["Admin@Example.COM".to_string()];

        // Create user with different case
        let admin_id = upsert_user_with_bootstrap(
            "admin@example.com".to_string(),
            Some("Admin User".to_string()),
            "google".to_string(),
            "google_admin_case".to_string(),
            &bootstrap_admins,
        )
        .unwrap();

        // Should still get admin role (case-insensitive comparison)
        let admin_user = get_user(&admin_id).unwrap();
        assert!(admin_user.has_role(&UserRole::Administrator));
    }
}
