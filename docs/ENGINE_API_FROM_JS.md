# Managing the Engine From a Script

```ts
// A route an agent's tool call reduces to.
function whoAreTheUsers() {
  if (!engine) throw new Error("no management tools in this execution");
  const { users, count } = engine.call("list_users", {});
  return { status: 200, body: JSON.stringify({ count }) };
}
```

Engine administration was deliberately not exposed to JavaScript. The reason
was never that scripts are untrustworthy — it was that **all scripts are
equal**, so exposing it would have meant every script in the engine seeing it,
and there is no privileged/restricted split to hang an exception on.

What answers that is the thing the capability model already does. Every call
goes through `engine_api::execute_native_mcp_tool` — the same function `/mcp`
calls — and is authorized against the **calling** `UserContext`. A script
holding `engine` holds nothing its caller does not. There is one implementation
of each tool rather than an in-process copy that could drift from the HTTP one,
and no second authorization model to keep in step with the first.

## Why it exists

An agent asked "who are the users" had no route to an answer. `list_users`
takes `AdministerEngine`; the agent's turns are queued, so they are delegated;
and `delegation.rs` capped delegated work at `authenticated`. The only way
round was for the script to store a `/mcp` bearer token and call the engine
over HTTP — which reaches the same place through a credential, **past** every
check in `delegation.rs` rather than through one, and does not work in local
development at all (`HttpClient::is_private_ip` refuses loopback).

Two changes together remove that: `Scope::Author` and `Scope::Administer` let a
person consent to management being done as them (see `docs/DELEGATION.md`), and
this makes the tools reachable without a credential to store.

## Where it is, and where it is not

Every execution acts for a `Principal` (`security/secure_globals/context.rs`), and
`engine` exists exactly when that principal came from a credential — a
`Caller` or a `Delegated` person:

| Execution                                | Principal   | Has it | Why                                              |
| ---------------------------------------- | ----------- | ------ | ------------------------------------------------ |
| A script serving a request               | `Caller`    | yes    | runs as whoever made the request                 |
| An MCP tool or prompt handler            | `Caller`    | yes    | runs as whoever called it                        |
| A stream or asset authorization function | `Caller`    | yes    | runs as whoever is connecting or reading         |
| A delegated task                         | `Delegated` | yes    | the person consented, on a page that said so     |
| A task nobody delegated                  | `Engine`    | no     | acts for nobody                                  |
| A scheduled job                          | `Engine`    | no     | acts for nobody                                  |
| `init()`, startup                        | `Engine`    | no     | the engine bringing a script up                  |
| A test run                               | `Contained` | no     | executes a script's cases, not somebody's intent |
| `/engine/eval_script`, `sandbox.run`     | `Contained` | no     | see below                                        |

**The capability check is not enough on its own**, which is the whole reason
the principal decides rather than `has_capability`.

The engine's own actors — a scheduled job, an undelegated task, `init()`,
startup — have nobody behind them. They hold what the script may do to its own
rows, files, storage and registrations (`UserContext::engine_actor`), and that
includes capabilities such as `ViewLogs` and `ManageMcp` that the management
tools are gated on while taking any script as an argument. Reaching
`engine.call` from a cron line would turn the script's own authority into
authority over every script, which is not something anybody conferred.

And a `sandbox.run` subset **cannot express the distinction this needs**. An
agent grants `view_logs` so that model-authored code can use `console`.
`read_logs` is gated on exactly `view_logs`, and takes any script's URI as an
argument. Granting the one would hand over the other, and no arrangement of the
capability vocabulary separates them — so model-authored code does not get this
surface at all.

## The shape of a call

`engine.call(name, args)` returns the tool's JSON and **throws** when the tool
refuses. Every native tool reports a refusal the same way (`{ error: "..." }`),
so that check lives here once instead of in every script that forgets it — the
`tasks_prelude.js` rule. The thrown error is named `EngineError` and carries
`.tool`.

An unknown tool name is a _different_ failure and says so: a plain `Error`
rather than an `EngineError`, because a name this engine does not serve is a
mistake in the script and not a permission problem, and reading "access denied"
for a typo sends somebody looking in the wrong place.

`engine.callRaw(name, args)` returns the envelope instead, for a caller that
would rather branch than catch.

## Discovery

`engine.tools(area?)` lists what this engine serves, names and descriptions
only. Read it rather than hard-coding a list: what a deployment serves is a
property of the deployment, and the input schemas are large enough that a model
should be handed them one at a time rather than all 52 at once.

For an agent, the useful shape is two tools rather than fifty-two —
`engine_tools(area)` to look and `engine_call(name, args)` to act — which keeps
the prompt flat and lets the list be filtered by what the caller could actually
use.

## What this is not

It is not a new capability, a bypass, or a special case. It reaches the
same operations as `/engine/*` and `/mcp`, with the same checks. Ownership is still checked at every write
(`repository::user_owns_script`), `AdministerEngine` still marks acting on what
you do not own, and a refusal still audits.

There is deliberately no engine endpoint for it either. `/engine/*` and `/mcp`
are how a person or a client reaches these tools; this is how a script reaches
them on that person's behalf, and a third door would be a third place to get
the rules right.
