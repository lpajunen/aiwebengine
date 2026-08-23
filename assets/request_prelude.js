// `Headers`, `URLSearchParams`, and the shape `context.request` presents.
//
// The response side of a script grew methods when `fetch` did: a response has
// `.json()` and `.text()`, and reading one no longer depends on which call
// produced it. The request side did not move, so the same script parsed a body
// it received differently from a body it fetched, and read headers out of a
// plain object where the case had to match exactly.
//
// What this adds to `context.request`:
//
//   - `headers` is a `Headers`, so `headers.get("content-type")` finds a header
//     the client spelled `Content-Type`. Reading it as an object still works —
//     `headers["content-type"]` and `Object.keys(headers)` both do — so the
//     scripts written against the plain object keep working, and become
//     case-insensitive in the process.
//   - `text()` and `json()`, mirroring what a `fetch` response answers.
//   - `searchParams`, a `URLSearchParams` parsed from the URL the request
//     arrived on. This is the only place a repeated parameter survives:
//     `request.query` is a plain object built from a map, so `?tag=a&tag=b`
//     leaves just one of them there.
//
// `request.query`, `request.form`, `request.params`, `request.files` and
// `request.body` are untouched.

(function () {
  // ---------------------------------------------------------------------
  // Headers
  // ---------------------------------------------------------------------

  // Header names are case-insensitive, so the map is keyed by the lowercased
  // name and remembers the spelling it was given for iteration.
  function normalize(name) {
    return String(name).toLowerCase();
  }

  function Headers(init) {
    // A plain object rather than a Map, so the instance can be handed to the
    // proxy below and read as the object `request.headers` used to be.
    // Configurable, because the proxy below hides this from `ownKeys` and a
    // proxy may only do that for a configurable property — `Object.keys` on the
    // headers throws otherwise.
    Object.defineProperty(this, "__values", {
      value: Object.create(null),
      enumerable: false,
      writable: true,
      configurable: true,
    });

    if (init === null || init === undefined) {
      return;
    }
    if (Array.isArray(init)) {
      for (var i = 0; i < init.length; i++) {
        this.append(init[i][0], init[i][1]);
      }
      return;
    }
    if (init instanceof Headers) {
      var self = this;
      init.forEach(function (value, name) {
        self.append(name, value);
      });
      return;
    }
    var names = Object.keys(init);
    for (var k = 0; k < names.length; k++) {
      this.append(names[k], init[names[k]]);
    }
  }

  Headers.prototype.get = function (name) {
    var key = normalize(name);
    return key in this.__values ? this.__values[key] : null;
  };

  Headers.prototype.has = function (name) {
    return normalize(name) in this.__values;
  };

  Headers.prototype.set = function (name, value) {
    this.__values[normalize(name)] = String(value);
  };

  // Repeated headers combine with ", ", which is how the spec says a
  // multi-valued header reads as one.
  Headers.prototype.append = function (name, value) {
    var key = normalize(name);
    this.__values[key] =
      key in this.__values
        ? this.__values[key] + ", " + String(value)
        : String(value);
  };

  Headers.prototype.delete = function (name) {
    delete this.__values[normalize(name)];
  };

  Headers.prototype.forEach = function (callback, thisArg) {
    var keys = Object.keys(this.__values).sort();
    for (var i = 0; i < keys.length; i++) {
      callback.call(thisArg, this.__values[keys[i]], keys[i], this);
    }
  };

  Headers.prototype.keys = function () {
    return Object.keys(this.__values).sort()[Symbol.iterator]();
  };

  Headers.prototype.values = function () {
    var self = this;
    return Object.keys(this.__values)
      .sort()
      .map(function (key) {
        return self.__values[key];
      })
      [Symbol.iterator]();
  };

  Headers.prototype.entries = function () {
    var self = this;
    return Object.keys(this.__values)
      .sort()
      .map(function (key) {
        return [key, self.__values[key]];
      })
      [Symbol.iterator]();
  };

  Headers.prototype[Symbol.iterator] = Headers.prototype.entries;

  // ---------------------------------------------------------------------
  // URLSearchParams
  // ---------------------------------------------------------------------

  function decodeComponent(text) {
    try {
      // `+` means a space in a query string, which `decodeURIComponent` does
      // not know about.
      return decodeURIComponent(String(text).replace(/\+/g, " "));
    } catch (e) {
      // A malformed escape is not worth losing the whole parameter over.
      return String(text);
    }
  }

  function encodeComponent(text) {
    return encodeURIComponent(String(text)).replace(/%20/g, "+");
  }

  function URLSearchParams(init) {
    // An array of pairs, not a map: the whole point is that a parameter can
    // appear more than once.
    Object.defineProperty(this, "__pairs", {
      value: [],
      enumerable: false,
      writable: true,
      configurable: true,
    });

    if (init === null || init === undefined || init === "") {
      return;
    }
    if (Array.isArray(init)) {
      for (var i = 0; i < init.length; i++) {
        this.append(init[i][0], init[i][1]);
      }
      return;
    }
    if (init instanceof URLSearchParams) {
      var self = this;
      init.forEach(function (value, name) {
        self.append(name, value);
      });
      return;
    }
    if (typeof init === "object") {
      var names = Object.keys(init);
      for (var k = 0; k < names.length; k++) {
        this.append(names[k], init[names[k]]);
      }
      return;
    }

    var query = String(init);
    if (query.charAt(0) === "?") {
      query = query.slice(1);
    }
    if (query === "") {
      return;
    }
    var parts = query.split("&");
    for (var p = 0; p < parts.length; p++) {
      if (parts[p] === "") {
        continue;
      }
      var split = parts[p].indexOf("=");
      if (split === -1) {
        this.__pairs.push([decodeComponent(parts[p]), ""]);
      } else {
        this.__pairs.push([
          decodeComponent(parts[p].slice(0, split)),
          decodeComponent(parts[p].slice(split + 1)),
        ]);
      }
    }
  }

  URLSearchParams.prototype.get = function (name) {
    var key = String(name);
    for (var i = 0; i < this.__pairs.length; i++) {
      if (this.__pairs[i][0] === key) {
        return this.__pairs[i][1];
      }
    }
    return null;
  };

  URLSearchParams.prototype.getAll = function (name) {
    var key = String(name);
    var found = [];
    for (var i = 0; i < this.__pairs.length; i++) {
      if (this.__pairs[i][0] === key) {
        found.push(this.__pairs[i][1]);
      }
    }
    return found;
  };

  URLSearchParams.prototype.has = function (name) {
    return this.get(name) !== null;
  };

  URLSearchParams.prototype.append = function (name, value) {
    this.__pairs.push([String(name), String(value)]);
  };

  // Replaces the first occurrence and drops the rest, so a name set once reads
  // back once however many times it arrived.
  URLSearchParams.prototype.set = function (name, value) {
    var key = String(name);
    var replaced = false;
    var kept = [];
    for (var i = 0; i < this.__pairs.length; i++) {
      if (this.__pairs[i][0] !== key) {
        kept.push(this.__pairs[i]);
      } else if (!replaced) {
        kept.push([key, String(value)]);
        replaced = true;
      }
    }
    if (!replaced) {
      kept.push([key, String(value)]);
    }
    this.__pairs = kept;
  };

  URLSearchParams.prototype.delete = function (name) {
    var key = String(name);
    this.__pairs = this.__pairs.filter(function (pair) {
      return pair[0] !== key;
    });
  };

  URLSearchParams.prototype.sort = function () {
    // Stable by name, so repeated values keep the order they arrived in.
    this.__pairs = this.__pairs
      .map(function (pair, index) {
        return [pair, index];
      })
      .sort(function (a, b) {
        if (a[0][0] < b[0][0]) return -1;
        if (a[0][0] > b[0][0]) return 1;
        return a[1] - b[1];
      })
      .map(function (entry) {
        return entry[0];
      });
  };

  URLSearchParams.prototype.forEach = function (callback, thisArg) {
    for (var i = 0; i < this.__pairs.length; i++) {
      callback.call(thisArg, this.__pairs[i][1], this.__pairs[i][0], this);
    }
  };

  URLSearchParams.prototype.keys = function () {
    return this.__pairs
      .map(function (pair) {
        return pair[0];
      })
      [Symbol.iterator]();
  };

  URLSearchParams.prototype.values = function () {
    return this.__pairs
      .map(function (pair) {
        return pair[1];
      })
      [Symbol.iterator]();
  };

  URLSearchParams.prototype.entries = function () {
    return this.__pairs
      .map(function (pair) {
        return [pair[0], pair[1]];
      })
      [Symbol.iterator]();
  };

  URLSearchParams.prototype[Symbol.iterator] =
    URLSearchParams.prototype.entries;

  URLSearchParams.prototype.toString = function () {
    return this.__pairs
      .map(function (pair) {
        return encodeComponent(pair[0]) + "=" + encodeComponent(pair[1]);
      })
      .join("&");
  };

  Object.defineProperty(URLSearchParams.prototype, "size", {
    get: function () {
      return this.__pairs.length;
    },
    configurable: true,
  });

  globalThis.Headers = Headers;
  globalThis.URLSearchParams = URLSearchParams;

  // ---------------------------------------------------------------------
  // context.request
  // ---------------------------------------------------------------------

  // A `Headers` that also reads as the plain object it replaces. Without this,
  // every `request.headers["content-type"]` already written would stop working;
  // with it, they keep working and stop depending on the client's capitalisation.
  function headersView(headers) {
    return new Proxy(headers, {
      get: function (target, property, receiver) {
        if (typeof property === "symbol" || property in target) {
          return Reflect.get(target, property, receiver);
        }
        var value = target.get(property);
        return value === null ? undefined : value;
      },
      has: function (target, property) {
        if (typeof property === "symbol" || property in target) {
          return Reflect.has(target, property);
        }
        return target.has(property);
      },
      ownKeys: function (target) {
        return Object.keys(target.__values).sort();
      },
      getOwnPropertyDescriptor: function (target, property) {
        if (typeof property === "symbol" || property in target) {
          return Reflect.getOwnPropertyDescriptor(target, property);
        }
        var value = target.get(property);
        if (value === null) {
          return undefined;
        }
        return {
          value: value,
          writable: true,
          enumerable: true,
          configurable: true,
        };
      },
    });
  }

  // Called by the engine once it has built the request object, before a handler
  // sees it. Everything added here is additive: no existing field changes type
  // except `headers`, which gains methods while still reading as an object.
  globalThis.__enhanceRequest = function (request) {
    if (!request || typeof request !== "object") {
      return request;
    }

    var raw = request.headers;
    var headers = new Headers(raw && typeof raw === "object" ? raw : null);
    // Non-enumerable so the enhanced request still serialises like the plain
    // object it was — `JSON.stringify(request)` keeps its old shape.
    Object.defineProperty(request, "headers", {
      value: headersView(headers),
      enumerable: true,
      writable: true,
      configurable: true,
    });

    var query = "";
    if (typeof request.url === "string") {
      var mark = request.url.indexOf("?");
      if (mark !== -1) {
        query = request.url.slice(mark + 1);
      }
    }
    var params = new URLSearchParams(query);
    Object.defineProperty(request, "searchParams", {
      value: params,
      enumerable: false,
      writable: true,
      configurable: true,
    });

    Object.defineProperty(request, "text", {
      value: function () {
        return typeof request.body === "string" ? request.body : "";
      },
      enumerable: false,
      writable: true,
      configurable: true,
    });

    Object.defineProperty(request, "json", {
      value: function () {
        // Throws where the script asks for the parse, the way a `fetch`
        // response does, rather than on the way in from a request that arrived
        // perfectly well and simply was not JSON.
        return JSON.parse(typeof request.body === "string" ? request.body : "");
      },
      enumerable: false,
      writable: true,
      configurable: true,
    });

    return request;
  };
})();
