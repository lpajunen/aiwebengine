// Test script for personalStorage fix
function testPersonalStorage(context) {
  const req = context.request || {};

  // Check authentication
  if (!req.auth || !req.auth.isAuthenticated) {
    return {
      status: 401,
      body: JSON.stringify({ error: "Not authenticated" }),
      contentType: "application/json",
    };
  }

  // Test setItem
  const setResult = personalStorage.setItem(
    "test_key",
    "test_value_" + Date.now(),
  );
  console.log("setItem result:", setResult);

  // Test getItem
  const getValue = personalStorage.getItem("test_key");
  console.log("getItem result:", getValue);

  // Return results
  return {
    status: 200,
    body: JSON.stringify({
      userId: req.auth.userId,
      setResult: setResult,
      getValue: getValue,
      success: getValue !== null && getValue !== undefined,
    }),
    contentType: "application/json",
  };
}

function init(context) {
  console.log("Registering personalStorage test endpoint");
  routeRegistry.registerRoute(
    "/api/test-personal-storage",
    "testPersonalStorage",
    "GET",
  );
  return { success: true };
}
