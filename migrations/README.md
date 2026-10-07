# Database Migrations

SQL migrations for the engine's Postgres database, applied in filename order.
The engine applies any pending ones itself when it starts (`Database::migrate`
in `src/database.rs`), with the statement and lock timeouts lifted.

## Adding one

```bash
cargo install sqlx-cli --no-default-features --features postgres
sqlx migrate add <description>   # creates migrations/<timestamp>_<description>.sql
```

A changed or added migration invalidates the test template database:
`DROP DATABASE aiwebengine_test_template` and the next test run rebuilds it.
Check a migration against a copy of real data too
(`CREATE DATABASE x TEMPLATE aiwebengine`).

Migrations are forward-only. For what that means during a rolling upgrade, see
`DEPLOYMENT.md` (Operational essentials).
