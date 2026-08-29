# Database Transactions

This document describes the transaction support in aiwebengine, allowing JavaScript handlers to perform atomic database operations.

## Overview

Transactions group database operations so they succeed or fail together:

- **Automatic lifecycle**: Transactions auto-commit on normal handler exit and auto-rollback on exceptions
- **Manual control**: JavaScript APIs allow explicit transaction management
- **Nested transactions**: PostgreSQL savepoints enable nested transaction scopes
- **Timeout protection**: Configurable timeouts prevent long-running transactions from holding connections

## What a Transaction Does Not Give You

A transaction makes a group of writes **all-or-nothing**. On its own it does
not stop another transaction reading the same rows at the same time.

Scripts run at PostgreSQL's default isolation level, `READ COMMITTED`, where a
plain `database.query()` takes no lock. So this counter is wrong, even though
every line of it looks right and every transaction commits successfully:

```javascript
// WRONG under concurrency — two callers can read the same seq
database.beginTransaction(5000);
const row = database.query("event_seq", null, 1).json()[0];
database.update("event_seq", row.id, JSON.stringify({ seq: row.seq + 1 }));
database.commitTransaction();
```

Run ten of those at once and they do not produce ten distinct numbers. Several
read the same value, each writes the same result, and the later writes replace
the earlier ones. Nothing errors — this is a **lost update**, and it is what
`READ COMMITTED` is specified to allow.

### Reading in Order to Write

Pass `{ forUpdate: true }` to hold the rows the query returns until the
transaction ends. A second caller then waits for the first to commit and reads
what it wrote:

```javascript
// Correct: the row is held for the rest of the transaction
database.beginTransaction(5000);
const row = database
  .query("event_seq", null, 1, null, null, JSON.stringify({ forUpdate: true }))
  .json()[0];
database.update("event_seq", row.id, JSON.stringify({ seq: row.seq + 1 }));
database.commitTransaction();
```

Rules of thumb:

- Any **read whose value decides a later write** in the same transaction needs
  `forUpdate`. A read whose result is only returned to the caller does not.
- `forUpdate` outside a transaction is **refused**. The lock would be released
  the moment the query returned, which would read like a guard without being
  one.
- Lock the rows in a consistent order across handlers. Two transactions that
  take the same two rows in opposite orders can deadlock.
- Hold the transaction for as little time as possible — every other caller
  wanting those rows is waiting on it. See [Keep Transactions
  Short](#2-keep-transactions-short).

For a whole-operation mutex rather than a row guard — "only one instance runs
this job" — use `database.acquireLease()` instead.

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

- `timeout_ms` (optional): budget in milliseconds for each statement, lock wait
  and idle gap in the transaction

**Returns:** JSON string with `{success: true}` or `{error: "..."}`

The budget is enforced by PostgreSQL rather than merely recorded. Within the
transaction it becomes `statement_timeout`, `lock_timeout` and
`idle_in_transaction_session_timeout`, which means:

- no single statement runs longer than the budget;
- no wait for a lock exceeds it;
- if the handler is stopped mid-transaction — a budget kill does not unwind the
  JavaScript stack, so the commit or rollback at the handler boundary never runs
  — PostgreSQL ends the transaction and releases its locks once the budget's
  worth of idleness has passed.

It bounds each step, not their sum: a hundred fast statements can still take
longer than the budget between them. Bounding that is the execution budget's
job.

A budget can only tighten the engine's configured limits, never loosen them.
Asking for ten minutes on an engine that allows five seconds gets five. Omitting
it leaves those limits in force, which for an abandoned transaction means the
engine's `idle_in_transaction_timeout_ms` rather than the handler's own budget.

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

Specify a budget close to what the work actually needs. It is the tightest bound
the transaction gets, and the one that decides how quickly its locks come back
if the handler never returns:

```javascript
// 30 second budget for complex operations
database.beginTransaction(30000);
```

A budget longer than the engine allows is silently clamped to the engine's
limit, so there is no benefit to padding it.

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

### 3. Guard Reads That Decide Writes

If a value read inside a transaction determines what that transaction writes,
read it with `forUpdate`. Without it the read takes no lock and the write can
be lost silently — see [What a Transaction Does Not Give
You](#what-a-transaction-does-not-give-you):

```javascript
// Good: the balance is held while it is being spent
database.beginTransaction(5000);
const account = database
  .query(
    "accounts",
    JSON.stringify({ id: accountId }),
    1,
    null,
    null,
    JSON.stringify({ forUpdate: true }),
  )
  .json()[0];
if (account.balance >= amount) {
  database.update(
    "accounts",
    account.id,
    JSON.stringify({ balance: account.balance - amount }),
  );
}
database.commitTransaction();
```

### 4. Use Savepoints for Partial Rollback

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

### 5. Handle Errors Explicitly

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

### Repository Operation Support

**Current Status**: Transaction infrastructure is in place, but individual repository operations (like `personalStorage.setItem()`, `scriptStorage.setItem()`) don't yet automatically participate in transactions.

**Manual Transaction Use**: You can use transactions to wrap multiple database operations:

```javascript
export function atomicUpdate(req) {
  database.beginTransaction();

  // These operations will each use separate connections from the pool
  // They are not yet automatically using the active transaction
  personalStorage.setItem("key1", "value1");
  personalStorage.setItem("key2", "value2");

  // But the automatic commit/rollback still works at the handler level
  return { status: 200, body: "Updated" };
}
```

**Future Enhancement**: A future update will refactor repository methods to automatically use the active transaction when available. This will make operations like `personalStorage.setItem()`, `scriptStorage.setItem()`, and database query/insert/update methods fully transaction-aware without code changes.

**Workaround**: For now, if you need true atomic operations, you can:

1. Use raw SQL queries within transactions (if you have database access)
2. Structure your handlers to use transactions for critical sections only
3. Rely on the automatic rollback on exception to prevent partial state

### Integration with Raw SQL

If your handlers execute raw SQL (via future database query APIs), those operations will automatically use the active transaction:

```javascript
export function transferWithSQL(req) {
  const { from, to, amount } = JSON.parse(req.body);

  database.beginTransaction();

  // Future API - raw SQL will use the active transaction
  // database.query("UPDATE accounts SET balance = balance - $1 WHERE id = $2", [amount, from]);
  // database.query("UPDATE accounts SET balance = balance + $1 WHERE id = $2", [amount, to]);

  // Auto-commits on success
  return { status: 200, body: "Transfer complete" };
}
```

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

## Testing

### Unit Tests

The transaction implementation includes 14 unit tests that run without requiring a database connection:

```bash
cargo test --lib database::tests
```

These tests verify:

- Transaction state management
- Timeout handling
- Error conditions (no database, no transaction, etc.)
- TransactionGuard RAII behavior

### Integration Tests

Three integration tests require a live PostgreSQL connection but will gracefully skip if DATABASE_URL is not available:

```bash
# Run all database tests (integration tests skip without DATABASE_URL)
cargo test --lib database::tests

# Run with DATABASE_URL to execute integration tests
DATABASE_URL="postgresql://user:pass@localhost/testdb" \
  cargo test --lib database::tests

# Run a specific integration test with verbose output
DATABASE_URL="postgresql://user:pass@localhost/testdb" \
  cargo test --lib database::tests::test_full_transaction_lifecycle -- --nocapture
```

**Tests included:**

- `test_full_transaction_lifecycle`: Begin transaction, create savepoint, release savepoint, commit
- `test_transaction_rollback_lifecycle`: Begin transaction, rollback (instead of commit)
- `test_nested_savepoints`: Test multiple savepoints and rollback to earlier savepoint

**Technical details:**

- Tests automatically skip if DATABASE_URL environment variable is not set
- Tests use `tokio::task::spawn_blocking` to simulate handler execution environment
- Each test creates its own database pool with increased connection limits (10 connections, 5-second timeout)
- Tests share a global database instance via `OnceLock` (first test initializes, others reuse)
- Transaction guards (`TransactionGuard`) must be stored to prevent automatic rollback on drop

## See Also

- [Database Schema Management](./DATABASE_SCHEMA.md)
- [GraphQL API](./GRAPHQL.md)
- [Error Handling](./ERROR_HANDLING.md)
