// Test script for sharedStorage functionality
routeRegistry.registerRoute("/test-storage", "testStorageHandler", "GET");

function testStorageHandler(request) {
  // Test setting an item
  const setResult = sharedStorage.setItem("test_key", "test_value");
  console.log("Set result: " + setResult);

  // Test getting the item
  const getResult = sharedStorage.getItem("test_key");
  console.log("Get result: " + getResult);

  // Test setting another item
  sharedStorage.setItem("counter", "1");

  // Test removing an item
  const removeResult = sharedStorage.removeItem("test_key");
  console.log("Remove result: " + removeResult);

  // Verify it's gone
  const getAfterRemove = sharedStorage.getItem("test_key");
  console.log("Get after remove: " + getAfterRemove);

  return {
    status: 200,
    body: JSON.stringify({
      message: "sharedStorage test completed",
      results: {
        setResult,
        getResult,
        removeResult,
        getAfterRemove,
      },
    }),
  };
}
