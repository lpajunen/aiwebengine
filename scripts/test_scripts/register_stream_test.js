// Test script for registerWebStream functionality
// This script demonstrates the new streaming API

function stream_test_handler(req) {
    writeLog('stream_test_handler called');
    return { 
        status: 200, 
        body: JSON.stringify({
            message: 'Stream test endpoint',
            path: req.path,
            method: req.method
        }),
        contentType: 'application/json'
    };
}

// Test registerWebStream function
try {
    registerWebStream('/test-stream');
    writeLog('Successfully registered stream /test-stream');
} catch (e) {
    writeLog('Error registering stream: ' + String(e));
}

// Test invalid stream paths
try {
    registerWebStream('invalid-path-no-slash');
    writeLog('ERROR: Should have failed for invalid path');
} catch (e) {
    writeLog('Expected error for invalid path: ' + String(e));
}

try {
    registerWebStream('');
    writeLog('ERROR: Should have failed for empty path');
} catch (e) {
    writeLog('Expected error for empty path: ' + String(e));
}

// Register a regular handler for testing
register('/stream-test', 'stream_test_handler', 'GET');

writeLog('registerWebStream test script loaded successfully');