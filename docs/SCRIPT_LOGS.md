# Reading a Script's Log

Everything a script writes with `console.log`, `console.info`, `console.warn`,
`console.error` or `console.debug` is stored, along with the failures the engine
reports on the script's behalf — an `init()` that threw, a scheduler job that
panicked.

Two things make that store usable rather than a wall of text: every line records
**which invocation emitted it**, and the log can be **followed as it is
written**.

```bash
# What did this one request do?
curl "https://your-engine/engine/script_logs?uri=myapp&request_id=$REQUEST_ID"

# Watch a live session, one route only
curl -N "https://your-engine/engine/script_logs/stream?uri=myapp&route=/world/:id/move"
```

Reading logs takes the `ViewLogs` capability, and the endpoints answer only on a
management host.

## What a log entry carries

```json
{
  "scriptUri": "myapp",
  "message": "moving alpha",
  "level": "LOG",
  "timestamp": 1755861234567,
  "seq": 91423,
  "requestId": "8f1c…",
  "kind": "httpRoute",
  "route": "/world/:id/move"
}
```

`requestId` groups the lines one invocation emitted. For an HTTP request it is
the request's `x-request-id` — the header the response carries back — so a
client that saw something go wrong can name the exact call. Invocations that
are not HTTP requests generate their own: one per scheduler tick, per message
listener call, per MCP tool call, per stream customization. That is what makes a
single tick separable from the hundreds around it.

`kind` says what sort of invocation it was: `httpRoute`, `graphqlQuery`,
`graphqlMutation`, `graphqlSubscription`, `scheduled`, `streamCustomization`,
`messageListener`, `mcpTool`, `mcpPrompt`, `init`, `eval` or `test`.

`route` is the **registered pattern** (`/world/:id/move`), not the concrete path,
so filtering by it collects every call to a handler instead of one bucket per
parameter value. For invocations that are not HTTP routes it names the job,
stream, resolver or tool.

`seq` is the entry's place in the engine's write order. It breaks timestamp ties
and doubles as a cursor — see [Reading forward](#reading-forward).

A handler can read its own id from the context and hand it to a client, so a bug
report arrives with the id of the run that produced it:

```ts
export function moveHandler(context: HandlerContext) {
  console.log("moving " + context.request.params.id);
  return { status: 500, body: JSON.stringify({ trace: context.invocationId }) };
}
```

What the engine writes _about_ a request is filed under that request too: a
handler that threw, one that exceeded its budget and was stopped, one that never
ran. Those arrive at `FATAL`, alongside the lines the handler managed to write
before it stopped.

Engine-internal writes that belong to no invocation — the startup banner,
transpiler diagnostics — carry no context, and their `requestId`, `kind` and
`route` are `null`.

## When a filter returns nothing

- **`route=` takes the registered pattern**, not the path you called.
  A route registered as `/world/:id/move` is stored under that, so
  `route=/world/17/move` matches nothing. `GET /engine/routes` (or the
  `list_routes` MCP tool) lists the patterns as registered.
- **Entries written before the engine gained these columns** have `null` for all
  three, so any filter on them skips those rows. Only lines written since carry
  an invocation.
- **A handler that is `async` stops at its first `await`.** Scripts run
  synchronously — host calls block rather than yielding — so a promise never
  settles and nothing after the await runs. Lines it would have logged there do
  not exist to be filtered. The request fails with a message naming the handler,
  and that failure is in the log under the request id.

## Listing

```text
GET /engine/script_logs
```

| Parameter    | Meaning                                                     |
| ------------ | ----------------------------------------------------------- |
| `uri`        | One script; omit for every script                           |
| `level`      | One level, e.g. `error` (case-insensitive)                  |
| `contains`   | Message contains this substring (case-insensitive, literal) |
| `request_id` | Only the lines one invocation emitted                       |
| `kind`       | Only invocations of this kind, e.g. `scheduled`             |
| `route`      | Only lines logged while serving this registered pattern     |
| `since`      | At or after this time (epoch millis or RFC 3339)            |
| `after_seq`  | Only entries written after this `seq`                       |
| `limit`      | Keep at most this many entries                              |

Filters combine, and they are applied in the database — a narrow question does
not pull the whole table across to answer it. Entries come back oldest-first for
a single script and newest-first across all of them.

`limit` keeps the _newest_ matching entries, except with `after_seq`, where it
keeps the next page after the cursor instead.

### Reading forward

`seq` is a cursor. Take the highest one you have seen and ask for what came
after it:

```bash
curl "https://your-engine/engine/script_logs?uri=myapp&after_seq=91423&limit=200"
```

Unlike `since`, this can neither repeat nor skip entries that share a timestamp,
and paging forward with a `limit` cannot step over what falls between pages.

## Following the log live

```text
GET /engine/script_logs/stream
```

A Server-Sent Events stream carrying entries as they are written. It takes every
filter the listing takes — narrowing a tail to one route, one kind or one
request id is what makes watching a live session legible — plus:

| Parameter   | Meaning                                                           |
| ----------- | ----------------------------------------------------------------- |
| `backlog`   | Replay this many of the newest matching entries before going live |
| `after_seq` | Resume after this `seq`, replaying everything written since       |
| `since`     | Start at this time instead of at the end of the log               |

With none of the three, the tail starts at the end: only what happens from now
on. `after_seq` takes precedence over `since`, which takes precedence over
`backlog`.

Events:

- `open` — the tail is live, with the `seq` it starts after. Sent immediately,
  so a client knows the connection is good even if nothing is ever logged.
- `log` — one entry, as the same JSON the listing returns, oldest-first.
- `error` — the tail gave up after repeated query failures, with the `seq` it
  reached so a client can reconnect from there.

```bash
curl -N "https://your-engine/engine/script_logs/stream?uri=myapp&backlog=50&level=error"
```

If a tail drops, reconnect with `after_seq` set to the last `seq` you saw and
nothing is missed in between.

The tail polls the database rather than being pushed to from the write path.
Two consequences worth knowing: in a cluster it sees every instance's output,
not just the one it is connected to; and it shows what was actually committed,
so lines from a transaction that rolled back — an `/engine/eval` snippet, a
handler that threw — never appear.

## Over MCP

The `read_logs` tool takes the same filters as the listing, including
`request_id`, `kind`, `route`, `contains` and `after_seq`. There is no streaming
equivalent — MCP is request/response — but polling `read_logs` with `after_seq`
returns only what is new, which is the same loop a tail runs.

`prune_logs` clears one script's logs, or prunes every script back to its 20
newest entries when no `uri` is given. `DELETE /engine/script_logs` does the
same over HTTP.
