# Model Context Protocol (MCP) Implementation Summary

## Overview

The aiwebengine now supports the Model Context Protocol (MCP), allowing JavaScript scripts to register **tools** (executable actions) and **prompts** (reusable templates) that can be discovered and used by AI clients. The implementation follows the same pattern as the GraphQL registry, providing a clean and consistent API.

## Architecture

### Rust Backend

1. **`src/mcp.rs`** - Core MCP module
   - `McpTool` struct: Represents a tool registration with name, description, input schema, handler function, and script URI
   - `McpPrompt` struct: Represents a prompt registration with name, description, arguments, and script URI
   - `PromptArgument` struct: Defines prompt argument with name, description, and required flag
   - `McpRegistry`: Thread-safe registry for storing registered tools and prompts
   - **Tool functions:**
     - `register_mcp_tool()`: Registers a new MCP tool
     - `list_tools()`: Returns all registered tools
     - `execute_mcp_tool()`: Executes a tool by calling its JavaScript handler
   - **Prompt functions:**
     - `register_mcp_prompt()`: Registers a new MCP prompt
     - `list_prompts()`: Returns all registered prompts
     - `get_prompt()`: Executes prompt handler to generate messages
   - `clear_script_mcp_registrations()`: Cleans up tools and prompts when scripts are updated

2. **`src/lib.rs`** - HTTP endpoints
   - `POST /mcp` - Single endpoint handling JSON-RPC 2.0 requests
   - Supports methods:
     - `initialize` - Capability negotiation
     - `notifications/initialized` - Client ready notification
     - `tools/list` - List available tools
     - `tools/call` - Execute a tool
     - `prompts/list` - List available prompts
     - `prompts/get` - Get prompt messages with arguments
   - Publicly accessible (no authentication required by default)

3. **`src/js_engine.rs`** - JavaScript execution
   - `execute_mcp_tool_handler()`: Executes JavaScript tool handlers with proper context
   - New `HandlerInvocationKind::McpTool` variant for MCP tool execution tracking

4. **`src/security/secure_globals.rs`** - JavaScript API
   - `setup_mcp_functions()`: Sets up the `mcpRegistry` object in JavaScript context
   - `registerTool()`: JavaScript function for registering MCP tools
   - `registerPrompt()`: JavaScript function for registering MCP prompts
   - Security validation and audit logging for tool and prompt registration

### JavaScript API

Scripts can register MCP tools and prompts using the `mcpRegistry` global object:

#### Tools

```javascript
mcpRegistry.registerTool(name, description, inputSchemaJson, handlerName);
```

**Parameters:**

- `name` (string): Unique tool identifier
- `description` (string): Human-readable description of what the tool does
- `inputSchemaJson` (string): JSON Schema as a string defining the tool's input parameters
- `handlerName` (string): Name of the JavaScript function that handles tool execution

**Handler Function:**

- Receives `context` object with `context.args` containing the tool arguments
- Should return a JSON string with the tool result
- Has access to all standard APIs (console, sharedStorage, fetch, etc.)

#### Prompts

```javascript
mcpRegistry.registerPrompt(name, description, argumentsJson);
```

**Parameters:**

- `name` (string): Unique prompt identifier
- `description` (string): Human-readable description of what the prompt generates
- `argumentsJson` (string): JSON array of argument definitions with name, description, and required flag

**Handler Function:**

- Named the same as the prompt name
- Receives object with user-provided arguments
- Should return object with `messages` array containing role/content pairs
- Has access to all standard APIs

## Example Usage

### Tools Example

See `/scripts/example_scripts/mcp_tools_demo.js` for a complete example with multiple tools:

```javascript
// Register a tool
function getCurrentTimeHandler(context) {
  const timezone = context.args.timezone || "UTC";
  return JSON.stringify({
    timestamp: new Date().toISOString(),
    timezone: timezone,
  });
}

function init(context) {
  const schema = JSON.stringify({
    type: "object",
    properties: {
      timezone: {
        type: "string",
        description: "IANA timezone",
        default: "UTC",
      },
    },
  });

  mcpRegistry.registerTool(
    "getCurrentTime",
    "Get current date and time in a timezone",
    schema,
    "getCurrentTimeHandler",
  );

  return { success: true };
}
```

### Prompts Example

See `/scripts/example_scripts/mcp_prompts_demo.js` for a complete example with multiple prompts:

```javascript
// Register a prompt
function init(context) {
  mcpRegistry.registerPrompt(
    "create_rest_endpoint",
    "Generate a complete REST API endpoint with handler and route",
    JSON.stringify([
      {
        name: "resourceName",
        description: "The resource name (e.g., 'users', 'products')",
        required: true,
      },
      {
        name: "method",
        description: "HTTP method (GET, POST, PUT, DELETE)",
        required: true,
      },
    ]),
  );

  return { success: true };
}

// Prompt handler function (same name as prompt)
function create_rest_endpoint(args) {
  const { resourceName, method } = args;

  const code = `
function handle${resourceName}${method}(req) {
  // TODO: Implement ${resourceName} ${method} logic
  return { success: true, data: {} };
}

endpoints.register("${method} /api/${resourceName}", handle${resourceName}${method});
  `.trim();

  return {
    messages: [
      {
        role: "user",
        content: {
          type: "text",
          text: `Create a ${method} endpoint for ${resourceName}`,
        },
      },
      {
        role: "assistant",
        content: {
          type: "text",
          text: code,
        },
      },
    ],
  };
}
```

## MCP Protocol Endpoints

The MCP implementation follows the [Model Context Protocol specification](https://modelcontextprotocol.io/specification/2025-06-18) using JSON-RPC 2.0.

### Single Endpoint

All MCP operations use a single endpoint:

```http
POST /mcp
Content-Type: application/json
```

### Initialize (Required First Step)

Before using any other MCP methods, clients must initialize the connection to negotiate protocol version and capabilities.

**JSON-RPC Request:**

```json
{
  "jsonrpc": "2.0",
  "id": 1,
  "method": "initialize",
  "params": {
    "protocolVersion": "2024-11-05",
    "capabilities": {},
    "clientInfo": {
      "name": "ExampleClient",
      "version": "1.0.0"
    }
  }
}
```

**JSON-RPC Response:**

```json
{
  "jsonrpc": "2.0",
  "id": 1,
  "result": {
    "protocolVersion": "2024-11-05",
    "capabilities": {
      "tools": {
        "listChanged": true
      },
      "prompts": {
        "listChanged": true
      }
    },
    "serverInfo": {
      "name": "aiwebengine",
      "version": "0.1.0"
    }
  }
}
```

After receiving the initialize response, the client should send an `initialized` notification:

**JSON-RPC Notification:**

```json
{
  "jsonrpc": "2.0",
  "method": "notifications/initialized"
}
```

### List Tools

**JSON-RPC Request:**

```json
{
  "jsonrpc": "2.0",
  "id": 1,
  "method": "tools/list",
  "params": {}
}
```

**JSON-RPC Response:**

```json
{
  "jsonrpc": "2.0",
  "id": 1,
  "result": {
    "tools": [
      {
        "name": "getCurrentTime",
        "description": "Get current date and time in a timezone",
        "inputSchema": {
          "type": "object",
          "properties": {
            "timezone": {
              "type": "string",
              "description": "IANA timezone",
              "default": "UTC"
            }
          }
        }
      }
    ]
  }
}
```

### Call Tool

**JSON-RPC Request:**

```json
{
  "jsonrpc": "2.0",
  "id": 2,
  "method": "tools/call",
  "params": {
    "name": "getCurrentTime",
    "arguments": {
      "timezone": "America/New_York"
    }
  }
}
```

**JSON-RPC Response (Success):**

```json
{
  "jsonrpc": "2.0",
  "id": 2,
  "result": {
    "content": [
      {
        "type": "text",
        "text": "{\"timestamp\":\"2025-12-02T12:00:00.000Z\",\"timezone\":\"America/New_York\"}"
      }
    ],
    "isError": false
  }
}
```

**JSON-RPC Response (Tool Execution Error):**

```json
{
  "jsonrpc": "2.0",
  "id": 3,
  "result": {
    "content": [
      {
        "type": "text",
        "text": "Tool execution failed: API rate limit exceeded"
      }
    ],
    "isError": true
  }
}
```

**JSON-RPC Response (Protocol Error):**

```json
{
  "jsonrpc": "2.0",
  "id": 4,
  "error": {
    "code": -32602,
    "message": "Unknown tool: invalidToolName"
  }
}
```

### Error Codes

The implementation uses standard JSON-RPC 2.0 error codes:

- `-32700`: Parse error (invalid JSON)
- `-32600`: Invalid Request (invalid JSON-RPC)
- `-32601`: Method not found
- `-32602`: Invalid params (invalid tool/prompt name or parameters)

Note: Tool execution errors are reported in the result with `isError: true` rather than as protocol errors.

### List Prompts

**JSON-RPC Request:**

```json
{
  "jsonrpc": "2.0",
  "id": 5,
  "method": "prompts/list",
  "params": {}
}
```

**JSON-RPC Response:**

```json
{
  "jsonrpc": "2.0",
  "id": 5,
  "result": {
    "prompts": [
      {
        "name": "create_rest_endpoint",
        "description": "Generate a complete REST API endpoint with handler and route",
        "arguments": [
          {
            "name": "resourceName",
            "description": "The resource name (e.g., 'users', 'products')",
            "required": true
          },
          {
            "name": "method",
            "description": "HTTP method (GET, POST, PUT, DELETE)",
            "required": true
          }
        ]
      }
    ]
  }
}
```

### Get Prompt

**JSON-RPC Request:**

```json
{
  "jsonrpc": "2.0",
  "id": 6,
  "method": "prompts/get",
  "params": {
    "name": "create_rest_endpoint",
    "arguments": {
      "resourceName": "products",
      "method": "GET"
    }
  }
}
```

**JSON-RPC Response:**

```json
{
  "jsonrpc": "2.0",
  "id": 6,
  "result": {
    "messages": [
      {
        "role": "user",
        "content": {
          "type": "text",
          "text": "Create a GET endpoint for products"
        }
      },
      {
        "role": "assistant",
        "content": {
          "type": "text",
          "text": "function handleProductsGET(req) {\n  // TODO: Implement products GET logic\n  return { success: true, data: {} };\n}\n\nendpoints.register(\"GET /api/products\", handleProductsGET);"
        }
      }
    ]
  }
}
```

## Security

- Tool and prompt registration requires the `ManageGraphQL` capability (admin-level access)
- Tool execution runs with admin privileges (similar to GraphQL resolvers)
- Input validation ensures names, descriptions, and schemas are within safe limits
- No dangerous patterns (like `__proto__` or `constructor`) allowed in schemas or arguments
- All operations are logged for security auditing

## Integration with Existing Systems

The MCP implementation integrates seamlessly with existing aiwebengine features:

1. **Script Lifecycle**: Tools and prompts are automatically cleared when scripts are updated
2. **Security Model**: Uses the same capability-based security as GraphQL
3. **Audit Logging**: All tool and prompt registrations and executions are logged
4. **Error Handling**: Consistent error handling and reporting
5. **Context**: Tool and prompt handlers receive the standard `context` object with all APIs

## Future Enhancements

Potential improvements for the MCP implementation:

1. **Completions**: Add MCP completions capability for autocomplete suggestions
2. **Authentication**: Add optional authentication for tool execution
3. **Rate Limiting**: Implement rate limiting for tool calls
4. **Streaming**: Support streaming responses for long-running tools
5. **Tool Discovery**: Add metadata like tags, categories, and examples
6. **Input Validation**: Automatic validation of arguments against JSON Schema
7. **MCP SSE**: Support Server-Sent Events for real-time tool notifications
8. **Versioning**: Support multiple versions of the same tool or prompt
9. **Performance Metrics**: Track execution times and success rates

## Testing

To test the MCP implementation:

1. Load the demo script:

   ```http
   POST /api/scripts/https://example.com/mcp_demo
   Content-Type: text/plain

   [paste mcp_tools_demo.js content]
   ```

2. Initialize the MCP connection:

   ```json
   {
     "jsonrpc": "2.0",
     "id": 1,
     "method": "initialize",
     "params": {
       "protocolVersion": "2024-11-05",
       "capabilities": {},
       "clientInfo": {
         "name": "TestClient",
         "version": "1.0.0"
       }
     }
   }
   ```

3. Send initialized notification:

   ```json
   {
     "jsonrpc": "2.0",
     "method": "notifications/initialized"
   }
   ```

4. List available tools:

   ```json
   {
     "jsonrpc": "2.0",
     "id": 2,
     "method": "tools/list",
     "params": {}
   }
   ```

5. Execute a tool:

   ```json
   {
     "jsonrpc": "2.0",
     "id": 3,
     "method": "tools/call",
     "params": {
       "name": "calculate",
       "arguments": {
         "operation": "add",
         "a": 5,
         "b": 3
       }
     }
   }
   ```

## Summary

The MCP implementation provides a clean, consistent API for registering and executing **tools** (actions) and **prompts** (templates) from JavaScript scripts. It follows the established patterns in aiwebengine (similar to GraphQL), integrates with the security model, and provides a foundation for AI clients to discover and use custom functionality defined in JavaScript.

**Key Features:**

- **Tools**: Executable actions for reading files, processing data, and performing operations
- **Prompts**: Reusable templates for code generation and workflow guidance
- **JSON-RPC 2.0**: Standard protocol with initialize, tools/_, and prompts/_ methods
- **Security**: Capability-based access control and audit logging
- **Integration**: Seamless integration with existing script lifecycle and APIs
