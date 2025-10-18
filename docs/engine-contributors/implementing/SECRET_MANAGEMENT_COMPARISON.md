# Secret Management: Insecure vs Secure Approach

## ❌ INSECURE: Original Approach (DO NOT USE)

```
┌────────────────────────────────────────────────────────────┐
│  JavaScript Layer (Untrusted)                              │
│                                                            │
│  const apiKey = Secrets.get("anthropic_api_key")          │
│  // apiKey = "sk-ant-api03-abc123..."  ← SECRET IN JS! ❌  │
│                                                            │
│  fetch("https://api.anthropic.com/v1/messages", {         │
│    headers: {                                             │
│      "x-api-key": apiKey  ← SECRET IN HEADER ❌           │
│    }                                                       │
│  })                                                        │
│                                                            │
│  Problems:                                                 │
│  • Secret value in JavaScript memory                       │
│  • Can be logged: console.log(apiKey)                      │
│  • Can be inspected with debugger                          │
│  • Can be accidentally returned to client                  │
│  • Appears in error stack traces                           │
└────────────────────────────────────────────────────────────┘
                            │
                            ▼
┌────────────────────────────────────────────────────────────┐
│  Rust Layer                                                │
│                                                            │
│  fn secrets_get(id: String) -> String {                   │
│      // Returns actual secret value to JS ❌               │
│      secrets_manager.get(&id).unwrap()                     │
│  }                                                         │
└────────────────────────────────────────────────────────────┘
```

## ✅ SECURE: Correct Approach (USE THIS)

```
┌────────────────────────────────────────────────────────────┐
│  JavaScript Layer (Untrusted)                              │
│                                                            │
│  // JavaScript only knows the identifier, not the value    │
│  fetch("https://api.anthropic.com/v1/messages", {         │
│    headers: {                                             │
│      "x-api-key": "{{secret:anthropic_api_key}}"  ✅      │
│      //            ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^          │
│      //            Template - not actual value!            │
│    },                                                      │
│    body: JSON.stringify({...})                            │
│  })                                                        │
│                                                            │
│  // Or use high-level API:                                 │
│  AI.chat("prompt", { provider: "claude" })  ✅             │
│  // Rust handles secret automatically                      │
│                                                            │
│  Benefits:                                                 │
│  • No secret value in JavaScript memory ✅                 │
│  • Cannot be logged or inspected ✅                        │
│  • Cannot be accidentally leaked ✅                        │
│  • Cannot appear in stack traces ✅                        │
└────────────────────────┬───────────────────────────────────┘
                         │ Reference by identifier only
                         │
                         ▼
┌────────────────────────────────────────────────────────────┐
│  Rust Layer (Trusted)                                      │
│                                                            │
│  async fn js_fetch(                                        │
│      url: String,                                          │
│      options: FetchOptions                                 │
│  ) -> Result<FetchResponse> {                              │
│                                                            │
│      let mut final_headers = HashMap::new();               │
│                                                            │
│      // 1. Detect template syntax                          │
│      for (key, value) in options.headers {                 │
│          if value.starts_with("{{secret:") {               │
│              let secret_id = extract_id(&value);           │
│                                                            │
│              // 2. Look up secret (stays in Rust)          │
│              let secret_value = SECRETS_MANAGER            │
│                  .get(&secret_id)?;                        │
│              //  ^^^^^^^^^^^^^ Actual value never          │
│              //                crosses to JavaScript        │
│                                                            │
│              // 3. Inject into headers                     │
│              final_headers.insert(key, secret_value);      │
│                                                            │
│              // 4. Audit log (identifier only)             │
│              audit_log::secret_access(&secret_id, &url);   │
│          }                                                 │
│      }                                                     │
│                                                            │
│      // 5. Make HTTP request with injected secret          │
│      let response = HTTP_CLIENT                            │
│          .post(&url)                                       │
│          .headers(final_headers)  ← Secret injected here   │
│          .send()                                           │
│          .await?;                                          │
│                                                            │
│      // 6. Return response (no secrets in it)              │
│      Ok(FetchResponse::from(response))                     │
│  }                                                         │
│                                                            │
│  // Only these functions exposed to JavaScript:            │
│  fn secrets_exists(id: String) -> bool { ... }             │
│  fn secrets_list() -> Vec<String> { ... }                  │
│  // NO secrets_get() that returns values!                  │
└────────────────────────┬───────────────────────────────────┘
                         │
                         ▼
                  External API
              (with secret in request)
```

## API Comparison

### ❌ Insecure (Original)

```javascript
// JavaScript code
const apiKey = Secrets.get("anthropic_api_key");  // Returns "sk-ant-..."
console.log(apiKey);  // ❌ Secret logged!

const response = await fetch("https://api.anthropic.com/v1/messages", {
  headers: {
    "x-api-key": apiKey  // ❌ Secret in JS variable
  }
});
```

**Problems**:
- Secret value exists in JavaScript memory
- Can be logged, inspected, or leaked
- Developer mistakes can expose it
- Violates security requirements

### ✅ Secure (Correct)

```javascript
// JavaScript code - never sees actual secret value

// Option 1: Template syntax
const response = await fetch("https://api.anthropic.com/v1/messages", {
  headers: {
    "x-api-key": "{{secret:anthropic_api_key}}"  // ✅ Just a reference
  }
});

// Option 2: High-level API (best)
const response = await AI.chat("What is Rust?", {
  provider: "claude"  // ✅ Rust handles secret automatically
});

// Check if secret exists (doesn't expose value)
if (Secrets.exists("anthropic_api_key")) {
  // Use feature
} else {
  return Response.json({ error: "AI not configured" }, { status: 503 });
}

// List available secrets (identifiers only)
const secrets = Secrets.list();  // ["anthropic_api_key", "sendgrid_api_key"]
```

**Benefits**:
- Secret value never enters JavaScript
- Impossible to log or leak
- Rust enforces security
- Aligns with requirements

## Trust Boundary

```
┌─────────────────────────────────────────────────────────────┐
│                    UNTRUSTED ZONE                           │
│                                                             │
│  JavaScript Runtime                                         │
│  • User-written scripts                                     │
│  • Can be buggy or malicious                                │
│  • Cannot be trusted with secrets                           │
│  • Only knows secret identifiers                            │
│                                                             │
└─────────────────────────┬───────────────────────────────────┘
                          │
     ═════════════════════╪═══════════════════════════════
     TRUST BOUNDARY       │       ↑
     ═════════════════════╪═══════════════════════════════
                          │       │
                          ▼       │ References only
┌─────────────────────────────────────────────────────────────┐
│                     TRUSTED ZONE                            │
│                                                             │
│  Rust Layer                                                 │
│  • Secure, compiled code                                    │
│  • Type-safe memory management                              │
│  • Enforces security policies                               │
│  • Stores actual secret values                              │
│  • Injects secrets at point of use                          │
│  • Audit logging                                            │
│                                                             │
│  SecretsManager                                             │
│  ┌─────────────────────────────────────────────────┐       │
│  │ secrets: HashMap<String, String>                │       │
│  │ {                                               │       │
│  │   "anthropic_api_key": "sk-ant-api03-abc123",  │       │
│  │   "sendgrid_api_key": "SG.xyz789...",          │       │
│  │ }                                               │       │
│  └─────────────────────────────────────────────────┘       │
│  ↑                                                          │
│  │ Actual values never cross this boundary                 │
│  └─────────────────────────────────────────────────────────┘
```

## Security Attack Vectors

### ❌ Insecure Approach - Attack Vectors

1. **Logging Attack**:
   ```javascript
   const key = Secrets.get("api_key");
   console.log("Using key:", key);  // ❌ Secret logged!
   ```

2. **Error Leak**:
   ```javascript
   const key = Secrets.get("api_key");
   try {
     // Something fails
   } catch (e) {
     throw new Error(`Failed with key ${key}`);  // ❌ Secret in error!
   }
   ```

3. **Response Leak**:
   ```javascript
   const key = Secrets.get("api_key");
   return Response.json({ 
     debug: { apiKey: key }  // ❌ Secret sent to client!
   });
   ```

4. **Debugger Inspection**:
   ```javascript
   const key = Secrets.get("api_key");
   debugger;  // ❌ Developer can inspect 'key' variable
   ```

### ✅ Secure Approach - Attack Vectors Mitigated

1. **Logging Attack** - Mitigated ✅:
   ```javascript
   // No secret value to log!
   fetch(url, { headers: { "x-api-key": "{{secret:key}}" } });
   console.log("Making request");  // ✅ No secret in scope
   ```

2. **Error Leak** - Mitigated ✅:
   ```javascript
   // No secret value to include in error
   try {
     await fetch(url, { headers: { "x-api-key": "{{secret:key}}" } });
   } catch (e) {
     throw new Error("Request failed");  // ✅ No secret accessible
   }
   ```

3. **Response Leak** - Mitigated ✅:
   ```javascript
   // No secret value to send
   const result = await AI.chat(prompt);
   return Response.json({ result });  // ✅ No secret in scope
   ```

4. **Debugger Inspection** - Mitigated ✅:
   ```javascript
   // No secret value in JavaScript memory
   debugger;  // ✅ No secret variables to inspect
   const response = await AI.chat(prompt);  // Secret in Rust only
   ```

## Implementation Patterns

### Pattern 1: High-Level APIs (Recommended)

```javascript
// AI Assistant
const response = await AI.chat("Explain Rust", {
  provider: "claude",  // Rust uses "anthropic_api_key"
  model: "claude-3-haiku-20240307",
  maxTokens: 1024
});

// Future: Email API
const result = await Email.send({
  to: "user@example.com",
  from: "app@example.com",
  subject: "Welcome",
  body: "Hello!",
  provider: "sendgrid"  // Rust uses "sendgrid_api_key"
});

// Future: Payment API
const charge = await Payment.charge({
  amount: 1999,
  currency: "usd",
  source: token,
  provider: "stripe"  // Rust uses "stripe_api_key"
});
```

**Benefits**:
- Most secure - no secret handling in JavaScript at all
- Simplest developer experience
- Consistent API across services

### Pattern 2: Template Syntax (For Custom APIs)

```javascript
// For services without high-level API
const response = await fetch("https://api.custom-service.com/v1/data", {
  method: "POST",
  headers: {
    "authorization": "Bearer {{secret:custom_api_key}}",
    "content-type": "application/json"
  },
  body: JSON.stringify({ query: "..." })
});
```

**Benefits**:
- Flexible for any external API
- Still secure - secret never in JavaScript
- Easy to audit - grep for `{{secret:`

### Pattern 3: Conditional Features (Check Availability)

```javascript
// Check which features are available based on configured secrets
const features = [];

if (Secrets.exists("anthropic_api_key")) {
  features.push("ai_assistant");
}

if (Secrets.exists("sendgrid_api_key")) {
  features.push("email");
}

if (Secrets.exists("stripe_api_key")) {
  features.push("payments");
}

return Response.json({ 
  available_features: features 
});
```

**Benefits**:
- Feature discovery without exposing values
- Graceful degradation
- Clear user feedback

## Summary

### Key Differences

| Aspect | Insecure (❌) | Secure (✅) |
|--------|--------------|-------------|
| Secret in JS memory | Yes - exposed | No - stays in Rust |
| Can be logged | Yes | No |
| Can be debugged | Yes | No |
| Can be leaked | Yes | No |
| Violates REQ-SEC-008 | Yes | No |
| Aligns with "script never sees value" | No | Yes |
| Defense in depth | No | Yes |
| Audit trail | Partial | Complete |
| Developer mistakes | Can expose secrets | Cannot expose secrets |

### The Right Way

1. **Secrets stay in Rust** - Never cross to JavaScript
2. **JavaScript references by ID** - Template syntax or high-level APIs
3. **Rust injects at point of use** - HTTP requests, AI calls, etc.
4. **Audit all usage** - Log identifier access, not values
5. **Fail secure** - Missing secret = error, not exposure

This approach makes it **impossible** for JavaScript code to leak secrets, even accidentally or maliciously.
