// The JavaScript half of `sandbox` — running part of a script with fewer
// capabilities than the script holds.
//
// `__hostSandbox.run` is the Rust call. It answers with an envelope rather
// than a value, for the reason `scriptTasks` does: a host binding cannot throw
// the right kind of exception, and answering `"Error: …"` as the value is
// indistinguishable from an answer that happens to be a string.
//
// The line this draws between throwing and returning is the whole of the
// interface, and it is drawn where an agent needs it:
//
//   - the *request* was wrong (a capability that is not a capability, one this
//     execution does not hold, nesting too deep) → throws. That is a bug in
//     the script, and it should fail where it was written.
//
//   - the *code* was wrong (the model wrote something that threw, looped until
//     the budget ran out, or was refused a capability it had not been given)
//     → returns, with `error` set. That is not a failure of the agent's loop;
//     it is the result of the turn, and what the agent does with it is feed it
//     back to the model. Throwing here would make every agent wrap every call
//     in a try/catch to recover a value it always wanted.
//
// `console` output from the sub-execution comes back in `console` whether or
// not it succeeded — it is usually the most useful part of a failed turn.

(function () {
  function call(options) {
    var answer;
    try {
      answer = JSON.parse(__hostSandbox.run(JSON.stringify(options)));
    } catch (e) {
      throw new Error("sandbox.run: " + String((e && e.message) || e));
    }
    if (!answer.ok) {
      var error = new Error(answer.message);
      error.name = answer.name || "Error";
      throw error;
    }
    return answer.result;
  }

  globalThis.sandbox = {
    // Evaluate `source` in this script's sandbox holding only `capabilities`.
    //
    // The source is evaluated after the script's own program and in the same
    // realm as it, so it sees what the script defined — its helpers, and the
    // bindings its entrypoint imported. What it does *not* see is anything
    // from the calling turn: the two run in separate contexts and the only
    // things that cross are `input` going in and a JSON value coming back.
    // That is the isolation, not a limitation of it — a value passed by
    // reference would be a capability passed by reference.
    run: function (source, options) {
      options = options || {};
      return call({
        source: String(source),
        capabilities: options.capabilities || [],
        input: options.input === undefined ? null : options.input,
        timeoutMs: options.timeoutMs,
        // Off by default, unlike `/engine/eval`. A turn that may not write
        // holds no write capability, which is a stronger guarantee than a
        // transaction that undoes what it did — and a turn that *may* write
        // is usually being run because its writes are wanted.
        rollback: options.rollback === true,
      });
    },

    // Every capability name the engine has, for a script building its own
    // menu of them rather than hard-coding a list that later drifts.
    capabilities: function () {
      return JSON.parse(__hostSandbox.capabilities());
    },

    // What the calling turn itself holds. A script narrowing itself asks this
    // first: `sandbox.run(code, { capabilities: sandbox.held() })` is a
    // sub-execution that took nothing away, which is the base a plan mode
    // subtracts from.
    held: function () {
      return JSON.parse(__hostSandbox.held());
    },
  };
})();
