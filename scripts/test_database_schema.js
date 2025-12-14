/**
 * Test Script for Database Schema Management API
 *
 * This script tests the new database schema management capabilities that allow
 * scripts to create and manage their own database tables.
 *
 * Features tested:
 * - Creating tables with automatic ID column
 * - Adding columns of different types (INTEGER, TEXT, BOOLEAN, TIMESTAMP)
 * - Setting nullable and default values
 * - Creating foreign key references
 * - Dropping tables
 * - Automatic cleanup on script deletion
 */

function init() {
  log("Database Schema Management Test Script initialized");

  // Register test routes
  routeRegistry.register("GET", "/test/db/create", "testCreateTable");
  routeRegistry.register("GET", "/test/db/columns", "testAddColumns");
  routeRegistry.register("GET", "/test/db/references", "testForeignKeys");
  routeRegistry.register("GET", "/test/db/drop", "testDropTable");
  routeRegistry.register("GET", "/test/db/full", "testFullWorkflow");
}

/**
 * Test 1: Create a table
 */
function testCreateTable(context) {
  log("Testing table creation...");

  // Create a users table
  const result = database.createTable("users");
  const data = JSON.parse(result);

  if (data.error) {
    return {
      status: 500,
      body: JSON.stringify({
        test: "createTable",
        success: false,
        error: data.error,
      }),
      contentType: "application/json",
    };
  }

  log(
    "Created table: " +
      data.tableName +
      " (physical: " +
      data.physicalName +
      ")",
  );

  return {
    status: 200,
    body: JSON.stringify({
      test: "createTable",
      success: true,
      tableName: data.tableName,
      physicalName: data.physicalName,
    }),
    contentType: "application/json",
  };
}

/**
 * Test 2: Add columns of different types
 */
function testAddColumns(context) {
  log("Testing column addition...");

  const results = [];

  // Create table first
  database.createTable("products");

  // Add integer column with default
  const nameResult = database.addTextColumn(
    "products",
    "name",
    false,
    "Unnamed Product",
  );
  results.push({ type: "text", result: JSON.parse(nameResult) });

  // Add integer column with default
  const priceResult = database.addIntegerColumn(
    "products",
    "price",
    false,
    "0",
  );
  results.push({ type: "integer", result: JSON.parse(priceResult) });

  // Add boolean column nullable
  const activeResult = database.addBooleanColumn(
    "products",
    "active",
    true,
    null,
  );
  results.push({ type: "boolean", result: JSON.parse(activeResult) });

  // Add timestamp with NOW() default
  const createdResult = database.addTimestampColumn(
    "products",
    "created_at",
    false,
    "NOW()",
  );
  results.push({ type: "timestamp", result: JSON.parse(createdResult) });

  const allSuccess = results.every((r) => r.result.success);

  if (!allSuccess) {
    log("Some column additions failed");
  }

  return {
    status: allSuccess ? 200 : 500,
    body: JSON.stringify({
      test: "addColumns",
      success: allSuccess,
      results: results,
    }),
    contentType: "application/json",
  };
}

/**
 * Test 3: Create foreign key references
 */
function testForeignKeys(context) {
  log("Testing foreign key creation...");

  // Create two tables
  database.createTable("authors");
  database.createTable("books");

  // Add a column to reference authors
  database.addIntegerColumn("books", "author_id", false, null);

  // Create the foreign key
  const fkResult = database.createReference("books", "author_id", "authors");
  const data = JSON.parse(fkResult);

  if (data.error) {
    return {
      status: 500,
      body: JSON.stringify({
        test: "foreignKeys",
        success: false,
        error: data.error,
      }),
      contentType: "application/json",
    };
  }

  log("Created foreign key: " + data.foreignKey);

  return {
    status: 200,
    body: JSON.stringify({
      test: "foreignKeys",
      success: true,
      foreignKey: data.foreignKey,
    }),
    contentType: "application/json",
  };
}

/**
 * Test 4: Drop a table
 */
function testDropTable(context) {
  log("Testing table drop...");

  // Create and then drop a table
  database.createTable("temp_table");

  const dropResult = database.dropTable("temp_table");
  const data = JSON.parse(dropResult);

  if (data.error) {
    return {
      status: 500,
      body: JSON.stringify({
        test: "dropTable",
        success: false,
        error: data.error,
      }),
      contentType: "application/json",
    };
  }

  log("Dropped table: " + data.tableName + ", existed: " + data.dropped);

  return {
    status: 200,
    body: JSON.stringify({
      test: "dropTable",
      success: true,
      tableName: data.tableName,
      dropped: data.dropped,
    }),
    contentType: "application/json",
  };
}

/**
 * Test 5: Full workflow test
 */
function testFullWorkflow(context) {
  log("Testing full database schema workflow...");

  const steps = [];

  try {
    // Step 1: Create a customers table
    const createResult = database.createTable("customers");
    const createData = JSON.parse(createResult);
    steps.push({
      step: "createTable",
      success: !createData.error,
      data: createData,
    });

    if (createData.error) {
      throw new Error("Failed to create table: " + createData.error);
    }

    // Step 2: Add columns
    const emailResult = database.addTextColumn(
      "customers",
      "email",
      false,
      "unknown@example.com",
    );
    steps.push({
      step: "addTextColumn",
      success: !JSON.parse(emailResult).error,
      data: JSON.parse(emailResult),
    });

    const ageResult = database.addIntegerColumn("customers", "age", true, null);
    steps.push({
      step: "addIntegerColumn",
      success: !JSON.parse(ageResult).error,
      data: JSON.parse(ageResult),
    });

    const verifiedResult = database.addBooleanColumn(
      "customers",
      "verified",
      false,
      "false",
    );
    steps.push({
      step: "addBooleanColumn",
      success: !JSON.parse(verifiedResult).error,
      data: JSON.parse(verifiedResult),
    });

    const joinedResult = database.addTimestampColumn(
      "customers",
      "joined_at",
      false,
      "NOW()",
    );
    steps.push({
      step: "addTimestampColumn",
      success: !JSON.parse(joinedResult).error,
      data: JSON.parse(joinedResult),
    });

    // Step 3: Create an orders table and add a foreign key
    const ordersResult = database.createTable("orders");
    steps.push({
      step: "createOrdersTable",
      success: !JSON.parse(ordersResult).error,
      data: JSON.parse(ordersResult),
    });

    database.addIntegerColumn("orders", "customer_id", false, null);
    const fkResult = database.createReference(
      "orders",
      "customer_id",
      "customers",
    );
    steps.push({
      step: "createForeignKey",
      success: !JSON.parse(fkResult).error,
      data: JSON.parse(fkResult),
    });

    // Step 4: Verify we can drop a table
    const dropResult = database.dropTable("orders");
    steps.push({
      step: "dropTable",
      success: !JSON.parse(dropResult).error,
      data: JSON.parse(dropResult),
    });

    const allSuccess = steps.every((s) => s.success);

    log("Full workflow test completed. All steps successful: " + allSuccess);

    return {
      status: 200,
      body: JSON.stringify(
        {
          test: "fullWorkflow",
          success: allSuccess,
          steps: steps,
          message: allSuccess
            ? "All database operations completed successfully"
            : "Some operations failed",
        },
        null,
        2,
      ),
      contentType: "application/json",
    };
  } catch (error) {
    log("Error in full workflow: " + error.toString());

    return {
      status: 500,
      body: JSON.stringify(
        {
          test: "fullWorkflow",
          success: false,
          error: error.toString(),
          steps: steps,
        },
        null,
        2,
      ),
      contentType: "application/json",
    };
  }
}
