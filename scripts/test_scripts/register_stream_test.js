// Test script for routeRegistry.registerStreamRoute functionality
// This script demonstrates the new streaming API

function stream_test_handler(context) {
  const req = context.request || {};
  console.log("stream_test_handler called");
  return ResponseBuilder.json({
    message: "Stream test endpoint",
    path: req.path,
    method: req.method,
  });
}

// Initialization function - called once when script is loaded
function init(context) {
  console.log(
    "Initializing register_stream_test.js at " + new Date().toISOString(),
  );

  // Test routeRegistry.registerStreamRoute function
  try {
    routeRegistry.registerStreamRoute("/test-stream");
    console.log("Successfully registered stream /test-stream");
  } catch (e) {
    console.log("Error registering stream: " + String(e));
  }

  // Test invalid stream paths
  try {
    routeRegistry.registerStreamRoute("invalid-path-no-slash");
    console.log("ERROR: Should have failed for invalid path");
  } catch (e) {
    console.log("Expected error for invalid path: " + String(e));
  }

  try {
    routeRegistry.registerStreamRoute("");
    console.log("ERROR: Should have failed for empty path");
  } catch (e) {
    console.log("Expected error for empty path: " + String(e));
  }

  // Register a regular handler for testing
  routeRegistry.registerRoute("/stream-test", "stream_test_handler", "GET");

  console.log(
    "routeRegistry.registerStreamRoute test script initialized successfully",
  );

  return { success: true };
}
