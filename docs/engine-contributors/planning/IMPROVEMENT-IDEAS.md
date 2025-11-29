# Engine Improvement Ideas

This document tracks potential improvements to the aiwebengine based on real-world usage patterns and developer experience feedback. Items are organized by priority and include detailed context from actual implementation challenges.

**Source:** Identified during chat application development (November 2025)  
**Status:** Planning - Not yet implemented

---

## Unified Handler Context (Implemented November 2025)

As of November 17, 2025 every JavaScript entry point now receives the same single `context` argument created by `JsHandlerContextBuilder` (`src/js_engine.rs`). This fixes the historical split between `req`/`args`, removes the need for multiple resolver call signatures, and guarantees that connection metadata only requires one user-defined hook.

### Context shape

All handler kinds receive:

- `context.kind`: one of `httpRoute`, `graphqlQuery`, `graphqlMutation`, `graphqlSubscription`, `streamCustomization`, or `init`.
- `context.scriptUri` / `context.handlerName`: useful for logging inside user code.
- `context.request`: normalized request info with `path`, `method`, `headers`, `query`, `form`, `body`, and the full `auth` helper built via `AuthJsApi`. HTTP routes inherit real headers; GraphQL/stream invocations get the synthetic values that match their transport.
- `context.args`: always defined. When no inputs exist the runtime sets this to `null` (HTTP) or `{}` (GraphQL) so handlers never have to guess about optional parameters.
- `context.connectionMetadata`: present only for long-lived transports (GraphQL subscriptions / custom streams) and mirrors the string-only metadata returned by customization hooks.
- `context.meta`: structured metadata the host populates (e.g., `meta.graphql = { fieldName, operation }`, `meta.stream = { path }`).

### HTTP route handlers

- Source: `execute_script_for_request_secure` now builds the full context and passes it as the sole argument.
- Headers and auth helpers are available for every request; `context.args` remains `null` because HTTP handlers consume request data directly from `context.request`.

### GraphQL queries & mutations

- Source: `build_schema` now constructs `GraphqlResolverExecutionParams` which flow through `execute_graphql_resolver` → `JsHandlerContextBuilder`.
- Resolvers receive `context.kind = "graphqlQuery" | "graphqlMutation"`, `context.args` containing the strongly typed field arguments (never omitted), and `context.meta.graphql` describing the field and operation type.
- Legacy two-argument fallbacks have been removed in favour of the single context payload; existing scripts need to destructure `context.request` / `context.args` instead of relying on arity inspection.

### GraphQL subscriptions

- The resolver only runs once per connection. Subscription variables are collected from the GraphQL executor, converted to strings for filter metadata, and attached to the single `context.args` object.
- `execute_stream_customization_function` is still used behind the scenes, but the resolver itself is now the customization hook, meaning there is no second opaque invocation. The metadata returned by the handler is enforced to `HashMap<String, String>` and echoed back via `context.connectionMetadata` for message handlers.
- SSE delivery happens entirely inside Rust; the JavaScript resolver is responsible solely for producing the initial filter map plus any asynchronous side effects.

### Custom stream handlers (non-GraphQL)

- Custom stream customization functions now receive the same `context` object with `meta.stream.path` and an `args` object built from query parameters.
- Returning anything other than string values results in a descriptive runtime error before the connection is accepted.

### Script init() hook

- `call_init_if_exists` now invokes `init(context)` where `context.kind = "init"`, `context.meta` contains `{ "timestamp": <ISO>, "isStartup": bool }`, and auth/request fields are omitted.

This unified shape eliminates the ambiguity called out in earlier sections (missing headers, absent helpers, and multi-call subscription behaviour). The remaining improvement ideas below reference the historical pain points for posterity, but the highest-priority ones (#1–#3) are already closed by this update.

## GraphQL Resolver Parameter Shape – Multiple args vs single object

- **Today’s behaviour:** The engine prefers `resolver(req, args)` but still supports the historical `resolver(args)` or `resolver(req)` fallbacks for legacy scripts. This is implemented in `execute_graphql_resolver` by trying multiple call signatures when `args` is `undefined`.
- **Benefits of separate parameters:**
  - Mirrors the mental model from GraphQL server libraries (`args` vs request context) and keeps mutations/queries aligned with HTTP handlers that only need `req`.
  - Allows the engine to skip constructing/serializing `args` when a field has no inputs, which reduces per-request overhead.
  - Backwards compatibility is easier: scripts that only expect `args` continue to work because `req` is just ignored.
- **Costs:**
  - Subscriptions never receive their variables in `args`, so resolver authors must special-case `req.query` for just that type.
  - Every handler has to branch on whether `args` exists, even though the runtime could always supply an object (possibly empty).
  - Maintaining dual call paths makes error reporting noisy—the engine swallows the first `TypeError` and re-invokes the function, which hides genuine bugs.
- **Single-object alternative:** Introduce a consolidated payload such as `resolver(context)` where `context` contains `{ req, args, fieldName, operationType, connectionMetadata }`. Advantages include consistent ergonomics with HTTP handlers, space to add future metadata, and the ability to populate subscriptions’ arguments once without juggling multiple parameters. The downside is the breaking change: every existing resolver would need to destructure the new object, or the engine would need a long deprecation shim that inspects `resolver.length` and adapts dynamically.
- **Recommended path:** keep `(req, args)` for queries/mutations in the near term but make the objects themselves consistent (always provide `args`—empty object if no inputs; always provide real `req.query`/`req.body` data). For subscriptions, phase in `args` support during the connection setup so resolver code can be uniform. Once that parity exists, revisiting a single-context object becomes easier because there is a predictable payload to wrap.

## High Priority Improvements

### ✅ Completed: Subscription Resolver Parameter Consistency

**Status:** Implemented via the unified handler context (November 17, 2025).

- `GraphqlResolverExecutionParams` now serializes every subscription argument into `context.args` before the resolver runs, matching the shape used for queries/mutations.
- URL query parameters are only used as a transport; the values are normalized into strings for filter metadata and made available through `context.connectionMetadata` as well.
- Legacy `req.query` access continues to work for backwards compatibility, but new code can rely solely on `context.args`.

**Follow-up:** Update the public docs/examples to describe the new ergonomics (tracked under the "Update scripts/tests/docs" todo).

---

### ✅ Completed: Subscription Resolver Single Invocation

**Status:** Delivered alongside the string-only filter enforcement.

- The resolver doubles as the stream customization hook, so it now runs exactly once per connection with fully populated `context.args`, `context.request`, and `context.meta.graphql`.
- The SSE pipeline performs all schema/introspection work without calling back into user code, eliminating the need for defensive `if (!req.query)` guards.
- Error reporting is clearer: any exception thrown during the single invocation is surfaced directly to the client and to the server logs with the field name.

**Follow-up:** Documentation needs to highlight that returning a value from the resolver now solely controls the connection filter; message broadcasting still happens through `sendSubscriptionMessageFiltered`.

---

### ✅ Completed: req.auth API Consistency

- Every invocation path (HTTP, GraphQL queries/mutations/subscriptions, stream customizations) now routes through `JsHandlerContextBuilder::build_request_object`, which always installs the full `AuthJsApi` helper.
- `requireAuth()` and the other helper methods behave identically regardless of transport, so subscription resolvers can drop the manual boolean checks shown in the older examples.
- The auth object also flows into customization hooks, allowing filter logic to short-circuit unauthenticated subscribers before the SSE connection is accepted.

---

## Medium Priority Improvements

### 4. GraphQL Resolver Return Type Validation

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

### 5. sendSubscriptionMessageFiltered Data Format

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

**Option A: Accept Objects (Recommended)**

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

**Option B: Clear Documentation**
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

### 6. SSE Endpoint Documentation and Discovery

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

**1. Documentation**
Add to docs/solution-developers/:

`````markdown
## GraphQL Subscriptions

### Endpoint

`GET /graphql/sse`

### Client Implementation

````javascript
// Pass query and variables as URL query parameters
const channelId = "channel_123";
const query =
  "subscription ($channelId: String!) { chatUpdates(channelId: $channelId) }";
const variables = JSON.stringify({ channelId });

const url = `/graphql/sse?query=${encodeURIComponent(query)}&variables=${encodeURIComponent(variables)}`;

// GET request with query parameters
```javascript
const eventSource = new EventSource(url + '?query=' + encodeURIComponent(query), {
  headers: {
    Accept: "text/event-stream",
  },
});

eventSource.onmessage = function(event) {
  const data = JSON.parse(event.data);
  // Process SSE data...
};
````
`````

```

```

**2. Better Error Messages**

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

**3. Example Scripts**
Add to `scripts/example_scripts/`:

- `graphql_subscription_client.js` - Reusable client helper
- Update existing examples to show subscription usage

**Estimated Effort:** Medium (4-6 hours for docs and error messages)

---

### 7. Subscription Filter Matching Algorithm

**Problem:**
The filter matching mechanism lacks clear documentation:

- Resolver returns object with string values
- Converted to `HashMap<String, String>` internally
- `sendSubscriptionMessageFiltered` matches filters
- Exact matching algorithm unclear
- No examples of complex filtering

**Current Understanding:**

```javascript
// Subscriber A
function resolver(req, args) {
  return { channelId: "channel_1", userId: "user_123" };
}

// Subscriber B
function resolver(req, args) {
  return { channelId: "channel_2" };
}

// Broadcasting
sendSubscriptionMessageFiltered(
  "chatUpdates",
  JSON.stringify(message),
  JSON.stringify({ channelId: "channel_1" }),
);
// Does this match Subscriber A? (has channelId + extra userId)
// Does this match Subscriber B? (different channelId)
```

**Unknown Behaviors:**

- Is it exact match or subset match?
- If subscriber has `{channelId: 'channel_1', userId: 'user_123'}` and broadcast filter is `{channelId: 'channel_1'}`, does it match?
- What about multiple filter criteria - AND or OR logic?
- Can filter values be anything other than strings?
- Are null/undefined values handled specially?

**Suggested Solution:**

**1. Document Algorithm Clearly**

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

````

**2. Add Unit Tests**
Create test cases covering:
- Exact matches
- Subset matches
- No matches
- Edge cases (empty filters, null values, etc.)

**3. Consider Enhanced Filtering**
Future enhancement - support more complex filters:
```javascript
// Pattern matching
{ channelId: 'channel_*' }  // Wildcard

// Multiple values
{ channelId: ['channel_1', 'channel_2'] }  // OR logic

// Negation
{ channelId: '!system' }  // Not system channel
````

**Estimated Effort:** Small (2-3 hours for documentation, more for enhanced filtering)

---

## Low Priority Improvements

### 8. QuickJS Environment - Timer Functions

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

**Option A: Implement Timer APIs**
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

**Option B: Custom Scheduling API**
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

### 9. Enhanced Error Context in Resolvers

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

### 10. GraphQL Playground / IDE Integration

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

## Implementation Priority

### Phase 1: API Consistency (Weeks 1-2)

1. Subscription resolver parameter consistency (#1)
2. req.auth API consistency (#3)
3. Resolver return type validation (#4)

**Rationale:** These directly impact developer experience and API predictability

### Phase 2: Documentation (Week 3)

4. SSE endpoint documentation (#6)
5. Filter matching algorithm docs (#7)
6. sendSubscriptionMessageFiltered clarification (#5)

**Rationale:** Quick wins that prevent future confusion

### Phase 3: Advanced Features (Future)

7. Subscription resolver invocation cleanup (#2)
8. Timer APIs in QuickJS (#8)
9. Enhanced error context (#9)
10. GraphQL Playground (#10)

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

These improvements are sourced from real implementation experience building a production-ready chat application with GraphQL subscriptions, authentication, and persistent storage.

**Primary Pain Points Encountered:**

1. Subscription parameter access (req.query vs args)
2. Multiple resolver invocations without context
3. Authentication API inconsistency
4. Lack of SSE endpoint documentation

**Developer Time Lost:**

- ~4 hours debugging subscription parameters
- ~2 hours figuring out multiple resolver calls
- ~2 hours discovering SSE endpoint details
- ~1 hour on auth API differences

**Total:** ~9 hours that could be saved with these improvements

---

## Related Documents

- [Chat Application Example](../../scripts/example_scripts/chat_app.js) - Real-world implementation
- [GraphQL API Documentation](../solution-developers/graphql-api.md) - Current API docs
- [Authentication Guide](../solution-developers/authentication.md) - Auth patterns

---

**Last Updated:** November 16, 2025  
**Contributor:** Development team feedback from chat application implementation  
**Status:** Planning phase - ready for prioritization and implementation
