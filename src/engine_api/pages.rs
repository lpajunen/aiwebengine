//! Engine pages and HTTP-only routes: install, sign-in refusal, health, favicon,
//! OpenAPI.

use super::*;
use crate::auth::AuthUser;
use crate::repository;
use crate::security::Capability;
use axum::extract::{Extension, Query};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use serde_json::{Value, json};

/// Installation confirmation page, shown after a fresh install (the root
/// path redirects here until further routes are registered).
pub(super) fn installed_page_html(nonce: &str) -> String {
    crate::engine_page::document(
        "aiwebengine installed",
        nonce,
        crate::engine_page::Width::Narrow,
        r#"<h1>Thanks for installing aiwebengine!</h1>
        <p class="aw-identity aw-muted">Your server is up and running.</p>"#,
    )
}

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
    let nonce = crate::security::generate_nonce();
    crate::engine_page::response(StatusCode::OK, installed_page_html(&nonce), &nonce)
}

/// How many rows of lock detail one health check reports.
///
/// A wedged table produces one blocked waiter per stalled request, and they all
/// name the same holder. A handful is enough to identify it; the rest is
/// repetition an operator has to scroll past.
pub(super) const LOCK_DIAGNOSTIC_LIMIT: i64 = 10;

/// Statements currently waiting on a lock, and the sessions holding them.
///
/// A wedged table shows up as requests that never return; this tells that
/// apart from a slow script without reaching for `psql`. A blocked statement
/// names its own query, and `pg_blocking_pids` names whoever is in front of it,
/// which together identify a lock wedge without leaving the health check.
///
/// Reported alongside the oldest transaction on the server, because the holder
/// of a lock nobody can break is usually a transaction that stopped making
/// progress rather than one doing something slow.
pub async fn lock_diagnostics(pool: &sqlx::PgPool) -> Value {
    use sqlx::Row;

    let waiting = sqlx::query(
        r#"
        SELECT
            a.pid,
            a.state,
            a.wait_event_type,
            EXTRACT(EPOCH FROM (now() - a.xact_start))::float8 AS xact_age_seconds,
            left(a.query, 200) AS query,
            pg_blocking_pids(a.pid) AS blocked_by
        FROM pg_stat_activity a
        WHERE a.datname = current_database()
          AND cardinality(pg_blocking_pids(a.pid)) > 0
        ORDER BY a.xact_start
        LIMIT $1
        "#,
    )
    .bind(LOCK_DIAGNOSTIC_LIMIT)
    .fetch_all(pool)
    .await;

    let waiting = match waiting {
        Ok(rows) => rows
            .into_iter()
            .map(|row| {
                json!({
                    "pid": row.try_get::<i32, _>("pid").unwrap_or_default(),
                    "state": row.try_get::<Option<String>, _>("state").unwrap_or_default(),
                    "wait_event_type": row
                        .try_get::<Option<String>, _>("wait_event_type")
                        .unwrap_or_default(),
                    "transaction_age_seconds": row
                        .try_get::<Option<f64>, _>("xact_age_seconds")
                        .unwrap_or_default(),
                    "query": row.try_get::<Option<String>, _>("query").unwrap_or_default(),
                    "blocked_by": row
                        .try_get::<Vec<i32>, _>("blocked_by")
                        .unwrap_or_default(),
                })
            })
            .collect::<Vec<_>>(),
        Err(e) => {
            // A diagnostic that cannot run must not decide whether the engine
            // is healthy; the database ping above already answered that.
            return json!({ "available": false, "message": e.to_string() });
        }
    };

    let oldest = sqlx::query(
        r#"
        SELECT
            pid,
            state,
            EXTRACT(EPOCH FROM (now() - xact_start))::float8 AS xact_age_seconds,
            left(query, 200) AS query
        FROM pg_stat_activity
        WHERE datname = current_database() AND xact_start IS NOT NULL
        ORDER BY xact_start
        LIMIT 1
        "#,
    )
    .fetch_optional(pool)
    .await
    .ok()
    .flatten()
    .map(|row| {
        json!({
            "pid": row.try_get::<i32, _>("pid").unwrap_or_default(),
            "state": row.try_get::<Option<String>, _>("state").unwrap_or_default(),
            "age_seconds": row
                .try_get::<Option<f64>, _>("xact_age_seconds")
                .unwrap_or_default(),
            "query": row.try_get::<Option<String>, _>("query").unwrap_or_default(),
        })
    });

    json!({
        "available": true,
        "blocked_statements": waiting.len(),
        "waiting": waiting,
        "oldest_transaction": oldest,
    })
}

/// Detailed cluster diagnostics. Administrators only.
///
/// Unlike the unauthenticated `/health` liveness probe, this reports internal
/// topology — connection-pool metrics, notification-listener state, and
/// per-script scheduler job counts — so it lives under the authorized
/// `/engine` prefix rather than being world-readable.
///
/// Like `/health`, it verifies the database with a real `SELECT 1` ping and
/// returns 503 when that fails.
#[utoipa::path(
    get,
    path = "/engine/health/cluster",
    tags = ["Health"],
    responses(
        (status = 200, description = "Detailed cluster health information", body = crate::openapi_schemas::ClusterHealthResponse),
        (status = 403, description = "Permission denied"),
        (status = 503, description = "Cluster is unhealthy (database unreachable)", body = crate::openapi_schemas::ClusterHealthResponse),
    )
)]
pub async fn cluster_health_route(auth_user: Option<Extension<AuthUser>>) -> Response {
    let user = user_context_from(auth_user.as_deref());
    // Deliberately the stricter `is_user_admin` check, not
    // `has_admin_capability`: the latter passes on capability alone. Topology
    // diagnostics require a real admin session.
    if !is_user_admin(&user) {
        return error_response(
            StatusCode::FORBIDDEN,
            "Permission denied. You must be an administrator".to_string(),
        );
    }

    let server_id = crate::notifications::get_server_id().unwrap_or_else(|| "unknown".to_string());

    // Verify the database with a real query and report pool stats alongside it.
    let (db_healthy, pool_stats, locks) = if let Some(db) = crate::database::get_global_database() {
        let connected = db.health_check().await.is_ok();
        let pool = db.pool();
        let size = pool.size() as usize;
        let idle = pool.num_idle();
        let locks = if connected {
            lock_diagnostics(pool).await
        } else {
            json!({ "available": false, "message": "Database unreachable" })
        };
        (
            connected,
            json!({
                "available": true,
                "connected": connected,
                "active_connections": size.saturating_sub(idle),
                "idle_connections": idle,
                "max_connections": pool.options().get_max_connections(),
            }),
            locks,
        )
    } else {
        (
            false,
            json!({
                "available": false,
                "connected": false,
                "message": "Database not initialized (memory mode)"
            }),
            json!({ "available": false, "message": "Database not initialized" }),
        )
    };

    // Get notification listener status
    let listener_status = if crate::notifications::get_global_listener().is_some() {
        json!({
            "active": true,
            "server_id": server_id.clone(),
        })
    } else {
        json!({
            "active": false,
            "message": "Notification listener not initialized"
        })
    };

    // Get scheduler job counts per script
    let scheduler = crate::scheduler::get_scheduler();
    let job_counts = scheduler.get_job_counts();
    let total_jobs: usize = job_counts.values().sum();

    // Reported rather than folded into `status`: a lost thread does not make
    // the engine unreachable, and a probe that starts failing would take the
    // instance out of rotation for a condition that often clears itself. It is
    // what to alert on, not what to fail on.
    let census = crate::worker_census::snapshot();

    let status_code = if db_healthy {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    };

    json_response(
        status_code,
        json!({
            "status": if db_healthy { "healthy" } else { "unhealthy" },
            "instance_id": server_id,
            "timestamp": iso_timestamp(),
            "version": {
                "cargo": env!("CARGO_PKG_VERSION"),
                "git_commit": option_env!("VERGEN_GIT_SHA").unwrap_or(""),
                "git_commit_timestamp": option_env!("VERGEN_GIT_COMMIT_TIMESTAMP").unwrap_or(""),
                "build_timestamp": option_env!("VERGEN_BUILD_TIMESTAMP").unwrap_or("")
            },
            "database": pool_stats,
            "locks": locks,
            "workers": {
                "abandoned": census.abandoned,
                "recovered": census.recovered,
                "in_flight": census.in_flight,
            },
            "notification_listener": listener_status,
            "scheduler": {
                "total_jobs": total_jobs,
                "jobs_by_script": job_counts,
            }
        }),
    )
}

pub(super) fn html_escape(input: &str) -> String {
    input
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

#[derive(Deserialize, Default)]
pub struct UnauthorizedQuery {
    pub(super) attempted: Option<String>,
}

/// Insufficient permissions page, shown when an authenticated user lacks the
/// role required for the page they attempted to access.
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
                .filter(|email| *email != user_name)
                .map(|email| format!(r#" <span class="aw-muted">({})</span>"#, html_escape(email)))
                .unwrap_or_default();
            format!(
                r#"
        <div class="aw-detail">
            <span class="aw-detail-label">Signed in as</span>
            {}{}
        </div>"#,
                html_escape(user_name),
                email_suffix
            )
        }
        None => String::new(),
    };

    let attempted_path_block = match attempted {
        Some(path) => format!(
            r#"
        <div class="aw-detail">
            <span class="aw-detail-label">Attempted to access</span>
            <code>{}</code>
        </div>"#,
            html_escape(path)
        ),
        None => String::new(),
    };

    let action_link = match auth_user {
        Some(_) => r#"<a class="aw-button" href="/auth/logout">Sign out</a>"#.to_string(),
        None => {
            let redirect_suffix = attempted
                .map(|path| format!("?redirect={}", urlencoding::encode(path)))
                .unwrap_or_default();
            format!(
                r#"<a class="aw-button" href="/auth/login{}">Sign in</a>"#,
                redirect_suffix
            )
        }
    };

    // Names the one <style> block the shell writes and nothing else, so
    // anything injected into this page stays inert. Fresh per response — a
    // nonce a caller can predict is not a nonce. The sheet is inlined rather
    // than linked because this page is shown when something has already gone
    // wrong, and must not depend on another resource being served.
    let nonce = crate::security::generate_nonce();

    let body = format!(
        r#"<h1>Insufficient permissions</h1>
        <p class="aw-identity aw-muted">You don't have the required permissions to access this
            resource.</p>{user_info_block}{attempted_path_block}
        <div class="aw-detail">
            <span class="aw-detail-label">Why am I seeing this?</span>
            <p class="aw-explain">This page or feature requires <strong>Editor</strong> or
                <strong>Administrator</strong> privileges. Your current account does not have
                these permissions.</p>
            <span class="aw-detail-label">What can I do?</span>
            <ul class="aw-list aw-explain">
                <li>Contact your system administrator to request the appropriate role</li>
                <li>Verify you're signed in with the correct account</li>
                <li>Return to the home page to access features available to you</li>
            </ul>
        </div>
        <div class="aw-actions">
            <a class="aw-button aw-button--secondary" href="/">Go to home</a>
            {action_link}
        </div>"#
    );

    crate::engine_page::response(
        StatusCode::FORBIDDEN,
        crate::engine_page::document(
            "Insufficient permissions",
            &nonce,
            crate::engine_page::Width::Narrow,
            &body,
        ),
        &nonce,
    )
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
            refuse(Refusal::Forbidden, "Insufficient permissions"),
        );
    }

    let spec = tokio::task::spawn_blocking(generate_merged_openapi_spec)
        .await
        .unwrap_or_else(|e| refuse(Refusal::BadRequest, format!("join error: {}", e)).to_string());

    (StatusCode::OK, [("content-type", "application/json")], spec).into_response()
}
