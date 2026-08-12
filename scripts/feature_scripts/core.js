/// <reference path="../../assets/aiwebengine-priv.d.ts" />

function getRequest(context) {
  return (context && context.request) || {};
}

function getArgs(context) {
  return (context && context.args) || {};
}

function logServerStarted() {
  console.log("server started");
}

function logServerStillRunning() {
  console.log("server still running");
}

// Health check endpoint
// Installation confirmation page
function installed_page(context) {
  return {
    status: 200,
    body: `<!DOCTYPE html>
<html lang="en">
<head>
  <meta charset="UTF-8">
  <meta name="viewport" content="width=device-width, initial-scale=1.0">
  <title>aiwebengine Installed</title>
  <style>
    body {
      margin: 0;
      padding: 0;
      font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, "Helvetica Neue", Arial, sans-serif;
      display: flex;
      justify-content: center;
      align-items: center;
      min-height: 100vh;
      background: linear-gradient(135deg, #667eea 0%, #764ba2 100%);
    }
    .container {
      text-align: center;
      background: white;
      padding: 3rem 4rem;
      border-radius: 1rem;
      box-shadow: 0 20px 60px rgba(0, 0, 0, 0.3);
    }
    h1 {
      color: #333;
      margin: 0 0 1rem 0;
      font-size: 2.5rem;
    }
    p {
      color: #666;
      font-size: 1.2rem;
      margin: 0;
    }
    .emoji {
      font-size: 4rem;
      margin-bottom: 1rem;
    }
  </style>
</head>
<body>
  <div class="container">
    <div class="emoji">🎉</div>
    <h1>Thanks for installing aiwebengine!</h1>
    <p>Your server is up and running.</p>
  </div>
</body>
</html>`,
    contentType: "text/html",
  };
}

// Stream customization function for script_updates
// NEW: Returns connection filter criteria based on request context
// This function is called once when a client connects to the stream
function scriptUpdatesCustomizer(context) {
  const req = getRequest(context);
  console.log("Customizing script_updates stream connection");
  console.log("Request path: " + req.path);
  console.log("Request query: " + JSON.stringify(req.query));
  console.log("Request auth: " + JSON.stringify(req.auth));

  // Return empty object to receive all messages
  // To filter messages, return criteria like: { category: "feature_scripts" }
  // Only messages with matching metadata will be sent to this connection
  return {};
}

// Helper function to broadcast script update messages
// Message metadata in the JSON object will be used for filtering.
// The default match mode is "subset", where connection criteria must be present
// in the message metadata. Callers can opt into "overlap" matching when they
// want one connection to receive personal, group, and global events.
function broadcastScriptUpdate(uri, action, details = {}) {
  try {
    var message = {
      type: "script_update",
      uri: uri,
      action: action, // 'inserted', 'updated', 'removed'
      timestamp: new Date().toISOString(),
    };

    // Add details to the message
    // These properties become message metadata for filtering
    for (var key in details) {
      if (details.hasOwnProperty(key)) {
        message[key] = details[key];
      }
    }

    // Broadcast to /script_updates stream
    // All connections will receive this since we return {} from customization function
    routeRegistry.sendStreamMessage("/script_updates", JSON.stringify(message));

    console.log("Broadcasted script update: " + action + " " + uri);
  } catch (error) {
    console.error("Failed to broadcast script update: " + error.message);
  }
}

// Script management endpoint
function upsert_script_handler(context) {
  const req = getRequest(context);
  try {
    // Extract parameters from form data (for POST requests)
    let uri = null;
    let content = null;

    if (req.form) {
      uri = req.form.uri;
      content = req.form.content;
    }

    // Fallback to query parameters if form data is not available
    if (!uri && req.query) {
      uri = req.query.uri;
    }
    if (!content && req.query) {
      content = req.query.content;
    }

    // Validate required parameters
    if (!uri) {
      return {
        status: 400,
        body: JSON.stringify({
          error: "Missing required parameter: uri",
          timestamp: new Date().toISOString(),
        }),
        contentType: "application/json",
      };
    }

    if (!content) {
      return {
        status: 400,
        body: JSON.stringify({
          error: "Missing required parameter: content",
          timestamp: new Date().toISOString(),
        }),
        contentType: "application/json",
      };
    }

    // Check if script already exists to determine action
    const existingScript =
      typeof scriptStorage !== "undefined" &&
      typeof scriptStorage.getScript === "function"
        ? scriptStorage.getScript(uri)
        : null;
    const action = existingScript ? "updated" : "inserted";

    // Call the upsertScript function
    const result =
      typeof scriptStorage !== "undefined" &&
      typeof scriptStorage.upsertScript === "function"
        ? scriptStorage.upsertScript(uri, content)
        : "Error: scriptStorage.upsertScript not available";

    // Check if the result indicates an error
    // The Rust function returns a string for both success and errors
    // Error messages start with "Error:"
    if (!result || result.startsWith("Error:")) {
      console.error(`Script upsert failed: ${result}`);
      return {
        status: 500,
        body: JSON.stringify({
          error: "Failed to upsert script",
          details: result || "Unknown error",
          timestamp: new Date().toISOString(),
        }),
        contentType: "application/json",
      };
    }

    // Broadcast the script update
    broadcastScriptUpdate(uri, action, {
      contentLength: content.length,
      previousExists: !!existingScript,
    });

    console.log(`Script upserted: ${uri} (${content.length} characters)`);

    return {
      status: 200,
      body: JSON.stringify({
        success: true,
        message: "Script upserted successfully",
        uri: uri,
        contentLength: content.length,
        timestamp: new Date().toISOString(),
      }),
      contentType: "application/json",
    };
  } catch (error) {
    console.error(`Script upsert failed: ${error.message}`);
    return {
      status: 500,
      body: JSON.stringify({
        error: "Failed to upsert script",
        details: error.message,
        timestamp: new Date().toISOString(),
      }),
      contentType: "application/json",
    };
  }
}

// Script deletion endpoint
function delete_script_handler(context) {
  const req = getRequest(context);
  try {
    // Extract uri parameter from form data (for POST requests)
    let uri = null;

    if (req.form) {
      uri = req.form.uri;
    }

    // Fallback to query parameters if form data is not available
    if (!uri && req.query) {
      uri = req.query.uri;
    }

    // Validate required parameter
    if (!uri) {
      return {
        status: 400,
        body: JSON.stringify({
          error: "Missing required parameter: uri",
          timestamp: new Date().toISOString(),
        }),
        contentType: "application/json",
      };
    }

    // Call the deleteScript function
    const deleted =
      typeof scriptStorage !== "undefined" &&
      typeof scriptStorage.deleteScript === "function"
        ? scriptStorage.deleteScript(uri)
        : false;

    if (deleted) {
      // Broadcast the script removal
      broadcastScriptUpdate(uri, "removed");

      console.log(`Script deleted: ${uri}`);
      return {
        status: 200,
        body: JSON.stringify({
          success: true,
          message: "Script deleted successfully",
          uri: uri,
          timestamp: new Date().toISOString(),
        }),
        contentType: "application/json",
      };
    } else {
      console.log(`Script not found for deletion: ${uri}`);
      return {
        status: 404,
        body: JSON.stringify({
          error: "Script not found",
          message: "No script with the specified URI was found",
          uri: uri,
          timestamp: new Date().toISOString(),
        }),
        contentType: "application/json",
      };
    }
  } catch (error) {
    console.error(`Script deletion failed: ${error.message}`);
    return {
      status: 500,
      body: JSON.stringify({
        error: "Failed to delete script",
        details: error.message,
        timestamp: new Date().toISOString(),
      }),
      contentType: "application/json",
    };
  }
}

// Script reading endpoint
function read_script_handler(context) {
  const req = getRequest(context);
  try {
    // Extract uri parameter from query string
    let uri = null;

    if (req.query) {
      uri = req.query.uri;
    }

    // Validate required parameter
    if (!uri) {
      return {
        status: 400,
        body: JSON.stringify({
          error: "Missing required parameter: uri",
          timestamp: new Date().toISOString(),
        }),
        contentType: "application/json",
      };
    }

    // Call the getScript function
    const content =
      typeof scriptStorage !== "undefined" &&
      typeof scriptStorage.getScript === "function"
        ? scriptStorage.getScript(uri)
        : null;

    // getScript returns null if script not found or access denied
    if (content !== null && content !== undefined) {
      console.log(`Script retrieved: ${uri} (${content.length} characters)`);
      return {
        status: 200,
        body: content,
        contentType: "application/javascript",
      };
    } else {
      console.log(`Script not found: ${uri}`);
      return {
        status: 404,
        body: JSON.stringify({
          error: "Script not found",
          message: "No script with the specified URI was found",
          uri: uri,
          timestamp: new Date().toISOString(),
        }),
        contentType: "application/json",
      };
    }
  } catch (error) {
    console.error(`Script read failed: ${error.message}`);
    return {
      status: 500,
      body: JSON.stringify({
        error: "Failed to read script",
        details: error.message,
        timestamp: new Date().toISOString(),
      }),
      contentType: "application/json",
    };
  }
}

// Script logs endpoint
function script_logs_handler(context) {
  const req = getRequest(context);
  try {
    // Extract uri parameter from query string
    let uri = null;

    if (req.query) {
      uri = req.query.uri;
    }

    // Validate required parameter
    if (!uri) {
      return {
        status: 400,
        body: JSON.stringify({
          error: "Missing required parameter: uri",
          timestamp: new Date().toISOString(),
        }),
        contentType: "application/json",
      };
    }

    // Call the console.listLogsForUri function
    const logsJson = console.listLogsForUri(uri);
    const logs = JSON.parse(logsJson);

    console.log(`Retrieved ${logs.length} log entries for script: ${uri}`);

    return {
      status: 200,
      body: JSON.stringify({
        uri: uri,
        logs: logs,
        count: logs.length,
        timestamp: new Date().toISOString(),
      }),
      contentType: "application/json",
    };
  } catch (error) {
    console.error(`Script logs retrieval failed: ${error.message}`);
    return {
      status: 500,
      body: JSON.stringify({
        error: "Failed to retrieve script logs",
        details: error.message,
        timestamp: new Date().toISOString(),
      }),
      contentType: "application/json",
    };
  }
}

// Read init status handler - reports init() success/failure for scripts
// (useful for debugging; covers one script or all scripts)
function readInitStatusHandler(context) {
  const args = getArgs(context);
  const uri = args.uri;

  try {
    if (uri) {
      const status = scriptStorage.getScriptInitStatus(uri);
      return JSON.stringify({
        uri: uri,
        status: status ? JSON.parse(status) : null,
        timestamp: new Date().toISOString(),
      });
    }

    const scriptsJson =
      typeof scriptStorage !== "undefined" &&
      typeof scriptStorage.listScripts === "function"
        ? scriptStorage.listScripts()
        : "[]";
    const scriptMetadata = JSON.parse(scriptsJson);
    const statuses = [];

    for (const meta of scriptMetadata) {
      const status = scriptStorage.getScriptInitStatus(meta.uri);
      if (status) {
        statuses.push(JSON.parse(status));
      }
    }

    return JSON.stringify({
      statuses: statuses,
      count: statuses.length,
      timestamp: new Date().toISOString(),
    });
  } catch (error) {
    console.error(`MCP read_init_status error: ${error.message}`);
    return JSON.stringify({
      error: `Failed to read init status: ${error.message}`,
    });
  }
}

// OpenAPI specification endpoint
function openapiSpec(context) {
  try {
    const spec = routeRegistry.generateOpenApi();
    return {
      status: 200,
      body: spec,
      contentType: "application/json",
    };
  } catch (error) {
    console.error("Error generating OpenAPI spec: " + error.message);
    return {
      status: 500,
      body: JSON.stringify({
        error: "Failed to generate OpenAPI specification",
      }),
      contentType: "application/json",
    };
  }
}

// MCP Tool Handlers for File Operations

// Read file handler - fetches content of a script
function readFileHandler(context) {
  const args = getArgs(context);
  const uri = args.uri;

  if (!uri) {
    return JSON.stringify({
      error: "Missing required parameter: uri",
    });
  }

  try {
    const content =
      typeof scriptStorage !== "undefined" &&
      typeof scriptStorage.getScript === "function"
        ? scriptStorage.getScript(uri)
        : null;

    if (content !== null && content !== undefined) {
      return JSON.stringify({
        uri: uri,
        content: content,
        size: content.length,
        timestamp: new Date().toISOString(),
      });
    } else {
      return JSON.stringify({
        error: `File not found: ${uri}`,
      });
    }
  } catch (error) {
    console.error(`MCP read_file error: ${error.message}`);
    return JSON.stringify({
      error: `Failed to read file: ${error.message}`,
    });
  }
}

// Write/edit file handler - creates or updates a script
function writeFileHandler(context) {
  const args = getArgs(context);
  const uri = args.uri;
  const content = args.content;

  if (!uri) {
    return JSON.stringify({
      error: "Missing required parameter: uri",
    });
  }

  if (content === undefined || content === null) {
    return JSON.stringify({
      error: "Missing required parameter: content",
    });
  }

  try {
    const existingScript =
      typeof scriptStorage !== "undefined" &&
      typeof scriptStorage.getScript === "function"
        ? scriptStorage.getScript(uri)
        : null;
    const action = existingScript ? "updated" : "created";

    const result =
      typeof scriptStorage !== "undefined" &&
      typeof scriptStorage.upsertScript === "function"
        ? scriptStorage.upsertScript(uri, content)
        : "Error: scriptStorage.upsertScript not available";

    // Check if the result indicates an error (Rust returns string for both success and errors)
    if (!result || result.startsWith("Error:")) {
      console.error(`MCP write_file failed: ${result}`);
      return JSON.stringify({
        error: `Failed to write file: ${result || "Unknown error"}`,
      });
    }

    broadcastScriptUpdate(uri, action === "created" ? "inserted" : "updated", {
      contentLength: content.length,
      via: "mcp",
    });

    console.log(`MCP ${action} file: ${uri} (${content.length} chars)`);

    return JSON.stringify({
      success: true,
      action: action,
      uri: uri,
      size: content.length,
      timestamp: new Date().toISOString(),
    });
  } catch (error) {
    console.error(`MCP write_file error: ${error.message}`);
    return JSON.stringify({
      error: `Failed to write file: ${error.message}`,
    });
  }
}

// Create file handler - creates a new script (fails if exists)
function createFileHandler(context) {
  const args = getArgs(context);
  const uri = args.uri;
  const content = args.content || "";

  if (!uri) {
    return JSON.stringify({
      error: "Missing required parameter: uri",
    });
  }

  try {
    const existingScript =
      typeof scriptStorage !== "undefined" &&
      typeof scriptStorage.getScript === "function"
        ? scriptStorage.getScript(uri)
        : null;

    if (existingScript !== null && existingScript !== undefined) {
      return JSON.stringify({
        error: `File already exists: ${uri}`,
      });
    }

    const result =
      typeof scriptStorage !== "undefined" &&
      typeof scriptStorage.upsertScript === "function"
        ? scriptStorage.upsertScript(uri, content)
        : "Error: scriptStorage.upsertScript not available";

    // Check if the result indicates an error (Rust returns string for both success and errors)
    if (!result || result.startsWith("Error:")) {
      console.error(`MCP create_file failed: ${result}`);
      return JSON.stringify({
        error: `Failed to create file: ${result || "Unknown error"}`,
      });
    }

    broadcastScriptUpdate(uri, "inserted", {
      contentLength: content.length,
      via: "mcp",
    });

    console.log(`MCP created file: ${uri} (${content.length} chars)`);

    return JSON.stringify({
      success: true,
      uri: uri,
      size: content.length,
      timestamp: new Date().toISOString(),
    });
  } catch (error) {
    console.error(`MCP create_file error: ${error.message}`);
    return JSON.stringify({
      error: `Failed to create file: ${error.message}`,
    });
  }
}

// List files handler - lists all scripts or filters by pattern
function listFilesHandler(context) {
  const args = getArgs(context);
  const pattern = args.pattern || null;

  try {
    const scriptsJson =
      typeof scriptStorage !== "undefined" &&
      typeof scriptStorage.listScripts === "function"
        ? scriptStorage.listScripts()
        : "[]";

    let scripts = JSON.parse(scriptsJson);

    // Filter by pattern if provided
    if (pattern) {
      const regex = new RegExp(pattern, "i");
      scripts = scripts.filter((script) => regex.test(script.uri));
    }

    const files = scripts.map((script) => ({
      uri: script.uri,
      size: script.size || 0,
      type: "script",
    }));

    return JSON.stringify({
      files: files,
      count: files.length,
      pattern: pattern,
      timestamp: new Date().toISOString(),
    });
  } catch (error) {
    console.error(`MCP list_files error: ${error.message}`);
    return JSON.stringify({
      error: `Failed to list files: ${error.message}`,
    });
  }
}

// Delete file handler - removes a script
function deleteFileHandler(context) {
  const args = getArgs(context);
  const uri = args.uri;

  if (!uri) {
    return JSON.stringify({
      error: "Missing required parameter: uri",
    });
  }

  try {
    const deleted =
      typeof scriptStorage !== "undefined" &&
      typeof scriptStorage.deleteScript === "function"
        ? scriptStorage.deleteScript(uri)
        : false;

    if (deleted) {
      broadcastScriptUpdate(uri, "removed", {
        via: "mcp",
      });

      console.log(`MCP deleted file: ${uri}`);

      return JSON.stringify({
        success: true,
        uri: uri,
        timestamp: new Date().toISOString(),
      });
    } else {
      return JSON.stringify({
        error: `File not found: ${uri}`,
      });
    }
  } catch (error) {
    console.error(`MCP delete_file error: ${error.message}`);
    return JSON.stringify({
      error: `Failed to delete file: ${error.message}`,
    });
  }
}

// Search files handler - searches for text across all scripts
function searchFilesHandler(context) {
  const args = getArgs(context);
  const query = args.query;
  const caseInsensitive = args.caseInsensitive !== false; // default true

  if (!query) {
    return JSON.stringify({
      error: "Missing required parameter: query",
    });
  }

  try {
    const scriptsJson =
      typeof scriptStorage !== "undefined" &&
      typeof scriptStorage.listScripts === "function"
        ? scriptStorage.listScripts()
        : "[]";

    const scriptMetadata = JSON.parse(scriptsJson);
    const results = [];
    const flags = caseInsensitive ? "gi" : "g";
    const searchRegex = new RegExp(query, flags);

    for (const meta of scriptMetadata) {
      const content =
        typeof scriptStorage !== "undefined" &&
        typeof scriptStorage.getScript === "function"
          ? scriptStorage.getScript(meta.uri)
          : null;

      if (content) {
        const lines = content.split("\n");
        const matches = [];

        for (let i = 0; i < lines.length; i++) {
          if (searchRegex.test(lines[i])) {
            matches.push({
              line: i + 1,
              content: lines[i].trim(),
              preview: lines[i].substring(0, 200),
            });
          }
        }

        if (matches.length > 0) {
          results.push({
            uri: meta.uri,
            matchCount: matches.length,
            matches: matches.slice(0, 50), // Limit to first 50 matches per file
          });
        }
      }
    }

    console.log(
      `MCP search found ${results.length} files with matches for: ${query}`,
    );

    return JSON.stringify({
      query: query,
      caseInsensitive: caseInsensitive,
      filesMatched: results.length,
      results: results,
      timestamp: new Date().toISOString(),
    });
  } catch (error) {
    console.error(`MCP search_files error: ${error.message}`);
    return JSON.stringify({
      error: `Failed to search files: ${error.message}`,
    });
  }
}

// Read logs handler - retrieves log messages for a specific script
function readLogsHandler(context) {
  const args = getArgs(context);
  const uri = args.uri;

  if (!uri) {
    return JSON.stringify({
      error: "Missing required parameter: uri",
    });
  }

  try {
    const logsJson = console.listLogsForUri(uri);
    const logs = JSON.parse(logsJson);

    console.log(
      `MCP read_logs retrieved ${logs.length} log entries for: ${uri}`,
    );

    return JSON.stringify({
      uri: uri,
      logs: logs,
      count: logs.length,
      timestamp: new Date().toISOString(),
    });
  } catch (error) {
    console.error(`MCP read_logs error: ${error.message}`);
    return JSON.stringify({
      error: `Failed to read logs: ${error.message}`,
    });
  }
}

// GraphQL resolvers are now handled in a separate script

// Initialization function - called when script is loaded or updated
function init(context) {
  try {
    console.log(`Initializing core.js script at ${new Date().toISOString()}`);
    console.log(`Init context: ${JSON.stringify(context)}`);

    // Register public asset paths
    routeRegistry.registerAssetRoute("/logo.svg", "logo.svg");
    routeRegistry.registerAssetRoute("/favicon.ico", "favicon.ico");
    routeRegistry.registerAssetRoute("/engine.css", "engine.css");

    // Register HTTP endpoints with OpenAPI metadata
    // Note: /health is served by the Rust engine (see health_handler in lib.rs),
    // which performs the database check directly; no script route is registered.
    routeRegistry.registerRoute("/engine/installed", "installed_page", "GET", {
      summary: "Installation confirmation",
      description: "Shows a confirmation page for successful installation",
      tags: ["Engine"],
    });
    routeRegistry.registerRoute(
      "/upsert_script",
      "upsert_script_handler",
      "POST",
      {
        summary: "Create or update script",
        description: "Upsert a script by URI",
        tags: ["Scripts"],
      },
    );
    routeRegistry.registerRoute(
      "/delete_script",
      "delete_script_handler",
      "POST",
      {
        summary: "Delete script",
        description: "Delete a script by URI",
        tags: ["Scripts"],
      },
    );
    routeRegistry.registerRoute("/read_script", "read_script_handler", "GET", {
      summary: "Read script",
      description: "Retrieve script content by URI",
      tags: ["Scripts"],
    });
    routeRegistry.registerRoute("/script_logs", "script_logs_handler", "GET", {
      summary: "Script logs",
      description: "Get logs for a specific script",
      tags: ["Logging"],
    });
    routeRegistry.registerRoute("/engine/openapi.json", "openapiSpec", "GET", {
      summary: "OpenAPI Specification",
      description:
        "Returns the OpenAPI 3.0 specification for all registered routes",
      tags: ["Engine"],
    });

    // Register WebSocket stream endpoint with customization function
    // NEW API: registerStreamRoute(path, customizationFunction)
    // The customization function returns filter criteria for each connection
    routeRegistry.registerStreamRoute(
      "/script_updates",
      "scriptUpdatesCustomizer",
    );

    if (typeof schedulerService !== "undefined") {
      const oneMinuteFromNow = new Date(Date.now() + 60 * 1000).toISOString();
      schedulerService.clearAll();
      schedulerService.registerOnce({
        handler: "logServerStarted",
        runAt: oneMinuteFromNow,
        name: "core-server-started",
      });
      schedulerService.registerRecurring({
        handler: "logServerStillRunning",
        intervalMinutes: 2,
        name: "core-server-heartbeat",
      });
    } else {
      console.warn("schedulerService unavailable; skipping background jobs");
    }

    // Register MCP tools for file operations
    if (typeof mcpRegistry !== "undefined") {
      console.log("Registering MCP file operation tools...");

      mcpRegistry.registerTool(
        "read_file",
        "Fetch the contents of a remote file (script) by URI",
        JSON.stringify({
          type: "object",
          properties: {
            uri: {
              type: "string",
              description: "Script URI (e.g., 'https://example.com/myscript')",
            },
          },
          required: ["uri"],
        }),
        "readFileHandler",
      );

      mcpRegistry.registerTool(
        "write_file",
        "Create or update a file (script) on the server",
        JSON.stringify({
          type: "object",
          properties: {
            uri: {
              type: "string",
              description: "Script URI",
            },
            content: {
              type: "string",
              description: "File content (JavaScript code)",
            },
          },
          required: ["uri", "content"],
        }),
        "writeFileHandler",
      );

      mcpRegistry.registerTool(
        "create_file",
        "Create a new file (script) on the server. Fails if file already exists.",
        JSON.stringify({
          type: "object",
          properties: {
            uri: {
              type: "string",
              description: "Script URI",
            },
            content: {
              type: "string",
              description: "File content (JavaScript code)",
              default: "",
            },
          },
          required: ["uri"],
        }),
        "createFileHandler",
      );

      mcpRegistry.registerTool(
        "list_files",
        "List all files (scripts) in the system, optionally filtered by pattern",
        JSON.stringify({
          type: "object",
          properties: {
            pattern: {
              type: "string",
              description: "Optional regex pattern to filter files by URI",
            },
          },
        }),
        "listFilesHandler",
      );

      mcpRegistry.registerTool(
        "delete_file",
        "Remove a file (script) from the server",
        JSON.stringify({
          type: "object",
          properties: {
            uri: {
              type: "string",
              description: "Script URI to delete",
            },
          },
          required: ["uri"],
        }),
        "deleteFileHandler",
      );

      mcpRegistry.registerTool(
        "search_files",
        "Perform text search across all files (grep-like functionality)",
        JSON.stringify({
          type: "object",
          properties: {
            query: {
              type: "string",
              description: "Text or regex pattern to search for",
            },
            caseInsensitive: {
              type: "boolean",
              description: "Whether search should be case-insensitive",
              default: true,
            },
          },
          required: ["query"],
        }),
        "searchFilesHandler",
      );

      mcpRegistry.registerTool(
        "read_logs",
        "Read log messages for a specific script (useful for debugging)",
        JSON.stringify({
          type: "object",
          properties: {
            uri: {
              type: "string",
              description: "Script URI to retrieve logs for",
            },
          },
          required: ["uri"],
        }),
        "readLogsHandler",
      );

      mcpRegistry.registerTool(
        "read_init_status",
        "Read init() status for scripts (useful for debugging). Returns status for one script when uri is given, otherwise for all scripts.",
        JSON.stringify({
          type: "object",
          properties: {
            uri: {
              type: "string",
              description:
                "Optional script URI to retrieve init status for; omit to list all scripts",
            },
          },
        }),
        "readInitStatusHandler",
      );

      console.log("MCP file operation tools registered successfully");
    } else {
      console.warn("mcpRegistry unavailable; skipping MCP tool registration");
    }

    console.log("Core script initialized successfully");

    return {
      success: true,
      message: "Core script initialized successfully",
      registeredEndpoints: 8,
      registeredAssets: 3,
      registeredMcpTools: 8,
    };
  } catch (error) {
    console.error(`Core script initialization failed: ${error.message}`);
    throw error;
  }
}

try {
  console.log(`server started ${new Date().toISOString()}`);
} catch (e) {
  // ignore if host function isn't present yet
}
