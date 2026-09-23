// The JavaScript half of `fetch()`.
//
// `__hostFetch` is the Rust call: it performs the request, blocks until it has
// an answer, and hands back the response as a JSON envelope. This wraps that
// envelope in an object that can be used three ways, so browser habits work
// without breaking the scripts written against the string it used to return:
//
//   await fetch(url)          — `then` makes it awaitable
//   fetch(url).status         — the fields are really there
//   JSON.parse(fetch(url))    — `toString` yields the original envelope
//
// The `await` is sequencing sugar, not concurrency. The request has already
// finished by the time `fetch` returns, so `Promise.all` over several fetches
// gives the right answers and runs them one after another.

(function () {
  function makeResponse(envelope, awaitable) {
    const data = JSON.parse(envelope);

    const response = {
      status: data.status,
      ok: data.ok,
      headers: data.headers,
      body: data.body,

      // Present only when the call asked for `{ binary: true }`. The two are
      // never both populated: one of them carries the answer, so a caller
      // never has to decide which to believe.
      bodyBase64: data.bodyBase64,

      text: function () {
        return data.body;
      },

      json: function () {
        return JSON.parse(data.body);
      },

      // What `fetch()` used to return. `JSON.parse` converts its argument with
      // ToString first, so `JSON.parse(fetch(url))` still means what it did.
      toString: function () {
        return envelope;
      },
    };

    if (awaitable) {
      // Resolve to a twin *without* `then`. Resolving to `response` itself
      // would hand the promise machinery another thenable, which it would
      // unwrap again, forever: the promise never settles and the request dies
      // at its execution deadline with nothing to point at the cause.
      response.then = function (onFulfilled) {
        onFulfilled(makeResponse(envelope, false));
      };
    }

    return response;
  }

  // `__hostFetch` parses its second argument as JSON, so the object every
  // caller writes — and every example in the type declarations shows — has to
  // be serialized here. Handing it over as it stands threw
  // `TypeError: Error converting from js 'object' into type 'string'` out of
  // the host binding, because the binding takes a string and QuickJS does not
  // coerce one for it. A string is passed through unchanged, since that is
  // what callers written against the host call already send.
  function encodeOptions(options) {
    if (options === undefined || options === null) {
      return undefined;
    }
    if (typeof options === "string") {
      return options;
    }
    return JSON.stringify(options);
  }

  globalThis.fetch = function (url, options) {
    return makeResponse(__hostFetch(url, encodeOptions(options)), true);
  };

  // Several requests at once.
  //
  // `Promise.all([fetch(a), fetch(b)])` gives the right answers and runs them
  // one after another, because each `fetch` has already finished by the time
  // it returns. That is fine for two quick calls and is the difference
  // between fitting inside the execution budget and not for an agent running
  // three tool calls — the wall clock is the sum rather than the slowest.
  //
  // These run together. One thread waits on the batch instead of one per
  // request in series, and each request gets the same validation, redirects
  // and secret substitution a single `fetch` gets.
  //
  // A failure is per request: a refused URL throws from *that* response
  // rather than from the call, so the answers that arrived are still usable.
  // Read `ok` first, or let the throw happen where you touch the one that
  // failed.
  globalThis.fetchAll = function (requests) {
    if (!Array.isArray(requests)) {
      throw new TypeError("fetchAll requires an array of requests");
    }

    var described = requests.map(function (request) {
      // A bare URL is the common case and worth not making people spell out.
      if (typeof request === "string") {
        return { url: request, options: {} };
      }
      if (request === null || typeof request !== "object" || !request.url) {
        throw new TypeError(
          "each fetchAll request must be a URL or an object with a url",
        );
      }
      return { url: String(request.url), options: request.options || {} };
    });

    var answers = JSON.parse(__hostFetchAll(JSON.stringify(described)));

    return answers.map(function (answer) {
      if (answer.ok) {
        return makeResponse(JSON.stringify(answer.response), false);
      }
      // Deferred rather than thrown here: the batch succeeded in the sense
      // the caller asked about, and only this slot did not.
      var failure = {
        ok: false,
        error: answer.error,
        get status() {
          throw new Error(answer.error);
        },
        text: function () {
          throw new Error(answer.error);
        },
        json: function () {
          throw new Error(answer.error);
        },
        toString: function () {
          return answer.error;
        },
      };
      return failure;
    });
  };

  // A response read a piece at a time.
  //
  // `fetch` reads the whole body before it returns anything, so a script
  // cannot consume a model's token stream: the page updates once per turn and
  // a turn is as long as the whole call. This hands back the status and
  // headers as soon as they arrive, and the body in pieces as they do.
  //
  // It is iterable, so the ordinary shape is a `for...of`:
  //
  //   const stream = fetchStream(url, { method: "POST", body });
  //   for (const chunk of stream) {
  //     routeRegistry.sendStreamMessage("progress", chunk);
  //   }
  //
  // The connection stays open between reads and is closed when the stream
  // ends, when `close()` is called, or when the execution does. A stream
  // nobody finishes reading holds a socket until then, so close one you are
  // done with early.
  //
  // Chunks fall where the network put them, not on any boundary that means
  // anything: a chunk is not a line and not an SSE event. Reassembling those
  // is the caller's job, and the reason is that only the caller knows which
  // it is reading.
  globalThis.fetchStream = function (url, options) {
    var opening = JSON.parse(
      __hostFetchStreamStart(url, encodeOptions(options)),
    );
    var id = opening.streamId;
    var done = false;

    function read() {
      if (done) {
        return { done: true };
      }
      var piece = JSON.parse(__hostFetchStreamRead(id));
      if (piece.done) {
        done = true;
        return { done: true };
      }
      return { done: false, value: piece.value };
    }

    var stream = {
      status: opening.status,
      ok: opening.ok,
      headers: opening.headers,

      // `{ done, value }`, the shape a reader answers with everywhere else.
      read: read,

      close: function () {
        done = true;
        return __hostFetchStreamClose(id);
      },

      // Everything that is left, joined. For a caller that wanted the
      // headers early and the body whole.
      text: function () {
        var parts = [];
        for (;;) {
          var piece = read();
          if (piece.done) {
            break;
          }
          parts.push(piece.value);
        }
        return parts.join("");
      },
    };

    stream[Symbol.iterator] = function () {
      return { next: read };
    };

    return stream;
  };
})();
