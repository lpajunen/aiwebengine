/// <reference path="../../assets/aiwebengine.d.ts" />

// JS test script: registers /js-log-test and writes a log entry

function js_log_test_handler(context) {
  console.log("js-log-test-called");
  return ResponseBuilder.text("logged");
}

// Initialization function
function init(context) {
  console.log("Initializing js_log_test.js at " + new Date().toISOString());
  routeRegistry.registerRoute("/js-log-test", "js_log_test_handler", "GET");
  console.log("JS log test endpoint registered");
  return { success: true };
}
