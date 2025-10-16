// JS script for testing script-management host functions

// rely on bootstrap-provided register/handle which supports handler name strings

// JS script for testing script-management host functions

// rely on bootstrap-provided register/handle which supports handler name strings

// JS script for testing script-management host functions

// JS script for testing script-management host functions

// Create a simple test script that doesn't register routes to avoid interference
const testScriptContent =
  "// Test script content\nfunction test_func() { return 'test'; }";

// Create and test script management operations
try {
  upsertScript("https://example.com/from_js", testScriptContent);
} catch (e) {
  // ignore errors when host function not available
}

// route that exercises getScript, listScripts, deleteScript
function js_mgmt_check(req) {
  try {
    const got =
      typeof getScript === "function"
        ? (getScript("https://example.com/from_js") ?? null)
        : null;
    const list = typeof listScripts === "function" ? (listScripts() ?? []) : [];
    const deleted_before =
      typeof deleteScript === "function"
        ? !!deleteScript("https://example.com/does-not-exist")
        : false;
    const deleted =
      typeof deleteScript === "function"
        ? !!deleteScript("https://example.com/from_js")
        : false;
    const after =
      typeof getScript === "function"
        ? (getScript("https://example.com/from_js") ?? null)
        : null;

    return {
      status: 200,
      body: JSON.stringify({ got, list, deleted_before, deleted, after }),
    };
  } catch (e) {
    return { status: 500, body: String(e) };
  }
}

// Initialization function
function init(context) {
  writeLog(
    "Initializing js_script_mgmt_test.js at " + new Date().toISOString(),
  );
  register("/js-mgmt-check", "js_mgmt_check", "GET");
  writeLog("JS script management test endpoint registered");
  return { success: true };
}
