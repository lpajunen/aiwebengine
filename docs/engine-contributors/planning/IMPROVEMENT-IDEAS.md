# Engine Improvement Ideas

This document tracks potential improvements to the aiwebengine based on real-world usage patterns and developer experience feedback. Items are organized by priority and include detailed context from actual implementation challenges.

**Source:** Identified during chat application development (November 2025)  
**Status:** Planning - Partially implemented (high-priority API consistency issues resolved)

---

## Medium Priority Improvements

### 1. GraphQL Resolver Return Type Validation

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

### 2. sendSubscriptionMessageFiltered Data Format

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

### 3. SSE Endpoint Documentation and Discovery

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
````

#### 3. Example Scripts

Add to `scripts/example_scripts/`:

- `graphql_subscription_client.js` - Reusable client helper
- Update existing examples to show subscription usage

**Estimated Effort:** Medium (4-6 hours)

---

### 4. Subscription Filter Matching Algorithm

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
{ channelId: 'channel_*' }  // Wildcard

// Multiple values
{ channelId: ['channel_1', 'channel_2'] }  // OR logic

// Negation
{ channelId: '!system' }  // Not system channel
````

**Estimated Effort:** Small (2-3 hours for documentation, more for enhanced filtering)

---

## Low Priority Improvements

### 5. QuickJS Environment - Timer Functions

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

### 6. Enhanced Error Context in Resolvers

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

### 7. GraphQL Playground / IDE Integration

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

For the remaining improvements:

### Phase 1: API Consistency (Weeks 1-2)

1. GraphQL Resolver Return Type Validation
2. sendSubscriptionMessageFiltered Data Format

**Rationale:** These directly impact developer experience and API predictability

### Phase 2: Documentation (Week 3)

3. SSE endpoint documentation
4. Filter matching algorithm docs

**Rationale:** Quick wins that prevent future confusion

### Phase 3: Advanced Features (Future)

5. Timer APIs in QuickJS
6. Enhanced error context
7. GraphQL Playground

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
- ~2 hours on auth API differences
- ~1 hour on SSE endpoint details

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
