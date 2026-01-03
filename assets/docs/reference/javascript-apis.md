# JavaScript APIs Reference

This page provides a complete reference for the JavaScript APIs available in aiwebengine scripts. These functions and objects allow you to handle HTTP requests, generate responses, log information, and interact with the server environment.

## Unified Handler Context

Every handler/resolver now receives a single `context` object. For HTTP routes you usually alias `context.request` to the familiar `req` variable:

```javascript
function myHandler(context) {
  const req = context.request;

  // Access req.method, req.path, req.query, req.form, req.headers, req.body
  // Use context.args, context.kind, context.meta as needed
}
```

The examples below follow this pattern—code snippets declare `const req = context.request;` when they need request data. If a snippet still shows `req`, it assumes this alias exists.

## Route Registry

The `routeRegistry` object provides all HTTP route and streaming functionality in a unified namespace.

### routeRegistry.registerRoute(path, handlerName, method)

Registers a route that maps a URL path to a handler function.

**Parameters:**

- `path` (string): URL path to register (e.g., `"/api/users"`)
- `handlerName` (string): Name of your handler function
- `method` (string): HTTP method (`"GET"`, `"POST"`, `"PUT"`, `"DELETE"`)

**Returns:** String describing registration result

**Example:**

```javascript
function getUsers(context) {
  const req = context.request;
  return {
    status: 200,
    body: "User list",
    contentType: "text/plain; charset=UTF-8",
  };
}

routeRegistry.registerRoute("/api/users", "getUsers", "GET");
```

### routeRegistry.registerStreamRoute(path)

Registers a Server-Sent Events (SSE) stream endpoint that clients can connect to for real-time updates.

**Parameters:**

- `path` (string): Stream path to register (must start with `/`, max 200 characters)

**Returns:** String describing registration result

**Example:**

```javascript
// Register a stream for live notifications
routeRegistry.registerStreamRoute("/notifications");

// Register a stream for chat messages
routeRegistry.registerStreamRoute("/chat/room1");
```

**Notes:**

- Stream paths must be unique
- Multiple clients can connect to the same stream
- Streams persist until the server restarts or the script is reloaded
- Use meaningful, descriptive paths for better organization

### routeRegistry.registerAssetRoute(assetPath)

Registers a static asset for serving via HTTP.

**Parameters:**

- `assetPath` (string): Path to asset file in the asset repository

**Returns:** String describing registration result

**Example:**

```javascript
routeRegistry.registerAssetRoute("/styles/main.css", "main.css");
routeRegistry.registerAssetRoute("/images/logo.svg", "logo.svg");
```

### routeRegistry.sendStreamMessage(path, data)

Sends a message to all clients connected to a specific stream path.

**Parameters:**

- `path` (string): Stream path to send to (must start with `/`)
- `data` (object): Data object to send (will be JSON serialized)

**Returns:** String describing broadcast result

**Example:**

```javascript
function notifyHandler(context) {
  // Send notification to specific stream
  routeRegistry.sendStreamMessage("/notifications", {
    type: "notification",
    message: "New update available",
    timestamp: new Date().toISOString(),
    priority: "high",
  });

  return { status: 200, body: "Notification sent" };
}

// Register the handler
routeRegistry.registerRoute("/notify", "notifyHandler", "POST");
```

**Real-time Chat Example:**

```javascript
// Register a chat stream
routeRegistry.registerStreamRoute("/chat");

function sendMessage(context) {
  const req = context.request;
  const { user, message } = req.form;

  if (!user || !message) {
    return { status: 400, body: "Missing user or message" };
  }

  // Send to the chat stream
  routeRegistry.sendStreamMessage("/chat", {
    type: "chat_message",
    user: user,
    message: message,
    timestamp: new Date().toISOString(),
  });

  return { status: 200, body: "Message sent" };
}

routeRegistry.registerRoute("/chat/send", "sendMessage", "POST");
```

### routeRegistry.sendStreamMessageFiltered(path, data, filterJson)

Sends a message to specific connections on a stream based on metadata filtering. This enables personalized broadcasting to subsets of users on stable endpoints.

**Parameters:**

- `path` (string): Stream path to send to (must start with `/`)
- `data` (object): Data object to send (will be JSON serialized)
- `filterJson` (string): JSON string with metadata filter criteria (empty `"{}"` matches all connections)

**Returns:** String describing broadcast result with success/failure counts

**Example:**

```javascript
// Send to connections where metadata.room == "general"
routeRegistry.sendStreamMessageFiltered(
  "/chat",
  {
    type: "room_message",
    message: "Hello room!",
    timestamp: new Date().toISOString(),
  },
  JSON.stringify({ room: "general" }),
);

// Send to specific user by ID
routeRegistry.sendStreamMessageFiltered(
  "/notifications",
  {
    type: "personal",
    message: "You have a new message",
  },
  JSON.stringify({ user_id: "user123" }),
);
```

### routeRegistry.listRoutes()

Lists all registered HTTP routes.

**Returns:** JSON string with array of route metadata

**Example:**

```javascript
const routes = JSON.parse(routeRegistry.listRoutes());
console.log("Registered routes:", routes);
```

### routeRegistry.listStreams()

Lists all registered stream endpoints with their metadata.

**Returns:** JSON string with array of objects containing `path` and `uri` properties

**Example:**

```javascript
const streams = JSON.parse(routeRegistry.listStreams());
// Returns: [{ path: "/chat", uri: "https://..." }, ...]
streams.forEach((stream) => {
  console.log("Stream path:", stream.path, "URI:", stream.uri);
});
```

### routeRegistry.listAssets()

Lists all registered asset paths.

**Returns:** JSON string with array of asset names

**Example:**

```javascript
const assets = JSON.parse(routeRegistry.listAssets());
console.log("Registered assets:", assets);
```

## Asset Storage

The `assetStorage` object provides functions for managing assets (files) in the asset repository. Assets can be uploaded, retrieved, listed, and deleted programmatically from your scripts.

### assetStorage.listAssets()

Returns a JSON string containing metadata for all assets in the repository.

**Returns:** JSON string with array of asset metadata objects. Each object contains:

- `name` (string): Asset name/identifier
- `size` (number): Size in bytes
- `mimetype` (string): MIME type of the asset
- `createdAt` (number): Creation timestamp (milliseconds since Unix epoch)
- `updatedAt` (number): Last update timestamp (milliseconds since Unix epoch)

**Required Capability:** `ReadAssets`

**Example:**

```javascript
function listAllAssets(req) {
  const assetsJson = assetStorage.listAssets();
  const assetMetadata = JSON.parse(assetsJson);

  // Map to simpler format if needed
  const assetList = assetMetadata.map((asset) => ({
    name: asset.name,
    size: asset.size,
    type: asset.mimetype,
    created: new Date(asset.createdAt).toISOString(),
    updated: new Date(asset.updatedAt).toISOString(),
  }));

  return {
    status: 200,
    body: JSON.stringify({
      assets: assetList,
      count: assetList.length,
    }),
    contentType: "application/json",
  };
}

routeRegistry.registerRoute("/assets", "listAllAssets", "GET");
```

### assetStorage.fetchAsset(asset_name)

Retrieves an asset's content from the repository.

**Parameters:**

- `asset_name` (string): Name of the asset to retrieve

**Returns:** Base64-encoded string containing the asset content, or an error message if the asset is not found

**Required Capability:** `ReadAssets`

**Example:**

```javascript
function getAsset(req) {
  const assetName = req.query.name;

  if (!assetName) {
    return {
      status: 400,
      body: "Missing asset name",
      contentType: "text/plain; charset=UTF-8",
    };
  }

  const contentB64 = assetStorage.fetchAsset(assetName);

  if (contentB64.startsWith("Asset '")) {
    // Error message returned
    return {
      status: 404,
      body: contentB64,
      contentType: "text/plain; charset=UTF-8",
    };
  }

  // Successfully retrieved
  return {
    status: 200,
    bodyBase64: contentB64,
    contentType: getMimeType(assetName),
  };
}

routeRegistry.registerRoute("/asset", "getAsset", "GET");
```

### assetStorage.upsertAsset(asset_name, content_b64, mimetype)

Creates a new asset or updates an existing one in the repository.

**Parameters:**

- `asset_name` (string): Name of the asset (1-255 characters, no path traversal characters)
- `content_b64` (string): Base64-encoded asset content
- `mimetype` (string): MIME type of the asset (e.g., `"image/png"`, `"text/css"`)

**Returns:** Success message string, or error message if validation fails

**Required Capability:** `WriteAssets`

**Validation Rules:**

- Asset name must be 1-255 characters
- No path traversal characters (`..`, `\`)
- Content size limited to 10MB
- Content must be valid base64

**Example:**

```javascript
function uploadAsset(req) {
  const { name, content, mimetype } = req.form;

  if (!name || !content || !mimetype) {
    return {
      status: 400,
      body: "Missing required fields: name, content, mimetype",
      contentType: "text/plain; charset=UTF-8",
    };
  }

  const result = assetStorage.upsertAsset(name, content, mimetype);

  if (result.startsWith("Error") || result.startsWith("Invalid")) {
    return {
      status: 400,
      body: result,
      contentType: "text/plain; charset=UTF-8",
    };
  }

  return {
    status: 201,
    body: JSON.stringify({ message: result, assetName: name }),
    contentType: "application/json",
  };
}

routeRegistry.registerRoute("/upload-asset", "uploadAsset", "POST");
```

**Example - Upload from form data:**

```javascript
function handleImageUpload(req) {
  // Assume req.form.image contains base64 encoded image
  const imageB64 = req.form.image;
  const filename = req.form.filename || "uploaded-image.png";

  try {
    const result = assetStorage.upsertAsset(filename, imageB64, "image/png");

    console.log("Asset uploaded: " + filename);

    return {
      status: 200,
      body: JSON.stringify({
        success: true,
        message: result,
        filename: filename,
      }),
      contentType: "application/json",
    };
  } catch (error) {
    console.error("Upload failed: " + error);
    return {
      status: 500,
      body: JSON.stringify({ success: false, error: error.message }),
      contentType: "application/json",
    };
  }
}
```

### assetStorage.deleteAsset(asset_name)

Deletes an asset from the repository.

**Parameters:**

- `asset_name` (string): Name of the asset to delete

**Returns:** Success message if deleted, or error message if not found

**Required Capability:** `DeleteAssets`

**Example:**

```javascript
function removeAsset(req) {
  const assetName = req.query.name;

  if (!assetName) {
    return {
      status: 400,
      body: "Missing asset name",
      contentType: "text/plain; charset=UTF-8",
    };
  }

  const result = assetStorage.deleteAsset(assetName);

  if (result.includes("deleted successfully")) {
    console.log("Asset deleted: " + assetName);
    return {
      status: 200,
      body: result,
      contentType: "text/plain; charset=UTF-8",
    };
  } else {
    return {
      status: 404,
      body: result,
      contentType: "text/plain; charset=UTF-8",
    };
  }
}

routeRegistry.registerRoute("/delete-asset", "removeAsset", "DELETE");
```

### Asset Management Example

Complete example showing asset CRUD operations:

```javascript
function assetHandler(req) {
  const method = req.method;
  const path = req.path;

  if (method === "GET" && path === "/assets") {
    // List all assets with metadata
    const assetsJson = assetStorage.listAssets();
    const assets = JSON.parse(assetsJson);
    return {
      status: 200,
      body: JSON.stringify({
        assets: assets,
        count: assets.length,
      }),
      contentType: "application/json",
    };
  }

  if (method === "GET" && path.startsWith("/assets/")) {
    // Get specific asset
    const assetName = path.substring("/assets/".length);
    const content = assetStorage.fetchAsset(assetName);

    if (content.startsWith("Asset '")) {
      return { status: 404, body: "Asset not found" };
    }

    return {
      status: 200,
      bodyBase64: content,
      contentType: "application/octet-stream",
    };
  }

  if (method === "POST" && path === "/assets") {
    // Create/update asset
    const { name, content, mimetype } = req.form;
    const result = assetStorage.upsertAsset(name, content, mimetype);

    return {
      status: 201,
      body: JSON.stringify({ message: result }),
      contentType: "application/json",
    };
  }

  if (method === "DELETE" && path.startsWith("/assets/")) {
    // Delete asset
    const assetName = path.substring("/assets/".length);
    const result = assetStorage.deleteAsset(assetName);

    return {
      status: 200,
      body: JSON.stringify({ message: result }),
      contentType: "application/json",
    };
  }

  return { status: 404, body: "Not found" };
}

routeRegistry.registerRoute("/assets", "assetHandler", "GET");
routeRegistry.registerRoute("/assets", "assetHandler", "POST");
routeRegistry.registerRoute("/assets", "assetHandler", "DELETE");
```

### Asset Security

- **Access Control**: Asset operations require specific capabilities (`ReadAssets`, `WriteAssets`, `DeleteAssets`)
- **Validation**: Asset names and content are validated to prevent security issues
- **Size Limits**: Assets are limited to 10MB to prevent resource exhaustion
- **Audit Logging**: Asset operations are logged for security monitoring
- **Path Traversal Protection**: Asset names cannot contain `..` or `\` characters

### Best Practices for Assets

1. **Validate file types**: Check MIME types match expected formats
2. **Handle errors gracefully**: Always check return messages for errors
3. **Use meaningful names**: Name assets descriptively for easy management
4. **Clean up unused assets**: Regularly delete assets that are no longer needed
5. **Check capabilities**: Ensure your script has required asset capabilities
6. **Log operations**: Use `console.log()` to track asset modifications
7. **Verify base64 encoding**: Ensure content is properly base64 encoded before upload

## Console Logging

### console.log(message)

Writes a message to the server log for debugging and monitoring.

**Parameters:**

- `message` (string): Message to log

**Example:**

```javascript
function myHandler(req) {
  console.log("Handler called with path: " + req.path);
  return {
    status: 200,
    body: "Logged",
    contentType: "text/plain; charset=UTF-8",
  };
}
```

## Storage APIs

### sharedStorage

Provides persistent key-value storage that is shared across all requests for a specific script. Data is script-scoped but accessible to all users.

#### sharedStorage.getItem(key)

Retrieves a value from shared storage.

**Parameters:**

- `key` (string): Storage key

**Returns:** String value or `null` if key doesn't exist

**Example:**

```javascript
function getCounter(context) {
  const count = sharedStorage.getItem("counter") || "0";
  return {
    status: 200,
    body: `Counter: ${count}`,
    contentType: "text/plain; charset=UTF-8",
  };
}
```

#### sharedStorage.setItem(key, value)

Stores a key-value pair in shared storage.

**Parameters:**

- `key` (string): Storage key (cannot be empty)
- `value` (string): Value to store (max 1MB)

**Returns:** Success message or error string

**Example:**

```javascript
function incrementCounter(context) {
  const count = parseInt(sharedStorage.getItem("counter") || "0");
  const newCount = count + 1;
  sharedStorage.setItem("counter", newCount.toString());
  return {
    status: 200,
    body: `New count: ${newCount}`,
    contentType: "text/plain; charset=UTF-8",
  };
}
```

#### sharedStorage.removeItem(key)

Removes a key-value pair from shared storage.

**Parameters:**

- `key` (string): Storage key to remove

**Returns:** Boolean - `true` if item was removed, `false` if it didn't exist

**Example:**

```javascript
function resetCounter(context) {
  const removed = sharedStorage.removeItem("counter");
  return {
    status: 200,
    body: removed ? "Counter reset" : "Counter didn't exist",
    contentType: "text/plain; charset=UTF-8",
  };
}
```

#### sharedStorage.clear()

Removes all data for the current script from shared storage.

**Returns:** Success message or error string

**Example:**

```javascript
function clearAllData(context) {
  sharedStorage.clear();
  return {
    status: 200,
    body: "All shared storage cleared",
    contentType: "text/plain; charset=UTF-8",
  };
}
```

### personalStorage

Provides persistent key-value storage that is isolated per user. Each authenticated user has their own private storage namespace within each script. **Requires authentication** - all methods return errors or null when user is not logged in.

#### personalStorage.getItem(key)

Retrieves a value from the current user's personal storage.

**Parameters:**

- `key` (string): Storage key

**Returns:** String value or `null` if key doesn't exist or user is not authenticated

**Example:**

```javascript
function getUserPreference(context) {
  const req = context.request;
  const theme = personalStorage.getItem("theme") || "light";
  return {
    status: 200,
    body: JSON.stringify({ theme }),
    contentType: "application/json",
  };
}
```

#### personalStorage.setItem(key, value)

Stores a key-value pair in the current user's personal storage.

**Parameters:**

- `key` (string): Storage key (cannot be empty)
- `value` (string): Value to store (max 1MB)

**Returns:** Success message or error string (error if not authenticated)

**Example:**

```javascript
function saveUserPreference(context) {
  const req = context.request;

  // Requires authentication
  if (!req.auth.isAuthenticated) {
    return {
      status: 401,
      body: "Authentication required",
      contentType: "text/plain; charset=UTF-8",
    };
  }

  const theme = req.form.theme || "light";
  const result = personalStorage.setItem("theme", theme);

  if (result.startsWith("Error:")) {
    return {
      status: 500,
      body: result,
      contentType: "text/plain; charset=UTF-8",
    };
  }

  return {
    status: 200,
    body: "Preference saved",
    contentType: "text/plain; charset=UTF-8",
  };
}
```

#### personalStorage.removeItem(key)

Removes a key-value pair from the current user's personal storage.

**Parameters:**

- `key` (string): Storage key to remove

**Returns:** Boolean - `true` if item was removed, `false` if it didn't exist or user is not authenticated

**Example:**

```javascript
function clearUserPreference(context) {
  const removed = personalStorage.removeItem("theme");
  return {
    status: 200,
    body: removed ? "Preference cleared" : "No preference found",
    contentType: "text/plain; charset=UTF-8",
  };
}
```

#### personalStorage.clear()

Removes all data for the current user in the current script from personal storage.

**Returns:** Success message or error string (error if not authenticated)

**Example:**

```javascript
function clearAllUserData(context) {
  const req = context.request;

  if (!req.auth.isAuthenticated) {
    return {
      status: 401,
      body: "Authentication required",
      contentType: "text/plain; charset=UTF-8",
    };
  }

  personalStorage.clear();
  return {
    status: 200,
    body: "All personal data cleared",
    contentType: "text/plain; charset=UTF-8",
  };
}
```

**Use Cases for personalStorage:**

- User preferences (theme, language, display settings)
- Shopping cart contents
- Form draft data
- User-specific cache
- Per-user feature flags
- Personalized recommendations data

**Security Notes:**

- User ID is handled transparently by the engine - scripts never see user IDs directly
- Each user can only access their own data
- Data persists across sessions when PostgreSQL is configured
- Unauthenticated requests cannot access personal storage

## HTTP Fetch

### fetch(url, options)

Makes HTTP requests to external APIs with built-in security features including secret injection for API keys.

**Parameters:**

- `url` (string): The URL to request
- `options` (string, optional): JSON string containing request options

**Options Object:**

- `method` (string, optional): HTTP method - `"GET"`, `"POST"`, `"PUT"`, `"DELETE"`, `"PATCH"`. Default: `"GET"`
- `headers` (object, optional): Request headers as key-value pairs
- `body` (string, optional): Request body for POST/PUT/PATCH requests
- `timeout_ms` (number, optional): Timeout in milliseconds. Default: 30000 (30 seconds)

**Returns:** JSON string with response object containing:

- `status` (number): HTTP status code
- `ok` (boolean): `true` if status is 2xx
- `headers` (object): Response headers
- `body` (string): Response body

**Example - Simple GET Request:**

```javascript
function fetchExample(req) {
  try {
    // Make a GET request
    const responseJson = fetch("https://api.example.com/data");
    const response = JSON.parse(responseJson);

    if (response.ok) {
      console.log("Fetch successful: " + response.status);
      return {
        status: 200,
        body: response.body,
        contentType: "application/json",
      };
    } else {
      return {
        status: response.status,
        body: "External API error",
        contentType: "text/plain; charset=UTF-8",
      };
    }
  } catch (error) {
    console.log("Fetch error: " + error);
    return { status: 500, body: "Request failed" };
  }
}
```

**Example - POST with JSON:**

```javascript
function createResource(req) {
  const requestData = {
    name: "New Item",
    description: "Created via API",
  };

  const options = JSON.stringify({
    method: "POST",
    headers: {
      "Content-Type": "application/json",
    },
    body: JSON.stringify(requestData),
  });

  const responseJson = fetch("https://api.example.com/items", options);
  const response = JSON.parse(responseJson);

  return {
    status: response.ok ? 200 : 502,
    body: response.body,
    contentType: "application/json",
  };
}
```

**Example - Using Secret Injection for API Keys:**

The fetch function supports secure secret injection using template syntax `{{secret:identifier}}`. This allows you to use API keys stored in the server's secrets manager without exposing them in your script code.

```javascript
function callSecureAPI(req) {
  // Use {{secret:identifier}} syntax to inject secrets securely
  const options = JSON.stringify({
    method: "GET",
    headers: {
      Authorization: "{{secret:api_key}}", // Secret injected by server
      "X-API-Key": "{{secret:external_api_key}}", // Another secret
    },
  });

  const responseJson = fetch("https://secure-api.example.com/data", options);
  const response = JSON.parse(responseJson);

  return {
    status: 200,
    body: response.body,
    contentType: "application/json",
  };
}
```

**Security Features:**

- **Secret Injection**: Use `{{secret:identifier}}` in headers to securely inject API keys. The secret values never appear in your JavaScript code.
- **URL Validation**: Blocks requests to localhost, private IPs (192.168.x.x, 10.x.x.x, etc.), and local networks
- **Protocol Restrictions**: Only HTTP and HTTPS are allowed (blocks file://, ftp://, etc.)
- **Response Size Limits**: Responses are limited to 10MB to prevent memory exhaustion
- **Timeout Enforcement**: All requests have a timeout (default 30 seconds)
- **TLS/SSL Validation**: HTTPS certificates are validated

**Error Handling:**

```javascript
function robustFetch(req) {
  try {
    const responseJson = fetch("https://api.example.com/data");
    const response = JSON.parse(responseJson);

    // Handle different response statuses
    if (response.status === 200) {
      return { status: 200, body: response.body };
    } else if (response.status === 404) {
      return { status: 404, body: "Resource not found" };
    } else if (response.status === 429) {
      return { status: 429, body: "Rate limit exceeded" };
    } else {
      return { status: 502, body: "Upstream error" };
    }
  } catch (error) {
    // Fetch errors (network, timeout, blocked URL, etc.)
    console.log("Fetch failed: " + error);
    return { status: 500, body: "Request failed" };
  }
}
```

**Blocked URLs:**

These URLs will be rejected for security reasons:

- `http://localhost/api` - Localhost
- `http://127.0.0.1/api` - Loopback address
- `http://192.168.1.1/api` - Private network
- `http://10.0.0.1/api` - Private network
- `file:///etc/passwd` - File protocol
- `ftp://example.com/file` - FTP protocol

**Best Practices:**

1. **Always use try-catch**: Network requests can fail in many ways
2. **Check response.ok**: Don't assume requests always succeed
3. **Use secrets for API keys**: Never hardcode API keys in scripts
4. **Set appropriate timeouts**: Adjust `timeout_ms` based on expected response time
5. **Handle rate limits**: Implement retry logic for 429 responses
6. **Log errors**: Use `console.log()` to track fetch failures
7. **Validate response data**: Parse and validate JSON responses before using them

## Streaming Connections

### Client-Side Connection

Clients connect to streams using the standard EventSource API:

```javascript
// Connect to a stream from the browser
const eventSource = new EventSource("/notifications");

eventSource.onmessage = function (event) {
  const data = JSON.parse(event.data);
  console.log("Received:", data);

  // Handle different message types
  if (data.type === "notification") {
    showNotification(data.message);
  }
};

eventSource.onerror = function (event) {
  console.error("Stream connection error:", event);
};
```

### Stream Lifecycle

1. **Registration**: Use `routeRegistry.registerStreamRoute()` to create a stream endpoint
2. **Connection**: Clients connect using EventSource or compatible SSE clients
3. **Broadcasting**: Use `routeRegistry.sendStreamMessage()` or `routeRegistry.sendStreamMessageFiltered()` to send data to connected clients
4. **Cleanup**: Connections are automatically cleaned up when clients disconnect

### Best Practices for Streaming

- **Register streams early**: Call `routeRegistry.registerStreamRoute()` when your script loads
- **Structure your data**: Use consistent message formats with `type` fields
- **Handle disconnections**: Clients should implement reconnection logic
- **Limit message frequency**: Avoid overwhelming clients with too many messages
- **Use meaningful paths**: Organize streams logically (e.g., `/chat/room1`, `/notifications`)
- **Use filtered broadcasting**: Use `routeRegistry.sendStreamMessageFiltered()` for personalized messages instead of creating dynamic endpoints
- **Leverage metadata**: Store user/room information in connection metadata for efficient filtering

## GraphQL APIs

aiwebengine provides comprehensive GraphQL support through the `graphQLRegistry` object, which contains all GraphQL-related functions for registering operations and executing queries directly from your JavaScript scripts.

### graphQLRegistry.registerQuery(name, sdl, resolverFunction, visibility)

Registers a GraphQL query that can be executed through the GraphQL endpoint.

**Parameters:**

- `name` (string): Name of the query (e.g., `"users"`, `"getPosts"`)
- `sdl` (string): GraphQL SDL (Schema Definition Language) for the query
- `resolverFunction` (string): Name of your JavaScript resolver function
- `visibility` (string): Visibility level - `"internal"` (script-only), `"engine"` (all scripts), or `"external"` (authenticated API access)

**Example:**

```javascript
// Define a simple query
function getUsers() {
  return JSON.stringify([
    { id: 1, name: "Alice", email: "alice@example.com" },
    { id: 2, name: "Bob", email: "bob@example.com" },
  ]);
}

// Register the query
graphQLRegistry.registerQuery(
  "users",
  `type User {
    id: Int!
    name: String!
    email: String!
  }
  type Query {
    users: [User!]!
  }`,
  "getUsers",
  "external",
);
```

**Example with Arguments:**

```javascript
function getUserById(args) {
  const userId = args.id;
  // Simulate database lookup
  const users = [
    { id: 1, name: "Alice", email: "alice@example.com" },
    { id: 2, name: "Bob", email: "bob@example.com" },
  ];

  const user = users.find((u) => u.id === userId);
  return user ? JSON.stringify(user) : JSON.stringify(null);
}

graphQLRegistry.registerQuery(
  "user",
  `type User {
    id: Int!
    name: String!
    email: String!
  }
  type Query {
    user(id: Int!): User
  }`,
  "getUserById",
  "external",
);
```

### graphQLRegistry.registerMutation(name, sdl, resolverFunction, visibility)

Registers a GraphQL mutation for modifying data.

**Parameters:**

- `name` (string): Name of the mutation
- `sdl` (string): GraphQL SDL (Schema Definition Language) for the mutation
- `resolverFunction` (string): Name of your JavaScript resolver function
- `visibility` (string): Visibility level - `"internal"` (script-only), `"engine"` (all scripts), or `"external"` (authenticated API access)

**Example:**

```javascript
function createUser(args) {
  const { name, email } = args;

  // Simulate creating a user
  const newUser = {
    id: Date.now(), // Simple ID generation
    name: name,
    email: email,
    createdAt: new Date().toISOString(),
  };

  console.log(`Created user: ${name} (${email})`);

  return JSON.stringify(newUser);
}

graphQLRegistry.registerMutation(
  "createUser",
  `type User {
    id: Int!
    name: String!
    email: String!
    createdAt: String!
  }
  type Mutation {
    createUser(name: String!, email: String!): User!
  }`,
  "createUser",
  "external",
);
```

### graphQLRegistry.registerSubscription(name, sdl, resolverFunction, visibility)

Registers a GraphQL subscription for real-time data streaming.

**Parameters:**

- `name` (string): Name of the subscription
- `sdl` (string): GraphQL SDL (Schema Definition Language) for the subscription
- `resolverFunction` (string): Name of your JavaScript resolver function
- `visibility` (string): Visibility level - `"internal"` (script-only), `"engine"` (all scripts), or `"external"` (authenticated API access)

**Example:**

```javascript
function onUserActivity() {
  // Initial subscription message
  return {
    type: "subscription_started",
    message: "User activity monitoring started",
    timestamp: new Date().toISOString(),
  };
}

graphQLRegistry.registerSubscription(
  "userActivity",
  `type ActivityEvent {
    type: String!
    message: String!
    timestamp: String!
    userId: String
  }
  type Subscription {
    userActivity: ActivityEvent!
  }`,
  "onUserActivity",
  "external",
);
```

### graphQLRegistry.sendSubscriptionMessage(subscriptionName, data)

Sends a message to all clients subscribed to a specific GraphQL subscription.

**Parameters:**

- `subscriptionName` (string): Name of the subscription to send to
- `data` (string): JSON string containing the message data

**Example:**

```javascript
function logUserAction(req) {
  const { userId, action } = req.form;

  // Send real-time update to subscribers
  const message = JSON.stringify({
    type: "user_action",
    userId: userId,
    action: action,
    timestamp: new Date().toISOString(),
  });

  graphQLRegistry.sendSubscriptionMessage("userActivity", message);

  return {
    status: 200,
    body: "Action logged",
    contentType: "text/plain; charset=UTF-8",
  };
}

routeRegistry.registerRoute("/log-action", "logUserAction", "POST");
```

### graphQLRegistry.sendSubscriptionMessageFiltered(subscriptionName, data, filterJson)

Sends a message to filtered clients subscribed to a specific GraphQL subscription based on connection metadata.

**Parameters:**

- `subscriptionName` (string): Name of the subscription to send to
- `data` (string): JSON string containing the message data
- `filterJson` (string, optional): JSON string with filter criteria for connection metadata

**Example:**

```javascript
function broadcastAdminMessage(req) {
  const { message } = req.form;

  // Send to all admin connections only
  const messageData = JSON.stringify({
    type: "admin_notification",
    message: message,
    timestamp: new Date().toISOString(),
  });

  const filter = JSON.stringify({ role: "admin" });

  graphQLRegistry.sendSubscriptionMessageFiltered(
    "adminNotifications",
    messageData,
    filter,
  );

  return {
    status: 200,
    body: "Admin notification sent",
    contentType: "text/plain; charset=UTF-8",
  };
}

routeRegistry.registerRoute(
  "/admin/broadcast",
  "broadcastAdminMessage",
  "POST",
);
```

### graphQLRegistry.executeGraphQL(query, variables)

Executes a GraphQL query or mutation directly against the registered schema without making an HTTP request.

**Parameters:**

- `query` (string): GraphQL query/mutation string
- `variables` (string, optional): JSON string containing variables for the query

**Returns:** JSON string containing the GraphQL response with `data` and/or `errors` fields

**Example - Simple Query:**

```javascript
function listScriptsHandler(req) {
  const query = `
    query {
      scripts {
        uri
        chars
      }
    }
  `;

  try {
    const resultJson = graphQLRegistry.executeGraphQL(query);
    const result = JSON.parse(resultJson);

    if (result.errors) {
      console.log("GraphQL errors: " + JSON.stringify(result.errors));
      return {
        status: 500,
        body: "GraphQL query failed",
        contentType: "text/plain; charset=UTF-8",
      };
    }

    return {
      status: 200,
      body: JSON.stringify(result.data),
      contentType: "application/json",
    };
  } catch (error) {
    console.log("executeGraphQL error: " + error);
    return {
      status: 500,
      body: "Internal error",
      contentType: "text/plain; charset=UTF-8",
    };
  }
}
```

**Example - Query with Variables:**

```javascript
function getScriptHandler(req) {
  const scriptUri = req.query.uri;

  if (!scriptUri) {
    return {
      status: 400,
      body: "Missing uri parameter",
      contentType: "text/plain; charset=UTF-8",
    };
  }

  const query = `
    query GetScript($uri: String!) {
      script(uri: $uri) {
        uri
        content
        contentLength
        logs
      }
    }
  `;

  const variables = JSON.stringify({
    uri: scriptUri,
  });

  const resultJson = graphQLRegistry.executeGraphQL(query, variables);
  const result = JSON.parse(resultJson);

  return {
    status: 200,
    body: JSON.stringify(result.data),
    contentType: "application/json",
  };
}
```

**Example - Mutation:**

```javascript
function createScriptHandler(req) {
  const { uri, content } = req.form;

  if (!uri || !content) {
    return {
      status: 400,
      body: "Missing uri or content",
      contentType: "text/plain; charset=UTF-8",
    };
  }

  const mutation = `
    mutation CreateScript($uri: String!, $content: String!) {
      upsertScript(uri: $uri, content: $content) {
        message
        uri
        chars
        success
      }
    }
  `;

  const variables = JSON.stringify({
    uri: uri,
    content: content,
  });

  const resultJson = graphQLRegistry.executeGraphQL(mutation, variables);
  const result = JSON.parse(resultJson);

  if (result.data?.upsertScript?.success) {
    return {
      status: 201,
      body: JSON.stringify(result.data.upsertScript),
      contentType: "application/json",
    };
  } else {
    return {
      status: 500,
      body: "Failed to create script",
      contentType: "text/plain; charset=UTF-8",
    };
  }
}
```

### GraphQL Schema Definition

GraphQL schemas are defined using the GraphQL Schema Definition Language (SDL). Here are the key concepts:

**Types:**

- `String!` - Non-nullable string
- `String` - Nullable string
- `Int!` - Non-nullable integer
- `Int` - Nullable integer
- `Boolean!` - Non-nullable boolean
- `Boolean` - Nullable boolean
- `[Type!]!` - Non-nullable array of non-nullable types
- `[Type!]` - Nullable array of non-nullable types

**Example Schema:**

```graphql
type User {
  id: Int!
  name: String!
  email: String!
  posts: [Post!]!
}

type Post {
  id: Int!
  title: String!
  content: String!
  author: User!
}

type Query {
  users: [User!]!
  user(id: Int!): User
  posts(limit: Int): [Post!]!
}

type Mutation {
  createUser(name: String!, email: String!): User!
  createPost(title: String!, content: String!, authorId: Int!): Post!
}

type Subscription {
  userCreated: User!
  postCreated: Post!
}
```

### Resolver Functions

Resolver functions receive arguments and return JSON strings:

```javascript
function getUserById(args) {
  const { id } = args;

  // Your logic here
  const user = findUserById(id);

  if (user) {
    return JSON.stringify({
      id: user.id,
      name: user.name,
      email: user.email,
    });
  } else {
    return JSON.stringify(null);
  }
}

function createUser(args) {
  const { name, email } = args;

  // Create user logic
  const newUser = {
    id: generateId(),
    name: name,
    email: email,
  };

  return JSON.stringify(newUser);
}
```

### GraphQL Client Usage

Once you've registered GraphQL operations, clients can query them via HTTP POST to `/graphql`:

```javascript
// From a web browser or external client
fetch("/graphql", {
  method: "POST",
  headers: {
    "Content-Type": "application/json",
  },
  body: JSON.stringify({
    query: `
      query GetUsers {
        users {
          id
          name
          email
        }
      }
    `,
  }),
})
  .then((response) => response.json())
  .then((data) => console.log(data));
```

### Best Practices for GraphQL

1. **Define clear schemas**: Use descriptive type and field names
2. **Handle nulls properly**: Use nullable types (`String`) when data might be missing
3. **Validate input**: Always validate arguments in your resolvers
4. **Use meaningful errors**: Return descriptive error messages
5. **Log important operations**: Use `console.log()` for debugging mutations
6. **Keep resolvers simple**: Complex logic should be in separate functions
7. **Use executeGraphQL for internal calls**: Prefer `graphQLRegistry.executeGraphQL()` over HTTP fetch for internal GraphQL operations
8. **Handle subscription connections**: Use `graphQLRegistry.sendSubscriptionMessage()` to broadcast real-time updates

## Request Object

The `req` parameter passed to handler functions contains information about the HTTP request.

### Properties

- `method` (string): HTTP method (`"GET"`, `"POST"`, `"PUT"`, `"DELETE"`)
- `path` (string): Request path (e.g., `"/api/users/123"`)
- `query` (object): Query parameters as key-value pairs
- `form` (object): Form data for POST requests (key-value pairs)
- `headers` (object): Request headers

### Examples

```javascript
function exampleHandler(req) {
  // GET /search?q=javascript&page=1
  console.log(req.method); // "GET"
  console.log(req.path); // "/search"
  console.log(req.query); // { q: "javascript", page: "1" }
  console.log(req.form); // {} (empty for GET)

  return { status: 200, body: "OK" };
}
```

For POST requests with form data:

```javascript
function postHandler(req) {
  // POST /submit with form fields: name=John&email=john@example.com
  console.log(req.form); // { name: "John", email: "john@example.com" }

  return { status: 200, body: "Form received" };
}
```

## Response Object

Handler functions must return a response object that defines how the server responds to the request.

### Required Properties

- `status` (number): HTTP status code (e.g., 200, 404, 500)
- `body` (string): Response content

### Optional Properties

- `contentType` (string): MIME type (defaults to `"text/plain; charset=UTF-8"`)

### Response Examples

```javascript
// Simple text response
return {
  status: 200,
  body: "Hello World",
  contentType: "text/plain; charset=UTF-8",
};

// JSON response
return {
  status: 200,
  body: JSON.stringify({ message: "Success", data: [] }),
  contentType: "application/json",
};

// HTML response
return {
  status: 200,
  body: "<h1>Welcome</h1><p>This is HTML content.</p>",
  contentType: "text/html; charset=UTF-8",
};

// Error response
return {
  status: 404,
  body: "Not Found",
  contentType: "text/plain; charset=UTF-8",
};
```

## Response Builders

Convenient helper functions for creating common HTTP responses. These functions return properly formatted response objects that can be returned directly from handlers.

### ResponseBuilder.json(data, status)

Creates a JSON response with automatic content-type header.

**Parameters:**

- `data` (any): Data to serialize as JSON
- `status` (number, optional): HTTP status code (default: 200)

**Returns:** Response object

**Example:**

```javascript
function apiHandler(context) {
  const data = { users: ["Alice", "Bob"], count: 2 };
  return ResponseBuilder.json(data);
}

function errorHandler(context) {
  return ResponseBuilder.json({ error: "Not found" }, 404);
}
```

### ResponseBuilder.text(text, status)

Creates a plain text response.

**Parameters:**

- `text` (string): Text content
- `status` (number, optional): HTTP status code (default: 200)

**Returns:** Response object

**Example:**

```javascript
function helloHandler(context) {
  return ResponseBuilder.text("Hello, World!");
}
```

### ResponseBuilder.html(html, status)

Creates an HTML response.

**Parameters:**

- `html` (string): HTML content
- `status` (number, optional): HTTP status code (default: 200)

**Returns:** Response object

**Example:**

```javascript
function pageHandler(context) {
  const html = `
    <!DOCTYPE html>
    <html>
    <body>
      <h1>Welcome</h1>
      <p>This is a dynamic page.</p>
    </body>
    </html>
  `;
  return ResponseBuilder.html(html);
}
```

### ResponseBuilder.error(status, message)

Creates a JSON error response.

**Parameters:**

- `status` (number): HTTP status code
- `message` (string): Error message

**Returns:** Response object

**Example:**

```javascript
function notFoundHandler(context) {
  return ResponseBuilder.error(404, "Resource not found");
}
```

### ResponseBuilder.noContent()

Creates a 204 No Content response.

**Returns:** Response object

**Example:**

```javascript
function deleteHandler(context) {
  // Delete resource
  deleteResource(context.request.params.id);
  return ResponseBuilder.noContent();
}
```

### ResponseBuilder.redirect(url, status)

Creates a redirect response.

**Parameters:**

- `url` (string): Redirect URL
- `status` (number, optional): HTTP status code (default: 302)

**Returns:** Response object

**Example:**

```javascript
function redirectHandler(context) {
  return ResponseBuilder.redirect("/new-location", 301);
}
```

## Validation Helpers

Functions for validating request parameters and input data. These helpers throw errors when validation fails, making it easy to handle invalid input.

### requireQueryParam(paramName)

Requires a query parameter to be present and non-empty.

**Parameters:**

- `paramName` (string): Name of the required query parameter

**Returns:** String value of the parameter

**Throws:** Error if parameter is missing or empty

**Example:**

```javascript
function searchHandler(context) {
  try {
    const query = requireQueryParam("q");
    const results = search(query);
    return ResponseBuilder.json({ results });
  } catch (error) {
    return ResponseBuilder.error(400, error.message);
  }
}
```

### requirePathParam(paramName)

Requires a path parameter to be present and non-empty.

**Parameters:**

- `paramName` (string): Name of the required path parameter

**Returns:** String value of the parameter

**Throws:** Error if parameter is missing or empty

**Example:**

```javascript
function userHandler(context) {
  try {
    const userId = requirePathParam("id");
    const user = getUser(userId);
    return ResponseBuilder.json({ user });
  } catch (error) {
    return ResponseBuilder.error(400, error.message);
  }
}

// Register with path parameter
routeRegistry.registerRoute("/users/:id", "userHandler", "GET");
```

### validateString(value, minLength, maxLength)

Validates a string's length constraints.

**Parameters:**

- `value` (string): String to validate
- `minLength` (number, optional): Minimum length (default: 0)
- `maxLength` (number, optional): Maximum length (default: Infinity)

**Returns:** The validated string

**Throws:** Error if validation fails

**Example:**

```javascript
function createUserHandler(context) {
  const req = context.request;

  try {
    const name = validateString(req.form.name, 1, 100);
    const email = validateString(req.form.email, 5, 255);

    const user = createUser(name, email);
    return ResponseBuilder.json({ user }, 201);
  } catch (error) {
    return ResponseBuilder.error(400, error.message);
  }
}
```

### validateNumber(value, min, max)

Validates a number's range constraints.

**Parameters:**

- `value` (any): Value to parse and validate as number
- `min` (number, optional): Minimum value
- `max` (number, optional): Maximum value

**Returns:** The validated number

**Throws:** Error if validation fails

**Example:**

```javascript
function updateScoreHandler(context) {
  const req = context.request;

  try {
    const score = validateNumber(req.form.score, 0, 100);
    updateScore(req.params.userId, score);
    return ResponseBuilder.json({ success: true });
  } catch (error) {
    return ResponseBuilder.error(400, error.message);
  }
}
```

### optionalQueryParam(paramName, defaultValue)

Gets an optional query parameter with a fallback value.

**Parameters:**

- `paramName` (string): Name of the query parameter
- `defaultValue` (any): Default value if parameter is missing

**Returns:** Parameter value or default value

**Example:**

```javascript
function listHandler(context) {
  const page = optionalQueryParam("page", "1");
  const limit = optionalQueryParam("limit", "10");

  const results = getItems(parseInt(page), parseInt(limit));
  return ResponseBuilder.json({ results });
}
```

## Validation Object

The `validate` object provides the same validation functions with a more structured API:

```javascript
// Object-style API
const userId = validate.requirePathParam(context, "userId");
const name = validate.validateString(req.form.name, {
  minLength: 1,
  maxLength: 100,
});
const age = validate.validateNumber(req.form.age, { min: 0, max: 150 });
```

**Available methods:**

- `validate.requireQueryParam(context, paramName)`
- `validate.requirePathParam(context, paramName)`
- `validate.validateString(value, options)` - options: `{ minLength, maxLength, pattern }`
- `validate.validateNumber(value, options)` - options: `{ min, max }`

### Console

Basic console logging (output goes to server logs).

**Methods:**

- `console.log(message)`: Log a message (level: LOG)
- `console.info(message)`: Log an informational message (level: INFO)
- `console.warn(message)`: Log a warning message (level: WARN)
- `console.error(message)`: Log an error message (level: ERROR)
- `console.debug(message)`: Log a debug message (level: DEBUG)
- `console.listLogs()`: Retrieve all log entries as a JSON string
- `console.listLogsForUri(uri)`: Retrieve log entries for a specific script URI as a JSON string
- `console.pruneLogs()`: Remove old log entries to free up storage space

**Example:**

```javascript
function debugHandler(req) {
  console.log("Request received: " + req.path);
  console.error("This is an error message");

  return { status: 200, body: "Check logs" };
}

function viewLogsHandler(req) {
  // Get all logs
  const allLogsJson = console.listLogs();
  const allLogs = JSON.parse(allLogsJson);

  // Get logs for a specific script
  const scriptLogsJson = console.listLogsForUri("/api/users");
  const scriptLogs = JSON.parse(scriptLogsJson);

  // Each log entry has: message, level, timestamp (in milliseconds)
  return {
    status: 200,
    body: JSON.stringify({ allLogs, scriptLogs }),
    contentType: "application/json",
  };
}
```

## HTTP Status Codes

Common HTTP status codes you might use:

- `200` - OK (success)
- `201` - Created (resource created)
- `400` - Bad Request (invalid input)
- `401` - Unauthorized (authentication required)
- `403` - Forbidden (access denied)
- `404` - Not Found (resource doesn't exist)
- `405` - Method Not Allowed (wrong HTTP method)
- `500` - Internal Server Error (server error)

## Content Types

Common MIME types for `contentType`:

- `"text/plain; charset=UTF-8"` - Plain text
- `"text/html; charset=UTF-8"` - HTML content
- `"application/json"` - JSON data
- `"application/xml"` - XML data
- `"image/jpeg"`, `"image/png"` - Images
- `"application/pdf"` - PDF files

## Conversion Functions

The `convert` object provides content conversion utilities.

### convert.markdown_to_html(markdown)

Converts a markdown string to HTML.

**Parameters:**

- `markdown` (string): Markdown content to convert (max 1MB)

**Returns:** String containing HTML output or error message (starting with "Error:")

**Example:**

```javascript
function renderBlogPost(context) {
  const req = context.request;

  const markdown = `# My Blog Post

This is **bold** and *italic* text.

\`\`\`javascript
const hello = "world";
\`\`\`
`;

  const html = convert.markdown_to_html(markdown);

  if (html.startsWith("Error:")) {
    return { status: 500, body: html };
  }

  return {
    status: 200,
    body: `<!DOCTYPE html>
<html>
<head><title>Blog</title></head>
<body>${html}</body>
</html>`,
    contentType: "text/html; charset=UTF-8",
  };
}
```

**Supported Features:**

- Headings, bold, italic, strikethrough
- Code blocks and inline code
- Lists (ordered and unordered)
- Tables with alignment
- Links and images
- Blockquotes
- Task lists
- Footnotes

**See Also:** [Conversion API Reference](conversion-api.md) for detailed documentation, examples, and best practices.

## Scheduler Service

Privileged scripts can use `schedulerService` to register background jobs that run even when no HTTP requests are active. Jobs live entirely in memory and are cleared automatically whenever the script is reinitialized or deleted.

> **Requirements:**
>
> - Only privileged scripts may call these methods.
> - All timestamps must be expressed in UTC (ISO-8601 strings ending with `Z`).
> - Scheduled handlers run with admin privileges scoped to the script.

### schedulerService.registerOnce(options)

Schedule a single execution at an exact UTC timestamp.

**Options:**

- `handler` (string, required): Name of the handler function to invoke.
- `runAt` (string, required): ISO-8601 timestamp in UTC, e.g. `"2025-03-01T12:00:00Z"`.
- `name` (string, optional): Friendly identifier used for logging/overwriting. Defaults to the handler name.

**Returns:** String describing the scheduled execution time and job id.

### schedulerService.registerRecurring(options)

Register a handler that executes on a fixed interval.

**Options:**

- `handler` (string, required)
- `intervalMinutes` (number, required, `>= 1`): Interval length in minutes.
- `startAt` (string, optional): UTC timestamp for the first execution. When omitted the first run happens one interval from now.
- `name` (string, optional): Friendly identifier used for logging/overwriting.

**Returns:** String indicating the cadence, next run, and job id.

### schedulerService.clearAll()

Removes every scheduled job for the current script. This runs automatically before each `init()` execution but can be called manually if your script rebuilds schedules dynamically.

### Scheduled Handler Context

Scheduled invocations receive the unified `context` object with extra metadata:

```javascript
function sendReport(context) {
  const schedule = context.meta?.schedule;
  console.log(
    `Running job ${schedule.name} (${schedule.jobId}) at ${schedule.scheduledFor}`,
  );
  // ... generate report ...
  return { status: 200, body: "OK" };
}
```

`context.meta.schedule` contains:

- `jobId`: UUID assigned by the scheduler
- `name`: Job name (defaults to handler name)
- `type`: `"one-off"` or `"recurring"`
- `scheduledFor`: Current execution time in UTC
- `intervalSeconds`: Interval length for recurring jobs (or `null`)

### Complete Example

```javascript
function sendDailyDigest(context) {
  const info = context.meta?.schedule || {};
  console.log(`Digest job ${info.name} running @ ${info.scheduledFor}`);
  // Build and send email, push notification, etc.
  return { status: 200, body: "sent" };
}

function init(context) {
  // Clear any stale definitions from previous deploys
  schedulerService.clearAll();

  // Run once at a fixed time tomorrow
  schedulerService.registerOnce({
    handler: "sendDailyDigest",
    runAt: "2025-01-01T09:00:00Z",
    name: "digest-onboarding",
  });

  // Run every 30 minutes starting immediately
  schedulerService.registerRecurring({
    handler: "sendDailyDigest",
    intervalMinutes: 30,
    name: "digest-heartbeat",
  });

  return { success: true };
}
```

Tips:

1. Pick deterministic `name` values so re-registration overwrites the previous job instead of creating duplicates.
2. Keep scheduled handlers idempotent—if the engine restarts, missed jobs resume on the next interval.
3. Log meaningful progress or failures. The engine also records `FATAL` log entries when a scheduled handler throws.

## Message Dispatcher

The `dispatcher` object enables internal communication between scripts through a publish-subscribe pattern. This allows scripts to coordinate actions, share events, and decouple their logic without making HTTP requests or using external systems.

### Why Use the Dispatcher?

- **Decouple scripts**: Scripts can notify others of events without knowing who's listening
- **Event-driven architecture**: Build reactive systems where scripts respond to domain events
- **Performance**: In-process messaging is faster than HTTP calls between scripts
- **Flexibility**: Add new listeners without modifying event publishers

### Use Cases

- User management script notifies presence tracker when users join/leave
- Content creation script broadcasts new post events to notification scripts
- Asset processing script signals completion to dependent workflows
- Chat group script informs search indexer about new messages

### dispatcher.registerListener(messageType, handlerName)

Registers a handler function to receive messages of a specific type.

**Parameters:**

- `messageType` (string, required): The type of message to listen for (e.g., `"user.created"`, `"message.sent"`)
- `handlerName` (string, required): Name of the handler function that will process messages

**Returns:** String describing registration result

**Handler Signature:**

Message handlers receive a `context` object with:

- `context.messageType`: The message type string
- `context.messageData`: The message data object (already parsed from JSON)

**Example:**

```javascript
function handleUserCreated(context) {
  const userData = context.messageData;
  console.log(`New user created: ${userData.username} (${userData.email})`);

  // Perform actions in response to the event
  // e.g., send welcome email, update analytics, etc.
}

function init(context) {
  // Register listener during script initialization
  dispatcher.registerListener("user.created", "handleUserCreated");

  return { success: true };
}
```

**Notes:**

- Multiple scripts can register listeners for the same message type
- A single script can register multiple handlers for the same message type
- Listeners are scoped to the script that registered them
- Registration typically happens in the `init()` function

### dispatcher.sendMessage(messageType, messageData)

Sends a message to all registered listeners for a specific message type.

**Parameters:**

- `messageType` (string, required): The type of message to send
- `messageData` (string, optional): JSON string containing message data. Defaults to `"{}"`

**Returns:** String with dispatch statistics (successful/failed handler invocations)

**Example:**

```javascript
function createUserHandler(context) {
  const req = context.request;
  const { username, email } = req.form;

  // Create the user (database operation, etc.)
  const userId = saveUserToDatabase(username, email);

  // Notify other scripts about the new user
  const messageData = JSON.stringify({
    userId: userId,
    username: username,
    email: email,
    createdAt: new Date().toISOString(),
  });

  const result = dispatcher.sendMessage("user.created", messageData);
  console.log(`User creation event: ${result}`);

  return {
    status: 200,
    body: JSON.stringify({ success: true, userId: userId }),
    contentType: "application/json",
  };
}

routeRegistry.registerRoute("/users", "createUserHandler", "POST");
```

**Message Delivery:**

- Messages are delivered synchronously to all registered handlers
- Handlers execute in separate contexts to prevent interference
- If a handler throws an error, other handlers still execute
- Failed handler invocations are logged but don't fail the `sendMessage()` call
- Returns a summary of successful/failed deliveries

### Complete Example: User Activity System

```javascript
/**
 * User Service Script - Manages user accounts
 */

function createUser(context) {
  const req = context.request;
  const { username, email } = req.form;

  // Store user in database
  const userId = sharedStorage.getItem("nextUserId") || "1";
  sharedStorage.setItem("user:" + userId, JSON.stringify({ username, email }));
  sharedStorage.setItem("nextUserId", String(parseInt(userId) + 1));

  // Broadcast user creation event
  dispatcher.sendMessage(
    "user.created",
    JSON.stringify({
      userId: userId,
      username: username,
      email: email,
      timestamp: new Date().toISOString(),
    }),
  );

  return {
    status: 201,
    body: JSON.stringify({ success: true, userId: userId }),
    contentType: "application/json",
  };
}

function init(context) {
  routeRegistry.registerRoute("/users", "createUser", "POST");
  return { success: true };
}
```

```javascript
/**
 * Analytics Script - Tracks user activity
 */

function onUserCreated(context) {
  const userData = context.messageData;
  console.log(
    `Analytics: New user ${userData.username} created at ${userData.timestamp}`,
  );

  // Update metrics
  const totalUsers = parseInt(
    sharedStorage.getItem("metrics:totalUsers") || "0",
  );
  sharedStorage.setItem("metrics:totalUsers", String(totalUsers + 1));
}

function init(context) {
  // Listen for user creation events
  dispatcher.registerListener("user.created", "onUserCreated");
  return { success: true };
}
```

```javascript
/**
 * Notification Script - Sends welcome messages
 */

function onUserCreated(context) {
  const userData = context.messageData;

  // Send welcome notification
  routeRegistry.sendStreamMessage("/notifications", {
    type: "welcome",
    userId: userData.userId,
    message: `Welcome ${userData.username}!`,
    timestamp: new Date().toISOString(),
  });

  console.log(`Sent welcome notification to user ${userData.username}`);
}

function init(context) {
  // Register stream and listener
  routeRegistry.registerStreamRoute("/notifications");
  dispatcher.registerListener("user.created", "onUserCreated");
  return { success: true };
}
```

### Dispatcher Best Practices

1. **Use descriptive message types**: Follow a naming convention like `"domain.action"` (e.g., `"user.created"`, `"message.sent"`, `"order.completed"`)

2. **JSON-serialize message data**: Always pass message data as a JSON string:

   ```javascript
   dispatcher.sendMessage("event.name", JSON.stringify({ key: "value" }));
   ```

3. **Register in init()**: Set up listeners in your script's `init()` function so they're established at startup

4. **Handle errors**: Message handlers should catch and log errors to prevent disrupting the dispatcher:

   ```javascript
   function handleMessage(context) {
     try {
       // Process message
     } catch (error) {
       console.log(`Error handling message: ${error.message}`);
     }
   }
   ```

5. **Keep handlers lightweight**: Handlers run synchronously, so avoid long-running operations

6. **Document your events**: Maintain a list of message types your scripts publish and consume

### Dispatcher vs. Other Communication Methods

**Use dispatcher when:**

- Scripts need to react to domain events
- You want loose coupling between scripts
- Events should fan out to multiple listeners
- Communication is within the same server instance

**Use HTTP routes when:**

- External clients need to trigger actions
- You need request/response semantics
- Operations are user-initiated or require authentication

**Use streams when:**

- Pushing real-time updates to external clients
- Broadcasting to web browsers or other HTTP consumers
- Need Server-Sent Events (SSE) functionality

**Use shared storage when:**

- Scripts need to persist data
- Coordinating state across requests
- Caching computed values

## Error Handling

Scripts run in a sandboxed environment. If a script throws an error:

- The server returns a `500 Internal Server Error`
- The error is logged to the server logs
- The request fails gracefully

**Example error handling:**

```javascript
function safeHandler(req) {
  try {
    // Your code here
    if (!req.query.id) {
      return { status: 400, body: "Missing id parameter" };
    }

    return { status: 200, body: "Success" };
  } catch (error) {
    console.log("Error in handler: " + error.message);
    return { status: 500, body: "Internal server error" };
  }
}
```

## Best Practices

1. **Validate input**: Always check required parameters
2. **Use appropriate status codes**: Return meaningful HTTP status codes
3. **Set content types**: Specify correct MIME types for responses
4. **Log important events**: Use `console.log()` for debugging
5. **Handle errors gracefully**: Use try-catch for robust scripts
6. **Keep responses small**: Avoid very large response bodies

## Next Steps

- See [examples](../examples/index.md) for practical usage patterns
- Use the web editor at `/editor` for testing and development
- Check the [deployment workflow](../getting-started/03-deployment-workflow.md) for publishing scripts
