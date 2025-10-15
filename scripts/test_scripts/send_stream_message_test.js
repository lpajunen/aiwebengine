// Test script for sendStreamMessage functionality
// This script demonstrates how to register a stream and send messages to it

// Initialization function - called once when script is loaded
function init(context) {
    writeLog('Initializing send_stream_message_test.js at ' + new Date().toISOString());
    
    // Register stream endpoints
    registerWebStream('/notifications');
    registerWebStream('/chat');
    
    writeLog('Stream endpoints registered successfully');
    return { success: true };
}

// Send different types of messages
function sendTestMessages() {
    // Simple text message
    sendStreamMessage('"Hello World!"');
    
    // JSON object message
    var notification = {
        type: "notification",
        title: "New Update",
        message: "System has been updated successfully",
        timestamp: new Date().toISOString(),
        priority: "high"
    };
    sendStreamMessage(JSON.stringify(notification));
    
    // Chat message
    var chatMessage = {
        type: "chat",
        user: "testUser",
        message: "Hello everyone!",
        channel: "general",
        timestamp: Date.now()
    };
    sendStreamMessage(JSON.stringify(chatMessage));
    
    // Status update
    var statusUpdate = {
        type: "status",
        service: "api-server",
        status: "healthy",
        uptime: 12345,
        metrics: {
            cpu: 45.2,
            memory: 67.8,
            requests_per_sec: 120
        }
    };
    sendStreamMessage(JSON.stringify(statusUpdate));
}

// Call the function to send test messages
sendTestMessages();

// Log completion
writeLog('Sent multiple test messages to all registered streams');
