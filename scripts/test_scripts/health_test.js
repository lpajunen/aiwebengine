// Health check test script
function health_test_handler(context) {
  try {
    // Test basic health check functionality
    const timestamp = new Date().toISOString();

    // Simulate a health check response
    const healthResponse = {
      status: "healthy",
      timestamp: timestamp,
      checks: {
        javascript: "ok",
        logging: "ok",
        json: "ok",
      },
    };

    return ResponseBuilder.json(healthResponse);
  } catch (error) {
    return ResponseBuilder.json(
      {
        status: "error",
        error: error.message,
      },
      500,
    );
  }
}

// Initialization function
function init(context) {
  console.log("Initializing health_test.js at " + new Date().toISOString());
  routeRegistry.registerRoute("/health-test", "health_test_handler", "GET");
  console.log("Health test endpoint registered");
  return { success: true };
}
