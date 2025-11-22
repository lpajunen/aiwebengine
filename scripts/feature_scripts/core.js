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
    var dbHealthJson = checkDatabaseHealth();
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

    const scriptArray = scriptMetadata.map((meta) => ({
      uri: meta.uri,
      chars: meta.size || 0,
    }));

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

    // getScript returns null if script not found
    if (content !== null && content !== undefined) {
      return JSON.stringify({
        uri: args.uri,
        content: content,
        contentLength: content.length,
        logs: logs,
      });
    } else {
      // Return null if script doesn't exist
      return JSON.stringify({
        uri: args.uri,
        content: null,
        contentLength: 0,
        logs: logs,
      });
    }
  } catch (error) {
    console.error(`Script query failed: ${error.message}`);
    return JSON.stringify({
      uri: args.uri,
      content: null,
      contentLength: 0,
      logs: [],
    });
  }
}

function scriptInitStatusQuery(context) {
  const req = getRequest(context);
  const args = getArgs(context);
  try {
    const status = getScriptInitStatus(args.uri);
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

// GraphQL resolvers are now handled in a separate script

// Initialization function - called when script is loaded or updated
function init(context) {
  try {
    console.log(`Initializing core.js script at ${new Date().toISOString()}`);
    console.log(`Init context: ${JSON.stringify(context)}`);

    // Register public asset paths
    routeRegistry.registerAssetRoute("/logo.svg", "logo.svg");
    routeRegistry.registerAssetRoute("/favicon.ico", "favicon.ico");
    routeRegistry.registerAssetRoute("/editor.css", "editor.css");
    routeRegistry.registerAssetRoute("/editor.js", "editor.js");
    routeRegistry.registerAssetRoute("/engine.css", "engine.css");

    // Register HTTP endpoints with OpenAPI metadata
    routeRegistry.registerRoute("/health", "health_check", "GET", {
      summary: "Health check",
      description:
        "Returns system health status including database connectivity",
      tags: ["Monitoring"],
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

    // Register GraphQL subscription
    graphQLRegistry.registerSubscription(
      "scriptUpdates",
      "type ScriptUpdate { type: String!, uri: String!, action: String!, timestamp: String!, contentLength: Int, previousExists: Boolean, via: String } type Subscription { scriptUpdates: ScriptUpdate! }",
      "scriptUpdatesResolver",
    );

    // Register GraphQL queries
    graphQLRegistry.registerQuery(
      "scripts",
      "type ScriptInfo { uri: String!, chars: Int! } type Query { scripts: [ScriptInfo!]! }",
      "scriptsQuery",
    );
    graphQLRegistry.registerQuery(
      "script",
      "type ScriptDetail { uri: String!, content: String!, contentLength: Int!, logs: [String!]! } type Query { script(uri: String!): ScriptDetail }",
      "scriptQuery",
    );
    graphQLRegistry.registerQuery(
      "scriptInitStatus",
      "type ScriptInitStatus { scriptName: String!, initialized: Boolean!, initError: String, lastInitTime: Float, createdAt: Float, updatedAt: Float } type Query { scriptInitStatus(uri: String!): ScriptInitStatus }",
      "scriptInitStatusQuery",
    );
    graphQLRegistry.registerQuery(
      "allScriptsInitStatus",
      "type ScriptInitStatus { scriptName: String!, initialized: Boolean!, initError: String, lastInitTime: Float, createdAt: Float, updatedAt: Float } type Query { allScriptsInitStatus: [ScriptInitStatus!]! }",
      "allScriptsInitStatusQuery",
    );

    // Register GraphQL mutations
    graphQLRegistry.registerMutation(
      "upsertScript",
      "type UpsertScriptResponse { message: String!, uri: String!, chars: Int!, success: Boolean! } type Mutation { upsertScript(uri: String!, content: String!): UpsertScriptResponse! }",
      "upsertScriptMutation",
    );
    graphQLRegistry.registerMutation(
      "deleteScript",
      "type DeleteScriptResponse { message: String!, uri: String!, success: Boolean! } type Mutation { deleteScript(uri: String!): DeleteScriptResponse! }",
      "deleteScriptMutation",
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

    console.log("Core script initialized successfully");

    return {
      success: true,
      message: "Core script initialized successfully",
      registeredEndpoints: 8,
      registeredAssets: 5,
      registeredGraphQLOperations: 8,
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
