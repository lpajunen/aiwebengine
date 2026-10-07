// The JavaScript half of `routeRegistry`.
//
// `__hostRouteRegistry` is the Rust call. Each of its methods answers with an
// envelope — `{ok: value}` or `{error: {name, message}}` — and this turns that
// into the interface a script uses: values out, real exceptions on failure.
//
// One registration call. What a path leads to is the spec's business:
//
//   routeRegistry.registerRoute("/things/:id", { handler: "getThing", method: "GET" });
//   routeRegistry.registerRoute("/things/:id/events", { stream: true, authorize: "mayWatch" });
//   routeRegistry.registerRoute("/thing.css", { file: "public/thing.css" });
//
// A mistake in the call throws. A refusal — a file outside `public/`, a call
// made outside `init()` — is the value `{ ok: false, reason }`, so a script
// that gets one path wrong keeps its other registrations.

(function () {
  function unwrap(raw, api) {
    var envelope;
    try {
      envelope = JSON.parse(raw);
    } catch (e) {
      throw new Error(api + ": the engine returned something unreadable");
    }

    if (envelope && envelope.error) {
      var error = new Error(envelope.error.message || api + " failed");
      error.name = envelope.error.name || "Error";
      throw error;
    }

    return envelope ? envelope.ok : null;
  }

  function isPlainObject(value) {
    return value !== null && typeof value === "object" && !Array.isArray(value);
  }

  var host = globalThis.__hostRouteRegistry;

  var routeRegistry = {
    // registerRoute(path, spec) -> { ok: true } | { ok: false, reason }
    registerRoute: function (path, spec) {
      var api = "routeRegistry.registerRoute";
      if (typeof path !== "string") {
        throw new TypeError(api + ": path must be a string");
      }
      if (typeof spec === "string") {
        // A positional handler name: answer with the call's correct shape.
        throw new TypeError(
          api +
            '(path, handlerName, method) is not a valid call; use registerRoute(path, { handler: "' +
            spec +
            '", method })',
        );
      }
      if (!isPlainObject(spec)) {
        throw new TypeError(
          api +
            ": the spec is an object: { handler }, { stream: true } or { file }",
        );
      }
      return unwrap(host.register(path, JSON.stringify(spec)), api);
    },

    // sendStreamMessage(path, data) -> { delivered, connections, failed }
    sendStreamMessage: function (path, data) {
      return unwrap(
        host.send(String(path), data),
        "routeRegistry.sendStreamMessage",
      );
    },

    // sendStreamMessageFiltered(path, data, filter, matchMode)
    //   -> { delivered, connections, failed }
    // `filter` is an object of string values, matched against what each
    // connection's authorize function returned.
    sendStreamMessageFiltered: function (path, data, filter, matchMode) {
      var api = "routeRegistry.sendStreamMessageFiltered";
      if (filter !== undefined && filter !== null && !isPlainObject(filter)) {
        throw new TypeError(api + ": the filter is an object of string values");
      }
      return unwrap(
        host.sendFiltered(
          String(path),
          data,
          filter === undefined || filter === null
            ? undefined
            : JSON.stringify(filter),
          matchMode === undefined || matchMode === null
            ? undefined
            : String(matchMode),
        ),
        api,
      );
    },
  };

  Object.defineProperty(globalThis, "routeRegistry", {
    value: Object.freeze(routeRegistry),
    writable: false,
    enumerable: true,
    configurable: false,
  });

  // The host object is an implementation detail; a script that reaches for it
  // is reaching around the interface above.
  try {
    delete globalThis.__hostRouteRegistry;
  } catch (e) {
    /* non-configurable in some contexts; the interface above is still what is documented */
  }
})();
