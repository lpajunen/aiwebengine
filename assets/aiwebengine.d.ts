/**
 * TypeScript type definitions for aiwebengine JavaScript API
 * @version 0.1.0
 *
 * Add this reference to your scripts for IDE autocomplete and type checking:
 * /// <reference path="https://your-engine.com/api/types/v0.1.0/aiwebengine.d.ts" />
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
 *   graphQLRegistry.registerQuery("getUser", "getUser(id: ID!): User", "getUserResolver");
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

  /** Request headers as key-value pairs */
  headers: Record<string, string>;

  /** URL query parameters as key-value pairs */
  query: Record<string, string>;

  /** Route parameters from path patterns (e.g., {id: "123"}) */
  params: Record<string, string>;

  /** Form data from POST requests as key-value pairs */
  form: Record<string, string>;

  /** Raw request body as string */
  body: string;

  /** Authentication context (available when user is authenticated) */
  auth?: AuthContext;
}

/**
 * Authentication context available in request.auth when user is authenticated
 */
interface AuthContext {
  /** User ID */
  userId: string;

  /** User email address */
  email: string;

  /** User display name */
  name: string;

  /** Authentication provider (google, microsoft, apple) */
  provider: string;

  /** Whether user has admin privileges */
  isAdmin: boolean;
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

/**
 * Context object passed to all handler functions
 */
interface HandlerContext {
  /** HTTP request information (for HTTP route handlers) */
  request?: HttpRequest;

  /** GraphQL or function arguments (for GraphQL resolvers) */
  args?: Record<string, any>;

  /** Handler invocation type */
  invocationType?:
    | "httpRoute"
    | "graphqlQuery"
    | "graphqlMutation"
    | "graphqlSubscription"
    | "scheduledJob"
    | "mcpTool";

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
   * @param method - HTTP method (GET, POST, PUT, DELETE, etc.)
   * @returns Registration result message
   * @example
   * routeRegistry.registerRoute("/api/users", "listUsers", "GET");
   */
  registerRoute(path: string, handlerName: string, method: string): string;

  /**
   * Register a Server-Sent Events (SSE) stream endpoint
   * @param path - URL path for the stream (must start with /)
   * @returns Registration result message
   * @example
   * routeRegistry.registerStreamRoute("/events/notifications");
   */
  registerStreamRoute(path: string): string;

  /**
   * Register a static asset route
   * @param httpPath - HTTP path where asset will be served (e.g., "/styles/main.css")
   * @param assetName - Name of the asset in the asset storage (e.g., "main.css")
   * @returns Registration result message
   * @example
   * routeRegistry.registerAssetRoute("/styles/main.css", "main.css");
   */
  registerAssetRoute(httpPath: string, assetName: string): string;

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
   * @returns Broadcast result message
   * @example
   * routeRegistry.sendStreamMessageFiltered(
   *   "/events/notifications",
   *   { message: "Admin alert" },
   *   JSON.stringify({ role: "admin" })
   * );
   */
  sendStreamMessageFiltered(
    path: string,
    data: any,
    filterJson: string,
  ): string;

  /**
   * List all registered routes
   * @returns JSON string array of registered routes
   * @example
   * const routes = JSON.parse(routeRegistry.listRoutes());
   */
  listRoutes(): string;

  /**
   * List all registered streams
   * @returns JSON string array of registered streams
   * @example
   * const streams = JSON.parse(routeRegistry.listStreams());
   */
  listStreams(): string;
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
 * Asset storage for managing static files
 */
interface AssetStorage {
  /**
   * List all assets with metadata
   * @returns JSON string array of asset metadata
   * @example
   * const assetsJson = assetStorage.listAssets();
   * const assets = JSON.parse(assetsJson);
   */
  listAssets(): string;

  /**
   * Fetch an asset's content
   * @param name - Asset name/URI
   * @returns Base64-encoded asset content or error message
   * @example
   * const content = assetStorage.fetchAsset("logo.svg");
   */
  fetchAsset(name: string): string;

  /**
   * Create or update an asset
   * @param name - Asset name/URI
   * @param mimetype - MIME type (e.g., "image/png", "text/css")
   * @param contentBase64 - Base64-encoded content
   * @returns Operation result message
   * @example
   * assetStorage.upsertAsset("logo.svg", "image/svg+xml", base64Content);
   */
  upsertAsset(name: string, mimetype: string, contentBase64: string): string;

  /**
   * Delete an asset
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
 * Shared storage (script-scoped, persistent key-value store)
 */
interface SharedStorage {
  /**
   * Get a value from shared storage
   * @param key - Storage key
   * @returns Stored value or null if not found
   * @example
   * const counter = sharedStorage.getItem("pageViews") || "0";
   */
  getItem(key: string): string | null;

  /**
   * Set a value in shared storage
   * @param key - Storage key
   * @param value - Value to store
   * @example
   * sharedStorage.setItem("pageViews", "42");
   */
  setItem(key: string, value: string): void;

  /**
   * Remove a key from shared storage
   * @param key - Storage key
   * @example
   * sharedStorage.removeItem("oldData");
   */
  removeItem(key: string): void;

  /**
   * Clear all data from shared storage
   * @example
   * sharedStorage.clear();
   */
  clear(): void;
}

/**
 * Personal storage (user-scoped, requires authentication)
 */
interface PersonalStorage {
  /**
   * Get a value from personal storage for the authenticated user
   * @param key - Storage key
   * @returns Stored value or null if not found
   * @throws If user is not authenticated
   * @example
   * const preferences = personalStorage.getItem("theme") || "light";
   */
  getItem(key: string): string | null;

  /**
   * Set a value in personal storage for the authenticated user
   * @param key - Storage key
   * @param value - Value to store
   * @throws If user is not authenticated
   * @example
   * personalStorage.setItem("theme", "dark");
   */
  setItem(key: string, value: string): void;

  /**
   * Remove a key from personal storage for the authenticated user
   * @param key - Storage key
   * @throws If user is not authenticated
   * @example
   * personalStorage.removeItem("oldPreference");
   */
  removeItem(key: string): void;

  /**
   * Clear all data from personal storage for the authenticated user
   * @throws If user is not authenticated
   * @example
   * personalStorage.clear();
   */
  clear(): void;
}

/**
 * Script metadata
 */
interface ScriptMetadata {
  /** Script URI */
  uri: string;

  /** Script name */
  name: string;

  /** Script size in bytes */
  size: number;

  /** Created timestamp (milliseconds since epoch) */
  createdAt: number;

  /** Updated timestamp (milliseconds since epoch) */
  updatedAt: number;

  /** Whether script has privileged access */
  privileged: boolean;

  /** Whether script has been initialized */
  initialized: boolean;

  /** Initialization error message if any */
  initError?: string;
}

/**
 * Script storage for managing JavaScript scripts
 */
interface ScriptStorage {
  /**
   * List all scripts with metadata
   * @returns JSON string array of script metadata
   * @example
   * const scripts = JSON.parse(scriptStorage.listScripts());
   */
  listScripts(): string;

  /**
   * Get script content by name
   * @param scriptName - Script name/URI
   * @returns Script content or null if not found
   * @example
   * const content = scriptStorage.getScript("my-script");
   */
  getScript(scriptName: string): string | null;

  /**
   * Get script initialization status
   * @param scriptName - Script name/URI
   * @returns JSON string with init status or null
   * @example
   * const status = JSON.parse(scriptStorage.getScriptInitStatus("my-script"));
   */
  getScriptInitStatus(scriptName: string): string | null;

  /**
   * Get script security profile
   * @param scriptName - Script name/URI
   * @returns JSON string with security profile or null
   * @example
   * const profile = JSON.parse(scriptStorage.getScriptSecurityProfile("my-script"));
   */
  getScriptSecurityProfile(scriptName: string): string | null;

  /**
   * Create or update a script
   * @param scriptName - Script name/URI
   * @param content - Script content
   * @returns Result message
   * @example
   * scriptStorage.upsertScript("my-script", "function init() { ... }");
   */
  upsertScript(scriptName: string, content: string): string;

  /**
   * Delete a script (requires ownership or admin privileges)
   * @param scriptName - Script name/URI
   * @returns True if deleted, false if failed
   * @example
   * scriptStorage.deleteScript("old-script");
   */
  deleteScript(scriptName: string): boolean;

  /**
   * Set privileged status for a script (admin only)
   * @param scriptName - Script name/URI
   * @param privileged - Whether script should be privileged
   * @returns True if successful
   * @example
   * scriptStorage.setScriptPrivileged("system-script", true);
   */
  setScriptPrivileged(scriptName: string, privileged: boolean): boolean;

  /**
   * Check if current user can manage script privileges
   * @returns True if user has admin capability
   * @example
   * if (scriptStorage.canManageScriptPrivileges()) { ... }
   */
  canManageScriptPrivileges(): boolean;

  /**
   * Get list of owner user IDs for a script
   * @param scriptName - Script name/URI
   * @returns JSON string array of owner user IDs
   * @example
   * const owners = JSON.parse(scriptStorage.getScriptOwners("my-script"));
   */
  getScriptOwners(scriptName: string): string;

  /**
   * Add an owner to a script (requires current ownership or admin)
   * @param scriptName - Script name/URI
   * @param userId - User ID to add as owner
   * @returns Result message
   * @example
   * scriptStorage.addScriptOwner("my-script", "user123");
   */
  addScriptOwner(scriptName: string, userId: string): string;

  /**
   * Remove an owner from a script (requires current ownership or admin)
   * @param scriptName - Script name/URI
   * @param userId - User ID to remove
   * @returns Result message
   * @example
   * scriptStorage.removeScriptOwner("my-script", "user123");
   */
  removeScriptOwner(scriptName: string, userId: string): string;
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
   * @returns Send result message
   * @example
   * graphQLRegistry.sendSubscriptionMessageFiltered(
   *   "messageAdded",
   *   JSON.stringify({ id: "123", text: "Admin message" }),
   *   JSON.stringify({ role: "admin" })
   * );
   */
  sendSubscriptionMessageFiltered(
    subscriptionName: string,
    message: string,
    filterJson?: string,
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
   */
  registerPrompt(
    name: string,
    description: string,
    argumentsJson: string,
    handlerFunction: string,
  ): string;
}

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

  /** Timeout in milliseconds (default: 30000) */
  timeout?: number;
}

/**
 * Fetch response
 */
interface FetchResponse {
  /** HTTP status code */
  status: number;

  /** Response body as string */
  body: string;

  /** Response headers */
  headers: Record<string, string>;
}

/**
 * HTTP client with secret injection support
 * @param url - URL to fetch (supports {{SECRET_NAME}} syntax for secret injection)
 * @param options - Fetch options
 * @returns Fetch response as JSON string
 * @example
 * // Simple GET request
 * const response = fetch("https://api.example.com/data");
 * const data = JSON.parse(response);
 *
 * // POST with secret injection
 * const response = fetch("https://api.example.com/endpoint", {
 *   method: "POST",
 *   headers: {
 *     "Authorization": "Bearer {{API_TOKEN}}",
 *     "Content-Type": "application/json"
 *   },
 *   body: JSON.stringify({ key: "value" })
 * });
 */
declare function fetch(url: string, options?: FetchOptions): string;

// ============================================================================
// Database API (Script-Scoped Table Management)
// ============================================================================

/**
 * Database interface for script-scoped table management and operations.
 * Each script can create and manage its own tables with automatic namespacing.
 */
interface Database {
  /**
   * Create a new table for this script
   * @param tableName - Logical table name (will be prefixed with script namespace)
   * @returns JSON string with result: {success: boolean, tableName: string, physicalName: string} or {error: string}
   * @example
   * const result = JSON.parse(database.createTable("users"));
   * // {success: true, tableName: "users", physicalName: "script_myapp_users"}
   */
  createTable(tableName: string): string;

  /**
   * Drop a table owned by this script
   * @param tableName - Table name to drop
   * @returns JSON string with result: {success: boolean, tableName: string, dropped: boolean} or {error: string}
   * @example
   * const result = JSON.parse(database.dropTable("old_data"));
   */
  dropTable(tableName: string): string;

  /**
   * Add an integer column to a table
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
  ): string;

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
  ): string;

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
  ): string;

  /**
   * Add a timestamp column to a table
   * @param tableName - Table name
   * @param columnName - Column name
   * @param nullable - Whether column can be NULL (default: true)
   * @param defaultValue - Default value (optional, e.g., "CURRENT_TIMESTAMP")
   * @returns JSON string with result: {success: boolean, column: string} or {error: string}
   * @example
   * database.addTimestampColumn("posts", "created_at", false, "CURRENT_TIMESTAMP");
   */
  addTimestampColumn(
    tableName: string,
    columnName: string,
    nullable?: boolean,
    defaultValue?: string,
  ): string;

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
  ): string;

  /**
   * Drop a column from a table
   * @param tableName - Table name
   * @param columnName - Column name
   * @returns JSON string with result: {success: boolean, tableName: string, columnName: string, dropped: boolean} or {error: string}
   * @example
   * const result = JSON.parse(database.dropColumn("users", "old_field"));
   */
  dropColumn(tableName: string, columnName: string): string;

  /**
   * Query rows from a table with optional filters and limit
   * @param tableName - Table name
   * @param filters - JSON string with filter conditions (optional)
   * @param limit - Maximum number of rows to return (optional)
   * @returns JSON string array of matching rows or {error: string}
   * @example
   * const users = JSON.parse(database.query("users", JSON.stringify({active: true}), 10));
   * const allPosts = JSON.parse(database.query("posts"));
   */
  query(tableName: string, filters?: string, limit?: number): string;

  /**
   * Insert a row into a table
   * @param tableName - Table name
   * @param data - JSON string with column values
   * @returns JSON string with inserted row (including generated id) or {error: string}
   * @example
   * const result = JSON.parse(
   *   database.insert("users", JSON.stringify({name: "John", email: "john@example.com"}))
   * );
   */
  insert(tableName: string, data: string): string;

  /**
   * Update a row in a table by ID
   * @param tableName - Table name
   * @param id - Row ID
   * @param data - JSON string with column values to update
   * @returns JSON string with updated row or {error: string}
   * @example
   * const result = JSON.parse(
   *   database.update("users", 1, JSON.stringify({name: "Jane"}))
   * );
   */
  update(tableName: string, id: number, data: string): string;

  /**
   * Delete a row from a table by ID
   * @param tableName - Table name
   * @param id - Row ID
   * @returns JSON string with result: {success: boolean, deleted: boolean} or {error: string}
   * @example
   * const result = JSON.parse(database.delete("users", 5));
   */
  delete(tableName: string, id: number): string;

  /**
   * Auto-generate GraphQL operations for a table
   * @param tableName - Table name
   * @param options - JSON string with options (optional): {visibility: "script_internal" | "public" | "authenticated"}
   * @returns JSON string with result: {success: boolean, table: string, queries: string[], mutations: string[]} or {error: string}
   * @example
   * const result = JSON.parse(
   *   database.generateGraphQLForTable("users", JSON.stringify({visibility: "authenticated"}))
   * );
   * // Automatically creates queries like: getUser, listUsers
   * // And mutations like: createUser, updateUser, deleteUser
   */
  generateGraphQLForTable(tableName: string, options?: string): string;
}

// ============================================================================
// Console API
// ============================================================================

/**
 * Console logging interface
 */
interface Console {
  /**
   * Write a log message
   * @param message - Message to log
   * @example
   * console.log("Request received: " + req.path);
   */
  log(message: string): void;

  /**
   * Write an info log message
   * @param message - Info message to log
   * @example
   * console.info("User logged in");
   */
  info(message: string): void;

  /**
   * Write a warning log message
   * @param message - Warning message to log
   * @example
   * console.warn("Deprecated API usage detected");
   */
  warn(message: string): void;

  /**
   * Write an error log message
   * @param message - Error message to log
   * @example
   * console.error("Failed to process request: " + error);
   */
  error(message: string): void;

  /**
   * Write a debug log message
   * @param message - Debug message to log
   * @example
   * console.debug("Processing item: " + item.id);
   */
  debug(message: string): void;

  /**
   * List all log entries
   * @returns JSON string array of log entries
   * @example
   * const logs = JSON.parse(console.listLogs());
   */
  listLogs(): string;

  /**
   * List log entries for a specific script URI
   * @param uri - Script URI to filter logs
   * @returns JSON string array of log entries
   * @example
   * const logs = JSON.parse(console.listLogsForUri("my-script"));
   */
  listLogsForUri(uri: string): string;

  /**
   * Prune old log entries
   * @returns Prune operation result message
   * @example
   * console.pruneLogs();
   */
  pruneLogs(): string;
}

// ============================================================================
// Global Objects
// ============================================================================

declare var routeRegistry: RouteRegistry;
declare var assetStorage: AssetStorage;
declare var sharedStorage: SharedStorage;
declare var personalStorage: PersonalStorage;
declare var scriptStorage: ScriptStorage;
declare var graphQLRegistry: GraphQLRegistry;
declare var mcpRegistry: McpRegistry;
declare var database: Database;
declare var console: Console;

/**
 * Check database health status
 * @returns Health status message
 * @example
 * const health = checkDatabaseHealth();
 * console.log(health);
 */
declare function checkDatabaseHealth(): string;

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
