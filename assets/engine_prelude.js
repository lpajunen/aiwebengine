// The JavaScript half of `engine` — the engine's own management tools, called
// from a script instead of over HTTP.
//
// `engine.call` answers with the tool's own JSON. A tool that refuses answers
// `{ error: "..." }`, which is what `/mcp` hands its clients, and this turns
// that into a thrown `Error` so a script does not have to check a field it
// might forget — the `tasks_prelude.js` rule. `engine.callRaw` is the way back
// to the envelope for a caller that would rather branch than catch.

(function () {
  var host = __hostEngine;

  function parse(where, answer) {
    try {
      return JSON.parse(answer);
    } catch (e) {
      throw new Error(
        where + ": the engine answered with something that is not JSON",
      );
    }
  }

  var api = {
    tools: function (area) {
      return parse(
        "engine.tools",
        host.tools(area === undefined || area === null ? "" : String(area)),
      );
    },

    callRaw: function (name, args) {
      if (typeof name !== "string" || name === "") {
        throw new Error("engine.call: a tool must be named");
      }
      return parse(
        "engine.call",
        host.call(name, JSON.stringify(args === undefined ? {} : args)),
      );
    },

    call: function (name, args) {
      var answer = api.callRaw(name, args);
      // Every native tool reports a refusal the same way, so this is one
      // check rather than one per tool.
      if (
        answer &&
        typeof answer === "object" &&
        typeof answer.error === "string"
      ) {
        var error = new Error(answer.error);
        error.name = "EngineError";
        error.tool = name;
        throw error;
      }
      return answer;
    },
  };

  Object.defineProperty(globalThis, "engine", {
    value: Object.freeze(api),
    writable: false,
    enumerable: true,
    configurable: false,
  });
})();
