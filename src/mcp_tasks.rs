//! Long-running tool calls, as the MCP tasks extension models them.
//!
//! `io.modelcontextprotocol/tasks` is the answer to a tool call that will not
//! finish inside a request: rather than holding the connection open, the server
//! returns a durable handle and the client polls `tasks/get` until the status is
//! terminal. The engine had the hard half already — [`crate::tasks`] is a
//! durable queue with attempts, backoff, leases and lanes — and what it lacked
//! was the mapping.
//!
//! # What decides that a call becomes a task
//!
//! Not the engine, and not a flag on the request. The specification is explicit
//! that this is server-directed: the client opts in once by declaring the
//! extension, and then handles whichever result shape arrives. But the engine
//! cannot know that a handler will be slow — by the time it could measure, the
//! handler has already started and the answer is a value, not a handle.
//!
//! So the *script* decides, from inside the handler, with `mcp.task`. That is
//! the same shape as `mcp.ask` ([`crate::mcp_elicitation`]): a call that ends
//! the current execution and changes what the client is answered with. A
//! handler that knows its work is long says so, and the work moves to the
//! queue.
//!
//! # Why a second table
//!
//! `script_tasks` deletes a row on success, deliberately: one row per success
//! grows the queue for the outcome nobody debugs, and what the task did is in
//! the script's log. An MCP task is the opposite case — the client has *not*
//! seen the answer, so a completed task is exactly the row that has to survive.
//! Folding the two together would mean making the queue keep its successes,
//! which is the decision it made the other way for good reasons.
//!
//! So `mcp_tasks` holds the client-facing state and `script_tasks` goes on being
//! a queue. The link is `script_task_id`, and it is nullable because the MCP row
//! outlives the queue row it points at.
//!
//! # What is not implemented, and why
//!
//! `input_required`. The extension lets a task surface `inputRequests` and take
//! answers through `tasks/update`, which is the elicitation machinery applied to
//! background work. The engine can express the status and refuses to produce it,
//! because "a queued run asks the person a question" is a design question rather
//! than a plumbing one: a task runs in script context or as a delegated user who
//! is by definition away, and there is no established answer to who is being
//! asked or whether they are there to answer. `tasks/update` is still served —
//! the specification says to acknowledge and ignore responses for keys that are
//! not outstanding, and with no `input_required` state every key is that.

use chrono::{DateTime, Duration, Utc};
use serde_json::{Value, json};
use sqlx::Row;
use tracing::{debug, warn};
use uuid::Uuid;

/// How long a finished task stays fetchable.
///
/// The extension allows `null` for "no expiry" and the engine never uses it: a
/// table of results nobody collected is a table that only grows. An hour is the
/// span of a client that disconnected and came back, which is the case the
/// handle exists for; anything longer is a client that is not coming.
const TASK_TTL: Duration = Duration::hours(1);

/// What a client is told to wait between polls.
///
/// Advice rather than a rule, and deliberately not tuned per task: a server
/// that could predict how long its work takes would not need tasks.
const POLL_INTERVAL_MS: u64 = 1000;

/// The extension's identifier, in capabilities on both sides.
pub const EXTENSION: &str = "io.modelcontextprotocol/tasks";

/// One task, as the extension's `Task` and `DetailedTask` describe it.
#[derive(Debug, Clone)]
pub struct McpTask {
    pub task_id: Uuid,
    pub script_uri: String,
    /// The MCP method the original call used, which decides how a stored
    /// result is shaped on the way out.
    pub method: String,
    pub status: String,
    pub status_message: Option<String>,
    pub result: Option<Value>,
    pub error: Option<Value>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
}

impl McpTask {
    /// Whether the status is one that will not change again.
    pub fn is_terminal(&self) -> bool {
        matches!(self.status.as_str(), "completed" | "failed" | "cancelled")
    }

    /// The `Task` fields every shape of this shares.
    ///
    /// `ttlMs` is what is *left*, not what was granted: the client is deciding
    /// whether it still has time to come back, and the age of the row is not
    /// its business.
    fn base(&self) -> serde_json::Map<String, Value> {
        let remaining = (self.expires_at - Utc::now()).num_milliseconds().max(0);
        let mut map = serde_json::Map::new();
        map.insert("taskId".to_string(), json!(self.task_id.to_string()));
        map.insert("status".to_string(), json!(self.status));
        map.insert("createdAt".to_string(), json!(self.created_at.to_rfc3339()));
        map.insert(
            "lastUpdatedAt".to_string(),
            json!(self.updated_at.to_rfc3339()),
        );
        map.insert("ttlMs".to_string(), json!(remaining));
        if let Some(message) = &self.status_message {
            map.insert("statusMessage".to_string(), json!(message));
        }
        // Only while there is something to poll for. A client that has a
        // terminal status and is still being told how often to poll has been
        // given advice about a question it has stopped asking.
        if !self.is_terminal() {
            map.insert("pollIntervalMs".to_string(), json!(POLL_INTERVAL_MS));
        }
        map
    }

    /// The `CreateTaskResult` a request answers with when it becomes a task.
    ///
    /// `resultType: "task"` rather than `"complete"`, which is what tells a
    /// client this is a handle and not an answer.
    pub fn create_result(&self) -> Value {
        let mut map = self.base();
        map.insert("resultType".to_string(), json!("task"));
        Value::Object(map)
    }

    /// What the original call would have returned synchronously.
    ///
    /// The stored value is the handler's own return, exactly as the
    /// synchronous path receives it — and the synchronous path then wraps it
    /// before answering. Doing the same wrapping here is what stops a client
    /// getting a *different shape* depending on whether its work happened to be
    /// queued, which would make the extension something a client has to branch
    /// on rather than a transport detail.
    ///
    /// Shaped at read rather than stored shaped, so the row holds what the
    /// handler said and the protocol layer does the protocol's work — the same
    /// division as everywhere else here.
    fn shape_result(&self, value: &Value) -> Value {
        match self.method.as_str() {
            "tools/call" => json!({
                "content": [{
                    "type": "text",
                    "text": serde_json::to_string(value).unwrap_or_else(|_| "{}".to_string())
                }],
                "isError": false
            }),
            // A prompt's result is its own object, and anything else is a
            // method that does not hand off today — passed through rather than
            // guessed at, since inventing an envelope for a shape we do not
            // know is worse than handing back what was stored.
            _ => value.clone(),
        }
    }

    /// The `DetailedTask` a `tasks/get` answers with.
    ///
    /// `resultType` is `"complete"` here even when the *task* is still working:
    /// the field describes this response — a `tasks/get` that answered is a
    /// complete answer to "what is the status" — and not the work it reports on.
    /// Conflating the two would tell a polling client its poll had not finished.
    pub fn detailed_result(&self) -> Value {
        let mut map = self.base();
        map.insert("resultType".to_string(), json!("complete"));
        if let Some(result) = &self.result {
            map.insert("result".to_string(), self.shape_result(result));
        }
        if let Some(error) = &self.error {
            map.insert("error".to_string(), error.clone());
        }
        Value::Object(map)
    }
}

fn row_to_task(row: &sqlx::postgres::PgRow) -> McpTask {
    McpTask {
        task_id: row.get("task_id"),
        script_uri: row.get("script_uri"),
        method: row.get("method"),
        status: row.get("status"),
        status_message: row.get("status_message"),
        result: row.get("result"),
        error: row.get("error"),
        created_at: row.get("created_at"),
        updated_at: row.get("updated_at"),
        expires_at: row.get("expires_at"),
    }
}

/// Record a task handle for work that has just been queued.
///
/// Written before the response goes out, which the specification requires: a
/// client holding a `taskId` that names nothing is worse than a slow answer,
/// and the whole value of the handle is that it survives a disconnect.
pub async fn create(
    task_id: Uuid,
    script_task_id: Uuid,
    script_uri: &str,
    method: &str,
    target: &str,
    status_message: Option<&str>,
) -> Result<McpTask, sqlx::Error> {
    let db = crate::database::get_global_database()
        .ok_or_else(|| sqlx::Error::Configuration("no database".into()))?;

    let expires_at = Utc::now() + TASK_TTL;
    let row = sqlx::query(
        r#"
        INSERT INTO mcp_tasks
            (task_id, script_task_id, script_uri, method, target, status, status_message, expires_at)
        VALUES ($1, $2, $3, $4, $5, 'working', $6, $7)
        RETURNING task_id, script_uri, method, status, status_message, result, error,
                  created_at, updated_at, expires_at
        "#,
    )
    .bind(task_id)
    .bind(script_task_id)
    .bind(script_uri)
    .bind(method)
    .bind(target)
    .bind(status_message)
    .bind(expires_at)
    .fetch_one(db.pool())
    .await?;

    debug!(task = %task_id, script = %script_uri, "MCP task created");
    Ok(row_to_task(&row))
}

/// The task a client is polling for, if it is still answerable.
///
/// An expired task answers `None` rather than its last state: the handle has a
/// stated lifetime and honouring it is the difference between a TTL and a
/// suggestion. The row may still be there — pruning is a background pass — so
/// the check is on the clock rather than on the row's existence.
pub async fn get(task_id: Uuid) -> Result<Option<McpTask>, sqlx::Error> {
    let db = crate::database::get_global_database()
        .ok_or_else(|| sqlx::Error::Configuration("no database".into()))?;

    let row = sqlx::query(
        r#"
        SELECT task_id, script_uri, method, status, status_message, result, error,
               created_at, updated_at, expires_at
        FROM mcp_tasks
        WHERE task_id = $1 AND expires_at > NOW()
        "#,
    )
    .bind(task_id)
    .fetch_optional(db.pool())
    .await?;

    Ok(row.as_ref().map(row_to_task))
}

/// Move a task to a terminal state, carrying whatever it produced.
///
/// Guarded on the status rather than applied unconditionally: a task that has
/// already finished must not be moved again, and the case that makes this
/// matter is a cancellation racing a completion. Whichever lands first is the
/// answer, and the loser changes nothing — the alternative is a client told a
/// task was cancelled after it had already been given the result.
pub async fn finish(
    script_task_id: Uuid,
    status: &str,
    result: Option<Value>,
    error: Option<Value>,
) -> Result<bool, sqlx::Error> {
    let Some(db) = crate::database::get_global_database() else {
        return Ok(false);
    };

    let updated = sqlx::query(
        r#"
        UPDATE mcp_tasks
        SET status = $1,
            result = $2,
            error = $3,
            updated_at = NOW()
        WHERE script_task_id = $4
          AND status NOT IN ('completed', 'failed', 'cancelled')
        "#,
    )
    .bind(status)
    .bind(result)
    .bind(error)
    .bind(script_task_id)
    .execute(db.pool())
    .await?
    .rows_affected();

    if updated > 0 {
        debug!(task = %script_task_id, status, "MCP task finished");
    }
    Ok(updated > 0)
}

/// What a script task's cancellation means for the handle a client holds.
pub async fn mark_cancelled(script_task_id: Uuid) -> Result<bool, sqlx::Error> {
    finish(script_task_id, "cancelled", None, None).await
}

/// Ask for a task to stop.
///
/// Cooperative, which the extension says plainly and the engine cannot improve
/// on: a handler that has already been claimed by a worker runs to its own
/// conclusion. What this does is cancel the queue row if it has not started, so
/// the common case — a client changing its mind while the task is still pending
/// — actually stops. The acknowledgement is the same either way, because a
/// client that was told "cancelled" and then got a result would have been lied
/// to by a more confident answer.
pub async fn cancel(task_id: Uuid) -> Result<bool, sqlx::Error> {
    let Some(db) = crate::database::get_global_database() else {
        return Ok(false);
    };

    let row = sqlx::query("SELECT script_task_id FROM mcp_tasks WHERE task_id = $1")
        .bind(task_id)
        .fetch_optional(db.pool())
        .await?;
    let Some(row) = row else {
        return Ok(false);
    };
    let script_task_id: Option<Uuid> = row.get("script_task_id");

    if let Some(script_task_id) = script_task_id {
        // Best effort: `false` here means the worker already has it, which is
        // the case the extension calls not obligated to stop.
        match crate::tasks::cancel(script_task_id).await {
            Ok(true) => {
                let _ = mark_cancelled(script_task_id).await;
            }
            Ok(false) => {
                debug!(task = %task_id, "MCP task already claimed; cancellation is advisory");
            }
            Err(e) => warn!(task = %task_id, error = %e, "Failed cancelling a task's queue row"),
        }
    }

    Ok(true)
}

/// Drop handles nobody came back for.
///
/// Runs on the same tick as the other pruners. A task is deleted once it is
/// past its expiry, whatever its status — an unfinished task past its TTL is a
/// handle the client has stopped being able to use, so keeping the row buys
/// nothing that the script's own log does not already hold.
pub async fn prune() -> Result<u64, sqlx::Error> {
    let Some(db) = crate::database::get_global_database() else {
        return Ok(0);
    };

    let deleted = sqlx::query("DELETE FROM mcp_tasks WHERE expires_at <= NOW()")
        .execute(db.pool())
        .await?
        .rows_affected();

    if deleted > 0 {
        debug!(deleted, "Pruned expired MCP task handles");
    }
    Ok(deleted)
}

/// Deleting a script takes its outstanding handles with it.
pub async fn delete_for_script(script_uri: &str) -> Result<u64, sqlx::Error> {
    let Some(db) = crate::database::get_global_database() else {
        return Ok(0);
    };

    Ok(sqlx::query("DELETE FROM mcp_tasks WHERE script_uri = $1")
        .bind(script_uri)
        .execute(db.pool())
        .await?
        .rows_affected())
}

/// Whether a client said it can be handed a task handle.
///
/// The specification is explicit that a server must never return one to a
/// client that did not declare the extension, and the reason is not politeness:
/// a client that does not know `resultType: "task"` reads a handle as the
/// answer, and a tool call comes back as a small object full of fields nobody
/// asked for. Declared per request, since `2026-07-28` has no handshake.
/// Takes the declared capabilities rather than the whole request, matching
/// [`crate::mcp_elicitation::client_can_elicit`]: the handler reads them out of
/// `_meta` once, and two places digging into the same field would eventually
/// disagree about where it lives.
pub fn client_accepts_tasks(client_capabilities: Option<&Value>) -> bool {
    client_capabilities
        .and_then(|capabilities| capabilities.get("extensions"))
        .and_then(|extensions| extensions.get(EXTENSION))
        .is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn task(status: &str) -> McpTask {
        McpTask {
            task_id: Uuid::nil(),
            script_uri: "test://script".to_string(),
            method: "tools/call".to_string(),
            status: status.to_string(),
            status_message: None,
            result: None,
            error: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
            expires_at: Utc::now() + TASK_TTL,
        }
    }

    /// A handle says it is one, and an answer says it is complete.
    #[test]
    fn a_created_task_is_not_a_complete_result() {
        let created = task("working").create_result();
        assert_eq!(created["resultType"], "task");
        assert_eq!(created["status"], "working");
        assert!(
            created["pollIntervalMs"].is_number(),
            "a working task should say how often to poll: {}",
            created
        );
        assert!(created["ttlMs"].as_i64().unwrap_or(0) > 0);
    }

    /// `resultType` describes the response, not the work it reports on.
    ///
    /// A `tasks/get` that answered is a complete answer to "what is the
    /// status", even when the status is `working`. Reporting `"task"` here
    /// would tell a polling client that its poll had not finished.
    #[test]
    fn polling_a_working_task_is_still_a_complete_answer() {
        let polled = task("working").detailed_result();
        assert_eq!(polled["resultType"], "complete");
        assert_eq!(polled["status"], "working");
    }

    /// A terminal task stops being given polling advice.
    #[test]
    fn a_finished_task_is_not_told_to_poll() {
        for status in ["completed", "failed", "cancelled"] {
            let finished = task(status);
            assert!(finished.is_terminal(), "{} is terminal", status);
            let answer = finished.detailed_result();
            assert!(
                answer.get("pollIntervalMs").is_none(),
                "{} should not carry polling advice: {}",
                status,
                answer
            );
        }
        assert!(!task("working").is_terminal());
        assert!(!task("input_required").is_terminal());
    }

    /// The extension has to be declared, and declared in the right place.
    #[test]
    fn a_client_must_say_it_understands_task_handles() {
        let declared = json!({ "extensions": { EXTENSION: {} } });
        assert!(client_accepts_tasks(Some(&declared)));

        // Every near miss is a no. A client that declared other extensions, or
        // capabilities with no extensions, or nothing at all, has not said it
        // can read a handle — and handing one to it turns a tool call into an
        // object full of fields it never asked for.
        for missed in [
            json!({}),
            json!({ "extensions": {} }),
            json!({ "extensions": { "io.example/other": {} } }),
            json!({ "elicitation": { "form": {} } }),
            // The whole request rather than the capabilities out of it, which
            // is the mistake this signature exists to make impossible.
            json!({ "_meta": { "io.modelcontextprotocol/clientCapabilities": {
                "extensions": { EXTENSION: {} }
            } } }),
        ] {
            assert!(
                !client_accepts_tasks(Some(&missed)),
                "should not count as declared: {}",
                missed
            );
        }
        assert!(!client_accepts_tasks(None));
    }
}
