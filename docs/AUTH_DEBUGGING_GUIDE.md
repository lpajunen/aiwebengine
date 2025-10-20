# Authentication Debugging Guide

## Problem
`/auth/status` shows authenticated, but `/editor` denies access with 401.

## Root Causes to Check

### 1. Check if Authentication is Actually Enabled

Look at server startup logs for this line:
```
✅ Authentication ENABLED - mounting auth routes and middleware
```

Or:
```
⚠️  Authentication DISABLED - no auth routes or middleware
```

**If DISABLED**: The auth middleware is not running, so AuthUser is never added to requests.

### 2. Check Session Cookie

When you visit `/auth/status`, check the browser developer tools:

**Application/Storage → Cookies**

Look for a cookie named `session` (or whatever `auth.cookie.name` is in your config).

**If NO cookie exists**: You're not actually authenticated, `/auth/status` might be cached or lying.

### 3. Check the Actual Response from /editor

Run this command:
```bash
curl -v -b cookies.txt http://localhost:3000/editor 2>&1 | grep -A 20 "^<"
```

Look for the JSON response body - it should now include a `debug` section:
```json
{
  "error": "Authentication required",
  "message": "Please login to access the editor",
  "loginUrl": "/auth/login",
  "debug": {
    "authExists": true,
    "isAuthenticated": false,
    "errorMessage": "Authentication required. Please login to access this resource."
  }
}
```

This tells you if:
- `authExists: false` → The `auth` global object isn't being set up in JavaScript
- `authExists: true, isAuthenticated: false` → Auth object exists but sees no session
- Error message → What `auth.requireAuth()` threw

### 4. Check Server Logs

When you request `/editor`, look for these log lines:

```
[request-id] No authentication context in request
```
OR
```
[request-id] Authentication context found: user_id=..., provider=...
```

Also check for:
```
=== Editor Authentication Check ===
auth object exists: true/false
auth.isAuthenticated: true/false
```

### 5. Verify OAuth Configuration

If you're using OAuth (Google, Microsoft, Apple), you MUST have valid credentials:

```toml
[auth.providers.google]
client_id = "your-actual-client-id"  # NOT "your-google-client-id"
client_secret = "your-actual-secret"
redirect_uri = "http://localhost:3000/auth/callback/google"
```

If these are placeholder values, you can't actually log in via OAuth.

## Steps to Debug

### Step 1: Check if you're REALLY authenticated

```bash
curl -i http://localhost:3000/auth/status
```

Look for:
1. HTTP status code (200?)
2. Response body (`"success": true` or `false`?)
3. Set-Cookie header (is a session cookie being set?)

### Step 2: Save the session cookie

If `/auth/status` returns success with a cookie:
```bash
curl -c cookies.txt http://localhost:3000/auth/status
```

### Step 3: Use that cookie to access /editor

```bash
curl -b cookies.txt http://localhost:3000/editor
```

### Step 4: Check server logs

```bash
tail -f logs/aiwebengine-dev.log | grep -E "(Authentication|Editor|auth\.)"
```

## Common Issues

### Issue 1: Authentication Disabled in Config

**Fix**: Set `auth.enabled = true` in config.toml

### Issue 2: No Valid OAuth Provider

**Fix**: Either:
- Configure a real OAuth provider (Google, Microsoft, Apple)
- OR implement a test/dev authentication bypass

### Issue 3: Middleware Not Applied

**Check**: Server logs should show:
```
✅ Adding optional_auth_middleware to all routes
```

### Issue 4: Session Cookie Not Being Sent

**Fix**: Make sure cookies are enabled and check SameSite settings

### Issue 5: Auth Context Not Passed to JavaScript

**Check logs for**:
```
[request-id] Authentication context found: user_id=...
```

If you see this but still get 401, then the JavaScript auth setup is broken.

## Testing Without OAuth

If you don't want to set up OAuth, you can create a test session manually.

**Option A**: Add a test endpoint in `scripts/feature_scripts/core.js`:

```javascript
function createTestSession(req) {
  // This is a DEVELOPMENT ONLY hack
  return {
    status: 200,
    body: "Session creation not implemented in JavaScript - use /auth/login",
    contentType: "text/plain"
  };
}
```

**Option B**: Implement a development-only authentication bypass in Rust.

## What to Report

If you're still stuck, provide:

1. Output of: `curl -i http://localhost:3000/auth/status`
2. Output of: `curl -i http://localhost:3000/editor`
3. Server logs when accessing /editor
4. Your `auth` configuration from config.toml
5. Browser cookies (if using browser instead of curl)

---

**Updated**: October 20, 2025
