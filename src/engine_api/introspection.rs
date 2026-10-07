//! What the engine serves: the exposure report, the route listing and the OpenAPI
//! document.

use super::*;
use crate::error::AppResult;
use crate::repository;
use crate::security::{Capability, UserContext};
use serde_json::{Value, json};
use tracing::warn;

// ============================================================================
// OpenAPI spec generation
// ============================================================================

/// Every registration in the engine as an introspection entry: script HTTP
/// routes, then SSE streams as `STREAM` rows, then asset routes as `ASSET`
/// rows. ReadScripts capability required (empty otherwise).
///
/// Backs `/engine/list_routes`. Host bindings are not applied here — this is the
/// whole engine's view; callers that care filter by `script_uri` (see
/// [`crate::route_index::script_serves_host`]).
/// What enforcing exposure-by-directory would change.
///
/// Reads the live registries, so it is a report about what this instance is
/// actually publishing rather than about what its scripts' source appears to
/// say. Administrator-only: the list names every file a deployment serves
/// from a directory that says it is private, which is a map of exactly the
/// mistakes worth exploiting.
pub fn exposure_report_authorized(
    user: &UserContext,
) -> AppResult<crate::exposure::ExposureReport> {
    if !may_administer(user) {
        return Err(crate::error::AppError::AuthorizationFailed {
            message: "The exposure report is not open to anonymous callers".to_string(),
        });
    }
    user.require_capability(&Capability::AdministerEngine)?;

    let metadata = repository::get_all_script_metadata()?;
    Ok(crate::exposure::report(&metadata))
}

/// The engine's own streams, which are not registrations of any script.
///
/// A script's streams are indexed with its routes; these have no script
/// behind them and no metadata to be indexed from, so they stay in
/// `stream_registry` — which is where they are registered — and are listed
/// from there.
pub(super) fn engine_owned_streams() -> Vec<(String, String)> {
    crate::stream_registry::GLOBAL_STREAM_REGISTRY
        .get_all_registrations()
        .into_iter()
        .filter(|(_, script_uri, _)| script_uri.starts_with("engine://"))
        .map(|(path, script_uri, _)| (path, script_uri))
        .collect()
}

pub fn routes_introspection_authorized(user: &UserContext) -> AppResult<Vec<Value>> {
    if !may_administer(user) {
        return Err(crate::error::AppError::AuthorizationFailed {
            message: "Engine route introspection is not open to anonymous callers".to_string(),
        });
    }
    user.require_capability(&Capability::ReadScripts)?;

    let metadata_list = repository::get_all_script_metadata()?;

    let mut all_routes = Vec::new();
    for metadata in metadata_list {
        if metadata.initialized && !metadata.registrations.is_empty() {
            for ((path, method), route_meta) in metadata.registrations {
                // Handlers and file routes come from the same list.
                let tags = if route_meta.tags.is_empty()
                    && route_meta.kind != repository::RouteKind::Handler
                {
                    vec![route_meta.default_tag().to_string()]
                } else {
                    route_meta.tags.clone()
                };
                all_routes.push(json!({
                    "path": path,
                    "method": method,
                    "handler": route_meta.target(),
                    "script_uri": metadata.uri,
                    "summary": route_meta.summary,
                    "description": route_meta.description,
                    "tags": tags,
                }));
            }
        }
    }

    for (path, script_uri) in engine_owned_streams() {
        all_routes.push(json!({
            "path": path,
            "method": repository::STREAM_METHOD,
            "handler": Value::Null,
            "script_uri": script_uri,
            "summary": Value::Null,
            "description": Value::Null,
            "tags": ["Streams"],
        }));
    }

    Ok(all_routes)
}

/// Keep only entries whose owning script publishes on `host`.
///
/// Registrations are published per host, so an unfiltered listing shows routes
/// that are not live on the host the caller is looking at. Each distinct script
/// is checked once.
pub(super) async fn filter_routes_by_host(routes: Vec<Value>, host: &str) -> Vec<Value> {
    let mut verdicts: std::collections::HashMap<String, bool> = std::collections::HashMap::new();
    let mut filtered = Vec::with_capacity(routes.len());
    for route in routes {
        let script_uri = route
            .get("script_uri")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let serves = match verdicts.get(&script_uri) {
            Some(serves) => *serves,
            None => {
                let serves = crate::route_index::script_serves_host(&script_uri, host).await;
                verdicts.insert(script_uri, serves);
                serves
            }
        };
        if serves {
            filtered.push(route);
        }
    }
    filtered
}

/// Generate the full OpenAPI spec: the Rust (utoipa) spec merged with
/// script-registered routes, asset routes, and SSE stream routes. Returns
/// the same `{"error": ...}` JSON strings as the former JS implementation
/// when a step fails, so callers can pass the result through unchanged.
pub fn generate_merged_openapi_spec() -> String {
    let rust_spec_str = crate::get_rust_openapi_spec();
    let mut rust_spec: Value = match serde_json::from_str(&rust_spec_str) {
        Ok(spec) => spec,
        Err(e) => {
            return refuse(
                Refusal::Failed,
                format!("Failed to parse Rust OpenAPI spec: {}", e),
            )
            .to_string();
        }
    };

    let metadata_list = match repository::get_all_script_metadata() {
        Ok(list) => list,
        Err(e) => {
            return refuse(
                Refusal::Failed,
                format!("Failed to fetch JavaScript routes: {}", e),
            )
            .to_string();
        }
    };

    let mut js_paths = serde_json::Map::new();

    // File routes, taken from the same registrations as the handler routes
    // below and rendered separately because what they document is a file
    // rather than an operation. The collect-first is only because the loop
    // below consumes the metadata list.
    let file_routes: Vec<(String, String, String, repository::RouteMetadata)> = metadata_list
        .iter()
        .filter(|metadata| metadata.initialized)
        .flat_map(|metadata| {
            metadata
                .registrations
                .iter()
                .filter(|(_, route_meta)| route_meta.kind == repository::RouteKind::File)
                .map(|((path, _), route_meta)| {
                    (
                        path.clone(),
                        metadata.uri.clone(),
                        route_meta.file.clone().unwrap_or_default(),
                        route_meta.clone(),
                    )
                })
                .collect::<Vec<_>>()
        })
        .collect();

    let stream_routes: Vec<(String, String, repository::RouteMetadata)> = metadata_list
        .iter()
        .filter(|metadata| metadata.initialized)
        .flat_map(|metadata| {
            metadata
                .registrations
                .iter()
                .filter(|(_, route_meta)| route_meta.kind == repository::RouteKind::Stream)
                .map(|((path, _), route_meta)| {
                    (path.clone(), metadata.uri.clone(), route_meta.clone())
                })
                .collect::<Vec<_>>()
        })
        .collect();

    // Script-registered HTTP routes
    for metadata in metadata_list {
        if metadata.initialized && !metadata.registrations.is_empty() {
            for ((path, method), route_meta) in metadata.registrations {
                if route_meta.kind != repository::RouteKind::Handler {
                    continue;
                }
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

    // Asset routes
    for (path, script_uri, asset_name, registration_meta) in file_routes {
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
        let asset_summary = registration_meta
            .summary
            .clone()
            .unwrap_or_else(|| format!("Static asset: {}", asset_name));
        asset_operation.insert("summary".to_string(), json!(asset_summary));
        let asset_description = registration_meta.description.clone().unwrap_or_else(|| {
            format!(
                "Serves static asset '{}' registered by script '{}'",
                asset_name, script_uri
            )
        });
        asset_operation.insert("description".to_string(), json!(asset_description));
        let asset_tags = if registration_meta.tags.is_empty() {
            vec!["Assets".to_string()]
        } else {
            registration_meta.tags.clone()
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
        asset_operation.insert("x-asset-name".to_string(), json!(asset_name));
        asset_operation.insert("x-script-uri".to_string(), json!(script_uri));
        asset_operation.insert("x-source".to_string(), json!("file-route"));

        let path_entry = js_paths.entry(path).or_insert_with(|| json!({}));
        if let Some(path_obj) = path_entry.as_object_mut() {
            path_obj.insert("get".to_string(), json!(asset_operation));
        }
    }

    // SSE stream routes, the scripts' and the engine's own
    let stream_routes: Vec<(String, String, repository::RouteMetadata)> = stream_routes
        .into_iter()
        .chain(
            engine_owned_streams()
                .into_iter()
                .map(|(path, script_uri)| {
                    (path, script_uri, repository::RouteMetadata::stream(None))
                }),
        )
        .collect();
    for (path, script_uri, metadata) in stream_routes {
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
        // The engine's own operations, from the table `/mcp` reads.
        for (path, operations) in crate::engine_http::openapi_paths() {
            rust_paths.insert(path, operations);
        }
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

    // What a caller — or a script written against this engine — may spend, as
    // this deployment is configured. A limit nobody can find is one every
    // caller meets by surprise, and the numbers here are read from the code
    // that enforces them rather than retyped beside it.
    match serde_json::to_value(crate::limits::snapshot()) {
        Ok(limits) => {
            if let Some(spec) = rust_spec.as_object_mut() {
                spec.insert("x-aiwebengine-limits".to_string(), limits);
            }
        }
        Err(e) => warn!(
            "Failed to serialize engine limits for the OpenAPI document: {}",
            e
        ),
    }

    match serde_json::to_string_pretty(&rust_spec) {
        Ok(json) => json,
        Err(e) => refuse(
            Refusal::Failed,
            format!("Failed to serialize merged OpenAPI spec: {}", e),
        )
        .to_string(),
    }
}

pub(super) fn tool_exposure_report(_args: &Value, user: &UserContext) -> Value {
    match exposure_report_authorized(user) {
        Ok(report) => json!({
            "publicDir": crate::exposure::PUBLIC_DIR,
            "resourceDir": crate::exposure::RESOURCE_DIR,
            "enforced": true,
            "refused": report.refused,
            "unclassified": report.unclassified,
            "scripts": report.scripts,
            "collided": report.collisions.len(),
            "collisions": report.collisions,
            "timestamp": iso_timestamp(),
        }),
        Err(e) => refuse(
            Refusal::Failed,
            format!("Failed to build the exposure report: {}", e),
        ),
    }
}

pub(super) fn tool_list_routes(args: &Value, user: &UserContext) -> Value {
    let host = arg_str(args, "host");
    match routes_introspection_authorized(user) {
        Ok(routes) => {
            let routes = match host {
                Some(host) => crate::database::run_blocking(filter_routes_by_host(
                    routes,
                    &crate::hosts::canonical_host(Some(host)),
                )),
                None => routes,
            };
            json!({
            "host": host,
            "routes": routes,
            "count": routes.len(),
            "timestamp": iso_timestamp(),
            })
        }
        Err(e) => refuse(Refusal::Failed, format!("Failed to list routes: {}", e)),
    }
}
