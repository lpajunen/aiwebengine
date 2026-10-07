# Database Transactions

`database.transaction(fn)` runs `fn` so that its writes succeed or fail
together: committed when `fn` returns, rolled back when it throws, and
answering what `fn` returned.

```javascript
function transfer(context) {
  const { from, to, amount } = context.request.json();
  return database.transaction(
    () => {
      const [source] = database.query("accounts", {
        where: { owner: from },
        forUpdate: true,
      });
      if (source.balance < amount) throw new Error("insufficient funds");
      database.update("accounts", source.id, {
        balance: source.balance - amount,
      });
      const [target] = database.query("accounts", {
        where: { owner: to },
        forUpdate: true,
      });
      database.update("accounts", target.id, {
        balance: target.balance + amount,
      });
      return ResponseBuilder.json({ ok: true });
    },
    { timeoutMs: 5000 },
  );
}
```

A scoped function is the whole API, because the one easy mistake with
separate begin and commit calls is leaving a transaction open.

## Nesting

Inside another transaction, `transaction(fn)` is a savepoint: if `fn` throws,
only its own work is undone and the outer transaction carries on.

```javascript
database.transaction(() => {
  database.insert("orders", order);
  try {
    database.transaction(() => {
      database.insert("loyalty_points", points); // may fail on its own
    });
  } catch (e) {
    console.warn("points not awarded: " + e.message);
  }
  // the order is still there, and commits with the outer transaction
});
```

## Async functions

An async `fn` is awaited before the transaction commits, so work after an
`await` is inside it:

```javascript
await database.transaction(async () => {
  const quote = await fetchQuote();
  database.insert("quotes", quote);
});
```

Await the transaction itself. One whose function never settles is finished at
the handler boundary — committed if the handler succeeded, rolled back if it
failed — so it cannot leak into the next request that runs on the same thread.

## Timeouts

`timeoutMs` is enforced by the database rather than recorded: within the
transaction no single statement runs longer, no lock is waited on longer, and
a transaction left idle that long is ended and its locks released. It bounds
each step, not their sum. A budget only tightens the engine's own limits —
asking for ten minutes on an engine that allows five seconds gets five.

## What a transaction does not give you

A transaction makes a group of writes all-or-nothing. On its own it does not
stop another transaction reading the same rows at the same time. Scripts run
at Postgres's `READ COMMITTED`, where a plain `query` takes no lock, so this
counter loses updates under concurrency even though every transaction
commits:

```javascript
// WRONG under concurrency: two callers can read the same seq
database.transaction(() => {
  const [row] = database.query("event_seq", { limit: 1 });
  database.update("event_seq", row.id, { seq: row.seq + 1 });
});
```

### Reading in order to write

`forUpdate: true` holds the rows a query returns until the transaction ends. A
second caller waits for the first to commit and then reads what it wrote:

```javascript
database.transaction(() => {
  const [row] = database.query("event_seq", { limit: 1, forUpdate: true });
  database.update("event_seq", row.id, { seq: row.seq + 1 });
});
```

- A read whose value decides a later write needs `forUpdate`. A read whose
  result is only returned to the caller does not.
- `forUpdate` outside a transaction is refused: the lock would be released as
  soon as the query returned, which would read like a guard without being one.
- Lock rows in a consistent order across handlers; two transactions taking the
  same two rows in opposite orders can deadlock.
- Keep the transaction short. Every other caller wanting those rows waits on
  it, and it holds a database connection for its whole life.

### One instance at a time

For "only one instance does this" rather than a row guard, use a row as the
lock: a table with a unique `lease_id`, read `forUpdate` inside a transaction,
taken when it is missing, expired or already yours. The unique index makes a
racing first `insert` fail rather than both callers believing they won. Work
that has to run one at a time per key is often better as `scriptTasks` with a
`lane`, which the queue serialises for you.

## Errors inside a transaction

A refused value — a fraction in an `integer` column, say — is refused before
anything is sent, so the transaction stays usable. A statement that fails in
the database, such as a duplicate key, runs inside a savepoint the engine
places around it, so catching the error and carrying on works too:

```javascript
database.transaction(() => {
  database.insert("readings", { amount: 1 });
  try {
    database.insert("readings", { amount: 1 }); // duplicate: throws
  } catch (e) {}
  database.insert("readings", { amount: 2 }); // still fine; both commit
});
```

## Implementation

The transaction lives in thread-local storage for the length of the
invocation, so every `database` call made inside `fn` joins it without being
passed anything. The host half is three calls the prelude hides —
`beginTransaction`, `commitTransaction`, `rollbackTransaction` — where a begin
inside an open transaction is a `SAVEPOINT` and its commit or rollback
releases or rolls back to it. `src/database.rs` holds the state;
`assets/database_prelude.js` holds `transaction(fn)`.

## See also

- [The Script Database](./DATABASE_SCHEMA_API.md)
