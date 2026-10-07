//! Where everything a script is and does is stored: its files, ownership and
//! host bindings, its log, storage, secrets and tables.
//!
//! One file per topic, each holding the SQL for its topic and the blocking
//! wrappers the rest of the engine calls. `postgres.rs` holds the `Repository`
//! trait the async callers use; every path stays `crate::repository::*`.
use crate::error::AppResult;
mod assets;
mod cache;
mod logs;
mod postgres;
mod registrations;
mod scripts;
mod secrets;
mod storage;
mod tables;
pub use assets::*;
pub use cache::*;
pub use logs::*;
pub use postgres::*;
pub use registrations::*;
pub use scripts::*;
pub use secrets::*;
pub use storage::*;
pub use tables::*;

/// Helper to run async code in a blocking context, handling different runtime scenarios
fn run_blocking<F, R>(future: F) -> R
where
    F: std::future::Future<Output = R>,
{
    crate::database::run_blocking(future)
}

/// The same, bounded by whatever remains of the caller's execution budget.
///
/// What every wrapper a script can reach goes through. A blocked database call
/// is invisible to the interrupt handler that enforces the budget between
/// bytecode operations, so without this the budget simply does not apply to the
/// one kind of work most likely to exceed it.
fn run_bounded<F, T>(future: F) -> AppResult<T>
where
    F: std::future::Future<Output = AppResult<T>>,
{
    crate::database::run_bounded(future)
}

/// Defines the types of repository errors that can occur
#[derive(Debug, thiserror::Error)]
pub enum RepositoryError {
    #[error("Mutex lock failed: {0}")]
    LockError(String),
    #[error("Script not found: {0}")]
    ScriptNotFound(String),
    #[error("Asset not found: {0}")]
    AssetNotFound(String),
    #[error("Invalid data format: {0}")]
    InvalidData(String),
}

/// Largest root source a script may have.
///
/// Named rather than repeated so the callers that build content before storing
/// it — a patch, which assembles the new source from edits — can refuse it
/// themselves, instead of handing it here and reporting the refusal as a
/// storage failure.
pub const MAX_SCRIPT_CONTENT_BYTES: usize = 1_000_000;

/// Largest content an asset may hold.
///
/// The storage-side cap, which is the one a caller actually meets: the write
/// endpoints bound a request body a little higher (`engine_api::MAX_ASSET_BYTES`),
/// so a body between the two is accepted by the router and refused here.
pub const MAX_ASSET_CONTENT_BYTES: usize = 10_000_000;

/// Largest value either Web Storage store — `scriptStorage`, `personalStorage`
/// — or a secret may hold. Named so the four write paths that enforce it and
/// the documents that publish it cannot drift apart.
pub const MAX_STORAGE_VALUE_BYTES: usize = 1_000_000;

/// Longest an asset's URI — its path within the owning script — may be.
pub const MAX_ASSET_URI_CHARS: usize = 255;

/// Rows a `database.query` returns when the caller names no limit, and the
/// ceiling a named one is clamped to. A caller asking for more is not refused,
/// it is answered with [`MAX_QUERY_LIMIT`] rows.
pub const DEFAULT_QUERY_LIMIT: i64 = 100;
/// See [`DEFAULT_QUERY_LIMIT`].
pub const MAX_QUERY_LIMIT: i64 = 1000;

#[cfg(test)]
pub static GLOBAL_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[cfg(test)]
mod tests;
