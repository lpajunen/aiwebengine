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

  globalThis.fetch = function (url, options) {
    return makeResponse(__hostFetch(url, options), true);
  };
})();
