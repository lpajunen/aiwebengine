/// <reference path="./aiwebengine.d.ts" />

/**
 * TypeScript type definitions for aiwebengine privileged JavaScript API
 * @version 0.1.0
 *
 * These APIs are only available to scripts marked as privileged.
 * Add this reference to your privileged scripts for IDE autocomplete and type checking:
 * /// <reference path="https://your-engine.com/api/types/v0.1.0/aiwebengine-priv.d.ts" />
 */

// ============================================================================
// User Management API (Privileged Scripts Only)
// ============================================================================

/**
 * User object returned from user management functions
 */
interface User {
  /** User ID */
  id: string;

  /** User email */
  email: string;

  /** User display name */
  name: string;

  /** Array of role names (e.g., ["Authenticated", "Editor"]) */
  roles: string[];

  /** Array of OAuth provider names (e.g., ["google", "microsoft"]) */
  providers: string[];

  /** Account creation timestamp (SystemTime debug format) */
  created_at: string;
}

/**
 * User storage API for managing users and roles (admin-only, privileged scripts only)
 */
interface UserStorage {
  /**
   * List all users (requires admin privileges)
   * @returns JSON string array of user objects
   * @throws If user doesn't have administrator privileges
   * @example
   * const usersJson = userStorage.listUsers();
   * const users = JSON.parse(usersJson);
   * users.forEach(user => {
   *   console.log(`${user.email}: ${user.roles.join(', ')}`);
   * });
   */
  listUsers(): string;

  /**
   * Add a role to a user (requires admin privileges)
   * @param userId - User ID
   * @param role - Role name ("Editor", "Administrator", or "Authenticated")
   * @throws If user doesn't have administrator privileges or role is invalid
   * @example
   * userStorage.addUserRole("user123", "Editor");
   */
  addUserRole(userId: string, role: string): void;

  /**
   * Remove a role from a user (requires admin privileges)
   * @param userId - User ID
   * @param role - Role name ("Editor" or "Administrator", cannot remove "Authenticated")
   * @throws If user doesn't have administrator privileges, role is invalid, or attempting to remove "Authenticated" role
   * @example
   * userStorage.removeUserRole("user123", "Editor");
   */
  removeUserRole(userId: string, role: string): void;
}

/**
 * Secret storage API for checking secret availability (privileged scripts only, read-only)
 *
 * SECURITY: Secret values are NEVER exposed to JavaScript. Only existence checks
 * and identifier listing are allowed. Actual secret values are injected by Rust
 * into HTTP requests using the {{secret:identifier}} template syntax.
 */
interface SecretStorage {
  /**
   * Check if a secret exists
   * @param identifier - Secret identifier to check
   * @returns true if the secret exists, false otherwise
   * @example
   * if (secretStorage.exists("API_KEY")) {
   *   // Use {{secret:API_KEY}} in fetch headers
   *   const response = fetch(url, {
   *     headers: {
   *       "Authorization": "Bearer {{secret:API_KEY}}"
   *     }
   *   });
   * }
   */
  exists(identifier: string): boolean;

  /**
   * List all available secret identifiers
   * @returns Array of secret identifier strings
   * @example
   * const secrets = secretStorage.list();
   * console.log("Available secrets:", secrets.join(", "));
   */
  list(): string[];
}

// ============================================================================
// Scheduler Service API (Privileged Scripts Only)
// ============================================================================

/**
 * Scheduler service for managing scheduled tasks
 * Only available to privileged scripts
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
   * @param options.intervalMinutes - Interval in minutes (minimum 1)
   * @param options.name - Optional job name/key
   * @param options.startAt - Optional UTC ISO timestamp for first run
   * @returns Result message with job details
   * @example
   * schedulerService.registerRecurring({
   *   handler: "cleanupOldData",
   *   intervalMinutes: 60,
   *   name: "cleanup-job"
   * });
   */
  registerRecurring(options: {
    handler: string;
    intervalMinutes: number;
    name?: string;
    startAt?: string;
  }): string;

  /**
   * Clear all scheduled jobs for the current script
   * @returns Result message with count of cleared jobs
   * @example
   * schedulerService.clearAll();
   */
  clearAll(): string;
}

// ============================================================================
// Console API (Privileged Scripts Only)
// ============================================================================

/**
 * Console logging interface extensions for privileged scripts
 * Requires ViewLogs capability (admin-level access)
 */
interface Console {
  /**
   * List all log entries (requires ViewLogs capability)
   * @returns JSON string array of log entries
   * @example
   * const logs = JSON.parse(console.listLogs());
   * logs.forEach(log => {
   *   console.log(`${log.timestamp} [${log.level}] ${log.message}`);
   * });
   */
  listLogs(): string;

  /**
   * List log entries for a specific script URI (requires ViewLogs capability)
   * @param uri - Script URI to filter logs
   * @returns JSON string array of log entries
   * @example
   * const logs = JSON.parse(console.listLogsForUri("my-script"));
   * console.log(`Found ${logs.length} log entries for my-script`);
   */
  listLogsForUri(uri: string): string;

  /**
   * Prune old log entries (requires ViewLogs capability)
   * @returns Prune operation result message
   * @example
   * const result = console.pruneLogs();
   * console.log(result); // "Pruned 150 old log entries"
   */
  pruneLogs(): string;
}

// ============================================================================
// Asset Storage API (Privileged Scripts Only)
// ============================================================================

/**
 * AssetStorage interface extensions for privileged scripts
 * Requires ReadAssets/WriteAssets/DeleteAssets capabilities
 */
interface AssetStorage {
  /**
   * List assets for a specific script URI (requires ReadAssets capability)
   * @param uri - Script URI to list assets for
   * @returns JSON string array of asset metadata
   * @example
   * const assetsJson = assetStorage.listAssetsForUri("my-script");
   * const assets = JSON.parse(assetsJson);
   * assets.forEach(asset => {
   *   console.log(`${asset.name}: ${asset.size} bytes`);
   * });
   */
  listAssetsForUri(uri: string): string;

  /**
   * Fetch an asset from a specific script URI (requires ReadAssets capability)
   * @param uri - Script URI that owns the asset
   * @param name - Asset name/identifier
   * @returns Base64-encoded asset content or error message
   * @example
   * const content = assetStorage.fetchAssetForUri("my-script", "logo.svg");
   * if (!content.startsWith("Error:")) {
   *   const decoded = atob(content);
   *   console.log("Asset content:", decoded);
   * }
   */
  fetchAssetForUri(uri: string, name: string): string;

  /**
   * Create or update an asset for a specific script URI (requires WriteAssets capability)
   * @param uri - Script URI that will own the asset
   * @param name - Asset name/identifier
   * @param mimetype - MIME type (e.g., "image/svg+xml", "text/css")
   * @param contentBase64 - Base64-encoded asset content
   * @returns Success message or error message
   * @example
   * const base64Content = btoa("<svg>...</svg>");
   * assetStorage.upsertAssetForUri("my-script", "logo.svg", "image/svg+xml", base64Content);
   */
  upsertAssetForUri(
    uri: string,
    name: string,
    mimetype: string,
    contentBase64: string,
  ): string;

  /**
   * Delete an asset from a specific script URI (requires DeleteAssets capability)
   * @param uri - Script URI that owns the asset
   * @param name - Asset name/identifier to delete
   * @returns Success message or error message
   * @example
   * assetStorage.deleteAssetForUri("my-script", "old-logo.svg");
   */
  deleteAssetForUri(uri: string, name: string): string;
}

// ============================================================================
// Global Objects (Privileged Scripts Only)
// ============================================================================

declare var userStorage: UserStorage;
declare var secretStorage: SecretStorage;
declare var schedulerService: SchedulerService;
