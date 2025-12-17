/// <reference path="../../assets/aiwebengine.d.ts" />

// Test script for personalStorage functionality
routeRegistry.registerRoute(
  "/test-personal-storage",
  "testPersonalStorageHandler",
  "GET",
);

function testPersonalStorageHandler(context) {
  const req = context.request;
  let results = [];

  // Test 1: Check authentication requirement
  results.push("=== Test 1: Authentication Requirement ===");
  if (!req.auth.isAuthenticated) {
    results.push("✓ User not authenticated (expected for this test)");

    // Try to set item without authentication
    const setResult = personalStorage.setItem("test_key", "test_value");
    if (setResult.includes("requires authentication")) {
      results.push("✓ setItem correctly requires authentication");
    } else {
      results.push("✗ FAIL: setItem should require authentication");
    }

    // Try to get item without authentication
    const getResult = personalStorage.getItem("test_key");
    if (getResult === null) {
      results.push("✓ getItem returns null when not authenticated");
    } else {
      results.push("✗ FAIL: getItem should return null when not authenticated");
    }

    // Try to remove item without authentication
    const removeResult = personalStorage.removeItem("test_key");
    if (removeResult === false) {
      results.push("✓ removeItem returns false when not authenticated");
    } else {
      results.push(
        "✗ FAIL: removeItem should return false when not authenticated",
      );
    }

    // Try to clear without authentication
    const clearResult = personalStorage.clear();
    if (clearResult.includes("requires authentication")) {
      results.push("✓ clear correctly requires authentication");
    } else {
      results.push("✗ FAIL: clear should require authentication");
    }
  } else {
    // User is authenticated - run authenticated tests
    results.push(`✓ User authenticated: ${req.auth.userId}`);

    // Test 2: Set and Get
    results.push("");
    results.push("=== Test 2: Set and Get ===");
    const setResult1 = personalStorage.setItem("test_key", "test_value");
    console.log("Set result: " + setResult1);
    results.push("Set result: " + setResult1);

    const getResult1 = personalStorage.getItem("test_key");
    console.log("Get result: " + getResult1);
    if (getResult1 === "test_value") {
      results.push("✓ Successfully set and retrieved value");
    } else {
      results.push(
        "✗ FAIL: Expected 'test_value', got: " + JSON.stringify(getResult1),
      );
    }

    // Test 3: Update existing value
    results.push("");
    results.push("=== Test 3: Update Value ===");
    personalStorage.setItem("test_key", "updated_value");
    const getResult2 = personalStorage.getItem("test_key");
    if (getResult2 === "updated_value") {
      results.push("✓ Successfully updated value");
    } else {
      results.push(
        "✗ FAIL: Expected 'updated_value', got: " + JSON.stringify(getResult2),
      );
    }

    // Test 4: Multiple keys
    results.push("");
    results.push("=== Test 4: Multiple Keys ===");
    personalStorage.setItem("key1", "value1");
    personalStorage.setItem("key2", "value2");
    const get1 = personalStorage.getItem("key1");
    const get2 = personalStorage.getItem("key2");
    if (get1 === "value1" && get2 === "value2") {
      results.push("✓ Multiple keys stored independently");
    } else {
      results.push(
        "✗ FAIL: key1=" +
          JSON.stringify(get1) +
          ", key2=" +
          JSON.stringify(get2),
      );
    }

    // Test 5: Remove item
    results.push("");
    results.push("=== Test 5: Remove Item ===");
    const removeResult1 = personalStorage.removeItem("test_key");
    console.log("Remove result: " + removeResult1);
    if (removeResult1 === true) {
      results.push("✓ removeItem returned true for existing key");
    } else {
      results.push("✗ FAIL: removeItem should return true for existing key");
    }

    const getAfterRemove = personalStorage.getItem("test_key");
    if (getAfterRemove === null) {
      results.push("✓ Value successfully removed");
    } else {
      results.push(
        "✗ FAIL: Value still exists: " + JSON.stringify(getAfterRemove),
      );
    }

    // Test 6: Remove non-existent key
    results.push("");
    results.push("=== Test 6: Remove Non-existent Key ===");
    const removeResult2 = personalStorage.removeItem("nonexistent_key");
    if (removeResult2 === false) {
      results.push("✓ removeItem returned false for non-existent key");
    } else {
      results.push(
        "✗ FAIL: removeItem should return false for non-existent key",
      );
    }

    // Test 7: Validation - empty key
    results.push("");
    results.push("=== Test 7: Empty Key Validation ===");
    const emptyKeyResult = personalStorage.setItem("", "value");
    if (emptyKeyResult.includes("cannot be empty")) {
      results.push("✓ Empty key rejected");
    } else {
      results.push("✗ FAIL: Empty key should be rejected");
    }

    // Test 8: Validation - large value
    results.push("");
    results.push("=== Test 8: Large Value Validation ===");
    const largeValue = "x".repeat(1_000_001); // Just over 1MB
    const largeResult = personalStorage.setItem("large", largeValue);
    if (largeResult.includes("too large")) {
      results.push("✓ Large value (>1MB) rejected");
    } else {
      results.push("✗ FAIL: Large value should be rejected");
    }

    // Test 9: Clear all user data
    results.push("");
    results.push("=== Test 9: Clear All Data ===");
    personalStorage.setItem("clear_test_1", "value1");
    personalStorage.setItem("clear_test_2", "value2");
    const clearResult1 = personalStorage.clear();
    results.push("Clear result: " + clearResult1);

    const afterClear1 = personalStorage.getItem("key1");
    const afterClear2 = personalStorage.getItem("key2");
    const afterClear3 = personalStorage.getItem("clear_test_1");
    const afterClear4 = personalStorage.getItem("clear_test_2");

    if (
      afterClear1 === null &&
      afterClear2 === null &&
      afterClear3 === null &&
      afterClear4 === null
    ) {
      results.push("✓ All personal data cleared successfully");
    } else {
      results.push(
        "✗ FAIL: Data still exists after clear: " +
          JSON.stringify({
            key1: afterClear1,
            key2: afterClear2,
            clear1: afterClear3,
            clear2: afterClear4,
          }),
      );
    }

    // Test 10: Data isolation (note: this can only truly test if there are multiple users)
    results.push("");
    results.push("=== Test 10: Data Isolation ===");
    results.push(
      "Note: Full data isolation can only be tested with multiple authenticated users",
    );
    personalStorage.setItem("isolation_test", `user_${req.auth.userId}_data`);
    const isolationData = personalStorage.getItem("isolation_test");
    if (isolationData === `user_${req.auth.userId}_data`) {
      results.push("✓ Data correctly associated with current user");
    } else {
      results.push("✗ FAIL: Data mismatch");
    }
  }

  return {
    status: 200,
    body: results.join("\n"),
    contentType: "text/plain; charset=UTF-8",
  };
}
