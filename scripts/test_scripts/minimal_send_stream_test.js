// Minimal test to isolate sendStreamMessage issue
writeLog("Starting minimal sendStreamMessage test");

try {
  writeLog("Calling sendStreamMessage...");
  sendStreamMessage('{"test": "message"}');
  writeLog("sendStreamMessage call completed successfully");
} catch (error) {
  writeLog("Error caught: " + error.toString());
}

writeLog("Test completed");
