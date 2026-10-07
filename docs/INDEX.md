# Documentation

What the engine is and how to run it is in the root [README](../README.md) and
[DEPLOYMENT.md](../DEPLOYMENT.md). Every configuration key is described in
`config.toml`. Rules for working on the engine are in `CLAUDE.md`; what is open
is in [ROADMAP.md](../ROADMAP.md).

## Running an engine

| Document                                                        | What it covers                                                        |
| --------------------------------------------------------------- | --------------------------------------------------------------------- |
| [Deployment options](../DEPLOYMENT.md)                          | Desktop, local, containerised and server deployments                  |
| [OAuth and secrets](engine-administrators/OAUTH-AND-SECRETS.md) | Providers, the first administrator, the engine's keys, script secrets |
| [Operations](engine-administrators/OPERATIONS.md)               | Health, logs, accounts and roles, backup and restore                  |
| [Troubleshooting](engine-administrators/TROUBLESHOOTING.md)     | Start-up, sign-in, proxy and CORS problems                            |
| [Internal authentication](INTERNAL_AUTH.md)                     | Guests and local username/password accounts                           |
| [Engine styles](ENGINE_STYLES.md)                               | The pages the engine renders itself, and how to restyle them          |

## Developing a script

The JavaScript API is `assets/aiwebengine.d.ts`; `assets/script-primer.md` is
the short introduction an agent reads first. Solution developers' guides are in
the `aiwebengine-dev` repository.

| Document                                                   | What it covers                                               |
| ---------------------------------------------------------- | ------------------------------------------------------------ |
| [Writing files as one change](ASSET_BATCH.md)              | `write_files`: several files, one transaction, one revision  |
| [Editing without resending](ASSET_EDIT.md)                 | `edit_file`, ranged and grepped reads, `search_files`        |
| [Checking scripts](SCRIPT_CHECKS.md)                       | `check_script`: bundle and run `init()` without deploying    |
| [Testing scripts](SCRIPT_TESTS.md)                         | `*.test.ts` files and `run_tests`                            |
| [Evaluating snippets](SCRIPT_EVAL.md)                      | `eval_script` against a deployed script's sandbox            |
| [Revisions and deployments](SCRIPT_REVISIONS.md)           | History, pinning, revert, diff, labels                       |
| [Reading a script's log](SCRIPT_LOGS.md)                   | Filters, correlation by request and revision, live tail      |
| [Sharing through Git](GIT_SYNC.md)                         | Pulling from and pushing to a GitHub repository              |
| [The script database](DATABASE_SCHEMA_API.md)              | `database`: a script's own tables                            |
| [Transactions](TRANSACTIONS.md)                            | `database.transaction(fn)`                                   |
| [Work that outlives the request](SCRIPT_TASKS.md)          | Script and personal tasks                                    |
| [Concurrent fetches](FETCH_CONCURRENCY.md)                 | `fetchAll`, `fetchStream`, and the per-script limits on both |
| [Verifying webhooks](SCRIPT_CRYPTO.md)                     | `crypto` with keys the script never holds                    |
| [Calling MCP servers](MCP_CLIENT.md)                       | `McpClient`: tools on another MCP server                     |
| [Asking the person mid-tool](MCP_ELICITATION.md)           | `mcp.ask` and elicitation                                    |
| [Managing the engine from a script](ENGINE_API_FROM_JS.md) | `engine.call` and the operation table                        |

## Authority and limits

| Document                                       | What it covers                                             |
| ---------------------------------------------- | ---------------------------------------------------------- |
| [What narrows what](WHAT_NARROWS_WHAT.md)      | Every way a call ends up with less authority, in one place |
| [Running with less](CAPABILITY_ATTENUATION.md) | `sandbox.run` and capability attenuation                   |
| [Acting for someone away](DELEGATION.md)       | Delegation: background work on a person's behalf           |
| [Session elevation](SESSION_ELEVATION.md)      | Holding roles only while using them                        |
| [What one script may spend](SCRIPT_LIMITS.md)  | Per-script limits                                          |

## Working on the engine

| Document                                                     | What it covers                                 |
| ------------------------------------------------------------ | ---------------------------------------------- |
| [Contributing](../CONTRIBUTING.md)                           | Workflow, checks and commits                   |
| [Architecture](engine-contributors/ARCHITECTURE.md)          | Why each module is shaped as it is             |
| [Roadmap](../ROADMAP.md)                                     | What is open                                   |
| [Haiku as a script author](HAIKU_AUTHORING.md)               | Building simple scripts with a small model     |
| [Settings in the database](SETTINGS_IN_DATABASE.md)          | Design: which settings could move into a table |
| [Engine ideas](engine-contributors/planning/ENGINE-IDEAS.md) | Ideas that are not commitments                 |
| [Editor ideas](engine-contributors/planning/EDITOR-IDEAS.md) | Ideas for the editor in `aiwebengine-dev`      |
