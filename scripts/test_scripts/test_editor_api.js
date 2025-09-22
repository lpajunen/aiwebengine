// Test endpoint for editor API
function testEditorAPI(req) {
    let result = "Testing editor API endpoints...\n\n";

    try {
        // Test 1: List scripts
        const scripts = (typeof listScripts === 'function') ? listScripts() : [];
        result += "Available scripts: " + JSON.stringify(scripts) + "\n\n";
    } catch (error) {
        result += "Error listing scripts: " + error.message + "\n\n";
    }

    result += "Basic test completed.";

    return {
        status: 200,
        body: result,
        contentType: 'text/plain'
    };
}

// Register the test endpoint
register('/test-editor-api', 'testEditorAPI', 'GET');