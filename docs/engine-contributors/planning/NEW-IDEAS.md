# Engine Improvement Ideas

This document contains ideas for improving the aiwebengine architecture to make implementing features simpler and more efficient. These ideas emerged from implementing the script ownership feature.

## 1. HTTP Method-Specific Route Registration

**Current Issue**: Can register same path with different methods, but the routing system doesn't clearly separate them in route matching.

**Problem Example**:

```javascript
// Both registered on same path, differentiation happens in handler
routeRegistry.registerRoute(
  "/api/scripts/*/owners",
  "apiAddScriptOwner",
  "POST",
);
routeRegistry.registerRoute(
  "/api/scripts/*/owners",
  "apiRemoveScriptOwner",
  "DELETE",
);
```

**Proposed Solution**: Make the route registry explicitly method-aware in the matching logic:

```rust
// Rust side
router.register("/api/scripts/*/owners", Method::POST, handler_add);
router.register("/api/scripts/*/owners", Method::DELETE, handler_remove);

// Automatic 405 Method Not Allowed if wrong method used
```

**Benefits**:

- Clearer route definitions
- Automatic 405 Method Not Allowed responses
- Better route listing/debugging
- Routes grouped by path in documentation

**Priority**: LOW - Nice to have, but not critical

---

## 2. Structured Error Responses

**Current Issue**: Error handling is repetitive boilerplate in every handler.

**Problem Example**:

```javascript
// Repeated in every handler
if (!payload.ownerId) {
  return {
    status: 400,
    body: JSON.stringify({ error: "ownerId required" }),
    contentType: "application/json",
  };
}

if (typeof payload.ownerId !== "string") {
  return {
    status: 400,
    body: JSON.stringify({ error: "ownerId must be string" }),
    contentType: "application/json",
  };
}
```

**Proposed Solution**: Helper functions or middleware for common patterns:

```javascript
// Built-in validation helpers
const payload = requireJsonBody(req, {
  ownerId: "string", // Throws 400 if invalid
});

const ownerId = requireQueryParam(req, "ownerId");

// Or exceptions that convert to proper responses
throw new BadRequestError("ownerId is required");
throw new UnauthorizedError("Admin access required");
throw new NotFoundError("Script not found");
```

**Implementation Options**:

1. **Helper functions**: Return error response objects
2. **Exceptions**: Try/catch in handler wrapper, convert to responses
3. **Schema validation**: Zod/Joi-like validation with automatic errors

**Benefits**:

- Less boilerplate (30-50% reduction in handler code)
- Consistent error responses
- Easier to validate inputs
- Self-documenting validation rules

**Priority**: MEDIUM - Significant code quality improvement

---

## 3. Middleware/Interceptor Support

**Current Issue**: Authorization checks and other cross-cutting concerns are repeated in every handler.

**Problem Example**:

```javascript
// Repeated in every handler that needs auth
function apiAddScriptOwner(context) {
  const req = getRequest(context);

  // Check if user is admin or owner (repeated everywhere!)
  const isAdmin = /* ... */;
  const userOwns = /* ... */;
  if (!isAdmin && !userOwns) {
    return { status: 403, body: "Permission denied" };
  }

  // Actual handler logic...
}
```

**Proposed Solution**: Route-level or handler-level middleware:

```rust
// Rust-side route registration with middleware
router.register("/api/scripts/*", handler)
  .require_auth()
  .require_role("editor");

router.register("/api/script-owners/*", handler)
  .require_ownership_or_admin(extract_script_from_path);
```

Or JavaScript-side:

```javascript
// Middleware composition
function apiAddScriptOwner(context) {
  return withAuth(
    withOwnershipCheck(scriptNameExtractor),
    actualHandler,
  )(context);
}
```

**Implementation Approaches**:

1. **Rust middleware**: Wrap handlers before passing to JS
2. **JS middleware**: Higher-order functions that wrap handlers
3. **Declarative**: Annotations or metadata on handlers

**Benefits**:

- DRY principle (don't repeat yourself)
- Centralized security logic
- Less chance of forgetting auth checks
- Easier to audit security
- Composable cross-cutting concerns

**Priority**: LOW - Architectural improvement for future

---

## 4. Database Transaction Support in JavaScript

**Current Issue**: Multiple database operations (like ownership checks + modifications) aren't atomic, creating race conditions.

**Problem Example**:

```javascript
// Race condition possible!
const ownerCount = countScriptOwners(scriptUri); // Read
if (ownerCount <= 1) {
  throw new Error("Cannot remove last owner");
}
removeScriptOwner(scriptUri, ownerId); // Write

// Another request might remove an owner between these operations!
```

**Proposed Solution**: Transaction API from JavaScript:

```javascript
await db.transaction(async (tx) => {
  const ownerCount = await tx.countScriptOwners(scriptUri);
  if (ownerCount <= 1) {
    throw new Error("Cannot remove last owner");
  }
  await tx.removeScriptOwner(scriptUri, ownerId);
  // Automatic rollback on error, commit on success
});
```

**Implementation Challenges**:

1. JavaScript is single-threaded - how to handle async transactions?
2. Connection pooling considerations
3. Deadlock prevention
4. Error handling and rollback semantics

**Benefits**:

- Data consistency
- Proper ACID guarantees
- Prevent race conditions
- Safer concurrent operations

**Priority**: LOW - Important for future, but requires careful design

---

## Priority Summary

### Medium Priority (Reduces boilerplate, improves code quality)

1. **Structured error responses** - Consistent error handling

### Low Priority (Architectural improvements for future)

1. **Middleware support** - Better code organization
2. **Database transactions** - Important but complex to implement
3. **Method-specific routing** - Nice to have, not critical

### Completed ✅

- **Route pattern matching with specificity** - Implemented November 2025
- **Request body handling for DELETE** - Implemented November 2025
- **Path parameter extraction** - Implemented November 2025
- **Query parameter parsing** - Implemented November 2025
- **Response builder helpers** - Implemented November 2025
- **Better context propagation** - Implemented November 2025

---

## Implementation Roadmap

### Phase 1: Developer Experience (2-3 weeks)

- Structured error responses (#2)

### Phase 2: Advanced Features (4+ weeks)

- Middleware support (#3)
- Database transaction API (#4)

### Phase 3: Nice-to-Have (1-2 weeks)

- Method-specific routing (#1)

---

## Related Issues

- Script ownership implementation revealed routing limitations
- DELETE request body issue needs investigation in Axum/Tower layer
- UserContext propagation bug fixed but architectural improvement needed

---

## Contributors

Ideas collected during script ownership feature implementation (November 2025).

---

## Next Steps

1. Prioritize structured error responses for biggest developer impact
2. Create detailed design document for exception-based error handling
3. Consider middleware support for complex authorization scenarios
4. Evaluate database transaction needs for future features
