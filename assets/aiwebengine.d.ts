/// <reference lib="es2020" />

/**
 * TypeScript type definitions for aiwebengine JavaScript API
 * @version 0.1.0
 *
 * Add this reference to your scripts for IDE autocomplete and type checking:
 * /// <reference path="https://your-engine.com/engine/types/v0.1.0/aiwebengine.d.ts" />
 *
 * IMPORTANT: Every script MUST export an init() function that registers routes,
 * MCP tools, or other initialization logic.
 *
 * @example
 * // Minimal script structure
 * function myHandler(context) {
 *   return ResponseBuilder.json({ message: "Hello" });
 * }
 *
 * function init() {
 *   routeRegistry.registerRoute("/api/hello", { handler: "myHandler", method: "GET" });
 * }
 */

// ============================================================================
// Script Initialization
// ============================================================================

/**
 * Initialization function that must be exported by every script.
 * This function is called when the script is loaded and should register
 * routes, MCP tools, or perform other setup tasks.
 *
 * @param context - Handler context (optional, may not be provided during init)
 * @example
 * function init() {
 *   // Register HTTP routes
 *   routeRegistry.registerRoute("/api/users", { handler: "listUsers", method: "GET" });
 *   routeRegistry.registerRoute("/api/users/:id", { handler: "getUser", method: "GET" });
 *
 *   // Register streams
 *   routeRegistry.registerRoute("/events/notifications", { stream: true });
 *
 *   // Log initialization
 *   console.log("Script initialized successfully");
 * }
 */
declare function init(context?: HandlerContext): void;

// ============================================================================
// When registration takes effect
// ============================================================================

/**
 * Every global in this file is present in every execution context. A script
 * sees the same API whether it was entered through an HTTP route, an MCP
 * resolver, an MCP tool, a scheduled job, a stream customizer, a message
 * listener or a test, so shared helpers never need `typeof x === "undefined"`
 * guards. What a call is *allowed* to do still depends on the caller's
 * capabilities, and what it *does* depends on the phase described here.
 *
 * A script's top-level program is re-evaluated on every invocation, and only
 * `init()` runs in the registration phase. So these methods:
 *
 * - `routeRegistry.registerRoute`
 * - `mcpRegistry.registerTool` / `registerPrompt` / `registerResource`
 * - `schedulerService.registerOnce` / `registerRecurring` / `clearAll`
 *
 * take effect during startup and `init()`, and elsewhere answer
 * `{ ok: false, reason }` saying the entry was not registered. They never throw
 * for being called at the wrong time — a script that registers at top level
 * rather than inside `init()` would otherwise fail on every request. A mistake
 * in the call — a bad path, an empty name, a malformed spec — throws in every
 * context.
 *
 * Everything else — `database`, `files`, `secretStorage`,
 * `scriptStorage`, `personalStorage`, `fetch`, `convert`, `console`,
 * `McpClient`, `routeRegistry.sendStreamMessage` — works in every context.
 *
 * Register in `init()`. Registering from a request handler silently does
 * nothing, which is rarely what the script intended.
 */

// ============================================================================
// Limits
// ============================================================================

/**
 * What a script may spend, and how large anything it handles may be.
 *
 * Every number below is written by the engine that served this file, from the
 * limits it is actually enforcing — the same source `GET /engine/openapi.json`
 * publishes as `x-aiwebengine-limits`. A copy read from the repository rather
 * than from a running engine still carries the markers themselves, which is
 * the honest answer: several of these are configuration, and a file on disk
 * cannot know what a particular deployment chose.
 *
 * **The execution model.** These are not numbers, and they are what surprises
 * people arriving from a browser or from Node:
 *
 * {{limits.notes}}
 *
 * **Budgets.** One invocation gets `javascript.execution_timeout_ms` of wall
 * clock ({{limits.execution.timeout}}), {{limits.execution.maxMemory}} of
 * memory and a {{limits.execution.stackSize}} stack — about
 * {{limits.execution.stackFrames}} frames of recursion, past which QuickJS
 * throws an error the script can see. `init()` gets its own budget
 * (`javascript.init_timeout_ms`, {{limits.execution.initTimeout}}), because it
 * runs once per deployment rather than once per request and a script whose
 * `init()` is cut off does not serve at all. A test module gets
 * {{limits.execution.testModuleTimeout}} and a whole test run
 * {{limits.execution.testRunTimeout}}.
 *
 * The budget is enforced between JavaScript operations *and* as a ceiling on
 * each host call, so a slow database statement cannot outlive it. What it
 * cannot do is interrupt a statement already running inside Postgres: that is
 * what `repository.statement_timeout_ms`
 * ({{limits.database.statementTimeout}}), `lock_timeout_ms`
 * ({{limits.database.lockTimeout}}) and `idle_in_transaction_timeout_ms`
 * ({{limits.database.idleInTransactionTimeout}}) are for, and why
 * `database.transaction(fn, { timeoutMs })` can only tighten them.
 *
 * At most `javascript.max_concurrent_executions` scripts run at once
 * ({{limits.execution.maxConcurrent}}). Past that a request waits for a slot
 * inside the timeout it already had, rather than being refused. The number is
 * sized against the connection pool rather than against the CPU: a script
 * touching the database holds a slot and a connection until it answers, so
 * slots far above the pool would only deepen the queue.
 *
 * **Sizes.** A script's root source is {{limits.size.maxScriptSource}}, one
 * asset {{limits.size.maxAsset}} with a URI of at most
 * {{limits.size.maxAssetUriChars}} characters, one `scriptStorage` /
 * `personalStorage` value {{limits.size.maxStorageValue}}, one secret
 * {{limits.size.maxSecretValue}}, a `fetch` response
 * {{limits.fetch.maxResponse}}, and `convert.markdown_to_html` /
 * `render_handlebars_template` {{limits.size.maxMarkdown}} of input each. A
 * request arriving at one of your routes is bounded by
 * `security.max_request_body_bytes` ({{limits.size.maxRequestBody}}), or by
 * `repository.max_upload_size_bytes` plus framing when it is form-encoded or
 * multipart ({{limits.size.maxUpload}}); past either, the caller gets a 413
 * and your handler is never entered.
 *
 * **The script database.** {{limits.database.maxTablesPerScript}} tables per
 * script, {{limits.database.maxColumnsPerTable}} columns per table, and
 * identifiers of at most {{limits.database.maxIdentifierChars}} characters
 * matching `^[a-z][a-z0-9_]*$`. `database.query` returns
 * {{limits.database.defaultQueryLimit}} rows when you name no limit and clamps
 * a named one to {{limits.database.maxQueryLimit}} — silently, so a query
 * wanting more has to page. The connection pool
 * ({{limits.database.maxConnections}}) is shared by every script on the
 * instance, and each call holds a connection for its whole round trip.
 *
 * **Network.** `fetch` and `McpClient` speak {{limits.fetch.schemes}} only,
 * refuse localhost and private, loopback and link-local addresses — including
 * a public host that resolves to one, at every hop — follow at most
 * {{limits.fetch.maxRedirects}} redirects within one budget, and understand
 * `{{limits.fetch.encodings}}`. Asking for `br` or `zstd` is an error rather
 * than an unreadable body. An `McpClient` naming a blocked address is refused
 * when it is constructed, before its token is looked up.
 *
 * **Scheduled jobs.** A recurring interval is at least
 * {{limits.scheduler.minRecurringInterval}} and a job name
 * 1-{{limits.scheduler.maxJobNameChars}} characters.
 *
 * **Retention.** Logs are not storage: a line survives only while it is within
 * both the newest {{limits.retention.logsKeepPerScript}} of its script and
 * {{limits.retention.logsRetentionHours}} hours of now, and either clause
 * alone removes it. Revisions are kept for
 * {{limits.retention.revisionsRetentionDays}} days *and* the newest
 * {{limits.retention.revisionsKeepPerScript}} per script — a revision has to
 * fall outside both before it goes — and a labelled revision, the newest that
 * initialised cleanly, and the newest of each script are kept regardless.
 */

// ============================================================================
// HTTP Request and Response Types
// ============================================================================

/**
 * HTTP request object passed in context
 */
interface HttpRequest {
  /** Request path (e.g., "/blog/post/123") */
  path: string;

  /** HTTP method (GET, POST, PUT, DELETE, etc.) */
  method: string;

  /**
   * Request headers.
   *
   * A {@link Headers}, so `headers.get("content-type")` finds a header the
   * client spelled `Content-Type`. It still reads as the plain object it used
   * to be — `headers["content-type"]`, `Object.keys(headers)` and spreading all
   * work — so existing code keeps working and stops depending on the
   * capitalisation the client happened to choose.
   */
  headers: Headers & Record<string, string>;

  /** URL query parameters as key-value pairs */
  query: Record<string, string>;

  /** Route parameters from path patterns (e.g., {id: "123"}) */
  params: Record<string, string>;

  /** Form data from POST requests as key-value pairs */
  form: Record<string, string>;

  /** Raw request body as string */
  body: string;

  /**
   * The absolute URL the request arrived on, origin included.
   *
   * `path` cannot say which of the engine's hosts served a request; this can.
   * Present only for HTTP routes — an MCP tool or stream
   * customization has no URL behind it.
   * @example
   * console.log(req.url); // "https://example.com/api/notes?tag=a"
   */
  url?: string;

  /**
   * The query string, parsed. Unlike {@link HttpRequest.query} — a plain object
   * built from a map — this keeps a parameter that appeared more than once.
   * @example
   * req.searchParams.getAll("tag"); // ["a", "b"] for ?tag=a&tag=b
   */
  searchParams: URLSearchParams;

  /**
   * The body, as text. Mirrors what a `fetch` response answers, so a body a
   * script receives reads the way a body it fetched does.
   */
  text(): string;

  /**
   * The body, parsed as JSON. Throws where the parse is asked for, not on the
   * way in from a request that arrived perfectly well and was not JSON.
   * @example
   * const { name } = req.json() as { name: string };
   */
  json(): unknown;

  /** Uploaded files from multipart form data */
  files: Array<{
    /** Form field name */
    field: string;
    /** Original filename (if provided) */
    filename?: string;
    /** MIME content type (if provided) */
    contentType?: string;
    /** Base64-encoded file data */
    data: string;
    /** File size in bytes */
    size: number;
  }>;

  /** Authentication context (available when user is authenticated) */
  auth?: AuthContext;
}

/**
 * Authentication context available in request.auth
 * Always present; check isAuthenticated before accessing user-specific fields.
 */
interface AuthContext {
  /** Whether the request is authenticated */
  isAuthenticated: boolean;

  /** Whether user has admin privileges */
  isAdmin: boolean;

  /** Whether user has editor privileges */
  isEditor: boolean;

  /** User ID, or null if not authenticated */
  userId: string | null;

  /** User email address, or null if not authenticated */
  userEmail: string | null;

  /** User display name, or null if not authenticated */
  userName: string | null;

  /** Authentication provider (google, microsoft, apple), or null if not authenticated */
  provider: string | null;

  /** Complete user object when authenticated, or null */
  user: {
    id: string | null;
    email: string | null;
    name: string | null;
    provider: string | null;
    isAuthenticated: boolean;
  } | null;

  /**
   * Asserts that the request is authenticated, returning the user object.
   * Throws an error if the request is not authenticated.
   * @throws Error if not authenticated
   * @example
   * const user = req.auth.requireAuth(); // throws if anonymous
   */
  requireAuth(): {
    id: string | null;
    email: string | null;
    name: string | null;
    provider: string | null;
    isAuthenticated: boolean;
  };
}

/**
 * HTTP response object returned from handlers
 */
interface HttpResponse {
  /** HTTP status code (200, 404, 500, etc.) */
  status: number;

  /** Response body as string (mutually exclusive with bodyBase64) */
  body?: string;

  /** Response body as base64-encoded string (for binary data) */
  bodyBase64?: string;

  /** Content-Type header value */
  contentType?: string;

  /** Additional response headers */
  headers?: Record<string, string>;
}

/** What sort of invocation a handler is running under */
type HandlerInvocationKind =
  | "httpRoute"
  | "streamCustomization"
  | "init"
  | "scheduled"
  | "mcpTool"
  | "mcpPrompt"
  | "test"
  | "eval";

/**
 * Context object passed to all handler functions
 */
interface HandlerContext {
  /** HTTP request information (for HTTP route handlers) */
  request?: HttpRequest;

  /** Function arguments */
  args?: Record<string, any>;

  /** Handler invocation type */
  invocationType?: HandlerInvocationKind;

  /** What kind of invocation this is */
  kind?: HandlerInvocationKind;

  /**
   * Identifies this invocation. Every log line the handler writes is filed
   * under it, so `GET /engine/script_logs?request_id=<id>` returns exactly the
   * lines this run produced. For an HTTP route it is the request's
   * `x-request-id`, which the response carries back to the caller.
   */
  invocationId?: string;

  /** Additional metadata */
  metadata?: Record<string, any>;

  /**
   * What this particular kind of invocation was given. A scheduled handler
   * finds its schedule under `meta.schedule`; a task handler finds `meta.task`.
   */
  meta?: {
    task?: TaskContext;
    [key: string]: any;
  };
}

/**
 * What a task handler is told about the run it is in — `context.meta.task`.
 */
interface TaskContext {
  /** Stable across retries, so work can be keyed by it. */
  taskId: string;
  handler: string;
  /** The object passed to `scriptTasks.enqueue`. */
  payload: Record<string, unknown>;
  /** Counts from one. A value above one means this is a retry. */
  attempt: number;
  maxAttempts: number;
}

// ============================================================================
// Route Registry API
// ============================================================================

/**
 * What a path leads to. Exactly one of `handler`, `stream` or `file`.
 */
type RouteSpec = HandlerRouteSpec | StreamRouteSpec | FileRouteSpec;

/** OpenAPI documentation every kind of route may carry. */
interface RouteDocs {
  summary?: string;
  description?: string;
  /** Swagger group. Defaults to "Streams" for a stream and "Assets" for a file. */
  tags?: string[];
}

/** A script function answering one HTTP method. */
interface HandlerRouteSpec extends RouteDocs {
  /** Name of the handler function to call. */
  handler: string;
  /**
   * HTTP method. Defaults to GET. Registering GET also serves HEAD, running
   * the same handler and returning its headers with an empty body; register
   * HEAD explicitly to answer it differently.
   */
  method?: string;
  /** OpenAPI parameters array. */
  parameters?: unknown[];
  /** OpenAPI requestBody object. */
  requestBody?: Record<string, unknown>;
  stream?: never;
  file?: never;
  /** A handler decides who may call it itself; there is nothing to name. */
  authorize?: never;
}

/** A Server-Sent Events stream the engine holds open. GET only. */
interface StreamRouteSpec extends RouteDocs {
  stream: true;
  /**
   * Name of the function that **decides who may subscribe**. Without one,
   * anyone who can reach the host gets the stream. See {@link AuthorizeFunction}.
   */
  authorize?: string;
  handler?: never;
  file?: never;
}

/** A file of the script's tree, served by the engine. GET only. */
interface FileRouteSpec extends RouteDocs {
  /** Path of the file in the script's tree. Must be under `public/`. */
  file: string;
  /**
   * Name of the function that decides who may read the file. Without one,
   * it is served to anyone who can reach the host — right for a stylesheet,
   * and why a served file lives under `public/`. See {@link AuthorizeFunction}.
   */
  authorize?: string;
  handler?: never;
  stream?: never;
}

/**
 * The function a stream's or a file route's `authorize` names.
 *
 * Whose order `/orders/1234/events` is, is a fact about your data model rather
 * than one the engine can know, so this is where that decision lives. It runs
 * under the requester's own context and answers either an allow — any object
 * without `deny`; for a stream, a flat object of strings that
 * `sendStreamMessageFiltered` matches against — or a refusal:
 *
 * ```ts
 * function mayWatchOrder(context) {
 *   if (!context.auth?.userId) return { deny: 401 };
 *   const orderId = context.request.params.id;
 *   if (!ownsOrder(context.auth.userId, orderId)) {
 *     return { deny: 403, reason: "not your order" };
 *   }
 *   return { orderId };
 * }
 * ```
 *
 * `deny` is `true` for a plain refusal or any 4xx status — `401` "sign in",
 * `403` "not yours", `404` "and I will not tell you it exists". `reason` is
 * returned to whoever was refused and trimmed to 200 characters. Throwing is
 * **not** how you deny: it means the function itself failed, answers 500, and
 * a file it guards is not served.
 */
type AuthorizeFunction = (
  context: HandlerContext,
) => Record<string, string> | { deny: true | number; reason?: string };

/** What `registerRoute` answers. A refusal is a value, not an exception. */
type RegisterRouteResult = { ok: true } | { ok: false; reason: string };

/** What sending to a stream answers. */
interface StreamSendResult {
  /** Connections the message reached. */
  delivered: number;
  /** Connections open on the path when it was sent. */
  connections: number;
  /** Connections that could not be written to. */
  failed: number;
}

/**
 * Route registry for HTTP endpoints, streams and served files
 */
interface RouteRegistry {
  /**
   * Publish a path. The spec says what it leads to:
   *
   * ```ts
   * routeRegistry.registerRoute("/api/users", { handler: "listUsers" });
   * routeRegistry.registerRoute("/api/users/:id", {
   *   handler: "updateUser",
   *   method: "PUT",
   *   summary: "Update user",
   *   tags: ["Users"],
   *   parameters: [{ name: "id", in: "path", required: true, schema: { type: "string" } }],
   *   requestBody: { required: true, content: { "application/json": { schema: { type: "object" } } } },
   * });
   * routeRegistry.registerRoute("/orders/:id/events", { stream: true, authorize: "mayWatchOrder" });
   * routeRegistry.registerRoute("/styles/main.css", { file: "public/main.css" });
   * ```
   *
   * `:param` and a trailing `/*` work for all three. A mistake in the call —
   * a spec naming no target or two, an unknown key, a reserved path, a
   * capability you do not hold — throws. A **refusal** is returned:
   * `{ ok: false, reason }` for a file outside `public/` or not in the tree,
   * and for any call made outside startup and `init()`, where there is
   * nothing to register into.
   */
  registerRoute(path: string, spec: RouteSpec): RegisterRouteResult;

  /**
   * Send a message to every connection on a stream.
   * @param data - Any JSON-serializable value
   * @example
   * routeRegistry.sendStreamMessage("/events/notifications", {
   *   type: "alert",
   *   message: "New update available",
   * });
   */
  sendStreamMessage(path: string, data: any): StreamSendResult;

  /**
   * Send a message to the connections whose authorize function returned
   * criteria matching `filter`.
   * @param matchMode - "subset" (default): every filter entry must match.
   *   "overlap": any one may.
   * @example
   * routeRegistry.sendStreamMessageFiltered(
   *   "/events/notifications",
   *   { message: "Admin alert" },
   *   { role: "admin" },
   * );
   */
  sendStreamMessageFiltered(
    path: string,
    data: any,
    filter?: Record<string, string>,
    matchMode?: "subset" | "overlap",
  ): StreamSendResult;
}

// ============================================================================
// Asset Storage API
// ============================================================================

/** One file of a script's tree, as `files.list()` describes it. */
interface FileInfo {
  /** Path in the tree, e.g. `"skills/refund.md"` or `"public/logo.svg"`. */
  path: string;
  /** Size in bytes. */
  size: number;
  mimetype: string;
  /** Milliseconds since the epoch. */
  createdAt: number;
  /** Milliseconds since the epoch. */
  updatedAt: number;
}

/** How a file's content is given or wanted. */
interface FileEncodingOptions {
  /** `"utf8"` (default) for text, `"base64"` for binary. */
  encoding?: "utf8" | "base64";
}

/**
 * The script's own tree, by path.
 *
 * Every script reaches only its own files. The entrypoint (`main.*`) is not
 * writable here: `engine.call("write_file", ...)` is the deliberate way to
 * change a script's program, and it takes what writing one takes.
 *
 * For content that only changes with a redeploy, prefer an import —
 * `import policy from "./skills/refund.md"` — which is resolved once, cached
 * with the program and pinned by the revision. A read is for content the
 * script writes, or that changes under it.
 */
interface Files {
  /**
   * Every file of the script, sorted by path.
   * @example
   * const skills = files.list().filter((f) => f.path.startsWith("skills/"));
   */
  list(): FileInfo[];

  /**
   * A file's content: text by default, base64 with `{ encoding: "base64" }`.
   * Answers `null` when there is no such file. Throws a `TypeError` for a
   * file that is not text unless base64 was asked for.
   * @example
   * const policy = files.read("skills/refund.md") ?? "";
   */
  read(path: string, options?: FileEncodingOptions): string | null;

  /**
   * Create or replace a file. `content` is text, or base64 with
   * `{ encoding: "base64" }`. The MIME type is inferred from the extension
   * unless given. At most {{limits.size.maxAssetUriChars}} characters of path and 10,000,000 bytes of
   * content; a path may not contain `..`. A write is recorded as a revision
   * of the script.
   * @example
   * files.write("notes/today.md", "# Today");
   * files.write("public/logo.png", pngBase64, { encoding: "base64" });
   */
  write(
    path: string,
    content: string,
    options?: FileEncodingOptions & { mimetype?: string },
  ): void;

  /**
   * Remove a file. Answers `false` when there was nothing to remove.
   */
  delete(path: string): boolean;
}

// ============================================================================
// Storage APIs
// ============================================================================

/**
 * The WHATWG Web Storage interface, as browsers expose it on `localStorage`
 * and `sessionStorage`.
 *
 * Two stores implement it. `scriptStorage` belongs to the script and is shared
 * by everyone using it, across every instance in a cluster. `personalStorage`
 * belongs to one authenticated user within one script; reaching it with nobody
 * logged in throws a `SecurityError`.
 *
 * Keys and values are coerced with `String()`, so `setItem("count", 1)` stores
 * `"1"`. Failures throw a `DOMException` rather than being returned:
 * `QuotaExceededError` when a value exceeds {{limits.size.maxStorageValue}}, `SecurityError` when the
 * store is not available to the caller.
 *
 * Named access — `store.foo`, `"foo" in store`, `delete store.foo`,
 * `Object.keys(store)` — works, but each of those is a database round trip, so
 * enumerating a large store costs one query per key.
 *
 * @example
 * scriptStorage.setItem("pageViews", "42");
 * const views = scriptStorage.getItem("pageViews") ?? "0";
 *
 * try {
 *   personalStorage.setItem("theme", "dark");
 * } catch (e) {
 *   // e.name === "SecurityError" when nobody is logged in
 * }
 */
interface Storage {
  /** How many keys the store holds. */
  readonly length: number;

  /**
   * The value stored under `key`, or `null` if there is none.
   * @example
   * const counter = scriptStorage.getItem("pageViews") ?? "0";
   */
  getItem(key: string): string | null;

  /**
   * Store `value` under `key`, replacing any previous value.
   * @throws DOMException `QuotaExceededError` if the value exceeds {{limits.size.maxStorageValue}},
   * `SecurityError` if the store is not available to the caller.
   * @example
   * scriptStorage.setItem("pageViews", "42");
   */
  setItem(key: string, value: string): void;

  /**
   * Remove `key`. Removing one that is not there is not an error.
   * @throws DOMException `SecurityError` if the store is not available.
   * @example
   * scriptStorage.removeItem("oldData");
   */
  removeItem(key: string): void;

  /**
   * Remove every key in the store.
   * @throws DOMException `SecurityError` if the store is not available.
   * @example
   * scriptStorage.clear();
   */
  clear(): void;

  /**
   * The nth key in ascending order, or `null` if the index is out of range.
   * @example
   * for (let i = 0; i < scriptStorage.length; i++) {
   *   console.log(scriptStorage.key(i));
   * }
   */
  key(index: number): string | null;

  /** Named access to a stored value. */
  [name: string]: any;
}

/**
 * The WHATWG `Headers` interface. Header names are case-insensitive, and a
 * header that arrived more than once reads as its values joined with ", ".
 *
 * @example
 * const headers = new Headers({ "Content-Type": "application/json" });
 * headers.get("content-type"); // "application/json"
 */
declare class Headers {
  constructor(init?: Headers | Record<string, string> | [string, string][]);
  /** The value under `name`, or `null` if there is none. */
  get(name: string): string | null;
  /** Whether `name` is present. */
  has(name: string): boolean;
  /** Set `name`, replacing any previous value. */
  set(name: string, value: string): void;
  /** Add `value` under `name`, joining any existing value with ", ". */
  append(name: string, value: string): void;
  /** Remove `name`. */
  delete(name: string): void;
  forEach(
    callback: (value: string, name: string, parent: Headers) => void,
    thisArg?: any,
  ): void;
  keys(): IterableIterator<string>;
  values(): IterableIterator<string>;
  entries(): IterableIterator<[string, string]>;
  [Symbol.iterator](): IterableIterator<[string, string]>;
}

/**
 * The WHATWG `URLSearchParams` interface. A name may appear more than once,
 * which is what {@link URLSearchParams.getAll} is for.
 *
 * @example
 * const params = new URLSearchParams("tag=a&tag=b");
 * params.getAll("tag"); // ["a", "b"]
 * params.get("tag");    // "a"
 */
declare class URLSearchParams {
  constructor(
    init?:
      | string
      | URLSearchParams
      | Record<string, string>
      | [string, string][],
  );
  /** How many name/value pairs there are, repeats counted separately. */
  readonly size: number;
  /** The first value under `name`, or `null`. */
  get(name: string): string | null;
  /** Every value under `name`, in the order they appeared. */
  getAll(name: string): string[];
  /** Whether `name` is present. */
  has(name: string): boolean;
  /** Add another `name`/`value` pair. */
  append(name: string, value: string): void;
  /** Replace the first `name` and drop the rest. */
  set(name: string, value: string): void;
  /** Remove every pair under `name`. */
  delete(name: string): void;
  /** Sort by name, keeping repeated values in the order they arrived. */
  sort(): void;
  forEach(
    callback: (value: string, name: string, parent: URLSearchParams) => void,
    thisArg?: any,
  ): void;
  keys(): IterableIterator<string>;
  values(): IterableIterator<string>;
  entries(): IterableIterator<[string, string]>;
  [Symbol.iterator](): IterableIterator<[string, string]>;
  /** The pairs, re-encoded as a query string. */
  toString(): string;
}

/**
 * The error thrown by the storage APIs when a call cannot be completed.
 * `name` identifies the failure — `QuotaExceededError`, `SecurityError`,
 * `SyntaxError`, `UnknownError` — and it is a real `Error`, so an existing
 * `catch` sees it either way.
 */
declare class DOMException extends Error {
  constructor(message?: string, name?: string);
  readonly name: string;
  readonly message: string;
}

// ============================================================================
// Secret Storage API
// ============================================================================

/**
 * Secret storage API for managing per-user secrets scoped to the current script.
 *
 * Write operations require an authenticated user.
 * The `exists` check looks in `user_secrets` first (when authenticated), then falls back to `script_secrets`.
 *
 * Nothing here reads a secret back. What a stored secret is for is `fetch`,
 * which replaces a `{{secret:NAME}}` in a header value with it — so the value
 * reaches the API it was stored for without passing through the script.
 */
interface SecretStorage {
  /**
   * Whether a secret is stored under `key`: the signed-in person's first,
   * then the script's.
   * @example
   * if (!secretStorage.exists("API_TOKEN")) return askForToken();
   */
  exists(key: string): boolean;

  /**
   * Store a secret for the signed-in person, in this script. Throws when
   * nobody is signed in, when the value is over {{limits.size.maxSecretValue}},
   * and in background work acting for somebody — storing a key is something
   * the person does in their own session.
   * @example
   * secretStorage.setSecret("API_TOKEN", token);
   */
  setSecret(key: string, value: string): void;

  /**
   * Remove one of the signed-in person's secrets. Answers `false` when there
   * was none; throws when the caller may not manage secrets.
   */
  removeSecret(key: string): boolean;

  /** Remove every secret the signed-in person stored in this script. */
  clear(): void;
}

// ============================================================================
// Scheduler Service API
// ============================================================================

/**
 * Scheduler service for managing scheduled tasks.
 *
 * A scheduled handler gets {{limits.execution.jobTimeout}} rather than the
 * {{limits.execution.timeout}} a request handler gets: a job is not answering
 * a request, so it is not held to a budget chosen to keep pages responsive.
 * Everything else is the same — the same memory ceiling, and `fetch` still
 * shortened to whatever is left of that budget.
 *
 * A one-off that throws is retried, waiting longer each time, and dropped
 * after a few failures with a line in the script's log saying so. A recurring
 * job is not retried: it runs again at its next interval.
 */
/** What a scheduler registration answers. */
type ScheduledJobResult =
  | { ok: true; jobId?: string; name?: string; nextRun?: string }
  | { ok: false; reason: string };

interface SchedulerService {
  /**
   * Run `handler` once, at `runAt` (a UTC ISO timestamp).
   * @param options.name - The job's key (1-{{limits.scheduler.maxJobNameChars}} characters); registering
   *   the same name again replaces it. Defaults to the handler's name.
   * @example
   * schedulerService.registerOnce({
   *   handler: "sendReminder",
   *   runAt: new Date(Date.now() + 3600000).toISOString(),
   *   name: "reminder-job",
   * });
   */
  registerOnce(options: {
    handler: string;
    runAt: string;
    name?: string;
  }): ScheduledJobResult;

  /**
   * Run `handler` every `intervalMilliseconds` (at least 100) or
   * `intervalMinutes` (at least 1) — one of the two.
   * @param options.startAt - Optional UTC ISO timestamp for the first run
   * @example
   * schedulerService.registerRecurring({
   *   handler: "cleanupOldData",
   *   intervalMinutes: 5,
   *   name: "cleanup-job",
   * });
   */
  registerRecurring(options: {
    handler: string;
    intervalMilliseconds?: number;
    intervalMinutes?: number;
    name?: string;
    startAt?: string;
  }): ScheduledJobResult;

  /** Remove every job this script registered. */
  clearAll(): { ok: true; cleared: number } | { ok: false; reason: string };
}

// ============================================================================
// Script Tasks API
// ============================================================================

/**
 * One queued task.
 */
interface ScriptTask {
  /** Stable across retries. */
  taskId: string;
  script: string;
  handler: string;
  payload: Record<string, unknown>;
  /**
   * `pending` waiting to run, `running` claimed by a worker, `failed` out of
   * attempts, `cancelled` stopped by a person. There is no `succeeded`: a task
   * that completes has its row deleted, so `get` answers `null` for one.
   */
  state: "pending" | "running" | "failed" | "cancelled";
  attempts: number;
  maxAttempts: number;
  /** Why the last attempt failed. Null until one has. */
  lastError: string | null;
  runAt: string;
  enqueuedBy: string | null;
  /** Who it acts as. Null for script context, which is the default. */
  runAs?: string | null;
  /**
   * What it will not run beside. Null for no lane. A personal task with no
   * lane named gets `person:<id>`.
   */
  lane?: string | null;
  kind?: "task" | "message";
  createdAt: string;
  updatedAt: string;
}

/**
 * A durable queue of work this script enqueued for itself.
 *
 * The shape this is for is: start something while handling a request, answer
 * the request, and let the work finish afterwards. Unlike `schedulerService`,
 * which registers a *schedule* and only takes effect during `init()`, these
 * may be called from anywhere — a route handler, another task — because a task
 * exists in response to something that happened rather than being part of what
 * the script declares about itself.
 *
 * A task survives deployment. Writing a new version of the script does not
 * discard work already accepted, where a scheduled job is wiped and
 * re-declared on every `init()`.
 *
 * A handler runs with {{limits.execution.jobTimeout}} — the scheduled-handler
 * budget, not the per-request one — and finds `context.meta.task` carrying the
 * payload, the attempt number, and the ceiling. It runs in **script context**:
 * it holds what the script holds, and nothing belonging to whoever enqueued
 * it, so `personalStorage` and a per-user secret are not reachable from one.
 *
 * A task that throws is retried with a widening delay and given up on after
 * `maxAttempts`, keeping its row and its last error so the failure can be read
 * back. A task that succeeds has no row: what it did is in the script's log.
 *
 * @example
 * // In a route handler: accept the work, answer now.
 * function startExport(context) {
 *   const task = scriptTasks.enqueue({
 *     handler: "runExport",
 *     payload: { accountId: context.request.query.account },
 *   });
 *   return { status: 202, body: JSON.stringify({ task: task.taskId }) };
 * }
 *
 * // The handler, named by the enqueue above.
 * function runExport(context) {
 *   const { accountId } = context.meta.task.payload;
 *   // ... and enqueue the next step, which is how work longer than one
 *   // budget is done: a chain of tasks survives a restart, one long run does not.
 * }
 */
interface ScriptTasks {
  /**
   * Accept a piece of work.
   *
   * @param options.handler - Name of the function to call. 1-{{limits.scheduler.maxJobNameChars}} characters.
   * @param options.payload - A JSON object describing the work. At most 64KB;
   *   put the data itself in storage and name it here, so a retry reads what
   *   is current rather than a copy taken at enqueue time.
   * @param options.runAt - UTC ISO timestamp to hold it until. Default: now.
   * @param options.maxAttempts - Attempts before giving up, 1-25. Default 5.
   * @returns The stored task.
   * @throws TypeError if the handler or payload is not usable, RangeError if a
   *   bound is exceeded, Error if the engine could not store it.
   */
  enqueue(options: {
    handler: string;
    payload?: Record<string, unknown>;
    runAt?: string;
    maxAttempts?: number;
    /**
     * What this must not run beside. At most one task per lane runs at a
     * time; the rest stay pending until it finishes.
     *
     * No lane by default, which is how every script task behaved before
     * lanes existed: claimed and run alongside anything else. There is
     * nothing for the engine to infer one from here — a script task belongs
     * to the solution rather than to a person — so name one when the work
     * shares state. `personalTasks` defaults its own.
     */
    lane?: string;
  }): ScriptTask;

  /**
   * Stop a task that has not started.
   *
   * @returns true if it was cancelled; false if it was not pending — already
   *   running, already failed, or already cancelled.
   */
  cancel(taskId: string): boolean;

  /**
   * Look one up.
   *
   * @returns The task, or null if this script has no such task — which is also
   *   the answer for one that already succeeded, since success keeps no row.
   */
  get(taskId: string): ScriptTask | null;
}

/**
 * What this person has authorised this script to do as them.
 */
/**
 * What a person can authorise a script to do as them while they are away.
 *
 * Two nouns and a verb. The nouns say whose things are in scope;
 * `"write"` says the run may change them rather than only read them — without
 * it a delegated run holds no write capability at all.
 */
type DelegationScope = "personal_storage" | "secrets" | "write";

interface DelegationState {
  /** False when nobody is signed in; nothing can be delegated then. */
  authenticated: boolean;
  /** True only if a grant exists and has not lapsed. */
  granted: boolean;
  expired?: boolean;
  expiresAt?: string;
  scopes?: DelegationScope[];
  /** Where to send them to authorise it. Absent when nobody is signed in. */
  consentUrl?: string;
}

/**
 * The same queue as `scriptTasks`, with the work acting as the person who
 * asked for it.
 *
 * `scriptTasks` runs in script context: it holds what the script holds, so
 * `personalStorage` throws and a `{{secret:...}}` resolves the script's key
 * rather than anybody's. That is right for work that belongs to the solution
 * and useless for work that belongs to a person — which is why an agent could
 * previously only act while its owner's tab was open.
 *
 * Acting as somebody while they are away is a grant they have to make. They
 * make it on a consent page (`authorization().consentUrl`), it names what it
 * covers, it expires, and they can withdraw it from their account page —
 * which also cancels whatever it had queued.
 *
 * Every one of those is checked again when the task runs, not just when it is
 * enqueued: a grant withdrawn in between takes effect rather than being
 * carried forward. A task whose grant has gone is abandoned with a reason,
 * without spending its retries.
 *
 * What a delegated task gets is what an ordinary request of that person's
 * gets, and never more — `personalStorage` and their own secrets, but not the
 * ability to author or administer anything, however much the person holds.
 *
 * @example
 * function scheduleDigest(context) {
 *   const auth = personalTasks.authorization();
 *   if (!auth.granted) {
 *     // Not an error — they simply have not been asked yet.
 *     return { status: 302, headers: { Location: auth.consentUrl } };
 *   }
 *   personalTasks.enqueue({ handler: "sendDigest", payload: {} });
 *   return { status: 202, body: "Scheduled" };
 * }
 *
 * function sendDigest(context) {
 *   // context.request.auth.userId is the person who authorised this.
 *   const last = personalStorage.getItem("lastDigest");
 *   fetch("https://api.example.com/send", {
 *     method: "POST",
 *     headers: { Authorization: "Bearer {{secret:their_api_key}}" },
 *   });
 * }
 */
interface PersonalTasks {
  /**
   * Queue work to run as the person making this request.
   *
   * @throws SecurityError if nobody is signed in, or if they have not
   *   authorised this script, or if that authorisation has expired. Check
   *   `authorization()` first to offer the consent page instead.
   */
  enqueue(options: {
    handler: string;
    payload?: Record<string, unknown>;
    runAt?: string;
    maxAttempts?: number;
    /**
     * What this must not run beside.
     *
     * **Defaults to the person.** Two prompts from one person otherwise
     * become two runs interleaving turn for turn, each reading and
     * overwriting the same `personalStorage` — which is a bug in essentially
     * every solution that queues per-person work, so the queue holds it
     * rather than leaving each caller to.
     *
     * Name your own for a finer lane (one conversation rather than one
     * person), or pass `null` to opt out and let this person's tasks run in
     * parallel.
     */
    lane?: string | null;
  }): ScriptTask;

  /** What this person has authorised, and where to send them if nothing. */
  authorization(): DelegationState;

  /**
   * Queue work to run as the person a message came **from**.
   *
   * The only way an execution with nobody signed in can act as anybody, and
   * so the way an agent reaches the places people already are: an inbound
   * Telegram or Slack webhook has no session, so `enqueue` above has nobody
   * to act as.
   *
   * A script never names a person here. It names a sender as the channel
   * reports them, and the engine resolves that through the links people have
   * consented to on `/auth/delegate`. An unlinked sender resolves to nobody
   * and nothing runs, so a script that trusts the wrong field in a request
   * body can be made to claim the wrong *sender* — not to name a different
   * account, and not to enumerate one.
   *
   * **Verify the message before calling this.** Telegram and Slack sign
   * their webhooks; email largely does not. Checking that signature is the
   * script's job, and the engine cannot do it for you — a handler that takes
   * the sender straight from an unauthenticated body is a way for anyone who
   * can reach the route to spend that person's budget.
   *
   * @throws SecurityError if nobody has linked that sender, or the person
   *   has not authorised this script, or that authorisation has expired.
   *   Check `sender()` first to reply with the link instead.
   * @throws RangeError if this sender has queued too much too quickly.
   *
   * @example
   * // A Telegram webhook, after checking the secret token Telegram sends.
   * const from = String(update.message.from.id);
   * const who = personalTasks.sender({ channel: "telegram", identity: from });
   * if (!who.granted) {
   *   return reply(`Authorise me first: ${who.linkUrl}`);
   * }
   * personalTasks.enqueueFrom({
   *   channel: "telegram",
   *   identity: from,
   *   handler: "runTurn",
   *   payload: { text: update.message.text },
   * });
   */
  enqueueFrom(options: {
    /** Where the message came from: a short slug, lower-cased by the engine. */
    channel: string;
    /** The sender as that channel names them. Compared exactly. */
    identity: string;
    handler: string;
    payload?: Record<string, unknown>;
    runAt?: string;
    maxAttempts?: number;
    /**
     * As `enqueue` above: defaults to the person, which is what stops two
     * messages arriving a second apart from becoming two interleaved turns.
     */
    lane?: string | null;
  }): ScriptTask;

  /**
   * Whether a sender is linked and still authorised. A read.
   *
   * Deliberately never answers with the account's id: everything a script
   * can do for that person goes through `enqueueFrom`, which names the
   * sender, and an id here would end up in whatever the bot logs or echoes
   * back into the chat.
   */
  sender(options: { channel: string; identity: string }): SenderState;

  /**
   * Mint the one URL that links this sender to whoever opens it, and
   * **reply with it into that sender's own chat**.
   *
   * The URL carries a single-use token rather than the sender's name, and
   * that is the security of the whole scheme. A link that named the sender
   * would be one anybody could construct for anybody — and linking somebody
   * else's id before they do does not merely squat on it, it *intercepts*
   * them: every message they send the bot would be processed as the
   * squatter's turn, with their text landing in the squatter's storage.
   *
   * Being able to read the sender's messages is the only evidence of
   * ownership the engine can have, so posting this link anywhere that sender
   * cannot read gives exactly that away.
   *
   * Minting invalidates any link still outstanding for the same sender, so
   * call it where a bot decides to send one — not in a loop that polls
   * `sender()`.
   *
   * @throws TypeError if the sender is not usable, RangeError if this sender
   *   has asked too often, SecurityError in a turn that may not write.
   */
  inviteLink(options: { channel: string; identity: string }): LinkInvitation;

  cancel(taskId: string): boolean;
  get(taskId: string): ScriptTask | null;
}

/** What `personalTasks.sender()` answers. */
interface SenderState {
  /** Whether anybody has linked this sender to this script. */
  linked: boolean;
  /** True only if it is linked *and* that person's grant is live. */
  granted: boolean;
  expired?: boolean;
  scopes?: DelegationScope[];
  /** The pair as the engine normalised it — the channel is lower-cased. */
  channel: string;
  identity: string;
}

/** What `personalTasks.inviteLink()` answers. */
interface LinkInvitation {
  /** Send this into the sender's own chat, and nowhere else. */
  linkUrl: string;
  channel: string;
  identity: string;
  /** How long it stays usable. */
  expiresInMinutes: number;
}

// ============================================================================
// MCP (Model Context Protocol) Registry API
// ============================================================================

/** What an MCP registration answers. */
type McpRegistrationResult = { ok: true } | { ok: false; reason: string };

/**
 * MCP Registry: what this script exposes to MCP clients — tools to call,
 * prompts to fill in, resources to read.
 */
interface McpRegistry {
  /**
   * Register an MCP tool.
   * @param name - 1-100 characters
   * @example
   * mcpRegistry.registerTool("calculateSum", {
   *   description: "Calculates the sum of two numbers",
   *   inputSchema: {
   *     type: "object",
   *     properties: { a: { type: "number" }, b: { type: "number" } },
   *     required: ["a", "b"],
   *   },
   *   handler: "handleCalculateSum",
   * });
   */
  registerTool(
    name: string,
    spec: {
      /** 1-1000 characters. */
      description: string;
      /** A JSON Schema object. Defaults to an object with no properties. */
      inputSchema?: Record<string, unknown>;
      /** Name of the function that handles a call. */
      handler: string;
    },
  ): McpRegistrationResult;

  /**
   * Register an MCP prompt.
   * @example
   * mcpRegistry.registerPrompt("generateCode", {
   *   description: "Generates code based on requirements",
   *   arguments: [
   *     { name: "language", description: "Programming language", required: true },
   *     { name: "task", description: "Task description", required: true },
   *   ],
   *   handler: "handleGenerateCode",
   * });
   */
  registerPrompt(
    name: string,
    spec: {
      description: string;
      arguments?: Array<{
        name: string;
        description?: string;
        required?: boolean;
      }>;
      handler: string;
    },
  ): McpRegistrationResult;

  /**
   * Publish one of this script's files as an MCP resource: the read half of
   * MCP, content a client fetches by URI. It is file-backed rather than
   * handler-backed on purpose — a resource answering from a handler would be
   * a tool with a different spelling — and the file is read when a client
   * asks, so rewriting it needs no redeploy. The file must be under
   * `resources/`, which is what says it is readable over MCP; one elsewhere
   * is refused. Content that is not valid UTF-8 is served as base64 `blob`.
   *
   * @param uri - How clients name it: a scheme (`docs://handbook`), 3-500
   *   characters, no whitespace.
   * @example
   * mcpRegistry.registerResource("docs://handbook", {
   *   file: "resources/handbook.md",
   *   name: "Handbook",
   *   description: "How the team works",
   *   mimeType: "text/markdown",
   * });
   */
  registerResource(
    uri: string,
    spec: {
      /** Path of the file in the script's tree, under `resources/`. */
      file: string;
      /** Shown in a client's resource picker. Defaults to the file's name. */
      name?: string;
      description?: string;
      /** Overrides the file's own MIME type. */
      mimeType?: string;
    },
  ): McpRegistrationResult;
}

// ============================================================================
// MCP Client API
// ============================================================================

/**
 * MCP tool information
 */
interface McpTool {
  /** Tool name */
  name: string;

  /** Tool description */
  description: string;

  /** JSON schema defining the tool's input parameters */
  inputSchema: any;
}

/**
 * MCP Client for connecting to external MCP servers and using their tools.
 *
 * The MCP Client implements the Model Context Protocol to connect to external
 * MCP servers (like GitHub Copilot MCP) and use their tools. Authentication
 * is handled via secrets stored in the environment.
 *
 * Every call here — the constructor included — requires **both**
 * `use_network` and `read_secrets`. A call to an MCP server is unconditionally
 * both: an outbound request to the URL you name, carrying the secret you name
 * as a `Bearer` token. There is no unauthenticated arm, which is why
 * `read_secrets` is required up front rather than only when a secret is
 * mentioned, as it is for `fetch`.
 *
 * So a `sandbox.run` narrowed out of either cannot mount an MCP server, and
 * that is deliberate: a credential resolved host-side by name would otherwise
 * be a way to hold, through a secret, authority the narrowing had just
 * refused.
 *
 * IMPORTANT: The McpClient uses a low-level API with static methods. For easier
 * usage, wrap it in a class as shown in scripts/examples/github_mcp_issues.js
 *
 * @example
 * // Low-level usage (not recommended for typical scripts)
 * const clientDataJson = McpClient.constructor(
 *   "https://api.githubcopilot.com/mcp/",
 *   "GITHUB_TOKEN"
 * );
 * const clientData = JSON.parse(clientDataJson);
 *
 * const toolsJson = McpClient._listTools(JSON.stringify(clientData));
 * const tools = JSON.parse(toolsJson);
 *
 * const resultJson = McpClient._callTool(
 *   JSON.stringify(clientData),
 *   "list_issues",
 *   JSON.stringify({owner: "example", repo: "project"})
 * );
 * const result = JSON.parse(resultJson);
 *
 * @example
 * // Recommended: Use a wrapper class (see scripts/examples/github_mcp_issues.js)
 * class GitHubMcpClient {
 *   constructor(serverUrl, secretIdentifier) {
 *     const clientDataJson = McpClient.constructor(serverUrl, secretIdentifier);
 *     this._clientData = JSON.parse(clientDataJson);
 *   }
 *
 *   listTools() {
 *     const toolsJson = McpClient._listTools(JSON.stringify(this._clientData));
 *     return JSON.parse(toolsJson);
 *   }
 *
 *   callTool(toolName, args) {
 *     const resultJson = McpClient._callTool(
 *       JSON.stringify(this._clientData),
 *       toolName,
 *       JSON.stringify(args)
 *     );
 *     return JSON.parse(resultJson);
 *   }
 * }
 *
 * const client = new GitHubMcpClient(
 *   "https://api.githubcopilot.com/mcp/",
 *   "GITHUB_TOKEN"
 * );
 * const tools = client.listTools();
 */
interface McpClientConstructor {
  /**
   * Create MCP client connection data (constructor function).
   * Returns a JSON string with server URL and secret identifier.
   *
   * @param serverUrl - MCP server URL (must be https://)
   * @param secretIdentifier - Name of the secret containing the authentication token
   * @returns JSON string with client data: {serverUrl: string, secretIdentifier: string}
   * @throws Error if serverUrl is invalid or secret doesn't exist
   * @example
   * const clientDataJson = McpClient.constructor(
   *   "https://api.githubcopilot.com/mcp/",
   *   "GITHUB_TOKEN"
   * );
   * const clientData = JSON.parse(clientDataJson);
   */
  constructor(serverUrl: string, secretIdentifier: string): string;

  /**
   * List all tools available from the MCP server (static method).
   * Results are cached for 1 hour to reduce network calls.
   *
   * @param clientDataJson - JSON string with client data from constructor
   * @returns JSON string with tool list: {tools: McpTool[]} or error: {error: string, details?: string}
   * @throws Error if authentication fails or network error occurs
   * @example
   * const toolsJson = McpClient._listTools(clientDataJson);
   * const response = JSON.parse(toolsJson);
   *
   * if (response.error) {
   *   console.error(`Failed to list tools: ${response.error}`);
   *   return;
   * }
   *
   * response.tools.forEach(tool => {
   *   console.log(`Tool: ${tool.name} - ${tool.description}`);
   * });
   */
  _listTools(clientDataJson: string): string;

  /**
   * Call a tool on the MCP server (static method).
   *
   * @param clientDataJson - JSON string with client data from constructor
   * @param toolName - Name of the tool to call
   * @param argsJson - JSON string with tool arguments
   * @returns JSON string with tool result or error object
   * @throws Error if authentication fails or network error occurs
   * @example
   * const resultJson = McpClient._callTool(
   *   clientDataJson,
   *   "search_repositories",
   *   JSON.stringify({query: "aiwebengine", limit: 10})
   * );
   *
   * const response = JSON.parse(resultJson);
   *
   * if (response.error) {
   *   console.error(`Tool error: ${response.error}`);
   *   return;
   * }
   *
   * console.log(`Tool result: ${JSON.stringify(response)}`);
   */
  _callTool(clientDataJson: string, toolName: string, argsJson: string): string;
}

declare var McpClient: McpClientConstructor;

// ============================================================================
// HTTP Fetch API
// ============================================================================

/**
 * Fetch options
 */
interface FetchOptions {
  /** HTTP method (default: GET) */
  method?: string;

  /**
   * Request headers.
   *
   * A `{{secret:NAME}}` anywhere in a value is replaced by that secret before
   * the request is sent: `"Authorization": "Bearer {{secret:api_token}}"`.
   */
  headers?: Record<string, string>;

  /** Request body */
  body?: string;

  /**
   * Timeout in milliseconds (default: 30000).
   *
   * Shortened to whatever is left of the handler's execution budget, so a
   * request cannot outlive the script that made it. Redirects are followed
   * within the same budget rather than each getting the full timeout.
   */
  timeout?: number;

  /**
   * Ask for the body as base64 in `bodyBase64` rather than as text in `body`.
   *
   * A response body is a string, and one that is not valid UTF-8 is an error
   * rather than a lossy decode — right for the JSON and HTML nearly every call
   * fetches, and the reason an image could not be retrieved at all. Set this
   * when you know the bytes are not text: a photo from a chat platform, a PDF,
   * an audio file.
   *
   * Exactly one of `body` and `bodyBase64` carries the answer. With `binary`,
   * `body` is the empty string and `text()` and `json()` have nothing to read.
   *
   * @example
   * const photo = fetch(fileUrl, { binary: true });
   * const block = {
   *   type: "image",
   *   source: { type: "base64", media_type: "image/jpeg", data: photo.bodyBase64 },
   * };
   */
  binary?: boolean;
}

/**
 * Fetch response.
 *
 * Usable three ways, so browser habits work without breaking the scripts
 * written against the JSON string `fetch` used to return:
 *
 * - `await fetch(url)` — it is thenable
 * - `fetch(url).status` — the fields are really there
 * - `JSON.parse(fetch(url))` — `toString` yields the original envelope
 */
interface FetchResponse {
  /** HTTP status code */
  status: number;

  /** Whether the status was a 2xx */
  ok: boolean;

  /**
   * Response body as string.
   *
   * Empty when the request asked for `{ binary: true }` — the bytes are in
   * `bodyBase64` instead.
   */
  body: string;

  /**
   * Response body as base64, present only when the request asked for
   * `{ binary: true }`.
   *
   * Never populated alongside a non-empty `body`, so there is never a question
   * of which one to read.
   */
  bodyBase64?: string;

  /** Response headers */
  headers: Record<string, string>;

  /** The body, as text. */
  text(): string;

  /** The body, parsed as JSON. Throws if the body is not JSON. */
  json(): unknown;

  /**
   * The raw JSON envelope — status, ok, headers and body — which is what
   * `fetch()` itself used to return. `JSON.parse(fetch(url))` still works
   * because `JSON.parse` converts its argument with ToString first.
   */
  toString(): string;
}

/**
 * HTTP client with secret injection support.
 *
 * The request is already finished by the time this returns: host calls block
 * rather than yielding. `await` here sequences, it does not parallelise, so
 * `Promise.all` over several fetches gives the right answers and runs them one
 * after another.
 *
 * Limits: {{limits.fetch.schemes}} only; localhost and private, loopback and link-local
 * addresses are refused, including a public host that resolves to one and
 * every hop of a redirect chain; at most {{limits.fetch.maxRedirects}} redirects and
 * {{limits.fetch.maxResponse}} of response;
 * and the {{limits.fetch.defaultTimeout}} default timeout is shortened to whatever is left of the
 * handler's execution budget, so a request cannot outlive the script that made
 * it.
 *
 * Responses are decompressed for you: the request offers `gzip, deflate` and
 * anything that comes back under one of those is inflated before you see it,
 * so `body` is text either way and `content-encoding` and `content-length` are
 * absent from `headers` when it was. Set your own `Accept-Encoding` if an API
 * demands one — it is sent as written, and the answer is still decoded. A
 * coding the engine cannot undo (`br`, `zstd`) is an error rather than an
 * unreadable body, so do not ask for those.
 *
 * Secrets are injected into header values, and only into header values. A
 * `{{secret:NAME}}` anywhere inside one is replaced before the request leaves,
 * so a bearer token is written as the prefix and the template together. The
 * name is looked up for the requesting user first (`secretStorage`), then for
 * the script, and a name nothing resolves fails the call rather than being
 * sent as itself — a request carrying template text where a credential should
 * be comes back as a 401 that explains nothing.
 *
 * A URL is substituted as well, under one extra rule: a `{{secret:NAME}}` may
 * fill in the **path, query or fragment**, and may not change the scheme, the
 * host or the port. `https://api.example.com/bot{{secret:TOKEN}}/send`
 * resolves; `https://{{secret:WHERE}}/send` is refused rather than resolved,
 * and so is any value whose substitution moves the origin.
 *
 * That rule is the whole of why this is safe to have. The template already
 * carries the origin, so every check about where a request may go runs
 * against a string with no credential in it, and everything that writes the
 * URL down — the audit line, the debug log, an error handed back to a script
 * — writes the template. A redirect target and a transport error are derived
 * from the resolved form, so both are scrubbed before they are returned.
 *
 * What it cannot cover is the far end's own access log, which sees the
 * resolved URL. That is the API's choice rather than this engine's, and it is
 * the reason to prefer a header wherever an API offers one. Substituting into
 * a path was refused outright until an API that offers no header form had to
 * be reachable from a script — the Telegram Bot API is the case — so treat it
 * as the exception it was added for rather than as the pattern.
 *
 * @param url - URL to fetch
 * @param options - Fetch options
 * @returns The response, readable directly or via `await`
 * @example
 * // Browser-shaped
 * const response = await fetch("https://api.example.com/data");
 * const data = await response.json();
 *
 * // Without awaiting — the same object
 * const response = fetch("https://api.example.com/data");
 * if (response.ok) { const data = response.json(); }
 *
 * // POST with secret injection
 * const response = await fetch("https://api.example.com/endpoint", {
 *   method: "POST",
 *   headers: {
 *     "Authorization": "Bearer {{secret:API_TOKEN}}",
 *     "Content-Type": "application/json"
 *   },
 *   body: JSON.stringify({ key: "value" })
 * });
 *
 * // A token the API insists on having in the path. The scheme and host are
 * // written out, so they are what every check and every log line sees.
 * const sent = fetch(
 *   "https://api.telegram.org/bot{{secret:TELEGRAM_BOT_TOKEN}}/sendMessage",
 *   { method: "POST", headers: { "Content-Type": "application/json" },
 *     body: JSON.stringify({ chat_id: id, text: text }) },
 * );
 */
declare function fetch(
  url: string,
  options?: FetchOptions,
): FetchResponse & PromiseLike<FetchResponse>;

/** One request of a `fetchAll` batch. A bare string is a GET of that URL. */
type ParallelFetchRequest = string | { url: string; options?: FetchOptions };

/**
 * Several requests at once.
 *
 * `Promise.all([fetch(a), fetch(b)])` gives the right answers and runs them
 * one after another — each `fetch` has already finished by the time it
 * returns, so the wall clock is the sum. That is fine for two quick calls and
 * is the difference between fitting inside the execution budget and not for
 * an agent running three tool calls.
 *
 * These run together: one thread waits on the batch rather than one per
 * request in series, and the wall clock becomes the slowest rather than the
 * sum. Each request gets the same validation, redirects and secret
 * substitution a single `fetch` gets.
 *
 * Answers are **positional** — the nth answer belongs to the nth request,
 * whatever order they arrived in.
 *
 * A failure is per request. A refused URL answers with `ok: false` and throws
 * from *that* response when you read its body, so the answers that arrived
 * are still usable. Check `ok` first, or let the throw happen where you touch
 * the one that failed.
 *
 * More requests than the engine runs at once are run in waves rather than
 * refused.
 *
 * @example
 * const [weather, news, mail] = fetchAll([
 *   "https://api.example.com/weather",
 *   { url: "https://api.example.com/news", options: { method: "POST", body } },
 *   "https://api.example.com/mail",
 * ]);
 * if (weather.ok) { use(weather.json()); }
 */
declare function fetchAll(requests: ParallelFetchRequest[]): FetchResponse[];

/** What a `fetchStream` read answers with. */
interface StreamChunk {
  done: boolean;
  /** Absent once `done`. */
  value?: string;
}

/**
 * A response read a piece at a time.
 *
 * Iterable, so the ordinary shape is a `for...of`. The connection stays open
 * between reads and closes when the stream ends, when `close()` is called, or
 * when the execution does.
 */
interface FetchStream {
  status: number;
  ok: boolean;
  headers: Record<string, string>;
  /** The next piece, blocking until one arrives. */
  read(): StreamChunk;
  /** Everything left, joined — for a caller that wanted the headers early. */
  text(): string;
  /** Give the socket back before the end. */
  close(): boolean;
  [Symbol.iterator](): Iterator<string>;
}

/**
 * Begin a request and read its body as it arrives.
 *
 * `fetch` reads the whole body before it returns anything, so a script cannot
 * consume a model's token stream: an agent's page updates once per turn, and
 * a turn is as long as the whole model call. This hands back the status and
 * headers as soon as they arrive and the body in pieces as they do — which,
 * bridged to `routeRegistry.sendStreamMessage`, turns a silent minute into
 * visible progress.
 *
 * Three things to know:
 *
 * - **A chunk is not a line and not an SSE event.** Boundaries fall wherever
 *   the network put them. Reassembling whatever you are reading is the
 *   caller's job, because only the caller knows what it is.
 * - **No content coding is requested.** `fetch` offers gzip and undoes it
 *   after reading the whole body, which a stream cannot do. Endpoints that
 *   stream are not compressed in practice.
 * - **Close what you stop reading.** An abandoned stream holds a socket until
 *   the execution ends, and an execution may hold only a few at once.
 *
 * Multi-byte characters split across chunks are reassembled for you.
 *
 * @example
 * const stream = fetchStream("https://api.example.com/v1/messages", {
 *   method: "POST",
 *   headers: { "Authorization": "Bearer {{secret:API_TOKEN}}" },
 *   body: JSON.stringify({ stream: true, messages }),
 * });
 *
 * let whole = "";
 * for (const chunk of stream) {
 *   whole += chunk;
 *   routeRegistry.sendStreamMessage("answer", chunk);
 * }
 */
declare function fetchStream(url: string, options?: FetchOptions): FetchStream;

// ============================================================================
// Database API (Script-Scoped Table Management)
// ============================================================================

/** A column `ensureTable` makes sure a table has. */
interface ColumnSpec {
  /** `^[a-z][a-z0-9_]*$`, at most 63 characters. */
  name: string;
  /**
   * `integer` holds whole numbers up to about 2.1 billion and refuses a
   * fraction rather than rounding it; `bigint` is for epoch milliseconds and
   * anything else past that; `float` keeps fractions; `reference` is an
   * integer pointing at another of the script's tables, with a foreign key.
   */
  type:
    | "integer"
    | "bigint"
    | "float"
    | "text"
    | "boolean"
    | "timestamp"
    | "reference";
  /** Defaults to true: a column added to a table with rows cannot be NOT NULL without a default. */
  nullable?: boolean;
  /** SQL default, as text. */
  default?: string;
  /** For a `reference`: the table it points at. */
  references?: string;
}

/** The shape `ensureTable` brings a table to. */
interface TableSchema {
  columns: ColumnSpec[];
  /** Each entry is the column list of one unique index — what `upsert` needs as its key. */
  uniqueIndexes?: string[][];
}

/** What `ensureTable` changed. */
interface EnsuredTable {
  created: boolean;
  columnsAdded: string[];
  uniqueIndexesEnsured: string[][];
}

/** A row as stored: its columns, plus the `id` every table has. */
type DatabaseRow = { id: number } & Record<string, any>;

/**
 * Which rows: `{ col: value }` for equality, or
 * `{ col: { $gt, $gte, $lt, $lte, $ne } }` for a comparison. Several
 * conditions are AND-ed.
 */
type WhereClause = Record<string, any>;

interface QueryOptions {
  where?: WhereClause;
  /** Default {{limits.database.defaultQueryLimit}}, at most {{limits.database.maxQueryLimit}}. */
  limit?: number;
  orderBy?: string;
  /** `"asc"` (default) or `"desc"`; anything else is refused rather than sorted ascending. */
  order?: "asc" | "desc";
  /**
   * Hold the returned rows until the surrounding transaction ends, so a
   * read-modify-write cannot lose an update. Refused outside a transaction,
   * where the lock would be released as soon as the query returned.
   */
  forUpdate?: boolean;
}

/**
 * The script's own tables.
 *
 * Every table belongs to the script that made it, named within it, so two
 * scripts may both have a `notes` table. A script may have 50 tables of 50
 * columns. Every call answers with a value and throws when it fails —
 * including when the caller lacks the capability it takes.
 */
interface Database {
  /**
   * Bring a table to the shape you describe, whatever shape it is in now:
   * created if missing, missing columns added, unique indexes made. Calling
   * it on a table that is already right changes nothing and costs one query,
   * so it belongs at the top of `init()` rather than behind a "has this run"
   * flag. Concurrent callers take turns rather than racing.
   * @example
   * database.ensureTable("notes", {
   *   columns: [
   *     { name: "owner", type: "text" },
   *     { name: "body", type: "text" },
   *     { name: "created_at", type: "bigint" },
   *   ],
   *   uniqueIndexes: [["owner", "created_at"]],
   * });
   */
  ensureTable(name: string, schema: TableSchema): EnsuredTable;

  dropTable(name: string): { tableName: string; dropped: boolean };

  dropColumn(
    name: string,
    column: string,
  ): { tableName: string; columnName: string; dropped: boolean };

  /**
   * Rows of a table.
   * @example
   * const recent = database.query("chat", { orderBy: "ts", order: "desc", limit: 100 });
   * const active = database.query("presence", {
   *   where: { last_active: { $gt: Date.now() - 90000 } },
   * });
   */
  query(name: string, options?: QueryOptions): DatabaseRow[];

  /** Insert a row; answers it as stored, with its `id`. */
  insert(name: string, row: Record<string, any>): DatabaseRow;

  /** Change a row by id; answers it as stored. */
  update(name: string, id: number, changes: Record<string, any>): DatabaseRow;

  delete(name: string, id: number): { deleted: boolean };

  /**
   * Insert, or update the row with the same key. `keyColumns` must be a
   * unique index — `ensureTable`'s `uniqueIndexes`.
   * @example
   * database.upsert("presence", ["user_id"], { user_id: id, last_active: Date.now() });
   */
  upsert(
    name: string,
    keyColumns: string | string[],
    row: Record<string, any>,
  ): DatabaseRow;

  /** Delete every row matching `where`, which may not be empty. */
  deleteWhere(name: string, where: WhereClause): { deleted: number };

  /**
   * Run `fn` in a transaction: committed when it returns, rolled back when it
   * throws, and answering what it returned. Inside another transaction it is
   * a savepoint, so an inner failure undoes only the inner work. An async
   * `fn` is awaited before committing.
   *
   * `timeoutMs` is enforced by the database: within the transaction no
   * statement runs longer, no lock is waited on longer, and an abandoned
   * transaction is ended after that much idleness. It only tightens the
   * engine's own limits.
   * @example
   * // A counter that stays correct under concurrency
   * database.transaction(() => {
   *   const [row] = database.query("event_seq", { limit: 1, forUpdate: true });
   *   database.update("event_seq", row.id, { seq: row.seq + 1 });
   * }, { timeoutMs: 5000 });
   */
  transaction<T>(fn: () => T, options?: { timeoutMs?: number }): T;
}

// ============================================================================
// Console API
// ============================================================================

/**
 * Console logging interface
 * Note: reading and pruning stored log entries is engine administration, not a
 * script API — use `GET|DELETE /engine/script_logs` or the equivalent MCP tools.
 *
 * A log is a diagnostic rather than a store: a line survives only while it is
 * within both the newest 10,000 of this script and 168 hours of now, and either
 * clause alone removes it. Anything that has to last belongs in `database` or
 * `scriptStorage`.
 */
interface Console {
  /**
   * Write a log message.
   *
   * Variadic and stringifying, as the browser's is: arguments are joined with
   * spaces, and anything that is not a string is rendered — objects and arrays
   * are inspected, an `Error` carries its stack. If the first argument is a
   * string containing format specifiers and more arguments follow, they are
   * substituted: `%s`, `%d`/`%i`, `%f`, `%o`/`%O`, `%j`, `%c` (consumed, styles
   * nothing) and `%%`.
   *
   * Inspection is capped — depth 4, 100 entries per level, 8192 characters per
   * line — because a log line is a database row rather than a devtools entry.
   * @param data - Values to log
   * @example
   * console.log("Request received:", req.path);
   * console.log(user);
   * console.log("%s took %dms", label, elapsed);
   */
  log(...data: any[]): void;

  /**
   * Write an info log message. Formats its arguments like {@link Console.log}.
   * @example
   * console.info("User logged in:", userId);
   */
  info(...data: any[]): void;

  /**
   * Write a warning log message. Formats its arguments like {@link Console.log}.
   * @example
   * console.warn("Deprecated API usage detected:", apiName);
   */
  warn(...data: any[]): void;

  /**
   * Write an error log message. Formats its arguments like {@link Console.log},
   * so an `Error` may be passed directly and logs with its stack.
   * @example
   * try { risky(); } catch (e) { console.error("failed:", e); }
   */
  error(...data: any[]): void;

  /**
   * Write a debug log message. Formats its arguments like {@link Console.log}.
   * @example
   * console.debug("Processing item:", item.id);
   */
  debug(...data: any[]): void;

  /**
   * Write a single value, always inspected rather than printed as a string.
   * @param item - Value to inspect
   * @example
   * console.dir(response.headers);
   */
  dir(item?: any): void;

  /**
   * Write a message followed by the current stack, at DEBUG level.
   * @example
   * console.trace("reached the fallback branch");
   */
  trace(...data: any[]): void;

  /**
   * Write "Assertion failed" at ERROR level when `condition` is falsy.
   * Does nothing when it holds.
   * @example
   * console.assert(rows.length > 0, "expected at least one row for", tableName);
   */
  assert(condition?: boolean, ...data: any[]): void;

  /**
   * Write tabular data as a bordered table.
   * @param tabularData - Array or object whose values become rows
   * @param columns - Restrict the output to these column names
   * @example
   * console.table(rows, ["id", "email"]);
   */
  table(tabularData?: any, columns?: string[]): void;

  /**
   * Write an optional label and indent everything logged until the matching
   * {@link Console.groupEnd} by two spaces.
   * @example
   * console.group("import");
   * console.log("42 rows");
   * console.groupEnd();
   */
  group(...data: any[]): void;

  /**
   * Alias of {@link Console.group}. Nothing here can collapse, so the two
   * behave identically.
   */
  groupCollapsed(...data: any[]): void;

  /** Close the innermost {@link Console.group}. */
  groupEnd(): void;

  /**
   * Start a timer. Warns if one is already running under this label.
   * @param label - Timer name (default: "default")
   * @example
   * console.time("query");
   */
  time(label?: string): void;

  /**
   * Write a running timer's elapsed time without stopping it.
   * @example
   * console.timeLog("query", "after the first page");
   */
  timeLog(label?: string, ...data: any[]): void;

  /**
   * Write a timer's elapsed time and stop it.
   * @example
   * console.timeEnd("query");
   */
  timeEnd(label?: string): void;

  /**
   * Write the number of times `count` has been called with this label.
   * @param label - Counter name (default: "default")
   * @example
   * console.count("cache-miss");
   */
  count(label?: string): void;

  /** Reset a counter created by {@link Console.count}. */
  countReset(label?: string): void;

  /**
   * Does nothing. Present so the call is not a `ReferenceError`, but stored log
   * lines are pruned through the engine's administration surface
   * (`DELETE /engine/script_logs`), which is not reachable from a script.
   */
  clear(): void;
}

// ============================================================================
// Conversion Functions API
// ============================================================================

/**
 * Conversion utilities for data transformation
 */
interface Convert {
  /**
   * Convert markdown string to HTML
   * @param markdown - Markdown content to convert (1 byte to {{limits.size.maxMarkdown}})
   * @returns HTML string
   * @example
   * const html = convert.markdown_to_html("# Hello\n\nThis is **bold**");
   */
  markdown_to_html(markdown: string): string;

  /**
   * Render a Handlebars template with data
   * @param template - Handlebars template string (1 byte to {{limits.size.maxTemplate}})
   * @param dataJson - JSON string with template data
   * @returns Rendered template string
   * @example
   * const output = convert.render_handlebars_template(
   *   "Hello {{name}}!",
   *   JSON.stringify({ name: "World" })
   * );
   */
  render_handlebars_template(template: string, dataJson: string): string;

  /**
   * Base64 encode a string
   * @param data - String to encode
   * @returns Base64-encoded string
   * @example
   * const encoded = convert.btoa("Hello World");
   */
  btoa(data: string): string;

  /**
   * Base64 decode a string
   * @param data - Base64-encoded string to decode
   * @returns Decoded string
   * @example
   * const decoded = convert.atob(encoded);
   */
  atob(data: string): string;
}

// ============================================================================
// Global Objects
/**
 * A capability, named the way `sandbox` names it.
 *
 * The list is closed: an unrecognised name is refused rather than ignored,
 * because silently dropping one would hand code a context the script believed
 * it had checked.
 */
type Capability =
  | "read_scripts"
  | "write_scripts"
  | "delete_scripts"
  | "read_assets"
  | "write_assets"
  | "delete_assets"
  | "delete_logs"
  | "view_logs"
  | "manage_streams"
  | "manage_mcp"
  | "read_script_data"
  | "write_script_data"
  | "manage_script_database"
  | "administer_engine"
  | "use_network"
  | "read_secrets"
  | "write_secrets"
  | "read_storage"
  | "write_storage"
  | "enqueue_tasks"
  | "send_messages";

/** One line the sub-execution wrote through `console`. */
interface SandboxConsoleLine {
  level: string;
  message: string;
  timestampMs: number;
}

/** What one narrowed sub-execution produced. */
interface SandboxResult {
  /**
   * The value the source evaluated to, run through `JSON.stringify` and
   * re-parsed. Absent when it has no JSON form — `undefined`, a function, a
   * symbol — which `valueType` tells apart from a value that really was null.
   */
  value?: unknown;
  /** `undefined`, `null`, `boolean`, `number`, `string`, `symbol`, `function`, `array`, `object`. */
  valueType?: string;
  /**
   * What it printed, whether or not it succeeded. Usually the most useful
   * part of a turn that failed.
   */
  console: SandboxConsoleLine[];
  /** Lines dropped after the capture limit, so a truncated capture is not mistaken for the whole. */
  consoleDropped: number;
  /** False when the source threw or ran out of budget. */
  ok: boolean;
  /**
   * Why it failed. Returned rather than thrown: code the model wrote that did
   * not work is the *result* of the turn, not a failure of the loop that ran
   * it, and an agent's next move is to feed this back rather than to unwind.
   */
  error?: string;
  durationMs: number;
  rolledBack: boolean;
}

interface SandboxRunOptions {
  /**
   * What the sub-execution may do. Must be a subset of what the calling turn
   * holds — asking for more throws rather than quietly narrowing, since a
   * script asking to keep something it never had has a bug.
   *
   * Omitted means *none*, not "everything I hold". A typo in the option name
   * must not hand model-authored code the whole of the caller's authority.
   */
  capabilities?: Capability[];
  /**
   * Which hosts the sub-execution may reach.
   *
   * The destination half of a narrowing, and the one a capability set cannot
   * express: `use_network` is a verb, so "may call the network" means "may
   * call anything". That gap is why **exfiltration needs no write
   * capability** — a planning turn holding only reads can still put what it
   * read into a URL. This takes the destination away rather than the data,
   * which is the one defence that holds regardless of what untrusted text
   * talked the model into.
   *
   * Each entry is a host: `api.example.com`, or `*.example.com` for its
   * subdomains. A wildcard does **not** match the bare parent — the CSP and
   * CORS rule — so name both when both are wanted. Ports are not part of it.
   *
   * Checked on the request's own host **and on every redirect hop**: an
   * allowlist checked once is one open redirector away from being none.
   * `McpClient` is bounded by it too, since an MCP server is a destination
   * like any other.
   *
   * Omitted means *wherever the calling turn could already go*, which is the
   * opposite default to `capabilities` and deliberately so: this only ever
   * subtracts from a set that is already the caller's, and defaulting it to
   * empty would silently take the network away from every existing call.
   * Pass `[]` for "nowhere". Must be covered by the caller's own scope —
   * asking to reach further throws.
   *
   * @example
   * // The model may call one API and cannot post what it read anywhere else.
   * sandbox.run(modelWroteThis, {
   *   capabilities: ["use_network"],
   *   hosts: ["api.anthropic.com"],
   * });
   */
  hosts?: string[];
  /** Handed to the source as `context.args`. Must be JSON-serializable. */
  input?: unknown;
  /** Budget in milliseconds, clamped to the engine's own ceiling. */
  timeoutMs?: number;
  /**
   * Roll the sub-execution's database writes back. Off by default, unlike
   * `/engine/eval`: a turn that may not write holds no write capability,
   * which is stronger than a transaction that undoes what it did.
   */
  rollback?: boolean;
}

/**
 * Running part of this script with fewer capabilities than it holds.
 *
 * A `UserContext` otherwise only ever flows downward from the caller, so the
 * smallest thing a script could run something with was everything it had.
 * This is the way down: a second execution of this same script, in a context
 * holding a chosen subset, with JSON the only thing that crosses between them.
 *
 * Two shapes it is for:
 *
 * **Code as the action** — the model writes JavaScript instead of emitting a
 * tool call. Composition comes free and the tool list stops growing.
 *
 * **A plan mode that is enforced** — a planning turn runs holding the reads
 * and none of the writes. The checks are underneath the JavaScript, so no
 * arrangement of the transcript reaches past them.
 *
 * It costs a fresh runtime and one re-evaluation of the script's program per
 * call: right for an agent turn, wrong for a loop.
 *
 * @example
 * // A plan turn: everything readable, nothing writable.
 * const plan = sandbox.run(modelWroteThis, {
 *   capabilities: ["read_script_data", "read_storage", "read_assets"],
 *   input: { question },
 * });
 * if (plan.error) {
 *   // Feed the failure back to the model rather than throwing.
 * }
 *
 * @example
 * // Narrowing from whatever this turn happens to hold, rather than from a
 * // hard-coded list that drifts as the vocabulary grows.
 * const readOnly = sandbox.held().filter(c => c.startsWith("read_"));
 * const out = sandbox.run(source, { capabilities: readOnly });
 */
interface Sandbox {
  /**
   * Evaluate `source` in this script's sandbox holding only what `options`
   * names.
   *
   * The source runs after the script's own program and in the same realm as
   * it, so it sees what the script defined — its helpers, and the bindings
   * its entrypoint imported. It sees nothing of the calling turn: the two are
   * separate contexts, and a value passed by reference would be a capability
   * passed by reference.
   *
   * @throws TypeError if a capability name is not one, RangeError if
   *   sub-executions are already nested as deep as they go, and a
   *   SecurityError if the caller does not hold what it asked to keep or
   *   cannot itself reach a host it named. What the *source* did comes back
   *   in the result instead.
   */
  run(source: string, options?: SandboxRunOptions): SandboxResult;

  /** Every capability name the engine has. */
  capabilities(): Capability[];

  /** What the calling turn holds, sorted. The set a narrowing subtracts from. */
  held(): Capability[];

  /**
   * Where the calling turn may go, or `null` for anywhere.
   *
   * `null` rather than an empty array, because an empty array is a real
   * answer — a turn that may reach nothing — and conflating the two would
   * make `sandbox.hosts() || []` quietly open the network back up.
   */
  hosts(): string[] | null;
}

// ============================================================================

declare var routeRegistry: RouteRegistry;
declare var files: Files;
// ============================================================================
// MCP elicitation — asking the caller a question mid-tool
// ============================================================================

/**
 * What the person did with the question, and what they said.
 *
 * Three actions, and the difference matters: `accept` carries `content`,
 * `decline` is an explicit refusal, `cancel` is a dismissal. Treating anything
 * but `accept` as a failure reports an error for somebody who closed a dialog.
 */
interface ElicitAnswer {
  action: "accept" | "decline" | "cancel";
  content?: Record<string, unknown>;
}

interface AskOptions {
  /** Why the information is needed. Shown to the person. Required. */
  message: string;
  /**
   * A flat JSON Schema object of primitive properties — string, number,
   * integer, boolean, or an enum of those. Nested objects and arrays of
   * objects are deliberately not supported by the protocol.
   */
  schema?: Record<string, unknown>;
  /**
   * Names this question across passes. Defaults to the call's position, which
   * is stable as long as the code before it is; name it when a handler asks
   * different questions down different branches.
   */
  key?: string;
}

/**
 * Asking the caller a question in the middle of a tool call.
 *
 * `ask` is not a pause. A tool call that asks **ends**: the engine answers the
 * client with `input_required`, the client asks the person, and then calls the
 * tool again. On that second call the handler runs from the top once more, and
 * `ask` returns the answer instead of ending anything.
 *
 * So everything before an `ask` runs on every pass. Put reads and validation
 * before it, writes after it, and wrap anything that must happen exactly once
 * in `once`.
 */
interface Mcp {
  /**
   * Whether this caller can be asked at all.
   *
   * False when the client declared no elicitation capability, and false
   * wherever there is no client — a scheduled job, a delegated task, a
   * listener. A tool that can manage without the answer should check this and
   * fall back; one that cannot should just ask and let the throw stand.
   */
  canAsk(): boolean;

  /**
   * Ask for structured input, returning what was given or ending the turn
   * asking for it.
   *
   * Throws if this caller cannot be asked. Never use it for passwords, API
   * keys, tokens or payment details — the protocol forbids collecting those
   * this way.
   *
   * @example
   * const answer = mcp.ask({
   *   message: "Which branch?",
   *   schema: {
   *     type: "object",
   *     properties: { branch: { type: "string" } },
   *     required: ["branch"],
   *   },
   * });
   * if (answer.action === "accept") {
   *   deploy(answer.content.branch);
   * }
   */
  ask(options: AskOptions): ElicitAnswer;

  /**
   * Run something exactly once across the whole exchange, however many times
   * the handler re-runs.
   *
   * The result travels to the client and back on every round trip, so it must
   * be JSON and it must be small: a decision or an identifier, not a document.
   *
   * @example
   * const draftId = mcp.once("draft", () => createDraft(repo));
   */
  once<T>(key: string, compute: () => T): T;

  /**
   * Whether this call may hand its work to a task
   * (`io.modelcontextprotocol/tasks`).
   *
   * Two conditions: there is a request to answer at all — false in a scheduled
   * job, a listener or a route handler — and the client declared the tasks
   * extension. A client that did not declare it has to be answered
   * synchronously, because a handle it cannot read is a tool call answered with
   * a small object full of fields it never asked for.
   *
   * Ask this before `mcp.task`, and write the synchronous path after it.
   */
  canTask(): boolean;

  /**
   * Hand this call's work to the durable queue, and answer the client with a
   * task handle it can poll.
   *
   * **This does not return.** Like `mcp.ask`, it ends the execution: the work
   * is queued, the handle is recorded, and the client is answered with
   * `resultType: "task"`. So the line after it is the synchronous fallback,
   * which is why `canTask()` is worth asking first.
   *
   * The engine never decides this for you. By the time it could measure that a
   * handler is slow, the handler has started and the answer is a value rather
   * than a handle — so the script says so, from inside the handler that knows.
   *
   * `handler` names a top-level function in this script, called later with the
   * payload on `context.meta.task.payload`. **What that handler returns is the
   * task's result**, and is what `tasks/get` answers with once the status is
   * `completed`; it is wrapped in the same envelope a synchronous call would
   * have produced, so a client never has to branch on whether its work was
   * queued. A handler that throws puts the task in `failed`.
   *
   * The work runs in *script* context, not as the caller — running it as them
   * would take a delegation grant, and an MCP client holding a token is not the
   * same as a person having consented to background work in their name. It gets
   * one attempt: a retried tool call is a second run of work the client was
   * told had started, with no way to tell it the first attempt failed.
   *
   * @example
   * function buildReport(context) {
   *   if (mcp.canTask()) {
   *     mcp.task({
   *       handler: "runReport",
   *       payload: context.args,
   *       statusMessage: "gathering",
   *     });
   *   }
   *   return runReportInline(context.args); // the client cannot poll
   * }
   *
   * function runReport(context) {
   *   const report = gather(context.meta.task.payload);
   *   return { content: [{ type: "text", text: report }] };
   * }
   *
   * @throws if this call cannot hand off, if the work cannot be queued, or if
   *   the execution does not hold `enqueue_tasks`.
   */
  task(options: {
    /** A top-level function in this script that does the work. */
    handler: string;
    /** Handed to it as `context.meta.task.payload`. Must be an object. */
    payload?: Record<string, unknown>;
    /** Progress text the client sees on every poll. */
    statusMessage?: string;
    /** What this must not run beside — see `scriptTasks.enqueue`. */
    lane?: string;
    /** The tool or prompt this is for, recorded on the handle. */
    target?: string;
  }): never;
}

declare var scriptStorage: Storage;
declare var personalStorage: Storage;
declare var secretStorage: SecretStorage;
declare var schedulerService: SchedulerService;
declare var scriptTasks: ScriptTasks;
declare var personalTasks: PersonalTasks;
declare var mcpRegistry: McpRegistry;
declare var mcp: Mcp;
/**
 * The cryptography a solution should not be writing for itself.
 *
 * The engine asks scripts to verify their own webhook signatures — it cannot
 * know whether a delivery is Telegram's scheme or Slack's — but the three
 * things that takes are all things an engine can get right once: an HMAC, a
 * comparison that does not leak where two strings differ, and a source of
 * randomness.
 *
 * The secret is always **named**, never passed. `secretStorage` has no read
 * from JavaScript, deliberately, and these keep that property: the value is
 * resolved host-side at the point of use, exactly as `fetch` resolves
 * `{{secret:NAME}}` in a header, so a script cannot log, forward or leak a key
 * it is never given.
 *
 * @example A Slack request signature
 * ```ts
 * const base = `v0:${timestamp}:${rawBody}`;
 * const ok = crypto.hmacVerify({
 *   secretName: "SLACK_SIGNING_SECRET",
 *   message: base,
 *   signature: signatureHeader.replace("v0=", ""),
 * });
 * ```
 *
 * @example A Telegram webhook's shared secret
 * ```ts
 * // The header Telegram echoes back, compared without `===`.
 * if (!crypto.secretEquals("TELEGRAM_WEBHOOK_SECRET", headerValue)) {
 *   return { status: 401, body: "no" };
 * }
 * ```
 */
interface Crypto {
  /**
   * A random version 4 UUID. The same as the web platform's
   * `crypto.randomUUID()`.
   */
  randomUUID(): string;

  /**
   * A fresh random token — for a webhook secret, a one-time link, anything
   * that has to be unguessable.
   *
   * Refused below 16 bytes and above 64 rather than clamped: a token shorter
   * than that is guessable, and a caller who asked for 8 and silently got 16
   * would go on believing it had asked for something it did not get.
   *
   * @param bytes - Bytes of entropy, 16 to 64. Defaults to 32.
   * @param encoding - `"hex"` (the default) or `"base64"`.
   */
  randomToken(bytes?: number, encoding?: "hex" | "base64"): string;

  /**
   * Compare two strings without revealing where they differ.
   *
   * For a value you already hold. When the value is a secret, prefer
   * {@link Crypto.secretEquals}, which never brings it into JavaScript at all.
   *
   * Length is not secret — it is visible in anything that carries the value —
   * so a length mismatch answers immediately. What stays constant is the time
   * taken over two strings of equal length.
   */
  constantTimeEqual(a: string, b: string): boolean;

  /**
   * Whether `candidate` equals the secret stored under `secretName`, compared
   * in constant time.
   *
   * The shape a shared-secret webhook header wants: Telegram's
   * `X-Telegram-Bot-Api-Secret-Token` and anything else that echoes a value
   * back for you to recognise.
   *
   * Throws when this script has no secret of that name. A missing key is a
   * deployment that was never finished, and answering `false` would make it
   * indistinguishable from an endpoint under attack.
   *
   * Requires the `read_secrets` capability.
   */
  secretEquals(secretName: string, candidate: string): boolean;

  /**
   * Whether `signature` is the HMAC of `message` under the secret stored as
   * `secretName`.
   *
   * Everything that is not a match answers `false` — a signature over
   * different bytes, one of the wrong length, one that is not valid hex at
   * all — because from the script's side they are one event: something arrived
   * that this key did not sign.
   *
   * Requires the `read_secrets` capability.
   */
  hmacVerify(options: HmacVerifyOptions): boolean;
}

interface HmacVerifyOptions {
  /**
   * The **name** of the secret to verify under, not its value. The value never
   * enters JavaScript.
   */
  secretName: string;

  /** The exact bytes that were signed. */
  message: string;

  /**
   * The signature as it arrived, without any scheme prefix — strip `sha256=`
   * or `v0=` yourself, since only the sender's documentation says what it is.
   */
  signature: string;

  /**
   * Defaults to `"sha256"`. `"sha1"` is here because webhooks still send it
   * (GitHub's original `X-Hub-Signature`) and a verifier that cannot speak it
   * cannot check those deliveries — it is not a choice to make for something
   * new.
   */
  algorithm?: "sha256" | "sha512" | "sha1";

  /** How the signature is written. Defaults to `"hex"`. */
  encoding?: "hex" | "base64";
}

/**
 * The engine's own management tools, called from a script.
 *
 * The same tools `/mcp` publishes — `list_users`, `write_file`, `read_logs`,
 * `list_revisions` and the rest — dispatched through the same function, with
 * the same authorization. **Every call is checked against whoever is calling**,
 * so holding this global grants nothing: a script reaches exactly what the
 * person driving it could reach over HTTP.
 *
 * **It is not always there.** `engine` is `undefined` outside the two
 * executions whose authority came from a credential — a script serving a
 * request, and a delegated task where somebody consented to `author` or
 * `administer`. A scheduled job, `init()`, a test run, `/engine/eval` and
 * `sandbox.run` do not get it, because those run as the
 * engine's own synthetic administrator or with a capability subset that cannot
 * express the difference between reading your own `console` and reading every
 * script's logs. Check for it before using it.
 *
 * @example Answering "who are the users"
 * ```ts
 * if (!engine) throw new Error("no management tools in this execution");
 * const { users, count } = engine.call("list_users", {});
 * ```
 *
 * @example Discovering what this engine serves
 * ```ts
 * const { tools } = engine.call ? engine.tools("revision") : { tools: [] };
 * // → list_revisions, diff_revisions, label_revision, revert_script
 * ```
 */
interface EngineApi {
  /**
   * The tools this engine serves, optionally narrowed to those whose name or
   * description mentions `area`.
   *
   * Names and descriptions only — the input schemas are large, and
   * `/engine/openapi.json` publishes them. Read this rather than hard-coding a
   * list: what a deployment serves is a property of the deployment.
   */
  tools(area?: string): {
    tools: { name: string; description: string }[];
    count: number;
  };

  /**
   * Call a tool and return its result.
   *
   * Throws an `Error` named `EngineError` when the tool refuses — which is
   * what a missing capability, a script you do not own, or a bad argument all
   * look like. Throws a plain `Error` when no tool has that name.
   */
  call(name: string, args?: Record<string, unknown>): any;

  /**
   * The same call, returning the tool's envelope instead of throwing.
   *
   * A refusal arrives as `{ error: string }`, which is the shape `/mcp` hands
   * its clients. For a caller that would rather branch than catch.
   */
  callRaw(name: string, args?: Record<string, unknown>): any;
}

declare var database: Database;
declare var console: Console;
declare var sandbox: Sandbox;
declare var convert: Convert;
declare var crypto: Crypto;
declare var engine: EngineApi | undefined;

// ============================================================================
// Testing
// ============================================================================

/**
 * Assertions available on the value passed to `expect()`.
 */
interface Matchers {
  /** Strict equality (`===`), with NaN considered equal to itself. */
  toBe(expected: unknown): void;
  /** Structural equality, comparing arrays and plain objects by their contents. */
  toEqual(expected: unknown): void;
  toBeTruthy(): void;
  toBeFalsy(): void;
  toBeNull(): void;
  toBeUndefined(): void;
  toBeDefined(): void;
  /** Numeric comparison to `digits` decimal places (default 2). */
  toBeCloseTo(expected: number, digits?: number): void;
  toBeGreaterThan(expected: number): void;
  toBeLessThan(expected: number): void;
  toHaveLength(expected: number): void;
  /** Substring of a string, or a structurally equal member of an array. */
  toContain(item: unknown): void;
  toMatch(pattern: string | RegExp): void;
  /**
   * Call the function under test and require that it throws. With an argument,
   * the thrown message must contain the string, or match the regular expression.
   */
  toThrow(expected?: string | RegExp): void;
  /** Every matcher above, inverted. */
  not: Omit<Matchers, "not">;
}

/**
 * Assert on a value inside a test case.
 *
 * @example
 * expect(totalCents([])).toBe(0);
 * expect(() => totalCents(null)).toThrow("items");
 * expect(basket.items).not.toContain({ sku: "gone" });
 */
declare function expect(actual: unknown): Matchers;

/**
 * Register a test case. Available only while the engine is running a script's
 * test modules — assets named `*.test.ts` (or `.js`, `.jsx`, `.tsx`) — which it
 * does on request via `POST /engine/run_tests?uri=<script>`.
 *
 * The body may be `async`: each case is settled before the next one starts, so
 * the verdict reflects the assertions it reached. `await` does not make
 * anything concurrent, though — host calls like `fetch()` and
 * `database.query()` block rather than yielding.
 *
 * Each test module runs in its own context, so one file cannot see globals set
 * by another. Database writes are rolled back unless the run asks otherwise;
 * asset writes, secret writes, and outbound HTTP are real.
 *
 * @example
 * import { totalCents } from "../server/basket.ts";
 *
 * test("an empty basket totals zero", () => {
 *   expect(totalCents([])).toBe(0);
 * });
 */
declare function test(name: string, fn: () => void): void;

/** Alias of {@link test}. */
declare function it(name: string, fn: () => void): void;

/**
 * Group cases under a shared name. Nested groups compose, so a case inside
 * `describe("basket")` is reported as `basket > an empty basket totals zero`.
 */
declare function describe(name: string, fn: () => void): void;

/**
 * Run before every case in the file. Hooks are file-scoped and apply to all of
 * its cases regardless of where they are declared, including cases declared
 * above the hook.
 */
declare function beforeEach(fn: () => void): void;

/** Run after every case in the file, including after a case that failed. */
declare function afterEach(fn: () => void): void;

/** Fail the current case with `message` unless `condition` holds. */
declare function assert(condition: unknown, message?: string): void;

// ============================================================================
// Response Builder Helpers
// ============================================================================

/**
 * Response builder utility object with methods for creating HTTP responses.
 */
declare var ResponseBuilder: {
  /**
   * Create a JSON response
   * @param data - Data to serialize as JSON
   * @param status - HTTP status code (default: 200)
   * @returns HTTP response object
   * @example
   * return ResponseBuilder.json({ message: "Success", data: results });
   */
  json(data: any, status?: number): HttpResponse;

  /**
   * Create a plain text response
   * @param text - Text content
   * @param status - HTTP status code (default: 200)
   * @returns HTTP response object
   * @example
   * return ResponseBuilder.text("Hello, World!");
   */
  text(text: string, status?: number): HttpResponse;

  /**
   * Create an HTML response
   * @param html - HTML content
   * @param status - HTTP status code (default: 200)
   * @returns HTTP response object
   * @example
   * return ResponseBuilder.html("<h1>Welcome</h1>");
   */
  html(html: string, status?: number): HttpResponse;

  /**
   * Create an error response
   * @param status - HTTP status code
   * @param message - Error message
   * @returns HTTP response object
   * @example
   * return ResponseBuilder.error(404, "Not found");
   */
  error(status: number, message: string): HttpResponse;

  /**
   * Create a 204 No Content response
   * @returns HTTP response object
   * @example
   * return ResponseBuilder.noContent();
   */
  noContent(): HttpResponse;

  /**
   * Create a 302 redirect response
   * @param location - Redirect URL
   * @returns HTTP response object
   * @example
   * return ResponseBuilder.redirect("/login");
   */
  redirect(location: string): HttpResponse;
};

// ============================================================================
// JSX Support for Server-Side HTML Generation
// ============================================================================

/**
 * JSX factory function for creating HTML elements
 * @param tag - HTML tag name or component function
 * @param props - Element attributes and properties
 * @param children - Child elements
 * @returns HTML string
 * @example
 * const element = <div className="container">Hello</div>;
 */
declare function h(
  tag: string | Function,
  props: Record<string, any> | null,
  ...children: any[]
): string;

/**
 * Fragment component for grouping elements without a wrapper
 * @param props - Props (typically null or contains children)
 * @param children - Child elements
 * @returns HTML string
 * @example
 * const list = <>
 *   <li>Item 1</li>
 *   <li>Item 2</li>
 * </>;
 */
declare function Fragment(
  props: { children?: any } | null,
  ...children: any[]
): string;

/**
 * Markdown and plain-text files are modules whose default export is their
 * content.
 *
 * ```ts
 * import refundPolicy from "./skills/refund.md";
 * ```
 *
 * The import resolves when the script is bundled, is cached in the prepared
 * program, is dropped when the file is written, and is part of what a
 * revision pins — so a system prompt or a skill definition costs nothing per
 * request and travels with the version of the code that was written against
 * it. `files.read` is for the other case: content that changes without a
 * redeploy.
 */
declare module "*.md" {
  const content: string;
  export default content;
}

declare module "*.txt" {
  const content: string;
  export default content;
}

declare module "react/jsx-runtime" {
  export { Fragment };

  export function jsx(
    tag: string | Function,
    props: Record<string, any> | null,
    key?: string | number,
  ): string;

  export function jsxs(
    tag: string | Function,
    props: Record<string, any> | null,
    key?: string | number,
  ): string;
}

/**
 * JSX namespace for TypeScript JSX type checking
 */
declare namespace JSX {
  /**
   * JSX elements are rendered as HTML strings
   */
  type Element = string;

  interface IntrinsicAttributes {
    key?: string | number;
  }

  /**
   * Intrinsic HTML elements with their attributes
   */
  interface IntrinsicElements {
    // Document metadata
    html: HtmlAttributes;
    head: HtmlAttributes;
    title: HtmlAttributes;
    meta: MetaAttributes;
    link: LinkAttributes;
    style: StyleAttributes;
    script: ScriptAttributes;
    base: BaseAttributes;

    // Content sectioning
    body: HtmlAttributes;
    header: HtmlAttributes;
    nav: HtmlAttributes;
    main: HtmlAttributes;
    section: HtmlAttributes;
    article: HtmlAttributes;
    aside: HtmlAttributes;
    footer: HtmlAttributes;
    h1: HtmlAttributes;
    h2: HtmlAttributes;
    h3: HtmlAttributes;
    h4: HtmlAttributes;
    h5: HtmlAttributes;
    h6: HtmlAttributes;

    // Text content
    div: HtmlAttributes;
    p: HtmlAttributes;
    span: HtmlAttributes;
    pre: HtmlAttributes;
    blockquote: HtmlAttributes;
    ul: HtmlAttributes;
    ol: HtmlAttributes;
    li: HtmlAttributes;
    dl: HtmlAttributes;
    dt: HtmlAttributes;
    dd: HtmlAttributes;
    hr: HtmlAttributes;
    br: HtmlAttributes;

    // Inline text semantics
    a: AnchorAttributes;
    abbr: HtmlAttributes;
    b: HtmlAttributes;
    strong: HtmlAttributes;
    em: HtmlAttributes;
    i: HtmlAttributes;
    code: HtmlAttributes;
    kbd: HtmlAttributes;
    mark: HtmlAttributes;
    q: HtmlAttributes;
    s: HtmlAttributes;
    small: HtmlAttributes;
    sub: HtmlAttributes;
    sup: HtmlAttributes;
    time: TimeAttributes;
    u: HtmlAttributes;
    var: HtmlAttributes;

    // Image and multimedia
    img: ImageAttributes;
    audio: AudioAttributes;
    video: VideoAttributes;
    source: SourceAttributes;
    track: TrackAttributes;
    canvas: CanvasAttributes;
    picture: HtmlAttributes;

    // Embedded content
    iframe: IframeAttributes;
    embed: EmbedAttributes;
    object: ObjectAttributes;
    param: ParamAttributes;

    // Forms
    form: FormAttributes;
    input: InputAttributes;
    textarea: TextareaAttributes;
    button: ButtonAttributes;
    select: SelectAttributes;
    option: OptionAttributes;
    optgroup: OptgroupAttributes;
    label: LabelAttributes;
    fieldset: FieldsetAttributes;
    legend: HtmlAttributes;
    datalist: HtmlAttributes;
    output: OutputAttributes;
    progress: ProgressAttributes;
    meter: MeterAttributes;

    // Tables
    table: TableAttributes;
    thead: HtmlAttributes;
    tbody: HtmlAttributes;
    tfoot: HtmlAttributes;
    tr: HtmlAttributes;
    th: ThAttributes;
    td: TdAttributes;
    col: ColAttributes;
    colgroup: ColgroupAttributes;
    caption: HtmlAttributes;

    // Interactive elements
    details: DetailsAttributes;
    summary: HtmlAttributes;
    dialog: DialogAttributes;
    menu: MenuAttributes;
  }

  /**
   * Common HTML attributes shared by all elements
   */
  interface HtmlAttributes {
    // Global attributes
    key?: string | number;
    id?: string;
    className?: string;
    class?: string;
    style?: string | Record<string, string>;
    title?: string;
    lang?: string;
    dir?: "ltr" | "rtl" | "auto";
    hidden?: boolean;
    tabIndex?: number;
    accessKey?: string;
    contentEditable?: boolean | "true" | "false";
    draggable?: boolean;
    spellCheck?: boolean;
    translate?: "yes" | "no";

    // ARIA attributes
    role?: string;
    "aria-label"?: string;
    "aria-labelledby"?: string;
    "aria-describedby"?: string;
    "aria-hidden"?: boolean;
    "aria-expanded"?: boolean;
    "aria-selected"?: boolean;
    "aria-checked"?: boolean;
    "aria-disabled"?: boolean;
    "aria-readonly"?: boolean;
    "aria-required"?: boolean;
    "aria-invalid"?: boolean;
    "aria-live"?: "polite" | "assertive" | "off";

    // Data attributes
    [key: `data-${string}`]: string | number | boolean;

    // Children
    children?: any;
  }

  interface AnchorAttributes extends HtmlAttributes {
    href?: string;
    target?: "_blank" | "_self" | "_parent" | "_top";
    rel?: string;
    download?: string | boolean;
    hreflang?: string;
    type?: string;
  }

  interface ImageAttributes extends HtmlAttributes {
    src?: string;
    alt?: string;
    width?: number | string;
    height?: number | string;
    loading?: "lazy" | "eager";
    decoding?: "async" | "sync" | "auto";
    crossOrigin?: "anonymous" | "use-credentials";
  }

  interface InputAttributes extends HtmlAttributes {
    type?:
      | "text"
      | "password"
      | "email"
      | "number"
      | "tel"
      | "url"
      | "search"
      | "date"
      | "time"
      | "datetime-local"
      | "month"
      | "week"
      | "color"
      | "file"
      | "checkbox"
      | "radio"
      | "submit"
      | "reset"
      | "button"
      | "hidden";
    name?: string;
    value?: string | number;
    placeholder?: string;
    required?: boolean;
    disabled?: boolean;
    readonly?: boolean;
    checked?: boolean;
    min?: number | string;
    max?: number | string;
    step?: number | string;
    minLength?: number;
    maxLength?: number;
    pattern?: string;
    autocomplete?: string;
    autofocus?: boolean;
    multiple?: boolean;
    accept?: string;
  }

  interface ButtonAttributes extends HtmlAttributes {
    type?: "button" | "submit" | "reset";
    name?: string;
    value?: string;
    disabled?: boolean;
    autofocus?: boolean;
    form?: string;
  }

  interface FormAttributes extends HtmlAttributes {
    action?: string;
    method?: "get" | "post";
    enctype?:
      | "application/x-www-form-urlencoded"
      | "multipart/form-data"
      | "text/plain";
    target?: "_blank" | "_self" | "_parent" | "_top";
    autocomplete?: "on" | "off";
    novalidate?: boolean;
  }

  interface TextareaAttributes extends HtmlAttributes {
    name?: string;
    value?: string;
    placeholder?: string;
    rows?: number;
    cols?: number;
    required?: boolean;
    disabled?: boolean;
    readonly?: boolean;
    minLength?: number;
    maxLength?: number;
    wrap?: "hard" | "soft";
    autofocus?: boolean;
  }

  interface SelectAttributes extends HtmlAttributes {
    name?: string;
    value?: string;
    required?: boolean;
    disabled?: boolean;
    multiple?: boolean;
    size?: number;
    autofocus?: boolean;
  }

  interface OptionAttributes extends HtmlAttributes {
    value?: string;
    selected?: boolean;
    disabled?: boolean;
    label?: string;
  }

  interface LabelAttributes extends HtmlAttributes {
    for?: string;
    form?: string;
  }

  interface TableAttributes extends HtmlAttributes {
    border?: number | string;
    cellPadding?: number | string;
    cellSpacing?: number | string;
  }

  interface ThAttributes extends HtmlAttributes {
    scope?: "row" | "col" | "rowgroup" | "colgroup";
    colspan?: number;
    rowspan?: number;
    headers?: string;
  }

  interface TdAttributes extends HtmlAttributes {
    colspan?: number;
    rowspan?: number;
    headers?: string;
  }

  interface ColAttributes extends HtmlAttributes {
    span?: number;
  }

  interface ColgroupAttributes extends HtmlAttributes {
    span?: number;
  }

  interface MetaAttributes extends HtmlAttributes {
    name?: string;
    content?: string;
    charset?: string;
    httpEquiv?: string;
  }

  interface LinkAttributes extends HtmlAttributes {
    href?: string;
    rel?: string;
    type?: string;
    media?: string;
    as?: string;
    crossOrigin?: "anonymous" | "use-credentials";
  }

  interface StyleAttributes extends HtmlAttributes {
    type?: string;
    media?: string;
  }

  interface ScriptAttributes extends HtmlAttributes {
    src?: string;
    type?: string;
    async?: boolean;
    defer?: boolean;
    crossOrigin?: "anonymous" | "use-credentials";
    integrity?: string;
    nomodule?: boolean;
  }

  interface BaseAttributes extends HtmlAttributes {
    href?: string;
    target?: string;
  }

  interface AudioAttributes extends HtmlAttributes {
    src?: string;
    autoplay?: boolean;
    controls?: boolean;
    loop?: boolean;
    muted?: boolean;
    preload?: "none" | "metadata" | "auto";
  }

  interface VideoAttributes extends AudioAttributes {
    width?: number | string;
    height?: number | string;
    poster?: string;
  }

  interface SourceAttributes extends HtmlAttributes {
    src?: string;
    type?: string;
    media?: string;
  }

  interface TrackAttributes extends HtmlAttributes {
    src?: string;
    kind?: "subtitles" | "captions" | "descriptions" | "chapters" | "metadata";
    srclang?: string;
    label?: string;
    default?: boolean;
  }

  interface CanvasAttributes extends HtmlAttributes {
    width?: number | string;
    height?: number | string;
  }

  interface IframeAttributes extends HtmlAttributes {
    src?: string;
    srcdoc?: string;
    width?: number | string;
    height?: number | string;
    name?: string;
    sandbox?: string;
    allow?: string;
    loading?: "lazy" | "eager";
  }

  interface EmbedAttributes extends HtmlAttributes {
    src?: string;
    type?: string;
    width?: number | string;
    height?: number | string;
  }

  interface ObjectAttributes extends HtmlAttributes {
    data?: string;
    type?: string;
    width?: number | string;
    height?: number | string;
    name?: string;
  }

  interface ParamAttributes extends HtmlAttributes {
    name?: string;
    value?: string;
  }

  interface TimeAttributes extends HtmlAttributes {
    datetime?: string;
  }

  interface FieldsetAttributes extends HtmlAttributes {
    disabled?: boolean;
    form?: string;
    name?: string;
  }

  interface OptgroupAttributes extends HtmlAttributes {
    disabled?: boolean;
    label?: string;
  }

  interface OutputAttributes extends HtmlAttributes {
    for?: string;
    form?: string;
    name?: string;
  }

  interface ProgressAttributes extends HtmlAttributes {
    value?: number;
    max?: number;
  }

  interface MeterAttributes extends HtmlAttributes {
    value?: number;
    min?: number;
    max?: number;
    low?: number;
    high?: number;
    optimum?: number;
  }

  interface DetailsAttributes extends HtmlAttributes {
    open?: boolean;
  }

  interface DialogAttributes extends HtmlAttributes {
    open?: boolean;
  }

  interface MenuAttributes extends HtmlAttributes {
    type?: "context" | "toolbar";
  }

  /**
   * Allows components to specify which prop contains children
   */
  interface ElementChildrenAttribute {
    children: {};
  }
}
