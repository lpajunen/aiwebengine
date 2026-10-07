# Contributing

The rules for working on the engine — coding rules, invariants, the module map
and how tests are run — are in [CLAUDE.md](CLAUDE.md), and they apply to people
and agents alike. Why the engine is shaped as it is:
[docs/engine-contributors/ARCHITECTURE.md](docs/engine-contributors/ARCHITECTURE.md).
What is open: [ROADMAP.md](ROADMAP.md).

## Setting up

```bash
make deps                          # development tools
make postgres-local                # Postgres; the engine and every test need it
source .env-local && cargo run     # http://localhost:3000
```

## Before a change

1. For anything larger than a fix, open an issue first and say what you intend.
2. Branch from `main`.
3. A change that alters the JavaScript API or an engine operation changes the
   script repositories with it (`aiwebengine-examples`, `aiwebengine-dev`,
   `aiwebengine-agent`, `aiwebengine-template`, `aiwebengine-private`). There
   is no
   backwards-compatibility requirement.

## Before a pull request

```bash
make check     # format-check, lint, typecheck and the test suite
```

CI runs the same commands. Tests are run with `cargo nextest` (`make test`);
`cargo test` is not a supported runner.

- Zero compiler and clippy warnings, and no `unwrap()`/`expect()` outside
  tests.
- Every change that alters behaviour comes with a test that fails without it.
- Update the documentation the change makes untrue, and `assets/aiwebengine.d.ts`
  with any API change. Docs and comments describe what is true now; history
  belongs in the commit message.
- Commit messages follow Conventional Commits: `feat:`, `fix:`, `refactor:`,
  `test:`, `docs:`, `perf:`, `chore:`, with `!` for a breaking change.

## Security

Changes under `src/security/`, `src/auth/` and `src/delegation.rs` are
security-critical. Report vulnerabilities as described in
[SECURITY.md](SECURITY.md), not in a public issue.
