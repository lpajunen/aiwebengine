# Engine Improvement Ideas

This document serves as a repository for ideas and suggestions to enhance the engine. Contributors are encouraged to propose new features, improvements, or changes that could benefit the engine and its users.

**Source:** Various development discussions and real-world usage patterns  
**Status:** Planning

---

## Medium Priority Improvements

### 1. API Refactoring

**Problem:**
The API should use as little namespace as possible from the actual scripts and have consistent naming conventions. The API structure should guarantee proper use of privileges and the structure is clear and easy to understand. There are system scripts that have access to all privileges, and user scripts that have limited access based on their assigned roles.

**Current Behavior:**
would it make sense to rename concept script to server script or to server code? at least in the documentation?

the engine is in horizontal lower layer providing services to the upper layer applications. the engine provides services such as storage, routing, logging, events, assets, secrets, identity management, scheduling, etc. the engine also provides an api for the upper layer applications to interact with these services. the engine also provides a way to run user scripts that can use these services and api.

the vertical upper layer applications are the ones that provide the user interface and business logic. these applications can be built using various frameworks and technologies. the engine should provide a way to integrate with these applications seamlessly.

in addition to horizontal lower layer, the engine also provides higher level vertical functions like logging, auditing, monitoring, metrics, tracing, etc. these functions are essential for the proper functioning of the engine and the upper layer applications.

**Impact:**
Inconsistent API usage and documentation confusion.

**Suggested Solution:**
Make the API more consistent and easier to use. Consider renaming concepts for clarity.

**Estimated Effort:** Medium

---

### 2. Transactional Storage Operations

**Problem:**
Need for grouping multiple storage operations into a single atomic operation, handling rollbacks in case of failure, and combining with streams for guaranteed updates.

**Current Behavior:**
No transaction support for storage operations.

**Impact:**
Race conditions and data inconsistency.

**Suggested Solution:**
Implement transactions or transactional storage operations. Handle rollbacks and integrate with streams.

**Estimated Effort:** Large

---

### 3. Security Enhancements

**Problem:**
Improve the security model for scripts and data access, security for business logic, secure storage and logging of sensitive data, audit trails for changes made by scripts.

**Current Behavior:**
Basic security measures in place.

**Impact:**
Potential security vulnerabilities and lack of auditability.

**Suggested Solution:**
Enhance security for business logic, add secure storage, logging, and audit trails.

**Estimated Effort:** Large

---

### 4. Monitoring and Analytics

**Problem:**
How to monitor event chains and script performance? Track full chain of events for debugging and optimization. Add support for prometheus metrics collection from scripts and from engine. Add support for open telemetry tracing from scripts and from engine. Visualize and monitor business logic performance and errors.

**Current Behavior:**
Limited monitoring capabilities.

**Impact:**
Difficulty in debugging and optimizing performance.

**Suggested Solution:**
Add support for prometheus metrics and open telemetry tracing from scripts and engine. Improve visualization of performance and errors.

**Estimated Effort:** Medium

---

### 5. AI Understanding of Scripts

**Problem:**
System prompt allow script generation AI to understand engine APIs. How AI can know about other scripts and their functionality?

**Current Behavior:**
AI lacks context about engine APIs and other scripts.

**Impact:**
Harder for AI to generate accurate scripts.

**Suggested Solution:**
Improve system prompts and provide context for AI to understand APIs and scripts.

**Estimated Effort:** Small

---

### 6. GraphQL Resolver Return Type Validation

**Problem:**
When resolvers incorrectly return JSON strings instead of objects, the error messages are unclear and the behavior is inconsistent.

**Current Behavior:**

```javascript
// Incorrect - returns JSON string
function channelsResolver(req, args) {
  const channels = loadChannels();
  return JSON.stringify(channels); // ✗ Creates parsing issues
}

// Correct - returns object
function channelsResolver(req, args) {
  const channels = loadChannels();
  return channels; // ✓ GraphQL handles serialization
}
```

**What Happens:**

- Sometimes appears to work initially
- Client receives double-encoded JSON
- Parsing errors occur on client side
- Error message doesn't clearly indicate the root cause

**Impact:**

- Wasted debugging time
- Confusing for developers new to GraphQL
- Easy mistake to make coming from REST APIs

**Suggested Solution:**

**Runtime Validation:**

```javascript
// In GraphQL resolver execution
function executeResolver(resolverName, req, args) {
  const result = callUserResolver(resolverName, req, args);

  if (typeof result === "string") {
    // Try to parse it - if it works, it's likely a mistake
    try {
      JSON.parse(result);
      console.error(`Warning: Resolver '${resolverName}' returned a JSON string. 
        Return the object directly instead - GraphQL will handle serialization.`);
    } catch (e) {
      // It's a legitimate string return
    }
  }

  return result;
}
```

**Documentation:**
Add clear examples in docs showing:

- ✓ Correct: `return { id: '123', name: 'Test' }`
- ✗ Incorrect: `return JSON.stringify({ id: '123', name: 'Test' })`

**Estimated Effort:** Small (2-3 hours)

---

### 7. sendSubscriptionMessageFiltered Data Format

**Problem:**
The `sendSubscriptionMessageFiltered` API has ambiguous expectations for the `data` parameter:

- Currently expects a JSON string
- GraphQL framework then parses this string
- Final client receives it wrapped in GraphQL response format
- Creates confusion about who handles serialization

**Current Usage:**

```javascript
function sendMessageResolver(req, args) {
  const message = { id: "123", text: "Hello", sender: "Alice" };

  // Must stringify the data
  const data = JSON.stringify(message);
  const filter = JSON.stringify({ channelId: "channel_1" });

  graphQLRegistry.sendSubscriptionMessageFiltered(
    "chatUpdates",
    data, // JSON string expected
    filter, // JSON string expected
  );
}

// Client receives:
// {"data": {"chatUpdates": {"id": "123", "text": "Hello", "sender": "Alice"}}}
```

**Confusion Points:**

- Why does data need to be stringified?
- Who is responsible for parsing?
- What if data is already a string vs object?

**Suggested Solutions:**

#### Option A: Accept Objects (Recommended)

```rust
// Change signature to accept objects
pub fn send_subscription_message_filtered(
    name: &str,
    data: JsValue,  // Accept JS object directly
    filter: JsValue
) {
    let data_json = JSON::stringify(&data)?;
    let filter_json = JSON::stringify(&filter)?;
    // ... rest of implementation
}
```

#### Option B: Clear Documentation

If there's a reason to keep JSON strings, document it clearly:

```javascript
/**
 * Send filtered subscription message
 * @param {string} name - Subscription name
 * @param {string} data - JSON string of the data (will be parsed and wrapped in GraphQL response)
 * @param {string} filter - JSON string of filter criteria (must match subscriber filters)
 */
```

**Recommended:** Option A - accept objects and handle serialization internally

**Estimated Effort:** Small-Medium (3-5 hours)

---

### 8. SSE Endpoint Documentation and Discovery

**Problem:**
The GraphQL subscription endpoint behavior is undocumented and must be discovered through trial and error:

- Endpoint is `/graphql/sse` (not `/graphql`)
- Must use POST method (not GET)
- Variables must be passed as URL query parameters
- Request body contains the subscription query

**What Developers Try First:**

```javascript
// ✗ Doesn't work - wrong endpoint
const eventSource = new EventSource("/graphql?query=subscription{chatUpdates}");

// ✗ Doesn't work - EventSource only supports GET
// Need to use fetch with ReadableStream instead

// ✓ What actually works
fetch("/graphql/sse?channelId=123", {
  method: "POST",
  headers: { "Content-Type": "application/json" },
  body: JSON.stringify({ query: "subscription { chatUpdates }" }),
});
```

**Impact:**

- Wasted development time
- Trial and error required
- Non-standard compared to other GraphQL implementations
- No clear error messages guide to correct approach

**Suggested Solutions:**

#### 1. Documentation

Add to docs/solution-developers/:

````markdown
## GraphQL Subscriptions

### Endpoint

`GET /graphql/sse`

### Client Implementation

```javascript
// Pass query and variables as URL query parameters
const channelId = "channel_123";
const query =
  "subscription ($channelId: String!) { chatUpdates(channelId: $channelId) }";
const variables = JSON.stringify({ channelId });

const url = `/graphql/sse?query=${encodeURIComponent(query)}&variables=${encodeURIComponent(variables)}`;

// GET request with query parameters
const eventSource = new EventSource(
  url + "?query=" + encodeURIComponent(query),
  {
    headers: {
      Accept: "text/event-stream",
    },
  },
);

eventSource.onmessage = function (event) {
  const data = JSON.parse(event.data);
  // Process SSE data...
};
```
````

#### 2. Better Error Messages

```rust
// When POST /graphql receives subscription query
if query_contains_subscription(&query) {
    return error_response(
        "Subscriptions must use the /graphql/sse endpoint.
         Example: GET /graphql/sse?query=subscription+{+chatUpdates+}&variables={}"
    );
}

// When POST /graphql/sse is used
if method == "POST" {
    return error_response(
        "Subscription endpoint requires GET method.
         Send subscription query as query parameter: /graphql/sse?query=..."
    );
}
```

#### 3. Example Scripts

Add to `scripts/example_scripts/`:

- `graphql_subscription_client.js` - Reusable client helper
- Update existing examples to show subscription usage

**Estimated Effort:** Medium (4-6 hours)

---

### 9. Subscription Filter Matching Algorithm

**Problem:**
The filter matching mechanism lacks clear documentation:

- Resolver returns object with string values
- Converted to `HashMap<String, String>` internally
- `sendSubscriptionMessageFiltered` matches filters
- Exact matching algorithm unclear
- No examples of complex filtering

**Current Understanding:**

```javascript
// Subscriber filter: { channelId: 'channel_1', userId: 'user_123' }
// Broadcast filter: { channelId: 'channel_1' }
// Result: MATCH (broadcast is subset of subscriber)

// Subscriber filter: { channelId: 'channel_1' }
// Broadcast filter: { channelId: 'channel_1', role: 'admin' }
// Result: NO MATCH (broadcast requires role, subscriber doesn't have it)

// Subscriber filter: { channelId: 'channel_2' }
// Broadcast filter: { channelId: 'channel_1' }
// Result: NO MATCH (values don't match)
```

**Unknown Behaviors:**

- Is it exact match or subset match?
- If subscriber has `{channelId: 'channel_1', userId: 'user_123'}` and broadcast filter is `{channelId: 'channel_1'}`, does it match?
- What about multiple filter criteria - AND or OR logic?
- Can filter values be anything other than strings?
- Are null/undefined values handled specially?

**Suggested Solution:**

#### 1. Document Algorithm Clearly

````markdown
## Subscription Filtering

### Matching Algorithm

sendSubscriptionMessageFiltered uses **subset matching**:

- All key-value pairs in the broadcast filter must exist in the subscriber's filter
- Subscriber's filter can have additional pairs (they're ignored)
- All comparisons are case-sensitive string equality

### Examples

```javascript
// Subscriber filter: { channelId: 'channel_1', role: 'admin' }
// Broadcast filter: { channelId: 'channel_1' }
// Result: MATCH (broadcast is subset of subscriber)

// Subscriber filter: { channelId: 'channel_1' }
// Broadcast filter: { channelId: 'channel_1', role: 'admin' }
// Result: NO MATCH (broadcast requires role, subscriber doesn't have it)

// Subscriber filter: { channelId: 'channel_1' }
// Broadcast filter: { channelId: 'channel_2' }
// Result: NO MATCH (values don't match)
```
````

#### 2. Add Unit Tests

Create test cases covering:

- Exact matches
- Subset matches
- No matches
- Edge cases (empty filters, null values, etc.)

#### 3. Consider Enhanced Filtering

Future enhancement - support more complex filters:

```javascript
// Pattern matching
{
  channelId: "channel_*";
} // Wildcard

// Multiple values
{
  channelId: ["channel_1", "channel_2"];
} // OR logic

// Negation
{
  channelId: "!system";
} // Not system channel
```

**Estimated Effort:** Small (2-3 hours for documentation, more for enhanced filtering)

---

### 10. HTTP Method-Specific Route Registration

**Problem:**
Can register same path with different methods, but the routing system doesn't clearly separate them in route matching.

**Current Issue:**

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

**Proposed Solution:**
Make the route registry explicitly method-aware in the matching logic:

```rust
// Rust side
router.register("/api/scripts/*/owners", Method::POST, handler_add);
router.register("/api/scripts/*/owners", Method::DELETE, handler_remove);

// Automatic 405 Method Not Allowed if wrong method used
```

**Benefits:**

- Clearer route definitions
- Automatic 405 Method Not Allowed responses
- Better route listing/debugging
- Routes grouped by path in documentation

**Priority:** LOW - Nice to have, but not critical

**Estimated Effort:** Small

---

### 11. Structured Error Responses

**Problem:**
Error handling is repetitive boilerplate in every handler.

**Current Issue:**

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

**Proposed Solution:**
Helper functions or middleware for common patterns:

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

**Implementation Options:**

1. **Helper functions**: Return error response objects
2. **Exceptions**: Try/catch in handler wrapper, convert to responses
3. **Schema validation**: Zod/Joi-like validation with automatic errors

**Benefits:**

- Less boilerplate (30-50% reduction in handler code)
- Consistent error responses
- Easier to validate inputs
- Self-documenting validation rules

**Priority:** MEDIUM - Significant code quality improvement

**Estimated Effort:** Medium

---

## Low Priority Improvements

### 12. URL Structure Ideas

**Problem:**
Need better URL structure for endpoints.

**Current Behavior:**
Current structure may be confusing.

**Impact:**
Confusing routing for developers.

**Suggested Solution:**

- /auth/... for authentication and authorization related endpoints
  - login, logout, token refresh
  - unauthorized
  - implement OAuth2 / OIDC protocols
  - auth status check
- /engine/... for engine management endpoints
  - /engine/status for engine status and health checks
  - /engine/metrics for metrics
  - /engine/editor for editor operations
    - script management
    - asset management
    - solution secret management (addition to engine secrets in config)
    - log management
  - /engine/graphql for graphql test console
  - /engine/admin for admin operations
    - user management
  - /engine/docs for docs and api reference
  - /engine/cli/... for external cli (deployer) tool operations
  - /engine/api/... for engine related api endpoints
- /graphql/... for GraphQL related endpoints
  - implement GraphQL queries, mutations, subscriptions
- /mcp/... for Model-Context-Protocol related endpoints
  - implement MCP interactions

- everything else is available for user scripts
  - implement HTTP endpoints
  - complete responses and streams
- if there is no script registered for /, redirect to /engine/status or show a default welcome page

**Estimated Effort:** Small

---

### 13. QuickJS Environment - Timer Functions

**Problem:**
Standard JavaScript timer functions are not available in the QuickJS context:

- `setTimeout` is undefined
- `setInterval` is undefined
- `clearTimeout` / `clearInterval` are undefined

**Impact:**

- Cannot schedule delayed operations
- Cannot create periodic tasks
- Must use external workarounds for time-based logic

**Current Workarounds:**

- Schedule tasks outside resolver (e.g., in mutation that triggers later)
- Use database or storage with polling
- Implement state machines without timers

**Example Use Case:**

```javascript
// Wanted: Send delayed notification
function sendMessageResolver(req, args) {
  const message = createMessage(args);
  saveMessage(message);

  // ✗ This doesn't work
  setTimeout(() => {
    sendNotification(message.sender + " sent a message");
  }, 5000);

  return message;
}
```

**Suggested Solutions:**

#### Option A: Implement Timer APIs

Add setTimeout/setInterval to QuickJS runtime:

```rust
// Pseudo-code
impl QuickJsRuntime {
    fn add_timer_apis(&mut self) {
        self.register_function("setTimeout", |callback, delay| {
            // Store callback and schedule execution
        });

        self.register_function("setInterval", |callback, delay| {
            // Schedule recurring execution
        });
    }
}
```

#### Option B: Custom Scheduling API

Provide engine-specific API:

```javascript
// Instead of setTimeout
scheduleTask({
  delay: 5000, // milliseconds
  task: "sendNotification",
  args: { messageId: message.id },
});

// Task executed later, can call registered function
```

**Challenges:**

- Timer callbacks need to be serializable (QuickJS context may not persist)
- Memory management for pending timers
- Cleanup when scripts are reloaded
- Error handling for failed callbacks

**Recommended:** Option B (more predictable in server context)

**Estimated Effort:** Large (12-20 hours)

---

### 14. Enhanced Error Context in Resolvers

**Problem:**
When errors occur in GraphQL resolvers, the error messages and stack traces could be more helpful.

**Current Experience:**

```
Error: Failed to send message
  at sendMessageResolver
```

**Desired Experience:**

```
GraphQLError: Failed to send message: Channel not found
  at sendMessageResolver (chat_app.js:185)
  Field: sendMessage
  Operation: mutation
  Variables: {"channelId":"channel_999","text":"Hello"}
  User: lasse@example.com
  Request ID: req_abc123
```

**Suggested Improvements:**

1. **Automatic Context Injection**
   - Current operation type (query/mutation/subscription)
   - Field name being resolved
   - Input variables (sanitized)
   - User information
   - Request ID for tracing

2. **Structured Error Responses**

   ```javascript
   // In resolver
   throw new GraphQLError("Channel not found", {
     code: "CHANNEL_NOT_FOUND",
     channelId: args.channelId,
     hint: "Use the channels query to see available channels",
   });
   ```

3. **Development vs Production Modes**
   - Dev: Full stack traces, variable values, debug info
   - Prod: Sanitized errors, no sensitive data, request IDs only

**Estimated Effort:** Medium (6-8 hours)

---

### 15. GraphQL Playground / IDE Integration

**Problem:**
No built-in way to explore and test GraphQL API during development.

**Current Workflow:**

- Write JavaScript test code
- Use curl commands
- Build custom test UI

**Suggested Addition:**
Enable GraphQL Playground or GraphiQL at `/graphql/playground` in development mode:

- Schema introspection
- Query/mutation testing
- Subscription testing
- Auto-complete
- Documentation explorer

**Implementation Options:**

1. Embed GraphQL Playground (static HTML/JS)
2. Integrate GraphiQL
3. Build minimal custom explorer

**Estimated Effort:** Medium (8-12 hours)

---

### 16. Middleware/Interceptor Support

**Problem:**
Authorization checks and other cross-cutting concerns are repeated in every handler.

**Current Issue:**

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

**Proposed Solution:**
Route-level or handler-level middleware:

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

**Implementation Approaches:**

1. **Rust middleware**: Wrap handlers before passing to JS
2. **JS middleware**: Higher-order functions that wrap handlers
3. **Declarative**: Annotations or metadata on handlers

**Benefits:**

- DRY principle (don't repeat yourself)
- Centralized security logic
- Less chance of forgetting auth checks
- Easier to audit security
- Composable cross-cutting concerns

**Priority:** LOW - Architectural improvement for future

**Estimated Effort:** Medium

---

### 17. Database Transaction Support in JavaScript

**Problem:**
Multiple database operations (like ownership checks + modifications) aren't atomic, creating race conditions.

**Current Issue:**

```javascript
// Race condition possible!
const ownerCount = countScriptOwners(scriptUri); // Read
if (ownerCount <= 1) {
  throw new Error("Cannot remove last owner");
}
removeScriptOwner(scriptUri, ownerId); // Write

// Another request might remove an owner between these operations!
```

**Proposed Solution:**
Transaction API from JavaScript:

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

**Implementation Challenges:**

1. JavaScript is single-threaded - how to handle async transactions?
2. Connection pooling considerations
3. Deadlock prevention
4. Error handling and rollback semantics

**Benefits:**

- Data consistency
- Proper ACID guarantees
- Prevent race conditions
- Safer concurrent operations

**Priority:** LOW - Important for future, but requires careful design

**Estimated Effort:** Large

---

### 18. Relational Storage API

**Problem:**
Scripts need a more structured way to store persistent data beyond simple key-value storage. Current storage mechanisms may not suffice for complex data relationships and queries.

**Current Behavior:**
Limited to basic storage operations, no support for relational data structures or SQL-like queries.

**Impact:**
Difficulty in managing complex data models, leading to inefficient or error-prone data handling in scripts.

**Suggested Solution:**
Provide a focused relational storage API that allows scripts to:

- Create SQL tables with defined columns (e.g., string, integer, boolean types)
- Perform basic queries (SELECT, INSERT, UPDATE, DELETE) on these tables
- Support simple relationships and constraints where feasible

Avoid full SQL support to maintain simplicity and security, focusing on essential relational features.

**Estimated Effort:** Large

---

## Implementation Priority

For the remaining improvements:

### Phase 1: Developer Experience (Weeks 1-2)

1. GraphQL Resolver Return Type Validation
2. sendSubscriptionMessageFiltered Data Format
3. Structured Error Responses

**Rationale:** These directly impact developer experience and API predictability

### Phase 2: Documentation (Week 3)

1. SSE endpoint documentation
2. Filter matching algorithm docs

**Rationale:** Quick wins that prevent future confusion

### Phase 3: Advanced Features (Future)

1. Timer APIs in QuickJS
2. Enhanced error context
3. GraphQL Playground
4. Middleware support
5. Database transaction API
6. Method-specific routing
7. Relational Storage API

**Rationale:** Nice-to-have improvements for mature product

---

## Testing Strategy

For each improvement:

1. **Create Failing Test**
   - Demonstrates the problem
   - Based on real-world usage patterns

2. **Implement Fix**
   - Minimal change to solve problem
   - Maintain backward compatibility where possible

3. **Verify Test Passes**
   - Original failing test now passes
   - No regressions in existing tests

4. **Update Documentation**
   - API reference
   - Migration guide (if breaking change)
   - Example code

5. **Add to Example Scripts**
   - Show correct usage
   - Demonstrate new patterns

---

## Breaking Change Policy

For improvements requiring breaking changes:

1. **Add deprecation warning** (v1.x)
   - Keep old behavior working
   - Log deprecation notice
   - Document migration path

2. **Support both patterns** (v1.x+1)
   - Old pattern: deprecated but works
   - New pattern: recommended

3. **Remove old pattern** (v2.0)
   - Only new pattern works
   - Clear migration docs
   - Changelog with examples

---

## Contribution Notes

These improvements are sourced from real implementation experience building production-ready applications with GraphQL subscriptions, authentication, and persistent storage.

**Primary Pain Points Encountered:**

1. Subscription parameter access (req.query vs args)
2. Multiple resolver invocations without context
3. Authentication API inconsistency
4. Lack of SSE endpoint documentation
5. Script ownership and authorization patterns

**Developer Time Lost:**

- ~4 hours debugging subscription parameters
- ~2 hours figuring out multiple resolver calls
- ~2 hours on auth API differences
- ~1 hour on SSE endpoint details
- ~3 hours on script ownership implementation

**Total:** ~12 hours that could be saved with these improvements

---

## Related Documents

- [Chat Application Example](../../scripts/example_scripts/chat_app.js) - Real-world implementation
- [GraphQL API Documentation](../solution-developers/graphql-api.md) - Current API docs
- [Authentication Guide](../solution-developers/authentication.md) - Auth patterns
- [Script Ownership Implementation](../engine-contributors/script-ownership.md) - Recent feature

---

**Last Updated:** December 2, 2025  
**Contributor:** Development team feedback from various implementations  
**Status:** Planning phase - ready for prioritization and implementation
