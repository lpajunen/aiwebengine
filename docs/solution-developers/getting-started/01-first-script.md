# Your First Script

Welcome to aiwebengine! This guide will walk you through creating your first JavaScript script and deploying it to the engine.

## What You'll Build

A simple "Hello World" API endpoint that:

- Responds to HTTP GET requests
- Accepts query parameters
- Returns personalized greetings
- Logs each request

## Prerequisites

Before you start, make sure you have:

- aiwebengine running and accessible
- Access to the `/editor` interface OR the deployer tool
- Basic JavaScript knowledge

## Step 1: Understanding Script Structure

Every aiwebengine script has three key parts:

```javascript
// 1. Handler Function - processes requests
function myHandler(req) {
  return {
    status: 200,
    body: "Hello!",
    contentType: "text/plain; charset=UTF-8",
  };
}

// 2. Initialization Function - registers routes
function init() {
  routeRegistry.registerRoute("/hello", "myHandler", "GET");
}

// 3. Init call - runs when script loads
init();
```

**Key Concepts:**

- **Handler functions** receive a `req` object and return a response object
- **`init()` function** registers your routes when the script loads
- **`routeRegistry.registerRoute(path, handlerName, method)`** maps URLs to handler functions

## Step 2: Create Your First Script

### Option A: Using the Web Editor

1. **Open the editor:**

   ```
   http://localhost:8080/editor
   ```

2. **Click "New Script"**

3. **Enter script name:**

   ```
   hello.js
   ```

4. **Paste this code:**

```javascript
/**
 * hello.js - Your first aiwebengine script
 *
 * A simple greeting API that demonstrates:
 * - Request handling
 * - Query parameters
 * - Response formatting
 * - Logging
 */

function helloHandler(req) {
  // Extract the 'name' parameter from the query string
  const name = req.query.name || "World";

  // Log the request
  console.log(`Greeting requested for: ${name}`);

  // Create the greeting message
  const greeting = `Hello, ${name}! Welcome to aiwebengine.`;

  // Return the response
  return {
    status: 200,
    body: greeting,
    contentType: "text/plain; charset=UTF-8",
  };
}

function init() {
  // Register the route
  routeRegistry.registerRoute("/hello", "helloHandler", "GET");
  console.log("Hello script initialized successfully");
}

// Initialize the script
init();
```

5. **Click "Save"**

### Option B: Using the Deployer Tool

1. **Create a file `hello.js`** on your local machine with the code above

2. **Deploy it:**
   ```bash
   cargo run --bin deployer \
     --uri "http://localhost:8080/hello" \
     --file "./hello.js"
   ```

## Step 3: Test Your Script

### Browser Test

Open your browser and visit:

```
http://localhost:8080/hello
```

You should see:

```
Hello, World! Welcome to aiwebengine.
```

### Test with Parameters

Try adding a query parameter:

```
http://localhost:8080/hello?name=Alice
```

You should see:

```
Hello, Alice! Welcome to aiwebengine.
```

### Test with curl

```bash
# Basic request
curl http://localhost:8080/hello

# With parameters
curl "http://localhost:8080/hello?name=Bob"
```

## Step 4: View the Logs

Your script is logging each request. Let's see the logs:

### Using the Editor

1. Go to `http://localhost:8080/editor`
2. Select your `hello.js` script
3. Click the "Logs" tab at the top
4. You'll see entries like:
   ```
   [2024-10-24 10:30:15] Greeting requested for: Alice
   [2024-10-24 10:30:12] Greeting requested for: World
   [2024-10-24 10:30:00] Hello script initialized successfully
   ```

### Using the Logs API

Create a simple endpoint to fetch logs programmatically:

```bash
curl "http://localhost:8080/api/logs?uri=/hello"
```

## Understanding the Request and Response

### The Request Object (`req`)

When a client makes a request to `/hello?name=Alice`, your handler receives:

```javascript
{
  method: "GET",
  path: "/hello",
  query: { name: "Alice" },
  form: {},
  headers: { /* request headers */ }
}
```

### The Response Object

Your handler must return:

```javascript
{
  status: 200,              // HTTP status code
  body: "Hello, Alice!",    // Response content
  contentType: "text/plain; charset=UTF-8" // MIME type (optional)
}
```

**Common Status Codes:**

- `200` - Success
- `201` - Created (for POST requests)
- `400` - Bad Request (invalid input)
- `404` - Not Found
- `500` - Server Error

## Step 5: Enhance Your Script

Let's add some features to make it more robust:

```javascript
function helloHandler(req) {
  const name = req.query.name;

  // Validate input
  if (!name) {
    return {
      status: 400,
      body: "Error: 'name' parameter is required",
      contentType: "text/plain; charset=UTF-8",
    };
  }

  // Sanitize input (basic example)
  if (name.length > 50) {
    return {
      status: 400,
      body: "Error: Name too long (max 50 characters)",
      contentType: "text/plain; charset=UTF-8",
    };
  }

  // Log the request
  console.log(`Greeting requested for: ${name}`);

  // Create a more detailed response
  const response = {
    greeting: `Hello, ${name}!`,
    message: "Welcome to aiwebengine",
    timestamp: new Date().toISOString(),
  };

  // Return JSON response
  return {
    status: 200,
    body: JSON.stringify(response),
    contentType: "application/json",
  };
}

function init() {
  routeRegistry.registerRoute("/hello", "helloHandler", "GET");
  console.log("Enhanced hello script initialized");
}

init();
```

Test it:

```bash
curl "http://localhost:8080/hello?name=Alice"
```

Response:

```json
{
  "greeting": "Hello, Alice!",
  "message": "Welcome to aiwebengine",
  "timestamp": "2024-10-24T10:30:15.123Z"
}
```

## Common Mistakes and Solutions

### ❌ Mistake 1: Forgetting to call `init()`

```javascript
function init() {
  routeRegistry.registerRoute("/hello", "helloHandler", "GET");
}
// Forgot to call init()!
```

**Solution:** Always call `init()` at the end of your script.

### ❌ Mistake 2: Handler name mismatch

```javascript
function helloHandler(req) {
  /* ... */
}

function init() {
  routeRegistry.registerRoute("/hello", "hello", "GET"); // Wrong name!
}
```

**Solution:** Use the exact function name as a string in `routeRegistry.registerRoute()`.

### ❌ Mistake 3: Forgetting to return a response

```javascript
function badHandler(req) {
  console.log("Processing request");
  // Forgot to return!
}
```

**Solution:** Always return a response object with `status` and `body`.

### ❌ Mistake 4: Wrong content type for JSON

```javascript
return {
  status: 200,
  body: JSON.stringify({ data: "value" }),
  contentType: "text/plain; charset=UTF-8", // Should be "application/json"!
};
```

**Solution:** Use `"application/json"` when returning JSON data.

## Next Steps

Now that you've created your first script, you can:

1. **[Learn the Web Editor](02-working-with-editor.md)** - Master the browser-based development environment
2. **[Explore the Deployment Workflow](03-deployment-workflow.md)** - Learn different ways to publish scripts
3. **[Study Script Development](../guides/scripts.md)** - Deep dive into script features
4. **[Check out Examples](../examples/index.md)** - See more complex patterns

## Quick Reference

### Essential Functions

```javascript
// Register a route
routeRegistry.registerRoute(path, handlerName, method);

// Write to logs
console.log(message);

// List all scripts
const scripts = scriptStorage.listScripts();

// List logs for current script (returns JSON string)
const logsJson = console.listLogs();
const logs = JSON.parse(logsJson);
```

### Handler Template

```javascript
function myHandler(req) {
  try {
    // Your logic here

    return {
      status: 200,
      body: "Success",
      contentType: "text/plain; charset=UTF-8",
    };
  } catch (error) {
    console.error(`Error: ${error.message}`);
    return {
      status: 500,
      body: "Internal server error",
      contentType: "text/plain; charset=UTF-8",
    };
  }
}

function init() {
  routeRegistry.registerRoute("/my-path", "myHandler", "GET");
}

init();
```

## Getting Help

- **API Reference**: [JavaScript APIs](../reference/javascript-apis.md)
- **Examples**: [Code Examples](../examples/index.md)
- **Community**: GitHub Issues

Congratulations on creating your first script! 🎉
