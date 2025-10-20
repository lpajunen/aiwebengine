# Cookie Name Mismatch Fix

## Issue

User was authenticated (verified by `/auth/status` returning success), but `/editor` returned 401 with:
```json
{
  "debug": {
    "authExists": true,
    "isAuthenticated": false,
    "errorMessage": "Authentication required."
  }
}
```

## Root Cause

**Hardcoded cookie name in middleware did not match configuration.**

The authentication middleware (`src/auth/middleware.rs`) had a hardcoded cookie name:

```rust
// WRONG - hardcoded!
if let Some((name, value)) = cookie.split_once('=')
    && name == "auth_session"  // ← Hardcoded!
```

But the configuration (`config.toml`) specified:

```toml
[auth.cookie]
name = "session"  # ← Different value!
```

### Why /auth/status worked but /editor didn't:

- **`/auth/status`** (Rust route): Directly accesses `auth_manager.config().session_cookie_name` ✅
- **`/editor`** (JavaScript route via middleware): Used hardcoded `"auth_session"` ❌

The middleware was looking for the wrong cookie name, so it never extracted the session token, which meant:
1. `AuthUser` was never added to request extensions
2. `JsAuthContext` was created as anonymous
3. JavaScript `auth.isAuthenticated` was always `false`
4. `auth.requireAuth()` always threw an error

## Solution

Updated `extract_session_token()` to accept the cookie name as a parameter and use the value from config:

### Before:

```rust
fn extract_session_token(req: &Request) -> Option<String> {
    // ...
    && name == "auth_session"  // Hardcoded!
    // ...
}
```

### After:

```rust
fn extract_session_token(req: &Request, cookie_name: &str) -> Option<String> {
    // ...
    && name == cookie_name  // From config!
    // ...
}

pub async fn optional_auth_middleware(
    State(auth_manager): State<Arc<AuthManager>>,
    mut req: Request,
    next: Next,
) -> Response {
    let cookie_name = &auth_manager.config().session_cookie_name;
    if let Some(session_token) = extract_session_token(&req, cookie_name) {
        // ...
    }
}
```

## Files Modified

1. **`src/auth/middleware.rs`**
   - Updated `extract_session_token()` signature to accept `cookie_name` parameter
   - Updated `optional_auth_middleware()` to pass config cookie name
   - Updated `required_auth_middleware()` to pass config cookie name
   - Updated tests to pass cookie name
   - Added enhanced debug logging with emoji indicators

## Testing

- ✅ All unit tests pass
- ✅ All integration tests pass
- ✅ Cookie extraction now respects configuration

## Verification

After this fix, when you access `/editor` with a valid session cookie:

1. Middleware extracts cookie using configured name (`"session"`)
2. Session is validated
3. `AuthUser` is injected into request extensions
4. `JsAuthContext.authenticated` is created
5. JavaScript `auth.isAuthenticated` returns `true`
6. `auth.requireAuth()` succeeds
7. Editor loads successfully ✅

## Debug Logging

The fix also adds comprehensive debug logging to track authentication flow:

- 🔐 Middleware called for path
- 🔑 Session token found (or not)
- ✅ Session validated successfully
- ⚠️ Session validation failed
- ℹ️ No session token found

View these logs with: `RUST_LOG=debug cargo run`

## Related Issues

This same bug would affect ANY JavaScript endpoint that uses `auth.requireAuth()` or checks `auth.isAuthenticated`.

## Prevention

To prevent similar issues in the future:

1. ✅ Avoid hardcoding configuration values in code
2. ✅ Always use config accessors (e.g., `auth_manager.config().session_cookie_name`)
3. ✅ Add tests that verify config values are respected
4. ✅ Use debug logging to trace config usage

---

**Date**: October 20, 2025  
**Issue**: Hardcoded cookie name in middleware  
**Status**: Fixed and tested  
**Impact**: All JavaScript endpoints using authentication
