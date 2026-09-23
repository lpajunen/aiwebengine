//! Asking the person a question in the middle of a tool call.
//!
//! The Multi Round-Trip Requests pattern (`2026-07-28`) replaced
//! server-initiated requests, and the thing worth understanding before reading
//! any of this is that **nothing suspends**. The retry is a new request with a
//! new JSON-RPC id, and the specification is explicit that the server
//! processing it needs nothing beyond what that request carries — the pattern
//! exists so a server needs no shared storage and no sticky load balancing.
//!
//! So a handler that asks is *run again from the top* when the answer arrives.
//! [`Exchange`] is what makes the second pass differ from the first: it holds
//! what the client has already answered, so `mcp.ask` finds an answer instead
//! of ending the execution.
//!
//! That shape is right for this engine rather than merely tolerable. A host
//! call blocks the script and there is no event loop to yield to
//! (`TODO-agent.md` item 6), so suspending a handler would have been an
//! interpreter change; re-running one needs nothing new. And each round trip is
//! an ordinary call with an ordinary budget, so no execution slot is held while
//! a person thinks.
//!
//! What it costs is that everything before the first `ask` happens again on
//! every pass. `tasks.rs` re-runs handlers too, but only after a failure — this
//! re-runs a prologue that succeeded, which is sharper, and why `mcp.once`
//! exists beside the documented rule. See `docs/MCP_ELICITATION.md`.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::cell::RefCell;
use std::sync::OnceLock;

use crate::security::encryption::DataEncryption;

/// How long a person has to answer before the state they were asked about
/// stops being accepted.
///
/// This is the span of somebody reading a dialog and typing into it, not of a
/// session. Short because the replay window is exactly this long: the
/// specification bounds reuse with expiry, principal and request binding rather
/// than with single-use, so the expiry is doing real work.
const STATE_TTL_SECS: i64 = 600;

/// A ceiling on what one exchange may carry back and forth.
///
/// `requestState` travels to the client and back on every round trip, so this
/// bounds a script that would otherwise memoize a document into it. Refusing
/// loudly beats a tool that mysteriously stops working once its state outgrows
/// whatever the client is willing to echo.
const MAX_STATE_BYTES: usize = 64 * 1024;

/// The key `requestState` is sealed under.
static REQUEST_STATE_ENCRYPTION: OnceLock<DataEncryption> = OnceLock::new();

/// Derive the request-state key from the session key and install it.
///
/// Derived rather than configured: a setting an operator has to set is a
/// setting that will be missing, and the engine has been here before with
/// `security.api_key`. Derived rather than *reused* because domain separation
/// is free — a blob sealed for one purpose should not decrypt under another,
/// even though both keys live or die together.
///
/// **The key has to be the same on every instance**, which is the one thing
/// that cannot be got wrong here. MRTR exists so a retry can land anywhere
/// behind a load balancer; state sealed on one instance and presented to
/// another has to open, and a per-process key would silently reintroduce the
/// session affinity the pattern was designed to remove. That is why this reads
/// `security.session_encryption_key` — a value a cluster already shares —
/// rather than generating something.
///
/// With no session key configured there is nothing shared to derive from, so
/// this installs a random one and says so. Single-instance deployments keep
/// working; a cluster in that state would have failed on the second hop
/// anyway, and the warning is the only chance to notice before it does.
pub fn initialize_request_state_encryption(session_key: Option<&[u8; 32]>) {
    use sha2::{Digest, Sha256};

    let derived: [u8; 32] = match session_key {
        Some(session_key) => {
            let mut hasher = Sha256::new();
            hasher.update(b"aiwebengine/mcp/request-state/v1");
            hasher.update(session_key);
            hasher.finalize().into()
        }
        None => {
            tracing::warn!(
                "security.session_encryption_key is not configured, so MCP elicitation state is \
                 sealed under a key generated for this process. An exchange will not survive a \
                 restart, and on a multi-instance deployment a retry reaching another instance \
                 will be refused."
            );
            rand::random()
        }
    };
    let _ = REQUEST_STATE_ENCRYPTION.set(DataEncryption::new(&derived));
}

/// What travels to the client and back, once sealed.
///
/// Everything here is checked on the way in. The specification requires
/// `requestState` be treated as attacker-controlled, and requires integrity
/// protection wherever it influences authorization or business logic — which it
/// does here, since it carries the answers a handler will act on.
#[derive(Serialize, Deserialize)]
struct CarriedState {
    /// The account this was minted for. State presented by anybody else is
    /// refused, which is what stops one person's exchange being finished by
    /// another.
    principal: String,
    /// The host it was minted on. The specification does not ask for this; a
    /// deployment serving several hosts does, for the reason realms and token
    /// audiences already exist.
    host: String,
    /// The tool it belongs to, and a digest of the arguments it was called
    /// with, so state cannot be carried onto a different call.
    tool: String,
    args_digest: String,
    /// Unix seconds. Past this the state is refused however well-formed.
    expires_at: i64,
    /// What the person has answered so far, keyed as `inputRequests` was.
    answers: Map<String, Value>,
    /// What `mcp.once` has already computed.
    memo: Map<String, Value>,
}

/// Why a presented `requestState` was not accepted.
///
/// Distinguished for the log rather than for the client: every one of these
/// answers the caller the same way, because telling somebody probing a tool
/// *which* check they failed is telling them how to pass it.
#[derive(Debug, PartialEq, Eq)]
pub enum StateRejected {
    Unsealed,
    Expired,
    WrongPrincipal,
    WrongHost,
    WrongRequest,
    TooLarge,
}

/// A canonical digest of a tool call's arguments.
///
/// `serde_json` orders object keys when a `Map` is a `BTreeMap`, which is the
/// crate's default feature set here, so the same arguments hash the same way
/// on both passes. The digest rather than the arguments themselves, because
/// this rides in a blob the client stores.
pub fn digest_arguments(tool: &str, arguments: &Value) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(tool.as_bytes());
    hasher.update(b"\0");
    hasher.update(arguments.to_string().as_bytes());
    hex::encode(hasher.finalize())
}

/// Seal what the next pass will need.
pub fn seal(
    principal: &str,
    host: &str,
    tool: &str,
    args_digest: &str,
    answers: Map<String, Value>,
    memo: Map<String, Value>,
) -> Result<String, String> {
    let encryption = REQUEST_STATE_ENCRYPTION
        .get()
        .ok_or_else(|| "request state encryption is not initialized".to_string())?;

    let carried = CarriedState {
        principal: principal.to_string(),
        host: host.to_string(),
        tool: tool.to_string(),
        args_digest: args_digest.to_string(),
        expires_at: chrono::Utc::now().timestamp() + STATE_TTL_SECS,
        answers,
        memo,
    };

    let plaintext =
        serde_json::to_string(&carried).map_err(|e| format!("request state encode: {e}"))?;
    if plaintext.len() > MAX_STATE_BYTES {
        return Err(format!(
            "request state is {} bytes, over the {MAX_STATE_BYTES}-byte ceiling — \
             mcp.once is for a decision or an identifier, not a document",
            plaintext.len()
        ));
    }

    let sealed = encryption
        .encrypt_field(&plaintext)
        .map_err(|e| format!("request state seal: {e}"))?;
    serde_json::to_string(&sealed).map_err(|e| format!("request state envelope: {e}"))
}

/// What an accepted `requestState` was carrying: the answers given so far, and
/// whatever `mcp.once` has already computed.
pub type Carried = (Map<String, Value>, Map<String, Value>);

/// Open a presented `requestState`, refusing anything that does not match the
/// request presenting it.
pub fn open(
    state: &str,
    principal: &str,
    host: &str,
    tool: &str,
    args_digest: &str,
) -> Result<Carried, StateRejected> {
    if state.len() > MAX_STATE_BYTES * 2 {
        return Err(StateRejected::TooLarge);
    }
    let encryption = REQUEST_STATE_ENCRYPTION
        .get()
        .ok_or(StateRejected::Unsealed)?;

    let envelope = serde_json::from_str(state).map_err(|_| StateRejected::Unsealed)?;
    let plaintext = encryption
        .decrypt_field(&envelope)
        .map_err(|_| StateRejected::Unsealed)?;
    let carried: CarriedState =
        serde_json::from_str(&plaintext).map_err(|_| StateRejected::Unsealed)?;

    // Order matters only for what gets logged: each of these is refused the
    // same way, and the cheapest checks come first.
    if carried.expires_at <= chrono::Utc::now().timestamp() {
        return Err(StateRejected::Expired);
    }
    if carried.principal != principal {
        return Err(StateRejected::WrongPrincipal);
    }
    if carried.host != host {
        return Err(StateRejected::WrongHost);
    }
    if carried.tool != tool || carried.args_digest != args_digest {
        return Err(StateRejected::WrongRequest);
    }

    Ok((carried.answers, carried.memo))
}

/// Whether the client declared that it can be asked.
///
/// A server **MUST NOT** send an input request a client has not declared
/// support for. An `elicitation` key present but empty means form mode, which
/// is the only mode the engine sends, so its mere presence is the answer;
/// `form` named explicitly is the same claim said longer.
pub fn client_can_elicit(client_capabilities: Option<&Value>) -> bool {
    let Some(elicitation) = client_capabilities.and_then(|caps| caps.get("elicitation")) else {
        return false;
    };
    let Some(declared) = elicitation.as_object() else {
        return false;
    };

    match declared.get("form") {
        Some(form) => form.is_object(),
        // An empty object is the specification's shorthand for form mode. A
        // *non*-empty one that does not name `form` named some other mode —
        // `url`, today — and saying nothing about form there is saying no. The
        // shorthand only applies when nothing was said at all.
        None => declared.is_empty(),
    }
}

/// One tool call's side of an exchange: what is known coming in, and what the
/// handler asked for on the way out.
#[derive(Debug, Default)]
pub struct Exchange {
    answers: Map<String, Value>,
    memo: Map<String, Value>,
    can_ask: bool,
    pending: Map<String, Value>,
    fresh_memo: Map<String, Value>,
}

impl Exchange {
    /// An exchange for a caller who can be asked, carrying whatever earlier
    /// passes established.
    pub fn new(answers: Map<String, Value>, memo: Map<String, Value>, can_ask: bool) -> Self {
        Self {
            answers,
            memo,
            can_ask,
            ..Default::default()
        }
    }

    /// An exchange for an execution with nobody to ask — a scheduled job, a
    /// delegated task, a listener. `mcp.canAsk()` is false and `mcp.ask`
    /// throws, rather than the engine returning `input_required` to a client
    /// that never made a request.
    pub fn unattended() -> Self {
        Self::default()
    }
}

thread_local! {
    /// The exchange the execution on this thread belongs to.
    ///
    /// A thread-local for the reason `http_client`'s stream registry and the
    /// host-call budget are: a global installed by `secure_globals` captures a
    /// clone at install time, and what the *current* call needs to see is
    /// whatever the execution around it established. Nested executions are
    /// handled by the guard, which restores its predecessor on drop.
    static EXCHANGE: RefCell<Option<Exchange>> = const { RefCell::new(None) };
}

/// Installs an exchange for as long as it is held, then gives it back.
///
/// Restores whatever was in place rather than clearing, so a tool call reached
/// from inside another execution — `dispatcher.sendMessage` builds a runtime
/// inside a running host call — cannot erase its caller's exchange.
pub struct ExchangeGuard {
    previous: Option<Exchange>,
}

impl ExchangeGuard {
    pub fn install(exchange: Exchange) -> Self {
        let previous = EXCHANGE.with(|cell| cell.borrow_mut().replace(exchange));
        Self { previous }
    }

    /// Take back what the handler asked for and memoized, putting whatever was
    /// installed before back in its place.
    ///
    /// Consumes the guard, which is why there is no `Drop`: an exchange that is
    /// never finished should be lost rather than left installed for whatever
    /// runs next on this thread.
    pub fn finish(self) -> Exchange {
        EXCHANGE
            .with(|cell| {
                let mut slot = cell.borrow_mut();
                let taken = slot.take();
                *slot = self.previous;
                taken
            })
            .unwrap_or_default()
    }
}

/// What the person already said for this key, if anything.
pub fn answer_for(key: &str) -> Option<Value> {
    EXCHANGE.with(|cell| {
        cell.borrow()
            .as_ref()
            .and_then(|exchange| exchange.answers.get(key).cloned())
    })
}

/// Whether this caller can be asked at all.
pub fn can_ask() -> bool {
    EXCHANGE.with(|cell| {
        cell.borrow()
            .as_ref()
            .map(|exchange| exchange.can_ask)
            .unwrap_or(false)
    })
}

/// Record a question. The prelude throws immediately afterwards, which ends the
/// execution; the outcome is decided by what is recorded here rather than by
/// the exception, so a script that catches it still ends its turn asking.
pub fn record_ask(key: &str, request: Value) {
    EXCHANGE.with(|cell| {
        if let Some(exchange) = cell.borrow_mut().as_mut() {
            exchange.pending.insert(key.to_string(), request);
        }
    });
}

/// What `mcp.once` computed on an earlier pass.
pub fn memo_get(key: &str) -> Option<Value> {
    EXCHANGE.with(|cell| {
        cell.borrow().as_ref().and_then(|exchange| {
            exchange
                .fresh_memo
                .get(key)
                .or_else(|| exchange.memo.get(key))
                .cloned()
        })
    })
}

/// Remember what `mcp.once` computed, for every later pass.
pub fn memo_set(key: &str, value: Value) {
    EXCHANGE.with(|cell| {
        if let Some(exchange) = cell.borrow_mut().as_mut() {
            exchange.fresh_memo.insert(key.to_string(), value);
        }
    });
}

/// How many questions this pass has asked, which names the next one.
pub fn asked_so_far() -> usize {
    EXCHANGE.with(|cell| {
        cell.borrow()
            .as_ref()
            .map(|exchange| exchange.pending.len())
            .unwrap_or(0)
    })
}

/// What a finished execution leaves behind: the questions it asked, and
/// everything the next pass has to be told.
pub struct Asked {
    pub requests: Map<String, Value>,
    pub answers: Map<String, Value>,
    pub memo: Map<String, Value>,
}

impl Exchange {
    /// `None` when the handler asked nothing, which is the ordinary case.
    pub fn into_asked(self) -> Option<Asked> {
        if self.pending.is_empty() {
            return None;
        }
        let mut memo = self.memo;
        memo.extend(self.fresh_memo);
        Some(Asked {
            requests: self.pending,
            answers: self.answers,
            memo,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key() -> [u8; 32] {
        [7u8; 32]
    }

    fn sealed_key() {
        initialize_request_state_encryption(Some(&key()));
    }

    fn answers(pairs: &[(&str, &str)]) -> Map<String, Value> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_string(), Value::String((*v).to_string())))
            .collect()
    }

    #[test]
    fn a_capability_declared_empty_means_form_mode() {
        // The specification's backwards-compatible shorthand: `elicitation: {}`
        // is support for form mode, which is the only mode the engine sends.
        let empty = serde_json::json!({ "elicitation": {} });
        assert!(client_can_elicit(Some(&empty)));

        let explicit = serde_json::json!({ "elicitation": { "form": {} } });
        assert!(client_can_elicit(Some(&explicit)));

        let both = serde_json::json!({ "elicitation": { "form": {}, "url": {} } });
        assert!(client_can_elicit(Some(&both)));
    }

    #[test]
    fn a_client_that_declared_nothing_must_not_be_asked() {
        assert!(!client_can_elicit(None));
        assert!(!client_can_elicit(Some(&serde_json::json!({}))));
        assert!(
            !client_can_elicit(Some(&serde_json::json!({ "sampling": {} }))),
            "another capability is not this one"
        );
        assert!(
            !client_can_elicit(Some(&serde_json::json!({ "elicitation": { "url": {} } }))),
            "url-only is a real declaration, and not one the engine can send to yet"
        );
    }

    #[test]
    fn state_round_trips_for_the_request_that_minted_it() {
        sealed_key();
        let memo = answers(&[("draft", "d-1")]);
        let state = seal(
            "user-1",
            "example.test",
            "open_issue",
            "digest-1",
            answers(&[("ask_0", "octocat")]),
            memo.clone(),
        )
        .expect("sealing should work once a key is installed");

        let (opened, opened_memo) =
            open(&state, "user-1", "example.test", "open_issue", "digest-1")
                .expect("the request that minted it opens it");
        assert_eq!(opened["ask_0"], "octocat");
        assert_eq!(opened_memo, memo);
    }

    #[test]
    fn state_is_refused_for_anybody_but_the_account_it_was_minted_for() {
        sealed_key();
        let state = seal("user-1", "example.test", "t", "d", Map::new(), Map::new())
            .expect("sealing should work");

        assert_eq!(
            open(&state, "user-2", "example.test", "t", "d"),
            Err(StateRejected::WrongPrincipal),
            "one person's exchange must not be finishable by another"
        );
    }

    #[test]
    fn state_does_not_cross_hosts() {
        sealed_key();
        let state = seal("user-1", "a.example", "t", "d", Map::new(), Map::new())
            .expect("sealing should work");

        // Not a rule the specification states. A deployment serving several
        // hosts needs it for the reason realms and token audiences exist.
        assert_eq!(
            open(&state, "user-1", "b.example", "t", "d"),
            Err(StateRejected::WrongHost)
        );
    }

    #[test]
    fn state_cannot_be_moved_onto_a_different_call() {
        sealed_key();
        let state = seal(
            "user-1",
            "a.example",
            "open_issue",
            "d",
            Map::new(),
            Map::new(),
        )
        .expect("sealing should work");

        assert_eq!(
            open(&state, "user-1", "a.example", "delete_repo", "d"),
            Err(StateRejected::WrongRequest),
            "answers given to one question must not arrive at another tool"
        );
        assert_eq!(
            open(&state, "user-1", "a.example", "open_issue", "other-digest"),
            Err(StateRejected::WrongRequest),
            "nor at the same tool called with different arguments"
        );
    }

    #[test]
    fn tampering_is_refused_rather_than_read() {
        sealed_key();
        let state = seal("user-1", "a.example", "t", "d", Map::new(), Map::new())
            .expect("sealing should work");

        // Flip something inside the sealed envelope. AEAD is what makes this a
        // refusal rather than a differently-shaped success.
        let mut envelope: serde_json::Value =
            serde_json::from_str(&state).expect("the envelope is JSON");
        envelope["ciphertext"] = serde_json::json!("YWFhYWFhYWFhYWFhYWFhYQ==");
        let tampered = envelope.to_string();

        assert_eq!(
            open(&tampered, "user-1", "a.example", "t", "d"),
            Err(StateRejected::Unsealed)
        );
        assert_eq!(
            open("not even json", "user-1", "a.example", "t", "d"),
            Err(StateRejected::Unsealed)
        );
    }

    #[test]
    fn the_arguments_digest_is_stable_and_specific() {
        let args = serde_json::json!({ "repo": "a/b", "count": 2 });
        assert_eq!(
            digest_arguments("t", &args),
            digest_arguments("t", &args),
            "both passes of one exchange have to agree"
        );
        assert_ne!(digest_arguments("t", &args), digest_arguments("u", &args));
        assert_ne!(
            digest_arguments("t", &args),
            digest_arguments("t", &serde_json::json!({ "repo": "a/b", "count": 3 }))
        );
    }

    #[test]
    fn an_exchange_hands_back_only_what_was_asked() {
        let guard = ExchangeGuard::install(Exchange::new(
            answers(&[("ask_0", "already said")]),
            Map::new(),
            true,
        ));

        assert!(can_ask());
        assert_eq!(
            answer_for("ask_0"),
            Some(Value::String("already said".into()))
        );
        assert_eq!(answer_for("ask_1"), None);
        assert_eq!(asked_so_far(), 0);

        let finished = guard.finish();
        assert!(
            finished.into_asked().is_none(),
            "a pass that asked nothing is an ordinary result"
        );
    }

    #[test]
    fn asking_is_what_makes_a_pass_incomplete() {
        let guard = ExchangeGuard::install(Exchange::new(Map::new(), Map::new(), true));
        record_ask(
            "ask_0",
            serde_json::json!({ "method": "elicitation/create" }),
        );
        assert_eq!(asked_so_far(), 1);

        let asked = guard
            .finish()
            .into_asked()
            .expect("a pass that asked is incomplete");
        assert_eq!(asked.requests.len(), 1);
        assert!(asked.requests.contains_key("ask_0"));
    }

    #[test]
    fn a_memo_survives_into_the_next_pass_and_is_visible_within_this_one() {
        let guard = ExchangeGuard::install(Exchange::new(
            Map::new(),
            answers(&[("earlier", "kept")]),
            true,
        ));

        assert_eq!(memo_get("earlier"), Some(Value::String("kept".into())));
        memo_set("fresh", serde_json::json!({ "value": 1 }));
        assert_eq!(
            memo_get("fresh"),
            Some(serde_json::json!({ "value": 1 })),
            "mcp.once must not run twice within one pass either"
        );
        record_ask("ask_0", serde_json::json!({}));

        let asked = guard.finish().into_asked().expect("asked");
        assert_eq!(asked.memo["earlier"], "kept");
        assert_eq!(asked.memo["fresh"], serde_json::json!({ "value": 1 }));
    }

    #[test]
    fn an_unattended_execution_can_neither_be_asked_nor_ask() {
        let guard = ExchangeGuard::install(Exchange::unattended());
        assert!(!can_ask(), "a scheduled job has nobody to ask");
        assert_eq!(answer_for("ask_0"), None);
        let _ = guard.finish();
    }

    #[test]
    fn a_nested_execution_restores_the_exchange_it_interrupted() {
        // `dispatcher.sendMessage` builds a runtime inside a running host call,
        // so an inner tool call must not erase its caller's exchange.
        let outer = ExchangeGuard::install(Exchange::new(
            answers(&[("ask_0", "outer")]),
            Map::new(),
            true,
        ));

        let inner = ExchangeGuard::install(Exchange::unattended());
        assert!(!can_ask(), "the inner execution has its own exchange");
        let _ = inner.finish();

        assert!(can_ask(), "and the outer one is back afterwards");
        assert_eq!(answer_for("ask_0"), Some(Value::String("outer".into())));
        let _ = outer.finish();
    }

    #[test]
    fn nothing_is_asked_when_there_is_no_exchange_at_all() {
        // An ordinary HTTP route handler installs none, and every accessor has
        // to answer rather than panic.
        assert!(!can_ask());
        assert_eq!(answer_for("anything"), None);
        assert_eq!(memo_get("anything"), None);
        assert_eq!(asked_so_far(), 0);
        record_ask("ask_0", serde_json::json!({}));
        memo_set("k", serde_json::json!(1));
    }
}
