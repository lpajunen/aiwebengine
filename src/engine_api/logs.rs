//! Reading a script's log and audit trail, and following the log live.

use super::*;
use crate::auth::AuthUser;
use crate::error::AppResult;
use crate::repository;
use crate::security::{Capability, UserContext};
use axum::extract::{Extension, Query};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use serde_json::{Value, json};
use tracing::warn;

/// One log entry as JSON. `scriptUri` is what lets an all-scripts listing
/// attribute each line to the script that logged it, and `requestId`/`kind`/
/// `route` are what let a caller pull one invocation's lines out of it.
///
/// `seq` is the entry's position in the engine's write order: pass the last one
/// seen back as `after_seq` to read only what has been written since.
pub fn log_entry_json(entry: &repository::LogEntry) -> Value {
    let timestamp_ms = entry
        .timestamp
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as f64;
    json!({
        "scriptUri": entry.script_uri,
        "message": entry.message,
        "level": entry.level,
        "timestamp": timestamp_ms,
        "seq": entry.seq,
        "requestId": entry.context.request_id,
        "kind": entry.context.kind,
        "route": entry.context.route,
        "revision": entry.context.revision,
    })
}

/// Run a filtered log query, newest first. See
/// [`query_log_entries_authorized`] for who may read what.
///
/// Denial is an error, not an empty result: over HTTP a caller has to be able
/// to tell "you may not read these" from "there is nothing to read". The
/// sandbox convention of answering `[]` belongs to the JS globals, where the
/// script has no status code to receive.
pub fn query_logs_authorized(
    user: &UserContext,
    query: &repository::LogQuery,
) -> AppResult<Vec<Value>> {
    Ok(query_log_entries_authorized(user, query)?
        .iter()
        .map(log_entry_json)
        .collect())
}

/// As [`query_logs_authorized`], but answering with the entries themselves.
///
/// The live tail needs each entry's `seq` to advance its cursor, and reading it
/// back out of the JSON would be a worse kind of coupling. Both functions go
/// through this one so the capability check exists once.
///
/// `ViewLogs`, and then the script's owner or an administrator: a log carries
/// whatever a script wrote while serving people, so it is read by whoever
/// answers for the script, as its audit trail is. A query naming no script
/// spans every script for an administrator and the caller's own for anyone
/// else.
pub fn query_log_entries_authorized(
    user: &UserContext,
    query: &repository::LogQuery,
) -> AppResult<Vec<repository::LogEntry>> {
    user.require_capability(&Capability::ViewLogs)?;
    if has_admin_capability(user) {
        return repository::query_log_messages(query);
    }

    let refused = || crate::error::AppError::AuthorizationFailed {
        message: match &query.script_uri {
            Some(uri) => format!(
                "You must be an administrator or owner to read the logs of script '{}'",
                uri
            ),
            None => "Reading logs takes being signed in as the owner of a script".to_string(),
        },
    };
    let Some(user_id) = user.user_id.clone() else {
        return Err(refused());
    };
    if let Some(uri) = &query.script_uri
        && !user_owns_script(user, uri)
    {
        warn!(
            user_id = %user_id,
            script_name = %uri,
            "Permission denied: only an administrator or owner may read a script's logs"
        );
        return Err(refused());
    }
    repository::query_log_messages(&repository::LogQuery {
        owned_by: Some(user_id),
        ..query.clone()
    })
}

/// Clear one script's logs; `DeleteLogs` and ownership of the script, or an
/// administrator.
///
/// Naming the script is required. This used to accept no `uri` as "prune every
/// script back to its newest entries", which is now what the background pruner
/// does on its own schedule — and which, reachable here, let anyone holding
/// the editor-tier `DeleteLogs` truncate the logs of every script in the
/// engine, on hosts they had nothing to do with. Acting on what you do not own
/// is what `AdministerEngine` marks.
pub fn delete_logs_authorized(user: &UserContext, uri: &str) -> AppResult<Value> {
    user.require_capability(&Capability::DeleteLogs)?;
    if !is_admin_or_owner(user, uri) {
        warn!(
            user_id = ?user.user_id,
            script_name = %uri,
            "Permission denied: only an administrator or owner may clear a script's logs"
        );
        return Err(crate::error::AppError::AuthorizationFailed {
            message: format!(
                "You must be an administrator or owner to clear the logs of script '{}'",
                uri
            ),
        });
    }
    repository::clear_log_messages(uri)?;
    Ok(json!({
        "uri": uri,
        "cleared": true,
        "timestamp": iso_timestamp(),
    }))
}

/// Parse a `since` bound given either as epoch milliseconds or RFC 3339.
pub(super) fn parse_since(raw: &str) -> Option<std::time::SystemTime> {
    if let Ok(millis) = raw.parse::<i64>() {
        let millis = u64::try_from(millis).ok()?;
        return Some(std::time::UNIX_EPOCH + std::time::Duration::from_millis(millis));
    }
    chrono::DateTime::parse_from_rfc3339(raw)
        .ok()
        .map(|dt| std::time::SystemTime::from(dt.with_timezone(&chrono::Utc)))
}

/// How often a live tail looks for entries written since its cursor.
///
/// Short enough that a person driving a client sees their own actions land,
/// long enough that an idle tail is a negligible query.
pub(super) const LOG_TAIL_POLL_INTERVAL: std::time::Duration =
    std::time::Duration::from_millis(500);

/// Entries one poll may carry. A burst larger than this is delivered over the
/// following polls rather than in one unbounded write, and nothing is dropped:
/// the cursor only advances past what was actually sent.
pub(super) const LOG_TAIL_BATCH_LIMIT: i64 = 500;

/// Consecutive failed polls tolerated before the tail gives up and says so.
/// One failure is a blip worth riding out; a run of them is a tail that has
/// stopped being a tail, and a client that is told can reconnect from its last
/// seq instead of silently missing everything.
pub(super) const LOG_TAIL_MAX_CONSECUTIVE_ERRORS: u32 = 3;

/// Most entries a tail will replay before going live.
pub(super) const LOG_TAIL_MAX_BACKLOG: i64 = 1000;

/// Filters and starting point for a live tail. The filters are the ones
/// [`LogParams`] carries; what differs is where the stream starts.
#[derive(Deserialize, Default)]
pub struct LogTailParams {
    pub(super) script: Option<String>,
    pub(super) level: Option<String>,
    pub(super) contains: Option<String>,
    pub(super) request_id: Option<String>,
    pub(super) kind: Option<String>,
    pub(super) route: Option<String>,
    pub(super) revision: Option<i32>,
    /// Start after this `seq`, replaying everything written since. This is how
    /// a dropped tail resumes without a gap: the client reconnects with the
    /// last seq it saw.
    pub(super) after_seq: Option<i64>,
    /// Start at this time (epoch millis or RFC 3339) instead of at the end of
    /// the log. Ignored when `after_seq` says where to start.
    pub(super) since: Option<String>,
    /// Replay this many of the newest matching entries before going live.
    /// Ignored when `after_seq` or `since` says where to start.
    pub(super) backlog: Option<i64>,
}

impl LogTailParams {
    /// The filters, with the starting point left to the caller.
    pub(super) fn to_query(&self) -> repository::LogQuery {
        repository::LogQuery {
            script_uri: self.script.clone(),
            level: self.level.clone(),
            since: None,
            after_seq: None,
            contains: self.contains.clone(),
            request_id: self.request_id.clone(),
            kind: self.kind.clone(),
            revision: self.revision,
            route: self.route.clone(),
            limit: None,
            // Set by the authorization check, never by the caller.
            owned_by: None,
        }
    }
}

/// One SSE event carrying a log entry.
pub(super) fn log_tail_event(entry: &repository::LogEntry) -> axum::response::sse::Event {
    axum::response::sse::Event::default()
        .event("log")
        .data(log_entry_json(entry).to_string())
}

/// Run one filtered log query off the async runtime.
///
/// The repository's query is blocking, and a tail runs one every poll for as
/// long as the client stays connected — running them inline would park a
/// runtime worker for the lifetime of every open tail.
pub(super) async fn query_logs_off_runtime(
    user: UserContext,
    query: repository::LogQuery,
) -> AppResult<Vec<repository::LogEntry>> {
    match tokio::task::spawn_blocking(move || query_log_entries_authorized(&user, &query)).await {
        Ok(result) => result,
        Err(e) => Err(crate::error::AppError::internal(format!(
            "Log query task failed: {}",
            e
        ))),
    }
}

/// Follow a script's log as it is written, as Server-Sent Events.
///
/// Answers the question a one-shot listing cannot: what is this script doing
/// *now*. The tick, lease and stream paths are the hardest to debug precisely
/// because their output interleaves with every other invocation's, so the same
/// filters the listing takes apply here — narrowing a tail to one route, one
/// invocation kind or one request id is what makes watching a live session
/// legible.
///
/// Entries are delivered oldest-first as `log` events whose data is the same
/// JSON the listing returns. Each carries a `seq`; reconnecting with
/// `after_seq` set to the last one seen resumes without a gap.
///
/// Polls the database rather than being pushed to from the write path: every
/// instance in a cluster writes to the same table, so a tail sees the whole
/// cluster's output without a message bus, and it shows what was actually
/// committed rather than lines a rolled-back transaction never kept.
#[utoipa::path(
    get,
    path = "/engine/script_logs/stream",
    tags = ["Logging"],
    params(
        ("script" = Option<String>, Query, description = "Script name; omit to tail every script"),
        ("level" = Option<String>, Query, description = "Only entries at this level, e.g. ERROR"),
        ("contains" = Option<String>, Query, description = "Only entries whose message contains this substring"),
        ("request_id" = Option<String>, Query, description = "Only the entries one invocation emits"),
        ("kind" = Option<String>, Query, description = "Only entries from invocations of this kind, e.g. httpRoute, scheduled"),
        ("route" = Option<String>, Query, description = "Only entries logged while serving this registered route pattern"),
        ("revision" = Option<i32>, Query, description = "Only entries written while this revision of the script was running"),
        ("after_seq" = Option<i64>, Query, description = "Resume after this seq, replaying everything written since"),
        ("since" = Option<String>, Query, description = "Start at this time (epoch millis or RFC 3339) instead of at the end of the log"),
        ("backlog" = Option<i64>, Query, description = "Replay this many of the newest matching entries before going live"),
    ),
    responses(
        (status = 200, description = "Event stream of log entries"),
        (status = 400, description = "Invalid query parameter"),
        (status = 403, description = "Access denied"),
    )
)]
pub async fn script_logs_stream_route(
    auth_user: Option<Extension<AuthUser>>,
    Query(params): Query<LogTailParams>,
) -> Response {
    let user = user_context_from(auth_user.as_deref());

    let since = match params.since.as_deref() {
        Some(raw) => match parse_since(raw) {
            Some(since) => Some(since),
            None => {
                return error_response(
                    StatusCode::BAD_REQUEST,
                    format!("Invalid 'since' value: {}", raw),
                );
            }
        },
        None => None,
    };
    if params.backlog.is_some_and(|backlog| backlog < 0) {
        return error_response(
            StatusCode::BAD_REQUEST,
            "Parameter 'backlog' must not be negative".to_string(),
        );
    }
    if params.after_seq.is_some_and(|seq| seq < 0) {
        return error_response(
            StatusCode::BAD_REQUEST,
            "Parameter 'after_seq' must not be negative".to_string(),
        );
    }

    // The opening query doubles as the authorization check: a caller without
    // ViewLogs is refused with a status code here, rather than being handed an
    // event stream that would never carry anything.
    let opening = {
        let mut query = params.to_query();
        match (params.after_seq, since) {
            // Resuming: everything written since the cursor, oldest first.
            (Some(after_seq), _) => {
                query.after_seq = Some(after_seq);
                query.limit = Some(LOG_TAIL_BATCH_LIMIT);
            }
            // Starting from a time. The cursor makes the batch the *oldest*
            // entries at or after it, which is what lets the poll loop carry
            // the rest forward; taking the newest instead would silently drop
            // everything between the requested time and the last page.
            (None, Some(since)) => {
                query.since = Some(since);
                query.after_seq = Some(0);
                query.limit = Some(LOG_TAIL_BATCH_LIMIT);
            }
            // Starting at the end: the requested backlog, or nothing at all.
            // Even a backlog of zero reads one entry, which is what tells the
            // tail the seq to start after; that entry is not sent on.
            (None, None) => {
                query.limit = Some(params.backlog.unwrap_or(0).clamp(1, LOG_TAIL_MAX_BACKLOG))
            }
        }
        query
    };
    let replay_opening =
        params.after_seq.is_some() || since.is_some() || params.backlog.unwrap_or(0) > 0;

    let mut opening_entries = match query_logs_off_runtime(user.clone(), opening).await {
        Ok(entries) => entries,
        Err(e) => {
            let status =
                StatusCode::from_u16(e.status_code()).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
            return error_response(status, format!("Failed to fetch logs: {}", e));
        }
    };
    // Queries answer newest-first; a tail reads in the order things happened.
    opening_entries.reverse();

    // Where the live tail picks up: after the last entry the opening query
    // named. With nothing to replay, finding that entry was all the opening
    // query was for, so it is not sent on.
    let opening_cursor = opening_entries.last().map(|entry| entry.seq);
    if !replay_opening {
        opening_entries.clear();
    }

    let mut cursor = match opening_cursor {
        Some(cursor) => cursor,
        // The opening query matched nothing, so it named no entry to start
        // after. Starting at zero would make the first poll replay the log from
        // the beginning, so the tail starts at the end of it instead — a filter
        // that has not matched yet waits for a line that does. The fallback
        // reads the newest entry written by anyone, since the filters have
        // already been shown to match nothing that exists.
        None => {
            let newest = repository::LogQuery {
                limit: Some(1),
                ..Default::default()
            };
            match query_logs_off_runtime(user.clone(), newest).await {
                Ok(entries) => entries.first().map(|entry| entry.seq).unwrap_or_default(),
                Err(e) => {
                    let status = StatusCode::from_u16(e.status_code())
                        .unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
                    return error_response(status, format!("Failed to fetch logs: {}", e));
                }
            }
        }
    };

    let filters = params.to_query();
    let stream = async_stream::stream! {
        // Tells the client the tail is live and where it starts, so it can
        // resume from here even if nothing is ever logged.
        yield Ok::<_, std::convert::Infallible>(
            axum::response::sse::Event::default()
                .event("open")
                .data(json!({ "seq": cursor, "timestamp": iso_timestamp() }).to_string()),
        );

        for entry in &opening_entries {
            yield Ok(log_tail_event(entry));
        }

        let mut consecutive_errors = 0u32;
        loop {
            tokio::time::sleep(LOG_TAIL_POLL_INTERVAL).await;

            let mut query = filters.clone();
            query.after_seq = Some(cursor);
            query.limit = Some(LOG_TAIL_BATCH_LIMIT);

            let mut entries = match query_logs_off_runtime(user.clone(), query).await {
                Ok(entries) => {
                    consecutive_errors = 0;
                    entries
                }
                Err(e) => {
                    consecutive_errors += 1;
                    warn!("Log tail query failed ({}): {}", consecutive_errors, e);
                    if consecutive_errors >= LOG_TAIL_MAX_CONSECUTIVE_ERRORS {
                        yield Ok(axum::response::sse::Event::default().event("error").data(
                            json!({
                                "error": format!("Log tail stopped: {}", e),
                                "seq": cursor,
                            })
                            .to_string(),
                        ));
                        break;
                    }
                    continue;
                }
            };

            entries.reverse();
            for entry in &entries {
                // Advance only past what has been sent, so a batch cut short by
                // the limit resumes at the right place on the next poll.
                cursor = entry.seq;
                yield Ok(log_tail_event(entry));
            }
        }
    };

    axum::response::Sse::new(stream)
        .keep_alive(axum::response::sse::KeepAlive::default())
        .into_response()
}

pub(super) fn tool_read_logs(args: &Value, user: &UserContext) -> Value {
    let uri = arg_str(args, "script");
    let since = match arg_str(args, "since") {
        Some(raw) => match parse_since(raw) {
            Some(since) => Some(since),
            None => {
                return refuse(
                    Refusal::BadRequest,
                    format!("Invalid 'since' value: {}", raw),
                );
            }
        },
        None => None,
    };
    let limit = args.get("limit").and_then(Value::as_i64);
    if limit.is_some_and(|limit| limit <= 0) {
        return refuse(
            Refusal::BadRequest,
            "Parameter 'limit' must be greater than zero",
        );
    }

    let query = repository::LogQuery {
        script_uri: uri.map(str::to_string),
        level: arg_str(args, "level").map(str::to_string),
        since,
        after_seq: args.get("after_seq").and_then(Value::as_i64),
        contains: arg_str(args, "contains").map(str::to_string),
        request_id: arg_str(args, "request_id").map(str::to_string),
        kind: arg_str(args, "kind").map(str::to_string),
        revision: args
            .get("revision")
            .and_then(Value::as_i64)
            .map(|revision| revision as i32),
        route: arg_str(args, "route").map(str::to_string),
        limit,
        // Set by the authorization check, never by the caller.
        owned_by: None,
    };

    match query_logs_authorized(user, &query) {
        Ok(mut logs) => {
            // Oldest-first for a single script, as its own log view reads.
            if uri.is_some() {
                logs.reverse();
            }
            json!({
                "uri": uri,
                "logs": logs,
                "count": logs.len(),
                "timestamp": iso_timestamp(),
            })
        }
        Err(e) => refuse_app(&e, format!("Failed to fetch logs: {}", e)),
    }
}

pub(super) fn tool_read_audit(args: &Value, user: &UserContext) -> Value {
    let Some(uri) = arg_str(args, "script") else {
        return missing_arg("script");
    };
    let limit = args.get("limit").and_then(Value::as_i64).unwrap_or(100);
    if limit <= 0 {
        return refuse(
            Refusal::BadRequest,
            "Parameter 'limit' must be greater than zero",
        );
    }
    match read_audit_authorized(
        user,
        uri,
        arg_str(args, "action"),
        args.get("before_id").and_then(Value::as_i64),
        limit,
    ) {
        Ok(events) => json!({
            "script": uri,
            "events": events,
            "count": events.len(),
            "timestamp": iso_timestamp(),
        }),
        Err(refusal) => refusal,
    }
}

/// A script's audit events: `ViewLogs` and ownership of the script, or an
/// administrator — the rule its log is read under, since an audit trail names
/// people and addresses and is read by whoever answers for the solution.
pub fn read_audit_authorized(
    user: &UserContext,
    uri: &str,
    action: Option<&str>,
    before_id: Option<i64>,
    limit: i64,
) -> Result<Vec<crate::script_audit::AuditEvent>, Value> {
    if !user.has_capability(&Capability::ViewLogs) || !is_admin_or_owner(user, uri) {
        return Err(refuse(
            Refusal::Forbidden,
            "Only the script's owner or an administrator may read its audit events",
        ));
    }
    crate::database::run_blocking(crate::script_audit::query(uri, action, before_id, limit))
        .map_err(|e| {
            refuse(
                Refusal::Failed,
                format!("Failed to read audit events: {}", e),
            )
        })
}

pub(super) fn tool_clear_logs(args: &Value, user: &UserContext) -> Value {
    let Some(uri) = arg_str(args, "script") else {
        return refuse(
            Refusal::BadRequest,
            "uri is required: name the script whose logs to clear",
        );
    };
    match delete_logs_authorized(user, uri) {
        Ok(body) => body,
        Err(e) => refuse(Refusal::Failed, format!("Failed to delete logs: {}", e)),
    }
}
