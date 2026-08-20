/// <reference path="../../assets/aiwebengine.d.ts" />

// Test endpoint for editor API
function testEditorAPI(context) {
  let result = "Testing editor API endpoints...\n\n";

  try {
    // Test 1: List this script's assets
    const assetsJson =
      typeof assetStorage !== "undefined" &&
      typeof assetStorage.listAssets === "function"
        ? assetStorage.listAssets()
        : "[]";
    const assetMetadata = JSON.parse(assetsJson);
    const assets = assetMetadata.map((meta) => meta.name);
    result += "Available assets: " + JSON.stringify(assets) + "\n\n";
  } catch (error) {
    result += "Error listing assets: " + error.message + "\n\n";
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
