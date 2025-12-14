# Database Schema API Refactoring (Dec 14, 2024)

## Summary

Refactored the JavaScript database schema management API based on user feedback to improve naming clarity and add missing functionality.

## Changes Made

### 1. Renamed `createReference` to `addReferenceColumn`

**Rationale**: The new name better reflects that this function both creates a column AND establishes a foreign key relationship in one step.

**Before:**
```javascript
// Two-step process
database.addIntegerColumn("books", "author_id", false, null);
database.createReference("books", "author_id", "authors");
```

**After:**
```javascript
// One-step process with clearer naming
database.addReferenceColumn("books", "author_id", "authors", false);
```

**New Parameter**: Added `nullable` (boolean, optional, default: true) to specify whether the reference column can be NULL.

### 2. Added `dropColumn` Function

**Purpose**: Allows scripts to remove columns from tables, matching the pattern of `dropTable`.

**Signature:**
```javascript
database.dropColumn(tableName, columnName)
```

**Returns**: JSON with `success`, `dropped` (boolean), `tableName`, `columnName`, or `error`

**Safety**: Prevents dropping the `id` column to maintain table integrity.

**Example:**
```javascript
const result = database.dropColumn("users", "age");
const data = JSON.parse(result);
if (data.dropped) {
  log("Column was dropped");
} else {
  log("Column did not exist");
}
```

## Files Modified

### Backend (Rust)

1. **src/repository.rs**
   - Removed: `db_create_foreign_key()` (old implementation)
   - Added: `db_add_reference_column()` - creates INTEGER column + FK in one operation
   - Added: `db_drop_column()` - drops column and updates metadata
   - Updated: Repository trait with new method signatures
   - Updated: PostgresRepository, MemoryRepository, UnifiedRepository implementations
   - Updated: Public wrapper functions `add_reference_column()` and `drop_column()`

2. **src/security/secure_globals.rs**
   - Renamed: `createReference` JavaScript function to `addReferenceColumn`
   - Added: `nullable` parameter to `addReferenceColumn`
   - Added: `dropColumn` JavaScript function
   - Updated: Return values to include nullable flag in addReferenceColumn response

### Documentation

3. **docs/DATABASE_SCHEMA_API.md**
   - Removed: `database.createReference` section
   - Added: `database.addReferenceColumn` section with nullable parameter documentation
   - Added: `database.dropColumn` section
   - Updated: Examples to show one-step reference creation

### Tests

4. **scripts/test_database_schema.js**
   - Updated: `testForeignKeys()` to use new `addReferenceColumn` API
   - Added: `testDropColumn()` test function
   - Added: Route registration for `/test/db/drop-column`
   - Updated: Comments to reflect new API

## API Changes Summary

### Removed
- `database.createReference(tableName, columnName, referencedTableName)` ❌

### Added
- `database.addReferenceColumn(tableName, columnName, referencedTableName, nullable)` ✅
- `database.dropColumn(tableName, columnName)` ✅

### Modified
- None of the existing functions changed signatures

## Backward Compatibility

⚠️ **Breaking Change**: Scripts using `database.createReference()` must be updated to use `database.addReferenceColumn()`.

**Migration Path:**
```javascript
// Old code (two steps):
database.addIntegerColumn("table", "col", false, null);
database.createReference("table", "col", "ref_table");

// New code (one step):
database.addReferenceColumn("table", "col", "ref_table", false);
```

## Technical Implementation Details

### `db_add_reference_column` Implementation

1. Validates all identifiers (script URI, table names, column name)
2. Checks that column doesn't already exist
3. Creates INTEGER column with appropriate NULL constraint
4. Creates foreign key to referenced table's `id` column
5. Updates schema_json metadata in script_tables
6. Uses PostgreSQL transaction for atomicity

### `db_drop_column` Implementation

1. Validates identifiers
2. Prevents dropping the `id` column
3. Checks if column exists (returns false if not)
4. Drops the column (CASCADE to remove dependent constraints)
5. Updates schema_json metadata
6. Returns boolean indicating whether column was found and dropped

## Security

- Both functions require `ManageScriptDatabase` capability
- Table isolation via hash-based prefixes remains unchanged
- Foreign keys can only reference other script-owned tables
- Maximum limits (10 tables, 50 columns per table) still enforced

## Test Coverage

All existing tests pass with updated API:
- Table creation ✅
- Column addition (all types) ✅
- Foreign key references (updated to use addReferenceColumn) ✅
- Column dropping (new test) ✅
- Table dropping ✅
- Full workflow ✅

## Build Verification

```bash
$ cargo check
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 4.43s
```

## Next Steps

1. Update any existing scripts using `createReference` to use `addReferenceColumn`
2. Consider adding integration tests for column dropping edge cases
3. Document migration guide for existing scripts
