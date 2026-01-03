/// <reference path="../../assets/aiwebengine-priv.d.ts" />

// Simple aiwebengine Editor script
// This script provides basic editor functionality

function getRequest(context) {
  return (context && context.request) || {};
}

function getArgs(context) {
  return (context && context.args) || {};
}

// Serve the editor HTML page
function serveEditor(context) {
  const req = getRequest(context);
  // Debug logging for authentication
  console.log("=== Editor Authentication Check ===");
  console.log("req.auth object exists: " + (typeof req.auth !== "undefined"));
  if (typeof req.auth !== "undefined") {
    console.log("req.auth.isAuthenticated: " + req.auth.isAuthenticated);
    console.log("req.auth.userId: " + req.auth.userId);
    console.log("req.auth.provider: " + req.auth.provider);
    console.log("req.auth.isEditor: " + req.auth.isEditor);
    console.log("req.auth.isAdmin: " + req.auth.isAdmin);
  }

  // Require authentication to access the editor
  let user;
  try {
    user = req.auth.requireAuth();
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
  if (!req.auth.isEditor && !req.auth.isAdmin) {
    console.log(
      "User " + user.id + " does not have Editor or Administrator role",
    );
    console.log(
      "isEditor: " + req.auth.isEditor + ", isAdmin: " + req.auth.isAdmin,
    );

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
      req.auth.isEditor +
      ", isAdmin: " +
      req.auth.isAdmin +
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
        <!-- Unified Header -->
        <header class="unified-header">
            <div class="header-left">
                <h1>aiwebengine</h1>
            </div>
            <nav class="unified-nav">
                <a href="/engine/docs" title="Documentation">📚 Documentation</a>
                <a href="/engine/editor" title="Code Editor">✏️ Editor</a>
                <a href="/engine/graphql" title="GraphQL API">🔗 GraphiQL</a>
                <a href="/engine/swagger" title="REST API">📖 Swagger</a>
            </nav>
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
                        <div class="scripts-filter">
                            <select id="scripts-filter-select" class="filter-select">
                                <option value="all">All Scripts</option>
                                <option value="mine">My Scripts</option>
                                <option value="system">System Scripts</option>
                                <option value="unowned">Unowned Scripts</option>
                            </select>
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
                        <div class="script-security-panel">
                          <span id="script-privileged-badge" class="privileged-badge neutral">No script selected</span>
                          <button id="toggle-privileged-btn" class="btn btn-secondary btn-small" disabled>Toggle Privileged</button>
                        </div>
                        <div class="script-ownership-panel">
                          <div class="ownership-info">
                            <span class="ownership-label">Owners:</span>
                            <span id="script-owners-list" class="owners-list">-</span>
                          </div>
                          <div class="ownership-actions">
                            <button id="manage-owners-btn" class="btn btn-secondary btn-small" disabled>Manage Owners</button>
                          </div>
                        </div>
                        <div id="monaco-editor" class="monaco-container"></div>
                    </div>
                </div>
            </div>

            <!-- Assets Tab -->
            <div id="assets-tab" class="tab-content">
                <div class="assets-container">
                    <div class="assets-sidebar">
                        <div class="sidebar-header">
                            <h3>Assets</h3>
                            <div class="asset-buttons">
                                <button id="new-asset-btn" class="btn btn-primary btn-small">+ New</button>
                                <input type="file" id="asset-upload" multiple style="display: none;">
                                <button id="upload-asset-btn" class="btn btn-primary btn-small">Upload</button>
                            </div>
                        </div>
                        <div class="assets-filter">
                            <select id="assets-script-select" class="filter-select">
                                <option value="">Select a script...</option>
                            </select>
                        </div>
                        <div id="assets-list" class="assets-list">
                            <!-- Assets will be loaded here -->
                        </div>
                    </div>
                    <div class="assets-editor">
                        <div class="editor-toolbar">
                            <span id="current-asset-name" class="current-file">No asset selected</span>
                            <div class="toolbar-actions">
                                <button id="save-asset-btn" class="btn btn-success" disabled>Save</button>
                                <button id="delete-asset-btn" class="btn btn-danger" disabled>Delete</button>
                            </div>
                        </div>
                        <div id="asset-editor-content" class="asset-editor-content">
                            <div id="monaco-asset-editor" class="monaco-container" style="display: none;"></div>
                            <div id="binary-asset-info" class="binary-asset-info" style="display: none;">
                                <div class="binary-info-content">
                                    <h3>Binary File</h3>
                                    <p>This is a binary file and cannot be edited as text.</p>
                                    <div id="binary-asset-details" class="binary-details"></div>
                                    <div id="binary-asset-preview" class="binary-preview"></div>
                                </div>
                            </div>
                            <div id="no-asset-selected" class="no-selection">
                                <p>Select an asset to view or edit</p>
                            </div>
                        </div>
                    </div>
                </div>
            </div>

            <!-- Logs Tab -->
            <div id="logs-tab" class="tab-content">
                <div class="logs-container">
                    <div class="logs-header">
                        <h3>Server Logs</h3>
                        <div class="logs-controls">
                            <button id="refresh-logs-btn" class="btn btn-secondary" aria-label="Jump to latest logs">Jump to latest</button>
                            <button id="clear-logs-btn" class="btn btn-warning">Clear</button>
                        </div>
                    </div>
                    <div id="logs-content" class="logs-content" tabindex="0">
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
    
    <!-- Active state detection for unified navigation -->
    <script>
        (function() {
            const path = window.location.pathname;
            document.querySelectorAll('.unified-nav a').forEach(function(link) {
                const linkPath = new URL(link.href, window.location.origin).pathname;
                if (path.startsWith(linkPath)) {
                    link.classList.add('active');
                }
            });
        })();
    </script>
</body>
</html>`;

  return {
    status: 200,
    body: html,
    contentType: "text/html; charset=UTF-8",
  };
}

// Serve GraphiQL interface
function serveGraphiQL(context) {
  const req = getRequest(context);

  // Serve GraphiQL using CDN
  const html = `<!DOCTYPE html>
<html lang="en">
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <title>aiwebengine GraphiQL Editor</title>
    <link rel="stylesheet" href="https://unpkg.com/graphiql@3/graphiql.min.css" />
    <style>
        body { 
            margin: 0; 
            padding: 0; 
            height: 100vh;
            display: flex;
            flex-direction: column;
            background: #1e1e1e;
        }
        .unified-header {
            display: flex;
            justify-content: space-between;
            align-items: center;
            padding: 10px 20px;
            background-color: #252526;
            border-bottom: 1px solid #3e3e42;
            min-height: 50px;
            flex-shrink: 0;
        }
        .unified-header h1 {
            font-size: 18px;
            font-weight: 600;
            color: #cccccc;
            margin: 0;
            line-height: 1;
            font-family: 'Segoe UI', Tahoma, Geneva, Verdana, sans-serif;
        }
        .unified-nav {
            display: flex;
            gap: 5px;
        }
        .unified-nav a {
            color: #999999;
            text-decoration: none;
            font-size: 12px;
            padding: 6px 12px;
            border-radius: 4px;
            transition: all 0.2s ease;
            display: flex;
            align-items: center;
            gap: 4px;
            font-family: 'Segoe UI', Tahoma, Geneva, Verdana, sans-serif;
        }
        .unified-nav a:hover {
            background-color: #37373d;
            color: #007acc;
        }
        .unified-nav a.active {
            background-color: #007acc;
            color: white;
            font-weight: 600;
            border-left: 3px solid #007acc;
        }
        #graphiql { 
            flex: 1;
            height: calc(100vh - 50px);
        }
    </style>
</head>
<body>
    <header class="unified-header">
        <div class="header-left">
            <h1>aiwebengine</h1>
        </div>
        <nav class="unified-nav">
            <a href="/engine/docs" title="Documentation">📚 Documentation</a>
            <a href="/engine/editor" title="Code Editor">✏️ Editor</a>
            <a href="/engine/graphql" title="GraphQL API">🔗 GraphiQL</a>
            <a href="/engine/swagger" title="REST API">📖 Swagger</a>
        </nav>
    </header>
    <div id="graphiql">Loading...</div>
    <script crossorigin src="https://unpkg.com/react@18/umd/react.production.min.js"></script>
    <script crossorigin src="https://unpkg.com/react-dom@18/umd/react-dom.production.min.js"></script>
    <script src="https://unpkg.com/graphiql@3/graphiql.min.js"></script>
    <script src="https://unpkg.com/graphql-ws@5/umd/graphql-ws.min.js"></script>
    <script>
        document.addEventListener('DOMContentLoaded', function() {
            // Create WebSocket client for subscriptions
            const wsProtocol = window.location.protocol === 'https:' ? 'wss:' : 'ws:';
            const wsUrl = wsProtocol + '//' + window.location.host + '/graphql/ws';
            
            let wsClient = null;
            
            // Helper function to detect if query is a subscription
            function isSubscriptionOperation(query) {
                if (!query) return false;
                // Remove all comment lines and normalize whitespace
                const normalized = query
                    .split('\\n')
                    .map(line => line.trim())
                    .filter(line => !line.startsWith('#')) // Remove comment lines
                    .join(' ')
                    .replace(/\\s+/g, ' ') // Normalize whitespace
                    .trim();
                // Check if it contains subscription operation
                return /^subscription[\\s{]/.test(normalized) || /\\bsubscription\\s+\\w+\\s*\\{/.test(normalized);
            }
            
            // Create a fetcher that handles both HTTP and WebSocket
            const fetcher = function(graphQLParams, fetcherOpts) {
                // Check if this is a subscription
                const isSubscription = isSubscriptionOperation(graphQLParams.query);
                
                if (isSubscription) {
                    // Use WebSocket for subscriptions - return an async iterable
                    if (!wsClient) {
                        wsClient = graphqlWs.createClient({
                            url: wsUrl,
                        });
                    }
                    
                    // Return an async iterable that GraphiQL expects
                    return {
                        [Symbol.asyncIterator]: function() {
                            const values = [];
                            let resolve = null;
                            let reject = null;
                            let done = false;
                            
                            const unsubscribe = wsClient.subscribe(
                                {
                                    query: graphQLParams.query,
                                    variables: graphQLParams.variables,
                                    operationName: graphQLParams.operationName,
                                },
                                {
                                    next: function(data) {
                                        if (resolve) {
                                            resolve({ value: data, done: false });
                                            resolve = null;
                                        } else {
                                            values.push(data);
                                        }
                                    },
                                    error: function(error) {
                                        if (reject) {
                                            reject(error);
                                        }
                                        done = true;
                                    },
                                    complete: function() {
                                        done = true;
                                        if (resolve) {
                                            resolve({ done: true });
                                        }
                                    }
                                }
                            );
                            
                            if (fetcherOpts && fetcherOpts.signal) {
                                fetcherOpts.signal.addEventListener('abort', function() {
                                    unsubscribe();
                                    done = true;
                                    if (resolve) {
                                        resolve({ done: true });
                                    }
                                });
                            }
                            
                            return {
                                next: function() {
                                    if (values.length > 0) {
                                        return Promise.resolve({ value: values.shift(), done: false });
                                    }
                                    if (done) {
                                        return Promise.resolve({ done: true });
                                    }
                                    return new Promise(function(res, rej) {
                                        resolve = res;
                                        reject = rej;
                                    });
                                },
                                return: function() {
                                    unsubscribe();
                                    return Promise.resolve({ done: true });
                                }
                            };
                        }
                    };
                } else {
                    // Use HTTP for queries and mutations
                    return fetch('/graphql', {
                        method: 'POST',
                        headers: {
                            'Content-Type': 'application/json',
                        },
                        body: JSON.stringify(graphQLParams),
                        signal: fetcherOpts ? fetcherOpts.signal : undefined,
                    }).then(function(response) {
                        return response.json();
                    });
                }
            };

            const root = ReactDOM.createRoot(document.getElementById('graphiql'));
            root.render(React.createElement(GraphiQL, { 
                fetcher: fetcher,
                defaultTheme: 'dark'
            }));

            // Active state detection for unified navigation
            const path = window.location.pathname;
            document.querySelectorAll('.unified-nav a').forEach(function(link) {
                const linkPath = new URL(link.href, window.location.origin).pathname;
                if (path.startsWith(linkPath)) {
                    link.classList.add('active');
                }
            });
        });
    </script>
</body>
</html>`;

  return {
    status: 200,
    body: html,
    contentType: "text/html; charset=UTF-8",
  };
}

// Serve Swagger UI interface
function serveSwaggerUI(context) {
  const req = getRequest(context);

  // Serve Swagger UI using CDN
  const html = `<!DOCTYPE html>
<html lang="en">
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <title>aiwebengine API Documentation</title>
    <link rel="stylesheet" href="https://cdn.jsdelivr.net/npm/swagger-ui-dist@5/swagger-ui.css" />
    <style>
        body { 
            margin: 0; 
            padding: 0;
            display: flex;
            flex-direction: column;
            height: 100vh;
            background: #1e1e1e;
        }
        .unified-header {
            display: flex;
            justify-content: space-between;
            align-items: center;
            padding: 10px 20px;
            background-color: #252526;
            border-bottom: 1px solid #3e3e42;
            min-height: 50px;
            flex-shrink: 0;
        }
        .unified-header h1 {
            font-size: 18px;
            font-weight: 600;
            color: #cccccc;
            margin: 0;
            line-height: 1;
            font-family: 'Segoe UI', Tahoma, Geneva, Verdana, sans-serif;
        }
        .unified-nav {
            display: flex;
            gap: 5px;
        }
        .unified-nav a {
            color: #999999;
            text-decoration: none;
            font-size: 12px;
            padding: 6px 12px;
            border-radius: 4px;
            transition: all 0.2s ease;
            display: flex;
            align-items: center;
            gap: 4px;
            font-family: 'Segoe UI', Tahoma, Geneva, Verdana, sans-serif;
        }
        .unified-nav a:hover {
            background-color: #37373d;
            color: #007acc;
        }
        .unified-nav a.active {
            background-color: #007acc;
            color: white;
            font-weight: 600;
            border-left: 3px solid #007acc;
        }
        #swagger-ui { 
            flex: 1;
            overflow-y: auto;
            max-width: 1460px; 
            margin: 0 auto;
            width: 100%;
        }
    </style>
</head>
<body>
    <header class="unified-header">
        <div class="header-left">
            <h1>aiwebengine</h1>
        </div>
        <nav class="unified-nav">
            <a href="/engine/docs" title="Documentation">📚 Documentation</a>
            <a href="/engine/editor" title="Code Editor">✏️ Editor</a>
            <a href="/engine/graphql" title="GraphQL API">🔗 GraphiQL</a>
            <a href="/engine/swagger" title="REST API">📖 Swagger</a>
        </nav>
    </header>
    <div id="swagger-ui"></div>
    <script src="https://cdn.jsdelivr.net/npm/swagger-ui-dist@5/swagger-ui-bundle.js"></script>
    <script src="https://cdn.jsdelivr.net/npm/swagger-ui-dist@5/swagger-ui-standalone-preset.js"></script>
    <script>
        document.addEventListener('DOMContentLoaded', function() {
            SwaggerUIBundle({
                url: '/engine/openapi.json',
                dom_id: '#swagger-ui',
                deepLinking: true,
                presets: [SwaggerUIBundle.presets.apis, SwaggerUIStandalonePreset],
                layout: "StandaloneLayout",
                theme: "dark"
            });

            // Active state detection for unified navigation
            const path = window.location.pathname;
            document.querySelectorAll('.unified-nav a').forEach(function(link) {
                const linkPath = new URL(link.href, window.location.origin).pathname;
                if (path.startsWith(linkPath)) {
                    link.classList.add('active');
                }
            });
        });
    </script>
</body>
</html>`;

  return {
    status: 200,
    body: html,
    contentType: "text/html; charset=UTF-8",
  };
}

// API: List all scripts
function apiListScripts(context) {
  const req = getRequest(context);
  try {
    const scriptsJson =
      typeof scriptStorage !== "undefined" &&
      typeof scriptStorage.listScripts === "function"
        ? scriptStorage.listScripts()
        : "[]";

    const scriptMetadata = JSON.parse(scriptsJson);

    // Get current user ID from request
    const currentUserId = req.auth && req.auth.userId ? req.auth.userId : null;

    // Sort scripts alphabetically (case-insensitive)
    scriptMetadata.sort((a, b) =>
      a.uri.toLowerCase().localeCompare(b.uri.toLowerCase()),
    );

    const canTogglePrivileged =
      typeof scriptStorage !== "undefined" &&
      typeof scriptStorage.canManageScriptPrivileges === "function"
        ? !!scriptStorage.canManageScriptPrivileges()
        : false;

    const scriptDetails = scriptMetadata.map((meta) => {
      // Get owners for this script
      let owners = [];
      let isOwner = false;
      if (
        typeof scriptStorage !== "undefined" &&
        typeof scriptStorage.getScriptOwners === "function"
      ) {
        try {
          const ownersJson = scriptStorage.getScriptOwners(meta.uri);
          owners = JSON.parse(ownersJson || "[]");
          isOwner = currentUserId && owners.includes(currentUserId);
        } catch (e) {
          console.log(
            "Error getting owners for " + meta.uri + ": " + e.message,
          );
        }
      }

      return {
        uri: meta.uri,
        displayName: meta.name || meta.uri,
        size: meta.size || 0,
        lastModified: new Date(meta.updatedAt || Date.now()).toISOString(),
        privileged: !!meta.privileged,
        defaultPrivileged: getSecurityField(meta.uri, "default_privileged"),
        owners: owners,
        isOwner: isOwner,
        ownerCount: owners.length,
      };
    });

    return {
      status: 200,
      body: JSON.stringify({
        scripts: scriptDetails,
        permissions: {
          canTogglePrivileged,
        },
      }),
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

function getSecurityField(scriptUri, field) {
  if (
    typeof scriptStorage === "undefined" ||
    typeof scriptStorage.getScriptSecurityProfile !== "function"
  ) {
    return field === "default_privileged" ? false : false;
  }

  try {
    const profileJson = scriptStorage.getScriptSecurityProfile(scriptUri);
    if (!profileJson) {
      return false;
    }
    const profile = JSON.parse(profileJson);
    if (field === "privileged") {
      return !!profile.privileged;
    }
    if (field === "default_privileged") {
      return !!profile.default_privileged;
    }
    return false;
  } catch (err) {
    console.log(
      "Failed to parse security profile for " + scriptUri + ": " + err.message,
    );
    return false;
  }
}

// API: Get script content
function apiGetScript(context) {
  const req = getRequest(context);
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

    if (
      typeof scriptStorage !== "undefined" &&
      typeof scriptStorage.getScript === "function"
    ) {
      content = scriptStorage.getScript(fullUri) || "";
    } else {
      return {
        status: 500,
        body: "scriptStorage.getScript function not available",
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
function apiSaveScript(context) {
  const req = getRequest(context);
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

    if (
      typeof scriptStorage !== "undefined" &&
      typeof scriptStorage.upsertScript === "function"
    ) {
      // Check if script already exists to determine action
      const existingScript = scriptStorage.getScript
        ? scriptStorage.getScript(fullUri)
        : null;
      const action = existingScript ? "updated" : "inserted";

      scriptStorage.upsertScript(fullUri, req.body);

      // Broadcast the script update notification
      if (
        typeof routeRegistry !== "undefined" &&
        typeof routeRegistry.sendStreamMessage === "function"
      ) {
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
          routeRegistry.sendStreamMessage(
            "/script_updates",
            JSON.stringify(message),
          );
          console.log(
            "Broadcasted script update from editor: " + action + " " + fullUri,
          );

          // Also send to GraphQL subscription
          if (
            typeof graphQLRegistry !== "undefined" &&
            typeof graphQLRegistry.sendSubscriptionMessage === "function"
          ) {
            try {
              graphQLRegistry.sendSubscriptionMessage(
                "scriptUpdates",
                JSON.stringify(message),
              );
              console.log("Sent update to GraphQL subscription: scriptUpdates");
            } catch (graphqlError) {
              console.log(
                "Failed to send to GraphQL subscription: " +
                  graphqlError.message,
              );
            }
          }
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
function apiDeleteScript(context) {
  const req = getRequest(context);
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

    if (
      typeof scriptStorage !== "undefined" &&
      typeof scriptStorage.deleteScript === "function"
    ) {
      const deleted = scriptStorage.deleteScript(fullUri);

      if (deleted) {
        // Broadcast the script removal notification
        if (
          typeof routeRegistry !== "undefined" &&
          typeof routeRegistry.sendStreamMessage === "function"
        ) {
          try {
            const message = {
              type: "script_update",
              uri: fullUri,
              action: "removed",
              timestamp: new Date().toISOString(),
              via: "editor",
            };
            routeRegistry.sendStreamMessage(
              "/script_updates",
              JSON.stringify(message),
            );
            console.log("Broadcasted script deletion from editor: " + fullUri);

            // Also send to GraphQL subscription
            if (
              typeof graphQLRegistry !== "undefined" &&
              typeof graphQLRegistry.sendSubscriptionMessage === "function"
            ) {
              try {
                graphQLRegistry.sendSubscriptionMessage(
                  "scriptUpdates",
                  JSON.stringify(message),
                );
                console.log(
                  "Sent deletion to GraphQL subscription: scriptUpdates",
                );
              } catch (graphqlError) {
                console.log(
                  "Failed to send to GraphQL subscription: " +
                    graphqlError.message,
                );
              }
            }
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
          error: "scriptStorage.deleteScript function not available",
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

// API: Update privileged flag
function apiUpdateScriptPrivilege(context) {
  const req = getRequest(context);
  try {
    if (
      typeof scriptStorage === "undefined" ||
      typeof scriptStorage.setScriptPrivileged !== "function"
    ) {
      return {
        status: 500,
        body: JSON.stringify({ error: "Privilege API unavailable" }),
        contentType: "application/json",
      };
    }

    // Expect path /api/script-security/<script>
    let scriptName = req.path.replace("/api/script-security/", "");
    scriptName = decodeURIComponent(scriptName);

    let fullUri;
    if (scriptName.startsWith("https://")) {
      fullUri = scriptName;
    } else {
      fullUri = "https://example.com/" + scriptName;
    }

    let payload = {};
    if (req.body) {
      try {
        payload = JSON.parse(req.body);
      } catch (err) {
        return {
          status: 400,
          body: JSON.stringify({ error: "Invalid JSON payload" }),
          contentType: "application/json",
        };
      }
    }

    if (typeof payload.privileged !== "boolean") {
      return {
        status: 400,
        body: JSON.stringify({ error: "privileged must be a boolean" }),
        contentType: "application/json",
      };
    }

    try {
      scriptStorage.setScriptPrivileged(fullUri, payload.privileged);
    } catch (error) {
      const message = error && error.message ? error.message : String(error);
      const forbidden =
        message.includes("Administrator privileges") ||
        message.includes("permission_denied");

      return {
        status: forbidden ? 403 : 500,
        body: JSON.stringify({ error: message }),
        contentType: "application/json",
      };
    }

    return {
      status: 200,
      body: JSON.stringify({
        message: "Script privilege updated",
        privileged: payload.privileged,
        uri: fullUri,
      }),
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

// API: Add script owner
function apiAddScriptOwner(context) {
  const req = getRequest(context);
  try {
    if (
      typeof scriptStorage === "undefined" ||
      typeof scriptStorage.addScriptOwner !== "function"
    ) {
      return {
        status: 500,
        body: JSON.stringify({ error: "Owner API unavailable" }),
        contentType: "application/json",
      };
    }

    // Extract script name from path /api/script-owners/<script>
    let scriptName = req.path.replace("/api/script-owners/", "");
    scriptName = decodeURIComponent(scriptName);

    let fullUri;
    if (scriptName.startsWith("https://")) {
      fullUri = scriptName;
    } else {
      fullUri = "https://example.com/" + scriptName;
    }

    let payload = {};
    if (req.body) {
      try {
        payload = JSON.parse(req.body);
      } catch (err) {
        return {
          status: 400,
          body: JSON.stringify({ error: "Invalid JSON payload" }),
          contentType: "application/json",
        };
      }
    }

    if (!payload.ownerId || typeof payload.ownerId !== "string") {
      return {
        status: 400,
        body: JSON.stringify({
          error: "ownerId is required and must be a string",
        }),
        contentType: "application/json",
      };
    }

    try {
      const result = scriptStorage.addScriptOwner(fullUri, payload.ownerId);
      if (typeof result === "string" && result.startsWith("Error:")) {
        const forbidden = result.includes("Permission denied");
        return {
          status: forbidden ? 403 : 500,
          body: JSON.stringify({ error: result }),
          contentType: "application/json",
        };
      }
    } catch (error) {
      const message = error && error.message ? error.message : String(error);
      const forbidden = message.includes("Permission denied");
      return {
        status: forbidden ? 403 : 500,
        body: JSON.stringify({ error: message }),
        contentType: "application/json",
      };
    }

    return {
      status: 200,
      body: JSON.stringify({
        message: "Owner added successfully",
        ownerId: payload.ownerId,
        uri: fullUri,
      }),
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

// API: Remove script owner
function apiRemoveScriptOwner(context) {
  const req = getRequest(context);
  try {
    if (
      typeof scriptStorage === "undefined" ||
      typeof scriptStorage.removeScriptOwner !== "function"
    ) {
      return {
        status: 500,
        body: JSON.stringify({ error: "Owner API unavailable" }),
        contentType: "application/json",
      };
    }

    // Extract script name from path /api/script-owners/<script>
    let scriptName = req.path.replace("/api/script-owners/", "");
    scriptName = decodeURIComponent(scriptName);

    // Get ownerId from request body (now properly supported for DELETE)
    let payload = {};
    if (req.body) {
      try {
        payload = JSON.parse(req.body);
      } catch (err) {
        return {
          status: 400,
          body: JSON.stringify({ error: "Invalid JSON payload" }),
          contentType: "application/json",
        };
      }
    }

    if (!payload.ownerId || typeof payload.ownerId !== "string") {
      return {
        status: 400,
        body: JSON.stringify({ error: "ownerId is required in request body" }),
        contentType: "application/json",
      };
    }

    const ownerId = payload.ownerId;

    let fullUri;
    if (scriptName.startsWith("https://")) {
      fullUri = scriptName;
    } else {
      fullUri = "https://example.com/" + scriptName;
    }

    try {
      const result = scriptStorage.removeScriptOwner(fullUri, ownerId);
      if (typeof result === "string" && result.startsWith("Error:")) {
        const forbidden =
          result.includes("Permission denied") ||
          result.includes("Cannot remove");
        return {
          status: forbidden ? 403 : 500,
          body: JSON.stringify({ error: result }),
          contentType: "application/json",
        };
      }
    } catch (error) {
      const message = error && error.message ? error.message : String(error);
      const forbidden =
        message.includes("Permission denied") ||
        message.includes("Cannot remove");
      return {
        status: forbidden ? 403 : 500,
        body: JSON.stringify({ error: message }),
        contentType: "application/json",
      };
    }

    return {
      status: 200,
      body: JSON.stringify({
        message: "Owner removed successfully",
        ownerId: ownerId,
        uri: fullUri,
      }),
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
function apiGetLogs(context) {
  const req = getRequest(context);
  try {
    const logsJson =
      typeof console.listLogs === "function" ? console.listLogs() : "[]";
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

// API: Prune logs (DELETE /api/logs)
function apiPruneLogs(context) {
  const req = getRequest(context);
  try {
    const result =
      typeof console.pruneLogs === "function" ? console.pruneLogs() : "";
    // If the console.pruneLogs() returns an error message, respond with 500
    if (typeof result === "string" && result.startsWith("Error:")) {
      return {
        status: 500,
        body: JSON.stringify({ error: result }),
        contentType: "application/json",
      };
    }

    return {
      status: 200,
      body: JSON.stringify({ message: result || "Pruned logs" }),
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
function apiGetAssets(context) {
  const req = getRequest(context);
  try {
    // Check if a script URI is provided in query parameters
    const scriptUri = req.query && req.query.uri ? req.query.uri : null;

    let assetsJson = "[]";
    if (scriptUri) {
      // Use privileged listAssetsForUri if a specific script is requested
      assetsJson =
        typeof assetStorage !== "undefined" &&
        typeof assetStorage.listAssetsForUri === "function"
          ? assetStorage.listAssetsForUri(scriptUri)
          : "[]";
    } else {
      // Fall back to current script's assets
      assetsJson =
        typeof assetStorage !== "undefined" &&
        typeof assetStorage.listAssets === "function"
          ? assetStorage.listAssets()
          : "[]";
    }

    const assetMetadata = JSON.parse(assetsJson);

    // Sort assets alphabetically by name (case-insensitive)
    assetMetadata.sort((a, b) =>
      a.name.toLowerCase().localeCompare(b.name.toLowerCase()),
    );

    const assetDetails = assetMetadata.map((meta) => ({
      uri: meta.uri,
      displayName: meta.name || meta.uri,
      path: meta.uri,
      size: meta.size || 0,
      type: meta.mimetype || getMimeTypeFromPath(meta.uri),
      createdAt: new Date(meta.createdAt || Date.now()).toISOString(),
      updatedAt: new Date(meta.updatedAt || Date.now()).toISOString(),
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

// API: Get individual asset
function apiGetAsset(context) {
  const req = getRequest(context);
  try {
    // Check if a script URI is provided in query parameters
    const scriptUri = req.query && req.query.uri ? req.query.uri : null;

    // Extract the asset path from the URL
    let assetPath = req.path.replace("/api/assets", "");

    // URL decode the asset path in case it contains encoded characters
    assetPath = decodeURIComponent(assetPath);

    // Remove leading slash to get asset name (new system uses names, not paths)
    let assetName = assetPath;
    if (assetName.startsWith("/")) {
      assetName = assetName.substring(1);
    }

    if (typeof assetStorage !== "undefined") {
      let assetData;

      if (scriptUri && typeof assetStorage.fetchAssetForUri === "function") {
        // Use privileged fetchAssetForUri for cross-script access
        assetData = assetStorage.fetchAssetForUri(scriptUri, assetName);
      } else if (typeof assetStorage.fetchAsset === "function") {
        // Fall back to current script's asset
        assetData = assetStorage.fetchAsset(assetName);
      }

      if (
        assetData &&
        !assetData.startsWith("Asset '") &&
        !assetData.startsWith("Error:")
      ) {
        // fetchAsset returns base64 encoded content
        // Return it directly in the bodyBase64 field for proper binary handling
        const mimetype = getMimeTypeFromPath(assetPath);

        // Add charset=UTF-8 for text-based MIME types
        let contentType = mimetype;
        if (
          mimetype.startsWith("text/") ||
          mimetype === "application/json" ||
          mimetype === "application/javascript" ||
          mimetype === "application/xml" ||
          mimetype === "image/svg+xml"
        ) {
          contentType = mimetype + "; charset=UTF-8";
        }

        return {
          status: 200,
          bodyBase64: assetData,
          contentType: contentType,
        };
      } else {
        return {
          status: 404,
          body: "Asset not found",
          contentType: "text/plain; charset=UTF-8",
        };
      }
    } else {
      return {
        status: 500,
        body: "fetchAsset function not available",
        contentType: "text/plain; charset=UTF-8",
      };
    }
  } catch (error) {
    return {
      status: 500,
      body: "Error: " + error.message,
      contentType: "text/plain; charset=UTF-8",
    };
  }
}

// Manual base64 decoder
function decodeBase64(base64) {
  const chars =
    "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
  let buffer = [];
  let bits = 0;
  let value = 0;

  for (let i = 0; i < base64.length; i++) {
    const char = base64[i];
    if (char === "=" || char === "\n" || char === "\r" || char === " ")
      continue;

    const charIndex = chars.indexOf(char);
    if (charIndex === -1) {
      throw new Error("Invalid base64 character: " + char);
    }

    value = (value << 6) | charIndex;
    bits += 6;

    if (bits >= 8) {
      bits -= 8;
      buffer.push((value >> bits) & 0xff);
      value &= (1 << bits) - 1;
    }
  }

  return new Uint8Array(buffer);
}

// Helper function to determine MIME type from file path
function getMimeTypeFromPath(path) {
  const ext = path.split(".").pop().toLowerCase();
  const mimeTypes = {
    png: "image/png",
    jpg: "image/jpeg",
    jpeg: "image/jpeg",
    gif: "image/gif",
    svg: "image/svg+xml",
    webp: "image/webp",
    ico: "image/x-icon",
    txt: "text/plain",
    html: "text/html",
    css: "text/css",
    js: "application/javascript",
    json: "application/json",
    pdf: "application/pdf",
    zip: "application/zip",
  };
  return mimeTypes[ext] || "application/octet-stream";
}

// API: Save/upload asset
function apiSaveAsset(context) {
  const req = getRequest(context);
  try {
    // Check if a script URI is provided in query parameters
    const scriptUri = req.query && req.query.uri ? req.query.uri : null;

    const body = JSON.parse(req.body || "{}");
    const { publicPath, mimetype, content } = body;

    if (!publicPath || !content) {
      return {
        status: 400,
        body: JSON.stringify({ error: "Missing publicPath or content" }),
        contentType: "application/json",
      };
    }

    // Extract asset name from path (remove leading slash)
    let assetName = publicPath;
    if (assetName.startsWith("/")) {
      assetName = assetName.substring(1);
    }

    console.log(
      "apiSaveAsset: publicPath '" +
        publicPath +
        "' -> asset name '" +
        assetName +
        "'" +
        (scriptUri ? " for script '" + scriptUri + "'" : ""),
    );

    if (typeof assetStorage !== "undefined") {
      let result;

      if (scriptUri && typeof assetStorage.upsertAssetForUri === "function") {
        // Use privileged upsertAssetForUri for cross-script access
        result = assetStorage.upsertAssetForUri(
          scriptUri,
          assetName,
          mimetype,
          content,
        );
      } else if (typeof assetStorage.upsertAsset === "function") {
        // Fall back to current script's asset
        result = assetStorage.upsertAsset(assetName, mimetype, content);
      } else {
        return {
          status: 500,
          body: JSON.stringify({ error: "upsertAsset function not available" }),
          contentType: "application/json",
        };
      }

      // Check if result indicates an error
      if (result && result.startsWith("Error:")) {
        return {
          status: 500,
          body: JSON.stringify({ error: result }),
          contentType: "application/json",
        };
      }

      return {
        status: 200,
        body: JSON.stringify({ message: "Asset saved successfully" }),
        contentType: "application/json",
      };
    } else {
      return {
        status: 500,
        body: JSON.stringify({ error: "assetStorage not available" }),
        contentType: "application/json",
      };
    }
  } catch (error) {
    return {
      status: 500,
      body: JSON.stringify({ error: error.message }),
      contentType: "application/json",
    };
  }
}

// API: Delete asset
function apiDeleteAsset(context) {
  const req = getRequest(context);
  try {
    // Check if a script URI is provided in query parameters
    const scriptUri = req.query && req.query.uri ? req.query.uri : null;

    // Extract the asset path from the URL
    let assetPath = req.path.replace("/api/assets", "");

    // URL decode the asset path in case it contains encoded characters
    assetPath = decodeURIComponent(assetPath);

    // Remove leading slash to get asset name (new system uses names, not paths)
    let assetName = assetPath;
    if (assetName.startsWith("/")) {
      assetName = assetName.substring(1);
    }

    console.log(
      "apiDeleteAsset: attempting to delete asset: " +
        assetName +
        (scriptUri ? " from script '" + scriptUri + "'" : ""),
    );

    if (typeof assetStorage !== "undefined") {
      let deleted;

      if (scriptUri && typeof assetStorage.deleteAssetForUri === "function") {
        // Use privileged deleteAssetForUri for cross-script access
        deleted = assetStorage.deleteAssetForUri(scriptUri, assetName);
      } else if (typeof assetStorage.deleteAsset === "function") {
        // Fall back to current script's asset
        deleted = assetStorage.deleteAsset(assetName);
      } else {
        console.log("apiDeleteAsset: deleteAsset function not available");
        return {
          status: 500,
          body: JSON.stringify({
            error: "deleteAsset function not available",
          }),
          contentType: "application/json",
        };
      }

      console.log("apiDeleteAsset: deleteAsset returned: " + deleted);

      // Check if result indicates success (starts with "Asset" and contains "deleted")
      if (deleted && deleted.includes("deleted successfully")) {
        console.log("apiDeleteAsset: successfully deleted asset: " + assetName);
        return {
          status: 200,
          body: JSON.stringify({
            message: "Asset deleted successfully",
            path: assetPath,
          }),
          contentType: "application/json",
        };
      } else {
        console.log("apiDeleteAsset: asset not found: " + assetName);
        return {
          status: 404,
          body: JSON.stringify({
            error: "Asset not found",
            message: "No asset with the specified path was found",
            path: assetPath,
          }),
          contentType: "application/json",
        };
      }
    } else {
      console.log("apiDeleteAsset: assetStorage not available");
      return {
        status: 500,
        body: JSON.stringify({
          error: "assetStorage not available",
        }),
        contentType: "application/json",
      };
    }
  } catch (error) {
    console.log("apiDeleteAsset: error - " + error.message);
    return {
      status: 500,
      body: JSON.stringify({
        error: "Failed to delete asset",
        details: error.message,
      }),
      contentType: "application/json",
    };
  }
}

// API: List all registered routes
function apiListRoutes(context) {
  const req = getRequest(context);
  try {
    const routes =
      typeof routeRegistry !== "undefined" &&
      typeof routeRegistry.listRoutes === "function"
        ? routeRegistry.listRoutes()
        : "[]";
    // Parse and re-stringify to ensure valid JSON
    const routesData = JSON.parse(routes);

    // Sort routes alphabetically by path (case-insensitive)
    routesData.sort((a, b) =>
      a.path.toLowerCase().localeCompare(b.path.toLowerCase()),
    );

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
function apiAIAssistant(context) {
  const req = getRequest(context);
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
  const currentAsset = body.currentAsset || null;
  const currentAssetContent = body.currentAssetContent || null;

  // Check if Anthropic API key is configured
  if (!secretStorage.exists("anthropic_api_key")) {
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
1. routeRegistry - Object containing all HTTP route and stream-related functions:
   
   routeRegistry.registerRoute(path, handlerName, method) - Register HTTP routes
   - path: string (e.g., "/api/users" or "/hello")
   - handlerName: string (name of your handler function)
   - method: "GET" | "POST" | "PUT" | "DELETE"

   routeRegistry.registerStreamRoute(path) - Register SSE (Server-Sent Events) stream endpoint
   - path: string (must start with /)
   - Returns: string describing registration result

   routeRegistry.registerAssetRoute(assetPath) - Register static asset for serving
   - assetPath: string (path to asset file)
   - Returns: string describing registration result

   routeRegistry.sendStreamMessage(path, data) - Broadcast message to all connections on a stream path
   - path: string (must start with /)
   - data: object (will be JSON serialized)
   - Returns: string describing broadcast result

   routeRegistry.sendStreamMessageFiltered(path, data, filterJson) - Send message to filtered connections based on metadata
   - path: string (must start with /)
   - data: object (will be JSON serialized)
   - filterJson: string (optional JSON string with metadata filter criteria, empty "{}" matches all)
   - Returns: string describing broadcast result with success/failure counts
   - Use for personalized broadcasting to specific users/groups on stable endpoints

   routeRegistry.listRoutes() - List all registered HTTP routes
   - Returns: JSON string with array of route metadata

   routeRegistry.listStreams() - List all registered stream endpoints
   - Returns: JSON string with array of [{path: string, uri: string}]

   routeRegistry.listAssets() - List all registered asset paths
   - Returns: JSON string with array of asset names

2. Console logging - Write messages to server logs and retrieve log entries
   - console.log(message) - General logging (level: LOG)
   - console.debug(message) - Debug-level logging (level: DEBUG)
   - console.info(message) - Informational logging (level: INFO)
   - console.warn(message) - Warning-level logging (level: WARN)
   - console.error(message) - Error-level logging (level: ERROR)
   - console.listLogs() - Retrieve all log entries as JSON string (returns array of {message, level, timestamp})
   - console.listLogsForUri(uri) - Retrieve log entries for specific script URI as JSON string
   - message: string
   - uri: string (script URI)

3. sharedStorage - Persistent key-value storage per script
   - sharedStorage.getItem(key) - Get stored value (returns string or null)
   - sharedStorage.setItem(key, value) - Store key-value pair (returns success message)
   - sharedStorage.removeItem(key) - Delete key-value pair (returns boolean)
   - sharedStorage.clear() - Remove all data for this script (returns success message)
   - Each script has its own isolated storage namespace
   - Data persists across requests and server restarts (when PostgreSQL configured)

4. personalStorage - Persistent key-value storage per script per user
   - personalStorage.getItem(key) - Get stored value for current user (returns string or null)
   - personalStorage.setItem(key, value) - Store key-value pair for current user (returns success message)
   - personalStorage.removeItem(key) - Delete key-value pair for current user (returns boolean)
   - personalStorage.clear() - Remove all data for current user in this script (returns success message)
   - Each authenticated user has their own isolated storage namespace within each script
   - REQUIRES AUTHENTICATION: All methods return errors/null when user is not logged in
   - User ID is handled transparently by the engine - scripts never see user IDs directly
   - Data persists across requests and server restarts (when PostgreSQL configured)
   - Use for: user preferences, shopping carts, personalized settings, per-user state

5. secretStorage - Read-only access to check secret availability
   - secretStorage.exists(identifier) - Check if a secret exists (returns boolean)
   - secretStorage.list() - List all secret identifiers (returns array of strings)
   - SECURITY: Secret values are NEVER exposed to JavaScript
   - Use {{secret:identifier}} syntax in fetch() headers to inject secret values
   - identifier: string (secret name)

6. convert - Markdown to HTML conversion functions
   - convert.markdown_to_html(markdown) - Convert markdown string to HTML
   - markdown: string (markdown content, max 1MB)
   - Returns: string (HTML output or error message starting with "Error:")
   - Supports: headings, lists, code blocks, tables, bold, italic, links, strikethrough
   - Use for: rendering blog posts, documentation, user content in HTML responses
   - Example: const html = convert.markdown_to_html('# Hello\\n\\nThis is **bold**');

   - convert.render_handlebars_template(template, data) - Render Handlebars template with data
   - template: string (Handlebars template content, max 1MB)
   - data: string (JSON string representing data object to populate template variables)
   - Returns: string (rendered template output or error message starting with "Error:")
   - Supports: variables {{variable}}, loops {{#each items}}, conditionals {{#if condition}}
   - Use for: dynamic HTML generation, email templates, configuration files
   - Example: const html = convert.render_handlebars_template('<h1>{{title}}</h1><p>{{content}}</p>', '{"title": "Hello", "content": "World"}');

7. fetch(url, options) - Make HTTP requests to external APIs
   - url: string
   - options: JSON string with {method, headers, body, timeout_ms}
   - Supports {{secret:identifier}} in headers for secure API keys
   - Returns: JSON string with {status, ok, headers, body}

8. graphQLRegistry - Object containing all GraphQL-related functions:
   
   graphQLRegistry.registerQuery(name, schema, resolverName) - Register GraphQL query
   - name: string (query name)
   - schema: string (GraphQL schema definition)
   - resolverName: string (name of resolver function)

   graphQLRegistry.registerMutation(name, schema, resolverName) - Register GraphQL mutation
   - name: string (mutation name)
   - schema: string (GraphQL schema definition)
   - resolverName: string (name of resolver function)

   graphQLRegistry.registerSubscription(name, schema, resolverName) - Register GraphQL subscription
   - name: string (subscription name)
   - schema: string (GraphQL schema definition)
   - resolverName: string (name of resolver function)

   graphQLRegistry.executeGraphQL(query, variables) - Execute GraphQL query
   - query: string (GraphQL query string)
   - variables: string (optional JSON string of variables)
   - Returns: JSON string with GraphQL response

   graphQLRegistry.sendSubscriptionMessage(subscriptionName, data) - Broadcast to all GraphQL subscription connections
   - subscriptionName: string (name of subscription)
   - data: string (JSON string to send to subscribers)
   - Returns: string describing broadcast result

   graphQLRegistry.sendSubscriptionMessageFiltered(subscriptionName, data, filterJson) - Send to filtered GraphQL subscription connections
   - subscriptionName: string (name of subscription)
   - data: string (JSON string to send to subscribers)
   - filterJson: string (optional JSON string with metadata filter criteria, empty "{}" matches all)
   - Returns: string describing broadcast result with success/failure counts

8. mcpRegistry - Model Context Protocol (MCP) tool registry

   mcpRegistry.registerTool(name, description, inputSchemaJson, handlerName) - Register an MCP tool
   - name: string (tool name, unique identifier)
   - description: string (what the tool does)
   - inputSchemaJson: string (JSON Schema defining tool parameters as JSON string)
   - handlerName: string (name of JavaScript function that handles tool execution)
   
   The handler function receives context with context.args containing the tool arguments.
   It should return a JSON string with the tool result.

   Example:
   mcpRegistry.registerTool(
     "getCurrentTime",
     "Get current time in specified timezone",
     JSON.stringify({
       type: "object",
       properties: {
         timezone: { type: "string", description: "IANA timezone", default: "UTC" }
       }
     }),
     "getCurrentTimeHandler"
   );

   function getCurrentTimeHandler(context) {
     const timezone = context.args.timezone || "UTC";
     return JSON.stringify({
       timestamp: new Date().toISOString(),
       timezone: timezone
     });
   }

   MCP tools are accessible at:
   - GET /mcp/tools/list - List all registered tools
   - POST /mcp/tools/call - Execute a tool with { "name": "toolName", "arguments": {...} }

9. schedulerService - Background job scheduler

  schedulerService.registerOnce({ handler, runAt, name? }) - Schedule a single execution
  - handler: string (function name)
  - runAt: string (UTC ISO-8601 timestamp ending with Z)
  - name: string (optional identifier used to overwrite existing jobs)

  schedulerService.registerRecurring({ handler, intervalMinutes, startAt?, name? }) - Schedule recurring execution
  - handler: string
  - intervalMinutes: integer >= 1
  - startAt: string (optional UTC timestamp for first run)
  - name: string (optional identifier)

  schedulerService.clearAll() - Remove every scheduled job for the current script

  Scheduled handlers run with admin privileges and receive context.meta.schedule containing jobId, name, type (one-off/recurring), scheduledFor (UTC timestamp), and intervalSeconds (null for one-off jobs).

10. dispatcher - Inter-script message passing for event-driven communication

  dispatcher.registerListener(messageType, handlerName) - Register handler for message type
  - messageType: string (event identifier like 'user:created', 'order:completed')
  - handlerName: string (name of handler function in this script)
  
  dispatcher.sendMessage(messageType, messageData) - Broadcast message to all registered listeners
  - messageType: string (event identifier)
  - messageData: string (JSON string with event data)
  
  Message handlers receive context.messageType and context.messageData. Use dispatcher for event-driven coordination between scripts without tight coupling. Example: user-service.js sends 'user:created' message, email-notifications.js and analytics.js both listen and react independently.

RESPONSE FORMAT - YOU MUST RESPOND WITH ONLY THIS JSON STRUCTURE:

For scripts:
{
  "type": "explanation" | "create_script" | "edit_script" | "delete_script",
  "message": "Human-readable explanation",
  "script_name": "name.js",
  "code": "complete JavaScript code",
  "original_code": "original code (for edits only)"
}

For assets (CSS, SVG, HTML, JSON, etc.):
{
  "type": "explanation" | "create_asset" | "edit_asset" | "delete_asset",
  "message": "Human-readable explanation",
  "asset_path": "/path/to/file.css",
  "code": "complete file content",
  "original_code": "original content (for edits only)"
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

function handlerName(context) {
  const req = getRequest(context);
  // req has: path, method, headers, query, params, form, body, auth
  try {
    // Generate your response using Response builders
    return ResponseBuilder.html('<h1>Hello World</h1>');
  } catch (error) {
    console.error('Error: ' + error);
    return ResponseBuilder.error('Internal error', 500);
  }
}

function init(context) {
  console.log('Initializing script');
  routeRegistry.registerRoute('/your-path', 'handlerName', 'GET');
  return { success: true };
}

AVAILABLE RESPONSE BUILDERS:
- ResponseBuilder.json(data) - Return JSON response (status 200)
- ResponseBuilder.html(html) - Return HTML response (status 200)
- ResponseBuilder.text(text) - Return plain text response (status 200)
- ResponseBuilder.error(message, status) - Return error response with custom status
- ResponseBuilder.redirect(url, status) - Return redirect response (default status 302)

AVAILABLE VALIDATION HELPERS:
- validate.requireQueryParam(name) - Require query parameter, throws error if missing
- validate.requireFormParam(name) - Require form parameter, throws error if missing
- validate.requireAuth() - Require authentication, throws error if not logged in
- validate.requireRole(role) - Require specific role, throws error if not authorized

USER OBJECT ACCESS:
- context.request.auth.user - Current authenticated user object (null if not logged in)
- context.request.auth.user.id - User ID
- context.request.auth.user.email - User email
- context.request.auth.user.roles - Array of user roles

IMPORTANT CONCEPTS:
1. Scripts are SERVER-SIDE JavaScript that handle HTTP requests
2. Use Response builders instead of manual response objects
3. Use validation helpers for input validation and authentication
4. Access user data through context.request.auth.user
5. Scripts don't have access to browser APIs or Node.js APIs
6. Use fetch() to call external APIs
7. Use routeRegistry.registerRoute() in init() to map URLs to handler functions
8. For real-time features, use routeRegistry.registerStreamRoute() and routeRegistry.sendStreamMessage()
9. For personalized broadcasting, use routeRegistry.sendStreamMessageFiltered() with metadata filters
10. Selective broadcasting enables chat apps and user-specific notifications without dynamic endpoints

RULES:
1. ALWAYS respond with ONLY valid JSON - no other text
2. Include complete, working JavaScript code
3. Use try-catch blocks in all handlers
4. ALWAYS include init() function that calls at least one registration function:
   - For HTTP services: routeRegistry.registerRoute() or routeRegistry.registerStreamRoute()
   - For GraphQL services: graphQLRegistry.registerQuery(), graphQLRegistry.registerMutation(), or graphQLRegistry.registerSubscription()
   - A script may use multiple registration types
5. Use Response builders (ResponseBuilder.json(), ResponseBuilder.html(), ResponseBuilder.text(), ResponseBuilder.error()) instead of manual response objects
6. Use validation helpers (validate.requireQueryParam(), validate.requireFormParam(), validate.requireAuth()) for input validation
7. Access user data through context.request.auth.user (not auth.currentUser())
8. Use console.log() for debugging
9. For edits, include both original_code and code fields
10. Never use Node.js APIs (fs, path, etc.) - they don't exist here
11. Never use browser APIs (localStorage, document, window) - they don't exist here
12. For external API keys, use {{secret:identifier}} in fetch headers
13. Escape all special characters in JSON strings

EXAMPLES OF CORRECT RESPONSES:

Example 1 - Create web page:
{"type":"create_script","message":"Creating a script that serves an HTML page","script_name":"hello-page.js","code":"// Hello page\\n\\nfunction servePage(context) {\\n  return ResponseBuilder.html('<!DOCTYPE html><html><head><title>Hello</title></head><body><h1>Hello World!</h1></body></html>');\\n}\\n\\nfunction init(context) {\\n  routeRegistry.registerRoute('/hello', 'servePage', 'GET');\\n  return { success: true };\\n}"}

Example 2 - Create JSON API:
{"type":"create_script","message":"Creating a REST API endpoint","script_name":"users-api.js","code":"// Users API\\n\\nfunction getUsers(context) {\\n  const users = [{id: 1, name: 'Alice'}, {id: 2, name: 'Bob'}];\\n  return ResponseBuilder.json(users);\\n}\\n\\nfunction init(context) {\\n  routeRegistry.registerRoute('/api/users', 'getUsers', 'GET');\\n  return { success: true };\\n}"}

Example 3 - Explanation:
{"type":"explanation","message":"This script registers a GET endpoint that returns JSON user data using ResponseBuilder.json() builder."}

Example 4 - Selective Broadcasting Chat:
{"type":"create_script","message":"Creating a chat application with selective broadcasting for personalized messages","script_name":"chat-app.js","code":"// Chat Application with Selective Broadcasting\\n\\n// Register one stream for all chat messages\\nfunction init(context) {\\n  routeRegistry.registerStreamRoute('/chat');\\n  routeRegistry.registerRoute('/chat/send', 'sendMessage', 'POST');\\n  routeRegistry.registerRoute('/chat/personal', 'sendPersonalMessage', 'POST');\\n  return { success: true };\\n}\\n\\n// Send message to specific room\\nfunction sendMessage(context) {\\n  const req = getRequest(context);\\n  const { room, message, sender } = req.form;\\n  \\n  const result = routeRegistry.sendStreamMessageFiltered('/chat', {\\n    type: 'room_message',\\n    room: room,\\n    message: message,\\n    sender: sender,\\n    timestamp: new Date().toISOString()\\n  }, JSON.stringify({ room: room }));\\n  \\n  return ResponseBuilder.json({ success: true, result: result });\\n}\\n\\n// Send personal message to specific user\\nfunction sendPersonalMessage(context) {\\n  const req = getRequest(context);\\n  const { targetUser, message, sender } = req.form;\\n  \\n  const result = routeRegistry.sendStreamMessageFiltered('/chat', {\\n    type: 'personal_message',\\n    message: message,\\n    sender: sender,\\n    timestamp: new Date().toISOString()\\n  }, JSON.stringify({ user_id: targetUser }));\\n  \\n  return ResponseBuilder.json({ success: true, result: result });\\n}"}

Example 5 - GraphQL Subscription with Selective Broadcasting:
{"type":"create_script","message":"Creating a GraphQL subscription with selective broadcasting for personalized notifications","script_name":"notification-subscription.js","code":"// GraphQL Subscription with Selective Broadcasting\\n\\nfunction init(context) {\\n  graphQLRegistry.registerSubscription(\\n    'userNotifications',\\n    'type Subscription { userNotifications: String }',\\n    'userNotificationsResolver'\\n  );\\n  routeRegistry.registerRoute('/notify/user', 'sendUserNotification', 'POST');\\n  return { success: true };\\n}\\n\\nfunction userNotificationsResolver() {\\n  return 'User notifications subscription active';\\n}\\n\\nfunction sendUserNotification(context) {\\n  const req = getRequest(context);\\n  const { userId, message, type } = req.form;\\n  \\n  const result = graphQLRegistry.sendSubscriptionMessageFiltered('userNotifications', {\\n    type: type || 'notification',\\n    message: message,\\n    timestamp: new Date().toISOString()\\n  }, JSON.stringify({ user_id: userId }));\\n  \\n  return ResponseBuilder.json({ success: true, result: result });\\n}"}

Example 6 - Create CSS file:
{"type":"create_asset","message":"Creating a custom stylesheet","asset_path":"/styles/custom.css","code":":root {\\n  --primary-color: #007acc;\\n  --secondary-color: #5a5a5a;\\n}\\n\\nbody {\\n  font-family: 'Arial', sans-serif;\\n  color: var(--secondary-color);\\n}\\n\\n.button {\\n  background-color: var(--primary-color);\\n  color: white;\\n  padding: 10px 20px;\\n  border: none;\\n  border-radius: 4px;\\n  cursor: pointer;\\n}\\n\\n.button:hover {\\n  opacity: 0.9;\\n}"}

Example 7 - Create SVG icon:
{"type":"create_asset","message":"Creating a simple SVG icon","asset_path":"/icons/check.svg","code":"<svg xmlns=\\"http://www.w3.org/2000/svg\\" viewBox=\\"0 0 24 24\\" width=\\"24\\" height=\\"24\\">\\n  <path fill=\\"#28a745\\" d=\\"M9 16.17L4.83 12l-1.42 1.41L9 19 21 7l-1.41-1.41z\\"/>\\n</svg>"}

Example 8 - Edit CSS file:
{"type":"edit_asset","message":"Adding dark mode support to existing CSS","asset_path":"/styles/main.css","original_code":".container {\\n  background: white;\\n  color: black;\\n}","code":".container {\\n  background: white;\\n  color: black;\\n}\\n\\n@media (prefers-color-scheme: dark) {\\n  .container {\\n    background: #1e1e1e;\\n    color: #ffffff;\\n  }\\n}"}

Example 9 - Using validation helpers and user object:
{"type":"create_script","message":"Creating a protected API that requires authentication and validates input","script_name":"protected-api.js","code":"// Protected API with validation\\n\\nfunction getProfile(context) {\\n  validate.requireAuth();\\n  const userId = validate.requireQueryParam('userId');\\n  \\n  const user = context.request.auth.user;\\n  if (user.id !== userId && !user.roles.includes('admin')) {\\n    return ResponseBuilder.error('Access denied', 403);\\n  }\\n  \\n  return ResponseBuilder.json({ id: user.id, email: user.email, roles: user.roles });\\n}\\n\\nfunction init(context) {\\n  routeRegistry.registerRoute('/api/profile', 'getProfile', 'GET');\\n  return { success: true };\\n}"}

Example 10 - Form handling with validation:
{"type":"create_script","message":"Creating a contact form with validation","script_name":"contact-form.js","code":"// Contact Form with Validation\\\\n\\\\nfunction submitContact(context) {\\\\n  const req = getRequest(context);\\\\n  const name = validate.requireFormParam('name');\\\\n  const email = validate.requireFormParam('email');\\\\n  const message = validate.requireFormParam('message');\\\\n  \\\\n  // Process the form data...\\\\n  console.log(\\\`Contact from \\\${name} (\\\${email}): \\\${message}\\\`);\\\\n  \\\\n  return ResponseBuilder.html('<h1>Thank you for your message!</h1><p>We will get back to you soon.</p>');\\\\n}\\\\n\\\\nfunction init(context) {\\\\n  routeRegistry.registerRoute('/contact', 'submitContact', 'POST');\\\\n  return { success: true };\\\\n}"}

ASSET CREATION GUIDELINES:
- For CSS files: Use modern CSS features (variables, flexbox, grid), include proper formatting
- For SVG files: Use clean, optimized SVG code with proper xmlns attribute
- For JSON files: Ensure valid JSON structure with proper formatting
- For HTML files: Use semantic HTML5 markup
- For Markdown files: Use proper markdown syntax
- Always use the asset_path field (not script_name) for asset operations
- Asset paths should start with / (e.g., "/styles/main.css", "/icons/logo.svg")

Remember: You are creating JavaScript scripts that run on the SERVER and handle HTTP requests. When someone asks for a "web page", you create a script that SERVES that HTML page using ResponseBuilder.html()! For styling, images, or static content, create assets instead of scripts. Use Response builders for all responses, validation helpers for input checking, and context.request.auth.user for user data.`;

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

  // Add context about current asset if available
  if (currentAsset && currentAssetContent) {
    contextualPrompt += "CURRENT ASSET CONTEXT:\\n";
    contextualPrompt += "Asset Path: " + currentAsset + "\\n";

    // Determine file type for appropriate code fence
    let fileType = "text";
    if (currentAsset.endsWith(".css")) fileType = "css";
    else if (currentAsset.endsWith(".svg") || currentAsset.endsWith(".xml"))
      fileType = "xml";
    else if (currentAsset.endsWith(".json")) fileType = "json";
    else if (currentAsset.endsWith(".html")) fileType = "html";
    else if (currentAsset.endsWith(".md")) fileType = "markdown";
    else if (currentAsset.endsWith(".js")) fileType = "javascript";

    contextualPrompt +=
      "Asset Content:\\n```" +
      fileType +
      "\\n" +
      currentAssetContent +
      "\\n```\\n\\n";
  }

  // Add available scripts list
  try {
    const scriptsJson =
      typeof scriptStorage !== "undefined" &&
      typeof scriptStorage.listScripts === "function"
        ? scriptStorage.listScripts()
        : "[]";
    const scriptMetadata = JSON.parse(scriptsJson);
    const scripts = scriptMetadata.map((meta) => meta.uri);
    if (scripts.length > 0) {
      contextualPrompt += "AVAILABLE SCRIPTS: " + scripts.join(", ") + "\\n\\n";
    }
  } catch (e) {
    console.log("Could not list scripts: " + e);
  }

  // Add available assets list
  try {
    const assetsJson =
      typeof assetStorage !== "undefined" &&
      typeof assetStorage.listAssets === "function"
        ? assetStorage.listAssets()
        : "[]";
    const assetMetadata = JSON.parse(assetsJson);
    if (assetMetadata.length > 0) {
      const assetNames = assetMetadata.map((a) => a.name);
      contextualPrompt +=
        "AVAILABLE ASSETS: " + assetNames.join(", ") + "\\n\\n";
    }
  } catch (e) {
    console.log("Could not list assets: " + e);
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
        max_tokens: 32768,
        system: systemPrompt,
        messages: [
          {
            role: "user",
            content: contextualPrompt,
          },
        ],
      }),
    });

    const responseJson = /** @type {string} */ (
      /** @type {unknown} */ (
        fetch("https://api.anthropic.com/v1/messages", JSON.parse(options))
      )
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

      const wasTruncated = stopReason === "max_tokens";
      if (wasTruncated) {
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
      let parseError = null;
      try {
        parsedResponse = JSON.parse(cleanedResponse);
        console.log(
          `AI Assistant: Successfully parsed structured response of type: ${parsedResponse.type}`,
        );
      } catch (error) {
        parseError = String(error);
        console.log(
          `AI Assistant: Response is plain text or invalid JSON - Error: ${parseError}`,
        );
        console.log(
          `AI Assistant: First 200 chars: ${cleanedResponse.substring(0, 200)}`,
        );

        // If it was truncated and JSON parsing failed, it's likely incomplete JSON
        if (wasTruncated) {
          console.log(
            `AI Assistant: Response was truncated AND JSON parsing failed - likely incomplete JSON`,
          );
        }
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
          truncated: wasTruncated,
          parse_error: parseError,
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

  // Register editor assets
  routeRegistry.registerAssetRoute("/editor.css", "editor.css");
  routeRegistry.registerAssetRoute("/editor.js", "editor.js");

  routeRegistry.registerRoute("/engine/editor", "serveEditor", "GET");
  routeRegistry.registerRoute("/engine/graphql", "serveGraphiQL", "GET");
  routeRegistry.registerRoute("/engine/swagger", "serveSwaggerUI", "GET");
  routeRegistry.registerRoute("/api/scripts", "apiListScripts", "GET");
  routeRegistry.registerRoute("/api/scripts/*", "apiGetScript", "GET");
  routeRegistry.registerRoute("/api/scripts/*", "apiSaveScript", "POST");
  routeRegistry.registerRoute("/api/scripts/*", "apiDeleteScript", "DELETE");
  routeRegistry.registerRoute(
    "/api/script-security/*",
    "apiUpdateScriptPrivilege",
    "POST",
  );
  // Owner management uses separate path to avoid wildcard conflicts
  routeRegistry.registerRoute(
    "/api/script-owners/*",
    "apiAddScriptOwner",
    "POST",
  );
  routeRegistry.registerRoute(
    "/api/script-owners/*",
    "apiRemoveScriptOwner",
    "DELETE",
  );
  routeRegistry.registerRoute("/api/logs", "apiGetLogs", "GET");
  routeRegistry.registerRoute("/api/logs", "apiPruneLogs", "DELETE");
  routeRegistry.registerRoute("/api/assets", "apiGetAssets", "GET");
  routeRegistry.registerRoute("/api/assets", "apiSaveAsset", "POST");
  routeRegistry.registerRoute("/api/assets/*", "apiGetAsset", "GET");
  routeRegistry.registerRoute("/api/assets/*", "apiDeleteAsset", "DELETE");
  routeRegistry.registerRoute("/api/routes", "apiListRoutes", "GET");
  routeRegistry.registerRoute("/api/ai-assistant", "apiAIAssistant", "POST");
  console.log("Editor endpoints registered");
  return { success: true };
}
