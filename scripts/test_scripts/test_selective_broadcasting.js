/// <reference path="../../assets/aiwebengine.d.ts" />

// Test script for selective broadcasting functionality
// NEW SEMANTICS: Customization functions return connection filter criteria
// Message metadata is matched against connection criteria for delivery

// Stream customization functions return filter criteria for connections
function chatStreamCustomizer(context) {
  const req = context.request || {};
  console.log("Customizing /test/chat connection");
  console.log("Query params: " + JSON.stringify(req.query));

  // Return filter criteria based on query parameters
  // This connection will only receive messages with matching metadata
  var filter = {};
  if (req.query.user_id) {
    filter.user_id = req.query.user_id;
  }
  if (req.query.room) {
    filter.room = req.query.room;
  }

  console.log("Connection filter criteria: " + JSON.stringify(filter));
  return filter;
}

function metadataDemoCustomizer(context) {
  const req = context.request || {};
  console.log("Customizing /test/metadata-demo connection");

  // Only connections with ?demo=true will receive messages with demo: "true"
  if (req.query.demo === "true") {
    return { demo: "true" };
  }

  // Return empty to receive all messages
  return {};
}

// GraphQL subscription resolver now returns filter criteria
function chatMessagesResolver(context) {
  const req = context.request || {};
  const args = context.args || {};
  console.log("Client subscribed to chatMessages GraphQL subscription");
  console.log("Request context: " + JSON.stringify(req));

  // Return filter criteria based on query params
  var filter = {};
  if (req.query && req.query.user_id) {
    filter.user_id = req.query.user_id;
  }

  console.log("Subscription filter criteria: " + JSON.stringify(filter));
  return filter;
}

// Initialization function - called once when script is loaded
function init(context) {
  console.log(
    "Initializing test_selective_broadcasting.js at " +
      new Date().toISOString(),
  );

  // Register test stream endpoints WITH customization functions
  routeRegistry.registerStreamRoute("/test/chat", "chatStreamCustomizer");
  routeRegistry.registerStreamRoute(
    "/test/metadata-demo",
    "metadataDemoCustomizer",
  );

  // Register GraphQL subscription (external - used by clients for testing)
  graphQLRegistry.registerSubscription(
    "chatMessages",
    "type Subscription { chatMessages: String }",
    "chatMessagesResolver",
    "external",
  );

  console.log("Selective broadcasting test endpoints registered successfully");
  return { success: true };
}

// Test selective broadcasting to stream connections
// NEW: Message metadata is matched against connection filter criteria
function testSelectiveStreamBroadcasting() {
  console.log("Testing selective stream broadcasting...");

  // Send message with metadata: user_id and room
  // This will be delivered to connections whose filter criteria match
  // (i.e., connections that require user_id="user123" AND room="general")
  const streamResult = routeRegistry.sendStreamMessageFiltered(
    "/test/chat",
    JSON.stringify({
      type: "selective_chat",
      message: "Hello selective chat!",
      timestamp: new Date().toISOString(),
    }),
    JSON.stringify({ user_id: "user123", room: "general" }), // Message metadata
  );

  console.log("Stream broadcast result: " + streamResult);

  // Test broadcasting with minimal metadata
  // Only connections with empty filter or subset of this metadata will receive
  const allResult = routeRegistry.sendStreamMessageFiltered(
    "/test/chat",
    JSON.stringify({
      type: "broadcast_all",
      message: "Hello everyone!",
      timestamp: new Date().toISOString(),
    }),
    JSON.stringify({}), // Empty metadata matches connections with empty filter
  );

  console.log("Broadcast result: " + allResult);
}

// Test selective broadcasting to GraphQL subscription connections
function testSelectiveSubscriptionBroadcasting() {
  console.log("Testing selective subscription broadcasting...");

  // Send message with user_id metadata
  // Delivered to connections whose filter requires user_id="user456"
  const subscriptionResult = graphQLRegistry.sendSubscriptionMessageFiltered(
    "chatMessages",
    JSON.stringify({
      type: "selective_subscription",
      message: "Hello selective subscription!",
      timestamp: new Date().toISOString(),
    }),
    JSON.stringify({ user_id: "user456" }), // Message metadata
  );

  console.log("Subscription broadcast result: " + subscriptionResult);
}

// Test metadata parsing from query parameters
function testMetadataDemo(context) {
  const req = context.request || {};
  console.log("Testing metadata demo endpoint...");

  // Send a message with demo metadata
  // Only connections with demo="true" in their filter will receive this
  const result = routeRegistry.sendStreamMessageFiltered(
    "/test/metadata-demo",
    JSON.stringify({
      type: "metadata_test",
      message: "This message is filtered by server-side connection criteria!",
      timestamp: new Date().toISOString(),
      server_received_params: req.query,
    }),
    JSON.stringify({ demo: "true" }), // Message metadata
  );

  console.log("Metadata demo broadcast result: " + result);

  return {
    status: 200,
    body: JSON.stringify({
      message: "Metadata demo triggered",
      broadcast_result: result,
      your_query_params: req.query,
      explanation: "Connections with ?demo=true will receive this message",
    }),
    contentType: "application/json",
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
