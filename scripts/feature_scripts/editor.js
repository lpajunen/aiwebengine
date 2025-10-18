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
  const body = JSON.parse(req.body || "{}");
  const prompt = body.prompt || "";

  // For now, return a placeholder response
  // Later this will be integrated with actual AI capabilities
  const response = {
    success: true,
    response: "Response under development",
    prompt: prompt,
    rawBody: req.body, // Debug: show what we received
    timestamp: new Date().toISOString(),
  };

  return {
    status: 200,
    body: JSON.stringify(response),
    contentType: "application/json",
  };
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
