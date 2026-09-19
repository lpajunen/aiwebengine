// The JavaScript half of `scriptTasks`.
//
// `__hostScriptTasks` is the Rust call. Each of its methods answers with an
// envelope — `{ok: value}` or `{error: {name, message}}` — and this turns that
// into the interface a script uses: real objects out, real exceptions on
// failure.
//
// The envelope exists because a host binding cannot throw a JavaScript
// exception of the right type, and because the alternative the engine used to
// use — returning `"Error: ..."` as the value — is indistinguishable from a
// successful answer that happens to be a string. `scriptStorage` learned this
// first; see `storage_prelude.js`.

(function () {
  function unwrap(raw) {
    var envelope;
    try {
      envelope = JSON.parse(raw);
    } catch (e) {
      throw new Error("scriptTasks: the engine returned something unreadable");
    }

    if (envelope && envelope.error) {
      var error = new Error(envelope.error.message || "scriptTasks failed");
      error.name = envelope.error.name || "Error";
      throw error;
    }

    return envelope ? envelope.ok : null;
  }

  var host = globalThis.__hostScriptTasks;

  var scriptTasks = {
    // enqueue({handler, payload, runAt, maxAttempts}) -> task
    enqueue: function (options) {
      if (options === null || typeof options !== "object") {
        throw new TypeError("scriptTasks.enqueue requires an options object");
      }
      return unwrap(host.enqueue(JSON.stringify(options)));
    },

    // cancel(taskId) -> boolean. False when the task was not pending: it has
    // already run, already failed, or is running right now.
    cancel: function (taskId) {
      return unwrap(host.cancel(String(taskId)));
    },

    // get(taskId) -> task or null. Null also means "finished successfully",
    // because a task that succeeds does not keep a row; what it did is in the
    // script's log.
    get: function (taskId) {
      return unwrap(host.get(String(taskId)));
    },
  };

  Object.defineProperty(globalThis, "scriptTasks", {
    value: Object.freeze(scriptTasks),
    writable: false,
    enumerable: true,
    configurable: false,
  });

  // The host object is an implementation detail; a script that reaches for it
  // is reaching around the interface above.
  try {
    delete globalThis.__hostScriptTasks;
  } catch (e) {
    /* non-configurable in some contexts; the interface above is still what is documented */
  }
})();
