//! A count of the blocking workers the engine gave up on.
//!
//! Every entry point that runs a script puts it on a blocking thread under an
//! outer timeout. When that timeout fires the engine answers the caller and
//! walks away, but the thread keeps running: nothing in the process can stop
//! work that has left JavaScript, and dropping the join handle only detaches
//! the task. The thread stays, and so does whatever it holds — a pooled
//! connection, an open transaction, the locks that transaction took.
//!
//! That is a leak with no symptom until the pool is empty and every script
//! stops at once. The engine knew when it abandoned a worker and never looked
//! again, so there was no way to tell "one slow request last Tuesday" from
//! "thirty threads gone and the next request is the last one that works".
//!
//! This module closes the loop. A worker carries a [`WorkerTicket`]; the task
//! awaiting it holds the matching [`WorkerWatch`]. Giving up marks the ticket,
//! and the ticket reports back if the worker ever finishes — which, since
//! host calls became bounded, it usually does. What is worth alerting on is
//! the difference: workers abandoned and never seen again.

use std::sync::Arc;
use std::sync::atomic::{AtomicU8, AtomicU64, Ordering};
use std::time::Instant;

use tracing::{error, info};

/// Workers the engine has given up waiting for, over the process's life.
static ABANDONED: AtomicU64 = AtomicU64::new(0);

/// Abandoned workers that finished anyway and released what they held.
static RECOVERED: AtomicU64 = AtomicU64::new(0);

/// The worker is still running and nobody has given up on it.
const RUNNING: u8 = 0;
/// The task awaiting it gave up; the thread is still out there.
const ABANDONED_STATE: u8 = 1;
/// The worker finished, whether or not anyone was still waiting.
const FINISHED: u8 = 2;

struct Tracked {
    state: AtomicU8,
    what: String,
    started: Instant,
}

/// Travels with the blocking worker and reports when it finishes.
///
/// Must be moved into the closure rather than held by the awaiting task: what
/// it measures is the thread's own lifetime, which outer timeouts cannot see.
pub struct WorkerTicket(Arc<Tracked>);

impl Drop for WorkerTicket {
    fn drop(&mut self) {
        if self.0.state.swap(FINISHED, Ordering::SeqCst) == ABANDONED_STATE {
            RECOVERED.fetch_add(1, Ordering::Relaxed);
            info!(
                worker = %self.0.what,
                elapsed_ms = self.0.started.elapsed().as_millis() as u64,
                "an abandoned worker finished and released what it held"
            );
        }
    }
}

/// Stays with the task that is waiting, and records giving up.
pub struct WorkerWatch(Arc<Tracked>);

impl WorkerWatch {
    /// Records that the engine stopped waiting for this worker.
    ///
    /// A worker that finished first is not counted: the backstop and the
    /// worker can land together, and calling a thread lost when it is already
    /// back would leave a permanent phantom in the census.
    pub fn abandon(self) {
        let swapped = self.0.state.compare_exchange(
            RUNNING,
            ABANDONED_STATE,
            Ordering::SeqCst,
            Ordering::SeqCst,
        );

        if swapped.is_ok() {
            ABANDONED.fetch_add(1, Ordering::Relaxed);
            let census = snapshot();
            error!(
                worker = %self.0.what,
                elapsed_ms = self.0.started.elapsed().as_millis() as u64,
                in_flight = census.in_flight,
                "gave up on a blocking worker; its thread and anything it holds \
                 stay out until it returns on its own"
            );
        }
    }
}

/// Issues a ticket for a worker about to be run on a blocking thread.
///
/// `what` names it for the log — the handler, the script, the tool.
pub fn watch(what: impl Into<String>) -> (WorkerTicket, WorkerWatch) {
    let tracked = Arc::new(Tracked {
        state: AtomicU8::new(RUNNING),
        what: what.into(),
        started: Instant::now(),
    });
    (WorkerTicket(Arc::clone(&tracked)), WorkerWatch(tracked))
}

/// What the census knows right now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Census {
    /// Workers given up on since the process started.
    pub abandoned: u64,
    /// How many of those came back.
    pub recovered: u64,
    /// Threads still out, each holding whatever it held when it was abandoned.
    /// The number worth alerting on.
    pub in_flight: u64,
}

/// Reads the census.
pub fn snapshot() -> Census {
    // Abandoned is read first so a worker recovering between the two reads can
    // only make `in_flight` too low, never negative.
    let abandoned = ABANDONED.load(Ordering::Relaxed);
    let recovered = RECOVERED.load(Ordering::Relaxed);
    Census {
        abandoned,
        recovered,
        in_flight: abandoned.saturating_sub(recovered),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_worker_that_finishes_first_is_never_counted_as_lost() {
        let before = snapshot();

        let (ticket, watch) = watch("a-fast-worker");
        drop(ticket);
        // The backstop fires anyway, as it can when the two land together.
        watch.abandon();

        let after = snapshot();
        assert_eq!(
            after.abandoned, before.abandoned,
            "a worker already back must not be counted as abandoned"
        );
        assert_eq!(after.in_flight, before.in_flight);
    }

    #[test]
    fn an_abandoned_worker_that_returns_stops_counting_against_the_engine() {
        let before = snapshot();

        let (ticket, watch) = watch("a-slow-worker");
        watch.abandon();

        let while_out = snapshot();
        assert_eq!(while_out.abandoned, before.abandoned + 1);
        assert_eq!(while_out.in_flight, before.in_flight + 1);

        drop(ticket);

        let after = snapshot();
        assert_eq!(after.recovered, before.recovered + 1);
        assert_eq!(
            after.in_flight, before.in_flight,
            "a thread that came back is not still out"
        );
    }

    #[test]
    fn a_worker_nobody_gave_up_on_leaves_no_trace() {
        let before = snapshot();
        let (ticket, _watch) = watch("an-ordinary-worker");
        drop(ticket);
        assert_eq!(snapshot(), before);
    }
}
