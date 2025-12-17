# MCP Tools Development

Learn how to create and register Model Context Protocol (MCP) tools in your scripts to extend AI capabilities.

## What are MCP Tools?

MCP (Model Context Protocol) tools allow you to expose custom functions that AI assistants like Claude, GitHub Copilot, and other MCP clients can discover and execute. This enables AI to interact with your application's APIs, access data, and perform actions on behalf of users.

## Quick Start

Here's a simple MCP tool that returns the current time:

```javascript
// Define the tool handler
function getCurrentTimeHandler(context) {
  const timezone = context.args.timezone || "UTC";
  const now = new Date();

  return JSON.stringify({
    timestamp: now.toISOString(),
    timezone: timezone,
    formatted: now.toLocaleString("en-US", { timeZone: timezone }),
  });
}

// Register the tool in init()
function init(context) {
  const schema = JSON.stringify({
    type: "object",
    properties: {
      timezone: {
        type: "string",
        description:
          "IANA timezone (e.g., 'America/New_York', 'Europe/London')",
        default: "UTC",
      },
    },
  });

  mcpRegistry.registerTool(
    "getCurrentTime",
    "Get the current date and time in a specified timezone",
    schema,
    "getCurrentTimeHandler",
  );

  return { success: true };
}
```

## MCP Registry API

### `mcpRegistry.registerTool(name, description, inputSchemaJson, handlerName)`

Registers a new MCP tool that AI clients can discover and execute.

**Parameters:**

- `name` (string) - Unique identifier for the tool (e.g., "getCurrentTime")
- `description` (string) - Human-readable description of what the tool does
- `inputSchemaJson` (string) - JSON Schema as a string defining the tool's input parameters
- `handlerName` (string) - Name of the JavaScript function that handles tool execution

**Example:**

```javascript
mcpRegistry.registerTool(
  "calculate",
  "Perform basic mathematical calculations",
  JSON.stringify({
    type: "object",
    properties: {
      operation: {
        type: "string",
        enum: ["add", "subtract", "multiply", "divide"],
        description: "Mathematical operation to perform",
      },
      a: { type: "number", description: "First operand" },
      b: { type: "number", description: "Second operand" },
    },
    required: ["operation", "a", "b"],
  }),
  "calculateHandler",
);
```

## Handler Functions

Tool handlers receive a `context` object with the following structure:

```javascript
{
  args: {
    // Tool arguments as specified in the input schema
    // Example: { timezone: "Europe/Helsinki" }
  }
}
```

**Handler Requirements:**

1. Must accept a `context` parameter
2. Must return a JSON string with the result
3. Should handle errors gracefully
4. Has access to all standard APIs (fetch, console, sharedStorage, etc.)

**Example Handler:**

```javascript
function calculateHandler(context) {
  const { operation, a, b } = context.args;

  // Validate inputs
  if (isNaN(a) || isNaN(b)) {
    return JSON.stringify({
      error: "Invalid numbers provided",
    });
  }

  // Perform calculation
  let result;
  switch (operation) {
    case "add":
      result = a + b;
      break;
    case "subtract":
      result = a - b;
      break;
    case "multiply":
      result = a * b;
      break;
    case "divide":
      if (b === 0) {
        return JSON.stringify({ error: "Cannot divide by zero" });
      }
      result = a / b;
      break;
    default:
      return JSON.stringify({ error: "Unknown operation" });
  }

  return JSON.stringify({
    operation: operation,
    a: a,
    b: b,
    result: result,
  });
}
```

## Input Schema (JSON Schema)

The input schema defines what parameters your tool accepts. It follows the [JSON Schema](https://json-schema.org/) specification.

**Common Schema Patterns:**

### Simple String Parameter

```javascript
{
  type: "object",
  properties: {
    location: {
      type: "string",
      description: "City name or location"
    }
  },
  required: ["location"]
}
```

### Enum (Limited Choices)

```javascript
{
  type: "object",
  properties: {
    format: {
      type: "string",
      enum: ["json", "xml", "csv"],
      description: "Output format"
    }
  }
}
```

### Number with Constraints

```javascript
{
  type: "object",
  properties: {
    count: {
      type: "number",
      description: "Number of items",
      minimum: 1,
      maximum: 100,
      default: 10
    }
  }
}
```

### Multiple Parameters

```javascript
{
  type: "object",
  properties: {
    query: {
      type: "string",
      description: "Search query"
    },
    limit: {
      type: "number",
      description: "Maximum results",
      default: 10
    },
    offset: {
      type: "number",
      description: "Results offset",
      default: 0
    }
  },
  required: ["query"]
}
```

## Complete Example

Here's a complete script with multiple MCP tools:

```javascript
// Weather tool handler (simulated data)
function getWeatherHandler(context) {
  const location = context.args.location || "Unknown";

  const conditions = ["Sunny", "Cloudy", "Rainy", "Snowy"];
  const randomCondition =
    conditions[Math.floor(Math.random() * conditions.length)];
  const temperature = Math.floor(Math.random() * 30) + 10;

  return JSON.stringify({
    location: location,
    condition: randomCondition,
    temperature: temperature,
    unit: "celsius",
    timestamp: new Date().toISOString(),
  });
}

// ID generator handler
function generateIdHandler(context) {
  const prefix = context.args.prefix || "id";
  const length = context.args.length || 8;

  const chars =
    "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";
  let randomPart = "";
  for (let i = 0; i < length; i++) {
    randomPart += chars.charAt(Math.floor(Math.random() * chars.length));
  }

  return JSON.stringify({
    id: prefix + "-" + randomPart,
    timestamp: Date.now(),
  });
}

// Initialize and register tools
function init(context) {
  console.log("Registering MCP tools...");

  // Register weather tool
  mcpRegistry.registerTool(
    "getWeather",
    "Get current weather information for a location (simulated data)",
    JSON.stringify({
      type: "object",
      properties: {
        location: {
          type: "string",
          description: "City name or location",
        },
      },
      required: ["location"],
    }),
    "getWeatherHandler",
  );

  // Register ID generator tool
  mcpRegistry.registerTool(
    "generateId",
    "Generate a random unique identifier with optional prefix",
    JSON.stringify({
      type: "object",
      properties: {
        prefix: {
          type: "string",
          description: "Prefix for the generated ID",
          default: "id",
        },
        length: {
          type: "number",
          description: "Length of the random part",
          default: 8,
          minimum: 4,
          maximum: 32,
        },
      },
    }),
    "generateIdHandler",
  );

  console.log("MCP tools registered successfully");

  return {
    success: true,
    tools: ["getWeather", "generateId"],
  };
}
```

## Using MCP Tools with AI Clients

### Configuring VS Code

To enable AI assistants in VS Code to use your MCP tools, create a configuration file:

**File: `.vscode/mcp.json`**

```json
{
  "servers": {
    "my-mcp-server": {
      "type": "http",
      "url": "https://yourdomain.com/mcp"
    }
  }
}
```

### Configuring Claude Desktop

Add your MCP server to Claude Desktop's configuration:

**File: `~/Library/Application Support/Claude/claude_desktop_config.json` (macOS)**

```json
{
  "mcpServers": {
    "my-server": {
      "command": "curl",
      "args": [
        "-X",
        "POST",
        "https://yourdomain.com/mcp",
        "-H",
        "Content-Type: application/json",
        "-d",
        "@-"
      ]
    }
  }
}
```

### Testing with curl

You can test your MCP tools using curl commands:

```bash
# Initialize the connection
curl -X POST https://yourdomain.com/mcp \
  -H "Content-Type: application/json" \
  -d '{
    "jsonrpc": "2.0",
    "id": 1,
    "method": "initialize",
    "params": {
      "protocolVersion": "2024-11-05",
      "capabilities": {},
      "clientInfo": {
        "name": "test-client",
        "version": "1.0.0"
      }
    }
  }'

# List available tools
curl -X POST https://yourdomain.com/mcp \
  -H "Content-Type: application/json" \
  -d '{
    "jsonrpc": "2.0",
    "id": 2,
    "method": "tools/list",
    "params": {}
  }'

# Call a tool
curl -X POST https://yourdomain.com/mcp \
  -H "Content-Type: application/json" \
  -d '{
    "jsonrpc": "2.0",
    "id": 3,
    "method": "tools/call",
    "params": {
      "name": "getWeather",
      "arguments": {
        "location": "Helsinki"
      }
    }
  }'
```

## Best Practices

### 1. Clear Tool Names

Use descriptive, action-oriented names:

✅ Good: `getCurrentTime`, `searchUsers`, `createDocument`  
❌ Bad: `time`, `users`, `doc`

### 2. Detailed Descriptions

Write clear descriptions that explain what the tool does and when to use it:

```javascript
mcpRegistry.registerTool(
  "searchProducts",
  "Search the product catalog by name, category, or SKU. Returns matching products with prices and availability.",
  schema,
  "searchProductsHandler",
);
```

### 3. Comprehensive Schemas

Provide detailed parameter descriptions and constraints:

```javascript
{
  type: "object",
  properties: {
    query: {
      type: "string",
      description: "Search query - can be product name, category, or SKU"
    },
    minPrice: {
      type: "number",
      description: "Minimum price filter in USD",
      minimum: 0
    },
    maxPrice: {
      type: "number",
      description: "Maximum price filter in USD",
      minimum: 0
    }
  },
  required: ["query"]
}
```

### 4. Error Handling

Always handle errors gracefully and return informative messages:

```javascript
function myToolHandler(context) {
  try {
    // Validate inputs
    if (!context.args.required_param) {
      return JSON.stringify({
        error: "Missing required parameter: required_param",
      });
    }

    // Process
    const result = doSomething(context.args.required_param);

    return JSON.stringify({ result: result });
  } catch (error) {
    console.error(`Tool error: ${error.message}`);
    return JSON.stringify({
      error: `Failed to execute tool: ${error.message}`,
    });
  }
}
```

### 5. Use Existing APIs

Leverage other aiwebengine features in your tools:

```javascript
function searchDataHandler(context) {
  const query = context.args.query;

  // Use fetch to call external API
  const response = fetch(
    `https://api.example.com/search?q=${encodeURIComponent(query)}`,
  );
  const data = JSON.parse(response.body);

  // Use sharedStorage to cache results
  sharedStorage.setItem(`search:${query}`, response.body);

  // Log the search
  console.log(`Search performed: ${query}`);

  return JSON.stringify({
    query: query,
    results: data.results,
    cached: true,
  });
}
```

### 6. Return Structured Data

Return well-structured JSON that's easy for AI to interpret:

```javascript
// ✅ Good - structured and clear
{
  "success": true,
  "data": {
    "user": {
      "id": 123,
      "name": "John Doe",
      "email": "john@example.com"
    }
  },
  "metadata": {
    "timestamp": "2025-12-02T15:00:00Z"
  }
}

// ❌ Bad - unstructured text
"User John Doe (ID: 123, email: john@example.com) retrieved at 2025-12-02T15:00:00Z"
```

## Security Considerations

### Authentication

MCP tools require admin-level access (ManageGraphQL capability) to register, but execution is unrestricted by default. Consider implementing your own authorization in handlers:

```javascript
function sensitiveOperationHandler(context) {
  // Check if user has permission
  const userId = context.args.userId;

  // Validate before executing
  if (!isAuthorized(userId)) {
    return JSON.stringify({
      error: "Unauthorized: User does not have permission",
    });
  }

  // Proceed with operation
  return performOperation();
}
```

### Input Validation

Always validate and sanitize inputs:

```javascript
function createUserHandler(context) {
  const { username, email } = context.args;

  // Validate username
  if (!/^[a-zA-Z0-9_-]{3,20}$/.test(username)) {
    return JSON.stringify({
      error: "Invalid username format",
    });
  }

  // Validate email
  if (!/^[^\s@]+@[^\s@]+\.[^\s@]+$/.test(email)) {
    return JSON.stringify({
      error: "Invalid email format",
    });
  }

  // Create user
  return createUser(username, email);
}
```

### Rate Limiting

Implement rate limiting for resource-intensive operations:

```javascript
const rateLimits = {};

function expensiveOperationHandler(context) {
  const clientId = context.args.clientId || "default";
  const now = Date.now();

  // Check rate limit
  if (rateLimits[clientId] && now - rateLimits[clientId] < 60000) {
    return JSON.stringify({
      error: "Rate limit exceeded. Please wait 60 seconds.",
    });
  }

  // Update rate limit
  rateLimits[clientId] = now;

  // Perform operation
  return performExpensiveOperation();
}
```

## Troubleshooting

### Tool Not Appearing

If your tool doesn't appear in the tools list:

1. Check that `init()` is being called
2. Verify the tool name doesn't contain invalid characters
3. Check console logs for registration errors
4. Ensure the schema is valid JSON

### Tool Execution Fails

If tool execution fails:

1. Check the handler name matches exactly
2. Verify the handler function is defined before `init()`
3. Look for JavaScript errors in console logs
4. Test the handler function independently

### Schema Validation Issues

If arguments aren't being passed correctly:

1. Ensure the schema matches JSON Schema specification
2. Check that property names in schema match what the handler expects
3. Verify required fields are specified
4. Test with simpler schema first

## Next Steps

- See complete example in `/scripts/example_scripts/mcp_tools_demo.js`
- Learn about [JavaScript APIs](../reference/javascript-apis.md)
- Explore [AI-Assisted Development](ai-development.md)
- Check [Script Development Guide](scripts.md)

## Additional Resources

- [MCP Specification](https://modelcontextprotocol.io/specification/2025-06-18)
- [JSON Schema Documentation](https://json-schema.org/)
- [Example MCP Tools Script](/scripts/example_scripts/mcp_tools_demo.js)
