# Script Development Guide

Complete guide to creating and managing JavaScript scripts in aiwebengine.

## Table of Contents

- [Script Basics](#script-basics)
- [Script Structure](#script-structure)
- [Handler Functions](#handler-functions)
- [Route Registration](#route-registration)
- [Request Handling](#request-handling)
- [Response Formatting](#response-formatting)
- [State Management](#state-management)
- [Error Handling](#error-handling)
- [Best Practices](#best-practices)
- [Advanced Patterns](#advanced-patterns)

## Script Basics

### What is a Script?

A script in aiwebengine is a JavaScript file that:

- Defines handler functions to process HTTP requests
- Registers routes to map URLs to handlers
- Can manage state, make external API calls, and serve content
- Runs in a secure QuickJS JavaScript environment

### Minimal Script Example

```javascript
function helloHandler(context) {
  const req = context.request;

  return {
    status: 200,
    body: "Hello, World!",
    contentType: "text/plain; charset=UTF-8",
  };
}

function init() {
  routeRegistry.registerRoute("/hello", "helloHandler", "GET");
}

init();
```

### Script Lifecycle

```text
1. Script Created     → JavaScript file written
2. Script Loaded      → Engine reads the file
3. Script Executed    → JavaScript code runs
4. init() Called      → Routes registered
5. Ready for Requests → Handlers respond to HTTP requests
```

## Script Structure

### Recommended Structure

return { valid: true };
}

function createResponse(status, data) {
return {
status: status,
body: JSON.stringify(data),
contentType: "application/json",
};
}

// ============================================
// Handler Functions
// ============================================

function listItemsHandler(context) {
console.log(`Listing items: ${items.length} total`);
return createResponse(200, { items: items });
}

function createItemHandler(context) {
const req = context.request;

const item = {
id: nextId++,
name: req.form.name,
created: new Date().toISOString(),
};

const validation = validateItem(item);
if (!validation.valid) {
return createResponse(400, { error: validation.error });
}

items.push(item);
console.log(`Item created: ${item.id}`);

return createResponse(201, { item: item });
}

// ============================================
// Initialization
// ============================================

function init() {
// Register routes
routeRegistry.registerRoute("/api/items", "listItemsHandler", "GET");
routeRegistry.registerRoute("/api/items", "createItemHandler", "POST");

// Log initialization
console.log("Items API initialized");
}

// ============================================
// Execute Initialization
// ============================================

init();

````

### Key Sections

1. **Header Comment** - Documentation
2. **Constants** - Configuration values
3. **Data Storage** - Global state (if needed)
4. **Helpers** - Utility functions
5. **Handlers** - Request processors
6. **Initialization** - Route registration
7. **Init Call** - Execute setup

## Handler Functions

### Handler Signature

All handlers receive a single `context` object. Most HTTP handlers immediately alias `context.request` so existing patterns using `req` stay familiar:

```javascript
function handlerName(context) {
  const req = context.request;

  return {
    status: 200,
    body: "response content",
    contentType: "text/plain",
  };
}
````

### Context Overview

Route handlers have access to the following `context` fields:

- `request`: HTTP method, path, headers, query params, form/body
- `args`: GraphQL or command arguments (null for plain HTTP routes)
- `kind`: invocation type (`httpRoute`, `graphqlQuery`, etc.)
- `scriptUri` / `handlerName`: metadata about the executing script
- `meta`, `connectionMetadata`: populated for streaming/subscription handlers

### Request Object Structure (context.request)

The `context.request` object contains:

```javascript
{
  method: "GET",           // HTTP method
  path: "/api/users",      // Request path
  query: {                 // Query parameters
    page: "1",
    limit: "10"
  },
  form: {                  // Form data (POST/PUT)
    name: "John",
    email: "john@example.com"
  },
  headers: {               // Request headers
    "content-type": "application/json",
    "user-agent": "Mozilla/5.0..."
  }
}
```

### Response Object Structure

Handlers must return:

```javascript
{
  status: 200,                    // HTTP status code (required)
  body: "Response content",       // Response body (required)
  contentType: "text/plain"       // MIME type (optional, defaults to text/plain)
}
```

### Handler Examples

**Simple text response:**

```javascript
function textHandler(context) {
  return {
    status: 200,
    body: "Plain text response",
    contentType: "text/plain; charset=UTF-8",
  };
}
```

**JSON API response:**

```javascript
function jsonHandler(context) {
  const data = {
    message: "Success",
    timestamp: new Date().toISOString(),
    data: [1, 2, 3],
  };

  return {
    status: 200,
    body: JSON.stringify(data),
    contentType: "application/json",
  };
}
```

**HTML page:**

```javascript
function htmlHandler(context) {
  const html = `
    <!DOCTYPE html>
    <html>
    <head>
      <title>My Page</title>
      <link rel="stylesheet" href="/style.css">
    </head>
    <body>
      <h1>Welcome</h1>
      <p>This is a dynamic page.</p>
    </body>
    </html>
  `;

  return {
    status: 200,
    body: html,
    contentType: "text/html; charset=UTF-8",
  };
}
```

**Error response:**

```javascript
function errorHandler(context) {
  return {
    status: 404,
    body: JSON.stringify({ error: "Resource not found" }),
    contentType: "application/json",
  };
}
```

## Route Registration

### The `routeRegistry.registerRoute()` Function

```javascript
routeRegistry.registerRoute(path, handlerName, method);
```

**Parameters:**

- `path` (string) - URL path starting with `/`
- `handlerName` (string) - Name of the handler function
- `method` (string) - HTTP method: `"GET"`, `"POST"`, `"PUT"`, `"DELETE"`, `"PATCH"`

### Route Specificity and Matching

When multiple routes could match the same request path, the engine selects the **most specific** route:

**Priority Order:**

1. **Exact matches** - Highest priority
2. **Parameterized routes** (`:param`) - Medium priority
3. **Wildcard routes** (`/*`) - Lowest priority

**Specificity Scoring:**

Routes are scored based on their pattern:

- Each exact path segment: +1000 points
- Each parameter segment (`:param`): +100 points
- Wildcard depth: -10 points per level

**Example:**

```javascript
function init() {
  // Register multiple overlapping routes
  routeRegistry.registerRoute("/api/scripts/*", "getScript", "GET"); // Score: 1990
  routeRegistry.registerRoute("/api/scripts/*/owners", "manageOwners", "GET"); // Score: 2990
  routeRegistry.registerRoute("/api/scripts/:name", "getByName", "GET"); // Score: 2100
  routeRegistry.registerRoute("/api/scripts/search", "search", "GET"); // Score: 3000
}

// Request routing:
// GET /api/scripts/search        → search (exact match, highest score)
// GET /api/scripts/my-script     → getByName (param match)
// GET /api/scripts/foo/owners    → manageOwners (specific wildcard)
// GET /api/scripts/foo/bar       → getScript (general wildcard)
```

**Best Practices:**

- ✅ Register specific routes before general ones (order doesn't matter, specificity wins)
- ✅ Use exact paths for well-known endpoints (`/api/scripts/search`)
- ✅ Use parameters for dynamic segments (`/api/scripts/:name`)
- ✅ Use wildcards sparingly for catch-all handlers
- ❌ Avoid overlapping wildcards that could cause confusion

### Registration Examples

**Basic route:**

```javascript
routeRegistry.registerRoute("/api/hello", "helloHandler", "GET");
```

**Multiple methods on same path:**

```javascript
routeRegistry.registerRoute("/api/users", "listUsers", "GET");
routeRegistry.registerRoute("/api/users", "createUser", "POST");
routeRegistry.registerRoute("/api/users", "updateUser", "PUT");
routeRegistry.registerRoute("/api/users", "deleteUser", "DELETE");
```

**RESTful API:**

```javascript
function init() {
  // Collection endpoints
  routeRegistry.registerRoute("/api/users", "listUsers", "GET");
  routeRegistry.registerRoute("/api/users", "createUser", "POST");

  // Resource endpoints
  routeRegistry.registerRoute("/api/users/:id", "getUser", "GET");
  routeRegistry.registerRoute("/api/users/:id", "updateUser", "PUT");
  routeRegistry.registerRoute("/api/users/:id", "deleteUser", "DELETE");
}
```

Note: Path parameters like `:id` are now automatically extracted and available via `req.params`. You can also use query parameters for additional filtering:

```javascript
// New approach - access path parameters directly
routeRegistry.registerRoute("/api/users/:id", "getUser", "GET");

function getUser(context) {
  const req = context.request;
  const id = req.params.id; // Access path parameter directly
  // ... use id to fetch user
}

// Query parameters still work for additional filtering
routeRegistry.registerRoute("/api/users/:id/posts", "getUserPosts", "GET");

function getUserPosts(context) {
  const req = context.request;
  const userId = req.params.id; // Path parameter
  const page = req.query.page || "1"; // Query parameter
  const limit = req.query.limit || "10"; // Query parameter
  // ... fetch posts for user with pagination
}
```

### Route Organization

**Organize by feature:**

```javascript
function init() {
  // User routes
  routeRegistry.registerRoute("/api/users", "listUsers", "GET");
  routeRegistry.registerRoute("/api/users", "createUser", "POST");

  // Product routes
  routeRegistry.registerRoute("/api/products", "listProducts", "GET");
  routeRegistry.registerRoute("/api/products", "createProduct", "POST");

  // Page routes
  routeRegistry.registerRoute("/", "homePage", "GET");
  routeRegistry.registerRoute("/about", "aboutPage", "GET");
}
```

## Request Handling

### Query Parameters

Access via `context.request.query` (or alias to `req`):

```javascript
function searchHandler(context) {
  const req = context.request;
  const query = req.query.q || "";
  const page = parseInt(req.query.page || "1");
  const limit = parseInt(req.query.limit || "10");

  console.log(`Search: q="${query}", page=${page}, limit=${limit}`);

  // Perform search...
  const results = performSearch(query, page, limit);

  return {
    status: 200,
    body: JSON.stringify(results),
    contentType: "application/json",
  };
}

routeRegistry.registerRoute("/search", "searchHandler", "GET");
// Test: /search?q=javascript&page=2&limit=20
```

### Form Data (POST/PUT)

Access via `context.request.form`:

```javascript
function createUserHandler(context) {
  const req = context.request;
  const name = req.form.name;
  const email = req.form.email;
  const age = parseInt(req.form.age || "0");

  // Validate
  if (!name || !email) {
    return {
      status: 400,
      body: JSON.stringify({ error: "Name and email required" }),
      contentType: "application/json",
    };
  }

  // Create user
  const user = { id: generateId(), name, email, age };
  saveUser(user);

  return {
    status: 201,
    body: JSON.stringify({ user: user }),
    contentType: "application/json",
  };
}

routeRegistry.registerRoute("/api/users", "createUserHandler", "POST");
```

### JSON Request Body

Parse JSON from form data:

```javascript
function apiHandler(context) {
  const req = context.request;
  try {
    // If client sends JSON with Content-Type: application/json
    // It may be available in req.form as a single key
    const jsonData = req.form.body ? JSON.parse(req.form.body) : req.form;

    console.log(`Received data: ${JSON.stringify(jsonData)}`);

    return {
      status: 200,
      body: JSON.stringify({ received: jsonData }),
      contentType: "application/json",
    };
  } catch (error) {
    return {
      status: 400,
      body: JSON.stringify({ error: "Invalid JSON" }),
      contentType: "application/json",
    };
  }
}
```

### Headers

Access request headers via `context.request.headers`:

```javascript
function headerHandler(context) {
  const req = context.request;
  const userAgent = req.headers["user-agent"] || "Unknown";
  const contentType = req.headers["content-type"] || "None";
  const authHeader = req.headers["authorization"] || "";

  console.log(`User-Agent: ${userAgent}`);

  return {
    status: 200,
    body: JSON.stringify({
      userAgent: userAgent,
      contentType: contentType,
      hasAuth: authHeader.length > 0,
    }),
    contentType: "application/json",
  };
}
```

## Response Formatting

### HTTP Status Codes

Use appropriate status codes:

**Success:**

- `200` - OK (successful GET, PUT, DELETE)
- `201` - Created (successful POST)
- `204` - No Content (successful but no body)

**Client Errors:**

- `400` - Bad Request (invalid input)
- `401` - Unauthorized (authentication required)
- `403` - Forbidden (insufficient permissions)
- `404` - Not Found (resource doesn't exist)
- `405` - Method Not Allowed (wrong HTTP method)
- `422` - Unprocessable Entity (validation failed)

**Server Errors:**

- `500` - Internal Server Error (unexpected error)
- `502` - Bad Gateway (upstream service error)
- `503` - Service Unavailable (temporary)

### Content Types

Common MIME types:

```javascript
// Text
contentType: "text/plain; charset=UTF-8";
contentType: "text/html; charset=UTF-8";
contentType: "text/css";

// Application
contentType: "application/json";
contentType: "application/xml";
contentType: "application/pdf";
contentType: "application/javascript";

// Images
contentType: "image/jpeg";
contentType: "image/png";
contentType: "image/gif";
contentType: "image/svg+xml";
```

### Response Builders

aiwebengine provides convenient helper functions for creating common response types:

```javascript
// JSON responses
return ResponseBuilder.json({ users: ["Alice", "Bob"] });
return ResponseBuilder.json({ error: "Not found" }, 404);

// Text responses
return ResponseBuilder.text("Hello, World!");

// HTML responses
return ResponseBuilder.html("<h1>Welcome</h1>");

// Error responses
return ResponseBuilder.error(400, "Invalid input");

// No content (204)
return ResponseBuilder.noContent();

// Redirects
return ResponseBuilder.redirect("/new-location", 301);
```

### Response Helper

Create a helper function:

```javascript
function jsonResponse(status, data) {
  return {
    status: status,
    body: JSON.stringify(data),
    contentType: "application/json",
  };
}

function textResponse(status, text) {
  return {
    status: status,
    body: text,
    contentType: "text/plain; charset=UTF-8",
  };
}

function errorResponse(status, message) {
  return jsonResponse(status, { error: message });
}

// Usage
function myHandler(context) {
  return jsonResponse(200, { message: "Success" });
}
```

## State Management

### In-Memory Storage

Scripts can maintain state between requests:

```javascript
// Global variables persist across requests
let counter = 0;
let users = [];
let cache = {};

function incrementHandler(context) {
  counter++;
  return jsonResponse(200, { counter: counter });
}

function resetHandler(context) {
  counter = 0;
  return jsonResponse(200, { counter: counter });
}
```

**Important:** State is lost when:

- Server restarts
- Script is reloaded
- Script is updated

### Session-like Storage

```javascript
const sessions = {};

function loginHandler(context) {
  const req = context.request;
  const sessionId = generateSessionId();
  sessions[sessionId] = {
    user: req.form.username,
    created: Date.now(),
  };

  return jsonResponse(200, { sessionId: sessionId });
}

function getUserHandler(context) {
  const req = context.request;
  const sessionId = req.headers["x-session-id"];
  const session = sessions[sessionId];

  if (!session) {
    return errorResponse(401, "Invalid session");
  }

  return jsonResponse(200, { user: session.user });
}
```

### Caching Pattern

```javascript
const cache = {};
const CACHE_TTL = 60000; // 60 seconds

function getCachedData(key) {
  const cached = cache[key];
  if (cached && Date.now() - cached.timestamp < CACHE_TTL) {
    return cached.data;
  }
  return null;
}

function setCachedData(key, data) {
  cache[key] = {
    data: data,
    timestamp: Date.now(),
  };
}

function apiHandler(context) {
  const req = context.request;
  const cacheKey = `users_${req.query.page || 1}`;

  // Check cache
  let data = getCachedData(cacheKey);

  if (!data) {
    // Fetch fresh data
    data = fetchUsers(req.query.page);
    setCachedData(cacheKey, data);
  }

  return jsonResponse(200, data);
}
```

## Error Handling

### Try-Catch Pattern

Always wrap risky operations:

```javascript
function riskyHandler(context) {
  const req = context.request;
  try {
    // Operations that might fail
    const data = JSON.parse(req.form.data);
    const result = processData(data);

    return jsonResponse(200, { result: result });
  } catch (error) {
    console.error(`Error in riskyHandler: ${error.message}`);
    return errorResponse(500, "Internal server error");
  }
}
```

### Validation

Validate all inputs:

```javascript
function createItemHandler(context) {
  const req = context.request;
  // Validate required fields
  if (!req.form.name) {
    return errorResponse(400, "Name is required");
  }

  if (!req.form.email) {
    return errorResponse(400, "Email is required");
  }

  // Validate format
  if (!isValidEmail(req.form.email)) {
    return errorResponse(400, "Invalid email format");
  }

  // Validate length
  if (req.form.name.length > 100) {
    return errorResponse(400, "Name too long (max 100 characters)");
  }

  // Process valid data
  const item = createItem(req.form);
  return jsonResponse(201, { item: item });
}

function isValidEmail(email) {
  return /^[^\s@]+@[^\s@]+\.[^\s@]+$/.test(email);
}
```

### Centralized Error Handler

```javascript
function handleError(error, context) {
  const errorId = Date.now().toString(36);
  console.error(`[${errorId}] Error in ${context}: ${error.message}`);

  return {
    status: 500,
    body: JSON.stringify({
      error: "Internal server error",
      errorId: errorId,
    }),
    contentType: "application/json",
  };
}

function myHandler(context) {
  try {
    // Your logic
    return jsonResponse(200, { success: true });
  } catch (error) {
    return handleError(error, "myHandler");
  }
}
```

## Best Practices

### 1. Use Descriptive Names

**Good:**

```javascript
function createUserHandler(context) {}
function getUserByIdHandler(context) {}
function updateUserEmailHandler(context) {}
```

**Bad:**

```javascript
function handler1(context) {}
function func(context) {}
function process(context) {}
```

### 2. Validate All Inputs

```javascript
function safeHandler(context) {
  const req = context.request;
  // Check required parameters
  if (!req.query.id) {
    return errorResponse(400, "Missing id parameter");
  }

  // Validate types
  const id = parseInt(req.query.id);
  if (isNaN(id)) {
    return errorResponse(400, "Invalid id format");
  }

  // Check ranges
  if (id < 1 || id > 1000000) {
    return errorResponse(400, "ID out of range");
  }

  // Process validated data
  return processId(id);
}
```

### 3. Log Important Events

```javascript
function handler(context) {
  const req = context.request;
  console.log(`Request started: ${req.path}`);

  try {
    const result = doSomething();
    console.log(`Request completed successfully`);
    return jsonResponse(200, result);
  } catch (error) {
    console.error(`Request failed: ${error.message}`);
    return errorResponse(500, "Internal error");
  }
}
```

### 4. Keep Handlers Focused

**Good - Single Responsibility:**

```javascript
function listUsers(context) {
  const users = getAllUsers();
  return jsonResponse(200, { users: users });
}

function createUser(context) {
  const user = buildUserFromForm(context.request.form);
  saveUser(user);
  return jsonResponse(201, { user: user });
}
```

**Bad - Too Much in One Handler:**

```javascript
function usersHandler(context) {
  const req = context.request;
  if (req.method === "GET") {
    // List logic
  } else if (req.method === "POST") {
    // Create logic
  } else if (req.method === "PUT") {
    // Update logic
  } else if (req.method === "DELETE") {
    // Delete logic
  }
  // Too complex!
}
```

### 5. Use Helper Functions

```javascript
// Helpers
function validateEmail(email) {
  return /^[^\s@]+@[^\s@]+\.[^\s@]+$/.test(email);
}

function generateId() {
  return Date.now().toString(36) + Math.random().toString(36).substr(2);
}

function sanitizeInput(str) {
  return str.trim().substring(0, 1000);
}

// Handler uses helpers
function createHandler(context) {
  const req = context.request;
  const email = sanitizeInput(req.form.email);

  if (!validateEmail(email)) {
    return errorResponse(400, "Invalid email");
  }

  const id = generateId();
  // ... continue processing
}
```

## Inter-Script Communication

### When Scripts Need to Communicate

Scripts may need to coordinate with each other for:

- **Event notifications** - User registration triggers email, analytics, and welcome message
- **Workflow orchestration** - Order processing cascades through inventory, billing, and shipping
- **Real-time updates** - Chat message broadcasts to notifications and activity tracking
- **Decoupled architecture** - Services remain independent while coordinating behavior

### Communication Methods

aiwebengine provides several ways for scripts to communicate:

**1. Message Dispatcher (Event-Driven)**

Best for: Loose coupling, broadcast events, multiple consumers

```javascript
// Producer script: user-service.js
function createUserHandler(context) {
  const user = createUser(context.request.form);

  // Broadcast event - all listeners will receive it
  dispatcher.sendMessage(
    "user:created",
    JSON.stringify({
      userId: user.id,
      email: user.email,
      timestamp: new Date().toISOString(),
    }),
  );

  return jsonResponse(201, { user: user });
}

// Consumer script: email-notifications.js
function handleUserCreated(context) {
  const data = JSON.parse(context.messageData);
  console.log(`Sending welcome email to ${data.email}`);
  sendWelcomeEmail(data.email);
}

function init() {
  dispatcher.registerListener("user:created", "handleUserCreated");
}
```

**2. HTTP Routes (Request-Response)**

Best for: Synchronous communication, return values needed

```javascript
// Service script: user-service.js
function getUserHandler(context) {
  const userId = context.request.query.id;
  const user = findUser(userId);
  return jsonResponse(200, { user: user });
}

function init() {
  routeRegistry.registerRoute("/internal/users/get", "getUserHandler", "GET");
}

// Consumer script: profile-page.js
function renderProfileHandler(context) {
  const userId = context.request.query.userId;

  // Make internal HTTP request to user service
  const response = fetch(
    "http://localhost:8080/internal/users/get?id=" + userId,
    "{}",
  );
  const data = JSON.parse(response);

  return renderProfile(data.user);
}
```

**3. Shared Storage (State Sharing)**

Best for: Configuration, simple data sharing, caching

```javascript
// Writer script: config-manager.js
function updateConfigHandler(context) {
  const config = context.request.form;
  sharedStorage.setItem("app:config", JSON.stringify(config));
  return jsonResponse(200, { updated: true });
}

// Reader script: api-service.js
function apiHandler(context) {
  const configStr = sharedStorage.getItem("app:config");
  const config = configStr ? JSON.parse(configStr) : {};

  // Use config settings
  const apiKey = config.apiKey || "default";
  // ...
}
```

**4. GraphQL Subscriptions (Real-Time Streams)**

Best for: Client-facing real-time updates, WebSocket streaming

```javascript
// Publisher script: chat-service.js
function sendMessageHandler(context) {
  const message = context.request.form;

  // Broadcast to all WebSocket subscribers
  graphQLRegistry.sendSubscriptionMessage(
    "onNewMessage",
    JSON.stringify({
      message: message,
      timestamp: new Date().toISOString(),
    }),
  );

  return jsonResponse(200, { sent: true });
}
```

### Choosing the Right Method

| Method             | Use When                                 | Pros                                         | Cons                             |
| ------------------ | ---------------------------------------- | -------------------------------------------- | -------------------------------- |
| **Dispatcher**     | Multiple scripts need to react to events | Loose coupling, scalable, no response needed | No return value, async-like      |
| **HTTP Routes**    | Need synchronous response or result      | Request-response, familiar pattern           | Tighter coupling, overhead       |
| **Shared Storage** | Simple config/state sharing              | Fast, simple                                 | No notifications, manual polling |
| **Subscriptions**  | Client needs real-time updates           | WebSocket streaming                          | Client-facing only               |

### Dispatcher Best Practices

**Use descriptive message types:**

```javascript
// Good - namespace with colons
dispatcher.sendMessage("user:created", data);
dispatcher.sendMessage("order:completed", data);
dispatcher.sendMessage("payment:failed", data);

// Bad - too generic
dispatcher.sendMessage("event", data);
dispatcher.sendMessage("update", data);
```

**Register listeners in init():**

```javascript
function init() {
  // Register routes
  routeRegistry.registerRoute("/api/orders", "createOrderHandler", "POST");

  // Register message listeners
  dispatcher.registerListener("user:created", "handleNewUser");
  dispatcher.registerListener("payment:completed", "handlePayment");
}
```

**Always use JSON for message data:**

```javascript
// Good - structured data
dispatcher.sendMessage(
  "order:created",
  JSON.stringify({
    orderId: "12345",
    userId: "user-456",
    total: 99.99,
    items: [{ sku: "ABC", qty: 2 }],
  }),
);

// Bad - plain string (hard to parse)
dispatcher.sendMessage("order:created", "Order 12345 created");
```

**Handle errors in message handlers:**

```javascript
function handleOrderCreated(context) {
  try {
    const order = JSON.parse(context.messageData);
    processOrder(order);
    console.log(`Processed order ${order.orderId}`);
  } catch (error) {
    console.error(`Failed to process order: ${error.message}`);
    // Handler errors don't affect sender
  }
}
```

### Complete Example: User Registration Flow

```javascript
// ============================================
// user-service.js - Main user management
// ============================================

function registerUserHandler(context) {
  const req = context.request;

  // Create user
  const user = {
    id: generateId(),
    email: req.form.email,
    name: req.form.name,
    createdAt: new Date().toISOString(),
  };

  saveUser(user);

  // Broadcast event - multiple scripts will react
  dispatcher.sendMessage(
    "user:registered",
    JSON.stringify({
      userId: user.id,
      email: user.email,
      name: user.name,
    }),
  );

  return jsonResponse(201, { user: user });
}

function init() {
  routeRegistry.registerRoute(
    "/api/users/register",
    "registerUserHandler",
    "POST",
  );
}

init();

// ============================================
// email-service.js - Handles email notifications
// ============================================

function handleUserRegistered(context) {
  const userData = JSON.parse(context.messageData);
  console.log(`Sending welcome email to ${userData.email}`);

  // Send welcome email (pseudocode)
  sendEmail({
    to: userData.email,
    subject: "Welcome!",
    body: `Hello ${userData.name}, welcome to our platform!`,
  });
}

function init() {
  dispatcher.registerListener("user:registered", "handleUserRegistered");
}

init();

// ============================================
// analytics-service.js - Tracks user metrics
// ============================================

function handleUserRegistered(context) {
  const userData = JSON.parse(context.messageData);
  console.log(`Recording analytics for user ${userData.userId}`);

  // Track registration event
  recordEvent({
    type: "user_registration",
    userId: userData.userId,
    timestamp: new Date().toISOString(),
  });
}

function init() {
  dispatcher.registerListener("user:registered", "handleUserRegistered");
}

init();

// ============================================
// onboarding-service.js - Manages onboarding flow
// ============================================

function handleUserRegistered(context) {
  const userData = JSON.parse(context.messageData);
  console.log(`Starting onboarding for user ${userData.userId}`);

  // Create onboarding checklist
  createOnboardingTasks(userData.userId);

  // Schedule first onboarding email
  schedulerService.registerOnce({
    handler: "sendOnboardingEmail",
    runAt: getTimeOneHourFromNow(),
    name: `onboarding-${userData.userId}`,
  });
}

function init() {
  dispatcher.registerListener("user:registered", "handleUserRegistered");
}

init();
```

In this example:

- **user-service.js** creates users and broadcasts `user:registered` events
- **email-service.js** sends welcome emails when users register
- **analytics-service.js** tracks registration metrics
- **onboarding-service.js** initiates onboarding workflows

All services remain independent - they can be developed, tested, and deployed separately. The dispatcher decouples them while enabling coordination.

For more examples, see **[Message Passing Examples](../examples/message-passing.md)**.

## Advanced Patterns

### Middleware Pattern

```javascript
// Middleware functions
function requireAuth(context, handler) {
  const req = context.request;
  const token = req.headers["authorization"];
  if (!token) {
    return errorResponse(401, "Authentication required");
  }

  // Validate token...
  if (!isValidToken(token)) {
    return errorResponse(401, "Invalid token");
  }

  // Call actual handler
  return handler(context);
}

function logRequest(context, handler) {
  const req = context.request;
  console.log(`${req.method} ${req.path}`);
  const response = handler(context);
  console.log(`Response: ${response.status}`);
  return response;
}

// Protected handler
function protectedDataHandler(context) {
  return requireAuth(context, (ctx) => {
    return logRequest(ctx, (finalCtx) => {
      return jsonResponse(200, { secret: "data" });
    });
  });
}
```

### Factory Pattern

```javascript
function createCrudHandlers(resourceName, storage) {
  return {
    list: function (context) {
      return jsonResponse(200, { [resourceName]: storage });
    },

    create: function (context) {
      const req = context.request;
      const item = { id: generateId(), ...req.form };
      storage.push(item);
      return jsonResponse(201, { [resourceName]: item });
    },

    // ... more handlers
  };
}

// Usage
const users = [];
const userHandlers = createCrudHandlers("users", users);

function init() {
  routeRegistry.registerRoute("/api/users", "listUsersHandler", "GET");
  routeRegistry.registerRoute("/api/users", "createUserHandler", "POST");
}

function listUsersHandler(context) {
  return userHandlers.list(context);
}

function createUserHandler(context) {
  return userHandlers.create(context);
}
```

### Pagination Pattern

```javascript
function paginatedHandler(context) {
  const req = context.request;
  const page = parseInt(req.query.page || "1");
  const limit = parseInt(req.query.limit || "10");

  const offset = (page - 1) * limit;
  const allItems = getAllItems();
  const totalItems = allItems.length;
  const totalPages = Math.ceil(totalItems / limit);

  const items = allItems.slice(offset, offset + limit);

  return jsonResponse(200, {
    items: items,
    pagination: {
      page: page,
      limit: limit,
      totalItems: totalItems,
      totalPages: totalPages,
      hasNext: page < totalPages,
      hasPrev: page > 1,
    },
  });
}
```

## Next Steps

- **[Asset Management](assets.md)** - Work with static files
- **[Logging Guide](logging.md)** - Debug and monitor scripts
- **[AI Development](ai-development.md)** - Use AI to generate scripts
- **[API Reference](../reference/javascript-apis.md)** - Complete API documentation
- **[Examples](../examples/index.md)** - See real-world patterns

## Quick Reference

### Essential Functions

```javascript
routeRegistry.registerRoute(path, handlerName, method); // Register route
console.log(message); // Write to logs
```

### Handler Template

```javascript
function myHandler(context) {
  const req = context.request;
  try {
    // Extract parameters
    const param = req.query.param || req.form.param;

    // Validate
    if (!param) {
      return {
        status: 400,
        body: JSON.stringify({ error: "Missing parameter" }),
        contentType: "application/json",
      };
    }

    // Process
    const result = process(param);

    // Return success
    return {
      status: 200,
      body: JSON.stringify({ result: result }),
      contentType: "application/json",
    };
  } catch (error) {
    console.error(`Error: ${error.message}`);
    return {
      status: 500,
      body: JSON.stringify({ error: "Internal error" }),
      contentType: "application/json",
    };
  }
}

function init() {
  routeRegistry.registerRoute("/my-endpoint", "myHandler", "GET");
}

init();
```
