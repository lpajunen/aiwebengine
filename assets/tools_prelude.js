// The JavaScript half of `tools` — the MCP tools other scripts publish,
// called from a script as the person it is running for.
//
// `__hostTools` answers with an envelope, `{ ok }` or `{ error: { name,
// message } }`, and this turns the second into a thrown `Error` of that name:
// a host binding cannot throw the right kind of exception itself.

(function () {
  var host = __hostTools;

  function unwrap(where, answer) {
    var parsed;
    try {
      parsed = JSON.parse(answer);
    } catch (e) {
      throw new Error(
        where + ": the engine answered with something that is not JSON",
      );
    }
    if (parsed && parsed.error) {
      var error = new Error(parsed.error.message);
      error.name = parsed.error.name || "Error";
      throw error;
    }
    return parsed.ok;
  }

  var api = {
    // Every tool this script may call, optionally narrowed to those whose
    // name, description or script mentions `filter`.
    list: function (filter) {
      return unwrap(
        "tools.list",
        host.list(
          filter === undefined || filter === null ? "" : String(filter),
        ),
      );
    },

    // Call one by name. `readOnly: true` runs it holding no write
    // capability, so a tool that tries to change something is refused.
    call: function (name, args, options) {
      if (typeof name !== "string" || name === "") {
        throw new TypeError("tools.call: a tool must be named");
      }
      var readOnly = !!(options && options.readOnly === true);
      return unwrap(
        "tools.call",
        host.call(
          name,
          JSON.stringify(args === undefined || args === null ? {} : args),
          readOnly,
        ),
      );
    },
  };

  Object.defineProperty(globalThis, "tools", {
    value: Object.freeze(api),
    writable: false,
    enumerable: true,
    configurable: false,
  });
})();
