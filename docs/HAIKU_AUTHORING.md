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
                                                 │ retry                      │        (request_approval) (deploy_script)
                                                 └──────── Fix from error ◄── no
                                                           (edit_file)
                                                                │ 3rd failure
                                                                ▼
                                                           Handoff note → developer in Claude Code
```

Haiku decides what to fix from the engine's checks, not from its own judgement.
Repeating a failure leads to a handoff note rather than another loop, and
nothing new is served until the person approves.

## Engine work (aiwebengine)

1. **A short script primer.** `aiwebengine.d.ts` is about 16,500 words. Write a
   primer of about 1,500 words covering the globals a simple script uses, and
   serve it as an engine asset. It is written by hand, like the `.d.ts`, and
   updated in the same commit as any API change it covers. _Done:_
   `assets/script-primer.md`, served at
   `/engine/types/v{version}/script-primer.md`; a test caps it at 1,500 words.
2. **Fix what the types get wrong.** The `.d.ts` says every script MUST export
   `init()`, which the engine does not require. A small model copies such
   statements literally. _Done:_ `init()` is documented as optional, and the
   header says handlers are named by string and must be top-level functions.
3. **Errors that name the fix.** `check_script`, `run_tests` and refused
   registrations answer with the file, the line and the corrected call.
4. **One call from write to verdict.** `write_files` can return the
   `check_script` result and the `init()` outcome together.
5. **Short tool descriptions.** The 50 engine tools' descriptions total about
   50,000 characters; cut each to what it does plus one rule.
6. **Drafts are pins, not another host.** The run's first write is the
   template's stub, whose `init()` registers nothing, and the run pins that
   revision with `deploy_script` at once. From then on writes advance head while
   the stub keeps serving, until an approved deploy moves the pin. No engine
   change: a new script has no revision to pin until its first write, which is
   why the stub goes first.

## Harness work (aiwebengine-agent)

1. **An authoring mode with its own budgets.** About 8,000 output tokens per
   turn (today `MAX_TOKENS` is 1,024), about 25 steps (today 12), and
   `jobTimeoutMs` 120 s via `set_script_limits`. Chat runs stay as they are.
2. **Direct tools instead of discovery.** `write_files`, `edit_file`,
   `read_file`, `check_script`, `run_tests`, `read_logs` and `deploy_script` —
   no `engine_tools` lookup. There is no HTTP probe: a pinned script's routes
   serve the pinned revision, so behaviour is checked through `run_tests`,
   which runs against head and calls handlers directly. The templates ship a
   test helper that builds a request context for that.
3. **One skill per shape.** `build-site`, `build-tool`, `build-agent`: steps,
   template and known pitfalls, in the existing SKILL.md format.
4. **Templates in git**, in
   [`aiwebengine-template`](https://github.com/lpajunen/aiwebengine-template),
   one directory per shape, fetched with `pull_from_git`.
5. **Escalate instead of looping.** After the same check fails three times, the
   run stops and writes a handoff note: the request, the script and revision,
   the last error and what was tried. The person gives the note to a developer
   working in Claude Code; nothing hands off automatically. The note's format
   is part of the authoring skill, so a developer can start from it without
   asking.
6. **Deployment needs approval.** `deploy_script` always goes through
   `request_approval`.

## Evaluation

- **Suite:** about 10 tasks per shape. Each is a written request plus
  acceptance tests the run never sees: `*.test.ts` against head, and HTTP
  probes once the evaluation has approved the deploy.
- **Run** after every change to the primer, skills, templates or engine API, and
  weekly otherwise.
- **Measured per task:** passed, turns, tokens and cost, escalated.
- **Failure review:** a larger model reads the failed runs and changes the
  platform. Changing the task to make it pass is not allowed.

| Metric                       | Target for phase 3 |
| ---------------------------- | ------------------ |
| Tasks passed without help    | at least 80%       |
| Turns per passed task        | 15 or fewer        |
| Cost per passed task         | under $0.10        |
| Escalations that were needed | fewer than 1 in 10 |

The targets are starting guesses; adjust them after the baseline.

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

## Phases and gates

| Phase       | Work                                                                                                   | Built by                    | Gate to the next phase                            |
| ----------- | ------------------------------------------------------------------------------------------------------ | --------------------------- | ------------------------------------------------- |
| 1. Baseline | Write the suite; run today's agent with only budgets raised                                            | Claude Code (Sonnet)        | A recorded pass rate and the top failure causes   |
| 2. Platform | Primer, `.d.ts` fixes, actionable errors, `write_files` returning its check, shorter tool descriptions | Claude Code (Opus)          | Baseline rerun improves; `make check` passes      |
| 3. Harness  | Authoring mode, direct tools, three skills, templates in git, escalation                               | Claude Code (Opus / Sonnet) | 80% pass, 15 turns or fewer, under $0.10 per task |
| 4. Real use | Used for your own small scripts; failures become new suite tasks                                       | Haiku, reviewed by you      | Two weeks without a fix needing a larger model    |
| 5. Widen    | Add one capability at a time from the out-of-scope list, each with tasks                               | Claude Code + Haiku         | Pass rate holds on the whole suite                |

## Later goals

These come after phase 5. Each starts as suite tasks, like any widening.

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

- **Drafts are pins**, not a separate host (engine work, item 6).
- **Templates live in
  [`aiwebengine-template`](https://github.com/lpajunen/aiwebengine-template).**
- **The primer is hand-written**, like `aiwebengine.d.ts`.
- **Escalation is a note a person carries** to a developer in Claude Code. The
  agent does not switch to a larger model itself; revisit once the note format
  has been used for a while.

## Open questions

- Does 8,000 output tokens per turn fit in 120 s for Haiku on this engine?
  Measure in phase 1.
- Which of the `admin/`, `editor/` and `docs/` features are actually used, and
  so need suite coverage before retiring them?
