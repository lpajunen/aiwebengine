# User Repository Integration Guide

## Overview

The `user_repository` module provides persistent user management for the authentication system. It stores user data, manages user roles, and ensures users get consistent IDs across sign-ins.

**🎉 As of the latest update, the user repository is now fully integrated with the OAuth authentication flow!** When users log in, their information is automatically saved, bootstrap admins are auto-promoted, and sessions are created with the correct role information.

## Features

### 1. User Management

- **Automatic ID Generation**: UUIDs are automatically generated for new users
- **Upsert Logic**: Returns existing user ID on subsequent sign-ins with the same provider credentials
- **Multi-Provider Support**: Users can authenticate with multiple OAuth providers
- **Provider Tracking**: Stores first and last authentication times per provider

### 2. Role Management

Three built-in roles with hierarchical privileges:

- **Authenticated**: Basic authenticated user (always present)
- **Editor**: User with editor privileges
- **Administrator**: User with full administrator privileges

### 3. Thread-Safe Operations

All operations use mutex-protected global state with poisoned state recovery.

## API Reference

### Core Functions

#### `upsert_user`

```rust
pub fn upsert_user(
    email: String,
    name: Option<String>,
    provider_name: String,
    provider_user_id: String,
) -> Result<String, UserRepositoryError>
```

**Purpose**: Insert or update a user based on provider authentication.

**Returns**: User ID (either existing or newly created)

**Example**:

```rust
use aiwebengine::user_repository;

let user_id = user_repository::upsert_user(
    "user@example.com".to_string(),
    Some("John Doe".to_string()),
    "google".to_string(),
    "google_123456".to_string(),
)?;
```

#### `get_user`

```rust
pub fn get_user(user_id: &str) -> Result<User, UserRepositoryError>
```

**Purpose**: Retrieve a user by their internal ID.

**Example**:

```rust
let user = user_repository::get_user(&user_id)?;
println!("Email: {}", user.email);
println!("Has Admin: {}", user.has_role(&UserRole::Administrator));
```

#### `find_user_by_provider`

```rust
pub fn find_user_by_provider(
    provider_name: &str,
    provider_user_id: &str,
) -> Result<Option<User>, UserRepositoryError>
```

**Purpose**: Find a user by their provider credentials.

**Example**:

```rust
if let Some(user) = user_repository::find_user_by_provider("google", "google_123456")? {
    println!("Found existing user: {}", user.id);
}
```

### Role Management Functions

#### `add_user_role`

```rust
pub fn add_user_role(user_id: &str, role: UserRole) -> Result<(), UserRepositoryError>
```

**Purpose**: Add a role to a user (idempotent).

**Example**:

```rust
user_repository::add_user_role(&user_id, UserRole::Editor)?;
```

#### `remove_user_role`

```rust
pub fn remove_user_role(user_id: &str, role: &UserRole) -> Result<(), UserRepositoryError>
```

**Purpose**: Remove a role from a user. Cannot remove `Authenticated` role.

**Example**:

```rust
user_repository::remove_user_role(&user_id, &UserRole::Editor)?;
```

#### `update_user_roles`

```rust
pub fn update_user_roles(
    user_id: &str,
    roles: Vec<UserRole>,
) -> Result<(), UserRepositoryError>
```

**Purpose**: Replace all user roles. Automatically ensures `Authenticated` role is present.

**Example**:

```rust
user_repository::update_user_roles(
    &user_id,
    vec![UserRole::Editor, UserRole::Administrator],
)?;
```

### Utility Functions

#### `list_users`

```rust
pub fn list_users() -> Result<Vec<User>, UserRepositoryError>
```

**Purpose**: Get all users (for admin purposes).

#### `get_user_count`

```rust
pub fn get_user_count() -> Result<usize, UserRepositoryError>
```

**Purpose**: Get the total number of users.

#### `delete_user`

```rust
pub fn delete_user(user_id: &str) -> Result<bool, UserRepositoryError>
```

**Purpose**: Delete a user and clean up all provider index entries.

## Integration with Authentication System

### Step 1: Update OAuth Callback Handler

In `src/auth/routes.rs`, update the callback handler to use `upsert_user`:

```rust
use crate::user_repository;

async fn handle_callback(
    State(auth_manager): State<Arc<AuthManager>>,
    Query(params): Query<CallbackParams>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
) -> Result<impl IntoResponse, AuthError> {
    // ... existing code to validate state and exchange tokens ...

    // Get user info from provider
    let user_info = provider.get_user_info(&token_response.access_token).await?;

    // Upsert user to get consistent user ID
    let user_id = user_repository::upsert_user(
        user_info.email.clone().unwrap_or_default(),
        user_info.name.clone(),
        provider_name.to_string(),
        user_info.id.clone(),
    ).map_err(|e| AuthError::Internal(format!("Failed to upsert user: {}", e)))?;

    // Create session with the user ID from repository
    let session_token = auth_manager
        .create_session(
            user_id, // Use repository user ID instead of provider ID
            provider_name.to_string(),
            user_info.email,
            user_info.name,
            false, // is_admin - check from user repository
            addr.ip().to_string(),
            headers
                .get(axum::http::header::USER_AGENT)
                .and_then(|v| v.to_str().ok())
                .unwrap_or("unknown")
                .to_string(),
        )
        .await?;

    // ... rest of the code ...
}
```

### Step 2: Check User Roles in Session Creation

Update the session creation to check user roles from the repository:

```rust
// Get user from repository to check roles
let user = user_repository::get_user(&user_id)
    .map_err(|e| AuthError::Internal(format!("Failed to get user: {}", e)))?;

let is_admin = user.has_role(&user_repository::UserRole::Administrator);

// Create session with correct admin flag
let session_token = auth_manager
    .create_session(
        user_id,
        provider_name.to_string(),
        user_info.email,
        user_info.name,
        is_admin, // Use role from repository
        addr.ip().to_string(),
        user_agent,
    )
    .await?;
```

### Step 3: Expose User Roles in Authentication Context

Update `AuthUser` in `src/auth/middleware.rs` to include roles:

```rust
use crate::user_repository::UserRole;

#[derive(Clone, Debug)]
pub struct AuthUser {
    pub user_id: String,
    pub provider: String,
    pub email: Option<String>,
    pub name: Option<String>,
    pub roles: Vec<UserRole>,
    pub is_admin: bool,
}

impl AuthUser {
    pub fn has_role(&self, role: &UserRole) -> bool {
        self.roles.iter().any(|r| r == role)
    }

    pub fn has_privilege(&self, required: &UserRole) -> bool {
        self.roles.iter().any(|r| r.has_privilege(required))
    }
}
```

## User Data Structure

```rust
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
```

## Role Hierarchy

The role system uses hierarchical privileges:

```rust
Administrator > Editor > Authenticated
```

Example privilege checks:

```rust
// Administrator has all privileges
assert!(UserRole::Administrator.has_privilege(&UserRole::Editor));
assert!(UserRole::Administrator.has_privilege(&UserRole::Authenticated));

// Editor has Editor and Authenticated privileges
assert!(UserRole::Editor.has_privilege(&UserRole::Editor));
assert!(UserRole::Editor.has_privilege(&UserRole::Authenticated));
assert!(!UserRole::Editor.has_privilege(&UserRole::Administrator));

// Authenticated only has Authenticated privilege
assert!(UserRole::Authenticated.has_privilege(&UserRole::Authenticated));
assert!(!UserRole::Authenticated.has_privilege(&UserRole::Editor));
```

## Example: Admin Management Endpoint

Create an admin endpoint to manage user roles:

```javascript
// In a JavaScript handler
function adminUpdateUserRole(request) {
  // This would call a Rust function exposed to JavaScript
  // that interacts with user_repository

  const { userId, role } = JSON.parse(request.body);

  // Check if requester is admin
  if (!request.auth.hasRole("Administrator")) {
    return {
      status: 403,
      body: JSON.stringify({ error: "Forbidden" }),
    };
  }

  // Add role to user
  // (Assuming we expose a function to JavaScript)
  addUserRole(userId, role);

  return {
    status: 200,
    body: JSON.stringify({ success: true }),
  };
}
```

## Best Practices

1. **Always use `upsert_user` during authentication**: This ensures users get consistent IDs across sign-ins.

2. **Check roles from the repository**: Don't rely solely on OAuth provider information for authorization.

3. **Use role hierarchy**: Check privileges with `has_privilege()` rather than exact role matches.

4. **Handle errors gracefully**: All repository functions return `Result` types.

5. **Audit role changes**: Consider logging all role modifications for security auditing.

## Future Enhancements

Potential improvements to consider:

1. **Database Backend**: Replace in-memory storage with persistent database
2. **Role History**: Track role changes over time
3. **User Groups**: Add support for group-based permissions
4. **Custom Roles**: Allow defining custom roles beyond the three built-in ones
5. **Role Expiration**: Add time-based role assignments
6. **User Metadata**: Store additional user attributes and preferences

## Testing

Run the user repository tests:

```bash
cargo test user_repository::tests --lib -- --test-threads=1
```

All tests use a global lock to serialize access to shared state.

## OAuth Integration

### How It Works

The user repository is automatically integrated with the OAuth authentication flow. When a user completes OAuth login:

1. **OAuth Callback Received** - The `handle_callback()` method in `src/auth/manager.rs` is called
2. **User Upserted** - `user_repository::upsert_user()` is called with the user's email, name, provider, and provider ID
3. **Bootstrap Admin Check** - If the email matches the `bootstrap_admins` configuration, the user automatically gets Administrator role
4. **Roles Retrieved** - The user record is fetched to determine the current roles
5. **Session Created** - A session is created with the correct `is_admin` flag based on the user's roles
6. **Access Granted** - The user can now access role-protected resources like `/manager`

### Code Flow

In `src/auth/manager.rs`:

```rust
// After successful OAuth token exchange...

// 1. Upsert user in repository (handles bootstrap admin)
let user_id = crate::user_repository::upsert_user(
    user_info.email.clone(),
    user_info.name.clone(),
    provider_name.to_string(),
    user_info.provider_user_id.clone(),
)?;

// 2. Get user to check roles
let user = crate::user_repository::get_user(&user_id)?;

// 3. Check if user has Administrator role
let is_admin = user.roles.contains(&crate::user_repository::UserRole::Administrator);

// 4. Create session with correct admin status
let session_token = self.session_manager.create_session(
    user_id,          // Our persistent UUID
    provider_name,
    email,
    name,
    is_admin,         // Correct admin status from roles
    ip_addr,
    user_agent,
)?;
```

### Bootstrap Admin Configuration

To make a user an administrator on their first login:

```toml
[auth]
bootstrap_admins = [
    "admin@company.com"
]
```

See [Bootstrap Admin Guide](./BOOTSTRAP_ADMIN.md) for detailed instructions.

### Troubleshooting OAuth Integration

**Problem**: "Redirected to login when accessing /manager"

**Solution**:

1. Ensure your email is in `bootstrap_admins` in `config.toml`
2. Rebuild the server: `cargo build --release`
3. Restart the server
4. Sign out completely (clear cookies)
5. Sign back in with OAuth

**Problem**: "User ID changes on each login"

**Solution**: This was fixed by the integration. The system now uses `user_repository::upsert_user()` which returns the same UUID for each user, regardless of how many times they log in.

**Problem**: "Admin status not reflected in session"

**Solution**: The integration now correctly checks user roles and passes `is_admin` to session creation. Rebuild and restart to get the fix.

## Error Handling

The module provides structured error types:

```rust
pub enum UserRepositoryError {
    LockError(String),      // Mutex lock failures
    UserNotFound(String),   // User doesn't exist
    InvalidData(String),    // Validation errors
}
```

All errors implement `std::error::Error` and can be converted to `anyhow::Error`.
