# Logging and Debugging Guide

Learn how to write, read, and use logs effectively for debugging and monitoring your aiwebengine scripts.

## Overview

Logging is essential for:

- **Debugging** - Finding and fixing issues
- **Monitoring** - Tracking script behavior
- **Auditing** - Recording important actions
- **Performance** - Identifying bottlenecks

## Writing Logs

### The `console.log()` Function

```javascript
console.log(message);
```

Writes a message to the server's log system.

**Example:**

```javascript
function myHandler(req) {
  console.log("Request received");

  const result = processRequest(req);

  console.log(`Request processed: ${result.status}`);

  return {
    status: 200,
    body: JSON.stringify(result),
    contentType: "application/json",
  };
}
```

### What to Log

**Log important events:**

```javascript
console.log("User login attempt: " + email);
console.log("Payment processed: $" + amount);
console.log("File uploaded: " + filename);
console.log("Email sent to: " + recipient);
```

**Log errors:**

```javascript
try {
  processData(data);
} catch (error) {
  console.log(`Error processing data: ${error.message}`);
}
```

**Log state changes:**

```javascript
console.log(`User ${userId} status changed from ${oldStatus} to ${newStatus}`);
console.log(`Cart updated: ${items.length} items, total $${total}`);
```

**Log performance metrics:**

```javascript
const start = Date.now();
const result = expensiveOperation();
const duration = Date.now() - start;
console.log(`Operation completed in ${duration}ms`);
```

### What NOT to Log

**Avoid logging:**

- Sensitive data (passwords, tokens, credit cards)
- Personal information (unless necessary)
- Excessive noise (every minor operation)
- Large data payloads

**Bad examples:**

```javascript
// DON'T log passwords
console.log(`User logged in with password: ${password}`); // ❌

// DON'T log tokens
console.log(`API token: ${apiToken}`); // ❌

// DON'T log full request objects
console.log(JSON.stringify(req)); // ❌ Too much data
```

### Structured Logging

Use consistent formats for easier parsing:

```javascript
function logEvent(event, data) {
  const timestamp = new Date().toISOString();
  const message = `[${event}] ${JSON.stringify(data)}`;
  console.log(message);
}

// Usage
logEvent("USER_LOGIN", { email: "user@example.com", success: true });
logEvent("API_CALL", { endpoint: "/api/users", duration: 234 });
logEvent("ERROR", { function: "createUser", error: "Email exists" });
```

## Reading Logs

### Method 1: Web Editor

The easiest way to view logs:

1. Open `/editor`
2. Click "Logs" tab
3. Select your script from dropdown
4. Logs auto-refresh every 5 seconds

**Features:**

- Real-time updates
- Filter by script
- Jump to latest button (scrolls view to newest entry)
- Timestamps included

### Method 2: `listLogs()` Function

Retrieve logs programmatically in your scripts:

```javascript
function logsHandler(req) {
  // Get logs for current script
  const logs = listLogs();

  return {
    status: 200,
    body: JSON.stringify({ logs: logs }),
    contentType: "application/json",
  };
}

routeRegistry.registerRoute("/my-logs", "logsHandler", "GET");
```

### Method 3: `listLogsForUri()` Function

Get logs for a specific script URI:

```javascript
function allLogsHandler(req) {
  const uri = req.query.uri; // e.g., "/api/users"

  if (!uri) {
    return {
      status: 400,
      body: JSON.stringify({ error: "URI parameter required" }),
      contentType: "application/json",
    };
  }

  const logs = listLogsForUri(uri);

  return {
    status: 200,
    body: JSON.stringify({ uri: uri, logs: logs }),
    contentType: "application/json",
  };
}

routeRegistry.registerRoute("/logs", "allLogsHandler", "GET");
// Usage: /logs?uri=/api/users
```

### Method 4: Server Logs

Check server console or log files:

```bash
# If running with cargo
cargo run
# Logs appear in console

# If running as service
journalctl -u aiwebengine -f

# Log files (if configured)
tail -f /var/log/aiwebengine/server.log
```

## Log Viewer Scripts

### Basic Log Viewer

```javascript
function logViewerHandler(req) {
  const logs = listLogs();

  const logItems = logs
    .map((log) => {
      return `<li><code>${log}</code></li>`;
    })
    .join("");

  const html = `
    <!DOCTYPE html>
    <html>
    <head>
      <title>Logs</title>
      <style>
        body { font-family: monospace; padding: 20px; }
        ul { list-style: none; padding: 0; }
        li { padding: 5px; border-bottom: 1px solid #eee; }
        code { color: #333; }
      </style>
    </head>
    <body>
      <h1>Script Logs</h1>
      <ul>${logItems}</ul>
    </body>
    </html>
  `;

  return {
    status: 200,
    body: html,
    contentType: "text/html",
  };
}

routeRegistry.registerRoute("/logs-viewer", "logViewerHandler", "GET");
```

### Advanced Log Viewer with Filtering

```javascript
function advancedLogViewerHandler(req) {
  const filter = req.query.filter || "";
  const level = req.query.level || "all";

  const logs = listLogs();

  // Filter logs
  const filteredLogs = logs.filter((log) => {
    if (filter && !log.toLowerCase().includes(filter.toLowerCase())) {
      return false;
    }

    if (level !== "all") {
      if (level === "error" && !log.toLowerCase().includes("error")) {
        return false;
      }
      if (level === "warning" && !log.toLowerCase().includes("warning")) {
        return false;
      }
    }

    return true;
  });

  const html = `
    <!DOCTYPE html>
    <html>
    <head>
      <title>Log Viewer</title>
      <style>
        body { font-family: Arial, sans-serif; padding: 20px; }
        .filters { margin-bottom: 20px; padding: 15px; background: #f5f5f5; border-radius: 5px; }
        .log-entry { padding: 8px; border-bottom: 1px solid #eee; font-family: monospace; font-size: 14px; }
        .log-entry:hover { background: #f9f9f9; }
        .error { color: #d32f2f; }
        .warning { color: #f57c00; }
        .info { color: #1976d2; }
      </style>
    </head>
    <body>
      <h1>Log Viewer</h1>
      
      <div class="filters">
        <form method="GET">
          <label>
            Filter: 
            <input type="text" name="filter" value="${filter}" placeholder="Search logs...">
          </label>
          
          <label>
            Level:
            <select name="level">
              <option value="all" ${level === "all" ? "selected" : ""}>All</option>
              <option value="error" ${level === "error" ? "selected" : ""}>Errors</option>
              <option value="warning" ${level === "warning" ? "selected" : ""}>Warnings</option>
              <option value="info" ${level === "info" ? "selected" : ""}>Info</option>
            </select>
          </label>
          
          <button type="submit">Filter</button>
        </form>
      </div>
      
      <div>
        <p><strong>${filteredLogs.length}</strong> log entries</p>
        ${filteredLogs
          .map((log) => {
            let className = "log-entry";
            if (log.toLowerCase().includes("error")) className += " error";
            else if (log.toLowerCase().includes("warning"))
              className += " warning";
            else if (log.toLowerCase().includes("info")) className += " info";

            return `<div class="${className}">${log}</div>`;
          })
          .join("")}
      </div>
    </body>
    </html>
  `;

  return {
    status: 200,
    body: html,
    contentType: "text/html",
  };
}

routeRegistry.registerRoute(
  "/advanced-logs",
  "advancedLogViewerHandler",
  "GET",
);
```

## Debugging Techniques

### 1. Trace Execution Flow

Add logs at key points:

```javascript
function complexHandler(req) {
  console.log("complexHandler: Start");

  console.log("complexHandler: Validating input");
  if (!validateInput(req)) {
    console.log("complexHandler: Validation failed");
    return errorResponse(400, "Invalid input");
  }

  console.log("complexHandler: Processing data");
  const result = processData(req.form);

  console.log(`complexHandler: Processing complete, result: ${result.status}`);

  console.log("complexHandler: Saving to database");
  saveToDatabase(result);

  console.log("complexHandler: End");
  return jsonResponse(200, result);
}
```

### 2. Log Variable Values

Inspect data at runtime:

```javascript
function debugHandler(req) {
  console.log(`Received query: ${JSON.stringify(req.query)}`);
  console.log(`Received form: ${JSON.stringify(req.form)}`);

  const processedData = transformData(req.form);
  console.log(`Processed data: ${JSON.stringify(processedData)}`);

  return jsonResponse(200, processedData);
}
```

### 3. Conditional Logging

Log only when needed:

```javascript
const DEBUG = true;

function debugLog(message) {
  if (DEBUG) {
    console.log(`[DEBUG] ${message}`);
  }
}

function myHandler(req) {
  debugLog("Handler called with path: " + req.path);

  // Your logic

  debugLog("Handler completed");
  return jsonResponse(200, { success: true });
}
```

### 4. Error Context

Provide context when logging errors:

```javascript
function createUserHandler(req) {
  try {
    const user = createUser(req.form);
    console.log(`User created successfully: ${user.email}`);
    return jsonResponse(201, { user: user });
  } catch (error) {
    console.log(`ERROR in createUserHandler: ${error.message}`);
    console.log(`  Input data: ${JSON.stringify(req.form)}`);
    console.log(`  Stack: ${error.stack || "No stack trace"}`);
    return errorResponse(500, "Failed to create user");
  }
}
```

### 5. Performance Logging

Measure execution time:

```javascript
function timedOperation(name, operation) {
  const start = Date.now();
  console.log(`${name}: Starting`);

  try {
    const result = operation();
    const duration = Date.now() - start;
    console.log(`${name}: Completed in ${duration}ms`);
    return result;
  } catch (error) {
    const duration = Date.now() - start;
    console.log(`${name}: Failed after ${duration}ms - ${error.message}`);
    throw error;
  }
}

function slowHandler(req) {
  return timedOperation("slowHandler", () => {
    // Your slow operation
    const result = expensiveComputation();
    return jsonResponse(200, result);
  });
}
```

## Log Patterns

### Request/Response Logging

```javascript
function loggedHandler(req) {
  const requestId = generateRequestId();

  console.log(`[${requestId}] Request: ${req.method} ${req.path}`);
  console.log(`[${requestId}] Query: ${JSON.stringify(req.query)}`);

  try {
    const response = processRequest(req);
    console.log(`[${requestId}] Response: ${response.status}`);
    return response;
  } catch (error) {
    console.log(`[${requestId}] Error: ${error.message}`);
    throw error;
  }
}
```

### Audit Logging

```javascript
function auditLog(action, user, details) {
  const entry = {
    timestamp: new Date().toISOString(),
    action: action,
    user: user,
    details: details,
  };
  console.log(`[AUDIT] ${JSON.stringify(entry)}`);
}

function deleteUserHandler(req) {
  const userId = req.query.id;
  const currentUser = getCurrentUser(req);

  auditLog("DELETE_USER", currentUser, { userId: userId });

  deleteUser(userId);

  return jsonResponse(200, { message: "User deleted" });
}
```

### Metric Collection

```javascript
const metrics = {
  requests: 0,
  errors: 0,
  totalDuration: 0,
};

function metricsHandler(req) {
  const start = Date.now();
  metrics.requests++;

  try {
    const result = processRequest(req);
    const duration = Date.now() - start;
    metrics.totalDuration += duration;

    console.log(`[METRICS] Request completed in ${duration}ms`);

    return result;
  } catch (error) {
    metrics.errors++;
    console.log(`[METRICS] Request failed: ${error.message}`);
    throw error;
  }
}

function statsHandler(req) {
  const avgDuration =
    metrics.requests > 0 ? metrics.totalDuration / metrics.requests : 0;

  const stats = {
    totalRequests: metrics.requests,
    totalErrors: metrics.errors,
    averageDuration: Math.round(avgDuration),
    errorRate:
      metrics.requests > 0
        ? ((metrics.errors / metrics.requests) * 100).toFixed(2) + "%"
        : "0%",
  };

  return jsonResponse(200, stats);
}

routeRegistry.registerRoute("/stats", "statsHandler", "GET");
```

## Best Practices

### 1. Use Consistent Formats

```javascript
// Good - consistent format
console.log("USER_LOGIN: user@example.com - SUCCESS");
console.log("USER_LOGOUT: user@example.com - SUCCESS");
console.log("USER_LOGIN: user@example.com - FAILED: Invalid password");

// Bad - inconsistent
console.log("User logged in: user@example.com");
console.log("Logout successful for user@example.com");
console.log("Login error: Invalid password");
```

### 2. Include Context

```javascript
// Good - includes context
console.log(
  `createOrder: User ${userId} ordered ${items.length} items, total $${total}`,
);

// Bad - lacks context
console.log("Order created");
```

### 3. Log Levels (Manual)

Implement log levels yourself:

```javascript
function logError(message) {
  console.log(`[ERROR] ${message}`);
}

function logWarning(message) {
  console.log(`[WARNING] ${message}`);
}

function logInfo(message) {
  console.log(`[INFO] ${message}`);
}

function logDebug(message) {
  if (DEBUG_MODE) {
    console.log(`[DEBUG] ${message}`);
  }
}

// Usage
logInfo("Server started");
logWarning("Cache miss for key: users_list");
logError("Database connection failed");
logDebug("Variable value: " + someVar);
```

### 4. Don't Log in Loops (Usually)

```javascript
// Bad - too many logs
for (let i = 0; i < 1000; i++) {
  console.log(`Processing item ${i}`); // ❌ 1000 log entries!
  processItem(items[i]);
}

// Good - log summary
console.log(`Processing ${items.length} items`);
for (let i = 0; i < items.length; i++) {
  processItem(items[i]);
}
console.log(`Processed all items successfully`);

// Also good - log milestones
for (let i = 0; i < 1000; i++) {
  if (i % 100 === 0) {
    console.log(`Processed ${i}/1000 items`);
  }
  processItem(items[i]);
}
```

### 5. Sanitize Sensitive Data

```javascript
function sanitizeEmail(email) {
  const [user, domain] = email.split("@");
  return `${user.substring(0, 2)}***@${domain}`;
}

function sanitizeCardNumber(card) {
  return `****-****-****-${card.slice(-4)}`;
}

// Usage
console.log(`Payment processed for ${sanitizeEmail(email)}`);
console.log(`Card ending in ${sanitizeCardNumber(cardNumber)}`);
```

## Troubleshooting

### Logs Not Appearing

**Check:**

- `console.log()` is actually being called
- Script executed successfully (no errors before log call)
- Correct script selected in log viewer
- Logs viewer refreshed

### Too Many Logs

**Solutions:**

- Add log levels and filter
- Remove debug logs from production
- Log summaries instead of details
- Use sampling (log every Nth request)

### Can't Find Specific Logs

**Solutions:**

- Use structured logging with searchable formats
- Include unique identifiers (request IDs)
- Use the log viewer with filters
- Add more context to log messages

## Next Steps

- **[Script Development](scripts.md)** - Learn more about handlers
- **[AI Development](ai-development.md)** - Use AI for debugging
- **[Examples](../examples/index.md)** - See logging in practice
- **[API Reference](../reference/javascript-apis.md)** - Complete API docs

## Quick Reference

```javascript
// Write a log message
console.log("Message");

// Get logs for current script
const logs = JSON.parse(console.listLogs());

// Get logs for specific script URI
const logs = JSON.parse(console.listLogsForUri("/api/users"));

// Log helper functions
function logError(msg) {
  console.log(`[ERROR] ${msg}`);
}
function logInfo(msg) {
  console.log(`[INFO] ${msg}`);
}
```
