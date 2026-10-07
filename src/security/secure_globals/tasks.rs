//! `scriptTasks` and `personalTasks`: work enqueued from any phase.

use super::*;
use crate::security::Capability;
use rquickjs::{Function, Result as JsResult};
use tracing::{debug, info};

pub(super) const TASKS_PRELUDE: &str = include_str!("../../../assets/tasks_prelude.js");

impl SecureGlobalContext {
    /// The Rust half of `scriptTasks` — the durable queue ([`crate::tasks`]).
    ///
    /// Deliberately *not* phase-gated, unlike `schedulerService` beside it.
    /// That gate is right for a declaration, which belongs to the version of
    /// the code that declared it; a task is a piece of work that exists because
    /// something happened, and the shape it is for — start during a request,
    /// answer, finish afterwards — only works if a handler can enqueue.
    ///
    /// Each method answers with an envelope rather than a value, and
    /// `tasks_prelude.js` turns that into a returned object or a thrown
    /// `Error`. A host binding cannot throw the right kind of exception, and
    /// answering `"Error: ..."` as the value is indistinguishable from an
    /// answer that happens to be a string.
    pub(super) fn setup_task_functions(
        &self,
        ctx: &rquickjs::Ctx<'_>,
        script_uri: &str,
    ) -> JsResult<()> {
        let global = ctx.globals();
        let host = rquickjs::Object::new(ctx.clone())?;

        let script_uri_enqueue = script_uri.to_string();
        let config_enqueue = self.config.clone();
        let user_enqueue = self.user_context.clone();
        let enqueue = Function::new(
            ctx.clone(),
            move |options_json: String| -> JsResult<String> {
                if config_enqueue.is_dry_run() {
                    // A check that deploys nothing must not leave work behind
                    // for a worker to pick up afterwards.
                    return Ok(host_failure(
                        "DryRunError",
                        "scriptTasks.enqueue: nothing was enqueued - this is a dry run",
                    ));
                }

                // Queueing is how an execution outlives itself: a task runs
                // later, in script context, holding what the script holds
                // rather than what this turn was narrowed to. So a turn that
                // may not write must not be able to queue a write for
                // afterwards — that is the whole of the loophole, and it is
                // closed here rather than at claim time because the narrowing
                // is a fact about this execution and nothing in the row
                // remembers it.
                if !user_enqueue.has_capability(&Capability::EnqueueTasks) {
                    return Ok(host_failure(
                        "SecurityError",
                        &capability_refusal(
                            "scriptTasks.enqueue",
                            &Capability::EnqueueTasks,
                            &user_enqueue,
                        ),
                    ));
                }

                let options: serde_json::Value = match serde_json::from_str(&options_json) {
                    Ok(options) => options,
                    Err(e) => {
                        return Ok(host_failure(
                            "TypeError",
                            &format!("scriptTasks.enqueue: options are not valid JSON: {}", e),
                        ));
                    }
                };

                let lane = match Self::lane_from_options(&options) {
                    Ok(lane) => lane,
                    Err(message) => {
                        return Ok(host_failure(
                            "TypeError",
                            &format!("scriptTasks.enqueue: {}", message),
                        ));
                    }
                };

                let run_at = match options.get("runAt").and_then(|v| v.as_str()) {
                    Some(value) => match crate::scheduler::parse_utc_timestamp(value) {
                        Ok(parsed) => Some(parsed),
                        Err(_) => {
                            return Ok(host_failure(
                                "RangeError",
                                "scriptTasks.enqueue: runAt must be a UTC timestamp ending with 'Z'",
                            ));
                        }
                    },
                    None => None,
                };

                let max_attempts = match options.get("maxAttempts") {
                    Some(serde_json::Value::Null) | None => None,
                    Some(value) => match value.as_i64() {
                        Some(number) if (i32::MIN as i64..=i32::MAX as i64).contains(&number) => {
                            Some(number as i32)
                        }
                        _ => {
                            return Ok(host_failure(
                                "RangeError",
                                "scriptTasks.enqueue: maxAttempts must be a whole number",
                            ));
                        }
                    },
                };

                let new_task = crate::tasks::NewTask {
                    script_uri: script_uri_enqueue.clone(),
                    handler_name: options
                        .get("handler")
                        .and_then(|v| v.as_str())
                        .unwrap_or_default()
                        .to_string(),
                    payload: options
                        .get("payload")
                        .cloned()
                        .unwrap_or_else(|| serde_json::json!({})),
                    run_at,
                    max_attempts,
                    // Recorded as where the work came from, not as an authority
                    // it runs under: a task runs in script context.
                    enqueued_by: user_enqueue.user_id.clone(),
                    // Script context. `personalTasks.enqueue` is the one that
                    // acts as somebody, and it takes a grant to do it.
                    run_as: None,
                    // No default here, unlike the personal queue. A script
                    // task belongs to the solution rather than to one
                    // person, so there is nothing for the engine to infer a
                    // lane from — and defaulting every script task into one
                    // lane would serialise the whole queue.
                    lane: lane.flatten(),
                };

                match crate::tasks::blocking::enqueue(new_task) {
                    Ok(task) => Ok(host_ok(crate::tasks::to_json(&task))),
                    Err(e) => Ok(host_failure(
                        Self::task_error_name(&e),
                        &format!("scriptTasks.enqueue: {}", e),
                    )),
                }
            },
        )?;
        host.set("enqueue", enqueue)?;

        let config_cancel = self.config.clone();
        let user_cancel = self.user_context.clone();
        let cancel = Function::new(ctx.clone(), move |task_id: String| -> JsResult<String> {
            if config_cancel.is_dry_run() {
                return Ok(host_failure(
                    "DryRunError",
                    "scriptTasks.cancel: nothing was cancelled - this is a dry run",
                ));
            }

            // Cancelling is changing the queue, so it takes the same
            // capability enqueueing does. A narrowed turn that could delete
            // work the script had accepted would be a write by another name.
            if !user_cancel.has_capability(&Capability::EnqueueTasks) {
                return Ok(host_failure(
                    "SecurityError",
                    &capability_refusal(
                        "scriptTasks.cancel",
                        &Capability::EnqueueTasks,
                        &user_cancel,
                    ),
                ));
            }

            let Ok(parsed) = uuid::Uuid::parse_str(task_id.trim()) else {
                return Ok(host_failure(
                    "TypeError",
                    "scriptTasks.cancel: that is not a task id",
                ));
            };

            match crate::tasks::blocking::cancel(parsed) {
                Ok(cancelled) => Ok(host_ok(serde_json::Value::Bool(cancelled))),
                Err(e) => Ok(host_failure("Error", &format!("scriptTasks.cancel: {}", e))),
            }
        })?;
        host.set("cancel", cancel)?;

        // Scoped to the calling script, so one script cannot read another's
        // queue by holding an id. Everything else here is already per script
        // because the URI comes from the binding rather than the caller.
        let script_uri_get = script_uri.to_string();
        let get = Function::new(ctx.clone(), move |task_id: String| -> JsResult<String> {
            let Ok(parsed) = uuid::Uuid::parse_str(task_id.trim()) else {
                return Ok(host_failure(
                    "TypeError",
                    "scriptTasks.get: that is not a task id",
                ));
            };

            match crate::tasks::blocking::get(parsed) {
                Ok(Some(task)) if task.script_uri == script_uri_get => {
                    Ok(host_ok(crate::tasks::to_json(&task)))
                }
                Ok(_) => Ok(host_ok(serde_json::Value::Null)),
                Err(e) => Ok(host_failure("Error", &format!("scriptTasks.get: {}", e))),
            }
        })?;
        host.set("get", get)?;

        // `personalTasks` — the same queue, acting as the person who asked.
        //
        // Three things have to hold before a row is written, and all three are
        // checked again when the task runs: there is an authenticated user,
        // they have granted this script a delegation, and it has not lapsed.
        // Checking here as well is not redundant — it is what lets a script
        // find out *now* that it needs to send someone to the consent page,
        // rather than queueing work that will be abandoned later.
        let script_uri_personal = script_uri.to_string();
        let config_personal = self.config.clone();
        let user_personal_enqueue = self.user_context.clone();
        let personal_enqueue = Function::new(
            ctx.clone(),
            move |ctx: rquickjs::Ctx<'_>, options_json: String| -> JsResult<String> {
                if config_personal.is_dry_run() {
                    return Ok(host_failure(
                        "DryRunError",
                        "personalTasks.enqueue: nothing was enqueued - this is a dry run",
                    ));
                }

                // As `scriptTasks.enqueue`: work queued now runs later under
                // what the grant allows, which is not what this turn was
                // narrowed to.
                if !user_personal_enqueue.has_capability(&Capability::EnqueueTasks) {
                    return Ok(host_failure(
                        "SecurityError",
                        &capability_refusal(
                            "personalTasks.enqueue",
                            &Capability::EnqueueTasks,
                            &user_personal_enqueue,
                        ),
                    ));
                }

                // The person this would act as is the one making the request,
                // read from the live context rather than from the binding: a
                // script serving a request runs under the requesting user, and
                // that is who is in a position to have consented.
                let Some(user_id) = Self::current_user_id(&ctx) else {
                    return Ok(host_failure(
                        "SecurityError",
                        "personalTasks.enqueue requires an authenticated user",
                    ));
                };

                match crate::database::run_blocking(crate::delegation::get(
                    &user_id,
                    &script_uri_personal,
                )) {
                    Ok(Some(grant)) if grant.is_live(chrono::Utc::now()) => {}
                    Ok(Some(_)) => {
                        return Ok(host_failure(
                            "SecurityError",
                            "personalTasks.enqueue: this person's authorisation for this script has expired",
                        ));
                    }
                    Ok(None) => {
                        return Ok(host_failure(
                            "SecurityError",
                            "personalTasks.enqueue: this person has not authorised this script to act for them",
                        ));
                    }
                    Err(e) => {
                        return Ok(host_failure(
                            "Error",
                            &format!(
                                "personalTasks.enqueue: could not read the authorisation: {}",
                                e
                            ),
                        ));
                    }
                }

                let options: serde_json::Value = match serde_json::from_str(&options_json) {
                    Ok(options) => options,
                    Err(e) => {
                        return Ok(host_failure(
                            "TypeError",
                            &format!("personalTasks.enqueue: options are not valid JSON: {}", e),
                        ));
                    }
                };

                let lane = match Self::lane_from_options(&options) {
                    Ok(lane) => lane,
                    Err(message) => {
                        return Ok(host_failure(
                            "TypeError",
                            &format!("personalTasks.enqueue: {}", message),
                        ));
                    }
                };

                let run_at = match options.get("runAt").and_then(|v| v.as_str()) {
                    Some(value) => match crate::scheduler::parse_utc_timestamp(value) {
                        Ok(parsed) => Some(parsed),
                        Err(_) => {
                            return Ok(host_failure(
                                "RangeError",
                                "personalTasks.enqueue: runAt must be a UTC timestamp ending with 'Z'",
                            ));
                        }
                    },
                    None => None,
                };

                let new_task = crate::tasks::NewTask {
                    script_uri: script_uri_personal.clone(),
                    handler_name: options
                        .get("handler")
                        .and_then(|v| v.as_str())
                        .unwrap_or_default()
                        .to_string(),
                    payload: options
                        .get("payload")
                        .cloned()
                        .unwrap_or_else(|| serde_json::json!({})),
                    run_at,
                    max_attempts: options
                        .get("maxAttempts")
                        .and_then(|v| v.as_i64())
                        .map(|n| n.clamp(i32::MIN as i64, i32::MAX as i64) as i32),
                    enqueued_by: Some(user_id.clone()),
                    run_as: Some(user_id.clone()),
                    // Per person by default, and this is the correctness fix
                    // rather than a convenience. Two prompts from one person
                    // otherwise become two runs interleaving turn for turn,
                    // each reading and overwriting the same
                    // `personalStorage` — which every script queueing
                    // per-person work had to work around with a table of its
                    // own. A caller that really wants them in parallel says
                    // `lane: null`, and one that wants a finer lane than the
                    // person names its own.
                    lane: match lane {
                        Some(Some(named)) => Some(named),
                        Some(None) => Some(Self::person_lane(&user_id)),
                        None => None,
                    },
                };

                match crate::tasks::blocking::enqueue(new_task) {
                    Ok(task) => Ok(host_ok(crate::tasks::to_json(&task))),
                    Err(e) => Ok(host_failure(
                        Self::task_error_name(&e),
                        &format!("personalTasks.enqueue: {}", e),
                    )),
                }
            },
        )?;
        host.set("enqueuePersonal", personal_enqueue)?;

        // `personalTasks.enqueueFrom` — work for a person the script did not
        // serve, named by the sender a message came from.
        //
        // This is the only way an execution with nobody signed in can act as
        // somebody, and it exists because an inbound webhook is exactly that:
        // a Telegram or Slack message arrives, there is no session, so there
        // is no person, so there was nothing to enqueue against and every
        // channel an agent might live on was out of reach.
        //
        // The shape is the security decision. A script cannot name a *person*
        // — no user id is accepted here — it names a sender as the channel
        // reports them, and the engine resolves that through the bindings
        // people have consented to (`delegation::resolve_channel`). So a
        // script that trusts the wrong field in a request body can be made to
        // claim the wrong sender, and not to name a different account: an
        // unbound sender resolves to nobody and nothing runs, and there is no
        // id to enumerate.
        //
        // What the engine cannot do is verify the message really came from
        // that sender. Telegram and Slack sign their webhooks and email
        // largely does not; checking the signature is the script's job, and
        // is said so in the documentation rather than pretended at here.
        let script_uri_from = script_uri.to_string();
        let config_from = self.config.clone();
        let user_from = self.user_context.clone();
        let enqueue_from = Function::new(
            ctx.clone(),
            move |options_json: String| -> JsResult<String> {
                if config_from.is_dry_run() {
                    return Ok(host_failure(
                        "DryRunError",
                        "personalTasks.enqueueFrom: nothing was enqueued - this is a dry run",
                    ));
                }

                if !user_from.has_capability(&Capability::EnqueueTasks) {
                    return Ok(host_failure(
                        "SecurityError",
                        &capability_refusal(
                            "personalTasks.enqueueFrom",
                            &Capability::EnqueueTasks,
                            &user_from,
                        ),
                    ));
                }

                let options: serde_json::Value = match serde_json::from_str(&options_json) {
                    Ok(options) => options,
                    Err(e) => {
                        return Ok(host_failure(
                            "TypeError",
                            &format!(
                                "personalTasks.enqueueFrom: options are not valid JSON: {}",
                                e
                            ),
                        ));
                    }
                };

                let (channel, identity) = match Self::sender_from_options(&options) {
                    Ok(pair) => pair,
                    Err(message) => {
                        return Ok(host_failure("TypeError", &message));
                    }
                };

                // Who that sender is here. Refused before anything else is
                // read, so an unlinked sender costs one indexed lookup.
                let user_id = match crate::database::run_blocking(
                    crate::delegation::resolve_channel(&script_uri_from, &channel, &identity),
                ) {
                    Ok(Some(user_id)) => user_id,
                    Ok(None) => {
                        return Ok(host_failure(
                            "SecurityError",
                            "personalTasks.enqueueFrom: nobody has linked that sender to this \
                             script - `personalTasks.inviteLink()` mints a link to send them",
                        ));
                    }
                    Err(e) => {
                        return Ok(host_failure(
                            "Error",
                            &format!("personalTasks.enqueueFrom: could not read the link: {}", e),
                        ));
                    }
                };

                // The same grant check the caller-facing enqueue makes, and
                // for the same reason: a link says which sender may trigger
                // the work, and the grant says whether there is any work to
                // trigger. Both, or neither.
                match crate::database::run_blocking(crate::delegation::get(
                    &user_id,
                    &script_uri_from,
                )) {
                    Ok(Some(grant)) if grant.is_live(chrono::Utc::now()) => {}
                    Ok(Some(_)) => {
                        return Ok(host_failure(
                            "SecurityError",
                            "personalTasks.enqueueFrom: this person's authorisation for this \
                             script has expired",
                        ));
                    }
                    Ok(None) => {
                        return Ok(host_failure(
                            "SecurityError",
                            "personalTasks.enqueueFrom: this person has not authorised this \
                             script to act for them",
                        ));
                    }
                    Err(e) => {
                        return Ok(host_failure(
                            "Error",
                            &format!(
                                "personalTasks.enqueueFrom: could not read the authorisation: {}",
                                e
                            ),
                        ));
                    }
                }

                // The one budget in the engine whose spender is chosen by
                // whoever sent the message. Spent after the link and the
                // grant are established, so an unlinked sender cannot drain
                // a stranger's budget by guessing at it.
                if let Err(message) =
                    Self::spend_channel_budget(&script_uri_from, &channel, &identity)
                {
                    return Ok(host_failure("RangeError", &message));
                }

                let lane = match Self::lane_from_options(&options) {
                    Ok(lane) => lane,
                    Err(message) => {
                        return Ok(host_failure(
                            "TypeError",
                            &format!("personalTasks.enqueueFrom: {}", message),
                        ));
                    }
                };

                let run_at = match options.get("runAt").and_then(|v| v.as_str()) {
                    Some(value) => match crate::scheduler::parse_utc_timestamp(value) {
                        Ok(parsed) => Some(parsed),
                        Err(_) => {
                            return Ok(host_failure(
                                "RangeError",
                                "personalTasks.enqueueFrom: runAt must be a UTC timestamp ending \
                                 with 'Z'",
                            ));
                        }
                    },
                    None => None,
                };

                let new_task = crate::tasks::NewTask {
                    script_uri: script_uri_from.clone(),
                    handler_name: options
                        .get("handler")
                        .and_then(|v| v.as_str())
                        .unwrap_or_default()
                        .to_string(),
                    payload: options
                        .get("payload")
                        .cloned()
                        .unwrap_or_else(|| serde_json::json!({})),
                    run_at,
                    max_attempts: options
                        .get("maxAttempts")
                        .and_then(|v| v.as_i64())
                        .map(|n| n.clamp(i32::MIN as i64, i32::MAX as i64) as i32),
                    // Nobody signed in, so there is nobody to record as
                    // having asked. `run_as` is the whole of the authority
                    // and it was worked out above, never taken from input.
                    enqueued_by: None,
                    run_as: Some(user_id.clone()),
                    // Per person by default, as the caller-facing enqueue
                    // is, and here it is the case the lane key was written
                    // for: two messages arriving a second apart from the
                    // same chat are exactly the two runs that must not
                    // interleave.
                    lane: match lane {
                        Some(Some(named)) => Some(named),
                        Some(None) => Some(Self::person_lane(&user_id)),
                        None => None,
                    },
                };

                match crate::tasks::blocking::enqueue(new_task) {
                    Ok(task) => {
                        // Worth a line: this is work queued to act as
                        // somebody by a request they did not make, and the
                        // person asking later how their agent came to run
                        // needs to see which sender set it going.
                        info!(
                            script_uri = %script_uri_from,
                            user_id = %user_id,
                            channel = %channel,
                            task_id = %task.task_id,
                            "Delegated work queued by an inbound sender"
                        );
                        Ok(host_ok(crate::tasks::to_json(&task)))
                    }
                    Err(e) => Ok(host_failure(
                        Self::task_error_name(&e),
                        &format!("personalTasks.enqueueFrom: {}", e),
                    )),
                }
            },
        )?;
        host.set("enqueueFrom", enqueue_from)?;

        // Whether this sender is linked, and where to send them if not — so a
        // bot can answer an unknown sender with "authorise me here" instead of
        // failing at the enqueue.
        let script_uri_sender = script_uri.to_string();
        let sender_state = Function::new(
            ctx.clone(),
            move |options_json: String| -> JsResult<String> {
                let options: serde_json::Value = match serde_json::from_str(&options_json) {
                    Ok(options) => options,
                    Err(e) => {
                        return Ok(host_failure(
                            "TypeError",
                            &format!("personalTasks.sender: options are not valid JSON: {}", e),
                        ));
                    }
                };

                let (channel, identity) = match Self::sender_from_options(&options) {
                    Ok(pair) => pair,
                    Err(message) => return Ok(host_failure("TypeError", &message)),
                };

                let linked = match crate::database::run_blocking(
                    crate::delegation::resolve_channel(&script_uri_sender, &channel, &identity),
                ) {
                    Ok(linked) => linked,
                    Err(e) => {
                        return Ok(host_failure(
                            "Error",
                            &format!("personalTasks.sender: {}", e),
                        ));
                    }
                };

                // Deliberately not the user id. A script has no use for it —
                // everything it can do for that person goes through
                // `enqueueFrom`, which names the sender — and handing it over
                // would put an account identifier into whatever the bot logs
                // or echoes back to the chat.
                let Some(user_id) = linked else {
                    return Ok(host_ok(serde_json::json!({
                        "linked": false,
                        "granted": false,
                        "channel": channel,
                        "identity": identity,
                    })));
                };

                let grant = crate::database::run_blocking(crate::delegation::get(
                    &user_id,
                    &script_uri_sender,
                ));

                let (granted, expired, scopes) = match grant {
                    Ok(Some(grant)) => {
                        let live = grant.is_live(chrono::Utc::now());
                        (
                            live,
                            !live,
                            grant
                                .scopes
                                .iter()
                                .map(|scope| scope.as_str())
                                .collect::<Vec<_>>(),
                        )
                    }
                    _ => (false, false, Vec::new()),
                };

                Ok(host_ok(serde_json::json!({
                    "linked": true,
                    "granted": granted,
                    "expired": expired,
                    "scopes": scopes,
                    "channel": channel,
                    "identity": identity,
                })))
            },
        )?;
        host.set("sender", sender_state)?;

        // The invitation to link a sender, minted in reply to a message.
        //
        // Separate from `sender()` above, which is a read, because this
        // writes: it mints a single-use token and invalidates whatever was
        // outstanding for the same sender. Folding it into `sender()` would
        // mean a bot polling "is this person linked yet" quietly invalidated
        // the link it had already sent them.
        //
        // The URL carries a token rather than the sender, and that is the
        // security of the whole scheme. `?channel=telegram&identity=12345`
        // would be a URL anybody could construct for anybody, so linking
        // would be first come first served on a guessable string — and the
        // harm is interception rather than squatting: bind somebody else's
        // id before they do and every message they send that bot becomes
        // your turn, with their text in your storage. A token delivered into
        // the sender's own chat is the only evidence of ownership the engine
        // can have.
        let script_uri_invite = script_uri.to_string();
        let config_invite = self.config.clone();
        let user_invite = self.user_context.clone();
        let invite_link = Function::new(
            ctx.clone(),
            move |options_json: String| -> JsResult<String> {
                if config_invite.is_dry_run() {
                    return Ok(host_failure(
                        "DryRunError",
                        "personalTasks.inviteLink: nothing was minted - this is a dry run",
                    ));
                }

                // It writes a row, so it is on the write side of the surface.
                // Nothing an invitation does is dangerous on its own — it
                // binds nothing until somebody signs in and agrees — but "a
                // narrowed read-only turn writes nothing" is worth more as a
                // rule without exceptions than this is as a convenience.
                if !user_invite.has_capability(&Capability::EnqueueTasks) {
                    return Ok(host_failure(
                        "SecurityError",
                        &capability_refusal(
                            "personalTasks.inviteLink",
                            &Capability::EnqueueTasks,
                            &user_invite,
                        ),
                    ));
                }

                let options: serde_json::Value = match serde_json::from_str(&options_json) {
                    Ok(options) => options,
                    Err(e) => {
                        return Ok(host_failure(
                            "TypeError",
                            &format!(
                                "personalTasks.inviteLink: options are not valid JSON: {}",
                                e
                            ),
                        ));
                    }
                };

                let (channel, identity) = match Self::sender_from_options(&options) {
                    Ok(pair) => pair,
                    Err(message) => return Ok(host_failure("TypeError", &message)),
                };

                // Minting is inbound-triggered and writes, so it spends the
                // same budget a trigger does. Without it a stranger could
                // make the engine write a row per message.
                if let Err(message) =
                    Self::spend_channel_budget(&script_uri_invite, &channel, &identity)
                {
                    return Ok(host_failure("RangeError", &message));
                }

                match crate::database::run_blocking(crate::delegation::invite_link(
                    &script_uri_invite,
                    &channel,
                    &identity,
                )) {
                    Ok(url) => Ok(host_ok(serde_json::json!({
                        "linkUrl": url,
                        "channel": channel,
                        "identity": identity,
                        "expiresInMinutes": crate::delegation::LINK_TOKEN_MINUTES,
                    }))),
                    Err(refusal) => Ok(host_failure(
                        "Error",
                        &format!("personalTasks.inviteLink: {}", refusal),
                    )),
                }
            },
        )?;
        host.set("inviteLink", invite_link)?;

        // Whether this person has authorised this script, and for what — so a
        // script can offer the consent page instead of failing at the enqueue.
        let script_uri_grant = script_uri.to_string();
        let delegation_state = Function::new(
            ctx.clone(),
            move |ctx: rquickjs::Ctx<'_>| -> JsResult<String> {
                let Some(user_id) = Self::current_user_id(&ctx) else {
                    return Ok(host_ok(serde_json::json!({
                        "authenticated": false,
                        "granted": false,
                        "scopes": [],
                    })));
                };

                match crate::database::run_blocking(crate::delegation::get(
                    &user_id,
                    &script_uri_grant,
                )) {
                    Ok(Some(grant)) => {
                        let live = grant.is_live(chrono::Utc::now());
                        Ok(host_ok(serde_json::json!({
                            "authenticated": true,
                            "granted": live,
                            "expired": !live,
                            "expiresAt": grant.expires_at.to_rfc3339(),
                            "scopes": grant.scopes.iter().map(|s| s.as_str()).collect::<Vec<_>>(),
                            "consentUrl": crate::delegation::consent_url(&script_uri_grant),
                        })))
                    }
                    Ok(None) => Ok(host_ok(serde_json::json!({
                        "authenticated": true,
                        "granted": false,
                        "expired": false,
                        "scopes": [],
                        "consentUrl": crate::delegation::consent_url(&script_uri_grant),
                    }))),
                    Err(e) => Ok(host_failure(
                        "Error",
                        &format!("personalTasks.authorization: {}", e),
                    )),
                }
            },
        )?;
        host.set("authorization", delegation_state)?;

        global.set("__hostScriptTasks", host)?;

        crate::bytecode::eval_program(ctx, "engine://tasks-prelude", TASKS_PRELUDE).map_err(
            |e| {
                rquickjs::Error::new_from_js_message(
                    "tasks",
                    "prelude",
                    &format!("tasks prelude failed to load: {}", e),
                )
            },
        )?;

        debug!("scriptTasks initialized for script: {}", script_uri);
        Ok(())
    }
}

impl SecureGlobalContext {
    /// The lane a person's own work runs in when nobody named one.
    ///
    /// Prefixed, so it reads as what it is wherever a lane is shown — and
    /// so a script choosing its own lane names is unlikely to collide with
    /// it by accident. A script that *wants* to share this lane can spell
    /// it out; that is a reasonable thing to want and not a hazard, since
    /// the only consequence of sharing a lane is running one at a time.
    pub(super) fn person_lane(user_id: &str) -> String {
        format!("person:{}", user_id)
    }
}

impl SecureGlobalContext {
    /// The lane a caller asked for, distinguishing "omitted" from "null".
    ///
    /// `Ok(None)` is an explicit `lane: null` — the caller saying this must
    /// not be serialised — and `Ok(Some(None))` is the field being absent,
    /// which lets each surface pick its own default. `personalTasks` reads
    /// the difference: absent means "serialise per person", which is almost
    /// always what per-person work wants, and `null` is how a script opts
    /// out of that on purpose.
    #[allow(clippy::type_complexity)]
    pub(super) fn lane_from_options(
        options: &serde_json::Value,
    ) -> Result<Option<Option<String>>, String> {
        match options.get("lane") {
            None => Ok(Some(None)),
            Some(serde_json::Value::Null) => Ok(None),
            Some(serde_json::Value::String(lane)) => Ok(Some(Some(lane.clone()))),
            Some(_) => Err("lane must be a string, or null for no lane".to_string()),
        }
    }
}

impl SecureGlobalContext {
    /// The `{ channel, identity }` pair a sender is named by, validated.
    ///
    /// Shared by `enqueueFrom` and `sender` so the two cannot come to
    /// disagree about what counts as a sender — a pair one accepted and the
    /// other refused would make "is this sender linked" answer about a
    /// different sender than the one the enqueue then looked up.
    pub(super) fn sender_from_options(
        options: &serde_json::Value,
    ) -> Result<(String, String), String> {
        let channel = options
            .get("channel")
            .and_then(|value| value.as_str())
            .unwrap_or_default();
        let identity = options
            .get("identity")
            .and_then(|value| value.as_str())
            .unwrap_or_default();

        crate::delegation::normalize_channel(channel, identity)
            .map_err(|refusal| format!("the sender is not usable: {}", refusal))
    }
}

impl SecureGlobalContext {
    /// Spend one token of this binding's trigger budget.
    ///
    /// Blocking, because every caller is already on the JavaScript thread.
    pub(super) fn spend_channel_budget(
        script_uri: &str,
        channel: &str,
        identity: &str,
    ) -> Result<(), String> {
        let Some(limiter) = crate::security::rate_limiting::shared() else {
            // Startup has not built one — a unit test. Carrying on is what
            // `git_sync::spend_budget` does in the same case and for the same
            // reason: refusing work for want of a budget to check would fail
            // closed on something that is not a security decision. The
            // decisions above it — the link, the grant — have already been
            // made by then.
            return Ok(());
        };

        let key = crate::security::rate_limiting::RateLimitKey::ChannelTrigger(format!(
            "{}:{}:{}",
            script_uri, channel, identity
        ));

        let allowed =
            crate::database::run_blocking(
                async move { limiter.check_rate_limit(key, 1).await.allowed },
            );

        if allowed {
            Ok(())
        } else {
            Err("personalTasks.enqueueFrom: this sender has queued too much too quickly - the                  budget refills over the next few minutes"
                .to_string())
        }
    }
}

impl SecureGlobalContext {
    /// Which kind of exception a refusal becomes, so a script can tell a
    /// mistake in its own call from the engine being unable to answer.
    pub(super) fn task_error_name(error: &crate::tasks::EnqueueError) -> &'static str {
        use crate::tasks::EnqueueError;
        match error {
            EnqueueError::MissingHandler
            | EnqueueError::InvalidHandler
            | EnqueueError::PayloadNotAnObject => "TypeError",
            EnqueueError::PayloadTooLarge
            | EnqueueError::InvalidMaxAttempts
            | EnqueueError::InvalidRunAt => "RangeError",
            EnqueueError::Unavailable | EnqueueError::Storage(_) => "Error",
        }
    }
}
