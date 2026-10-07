# What Belongs in the Database, and What Must Not

The question this answers: the engine takes its configuration from `config.toml`
plus `APP_*` environment variables, and some of it has to arrive that way — a
connection string cannot be stored in the database it names. Could the rest move
into a table, leaving only what the execution environment must supply, so that
operating the engine becomes a UI rather than an env file?

Partly, and the dividing line is not "vital to the execution environment versus
the rest". That line puts `security.csrf_key` and `javascript.job_timeout_ms` on
the same side, and they could not be less alike: one is the anchor the editing
surface itself stands on, the other is a number somebody wants to change at two
in the morning without a rolling restart. Three narrower tests do the sorting,
applied in order, and each one disqualifies keys the next would have accepted.

## Test 1: can it be read before the database exists?

The chicken-and-egg set, and the smallest of the three:

`repository.database_url`, `repository.embedded`, `repository.embedded_data_dir`,
`repository.embedded_port`, `repository.max_connections`, `logging.level`,
`logging.format`.

`max_connections` is in this list because it sizes the pool that would do the
reading, and logging because a configuration load that fails wants to say so in
the format the operator is collecting.

## Test 2: is it a trust anchor for the surface that would edit it?

This is the test that matters, and it disqualifies far more than bootstrapping
does. A settings table edited over HTTP is reachable by whoever holds an
administrator session. So any value that decides **who is an administrator** or
**which requests are believed** cannot live where that surface can write it —
not because writing it is hard to authorize, but because the authorization would
be deciding its own preconditions.

| key                                                                                       | what a writable copy would mean                                                                                                                             |
| ----------------------------------------------------------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `security.csrf_key`, `session_encryption_key`, `secret_encryption_key`, `auth.jwt_secret` | A key stored beside the ciphertext it protects is not encryption at rest. `user_secrets` and `user_git_credentials` are encrypted under the third of these. |
| `server.trusted_proxies`                                                                  | Every rate-limit bucket and session fingerprint becomes forgeable again by one header — the whole of what `security/client_ip.rs` exists to close.          |
| `server.management_hosts`, `server.base_url`, `additional_base_urls`                      | These decide where `/engine/*` answers at all. A surface that can widen its own reachability is not a boundary.                                             |
| `auth.bootstrap_admins`, `auth.internal.bootstrap_admin_usernames`                        | Self-promotion in one row, applied on the next sign-in.                                                                                                     |
| `auth.cookie.secure`, `security.strict_ip_validation`                                     | Each one downgrades a protection the editing session is itself relying on.                                                                                  |
| `security.cors_allowed_origins`, `security.content_security_policy`                       | The reasoning in `security/cors.rs` about `"*"` plus credentials stops being an operator's decision and becomes an attacker's.                              |

These stay in the file and the environment precisely because changing them
should take reaching the deployment rather than holding a login. The test is not
"is this security-related" — `[logs]` retention is security-relevant and passes
fine. It is "does the check that would guard this write depend on this value".

## Test 3: what breaks when the value changes mid-flight?

What survives both tests is genuinely tunable, but the code today does not
merely read these once by convention; several of them are baked structurally,
and each needs its own answer to "when does a change take effect".

| setting                                                                                                          | what changing it actually requires                                                                                     |
| ---------------------------------------------------------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------- |
| `javascript.execution_timeout_ms`, `init_timeout_ms`, `job_timeout_ms`, `test_timeout_ms`, `test_run_timeout_ms` | Clean. Read per invocation already, through the `OnceLock`s in `script_init` and `scheduler`.                          |
| `javascript.max_memory_bytes`, `stack_size_bytes`                                                                | Per-runtime and per-thread; a change applies to the next execution, which is the honest and acceptable answer.         |
| `javascript.max_concurrent_executions`                                                                           | A `Semaphore` built once in `execution_slots`. Adding permits is easy, removing them is not.                           |
| `repository.statement_timeout_ms`, `lock_timeout_ms`, `idle_in_transaction_timeout_ms`                           | Applied per connection in `database.rs`; new connections only, so a change is invisible until the pool turns over.     |
| `server.max_connections_per_stream`, `max_total_stream_connections`                                              | Clean. Compared at `stream_registry::open_connection`.                                                                 |
| `[logs]` and `[revisions]` retention                                                                             | Clean, read by the pruning statement.                                                                                  |
| `prune_interval_secs` (both)                                                                                     | The loop's own sleep (`revisions.rs:1519`, `log_retention.rs:77`); takes effect on the following tick at the earliest. |
| `repository.max_script_size_bytes`, `max_upload_size_bytes`                                                      | Checked per write; clean.                                                                                              |
| `security.max_request_body_bytes`                                                                                | An Axum layer built at startup.                                                                                        |

So "move the timeouts into the database" is a small piece of plumbing. "Move
all the numbers" is a per-setting decision about whether a change lands now, on
the next execution, or on the next connection — and a UI that does not say
which is worse than a config file, because a file at least does not imply that
it took.

## Most of the machinery already exists

`script_limits.rs` is this pattern, already built, and its doctrine is the one
to inherit wholesale rather than to reinvent:

- Every field optional, absent meaning "follow the engine", so a row can raise
  a timeout without restating a memory ceiling.
- **Clamped rather than refused** (`MAX_TIMEOUT_MS` and its neighbours). This is
  the load-bearing part: a stored value takes effect with no restart, so there
  is no deploy for a mistyped one to fail at.
- Administrator rather than owner, because the thing being claimed is shared
  with every other tenant.
- Cached in a process-local map, loaded at startup before anything runs,
  refreshed through `notifications.rs` when a peer changes one.

`deployments::pinned` is the same shape for the same reasons. An
`engine_settings` table would slot into that machinery rather than needing new
machinery.

Which also means part of the benefit already exists. The number people most
want to change while reacting to a problem — one script's timeout — is settable
through `/engine/limits` today without touching a file. The remaining gap is
engine-wide defaults and retention, which is narrower than it first looks, and
that is worth knowing before sizing the work.

## Two decisions to take first

**Precedence, and who can still answer "what is running".** Today it is file,
then environment, and `cargo run -- --validate-config` describes what will run.
A database layer on top makes the env file no longer the final word — which
inverts what an operator expects — and turns that flag into a partial answer.
Keep file-then-environment as the floor, let the database override only keys
explicitly marked overridable, and add a `GET /engine/settings` reporting each
effective value **with its provenance**: default, file, environment, or
database. The provenance column is what makes the page safe to look at; without
it, a value that looks wrong cannot be traced to whoever set it.

**Recovery.** Configuration in the database can lock you out of the surface
that would fix it. Clamping covers a typo; it does not cover a policy mistake,
and it does not cover a pruner set to delete more than intended. The existing
escape hatches are the right precedent — `--grant-role` and `--set-password`
need the database and no running server — so this wants a `--reset-settings`
alongside them before it wants a UI.

## Where the win actually is, and where it is negative

Largest for the desktop and embedded install. There is no configuration
management tooling there, the database is a directory inside the app's own data
folder, and asking whoever runs a packaged application to edit TOML is a bad
answer to a question they should not have been asked.

Real for reactive tuning on a server: containing a script that has started
holding execution slots, or widening a budget, without a rolling restart. This
is the case `script_limits` was built for, extended to engine-wide defaults.

Negative for anything an operator wants reviewed before it changes. A cluster
deployment generally wants configuration in version control — diffable,
reviewable, rolling back with the release — and database-stored settings are
invisible to all of that. Which argues for the shape of the end state: the file
stays the source of truth, and the database layer is an explicit, audited,
clamped set of overrides over it, not a migration of the configuration into the
database.
