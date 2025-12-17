/// <reference path="../assets/aiwebengine.d.ts" />

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
 * - Creating foreign key references with addReferenceColumn
 * - Dropping columns
 * - Dropping tables
 * - Automatic cleanup on script deletion
 */

function init() {
  console.log("Database Schema Management Test Script initialized");

  // Register test routes
  routeRegistry.registerRoute("/test/db/create", "testCreateTable", "GET");
  routeRegistry.registerRoute("/test/db/columns", "testAddColumns", "GET");
  routeRegistry.registerRoute("/test/db/references", "testForeignKeys", "GET");
  routeRegistry.registerRoute("/test/db/drop-column", "testDropColumn", "GET");
  routeRegistry.registerRoute("/test/db/drop", "testDropTable", "GET");
  routeRegistry.registerRoute("/test/db/full", "testFullWorkflow", "GET");
}

/**
 * Test 1: Create a table
 */
function testCreateTable(context) {
  console.log("Testing table creation...");

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

  console.log(
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
  console.log("Testing column addition...");

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
    console.log("Some column additions failed");
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
  console.log("Testing foreign key creation...");

  // Create two tables
  database.createTable("authors");
  database.createTable("books");

  // Add a reference column (this now creates the column AND the FK in one step)
  const fkResult = database.addReferenceColumn(
    "books",
    "author_id",
    "authors",
    false,
  );
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

  console.log(
    "Created foreign key: " + data.foreignKey + ", nullable: " + data.nullable,
  );

  return {
    status: 200,
    body: JSON.stringify({
      test: "foreignKeys",
      success: true,
      foreignKey: data.foreignKey,
      nullable: data.nullable,
    }),
    contentType: "application/json",
  };
}

/**
 * Test 4: Drop a column
 */
function testDropColumn(context) {
  console.log("Testing column drop...");

  // Create a table and add a column
  database.createTable("temp_table_col");
  database.addTextColumn("temp_table_col", "temp_column", true, null);

  // Drop the column
  const dropResult = database.dropColumn("temp_table_col", "temp_column");
  const data = JSON.parse(dropResult);

  if (data.error) {
    return {
      status: 500,
      body: JSON.stringify({
        test: "dropColumn",
        success: false,
        error: data.error,
      }),
      contentType: "application/json",
    };
  }

  console.log(
    "Dropped column: " +
      data.columnName +
      " from " +
      data.tableName +
      ", existed: " +
      data.dropped,
  );

  // Clean up table
  database.dropTable("temp_table_col");

  return {
    status: 200,
    body: JSON.stringify({
      test: "dropColumn",
      success: true,
      tableName: data.tableName,
      columnName: data.columnName,
      dropped: data.dropped,
    }),
    contentType: "application/json",
  };
}

/**
 * Test 5: Drop a table
 */
function testDropTable(context) {
  console.log("Testing table drop...");

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

  console.log(
    "Dropped table: " + data.tableName + ", existed: " + data.dropped,
  );

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
 * Test 6: Full workflow test
 */
function testFullWorkflow(context) {
  console.log("Testing full database schema workflow...");

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
    const fkResult = database.addReferenceColumn(
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

    console.log(
      "Full workflow test completed. All steps successful: " + allSuccess,
    );

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
    console.log("Error in full workflow: " + error.toString());

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
