# Database Transactions

This document describes the transaction support in aiwebengine, allowing JavaScript handlers to perform atomic database operations.

## Overview

Transactions provide ACID guarantees for database operations:

- **Automatic lifecycle**: Transactions auto-commit on normal handler exit and auto-rollback on exceptions
- **Manual control**: JavaScript APIs allow explicit transaction management
- **Nested transactions**: PostgreSQL savepoints enable nested transaction scopes
- **Timeout protection**: Configurable timeouts prevent long-running transactions from holding connections

## Automatic Transaction Management

All handler invocations (HTTP, GraphQL, MCP tools, scheduled jobs) automatically handle transaction lifecycle:

```javascript
// Handler example
export function myHandler(req) {
  // Start a transaction
  database.beginTransaction();

  // Perform database operations...
  // If handler completes normally, transaction auto-commits
  // If handler throws, transaction auto-rollbacks

  return { status: 200, body: "Success" };
}
```

## JavaScript Transaction APIs

### `database.beginTransaction(timeout_ms?)`

Begin a new transaction or create a savepoint if already in a transaction.

```javascript
// Start transaction with 30 second timeout
const result = JSON.parse(database.beginTransaction(30000));
if (result.error) {
  console.error("Failed to start transaction:", result.error);
}
```

**Parameters:**

- `timeout_ms` (optional): Transaction timeout in milliseconds

**Returns:** JSON string with `{success: true}` or `{error: "..."}`

### `database.commitTransaction()`

Commit the current transaction or release the most recent savepoint.

```javascript
const result = JSON.parse(database.commitTransaction());
if (result.error) {
  console.error("Failed to commit:", result.error);
}
```

**Returns:** JSON string with `{success: true}` or `{error: "..."}`

### `database.rollbackTransaction()`

Rollback the current transaction or to the most recent savepoint.

```javascript
const result = JSON.parse(database.rollbackTransaction());
if (result.error) {
  console.error("Failed to rollback:", result.error);
}
```

**Returns:** JSON string with `{success: true}` or `{error: "..."}`

### `database.createSavepoint(name?)`

Create a named or auto-generated savepoint for nested transactions.

```javascript
// Auto-generated name
const result = JSON.parse(database.createSavepoint());
console.log("Savepoint:", result.savepoint); // e.g., "sp_1"

// Named savepoint
const result2 = JSON.parse(database.createSavepoint("my_checkpoint"));
```

**Parameters:**

- `name` (optional): Savepoint name. If omitted, generates name like "sp_1", "sp_2", etc.

**Returns:** JSON string with `{success: true, savepoint: "name"}` or `{error: "..."}`

### `database.rollbackToSavepoint(name)`

Rollback to a specific savepoint without ending the transaction.

```javascript
database.rollbackToSavepoint("my_checkpoint");
```

**Parameters:**

- `name` (required): Savepoint name

**Returns:** JSON string with `{success: true}` or `{error: "..."}`

### `database.releaseSavepoint(name)`

Release a savepoint, making its changes permanent in the transaction scope.

```javascript
database.releaseSavepoint("my_checkpoint");
```

**Parameters:**

- `name` (required): Savepoint name

**Returns:** JSON string with `{success: true}` or `{error: "..."}`

## Usage Examples

### Basic Transaction

```javascript
export function transferFunds(req) {
  const { fromAccount, toAccount, amount } = JSON.parse(req.body);

  // Start transaction
  database.beginTransaction(5000); // 5 second timeout

  try {
    // Deduct from source account
    database.query("UPDATE accounts SET balance = balance - $1 WHERE id = $2", [
      amount,
      fromAccount,
    ]);

    // Add to destination account
    database.query("UPDATE accounts SET balance = balance + $1 WHERE id = $2", [
      amount,
      toAccount,
    ]);

    // Auto-commits on return
    return { status: 200, body: "Transfer successful" };
  } catch (error) {
    // Auto-rollbacks on throw
    throw error;
  }
}
```

### Manual Commit/Rollback

```javascript
export function complexOperation(req) {
  database.beginTransaction();

  // Perform operations...
  const data = performStep1();

  if (!validateData(data)) {
    // Explicitly rollback
    database.rollbackTransaction();
    return { status: 400, body: "Validation failed" };
  }

  performStep2(data);

  // Explicitly commit
  database.commitTransaction();
  return { status: 200, body: "Success" };
}
```

### Nested Transactions with Savepoints

```javascript
export function batchProcess(req) {
  database.beginTransaction();

  const items = JSON.parse(req.body).items;
  const results = [];

  for (const item of items) {
    // Create savepoint for this item
    const sp = JSON.parse(database.createSavepoint());

    try {
      processItem(item);
      results.push({ item: item.id, status: "success" });
      // Implicitly releases savepoint on next iteration or commit
    } catch (error) {
      // Rollback just this item, continue with others
      database.rollbackToSavepoint(sp.savepoint);
      results.push({ item: item.id, status: "failed", error: error.message });
    }
  }

  // Commit all successful items
  database.commitTransaction();

  return {
    status: 200,
    body: JSON.stringify({ results }),
  };
}
```

### GraphQL Mutation with Transaction

```javascript
// Register GraphQL mutation
registerGraphQL({
  mutations: [
    {
      name: "createUserWithProfile",
      sdl: "createUserWithProfile(email: String!, name: String!): User",
      resolverFunctionName: "resolveCreateUserWithProfile",
    },
  ],
});

export function resolveCreateUserWithProfile(args) {
  const { email, name } = args;

  // Start transaction
  database.beginTransaction();

  // Create user
  const userId = database.insert("users", { email });

  // Create profile
  database.insert("profiles", { user_id: userId, name });

  // Auto-commits on normal return
  return { id: userId, email, name };
}
```

## Best Practices

### 1. Use Timeouts

Always specify reasonable timeouts to prevent connection pool exhaustion:

```javascript
// 30 second timeout for complex operations
database.beginTransaction(30000);
```

### 2. Keep Transactions Short

Hold transactions for the minimum time needed:

```javascript
// Good: Short transaction
database.beginTransaction();
performDatabaseOperations();
database.commitTransaction();

// Bad: Long-held transaction
database.beginTransaction();
await fetch("https://slow-api.com"); // Don't do this!
performDatabaseOperations();
database.commitTransaction();
```

### 3. Use Savepoints for Partial Rollback

When processing batches, use savepoints to rollback individual items while keeping successful ones:

```javascript
for (const item of items) {
  const sp = JSON.parse(database.createSavepoint());
  try {
    processItem(item);
  } catch (e) {
    database.rollbackToSavepoint(sp.savepoint);
  }
}
```

### 4. Handle Errors Explicitly

Always check return values for errors:

```javascript
const result = JSON.parse(database.beginTransaction());
if (result.error) {
  console.error("Transaction failed:", result.error);
  return { status: 500, body: "Transaction error" };
}
```

## Implementation Details

### Thread-Local Storage

Transaction state is stored in thread-local storage, making it available across all database operations within the same handler invocation without explicit parameter passing.

### Automatic Cleanup

The `TransactionGuard` with Rust's `Drop` trait ensures transactions are rolled back if:

- Handler panics
- Exception thrown before commit
- Early return without explicit commit/rollback

### Connection Pool Impact

Transactions hold a database connection for their lifetime. With default pool size of 5 connections, consider:

- Keep transactions short
- Use appropriate timeouts
- Monitor connection pool metrics in production

### Future Integration

Currently, transaction state is available but repository operations don't yet use it automatically. A future update will refactor repository methods to use the active transaction when available, making all operations (personalStorage, sharedStorage, etc.) transaction-aware automatically.

## Troubleshooting

### "No active transaction" Error

This occurs when calling commit/rollback without first calling `beginTransaction()`:

```javascript
// Wrong
database.commitTransaction(); // Error: No active transaction

// Correct
database.beginTransaction();
// ... operations ...
database.commitTransaction();
```

### "Transaction timeout exceeded" Error

Transaction took longer than the specified timeout:

```javascript
// Increase timeout for complex operations
database.beginTransaction(60000); // 60 seconds
```

### "Savepoint not found" Error

Trying to rollback to a savepoint that doesn't exist or was already released:

```javascript
const sp = JSON.parse(database.createSavepoint());
database.rollbackToSavepoint(sp.savepoint); // OK
database.rollbackToSavepoint(sp.savepoint); // Error: Already rolled back
```

## See Also

- [Database Schema Management](./DATABASE_SCHEMA.md)
- [GraphQL API](./GRAPHQL.md)
- [Error Handling](./ERROR_HANDLING.md)
