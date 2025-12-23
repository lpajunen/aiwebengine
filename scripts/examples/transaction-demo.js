// Example: Transaction support demonstration
// This script shows how to use database transactions in aiwebengine

// Register HTTP route
registerRoute({
  method: "POST",
  path: "/transaction-demo/transfer",
  handlerFunctionName: "handleTransfer",
});

registerRoute({
  method: "POST",
  path: "/transaction-demo/batch",
  handlerFunctionName: "handleBatch",
});

registerRoute({
  method: "POST",
  path: "/transaction-demo/nested",
  handlerFunctionName: "handleNested",
});

/**
 * Example 1: Basic transaction for fund transfer
 *
 * This demonstrates automatic commit on success and rollback on error
 */
export function handleTransfer(req) {
  const body = JSON.parse(req.body);
  const { fromAccount, toAccount, amount } = body;

  // Start transaction with 5 second timeout
  const beginResult = JSON.parse(database.beginTransaction(5000));
  if (beginResult.error) {
    return {
      status: 500,
      body: JSON.stringify({ error: "Failed to start transaction" }),
    };
  }

  console.log(
    `Starting transfer: $${amount} from ${fromAccount} to ${toAccount}`,
  );

  try {
    // Simulate database operations
    // In real usage, these would be actual database queries

    // Check source account balance
    const sourceBalance = 1000; // Mock value
    if (sourceBalance < amount) {
      throw new Error("Insufficient funds");
    }

    // Deduct from source
    console.log(`Deducting $${amount} from account ${fromAccount}`);
    // database.query("UPDATE accounts SET balance = balance - $1 WHERE id = $2", [amount, fromAccount]);

    // Add to destination
    console.log(`Adding $${amount} to account ${toAccount}`);
    // database.query("UPDATE accounts SET balance = balance + $1 WHERE id = $2", [amount, toAccount]);

    // Transaction will auto-commit on successful return
    return {
      status: 200,
      body: JSON.stringify({
        success: true,
        message: "Transfer completed",
        amount: amount,
        from: fromAccount,
        to: toAccount,
      }),
    };
  } catch (error) {
    // Transaction will auto-rollback on exception
    console.error("Transfer failed:", error.message);
    throw error; // Re-throw to trigger auto-rollback
  }
}

/**
 * Example 2: Batch processing with savepoints
 *
 * This demonstrates processing multiple items where individual failures
 * don't abort the entire batch
 */
export function handleBatch(req) {
  const body = JSON.parse(req.body);
  const { items } = body;

  if (!Array.isArray(items)) {
    return {
      status: 400,
      body: JSON.stringify({ error: "items must be an array" }),
    };
  }

  // Start outer transaction
  database.beginTransaction(30000); // 30 second timeout

  const results = [];

  for (let i = 0; i < items.length; i++) {
    const item = items[i];

    // Create savepoint for this item
    const spResult = JSON.parse(database.createSavepoint());
    if (spResult.error) {
      results.push({
        item: item.id || i,
        status: "error",
        error: "Failed to create savepoint",
      });
      continue;
    }

    const savepoint = spResult.savepoint;
    console.log(`Processing item ${item.id || i} with savepoint ${savepoint}`);

    try {
      // Process the item (mock operation)
      if (item.shouldFail) {
        throw new Error("Simulated failure");
      }

      // Simulate successful processing
      console.log(`Item ${item.id || i} processed successfully`);
      results.push({
        item: item.id || i,
        status: "success",
      });

      // Savepoint automatically released on next iteration or commit
    } catch (error) {
      // Rollback just this item
      console.log(`Rolling back item ${item.id || i}: ${error.message}`);
      database.rollbackToSavepoint(savepoint);

      results.push({
        item: item.id || i,
        status: "failed",
        error: error.message,
      });
    }
  }

  // Commit all successful items
  const commitResult = JSON.parse(database.commitTransaction());
  if (commitResult.error) {
    return {
      status: 500,
      body: JSON.stringify({
        error: "Failed to commit transaction",
        results: results,
      }),
    };
  }

  const successCount = results.filter((r) => r.status === "success").length;
  const failCount = results.filter((r) => r.status === "failed").length;

  return {
    status: 200,
    body: JSON.stringify({
      success: true,
      processed: items.length,
      successful: successCount,
      failed: failCount,
      results: results,
    }),
  };
}

/**
 * Example 3: Nested transactions with explicit control
 *
 * This demonstrates manual transaction management with multiple
 * savepoint levels
 */
export function handleNested(req) {
  const body = JSON.parse(req.body);

  // Start outer transaction
  database.beginTransaction(10000);
  console.log("Outer transaction started");

  try {
    // First level of work
    console.log("Performing first-level operations...");
    // database.insert("audit_log", { action: "started", timestamp: Date.now() });

    // Create savepoint before risky operation
    const sp1 = JSON.parse(database.createSavepoint("checkpoint_1"));
    console.log("Created savepoint:", sp1.savepoint);

    try {
      // Risky operation
      console.log("Performing risky operation...");

      if (body.simulateError) {
        throw new Error("Simulated error in risky operation");
      }

      // database.insert("data", { value: body.value });

      // Create another savepoint for even riskier operation
      const sp2 = JSON.parse(database.createSavepoint("checkpoint_2"));
      console.log("Created nested savepoint:", sp2.savepoint);

      try {
        // Very risky operation
        if (body.simulateNestedError) {
          throw new Error("Simulated error in nested operation");
        }

        console.log("Both operations succeeded");

        // Release inner savepoint explicitly
        database.releaseSavepoint(sp2.savepoint);
      } catch (nestedError) {
        // Rollback just the innermost operation
        console.log("Rolling back nested operation:", nestedError.message);
        database.rollbackToSavepoint(sp2.savepoint);
      }

      // Release outer savepoint
      database.releaseSavepoint(sp1.savepoint);
    } catch (error) {
      // Rollback to first savepoint
      console.log("Rolling back to first checkpoint:", error.message);
      database.rollbackToSavepoint(sp1.savepoint);

      // Continue with fallback logic
      console.log("Executing fallback logic...");
      // database.insert("audit_log", { action: "fallback", timestamp: Date.now() });
    }

    // Final operations
    console.log("Performing final operations...");
    // database.insert("audit_log", { action: "completed", timestamp: Date.now() });

    // Explicitly commit
    const commitResult = JSON.parse(database.commitTransaction());
    if (commitResult.error) {
      throw new Error("Failed to commit: " + commitResult.error);
    }

    console.log("Transaction committed successfully");

    return {
      status: 200,
      body: JSON.stringify({
        success: true,
        message: "Nested transaction completed",
      }),
    };
  } catch (error) {
    // Auto-rollback on exception
    console.error("Transaction failed:", error.message);
    return {
      status: 500,
      body: JSON.stringify({
        error: "Transaction failed: " + error.message,
      }),
    };
  }
}

// Test data examples:
//
// Basic transfer:
// POST /transaction-demo/transfer
// { "fromAccount": "A123", "toAccount": "B456", "amount": 100 }
//
// Batch processing (all succeed):
// POST /transaction-demo/batch
// { "items": [{ "id": 1 }, { "id": 2 }, { "id": 3 }] }
//
// Batch processing (mixed results):
// POST /transaction-demo/batch
// { "items": [{ "id": 1 }, { "id": 2, "shouldFail": true }, { "id": 3 }] }
//
// Nested transactions (success):
// POST /transaction-demo/nested
// { "value": "test" }
//
// Nested transactions (outer error):
// POST /transaction-demo/nested
// { "simulateError": true }
//
// Nested transactions (inner error):
// POST /transaction-demo/nested
// { "simulateNestedError": true }
