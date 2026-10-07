# Pilot readiness

What stands between the engine and its first outside users: a closed pilot of
5–15 people on a hosted instance, building small web apps and AI tools by
prompting `aiwebengine-agent`. Tick an item when it is done and verified, and
say where in the item. Items that are also on [ROADMAP.md](../../../ROADMAP.md)
name their entry there; the reasoning stays in the roadmap.

Written 2026-10-07 from a review of the engine, `aiwebengine-agent`,
`aiwebengine-dev`, `aiwebengine-examples` and `aiwebengine-template`.

## Where it stands

Strong enough to show:

- **Built for an agent to develop on.** One operation table reached by HTTP,
  MCP and `engine.call`; short tool descriptions; errors that name the fix
  (`fix_hints.rs`); `write_files` answers with a check.
- **Code as the action, with a real sandbox.** `sandbox.run` with capability
  attenuation is what other agent frameworks have to build first.
- **Safe iteration.** A revision per write, drafts as pins, revert and diff,
  tests in the script's sandbox, `eval_script` with rollback.
- **A security model with documents behind it.** Capabilities, attenuation,
  consent-gated delegation, `{{secret:...}}`, host-side `crypto`.
- **Most of what a small app needs**: routes, SSE, MCP tools, prompts and
  resources, an MCP client, `tools`, a scheduler, durable personal tasks, a
  database, rate limits, audit, OAuth and local accounts, git sync.
- **Cheap authoring** by Haiku against an evaluation suite, where a failure
  is fixed in the platform rather than the task.
- **Simple to run**: one binary and Postgres, or the desktop mode.

## Before the pilot

### 1. Who it is for

- [ ] Name the pilot user and the one job they come with (for example: a
      non-developer building a small internal tool or MCP tool by prompting).
      Measure the pilot against that job, not against "any web app".
- [ ] Write down what the pilot should learn, and how it will be read:
      which numbers, which interviews, after how long.

### 2. Onboarding without the author present

Today an author needs an administrator to grant the editor role, their own
Anthropic key, a delegation grant with the `author` scope,
`set_script_limits jobTimeoutMs=120000` on the agent, and the templates pulled
from git.

- [ ] A hosted pilot instance with the agent, the templates and the agent's
      limits already in place.
- [ ] A way for an invited person to become an author without an
      administrator acting by hand (an invitation that grants the editor role,
      or a pilot realm).
- [ ] One "start here" page: sign in, give the key and the grant, build the
      first thing.
- [ ] A short demo in the root README: what gets built, in how long, with a
      picture.

### 3. One namespace per script

Routes are first-come per `(host, path, method)`; on a shared host the first
person to claim `/` or `/api/items` blocks everyone after. The evaluation
avoids it by prefixing every task.

- [ ] Give each new script its own place by default — a subdomain or a path
      prefix — so two pilot users cannot collide.
- [ ] The templates and the authoring guides use it.

### 4. Quotas

- [ ] A per-account budget on the operation table (ROADMAP: Security 3).
- [ ] Low default script limits for scripts the agent creates, and a storage
      cap per account.
- [ ] Each person sees what their runs cost, on every channel (the agent's
      "Spend on a channel").

### 5. The agent's open security items

- [ ] `run_js` passes a `hosts` allowlist (agent `TODO-improvements.md`:
      "Where `run_js` may send").
- [ ] Decide whether a delegated execution may send a bearer token to this
      engine's own `/mcp` (ROADMAP: Security 1).
- [ ] The Telegram webhook secret moves to `secretStorage`, compared with
      `crypto.secretEquals`.
- [ ] The audit line says when an agent acted rather than the person
      (ROADMAP: Security 4).

### 6. Documents that agree with the engine

- [x] Every document in the six repositories checked against the engine
      (2026-10-07: operation arguments, `/engine` and `/auth` paths, config
      keys, `make` targets and JS globals checked mechanically; guides read and
      rewritten where they had drifted).
- [ ] Repeat that check just before the pilot starts.
- [ ] One place a pilot user is sent to read, rather than four repositories.
- [ ] The `docs` script's own copy of `engine.css` (served at `/engine.css`)
      has drifted from the engine's `/engine/engine.css`, and nothing in these
      repositories links it any more; remove the copy and its route.

### 7. Verified in a browser

- [ ] The editor's and administrator's flows after the operation-table and
      slug changes (ROADMAP: Engine internals 7).
- [ ] Git sync against a real repository.
- [ ] One end-to-end smoke run: sign in, prompt the agent, approve the deploy,
      reach the result.

### 8. Learning from every run

- [ ] Keep every handoff note and failed authoring run where they can be
      read later.
- [ ] Turn each one into an evaluation task; the ten seed tasks all pass and
      no longer separate a good change from a bad one.
- [ ] The agent's tests run under `run_tests` in the engine, and the case
      counts in its documents match.

### 9. Personal data

- [ ] Account erasure reaches personal data in scripts' own tables
      (ROADMAP: Security 2), or the pilot says plainly that it does not.
- [ ] A short privacy statement: what is stored, where, for how long, and how
      to have it deleted.
- [ ] Decide whether AGPL-3.0 is the license the pilot's users should meet.

## After the first pilot: more kinds of app

In the order they would widen what people can build.

- [ ] Packages: a way to bring an npm package into a script's tree, pinned,
      without a build step.
- [ ] Web-standard globals: `URL`, `TextEncoder`/`TextDecoder`,
      `structuredClone` (ROADMAP: Script API 4).
- [ ] Handlers as function values, not names (ROADMAP: Script API 2).
- [ ] Sending email or notifications: invitations, resets, alerts.
- [ ] `database`: aggregates, typed rows, schema changes after the first
      version (ROADMAP: Script API 3).
- [ ] A frontend kit in the templates: one small vendored client library so
      generated pages look and behave alike.
- [ ] App-level groups and invitations, so each app does not reinvent
      membership.
- [ ] An event loop: `setTimeout`, promise-returning `fetch`
      (ROADMAP: Script API 1).
- [ ] Custom domains for a script.
- [ ] The agent edits scripts it did not create, and changes schemas
      (`docs/HAIKU_AUTHORING.md`, "Out of scope at first").

## Engine health

Matters to contributors more than to pilot users; all on the roadmap.

- [ ] Split the `/mcp` dispatch out of `lib.rs` (ROADMAP: Engine internals 1).
- [ ] Error types below the operations, so no refusal is read from its text
      (ROADMAP: Engine internals 2).
- [ ] Delete the unused security scaffolding (ROADMAP: Engine internals 4).
- [ ] No unit test skips without a database (ROADMAP: Engine internals 5).
