//! How long a script's log lines are kept, and the pass that enforces it.
//!
//! Scripts write to the `logs` table in whatever volume they like, and until
//! this module existed nothing removed a line unless a person asked: the
//! configured retention was parsed and read by nothing, and the only prune was
//! a manual engine action that kept a hardcoded twenty lines per script. A
//! deployment nobody was babysitting grew the table forever.
//!
//! The policy lives here; the statement that carries it out lives with the
//! rest of the log SQL in [`crate::repository`].

use crate::config::LogsConfig;
use crate::error::AppResult;
use std::sync::OnceLock;

/// How much of each script's log survives a pass.
///
/// The two fields bound different things and neither bounds the table on its
/// own — see [`LogsConfig`] for why both are enforced.
#[derive(Debug, Clone, Copy)]
pub struct LogRetention {
    /// Keep this many of the newest lines per script. Clamped to at least one,
    /// so a misconfigured zero cannot silently empty every log.
    pub keep_per_script: i64,

    /// Delete lines older than this many hours. Zero disables the age bound,
    /// matching the convention the `[repository]` timeouts already use.
    pub keep_hours: i32,
}

impl Default for LogRetention {
    fn default() -> Self {
        Self::from_config(&LogsConfig::default())
    }
}

impl LogRetention {
    pub fn from_config(config: &LogsConfig) -> Self {
        Self {
            keep_per_script: i64::from(config.keep_per_script).max(1),
            keep_hours: config.retention_hours.min(i32::MAX as u64) as i32,
        }
    }
}

/// The retention this engine was configured with, set once at startup.
static CONFIGURED: OnceLock<LogRetention> = OnceLock::new();

/// Records the configured retention so callers outside the pruner — the engine
/// API reports it — agree with what the background pass enforces. Returns
/// false if it was already set.
pub fn configure(retention: LogRetention) -> bool {
    CONFIGURED.set(retention).is_ok()
}

/// The retention in effect: what was configured at startup, or the defaults.
pub fn configured() -> LogRetention {
    CONFIGURED.get().copied().unwrap_or_default()
}

/// Deletes whatever the retention no longer covers, returning the row count.
pub async fn prune(retention: LogRetention) -> AppResult<u64> {
    crate::repository::prune_log_messages_async(retention).await
}

/// Starts the background pruner.
///
/// Every instance runs this; the advisory lock the statement takes means only
/// one of them does the work on any given tick.
pub fn spawn_pruner(config: LogsConfig, shutdown: tokio::sync::oneshot::Receiver<()>) {
    if !config.prune_enabled {
        tracing::info!("Log pruning is disabled; the logs table will grow without bound");
        return;
    }

    let retention = LogRetention::from_config(&config);
    let period = std::time::Duration::from_secs(config.prune_interval_secs.max(60));

    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(period);
        // The first tick fires immediately, which would put a prune in the
        // middle of startup — when the engine has least to spare and the log
        // has least to gain.
        ticker.tick().await;

        let mut shutdown = shutdown;
        loop {
            tokio::select! {
                _ = ticker.tick() => {
                    match prune(retention).await {
                        Ok(deleted) if deleted > 0 => {
                            tracing::info!(deleted = deleted, "Pruned script logs");
                        }
                        Ok(_) => {}
                        Err(e) => tracing::warn!("Log pruning pass failed: {}", e),
                    }
                }
                _ = &mut shutdown => {
                    tracing::debug!("Log pruner stopping");
                    return;
                }
            }
        }
    });

    tracing::info!(
        retention_hours = retention.keep_hours,
        keep_per_script = retention.keep_per_script,
        interval_secs = period.as_secs(),
        "Log pruner started"
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zero_keep_per_script_is_clamped_to_one() {
        let retention = LogRetention::from_config(&LogsConfig {
            keep_per_script: 0,
            ..LogsConfig::default()
        });
        assert_eq!(retention.keep_per_script, 1);
    }

    #[test]
    fn zero_retention_hours_disables_the_age_bound() {
        let retention = LogRetention::from_config(&LogsConfig {
            retention_hours: 0,
            ..LogsConfig::default()
        });
        assert_eq!(retention.keep_hours, 0);
    }

    #[test]
    fn defaults_keep_a_hundred_lines_for_a_day() {
        let retention = LogRetention::default();
        assert_eq!(retention.keep_per_script, 100);
        assert_eq!(retention.keep_hours, 24);
    }
}
