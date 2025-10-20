# Editor Authentication Requirement

## Summary

The `/editor` endpoint now requires authentication to access. Users must be logged in to use the built-in web editor.

## Changes Made

### Modified Files

1. **`scripts/feature_scripts/editor.js`**
   - Added authentication check using `auth.requireAuth()` at the beginning of the `serveEditor()` function
   - Returns 401 Unauthorized with a JSON response if the user is not authenticated
   - Response includes a redirect to `/auth/login` for unauthenticated users

2. **`src/lib.rs`** (Main request handler)
   - Updated to extract `AuthUser` from request extensions (added by middleware)
   - Converts `AuthUser` to `JsAuthContext` for JavaScript runtime
   - Switched from `execute_script_for_request` to `execute_script_for_request_secure` with `RequestExecutionParams`
   - Now passes authentication context to all JavaScript handlers via the `auth` global object

### Implementation Details

#### JavaScript Handler (editor.js)

```javascript
function serveEditor(req) {
  // Require authentication to access the editor
  try {
    auth.requireAuth();
  } catch (error) {
    return {
      status: 401,
      body: JSON.stringify({
        error: "Authentication required",
        message: "Please login to access the editor",
        loginUrl: "/auth/login"
      }),
      contentType: "application/json"
    };
  }
  
  // ... rest of the editor serving logic
}
```

#### Rust Request Handler (lib.rs)

The fix involved updating the main request handler to properly pass authentication context from the middleware to the JavaScript runtime:

```rust
// Extract authentication context from middleware
let auth_user = req.extensions().get::<auth::AuthUser>().cloned();

// Create JavaScript authentication context
let auth_context = if let Some(ref auth_user) = auth_user {
    auth::JsAuthContext::authenticated(
        auth_user.user_id.clone(),
        None, // email - stored in session
        None, // name - stored in session  
        auth_user.provider.clone(),
    )
} else {
    auth::JsAuthContext::anonymous()
};

// Execute with authentication context
let params = js_engine::RequestExecutionParams {
    // ... other params
    auth_context: Some(auth_context),
};

js_engine::execute_script_for_request_secure(params)
```

### Root Cause

The initial implementation had an authentication flow disconnect:

1. The `/auth/status` endpoint (Rust-based) correctly validated sessions via middleware
2. The `/editor` endpoint (JavaScript-based) wasn't receiving the authentication context
3. The server was using the old `execute_script_for_request` function which didn't pass auth context
4. JavaScript handlers always saw `auth.isAuthenticated = false` even with valid sessions

The fix bridges this gap by:

1. Extracting `AuthUser` from request extensions (added by `optional_auth_middleware`)
2. Converting it to `JsAuthContext` for the JavaScript runtime
3. Passing it through `RequestExecutionParams` to the JavaScript engine
4. Making it available as the global `auth` object in JavaScript handlers

## Behavior

### Authenticated Users

- Can access `/editor` normally
- See the full editor interface
- Can create, edit, and manage scripts

### Unauthenticated Users

- Receive HTTP 401 Unauthorized response
- Get JSON response with error message and login URL
- Should be redirected to `/auth/login` to authenticate

## API Endpoints

The editor API endpoints (`/api/scripts/*`, `/api/assets/*`, etc.) maintain their existing authentication and authorization requirements based on capabilities. This change only affects the `/editor` HTML interface endpoint.

## Testing

All existing tests pass:

- `cargo test --lib` - All unit tests pass
- `cargo test --test api_editor` - All integration tests pass

Tests use `test_config.auth = None` by default, so they bypass authentication and continue to work correctly. The tests focus on API endpoints rather than the HTML editor interface.

## Security Implications

This change improves security by:

1. **Preventing unauthorized access** - Only authenticated users can access the editor interface
2. **Protecting sensitive features** - The editor allows script management which should be restricted
3. **Clear error messaging** - Users get clear guidance on how to authenticate
4. **Consistent with capability model** - Aligns with the existing security model where script management requires authentication

## Migration Notes

If you're running an aiwebengine instance:

1. Ensure authentication is configured in your `config.toml` (if not already)
2. Users will need to authenticate via `/auth/login` before accessing `/editor`
3. Public API endpoints remain accessible without authentication (unless specifically protected)

## Related Documentation

- [Authentication JS API](./solution-developers/AUTH_JS_API.md) - JavaScript authentication API reference
- [Authentication Setup](./solution-developers/AUTH_SETUP.md) - How to configure OAuth2 providers
- [Security Capabilities](./engine-contributors/planning/REQUIREMENTS.md#authentication--authorization) - Capability-based security model

---

**Date**: October 20, 2025  
**Status**: Implemented and tested
