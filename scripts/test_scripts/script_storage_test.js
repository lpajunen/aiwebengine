// Test script for scriptStorage functionality
register("/test-storage", "testStorageHandler", "GET");

function testStorageHandler(request) {
  // Test setting an item
  const setResult = scriptStorage.setItem("test_key", "test_value");
  console.log("Set result: " + setResult);

  // Test getting the item
  const getResult = scriptStorage.getItem("test_key");
  console.log("Get result: " + getResult);

  // Test setting another item
  scriptStorage.setItem("counter", "1");

  // Test removing an item
  const removeResult = scriptStorage.removeItem("test_key");
  console.log("Remove result: " + removeResult);

  // Verify it's gone
  const getAfterRemove = scriptStorage.getItem("test_key");
  console.log("Get after remove: " + getAfterRemove);

  return {
    status: 200,
    body: JSON.stringify({
      message: "scriptStorage test completed",
      results: {
        setResult,
        getResult,
        removeResult,
        getAfterRemove,
      },
    }),
  };
}
