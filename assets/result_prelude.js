// Gives the host APIs that answer with a JSON string the same shape `fetch()`
// hands back, so reading a result does not depend on which API produced it:
//
//   const rows = await database.query("notes");
//   rows.json()                        — the parsed value
//   JSON.parse(database.query("notes")) — still the string it always was
//
// The `await` is sequencing sugar. Host calls block rather than yielding, so
// the answer is already in hand by the time the call returns.

(function () {
  function makeResult(envelope, awaitable) {
    // A String *object*, not a plain one: it carries every string method, so
    // the code written against what these calls used to return keeps working
    // to the letter — `.length`, `.slice()`, `JSON.parse()`, concatenation.
    // The one thing that changes is `typeof`, which is now "object".
    var result = new String(envelope);

    // Parsed on demand rather than up front: a host call that answers with
    // something other than JSON should fail where the script asks for the
    // parse, not on the way back from a call that worked.
    result.json = function () {
      return JSON.parse(envelope);
    };

    result.text = function () {
      return envelope;
    };

    // An own `toString` closing over the envelope, rather than leaning on
    // `String.prototype.toString`, which reads its value off `this` and so
    // depends on every caller binding a receiver.
    result.toString = function () {
      return envelope;
    };

    if (awaitable) {
      // Resolve to a twin *without* `then`. Resolving to `result` itself would
      // hand the promise machinery another thenable, which it would unwrap
      // again, forever: the promise never settles and the invocation dies at
      // its execution deadline with nothing to point at the cause.
      result.then = function (onFulfilled) {
        onFulfilled(makeResult(envelope, false));
      };
    }

    return result;
  }

  // Wraps every method of a host namespace, leaving the Rust side untouched.
  function wrapNamespace(host) {
    var wrapped = {};
    var names = Object.keys(host);
    for (var i = 0; i < names.length; i++) {
      wrapped[names[i]] = (function (method) {
        return function () {
          var answer = method.apply(host, arguments);
          // Only a string is an envelope. Anything else is passed straight
          // through rather than wrapped in a shape it does not have.
          return typeof answer === "string" ? makeResult(answer, true) : answer;
        };
      })(host[names[i]]);
    }
    return wrapped;
  }

  globalThis.database = wrapNamespace(__hostDatabase);
})();
