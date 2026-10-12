---
name: run-one-test
description: Run one engine test, one test file or a filtered set with cargo nextest, and get past the usual reasons it will not start. Use when asked to run, rerun or debug a specific test, or to check a change against the tests that cover it before the full suite.
---

# Run one test

Every test needs Postgres (`make postgres-local`); none skips without it.
`cargo nextest` is the only supported runner — `cargo test` does not pass.

| What                      | Command                                         |
| ------------------------- | ----------------------------------------------- |
| Tests whose name matches  | `cargo nextest run <filter>`                    |
| One integration test file | `cargo nextest run --test <file_stem>`          |
| One test in that file     | `cargo nextest run --test <file_stem> <name>`   |
| Unit tests of one module  | `cargo nextest run --lib <module>::`            |
| With its output           | add `--no-capture`                              |
| The whole suite           | `make test` (or `make check` before committing) |

`<file_stem>` is the name under `tests/` without `.rs`, e.g.
`--test security_capabilities`.

## When it will not start or behaves oddly

- **Cannot reach the database:** start it with `make postgres-local`. Never
  point a test at a connection string of its own; tests reach the database
  through `common::test_pool` / `test_db::pool`.
- **Schema errors after editing an existing migration:** the test template is
  rebuilt automatically when a migration is _added_, not when one is edited.
  Drop it and the next run rebuilds it:

  ```bash
  docker exec aiwebengine-local-postgres \
    psql -U aiwebengine -d aiwebengine -c 'DROP DATABASE aiwebengine_test_template'
  ```

- **Fails sometimes, passes on rerun:** a race to fix, not to retry. No
  retries, serial groups or skips (`CLAUDE.md`, Tests).
