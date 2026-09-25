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

  // `personalTasks` — the same queue, acting as the person who asked.
  //
  // Separate from `scriptTasks` rather than an option on it, because the two
  // differ in what the work is allowed to touch, and a flag would make that
  // the kind of thing you set without noticing. Enqueueing here needs the
  // person to have authorised this script; `authorization()` says whether they
  // have, and carries the page to send them to if they have not.
  //
  // One thing differs from `scriptTasks` besides the authority: a personal
  // task is serialised per person by default. Two prompts from one person
  // otherwise become two runs interleaving turn for turn over the same
  // storage, which is a bug in essentially every solution that queues
  // per-person work. Pass `lane: null` to opt out, or a lane of your own for
  // a finer one.
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

    // The same queue, acting as the person a message came *from* — which is
    // the only way an execution with nobody signed in can act as anybody.
    //
    // A script names the sender, never a person: the engine resolves
    // {channel, identity} through the links people have consented to. So the
    // worst a script that trusts the wrong field in a request body can do is
    // claim the wrong sender, and an unlinked sender resolves to nobody.
    //
    // Verifying the message really came from that sender is the script's
    // job. Telegram and Slack sign their webhooks; check the signature
    // before calling this, because the engine cannot.
    enqueueFrom: function (options) {
      if (options === null || typeof options !== "object") {
        throw new TypeError(
          "personalTasks.enqueueFrom requires an options object",
        );
      }
      return unwrap(host.enqueueFrom(JSON.stringify(options)));
    },

    // Whether a sender is linked and still authorised:
    // {linked, granted, expired, scopes, channel, identity}. A read.
    //
    // Never the account's id — everything a script can do for that person
    // goes through `enqueueFrom`, which names the sender, and an id here
    // would end up in whatever the bot logs or echoes back to the chat.
    sender: function (options) {
      if (options === null || typeof options !== "object") {
        throw new TypeError("personalTasks.sender requires an options object");
      }
      return unwrap(host.sender(JSON.stringify(options)));
    },

    // Mint the one URL that links this sender, to reply with into that
    // sender's own chat: {linkUrl, channel, identity, expiresInMinutes}.
    //
    // Reply with it *there* and nowhere else. The URL carries a token rather
    // than the sender's name, which is what stops somebody linking an id
    // that is not theirs — and linking somebody else's id before they do
    // would intercept their messages, not merely squat on them. Posting the
    // link anywhere the sender cannot read gives that away.
    //
    // Minting invalidates any link outstanding for the same sender, so this
    // belongs where a bot decides to send one, not in a polling loop.
    inviteLink: function (options) {
      if (options === null || typeof options !== "object") {
        throw new TypeError(
          "personalTasks.inviteLink requires an options object",
        );
      }
      return unwrap(host.inviteLink(JSON.stringify(options)));
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
