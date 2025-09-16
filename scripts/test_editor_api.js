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

    try {
        // Test 2: Get a specific script
        const scriptUri = "https://example.com/core";
        const content = (typeof getScript === 'function') ? getScript(scriptUri) : "";
        result += "Core script content length: " + content.length + "\n";
        result += "Script starts with: " + content.substring(0, 50) + "...\n\n";
    } catch (error) {
        result += "Error getting script: " + error.message + "\n\n";
    }

    try {
        // Test 3: Save a script
        const testUri = "https://example.com/test_script";
        const testContent = "// Test script content\nfunction test() { return 'hello'; }";
        if (typeof upsertScript === 'function') {
            upsertScript(testUri, testContent);
            result += "Script saved successfully\n";

            // Verify it was saved
            const retrieved = getScript(testUri);
            result += "Retrieved script matches: " + (retrieved === testContent) + "\n";
        } else {
            result += "upsertScript function not available\n";
        }
    } catch (error) {
        result += "Error saving script: " + error.message + "\n";
    }

    result += "\nEditor API tests completed.";

    return {
        status: 200,
        body: result,
        contentType: 'text/plain'
    };
}

// Register the test endpoint
register('/test-editor-api', 'testEditorAPI', 'GET');