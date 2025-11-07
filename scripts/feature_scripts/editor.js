// Simple aiwebengine Editor script
// This script provides basic editor functionality

// Serve the editor HTML page
function serveEditor(req) {
  // Debug logging for authentication
  console.log("=== Editor Authentication Check ===");
  console.log("auth object exists: " + (typeof auth !== "undefined"));
  if (typeof auth !== "undefined") {
    console.log("auth.isAuthenticated: " + auth.isAuthenticated);
    console.log("auth.userId: " + auth.userId);
    console.log("auth.provider: " + auth.provider);
    console.log("auth.isEditor: " + auth.isEditor);
    console.log("auth.isAdmin: " + auth.isAdmin);
  }

  // Require authentication to access the editor
  let user;
  try {
    user = auth.requireAuth();
    console.log("Authentication successful for user: " + user.id);
  } catch (error) {
    console.log("Authentication failed: " + error.message);
    // Redirect to login page with return URL
    const currentPath = encodeURIComponent(req.path || "/editor");
    const loginUrl = "/auth/login?redirect=" + currentPath;
    console.log("Redirecting to: " + loginUrl);

    return {
      status: 302,
      headers: {
        Location: loginUrl,
      },
      body: "",
      contentType: "text/plain; charset=UTF-8",
    };
  }

  // Check if user has Editor or Administrator role
  if (!auth.isEditor && !auth.isAdmin) {
    console.log(
      "User " + user.id + " does not have Editor or Administrator role",
    );
    console.log("isEditor: " + auth.isEditor + ", isAdmin: " + auth.isAdmin);

    // Redirect to insufficient permissions page
    const currentPath = encodeURIComponent(req.path || "/editor");
    const insufficientPermissionsUrl =
      "/insufficient-permissions?attempted=" + currentPath;
    console.log("Redirecting to: " + insufficientPermissionsUrl);

    return {
      status: 302,
      headers: {
        Location: insufficientPermissionsUrl,
      },
      body: "",
      contentType: "text/plain; charset=UTF-8",
    };
  }

  console.log(
    "User " +
      user.id +
      " has required permissions (isEditor: " +
      auth.isEditor +
      ", isAdmin: " +
      auth.isAdmin +
      ")",
  );

  // Serve the modern editor UI
  // Note: The HTML is embedded here to ensure /editor is the single entry point
  const html = `<!DOCTYPE html>
<html lang="en">
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <title>Editor</title>
    <link rel="icon" type="image/x-icon" href="/favicon.ico">
    <link rel="stylesheet" href="/editor.css">
</head>
<body>
    <div class="editor-container">
        <!-- Header -->
        <header class="editor-header">
            <div class="header-left">
                <h1>aiwebengine Editor</h1>
            </div>
            <div class="header-right">
                <a href="/docs" class="header-link" title="View Documentation">📚 Docs</a>
                <a href="/graphql" class="header-link" title="GraphQL Editor">🔗 GraphQL</a>
                <span id="server-status" class="status-indicator">● Connected</span>
            </div>
        </header>

        <!-- Navigation -->
        <nav class="editor-nav">
            <button class="nav-tab active" data-tab="scripts">
                <span class="tab-icon">📄</span>
                Scripts
            </button>
            <button class="nav-tab" data-tab="assets">
                <span class="tab-icon">🖼️</span>
                Assets
            </button>
            <button class="nav-tab" data-tab="logs">
                <span class="tab-icon">📋</span>
                Logs
            </button>
            <button class="nav-tab" data-tab="routes">
                <span class="tab-icon">🔗</span>
                Routes
            </button>
        </nav>

        <!-- Main Content Area -->
        <main class="editor-main">
            <!-- Scripts Tab -->
            <div id="scripts-tab" class="tab-content active">
                <div class="scripts-container">
                    <div class="scripts-sidebar">
                        <div class="sidebar-header">
                            <h3>Scripts</h3>
                            <button id="new-script-btn" class="btn btn-primary btn-small">+ New</button>
                        </div>
                        <div id="scripts-list" class="scripts-list">
                            <!-- Scripts will be loaded here -->
                        </div>
                    </div>
                    <div class="scripts-editor">
                        <div class="editor-toolbar">
                            <span id="current-script-name" class="current-file">No script selected</span>
                            <div class="toolbar-actions">
                                <button id="save-script-btn" class="btn btn-success" disabled>Save</button>
                                <button id="delete-script-btn" class="btn btn-danger" disabled>Delete</button>
                            </div>
                        </div>
                        <div id="monaco-editor" class="monaco-container"></div>
                    </div>
                </div>
            </div>

            <!-- Assets Tab -->
            <div id="assets-tab" class="tab-content">
                <div class="assets-container">
                    <div class="assets-header">
                        <h3>Assets</h3>
                        <div class="upload-area">
                            <input type="file" id="asset-upload" multiple style="display: none;">
                            <button id="upload-asset-btn" class="btn btn-primary">Upload Assets</button>
                        </div>
                    </div>
                    <div id="assets-grid" class="assets-grid">
                        <!-- Assets will be loaded here -->
                    </div>
                </div>
            </div>

            <!-- Logs Tab -->
            <div id="logs-tab" class="tab-content">
                <div class="logs-container">
                    <div class="logs-header">
                        <h3>Server Logs</h3>
                        <div class="logs-controls">
                            <button id="refresh-logs-btn" class="btn btn-secondary">Refresh</button>
                            <button id="clear-logs-btn" class="btn btn-warning">Clear</button>
                        </div>
                    </div>
                    <div id="logs-content" class="logs-content">
                        <!-- Logs will be loaded here -->
                    </div>
                </div>
            </div>

            <!-- Routes Tab -->
            <div id="routes-tab" class="tab-content">
                <div class="routes-container">
                    <div class="routes-header">
                        <h3>Registered Routes</h3>
                        <button id="refresh-routes-btn" class="btn btn-secondary">Refresh</button>
                    </div>
                    <div id="routes-list" class="routes-list">
                        <!-- Routes will be loaded here -->
                    </div>
                </div>
            </div>
        </main>

        <!-- AI Assistant Section -->
        <div class="ai-assistant">
            <div class="ai-assistant-header">
                <h3>🤖 AI Assistant</h3>
                <button id="toggle-ai-assistant" class="btn btn-secondary btn-small">▼</button>
            </div>
            <div class="ai-assistant-content">
                <div class="ai-assistant-body">
                    <div class="ai-response-container">
                        <div class="ai-response-header">Response</div>
                        <div id="ai-response" class="ai-response">
                            <p class="ai-placeholder">AI responses will appear here...</p>
                        </div>
                    </div>
                    <div class="ai-prompt-container">
                        <textarea id="ai-prompt" class="ai-prompt" placeholder="Ask the AI assistant for help with your scripts..."></textarea>
                        <div class="ai-prompt-actions">
                            <button id="clear-prompt-btn" class="btn btn-secondary btn-small">Clear</button>
                            <button id="submit-prompt-btn" class="btn btn-primary">Submit</button>
                        </div>
                    </div>
                </div>
            </div>
        </div>

        <!-- Status Bar -->
        <footer class="editor-footer">
            <div class="status-info">
                <span id="status-message">Ready</span>
            </div>
            <div class="status-actions">
                <button id="test-endpoint-btn" class="btn btn-secondary btn-small">Test API</button>
            </div>
        </footer>
    </div>

    <!-- Diff Preview Modal -->
    <div id="diff-modal" class="modal">
        <div class="modal-content">
            <div class="modal-header">
                <h3 id="diff-modal-title">Preview Changes</h3>
                <button id="close-diff-modal" class="btn btn-secondary btn-small">&times;</button>
            </div>
            <div class="modal-body">
                <div id="diff-explanation" class="diff-explanation"></div>
                <div id="monaco-diff-editor" class="monaco-diff-container"></div>
            </div>
            <div class="modal-footer">
                <button id="reject-changes-btn" class="btn btn-danger">Reject</button>
                <button id="apply-changes-btn" class="btn btn-success">Apply Changes</button>
            </div>
        </div>
    </div>

    <!-- Load Monaco Editor -->
    <script src="https://unpkg.com/monaco-editor@0.45.0/min/vs/loader.js"></script>
    
    <!-- Main JavaScript -->
    <script src="/editor.js"></script>
</body>
</html>`;

  return {
    status: 200,
    body: html,
    contentType: "text/html; charset=UTF-8",
  };
}

// API: List all scripts
function apiListScripts(req) {
  try {
    const scripts = typeof listScripts === "function" ? listScripts() : [];

    // Sort scripts alphabetically (case-insensitive)
    scripts.sort((a, b) => a.toLowerCase().localeCompare(b.toLowerCase()));

    const scriptDetails = scripts.map((name) => ({
      name: name,
      size: 0,
      lastModified: new Date().toISOString(),
    }));

    return {
      status: 200,
      body: JSON.stringify(scriptDetails),
      contentType: "application/json",
    };
  } catch (error) {
    return {
      status: 500,
      body: JSON.stringify({ error: error.message }),
      contentType: "application/json",
    };
  }
}

// API: Get script content
function apiGetScript(req) {
  try {
    // Extract the script name from the path
    // The path will be something like /api/scripts/https://example.com/core
    let scriptName = req.path.replace("/api/scripts/", "");

    // URL decode the script name in case it contains encoded characters
    scriptName = decodeURIComponent(scriptName);

    // If it's already a full URI, use it as-is
    // If it's just a short name, convert it to full URI
    let fullUri;
    if (scriptName.startsWith("https://")) {
      fullUri = scriptName;
    } else {
      fullUri = "https://example.com/" + scriptName;
    }

    let content = "";

    if (typeof getScript === "function") {
      content = getScript(fullUri) || "";
    } else {
      return {
        status: 500,
        body: "getScript function not available",
        contentType: "text/plain; charset=UTF-8",
      };
    }

    if (!content) {
      return {
        status: 404,
        body: "Script not found",
        contentType: "text/plain; charset=UTF-8",
      };
    }

    return {
      status: 200,
      body: content,
      contentType: "text/plain; charset=UTF-8",
    };
  } catch (error) {
    return {
      status: 500,
      body: "Error: " + error.message,
      contentType: "text/plain; charset=UTF-8",
    };
  }
}

// API: Save/update script
function apiSaveScript(req) {
  try {
    // Extract the script name from the path
    let scriptName = req.path.replace("/api/scripts/", "");

    // URL decode the script name in case it contains encoded characters
    scriptName = decodeURIComponent(scriptName);

    // If it's already a full URI, use it as-is
    // If it's just a short name, convert it to full URI
    let fullUri;
    if (scriptName.startsWith("https://")) {
      fullUri = scriptName;
    } else {
      fullUri = "https://example.com/" + scriptName;
    }

    if (typeof upsertScript === "function") {
      // Check if script already exists to determine action
      const existingScript = getScript ? getScript(fullUri) : null;
      const action = existingScript ? "updated" : "inserted";

      upsertScript(fullUri, req.body);

      // Broadcast the script update notification
      if (typeof sendStreamMessageToPath === "function") {
        try {
          const message = {
            type: "script_update",
            uri: fullUri,
            action: action,
            timestamp: new Date().toISOString(),
            contentLength: req.body.length,
            previousExists: !!existingScript,
            via: "editor",
          };
          sendStreamMessageToPath("/script_updates", JSON.stringify(message));
          console.log(
            "Broadcasted script update from editor: " + action + " " + fullUri,
          );
        } catch (broadcastError) {
          console.log(
            "Failed to broadcast script update from editor: " +
              broadcastError.message,
          );
        }
      }
    }

    return {
      status: 200,
      body: JSON.stringify({ message: "Script saved" }),
      contentType: "application/json",
    };
  } catch (error) {
    return {
      status: 500,
      body: JSON.stringify({ error: error.message }),
      contentType: "application/json",
    };
  }
}

// API: Delete script
function apiDeleteScript(req) {
  try {
    // Extract the script name from the path
    let scriptName = req.path.replace("/api/scripts/", "");

    // URL decode the script name in case it contains encoded characters
    scriptName = decodeURIComponent(scriptName);

    // If it's already a full URI, use it as-is
    // If it's just a short name, convert it to full URI
    let fullUri;
    if (scriptName.startsWith("https://")) {
      fullUri = scriptName;
    } else {
      fullUri = "https://example.com/" + scriptName;
    }

    if (typeof deleteScript === "function") {
      const deleted = deleteScript(fullUri);

      if (deleted) {
        // Broadcast the script removal notification
        if (typeof sendStreamMessageToPath === "function") {
          try {
            const message = {
              type: "script_update",
              uri: fullUri,
              action: "removed",
              timestamp: new Date().toISOString(),
              via: "editor",
            };
            sendStreamMessageToPath("/script_updates", JSON.stringify(message));
            console.log("Broadcasted script deletion from editor: " + fullUri);
          } catch (broadcastError) {
            console.log(
              "Failed to broadcast script deletion from editor: " +
                broadcastError.message,
            );
          }
        }

        console.log("Script deleted via editor API: " + fullUri);
        return {
          status: 200,
          body: JSON.stringify({
            message: "Script deleted successfully",
            uri: fullUri,
          }),
          contentType: "application/json",
        };
      } else {
        console.log("Script not found for deletion via editor API: " + fullUri);
        return {
          status: 404,
          body: JSON.stringify({
            error: "Script not found",
            message: "No script with the specified name was found",
            uri: fullUri,
          }),
          contentType: "application/json",
        };
      }
    } else {
      return {
        status: 500,
        body: JSON.stringify({
          error: "deleteScript function not available",
        }),
        contentType: "application/json",
      };
    }
  } catch (error) {
    console.log("Script deletion failed via editor API: " + error.message);
    return {
      status: 500,
      body: JSON.stringify({
        error: "Failed to delete script",
        details: error.message,
      }),
      contentType: "application/json",
    };
  }
}

// API: Get logs
function apiGetLogs(req) {
  try {
    const logsJson = typeof listLogs === "function" ? listLogs() : "[]";
    const logs = JSON.parse(logsJson);
    const formattedLogs = logs.map((log) => ({
      timestamp: new Date(log.timestamp),
      level: log.level || "INFO",
      message: log.message,
    }));

    return {
      status: 200,
      body: JSON.stringify(formattedLogs),
      contentType: "application/json",
    };
  } catch (error) {
    return {
      status: 500,
      body: JSON.stringify({ error: error.message }),
      contentType: "application/json",
    };
  }
}

// API: Get assets
function apiGetAssets(req) {
  try {
    const assets = typeof listAssets === "function" ? listAssets() : [];
    const assetDetails = assets.map((path) => ({
      path: path,
      name: path.split("/").pop(),
      size: 0,
      type: "application/octet-stream",
    }));

    return {
      status: 200,
      body: JSON.stringify({ assets: assetDetails }),
      contentType: "application/json",
    };
  } catch (error) {
    return {
      status: 500,
      body: JSON.stringify({ error: error.message }),
      contentType: "application/json",
    };
  }
}

// API: List all registered routes
function apiListRoutes(req) {
  try {
    const routes = typeof listRoutes === "function" ? listRoutes() : "[]";
    // Parse and re-stringify to ensure valid JSON
    const routesData = JSON.parse(routes);

    return {
      status: 200,
      body: JSON.stringify(routesData),
      contentType: "application/json",
    };
  } catch (error) {
    return {
      status: 500,
      body: JSON.stringify({ error: error.message }),
      contentType: "application/json",
    };
  }
}

// API: AI Assistant prompt handler
function apiAIAssistant(req) {
  // Debug: Log the raw request body
  console.log(`AI Assistant: Raw request body: ${req.body}`);

  const body = JSON.parse(req.body || "{}");

  // Debug: Log the parsed body
  console.log(`AI Assistant: Parsed body: ${JSON.stringify(body)}`);
  console.log(`AI Assistant: Prompt value: "${body.prompt}"`);
  console.log(
    `AI Assistant: Prompt length: ${body.prompt ? body.prompt.length : 0}`,
  );

  const prompt = body.prompt || "";
  const currentScript = body.currentScript || null;
  const currentScriptContent = body.currentScriptContent || null;

  // Check if Anthropic API key is configured
  if (!Secrets.exists("anthropic_api_key")) {
    console.log(`AI Assistant: ERROR - Anthropic API key not configured`);
    return {
      status: 503,
      body: JSON.stringify({
        success: false,
        error: "Anthropic API key not configured",
        message:
          "Please set SECRET_ANTHROPIC_API_KEY environment variable or configure secrets.values.anthropic_api_key in config file",
      }),
      contentType: "application/json",
    };
  }

  // Validate prompt is not empty
  if (!prompt || prompt.trim().length === 0) {
    console.log(`AI Assistant: ERROR - Empty prompt received`);
    return {
      status: 400,
      body: JSON.stringify({
        success: false,
        error: "Empty prompt",
        message: "Please provide a non-empty prompt",
      }),
      contentType: "application/json",
    };
  }

  console.log(
    `AI Assistant: Processing request with prompt: ${prompt.substring(0, 50)}...`,
  );

  // Build system prompt with comprehensive API documentation
  const systemPrompt = `You are an AI assistant for aiwebengine, a JavaScript-based web application engine.

YOUR JOB: Help users create JavaScript scripts that handle HTTP requests and return responses (HTML, JSON, text, etc.).

CRITICAL: You MUST respond with ONLY valid JSON. No markdown, no code blocks, no explanations outside the JSON.

WHAT ARE aiwebengine SCRIPTS?
- JavaScript files that handle HTTP requests
- Return HTML pages, JSON APIs, file uploads, etc.
- Use handler functions that take a request and return a response
- Must have an init() function that registers routes

AVAILABLE JAVASCRIPT APIs:
1. register(path, handlerName, method) - Register HTTP routes
   - path: string (e.g., "/api/users" or "/hello")
   - handlerName: string (name of your handler function)
   - method: "GET" | "POST" | "PUT" | "DELETE"

2. console.log(message) - Write to server logs
   - message: string

3. scriptStorage - Persistent key-value storage per script
   - scriptStorage.getItem(key) - Get stored value (returns string or null)
   - scriptStorage.setItem(key, value) - Store key-value pair (returns success message)
   - scriptStorage.removeItem(key) - Delete key-value pair (returns boolean)
   - scriptStorage.clear() - Remove all data for this script (returns success message)
   - Each script has its own isolated storage namespace
   - Data persists across requests and server restarts (when PostgreSQL configured)

4. fetch(url, options) - Make HTTP requests to external APIs
   - url: string
   - options: JSON string with {method, headers, body, timeout_ms}
   - Supports {{secret:identifier}} in headers for secure API keys
   - Returns: JSON string with {status, ok, headers, body}

5. registerWebStream(path) - Register SSE stream endpoint
   - path: string (must start with /)

6. sendStreamMessageToPath(path, data) - Send message to specific stream path
   - path: string (must start with /)
   - data: object (will be JSON serialized)

7. sendStreamMessageToConnections(path, data, filterJson) - Send message to specific connections based on metadata filtering
   - path: string (must start with /)
   - data: object (will be JSON serialized)
   - filterJson: string (optional JSON string with metadata filter criteria, empty "{}" matches all)
   - Returns: string describing broadcast result with success/failure counts
   - Use for personalized broadcasting to specific users/groups on stable endpoints

8. sendSubscriptionMessageToConnections(subscriptionName, data, filterJson) - Send message to specific GraphQL subscription connections
   - subscriptionName: string (name of GraphQL subscription)
   - data: object (will be JSON serialized)
   - filterJson: string (optional JSON string with metadata filter criteria, empty "{}" matches all)
   - Returns: string describing broadcast result with success/failure counts
   - Use for personalized GraphQL subscription broadcasting

9. registerGraphQLQuery(name, schema, resolverName) - Register GraphQL query
   - name: string (query name)
   - schema: string (GraphQL schema definition)
   - resolverName: string (name of resolver function)

10. registerGraphQLMutation(name, schema, resolverName) - Register GraphQL mutation
    - name: string (mutation name)
    - schema: string (GraphQL schema definition)
    - resolverName: string (name of resolver function)

11. registerGraphQLSubscription(name, schema, resolverName) - Register GraphQL subscription
    - name: string (subscription name)
    - schema: string (GraphQL schema definition)
    - resolverName: string (name of resolver function)

12. executeGraphQL(query, variables) - Execute GraphQL query
    - query: string (GraphQL query string)
    - variables: string (optional JSON string of variables)
    - Returns: JSON string with GraphQL response

RESPONSE FORMAT - YOU MUST RESPOND WITH ONLY THIS JSON STRUCTURE:
{
  "type": "explanation" | "create_script" | "edit_script" | "delete_script",
  "message": "Human-readable explanation",
  "script_name": "name.js",
  "code": "complete JavaScript code",
  "original_code": "original code (for edits only)"
}

CRITICAL JSON RULES:
- Do NOT wrap your response in markdown code blocks (no \`\`\`json)
- Do NOT add any text before or after the JSON  
- Start your response with { and end with }
- Your response must be valid, parseable JSON
- In the "code" field, use standard JSON escaping: newline = \\n, quote = \\", backslash = \\\\
- Do NOT double-escape! A newline in your code should be represented as ONE \\n in the JSON, not \\\\n

SCRIPT STRUCTURE - Every script MUST follow this pattern:
// Script description
// Handles HTTP requests and returns responses

function handlerName(req) {
  // req has: path, method, headers, query, form, body
  try {
    // Generate your response (HTML, JSON, etc.)
    return {
      status: 200,
      body: 'your response content here',
      contentType: 'text/html; charset=UTF-8' // or 'application/json', 'text/plain; charset=UTF-8'
    };
  } catch (error) {
    console.log('Error: ' + error);
    return { status: 500, body: 'Internal error' };
  }
}

function init(context) {
  console.log('Initializing script');
  register('/your-path', 'handlerName', 'GET');
  return { success: true };
}

IMPORTANT CONCEPTS:
1. Scripts are SERVER-SIDE JavaScript that handle HTTP requests
2. To create a web page, return HTML in the body with contentType: 'text/html; charset=UTF-8'
3. To create an API, return JSON in the body with contentType: 'application/json; charset=UTF-8'
4. Scripts don't have access to browser APIs or Node.js APIs
5. Use fetch() to call external APIs
6. Use register() in init() to map URLs to handler functions
7. For real-time features, use registerWebStream() and sendStreamMessageToPath()
8. For personalized broadcasting, use sendStreamMessageToConnections() with metadata filters
9. Selective broadcasting enables chat apps and user-specific notifications without dynamic endpoints

RULES:
1. ALWAYS respond with ONLY valid JSON - no other text
2. Include complete, working JavaScript code
3. Use try-catch blocks in all handlers
4. ALWAYS include init() function that calls register()
5. Use console.log() for debugging
6. For edits, include both original_code and code fields
7. Never use Node.js APIs (fs, path, etc.) - they don't exist here
8. Never use browser APIs (localStorage, document, window) - they don't exist here
9. For external API keys, use {{secret:identifier}} in fetch headers
10. Escape all special characters in JSON strings

EXAMPLES OF CORRECT RESPONSES:

Example 1 - Create web page:
{"type":"create_script","message":"Creating a script that serves an HTML page","script_name":"hello-page.js","code":"// Hello page\\n\\nfunction servePage(req) {\\n  const html = '<!DOCTYPE html><html><head><title>Hello</title></head><body><h1>Hello World!</h1></body></html>';\\n  return { status: 200, body: html, contentType: 'text/html; charset=UTF-8' };\\n}\\n\\nfunction init(context) {\\n  register('/hello', 'servePage', 'GET');\\n  return { success: true };\\n}"}

Example 2 - Create JSON API:
{"type":"create_script","message":"Creating a REST API endpoint","script_name":"users-api.js","code":"// Users API\\n\\nfunction getUsers(req) {\\n  const users = [{id: 1, name: 'Alice'}, {id: 2, name: 'Bob'}];\\n  return { status: 200, body: JSON.stringify(users), contentType: 'application/json; charset=UTF-8' };\\n}\\n\\nfunction init(context) {\\n  register('/api/users', 'getUsers', 'GET');\\n  return { success: true };\\n}"}

Example 3 - Explanation:
{"type":"explanation","message":"This script registers a GET endpoint that returns JSON user data with proper error handling and content type."}

Example 4 - Selective Broadcasting Chat:
{"type":"create_script","message":"Creating a chat application with selective broadcasting for personalized messages","script_name":"chat-app.js","code":"// Chat Application with Selective Broadcasting\\n\\n// Register one stream for all chat messages\\nfunction init(context) {\\n  registerWebStream('/chat');\\n  register('/chat/send', 'sendMessage', 'POST');\\n  register('/chat/personal', 'sendPersonalMessage', 'POST');\\n  return { success: true };\\n}\\n\\n// Send message to specific room\\nfunction sendMessage(req) {\\n  const { room, message, sender } = req.form;\\n  \\n  const result = sendStreamMessageToConnections('/chat', {\\n    type: 'room_message',\\n    room: room,\\n    message: message,\\n    sender: sender,\\n    timestamp: new Date().toISOString()\\n  }, JSON.stringify({ room: room }));\\n  \\n  return {\\n    status: 200,\\n    body: JSON.stringify({ success: true, result: result }),\\n    contentType: 'application/json; charset=UTF-8'\\n  };\\n}\\n\\n// Send personal message to specific user\\nfunction sendPersonalMessage(req) {\\n  const { targetUser, message, sender } = req.form;\\n  \\n  const result = sendStreamMessageToConnections('/chat', {\\n    type: 'personal_message',\\n    message: message,\\n    sender: sender,\\n    timestamp: new Date().toISOString()\\n  }, JSON.stringify({ user_id: targetUser }));\\n  \\n  return {\\n    status: 200,\\n    body: JSON.stringify({ success: true, result: result }),\\n    contentType: 'application/json; charset=UTF-8'\\n  };\\n}"}

Example 5 - GraphQL Subscription with Selective Broadcasting:
{"type":"create_script","message":"Creating a GraphQL subscription with selective broadcasting for personalized notifications","script_name":"notification-subscription.js","code":"// GraphQL Subscription with Selective Broadcasting\\n\\nfunction init(context) {\\n  registerGraphQLSubscription(\\n    'userNotifications',\\n    'type Subscription { userNotifications: String }',\\n    'userNotificationsResolver'\\n  );\\n  register('/notify/user', 'sendUserNotification', 'POST');\\n  return { success: true };\\n}\\n\\nfunction userNotificationsResolver() {\\n  return 'User notifications subscription active';\\n}\\n\\nfunction sendUserNotification(req) {\\n  const { userId, message, type } = req.form;\\n  \\n  const result = sendSubscriptionMessageToConnections('userNotifications', {\\n    type: type || 'notification',\\n    message: message,\\n    timestamp: new Date().toISOString()\\n  }, JSON.stringify({ user_id: userId }));\\n  \\n  return {\\n    status: 200,\\n    body: JSON.stringify({ success: true, result: result }),\\n    contentType: 'application/json; charset=UTF-8'\\n  };\\n}"}

IMPORTANT: In these examples, each \\n represents ONE newline character in the JavaScript code. When you output JSON, a newline in the source code becomes \\n in the JSON string.

Remember: You are creating JavaScript scripts that run on the SERVER and handle HTTP requests. When someone asks for a "web page", you create a script that SERVES that HTML page!`;

  // Build contextual user prompt
  let contextualPrompt = "";

  // Add context about current script if available
  if (currentScript && currentScriptContent) {
    contextualPrompt += "CURRENT SCRIPT CONTEXT:\\n";
    contextualPrompt += "Script Name: " + currentScript + "\\n";
    contextualPrompt +=
      "Script Content:\\n```javascript\\n" +
      currentScriptContent +
      "\\n```\\n\\n";
  }

  // Add available scripts list
  try {
    const scripts = typeof listScripts === "function" ? listScripts() : [];
    if (scripts.length > 0) {
      contextualPrompt += "AVAILABLE SCRIPTS: " + scripts.join(", ") + "\\n\\n";
    }
  } catch (e) {
    console.log("Could not list scripts: " + e);
  }

  // Add user's actual prompt
  contextualPrompt += "USER REQUEST: " + prompt;

  console.log("AI Assistant: Sending request with context...");

  try {
    // Make request to Anthropic API with secret injection and system prompt
    const options = JSON.stringify({
      method: "POST",
      headers: {
        "Content-Type": "application/json",
        "x-api-key": "{{secret:anthropic_api_key}}",
        "anthropic-version": "2023-06-01",
      },
      body: JSON.stringify({
        model: "claude-haiku-4-5-20251001",
        max_tokens: 8192 * 3,
        system: systemPrompt,
        messages: [
          {
            role: "user",
            content: contextualPrompt,
          },
        ],
      }),
    });

    const responseJson = fetch(
      "https://api.anthropic.com/v1/messages",
      options,
    );
    const response = JSON.parse(responseJson);

    if (response.ok) {
      const data = JSON.parse(response.body);
      let aiResponse = data.content[0].text;
      console.log(`AI Assistant: Success - Model: ${data.model}`);
      console.log(
        `AI Assistant: Raw response length: ${aiResponse.length} chars`,
      );
      console.log(
        `AI Assistant: Raw response start: ${aiResponse.substring(0, 100)}...`,
      );

      // Check if response was truncated (stopped mid-response)
      const stopReason = data.stop_reason || "unknown";
      console.log(`AI Assistant: Stop reason: ${stopReason}`);

      if (stopReason === "max_tokens") {
        console.log(
          `AI Assistant: WARNING - Response truncated due to max_tokens limit`,
        );
      }

      // Clean up response - remove markdown code blocks if present
      let cleanedResponse = aiResponse.trim();

      // Remove markdown code blocks (```json ... ``` or ``` ... ```)
      if (cleanedResponse.startsWith("```")) {
        console.log(`AI Assistant: Removing markdown code blocks`);
        // Remove opening ```json or ```
        cleanedResponse = cleanedResponse.replace(/^```(?:json)?\s*\n?/, "");
        // Remove closing ```
        cleanedResponse = cleanedResponse.replace(/\n?```\s*$/, "");
        cleanedResponse = cleanedResponse.trim();
        console.log(
          `AI Assistant: Cleaned response start: ${cleanedResponse.substring(0, 100)}...`,
        );
      }

      // Try to parse AI response as JSON for structured commands
      let parsedResponse = null;
      try {
        parsedResponse = JSON.parse(cleanedResponse);
        console.log(
          `AI Assistant: Successfully parsed structured response of type: ${parsedResponse.type}`,
        );
      } catch (parseError) {
        console.log(
          `AI Assistant: Response is plain text, not JSON - Error: ${parseError}`,
        );
        console.log(
          `AI Assistant: First 200 chars: ${cleanedResponse.substring(0, 200)}`,
        );
      }

      return {
        status: 200,
        body: JSON.stringify({
          success: true,
          response: aiResponse,
          parsed: parsedResponse,
          model: data.model,
          usage: data.usage,
          stop_reason: stopReason,
        }),
        contentType: "application/json",
      };
    } else {
      // Log the full error response for debugging
      console.log(`AI Assistant: API error - Status: ${response.status}`);
      console.log(`AI Assistant: Error body: ${response.body}`);

      let errorMessage = "API request failed";
      try {
        const errorData = JSON.parse(response.body);
        errorMessage =
          errorData.error?.message || errorData.message || errorMessage;
        console.log(`AI Assistant: Error details: ${errorMessage}`);
      } catch (e) {
        // If we can't parse the error, just log the raw body
        console.log(`AI Assistant: Could not parse error response`);
      }

      return {
        status: response.status,
        body: JSON.stringify({
          success: false,
          error: errorMessage,
          status: response.status,
          details: response.body,
        }),
        contentType: "application/json",
      };
    }
  } catch (error) {
    console.log(`AI Assistant: Error - ${error}`);
    return {
      status: 500,
      body: JSON.stringify({
        success: false,
        error: "Internal error",
        message: String(error),
      }),
      contentType: "application/json",
    };
  }
}

// Initialization function
function init(context) {
  console.log("Initializing editor.js at " + new Date().toISOString());
  register("/editor", "serveEditor", "GET");
  register("/api/scripts", "apiListScripts", "GET");
  register("/api/scripts/*", "apiGetScript", "GET");
  register("/api/scripts/*", "apiSaveScript", "POST");
  register("/api/scripts/*", "apiDeleteScript", "DELETE");
  register("/api/logs", "apiGetLogs", "GET");
  register("/api/assets", "apiGetAssets", "GET");
  register("/api/routes", "apiListRoutes", "GET");
  register("/api/ai-assistant", "apiAIAssistant", "POST");
  console.log("Editor endpoints registered");
  return { success: true };
}
