# Middleware Layer Ordering Fix

## Issue

Authentication middleware was not being called for any dynamic routes (`/`, `/{*path}`). This meant:

- `/auth/status` worked (authenticated) ✅
- `/editor` failed (not authenticated) ❌
- `/` failed (not authenticated) ❌
- All JavaScript handlers saw `auth.isAuthenticated = false` ❌

**Logs showed:**
```
INFO aiwebengine: [req_xxx] No authentication context in request
INFO aiwebengine: [req_xxx] Executing handler ... (authenticated: false)
```

**No middleware logs:**
```
# Expected but missing:
🔐 optional_auth_middleware called for path: /
```

## Root Cause

**Incorrect Axum middleware layer ordering.**

In the original code:

```rust
// 1. Mount auth routes
app = app.nest("/auth", auth_router);

// 2. Add auth middleware layer
app = app.layer(axum::middleware::from_fn_with_state(
    auth_mgr,
    auth::optional_auth_middleware,
));

// 3. Add catch-all routes AFTER middleware
app = app
    .route("/", any(handle_dynamic_request))
    .route("/{*path}", any(handle_dynamic_request));
```

### Axum Layer Rules

In Axum:
1. **Middleware layers only apply to routes added BEFORE the layer**
2. Layers execute in REVERSE order of how they're added
3. Routes added AFTER a layer don't get that middleware

So the auth middleware was added, but then the catch-all routes were added after, which meant they bypassed the middleware entirely!

## Solution

**Move middleware layers to AFTER all routes are defined:**

```rust
// 1. Mount auth routes
app = app.nest("/auth", auth_router);

// 2. Add catch-all routes FIRST
app = app
    .route("/", any(handle_dynamic_request))
    .route("/{*path}", any(handle_dynamic_request));

// 3. Add middleware layers AFTER routes
if let Some(ref auth_mgr) = auth_manager {
    app = app.layer(axum::middleware::from_fn_with_state(
        auth_mgr,
        auth::optional_auth_middleware,
    ));
}

app = app.layer(axum::middleware::from_fn(
    middleware::request_id_middleware
));
```

### Execution Order

With this ordering:

1. **Request comes in**
2. **request_id_middleware** runs (added last, runs first)
3. **optional_auth_middleware** runs (added second-to-last, runs second)
4. **Route handler** executes (with `AuthUser` in extensions if authenticated)

## Files Modified

**`src/lib.rs`**
- Moved middleware layer application to AFTER route definitions
- Added comment explaining Axum layer ordering
- Separated middleware layer setup for clarity

## Impact

### Before Fix
- ❌ Middleware not called for `/`, `/{*path}`
- ❌ `AuthUser` never injected into requests
- ❌ All JavaScript handlers see anonymous user
- ❌ `auth.requireAuth()` always fails

### After Fix
- ✅ Middleware called for ALL routes
- ✅ `AuthUser` injected when session valid
- ✅ JavaScript handlers see authenticated user
- ✅ `auth.requireAuth()` succeeds when logged in

## Testing

After this fix, when you access ANY route with a valid session cookie:

```bash
# With valid session
curl -b cookies.txt http://localhost:3000/
curl -b cookies.txt http://localhost:3000/editor
curl -b cookies.txt http://localhost:3000/api/anything
```

**Expected logs:**
```
🔐 optional_auth_middleware called for path: /
🔑 Session token found for /: _IXIH...
✅ Session validated for /: user_id=114616196929829287625
✅ AuthUser injected into request extensions for /
[req_xxx] Authentication context found: user_id=114616196929829287625
[req_xxx] Executing handler ... (authenticated: true)
```

## Verification Steps

1. **Start server with debug logging:**
   ```bash
   RUST_LOG=debug cargo run
   ```

2. **Access any route:**
   ```bash
   curl http://localhost:3000/
   ```

3. **Check logs for middleware activity:**
   - Should see `🔐 optional_auth_middleware called`
   - Should see `ℹ️  No session token found` (if not logged in)
   - OR see `✅ Session validated` (if logged in)

## Related Issues

This fix resolves:
- ✅ Editor authentication not working
- ✅ All JavaScript endpoints seeing anonymous users
- ✅ Session cookies being ignored
- ✅ `auth.isAuthenticated` always false in JavaScript

## Prevention

To prevent similar issues in the future:

1. **Always add middleware layers AFTER routes** (unless you specifically need route-specific middleware)
2. **Remember Axum layer execution is REVERSE** of the order they're added
3. **Use debug logging** to verify middleware is called
4. **Test with actual HTTP requests**, not just unit tests

### Correct Pattern

```rust
// 1. Define all routes first
app = app
    .route("/route1", handler1)
    .route("/route2", handler2);

// 2. Add middleware layers last (they apply to routes above)
app = app
    .layer(middleware1)  // Runs second
    .layer(middleware2); // Runs first
```

## Documentation References

- [Axum Middleware Ordering](https://docs.rs/axum/latest/axum/middleware/index.html#ordering)
- [Tower Service Layer](https://docs.rs/tower/latest/tower/trait.Layer.html)

---

**Date**: October 20, 2025  
**Issue**: Middleware not applying to catch-all routes  
**Status**: Fixed and tested  
**Impact**: ALL dynamic routes now properly authenticated
