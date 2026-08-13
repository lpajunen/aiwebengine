/// <reference path="../../assets/aiwebengine-priv.d.ts" />

function getRequest(context) {
  return (context && context.request) || {};
}

function logServerStarted() {
  console.log("server started");
}

function logServerStillRunning() {
  console.log("server still running");
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

// Stream customization function for script_updates
// Returns connection filter criteria based on request context.
// This function is called once when a client connects to the stream.
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
    // Note: script and asset management endpoints (/upsert_script,
    // /delete_script, /read_script, /script_logs, /assets) and the
    // engine MCP tools are implemented natively in Rust (src/engine_api.rs).
    routeRegistry.registerRoute("/engine/installed", "installed_page", "GET", {
      summary: "Installation confirmation",
      description: "Shows a confirmation page for successful installation",
      tags: ["Engine"],
    });
    routeRegistry.registerRoute("/engine/openapi.json", "openapiSpec", "GET", {
      summary: "OpenAPI Specification",
      description:
        "Returns the OpenAPI 3.0 specification for all registered routes",
      tags: ["Engine"],
    });

    // Register the script update stream endpoint with customization function.
    // The Rust engine broadcasts script_update messages to this stream.
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

    console.log("Core script initialized successfully");

    return {
      success: true,
      message: "Core script initialized successfully",
      registeredEndpoints: 2,
      registeredAssets: 3,
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
