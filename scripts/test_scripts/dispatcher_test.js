/**
 * Dispatcher Test Script
 *
 * Tests the message dispatcher functionality for inter-script communication
 */

// Track received messages for testing
let receivedMessages = [];

// Handler for user.created messages
function handleUserCreated(context) {
  console.log(
    "handleUserCreated called with context:",
    JSON.stringify(context),
  );
  receivedMessages.push({
    handler: "handleUserCreated",
    messageType: context.messageType,
    messageData: context.messageData,
    timestamp: new Date().toISOString(),
  });
}

// Handler for message.sent messages
function handleMessageSent(context) {
  console.log(
    "handleMessageSent called with context:",
    JSON.stringify(context),
  );
  receivedMessages.push({
    handler: "handleMessageSent",
    messageType: context.messageType,
    messageData: context.messageData,
    timestamp: new Date().toISOString(),
  });
}

// Handler for testing multiple listeners on same message type
function handleUserCreatedSecondary(context) {
  console.log("handleUserCreatedSecondary called");
  receivedMessages.push({
    handler: "handleUserCreatedSecondary",
    messageType: context.messageType,
    messageData: context.messageData,
    timestamp: new Date().toISOString(),
  });
}

// Handler for testing error handling
function handleErrorTest(context) {
  console.log("handleErrorTest called - intentionally throwing error");
  throw new Error("Intentional test error");
}

/**
 * Test basic listener registration
 */
function testRegisterListener(context) {
  const results = [];

  try {
    // Test registering a listener
    const result1 = dispatcher.registerListener(
      "user.created",
      "handleUserCreated",
    );
    results.push({
      test: "registerListener - valid",
      success: true,
      result: result1,
    });
  } catch (error) {
    results.push({
      test: "registerListener - valid",
      success: false,
      error: error.message,
    });
  }

  try {
    // Test empty message type (should fail)
    const result2 = dispatcher.registerListener("", "handleUserCreated");
    results.push({
      test: "registerListener - empty message type",
      success: result2.includes("cannot be empty"),
      result: result2,
    });
  } catch (error) {
    results.push({
      test: "registerListener - empty message type",
      success: false,
      error: error.message,
    });
  }

  try {
    // Test empty handler name (should fail)
    const result3 = dispatcher.registerListener("test.event", "");
    results.push({
      test: "registerListener - empty handler",
      success: result3.includes("cannot be empty"),
      result: result3,
    });
  } catch (error) {
    results.push({
      test: "registerListener - empty handler",
      success: false,
      error: error.message,
    });
  }

  return {
    status: 200,
    body: JSON.stringify({ success: true, results: results }),
    contentType: "application/json",
  };
}

/**
 * Test sending messages
 */
function testSendMessage(context) {
  const results = [];

  // First register a listener
  dispatcher.registerListener("test.message", "handleMessageSent");

  try {
    // Send a message with data
    const messageData = JSON.stringify({
      text: "Hello World",
      timestamp: new Date().toISOString(),
      userId: 123,
    });

    const result1 = dispatcher.sendMessage("test.message", messageData);
    results.push({
      test: "sendMessage - with data",
      success: true,
      result: result1,
      receivedCount: receivedMessages.filter(
        (m) => m.messageType === "test.message",
      ).length,
    });
  } catch (error) {
    results.push({
      test: "sendMessage - with data",
      success: false,
      error: error.message,
    });
  }

  try {
    // Send message to non-existent listeners
    const result2 = dispatcher.sendMessage("nonexistent.event");
    results.push({
      test: "sendMessage - no listeners",
      success: result2.includes("No listeners"),
      result: result2,
    });
  } catch (error) {
    results.push({
      test: "sendMessage - no listeners",
      success: false,
      error: error.message,
    });
  }

  try {
    // Send with empty message type (should fail)
    const result3 = dispatcher.sendMessage("");
    results.push({
      test: "sendMessage - empty message type",
      success: result3.includes("cannot be empty"),
      result: result3,
    });
  } catch (error) {
    results.push({
      test: "sendMessage - empty message type",
      success: false,
      error: error.message,
    });
  }

  return {
    status: 200,
    body: JSON.stringify({ success: true, results: results }),
    contentType: "application/json",
  };
}

/**
 * Test multiple handlers for same message type
 */
function testMultipleHandlers(context) {
  const results = [];

  // Clear received messages
  receivedMessages = [];

  try {
    // Register multiple handlers for same message type
    dispatcher.registerListener("user.created", "handleUserCreated");
    dispatcher.registerListener("user.created", "handleUserCreatedSecondary");

    // Send a message
    const messageData = JSON.stringify({
      username: "testuser",
      email: "test@example.com",
      id: 456,
    });

    const result = dispatcher.sendMessage("user.created", messageData);

    // Wait a moment for handlers to execute
    // Note: In real async scenarios, you'd want proper synchronization

    results.push({
      test: "multiple handlers",
      success: true,
      result: result,
      receivedCount: receivedMessages.filter(
        (m) => m.messageType === "user.created",
      ).length,
      receivedMessages: receivedMessages.filter(
        (m) => m.messageType === "user.created",
      ),
    });
  } catch (error) {
    results.push({
      test: "multiple handlers",
      success: false,
      error: error.message,
    });
  }

  return {
    status: 200,
    body: JSON.stringify({ success: true, results: results }),
    contentType: "application/json",
  };
}

/**
 * Test message data serialization
 */
function testMessageDataSerialization(context) {
  const results = [];

  // Clear received messages
  receivedMessages = [];

  dispatcher.registerListener("data.test", "handleMessageSent");

  try {
    // Test with complex data structure
    const complexData = JSON.stringify({
      nested: {
        value: "test",
        number: 42,
        boolean: true,
        array: [1, 2, 3],
      },
      metadata: {
        timestamp: new Date().toISOString(),
        version: "1.0",
      },
    });

    const result = dispatcher.sendMessage("data.test", complexData);

    const received = receivedMessages.find(
      (m) => m.messageType === "data.test",
    );

    results.push({
      test: "complex data serialization",
      success: true,
      sent: complexData,
      received: received ? received.messageData : null,
      result: result,
    });
  } catch (error) {
    results.push({
      test: "complex data serialization",
      success: false,
      error: error.message,
    });
  }

  return {
    status: 200,
    body: JSON.stringify({ success: true, results: results }),
    contentType: "application/json",
  };
}

/**
 * Test error handling in message handlers
 */
function testErrorHandling(context) {
  const results = [];

  try {
    // Register a handler that will throw an error
    dispatcher.registerListener("error.test", "handleErrorTest");

    // Send a message that will trigger the error
    const result = dispatcher.sendMessage(
      "error.test",
      JSON.stringify({ test: true }),
    );

    // The dispatcher should continue despite handler errors
    results.push({
      test: "error handling",
      success: result.includes("failed") || result.includes("Failed"),
      result: result,
      note: "Dispatcher should report handler failures but continue",
    });
  } catch (error) {
    results.push({
      test: "error handling",
      success: false,
      error: error.message,
    });
  }

  return {
    status: 200,
    body: JSON.stringify({ success: true, results: results }),
    contentType: "application/json",
  };
}

/**
 * Get received messages for inspection
 */
function getReceivedMessages(context) {
  return {
    status: 200,
    body: JSON.stringify({
      success: true,
      messages: receivedMessages,
      count: receivedMessages.length,
    }),
    contentType: "application/json",
  };
}

/**
 * Clear received messages
 */
function clearReceivedMessages(context) {
  const count = receivedMessages.length;
  receivedMessages = [];

  return {
    status: 200,
    body: JSON.stringify({
      success: true,
      cleared: count,
    }),
    contentType: "application/json",
  };
}

/**
 * Run all tests
 */
function runAllTests(context) {
  const testResults = [];

  // Clear messages before starting
  receivedMessages = [];

  console.log("Running dispatcher tests...");

  // Test 1: Register listener
  try {
    const result = testRegisterListener(context);
    testResults.push({
      test: "testRegisterListener",
      success: true,
      result: JSON.parse(result.body),
    });
  } catch (error) {
    testResults.push({
      test: "testRegisterListener",
      success: false,
      error: error.message,
    });
  }

  // Test 2: Send message
  try {
    const result = testSendMessage(context);
    testResults.push({
      test: "testSendMessage",
      success: true,
      result: JSON.parse(result.body),
    });
  } catch (error) {
    testResults.push({
      test: "testSendMessage",
      success: false,
      error: error.message,
    });
  }

  // Test 3: Multiple handlers
  try {
    const result = testMultipleHandlers(context);
    testResults.push({
      test: "testMultipleHandlers",
      success: true,
      result: JSON.parse(result.body),
    });
  } catch (error) {
    testResults.push({
      test: "testMultipleHandlers",
      success: false,
      error: error.message,
    });
  }

  // Test 4: Data serialization
  try {
    const result = testMessageDataSerialization(context);
    testResults.push({
      test: "testMessageDataSerialization",
      success: true,
      result: JSON.parse(result.body),
    });
  } catch (error) {
    testResults.push({
      test: "testMessageDataSerialization",
      success: false,
      error: error.message,
    });
  }

  // Test 5: Error handling
  try {
    const result = testErrorHandling(context);
    testResults.push({
      test: "testErrorHandling",
      success: true,
      result: JSON.parse(result.body),
    });
  } catch (error) {
    testResults.push({
      test: "testErrorHandling",
      success: false,
      error: error.message,
    });
  }

  console.log("All dispatcher tests completed");

  return {
    status: 200,
    body: JSON.stringify({
      success: true,
      totalTests: testResults.length,
      passed: testResults.filter((t) => t.success).length,
      failed: testResults.filter((t) => !t.success).length,
      results: testResults,
      receivedMessagesCount: receivedMessages.length,
    }),
    contentType: "application/json",
  };
}

/**
 * Initialize the test script
 */
function init(context) {
  console.log(
    "Initializing dispatcher test script at " + new Date().toISOString(),
  );

  // Register test routes
  routeRegistry.registerRoute(
    "/test-dispatcher/register-listener",
    "testRegisterListener",
    "GET",
  );
  routeRegistry.registerRoute(
    "/test-dispatcher/send-message",
    "testSendMessage",
    "GET",
  );
  routeRegistry.registerRoute(
    "/test-dispatcher/multiple-handlers",
    "testMultipleHandlers",
    "GET",
  );
  routeRegistry.registerRoute(
    "/test-dispatcher/data-serialization",
    "testMessageDataSerialization",
    "GET",
  );
  routeRegistry.registerRoute(
    "/test-dispatcher/error-handling",
    "testErrorHandling",
    "GET",
  );
  routeRegistry.registerRoute(
    "/test-dispatcher/received-messages",
    "getReceivedMessages",
    "GET",
  );
  routeRegistry.registerRoute(
    "/test-dispatcher/clear-messages",
    "clearReceivedMessages",
    "POST",
  );
  routeRegistry.registerRoute("/test-dispatcher/run-all", "runAllTests", "GET");

  console.log("Dispatcher test routes registered");

  return { success: true };
}
