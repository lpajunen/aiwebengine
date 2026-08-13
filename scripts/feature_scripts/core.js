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

// Initialization function - called when script is loaded or updated
function init(context) {
  try {
    console.log(`Initializing core.js script at ${new Date().toISOString()}`);
    console.log(`Init context: ${JSON.stringify(context)}`);

    // Note: the engine's own endpoints (script/asset management,
    // /engine/installed, /engine/openapi.json, /favicon.ico, MCP tools)
    // are implemented natively in Rust (src/engine_api.rs), and the
    // engine-served pages carry their own inline styles.

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
