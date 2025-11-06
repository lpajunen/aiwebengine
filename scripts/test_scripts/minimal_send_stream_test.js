// Minimal test to isolate sendStreamMessageToPath issue
console.log("Starting minimal sendStreamMessageToPath test");

try {
  console.log("Calling sendStreamMessageToPath...");
  sendStreamMessageToPath("/test-stream", '{"test": "message"}');
  console.log("sendStreamMessageToPath call completed successfully");
} catch (error) {
  console.log("Error caught: " + error.toString());
}

console.log("Test completed");
