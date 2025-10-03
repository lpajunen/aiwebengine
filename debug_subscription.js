// Debug script to test GraphQL subscription message sending

// First register the subscription (like core.js does)
registerGraphQLSubscription(
    "testSubscription", 
    "type Subscription { testSubscription: String }", 
    "testSubscriptionResolver"
);

// Register a test mutation 
registerGraphQLMutation(
    "sendTestMessage", 
    "type Mutation { sendTestMessage(message: String!): String }", 
    "sendTestMessageResolver"
);

function testSubscriptionResolver() {
    writeLog("Client subscribed to testSubscription");
    return "Test subscription initialized";
}

function sendTestMessageResolver(args) {
    writeLog(`Sending test message: ${args.message}`);
    
    // Try sending a subscription message
    try {
        sendSubscriptionMessage("testSubscription", JSON.stringify({
            type: "test_message",
            message: args.message,
            timestamp: new Date().toISOString()
        }));
        writeLog("sendSubscriptionMessage called successfully");
        return `Test message sent: ${args.message}`;
    } catch (error) {
        writeLog(`Error in sendSubscriptionMessage: ${error.message}`);
        return `Error: ${error.message}`;
    }
}