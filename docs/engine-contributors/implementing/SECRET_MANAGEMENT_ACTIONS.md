# Recommended Actions for Secret Management

**Date**: 2025-10-18  
**Priority**: CRITICAL - Security Issue  
**Impact**: Requirements, Use Cases, Implementation Plan

## Executive Summary

The original approach of exposing secret values to JavaScript (`Secrets.get()` returning actual values) is a critical security flaw. This document outlines the required changes to documentation and implementation to ensure secrets never leave the Rust layer.

## Immediate Actions Required

### 1. Update Use Case UC-504
**File**: `docs/engine-contributors/planning/USE_CASES.md`  
**Lines**: ~1256-1320

**Change Required**: Fix the example code to use secure approach

**Current (Insecure)**:
```javascript
const apiKey = Secrets.get("sendgrid_api_key");  // ❌ Returns value

const response = await fetch("https://api.sendgrid.com/v3/mail/send", {
  method: "POST",
  headers: {
    Authorization: `Bearer ${apiKey}`,  // ❌ Secret in JS
    "Content-Type": "application/json",
  },
  body: JSON.stringify({...})
});
```

**Should Be (Secure)**:
```javascript
// Option 1: Template syntax
const response = await fetch("https://api.sendgrid.com/v3/mail/send", {
  method: "POST",
  headers: {
    Authorization: "Bearer {{secret:sendgrid_api_key}}",  // ✅ Reference only
    "Content-Type": "application/json",
  },
  body: JSON.stringify({...})
});

// Option 2: High-level Email API (future)
const response = await Email.send({
  to: "support@company.com",
  from: { email, name },
  subject: "New Contact Form Submission",
  text: message,
  provider: "sendgrid"  // ✅ Rust handles secret
});
```

**Impact**: Medium - Example code only, principle already stated correctly

---

### 2. Update Requirement REQ-JSAPI-008
**File**: `docs/engine-contributors/planning/REQUIREMENTS.md`  
**Lines**: ~1415-1470

**Change Required**: Remove `Secrets.get()`, clarify constraints

**Current Spec**:
```markdown
**Core API**:

- `Secrets.get(identifier)` - Retrieve secret value by identifier  ❌
- `Secrets.exists(identifier)` - Check if secret exists
- `Secrets.list()` - List available secret identifiers
```

**Should Be**:
```markdown
**Core API**:

- `Secrets.exists(identifier)` - Check if secret exists (returns boolean)
- `Secrets.list()` - List available secret identifiers (not values)
- ~~`Secrets.get(identifier)`~~ - **REMOVED** - Never expose values to JavaScript

**Critical Security Principle**:

JavaScript can ONLY check if secrets exist or list identifiers. JavaScript can NEVER retrieve actual secret values. Secrets are injected by the Rust layer at point of use (HTTP requests, AI calls, etc.).
```

**Impact**: Critical - This is a requirement specification change

---

### 3. Update Requirement REQ-JSAPI-007
**File**: `docs/engine-contributors/planning/REQUIREMENTS.md`  
**Lines**: ~1364-1410

**Change Required**: Add secret injection specification

**Add Section**:
```markdown
**Integration with Secrets**:

The engine MUST support secure secret injection in HTTP requests:

**Method 1: Template Syntax** (Recommended)
```javascript
fetch("https://api.example.com/v1/data", {
  headers: {
    "authorization": "Bearer {{secret:api_key}}",
    "x-api-key": "{{secret:api_key}}"
  }
});
```

Rust detects `{{secret:identifier}}` pattern, looks up secret from SecretsManager, replaces with actual value before making HTTP request. Secret never enters JavaScript context.

**Method 2: Dedicated Secrets Parameter** (Alternative)
```javascript
fetch("https://api.example.com/v1/data", {
  headers: {
    "content-type": "application/json"
  },
  secrets: {
    "authorization": "Bearer api_key",  // Maps header name to secret ID
    "x-api-key": "api_key"
  }
});
```

**Security Guarantees**:
- Secret values never cross Rust/JavaScript boundary
- Secrets only injected at HTTP request time by Rust layer
- Secret access logged for audit trail (identifier only)
- Error if secret not found (before HTTP request)
- Automatic secret redaction from all logs

**Error Handling**:
- Clear error if secret identifier not found: "Secret 'api_key' not configured"
- Never reveal which secrets exist to unauthorized contexts
- Log secret access attempts with script ID and URL
```

**Impact**: High - Specifies new functionality for `fetch()`

---

### 4. Clarify Requirement REQ-SEC-005
**File**: `docs/engine-contributors/planning/REQUIREMENTS.md`  
**Lines**: ~420-460

**Change Required**: Add explicit trust boundary principle

**Add at the beginning of REQ-SEC-005**:
```markdown
### REQ-SEC-005: Secret Management

**Priority**: CRITICAL  
**Status**: PLANNED

**Critical Security Principle**:

Secrets MUST NEVER cross the Rust/JavaScript trust boundary. JavaScript code can only reference secrets by identifier. The Rust layer is responsible for:
- Storing actual secret values
- Injecting secrets at point of use (HTTP requests, database connections, etc.)
- Auditing secret access
- Redacting secrets from all output

JavaScript layer responsibilities:
- Reference secrets by identifier only
- Check secret existence with `Secrets.exists()`
- List available secret identifiers with `Secrets.list()`
- NEVER access secret values directly

**Architecture Principle**: Trust boundary enforcement is non-negotiable. Secrets stay in Rust, references cross to JavaScript.

[...rest of existing content...]
```

**Impact**: High - Clarifies fundamental security architecture

---

### 5. Update Implementation Plan
**File**: `docs/engine-contributors/implementing/AI_ASSISTANT_IMPLEMENTATION_PLAN.md`  
**Multiple sections**

**Changes Required**:

#### Phase 1.2 - JavaScript API Changes
```markdown
**Add Global `Secrets` Object**:
```javascript
// Available in all scripts
Secrets.exists("anthropic_api_key")   // Returns boolean
Secrets.list()                        // Returns array of identifiers

// NO Secrets.get() - Never expose values!
```

**Rust Implementation**:
- Create `secrets_exists()` and `secrets_list()` functions ONLY
- DO NOT implement `secrets_get()` that returns values
- Expose to JavaScript runtime via `deno_core::extension!`
```

#### Phase 2.2 - Fetch with Secret Injection (NEW SECTION)
```markdown
**Secret Injection in fetch() API**:

Rust must detect and inject secrets before making HTTP requests:

```rust
pub async fn js_fetch(
    url: String,
    options: FetchOptions,
) -> Result<FetchResponse, HttpError> {
    let mut final_headers = HashMap::new();
    
    // Process headers and inject secrets
    for (key, value) in options.headers.unwrap_or_default() {
        if value.starts_with("{{secret:") && value.ends_with("}}") {
            // Extract secret identifier
            let secret_id = value
                .strip_prefix("{{secret:")
                .unwrap()
                .strip_suffix("}}")
                .unwrap()
                .trim();
            
            // Look up secret (stays in Rust)
            let secret_value = SECRETS_MANAGER
                .read()
                .unwrap()
                .get(secret_id)
                .ok_or_else(|| HttpError::SecretNotFound(secret_id.to_string()))?;
            
            // Inject actual value
            final_headers.insert(key, secret_value);
            
            // Audit log
            audit_log::log_secret_access(secret_id, "fetch", &url);
        } else {
            // Regular header
            final_headers.insert(key, value);
        }
    }
    
    // Make HTTP request with injected secrets
    let response = HTTP_CLIENT
        .request(options.method.as_str(), &url)
        .headers(final_headers)
        .body(options.body.unwrap_or_default())
        .send()
        .await?;
    
    Ok(FetchResponse::from(response).await?)
}
```

**Testing**:
- Verify secrets never appear in JavaScript context
- Verify template syntax correctly parsed and injected
- Verify errors for missing secrets
- Verify audit logging works
```

#### Phase 4 - Editor Changes (MINOR)
```markdown
Update endpoint to use `Secrets.exists()` instead of trying to get value:

```javascript
// Check if configured (don't get value)
if (!Secrets.exists("anthropic_api_key")) {
  return Response.json({
    error: "AI assistant not configured. Please add 'anthropic_api_key' secret.",
    configured: false
  }, { status: 503 });
}
```

The rest of Phase 4 is already correct (uses `AI.chat()` which is secure).
```

**Impact**: Critical - Fundamental implementation approach change

---

## Testing Requirements

### New Test Cases Required

#### 1. Secret Isolation Tests
```rust
#[test]
fn test_secrets_never_exposed_to_javascript() {
    // Verify no JavaScript API returns secret values
    // Verify only exists() and list() work
    // Verify get() does not exist or throws error
}

#[test]
fn test_secret_template_injection() {
    // Verify "{{secret:id}}" gets replaced with value
    // Verify actual HTTP request has real value
    // Verify response doesn't contain secret
}

#[test]
fn test_secret_not_found_error() {
    // Verify clear error when secret doesn't exist
    // Verify error doesn't list available secrets
}
```

#### 2. Security Tests
```rust
#[test]
fn test_secret_not_in_logs() {
    // Make request with secret
    // Verify logs contain identifier, not value
}

#[test]
fn test_secret_not_in_error_messages() {
    // Trigger error with secret in use
    // Verify error message doesn't contain value
}

#[test]
fn test_audit_trail() {
    // Use secret in fetch()
    // Verify audit log has: script ID, secret ID, URL, timestamp
    // Verify audit log does NOT have secret value
}
```

#### 3. JavaScript API Tests
```javascript
// Test in test script
function testSecrets(req) {
  const tests = [];
  
  // Test exists() returns boolean
  tests.push({
    name: "Secrets.exists() works",
    passed: typeof Secrets.exists("anthropic_api_key") === "boolean"
  });
  
  // Test list() returns array
  tests.push({
    name: "Secrets.list() works",
    passed: Array.isArray(Secrets.list())
  });
  
  // Test get() doesn't exist or throws
  try {
    const value = Secrets.get("anthropic_api_key");
    tests.push({
      name: "Secrets.get() should not exist",
      passed: false,
      error: "get() should not be available"
    });
  } catch (e) {
    tests.push({
      name: "Secrets.get() correctly unavailable",
      passed: true
    });
  }
  
  return Response.json({ tests });
}
```

---

## Documentation Updates

### 1. Create Security Guide
**New File**: `docs/solution-developers/SECRETS_SECURITY.md`

Content should explain:
- Why secrets never appear in JavaScript
- How to use template syntax in fetch()
- How to use high-level APIs (AI.chat, etc.)
- How to check secret availability
- Security best practices
- Common mistakes to avoid

### 2. Update Editor Documentation
**File**: `docs/solution-developers/EDITOR_README.md`

Add section on AI assistant configuration:
```markdown
## AI Assistant Setup

The editor includes an AI assistant powered by Claude. To enable it:

1. Get API key from https://console.anthropic.com/
2. Set environment variable: `ANTHROPIC_API_KEY=sk-ant-api03-...`
3. Or add to configuration:
   ```yaml
   secrets:
     anthropic_api_key: "${ANTHROPIC_API_KEY}"
   ```
4. Restart server
5. Open editor and use AI assistant panel

**Security Note**: The API key is managed securely by the Rust layer and never exposed to JavaScript code.
```

### 3. Update API Documentation
**File**: `docs/solution-developers/javascript-apis.md`

Add section on Secrets API:
```markdown
## Secrets API

Check secret availability for conditional features:

```javascript
// Check if secret exists
if (Secrets.exists("stripe_api_key")) {
  // Enable payment features
}

// List all available secrets (identifiers only)
const secrets = Secrets.list();
// Returns: ["anthropic_api_key", "sendgrid_api_key", ...]

// Note: You cannot retrieve secret values in JavaScript.
// Use template syntax in fetch() or high-level APIs that handle secrets automatically.
```

### Using Secrets in HTTP Requests

```javascript
// Template syntax (recommended)
const response = await fetch("https://api.example.com/data", {
  headers: {
    "authorization": "Bearer {{secret:api_key}}"
  }
});

// High-level API (best for common services)
const response = await AI.chat("Hello", {
  provider: "claude"  // Automatically uses anthropic_api_key
});
```
```

---

## Timeline Impact

**Original Estimate**: 12-17 days  
**Updated Estimate**: 12-17 days (no change)

**Reasoning**:
- Phase 1: No change (Rust SecretsManager same)
- Phase 2: Slightly more complex (secret injection logic), but not significantly more time
- Phase 3: No change (already secure)
- Phase 4: Minimal change (use exists() instead of get())
- Phase 5: Minor doc updates
- Phase 6: Additional security tests (already planned)

**Net Impact**: ~1 day additional work for secret injection in Phase 2, offset by simpler JavaScript API (fewer functions to expose).

---

## Risk Assessment

### Risks of NOT Making These Changes

1. **High Risk**: Secrets leaked through logging
2. **High Risk**: Secrets leaked through error messages
3. **High Risk**: Secrets leaked through debugging
4. **Medium Risk**: Secrets accidentally returned to clients
5. **High Risk**: Violation of security requirements (REQ-SEC-008)
6. **Critical Risk**: Security audit failure

### Risks of Making These Changes

1. **Low Risk**: Implementation complexity (template parsing is straightforward)
2. **Low Risk**: Developer confusion (mitigated by clear documentation)
3. **None**: Timeline impact (minimal)

**Recommendation**: Make the changes. The security benefits far outweigh any risks.

---

## Approval and Next Steps

### Requires Approval
- [ ] Security approach approved
- [ ] Use case changes approved
- [ ] Requirements changes approved
- [ ] Implementation approach approved

### After Approval
1. Update UC-504 in USE_CASES.md
2. Update REQ-JSAPI-008 in REQUIREMENTS.md
3. Update REQ-JSAPI-007 in REQUIREMENTS.md
4. Clarify REQ-SEC-005 in REQUIREMENTS.md
5. Update AI_ASSISTANT_IMPLEMENTATION_PLAN.md
6. Create SECRETS_SECURITY.md guide
7. Begin implementation with secure approach

### Implementation Order
1. Phase 1: Secrets Management (Rust layer)
2. Phase 2: HTTP Client with secret injection
3. Phase 3: AI Integration (already secure)
4. Phase 4: Editor endpoint (minor update)
5. Phase 5: Configuration
6. Phase 6: Security testing

---

## Decision Required

**Question**: Should we proceed with the secure approach (secrets stay in Rust)?

**Recommendation**: **YES** - This is the only secure approach that:
- Aligns with requirements (REQ-SEC-008)
- Prevents secret leakage
- Provides defense in depth
- Enables audit trail
- Maintains developer experience (via high-level APIs)

**Alternative**: Continue with insecure approach
- ❌ Violates security requirements
- ❌ Secrets can be leaked
- ❌ Will fail security audit
- ❌ Not recommended

---

## Contact / Questions

If you have questions about this analysis:
1. Review `SECRET_MANAGEMENT_SECURITY_ANALYSIS.md` for detailed analysis
2. Review `SECRET_MANAGEMENT_COMPARISON.md` for visual comparison
3. Review `SECRET_MANAGEMENT_SUMMARY.md` for quick overview

All documents are in: `docs/engine-contributors/implementing/`
