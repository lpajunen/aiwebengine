# JavaScript APIs Reference

This page provides a complete reference for the JavaScript APIs available in aiwebengine scripts. These functions and objects allow you to handle HTTP requests, generate responses, log information, and interact with the server environment.

## Global Functions

### register(path, handlerName, method)

Registers a route that maps a URL path to a handler function.

**Parameters:**

- `path` (string): URL path to register (e.g., `"/api/users"`)
- `handlerName` (string): Name of your handler function
- `method` (string): HTTP method (`"GET"`, `"POST"`, `"PUT"`, `"DELETE"`)

**Example:**

```javascript
function getUsers(req) {
  return { status: 200, body: "User list", contentType: "text/plain" };
}

register("/api/users", "getUsers", "GET");
```

### writeLog(message)

Writes a message to the server log for debugging and monitoring.

**Parameters:**

- `message` (string): Message to log

**Example:**

```javascript
function myHandler(req) {
  writeLog("Handler called with path: " + req.path);
  return { status: 200, body: "Logged", contentType: "text/plain" };
}
```

### registerWebStream(path)

Registers a Server-Sent Events (SSE) stream endpoint that clients can connect to for real-time updates.

**Parameters:**

- `path` (string): Stream path to register (must start with `/`, max 200 characters)

**Example:**

```javascript
// Register a stream for live notifications
registerWebStream("/notifications");

// Register a stream for chat messages
registerWebStream("/chat/room1");
```

**Notes:**

- Stream paths must be unique
- Multiple clients can connect to the same stream
- Streams persist until the server restarts or the script is reloaded
- Use meaningful, descriptive paths for better organization

### sendStreamMessage(data)

Broadcasts a message to all clients connected to registered streams.

**Parameters:**

- `data` (object): Data object to send (will be JSON serialized)

**Example:**

```javascript
function notifyHandler(req) {
  // Send notification to all connected streams
  sendStreamMessage({
    type: "notification",
    message: "New update available",
    timestamp: new Date().toISOString(),
    priority: "high",
  });

  return { status: 200, body: "Notification sent" };
}

// Register the handler
register("/notify", "notifyHandler", "POST");
```

**Real-time Chat Example:**

```javascript
// Register a chat stream
registerWebStream("/chat");

function sendMessage(req) {
  const { user, message } = req.form;

  if (!user || !message) {
    return { status: 400, body: "Missing user or message" };
  }

  // Broadcast to all connected chat clients
  sendStreamMessage({
    type: "chat_message",
    user: user,
    message: message,
    timestamp: new Date().toISOString(),
  });

  return { status: 200, body: "Message sent" };
}

register("/chat/send", "sendMessage", "POST");
```

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

1. **Registration**: Use `registerWebStream()` to create a stream endpoint
2. **Connection**: Clients connect using EventSource or compatible SSE clients
3. **Broadcasting**: Use `sendStreamMessage()` to send data to all connected clients
4. **Cleanup**: Connections are automatically cleaned up when clients disconnect

### Best Practices for Streaming

- **Register streams early**: Call `registerWebStream()` when your script loads
- **Structure your data**: Use consistent message formats with `type` fields
- **Handle disconnections**: Clients should implement reconnection logic
- **Limit message frequency**: Avoid overwhelming clients with too many messages
- **Use meaningful paths**: Organize streams logically (e.g., `/chat/room1`, `/notifications/user123`)

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

- `contentType` (string): MIME type (defaults to `"text/plain"`)

### Response Examples

```javascript
// Simple text response
return {
  status: 200,
  body: "Hello World",
  contentType: "text/plain",
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
  contentType: "text/html",
};

// Error response
return {
  status: 404,
  body: "Not Found",
  contentType: "text/plain",
};
```

## Built-in JavaScript Objects

### JSON

Standard JavaScript JSON object for parsing and stringifying JSON data.

**Methods:**

- `JSON.stringify(obj)`: Convert object to JSON string
- `JSON.parse(str)`: Parse JSON string to object

**Example:**

```javascript
function apiHandler(req) {
  const data = { users: ["Alice", "Bob"], count: 2 };
  return {
    status: 200,
    body: JSON.stringify(data),
    contentType: "application/json",
  };
}
```

### Console

Basic console logging (output goes to server logs).

**Methods:**

- `console.log(message)`: Log a message
- `console.error(message)`: Log an error message

**Example:**

```javascript
function debugHandler(req) {
  console.log("Request received: " + req.path);
  console.error("This is an error message");

  return { status: 200, body: "Check logs" };
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

- `"text/plain"` - Plain text
- `"text/html"` - HTML content
- `"application/json"` - JSON data
- `"application/xml"` - XML data
- `"image/jpeg"`, `"image/png"` - Images
- `"application/pdf"` - PDF files

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
    writeLog("Error in handler: " + error.message);
    return { status: 500, body: "Internal server error" };
  }
}
```

## Best Practices

1. **Validate input**: Always check required parameters
2. **Use appropriate status codes**: Return meaningful HTTP status codes
3. **Set content types**: Specify correct MIME types for responses
4. **Log important events**: Use `writeLog()` for debugging
5. **Handle errors gracefully**: Use try-catch for robust scripts
6. **Keep responses small**: Avoid very large response bodies

## Next Steps

- See [examples](examples.md) for practical usage patterns
- Learn about [local development](../engine-administrators/local-development.md) workflows
- Try the [remote editor](../engine-administrators/remote-development.md) for testing
