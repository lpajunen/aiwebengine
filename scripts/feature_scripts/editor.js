// Simple aiwebengine Editor script
// This script provides basic editor functionality

// Serve the editor HTML page
function serveEditor(req) {
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
    contentType: "text/html",
  };
}

// API: List all scripts
function apiListScripts(req) {
  try {
    const scripts = typeof listScripts === "function" ? listScripts() : [];
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
        contentType: "text/plain",
      };
    }

    if (!content) {
      return {
        status: 404,
        body: "Script not found",
        contentType: "text/plain",
      };
    }

    return {
      status: 200,
      body: content,
      contentType: "text/plain",
    };
  } catch (error) {
    return {
      status: 500,
      body: "Error: " + error.message,
      contentType: "text/plain",
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
          writeLog(
            "Broadcasted script update from editor: " + action + " " + fullUri,
          );
        } catch (broadcastError) {
          writeLog(
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

// API: Get logs
function apiGetLogs(req) {
  try {
    const logs = typeof listLogs === "function" ? listLogs() : [];
    const formattedLogs = logs.map((log) => ({
      timestamp: new Date().toISOString(),
      level: "info",
      message: log,
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

// API: AI Assistant prompt handler
function apiAIAssistant(req) {
  // Debug: Log the raw request body
  writeLog(`AI Assistant: Raw request body: ${req.body}`);

  const body = JSON.parse(req.body || "{}");

  // Debug: Log the parsed body
  writeLog(`AI Assistant: Parsed body: ${JSON.stringify(body)}`);
  writeLog(`AI Assistant: Prompt value: "${body.prompt}"`);
  writeLog(
    `AI Assistant: Prompt length: ${body.prompt ? body.prompt.length : 0}`,
  );

  const prompt = body.prompt || "";
  const currentScript = body.currentScript || null;
  const currentScriptContent = body.currentScriptContent || null;

  // Check if Anthropic API key is configured
  if (!Secrets.exists("anthropic_api_key")) {
    writeLog(`AI Assistant: ERROR - Anthropic API key not configured`);
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
    writeLog(`AI Assistant: ERROR - Empty prompt received`);
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

  writeLog(
    `AI Assistant: Processing request with prompt: ${prompt.substring(0, 50)}...`,
  );

  // Build system prompt with comprehensive API documentation
  const systemPrompt = `You are an AI assistant for aiwebengine, a JavaScript-based web application engine.

CRITICAL: You MUST respond with ONLY valid JSON. No markdown, no code blocks, no explanations outside the JSON.

AVAILABLE JAVASCRIPT APIs:
1. register(path, handlerName, method) - Register HTTP routes
   - path: string (e.g., "/api/users")
   - handlerName: string (name of handler function)
   - method: "GET" | "POST" | "PUT" | "DELETE"

2. writeLog(message) - Write to server logs
   - message: string

3. fetch(url, options) - Make HTTP requests with secret injection support
   - url: string
   - options: JSON string with {method, headers, body, timeout_ms}
   - Supports {{secret:identifier}} in headers for secure API keys
   - Returns: JSON string with {status, ok, headers, body}

4. registerWebStream(path) - Register SSE stream endpoint
   - path: string (must start with /)

5. sendStreamMessage(data) - Broadcast to all stream clients
   - data: object (will be JSON serialized)

6. getSecret(key) - Retrieve secret value (use sparingly, prefer {{secret:}} in fetch)
   - key: string
   - Returns: string or null

7. listScripts() - Get list of all script URIs
   - Returns: array of strings

8. getScript(uri) - Get script content
   - uri: string (e.g., "https://example.com/blog")
   - Returns: string (script content)

RESPONSE FORMAT - YOU MUST RESPOND WITH ONLY THIS JSON STRUCTURE:
{
  "type": "explanation" | "create_script" | "edit_script" | "delete_script",
  "message": "Human-readable explanation of what you're suggesting",
  "script_name": "name.js",
  "code": "complete script code here",
  "original_code": "original code (for edits only)"
}

IMPORTANT: 
- Do NOT wrap your response in markdown code blocks
- Do NOT add any text before or after the JSON
- Start your response with { and end with }
- Escape all special characters in strings (newlines as \\n, quotes as \\")

SCRIPT STRUCTURE - All scripts must follow this pattern:
// Script description

function handlerName(req) {
  try {
    // Your logic here
    return {
      status: 200,
      body: 'response content',
      contentType: 'text/plain'
    };
  } catch (error) {
    writeLog('Error: ' + error);
    return { status: 500, body: 'Internal error' };
  }
}

function init(context) {
  writeLog('Initializing script');
  register('/path', 'handlerName', 'GET');
  return { success: true };
}

RULES:
1. ALWAYS respond with ONLY valid JSON - no other text
2. Include complete, working code with proper error handling
3. Use try-catch blocks in handlers
4. Always include init() function that registers routes
5. Use writeLog() for debugging
6. For edits, include both original_code and code fields
7. Never use Node.js APIs (fs, path, etc.) - they don't exist
8. Never use browser APIs (localStorage, document, window) - they don't exist
9. For API keys, use {{secret:identifier}} in fetch headers
10. Escape newlines as \\n in JSON strings

EXAMPLES OF CORRECT RESPONSES:

For explanation:
{"type":"explanation","message":"This script registers a GET endpoint at /api/users that returns a list of users in JSON format. It includes error handling and logging for debugging purposes."}

For create:
{"type":"create_script","message":"I'll create a simple hello world API endpoint","script_name":"hello.js","code":"function handleHello(req) {\\n  return {\\n    status: 200,\\n    body: 'Hello World!',\\n    contentType: 'text/plain'\\n  };\\n}\\n\\nfunction init(context) {\\n  register('/hello', 'handleHello', 'GET');\\n  return { success: true };\\n}"}

For edit:
{"type":"edit_script","message":"Adding error handling to make the code more robust","script_name":"api.js","original_code":"function handler(req) {\\n  return { status: 200, body: 'OK' };\\n}","code":"function handler(req) {\\n  try {\\n    return { status: 200, body: 'OK' };\\n  } catch (error) {\\n    writeLog('Error: ' + error);\\n    return { status: 500, body: 'Internal error' };\\n  }\\n}"}

Remember: Response must be PURE JSON only, nothing else!`;

  // Build contextual user prompt
  let contextualPrompt = "";
  
  // Add context about current script if available
  if (currentScript && currentScriptContent) {
    contextualPrompt += "CURRENT SCRIPT CONTEXT:\\n";
    contextualPrompt += "Script Name: " + currentScript + "\\n";
    contextualPrompt += "Script Content:\\n```javascript\\n" + currentScriptContent + "\\n```\\n\\n";
  }

  // Add available scripts list
  try {
    const scripts = typeof listScripts === "function" ? listScripts() : [];
    if (scripts.length > 0) {
      contextualPrompt += "AVAILABLE SCRIPTS: " + scripts.join(", ") + "\\n\\n";
    }
  } catch (e) {
    writeLog("Could not list scripts: " + e);
  }

  // Add user's actual prompt
  contextualPrompt += "USER REQUEST: " + prompt;

  writeLog("AI Assistant: Sending request with context...");

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
        max_tokens: 8192,
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
      writeLog(`AI Assistant: Success - Model: ${data.model}`);
      writeLog(`AI Assistant: Raw response length: ${aiResponse.length} chars`);
      writeLog(`AI Assistant: Raw response start: ${aiResponse.substring(0, 100)}...`);
      
      // Check if response was truncated (stopped mid-response)
      const stopReason = data.stop_reason || "unknown";
      writeLog(`AI Assistant: Stop reason: ${stopReason}`);
      
      if (stopReason === "max_tokens") {
        writeLog(`AI Assistant: WARNING - Response truncated due to max_tokens limit`);
      }
      
      // Clean up response - remove markdown code blocks if present
      let cleanedResponse = aiResponse.trim();
      
      // Remove markdown code blocks (```json ... ``` or ``` ... ```)
      if (cleanedResponse.startsWith("```")) {
        writeLog(`AI Assistant: Removing markdown code blocks`);
        // Remove opening ```json or ```
        cleanedResponse = cleanedResponse.replace(/^```(?:json)?\s*\n?/, "");
        // Remove closing ```
        cleanedResponse = cleanedResponse.replace(/\n?```\s*$/, "");
        cleanedResponse = cleanedResponse.trim();
        writeLog(`AI Assistant: Cleaned response start: ${cleanedResponse.substring(0, 100)}...`);
      }
      
      // Try to parse AI response as JSON for structured commands
      let parsedResponse = null;
      try {
        parsedResponse = JSON.parse(cleanedResponse);
        writeLog(`AI Assistant: Successfully parsed structured response of type: ${parsedResponse.type}`);
      } catch (parseError) {
        writeLog(`AI Assistant: Response is plain text, not JSON - Error: ${parseError}`);
        writeLog(`AI Assistant: First 200 chars: ${cleanedResponse.substring(0, 200)}`);
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
      writeLog(`AI Assistant: API error - Status: ${response.status}`);
      writeLog(`AI Assistant: Error body: ${response.body}`);

      let errorMessage = "API request failed";
      try {
        const errorData = JSON.parse(response.body);
        errorMessage =
          errorData.error?.message || errorData.message || errorMessage;
        writeLog(`AI Assistant: Error details: ${errorMessage}`);
      } catch (e) {
        // If we can't parse the error, just log the raw body
        writeLog(`AI Assistant: Could not parse error response`);
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
    writeLog(`AI Assistant: Error - ${error}`);
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
  writeLog("Initializing editor.js at " + new Date().toISOString());
  register("/editor", "serveEditor", "GET");
  register("/api/scripts", "apiListScripts", "GET");
  register("/api/scripts/*", "apiGetScript", "GET");
  register("/api/scripts/*", "apiSaveScript", "POST");
  register("/api/logs", "apiGetLogs", "GET");
  register("/api/assets", "apiGetAssets", "GET");
  register("/api/ai-assistant", "apiAIAssistant", "POST");
  writeLog("Editor endpoints registered");
  return { success: true };
}
