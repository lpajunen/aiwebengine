# Model Context Protocol (MCP) Implementation Summary

## Overview

The aiwebengine now supports the Model Context Protocol (MCP), allowing JavaScript scripts to register tools that can be discovered and executed by AI clients. The implementation follows the same pattern as the GraphQL registry, providing a clean and consistent API.

## Architecture

### Rust Backend

1. **`src/mcp.rs`** - Core MCP module
   - `McpTool` struct: Represents a tool registration with name, description, input schema, handler function, and script URI
   - `McpRegistry`: Thread-safe registry for storing registered tools
   - `register_mcp_tool()`: Registers a new MCP tool
   - `list_tools()`: Returns all registered tools
   - `execute_mcp_tool()`: Executes a tool by calling its JavaScript handler
   - `clear_script_mcp_registrations()`: Cleans up tools when scripts are updated

2. **`src/lib.rs`** - HTTP endpoints
   - `POST /mcp` - Single endpoint handling JSON-RPC 2.0 requests
   - Supports `tools/list` and `tools/call` methods
   - Publicly accessible (no authentication required by default)

3. **`src/js_engine.rs`** - JavaScript execution
   - `execute_mcp_tool_handler()`: Executes JavaScript tool handlers with proper context
   - New `HandlerInvocationKind::McpTool` variant for MCP tool execution tracking

4. **`src/security/secure_globals.rs`** - JavaScript API
   - `setup_mcp_functions()`: Sets up the `mcpRegistry` object in JavaScript context
   - `registerTool()`: JavaScript function for registering MCP tools
   - Security validation and audit logging for tool registration

### JavaScript API

Scripts can register MCP tools using the `mcpRegistry` global object:

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

## Example Usage

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
- `-32602`: Invalid params (invalid tool name or parameters)

Note: Tool execution errors are reported in the result with `isError: true` rather than as protocol errors.

## Security

- Tool registration requires the `ManageGraphQL` capability (admin-level access)
- Tool execution runs with admin privileges (similar to GraphQL resolvers)
- Input validation ensures tool names and schemas are within safe limits
- No dangerous patterns (like `__proto__` or `constructor`) allowed in schemas
- All operations are logged for security auditing

## Integration with Existing Systems

The MCP implementation integrates seamlessly with existing aiwebengine features:

1. **Script Lifecycle**: Tools are automatically cleared when scripts are updated
2. **Security Model**: Uses the same capability-based security as GraphQL
3. **Audit Logging**: All tool registrations and executions are logged
4. **Error Handling**: Consistent error handling and reporting
5. **Context**: Tool handlers receive the standard `context` object with all APIs

## Future Enhancements

Potential improvements for the MCP implementation:

1. **Authentication**: Add optional authentication for tool execution
2. **Rate Limiting**: Implement rate limiting for tool calls
3. **Streaming**: Support streaming responses for long-running tools
4. **Tool Discovery**: Add metadata like tags, categories, and examples
5. **Input Validation**: Automatic validation of arguments against JSON Schema
6. **MCP SSE**: Support Server-Sent Events for real-time tool notifications
7. **Tool Versioning**: Support multiple versions of the same tool
8. **Performance Metrics**: Track tool execution times and success rates

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

The MCP implementation provides a clean, consistent API for registering and executing tools from JavaScript scripts. It follows the established patterns in aiwebengine (similar to GraphQL), integrates with the security model, and provides a foundation for AI clients to discover and use custom tools defined in JavaScript.
