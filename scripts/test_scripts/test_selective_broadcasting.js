// Test script for selective broadcasting functionality
// This demonstrates the new sendStreamMessageToConnections and sendSubscriptionMessageToConnections functions

// Initialization function - called once when script is loaded
function init(context) {
  writeLog(
    "Initializing test_selective_broadcasting.js at " +
      new Date().toISOString(),
  );

  // Register test stream endpoints
  registerWebStream("/test/chat");

  // Register GraphQL subscription for testing
  registerGraphQLSubscription(
    "chatMessages",
    "type Subscription { chatMessages: String }",
    "chatMessagesResolver",
  );

  writeLog("Selective broadcasting test endpoints registered successfully");
  return { success: true };
}

// GraphQL subscription resolver
function chatMessagesResolver() {
  return "Chat messages subscription active";
}

// Test selective broadcasting to stream connections
function testSelectiveStreamBroadcasting() {
  writeLog("Testing selective stream broadcasting...");

  // This would send to connections where metadata.user_id == "user123" and metadata.room == "general"
  const streamResult = sendStreamMessageToConnections(
    "/test/chat",
    JSON.stringify({
      type: "selective_chat",
      message: "Hello selective chat!",
      timestamp: new Date().toISOString(),
    }),
    JSON.stringify({ user_id: "user123", room: "general" }),
  );

  writeLog("Stream broadcast result: " + streamResult);

  // Test broadcasting to all connections (empty filter)
  const allResult = sendStreamMessageToConnections(
    "/test/chat",
    JSON.stringify({
      type: "broadcast_all",
      message: "Hello everyone!",
      timestamp: new Date().toISOString(),
    }),
    "{}", // Empty filter matches all
  );

  writeLog("Broadcast to all result: " + allResult);
}

// Test selective broadcasting to GraphQL subscription connections
function testSelectiveSubscriptionBroadcasting() {
  writeLog("Testing selective subscription broadcasting...");

  // This would send to connections where metadata.user_id == "user456"
  const subscriptionResult = sendSubscriptionMessageToConnections(
    "chatMessages",
    JSON.stringify({
      type: "selective_subscription",
      message: "Hello selective subscription!",
      timestamp: new Date().toISOString(),
    }),
    JSON.stringify({ user_id: "user456" }),
  );

  writeLog("Subscription broadcast result: " + subscriptionResult);
}

// Run the tests
testSelectiveStreamBroadcasting();
testSelectiveSubscriptionBroadcasting();

writeLog("Selective broadcasting tests completed");
