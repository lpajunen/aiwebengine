// JS test script: registers /js-log-test and uses writeLog

function js_log_test_handler(context) {
  console.log("js-log-test-called");
  return { status: 200, body: "logged" };
}

function js_list_handler(context) {
  try {
    const logs = console.listLogs();
    return { status: 200, body: JSON.stringify(logs) };
  } catch (e) {
    return { status: 500, body: String(e) };
  }
}

// Initialization function
function init(context) {
  console.log("Initializing js_log_test.js at " + new Date().toISOString());
  routeRegistry.registerRoute("/js-log-test", "js_log_test_handler", "GET");
  routeRegistry.registerRoute("/js-list", "js_list_handler", "GET");
  console.log("JS log test endpoints registered");
  return { success: true };
}
