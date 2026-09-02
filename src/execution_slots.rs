//! How many scripts may be running at once.
//!
//! Every entry point that runs a script puts it on a blocking thread, and
//! nothing counted them. The ceiling was therefore Tokio's default blocking
//! pool — 512 threads, a number nobody chose — while
//! `javascript.max_concurrent_executions` was parsed and read by nothing. Each
//! of those threads may hold a QuickJS runtime allowed `max_memory_bytes`, so
//! on a production configuration the unenforced worst case is 512 × 128 MB.
//! Worse, an execution that outruns its timeout is abandoned but keeps
//! running (see [`crate::worker_census`]), so the threads that pile up under
//! load are exactly the ones nothing was accounting for.
//!
//! A caller waits for a slot rather than being refused one. The wait happens
//! inside the timeout the call already had, so a request that cannot get a
//! slot in time fails the way a slow request fails, rather than turning a busy
//! engine into an error the caller has to distinguish from a broken one.
//!
//! The permit is moved into the blocking closure rather than held by the task
//! that spawned it. Dropping it at the timeout would release a slot to a new
//! execution while the abandoned one still holds its thread and its memory,
//! which is the case the limit exists for.

use std::sync::Arc;
use std::sync::OnceLock;

use tokio::sync::{OwnedSemaphorePermit, Semaphore};

static SLOTS: OnceLock<Arc<Semaphore>> = OnceLock::new();

/// Sets the ceiling from `javascript.max_concurrent_executions`. Returns false
/// if it was already set.
pub fn configure(max_concurrent: usize) -> bool {
    SLOTS
        .set(Arc::new(Semaphore::new(max_concurrent.max(1))))
        .is_ok()
}

/// Waits for a slot, and returns the permit that holds it.
///
/// `None` when no ceiling has been configured — the tests and the paths that
/// run before startup finishes are not the fan-out this bounds, and gating
/// them on a limit nobody set would be a worse default than not counting.
///
/// The result must be moved into the blocking work it accounts for: the slot
/// is held until the permit drops, which is the point.
pub async fn acquire() -> Option<OwnedSemaphorePermit> {
    let slots = Arc::clone(SLOTS.get()?);
    // A closed semaphore is the only error, and nothing closes this one.
    slots.acquire_owned().await.ok()
}

/// Slots not currently held, for diagnostics.
pub fn available() -> Option<usize> {
    SLOTS.get().map(|slots| slots.available_permits())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn an_unconfigured_engine_does_not_gate_anything() {
        // `configure` is process-global and this test must not claim it for
        // the others, so it asserts the shape of the unset case only.
        if SLOTS.get().is_none() {
            assert!(acquire().await.is_none());
            assert!(available().is_none());
        }
    }

    #[tokio::test]
    async fn a_permit_holds_its_slot_until_it_is_dropped() {
        let slots = Arc::new(Semaphore::new(1));

        let held = Arc::clone(&slots)
            .acquire_owned()
            .await
            .expect("the first slot is free");
        assert_eq!(slots.available_permits(), 0);

        // A second caller waits rather than being refused.
        let waiting = tokio::spawn({
            let slots = Arc::clone(&slots);
            async move { slots.acquire_owned().await.is_ok() }
        });
        assert!(!waiting.is_finished());

        drop(held);
        assert!(
            waiting.await.expect("the waiter should not panic"),
            "dropping a permit should hand the slot to whoever is waiting"
        );
    }
}
