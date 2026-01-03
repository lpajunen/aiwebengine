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
function myHandler(context) {
  const req = context.request;

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

- **Handler functions** receive a `context` object and return a response object
- **`init()` function** registers your routes when the script loads
- **`routeRegistry.registerRoute(path, handlerName, method)`** maps URLs to handler functions

## Step 2: Create Your First Script

### Option A: Using the Web Editor

1. **Open the editor:**

```text
http://localhost:8080/editor
```

1. **Click "New Script"**

1. **Enter script name:**

```text
hello.js
```

1. **Paste this code:**

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

function helloHandler(context) {
  const req = context.request;

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

1. **Click "Save"**

### Option B: Using the Deployer Tool

1. **Create a file `hello.js`** on your local machine with the code above

1. **Deploy it:**

   ```bash
   cargo run --bin deployer \
     --uri "http://localhost:8080/hello" \
     --file "./hello.js"
   ```

## Step 3: Test Your Script

### Browser Test

Open your browser and visit:

```text
http://localhost:8080/hello
```

You should see:

```text
Hello, World! Welcome to aiwebengine.
```

### Test with Parameters

Try adding a query parameter:

```text
http://localhost:8080/hello?name=Alice
```

You should see:

```text
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

```text
[2024-10-24 10:30:15] Greeting requested for: Alice
[2024-10-24 10:30:12] Greeting requested for: World
[2024-10-24 10:30:00] Hello script initialized successfully
```

### Using the Logs API

Create a simple endpoint to fetch logs programmatically:

```bash
curl "http://localhost:8080/api/logs?uri=/hello"
```

## Understanding the Context and Response

### The Handler Context (`context`)

Every handler receives a single `context` object. It always includes:

- `request`: normalized HTTP request information
- `args`: resolver or command arguments (if applicable)
- `kind`: invocation type (`httpRoute`, `graphqlQuery`, etc.)
- `scriptUri` / `handlerName`: metadata about the running script
- `meta` and `connectionMetadata`: optional maps for stream/subscription handlers

Pattern most handlers use:

```javascript
function helloHandler(context) {
  const req = context.request;
  // use req as shown below
}
```

### The Request Object (`context.request`)

When a client makes a request to `/hello?name=Alice`, `context.request` looks like:

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
function helloHandler(context) {
  const req = context.request;
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
function helloHandler(context) {
  const req = context.request;
  /* ... */
}

function init() {
  routeRegistry.registerRoute("/hello", "hello", "GET"); // Wrong name!
}
```

**Solution:** Use the exact function name as a string in `routeRegistry.registerRoute()`.

### ❌ Mistake 3: Forgetting to return a response

```javascript
function badHandler(context) {
  const req = context.request;
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
function myHandler(context) {
  const req = context.request;

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

## IDE Support with TypeScript Definitions

For better development experience with autocomplete and type checking in your IDE (VS Code, WebStorm, etc.), add this reference comment at the top of your scripts:

```javascript
/// <reference path="https://your-engine.com/api/types/v0.1.0/aiwebengine.d.ts" />
```

Replace `your-engine.com` with your actual engine URL (e.g., `localhost:8080` for local development).

### Benefits

- **Autocomplete**: Get suggestions for all available APIs as you type
- **Type checking**: Catch errors before runtime
- **Documentation**: See inline documentation for all functions and parameters
- **Better refactoring**: Safely rename variables and functions

### Example with Type Support

```javascript
/// <reference path="http://localhost:8080/api/types/v0.1.0/aiwebengine.d.ts" />

/**
 * @param {HandlerContext} context
 * @returns {HttpResponse}
 */
function helloHandler(context) {
  const req = context.request;

  // IDE now provides autocomplete for req.query, req.method, etc.
  const name = req.query.name || "World";

  // IDE knows about ResponseBuilder.text() and its parameters
  return ResponseBuilder.text(`Hello, ${name}!`);
}

function init() {
  // Autocomplete for routeRegistry methods
  routeRegistry.registerRoute("/hello", "helloHandler", "GET");
}

init();
```

### Optional: Configure jsconfig.json

For persistent type checking across all your scripts, create a `jsconfig.json` file in your scripts directory:

```json
{
  "compilerOptions": {
    "checkJs": true,
    "target": "ES2020",
    "lib": ["ES2020"]
  },
  "include": ["*.js"],
  "exclude": ["node_modules"]
}
```

Then configure VS Code to use the type definitions globally by adding to your workspace settings (`.vscode/settings.json`):

```json
{
  "js/ts.implicitProjectConfig.checkJs": true
}
```

Congratulations on creating your first script! 🎉
