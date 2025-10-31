// Minimal test to isolate sendStreamMessageToPath issue
writeLog("Starting minimal sendStreamMessageToPath test");

try {
  writeLog("Calling sendStreamMessageToPath...");
  sendStreamMessageToPath("/test-stream", '{"test": "message"}');
  writeLog("sendStreamMessageToPath call completed successfully");
} catch (error) {
  writeLog("Error caught: " + error.toString());
}

writeLog("Test completed");
