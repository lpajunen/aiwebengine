/// <reference path="../../assets/aiwebengine.d.ts" />

// Minimal test to isolate routeRegistry.sendStreamMessage issue
console.log("Starting minimal routeRegistry.sendStreamMessage test");

try {
  console.log("Calling routeRegistry.sendStreamMessage...");
  routeRegistry.sendStreamMessage("/test-stream", '{"test": "message"}');
  console.log("routeRegistry.sendStreamMessage call completed successfully");
} catch (error) {
  console.log("Error caught: " + error.toString());
}

console.log("Test completed");
