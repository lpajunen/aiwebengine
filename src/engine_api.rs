//! Native engine management API for scripts and assets.
//!
//! These REST routes and MCP tools were previously implemented by the
//! bootstrapped `core.js` and `cli.js` scripts. They are engine
//! functionality, so they live in Rust: the HTTP contract and MCP tool
//! names/shapes are kept identical to the script implementations they
//! replace. Authorization mirrors the checks that `secure_globals`
//! applies to the equivalent `scriptStorage`/`assetStorage` functions.

use axum::extract::{Extension, Query};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use base64::Engine as _;
use serde::Deserialize;
use serde_json::{Value, json};
use tracing::{debug, warn};

use crate::auth::AuthUser;
use crate::repository;
use crate::security::{
    Capability, SecurityAuditor, SecurityEvent, SecurityEventType, SecuritySeverity, UserContext,
};

/// Maximum asset size accepted by the write paths (same limit as the sandbox).
const MAX_ASSET_BYTES: usize = 10 * 1024 * 1024;

fn iso_timestamp() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

fn user_context_from(auth_user: Option<&AuthUser>) -> UserContext {
    match auth_user {
        Some(user) if user.is_admin => UserContext::admin(user.user_id.clone()),
        Some(user) => UserContext::authenticated(user.user_id.clone()),
        None => UserContext::anonymous(),
    }
}

fn auditor() -> SecurityAuditor {
    let pool = crate::database::get_global_database().map(|db| db.pool().clone());
    SecurityAuditor::new(pool)
}

fn user_owns_script(user: &UserContext, script_uri: &str) -> bool {
    match &user.user_id {
        Some(user_id) => repository::user_owns_script(script_uri, user_id).unwrap_or(false),
        None => false,
    }
}

/// Broadcast a script update to the `/script_updates` stream, matching the
/// message format core.js used. Extra `details` entries become message
/// metadata used for connection filtering.
pub fn broadcast_script_update(uri: &str, action: &str, details: &[(&str, Value)]) {
    let mut message = json!({
        "type": "script_update",
        "uri": uri,
        "action": action,
        "timestamp": iso_timestamp(),
    });
    if let Some(obj) = message.as_object_mut() {
        for (key, value) in details {
            obj.insert((*key).to_string(), value.clone());
        }
    }

    match crate::stream_registry::GLOBAL_STREAM_REGISTRY
        .broadcast_to_stream("/script_updates", &message.to_string())
    {
        Ok(_) => debug!("Broadcasted script update: {} {}", action, uri),
        Err(e) => warn!("Failed to broadcast script update for {}: {}", uri, e),
    }
}

/// Re-initialize a script in the background after an upsert: clear its
/// GraphQL/MCP registrations, run init(), and rebuild the GraphQL schema.
/// Mirrors the behavior of the sandboxed `scriptStorage.upsertScript`.
fn spawn_script_init(script_uri: String) {
    tokio::task::spawn(async move {
        crate::graphql::clear_script_graphql_registrations(&script_uri);
        crate::mcp::clear_script_mcp_registrations(&script_uri);

        let initializer = crate::script_init::ScriptInitializer::with_configured_timeout();
        match initializer.initialize_script(&script_uri, false).await {
            Ok(result) => {
                if result.success {
                    if let Err(e) = crate::graphql::rebuild_schema() {
                        warn!(
                            "Failed to rebuild GraphQL schema after script '{}' initialization: {:?}",
                            script_uri, e
                        );
                    }
                } else if let Some(err) = result.error {
                    warn!("Script '{}' init failed after upsert: {}", script_uri, err);
                }
            }
            Err(e) => {
                warn!(
                    "Failed to initialize script '{}' after upsert: {}",
                    script_uri, e
                );
            }
        }
    });
}

// ============================================================================
// Authorized core operations (shared by the REST routes and MCP tools)
// ============================================================================

/// Outcome of an authorized script upsert.
pub enum UpsertAction {
    Inserted,
    Updated,
}

impl UpsertAction {
    fn as_str(&self) -> &'static str {
        match self {
            UpsertAction::Inserted => "inserted",
            UpsertAction::Updated => "updated",
        }
    }
}

/// Create or update a script with the same authorization semantics as
/// `scriptStorage.upsertScript`: WriteScripts capability required, and
/// existing scripts can only be modified by an admin or an owner.
/// Broadcasts the update and re-initializes the script on success.
pub fn upsert_script_authorized(
    user: &UserContext,
    uri: &str,
    content: &str,
    via: Option<&str>,
) -> Result<UpsertAction, String> {
    if let Err(e) = user.require_capability(&Capability::WriteScripts) {
        return Err(format!("Error: {}", e));
    }
    if uri.is_empty() || content.is_empty() {
        return Err("Error: Script name and content cannot be empty".to_string());
    }

    let exists = repository::fetch_script(uri).is_some();
    if exists {
        let is_admin = user.has_capability(&Capability::DeleteScripts);
        if !is_admin && !user_owns_script(user, uri) {
            warn!(
                user_id = ?user.user_id,
                script_name = %uri,
                "Permission denied: user is neither admin nor owner"
            );
            return Err(format!(
                "Error: Permission denied. You must be an administrator or owner to modify script '{}'",
                uri
            ));
        }
    }

    if let Err(e) = repository::upsert_script_with_owner(uri, content, user.user_id.as_deref()) {
        return Err(format!("Error storing script: {}", e));
    }

    spawn_script_init(uri.to_string());

    let action = if exists {
        UpsertAction::Updated
    } else {
        UpsertAction::Inserted
    };
    let mut details = vec![
        ("contentLength", json!(content.len())),
        ("previousExists", json!(exists)),
    ];
    if let Some(via) = via {
        details.push(("via", json!(via)));
    }
    broadcast_script_update(uri, action.as_str(), &details);

    Ok(action)
}

/// Delete a script with the same semantics as `scriptStorage.deleteScript`:
/// DeleteScripts capability required. Returns false when the capability is
/// missing or the script does not exist. Broadcasts the removal on success.
pub fn delete_script_authorized(user: &UserContext, uri: &str, via: Option<&str>) -> bool {
    if let Err(e) = user.require_capability(&Capability::DeleteScripts) {
        let auditor = auditor();
        let user_id = user.user_id.clone();
        tokio::task::spawn(async move {
            let _ = auditor
                .log_authz_failure(
                    user_id,
                    "script".to_string(),
                    "delete".to_string(),
                    "DeleteScripts".to_string(),
                )
                .await;
        });
        warn!(
            user_id = ?user.user_id,
            script_name = %uri,
            error = %e,
            "deleteScript capability check failed"
        );
        return false;
    }

    let auditor = auditor();
    let user_id = user.user_id.clone();
    let uri_owned = uri.to_string();
    tokio::task::spawn(async move {
        let _ = auditor
            .log_event(
                SecurityEvent::new(
                    SecurityEventType::SystemSecurityEvent,
                    SecuritySeverity::High,
                    user_id,
                )
                .with_resource("script".to_string())
                .with_action("delete".to_string())
                .with_detail("script_name", &uri_owned),
            )
            .await;
    });

    let deleted = repository::delete_script(uri);
    if deleted {
        let details: Vec<(&str, Value)> = via.map(|v| ("via", json!(v))).into_iter().collect();
        broadcast_script_update(uri, "removed", &details);
    }
    deleted
}

/// Fetch script content; ReadScripts capability required (None otherwise).
pub fn get_script_authorized(user: &UserContext, uri: &str) -> Option<String> {
    if user.require_capability(&Capability::ReadScripts).is_err() {
        return None;
    }
    repository::fetch_script(uri)
}

/// List script metadata; ReadScripts capability required (empty otherwise).
pub fn list_scripts_authorized(user: &UserContext) -> Vec<repository::ScriptMetadata> {
    if user.require_capability(&Capability::ReadScripts).is_err() {
        return Vec::new();
    }
    repository::get_all_script_metadata().unwrap_or_default()
}

/// Fetch logs for a script as JSON objects; ViewLogs capability required
/// (empty otherwise) — same as `console.listLogsForUri`.
pub fn logs_authorized(user: &UserContext, uri: &str) -> Vec<Value> {
    if user.require_capability(&Capability::ViewLogs).is_err() {
        return Vec::new();
    }
    repository::fetch_log_messages(uri)
        .iter()
        .map(|entry| {
            let timestamp_ms = entry
                .timestamp
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as f64;
            json!({
                "message": entry.message,
                "level": entry.level,
                "timestamp": timestamp_ms
            })
        })
        .collect()
}

/// Init status for one script; ReadScripts capability required.
pub fn init_status_authorized(user: &UserContext, uri: &str) -> Option<Value> {
    if user.require_capability(&Capability::ReadScripts).is_err() {
        return None;
    }
    let metadata = repository::get_script_metadata(uri).ok()?;
    Some(script_init_status_json(&metadata))
}

fn script_init_status_json(metadata: &repository::ScriptMetadata) -> Value {
    let millis = |t: std::time::SystemTime| {
        t.duration_since(std::time::UNIX_EPOCH)
            .ok()
            .map(|d| d.as_millis() as f64)
    };
    json!({
        "scriptName": metadata.uri,
        "initialized": metadata.initialized,
        "initError": metadata.init_error,
        "lastInitTime": metadata.last_init_time.and_then(millis),
        "createdAt": millis(metadata.created_at),
        "updatedAt": millis(metadata.updated_at),
    })
}

/// Whether the user may access assets of `script_uri` given the per-operation
/// capability: capability holders, script owners, and admins all qualify —
/// same rule as the `assetStorage.*ForUri` functions.
fn can_access_assets(user: &UserContext, script_uri: &str, capability: &Capability) -> bool {
    user.has_capability(capability)
        || user.has_capability(&Capability::DeleteScripts)
        || user_owns_script(user, script_uri)
}

/// List asset metadata for a script (empty when access is denied).
pub fn list_assets_authorized(user: &UserContext, script_uri: &str) -> Vec<Value> {
    if !can_access_assets(user, script_uri, &Capability::ReadAssets) {
        return Vec::new();
    }
    let millis = |t: std::time::SystemTime| {
        t.duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as f64
    };
    repository::fetch_assets(script_uri)
        .values()
        .map(|asset| {
            json!({
                "uri": asset.uri,
                "name": asset.name,
                "size": asset.content.len(),
                "mimetype": asset.mimetype,
                "createdAt": millis(asset.created_at),
                "updatedAt": millis(asset.updated_at),
            })
        })
        .collect()
}

pub enum AssetFetchError {
    AccessDenied,
    NotFound,
}

/// Fetch an asset's content as base64 (same transfer encoding the sandbox
/// used, which the /assets and MCP contracts expose).
pub fn fetch_asset_authorized(
    user: &UserContext,
    script_uri: &str,
    asset_uri: &str,
) -> Result<String, AssetFetchError> {
    if !can_access_assets(user, script_uri, &Capability::ReadAssets) {
        return Err(AssetFetchError::AccessDenied);
    }
    match repository::fetch_asset(script_uri, asset_uri) {
        Some(asset) => Ok(base64::engine::general_purpose::STANDARD.encode(asset.content)),
        None => Err(AssetFetchError::NotFound),
    }
}

pub enum AssetWriteError {
    AccessDenied,
    Validation(String),
    Storage(String),
}

/// Create or update an asset from base64 content.
pub fn upsert_asset_authorized(
    user: &UserContext,
    script_uri: &str,
    asset_uri: &str,
    mimetype: &str,
    content_b64: &str,
) -> Result<(), AssetWriteError> {
    let content = base64::engine::general_purpose::STANDARD
        .decode(content_b64)
        .map_err(|e| {
            AssetWriteError::Validation(format!("Error decoding base64 content: {}", e))
        })?;

    if !can_access_assets(user, script_uri, &Capability::WriteAssets) {
        return Err(AssetWriteError::AccessDenied);
    }

    if asset_uri.is_empty() || asset_uri.len() > 255 {
        return Err(AssetWriteError::Validation(
            "Invalid asset URI: must be 1-255 characters".to_string(),
        ));
    }
    if asset_uri.contains("..") || asset_uri.contains('\\') {
        return Err(AssetWriteError::Validation(
            "Invalid asset URI: path traversal not allowed".to_string(),
        ));
    }
    if content.len() > MAX_ASSET_BYTES {
        return Err(AssetWriteError::Validation(
            "Asset too large (max 10MB)".to_string(),
        ));
    }

    let auditor = auditor();
    let user_id = user.user_id.clone();
    let script_uri_owned = script_uri.to_string();
    let asset_uri_owned = asset_uri.to_string();
    let content_len = content.len();
    let mimetype_owned = mimetype.to_string();
    tokio::task::spawn(async move {
        let _ = auditor
            .log_event(
                SecurityEvent::new(
                    SecurityEventType::SystemSecurityEvent,
                    SecuritySeverity::Medium,
                    user_id,
                )
                .with_resource("asset".to_string())
                .with_action("upsert_for_uri".to_string())
                .with_detail("uri", &asset_uri_owned)
                .with_detail("script_uri", &script_uri_owned)
                .with_detail("content_size", content_len.to_string())
                .with_detail("mimetype", &mimetype_owned),
            )
            .await;
    });

    let now = std::time::SystemTime::now();
    let asset = repository::Asset {
        uri: asset_uri.to_string(),
        name: Some(asset_uri.to_string()),
        mimetype: mimetype.to_string(),
        content,
        created_at: now,
        updated_at: now,
        script_uri: script_uri.to_string(),
    };
    repository::upsert_asset(asset)
        .map_err(|e| AssetWriteError::Storage(format!("Error upserting asset: {}", e)))
}

/// Delete an asset.
pub fn delete_asset_authorized(
    user: &UserContext,
    script_uri: &str,
    asset_uri: &str,
) -> Result<bool, AssetFetchError> {
    if !can_access_assets(user, script_uri, &Capability::DeleteAssets) {
        let auditor = auditor();
        let user_id = user.user_id.clone();
        tokio::task::spawn(async move {
            let _ = auditor
                .log_authz_failure(
                    user_id,
                    "asset".to_string(),
                    "delete_for_uri".to_string(),
                    "DeleteAssets".to_string(),
                )
                .await;
        });
        return Err(AssetFetchError::AccessDenied);
    }

    let auditor = auditor();
    let user_id = user.user_id.clone();
    let script_uri_owned = script_uri.to_string();
    let asset_uri_owned = asset_uri.to_string();
    tokio::task::spawn(async move {
        let _ = auditor
            .log_event(
                SecurityEvent::new(
                    SecurityEventType::SystemSecurityEvent,
                    SecuritySeverity::High,
                    user_id,
                )
                .with_resource("asset".to_string())
                .with_action("delete_for_uri".to_string())
                .with_detail("uri", &asset_uri_owned)
                .with_detail("script_uri", &script_uri_owned),
            )
            .await;
    });

    Ok(repository::delete_asset(script_uri, asset_uri))
}

// ============================================================================
// OpenAPI spec generation
// ============================================================================

/// Generate the full OpenAPI spec: the Rust (utoipa) spec merged with
/// script-registered routes, asset routes, and SSE stream routes. Returns
/// the same `{"error": ...}` JSON strings as the former JS implementation
/// when a step fails, so callers can pass the result through unchanged.
pub fn generate_merged_openapi_spec() -> String {
    let rust_spec_str = crate::get_rust_openapi_spec();
    let mut rust_spec: Value = match serde_json::from_str(&rust_spec_str) {
        Ok(spec) => spec,
        Err(e) => {
            return format!(
                "{{\"error\": \"Failed to parse Rust OpenAPI spec: {}\"}}",
                e
            );
        }
    };

    let metadata_list = match repository::get_all_script_metadata() {
        Ok(list) => list,
        Err(e) => {
            return format!(
                "{{\"error\": \"Failed to fetch JavaScript routes: {}\"}}",
                e
            );
        }
    };

    let mut js_paths = serde_json::Map::new();

    // Script-registered HTTP routes
    for metadata in metadata_list {
        if metadata.initialized && !metadata.registrations.is_empty() {
            for ((path, method), route_meta) in metadata.registrations {
                let path_item = js_paths.entry(path.clone()).or_insert_with(|| json!({}));
                let Some(path_obj) = path_item.as_object_mut() else {
                    continue;
                };

                let mut operation = serde_json::Map::new();
                operation.insert(
                    "summary".to_string(),
                    json!(
                        route_meta
                            .summary
                            .unwrap_or_else(|| format!("{} {}", method, path))
                    ),
                );
                if let Some(desc) = route_meta.description {
                    operation.insert("description".to_string(), json!(desc));
                }
                if !route_meta.tags.is_empty() {
                    operation.insert("tags".to_string(), json!(route_meta.tags));
                } else {
                    operation.insert("tags".to_string(), json!(["API"]));
                }
                if let Some(params) = &route_meta.parameters {
                    operation.insert("parameters".to_string(), params.clone());
                }
                if let Some(body) = &route_meta.request_body {
                    operation.insert("requestBody".to_string(), body.clone());
                }
                operation.insert(
                    "responses".to_string(),
                    json!({ "200": { "description": "Success" } }),
                );
                operation.insert("x-handler".to_string(), json!(route_meta.handler_name));
                operation.insert("x-script-uri".to_string(), json!(metadata.uri));
                operation.insert("x-source".to_string(), json!("javascript"));

                path_obj.insert(method.to_lowercase(), json!(operation));
            }
        }
    }

    // Asset routes from the asset registry
    let asset_registrations = crate::asset_registry::get_global_registry().get_all_registrations();
    for (path, registration) in asset_registrations {
        let extension = path.rsplit('.').next().unwrap_or("");
        let mime_type = match extension {
            "css" => "text/css",
            "js" => "application/javascript",
            "svg" => "image/svg+xml",
            "png" => "image/png",
            "jpg" | "jpeg" => "image/jpeg",
            "gif" => "image/gif",
            "ico" => "image/x-icon",
            "html" => "text/html",
            "json" => "application/json",
            "xml" => "application/xml",
            "pdf" => "application/pdf",
            "woff" | "woff2" => "font/woff2",
            "ttf" => "font/ttf",
            _ => "application/octet-stream",
        };

        let mut asset_operation = serde_json::Map::new();
        let asset_summary = registration
            .metadata
            .summary
            .clone()
            .unwrap_or_else(|| format!("Static asset: {}", registration.asset_name));
        asset_operation.insert("summary".to_string(), json!(asset_summary));
        let asset_description = registration
            .metadata
            .description
            .clone()
            .unwrap_or_else(|| {
                format!(
                    "Serves static asset '{}' registered by script '{}'",
                    registration.asset_name, registration.script_uri
                )
            });
        asset_operation.insert("description".to_string(), json!(asset_description));
        let asset_tags = if registration.metadata.tags.is_empty() {
            vec!["Assets".to_string()]
        } else {
            registration.metadata.tags.clone()
        };
        asset_operation.insert("tags".to_string(), json!(asset_tags));
        asset_operation.insert(
            "responses".to_string(),
            json!({
                "200": {
                    "description": "Asset content",
                    "content": {
                        mime_type: {
                            "schema": { "type": "string", "format": "binary" }
                        }
                    }
                },
                "404": { "description": "Asset not found" }
            }),
        );
        asset_operation.insert("x-asset-name".to_string(), json!(registration.asset_name));
        asset_operation.insert("x-script-uri".to_string(), json!(registration.script_uri));
        asset_operation.insert("x-source".to_string(), json!("asset-registry"));

        let path_entry = js_paths.entry(path).or_insert_with(|| json!({}));
        if let Some(path_obj) = path_entry.as_object_mut() {
            path_obj.insert("get".to_string(), json!(asset_operation));
        }
    }

    // SSE stream routes from the stream registry
    for (path, script_uri, metadata) in
        crate::stream_registry::GLOBAL_STREAM_REGISTRY.get_all_registrations()
    {
        let stream_tags = if metadata.tags.is_empty() {
            vec!["Streams".to_string()]
        } else {
            metadata.tags
        };

        let mut stream_operation = serde_json::Map::new();
        let stream_summary = metadata
            .summary
            .unwrap_or_else(|| format!("SSE stream: {}", path));
        stream_operation.insert("summary".to_string(), json!(stream_summary));
        let stream_description = metadata.description.unwrap_or_else(|| {
            format!(
                "Server-Sent Events stream registered by script '{}'",
                script_uri
            )
        });
        stream_operation.insert("description".to_string(), json!(stream_description));
        stream_operation.insert("tags".to_string(), json!(stream_tags));
        stream_operation.insert(
            "responses".to_string(),
            json!({
                "200": {
                    "description": "SSE event stream",
                    "content": {
                        "text/event-stream": { "schema": { "type": "string" } }
                    }
                }
            }),
        );
        stream_operation.insert("x-script-uri".to_string(), json!(script_uri));
        stream_operation.insert("x-source".to_string(), json!("stream-registry"));

        let path_entry = js_paths.entry(path).or_insert_with(|| json!({}));
        if let Some(path_obj) = path_entry.as_object_mut() {
            path_obj.insert("get".to_string(), json!(stream_operation));
        }
    }

    // Merge collected paths into the Rust spec
    if let Some(rust_paths) = rust_spec["paths"].as_object_mut() {
        for (path, operations) in js_paths {
            if let Some(existing) = rust_paths.get_mut(&path) {
                if let (Some(existing_obj), Some(new_ops)) =
                    (existing.as_object_mut(), operations.as_object())
                {
                    for (method, operation) in new_ops {
                        existing_obj.insert(method.clone(), operation.clone());
                    }
                }
            } else {
                rust_paths.insert(path, operations);
            }
        }
    }

    match serde_json::to_string_pretty(&rust_spec) {
        Ok(json) => json,
        Err(e) => format!(
            "{{\"error\": \"Failed to serialize merged OpenAPI spec: {}\"}}",
            e
        ),
    }
}

// ============================================================================
// REST routes (contracts identical to the core.js/cli.js handlers)
// ============================================================================

fn json_response(status: StatusCode, body: Value) -> Response {
    (
        status,
        [("content-type", "application/json")],
        body.to_string(),
    )
        .into_response()
}

fn missing_param_response(name: &str) -> Response {
    json_response(
        StatusCode::BAD_REQUEST,
        json!({
            "error": format!("Missing required parameter: {}", name),
            "timestamp": iso_timestamp(),
        }),
    )
}

#[derive(Deserialize, Default)]
pub struct ScriptParams {
    uri: Option<String>,
    content: Option<String>,
}

/// Create or update a script.
#[utoipa::path(
    post,
    path = "/upsert_script",
    tags = ["Scripts"],
    request_body(content_type = "application/x-www-form-urlencoded",
        description = "Form fields: uri (required), content (required)"),
    responses(
        (status = 200, description = "Script upserted successfully"),
        (status = 400, description = "Missing required parameter"),
        (status = 500, description = "Failed to upsert script"),
    )
)]
pub async fn upsert_script_route(
    auth_user: Option<Extension<AuthUser>>,
    Query(query): Query<ScriptParams>,
    body: axum::body::Bytes,
) -> Response {
    let user = user_context_from(auth_user.as_deref());
    let form: ScriptParams = serde_urlencoded::from_bytes(&body).unwrap_or_default();
    let Some(uri) = form.uri.or(query.uri) else {
        return missing_param_response("uri");
    };
    let Some(content) = form.content.or(query.content) else {
        return missing_param_response("content");
    };

    let result = tokio::task::spawn_blocking(move || {
        upsert_script_authorized(&user, &uri, &content, None).map(|_| (uri, content.len()))
    })
    .await;

    match result {
        Ok(Ok((uri, content_length))) => json_response(
            StatusCode::OK,
            json!({
                "success": true,
                "message": "Script upserted successfully",
                "uri": uri,
                "contentLength": content_length,
                "timestamp": iso_timestamp(),
            }),
        ),
        Ok(Err(details)) => json_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            json!({
                "error": "Failed to upsert script",
                "details": details,
                "timestamp": iso_timestamp(),
            }),
        ),
        Err(e) => json_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            json!({
                "error": "Failed to upsert script",
                "details": format!("join error: {}", e),
                "timestamp": iso_timestamp(),
            }),
        ),
    }
}

/// Delete a script.
#[utoipa::path(
    post,
    path = "/delete_script",
    tags = ["Scripts"],
    request_body(content_type = "application/x-www-form-urlencoded",
        description = "Form fields: uri (required)"),
    responses(
        (status = 200, description = "Script deleted successfully"),
        (status = 400, description = "Missing required parameter"),
        (status = 404, description = "Script not found"),
    )
)]
pub async fn delete_script_route(
    auth_user: Option<Extension<AuthUser>>,
    Query(query): Query<ScriptParams>,
    body: axum::body::Bytes,
) -> Response {
    let user = user_context_from(auth_user.as_deref());
    let form: ScriptParams = serde_urlencoded::from_bytes(&body).unwrap_or_default();
    let Some(uri) = form.uri.or(query.uri) else {
        return missing_param_response("uri");
    };

    let uri_for_task = uri.clone();
    let deleted =
        tokio::task::spawn_blocking(move || delete_script_authorized(&user, &uri_for_task, None))
            .await
            .unwrap_or(false);

    if deleted {
        json_response(
            StatusCode::OK,
            json!({
                "success": true,
                "message": "Script deleted successfully",
                "uri": uri,
                "timestamp": iso_timestamp(),
            }),
        )
    } else {
        json_response(
            StatusCode::NOT_FOUND,
            json!({
                "error": "Script not found",
                "message": "No script with the specified URI was found",
                "uri": uri,
                "timestamp": iso_timestamp(),
            }),
        )
    }
}

/// Read a script's content.
#[utoipa::path(
    get,
    path = "/read_script",
    tags = ["Scripts"],
    params(("uri" = String, Query, description = "Script URI")),
    responses(
        (status = 200, description = "Script content", content_type = "application/javascript"),
        (status = 400, description = "Missing required parameter"),
        (status = 404, description = "Script not found"),
    )
)]
pub async fn read_script_route(
    auth_user: Option<Extension<AuthUser>>,
    Query(query): Query<ScriptParams>,
) -> Response {
    let user = user_context_from(auth_user.as_deref());
    let Some(uri) = query.uri else {
        return missing_param_response("uri");
    };

    let uri_for_task = uri.clone();
    let content = tokio::task::spawn_blocking(move || get_script_authorized(&user, &uri_for_task))
        .await
        .unwrap_or(None);

    match content {
        Some(content) => (
            StatusCode::OK,
            [("content-type", "application/javascript")],
            content,
        )
            .into_response(),
        None => json_response(
            StatusCode::NOT_FOUND,
            json!({
                "error": "Script not found",
                "message": "No script with the specified URI was found",
                "uri": uri,
                "timestamp": iso_timestamp(),
            }),
        ),
    }
}

/// Get logs for a script.
#[utoipa::path(
    get,
    path = "/script_logs",
    tags = ["Logging"],
    params(("uri" = String, Query, description = "Script URI")),
    responses(
        (status = 200, description = "Log entries for the script"),
        (status = 400, description = "Missing required parameter"),
    )
)]
pub async fn script_logs_route(
    auth_user: Option<Extension<AuthUser>>,
    Query(query): Query<ScriptParams>,
) -> Response {
    let user = user_context_from(auth_user.as_deref());
    let Some(uri) = query.uri else {
        return missing_param_response("uri");
    };

    let uri_for_task = uri.clone();
    let logs = tokio::task::spawn_blocking(move || logs_authorized(&user, &uri_for_task))
        .await
        .unwrap_or_default();

    json_response(
        StatusCode::OK,
        json!({
            "uri": uri,
            "logs": logs,
            "count": logs.len(),
            "timestamp": iso_timestamp(),
        }),
    )
}

#[derive(Deserialize, Default)]
pub struct AssetQuery {
    script: Option<String>,
    asset: Option<String>,
}

#[derive(Deserialize, Default)]
pub struct AssetBody {
    asset: Option<String>,
    mimetype: Option<String>,
    content: Option<String>,
}

fn error_response(status: StatusCode, message: String) -> Response {
    json_response(status, json!({ "error": message }))
}

/// List assets for a script, or fetch one asset when `asset` is given.
#[utoipa::path(
    get,
    path = "/assets",
    tags = ["Assets"],
    params(
        ("script" = String, Query, description = "URI of the script whose assets to manage"),
        ("asset" = Option<String>, Query, description = "Asset URI to fetch; omit to list all"),
    ),
    responses(
        (status = 200, description = "Asset list or asset content (base64)"),
        (status = 400, description = "Missing required parameter"),
        (status = 404, description = "Asset not found or access denied"),
    )
)]
pub async fn assets_get_route(
    auth_user: Option<Extension<AuthUser>>,
    Query(query): Query<AssetQuery>,
) -> Response {
    let user = user_context_from(auth_user.as_deref());
    let Some(script) = query.script else {
        return error_response(
            StatusCode::BAD_REQUEST,
            "Missing required parameter: script".to_string(),
        );
    };

    match query.asset {
        Some(asset) => {
            let (user, script_cl, asset_cl) = (user, script.clone(), asset.clone());
            let result = tokio::task::spawn_blocking(move || {
                fetch_asset_authorized(&user, &script_cl, &asset_cl)
            })
            .await
            .unwrap_or(Err(AssetFetchError::NotFound));

            match result {
                Ok(content) => json_response(
                    StatusCode::OK,
                    json!({ "script": script, "asset": asset, "content": content }),
                ),
                Err(AssetFetchError::AccessDenied) => {
                    error_response(StatusCode::NOT_FOUND, "Error: Access denied".to_string())
                }
                Err(AssetFetchError::NotFound) => error_response(
                    StatusCode::NOT_FOUND,
                    format!("Asset '{}' not found", asset),
                ),
            }
        }
        None => {
            let script_cl = script.clone();
            let assets =
                tokio::task::spawn_blocking(move || list_assets_authorized(&user, &script_cl))
                    .await
                    .unwrap_or_default();
            json_response(
                StatusCode::OK,
                json!({ "script": script, "assets": assets }),
            )
        }
    }
}

/// Create or update an asset for a script.
#[utoipa::path(
    post,
    path = "/assets",
    tags = ["Assets"],
    params(("script" = String, Query, description = "URI of the script that will own this asset")),
    request_body(content_type = "application/json",
        description = "JSON fields: asset (required), mimetype (required), content (required, base64, max 10MB)"),
    responses(
        (status = 201, description = "Asset created or updated"),
        (status = 400, description = "Missing or invalid parameters"),
        (status = 403, description = "Access denied"),
    )
)]
pub async fn assets_post_route(
    auth_user: Option<Extension<AuthUser>>,
    Query(query): Query<AssetQuery>,
    body: axum::body::Bytes,
) -> Response {
    let user = user_context_from(auth_user.as_deref());
    let Some(script) = query.script else {
        return error_response(
            StatusCode::BAD_REQUEST,
            "Missing required parameter: script".to_string(),
        );
    };
    let body: AssetBody = serde_json::from_slice(&body).unwrap_or_default();
    let Some(asset) = body.asset.or(query.asset) else {
        return error_response(
            StatusCode::BAD_REQUEST,
            "Missing required parameter: asset".to_string(),
        );
    };
    let (Some(mimetype), Some(content)) = (body.mimetype, body.content) else {
        return error_response(
            StatusCode::BAD_REQUEST,
            "Missing required fields: mimetype, content".to_string(),
        );
    };

    let (script_cl, asset_cl) = (script.clone(), asset.clone());
    let result = tokio::task::spawn_blocking(move || {
        upsert_asset_authorized(&user, &script_cl, &asset_cl, &mimetype, &content)
    })
    .await
    .unwrap_or(Err(AssetWriteError::Storage("join error".to_string())));

    match result {
        Ok(()) => json_response(
            StatusCode::CREATED,
            json!({
                "message": format!("Asset '{}' upserted successfully", asset),
                "script": script,
                "asset": asset,
            }),
        ),
        Err(AssetWriteError::AccessDenied) => {
            error_response(StatusCode::FORBIDDEN, "Error: Access denied".to_string())
        }
        Err(AssetWriteError::Validation(msg)) => error_response(StatusCode::BAD_REQUEST, msg),
        Err(AssetWriteError::Storage(msg)) => {
            error_response(StatusCode::INTERNAL_SERVER_ERROR, msg)
        }
    }
}

/// Delete an asset from a script.
#[utoipa::path(
    delete,
    path = "/assets",
    tags = ["Assets"],
    params(
        ("script" = String, Query, description = "URI of the script that owns the asset"),
        ("asset" = String, Query, description = "Asset URI to delete"),
    ),
    responses(
        (status = 200, description = "Asset deleted"),
        (status = 400, description = "Missing required parameter"),
        (status = 403, description = "Access denied"),
        (status = 404, description = "Asset not found"),
    )
)]
pub async fn assets_delete_route(
    auth_user: Option<Extension<AuthUser>>,
    Query(query): Query<AssetQuery>,
) -> Response {
    let user = user_context_from(auth_user.as_deref());
    let Some(script) = query.script else {
        return error_response(
            StatusCode::BAD_REQUEST,
            "Missing required parameter: script".to_string(),
        );
    };
    let Some(asset) = query.asset else {
        return error_response(
            StatusCode::BAD_REQUEST,
            "Missing required parameter: asset".to_string(),
        );
    };

    let (script_cl, asset_cl) = (script.clone(), asset.clone());
    let result =
        tokio::task::spawn_blocking(move || delete_asset_authorized(&user, &script_cl, &asset_cl))
            .await
            .unwrap_or(Ok(false));

    match result {
        Err(AssetFetchError::AccessDenied) => {
            error_response(StatusCode::FORBIDDEN, "Error: Access denied".to_string())
        }
        Ok(false) | Err(AssetFetchError::NotFound) => error_response(
            StatusCode::NOT_FOUND,
            format!("Asset '{}' not found", asset),
        ),
        Ok(true) => json_response(
            StatusCode::OK,
            json!({
                "message": format!("Asset '{}' deleted successfully", asset),
                "script": script,
                "asset": asset,
            }),
        ),
    }
}

/// Installation confirmation page, shown after a fresh install (the root
/// path redirects here until further routes are registered).
const INSTALLED_PAGE_HTML: &str = r#"<!DOCTYPE html>
<html lang="en">
<head>
  <meta charset="UTF-8">
  <meta name="viewport" content="width=device-width, initial-scale=1.0">
  <title>aiwebengine Installed</title>
  <style>
    body {
      margin: 0;
      padding: 0;
      font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, "Helvetica Neue", Arial, sans-serif;
      display: flex;
      justify-content: center;
      align-items: center;
      min-height: 100vh;
      background: linear-gradient(135deg, #667eea 0%, #764ba2 100%);
    }
    .container {
      text-align: center;
      background: white;
      padding: 3rem 4rem;
      border-radius: 1rem;
      box-shadow: 0 20px 60px rgba(0, 0, 0, 0.3);
    }
    h1 {
      color: #333;
      margin: 0 0 1rem 0;
      font-size: 2.5rem;
    }
    p {
      color: #666;
      font-size: 1.2rem;
      margin: 0;
    }
    .emoji {
      font-size: 4rem;
      margin-bottom: 1rem;
    }
  </style>
</head>
<body>
  <div class="container">
    <div class="emoji">🎉</div>
    <h1>Thanks for installing aiwebengine!</h1>
    <p>Your server is up and running.</p>
  </div>
</body>
</html>"#;

/// Installation confirmation page.
#[utoipa::path(
    get,
    path = "/engine/installed",
    tags = ["Engine"],
    responses(
        (status = 200, description = "Shows a confirmation page for successful installation",
            content_type = "text/html"),
    )
)]
pub async fn installed_page_route() -> Response {
    (
        StatusCode::OK,
        [("content-type", "text/html")],
        INSTALLED_PAGE_HTML,
    )
        .into_response()
}

fn html_escape(input: &str) -> String {
    input
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

#[derive(Deserialize, Default)]
pub struct UnauthorizedQuery {
    attempted: Option<String>,
}

/// Insufficient permissions page, shown when an authenticated user lacks the
/// role required for the page they attempted to access (formerly auth.js).
#[utoipa::path(
    get,
    path = "/auth/unauthorized",
    tags = ["Authentication"],
    params(("attempted" = Option<String>, Query, description = "Path the user attempted to access")),
    responses(
        (status = 403, description = "Insufficient permissions page", content_type = "text/html"),
    )
)]
pub async fn unauthorized_page_route(
    auth_user: Option<Extension<AuthUser>>,
    Query(query): Query<UnauthorizedQuery>,
) -> Response {
    let auth_user = auth_user.as_deref();
    let attempted = query.attempted.as_deref();

    let user_info_block = match auth_user {
        Some(user) => {
            let user_name = user
                .name
                .as_deref()
                .or(user.email.as_deref())
                .unwrap_or("User");
            let email_suffix = user
                .email
                .as_deref()
                .map(|email| format!(" ({})", html_escape(email)))
                .unwrap_or_default();
            format!(
                r#"
            <div class="user-info">
                <strong>Signed in as:</strong> {}{}
            </div>
            "#,
                html_escape(user_name),
                email_suffix
            )
        }
        None => String::new(),
    };

    let attempted_path_block = match attempted {
        Some(path) => format!(
            r#"
            <div class="attempted-path">
                <strong>Attempted to access:</strong> {}
            </div>
            "#,
            html_escape(path)
        ),
        None => String::new(),
    };

    let action_link = match auth_user {
        Some(_) => r#"<a href="/auth/logout">Sign Out</a>"#.to_string(),
        None => {
            let redirect_suffix = attempted
                .map(|path| format!("?redirect={}", urlencoding::encode(path)))
                .unwrap_or_default();
            format!(r#"<a href="/auth/login{}">Sign In</a>"#, redirect_suffix)
        }
    };

    let html = format!(
        r#"<!DOCTYPE html>
<html lang="en">
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <title>Insufficient Permissions - aiwebengine</title>
    <link rel="icon" type="image/x-icon" href="/favicon.ico">
    <style>
        /* Self-contained: this page is shown in error situations and must
           not depend on any other resource being served correctly. */
        :root {{
            --primary-color: #007acc;
            --bg-primary: #ffffff;
            --bg-secondary: #f8f9fa;
            --text-color: #212529;
            --text-muted: #6c757d;
            --border-color: #dee2e6;
            --border-radius: 6px;
            --border-radius-lg: 8px;
            --shadow: 0 1px 3px rgba(0, 0, 0, 0.1), 0 1px 2px rgba(0, 0, 0, 0.06);
            --shadow-lg: 0 10px 15px rgba(0, 0, 0, 0.1), 0 4px 6px rgba(0, 0, 0, 0.05);
            --transition: all 0.2s ease;
            --info-bg: #e8f4fd;
            --info-border: #b6def7;
            --info-color: #0c5464;
            --error-bg: #f8d7da;
            --error-color: #dc3545;
        }}

        body {{
            margin: 0;
            font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, "Helvetica Neue", Arial, sans-serif;
            font-size: 14px;
            line-height: 1.5;
        }}

        body {{
            background: linear-gradient(135deg, #667eea 0%, #764ba2 100%);
            min-height: 100vh;
            display: flex;
            align-items: center;
            justify-content: center;
            padding: 2rem 0;
        }}

        .permissions-container {{
            max-width: 600px;
            margin: 0 auto;
            background: rgba(255, 255, 255, 0.95);
            backdrop-filter: blur(10px);
            border-radius: var(--border-radius-lg);
            box-shadow: var(--shadow-lg);
            overflow: hidden;
        }}

        .permissions-content {{
            padding: 3rem 2rem;
            text-align: center;
        }}

        .permissions-icon {{
            width: 80px;
            height: 80px;
            margin: 0 auto 1.5rem;
            background: linear-gradient(135deg, #667eea 0%, #764ba2 100%);
            border-radius: 50%;
            display: flex;
            align-items: center;
            justify-content: center;
            font-size: 40px;
            color: white;
        }}

        .permissions-content h1 {{
            color: var(--text-color);
            margin-bottom: 1rem;
            font-size: 2rem;
        }}

        .permissions-subtitle {{
            color: var(--text-muted);
            margin-bottom: 2rem;
            font-size: 1.1rem;
            line-height: 1.6;
        }}

        .info-box {{
            background: var(--bg-secondary);
            border-left: 4px solid var(--primary-color);
            border-radius: var(--border-radius);
            padding: 1.5rem;
            margin-bottom: 2rem;
            text-align: left;
        }}

        .info-box p {{
            color: var(--text-muted);
            line-height: 1.6;
            margin-bottom: 0.75rem;
        }}

        .info-box p:last-child {{
            margin-bottom: 0;
        }}

        .info-box strong {{
            color: var(--text-color);
        }}

        .user-info {{
            background: var(--info-bg);
            border: 1px solid var(--info-border);
            border-radius: var(--border-radius);
            padding: 1rem;
            margin-bottom: 1.5rem;
            font-size: 0.9rem;
            color: var(--info-color);
        }}

        .user-info strong {{
            color: var(--text-color);
        }}

        .attempted-path {{
            background: var(--error-bg);
            border-left: 4px solid var(--error-color);
            border-radius: var(--border-radius);
            padding: 1rem;
            margin-bottom: 1.5rem;
            text-align: left;
            font-size: 0.9rem;
            color: var(--error-color);
            word-break: break-all;
        }}

        .permissions-actions {{
            display: flex;
            gap: 1rem;
            justify-content: center;
            flex-wrap: wrap;
            margin-bottom: 2rem;
        }}

        .permissions-actions a {{
            padding: 0.75rem 1.5rem;
            border-radius: var(--border-radius);
            text-decoration: none;
            font-weight: 600;
            font-size: 1rem;
            transition: var(--transition);
            display: inline-block;
            text-align: center;
        }}

        .permissions-actions a:first-child {{
            background: linear-gradient(135deg, #667eea 0%, #764ba2 100%);
            color: white;
        }}

        .permissions-actions a:first-child:hover {{
            transform: translateY(-2px);
            box-shadow: var(--shadow);
        }}

        .permissions-actions a:last-child {{
            background: var(--bg-secondary);
            color: var(--text-muted);
            border: 1px solid var(--border-color);
        }}

        .permissions-actions a:last-child:hover {{
            background: var(--bg-primary);
        }}

        .contact-info {{
            margin-top: 2rem;
            padding-top: 1.5rem;
            border-top: 1px solid var(--border-color);
            font-size: 0.9rem;
            color: var(--text-muted);
        }}

        @media (max-width: 768px) {{
            .permissions-content {{
                padding: 2rem 1rem;
            }}

            .permissions-content h1 {{
                font-size: 1.75rem;
            }}

            .permissions-actions {{
                flex-direction: column;
            }}

            .permissions-actions a {{
                width: 100%;
            }}
        }}
    </style>
</head>
<body>
    <div class="permissions-container">
        <div class="permissions-content">
            <div class="permissions-icon">
                🔒
            </div>

            <h1>Insufficient Permissions</h1>

            <p class="permissions-subtitle">
                You don't have the required permissions to access this resource.
            </p>
            {user_info_block}{attempted_path_block}
            <div class="info-box">
                <p><strong>Why am I seeing this?</strong></p>
                <p>This page or feature requires <strong>Editor</strong> or <strong>Administrator</strong> privileges. Your current account does not have these permissions.</p>
                <p><strong>What can I do?</strong></p>
                <p>• Contact your system administrator to request the appropriate role</p>
                <p>• Verify you're signed in with the correct account</p>
                <p>• Return to the home page to access features available to you</p>
            </div>

            <div class="permissions-actions">
                <a href="/">Go to Home</a>
                {action_link}
            </div>

            <div class="contact-info">
                If you believe this is an error, please contact your system administrator.
            </div>
        </div>
    </div>
</body>
</html>"#
    );

    (
        StatusCode::FORBIDDEN,
        [("content-type", "text/html; charset=UTF-8")],
        html,
    )
        .into_response()
}

/// Site favicon, served from the engine's bootstrapped assets.
#[utoipa::path(
    get,
    path = "/favicon.ico",
    tags = ["Assets"],
    responses(
        (status = 200, description = "Favicon", content_type = "image/x-icon"),
        (status = 404, description = "Favicon not found"),
    )
)]
pub async fn favicon_route() -> Response {
    match repository::fetch_asset_async("https://example.com/core", "favicon.ico").await {
        Some(asset) => (
            StatusCode::OK,
            [
                ("content-type", asset.mimetype),
                ("cache-control", "public, max-age=3600".to_string()),
            ],
            asset.content,
        )
            .into_response(),
        None => (StatusCode::NOT_FOUND, "Favicon not found").into_response(),
    }
}

/// OpenAPI specification for all registered routes.
#[utoipa::path(
    get,
    path = "/engine/openapi.json",
    tags = ["Engine"],
    responses(
        (status = 200, description = "OpenAPI 3.0 specification for all registered routes"),
        (status = 403, description = "Insufficient permissions"),
    )
)]
pub async fn openapi_route(auth_user: Option<Extension<AuthUser>>) -> Response {
    let user = user_context_from(auth_user.as_deref());
    if user.require_capability(&Capability::ReadScripts).is_err() {
        return json_response(
            StatusCode::FORBIDDEN,
            json!({ "error": "Insufficient permissions" }),
        );
    }

    let spec = tokio::task::spawn_blocking(generate_merged_openapi_spec)
        .await
        .unwrap_or_else(|e| format!("{{\"error\": \"join error: {}\"}}", e));

    (StatusCode::OK, [("content-type", "application/json")], spec).into_response()
}

// ============================================================================
// Native MCP tools (names and result shapes identical to the script tools)
// ============================================================================

/// Descriptor of a native MCP tool for tools/list.
pub struct NativeToolDescriptor {
    pub name: &'static str,
    pub description: &'static str,
    pub input_schema: Value,
}

type NativeToolHandler = fn(&Value, &UserContext) -> Value;
type NativeToolEntry = (&'static str, &'static str, fn() -> Value, NativeToolHandler);

fn arg_str<'a>(args: &'a Value, key: &str) -> Option<&'a str> {
    args.get(key).and_then(Value::as_str)
}

fn missing_arg(name: &str) -> Value {
    json!({ "error": format!("Missing required parameter: {}", name) })
}

fn native_tools() -> &'static [NativeToolEntry] {
    &[
        (
            "read_file",
            "Fetch the contents of a remote file (script) by URI",
            || {
                json!({
                    "type": "object",
                    "properties": {
                        "uri": { "type": "string", "description": "Script URI (e.g., 'https://example.com/myscript')" }
                    },
                    "required": ["uri"]
                })
            },
            tool_read_file,
        ),
        (
            "write_file",
            "Create or update a file (script) on the server",
            || {
                json!({
                    "type": "object",
                    "properties": {
                        "uri": { "type": "string", "description": "Script URI" },
                        "content": { "type": "string", "description": "File content (JavaScript code)" }
                    },
                    "required": ["uri", "content"]
                })
            },
            tool_write_file,
        ),
        (
            "create_file",
            "Create a new file (script) on the server. Fails if file already exists.",
            || {
                json!({
                    "type": "object",
                    "properties": {
                        "uri": { "type": "string", "description": "Script URI" },
                        "content": { "type": "string", "description": "File content (JavaScript code)", "default": "" }
                    },
                    "required": ["uri"]
                })
            },
            tool_create_file,
        ),
        (
            "list_files",
            "List all files (scripts) in the system, optionally filtered by pattern",
            || {
                json!({
                    "type": "object",
                    "properties": {
                        "pattern": { "type": "string", "description": "Optional regex pattern to filter files by URI" }
                    }
                })
            },
            tool_list_files,
        ),
        (
            "delete_file",
            "Remove a file (script) from the server",
            || {
                json!({
                    "type": "object",
                    "properties": {
                        "uri": { "type": "string", "description": "Script URI to delete" }
                    },
                    "required": ["uri"]
                })
            },
            tool_delete_file,
        ),
        (
            "search_files",
            "Perform text search across all files (grep-like functionality)",
            || {
                json!({
                    "type": "object",
                    "properties": {
                        "query": { "type": "string", "description": "Text or regex pattern to search for" },
                        "caseInsensitive": { "type": "boolean", "description": "Whether search should be case-insensitive", "default": true }
                    },
                    "required": ["query"]
                })
            },
            tool_search_files,
        ),
        (
            "read_logs",
            "Read log messages for a specific script (useful for debugging)",
            || {
                json!({
                    "type": "object",
                    "properties": {
                        "uri": { "type": "string", "description": "Script URI to retrieve logs for" }
                    },
                    "required": ["uri"]
                })
            },
            tool_read_logs,
        ),
        (
            "read_init_status",
            "Read init() status for scripts (useful for debugging). Returns status for one script when uri is given, otherwise for all scripts.",
            || {
                json!({
                    "type": "object",
                    "properties": {
                        "uri": { "type": "string", "description": "Optional script URI to retrieve init status for; omit to list all scripts" }
                    }
                })
            },
            tool_read_init_status,
        ),
        (
            "list_assets",
            "List all assets owned by a script. Requires the user to own the script, have ReadAssets capability, or be an administrator.",
            || {
                json!({
                    "type": "object",
                    "properties": {
                        "script": { "type": "string", "description": "URI of the script whose assets to list (e.g., 'https://example.com/myscript')" }
                    },
                    "required": ["script"]
                })
            },
            tool_list_assets,
        ),
        (
            "read_asset",
            "Fetch the base64-encoded content of a specific asset owned by a script. Requires the user to own the script, have ReadAssets capability, or be an administrator.",
            || {
                json!({
                    "type": "object",
                    "properties": {
                        "script": { "type": "string", "description": "URI of the script that owns the asset" },
                        "asset": { "type": "string", "description": "URI/path of the asset to fetch (e.g., '/images/logo.png')" }
                    },
                    "required": ["script", "asset"]
                })
            },
            tool_read_asset,
        ),
        (
            "write_asset",
            "Create or update an asset for a script. Requires the user to own the script, have WriteAssets capability, or be an administrator.",
            || {
                json!({
                    "type": "object",
                    "properties": {
                        "script": { "type": "string", "description": "URI of the script that will own the asset" },
                        "asset": { "type": "string", "description": "URI/path of the asset (e.g., '/images/logo.png')" },
                        "mimetype": { "type": "string", "description": "MIME type of the asset (e.g., 'image/png', 'text/css')" },
                        "content": { "type": "string", "description": "Base64-encoded content of the asset (max 10MB)" }
                    },
                    "required": ["script", "asset", "mimetype", "content"]
                })
            },
            tool_write_asset,
        ),
        (
            "delete_asset",
            "Delete an asset from a script. Requires the user to own the script, have DeleteAssets capability, or be an administrator.",
            || {
                json!({
                    "type": "object",
                    "properties": {
                        "script": { "type": "string", "description": "URI of the script that owns the asset" },
                        "asset": { "type": "string", "description": "URI/path of the asset to delete (e.g., '/images/logo.png')" }
                    },
                    "required": ["script", "asset"]
                })
            },
            tool_delete_asset,
        ),
    ]
}

/// Descriptors of all native MCP tools, for tools/list.
pub fn native_mcp_tool_descriptors() -> Vec<NativeToolDescriptor> {
    native_tools()
        .iter()
        .map(|(name, description, schema, _)| NativeToolDescriptor {
            name,
            description,
            input_schema: schema(),
        })
        .collect()
}

/// Execute a native MCP tool. Returns None when no native tool has this name
/// (the caller then falls back to script-registered tools).
pub fn execute_native_mcp_tool(
    tool_name: &str,
    arguments: &Value,
    user_context: &UserContext,
) -> Option<Value> {
    let handler = native_tools()
        .iter()
        .find(|(name, _, _, _)| *name == tool_name)
        .map(|(_, _, _, handler)| *handler)?;
    Some(handler(arguments, user_context))
}

fn tool_read_file(args: &Value, user: &UserContext) -> Value {
    let Some(uri) = arg_str(args, "uri") else {
        return missing_arg("uri");
    };
    match get_script_authorized(user, uri) {
        Some(content) => json!({
            "uri": uri,
            "content": content,
            "size": content.len(),
            "timestamp": iso_timestamp(),
        }),
        None => json!({ "error": format!("File not found: {}", uri) }),
    }
}

fn tool_write_file(args: &Value, user: &UserContext) -> Value {
    let Some(uri) = arg_str(args, "uri") else {
        return missing_arg("uri");
    };
    let Some(content) = arg_str(args, "content") else {
        return missing_arg("content");
    };
    match upsert_script_authorized(user, uri, content, Some("mcp")) {
        Ok(action) => {
            let action = match action {
                UpsertAction::Inserted => "created",
                UpsertAction::Updated => "updated",
            };
            json!({
                "success": true,
                "action": action,
                "uri": uri,
                "size": content.len(),
                "timestamp": iso_timestamp(),
            })
        }
        Err(e) => json!({ "error": format!("Failed to write file: {}", e) }),
    }
}

fn tool_create_file(args: &Value, user: &UserContext) -> Value {
    let Some(uri) = arg_str(args, "uri") else {
        return missing_arg("uri");
    };
    let content = arg_str(args, "content").unwrap_or("");

    if get_script_authorized(user, uri).is_some() {
        return json!({ "error": format!("File already exists: {}", uri) });
    }
    match upsert_script_authorized(user, uri, content, Some("mcp")) {
        Ok(_) => json!({
            "success": true,
            "uri": uri,
            "size": content.len(),
            "timestamp": iso_timestamp(),
        }),
        Err(e) => json!({ "error": format!("Failed to create file: {}", e) }),
    }
}

fn tool_list_files(args: &Value, user: &UserContext) -> Value {
    let pattern = arg_str(args, "pattern");
    let regex = match pattern {
        Some(p) => match regex::RegexBuilder::new(p).case_insensitive(true).build() {
            Ok(r) => Some(r),
            Err(e) => return json!({ "error": format!("Failed to list files: {}", e) }),
        },
        None => None,
    };

    let files: Vec<Value> = list_scripts_authorized(user)
        .iter()
        .filter(|meta| regex.as_ref().is_none_or(|r| r.is_match(&meta.uri)))
        .map(|meta| {
            json!({
                "uri": meta.uri,
                "size": meta.content.len(),
                "type": "script",
            })
        })
        .collect();

    json!({
        "files": files,
        "count": files.len(),
        "pattern": pattern,
        "timestamp": iso_timestamp(),
    })
}

fn tool_delete_file(args: &Value, user: &UserContext) -> Value {
    let Some(uri) = arg_str(args, "uri") else {
        return missing_arg("uri");
    };
    if delete_script_authorized(user, uri, Some("mcp")) {
        json!({
            "success": true,
            "uri": uri,
            "timestamp": iso_timestamp(),
        })
    } else {
        json!({ "error": format!("File not found: {}", uri) })
    }
}

fn tool_search_files(args: &Value, user: &UserContext) -> Value {
    let Some(query) = arg_str(args, "query") else {
        return missing_arg("query");
    };
    let case_insensitive = args
        .get("caseInsensitive")
        .and_then(Value::as_bool)
        .unwrap_or(true);

    let regex = match regex::RegexBuilder::new(query)
        .case_insensitive(case_insensitive)
        .build()
    {
        Ok(r) => r,
        Err(e) => return json!({ "error": format!("Failed to search files: {}", e) }),
    };

    let mut results: Vec<Value> = Vec::new();
    for meta in list_scripts_authorized(user) {
        let matches: Vec<Value> = meta
            .content
            .lines()
            .enumerate()
            .filter(|(_, line)| regex.is_match(line))
            .take(50)
            .map(|(i, line)| {
                json!({
                    "line": i + 1,
                    "content": line.trim(),
                    "preview": line.chars().take(200).collect::<String>(),
                })
            })
            .collect();

        if !matches.is_empty() {
            results.push(json!({
                "uri": meta.uri,
                "matchCount": matches.len(),
                "matches": matches,
            }));
        }
    }

    json!({
        "query": query,
        "caseInsensitive": case_insensitive,
        "filesMatched": results.len(),
        "results": results,
        "timestamp": iso_timestamp(),
    })
}

fn tool_read_logs(args: &Value, user: &UserContext) -> Value {
    let Some(uri) = arg_str(args, "uri") else {
        return missing_arg("uri");
    };
    let logs = logs_authorized(user, uri);
    json!({
        "uri": uri,
        "logs": logs,
        "count": logs.len(),
        "timestamp": iso_timestamp(),
    })
}

fn tool_read_init_status(args: &Value, user: &UserContext) -> Value {
    match arg_str(args, "uri") {
        Some(uri) => json!({
            "uri": uri,
            "status": init_status_authorized(user, uri),
            "timestamp": iso_timestamp(),
        }),
        None => {
            let statuses: Vec<Value> = list_scripts_authorized(user)
                .iter()
                .map(script_init_status_json)
                .collect();
            json!({
                "statuses": statuses,
                "count": statuses.len(),
                "timestamp": iso_timestamp(),
            })
        }
    }
}

fn tool_list_assets(args: &Value, user: &UserContext) -> Value {
    let Some(script) = arg_str(args, "script") else {
        return missing_arg("script");
    };
    let assets = list_assets_authorized(user, script);
    json!({
        "script": script,
        "assets": assets,
        "count": assets.len(),
        "timestamp": iso_timestamp(),
    })
}

fn tool_read_asset(args: &Value, user: &UserContext) -> Value {
    let Some(script) = arg_str(args, "script") else {
        return missing_arg("script");
    };
    let Some(asset) = arg_str(args, "asset") else {
        return missing_arg("asset");
    };
    match fetch_asset_authorized(user, script, asset) {
        Ok(content) => json!({
            "script": script,
            "asset": asset,
            "content": content,
            "timestamp": iso_timestamp(),
        }),
        Err(AssetFetchError::AccessDenied) => json!({ "error": "Error: Access denied" }),
        Err(AssetFetchError::NotFound) => {
            json!({ "error": format!("Asset not found: {}", asset) })
        }
    }
}

fn tool_write_asset(args: &Value, user: &UserContext) -> Value {
    let Some(script) = arg_str(args, "script") else {
        return missing_arg("script");
    };
    let Some(asset) = arg_str(args, "asset") else {
        return missing_arg("asset");
    };
    let Some(mimetype) = arg_str(args, "mimetype") else {
        return missing_arg("mimetype");
    };
    let Some(content) = arg_str(args, "content") else {
        return missing_arg("content");
    };

    match upsert_asset_authorized(user, script, asset, mimetype, content) {
        Ok(()) => json!({
            "success": true,
            "message": format!("Asset '{}' upserted successfully", asset),
            "script": script,
            "asset": asset,
            "timestamp": iso_timestamp(),
        }),
        Err(AssetWriteError::AccessDenied) => {
            json!({ "error": "Failed to write asset: Access denied" })
        }
        Err(AssetWriteError::Validation(msg)) | Err(AssetWriteError::Storage(msg)) => {
            json!({ "error": format!("Failed to write asset: {}", msg) })
        }
    }
}

fn tool_delete_asset(args: &Value, user: &UserContext) -> Value {
    let Some(script) = arg_str(args, "script") else {
        return missing_arg("script");
    };
    let Some(asset) = arg_str(args, "asset") else {
        return missing_arg("asset");
    };
    match delete_asset_authorized(user, script, asset) {
        Ok(true) => json!({
            "success": true,
            "message": format!("Asset '{}' deleted successfully", asset),
            "script": script,
            "asset": asset,
            "timestamp": iso_timestamp(),
        }),
        Ok(false) => json!({ "error": format!("Asset '{}' not found", asset) }),
        Err(_) => json!({ "error": "Failed to delete asset: Access denied" }),
    }
}
