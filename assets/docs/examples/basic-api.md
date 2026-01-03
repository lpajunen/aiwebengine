# Basic API Examples

Learn how to build RESTful APIs with aiwebengine through practical examples. This guide covers common API patterns from simple GET endpoints to full CRUD operations.

## Simple GET Endpoint

The most basic API returns JSON data:

```javascript
function init() {
  routeRegistry.registerRoute("/api/hello", "helloHandler", "GET");
}

function helloHandler(request) {
  const data = {
    message: "Hello from aiwebengine!",
    timestamp: new Date().toISOString(),
    version: "1.0.0",
  };

  return {
    status: 200,
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(data),
  };
}

init();
```

**Test it:**

```bash
curl http://localhost:8080/api/hello
```

**Response:**

```json
{
  "message": "Hello from aiwebengine!",
  "timestamp": "2024-01-15T10:30:00.000Z",
  "version": "1.0.0"
}
```

## Query Parameters

Handle query parameters for filtering and searching:

```javascript
function init() {
  routeRegistry.registerRoute("/api/users", "getUsersHandler", "GET");
}

function getUsersHandler(request) {
  // Parse query parameters
  const params = parseQueryParams(request.url);
  const role = params.role || "all";
  const limit = parseInt(params.limit || "10");

  console.log("info", "Fetching users", { role, limit });

  // Mock data
  const allUsers = [
    { id: 1, name: "Alice", role: "admin" },
    { id: 2, name: "Bob", role: "user" },
    { id: 3, name: "Charlie", role: "user" },
    { id: 4, name: "Diana", role: "admin" },
  ];

  // Filter by role
  let users = allUsers;
  if (role !== "all") {
    users = users.filter((u) => u.role === role);
  }

  // Apply limit
  users = users.slice(0, limit);

  return {
    status: 200,
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({
      data: users,
      count: users.length,
      filters: { role, limit },
    }),
  };
}

function parseQueryParams(url) {
  const params = {};
  const queryString = url.split("?")[1];

  if (!queryString) return params;

  const pairs = queryString.split("&");
  for (const pair of pairs) {
    const [key, value] = pair.split("=");
    params[decodeURIComponent(key)] = decodeURIComponent(value || "");
  }

  return params;
}

init();
```

**Test it:**

```bash
# All users
curl http://localhost:8080/api/users

# Filter by role
curl "http://localhost:8080/api/users?role=admin"

# Limit results
curl "http://localhost:8080/api/users?limit=2"

# Combine filters
curl "http://localhost:8080/api/users?role=user&limit=1"
```

## URL Parameters

Extract parameters from the URL path:

```javascript
function init() {
  routeRegistry.registerRoute("/api/users/:id", "getUserByIdHandler", "GET");
}

function getUserByIdHandler(context) {
  const req = context.request;
  const id = req.params.id; // Access path parameter directly

  console.log("info", "Fetching user by ID", { id });

  // Mock database lookup
  const users = {
    1: { id: 1, name: "Alice", email: "alice@example.com", role: "admin" },
    2: { id: 2, name: "Bob", email: "bob@example.com", role: "user" },
    3: { id: 3, name: "Charlie", email: "charlie@example.com", role: "user" },
  };

  const user = users[id];

  if (!user) {
    return {
      status: 404,
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({
        error: "User not found",
        id: id,
      }),
    };
  }

  return {
    status: 200,
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ data: user }),
  };
}

init();
```

**Test it:**

```bash
# Get user 1
curl http://localhost:8080/api/users/1

# Get user 2
curl http://localhost:8080/api/users/2

# User not found
curl http://localhost:8080/api/users/999
```

## POST Endpoint - Create Resource

Handle POST requests with JSON body:

```javascript
function init() {
  routeRegistry.registerRoute("/api/users", "createUserHandler", "POST");
}

function createUserHandler(request) {
  // Parse JSON body
  let userData;
  try {
    userData = JSON.parse(request.body || "{}");
  } catch (e) {
    return {
      status: 400,
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({
        error: "Invalid JSON",
        message: e.message,
      }),
    };
  }

  // Validate required fields
  const errors = [];
  if (!userData.name) errors.push("name is required");
  if (!userData.email) errors.push("email is required");

  if (errors.length > 0) {
    return {
      status: 400,
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({
        error: "Validation failed",
        errors: errors,
      }),
    };
  }

  // Create user (mock)
  const newUser = {
    id: Date.now(),
    name: userData.name,
    email: userData.email,
    role: userData.role || "user",
    createdAt: new Date().toISOString(),
  };

  console.log("info", "User created", newUser);

  return {
    status: 201,
    headers: {
      "Content-Type": "application/json",
      Location: `/api/users/${newUser.id}`,
    },
    body: JSON.stringify({
      message: "User created successfully",
      data: newUser,
    }),
  };
}

init();
```

**Test it:**

```bash
# Create user
curl -X POST http://localhost:8080/api/users \
  -H "Content-Type: application/json" \
  -d '{"name":"Eve","email":"eve@example.com","role":"user"}'

# Missing fields
curl -X POST http://localhost:8080/api/users \
  -H "Content-Type: application/json" \
  -d '{"name":"Eve"}'
```

## PUT Endpoint - Update Resource

Full update of a resource:

```javascript
function init() {
  routeRegistry.registerRoute("/api/users/:id", "updateUserHandler", "PUT");
}

function updateUserHandler(context) {
  const req = context.request;
  const id = req.params.id; // Access path parameter directly

  // Parse body
  let userData;
  try {
    userData = JSON.parse(req.body || "{}");
  } catch (e) {
    return {
      status: 400,
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ error: "Invalid JSON" }),
    };
  }

  // Validate
  const errors = [];
  if (!userData.name) errors.push("name is required");
  if (!userData.email) errors.push("email is required");

  if (errors.length > 0) {
    return {
      status: 400,
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ error: "Validation failed", errors }),
    };
  }

  // Update user (mock)
  const updatedUser = {
    id: parseInt(id),
    name: userData.name,
    email: userData.email,
    role: userData.role || "user",
    updatedAt: new Date().toISOString(),
  };

  console.log("User updated", updatedUser);

  return {
    status: 200,
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({
      message: "User updated successfully",
      data: updatedUser,
    }),
  };
}

init();
```

**Test it:**

```bash
curl -X PUT http://localhost:8080/api/users/1 \
  -H "Content-Type: application/json" \
  -d '{"name":"Alice Updated","email":"alice.new@example.com","role":"admin"}'
```

## DELETE Endpoint

Delete a resource:

```javascript
function init() {
  routeRegistry.registerRoute("/api/users/:id", "deleteUserHandler", "DELETE");
}

function deleteUserHandler(context) {
  const req = context.request;
  const id = req.params.id; // Access path parameter directly

  console.log("info", "Deleting user", { id });

  // Mock deletion - check if user exists
  if (id === "999") {
    return {
      status: 404,
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({
        error: "User not found",
        id: id,
      }),
    };
  }

  return {
    status: 200,
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({
      message: "User deleted successfully",
      id: parseInt(id),
    }),
  };
}

init();
```

**Test it:**

```bash
# Delete user
curl -X DELETE http://localhost:8080/api/users/1

# Delete non-existent user
curl -X DELETE http://localhost:8080/api/users/999
```

## Complete CRUD API

Here's a complete CRUD API in one script:

```javascript
// Simple in-memory storage
let users = [
  { id: 1, name: "Alice", email: "alice@example.com", role: "admin" },
  { id: 2, name: "Bob", email: "bob@example.com", role: "user" },
  { id: 3, name: "Charlie", email: "charlie@example.com", role: "user" },
];
let nextId = 4;

function init() {
  routeRegistry.registerRoute("/api/users", "listUsers", "GET");
  routeRegistry.registerRoute("/api/users/:id", "getUser", "GET");
  routeRegistry.registerRoute("/api/users", "createUser", "POST");
  routeRegistry.registerRoute("/api/users/:id", "updateUser", "PUT");
  routeRegistry.registerRoute("/api/users/:id", "deleteUser", "DELETE");
}

// LIST - Get all users
function listUsers(context) {
  const req = context.request;
  const params = parseQueryParams(req.url);
  const role = params.role;

  let filtered = users;
  if (role) {
    filtered = users.filter((u) => u.role === role);
  }

  return jsonResponse(200, {
    data: filtered,
    count: filtered.length,
  });
}

// GET - Get single user
function getUser(context) {
  const req = context.request;
  const id = req.params.id; // Access path parameter directly
  const user = users.find((u) => u.id === parseInt(id));

  if (!user) {
    return jsonResponse(404, { error: "User not found" });
  }

  return jsonResponse(200, { data: user });
}

// CREATE - Create new user
function createUser(context) {
  const req = context.request;
  const userData = parseBody(req.body);
  if (userData.error) {
    return jsonResponse(400, userData);
  }

  const errors = validateUser(userData);
  if (errors.length > 0) {
    return jsonResponse(400, { error: "Validation failed", errors });
  }

  const newUser = {
    id: nextId++,
    name: userData.name,
    email: userData.email,
    role: userData.role || "user",
    createdAt: new Date().toISOString(),
  };

  users.push(newUser);
  console.log("info", "User created", newUser);

  return jsonResponse(201, {
    message: "User created",
    data: newUser,
  });
}

// UPDATE - Update user
function updateUser(context) {
  const req = context.request;
  const id = req.params.id; // Access path parameter directly
  const index = users.findIndex((u) => u.id === parseInt(id));

  if (index === -1) {
    return jsonResponse(404, { error: "User not found" });
  }

  const userData = parseBody(req.body);
  if (userData.error) {
    return jsonResponse(400, userData);
  }

  const errors = validateUser(userData);
  if (errors.length > 0) {
    return jsonResponse(400, { error: "Validation failed", errors });
  }

  users[index] = {
    ...users[index],
    name: userData.name,
    email: userData.email,
    role: userData.role || users[index].role,
    updatedAt: new Date().toISOString(),
  };

  console.log("info", "User updated", users[index]);

  return jsonResponse(200, {
    message: "User updated",
    data: users[index],
  });
}

// DELETE - Delete user
function deleteUser(context) {
  const req = context.request;
  const id = req.params.id; // Access path parameter directly
  const index = users.findIndex((u) => u.id === parseInt(id));

  if (index === -1) {
    return jsonResponse(404, { error: "User not found" });
  }

  const deleted = users.splice(index, 1)[0];
  console.log("info", "User deleted", deleted);

  return jsonResponse(200, {
    message: "User deleted",
    data: deleted,
  });
}

// Helper functions
function parseBody(body) {
  try {
    return JSON.parse(body || "{}");
  } catch (e) {
    return { error: "Invalid JSON" };
  }
}

function validateUser(user) {
  const errors = [];
  if (!user.name || user.name.trim() === "") {
    errors.push("name is required");
  }
  if (!user.email || user.email.trim() === "") {
    errors.push("email is required");
  } else if (!user.email.includes("@")) {
    errors.push("email must be valid");
  }
  return errors;
}

function parseQueryParams(url) {
  const params = {};
  const queryString = url.split("?")[1];
  if (!queryString) return params;

  const pairs = queryString.split("&");
  for (const pair of pairs) {
    const [key, value] = pair.split("=");
    params[decodeURIComponent(key)] = decodeURIComponent(value || "");
  }
  return params;
}

function jsonResponse(status, data) {
  return {
    status: status,
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(data),
  };
}

init();
```

**Test the complete API:**

```bash
# List all users
curl http://localhost:8080/api/users

# Get user by ID
curl http://localhost:8080/api/users/1

# Create user
curl -X POST http://localhost:8080/api/users \
  -H "Content-Type: application/json" \
  -d '{"name":"Diana","email":"diana@example.com","role":"user"}'

# Update user
curl -X PUT http://localhost:8080/api/users/1 \
  -H "Content-Type: application/json" \
  -d '{"name":"Alice Smith","email":"alice.smith@example.com","role":"admin"}'

# Delete user
curl -X DELETE http://localhost:8080/api/users/3

# Filter by role
curl "http://localhost:8080/api/users?role=admin"
```

## Error Handling

Always handle errors gracefully:

```javascript
function safeHandler(context) {
  try {
    // Your logic here
    const data = processRequest(context.request);

    return jsonResponse(200, { data });
  } catch (error) {
    console.log("error", "Request failed", {
      error: error.message,
      url: context.request.url,
    });

    return jsonResponse(500, {
      error: "Internal server error",
      message: error.message,
    });
  }
}

function processRequest(request) {
  // Simulate processing that might fail
  if (Math.random() > 0.9) {
    throw new Error("Random failure for testing");
  }

  return { success: true };
}
```

## Best Practices

### 1. Use Proper Status Codes

```javascript
// 200 - Success
return jsonResponse(200, { data: users });

// 201 - Created
return jsonResponse(201, { data: newUser });

// 400 - Bad Request
return jsonResponse(400, { error: "Invalid input" });

// 404 - Not Found
return jsonResponse(404, { error: "User not found" });

// 500 - Server Error
return jsonResponse(500, { error: "Internal error" });
```

### 2. Validate Input

```javascript
function validateEmail(email) {
  return email && email.includes("@") && email.includes(".");
}

function validateUser(user) {
  const errors = [];

  if (!user.name || user.name.length < 2) {
    errors.push("Name must be at least 2 characters");
  }

  if (!validateEmail(user.email)) {
    errors.push("Invalid email address");
  }

  return errors;
}
```

### 3. Log Important Events

```javascript
// Log successful operations
console.log("info", "User created", { id: newUser.id, name: newUser.name });

// Log errors
console.log("error", "Failed to create user", { error: error.message });

// Log warnings
console.log("warn", "Invalid email format", { email: userData.email });
```

### 4. Return Consistent Responses

```javascript
// Success format
{
  "data": {...},
  "message": "Operation successful"
}

// Error format
{
  "error": "Error type",
  "message": "Detailed message",
  "errors": ["field1 error", "field2 error"]
}
```

### 5. Use Helper Functions

```javascript
// Create reusable helpers
function jsonResponse(status, data) {
  return {
    status: status,
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(data),
  };
}

function parseJsonBody(body) {
  try {
    return { success: true, data: JSON.parse(body || "{}") };
  } catch (e) {
    return { success: false, error: "Invalid JSON" };
  }
}
```

## Next Steps

- **[Forms and Data](forms-and-data.md)** - Handle form submissions
- **[Real-Time Features](real-time-features.md)** - WebSocket and streaming
- **[AI Integration](ai-integration.md)** - Add AI capabilities
- **[JavaScript APIs](../reference/javascript-apis.md)** - Full API reference
- **[Script Development Guide](../guides/scripts.md)** - Advanced patterns

## Quick Reference

```javascript
// GET endpoint
routeRegistry.registerRoute("/api/resource", "handler", "GET");

// POST endpoint
routeRegistry.registerRoute("/api/resource", "handler", "POST");

// PUT endpoint
routeRegistry.registerRoute("/api/resource/:id", "handler", "PUT");

// DELETE endpoint
routeRegistry.registerRoute("/api/resource/:id", "handler", "DELETE");

// JSON response
return {
  status: 200,
  headers: { "Content-Type": "application/json" },
  body: JSON.stringify(data),
};
```

Start building powerful APIs with aiwebengine! 🚀
