# MCP Client - External MCP Server Integration

## Overview

The MCP Client module enables AIWebEngine scripts to connect to external Model Context Protocol (MCP) servers and use their tools. This allows integration with services like GitHub, Anthropic, and other MCP-compatible servers.

## Features

- **Protocol Support**: JSON-RPC 2.0 over HTTP/HTTPS
- **Protocol Era**: `2026-07-28` where the server says it speaks it, the
  `initialize` handshake where it does not — learned per server by one
  `server/discover` probe, not assumed
- **Caching**: the server's own `ttlMs` where it offers one, else 1 hour; max 5
  servers with LRU eviction
- **Security**: Secret-based authentication with zero-exposure to JavaScript
- **Capabilities**: every call requires both `use_network` and `read_secrets`
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

### Which servers can be reached

A server URL is validated the same way a URL passed to `fetch` is, and for the
same reason: the URL comes from the script, so a client that would contact any
address it names is a way to reach whatever network the engine runs in. The
constructor throws for

- anything that is not `http` or `https`,
- `localhost`, loopback (`127.0.0.1`, `::1`), private ranges (`10/8`,
  `172.16/12`, `192.168/16`), carrier-grade NAT, and link-local addresses —
  `169.254.169.254`, the cloud metadata endpoint, among them,
- a public hostname that resolves to any of those.

Redirects are followed one hop at a time with the same check applied to each,
and the `Authorization` header is dropped when a redirect changes host.

This includes the engine's own `/mcp`: a script reaching the engine it runs in
does so through the engine's public host, with a token whose audience names it,
exactly as any other client would.

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

- **TTL**: the `ttlMs` the server's `tools/list` result asked for, clamped to at
  most 1 hour; 1 hour when the server named none. Every revision before
  `2026-07-28` names none, so that is the common case — but the engine's own
  `/mcp` publishes 60 seconds, and a client that ignored it would take an hour
  to notice a newly deployed script's tools.
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

See [github_mcp_issues.js](https://github.com/lpajunen/aiwebengine-examples/blob/main/src/github_mcp_issues/github_mcp_issues.js) in the aiwebengine-examples repo for a complete working example.

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
- `2026-07-28`, or any revision that answers `initialize` (2025-11-25 and
  earlier). A dual-era server is served under the modern rules.
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

### Capabilities

Every arm of `McpClient` — `constructor`, `_listTools` and `_callTool` —
requires **both** `use_network` and `read_secrets`. A call to an MCP server is
unconditionally both: an outbound request to the URL you name, carrying the
secret you name as a `Bearer` token. There is no unauthenticated arm, which is
why `read_secrets` is required up front rather than only when a secret is
mentioned, the way it is for `fetch`.

This matters most for [capability attenuation](CAPABILITY_ATTENUATION.md): a
`sandbox.run` narrowed out of either name cannot mount an MCP server. Secrets
being invisible to JavaScript (below) is what makes the client safe to hand a
script; it is not what decides whether _this_ execution should be holding one.
A credential resolved host-side by name would otherwise be a way to obtain,
through a secret, exactly the authority a narrowing had just refused.

The check is on the methods and not only on the constructor, because
`constructor` returns a plain JSON blob and the two methods rebuild the client
from whatever blob they are handed.

### Secret Management

- Secrets are **never** exposed to JavaScript
- Secret values only exist in Rust memory
- JavaScript receives only identifiers (e.g., `"github_token"`)
- Rust injects actual values into HTTP headers at request time
- All secret access is audit-logged (identifier only, not values)

### Secret Configuration

Secrets are stored in the database and are **never** exposed to JavaScript code. An administrator or script owner sets script-level secrets over the engine API; users can also set their own from JavaScript.

```bash
# Admin or script owner: set a secret for a specific script
curl -X POST "https://your-engine.com/engine/secrets?script=https://example.com/my-script" \
  -H "Content-Type: application/json" \
  -d '{"key": "github_token", "value": "ghp_abc123..."}'
```

```javascript
// Per-user: set a secret scoped to the current user
secretStorage.setSecret("github_token", "ghp_abc123...");
```

At request time, Rust resolves the identifier against the database (user secret takes priority over script secret) and injects the value into the HTTP header — JavaScript never sees the value.

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
  "my_custom_token", // Secret identifier stored in the database
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

Ensure the secret has been stored in the database:

```bash
# As an admin or owner of the script: store a script-level secret
curl -X POST "https://your-engine.com/engine/secrets?script=https://your-script-uri" \
  -H "Content-Type: application/json" \
  -d '{"key": "github_token", "value": "ghp_..."}'
```

```javascript
// Or each user stores their own
secretStorage.setSecret("github_token", "ghp_...");
```

### "Authentication failed: HTTP 401"

Check that:

1. Token is valid and not expired
2. Token has required scopes (e.g., `repo` for GitHub)
3. Secret identifier matches the key stored in the database

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

- `serverUrl`: String - MCP server endpoint URL. `http` or `https`, and an
  address outside the deployment's own network — see [Which servers can be
  reached](#which-servers-can-be-reached).
- `secretIdentifier`: String - Secret identifier in the database (set via `secretStorage` API)
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
- [Example Scripts](https://github.com/lpajunen/aiwebengine-examples)
