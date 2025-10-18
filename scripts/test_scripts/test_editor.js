// Test script for the aiwebengine editor
// This script tests the editor API endpoints

function testEditorAPI(req) {
  const testResults = [];

  try {
    // Test 1: List scripts
    testResults.push("Testing script listing...");
    const scripts = listScripts();
    testResults.push(`Found ${scripts.length} scripts: ${scripts.join(", ")}`);

    // Test 2: List assets
    testResults.push("Testing asset listing...");
    const assets = listAssets();
    testResults.push(`Found ${assets.length} assets: ${assets.join(", ")}`);

    // Test 3: List logs
    testResults.push("Testing log listing...");
    const logs = listLogs();
    testResults.push(`Found ${logs.length} log entries`);

    // Test 4: Check if editor files exist
    testResults.push("Checking editor files...");
    // Note: editor.html is not a public asset - it's served via /editor endpoint
    const editorCss = fetchAsset("/editor.css");
    const editorJs = fetchAsset("/editor.js");

    testResults.push(
      `Editor CSS: ${editorCss !== "null" ? "Found" : "Missing"}`,
    );
    testResults.push(`Editor JS: ${editorJs !== "null" ? "Found" : "Missing"}`);

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
  writeLog("Initializing test_editor.js at " + new Date().toISOString());
  register("/test-editor", "testEditorAPI", "GET");
  writeLog("Editor test endpoint registered");
  return { success: true };
}
