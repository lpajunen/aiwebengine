//! Running part of a script with fewer capabilities than the script holds.
//!
//! Everything else in the engine narrows a context on the way *in*: a request
//! arrives as somebody, [`crate::delegation`] caps background work at what was
//! consented to, [`crate::script_limits`] bounds what one script may spend.
//! What none of them can express is a script narrowing *itself* — running the
//! next part of its own work holding less than it was handed. A
//! [`crate::security::UserContext`] only ever flowed downward from the caller,
//! so the smallest thing a script could run something with was everything it
//! had.
//!
//! Two things want exactly that, and neither is reachable without it.
//!
//! **Code as the action.** The interesting shape for an agent hosted here is
//! the model writing JavaScript rather than emitting a tool call: composition
//! comes free and the tool list stops growing. Every other project adopting
//! that pattern has to build a sandbox first, and this engine already is one.
//! What was missing was evaluating model-authored code holding *less* than the
//! script that asked for it — `/engine/eval` could already evaluate a snippet,
//! but as an administrator's endpoint rather than as something a script calls
//! on itself.
//!
//! **A plan mode that is enforced rather than offered.** Every harness that
//! ships one implements it as a filtered tool list plus a prompt asking the
//! model nicely; none can actually enforce read-only, because the filtering
//! lives in the agent loop and the model writes the transcript. Here the
//! checks are underneath the JavaScript, so a planning turn runs in a context
//! holding the reads and none of the writes, and no amount of cleverness in
//! the transcript reaches past it.
//!
//! # Why a sub-execution rather than a mask
//!
//! The alternative was a restriction that could be pushed and popped around a
//! call inside the running context. Every global closure in
//! [`crate::security::secure_globals`] captures a *clone* of the context when
//! the globals are installed, so a mask means making those clones share an
//! interior-mutable cell. That is buildable, and it is wrong here for two
//! reasons.
//!
//! The first is that a mask has to be observed everywhere a context escapes
//! the JavaScript thread — a closure handing it to a blocking database call, a
//! stream customization function, a registration that outlives the call. Each
//! is a place the mask can be missed, or worse, observed after the pop.
//!
//! The second is the one that settles it: **the thing being restricted is not
//! a closure.** It is a string of model-authored source. A restricted region
//! sharing an object graph with its unrestricted parent can be handed a
//! function by that parent and call it, which is a capability leak by
//! reference that no mask catches — the classic way a same-realm sandbox
//! fails. A separate context cannot be handed anything but JSON, so what it
//! may reach is fixed when its globals are installed and there is nothing to
//! re-check afterwards. The reduced context is proved to the things that check
//! it by construction rather than by discipline.
//!
//! The engine already nests executions this way: `dispatcher.sendMessage`
//! builds a whole runtime inside a running host call, and two mechanisms that
//! would otherwise make nesting wrong are already right. A nested runtime's
//! budget is clamped to the parent's remaining time
//! (`create_sandboxed_runtime` → `within_host_budget`), so a chain of
//! sub-executions cannot outlive the request that started it. And
//! `Database::begin_transaction` degrades to a `SAVEPOINT` when one is already
//! open, so a sub-execution that rolls back rolls back its own work and not
//! its caller's.
//!
//! # What it costs
//!
//! One QuickJS runtime and one re-evaluation of the script's program per call.
//! That is the price of the isolation rather than an implementation detail to
//! optimise away: the program has to be evaluated *in* the narrowed context to
//! be narrowed at all. It is the right price for an agent turn and the wrong
//! one for a loop, which is worth saying in the documentation rather than
//! discovering in production.

use crate::security::{Capability, UserContext};

/// How deep sub-executions may nest.
///
/// Each level is a live QuickJS runtime and a blocking thread frame, and
/// nothing else bounds them: the budget is shared, so a chain cannot run
/// *longer* than its caller, but a hundred nested runtimes exhaust the native
/// stack before the budget notices. Four is past anything a legitimate use has
/// — an agent turn narrowing once, or narrowing again to evaluate one
/// expression — and far short of where the process is in danger.
pub const MAX_DEPTH: usize = 4;

thread_local! {
    /// How many sub-executions are on this thread's stack.
    ///
    /// Thread-local rather than carried in the context because it is a fact
    /// about the machine rather than about the caller, and because a
    /// sub-execution runs on the thread that asked for it — the whole chain is
    /// one blocking thread from the request down.
    static DEPTH: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// Increments the nesting depth and restores it when dropped.
///
/// A guard rather than a pair of calls: the sub-execution below it can return
/// early, throw, or be stopped by the interrupt handler, and a depth that only
/// decremented on the success path would ratchet up until the thread refused
/// everything.
pub struct DepthGuard;

impl DepthGuard {
    /// Enter one level, or report the depth that refused.
    pub fn enter() -> Result<Self, usize> {
        DEPTH.with(|depth| {
            let current = depth.get();
            if current >= MAX_DEPTH {
                return Err(current);
            }
            depth.set(current + 1);
            Ok(Self)
        })
    }
}

impl Drop for DepthGuard {
    fn drop(&mut self) {
        DEPTH.with(|depth| depth.set(depth.get().saturating_sub(1)));
    }
}

/// What went wrong before anything ran.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refusal {
    /// A name that is not a capability. Refused rather than ignored: a script
    /// asking for `read_only` has spelled something the engine does not have,
    /// and dropping it silently would hand back a context it believed it had
    /// checked.
    UnknownCapability(String),
    /// Capabilities the caller does not hold. Attenuation only ever narrows,
    /// so this could be treated as a no-op — but a script asking to *keep*
    /// something it never had has a bug, and one that shows up later as a
    /// refusal from inside the sub-execution rather than here.
    NotHeld(Vec<Capability>),
    /// The nesting limit.
    TooDeep(usize),
}

impl std::fmt::Display for Refusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Refusal::UnknownCapability(name) => {
                write!(f, "'{}' is not a capability", name)
            }
            Refusal::NotHeld(missing) => write!(
                f,
                "cannot keep what this execution does not hold: {}",
                missing
                    .iter()
                    .map(|capability| capability.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            Refusal::TooDeep(depth) => write!(
                f,
                "sandboxed executions are already {} deep, which is the limit",
                depth
            ),
        }
    }
}

/// Turn the names a script asked for into the context its sub-execution runs
/// with.
///
/// Both refusals are about the *request* rather than about the result: the
/// narrowing itself cannot fail, since an intersection is always a subset of
/// what the caller held. They exist so that a script hears about a mistake
/// where it made it.
pub fn narrow(caller: &UserContext, requested: &[String]) -> Result<UserContext, Refusal> {
    let mut keep = Vec::with_capacity(requested.len());
    for name in requested {
        match Capability::parse(name) {
            Some(capability) => keep.push(capability),
            None => return Err(Refusal::UnknownCapability(name.clone())),
        }
    }

    let missing = caller.unheld(keep.clone());
    if !missing.is_empty() {
        return Err(Refusal::NotHeld(missing));
    }

    Ok(caller.attenuated(keep))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn narrowing_keeps_the_identity_and_drops_everything_unnamed() {
        let caller = UserContext::admin("agent".to_string());

        let narrowed = narrow(
            &caller,
            &["read_script_data".to_string(), "read_assets".to_string()],
        )
        .expect("an administrator holds both of these");

        // Who is acting does not change. Ownership checks and per-person
        // secret resolution go on resolving against the same account.
        assert_eq!(narrowed.user_id, caller.user_id);
        assert!(narrowed.is_authenticated);
        assert!(narrowed.attenuated);

        assert!(narrowed.has_capability(&Capability::ReadScriptData));
        assert!(narrowed.has_capability(&Capability::ReadAssets));
        assert_eq!(narrowed.capabilities.len(), 2);
    }

    /// The point of the whole exercise: a context that may read a table and
    /// not write it, which was not expressible while one capability gated
    /// both.
    #[test]
    fn a_read_only_turn_is_expressible() {
        let caller = UserContext::authenticated("person".to_string());
        let narrowed = narrow(
            &caller,
            &[
                "read_script_data".to_string(),
                "read_storage".to_string(),
                "read_scripts".to_string(),
                "read_assets".to_string(),
            ],
        )
        .expect("an authenticated user holds all of these");

        assert!(narrowed.has_capability(&Capability::ReadScriptData));
        assert!(narrowed.has_capability(&Capability::ReadStorage));

        for denied in [
            Capability::WriteScriptData,
            Capability::WriteStorage,
            Capability::UseNetwork,
            Capability::ReadSecrets,
            Capability::EnqueueTasks,
            Capability::SendMessages,
        ] {
            assert!(
                !narrowed.has_capability(&denied),
                "a planning turn must not hold {:?}",
                denied
            );
        }
    }

    #[test]
    fn a_name_the_engine_does_not_have_is_refused() {
        let caller = UserContext::admin("agent".to_string());
        assert_eq!(
            narrow(&caller, &["read_only".to_string()]).err(),
            Some(Refusal::UnknownCapability("read_only".to_string()))
        );
    }

    /// Asking for more than you hold is a bug in the caller, reported here
    /// rather than as a puzzling refusal from inside the sub-execution.
    #[test]
    fn asking_to_keep_what_you_never_had_is_refused() {
        let caller = UserContext::authenticated("person".to_string());
        assert_eq!(
            narrow(&caller, &["write_scripts".to_string()]).err(),
            Some(Refusal::NotHeld(vec![Capability::WriteScripts]))
        );
    }

    /// Narrowing cannot widen, whatever is asked for. This is the floor under
    /// the check above rather than a second way of spelling it: even a caller
    /// that skipped `narrow` gets an intersection.
    #[test]
    fn attenuation_is_an_intersection_even_when_it_is_asked_to_widen() {
        let caller = UserContext::authenticated("person".to_string());
        let narrowed = caller.attenuated([
            Capability::ReadScriptData,
            Capability::AdministerEngine,
            Capability::WriteScripts,
        ]);

        assert!(narrowed.has_capability(&Capability::ReadScriptData));
        assert!(!narrowed.has_capability(&Capability::AdministerEngine));
        assert!(!narrowed.has_capability(&Capability::WriteScripts));
    }

    /// Narrowing a narrowed context narrows again. An agent that plans inside
    /// a turn that was itself restricted must not get back what the outer
    /// restriction took.
    #[test]
    fn narrowing_composes_downward() {
        let caller = UserContext::admin("agent".to_string());
        let once = narrow(
            &caller,
            &["read_script_data".to_string(), "use_network".to_string()],
        )
        .expect("held");
        let twice = narrow(&once, &["read_script_data".to_string()]).expect("held");

        assert!(twice.has_capability(&Capability::ReadScriptData));
        assert!(!twice.has_capability(&Capability::UseNetwork));

        // And the inner one cannot reach back out to what the outer dropped.
        assert_eq!(
            narrow(&once, &["write_script_data".to_string()]).err(),
            Some(Refusal::NotHeld(vec![Capability::WriteScriptData]))
        );
    }

    #[test]
    fn the_depth_guard_bottoms_out_and_recovers() {
        let mut held = Vec::new();
        for _ in 0..MAX_DEPTH {
            held.push(DepthGuard::enter().expect("within the limit"));
        }

        assert_eq!(DepthGuard::enter().err(), Some(MAX_DEPTH));

        // Dropping one frees exactly one level, so a chain that bottomed out
        // does not leave the thread refusing everything afterwards.
        held.pop();
        let recovered = DepthGuard::enter().expect("a level was freed");
        drop(recovered);
        drop(held);

        assert!(DepthGuard::enter().is_ok(), "the depth must unwind fully");
    }
}
