// JS test script: tests console.listLogsForUri function

function list_logs_for_uri_handler(req) {
  try {
    // Test with the current script's URI
    const current_logs = console.listLogsForUri(
      "https://example.com/js-log-test-uri",
    );
    // Test with a different URI
    const other_logs = console.listLogsForUri(
      "https://example.com/other-script",
    );
    return {
      status: 200,
      body: JSON.stringify({
        current: current_logs,
        other: other_logs,
      }),
      contentType: "application/json",
    };
  } catch (e) {
    return { status: 500, body: String(e) };
  }
}

// Initialization function
function init(context) {
  console.log("Initializing js_log_test_uri.js at " + new Date().toISOString());
  register("/js-list-for-uri", "list_logs_for_uri_handler", "GET");
  console.log("JS log test URI endpoint registered");
  return { success: true };
}
