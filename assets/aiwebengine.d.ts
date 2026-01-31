/// <reference lib="es2020" />

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
   * @param metadata - Optional OpenAPI metadata (summary, description, tags, parameters, requestBody)
   * @returns Registration result message
   * @example
   * routeRegistry.registerRoute("/api/users", "listUsers", "GET");
   * routeRegistry.registerRoute("/api/users", "createUser", "POST", {
   *   summary: "Create user",
   *   description: "Create a new user account",
   *   tags: ["Users"]
   * });
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
   * @returns Registration result message
   * @example
   * routeRegistry.registerStreamRoute("/events/notifications");
   * routeRegistry.registerStreamRoute("/events/chat", "chatCustomizer");
   */
  registerStreamRoute(path: string, customizationFunction?: string): string;

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
   * @returns Stored value or error message string
   * @example
   * const preferences = personalStorage.getItem("theme");
   */
  getItem(key: string): string;

  /**
   * Set a value in personal storage for the authenticated user
   * @param key - Storage key
   * @param value - Value to store
   * @returns Success or error message
   * @example
   * const result = personalStorage.setItem("theme", "dark");
   */
  setItem(key: string, value: string): string;

  /**
   * Remove a key from personal storage for the authenticated user
   * @param key - Storage key
   * @returns Success or error message
   * @example
   * const result = personalStorage.removeItem("oldPreference");
   */
  removeItem(key: string): string;

  /**
   * Clear all data from personal storage for the authenticated user
   * @returns Success or error message
   * @example
   * const result = personalStorage.clear();
   */
  clear(): string;
}

// ============================================================================
// Secret Storage API
// ============================================================================

/**
 * Secret storage API for checking secret availability (read-only)
 *
 * SECURITY: Secret values are NEVER exposed to JavaScript. Only existence checks
 * are allowed. Actual secret values are injected by Rust into HTTP requests
 * using the {{SECRET_NAME}} template syntax.
 */
interface SecretStorage {
  /**
   * Check if a secret exists
   * @param identifier - Secret identifier to check
   * @returns true if the secret exists, false otherwise
   * @example
   * if (secretStorage.exists("API_KEY")) {
   *   // Use {{API_KEY}} in fetch headers
   *   const response = fetch(url, {
   *     headers: {
   *       "Authorization": "Bearer {{API_KEY}}"
   *     }
   *   });
   * }
   */
  exists(identifier: string): boolean;
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

  // Transaction Management

  /**
   * Begin a new database transaction or create a savepoint if already in a transaction.
   * Transactions auto-commit on normal handler exit and auto-rollback on exceptions.
   * @param timeout_ms - Optional timeout in milliseconds (prevents long-running transactions)
   * @returns JSON string with result: {success: boolean, message: string} or {error: string}
   * @example
   * // Start transaction with 5 second timeout
   * const result = JSON.parse(database.beginTransaction(5000));
   * if (result.error) {
   *   console.error("Failed to start transaction:", result.error);
   *   return ResponseBuilder.error(500, "Transaction error");
   * }
   *
   * // Perform database operations...
   * // Transaction auto-commits on normal return or auto-rollbacks on exception
   */
  beginTransaction(timeout_ms?: number): string;

  /**
   * Commit the current transaction or release the most recent savepoint.
   * Note: Transactions auto-commit on handler success, so explicit commit is optional.
   * @returns JSON string with result: {success: boolean, message: string} or {error: string}
   * @example
   * const result = JSON.parse(database.commitTransaction());
   * if (result.error) {
   *   console.error("Failed to commit:", result.error);
   * }
   */
  commitTransaction(): string;

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
  rollbackTransaction(): string;

  /**
   * Create a named or auto-generated savepoint for nested transaction control.
   * Savepoints allow partial rollback within a transaction.
   * @param name - Optional savepoint name. If omitted, generates name like "sp_1", "sp_2", etc.
   * @returns JSON string with result: {success: boolean, savepoint: string} or {error: string}
   * @example
   * // Auto-generated savepoint
   * const sp = JSON.parse(database.createSavepoint());
   * console.log("Savepoint:", sp.savepoint); // "sp_1"
   *
   * // Named savepoint
   * database.createSavepoint("checkpoint_before_insert");
   */
  createSavepoint(name?: string): string;

  /**
   * Rollback to a specific savepoint without ending the transaction.
   * @param name - Savepoint name to rollback to
   * @returns JSON string with result: {success: boolean, message: string} or {error: string}
   * @example
   * const sp = JSON.parse(database.createSavepoint("before_update"));
   *
   * try {
   *   database.update("users", userId, JSON.stringify({status: "active"}));
   * } catch (error) {
   *   // Rollback just this update, keep other changes
   *   database.rollbackToSavepoint(sp.savepoint);
   * }
   */
  rollbackToSavepoint(name: string): string;

  /**
   * Release a savepoint, making its changes permanent within the transaction scope.
   * @param name - Savepoint name to release
   * @returns JSON string with result: {success: boolean, message: string} or {error: string}
   * @example
   * const sp = JSON.parse(database.createSavepoint("checkpoint"));
   *
   * // Perform operations...
   *
   * // Release savepoint (changes become permanent in transaction)
   * database.releaseSavepoint(sp.savepoint);
   */
  releaseSavepoint(name: string): string;

  /**
   * Check database health status
   * @returns Health status message
   * @example
   * const health = database.checkDatabaseHealth();
   * console.log(health);
   */
  checkDatabaseHealth(): string;
}

// ============================================================================
// Console API
// ============================================================================

/**
 * Console logging interface
 * Note: Privileged scripts have additional methods available (listLogs, listLogsForUri, pruneLogs)
 * defined in aiwebengine-priv.d.ts
 */
interface Console {
  /**
   * Write a log message
   * @param message - Message to log (multiple arguments will be concatenated)
   * @param optionalParams - Additional parameters to log
   * @example
   * console.log("Request received:", req.path);
   * console.log("User:", user.id, user.name);
   */
  log(message?: any, ...optionalParams: any[]): void;

  /**
   * Write an info log message
   * @param message - Info message to log
   * @param optionalParams - Additional parameters to log
   * @example
   * console.info("User logged in:", userId);
   */
  info(message?: any, ...optionalParams: any[]): void;

  /**
   * Write a warning log message
   * @param message - Warning message to log
   * @param optionalParams - Additional parameters to log
   * @example
   * console.warn("Deprecated API usage detected:", apiName);
   */
  warn(message?: any, ...optionalParams: any[]): void;

  /**
   * Write an error log message
   * @param message - Error message to log
   * @param optionalParams - Additional parameters to log
   * @example
   * console.error("Failed to process request:", error);
   */
  error(message?: any, ...optionalParams: any[]): void;

  /**
   * Write a debug log message
   * @param message - Debug message to log
   * @param optionalParams - Additional parameters to log
   * @example
   * console.debug("Processing item:", item.id);
   */
  debug(message?: any, ...optionalParams: any[]): void;
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
}

// ============================================================================
// Global Objects
// ============================================================================

declare var routeRegistry: RouteRegistry;
declare var assetStorage: AssetStorage;
declare var sharedStorage: SharedStorage;
declare var personalStorage: PersonalStorage;
declare var secretStorage: SecretStorage;
declare var graphQLRegistry: GraphQLRegistry;
declare var mcpRegistry: McpRegistry;
declare var database: Database;
declare var console: Console;
declare var dispatcher: MessageDispatcher;
declare var convert: Convert;

/**
 * Base64 encode a string
 * @param data - String to encode
 * @returns Base64-encoded string
 * @example
 * const encoded = btoa("Hello World");
 */
declare function btoa(data: string): string;

/**
 * Base64 decode a string
 * @param data - Base64-encoded string to decode
 * @returns Decoded string
 * @example
 * const decoded = atob(encoded);
 */
declare function atob(data: string): string;

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
 * JSX namespace for TypeScript JSX type checking
 */
declare namespace JSX {
  /**
   * JSX elements are rendered as HTML strings
   */
  type Element = string;

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
