//! Git sync and the credentials it uses.

use super::*;
use crate::security::{Capability, UserContext};
use serde::Deserialize;
use serde_json::{Value, json};

// ============================================================================
// Git sync
// ============================================================================

/// What to pull, and from where.
#[derive(Deserialize, Default, utoipa::ToSchema)]
#[schema(example = json!({ "repo": "octocat/hello-world" }))]
pub struct GitPullBody {
    /// The repository, as `owner/repo` or as any GitHub URL naming it —
    /// `octocat/hello-world`, `https://github.com/octocat/hello-world`, or the
    /// `.git` clone URL. Required.
    #[schema(example = "octocat/hello-world")]
    pub repo: Option<String>,

    /// Branch to read. Omit to use whichever branch the repository calls its
    /// default.
    #[schema(example = "main")]
    pub branch: Option<String>,

    /// URI prefix the pulled scripts land under, appended to this engine's own
    /// origin. Defaults to the repository name. Set it when two repositories
    /// would otherwise collide, or to place a solution somewhere specific.
    #[schema(example = "examples")]
    pub prefix: Option<String>,

    /// Download and re-apply even when the repository has not moved since the
    /// last pull. Off by default, because an unchanged repository is normally
    /// nothing to do.
    #[serde(default)]
    pub force: bool,
}

pub(super) fn pull_report_json(report: &crate::git_sync::PullReport) -> Value {
    json!({
        "success": true,
        "repo": report.repo,
        "branch": report.branch,
        "commit": report.commit,
        "upToDate": report.up_to_date,
        "scripts": report.scripts.iter().map(|script| json!({
            "script": script.script_uri,
            "source": script.source,
            "action": script.action,
            "changed": script.changed,
            "written": script.written,
            "deleted": script.deleted,
            "unchanged": script.unchanged,
            "revision": script.revision,
            "init": script.init,
        })).collect::<Vec<Value>>(),
        "timestamp": iso_timestamp(),
    })
}

/// A personal access token to store for a git host.
#[derive(Deserialize, Default, utoipa::ToSchema)]
#[schema(example = json!({ "token": "github_pat_11ABCDEFG..." }))]
pub struct GitCredentialBody {
    /// The token. A fine-grained token scoped to the repositories you want is
    /// enough; it needs no more than read access to their contents. Required.
    ///
    /// It is encrypted at rest and is never returned by this or any other
    /// endpoint.
    pub token: Option<String>,

    /// Git host the token authenticates against. Defaults to `github.com`,
    /// which is the only host supported.
    #[schema(example = "github.com")]
    pub host: Option<String>,
}

pub(super) fn credential_json(summary: &crate::git_credentials::CredentialSummary) -> Value {
    json!({
        "host": summary.remote_host,
        "account": summary.account,
        "createdAt": summary.created_at.to_rfc3339(),
        "updatedAt": summary.updated_at.to_rfc3339(),
        "lastUsedAt": summary.last_used_at.map(|t| t.to_rfc3339()),
    })
}

#[derive(Deserialize, Default, utoipa::IntoParams)]
pub struct GitHostQuery {
    /// Git host to act on. Defaults to `github.com`.
    pub host: Option<String>,
}

pub(super) fn tool_set_git_credential(args: &Value, user: &UserContext) -> Value {
    let Some(token) = arg_str(args, "token") else {
        return missing_arg("token");
    };
    if !user.has_capability(&Capability::WriteScripts) {
        return refuse(Refusal::Forbidden, "Access denied");
    }
    let Some(user_id) = user.user_id.clone() else {
        return refuse(
            Refusal::BadRequest,
            "A git credential belongs to an account, and this request has none",
        );
    };
    let host = arg_str(args, "host").unwrap_or(crate::git_github::HOST);
    if host != crate::git_github::HOST {
        return refuse(
            Refusal::BadRequest,
            format!("Only {} is supported", crate::git_github::HOST),
        );
    }
    if !crate::config::git_config().allows(host) {
        return refuse(
            Refusal::BadRequest,
            format!("This engine is not configured to read from {}", host),
        );
    }

    let account = match crate::git_github::GitHubClient::new()
        .map(|client| client.with_token(Some(token.to_string())))
        .and_then(|client| client.verify_token())
    {
        Ok(account) => account,
        Err(e) => return refuse(Refusal::Failed, e.to_string()),
    };

    match crate::database::run_blocking(crate::git_credentials::store(
        &user_id,
        host,
        token,
        Some(&account),
    )) {
        Ok(()) => json!({
            "success": true,
            "host": host,
            "account": account,
            "timestamp": iso_timestamp(),
        }),
        Err(e) => refuse(Refusal::Failed, e.to_string()),
    }
}

pub(super) fn tool_list_git_credentials(_args: &Value, user: &UserContext) -> Value {
    if !user.has_capability(&Capability::WriteScripts) {
        return refuse(Refusal::Forbidden, "Access denied");
    }
    let Some(user_id) = user.user_id.clone() else {
        return json!({ "credentials": [] });
    };
    match crate::database::run_blocking(crate::git_credentials::list(&user_id)) {
        Ok(credentials) => json!({
            "credentials": credentials.iter().map(credential_json).collect::<Vec<Value>>(),
            "timestamp": iso_timestamp(),
        }),
        Err(e) => refuse(Refusal::Failed, e.to_string()),
    }
}

pub(super) fn tool_delete_git_credential(args: &Value, user: &UserContext) -> Value {
    if !user.has_capability(&Capability::WriteScripts) {
        return refuse(Refusal::Forbidden, "Access denied");
    }
    let Some(user_id) = user.user_id.clone() else {
        return refuse(
            Refusal::BadRequest,
            "A git credential belongs to an account, and this request has none",
        );
    };
    let host = arg_str(args, "host").unwrap_or(crate::git_github::HOST);
    match crate::database::run_blocking(crate::git_credentials::forget(&user_id, host)) {
        Ok(removed) => json!({
            "success": true,
            "host": host,
            "removed": removed,
            "timestamp": iso_timestamp(),
        }),
        Err(e) => refuse(Refusal::Failed, e.to_string()),
    }
}

/// What to publish, and where.
#[derive(Deserialize, Default, utoipa::ToSchema)]
#[schema(example = json!({ "script": "https://example.com/shop.js" }))]
pub struct GitPushBody {
    /// URI of the script to publish. Required.
    #[schema(example = "https://example.com/shop.js")]
    pub script: Option<String>,

    /// Repository to publish to, as `owner/repo` or any GitHub URL naming it.
    /// Optional once the script has been pulled, since it already knows where
    /// it came from.
    #[schema(example = "octocat/hello-world")]
    pub repo: Option<String>,

    /// Branch to write. Defaults to the branch the script was pulled from, or
    /// the repository's default branch on a first publish.
    #[schema(example = "main")]
    pub branch: Option<String>,

    /// Commit message. Defaulted to one naming the script.
    #[schema(example = "Fix the cart total")]
    pub message: Option<String>,

    /// Publish even when this engine believes the repository has moved. GitHub
    /// still refuses a non-fast-forward update, so this cannot overwrite work
    /// it has not seen.
    #[serde(default)]
    pub force: bool,
}

pub(super) fn push_report_json(report: &crate::git_sync::PushReport) -> Value {
    json!({
        "success": true,
        "script": report.script_uri,
        "repo": report.repo,
        "branch": report.branch,
        "commit": report.commit,
        "upToDate": report.up_to_date,
        "written": report.written,
        "removed": report.removed,
        "timestamp": iso_timestamp(),
    })
}

pub(super) fn tool_push_to_git(args: &Value, user: &UserContext) -> Value {
    let Some(script) = arg_str(args, "script") else {
        return missing_arg("script");
    };

    let request = crate::git_sync::PushRequest {
        script_uri: script.to_string(),
        repo: arg_str(args, "repo").map(str::to_string),
        branch: arg_str(args, "branch").map(str::to_string),
        message: arg_str(args, "message").map(str::to_string),
        force: args.get("force").and_then(Value::as_bool).unwrap_or(false),
    };

    let user = user.clone();
    match crate::database::run_blocking(crate::git_sync::push(&user, request)) {
        Ok(report) => push_report_json(&report),
        Err(e) => refuse_sync(&e),
    }
}

#[derive(Deserialize, Default, utoipa::IntoParams)]
pub struct GitStatusQuery {
    /// URI of the script to report on.
    pub script: Option<String>,
}

pub(super) fn status_json(status: &crate::git_sync::SyncStatus) -> Value {
    json!({
        "script": status.script_uri,
        "state": status.state.as_str(),
        "action": status.state.advice(),
        "repo": status.remote,
        "branch": status.branch,
        "commit": status.remote_commit,
        "commitAtSync": status.commit_at_sync,
        "revision": status.revision,
        "revisionAtSync": status.revision_at_sync,
        "pinnedRevision": status.pinned,
        "unreachable": status.unreachable,
        "timestamp": iso_timestamp(),
    })
}

pub(super) fn tool_get_git_status(args: &Value, user: &UserContext) -> Value {
    let Some(script) = arg_str(args, "script") else {
        return missing_arg("script");
    };
    if !can_read_history(user, script) {
        return refuse(Refusal::Forbidden, "Access denied");
    }

    let user = user.clone();
    let script = script.to_string();
    match crate::database::run_blocking(
        async move { crate::git_sync::status(&user, &script).await },
    ) {
        Ok(status) => status_json(&status),
        Err(e) => refuse_sync(&e),
    }
}

pub(super) fn binding_json(binding: &crate::git_sync::Binding) -> Value {
    json!({
        "script": binding.script_uri,
        "repo": binding.remote,
        "branch": binding.branch,
        "commitAtSync": binding.last_commit,
        "syncedAt": binding.synced_at.to_rfc3339(),
        "syncedBy": binding.synced_by,
    })
}

pub(super) fn tool_list_git_bindings(_args: &Value, user: &UserContext) -> Value {
    match crate::database::run_blocking(crate::git_sync::bindings()) {
        Ok(bindings) => json!({
            "bindings": bindings
                .iter()
                .filter(|binding| can_read_history(user, &binding.script_uri))
                .map(binding_json)
                .collect::<Vec<Value>>(),
            "timestamp": iso_timestamp(),
        }),
        Err(e) => refuse(Refusal::Failed, e.to_string()),
    }
}

pub(super) fn tool_clear_git_remote(args: &Value, user: &UserContext) -> Value {
    let Some(script) = arg_str(args, "script") else {
        return missing_arg("script");
    };
    if !can_write_script(user, script) {
        return refuse(Refusal::Forbidden, "Access denied");
    }
    match crate::database::run_blocking(crate::git_sync::unbind(script)) {
        Ok(removed) => json!({
            "success": true,
            "script": script,
            "removed": removed,
            "timestamp": iso_timestamp(),
        }),
        Err(e) => refuse(Refusal::Failed, e.to_string()),
    }
}

pub(super) fn tool_pull_from_git(args: &Value, user: &UserContext) -> Value {
    let Some(repo) = arg_str(args, "repo") else {
        return missing_arg("repo");
    };

    let request = crate::git_sync::PullRequest {
        repo: repo.to_string(),
        branch: arg_str(args, "branch").map(str::to_string),
        prefix: arg_str(args, "prefix").map(str::to_string),
        force: args.get("force").and_then(Value::as_bool).unwrap_or(false),
    };

    let user = user.clone();
    match crate::database::run_blocking(crate::git_sync::pull(&user, request)) {
        Ok(report) => pull_report_json(&report),
        Err(e) => refuse_sync(&e),
    }
}
