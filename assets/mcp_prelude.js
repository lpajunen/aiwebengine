// The JavaScript half of `mcp` — a tool asking the person a question.
//
// The shape to hold in mind is that `ask` is not a pause. A tool call that
// asks *ends*: the engine answers the client with `input_required`, the client
// asks the person, and then calls the tool again. On that second call the
// handler runs from the top once more, and `ask` returns the answer instead of
// ending anything.
//
// So the code reads straight-line while the execution model underneath is
// re-entry. What that costs is that everything before the `ask` runs on every
// pass — put reads and validation before it, writes after it, and wrap
// anything that must happen exactly once in `mcp.once`.

(function () {
  function hostCall(name, fn) {
    try {
      return fn();
    } catch (e) {
      throw new Error("mcp." + name + ": " + String((e && e.message) || e));
    }
  }

  // Thrown to end the turn once a question has been recorded. The engine
  // decides the outcome from what was recorded, not from this exception, so a
  // script that catches it still ends its turn asking — catching cannot smuggle
  // execution past an unanswered question.
  function AskedError(message) {
    var error = new Error(message);
    error.name = "McpInputRequired";
    return error;
  }

  // The same trick as AskedError, for the other way a call ends without an
  // answer. The *outcome* is decided by what the host recorded, not by this
  // exception, so a handler that catches it still ends its call having handed
  // off — the work is already queued and a throw cannot unqueue it.
  function HandedError(message) {
    var error = new Error(message);
    error.name = "McpTaskHandedOff";
    return error;
  }

  var mcp = {
    // Whether this caller can be asked at all.
    //
    // False when the client declared no elicitation capability — a server must
    // not send a request the client never said it could handle — and false
    // wherever there is no client: a scheduled job, a delegated task, a
    // listener. Background work runs for somebody who is not there.
    //
    // A tool that can manage without the answer should check this and fall
    // back. A tool that cannot should just ask and let the throw stand.
    canAsk: function () {
      return hostCall("canAsk", function () {
        return __hostMcp.canAsk();
      });
    },

    // Ask for structured input, and either return what was given or end the
    // turn asking for it.
    //
    //   const answer = mcp.ask({
    //     message: "Which branch?",
    //     schema: { type: "object", properties: { branch: { type: "string" } } },
    //   });
    //
    // The answer is `{ action, content }`. Three actions, and the difference
    // matters: `accept` carries `content`, `decline` is a decision, `cancel` is
    // a dismissal. Treating anything but `accept` as a failure reports an error
    // for somebody who simply closed a dialog.
    //
    // `key` names the question across passes. It defaults to the call's
    // position, which is stable as long as the code before the ask is; name it
    // explicitly if a handler asks different questions down different branches.
    ask: function (options) {
      options = options || {};
      if (typeof options.message !== "string" || options.message.length === 0) {
        throw new Error(
          "mcp.ask: a message is required, saying why you need this",
        );
      }

      var key = options.key;
      if (key === undefined || key === null) {
        key = "ask_" + __hostMcp.askedSoFar();
      }
      key = String(key);

      var existing = hostCall("ask", function () {
        return __hostMcp.answer(key);
      });
      if (existing !== null && existing !== undefined) {
        return JSON.parse(existing);
      }

      if (!mcp.canAsk()) {
        throw new Error(
          "mcp.ask: this caller cannot be asked — no elicitation capability, " +
            "or no client at all. Guard with mcp.canAsk().",
        );
      }

      // Form mode only. The specification forbids asking for passwords, API
      // keys, tokens or payment details this way; those need URL mode, which
      // the engine does not implement yet.
      var request = {
        method: "elicitation/create",
        params: {
          mode: "form",
          message: options.message,
          requestedSchema: options.schema || { type: "object", properties: {} },
        },
      };

      hostCall("ask", function () {
        __hostMcp.ask(key, JSON.stringify(request));
      });
      throw AskedError("mcp.ask: waiting on " + key);
    },

    // Run something exactly once across the whole exchange, however many times
    // the handler re-runs.
    //
    //   const id = mcp.once("draft", () => createDraft(repo));
    //
    // The result travels in the request state to the client and back on every
    // round trip, so it must be JSON and it must be small: this is for a
    // decision or an identifier, not for a document.
    once: function (key, fn) {
      if (typeof key !== "string" || key.length === 0) {
        throw new Error("mcp.once: a key is required");
      }
      if (typeof fn !== "function") {
        throw new Error("mcp.once: a function is required");
      }

      var remembered = hostCall("once", function () {
        return __hostMcp.memoGet(key);
      });
      if (remembered !== null && remembered !== undefined) {
        return JSON.parse(remembered).value;
      }

      var value = fn();
      hostCall("once", function () {
        __hostMcp.memoSet(
          key,
          JSON.stringify({ value: value === undefined ? null : value }),
        );
      });
      return value;
    },

    // Whether this call may hand its work to a task.
    //
    // Two conditions: there is a request to answer at all (false in a
    // scheduled job or a listener), and the client declared the tasks
    // extension. A client that did not declare it has to be answered
    // synchronously, because a handle it cannot read is a tool call answered
    // with an object full of fields it never asked for.
    canTask: function () {
      return hostCall("canTask", function () {
        return __hostMcp.canTask();
      });
    },

    // Hand this call's work to the durable queue and answer the client with a
    // task handle it can poll.
    //
    //   function buildReport(context) {
    //     if (mcp.canTask()) {
    //       mcp.task({ handler: "runReport", payload: context.request.arguments });
    //     }
    //     return runReportInline(context.request.arguments);
    //   }
    //
    // Like `mcp.ask`, this does not return: the work is queued, the handle is
    // recorded, and the call ends. So the line after it is the synchronous
    // fallback, which is why `canTask()` is worth asking first — a client that
    // cannot be handed a task still needs an answer.
    //
    // `handler` names a top-level function in this script, called later with
    // `context.meta.task.payload`. **What it returns is the task's result**,
    // and is what `tasks/get` answers with once the status is `completed`; it
    // has to be JSON. A handler that throws puts the task in `failed`.
    //
    // The work runs in *script* context, not as the caller. Running it as them
    // would need a delegation grant, and an MCP client holding a token is not
    // the same as a person having consented to background work in their name.
    task: function (options) {
      options = options || {};
      if (typeof options.handler !== "string" || options.handler.length === 0) {
        throw new Error("mcp.task: a handler name is required");
      }

      var created = hostCall("task", function () {
        return __hostMcp.handOff(
          JSON.stringify({
            handler: options.handler,
            payload: options.payload === undefined ? {} : options.payload,
            statusMessage: options.statusMessage,
            lane: options.lane,
            method: options.method,
            target: options.target,
          }),
        );
      });

      throw HandedError(
        "mcp.task: handed off as " + JSON.parse(created).taskId,
      );
    },
  };

  globalThis.mcp = mcp;
})();
