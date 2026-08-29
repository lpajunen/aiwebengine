/// <reference lib="es2020" />

/**
 * TypeScript type definitions for aiwebengine JavaScript API
 * @version 0.1.0
 *
 * Add this reference to your scripts for IDE autocomplete and type checking:
 * /// <reference path="https://your-engine.com/engine/types/v0.1.0/aiwebengine.d.ts" />
 *
 * IMPORTANT: Every script MUST export an init() function that registers routes,
 * GraphQL resolvers, or other initialization logic.
 *
 * @example
 * // Minimal script structure
 * function myHandler(context) {
 *   return ResponseBuilder.json({ message: "Hello" });
 * }
 *
 * function init() {
 *   routeRegistry.registerRoute("/api/hello", "myHandler", "GET");
 * }
 */

// ============================================================================
// Script Initialization
// ============================================================================

/**
 * Initialization function that must be exported by every script.
 * This function is called when the script is loaded and should register
 * routes, GraphQL resolvers, or perform other setup tasks.
 *
 * @param context - Handler context (optional, may not be provided during init)
 * @example
 * function init() {
 *   // Register HTTP routes
 *   routeRegistry.registerRoute("/api/users", "listUsers", "GET");
 *   routeRegistry.registerRoute("/api/users/:id", "getUser", "GET");
 *
 *   // Register GraphQL queries
 *   graphQLRegistry.registerQuery(
 *     "getUser",
 *     "getUser(id: ID!): User",
 *     "getUserResolver",
 *     "external",
 *   );
 *
 *   // Register streams
 *   routeRegistry.registerStreamRoute("/events/notifications");
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
 * sees the same API whether it was entered through an HTTP route, a GraphQL
 * resolver, an MCP tool, a scheduled job, a stream customizer, a message
 * listener or a test, so shared helpers never need `typeof x === "undefined"`
 * guards. What a call is *allowed* to do still depends on the caller's
 * capabilities, and what it *does* depends on the phase described here.
 *
 * A script's top-level program is re-evaluated on every invocation, and only
 * `init()` runs in the registration phase. So these methods:
 *
 * - `routeRegistry.registerRoute` / `registerAssetRoute` / `registerStreamRoute`
 * - `graphQLRegistry.registerQuery` / `registerMutation` / `registerSubscription`
 * - `mcpRegistry.registerTool` / `registerPrompt`
 * - `schedulerService.registerOnce` / `registerRecurring` / `clearAll`
 * - `dispatcher.registerListener`
 *
 * take effect during startup and `init()`, and elsewhere return a string saying
 * the entry was not registered. They never throw for being called at the wrong
 * time — a script that registers at top level rather than inside `init()` would
 * otherwise fail on every request. Argument validation is unaffected: a bad
 * path or an empty name is reported the same way in every context.
 *
 * Everything else — `database`, `assetStorage`, `secretStorage`,
 * `scriptStorage`, `personalStorage`, `fetch`, `convert`, `console`,
 * `McpClient`, `dispatcher.sendMessage`, `graphQLRegistry.executeGraphQL`,
 * `routeRegistry.sendStreamMessage` — works in every context.
 *
 * Register in `init()`. Registering from a request handler silently does
 * nothing, which is rarely what the script intended.
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
   * Present only for HTTP routes — a GraphQL resolver, MCP tool or stream
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
  | "graphqlQuery"
  | "graphqlMutation"
  | "graphqlSubscription"
  | "streamCustomization"
  | "messageListener"
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

  /** GraphQL or function arguments (for GraphQL resolvers) */
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
}

// ============================================================================
// Route Registry API
// ============================================================================

/**
 * Route registry for HTTP endpoints and streaming
 */
interface RouteRegistry {
  /**
   * Register an HTTP route handler
   * @param path - URL path pattern (e.g., "/blog/post/:id")
   * @param handlerName - Name of the handler function to call
   * @param method - HTTP method (GET, POST, PUT, DELETE, etc.). Registering
   *   GET automatically serves HEAD requests too, running the same handler
   *   and returning its headers with an empty body. Register HEAD explicitly
   *   to override this with custom behavior.
   * @param metadata - Optional OpenAPI metadata (summary, description, tags, parameters, requestBody)
   * @returns Registration result message
   * @example
   * routeRegistry.registerRoute("/api/users", "listUsers", "GET");
   * routeRegistry.registerRoute("/api/users", "createUser", "POST", {
   *   summary: "Create user",
   *   description: "Create a new user account",
   *   tags: ["Users"]
   * });
   * routeRegistry.registerRoute("/api/users/:id", "updateUser", "PUT", {
   *   summary: "Update user",
   *   description: "Update an existing user account",
   *   tags: ["Users"],
   *   parameters: JSON.stringify([
   *     {
   *       name: "id",
   *       in: "path",
   *       required: true,
   *       schema: { type: "string" }
   *     }
   *   ]),
   *   requestBody: JSON.stringify({
   *     required: true,
   *     content: {
   *       "application/json": {
   *         schema: {
   *           type: "object"
   *         }
   *       }
   *     }
   *   })
   * });
   *
   * Only takes effect during startup and `init()`. Called from a handler it
   * returns a message saying nothing was registered, and does not throw.
   */
  registerRoute(
    path: string,
    handlerName: string,
    method: string,
    metadata?: {
      summary?: string;
      description?: string;
      tags?: string[];
      parameters?: string; // JSON string of OpenAPI parameters array
      requestBody?: string; // JSON string of OpenAPI requestBody object
    },
  ): string;

  /**
   * Register a Server-Sent Events (SSE) stream endpoint
   * @param path - URL path for the stream (must start with /)
   * @param customizationFunction - Optional name of a function that returns connection filter criteria
   * @param metadata - Optional OpenAPI metadata. `tags` sets the Swagger group
   *   (defaults to "Streams"); `summary`/`description` override the
   *   auto-generated documentation text.
   * @returns Registration result message
   * @example
   * routeRegistry.registerStreamRoute("/events/notifications");
   * routeRegistry.registerStreamRoute("/events/chat", "chatCustomizer");
   * routeRegistry.registerStreamRoute("/events/alerts", undefined, {
   *   tags: ["Alerts"],
   *   summary: "Alert stream",
   * });
   *
   * Only takes effect during startup and `init()`. Called from a handler it
   * returns a message saying nothing was registered, and does not throw.
   */
  registerStreamRoute(
    path: string,
    customizationFunction?: string,
    metadata?: {
      summary?: string;
      description?: string;
      tags?: string[];
    },
  ): string;

  /**
   * Register a static asset route
   * @param httpPath - HTTP path where asset will be served (e.g., "/styles/main.css")
   * @param assetName - Name of the asset in the asset storage (e.g., "main.css")
   * @param metadata - Optional OpenAPI metadata. `tags` sets the Swagger group
   *   (defaults to "Assets"); `summary`/`description` override the
   *   auto-generated documentation text.
   * @returns Registration result message
   * @example
   * routeRegistry.registerAssetRoute("/styles/main.css", "main.css");
   * routeRegistry.registerAssetRoute("/logo.svg", "logo.svg", {
   *   tags: ["Branding"],
   *   summary: "Company logo",
   * });
   *
   * Only takes effect during startup and `init()`. Called from a handler it
   * returns a message saying nothing was registered, and does not throw.
   */
  registerAssetRoute(
    httpPath: string,
    assetName: string,
    metadata?: {
      summary?: string;
      description?: string;
      tags?: string[];
    },
  ): string;

  /**
   * Broadcast a message to all connections on a stream
   * @param path - Stream path
   * @param data - Data to send (will be JSON serialized)
   * @returns Broadcast result message
   * @example
   * routeRegistry.sendStreamMessage("/events/notifications", {
   *   type: "alert",
   *   message: "New update available"
   * });
   */
  sendStreamMessage(path: string, data: any): string;

  /**
   * Send a message to filtered connections based on metadata
   * @param path - Stream path
   * @param data - Data to send (will be JSON serialized)
   * @param filterJson - JSON filter criteria for connection metadata
   * @param matchMode - Optional filter matching mode. Defaults to "subset".
   * @returns Broadcast result message
   * @example
   * routeRegistry.sendStreamMessageFiltered(
   *   "/events/notifications",
   *   { message: "Admin alert" },
   *   JSON.stringify({ role: "admin" }),
   *   "subset"
   * );
   */
  sendStreamMessageFiltered(
    path: string,
    data: any,
    filterJson: string,
    matchMode?: "subset" | "overlap",
  ): string;
}

// ============================================================================
// Asset Storage API
// ============================================================================

/**
 * Asset metadata
 */
interface AssetMetadata {
  /** Asset URI/name */
  uri: string;

  /** Display name */
  name: string;

  /** MIME type */
  mimetype: string;

  /** Size in bytes */
  size: number;

  /** Creation timestamp */
  created_at: string;

  /** Last update timestamp */
  updated_at: string;
}

/**
 * Asset storage for managing static files (script-scoped)
 * Each script can only access and manage its own assets.
 */
interface AssetStorage {
  /**
   * List all assets owned by this script with metadata
   * @returns JSON string array of asset metadata
   * @example
   * const assetsJson = assetStorage.listAssets();
   * const assets = JSON.parse(assetsJson);
   */
  listAssets(): string;

  /**
   * Fetch an asset's content owned by this script
   * @param name - Asset name/URI
   * @returns Base64-encoded asset content or error message
   * @example
   * const content = assetStorage.fetchAsset("logo.svg");
   */
  fetchAsset(name: string): string;

  /**
   * Create or update an asset owned by this script
   * @param name - Asset name/URI
   * @param mimetype - MIME type (e.g., "image/png", "text/css")
   * @param contentBase64 - Base64-encoded content
   * @returns Operation result message
   * @example
   * assetStorage.upsertAsset("logo.svg", "image/svg+xml", base64Content);
   */
  upsertAsset(name: string, mimetype: string, contentBase64: string): string;

  /**
   * Delete an asset owned by this script
   * @param name - Asset name/URI
   * @returns Operation result message
   * @example
   * assetStorage.deleteAsset("old-logo.svg");
   */
  deleteAsset(name: string): string;
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
 * `QuotaExceededError` when a value exceeds 1 MB, `SecurityError` when the
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
   * @throws DOMException `QuotaExceededError` if the value exceeds 1 MB,
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
 */
interface SecretStorage {
  /**
   * Check if a secret exists for the current script.
   * First checks `user_secrets` for the authenticated user, then falls back to `script_secrets`.
   * @param key - Secret key to check
   * @returns true if the secret exists in either table, false otherwise
   * @example
   * if (secretStorage.exists("API_TOKEN")) {
   *   // secret is available (user-level or script-level)
   * }
   */
  exists(key: string): boolean;

  /**
   * Store a secret for the authenticated user in the current script.
   * @param key - Secret key
   * @param value - Secret value to store (max 1 MB)
   * @returns Success message or error string if unauthenticated or validation fails
   * @example
   * const result = secretStorage.setSecret("API_TOKEN", "abc123");
   * if (result.startsWith("Error")) {
   *   console.error(result);
   * }
   */
  setSecret(key: string, value: string): string;

  /**
   * Remove a single secret for the authenticated user in the current script.
   * @param key - Secret key to remove
   * @returns true if the secret existed and was removed, false otherwise
   * @example
   * secretStorage.removeSecret("OLD_TOKEN");
   */
  removeSecret(key: string): boolean;

  /**
   * Clear all secrets for the authenticated user in the current script.
   * @returns Success message or error string if unauthenticated
   * @example
   * secretStorage.clear();
   */
  clear(): string;
}

// ============================================================================
// Scheduler Service API
// ============================================================================

/**
 * Scheduler service for managing scheduled tasks
 */
interface SchedulerService {
  /**
   * Register a one-time scheduled job
   * @param options - Job options
   * @param options.handler - Name of the handler function to call
   * @param options.runAt - UTC ISO timestamp when to run (e.g., "2025-12-17T15:30:00Z")
   * @param options.name - Optional job name/key
   * @returns Result message with job details
   * @example
   * const oneHourFromNow = new Date(Date.now() + 3600000).toISOString();
   * schedulerService.registerOnce({
   *   handler: "sendReminder",
   *   runAt: oneHourFromNow,
   *   name: "reminder-job"
   * });
   *
   * Only takes effect during startup and `init()`. Called from a handler it
   * returns a message saying nothing was registered, and does not throw.
   */
  registerOnce(options: {
    handler: string;
    runAt: string;
    name?: string;
  }): string;

  /**
   * Register a recurring scheduled job
   * @param options - Job options
   * @param options.handler - Name of the handler function to call
   * @param options.intervalMilliseconds - Interval in milliseconds (minimum 100)
   * @param options.intervalMinutes - Interval in minutes (minimum 1, backward compatible)
   * @param options.name - Optional job name/key
   * @param options.startAt - Optional UTC ISO timestamp for first run
   * @returns Result message with job details
   * @example
   * schedulerService.registerRecurring({
   *   handler: "cleanupOldData",
   *   intervalMilliseconds: 5000,
   *   name: "cleanup-job"
   * });
   *
   * Only takes effect during startup and `init()`. Called from a handler it
   * returns a message saying nothing was registered, and does not throw.
   */
  registerRecurring(options: {
    handler: string;
    intervalMilliseconds?: number;
    intervalMinutes?: number;
    name?: string;
    startAt?: string;
  }): string;

  /**
   * Clear all scheduled jobs for the current script
   * @returns Result message with count of cleared jobs
   * @example
   * schedulerService.clearAll();
   *
   * Only takes effect during startup and `init()`. Called from a handler it
   * clears nothing and says so in the returned message, and does not throw.
   */
  clearAll(): string;
}

// ============================================================================
// GraphQL Registry API
// ============================================================================

/**
 * GraphQL schema and resolver registration
 */
interface GraphQLRegistry {
  /**
   * Register a GraphQL query
   * @param name - Query name
   * @param sdl - GraphQL SDL (Schema Definition Language) for the query
   * @param resolverFunction - Name of the resolver function
   * @param visibility - Visibility level: "internal" (script-only), "engine" (all scripts), or "external" (authenticated API access)
   * @returns Registration result message
   * @example
   * graphQLRegistry.registerQuery(
   *   "getUser",
   *   "getUser(id: ID!): User",
   *   "getUserResolver",
   *   "external"
   * );
   *
   * Only takes effect during startup and `init()`. Called from a handler it
   * returns a message saying nothing was registered, and does not throw.
   */
  registerQuery(
    name: string,
    sdl: string,
    resolverFunction: string,
    visibility: string,
  ): string;

  /**
   * Register a GraphQL mutation
   * @param name - Mutation name
   * @param sdl - GraphQL SDL (Schema Definition Language) for the mutation
   * @param resolverFunction - Name of the resolver function
   * @param visibility - Visibility level: "internal" (script-only), "engine" (all scripts), or "external" (authenticated API access)
   * @returns Registration result message
   * @example
   * graphQLRegistry.registerMutation(
   *   "createUser",
   *   "createUser(name: String!, email: String!): User",
   *   "createUserResolver",
   *   "external"
   * );
   *
   * Only takes effect during startup and `init()`. Called from a handler it
   * returns a message saying nothing was registered, and does not throw.
   */
  registerMutation(
    name: string,
    sdl: string,
    resolverFunction: string,
    visibility: string,
  ): string;

  /**
   * Register a GraphQL subscription
   * @param name - Subscription name
   * @param sdl - GraphQL SDL (Schema Definition Language) for the subscription
   * @param resolverFunction - Name of the resolver function
   * @param visibility - Visibility level: "internal" (script-only), "engine" (all scripts), or "external" (authenticated API access)
   * @returns Registration result message
   * @example
   * graphQLRegistry.registerSubscription(
   *   "messageAdded",
   *   "messageAdded(chatId: ID!): Message",
   *   "messageAddedResolver",
   *   "external"
   * );
   *
   * Only takes effect during startup and `init()`. Called from a handler it
   * returns a message saying nothing was registered, and does not throw.
   */
  registerSubscription(
    name: string,
    sdl: string,
    resolverFunction: string,
    visibility: string,
  ): string;

  /**
   * Execute a GraphQL query internally
   * @param query - GraphQL query string
   * @param variables - Query variables (optional)
   * @returns JSON string with query results
   * @example
   * const result = graphQLRegistry.executeGraphQL(
   *   "query { getUser(id: \"123\") { name } }",
   *   "{}"
   * );
   */
  executeGraphQL(query: string, variables?: string): string;

  /**
   * Send a message to all connections subscribed to a GraphQL subscription
   * @param subscriptionName - Name of the subscription
   * @param message - Message to send (will be JSON serialized)
   * @returns Send result message
   * @example
   * graphQLRegistry.sendSubscriptionMessage(
   *   "messageAdded",
   *   JSON.stringify({ id: "123", text: "Hello" })
   * );
   */
  sendSubscriptionMessage(subscriptionName: string, message: string): string;

  /**
   * Send a message to filtered connections based on metadata
   * @param subscriptionName - Name of the subscription
   * @param message - Message to send (will be JSON serialized)
   * @param filterJson - JSON filter criteria for connection metadata (optional)
   * @param matchMode - Optional filter matching mode. Defaults to "subset".
   * @returns Send result message
   * @example
   * graphQLRegistry.sendSubscriptionMessageFiltered(
   *   "messageAdded",
   *   JSON.stringify({ id: "123", text: "Admin message" }),
   *   JSON.stringify({ role: "admin" }),
   *   "subset"
   * );
   */
  sendSubscriptionMessageFiltered(
    subscriptionName: string,
    message: string,
    filterJson?: string,
    matchMode?: "subset" | "overlap",
  ): string;
}

// ============================================================================
// MCP (Model Context Protocol) Registry API
// ============================================================================

/**
 * MCP Registry for registering tools and prompts
 */
interface McpRegistry {
  /**
   * Register an MCP tool
   * @param name - Tool name (1-100 characters)
   * @param description - Tool description (1-1000 characters)
   * @param inputSchemaJson - JSON string defining input schema
   * @param handlerFunction - Name of handler function to call
   * @returns Registration result message
   * @example
   * mcpRegistry.registerTool(
   *   "calculateSum",
   *   "Calculates the sum of two numbers",
   *   JSON.stringify({
   *     type: "object",
   *     properties: {
   *       a: { type: "number" },
   *       b: { type: "number" }
   *     },
   *     required: ["a", "b"]
   *   }),
   *   "handleCalculateSum"
   * );
   *
   * Only takes effect during startup and `init()`. Called from a handler it
   * returns a message saying nothing was registered, and does not throw.
   */
  registerTool(
    name: string,
    description: string,
    inputSchemaJson: string,
    handlerFunction: string,
  ): string;

  /**
   * Register an MCP prompt
   * @param name - Prompt name (1-100 characters)
   * @param description - Prompt description (1-1000 characters)
   * @param argumentsJson - JSON string defining prompt arguments
   * @param handlerFunction - Name of handler function to call (1-100 characters)
   * @returns Registration result message
   * @example
   * mcpRegistry.registerPrompt(
   *   "generateCode",
   *   "Generates code based on requirements",
   *   JSON.stringify({
   *     language: { type: "string", description: "Programming language" },
   *     task: { type: "string", description: "Task description" }
   *   }),
   *   "handleGenerateCode"
   * );
   *
   * Only takes effect during startup and `init()`. Called from a handler it
   * returns a message saying nothing was registered, and does not throw.
   */
  registerPrompt(
    name: string,
    description: string,
    argumentsJson: string,
    handlerFunction: string,
  ): string;
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

  /** Request headers */
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

  /** Response body as string */
  body: string;

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
 * @param url - URL to fetch (supports {{SECRET_NAME}} syntax for secret injection)
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
 *     "Authorization": "Bearer {{API_TOKEN}}",
 *     "Content-Type": "application/json"
 *   },
 *   body: JSON.stringify({ key: "value" })
 * });
 */
declare function fetch(
  url: string,
  options?: FetchOptions,
): FetchResponse & PromiseLike<FetchResponse>;

// ============================================================================
// Database API (Script-Scoped Table Management)
// ============================================================================

/**
 * Database interface for script-scoped table management and operations.
 * Each script can create and manage its own tables with automatic namespacing.
 */
/**
 * The answer from a `database` call.
 *
 * Readable three ways, so the shape does not depend on which host API produced
 * it and the scripts written against the JSON string keep working:
 *
 * - `(await database.query("notes")).json()` — awaitable, like a `fetch` response
 * - `database.query("notes").json()` — the same, without awaiting
 * - `JSON.parse(database.query("notes"))` — `toString` yields the raw string
 *
 * `await` here is sequencing sugar: the call has already finished by the time
 * it returns.
 */
interface DatabaseResult {
  /** The answer, parsed. Throws if the call did not answer with JSON. */
  json(): unknown;

  /** The answer, as the raw string. */
  text(): string;

  /**
   * The raw JSON string, which is what these calls used to return.
   * `JSON.parse(database.query(t))` still works because `JSON.parse` converts
   * its argument with ToString first.
   */
  toString(): string;
}

/**
 * A {@link DatabaseResult}, usable with or without `await`, and anywhere the
 * string these calls used to return was.
 *
 * The `string` in this intersection is not a convenience: the value really is
 * a `String` object, so every string method works on it and existing code
 * needs no change. The single exception is `typeof`, which reports `"object"`
 * rather than `"string"` — a check written that way has to become
 * `typeof String(result)` or, better, `result.json()`.
 */
type DatabaseAnswer = string & DatabaseResult & PromiseLike<DatabaseResult>;

interface Database {
  /**
   * Create a new table for this script
   * @param tableName - Logical table name (will be prefixed with script namespace)
   * @returns JSON string with result: {success: boolean, tableName: string, physicalName: string} or {error: string}
   * @example
   * const result = database.createTable("users").json();
   * // {success: true, tableName: "users", physicalName: "script_myapp_users"}
   */
  createTable(tableName: string): DatabaseAnswer;

  /**
   * Bring a table to the shape you describe, whatever shape it is in now.
   *
   * The idempotent form of `createTable` plus a run of `add*Column` plus
   * `addUniqueIndex`. Every step is checked before it is attempted rather than
   * attempted and forgiven, so calling this on a table that is already correct
   * costs one query and reports that it changed nothing — and an error that
   * comes back means something other than "already done".
   *
   * The whole convergence runs under one lock keyed on this script and table,
   * so concurrent callers — a cold start where every instance's first write
   * arrives at once — take turns instead of racing.
   *
   * Columns default to nullable. A column added to a table that already holds
   * rows cannot be `NOT NULL` without a default, and being safe against a table
   * already in use is the point of this call.
   *
   * @param tableName - Logical table name
   * @param schema - JSON string: `{ columns: [{ name, type, nullable?, default? }], uniqueIndexes?: string[][] }`
   *   where `type` is one of integer, bigint, float, text, boolean, timestamp
   * @returns JSON string with result: {success, created, columnsAdded, uniqueIndexesEnsured} or {error: string}
   * @example
   * const result = database.ensureTable("world_items", JSON.stringify({
   *   columns: [
   *     { name: "item_id", type: "text" },
   *     { name: "owner", type: "text" },
   *     { name: "updated_at", type: "bigint" },
   *   ],
   *   uniqueIndexes: [["item_id"]],
   * })).json();
   * // First run:  {success: true, created: true, columnsAdded: ["item_id", "owner", "updated_at"], ...}
   * // Every run after: {success: true, created: false, columnsAdded: [], ...}
   */
  ensureTable(tableName: string, schema: string): DatabaseAnswer;

  /**
   * Drop a table owned by this script
   * @param tableName - Table name to drop
   * @returns JSON string with result: {success: boolean, tableName: string, dropped: boolean} or {error: string}
   * @example
   * const result = database.dropTable("old_data").json();
   */
  dropTable(tableName: string): DatabaseAnswer;

  /**
   * Add a 32-bit integer column to a table
   *
   * Holds whole numbers up to about 2.1 billion. A value with a fraction is
   * refused rather than rounded — use `addFloatColumn` to keep it — and a
   * value past the range is refused rather than wrapped. For epoch
   * milliseconds and anything else that outgrows 2.1 billion, use
   * `addBigintColumn`.
   *
   * @param tableName - Table name
   * @param columnName - Column name
   * @param nullable - Whether column can be NULL (default: true)
   * @param defaultValue - Default value (optional)
   * @returns JSON string with result: {success: boolean, column: string} or {error: string}
   * @example
   * database.addIntegerColumn("users", "age", true);
   * database.addIntegerColumn("products", "stock", false, "0");
   */
  addIntegerColumn(
    tableName: string,
    columnName: string,
    nullable?: boolean,
    defaultValue?: string,
  ): DatabaseAnswer;

  /**
   * Add a 64-bit integer column to a table
   *
   * What `Date.now()` needs: epoch milliseconds are past 1.7 trillion, which
   * an `addIntegerColumn` column cannot hold. JavaScript integers are exact to
   * 2^53, so anything a script can count with round-trips exactly.
   *
   * @param tableName - Table name
   * @param columnName - Column name
   * @param nullable - Whether column can be NULL (default: true)
   * @param defaultValue - Default value (optional)
   * @returns JSON string with result: {success: boolean, column: string} or {error: string}
   * @example
   * database.addBigintColumn("events", "occurred_at_ms", false, "0");
   * database.insert("events", JSON.stringify({ occurred_at_ms: Date.now() }));
   */
  addBigintColumn(
    tableName: string,
    columnName: string,
    nullable?: boolean,
    defaultValue?: string,
  ): DatabaseAnswer;

  /**
   * Add a floating-point column to a table
   *
   * The column type that holds a JavaScript number as it is: rates, ratios,
   * scores, measurements. The value round-trips exactly, because the column is
   * a double and so is a JavaScript number.
   *
   * Not for money. `0.1 + 0.2` is not `0.3` in any double, here or in
   * JavaScript. Store amounts as whole minor units — cents, not euros — in an
   * `addIntegerColumn` or `addBigintColumn` column.
   *
   * @param tableName - Table name
   * @param columnName - Column name
   * @param nullable - Whether column can be NULL (default: true)
   * @param defaultValue - Default value (optional)
   * @returns JSON string with result: {success: boolean, column: string} or {error: string}
   * @example
   * database.addFloatColumn("readings", "celsius", true);
   * database.insert("readings", JSON.stringify({ celsius: 21.5 }));
   */
  addFloatColumn(
    tableName: string,
    columnName: string,
    nullable?: boolean,
    defaultValue?: string,
  ): DatabaseAnswer;

  /**
   * Add a text column to a table
   * @param tableName - Table name
   * @param columnName - Column name
   * @param nullable - Whether column can be NULL (default: true)
   * @param defaultValue - Default value (optional)
   * @returns JSON string with result: {success: boolean, column: string} or {error: string}
   * @example
   * database.addTextColumn("users", "email", false);
   * database.addTextColumn("posts", "title", false, "Untitled");
   */
  addTextColumn(
    tableName: string,
    columnName: string,
    nullable?: boolean,
    defaultValue?: string,
  ): DatabaseAnswer;

  /**
   * Add a boolean column to a table
   * @param tableName - Table name
   * @param columnName - Column name
   * @param nullable - Whether column can be NULL (default: true)
   * @param defaultValue - Default value (optional, "true" or "false")
   * @returns JSON string with result: {success: boolean, column: string} or {error: string}
   * @example
   * database.addBooleanColumn("users", "active", false, "true");
   */
  addBooleanColumn(
    tableName: string,
    columnName: string,
    nullable?: boolean,
    defaultValue?: string,
  ): DatabaseAnswer;

  /**
   * Add a timestamp column to a table.
   *
   * `defaultValue` is either the moment the row is written — `"NOW()"` or
   * `"CURRENT_TIMESTAMP"`, which mean the same thing — or a fixed instant as
   * `"YYYY-MM-DD"`, `"YYYY-MM-DD HH:MM:SS"`, or ISO 8601. Anything else is
   * refused rather than passed to the database to judge, so a default means
   * the same time wherever the table is later read.
   *
   * @param tableName - Table name
   * @param columnName - Column name
   * @param nullable - Whether column can be NULL (default: true)
   * @param defaultValue - `"NOW()"`, `"CURRENT_TIMESTAMP"`, or a fixed instant (optional)
   * @returns JSON string with result: {success: boolean, column: string} or {error: string}
   * @example
   * database.addTimestampColumn("posts", "created_at", false, "CURRENT_TIMESTAMP");
   * database.addTimestampColumn("posts", "embargoed_until", true, "2030-01-01");
   */
  addTimestampColumn(
    tableName: string,
    columnName: string,
    nullable?: boolean,
    defaultValue?: string,
  ): DatabaseAnswer;

  /**
   * Add a foreign key reference column to a table
   * @param tableName - Table name
   * @param columnName - Column name
   * @param referencedTableName - Referenced table name
   * @param nullable - Whether column can be NULL (default: true)
   * @returns JSON string with result: {success: boolean, foreignKey: string, nullable: boolean} or {error: string}
   * @example
   * database.addReferenceColumn("posts", "author_id", "users", false);
   */
  addReferenceColumn(
    tableName: string,
    columnName: string,
    referencedTableName: string,
    nullable?: boolean,
  ): DatabaseAnswer;

  /**
   * Drop a column from a table
   * @param tableName - Table name
   * @param columnName - Column name
   * @returns JSON string with result: {success: boolean, tableName: string, columnName: string, dropped: boolean} or {error: string}
   * @example
   * const result = database.dropColumn("users", "old_field").json();
   */
  dropColumn(tableName: string, columnName: string): DatabaseAnswer;

  /**
   * Query rows from a table with optional filters, limit, and ordering.
   *
   * `filters` supports two forms:
   * - Equality: `{ "col": value }` → `col = value`
   * - Comparison: `{ "col": { "$gt": v } }` → `col > v`
   *   Supported operators: `$gt`, `$gte`, `$lt`, `$lte`, `$ne`
   *   Multiple operators on the same column are AND-ed together.
   *
   * Any positional argument may be passed as `null` to skip it and take its
   * default — `query("chat", null, 100)`.
   *
   * **Reading in order to write.** A plain query takes no lock, so two
   * transactions can read the same row, each compute from what they read, and
   * each commit — leaving only the second write. Both report success. To make
   * a read-modify-write safe, open a transaction and pass
   * `{ forUpdate: true }`: the rows the query returns are then held until the
   * transaction ends, and a second caller waits and re-reads what the first
   * one wrote. Asking for it outside a transaction is refused, because a lock
   * taken there is released as soon as the query returns.
   *
   * @param tableName - Table name
   * @param filters - JSON string with filter conditions (optional)
   * @param limit - Maximum rows to return (default 100, max 1000)
   * @param orderBy - Column to sort by (optional)
   * @param orderDir - Sort direction: `"asc"` (default) or `"desc"`. Anything
   *                   else is refused rather than sorted ascending
   * @param options - JSON string of query options: `{ forUpdate?: boolean }`.
   *                  An unrecognised option is refused rather than ignored
   * @returns JSON string array of matching rows or {error: string}
   * @example
   * // Range filter: users active in the last 90 seconds
   * const cutoff = Date.now() - 90000;
   * const active = database.query(
   *   "presence",
   *   JSON.stringify({ last_active: { "$gt": cutoff } })
   * ).json();
   *
   * // Last 100 chat messages, newest first
   * const msgs = database.query("chat", null, 100, "ts", "desc").json();
   *
   * // A counter that stays correct under concurrency
   * database.beginTransaction(5000);
   * const row = database
   *   .query("event_seq", null, 1, null, null, JSON.stringify({ forUpdate: true }))
   *   .json()[0];
   * database.update("event_seq", row.id, JSON.stringify({ seq: row.seq + 1 }));
   * database.commitTransaction();
   */
  query(
    tableName: string,
    filters?: string | null,
    limit?: number | null,
    orderBy?: string | null,
    orderDir?: "asc" | "desc" | null,
    options?: string | null,
  ): DatabaseAnswer;

  /**
   * Insert a row into a table
   * @param tableName - Table name
   * @param data - JSON string with column values
   * @returns JSON string with inserted row (including generated id) or {error: string}
   * @example
   * const result = database
   *   .insert("users", JSON.stringify({name: "John", email: "john@example.com"}))
   *   .json();
   */
  insert(tableName: string, data: string): DatabaseAnswer;

  /**
   * Update a row in a table by ID
   * @param tableName - Table name
   * @param id - Row ID
   * @param data - JSON string with column values to update
   * @returns JSON string with updated row or {error: string}
   * @example
   * const result = database.update("users", 1, JSON.stringify({name: "Jane"})).json();
   */
  update(tableName: string, id: number, data: string): DatabaseAnswer;

  /**
   * Delete a row from a table by ID
   * @param tableName - Table name
   * @param id - Row ID
   * @returns JSON string with result: {success: boolean, deleted: boolean} or {error: string}
   * @example
   * const result = database.delete("users", 5).json();
   */
  delete(tableName: string, id: number): DatabaseAnswer;

  /**
   * Insert or update a row by a unique key (atomic upsert).
   *
   * Uses PostgreSQL `INSERT … ON CONFLICT DO UPDATE`, so the table must have a
   * unique index on `keyColumns` — create one with `database.addUniqueIndex()`.
   *
   * @param tableName - Table name
   * @param keyColumns - JSON array of column names that form the conflict target, or a single column name string
   * @param data - JSON string with all column values (including key columns)
   * @returns JSON string with the upserted row or {error: string}
   * @example
   * // Ensure unique index exists first (idempotent):
   * database.addUniqueIndex("presence", JSON.stringify(["user_id"]));
   *
   * database.upsert("presence",
   *   JSON.stringify(["user_id"]),
   *   JSON.stringify({ user_id: userId, nick: nick, last_active: Date.now() })
   * );
   */
  upsert(tableName: string, keyColumns: string, data: string): DatabaseAnswer;

  /**
   * Delete rows matching filter conditions (bulk delete).
   *
   * Supports the same filter syntax as `query()` including range operators.
   * At least one filter is required to prevent accidental full-table deletes.
   *
   * @param tableName - Table name
   * @param filters - JSON string with filter conditions (required)
   * @returns JSON string with result: {success: boolean, deleted: number} or {error: string}
   * @example
   * // Prune stale presence rows
   * const cutoff = Date.now() - 90000;
   * database.deleteWhere("presence",
   *   JSON.stringify({ last_active: { "$lt": cutoff } })
   * );
   */
  deleteWhere(tableName: string, filters: string): DatabaseAnswer;

  /**
   * Atomically acquire or extend a distributed lease (compare-and-swap).
   *
   * The table must be created with `database.createLeaseTable()`, which sets up
   * the required schema.
   *
   * Acquisition rules:
   * - No existing row → acquired.
   * - Existing row is expired → acquired.
   * - Existing row belongs to the same owner → acquired, extending the TTL.
   * - Existing row belongs to a different owner and is not expired → not acquired.
   *
   * Expiry is measured against the engine's clock, not the database's, so
   * instances competing for one lease must keep time to well inside the TTL —
   * an instance whose clock runs ahead may take a lease slightly before it
   * truly lapses. Seconds-long TTLs on NTP-synchronised hosts are well clear
   * of this; sub-second TTLs across several machines are not.
   *
   * A `ttlMs` that is not positive is refused, as is one so large that the
   * moment it would expire cannot be represented.
   *
   * @param tableName - Lease table created with `createLeaseTable`
   * @param leaseId - Unique identifier for this lease slot (e.g., `"npc_tick_world_42"`)
   * @param owner - Unique token for this process/server instance
   * @param ttlMs - Lease duration in milliseconds
   * @returns JSON string: `{acquired: boolean, owner: string, expires_at: string}` or `{error: string}`
   * @example
   * const lease = database
   *   .acquireLease("npc_leases", "world_" + worldId, myServerId, 2000)
   *   .json();
   * if (!lease.acquired) return; // another instance owns the lease
   */
  acquireLease(
    tableName: string,
    leaseId: string,
    owner: string,
    ttlMs: number,
  ): DatabaseAnswer;

  /**
   * Create a lease table with the correct schema for use with `acquireLease()`.
   *
   * The table is idempotent — calling it a second time returns the existing physical name.
   * Its columns are the lease id (the slot), its owner, and when the lease
   * expires; they are managed by the engine and not added or altered directly.
   *
   * @param tableName - Logical table name
   * @returns JSON string: `{success: boolean, tableName: string, physicalName: string}` or `{error: string}`
   * @example
   * database.createLeaseTable("npc_leases");
   */
  createLeaseTable(tableName: string): DatabaseAnswer;

  /**
   * Add a unique index to a table column (or set of columns).
   *
   * Required before using those columns as the conflict target in `upsert()`.
   * Uses `CREATE UNIQUE INDEX IF NOT EXISTS` — safe to call repeatedly.
   *
   * @param tableName - Table name
   * @param columns - JSON array of column names, or a single column name string
   * @returns JSON string: `{success: boolean, tableName: string, columns: …}` or `{error: string}`
   * @example
   * database.addUniqueIndex("presence", JSON.stringify(["user_id"]));
   * database.addUniqueIndex("scores", JSON.stringify(["user_id", "world_id"]));
   */
  addUniqueIndex(tableName: string, columns: string): DatabaseAnswer;

  /**
   * Auto-generate GraphQL operations for a table
   * @param tableName - Table name
   * @param options - JSON string with options (optional): {visibility: "script_internal" | "public" | "authenticated"}
   * @returns JSON string with result: {success: boolean, table: string, queries: string[], mutations: string[]} or {error: string}
   * @example
   * const result = database
   *   .generateGraphQLForTable("users", JSON.stringify({visibility: "authenticated"}))
   *   .json();
   * // Automatically creates queries like: getUser, listUsers
   * // And mutations like: createUser, updateUser, deleteUser
   */
  generateGraphQLForTable(tableName: string, options?: string): DatabaseAnswer;

  // Transaction Management

  /**
   * Begin a new database transaction or create a savepoint if already in a transaction.
   * Transactions auto-commit on normal handler exit and auto-rollback on exceptions.
   *
   * `timeout_ms` is enforced by the database, not merely recorded. Within the
   * transaction, no single statement runs longer than the budget, no wait for
   * a lock exceeds it, and if the handler is stopped mid-transaction the
   * database ends the transaction and releases its locks once the budget's
   * worth of idleness has passed. It bounds each step, not their sum: many
   * fast statements can still take longer than the budget between them.
   *
   * A budget can only tighten the engine's own limits, never loosen them —
   * asking for ten minutes on an engine that allows five seconds gets five.
   * Omitting it leaves the engine's configured limits in force.
   *
   * @param timeout_ms - Optional budget in milliseconds for each statement,
   *   lock wait and idle gap in the transaction
   * @returns JSON string with result: {success: boolean, message: string} or {error: string}
   * @example
   * // Start transaction with 5 second timeout
   * const result = database.beginTransaction(5000).json();
   * if (result.error) {
   *   console.error(`Failed to start transaction: ${result.error}`);
   *   return ResponseBuilder.error(500, "Transaction error");
   * }
   *
   * // Perform database operations...
   * // Transaction auto-commits on normal return or auto-rollbacks on exception
   */
  beginTransaction(timeout_ms?: number): DatabaseAnswer;

  /**
   * Commit the current transaction or release the most recent savepoint.
   * Note: Transactions auto-commit on handler success, so explicit commit is optional.
   * @returns JSON string with result: {success: boolean, message: string} or {error: string}
   * @example
   * const result = database.commitTransaction().json();
   * if (result.error) {
   *   console.error(`Failed to commit: ${result.error}`);
   * }
   */
  commitTransaction(): DatabaseAnswer;

  /**
   * Rollback the current transaction or to the most recent savepoint.
   * Note: Transactions auto-rollback on exceptions, so explicit rollback is optional.
   * @returns JSON string with result: {success: boolean, message: string} or {error: string}
   * @example
   * // Explicitly rollback on validation failure
   * if (!isValid(data)) {
   *   database.rollbackTransaction();
   *   return ResponseBuilder.error(400, "Invalid data");
   * }
   */
  rollbackTransaction(): DatabaseAnswer;

  /**
   * Create a named or auto-generated savepoint for nested transaction control.
   * Savepoints allow partial rollback within a transaction.
   * @param name - Optional savepoint name. If omitted, generates name like "sp_1", "sp_2", etc.
   * @returns JSON string with result: {success: boolean, savepoint: string} or {error: string}
   * @example
   * // Auto-generated savepoint
   * const sp = database.createSavepoint().json();
   * console.log(`Savepoint: ${sp.savepoint}`); // "sp_1"
   *
   * // Named savepoint
   * database.createSavepoint("checkpoint_before_insert");
   */
  createSavepoint(name?: string): DatabaseAnswer;

  /**
   * Rollback to a specific savepoint without ending the transaction.
   * @param name - Savepoint name to rollback to
   * @returns JSON string with result: {success: boolean, message: string} or {error: string}
   * @example
   * const sp = database.createSavepoint("before_update").json();
   *
   * try {
   *   database.update("users", userId, JSON.stringify({status: "active"}));
   * } catch (error) {
   *   // Rollback just this update, keep other changes
   *   database.rollbackToSavepoint(sp.savepoint);
   * }
   */
  rollbackToSavepoint(name: string): DatabaseAnswer;

  /**
   * Release a savepoint, making its changes permanent within the transaction scope.
   * @param name - Savepoint name to release
   * @returns JSON string with result: {success: boolean, message: string} or {error: string}
   * @example
   * const sp = database.createSavepoint("checkpoint").json();
   *
   * // Perform operations...
   *
   * // Release savepoint (changes become permanent in transaction)
   * database.releaseSavepoint(sp.savepoint);
   */
  releaseSavepoint(name: string): DatabaseAnswer;
}

// ============================================================================
// Console API
// ============================================================================

/**
 * Console logging interface
 * Note: reading and pruning stored log entries is engine administration, not a
 * script API — use `GET|DELETE /engine/script_logs` or the equivalent MCP tools.
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
// Message Dispatcher API
// ============================================================================

/**
 * Message dispatcher for inter-script communication
 */
interface MessageDispatcher {
  /**
   * Register a listener for a message type
   * @param messageType - Type of message to listen for (e.g., "user.created")
   * @param handlerName - Name of the handler function to call
   * @returns Registration result message
   * @throws If messageType or handlerName is empty
   * @example
   * dispatcher.registerListener("user.created", "handleUserCreated");
   *
   * Only takes effect during startup and `init()`. Called from a handler it
   * returns a message saying nothing was registered, and does not throw.
   */
  registerListener(messageType: string, handlerName: string): string;

  /**
   * Send a message to all listeners of a message type
   * @param messageType - Type of message to send
   * @param messageData - Optional data to send with the message (will be JSON serialized)
   * @returns Send result message with delivery count
   * @example
   * dispatcher.sendMessage("user.created", { userId: "123", email: "user@example.com" });
   */
  sendMessage(messageType: string, messageData?: any): string;
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
   * @param markdown - Markdown content to convert
   * @returns HTML string
   * @example
   * const html = convert.markdown_to_html("# Hello\n\nThis is **bold**");
   */
  markdown_to_html(markdown: string): string;

  /**
   * Render a Handlebars template with data
   * @param template - Handlebars template string
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
// ============================================================================

declare var routeRegistry: RouteRegistry;
declare var assetStorage: AssetStorage;
declare var scriptStorage: Storage;
declare var personalStorage: Storage;
declare var secretStorage: SecretStorage;
declare var schedulerService: SchedulerService;
declare var graphQLRegistry: GraphQLRegistry;
declare var mcpRegistry: McpRegistry;
declare var database: Database;
declare var console: Console;
declare var dispatcher: MessageDispatcher;
declare var convert: Convert;

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
