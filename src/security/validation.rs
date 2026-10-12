use thiserror::Error;

#[derive(Error, Debug)]
pub enum SecurityError {
    /// Named by [`Capability::as_str`] rather than by `{:?}`, which prints the
    /// Rust variant. Every other surface spells a capability the snake_case
    /// way — `sandbox.run` takes it, the JavaScript refusals report it, and it
    /// is what a caller would have to name to ask for it — so a refusal
    /// reading `WriteScripts` was the one place the engine used a name its
    /// caller could not use.
    #[error(
        "Insufficient capabilities: required {}",
        .required.iter().map(Capability::as_str).collect::<Vec<_>>().join(", ")
    )]
    InsufficientCapabilities { required: Vec<Capability> },

    #[error("CSRF token validation failed")]
    CsrfValidationFailed,
}

#[derive(Debug, Clone, Hash, Eq, PartialEq)]
pub enum Capability {
    ReadScripts,
    WriteScripts,
    DeleteScripts,
    ReadAssets,
    WriteAssets,
    DeleteAssets,
    DeleteLogs,
    ViewLogs,
    ManageStreams,
    /// Publish MCP tools, prompts and resources (`mcpRegistry.register*`).
    ///
    /// What is being decided is whether the solution exposes an MCP surface,
    /// not whether the asset behind a resource may be written.
    ManageMcp,
    /// Read rows from a script's tables.
    ///
    /// Separate from [`Capability::WriteScriptData`] so that a read-only
    /// execution is expressible: the narrowest context that can read a table
    /// cannot empty it.
    ReadScriptData,
    /// Insert, update, or delete rows in a script's tables.
    ///
    /// Every tier that reads also writes, so splitting the two changes nothing
    /// about who may do what. It exists to be taken away — see
    /// [`super::capabilities::UserContext::attenuated`].
    WriteScriptData,
    /// Change a script's schema — create or drop tables, columns, and indexes.
    /// Editing the shape of a solution's data, so it belongs to the editor
    /// tier, not to the people using the solution.
    ManageScriptDatabase,
    /// Administer the engine: act on scripts, assets, and users the caller does
    /// not own. This is the administrator marker; `DeleteScripts` is not,
    /// since deleting your own script is not administering everyone's.
    AdministerEngine,

    // The rest of this enum names what a *script* does while it runs, rather
    // than what a person may do to a solution. Each of these gated nothing
    // before attenuation existed: `fetch`, `secretStorage` and
    // `personalStorage` were reachable from any context that had them
    // installed, which for `fetch` is every context there is.
    //
    // They are held by every tier that could already do the thing, so adding
    // them took nothing away from anybody. They exist so that a sub-execution
    // can be given less — a planning turn that may read and not write, or
    // model-authored code that may compute and not call out.
    /// Make outbound HTTP requests with `fetch`.
    ///
    /// The one an agent restricts first: without it, model-authored code
    /// cannot reach an API, spend tokens, or exfiltrate what it was shown.
    UseNetwork,
    /// Read this script's secrets, and resolve `{{secret:...}}` in a `fetch`.
    ///
    /// Withholding it does not downgrade the lookup the way a missing
    /// delegation scope does — a template that cannot be resolved is an error,
    /// because silently sending a request with the literal `{{secret:...}}` in
    /// a header is worse than not sending it.
    ReadSecrets,
    /// Store or remove this script's secrets.
    WriteSecrets,
    /// Read a person's `personalStorage`, or the script's own `scriptStorage`.
    ReadStorage,
    /// Write or clear the person's `personalStorage`.
    WriteStorage,
    /// Write or clear the script's own `scriptStorage`.
    ///
    /// Apart from [`Capability::WriteStorage`] because the two stores belong
    /// to different people: `personalStorage` is the caller's, `scriptStorage`
    /// is state every caller of the script reads. A narrowing that lets code
    /// keep a note for the person it runs for must not let that code change
    /// what every other person's request reads.
    WriteScriptStorage,
    /// Enqueue and cancel durable tasks (`scriptTasks`, `personalTasks`).
    ///
    /// Queueing work is how an execution outlives itself, so a context that
    /// may not write must not be able to queue a write for later.
    EnqueueTasks,
    /// Call another script's MCP tools as this caller (`tools.call`).
    ///
    /// The in-process counterpart of `tools/call` on `/mcp`, which an
    /// authenticated caller can already make. It is named so that it can be
    /// withheld: the engine's own executions never hold it, because a tool
    /// reached from a scheduled job would run as nobody, and model-authored
    /// code holds it only when given it by name.
    CallTools,
}

#[cfg(test)]
mod tests {

    /// The refusal names what a caller would have to ask for.
    ///
    /// `{:?}` printed `WriteScripts`, which is the Rust variant and is not a
    /// name any surface of the engine accepts. Every other one — `sandbox.run`,
    /// the JavaScript refusals, the capability names on the wire — spells it
    /// `write_scripts`.
    #[test]
    fn an_insufficient_capability_is_named_the_way_it_is_asked_for() {
        let one = super::SecurityError::InsufficientCapabilities {
            required: vec![super::Capability::WriteScripts],
        };
        let several = super::SecurityError::InsufficientCapabilities {
            required: vec![
                super::Capability::WriteScripts,
                super::Capability::AdministerEngine,
            ],
        };

        assert_eq!(
            one.to_string(),
            "Insufficient capabilities: required write_scripts"
        );
        assert_eq!(
            several.to_string(),
            "Insufficient capabilities: required write_scripts, administer_engine"
        );
    }
}
