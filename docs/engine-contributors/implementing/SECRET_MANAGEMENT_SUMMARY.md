# Secret Management Security - Summary of Changes

**Date**: 2025-10-18  
**Issue**: Critical security flaw in original approach  
**Status**: Analysis complete, implementation plan needs updates

## The Core Problem

**Original Approach** (Insecure):
```javascript
const apiKey = Secrets.get("api_key");  // Returns actual secret value!
fetch(url, { headers: { "x-api-key": apiKey } });  // Secret in JS memory
```

**Why This is Bad**:
- Secret value exists in JavaScript memory
- Can be logged, inspected with debugger, or accidentally leaked
- Violates REQ-SEC-008 (security must be in Rust, not JS)
- Contradicts UC-504 claim: "script never sees the actual value"

## The Correct Approach

**Principle**: Secrets never leave Rust. JavaScript only references them by identifier.

```javascript
// JavaScript only knows the identifier, never the value
fetch(url, {
  headers: {
    "x-api-key": "{{secret:anthropic_api_key}}"  // Template syntax
  }
});

// Or use high-level API that handles secrets automatically
AI.chat("prompt");  // Rust injects the right secret
```

## Required Changes

### 1. JavaScript APIs

**Remove**:
- ❌ `Secrets.get(identifier)` - Never return values to JavaScript

**Keep/Add**:
- ✅ `Secrets.exists(identifier)` - Check if secret is configured
- ✅ `Secrets.list()` - List available secret identifiers

### 2. HTTP Client (fetch)

**Add secret injection** via template syntax:

```javascript
fetch("https://api.anthropic.com/v1/messages", {
  method: "POST",
  headers: {
    "x-api-key": "{{secret:anthropic_api_key}}",  // Rust replaces with value
    "content-type": "application/json"
  },
  body: JSON.stringify({...})
});
```

Rust layer:
1. Detects `{{secret:*}}` pattern in headers
2. Looks up secret from SecretsManager
3. Replaces with actual value
4. Makes HTTP request
5. Returns response (secret never in JavaScript)

### 3. AI API

**No changes needed!** Already secure:

```javascript
// High-level API - JavaScript never sees secret
AI.chat("What is Rust?", {
  provider: "claude",  // Rust knows to use "anthropic_api_key"
  model: "claude-3-haiku-20240307"
});
```

This is the right pattern - Rust handles all secret management internally.

### 4. Documentation Updates

**Use Case UC-504**: Fix example code
- Remove: `const apiKey = Secrets.get(...)`
- Replace with: Template syntax or high-level APIs

**REQ-JSAPI-008**: Remove `Secrets.get()`
- Only expose `exists()` and `list()`
- Never expose `get()`

**REQ-JSAPI-007**: Add secret injection spec
- Document template syntax: `"{{secret:id}}"`
- Alternative: `secrets` parameter in fetch options

**REQ-SEC-005**: Clarify boundary
- Secrets never cross Rust/JavaScript boundary
- JavaScript can only reference by identifier

## Implementation Impact

### Phases Not Affected
- ✅ Phase 1: SecretsManager (Rust side unchanged)
- ✅ Phase 3: AI Integration (already using correct pattern)
- ✅ Phase 4: Editor Backend (uses AI.chat, which is secure)
- ✅ Phase 5: Configuration (no changes)
- ✅ Phase 6: Testing (test secure implementation)

### Phases Requiring Changes
- ⚠️ **Phase 2: HTTP Client** - Add secret injection logic
  - Parse template syntax `{{secret:*}}`
  - Look up and inject secrets in Rust
  - Return error if secret not found
  
### New JavaScript API (Simplified)

```javascript
// Only two secret functions exposed
Secrets.exists("anthropic_api_key")  // Returns: true/false
Secrets.list()                       // Returns: ["anthropic_api_key", ...]

// No Secrets.get() !

// Using secrets in fetch
fetch(url, {
  headers: {
    "authorization": "Bearer {{secret:api_token}}"  // Template
  }
});

// Using secrets in high-level APIs (automatic)
AI.chat(prompt, { provider: "claude" })  // Rust handles secret
Email.send({ to, from, subject, body, provider: "sendgrid" })  // Future
```

## Security Benefits

1. **Impossible to leak secrets from JS** - They're not there
2. **No accidental logging** - Can't log what doesn't exist in JS
3. **No debugger access** - Debugger can't see Rust memory
4. **No response leaks** - Can't return what you never had
5. **Aligns with requirements** - REQ-SEC-008 compliance
6. **Audit trail** - Log all secret usage by identifier
7. **Defense in depth** - Even malicious JS can't access secrets

## Recommendation

**Proceed with secure approach**:

1. Implement `Secrets.exists()` and `Secrets.list()` only
2. Implement template-based secret injection in `fetch()`
3. Use high-level APIs (AI.chat, future Email.send, etc.) where possible
4. Update documentation to remove insecure examples
5. Add tests to verify secrets never enter JavaScript context

**Timeline impact**: Minimal
- Phase 2 implementation slightly more complex (secret injection logic)
- Overall timeline: Still 12-17 days
- Security: Significantly improved ✅

## Next Steps

1. Review and approve security approach
2. Update implementation plan with new Phase 2 details
3. Update UC-504 example code
4. Update REQ-JSAPI-008 specification
5. Begin implementation with secure design

## References

- Full analysis: `SECRET_MANAGEMENT_SECURITY_ANALYSIS.md`
- Original plan: `AI_ASSISTANT_IMPLEMENTATION_PLAN.md`
- Requirements: `docs/engine-contributors/planning/REQUIREMENTS.md`
- Use cases: `docs/engine-contributors/planning/USE_CASES.md`
