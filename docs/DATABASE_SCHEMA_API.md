# Database Schema Management API

This document describes the new database schema management API that allows JavaScript scripts to create and manage their own database tables.

## Overview

Scripts can now dynamically create database tables, add columns of various types, create foreign key relationships, and drop tables when needed. All tables are automatically namespaced per script and cleaned up when the script is deleted.

## Security

- Requires the `ManageScriptDatabase` capability (granted to authenticated users and admins)
- In development mode, anonymous users also have this capability for testing
- Each script's tables are isolated using a hash-based prefix
- Maximum limits: 10 tables per script, 50 columns per table

## API Reference

### `database.createTable(tableName)`

Creates a new table for the current script with an automatic `id` column (SERIAL PRIMARY KEY).

**Parameters:**

- `tableName` (string): Logical table name (must match `^[a-z][a-z0-9_]*$`)

**Returns:** JSON string with `success` and `tableName` or `error`

**Example:**

```javascript
const result = database.createTable("users");
const data = JSON.parse(result);
if (data.error) {
  log("Error: " + data.error);
} else {
  log("Created table: " + data.tableName);
}
```

### `database.addIntegerColumn(tableName, columnName, nullable, defaultValue)`

Adds an INTEGER column to an existing table.

**Parameters:**

- `tableName` (string): Table name
- `columnName` (string): Column name (must match `^[a-z][a-z0-9_]*$`)
- `nullable` (boolean, optional): Whether column can be NULL (default: true)
- `defaultValue` (string, optional): Default value as string (e.g., "42")

**Returns:** JSON string with `success` or `error`

**Example:**

```javascript
// Add age column, not nullable, default 0
database.addIntegerColumn("users", "age", false, "0");

// Add score column, nullable, no default
database.addIntegerColumn("users", "score", true, null);
```

### `database.addTextColumn(tableName, columnName, nullable, defaultValue)`

Adds a TEXT column to an existing table.

**Parameters:**

- `tableName` (string): Table name
- `columnName` (string): Column name
- `nullable` (boolean, optional): Whether column can be NULL (default: true)
- `defaultValue` (string, optional): Default value (will be automatically quoted)

**Returns:** JSON string with `success` or `error`

**Example:**

```javascript
// Add name column with default
database.addTextColumn("users", "name", false, "Anonymous");

// Add description column, nullable
database.addTextColumn("users", "description", true, null);
```

### `database.addBooleanColumn(tableName, columnName, nullable, defaultValue)`

Adds a BOOLEAN column to an existing table.

**Parameters:**

- `tableName` (string): Table name
- `columnName` (string): Column name
- `nullable` (boolean, optional): Whether column can be NULL (default: true)
- `defaultValue` (string, optional): "true", "false", "1", "0", etc.

**Returns:** JSON string with `success` or `error`

**Example:**

```javascript
// Add active flag, default true
database.addBooleanColumn("users", "active", false, "true");

// Add verified flag, nullable
database.addBooleanColumn("users", "verified", true, null);
```

### `database.addTimestampColumn(tableName, columnName, nullable, defaultValue)`

Adds a TIMESTAMPTZ column to an existing table.

**Parameters:**

- `tableName` (string): Table name
- `columnName` (string): Column name
- `nullable` (boolean, optional): Whether column can be NULL (default: true)
- `defaultValue` (string, optional): "NOW()" or specific timestamp

**Returns:** JSON string with `success` or `error`

**Example:**

```javascript
// Add created_at with automatic timestamp
database.addTimestampColumn("users", "created_at", false, "NOW()");

// Add last_login, nullable
database.addTimestampColumn("users", "last_login", true, null);
```

### `database.createReference(tableName, columnName, referencedTableName)`

Creates a foreign key constraint between two script-owned tables.

**Parameters:**

- `tableName` (string): Source table name
- `columnName` (string): Column name in source table (must be INTEGER)
- `referencedTableName` (string): Target table name (references the `id` column)

**Returns:** JSON string with `success` and `foreignKey` description or `error`

**Example:**

```javascript
// Create tables
database.createTable("authors");
database.createTable("books");

// Add foreign key column
database.addIntegerColumn("books", "author_id", false, null);

// Create the reference
const result = database.createReference("books", "author_id", "authors");
// Result: {"success": true, "foreignKey": "books.author_id -> authors"}
```

### `database.dropTable(tableName)`

Drops a script-owned table and all its data.

**Parameters:**

- `tableName` (string): Table name to drop

**Returns:** JSON string with `success`, `dropped` (boolean indicating if table existed), or `error`

**Example:**

```javascript
const result = database.dropTable("users");
const data = JSON.parse(result);
if (data.dropped) {
  log("Table was dropped");
} else {
  log("Table did not exist");
}
```

## Table Naming and Isolation

- **Logical names**: What you use in the API (e.g., "users")
- **Physical names**: Actual PostgreSQL table names (e.g., "script_a1b2c3d4_users")
- Tables are prefixed with `script_{hash}_` where hash is derived from the script URI
- Multiple scripts can create tables with the same logical name without conflicts
- Physical table names are tracked in the `script_tables` metadata table

## Automatic Cleanup

When a script is deleted:

1. All tables owned by that script are automatically dropped
2. Metadata entries in `script_tables` are removed via CASCADE
3. Foreign key relationships are cleaned up

When a script is updated:

- Tables are NOT affected
- Schema changes must be done explicitly via the API

## Limits and Constraints

- **Maximum tables per script**: 10
- **Maximum columns per table**: 50
- **Identifier requirements**: Must match `^[a-z][a-z0-9_]*$` (lowercase, alphanumeric + underscore)
- **Reserved keywords**: Cannot use SQL reserved words as identifiers
- **Column types**: Only INTEGER, TEXT, BOOLEAN, and TIMESTAMPTZ are supported

## Complete Example

```javascript
function init() {
  // Create a blog system with authors and posts

  // Authors table
  database.createTable("authors");
  database.addTextColumn("authors", "name", false, "Anonymous");
  database.addTextColumn("authors", "email", false, "unknown@example.com");
  database.addTimestampColumn("authors", "joined_at", false, "NOW()");

  // Posts table
  database.createTable("posts");
  database.addTextColumn("posts", "title", false, "Untitled");
  database.addTextColumn("posts", "content", true, null);
  database.addIntegerColumn("posts", "author_id", false, null);
  database.addBooleanColumn("posts", "published", false, "false");
  database.addTimestampColumn("posts", "created_at", false, "NOW()");
  database.addTimestampColumn("posts", "updated_at", true, null);

  // Create foreign key relationship
  database.createReference("posts", "author_id", "authors");

  log("Blog database schema created successfully");
}
```

## Error Handling

All API functions return JSON strings. Always parse the result and check for errors:

```javascript
const result = database.createTable("users");
const data = JSON.parse(result);

if (data.error) {
  log("Error creating table: " + data.error);
  // Handle error (common errors: permission denied, table exists, limit exceeded)
} else {
  log("Success: " + JSON.stringify(data));
}
```

## Future Enhancements

The current API only supports schema management (DDL operations). Future versions will add:

- Data manipulation: INSERT, SELECT, UPDATE, DELETE operations
- Query builder API for safe SQL generation
- Transaction support
- Bulk operations
- Table introspection (list tables, describe columns)

## Testing

A comprehensive test script is available at `scripts/test_database_schema.js`. To run the tests:

1. Make sure the migration has been applied: `sqlx migrate run`
2. Upload the test script to the system
3. Access the test endpoints:
   - `/test/db/create` - Test table creation
   - `/test/db/columns` - Test column additions
   - `/test/db/references` - Test foreign keys
   - `/test/db/drop` - Test table deletion
   - `/test/db/full` - Run full workflow test
