# Editor Authentication Fix

## Issue

Users reported that `/auth/status` showed they were authenticated, but they could not access `/editor`. The editor returned a 401 Unauthorized error even with valid authentication sessions.

## Root Cause

The authentication system had a **flow disconnect** between Rust-based routes and JavaScript-based routes:

1. **Rust Routes** (like `/auth/status`):
   - Used middleware to extract and validate session tokens
   - Had access to authentication context via request extensions
   - Correctly showed authentication status

2. **JavaScript Routes** (like `/editor`):
   - Were executed via `execute_script_for_request` (legacy function)
   - Did NOT receive authentication context from middleware
   - Always saw `auth.isAuthenticated = false` in JavaScript
   - `auth.requireAuth()` always threw errors

The authentication middleware (`optional_auth_middleware`) was correctly:

- Extracting session tokens from cookies
- Validating sessions with the AuthManager
- Storing `AuthUser` in request extensions

But the request handler was:

- Using the old `execute_script_for_request` function
- NOT extracting `AuthUser` from request extensions
- NOT passing authentication context to JavaScript runtime

## Solution

Updated the main request handler in `src/lib.rs` to bridge the authentication gap:

### 1. Extract Authentication from Request Extensions

```rust
// Extract authentication context from middleware
let auth_user = req.extensions().get::<auth::AuthUser>().cloned();
```

### 2. Convert to JavaScript Authentication Context

```rust
// Create JavaScript authentication context
let auth_context = if let Some(ref auth_user) = auth_user {
    auth::JsAuthContext::authenticated(
        auth_user.user_id.clone(),
        None, // email - would be in session
        None, // name - would be in session  
        auth_user.provider.clone(),
    )
} else {
    auth::JsAuthContext::anonymous()
};
```

### 3. Use Secure Execution Path

```rust
// Use the secure execution path with authentication context
let params = js_engine::RequestExecutionParams {
    script_uri: owner_uri_cl.clone(),
    handler_name: handler_cl.clone(),
    path: path_clone.clone(),
    method: request_method.clone(),
    query_params: Some(query_params.clone()),
    form_data: Some(form_data.clone()),
    raw_body: raw_body.clone(),
    user_context: security::UserContext::anonymous(),
    auth_context: Some(auth_context), // ← Authentication context!
};

js_engine::execute_script_for_request_secure(params)
```

## Authentication Flow (After Fix)

```text
1. HTTP Request with session cookie
   ↓
2. optional_auth_middleware
   - Extracts session token from cookie
   - Validates with AuthManager
   - Stores AuthUser in req.extensions()
   ↓
3. Request Handler (lib.rs)
   - Extracts AuthUser from extensions
   - Converts to JsAuthContext
   - Passes via RequestExecutionParams
   ↓
4. JavaScript Engine (js_engine.rs)
   - Receives auth_context parameter
   - Calls AuthJsApi::setup_auth_globals()
   - Makes 'auth' object available globally
   ↓
5. JavaScript Handler (editor.js)
   - Can access auth.isAuthenticated
   - Can call auth.requireAuth()
   - Can get user info via auth.currentUser()
```

## Files Modified

1. **`src/lib.rs`**
   - Extract `AuthUser` from request extensions
   - Convert to `JsAuthContext`
   - Switch to `execute_script_for_request_secure`
   - Pass `auth_context` via `RequestExecutionParams`

2. **`scripts/feature_scripts/editor.js`**
   - Added `auth.requireAuth()` at start of `serveEditor()`
   - Returns 401 with login URL if not authenticated

## Testing

- ✅ All 215 unit tests pass
- ✅ All 3 editor API integration tests pass
- ✅ Build completes successfully

## Verification Steps

To verify the fix works:

1. **With valid session:**
   - Visit `/auth/login` and authenticate
   - Check `/auth/status` → should show `"success": true`
   - Visit `/editor` → should now load the editor interface

2. **Without session:**
   - Clear cookies or use incognito mode
   - Check `/auth/status` → should show `"success": false`
   - Visit `/editor` → should return 401 with login URL

## Benefits

This fix provides several benefits:

1. **Consistency**: JavaScript handlers now have the same authentication context as Rust handlers
2. **Security**: Authentication checks in JavaScript actually work
3. **Developer Experience**: The `auth` global object works as documented
4. **Future-Proof**: Uses the secure execution path with proper context passing

## Related Documentation

- [JavaScript Authentication API](./solution-developers/AUTH_JS_API.md)
- [Editor Authentication Requirement](./EDITOR_AUTH_REQUIREMENT.md)
- [Authentication Middleware](../src/auth/middleware.rs)
- [JavaScript Engine](../src/js_engine.rs)

---

**Date**: October 20, 2025  
**Issue**: Authentication context not passed to JavaScript handlers  
**Status**: Fixed and tested
