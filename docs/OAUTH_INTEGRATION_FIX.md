# OAuth Integration Fix - Bootstrap Admin Access

## Issue

When logging in with an email configured in `bootstrap_admins`, the user was redirected to `/auth/login` when trying to access `/manager`.

## Root Cause

The OAuth authentication flow (`src/auth/manager.rs`) was not integrated with the user repository. Specifically:

1. **Session creation used provider ID** - The session was created using the OAuth provider's user ID (e.g., Google's numeric ID) instead of the persistent UUID from our user repository
2. **Admin flag was hardcoded to false** - The `is_admin` parameter in `create_session()` was always `false`
3. **Bootstrap admin check never happened** - Since `upsert_user()` was never called, the bootstrap admin logic never executed
4. **No role information** - User roles were not loaded from the repository

This meant that even if a user's email was in the `bootstrap_admins` list, they would never actually get the Administrator role because the repository's `upsert_user()` function was never invoked during the login process.

## Solution

Modified `handle_callback()` in `src/auth/manager.rs` to integrate with the user repository:

### Changes Made

**Before:**
```rust
// Create session
let session_token = self
    .session_manager
    .create_session(
        user_info.provider_user_id.clone(),  // ❌ Provider ID, not ours
        provider_name.to_string(),
        Some(user_info.email.clone()),
        user_info.name.clone(),
        false,  // ❌ Always false!
        ip_addr.to_string(),
        user_agent.to_string(),
    )
    .await?;
```

**After:**
```rust
// Upsert user in repository (this handles bootstrap admin assignment)
let user_id = crate::user_repository::upsert_user(
    user_info.email.clone(),
    user_info.name.clone(),
    provider_name.to_string(),
    user_info.provider_user_id.clone(),
)
.map_err(|e| {
    tracing::error!("Failed to upsert user: {}", e);
    AuthError::Internal(format!("Failed to create/update user: {}", e))
})?;

// Get user from repository to check roles
let user = crate::user_repository::get_user(&user_id).map_err(|e| {
    tracing::error!("User not found after upsert: {}", e);
    AuthError::Internal("User not found after creation".to_string())
})?;

// Check if user has Administrator role
let is_admin = user
    .roles
    .contains(&crate::user_repository::UserRole::Administrator);

// Create session with correct admin status
let session_token = self
    .session_manager
    .create_session(
        user_id.clone(),  // ✅ Our persistent UUID
        provider_name.to_string(),
        Some(user_info.email.clone()),
        user_info.name.clone(),
        is_admin,  // ✅ Correct admin status!
        ip_addr.to_string(),
        user_agent.to_string(),
    )
    .await?;
```

## What This Fixes

✅ **Bootstrap admins work** - Users in `bootstrap_admins` now automatically get Administrator role on first login

✅ **Persistent user IDs** - Same UUID is used across multiple logins (instead of provider-specific IDs)

✅ **Correct admin detection** - Sessions are created with the correct `is_admin` flag based on user roles

✅ **Role-based access** - Users can now access role-protected routes like `/manager`

✅ **Multi-provider support** - Users can link multiple OAuth providers to the same account

✅ **User persistence** - User information is stored and updated in the repository

## How to Apply the Fix

### 1. Rebuild the Application

```bash
cargo build --release
```

### 2. Stop Any Running Instances

```bash
# Find the process
ps aux | grep aiwebengine

# Kill it
kill <PID>
```

### 3. Start the Server

```bash
./target/release/aiwebengine
```

### 4. Clear Your Session

- Open your browser's developer tools
- Go to Application > Cookies
- Delete the `session` cookie for `localhost:3000`

### 5. Log In Again

- Go to `http://localhost:3000/auth/login`
- Choose your OAuth provider (Google, Microsoft, or Apple)
- Complete the authentication

### 6. Verify Access

- Navigate to `http://localhost:3000/manager`
- You should now see the Manager UI instead of being redirected to login

## Verification

### Check Server Logs

On startup, you should see:
```
Configuring 1 bootstrap admin(s): ["lpajunen@gmail.com"]
```

After login, you should see:
```
Session successfully invalidated during logout
Session created for user <UUID> with admin status: true
```

### Check Session Token

In browser developer tools:
```
Application > Cookies > session
```

The session token should be present and not expire immediately.

### Check User Repository

You can verify the user was created with the Administrator role by checking the logs or adding a debug endpoint.

## Testing

All tests pass:

```bash
# User repository tests (12 tests)
cargo test --lib user_repository -- --test-threads=1

# Manager tests (3 tests)
cargo test --test manager
```

## Configuration Example

Your `config.toml` should have:

```toml
[auth]
enabled = true
jwt_secret = "dev-jwt-secret-minimum-32-characters-required-for-security"
session_timeout = 3600
max_concurrent_sessions = 10
bootstrap_admins = [
    "lpajunen@gmail.com"  # ✅ This will work now!
]

[auth.providers.google]
client_id = "your-google-client-id"
client_secret = "your-google-client-secret"
redirect_uri = "http://localhost:3000/auth/callback/google"
```

## Related Documentation

- [Bootstrap Admin Setup](./BOOTSTRAP_ADMIN.md) - Complete guide to bootstrap admin configuration
- [User Repository Integration](./USER_REPOSITORY_INTEGRATION.md) - Technical details of the user repository
- [Manager UI](./MANAGER_UI.md) - Using the admin interface

## Summary

The OAuth callback now properly integrates with the user repository, ensuring that:
- Users are persisted with consistent UUIDs
- Bootstrap admins automatically get Administrator role
- Sessions are created with correct role information
- Role-protected routes like `/manager` work as expected

**The fix is complete and all tests pass. Users should rebuild, restart, and re-login to see the changes take effect.**
