/// <reference path="../../assets/aiwebengine.d.ts" />

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
  if (
    typeof scriptStorage !== "undefined" &&
    typeof scriptStorage.upsertScript === "function"
  ) {
    scriptStorage.upsertScript(
      "https://example.com/from_js",
      testScriptContent,
    );
  }
} catch (e) {
  // ignore errors when host function not available
}

// route that exercises getScript, listScripts, deleteScript
function js_mgmt_check(context) {
  try {
    const got =
      typeof scriptStorage !== "undefined" &&
      typeof scriptStorage.getScript === "function"
        ? (scriptStorage.getScript("https://example.com/from_js") ?? null)
        : null;
    const listJson =
      typeof scriptStorage !== "undefined" &&
      typeof scriptStorage.listScripts === "function"
        ? (scriptStorage.listScripts() ?? "[]")
        : "[]";
    const listMetadata = JSON.parse(listJson);
    const list = listMetadata.map((meta) => meta.uri);
    const deleted_before =
      typeof scriptStorage !== "undefined" &&
      typeof scriptStorage.deleteScript === "function"
        ? !!scriptStorage.deleteScript("https://example.com/does-not-exist")
        : false;
    const deleted =
      typeof scriptStorage !== "undefined" &&
      typeof scriptStorage.deleteScript === "function"
        ? !!scriptStorage.deleteScript("https://example.com/from_js")
        : false;
    const after =
      typeof scriptStorage !== "undefined" &&
      typeof scriptStorage.getScript === "function"
        ? (scriptStorage.getScript("https://example.com/from_js") ?? null)
        : null;

    return ResponseBuilder.json({ got, list, deleted_before, deleted, after });
  } catch (e) {
    return ResponseBuilder.error(500, String(e));
  }
}

// Initialization function
function init(context) {
  console.log(
    "Initializing js_script_mgmt_test.js at " + new Date().toISOString(),
  );
  routeRegistry.registerRoute("/js-mgmt-check", "js_mgmt_check", "GET");
  console.log("JS script management test endpoint registered");
  return { success: true };
}
