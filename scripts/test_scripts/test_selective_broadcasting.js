// Test script for selective broadcasting functionality
// This demonstrates the new sendStreamMessageToConnections and sendSubscriptionMessageToConnections functions

// Initialization function - called once when script is loaded
function init(context) {
  console.log(
    "Initializing test_selective_broadcasting.js at " +
      new Date().toISOString(),
  );

  // Register test stream endpoints
  registerWebStream("/test/chat");
  registerWebStream("/test/metadata-demo");

  // Register GraphQL subscription for testing
  graphQLRegistry.registerSubscription(
    "chatMessages",
    "type Subscription { chatMessages: String }",
    "chatMessagesResolver",
  );

  console.log("Selective broadcasting test endpoints registered successfully");
  return { success: true };
}

// GraphQL subscription resolver
function chatMessagesResolver() {
  return "Chat messages subscription active";
}

// Test selective broadcasting to stream connections
function testSelectiveStreamBroadcasting() {
  console.log("Testing selective stream broadcasting...");

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

  console.log("Stream broadcast result: " + streamResult);

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

  console.log("Broadcast to all result: " + allResult);
}

// Test selective broadcasting to GraphQL subscription connections
function testSelectiveSubscriptionBroadcasting() {
  console.log("Testing selective subscription broadcasting...");

  // This would send to connections where metadata.user_id == "user456"
  const subscriptionResult = graphQLRegistry.sendSubscriptionMessageFiltered(
    "chatMessages",
    JSON.stringify({
      type: "selective_subscription",
      message: "Hello selective subscription!",
      timestamp: new Date().toISOString(),
    }),
    JSON.stringify({ user_id: "user456" }),
  );

  console.log("Subscription broadcast result: " + subscriptionResult);
}

// Test metadata parsing from query parameters
function testMetadataDemo(req) {
  console.log("Testing metadata demo endpoint...");

  // Send a message that will be filtered by the metadata from query params
  const result = sendStreamMessageToConnections(
    "/test/metadata-demo",
    JSON.stringify({
      type: "metadata_test",
      message: "This message is filtered by query parameter metadata!",
      timestamp: new Date().toISOString(),
      server_received_params: req.query,
    }),
    JSON.stringify({ demo: "true" }), // Only send to connections with demo=true
  );

  console.log("Metadata demo broadcast result: " + result);

  return {
    status: 200,
    body: JSON.stringify({
      message: "Metadata demo triggered",
      broadcast_result: result,
      your_query_params: req.query,
    }),
  };
}

// Register test endpoints
register("/test/selective/stream", "testSelectiveStreamBroadcasting", "POST");
register(
  "/test/selective/subscription",
  "testSelectiveSubscriptionBroadcasting",
  "POST",
);
register("/test/metadata-demo", "testMetadataDemo", "POST");

// Run the tests
testSelectiveStreamBroadcasting();
testSelectiveSubscriptionBroadcasting();

console.log("Selective broadcasting tests completed");
