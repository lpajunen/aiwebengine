// Test script for transaction functionality
// This demonstrates and tests the transaction support

registerRoute({
  method: "GET",
  path: "/test/transaction-commit",
  handlerFunctionName: "testCommit"
});

registerRoute({
  method: "GET",
  path: "/test/transaction-rollback",
  handlerFunctionName: "testRollback"
});

registerRoute({
  method: "GET",
  path: "/test/transaction-savepoint",
  handlerFunctionName: "testSavepoint"
});

registerRoute({
  method: "GET",
  path: "/test/transaction-timeout",
  handlerFunctionName: "testTimeout"
});

registerRoute({
  method: "GET",
  path: "/test/transaction-nested",
  handlerFunctionName: "testNested"
});

/**
 * Test 1: Basic commit - handler completes normally
 */
export function testCommit(req) {
  console.log("TEST: Transaction commit");
  
  // Begin transaction
  const beginResult = JSON.parse(database.beginTransaction(5000));
  console.log("Begin result:", JSON.stringify(beginResult));
  
  if (beginResult.error) {
    return {
      status: 500,
      body: JSON.stringify({ 
        test: "testCommit",
        passed: false, 
        error: beginResult.error 
      })
    };
  }
  
  // Perform some operations (would be database operations in real usage)
  console.log("Performing operations within transaction...");
  
  // Normal return should auto-commit
  return {
    status: 200,
    body: JSON.stringify({
      test: "testCommit",
      passed: true,
      message: "Transaction should auto-commit on normal return"
    })
  };
}

/**
 * Test 2: Rollback - handler throws exception
 */
export function testRollback(req) {
  console.log("TEST: Transaction rollback");
  
  // Begin transaction
  const beginResult = JSON.parse(database.beginTransaction(5000));
  console.log("Begin result:", JSON.stringify(beginResult));
  
  if (beginResult.error) {
    return {
      status: 500,
      body: JSON.stringify({ 
        test: "testRollback",
        passed: false, 
        error: beginResult.error 
      })
    };
  }
  
  // Perform some operations
  console.log("Performing operations within transaction...");
  
  // Throw error to trigger auto-rollback
  throw new Error("Intentional error to test rollback");
}

/**
 * Test 3: Savepoint - create and rollback to savepoint
 */
export function testSavepoint(req) {
  console.log("TEST: Savepoint rollback");
  
  // Begin transaction
  const beginResult = JSON.parse(database.beginTransaction(10000));
  if (beginResult.error) {
    return {
      status: 500,
      body: JSON.stringify({ 
        test: "testSavepoint",
        passed: false, 
        error: beginResult.error 
      })
    };
  }
  
  console.log("Transaction started");
  
  // Create a savepoint
  const sp1 = JSON.parse(database.createSavepoint("test_sp1"));
  console.log("Savepoint created:", JSON.stringify(sp1));
  
  if (sp1.error) {
    return {
      status: 500,
      body: JSON.stringify({ 
        test: "testSavepoint",
        passed: false, 
        error: "Failed to create savepoint: " + sp1.error 
      })
    };
  }
  
  // Do some work
  console.log("Work after savepoint...");
  
  // Rollback to savepoint
  const rollbackResult = JSON.parse(database.rollbackToSavepoint(sp1.savepoint));
  console.log("Rollback to savepoint result:", JSON.stringify(rollbackResult));
  
  if (rollbackResult.error) {
    return {
      status: 500,
      body: JSON.stringify({ 
        test: "testSavepoint",
        passed: false, 
        error: "Failed to rollback to savepoint: " + rollbackResult.error 
      })
    };
  }
  
  // Commit the transaction
  const commitResult = JSON.parse(database.commitTransaction());
  console.log("Commit result:", JSON.stringify(commitResult));
  
  if (commitResult.error) {
    return {
      status: 500,
      body: JSON.stringify({ 
        test: "testSavepoint",
        passed: false, 
        error: "Failed to commit: " + commitResult.error 
      })
    };
  }
  
  return {
    status: 200,
    body: JSON.stringify({
      test: "testSavepoint",
      passed: true,
      message: "Successfully created savepoint, rolled back, and committed",
      savepoint: sp1.savepoint
    })
  };
}

/**
 * Test 4: Transaction timeout
 */
export function testTimeout(req) {
  console.log("TEST: Transaction timeout");
  
  // Begin transaction with very short timeout (100ms)
  const beginResult = JSON.parse(database.beginTransaction(100));
  if (beginResult.error) {
    return {
      status: 500,
      body: JSON.stringify({ 
        test: "testTimeout",
        passed: false, 
        error: beginResult.error 
      })
    };
  }
  
  console.log("Transaction started with 100ms timeout");
  
  // Wait longer than timeout (simulate slow operation)
  // JavaScript doesn't have sleep, so we'll just try to commit after a delay simulation
  // In practice, the timeout will be checked on the next transaction operation
  
  // Busy wait to simulate delay (not ideal but works for testing)
  const start = Date.now();
  while (Date.now() - start < 200) {
    // Busy loop
  }
  
  // Try to commit - should fail with timeout error
  const commitResult = JSON.parse(database.commitTransaction());
  console.log("Commit after timeout result:", JSON.stringify(commitResult));
  
  if (commitResult.error && commitResult.error.includes("timeout")) {
    return {
      status: 200,
      body: JSON.stringify({
        test: "testTimeout",
        passed: true,
        message: "Transaction correctly timed out",
        error: commitResult.error
      })
    };
  } else {
    return {
      status: 500,
      body: JSON.stringify({
        test: "testTimeout",
        passed: false,
        message: "Transaction should have timed out but didn't",
        commitResult: commitResult
      })
    };
  }
}

/**
 * Test 5: Nested transactions with multiple savepoints
 */
export function testNested(req) {
  console.log("TEST: Nested transactions");
  
  // Begin outer transaction
  const beginResult = JSON.parse(database.beginTransaction(10000));
  if (beginResult.error) {
    return {
      status: 500,
      body: JSON.stringify({ 
        test: "testNested",
        passed: false, 
        error: beginResult.error 
      })
    };
  }
  
  console.log("Outer transaction started");
  
  // Create first savepoint
  const sp1 = JSON.parse(database.createSavepoint());
  console.log("Savepoint 1:", JSON.stringify(sp1));
  
  if (sp1.error) {
    return {
      status: 500,
      body: JSON.stringify({ 
        test: "testNested",
        passed: false, 
        error: "Savepoint 1 failed: " + sp1.error 
      })
    };
  }
  
  // Do some work at level 1
  console.log("Work at savepoint level 1");
  
  // Create second savepoint (nested)
  const sp2 = JSON.parse(database.createSavepoint());
  console.log("Savepoint 2:", JSON.stringify(sp2));
  
  if (sp2.error) {
    return {
      status: 500,
      body: JSON.stringify({ 
        test: "testNested",
        passed: false, 
        error: "Savepoint 2 failed: " + sp2.error 
      })
    };
  }
  
  // Do some work at level 2
  console.log("Work at savepoint level 2");
  
  // Rollback inner savepoint
  const rollback2 = JSON.parse(database.rollbackToSavepoint(sp2.savepoint));
  console.log("Rollback sp2:", JSON.stringify(rollback2));
  
  if (rollback2.error) {
    return {
      status: 500,
      body: JSON.stringify({ 
        test: "testNested",
        passed: false, 
        error: "Rollback sp2 failed: " + rollback2.error 
      })
    };
  }
  
  // Release first savepoint
  const release1 = JSON.parse(database.releaseSavepoint(sp1.savepoint));
  console.log("Release sp1:", JSON.stringify(release1));
  
  if (release1.error) {
    return {
      status: 500,
      body: JSON.stringify({ 
        test: "testNested",
        passed: false, 
        error: "Release sp1 failed: " + release1.error 
      })
    };
  }
  
  // Commit entire transaction
  const commit = JSON.parse(database.commitTransaction());
  console.log("Commit:", JSON.stringify(commit));
  
  if (commit.error) {
    return {
      status: 500,
      body: JSON.stringify({ 
        test: "testNested",
        passed: false, 
        error: "Commit failed: " + commit.error 
      })
    };
  }
  
  return {
    status: 200,
    body: JSON.stringify({
      test: "testNested",
      passed: true,
      message: "Nested transactions with savepoints worked correctly",
      savepoints: [sp1.savepoint, sp2.savepoint]
    })
  };
}

console.log("Transaction test handlers registered");
console.log("Test URLs:");
console.log("  GET /test/transaction-commit - Test auto-commit on normal return");
console.log("  GET /test/transaction-rollback - Test auto-rollback on exception");
console.log("  GET /test/transaction-savepoint - Test savepoint operations");
console.log("  GET /test/transaction-timeout - Test transaction timeout");
console.log("  GET /test/transaction-nested - Test nested savepoints");
