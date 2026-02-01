# MCP Client - External MCP Server Integration

## Overview

The MCP Client module enables AIWebEngine scripts to connect to external Model Context Protocol (MCP) servers and use their tools. This allows integration with services like GitHub, Anthropic, and other MCP-compatible servers.

## Features

- **Protocol Support**: JSON-RPC 2.0 over HTTP/HTTPS
- **Protocol Version**: 2025-11-25 (with backward compatibility)
- **Caching**: 1-hour TTL cache for tool lists (max 5 servers with LRU eviction)
- **Security**: Secret-based authentication with zero-exposure to JavaScript
- **Error Handling**: Distinguishes between network/auth errors (exceptions) and protocol errors (error objects)

## Setup

### 1. Configure Secrets

Add your MCP server authentication tokens to `.env`:

```bash
# GitHub MCP Server
SECRET_GITHUB_TOKEN=ghp_your_personal_access_token_here
```

Get GitHub token from: https://github.com/settings/tokens
Required scopes: `repo` (for private repos) or `public_repo` (for public repos only)

### 2. Create MCP Client in JavaScript

```javascript
// Simple wrapper for easier usage
class GitHubMcpClient {
  constructor(serverUrl, secretIdentifier) {
    const clientDataJson = McpClient.constructor(serverUrl, secretIdentifier);
    this._clientData = JSON.parse(clientDataJson);
  }

  listTools() {
    const clientDataJson = JSON.stringify(this._clientData);
    const toolsJson = McpClient._listTools(clientDataJson);
    return JSON.parse(toolsJson);
  }

  callTool(toolName, args) {
    const clientDataJson = JSON.stringify(this._clientData);
    const argsJson = JSON.stringify(args);
    const resultJson = McpClient._callTool(clientDataJson, toolName, argsJson);
    const result = JSON.parse(resultJson);

    // Check for JSON-RPC errors
    if (result.error) {
      console.error(
        `MCP Tool Error [${result.error.code}]: ${result.error.message}`,
      );
      return result;
    }

    return result;
  }
}

// Initialize client
const client = new GitHubMcpClient(
  "https://api.githubcopilot.com/mcp/",
  "github_token",
);
```

## Usage

### List Available Tools

```javascript
// Discover what tools the MCP server provides
const tools = client.listTools();

console.log(`Found ${tools.length} tools`);
tools.forEach((tool) => {
  console.log(`- ${tool.name}: ${tool.description}`);
  console.log(`  Input schema:`, tool.inputSchema);
});
```

Tool objects have the structure:

```javascript
{
    name: "issue_read:get",
    description: "Read individual issues",
    inputSchema: {
        type: "object",
        properties: {
            owner: { type: "string" },
            repo: { type: "string" },
            issue_number: { type: "number" }
        },
        required: ["owner", "repo", "issue_number"]
    }
}
```

### Call a Tool

```javascript
// Fetch a GitHub issue
const result = client.callTool("issue_read:get", {
  owner: "github",
  repo: "github-mcp-server",
  issue_number: 1,
});

// Check for errors
if (result.error) {
  console.error(`Error: ${result.error.message}`);
} else {
  console.log(`Issue: ${result.title}`);
  console.log(`State: ${result.state}`);
  console.log(`Author: ${result.user.login}`);
}
```

## Error Handling

### Network/Authentication Errors (Exceptions)

These errors throw JavaScript exceptions:

- Network timeouts
- Connection failures
- 401/403 authentication errors
- Invalid server URLs

```javascript
try {
    const result = client.callTool("some_tool", {...});
} catch (error) {
    console.error("Failed to connect:", error.message);
}
```

### JSON-RPC Protocol Errors (Error Objects)

Protocol-level errors are returned as objects with an `error` field:

```javascript
const result = client.callTool("nonexistent_tool", {});

if (result.error) {
  console.error(`Code ${result.error.code}: ${result.error.message}`);
  // Example: Code -32601: Method not found
}
```

Common JSON-RPC error codes:

- `-32700`: Parse error
- `-32600`: Invalid request
- `-32601`: Method not found
- `-32602`: Invalid params
- `-32603`: Internal error

## Caching

Tool lists are cached for 1 hour to reduce external API calls:

```javascript
// First call: Makes HTTP request to MCP server
const tools1 = client.listTools();

// Subsequent calls within 1 hour: Returns cached results
const tools2 = client.listTools();

// After 1 hour: Cache expires, makes new HTTP request
```

Cache behavior:

- **TTL**: 1 hour (3600 seconds)
- **Max servers**: 5 concurrent MCP servers
- **Eviction**: LRU (Least Recently Used)
- **Per-server**: Each server URL has its own cache entry

## GitHub MCP Server Example

### Available Tools

The GitHub MCP server provides tools for:

- **Issues**: `issue_read:get`, `issue_read:get_comments`, `issue_read:list`
- **Pull Requests**: `pull_request_read:get`, `pull_request_read:get_comments`
- **Repositories**: `repo_read:get`, `repo_read:list`
- **Users**: `user_read:get`
- And many more...

### Full Example

See [scripts/examples/github_mcp_issues.js](../../scripts/examples/github_mcp_issues.js) for a complete working example.

```javascript
// List all open issues in a repository
function listOpenIssues(owner, repo) {
  const client = new GitHubMcpClient(
    "https://api.githubcopilot.com/mcp/",
    "github_token",
  );

  const result = client.callTool("issue_read:list", {
    owner: owner,
    repo: repo,
    state: "open",
  });

  if (result.error) {
    console.error("Error:", result.error.message);
    return [];
  }

  return result.issues || [];
}

const issues = listOpenIssues("github", "github-mcp-server");
console.log(`Found ${issues.length} open issues`);
```

## Supported MCP Servers

The client works with any MCP server that implements:

- JSON-RPC 2.0 over HTTP/HTTPS
- Protocol version 2025-11-25 (or compatible)
- Bearer token authentication (via `Authorization` header)

### Known Compatible Servers

1. **GitHub MCP Server**
   - URL: `https://api.githubcopilot.com/mcp/`
   - Authentication: GitHub Personal Access Token
   - Tools: Issues, PRs, Repos, Users, Actions, etc.

2. **Custom MCP Servers**
   - Any server implementing the MCP specification
   - Must support HTTP transport with Bearer token auth

## Security

### Secret Management

- Secrets are **never** exposed to JavaScript
- Secret values only exist in Rust memory
- JavaScript receives only identifiers (e.g., `"github_token"`)
- Rust injects actual values into HTTP headers at request time
- All secret access is audit-logged (identifier only, not values)

### Secret Configuration

Secrets are loaded from environment variables with `SECRET_` prefix:

```bash
# .env file
SECRET_GITHUB_TOKEN=ghp_abc123...
SECRET_ANTHROPIC_API_KEY=sk-ant-api03-...
```

Conversion: `SECRET_GITHUB_TOKEN` → identifier: `github_token`

## Limitations

Current implementation:

- ✅ Complete responses only (no streaming)
- ✅ Single tool calls (no batch operations)
- ✅ Authorization header only (Bearer token)
- ⏳ Future: Streaming support for large results
- ⏳ Future: Batch tool calls for parallel execution
- ⏳ Future: Custom headers (User-Agent, API version, etc.)

## Advanced Usage

### Custom MCP Server

```javascript
// Connect to a custom MCP server
const client = new GitHubMcpClient(
  "https://my-mcp-server.example.com/mcp",
  "my_custom_token", // References SECRET_MY_CUSTOM_TOKEN in .env
);

const tools = client.listTools();
const result = client.callTool("my_custom_tool", { arg1: "value" });
```

### Error Recovery

```javascript
function callToolWithRetry(client, toolName, args, maxRetries = 3) {
  for (let i = 0; i < maxRetries; i++) {
    try {
      const result = client.callTool(toolName, args);

      if (result.error) {
        // Protocol error - no point retrying
        return result;
      }

      return result;
    } catch (error) {
      // Network error - retry
      if (i === maxRetries - 1) throw error;
      console.log(`Retry ${i + 1}/${maxRetries}...`);
    }
  }
}
```

## Troubleshooting

### "Secret not found" error

Ensure secret is defined in `.env`:

```bash
SECRET_GITHUB_TOKEN=ghp_...
```

And restart the server to load new environment variables.

### "Authentication failed: HTTP 401"

Check that:

1. Token is valid and not expired
2. Token has required scopes (e.g., `repo` for GitHub)
3. Secret identifier matches environment variable (case-insensitive after `SECRET_` prefix)

### "Failed to list tools: Network error"

Check:

1. Server URL is correct and accessible
2. Network connectivity
3. Server is running and accepting connections

### Cache not updating

Tool list cache expires after 1 hour. To force refresh:

1. Restart the AIWebEngine server, or
2. Wait for cache TTL to expire, or
3. Connect to a different server URL (cache is per-URL)

## API Reference

### McpClient Class

#### Constructor

```javascript
new McpClient(serverUrl, secretIdentifier);
```

- `serverUrl`: String - MCP server endpoint URL
- `secretIdentifier`: String - Secret identifier (without `SECRET_` prefix)
- Returns: Client data JSON string (internal use only)

#### Methods

**listTools()**

```javascript
listTools() -> Array<Tool>
```

Returns array of available tools with schema information.

**callTool(name, arguments)**

```javascript
callTool(name, arguments) -> Object
```

- `name`: String - Tool name (e.g., "issue_read:get")
- `arguments`: Object - Tool arguments matching inputSchema
- Returns: Tool result or error object

### Tool Object Structure

```typescript
interface Tool {
  name: string;
  description?: string;
  inputSchema: JSONSchema;
}
```

### Error Object Structure

```typescript
interface ErrorResult {
  error: {
    code: number;
    message: string;
  };
}
```

## Implementation Details

### Protocol Flow

1. **Initialization** (automatic, on first request)
   - Send `initialize` JSON-RPC request
   - Negotiate protocol version
   - Exchange capabilities

2. **Tool Discovery** (cached)
   - Send `tools/list` JSON-RPC request
   - Parse tool schemas
   - Cache results for 1 hour

3. **Tool Invocation**
   - Send `tools/call` JSON-RPC request
   - Include tool name and arguments
   - Return result or error

### JSON-RPC Request Format

```json
{
  "jsonrpc": "2.0",
  "id": 1,
  "method": "tools/call",
  "params": {
    "name": "issue_read:get",
    "arguments": {
      "owner": "github",
      "repo": "github-mcp-server",
      "issue_number": 1
    }
  }
}
```

### HTTP Headers

```http
POST /mcp/ HTTP/1.1
Host: api.githubcopilot.com
Content-Type: application/json
Authorization: Bearer <token-injected-by-rust>
```

## Related Documentation

- [Model Context Protocol Specification](https://modelcontextprotocol.io/)
- [GitHub MCP Server](https://github.com/github/github-mcp-server)
- [AIWebEngine Secrets Management](./secrets.md)
- [Example Scripts](../../scripts/examples/)
