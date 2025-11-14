// Test script for selective broadcasting functionality
// This demonstrates the new routeRegistry.sendStreamMessageFiltered and graphQLRegistry.sendSubscriptionMessageFiltered functions

// Initialization function - called once when script is loaded
function init(context) {
  console.log(
    "Initializing test_selective_broadcasting.js at " +
      new Date().toISOString(),
  );

  // Register test stream endpoints
  routeRegistry.registerStreamRoute("/test/chat");
  routeRegistry.registerStreamRoute("/test/metadata-demo");

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
  const streamResult = routeRegistry.sendStreamMessageFiltered(
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
  const allResult = routeRegistry.sendStreamMessageFiltered(
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
  const result = routeRegistry.sendStreamMessageFiltered(
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
routeRegistry.registerRoute(
  "/test/selective/stream",
  "testSelectiveStreamBroadcasting",
  "POST",
);
routeRegistry.registerRoute(
  "/test/selective/subscription",
  "testSelectiveSubscriptionBroadcasting",
  "POST",
);
routeRegistry.registerRoute("/test/metadata-demo", "testMetadataDemo", "POST");

// Run the tests
testSelectiveStreamBroadcasting();
testSelectiveSubscriptionBroadcasting();

console.log("Selective broadcasting tests completed");
