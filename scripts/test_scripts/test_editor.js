// Test script for the aiwebengine editor
// This script tests the editor API endpoints

function testEditorAPI(req) {
  const testResults = [];

  try {
    // Test 1: List scripts
    testResults.push("Testing script listing...");
    const scripts =
      typeof scriptStorage !== "undefined" &&
      typeof scriptStorage.listScripts === "function"
        ? scriptStorage.listScripts()
        : [];
    testResults.push(`Found ${scripts.length} scripts: ${scripts.join(", ")}`);

    // Test 2: List assets
    testResults.push("Testing asset listing...");
    const assets =
      typeof assetStorage !== "undefined" &&
      typeof assetStorage.listAssets === "function"
        ? assetStorage.listAssets()
        : [];
    testResults.push(`Found ${assets.length} assets: ${assets.join(", ")}`);

    // Test 3: List logs
    testResults.push("Testing log listing...");
    const logs =
      typeof logStorage !== "undefined" &&
      typeof logStorage.listLogs === "function"
        ? logStorage.listLogs()
        : [];
    testResults.push(`Found ${logs.length} log entries`);

    // Test 4: Check if editor files exist
    testResults.push("Checking editor files...");
    // Note: editor.html is not a public asset - it's served via /editor endpoint
    const editorCss =
      typeof assetStorage !== "undefined" &&
      typeof assetStorage.getAsset === "function"
        ? assetStorage.getAsset("/editor.css")
        : null;
    const editorJs =
      typeof assetStorage !== "undefined" &&
      typeof assetStorage.getAsset === "function"
        ? assetStorage.getAsset("/editor.js")
        : null;

    testResults.push(`Editor CSS: ${editorCss !== null ? "Found" : "Missing"}`);
    testResults.push(`Editor JS: ${editorJs !== null ? "Found" : "Missing"}`);

    return {
      status: 200,
      body: JSON.stringify(
        {
          success: true,
          message: "Editor API test completed",
          results: testResults,
        },
        null,
        2,
      ),
      contentType: "application/json",
    };
  } catch (error) {
    return {
      status: 500,
      body: JSON.stringify(
        {
          success: false,
          error: error.message,
          results: testResults,
        },
        null,
        2,
      ),
      contentType: "application/json",
    };
  }
}

// Initialization function
function init(context) {
  console.log("Initializing test_editor.js at " + new Date().toISOString());
  routeRegistry.registerRoute("/test-editor", "testEditorAPI", "GET");
  console.log("Editor test endpoint registered");
  return { success: true };
}
