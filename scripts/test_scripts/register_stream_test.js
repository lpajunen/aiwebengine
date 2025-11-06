// Test script for registerWebStream functionality
// This script demonstrates the new streaming API

function stream_test_handler(req) {
  console.log("stream_test_handler called");
  return {
    status: 200,
    body: JSON.stringify({
      message: "Stream test endpoint",
      path: req.path,
      method: req.method,
    }),
    contentType: "application/json",
  };
}

// Initialization function - called once when script is loaded
function init(context) {
  console.log(
    "Initializing register_stream_test.js at " + new Date().toISOString(),
  );

  // Test registerWebStream function
  try {
    registerWebStream("/test-stream");
    console.log("Successfully registered stream /test-stream");
  } catch (e) {
    console.log("Error registering stream: " + String(e));
  }

  // Test invalid stream paths
  try {
    registerWebStream("invalid-path-no-slash");
    console.log("ERROR: Should have failed for invalid path");
  } catch (e) {
    console.log("Expected error for invalid path: " + String(e));
  }

  try {
    registerWebStream("");
    console.log("ERROR: Should have failed for empty path");
  } catch (e) {
    console.log("Expected error for empty path: " + String(e));
  }

  // Register a regular handler for testing
  register("/stream-test", "stream_test_handler", "GET");

  console.log("registerWebStream test script initialized successfully");

  return { success: true };
}
