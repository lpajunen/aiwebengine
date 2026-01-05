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
function health_check(context) {
  const req = getRequest(context);
  // Call Rust function to check database health
  var databaseHealth;
  try {
    var dbHealthJson = database.checkDatabaseHealth();
    databaseHealth = JSON.parse(dbHealthJson);
  } catch (error) {
    databaseHealth = {
      healthy: false,
      error: "Failed to check database health: " + error.message,
    };
  }

  // Determine overall health status
  var isHealthy = databaseHealth.healthy;
  var status = isHealthy ? "healthy" : "unhealthy";

  return {
    status: 200,
    body: JSON.stringify({
      status: status,
      timestamp: new Date().toISOString(),
      checks: {
        javascript: "ok",
        logging: "ok",
        json: "ok",
        database: databaseHealth.healthy ? "ok" : "error",
      },
      details: {
        database: databaseHealth,
      },
    }),
    contentType: "application/json",
  };
}

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

// GraphQL subscription resolver for script updates
// NEW: Returns filter criteria (empty object = broadcast to all connections)
function scriptUpdatesResolver(context) {
  const req = getRequest(context);
  const args = getArgs(context);
  console.log("Client subscribed to scriptUpdates GraphQL subscription");
  console.log("Request context: " + JSON.stringify(req));

  // Return empty object to broadcast to all connections
  // To filter, return an object like: { userId: req.auth.userId, role: "admin" }
  return {};
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
// NEW SEMANTICS: Message metadata in the JSON object will be used for filtering
// Connections receive messages when their filter criteria (set by customization function)
// is a subset of the message metadata
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

    // Send to GraphQL subscription using modern approach
    graphQLRegistry.sendSubscriptionMessage(
      "scriptUpdates",
      JSON.stringify(message),
    );

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
    const success =
      typeof scriptStorage !== "undefined" &&
      typeof scriptStorage.upsertScript === "function"
        ? scriptStorage.upsertScript(uri, content)
        : false;

    if (!success) {
      return {
        status: 500,
        body: JSON.stringify({
          error: "Failed to upsert script",
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

// GraphQL resolvers
function scriptsQuery(context) {
  const req = getRequest(context);
  const args = getArgs(context);
  try {
    const scriptsJson =
      typeof scriptStorage !== "undefined" &&
      typeof scriptStorage.listScripts === "function"
        ? scriptStorage.listScripts()
        : "[]";

    const scriptMetadata = JSON.parse(scriptsJson);

    const scriptArray = scriptMetadata.map((meta) => {
      // Get owners for this script
      let owners = [];
      try {
        if (
          typeof scriptStorage !== "undefined" &&
          typeof scriptStorage.getScriptOwners === "function"
        ) {
          const ownersJson = scriptStorage.getScriptOwners(meta.uri);
          owners = JSON.parse(ownersJson);
        }
      } catch (e) {
        console.warn(
          `Failed to get owners for script ${meta.uri}: ${e.message}`,
        );
      }

      return {
        uri: meta.uri,
        chars: meta.size || 0,
        owners: owners,
      };
    });

    return JSON.stringify(scriptArray);
  } catch (error) {
    console.error(`Scripts query failed: ${error.message}`);
    return JSON.stringify([]);
  }
}

function scriptQuery(context) {
  const req = getRequest(context);
  const args = getArgs(context);
  try {
    const content =
      typeof scriptStorage !== "undefined" &&
      typeof scriptStorage.getScript === "function"
        ? scriptStorage.getScript(args.uri)
        : null;
    const logsJson = console.listLogsForUri(args.uri);
    const logs = JSON.parse(logsJson);

    // Get owners for this script
    let owners = [];
    try {
      if (
        typeof scriptStorage !== "undefined" &&
        typeof scriptStorage.getScriptOwners === "function"
      ) {
        const ownersJson = scriptStorage.getScriptOwners(args.uri);
        owners = JSON.parse(ownersJson);
      }
    } catch (e) {
      console.warn(`Failed to get owners for script ${args.uri}: ${e.message}`);
    }

    // getScript returns null if script not found
    if (content !== null && content !== undefined) {
      return JSON.stringify({
        uri: args.uri,
        content: content,
        contentLength: content.length,
        logs: logs,
        owners: owners,
      });
    } else {
      // Return null if script doesn't exist
      return JSON.stringify({
        uri: args.uri,
        content: null,
        contentLength: 0,
        logs: logs,
        owners: owners,
      });
    }
  } catch (error) {
    console.error(`Script query failed: ${error.message}`);
    return JSON.stringify({
      uri: args.uri,
      content: null,
      contentLength: 0,
      logs: [],
      owners: [],
    });
  }
}

function scriptInitStatusQuery(context) {
  const req = getRequest(context);
  const args = getArgs(context);
  try {
    const status = scriptStorage.getScriptInitStatus(args.uri);
    if (status) {
      return status; // Already JSON string
    } else {
      // Script not found or no metadata
      return JSON.stringify(null);
    }
  } catch (error) {
    console.error(`Script init status query failed: ${error.message}`);
    return JSON.stringify(null);
  }
}

function allScriptsInitStatusQuery(context) {
  const req = getRequest(context);
  const args = getArgs(context);
  try {
    const scriptsJson =
      typeof scriptStorage !== "undefined" &&
      typeof scriptStorage.listScripts === "function"
        ? scriptStorage.listScripts()
        : "[]";
    const scriptMetadata = JSON.parse(scriptsJson);
    const scriptUris = scriptMetadata.map((meta) => meta.uri);
    const statusArray = [];

    for (const uri of scriptUris) {
      const statusStr =
        typeof scriptStorage !== "undefined" &&
        typeof scriptStorage.getScriptInitStatus === "function"
          ? scriptStorage.getScriptInitStatus(uri)
          : null;
      if (statusStr) {
        const status = JSON.parse(statusStr);
        statusArray.push(status);
      }
    }

    return JSON.stringify(statusArray);
  } catch (error) {
    console.error(`All scripts init status query failed: ${error.message}`);
    return JSON.stringify([]);
  }
}

function upsertScriptMutation(context) {
  const req = getRequest(context);
  const args = getArgs(context);
  try {
    // Check if script already exists to determine action
    const existingScript =
      typeof scriptStorage !== "undefined" &&
      typeof scriptStorage.getScript === "function"
        ? scriptStorage.getScript(args.uri)
        : null;
    const action = existingScript ? "updated" : "inserted";

    const success =
      typeof scriptStorage !== "undefined" &&
      typeof scriptStorage.upsertScript === "function"
        ? scriptStorage.upsertScript(args.uri, args.content)
        : false;

    if (!success) {
      return JSON.stringify({
        message: "Failed to upsert script",
        uri: args.uri,
        chars: 0,
        success: false,
      });
    }

    // Broadcast the script update
    broadcastScriptUpdate(args.uri, action, {
      contentLength: args.content.length,
      previousExists: !!existingScript,
      via: "graphql",
    });

    console.log(
      `Script upserted via GraphQL: ${args.uri} (${args.content.length} characters)`,
    );
    return JSON.stringify({
      message: `Script upserted successfully: ${args.uri} (${args.content.length} characters)`,
      uri: args.uri,
      chars: args.content.length,
      success: true,
    });
  } catch (error) {
    console.error(`Script upsert mutation failed: ${error.message}`);
    return JSON.stringify({
      message: `Error: Failed to upsert script: ${error.message}`,
      uri: args ? args.uri : null,
      chars: args && args.content ? args.content.length : 0,
      success: false,
    });
  }
}

function deleteScriptMutation(context) {
  const req = getRequest(context);
  const args = getArgs(context);
  try {
    const result =
      typeof scriptStorage !== "undefined" &&
      typeof scriptStorage.deleteScript === "function"
        ? scriptStorage.deleteScript(args.uri)
        : false;
    // deleteScript now returns boolean: true if deleted, false if not found

    if (result) {
      // Broadcast the script removal
      broadcastScriptUpdate(args.uri, "removed", {
        via: "graphql",
      });

      console.log(`Script deleted via GraphQL: ${args.uri}`);
      return JSON.stringify({
        message: `Script deleted successfully: ${args.uri}`,
        uri: args.uri,
        success: true,
      });
    } else {
      return JSON.stringify({
        message: `Script not found: ${args.uri}`,
        uri: args.uri,
        success: false,
      });
    }
  } catch (error) {
    console.error(`Script delete mutation failed: ${error.message}`);
    return JSON.stringify({
      message: `Error: Failed to delete script: ${error.message}`,
      uri: args.uri,
      success: false,
    });
  }
}

function addScriptOwnerMutation(context) {
  const req = getRequest(context);
  const args = getArgs(context);
  try {
    const result =
      typeof scriptStorage !== "undefined" &&
      typeof scriptStorage.addScriptOwner === "function"
        ? scriptStorage.addScriptOwner(args.uri, args.userId)
        : "Error: scriptStorage.addScriptOwner not available";

    // Check if result indicates an error
    if (typeof result === "string" && result.startsWith("Error:")) {
      console.error(`Add script owner mutation failed: ${result}`);
      return JSON.stringify({
        message: result,
        uri: args.uri,
        userId: args.userId,
        success: false,
      });
    }

    console.log(
      `Owner added via GraphQL: ${args.userId} to script ${args.uri}`,
    );
    return JSON.stringify({
      message:
        result ||
        `Successfully added owner ${args.userId} to script ${args.uri}`,
      uri: args.uri,
      userId: args.userId,
      success: true,
    });
  } catch (error) {
    console.error(`Add script owner mutation failed: ${error.message}`);
    return JSON.stringify({
      message: `Error: Failed to add owner: ${error.message}`,
      uri: args.uri,
      userId: args.userId,
      success: false,
    });
  }
}

function removeScriptOwnerMutation(context) {
  const req = getRequest(context);
  const args = getArgs(context);
  try {
    const result =
      typeof scriptStorage !== "undefined" &&
      typeof scriptStorage.removeScriptOwner === "function"
        ? scriptStorage.removeScriptOwner(args.uri, args.userId)
        : "Error: scriptStorage.removeScriptOwner not available";

    // Check if result indicates an error
    if (typeof result === "string" && result.startsWith("Error:")) {
      console.error(`Remove script owner mutation failed: ${result}`);
      return JSON.stringify({
        message: result,
        uri: args.uri,
        userId: args.userId,
        success: false,
      });
    }

    console.log(
      `Owner removed via GraphQL: ${args.userId} from script ${args.uri}`,
    );
    return JSON.stringify({
      message:
        result ||
        `Successfully removed owner ${args.userId} from script ${args.uri}`,
      uri: args.uri,
      userId: args.userId,
      success: true,
    });
  } catch (error) {
    console.error(`Remove script owner mutation failed: ${error.message}`);
    return JSON.stringify({
      message: `Error: Failed to remove owner: ${error.message}`,
      uri: args.uri,
      userId: args.userId,
      success: false,
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

    const success =
      typeof scriptStorage !== "undefined" &&
      typeof scriptStorage.upsertScript === "function"
        ? scriptStorage.upsertScript(uri, content)
        : false;

    if (!success) {
      return JSON.stringify({
        error: "Failed to write file",
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

    const success =
      typeof scriptStorage !== "undefined" &&
      typeof scriptStorage.upsertScript === "function"
        ? scriptStorage.upsertScript(uri, content)
        : false;

    if (!success) {
      return JSON.stringify({
        error: "Failed to create file",
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
    routeRegistry.registerRoute("/health", "health_check", "GET", {
      summary: "Health check",
      description:
        "Returns system health status including database connectivity",
      tags: ["Monitoring"],
    });
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

    // Register GraphQL subscription (authenticated - used by UI clients)
    graphQLRegistry.registerSubscription(
      "scriptUpdates",
      "type ScriptUpdate { type: String!, uri: String!, action: String!, timestamp: String!, contentLength: Int, previousExists: Boolean, via: String } type Subscription { scriptUpdates: ScriptUpdate! }",
      "scriptUpdatesResolver",
      "external",
    );

    // Register GraphQL queries (authenticated - used by clients and tests)
    graphQLRegistry.registerQuery(
      "scripts",
      "type ScriptInfo { uri: String!, chars: Int!, owners: [String!]! } type Query { scripts: [ScriptInfo!]! }",
      "scriptsQuery",
      "external",
    );
    graphQLRegistry.registerQuery(
      "script",
      "type ScriptDetail { uri: String!, content: String!, contentLength: Int!, logs: [String!]!, owners: [String!]! } type Query { script(uri: String!): ScriptDetail }",
      "scriptQuery",
      "external",
    );
    graphQLRegistry.registerQuery(
      "scriptInitStatus",
      "type ScriptInitStatus { scriptName: String!, initialized: Boolean!, initError: String, lastInitTime: Float, createdAt: Float, updatedAt: Float } type Query { scriptInitStatus(uri: String!): ScriptInitStatus }",
      "scriptInitStatusQuery",
      "external",
    );
    graphQLRegistry.registerQuery(
      "allScriptsInitStatus",
      "type ScriptInitStatus { scriptName: String!, initialized: Boolean!, initError: String, lastInitTime: Float, createdAt: Float, updatedAt: Float } type Query { allScriptsInitStatus: [ScriptInitStatus!]! }",
      "allScriptsInitStatusQuery",
      "external",
    );

    // Register GraphQL mutations (authenticated - used by clients and tests)
    graphQLRegistry.registerMutation(
      "upsertScript",
      "type UpsertScriptResponse { message: String!, uri: String!, chars: Int!, success: Boolean! } type Mutation { upsertScript(uri: String!, content: String!): UpsertScriptResponse! }",
      "upsertScriptMutation",
      "external",
    );
    graphQLRegistry.registerMutation(
      "deleteScript",
      "type DeleteScriptResponse { message: String!, uri: String!, success: Boolean! } type Mutation { deleteScript(uri: String!): DeleteScriptResponse! }",
      "deleteScriptMutation",
      "external",
    );
    graphQLRegistry.registerMutation(
      "addScriptOwner",
      "type OwnershipResponse { message: String!, uri: String!, userId: String!, success: Boolean! } type Mutation { addScriptOwner(uri: String!, userId: String!): OwnershipResponse! }",
      "addScriptOwnerMutation",
      "external",
    );
    graphQLRegistry.registerMutation(
      "removeScriptOwner",
      "type OwnershipResponse { message: String!, uri: String!, userId: String!, success: Boolean! } type Mutation { removeScriptOwner(uri: String!, userId: String!): OwnershipResponse! }",
      "removeScriptOwnerMutation",
      "external",
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
      registeredGraphQLOperations: 8,
      registeredMcpTools: 7,
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
