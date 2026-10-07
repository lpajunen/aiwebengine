# Haiku as a script author

The larger models build the platform: the engine, the agent harness, starter
templates, authoring skills and an evaluation suite. Haiku 4.5, running inside
`aiwebengine-agent`, builds simple scripts on top of them, and the engine checks
every step it takes.

## Who builds what

| Work                                                      | Model                    | Where                                                           |
| --------------------------------------------------------- | ------------------------ | --------------------------------------------------------------- |
| Engine: APIs, operations, error messages, the sandbox     | Opus / Fable             | Claude Code, `aiwebengine`                                      |
| Agent harness: the loop, tools, budgets, approvals        | Opus / Sonnet            | Claude Code, `aiwebengine-agent`                                |
| Complex scripts: other agents, multi-module solutions     | Sonnet / Opus            | Claude Code                                                     |
| Starter templates, authoring skills, the evaluation suite | Sonnet, reviewed by Opus | Claude Code, versioned in git                                   |
| Simple scripts: small sites, tools, single-purpose agents | Haiku 4.5                | aiwebengine-agent, in the engine                                |
| Fixing why Haiku failed an evaluation task                | Sonnet / Opus            | Claude Code: change the template, skill or engine, not the task |

When Haiku fails, the fix goes into the platform, so the next attempt succeeds
without a larger model.

## What counts as a simple script

A simple script is one a starter template mostly provides. Haiku fills in what
differs; it does not design the structure.

| Shape       | Files                                    | Engine features used                                        | Example                          |
| ----------- | ---------------------------------------- | ----------------------------------------------------------- | -------------------------------- |
| Web site    | `main.ts`, `public/*`, one or two routes | file routes, `registerRoute`, `scriptStorage`               | a sign-up page, a status board   |
| Tool        | `main.ts`, `lib/*.ts`, `*.test.ts`       | `mcpRegistry` tool, `fetch` with `{{secret:...}}`           | wrap a public API as an MCP tool |
| Small agent | `main.ts`, `skills/*.md`, `*.test.ts`    | one model call per turn, `personalStorage`, `personalTasks` | a FAQ bot over one document      |

Out of scope at first:

- database schema changes after the first version
- authorization callbacks on streams and file routes
- delegation, channels (Telegram, Slack) and webhooks that must check signatures
- scripts of more than about five files
- editing a script the run did not create

## The authoring loop

```text
Request → Copy a template → Pin the stub → Write files → Check and test → Passes?
          (pull_from_git)   (deploy_script) (write_files)  (check_script,     │
                                                 ▲          run_tests)        ├─ yes → Ask approval → Deploy
                                                 │ retry                      │        (request_deploy)   (deploy_script)
                                                 └──────── Fix from error ◄── no
                                                           (edit_file)
                                                                │ 3rd failure
                                                                ▼
                                                           Handoff note → developer in Claude Code
```

Haiku decides what to fix from the engine's checks, not from its own judgement.
Repeating a failure leads to a handoff note rather than another loop, and
nothing new is served until the person approves.

## What the engine provides

- **A short script primer**, `assets/script-primer.md`, served at
  `/engine/types/v{version}/script-primer.md` beside the 16,500-word
  `aiwebengine.d.ts`. It is hand-written, updated in the same commit as any API
  change it covers, and a test caps it at 1,500 words. A small model copies
  statements literally, so the primer and the `.d.ts` say only what is true —
  for example that `init()` is optional.
- **Errors that name the fix.** `check_script` reports a refused registration as
  `registration-refused` with the engine's reason; module-loader errors (export
  in `main.*`, dynamic `import()`, missing module, bad import binding) name the
  corrected form; `init-failed` and failing test cases carry a `Hint:` line for
  the usual causes (`src/fix_hints.rs`).
- **One call from write to verdict.** `write_files` and `edit_file` answer with
  a `check` report beside `init`; `check: false` opts out.
- **Short tool descriptions.** Each is capped at 500 characters and the whole
  listing at 26,000 by a test.
- **Drafts are pins.** The run's first write is the template's stub, whose
  `init()` registers nothing, and the run pins it with `deploy_script` at once;
  later writes advance head while the stub serves, until an approved deploy
  moves the pin. `tests/revisions.rs`
  (`a_draft_written_beside_a_pinned_stub_is_checked_and_tested_but_not_served`)
  runs the sequence over HTTP.

## What the harness provides

Described in the `aiwebengine-agent` README; `aiwebengine-agent/eval` runs the
suite.

1. **An authoring mode with its own budgets.** A run started with
   `kind: "author"` gets 8,000 output tokens and 25 turns (chat keeps 1,024 and
   12), and `jobTimeoutMs` 120 s via `set_script_limits`.
2. **Direct tools.** `create_script`, `write_files`, `edit_file`, `read_file`,
   `check_script`, `run_tests`, `read_logs` and `request_deploy`, each bound to
   the scripts the run created. Behaviour is checked through `run_tests`, since
   a pinned script's routes serve the pinned revision. Handlers live in
   `lib/handlers.ts` and `main.ts` makes them global with
   `Object.assign(globalThis, {...})`, so a test imports a handler and calls it
   with a context from `makeContext` in `lib/testing.ts`; an agent's model call
   is a parameter, so a test passes a fake.
3. **One guide per shape.** `build-site`, `build-tool` and `build-agent` in
   `agent/authoring/`, returned by `create_script` with the primer rather than
   left as skills to be found, because a small model skips that step.
4. **Templates in git**, in
   [`aiwebengine-template`](https://github.com/lpajunen/aiwebengine-template),
   one directory per shape, pulled with `pull_from_git` as `template-<shape>`.
5. **Escalate instead of looping.** The same check or test failure three times,
   or running out of turns, stops the run and writes a handoff note. A turn
   that speaks and calls no tool is nudged to act, three times at most.
6. **Deployment needs approval.** `request_deploy` runs the check and the tests,
   refuses on a failure, and then asks; approving moves the pin to head.

## Evaluation

- **Suite:** about 10 tasks per shape. Each is a written request plus
  acceptance tests the run never sees: `*.test.ts` against head, and HTTP
  probes once the evaluation has approved the deploy.
- **Run** after every change to the primer, skills, templates or engine API, and
  weekly otherwise.
- **Measured per task:** passed, turns, tokens and cost, escalated.
- **Failure review:** a larger model reads the failed runs and changes the
  platform. Changing the task to make it pass is not allowed.

| Metric                       | Target             |
| ---------------------------- | ------------------ |
| Tasks passed without help    | at least 80%       |
| Turns per passed task        | 15 or fewer        |
| Cost per passed task         | under $0.10        |
| Escalations that were needed | fewer than 1 in 10 |

## Safety

- **The `author` delegation scope only, never `administer`**, for authoring
  runs. The person must hold the editor role and owns what is created.
- **Only its own scripts.** The harness refuses writes to scripts the run did
  not create.
- **Drafts before production.** Serving changes only through an approved
  `deploy_script`.
- **Bounded spending.** New scripts get low `set_script_limits` values;
  `run_js` keeps its host allowlist.
- **Undo is easy.** Every write is a revision; `revert_script` restores the last
  version that started cleanly.

## Where it stands

The baseline, platform and harness phases are done: on the ten seed tasks
the agent passed every check in the latest run (`aiwebengine-agent/eval`),
against none of five before this work. What is next is real use — Haiku
building small scripts for people, with each failure becoming a new suite task
— and then widening the scope one capability at a time from the out-of-scope
list, each with tasks, as long as the pass rate holds on the whole suite.

## Later goals

These come after the widening. Each starts as suite tasks.

1. **The agent replaces the `aiwebengine-dev` tools.** `admin/`, `editor/` and
   `docs/` stop being deployed; their work is done by prompting the agent with
   engine authority (`author` and `administer` scopes, which `delegation.rs`
   already grants only as far as the person's own roles reach). Retire each
   script only once the suite covers what people used it for.
2. **A small web app when a prompt is the wrong interface.** For work that is
   awkward in a conversation — reviewing and changing dozens of users, browsing
   logs, comparing revisions — the agent builds a single-purpose page for the
   task instead. That page is an ordinary script owned by the person, using the
   same engine operations through `engine.call`, so it gets no authority the
   person lacks. A "build an admin page" template makes this a simple script.
3. **Improving existing solutions.** The agent creates and changes solutions in
   `aiwebengine-examples` and `aiwebengine-private`, pulling them with
   `pull_from_git` and pushing changes back with `push_to_git` for review. This
   needs editing scripts the run did not create, so it waits until the
   own-scripts-only rule can be relaxed per task.
4. **Channel bots.** New Telegram (and later Slack) bots built from a template
   that already checks the webhook signature and links senders through
   `personalTasks.enqueueFrom` invitations — the parts that are easy to get
   wrong live in the template, not in what Haiku writes.

## Decided

- **Drafts are pins**, not a separate host.
- **Templates live in
  [`aiwebengine-template`](https://github.com/lpajunen/aiwebengine-template).**
- **The primer is hand-written**, like `aiwebengine.d.ts`.
- **Escalation is a note a person carries** to a developer in Claude Code. The
  agent does not switch to a larger model itself; revisit once the note format
  has been used for a while.

## Open questions

- Which of the `admin/`, `editor/` and `docs/` features are actually used, and
  so need suite coverage before retiring them?
