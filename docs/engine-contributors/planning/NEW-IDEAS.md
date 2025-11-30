# Engine Improvement Ideas

This document contains ideas for improving the aiwebengine architecture to make implementing features simpler and more efficient. These ideas emerged from implementing the script ownership feature.

## 1. Route Pattern Matching with Priority/Specificity ✅ IMPLEMENTED

**Previous Issue**: Wildcard routes (`/api/scripts/*`) match too greedily, requiring workarounds like separate base paths (`/api/script-owners/*`) or path validation in handlers.

**Problem Example**:

- Route `/api/scripts/*` matches everything, including `/api/scripts/foo/owners`
- More specific routes like `/api/scripts/*/owners` don't get priority
- Forces awkward workarounds (separate base paths, manual path validation)

**Solution Implemented** (November 2025):

Implemented route matching with specificity ordering in `src/lib.rs`:

1. **Specificity Scoring**: Each route pattern gets a score based on:
   - Exact segments × 1000 (e.g., `/api/users`)
   - Parameter segments × 100 (e.g., `:id`)
   - Wildcard depth × -10 (e.g., `/*`)

2. **Route Selection**: When multiple routes match a path, the route with the highest specificity score is selected

3. **Priority Order**: Exact matches > Parameterized matches > Wildcard matches

**Example Behavior**:

```javascript
// These routes now work as expected:
routeRegistry.registerRoute("/api/scripts/*/owners", "manageOwners", "GET"); // Score: 2990
routeRegistry.registerRoute("/api/scripts/*", "getScript", "GET"); // Score: 1990

// Request to /api/scripts/foo/owners → manageOwners (more specific)
// Request to /api/scripts/foo → getScript (only match)
```

**Implementation Details**:

- `calculate_route_specificity()` - Computes specificity score
- `find_route_handler()` - Collects all matching routes, sorts by specificity
- Comprehensive unit tests verify correct behavior

**Benefits Achieved**:

- ✅ More intuitive REST API design
- ✅ No need for separate base paths or validation hacks
- ✅ Standard RESTful patterns work as expected
- ✅ Consistent with Express.js, Flask, Axum routing behavior
- ✅ Script ownership feature can now use natural `/api/scripts/*/owners` paths

**Status**: COMPLETE - Ownership feature can be refactored to use natural paths

---

## 2. HTTP Method-Specific Route Registration

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

## 3. Request Body Handling for All HTTP Methods ✅ IMPLEMENTED

**Previous Issue**: DELETE request bodies were `null` in JavaScript context, even when sent from client.

**Problem Example**:

```javascript
// Client sends DELETE with body
fetch("/api/resource", {
  method: "DELETE",
  body: JSON.stringify({ id: "123" }),
});

// Server receives
req.body === null; // Body was lost!
```

**Root Cause**: The HTTP request handler in `src/lib.rs` was only extracting request bodies for POST/PUT/PATCH methods, explicitly excluding DELETE and other methods.

**Solution Implemented** (November 2025):

Changed `src/lib.rs` line ~1410-1420 to extract request body for ALL HTTP methods:

```rust
// Make raw body available for all requests that might have a body
// Note: While RFC 7231 doesn't explicitly forbid request bodies for DELETE,
// some HTTP clients and proxies may not support it. However, we support it
// for maximum flexibility in API design.
let raw_body = if !body_bytes.is_empty() {
    Some(String::from_utf8(body_bytes.to_vec()).unwrap_or_default())
} else {
    None
};
```

**Benefits Achieved**:

- ✅ Standard RESTful patterns now work (DELETE with body)
- ✅ No need for query parameter workarounds
- ✅ More flexible API design
- ✅ Consistent behavior across all methods
- ✅ Script ownership feature can use proper REST semantics

**Notes**:

- RFC 7231 doesn't prohibit request bodies in DELETE requests
- Most modern HTTP clients support it (fetch, axios, curl)
- Some older proxies/firewalls might strip bodies from DELETE - document this limitation
- Consider adding a warning in API documentation about potential proxy issues

**Status**: COMPLETE - No longer a priority item

---

## 4. Path Parameter Extraction

**Current Issue**: Handlers manually parse paths with error-prone string operations.

**Problem Example**:

```javascript
// Manual parsing in every handler
let scriptName = req.path.replace("/api/scripts/", "");
scriptName = decodeURIComponent(scriptName);

// What if path format changes? All handlers break!
```

**Proposed Solution**: Automatic path parameter extraction with named parameters:

```rust
// Define route with named parameters
router.register("/api/scripts/:scriptName/owners", handler);

// In JavaScript handler:
function handler(context) {
  const req = getRequest(context);
  const scriptName = req.params.scriptName; // Already decoded!
  const ownerId = req.params.ownerId;
}
```

**Implementation Approach**:

1. Parse route patterns for `:paramName` syntax
2. Extract values during route matching
3. Store in `req.params` object
4. Automatic URL decoding

**Benefits**:

- Less error-prone (no manual string manipulation)
- Automatic URL decoding
- Clearer intent in route definitions
- Consistent with Express.js, Actix, Axum, Flask patterns
- Self-documenting routes

**Priority**: HIGH - Would significantly reduce boilerplate and bugs

---

## 5. Query Parameter Parsing

**Current Issue**: Query parameters need manual parsing and null checking.

**Problem Example**:

```javascript
// Manual null checking in every handler
const ownerId = req.query && req.query.ownerId ? req.query.ownerId : null;
if (!ownerId) {
  return { status: 400, body: "Missing ownerId" };
}
```

**Proposed Solution**: Automatically parse query strings into a proper object:

```javascript
// req.query is always an object (never null/undefined)
const ownerId = req.query.ownerId;

// Optional: validation helpers
const ownerId = requireQueryParam(req, "ownerId");
```

**Implementation Details**:

- Parse query string in Rust
- Always provide empty object `{}` if no query params
- Support array parameters: `?tag=foo&tag=bar` → `{ tag: ["foo", "bar"] }`
- Proper URL decoding

**Benefits**:

- Less null checking boilerplate
- Consistent API surface
- Easier to work with
- Reduces bugs from incorrect parsing

**Priority**: MEDIUM - Nice quality of life improvement

---

## 6. Structured Error Responses

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

## 7. Middleware/Interceptor Support

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

## 8. Better Context Propagation

**Current Issue**: UserContext needs to be manually created and passed through multiple layers (HTTP → Rust → JavaScript), leading to bugs like the one fixed during ownership implementation.

**Problem Example**:

```rust
// Bug we had: UserContext was always anonymous
let user_context = UserContext::anonymous();  // Wrong!

// Should have been:
let user_context = if auth_user.is_admin {
  UserContext::admin(auth_user.user_id.clone())
} else {
  UserContext::authenticated(auth_user.user_id.clone())
};
```

**Proposed Solution**: Automatically attach user context to all script executions:

```javascript
function handler(context) {
  const req = getRequest(context);

  // User is always available if authenticated, null if anonymous
  if (req.user) {
    console.log(req.user.id); // Instead of req.auth.userId
    console.log(req.user.isAdmin);
    console.log(req.user.roles); // Future: role-based access
  }
}
```

**Implementation Details**:

1. Extract user info from AuthUser in Rust
2. Serialize to JavaScript context as `req.user` object
3. Always present (null for anonymous, object for authenticated)
4. No manual propagation needed

**Benefits**:

- Simpler API
- User info always consistent
- Less chance of forgetting to check auth
- Prevents bugs from incorrect context creation

**Priority**: MEDIUM - Prevents a class of bugs we encountered

---

## 9. Unified API Response Format

**Current Issue**: Every handler manually constructs response objects with status, body, contentType.

**Problem Example**:

```javascript
// Verbose and error-prone
return {
  status: 200,
  body: JSON.stringify({ message: "Success", data: result }),
  contentType: "application/json", // Easy to forget!
};

// What if you forget contentType? Client gets wrong interpretation!
```

**Proposed Solution**: Response builder helpers:

```javascript
// Instead of verbose object construction, use builders:
return Response.json({ message: "Success", data: result });
return Response.json({ error: "Not found" }, 404);
return Response.text("Hello");
return Response.html("<h1>Hello</h1>");
return Response.redirect("/new-location");
return Response.noContent(); // 204
return Response.error(500, "Internal error");
```

**Implementation Options**:

1. **Global Response object**: Available in all handlers
2. **Return format detection**: Auto-detect JSON objects
3. **Context method**: `context.json(data)`, `context.html(html)`

**Benefits**:

- Less boilerplate
- Harder to forget content-type
- More readable
- Consistent response format
- Type-safe (if using TypeScript for scripts)

**Priority**: MEDIUM - Significant code quality improvement

---

## 10. Database Transaction Support in JavaScript

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

### High Priority (Would save significant time on future features)

1. **Path parameter extraction** - Reduces boilerplate and prevents bugs

### Medium Priority (Reduces boilerplate, improves code quality)

2. **Response builder helpers** - Makes handlers more readable
3. **Query parameter parsing** - Reduces null checking everywhere
4. **Structured error responses** - Consistent error handling
5. **Better context propagation** - Prevents auth bugs

### Low Priority (Architectural improvements for future)

6. **Middleware support** - Better code organization
7. **Database transactions** - Important but complex to implement
8. **Method-specific routing** - Nice to have, not critical

### Completed ✅

- **Route pattern matching with specificity** - Implemented November 2025
- **Request body handling for DELETE** - Implemented November 2025

---

## Implementation Roadmap

### Phase 1: Quick Wins (1-2 weeks)

- Response builder helpers (#9)
- Query parameter parsing (#5)
- Path parameter extraction (#4)

### Phase 2: Routing Improvements (2-3 weeks)

- Route pattern matching with specificity (#1)
- Request body handling for DELETE (#3)
- Method-specific routing (#2)

### Phase 3: Developer Experience (3-4 weeks)

- Structured error responses (#6)
- Better context propagation (#8)

### Phase 4: Advanced Features (4+ weeks)

- Middleware support (#7)
- Database transaction API (#10)

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

1. Prioritize which improvements to implement first
2. Create detailed design documents for high-priority items
3. Prototype route specificity matching
4. Investigate DELETE request body handling in Axum
