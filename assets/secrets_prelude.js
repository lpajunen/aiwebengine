// The JavaScript half of `secretStorage`.
//
// `__hostSecrets` is the Rust call, answering in the envelope every preluded
// global uses — `{ok: value}` or `{error: {name, message}}` — and this turns
// that into values and thrown errors. Nothing here reads a secret back: a
// stored secret reaches the API it is for through `fetch`, which replaces a
// `{{secret:NAME}}` in a header with it.

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

  function key(value, api) {
    if (typeof value !== "string" || value === "") {
      throw new TypeError(api + ": the key must be a non-empty string");
    }
    return value;
  }

  var host = globalThis.__hostSecrets;

  var secretStorage = {
    // exists(key) -> boolean: the signed-in person's secret, else the script's.
    exists: function (name) {
      var api = "secretStorage.exists";
      return unwrap(host.exists(key(name, api)), api);
    },

    // setSecret(key, value) -> undefined. Stored for the signed-in person.
    setSecret: function (name, value) {
      var api = "secretStorage.setSecret";
      if (typeof value !== "string") {
        throw new TypeError(api + ": the value must be a string");
      }
      unwrap(host.setSecret(key(name, api), value), api);
    },

    // removeSecret(key) -> boolean: false when there was nothing to remove.
    removeSecret: function (name) {
      var api = "secretStorage.removeSecret";
      return unwrap(host.removeSecret(key(name, api)), api);
    },

    // clear() -> undefined. Every secret the signed-in person stored here.
    clear: function () {
      unwrap(host.clear(), "secretStorage.clear");
    },
  };

  Object.defineProperty(globalThis, "secretStorage", {
    value: Object.freeze(secretStorage),
    writable: false,
    enumerable: true,
    configurable: false,
  });

  try {
    delete globalThis.__hostSecrets;
  } catch (e) {
    /* non-configurable in some contexts; the interface above is still what is documented */
  }
})();
