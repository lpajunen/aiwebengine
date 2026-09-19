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

  // `dispatcher.post` is the queue's face on the dispatcher, so it is built
  // here rather than in `dispatcher`'s own setup: it needs the same unwrapping,
  // and the thing it enqueues onto is this module.
  //
  // `sendMessage` runs every listener inline, on the caller's budget. `post`
  // queues one task per listener instead, so the caller returns at once and
  // each listener gets a budget, a retry and a visible state of its own. The
  // listener itself is the same registration and the same code either way.
  var dispatcher = globalThis.dispatcher;
  if (dispatcher && typeof dispatcher.__post === "function") {
    var hostPost = dispatcher.__post;
    dispatcher.post = function (messageType, messageData) {
      return unwrap(
        hostPost(
          String(messageType),
          messageData === undefined ? undefined : JSON.stringify(messageData),
        ),
      );
    };
    try {
      delete dispatcher.__post;
    } catch (e) {
      /* the documented interface is `post` either way */
    }
  }

  // `personalTasks` — the same queue, acting as the person who asked.
  //
  // Separate from `scriptTasks` rather than an option on it, because the two
  // differ in what the work is allowed to touch, and a flag would make that
  // the kind of thing you set without noticing. Enqueueing here needs the
  // person to have authorised this script; `authorization()` says whether they
  // have, and carries the page to send them to if they have not.
  var personalTasks = {
    enqueue: function (options) {
      if (options === null || typeof options !== "object") {
        throw new TypeError("personalTasks.enqueue requires an options object");
      }
      return unwrap(host.enqueuePersonal(JSON.stringify(options)));
    },

    // What this person has authorised this script to do:
    // {authenticated, granted, expired, expiresAt, scopes, consentUrl}
    authorization: function () {
      return unwrap(host.authorization());
    },

    cancel: function (taskId) {
      return unwrap(host.cancel(String(taskId)));
    },

    get: function (taskId) {
      return unwrap(host.get(String(taskId)));
    },
  };

  Object.defineProperty(globalThis, "personalTasks", {
    value: Object.freeze(personalTasks),
    writable: false,
    enumerable: true,
    configurable: false,
  });

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
