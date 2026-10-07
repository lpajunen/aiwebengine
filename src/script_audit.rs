//! A record a script can add to and cannot take back (`audit.record`).
//!
//! `console` is the wrong place for "a refund was issued" or "a role was
//! changed": the log is pruned to a count and `clear_logs` empties it, and the
//! line names whoever the script says it does. Here the script supplies what
//! happened and the engine supplies who — the execution's principal and the
//! address the edge judged — and nothing reachable by a script or an operation
//! deletes a row. Rows leave by age, or with their script.
//!
//! An event is written on the engine's own connection rather than inside the
//! invocation's transaction, so a handler that records something and then fails
//! still leaves the record. That is the point of an audit trail: what was
//! attempted, and by whom, survives the attempt.

use serde::Serialize;
use serde_json::Value;

/// The longest action name, in bytes.
pub const MAX_ACTION_BYTES: usize = 128;
/// The largest `details`, serialized, in bytes.
pub const MAX_DETAILS_BYTES: usize = 16 * 1024;
/// The most events one read answers with.
pub const MAX_READ: i64 = 1000;

/// One event to record. Everything but `action` and `details` is the
/// engine's to fill in.
#[derive(Debug, Clone)]
pub struct NewAuditEvent {
    pub script_uri: String,
    pub action: String,
    pub details: Option<Value>,
    pub actor_kind: &'static str,
    pub actor_id: Option<String>,
    pub client_ip: Option<String>,
    pub request_id: Option<String>,
}

/// One recorded event, as a read answers with it.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AuditEvent {
    pub id: i64,
    pub action: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<Value>,
    pub actor_kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub actor_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_ip: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

/// Why an event cannot be recorded as given, or `None` when it can.
pub fn refusal(action: &str, details: Option<&Value>) -> Option<String> {
    if action.trim().is_empty() {
        return Some("an action must be named".to_string());
    }
    if action.len() > MAX_ACTION_BYTES {
        return Some(format!(
            "the action is {} bytes; at most {} are kept",
            action.len(),
            MAX_ACTION_BYTES
        ));
    }
    if let Some(details) = details {
        if !details.is_object() {
            return Some("details must be an object".to_string());
        }
        let size = details.to_string().len();
        if size > MAX_DETAILS_BYTES {
            return Some(format!(
                "details are {} bytes serialized; at most {} are kept",
                size, MAX_DETAILS_BYTES
            ));
        }
    }
    None
}

fn pool() -> Result<sqlx::PgPool, sqlx::Error> {
    crate::database::get_global_database()
        .map(|db| db.pool().clone())
        .ok_or_else(|| sqlx::Error::Configuration("no database".into()))
}

/// Records `event`, returning its id.
pub async fn record(event: NewAuditEvent) -> Result<i64, sqlx::Error> {
    sqlx::query_scalar(
        "INSERT INTO script_audit_events
             (script_uri, action, details, actor_kind, actor_id, client_ip, request_id)
         VALUES ($1, $2, $3, $4, $5, $6, $7)
         RETURNING id",
    )
    .bind(&event.script_uri)
    .bind(&event.action)
    .bind(&event.details)
    .bind(event.actor_kind)
    .bind(&event.actor_id)
    .bind(&event.client_ip)
    .bind(&event.request_id)
    .fetch_one(&pool()?)
    .await
}

/// One script's events, newest first: at most `limit`, only those named
/// `action` when given, and only those older than `before_id` when given — the
/// smallest id of the last page, to read further back.
pub async fn query(
    script_uri: &str,
    action: Option<&str>,
    before_id: Option<i64>,
    limit: i64,
) -> Result<Vec<AuditEvent>, sqlx::Error> {
    let rows = sqlx::query_as::<
        _,
        (
            i64,
            String,
            Option<Value>,
            String,
            Option<String>,
            Option<String>,
            Option<String>,
            chrono::DateTime<chrono::Utc>,
        ),
    >(
        "SELECT id, action, details, actor_kind, actor_id, client_ip, request_id, created_at
           FROM script_audit_events
          WHERE script_uri = $1
            AND ($2::TEXT IS NULL OR action = $2)
            AND ($3::BIGINT IS NULL OR id < $3)
          ORDER BY id DESC
          LIMIT $4",
    )
    .bind(script_uri)
    .bind(action)
    .bind(before_id)
    .bind(limit.clamp(1, MAX_READ))
    .fetch_all(&pool()?)
    .await?;

    Ok(rows
        .into_iter()
        .map(
            |(id, action, details, actor_kind, actor_id, client_ip, request_id, created_at)| {
                AuditEvent {
                    id,
                    action,
                    details,
                    actor_kind,
                    actor_id,
                    client_ip,
                    request_id,
                    created_at,
                }
            },
        )
        .collect())
}

/// Deletes events older than `retention_days`, returning how many. Zero keeps
/// them for as long as their script exists.
pub async fn prune(retention_days: u32) -> Result<u64, sqlx::Error> {
    if retention_days == 0 {
        return Ok(0);
    }
    let Ok(pool) = pool() else {
        return Ok(0);
    };
    Ok(sqlx::query(
        "DELETE FROM script_audit_events
          WHERE created_at < NOW() - make_interval(days => $1)",
    )
    .bind(retention_days as i32)
    .execute(&pool)
    .await?
    .rows_affected())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn an_event_needs_a_named_action_and_object_details_of_bounded_size() {
        assert!(refusal("refund.issued", Some(&json!({ "amount": 10 }))).is_none());
        assert!(refusal("refund.issued", None).is_none());
        assert!(refusal("  ", None).is_some());
        assert!(refusal(&"a".repeat(MAX_ACTION_BYTES + 1), None).is_some());
        assert!(refusal("x", Some(&json!([1, 2]))).is_some());
        let large = json!({ "blob": "a".repeat(MAX_DETAILS_BYTES) });
        assert!(refusal("x", Some(&large)).is_some());
    }
}
