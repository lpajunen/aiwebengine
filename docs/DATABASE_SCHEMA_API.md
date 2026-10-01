# The Script Database

`database` is a script's own tables: described with `ensureTable`, read and
written with six row calls, and grouped with `transaction(fn)`. Every call
takes objects, answers with a value, and throws when it fails — including when
the caller lacks the capability it takes.

```javascript
function init() {
  database.ensureTable("notes", {
    columns: [
      { name: "owner", type: "text" },
      { name: "body", type: "text" },
      { name: "created_at", type: "bigint" },
    ],
    uniqueIndexes: [["owner", "created_at"]],
  });
}

function addNote(context) {
  const row = database.insert("notes", {
    owner: context.request.auth.userId,
    body: context.request.body,
    created_at: Date.now(),
  });
  return ResponseBuilder.json(row); // { id, owner, body, created_at }
}
```

## The ten calls

| call                              | answers                                           |
| --------------------------------- | ------------------------------------------------- |
| `ensureTable(name, schema)`       | `{ created, columnsAdded, uniqueIndexesEnsured }` |
| `dropTable(name)`                 | `{ tableName, dropped }`                          |
| `dropColumn(name, column)`        | `{ tableName, columnName, dropped }`              |
| `query(name, options?)`           | the rows                                          |
| `insert(name, row)`               | the row as stored, with its `id`                  |
| `update(name, id, changes)`       | the row as stored                                 |
| `delete(name, id)`                | `{ deleted }`                                     |
| `upsert(name, keyColumns, row)`   | the row as stored                                 |
| `deleteWhere(name, where)`        | `{ deleted }` — how many                          |
| `transaction(fn, { timeoutMs? })` | what `fn` returned                                |

`transaction` has [its own page](TRANSACTIONS.md).

## Describing a table: `ensureTable`

`ensureTable(name, { columns, uniqueIndexes? })` brings a table to the shape
described, whatever shape it is in now: it creates the table if it is missing,
adds the columns it lacks, and makes the unique indexes. A table that is
already right costs one query and changes nothing, so the call belongs at the
top of `init()` rather than behind a "has this run" flag. Concurrent callers —
a cold start where every instance's first write arrives at once — take turns
rather than racing.

Every table has an `id` column of its own. A column is
`{ name, type, nullable?, default?, references? }`:

| type        | holds                                                                     |
| ----------- | ------------------------------------------------------------------------- |
| `integer`   | whole numbers up to about 2.1 billion; a fraction is refused, not rounded |
| `bigint`    | whole numbers to 2^53 — epoch milliseconds and anything past `integer`    |
| `float`     | any JavaScript number                                                     |
| `text`      | strings                                                                   |
| `boolean`   | `true` / `false`                                                          |
| `timestamp` | an instant, read back as an ISO 8601 string                               |
| `reference` | the `id` of a row in the table named by `references`, with a foreign key  |

Columns default to nullable: a column added to a table that already has rows
cannot be `NOT NULL` without a default, and being safe against a table in use
is the point of the call. `default` is SQL text (`"0"`, `"true"`, `"NOW()"`).

A column already present is left as it is — `ensureTable` never changes a
column's type. Changing one is `dropColumn` and then `ensureTable` again.

```javascript
database.ensureTable("authors", { columns: [{ name: "name", type: "text" }] });
database.ensureTable("books", {
  columns: [
    { name: "title", type: "text" },
    { name: "author_id", type: "reference", references: "authors" },
  ],
});
```

## Reading and writing rows

```javascript
database.query("chat", { orderBy: "ts", order: "desc", limit: 100 });
database.query("presence", {
  where: { last_active: { $gt: Date.now() - 90000 } },
});
```

`query`'s options are `where`, `limit` (default
{{limits.database.defaultQueryLimit}}, at most {{limits.database.maxQueryLimit}}),
`orderBy`, `order` (`"asc"` or `"desc"`) and `forUpdate` (see
[transactions](TRANSACTIONS.md#reading-in-order-to-write)). An option the
engine does not know is refused rather than ignored: `{ limt: 5 }` answering
every row would be the wrong failure.

A `where` is `{ col: value }` for equality and `{ col: { $gt, $gte, $lt,
$lte, $ne } }` for a comparison; several conditions are AND-ed. `deleteWhere`
takes the same, and refuses an empty one.

`upsert(name, keyColumns, row)` inserts, or updates the row with the same key.
The key columns must carry a unique index — `ensureTable`'s `uniqueIndexes`.

Values are typed by their column. A value the column cannot hold is refused,
naming the column, rather than coerced: a fraction in an `integer` column is
not rounded, and a number past a column's range is not wrapped. Values come
back as their column's type, not as whatever the stored bytes look like.

## Capabilities

- `ensureTable`, `dropTable` and `dropColumn` take `ManageScriptDatabase`,
  held by editors and administrators. Changing the shape of a solution's data
  is authoring, not using.
- `query` takes `ReadScriptData`; the writes take `WriteScriptData`. Every
  signed-in user holds both, because a script serving a request runs as the
  person asking, and their ordinary use of the solution has to work.
- A sandboxed sub-execution or a delegated run may hold the read without the
  write; its writes then throw.

## Naming, isolation and cleanup

- The name a script uses (`notes`) is logical. The table Postgres holds is
  `script_{hash}_notes`, so two scripts may each have a `notes` table.
- A name matches `^[a-z][a-z0-9_]*$`, at most 63 characters, and is not an SQL
  reserved word.
- A script may have 50 tables of 50 columns.
- Deleting a script drops every table it owns. Rewriting a script leaves its
  tables alone; their shape changes only through `ensureTable` and the drops.
