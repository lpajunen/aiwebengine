//! Installing repositories when the engine starts (`git.install`).
//!
//! A fresh engine has no scripts, and the agent that would help somebody write
//! them is itself a script. Pulling it takes an account, and the first account
//! is made by signing in to an engine that has nothing on it yet. So the
//! configuration can name repositories to install, and the engine pulls each
//! one at startup that is not installed yet.
//!
//! It installs and never manages. A repository this engine already follows or
//! has synced anything from is left alone, so a restart does not pull again,
//! does not move what an administrator has moved since, and never meets a
//! change made here. Keeping an installed repository up to date is what
//! `pull_from_git` and `follow_git` are for.
//!
//! The pull is the one a person makes, as [`UserContext::engine_installer`]:
//! public repositories only, since it holds no credential, and the scripts it
//! creates have no owner, so administrators act on them and nobody else does
//! until an administrator adds an owner. A pull refuses to replace a script the
//! repository did not write, so an install never takes over a script of the
//! same name.
//!
//! Each install holds a Postgres advisory lock while it runs, so several
//! instances starting at once install a repository once; an instance that
//! finds it held skips it. A failure is logged and the engine carries on: an
//! install that cannot reach GitHub is no reason to refuse to serve.

use tracing::{info, warn};

use crate::config::GitInstall;
use crate::git_sync::PullRequest;
use crate::security::UserContext;

/// What became of one configured install.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Installed {
    /// Pulled now: the scripts it wrote.
    Pulled(Vec<String>),
    /// Already installed, or being installed by another instance.
    Skipped(String),
    /// It could not be installed; the engine runs without it.
    Failed(String),
}

/// Install every configured repository in the background, in order.
pub fn spawn(installs: Vec<GitInstall>) {
    if installs.is_empty() {
        return;
    }
    tokio::spawn(async move {
        for install in &installs {
            install_one(install, None).await;
        }
    });
}

/// Install one repository, unless it is installed already.
///
/// `client` is for the suite; startup builds one with no credential.
pub async fn install_one(
    install: &GitInstall,
    client: Option<crate::git_github::GitHubClient>,
) -> Installed {
    let outcome = attempt(install, client).await;
    match &outcome {
        Installed::Pulled(scripts) => info!("Installed {}: {}", install.repo, scripts.join(", ")),
        Installed::Skipped(why) => {
            info!("Not installing {}: {}", install.repo, why)
        }
        Installed::Failed(why) => warn!(
            "Could not install {}: {}. The engine runs without it; pull it with pull_from_git.",
            install.repo, why
        ),
    }
    outcome
}

async fn attempt(
    install: &GitInstall,
    client: Option<crate::git_github::GitHubClient>,
) -> Installed {
    let remote = match crate::git_github::RepoRef::parse(&install.repo) {
        Ok(repo) => repo.to_string(),
        Err(e) => return Installed::Failed(e.to_string()),
    };
    let pool = match crate::repository::get_db_pool() {
        Some(db) => db.pool().clone(),
        None => return Installed::Failed("no database is configured".to_string()),
    };

    // Held on one connection for the whole install, and released with it: a
    // session lock outlives no instance that dies holding it.
    let mut connection = match pool.acquire().await {
        Ok(connection) => connection,
        Err(e) => return Installed::Failed(format!("no database connection: {}", e)),
    };
    let lock_key = format!("aiwebengine.git_install:{}", remote);
    let locked: Result<bool, sqlx::Error> =
        sqlx::query_scalar("SELECT pg_try_advisory_lock(hashtextextended($1, 0))")
            .bind(&lock_key)
            .fetch_one(&mut *connection)
            .await;
    match locked {
        Ok(true) => {}
        Ok(false) => return Installed::Skipped("another instance is installing it".to_string()),
        Err(e) => return Installed::Failed(format!("could not take the install lock: {}", e)),
    }

    let outcome = pull_unless_installed(install, &remote, client).await;

    if let Err(e) = sqlx::query("SELECT pg_advisory_unlock(hashtextextended($1, 0))")
        .bind(&lock_key)
        .execute(&mut *connection)
        .await
    {
        warn!(repo = %remote, error = %e, "Could not release the install lock");
    }
    outcome
}

async fn pull_unless_installed(
    install: &GitInstall,
    remote: &str,
    client: Option<crate::git_github::GitHubClient>,
) -> Installed {
    // Asked under the lock, so an instance that waited behind another's
    // install sees what it wrote.
    match crate::git_sync::followed(remote).await {
        Ok(Some(_)) => return Installed::Skipped("this engine already follows it".to_string()),
        Ok(None) => {}
        Err(e) => return Installed::Failed(e.to_string()),
    }
    match crate::git_sync::last_synced(remote).await {
        Ok(rows) if !rows.is_empty() => {
            return Installed::Skipped("scripts here were synced from it".to_string());
        }
        Ok(_) => {}
        Err(e) => return Installed::Failed(e.to_string()),
    }

    let installer = UserContext::engine_installer();
    let client = match client {
        Some(client) => client,
        None => match crate::git_sync::client_for(&installer).await {
            Ok(client) => client,
            Err(e) => return Installed::Failed(e.to_string()),
        },
    };
    let request = PullRequest {
        repo: install.repo.clone(),
        git_ref: install.git_ref.clone(),
        prefix: install.prefix.clone(),
        force: false,
        scripts: install.scripts.clone(),
    };
    match crate::git_sync::pull_through(client, &installer, request).await {
        Ok(report) => Installed::Pulled(
            report
                .scripts
                .into_iter()
                .map(|script| script.script_uri)
                .collect(),
        ),
        Err(e) => Installed::Failed(e.to_string()),
    }
}
