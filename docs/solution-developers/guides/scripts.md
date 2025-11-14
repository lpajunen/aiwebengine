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
function helloHandler(req) {
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

```javascript
/**
 * script-name.js - Brief description
 *
 * Longer description of what this script does.
 * List key features or routes.
 */

// ============================================
// Constants and Configuration
// ============================================

const MAX_ITEMS = 100;
const API_VERSION = "v1";

// ============================================
// Data Storage (in-memory)
// ============================================

let items = [];
let nextId = 1;

// ============================================
// Helper Functions
// ============================================

function validateItem(item) {
  if (!item.name || item.name.length === 0) {
    return { valid: false, error: "Name is required" };
  }
  if (item.name.length > 100) {
    return { valid: false, error: "Name too long" };
  }
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

function listItemsHandler(req) {
  console.log(`Listing items: ${items.length} total`);
  return createResponse(200, { items: items });
}

function createItemHandler(req) {
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
```

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

All handlers receive one parameter (`req`) and must return a response object:

```javascript
function handlerName(req) {
  // Process request
  return {
    status: 200,
    body: "response content",
    contentType: "text/plain",
  };
}
```

### Request Object Structure

The `req` parameter contains:

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
function textHandler(req) {
  return {
    status: 200,
    body: "Plain text response",
    contentType: "text/plain; charset=UTF-8",
  };
}
```

**JSON API response:**

```javascript
function jsonHandler(req) {
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
function htmlHandler(req) {
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
function errorHandler(req) {
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

Note: Path parameters like `:id` are not currently extracted automatically. Use query parameters instead:

```javascript
// Current approach
routeRegistry.registerRoute("/api/users/get", "getUser", "GET");

function getUser(req) {
  const id = req.query.id; // Access via ?id=123
  // ...
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

Access via `req.query`:

```javascript
function searchHandler(req) {
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

Access via `req.form`:

```javascript
function createUserHandler(req) {
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
function apiHandler(req) {
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

Access request headers via `req.headers`:

```javascript
function headerHandler(req) {
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
function myHandler(req) {
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

function incrementHandler(req) {
  counter++;
  return jsonResponse(200, { counter: counter });
}

function resetHandler(req) {
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

function loginHandler(req) {
  const sessionId = generateSessionId();
  sessions[sessionId] = {
    user: req.form.username,
    created: Date.now(),
  };

  return jsonResponse(200, { sessionId: sessionId });
}

function getUserHandler(req) {
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

function apiHandler(req) {
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
function riskyHandler(req) {
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
function createItemHandler(req) {
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

function myHandler(req) {
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
function createUserHandler(req) {}
function getUserByIdHandler(req) {}
function updateUserEmailHandler(req) {}
```

**Bad:**

```javascript
function handler1(req) {}
function func(req) {}
function process(req) {}
```

### 2. Validate All Inputs

```javascript
function safeHandler(req) {
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
function handler(req) {
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
function listUsers(req) {
  const users = getAllUsers();
  return jsonResponse(200, { users: users });
}

function createUser(req) {
  const user = buildUserFromForm(req.form);
  saveUser(user);
  return jsonResponse(201, { user: user });
}
```

**Bad - Too Much in One Handler:**

```javascript
function usersHandler(req) {
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
function createHandler(req) {
  const email = sanitizeInput(req.form.email);

  if (!validateEmail(email)) {
    return errorResponse(400, "Invalid email");
  }

  const id = generateId();
  // ... continue processing
}
```

## Advanced Patterns

### Middleware Pattern

```javascript
// Middleware functions
function requireAuth(req, handler) {
  const token = req.headers["authorization"];
  if (!token) {
    return errorResponse(401, "Authentication required");
  }

  // Validate token...
  if (!isValidToken(token)) {
    return errorResponse(401, "Invalid token");
  }

  // Call actual handler
  return handler(req);
}

function logRequest(req, handler) {
  console.log(`${req.method} ${req.path}`);
  const response = handler(req);
  console.log(`Response: ${response.status}`);
  return response;
}

// Protected handler
function protectedDataHandler(req) {
  return requireAuth(req, (req) => {
    return logRequest(req, (req) => {
      return jsonResponse(200, { secret: "data" });
    });
  });
}
```

### Factory Pattern

```javascript
function createCrudHandlers(resourceName, storage) {
  return {
    list: function (req) {
      return jsonResponse(200, { [resourceName]: storage });
    },

    create: function (req) {
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

function listUsersHandler(req) {
  return userHandlers.list(req);
}

function createUserHandler(req) {
  return userHandlers.create(req);
}
```

### Pagination Pattern

```javascript
function paginatedHandler(req) {
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
function myHandler(req) {
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
