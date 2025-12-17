/// <reference path="../../assets/aiwebengine.d.ts" />

// Test endpoint for editor API
function testEditorAPI(context) {
  let result = "Testing editor API endpoints...\n\n";

  try {
    // Test 1: List scripts
    const scriptsJson =
      typeof scriptStorage !== "undefined" &&
      typeof scriptStorage.listScripts === "function"
        ? scriptStorage.listScripts()
        : "[]";
    const scriptMetadata = JSON.parse(scriptsJson);
    const scripts = scriptMetadata.map((meta) => meta.uri);
    result += "Available scripts: " + JSON.stringify(scripts) + "\n\n";
  } catch (error) {
    result += "Error listing scripts: " + error.message + "\n\n";
  }

  result += "Basic test completed.";

  return ResponseBuilder.text(result);
}

// Initialization function
function init(context) {
  console.log("Initializing test_editor_api.js at " + new Date().toISOString());
  routeRegistry.registerRoute("/test-editor-api", "testEditorAPI", "GET");
  console.log("Editor API test endpoint registered");
  return { success: true };
}
