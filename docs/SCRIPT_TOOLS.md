# Using Another Script's Tools

`tools` calls the MCP tools other scripts on this engine publish, in process,
as the person the calling execution runs for. It is what `tools/call` on
`/mcp` would do for that person, without a token: an agent script can use the
apps its person built the way the person could.

```js
const found = tools.list("backlog"); // [{ name, description, inputSchema, script }]
const open = tools.call("backlog_list", { status: "open" }, { readOnly: true });
tools.call("backlog_add", { title: "Write the release notes" });
```

## Who the tool runs as

The caller. The tool's handler runs under the calling execution's principal
and capabilities, narrowing included. A delegated run stays delegated, with the
same scopes, so the tool reaches only what the person granted. Which script
makes the call decides nothing.

`readOnly: true` takes every write capability away for the call. A tool that
tries to change something then gets the refusal it would get anywhere else,
and the engine enforces that, not the tool.

## Who may call

The capability is `call_tools`:

| Execution                                          | Holds it                                 |
| -------------------------------------------------- | ---------------------------------------- |
| A signed-in request or `/mcp` call                 | Yes                                      |
| An anonymous request                               | No: `/mcp` takes no call without a token |
| A delegated task                                   | Only when the grant includes `tools`     |
| `sandbox.run`                                      | Only when named in `capabilities`        |
| `init()`, a scheduled job, a task nobody delegated | Never: nobody to act as                  |

## What is reachable

Tools of scripts that share a host with the calling script, the set `/mcp`
lists on those hosts. With no host binding configured, every script tool. The
engine's own tools are not listed and not callable here; they are
`engine.call`'s, behind its own grant.

## What a tool cannot do when called this way

Ask the person anything (`mcp.canAsk()` is false) or hand back a task handle
(`mcp.canTask()` is false). There is no client to answer either.

Calls nest up to the `sandbox.run` depth limit, and a nested call shares its
caller's time budget.

Failures throw: `NotFoundError` for a name that is not reachable,
`SecurityError` without `call_tools`, and `ToolError` when the tool itself
failed.
