// The JavaScript half of `scriptStorage` and `personalStorage`.
//
// `__hostScriptStorage` and `__hostPersonalStorage` are the Rust calls. They
// answer with values rather than with prose about values — `null` for a key
// that is not there, nothing for a write that worked, and the envelope of an
// exception for one that did not. What is missing from them is the interface
// itself, which is what this builds: the WHATWG `Storage` that every browser
// exposes as `localStorage` and `sessionStorage`.
//
// What that buys, beyond familiarity:
//
//   - a failed write throws instead of returning a string. `setItem` used to
//     answer `"Error: …"` while the type declaration said `void`, so a quota
//     overflow, a database error, or writing to personal storage with nobody
//     logged in were all invisible to a script that believed its own types.
//   - keys and values are coerced with `String()`, as the spec requires, so
//     `setItem("count", 1)` stores `"1"` rather than throwing a TypeError out
//     of the host binding.
//   - `length`, `key(i)` and named access (`store.foo`, `"foo" in store`,
//     `delete store.foo`, `Object.keys(store)`) all work.
//
// Two things worth knowing about the proxy that makes named access work.
//
// It is a database round trip per trap: `store.foo` costs what
// `store.getItem("foo")` costs, and enumerating the store costs one query for
// the keys plus one per key. That is the same bargain the explicit calls
// offer, but a browser's `Storage` is in memory and this one is not, so a loop
// over a large store is not free the way the habit suggests.
//
// And a property the interface itself defines is never treated as a stored
// key. That covers the inherited ones as much as the obvious ones: without it
// `store.toString` would read a key rather than find a method, and a template
// literal — which reaches for `toString` and then `valueOf` — would fail to
// convert the store at all. A browser resolves these on `Storage.prototype`
// for the same reason.

(function () {
  function fail(envelope) {
    var described;
    try {
      described = JSON.parse(envelope);
    } catch (e) {
      described = { name: "UnknownError", message: String(envelope) };
    }
    // `DOMException` is what a browser raises here: `QuotaExceededError` when
    // the write does not fit, `SecurityError` when the store is not available
    // to the caller. It is already in this runtime and is a real `Error`, so a
    // script can catch it either way.
    throw new DOMException(described.message, described.name);
  }

  // The host answers `null`/`undefined` when a write worked and an envelope
  // when it did not, so anything at all coming back is a failure.
  function check(answer) {
    if (answer !== null && answer !== undefined) {
      fail(answer);
    }
  }

  function build(host, name) {
    // Reaching a store that has no user behind it is not the same as finding
    // it empty, and answering `null` or `0` would say the second while meaning
    // the first. Where a script asks on purpose, the store says so.
    //
    // Where it is *not* asking on purpose, it stays quiet, and the line
    // between the two is what a JavaScript value has to tolerate to be
    // ordinary. Reading a property is how generic code probes an object —
    // `JSON.stringify` looks for `toJSON`, `await` looks for `then`, any
    // feature test looks for the thing it wants — and a property read that
    // throws makes the store unusable by anything that did not already know
    // what it was holding. So:
    //
    //   getItem, setItem, removeItem, clear, key, length   throw
    //   store.foo, "foo" in store                          quiet
    //   store.foo = x, delete store.foo                    throw
    //   Object.keys(store), JSON.stringify(store)          quiet
    //
    // Reads through the interface are deliberate and say why they failed;
    // reads through a property are how the language pokes at any object, and
    // answer as if the store were empty. Nothing probes an object by writing
    // to it, so every write throws whichever way it was spelled.
    function requireAvailable() {
      if (!host.available()) {
        throw new DOMException(
          name + " requires an authenticated user",
          "SecurityError",
        );
      }
    }

    function arity(method, required, got) {
      if (got < required) {
        throw new TypeError(
          name +
            "." +
            method +
            " requires " +
            required +
            (required === 1
              ? " argument, but only "
              : " arguments, but only ") +
            got +
            " were passed",
        );
      }
    }

    var storage = {
      getItem: function (key) {
        arity("getItem", 1, arguments.length);
        requireAvailable();
        var value = host.getItem(String(key));
        // The spec is explicit that a missing key is `null`, never `undefined`.
        return value === undefined ? null : value;
      },

      setItem: function (key, value) {
        arity("setItem", 2, arguments.length);
        requireAvailable();
        check(host.setItem(String(key), String(value)));
      },

      removeItem: function (key) {
        arity("removeItem", 1, arguments.length);
        requireAvailable();
        check(host.removeItem(String(key)));
      },

      clear: function () {
        requireAvailable();
        check(host.clear());
      },

      key: function (index) {
        arity("key", 1, arguments.length);
        requireAvailable();
        var position = Number(index);
        // `key(i)` is defined over unsigned integers; anything else indexes
        // nothing rather than being an error.
        if (!isFinite(position) || position < 0) {
          return null;
        }
        var keys = host.keys();
        position = Math.floor(position);
        return position < keys.length ? keys[position] : null;
      },
    };

    Object.defineProperty(storage, "length", {
      get: function () {
        requireAvailable();
        return host.keys().length;
      },
      enumerable: false,
      configurable: true,
    });

    // A property the interface defines — its own methods, `length`, and the
    // ones every object inherits — resolves against the target. Everything
    // else is a stored key.
    function isInterface(property) {
      return typeof property === "symbol" || property in storage;
    }

    return new Proxy(storage, {
      get: function (target, property, receiver) {
        if (isInterface(property)) {
          return Reflect.get(target, property, receiver);
        }
        if (!host.available()) {
          return undefined;
        }
        var value = host.getItem(String(property));
        // A key that is not stored is `undefined` as a property, even though
        // `getItem` reports the same absence as `null`.
        return value === null || value === undefined ? undefined : value;
      },

      set: function (target, property, value, receiver) {
        if (isInterface(property)) {
          return Reflect.set(target, property, value, receiver);
        }
        requireAvailable();
        check(host.setItem(String(property), String(value)));
        return true;
      },

      has: function (target, property) {
        if (isInterface(property)) {
          return Reflect.has(target, property);
        }
        if (!host.available()) {
          return false;
        }
        var value = host.getItem(String(property));
        return value !== null && value !== undefined;
      },

      deleteProperty: function (target, property) {
        if (isInterface(property)) {
          return Reflect.deleteProperty(target, property);
        }
        requireAvailable();
        check(host.removeItem(String(property)));
        return true;
      },

      // An unavailable store enumerates as empty rather than throwing, so that
      // inspecting one is always safe.
      ownKeys: function () {
        return host.available() ? host.keys() : [];
      },

      getOwnPropertyDescriptor: function (target, property) {
        if (isInterface(property)) {
          return Reflect.getOwnPropertyDescriptor(target, property);
        }
        if (!host.available()) {
          return undefined;
        }
        var value = host.getItem(String(property));
        if (value === null || value === undefined) {
          return undefined;
        }
        // Configurable, because a non-configurable descriptor for a value the
        // store can drop underneath us would make the proxy inconsistent.
        return {
          value: value,
          writable: true,
          enumerable: true,
          configurable: true,
        };
      },
    });
  }

  globalThis.scriptStorage = build(__hostScriptStorage, "scriptStorage");
  globalThis.personalStorage = build(__hostPersonalStorage, "personalStorage");
})();
