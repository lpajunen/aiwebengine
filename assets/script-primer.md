# aiwebengine script primer

A short guide to writing a simple script. The full API is in
`aiwebengine.d.ts`, served beside this file; read it for anything not here.

## What a script is

A script is a tree of files. There is no build step: TypeScript (`.ts`, `.tsx`)
and JavaScript run as written, in a sandboxed QuickJS runtime.

- The entrypoint is the file `main.ts` (or `main.js`, `main.tsx`, `main.jsx`).
- `public/` holds files a route may serve to anyone. `resources/` holds files
  that may be MCP resources. Everything else is private.
- Imports name the file exactly, extension included: `import { total } from "./lib/basket.ts"`.
  `import text from "./skills/faq.md"` gives the file's text.
- Tests are files named `*.test.ts`.

## Registering things: `init()`

Routes, MCP tools and jobs are registered by a top-level `function init()` in
`main.ts`. It is optional; a script without one registers nothing.

```ts
/// <reference path="../types/aiwebengine.d.ts" />

function hello(context: HandlerContext): HttpResponse {
  const name = context.request?.query.name ?? "World";
  return ResponseBuilder.json({ message: `Hello, ${name}!` });
}

function init(): void {
  routeRegistry.registerRoute("/hello", { handler: "hello", method: "GET" });
}
```

Rules that cause most failures:

- A handler is named **by string** and must be a **global function** of
  `main.ts`: either a top-level function there, or one imported from `lib/` and
  made global with `Object.assign(globalThis, { home, about })`. A handler that
  is not defined, or is not a function, is a 500 on the first request. `main.ts`
  itself never uses `export`.
- Register only inside `init()`. Registering from a handler does nothing and
  answers `{ ok: false, reason }`.
- Registration problems are returned, not thrown: always read the result. A
  path another script already holds is refused, and the reason names it.
- Top-level code runs again on every request. Keep it to definitions.

## HTTP routes

```ts
routeRegistry.registerRoute("/api/items", { handler: "listItems" }); // GET
routeRegistry.registerRoute("/api/items", {
  handler: "addItem",
  method: "POST",
});
routeRegistry.registerRoute("/api/items/:id", { handler: "getItem" });
routeRegistry.registerRoute("/", { file: "public/index.html" }); // a served file
```

`:param` and a trailing `/*` match path parts. A `file` route must name a file
under `public/` that exists in the tree.

The handler receives `context.request`:

| Field            | Meaning                                                               |
| ---------------- | --------------------------------------------------------------------- |
| `method`, `path` | the request line                                                      |
| `query`          | `?a=1` as `{ a: "1" }`                                                |
| `params`         | `:id` from the path as `{ id: "..." }`                                |
| `headers`        | `headers.get("content-type")`, or `headers["content-type"]`           |
| `body`, `text()` | the raw body                                                          |
| `json()`         | the body parsed; throws if it is not JSON                             |
| `form`           | form-encoded fields as `{ name: "value" }`                            |
| `auth`           | `isAuthenticated`, `userId`, `userName`, `userEmail`, `requireAuth()` |

It returns a response built with `ResponseBuilder`:

```ts
ResponseBuilder.json({ ok: true }); // 200
ResponseBuilder.json({ error: "bad" }, 400);
ResponseBuilder.text("plain");
ResponseBuilder.html("<h1>Hi</h1>");
ResponseBuilder.error(404, "Not found");
ResponseBuilder.noContent();
ResponseBuilder.redirect("/login");
```

Check input yourself and answer 400 for a bad body:

```ts
function addItem(context: HandlerContext): HttpResponse {
  const req = context.request!;
  let body: { name?: string };
  try {
    body = req.json() as { name?: string };
  } catch {
    return ResponseBuilder.error(400, "Body must be JSON");
  }
  if (!body.name) return ResponseBuilder.error(400, "name is required");
  // ...
  return ResponseBuilder.json({ name: body.name }, 201);
}
```

## Storage

Both stores follow the browser `Storage` interface. Values are strings; use
`JSON.stringify` and `JSON.parse` for anything else. A value is at most 1 MB.

- `scriptStorage` is shared by everyone using the script.
- `personalStorage` belongs to one signed-in user and throws when nobody is
  signed in. Check `context.request.auth.isAuthenticated` first.

```ts
const items = JSON.parse(scriptStorage.getItem("items") ?? "[]") as string[];
items.push(name);
scriptStorage.setItem("items", JSON.stringify(items));
```

Two requests may read and write the same key at the same time. For a counter or
a list that many people change, use a `database` table instead.

## Files

`files` reads and writes the script's own tree: `files.read(path)` (text, or
`null` when absent), `files.write(path, text)`, `files.list()`,
`files.delete(path)`. It cannot touch `main.*`. To ship content with the script,
prefer an import over `files.read`.

## Calling other services

`fetch(url, { method, headers, body })` returns the response **already
finished** (no `await` needed): `status`, `ok`, `text()`, `json()`, `headers`.
It speaks http and https to public hosts only.

Put a secret in the URL or a header as `{{secret:NAME}}`; the engine
substitutes it, so the script never sees the value:

```ts
const res = fetch("https://api.example.com/v1/data", {
  headers: { Authorization: "Bearer {{secret:EXAMPLE_API_KEY}}" },
});
if (!res.ok)
  return ResponseBuilder.error(502, `Upstream answered ${res.status}`);
const data = res.json();
```

Binary data travels as base64. `{ binary: true }` returns a body as
`bodyBase64`; `bodyBase64` in the options sends bytes; and
`form: [{ name, value }, { name, base64, filename, contentType }]` sends
`multipart/form-data` (leave `Content-Type` out — the engine sets it).

Never write a key, token or password into a file. A person stores secrets
separately; ask for the name and use it.

## MCP tools

A tool lets an AI client call the script. Register it in `init()`; the handler
receives its arguments in `context.args` and returns plain data, not an HTTP
response.

```ts
function init(): void {
  mcpRegistry.registerTool("convert_units", {
    description: "Convert a length between metres and feet",
    inputSchema: {
      type: "object",
      properties: {
        value: { type: "number" },
        to: { type: "string", enum: ["m", "ft"] },
      },
      required: ["value", "to"],
    },
    handler: "convertUnits",
  });
}

function convertUnits(context: HandlerContext) {
  const { value, to } = context.args as { value: number; to: "m" | "ft" };
  return { result: to === "ft" ? value * 3.28084 : value / 3.28084 };
}
```

Throw an `Error` for a failure the caller should see.

## Logging

`console.log`, `console.warn` and `console.error` write to the script's log,
which `read_logs` returns. Log the first thing you would want to know when a
handler fails.

## Safety the engine already provides

- A `POST`, `PUT`, `PATCH` or `DELETE` from a signed-in browser on another
  origin is refused before your handler runs. A form needs no CSRF token.
- `rateLimit.consume("signup", { limit: 5, windowSeconds: 600 })` spends from a
  budget the engine keys to the caller (the person, else their address). On
  `allowed: false`, answer 429.
- `audit.record("refund.issued", { orderId })` keeps an event the script cannot
  delete, attributed by the engine. Use it for what someone will later ask
  about, and `console` for debugging.

## Tests

A file named `*.test.ts` is run by `run_tests`. It runs against the latest
revision, with no server and no HTTP: call functions directly. Registrations
are switched off during a run, and database writes are rolled back.

```ts
import { totalCents } from "./lib/basket.ts";

describe("basket", () => {
  test("an empty basket totals zero", () => {
    expect(totalCents([])).toBe(0);
  });
  test("a negative price is refused", () => {
    expect(() => totalCents([{ cents: -1 }])).toThrow("negative");
  });
});
```

Matchers: `toBe`, `toEqual`, `toBeTruthy`, `toBeFalsy`, `toBeNull`,
`toBeDefined`, `toBeUndefined`, `toContain`, `toHaveLength`, `toMatch`,
`toBeGreaterThan`, `toBeLessThan`, `toThrow`, and `.not` before any of them.

A test cannot import `main.ts`, so put handlers in `lib/handlers.ts` (exported),
import them in `main.ts` and add each to the `Object.assign(globalThis, {...})`
line. A test then calls a handler with a request context it builds, and asserts
on the answer's `status` and `body`:

```ts
import { addItem } from "./handlers.ts";

test("a body with no name is a 400", () => {
  const context = { request: { json: () => ({}), query: {}, params: {} } };
  expect(addItem(context).status).toBe(400);
});
```

## Limits

One invocation has a wall-clock budget and a memory limit; `init()` has its own
budget. A request body, a stored value and a fetched response are each bounded.
The exact numbers are in `aiwebengine.d.ts`, under "Limits". Do work in small
steps, and answer with an error rather than loop.

## Not in this primer

Streams, scheduled jobs and tasks, `database` schemas, delegation, outbound MCP
clients, sandboxed code and elevation. Read their sections of
`aiwebengine.d.ts` before using one, and ask the person before changing a
database schema.
