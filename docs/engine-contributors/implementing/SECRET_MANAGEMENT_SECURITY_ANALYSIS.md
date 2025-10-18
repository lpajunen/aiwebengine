# Secret Management Security Analysis

## Critical Security Issue Identified

**Date**: 2025-10-18  
**Issue**: The current approach of exposing secret values to JavaScript context violates security principles and requirements.

## The Problem

### Current (Insecure) Approach in Documents

The use case UC-504 and initial implementation plan show:

```javascript
// INSECURE - Secret value exposed to JavaScript
const apiKey = Secrets.get("sendgrid_api_key");  // Returns actual value!

const response = await fetch("https://api.sendgrid.com/v3/mail/send", {
  headers: {
    Authorization: `Bearer ${apiKey}`,  // apiKey is in JS memory
  }
});
```

**Security Problems**:
1. ❌ **Secret value exists in JavaScript memory** - Can be inspected, logged, or leaked
2. ❌ **Variable can be accidentally logged** - `console.log(apiKey)` exposes secret
3. ❌ **Secret appears in stack traces** - Errors may expose the value
4. ❌ **Secret could be sent in responses** - Developer error could return it
5. ❌ **Violates REQ-SEC-008** - Security must be enforced in Rust, not JavaScript
6. ❌ **Violates principle**: "Script never sees the actual value" (from UC-504)

### Why This is Critical

Even with redaction in logging, once the secret value is in JavaScript:
- It can be captured via `debugger` statements
- It can be stored in closure variables
- It can be stringified and sent anywhere
- JavaScript cannot be trusted to handle secrets securely

## The Correct Approach

### Principle: Secrets Never Leave Rust

**Core Rule**: JavaScript code references secrets by identifier, but the Rust layer handles the actual values.

### Architecture

```
┌──────────────────────────────────────────────────────────┐
│  JavaScript Layer (Untrusted)                            │
│  ┌────────────────────────────────────────────────────┐ │
│  │ Script only knows identifier: "anthropic_api_key"  │ │
│  │ Secret value NEVER enters JavaScript context       │ │
│  └────────────────────────────────────────────────────┘ │
└───────────────────────┬──────────────────────────────────┘
                        │ Reference by ID only
                        ▼
┌──────────────────────────────────────────────────────────┐
│  Rust Layer (Trusted)                                    │
│  ┌────────────────────────────────────────────────────┐ │
│  │ SecretsManager                                     │ │
│  │ - Stores actual secret values                      │ │
│  │ - Injects secrets into requests                    │ │
│  │ - Redacts secrets from responses                   │ │
│  │ - Validates usage                                  │ │
│  └────────────────────────────────────────────────────┘ │
└──────────────────────────────────────────────────────────┘
```

## Required Changes

### 1. JavaScript APIs - Complete Redesign

#### ❌ WRONG: Expose Secret Values

```javascript
// BAD - Returns secret value to JavaScript
Secrets.get("api_key")  // Returns: "sk-ant-api03-..."
```

#### ✅ CORRECT: Secret References Only

```javascript
// Option A: Secrets.exists() and Secrets.list() only
Secrets.exists("anthropic_api_key")  // Returns: true/false
Secrets.list()                       // Returns: ["anthropic_api_key", ...]

// NO Secrets.get() that returns values!
```

### 2. HTTP Client (fetch) - Rust-Side Secret Injection

#### ❌ WRONG: JavaScript Provides Secret

```javascript
// BAD - Secret in JavaScript
const response = await fetch(url, {
  headers: {
    "x-api-key": Secrets.get("anthropic_api_key")  // Value in JS!
  }
});
```

#### ✅ CORRECT: Reference Secret, Rust Injects Value

```javascript
// Option 1: Special header syntax for secret references
const response = await fetch("https://api.anthropic.com/v1/messages", {
  headers: {
    "x-api-key": "{{secret:anthropic_api_key}}",  // Placeholder
    "content-type": "application/json"
  },
  body: JSON.stringify({ /* ... */ })
});

// Rust layer:
// 1. Detects "{{secret:*}}" pattern in headers
// 2. Replaces with actual secret value
// 3. Makes HTTP request
// 4. Returns response (secret never in JS)
```

```javascript
// Option 2: Dedicated secrets parameter
const response = await fetch("https://api.anthropic.com/v1/messages", {
  headers: {
    "content-type": "application/json"
  },
  body: JSON.stringify({ /* ... */ }),
  secrets: {
    "x-api-key": "anthropic_api_key"  // Maps header to secret ID
  }
});

// Rust layer:
// 1. Reads secrets map
// 2. Looks up values from SecretsManager
// 3. Adds to headers
// 4. Makes request
```

```javascript
// Option 3: Template-based requests (most explicit)
const response = await fetch("https://api.anthropic.com/v1/messages", {
  headers: {
    "content-type": "application/json"
  },
  body: JSON.stringify({ /* ... */ }),
  auth: {
    type: "header",
    header: "x-api-key",
    secret: "anthropic_api_key"  // Secret identifier
  }
});
```

### 3. AI API - Complete Abstraction

#### ❌ WRONG: JavaScript Constructs API Call

```javascript
// BAD - Requires secret in JavaScript
const apiKey = Secrets.get("anthropic_api_key");
const response = await fetch(url, {
  headers: { "x-api-key": apiKey }
});
```

#### ✅ CORRECT: High-Level API in Rust

```javascript
// GOOD - Rust handles all secret management
const response = await AI.chat("What is Rust?", {
  provider: "claude",  // Rust knows which secret to use
  model: "claude-3-haiku-20240307",
  maxTokens: 1024
});

// JavaScript never sees:
// - API key
// - API endpoint
// - Request headers
// - Raw API request/response format
```

**Rust Implementation**:
```rust
async fn js_ai_chat(
    prompt: String,
    options: ChatOptions,
) -> Result<ChatResponse, AIError> {
    // 1. Determine provider from options
    let provider_name = options.provider.unwrap_or_else(|| "claude".to_string());
    
    // 2. Get AI manager with access to SecretsManager
    let ai_manager = get_ai_manager();
    
    // 3. AI manager looks up secret based on provider
    // e.g., "claude" -> needs "anthropic_api_key"
    let api_key = secrets_manager
        .get(&format!("{}_api_key", provider_name))
        .ok_or(AIError::SecretNotFound)?;
    
    // 4. Make HTTP request with secret (JavaScript never sees it)
    let response = http_client.post("https://api.anthropic.com/v1/messages")
        .header("x-api-key", api_key)  // Secret stays in Rust
        .json(&request_body)
        .send()
        .await?;
    
    // 5. Parse and return response (no secrets)
    Ok(ChatResponse {
        content: response.content,
        model: response.model,
        usage: response.usage,
        finish_reason: response.finish_reason,
    })
}
```

## Impact Analysis

### 1. Use Case UC-504: External API Integration

**Current Description Issues**:
- Shows `const apiKey = Secrets.get("sendgrid_api_key");` returning value
- Example code puts secret in JavaScript variable
- Violates stated principle: "script never sees the actual value"

**Required Changes**:

```javascript
// BEFORE (Insecure - from current UC-504)
async function submitContactForm(req) {
  const apiKey = Secrets.get("sendgrid_api_key");  // ❌ Value in JS
  
  const response = await fetch("https://api.sendgrid.com/v3/mail/send", {
    method: "POST",
    headers: {
      Authorization: `Bearer ${apiKey}`,  // ❌ Secret in header
      "Content-Type": "application/json",
    },
    body: JSON.stringify({ /* ... */ })
  });
}

// AFTER (Secure - secrets stay in Rust)
async function submitContactForm(req) {
  // Option 1: Template syntax
  const response = await fetch("https://api.sendgrid.com/v3/mail/send", {
    method: "POST",
    headers: {
      Authorization: "Bearer {{secret:sendgrid_api_key}}",  // ✅ Reference
      "Content-Type": "application/json",
    },
    body: JSON.stringify({ /* ... */ })
  });
  
  // Option 2: Dedicated secrets parameter
  const response = await fetch("https://api.sendgrid.com/v3/mail/send", {
    method: "POST",
    headers: {
      "Content-Type": "application/json",
    },
    body: JSON.stringify({ /* ... */ }),
    secrets: {
      Authorization: "Bearer sendgrid_api_key"  // ✅ Secret ID
    }
  });
  
  // Option 3: High-level email API (best for common use cases)
  const response = await Email.send({
    to: "support@company.com",
    from: { email, name },
    subject: "New Contact Form Submission",
    text: message,
    provider: "sendgrid"  // ✅ Uses sendgrid_api_key automatically
  });
}
```

### 2. Requirement REQ-JSAPI-008: Secrets API

**Current Spec Issues**:
- States: `Secrets.get(identifier)` - Retrieve secret value by identifier
- Shows usage: `Authorization: Bearer ${Secrets.get("api_token")}`
- Says "value never stored in JS variables" but example does exactly that

**Required Changes**:

```markdown
### REQ-JSAPI-008: Secrets API (REVISED)

**Priority**: CRITICAL  
**Status**: PLANNED

The engine MUST expose secrets management API to JavaScript:

**Core API**:

- `Secrets.exists(identifier)` - Check if secret exists (returns boolean)
- `Secrets.list()` - List available secret identifiers (not values)
- ~~`Secrets.get(identifier)`~~ - **REMOVED** - Never expose values to JavaScript

**Security Constraints**:

- Scripts can ONLY check secret existence, never retrieve values
- Secret values NEVER leave the Rust layer
- Secret identifiers are strings (e.g., "stripe_api_key", "sendgrid_token")
- Secrets are injected by Rust layer during HTTP requests
- Engine automatically redacts secrets from all output

**Usage Patterns**:

```javascript
// Check if secret is configured before using feature
if (Secrets.exists("sendgrid_api_key")) {
  // Use sendgrid integration (Rust will inject secret)
} else {
  return Response.json({
    error: "Email feature not configured"
  }, { status: 503 });
}

// List available integrations based on configured secrets
const availableSecrets = Secrets.list();
// Returns: ['stripe_api_key', 'sendgrid_token', 'anthropic_api_key']
const availableIntegrations = availableSecrets.map(s => s.replace('_api_key', ''));
```

**Implementation Notes**:

- Secrets stay in Rust SecretsManager
- fetch() API supports secret injection via template syntax
- High-level APIs (AI, Email, etc.) handle secrets automatically
- No way for JavaScript to access secret values
```

### 3. Requirement REQ-JSAPI-007: HTTP Client API

**Required Changes**:

Add secret injection specification:

```markdown
### REQ-JSAPI-007: HTTP Client API (REVISED)

**Priority**: HIGH  
**Status**: PLANNED

...existing content...

**Integration with Secrets**:

- **Secret template syntax** in headers: `"{{secret:identifier}}"`
- **Automatic secret injection** by Rust layer before HTTP request
- **Secret values never enter JavaScript context**
- **Secrets automatically redacted** from request logs
- Support for common auth patterns without exposing secrets:
  - Bearer token: `Authorization: "Bearer {{secret:api_key}}"`
  - API key header: `x-api-key: "{{secret:api_key}}"`
  - Basic auth: `Authorization: "Basic {{secret:credentials_base64}}"`

**Alternative: Dedicated Secrets Parameter**:

```javascript
fetch(url, {
  headers: { /* non-secret headers */ },
  secrets: {
    "Authorization": "Bearer api_key",  // Secret ID, not value
    "x-api-key": "another_secret"
  }
})
```

**Error Handling**:

- Throw error if referenced secret doesn't exist
- Error messages never reveal which secrets are available
- Log secret reference attempts for audit trail
```

### 4. Requirement REQ-SEC-005: Secret Management

**Required Clarifications**:

```markdown
### REQ-SEC-005: Secret Management (CLARIFICATION)

...existing content...

**Critical Security Principle**:

- **Secrets NEVER cross the Rust/JavaScript boundary**
- **JavaScript can only reference secrets by identifier**
- **Rust layer injects secrets at point of use**
- **No JavaScript API returns secret values**

**Secret Injection Points**:

1. **HTTP Client (fetch)**: Headers via template syntax or secrets parameter
2. **AI APIs**: Automatic injection based on provider configuration
3. **Email APIs**: Automatic injection based on provider
4. **Database connections**: Managed in Rust, never exposed
5. **OAuth providers**: Managed in Rust authentication layer

**Validation**:

- Rust validates all secret references before use
- Unknown secret references result in clear errors
- Audit log records all secret usage (by identifier only)
```

### 5. Requirement REQ-SEC-008: Security Enforcement Architecture

**Already Correct**: This requirement already states:

> The engine MUST enforce security at the Rust layer, not JavaScript:
> - Security validation MUST be in Rust
> - JavaScript contains only business logic

Our new approach aligns perfectly with this requirement.

## Implementation Plan Changes

### Phase 1: Secrets Management (REVISED)

#### 1.1 SecretsManager - No Change

The Rust `SecretsManager` remains the same - it stores secrets securely.

#### 1.2 JavaScript API - MAJOR CHANGE

**REMOVE**:
```rust
// DON'T implement this!
fn secrets_get(identifier: String) -> Result<String, Error> {
    // Returns secret value - INSECURE
}
```

**IMPLEMENT**:
```rust
// Only these two functions
fn secrets_exists(identifier: String) -> bool {
    SECRETS_MANAGER.read().unwrap().exists(&identifier)
}

fn secrets_list() -> Vec<String> {
    SECRETS_MANAGER.read().unwrap().list_identifiers()
}
```

### Phase 2: HTTP Client (REVISED)

#### 2.1 Secret Injection in fetch()

**New Implementation**:

```rust
pub async fn js_fetch(
    url: String,
    options: FetchOptions,
) -> Result<FetchResponse, HttpError> {
    // 1. Extract secret references from headers
    let mut final_headers = HashMap::new();
    
    for (key, value) in options.headers.unwrap_or_default() {
        // Check for template syntax: "{{secret:identifier}}"
        if value.starts_with("{{secret:") && value.ends_with("}}") {
            let secret_id = value
                .strip_prefix("{{secret:")
                .unwrap()
                .strip_suffix("}}")
                .unwrap()
                .trim();
            
            // Look up secret in SecretsManager
            let secret_value = SECRETS_MANAGER
                .read()
                .unwrap()
                .get(secret_id)
                .ok_or_else(|| HttpError::SecretNotFound(secret_id.to_string()))?;
            
            // Inject actual secret value
            final_headers.insert(key, secret_value);
            
            // Log secret usage (identifier only, never value)
            audit_log::log_secret_access(secret_id, "fetch", &url);
        } else {
            // Regular header
            final_headers.insert(key, value);
        }
    }
    
    // 2. Alternative: Process secrets parameter if provided
    if let Some(secrets_map) = options.secrets {
        for (header_name, secret_id) in secrets_map {
            let secret_value = SECRETS_MANAGER
                .read()
                .unwrap()
                .get(&secret_id)
                .ok_or_else(|| HttpError::SecretNotFound(secret_id.clone()))?;
            
            final_headers.insert(header_name, secret_value);
            audit_log::log_secret_access(&secret_id, "fetch", &url);
        }
    }
    
    // 3. Make HTTP request with injected secrets
    let response = HTTP_CLIENT
        .request(options.method.as_str(), &url)
        .headers(final_headers)
        .body(options.body.unwrap_or_default())
        .send()
        .await?;
    
    // 4. Return response (no secrets in it)
    Ok(FetchResponse::from(response).await?)
}
```

**New FetchOptions Type**:
```rust
pub struct FetchOptions {
    pub method: String,
    pub headers: Option<HashMap<String, String>>,
    pub body: Option<String>,
    pub timeout: Option<Duration>,
    pub secrets: Option<HashMap<String, String>>,  // NEW: header -> secret_id
}
```

### Phase 3: AI Integration (NO CHANGE)

The AI API approach is already secure:

```javascript
// JavaScript sees high-level API only
AI.chat("prompt", { provider: "claude" })

// Rust handles:
// - Which secret to use ("anthropic_api_key" for "claude")
// - API endpoint
// - Request format
// - Secret injection
```

This is perfect - no changes needed!

### Phase 4: Editor Backend (MINOR CHANGE)

The `/api/ai-assistant` endpoint already uses `AI.chat()`, which is secure:

```javascript
// Already secure - no changes needed
const response = await AI.chat(prompt, {
  model: "claude-3-haiku-20240307",
  maxTokens: 1024
});
```

Only change needed:

```javascript
// Check secret exists (not get value)
if (!Secrets.exists("anthropic_api_key")) {
  return Response.json({
    error: "AI assistant not configured",
    configured: false
  }, { status: 503 });
}
```

## Recommended Approach

### Option 1: Template Syntax (Recommended)

**Pros**:
- Explicit and clear where secrets are used
- Easy to grep for security audits
- Works with existing fetch() API structure
- No new parameters needed

**Cons**:
- Magic string pattern
- Could conflict with actual header values (unlikely)

**Usage**:
```javascript
fetch(url, {
  headers: {
    "x-api-key": "{{secret:anthropic_api_key}}"
  }
})
```

### Option 2: Dedicated Secrets Parameter

**Pros**:
- Type-safe and explicit
- Clear separation of secrets from regular headers
- Easy to validate

**Cons**:
- New parameter to document
- Slightly more verbose

**Usage**:
```javascript
fetch(url, {
  headers: { "content-type": "application/json" },
  secrets: {
    "x-api-key": "anthropic_api_key"
  }
})
```

### Option 3: High-Level APIs Only

**Pros**:
- Most secure - no secret references in JS at all
- Simplest for developers
- Best user experience

**Cons**:
- Need to build API wrappers for every external service
- Less flexible for custom integrations

**Usage**:
```javascript
// For common use cases, provide high-level APIs
AI.chat(prompt)
Email.send({to, from, subject, body})
Payment.charge({amount, token})
```

### Recommended Hybrid Approach

1. **High-level APIs for common use cases**:
   - `AI.chat()` - for AI providers
   - `Email.send()` - for email services
   - Future: `Payment.charge()`, `Storage.upload()`, etc.

2. **Template syntax for custom integrations**:
   - When high-level API doesn't exist
   - For custom/unusual services
   - Advanced use cases

3. **No direct secret access**:
   - Remove `Secrets.get()` entirely
   - Only `Secrets.exists()` and `Secrets.list()`

## Security Validation

### ✅ Checklist for Secure Implementation

- [ ] SecretsManager never exposes values to JavaScript
- [ ] No `Secrets.get()` function that returns values
- [ ] fetch() injects secrets at Rust layer
- [ ] All secret injection logged (identifier only)
- [ ] Errors never reveal secret existence to unauthorized users
- [ ] High-level APIs (AI, Email) handle secrets transparently
- [ ] Template syntax or secrets parameter properly documented
- [ ] Audit trail for all secret usage
- [ ] Tests verify secrets never appear in JavaScript context
- [ ] Documentation updated to reflect secure approach

### ✅ Attack Vectors Mitigated

- ✅ **Memory inspection**: Secret not in JS heap
- ✅ **Logging leaks**: Secret not in JS, can't be logged by script
- ✅ **Error exposure**: Errors can't contain values that never existed in JS
- ✅ **Response leaks**: Can't accidentally return value to client
- ✅ **Debugger access**: Debugger can't see what's not in JS context
- ✅ **Stack traces**: No secret values in JavaScript stack
- ✅ **Closure capture**: Can't capture what was never there

## Summary

### Critical Changes Required

1. **Remove `Secrets.get()` from API** - Never return secret values to JavaScript
2. **Add secret injection to `fetch()`** - Template syntax or secrets parameter
3. **Update Use Case UC-504** - Fix example code to be secure
4. **Update REQ-JSAPI-008** - Remove `Secrets.get()`, clarify constraints
5. **Update REQ-JSAPI-007** - Add secret injection specification
6. **Clarify REQ-SEC-005** - Emphasize secrets never cross to JavaScript

### Design Principles

1. **Trust boundary**: Rust (trusted) vs JavaScript (untrusted)
2. **Principle of least privilege**: JS only knows secret exists, not its value
3. **Defense in depth**: Even if JS tries to leak secrets, it can't (it doesn't have them)
4. **Audit trail**: Log all secret usage by identifier
5. **Fail secure**: Missing secret = clear error, never expose what exists

### Benefits of This Approach

- ✅ **Impossible to leak secrets from JavaScript** - they're not there
- ✅ **Simpler for developers** - high-level APIs abstract complexity
- ✅ **Aligns with REQ-SEC-008** - security enforced in Rust
- ✅ **Future-proof** - can add providers without JS changes
- ✅ **Auditable** - secret usage clearly logged
- ✅ **Compliant** - meets security requirements properly
