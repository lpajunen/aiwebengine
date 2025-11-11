// Test script demonstrating the new asset registration system
// This script shows how to register assets to public HTTP paths

function serveAssetTestPage(req) {
  const html = `
    <!DOCTYPE html>
    <html>
    <head>
      <title>Asset Registration Test</title>
      <link rel="stylesheet" href="/test/styles.css">
    </head>
    <body>
      <h1>Asset Registration System Test</h1>
      <p>This page demonstrates the new asset registration system.</p>
      <img src="/test/sample-image.svg" alt="Test Image">
      <p>Logo also available at alternate path:</p>
      <img src="/alternate/logo.svg" alt="Logo" width="100">
      <script src="/test/script.js"></script>
    </body>
    </html>
  `;

  return {
    status: 200,
    body: html,
    contentType: "text/html; charset=UTF-8",
  };
}

function uploadTestAsset(req) {
  try {
    // Create a simple CSS asset
    const cssContent = `
      body { font-family: Arial, sans-serif; margin: 20px; }
      h1 { color: #333; }
      p { color: #666; }
    `;

    // Create a simple SVG asset
    const svgContent = `
      <svg width="100" height="100" xmlns="http://www.w3.org/2000/svg">
        <circle cx="50" cy="50" r="40" fill="blue" />
      </svg>
    `;

    // Create a simple JavaScript asset
    const jsContent = `
      console.log('Asset registration test script loaded');
      document.addEventListener('DOMContentLoaded', function() {
        console.log('Page loaded with new asset system');
      });
    `;

    // Convert to base64
    const cssB64 = btoa(cssContent);
    const svgB64 = btoa(svgContent);
    const jsB64 = btoa(jsContent);

    // Upload assets using new system (asset names only, no paths)
    upsertAsset("test-styles.css", cssB64, "text/css");
    upsertAsset("test-image.svg", svgB64, "image/svg+xml");
    upsertAsset("test-script.js", jsB64, "application/javascript");

    console.log("Test assets uploaded successfully");

    return {
      status: 200,
      body: JSON.stringify({
        success: true,
        message: "Test assets uploaded successfully",
        assets: ["test-styles.css", "test-image.svg", "test-script.js"],
      }),
      contentType: "application/json",
    };
  } catch (error) {
    console.error("Failed to upload test assets: " + error.message);
    return {
      status: 500,
      body: JSON.stringify({ error: error.message }),
      contentType: "application/json",
    };
  }
}

// Initialization function
function init(context) {
  console.log(
    "Initializing asset_registration_test.js at " + new Date().toISOString(),
  );

  // Register HTTP routes
  register("/asset-test", "serveAssetTestPage", "GET");
  register("/asset-test/upload", "uploadTestAsset", "POST");

  // Register public asset paths
  // These map HTTP paths to asset names in the repository

  // Map /test/* paths to test assets
  registerPublicAsset("/test/styles.css", "test-styles.css");
  registerPublicAsset("/test/sample-image.svg", "test-image.svg");
  registerPublicAsset("/test/script.js", "test-script.js");

  // Demonstrate: Same asset at multiple HTTP paths
  // Register the built-in logo at an alternate path
  registerPublicAsset("/alternate/logo.svg", "logo.svg");

  console.log("Asset registration test endpoints configured");

  return {
    success: true,
    message: "Asset registration test initialized",
    registeredRoutes: 2,
    registeredAssetPaths: 4,
  };
}
