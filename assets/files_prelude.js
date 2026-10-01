// The JavaScript half of `files`: a script's own tree, by path.
//
// `__hostFiles` is the Rust call. Each of its methods answers with an envelope
// — `{ok: value}` or `{error: {name, message}}` — and this turns that into the
// interface a script uses: values out, real exceptions on failure.
//
//   files.list()                          -> [{ path, size, mimetype, createdAt, updatedAt }]
//   files.read("skills/refund.md")        -> text, or null if there is no such file
//   files.read("logo.png", { encoding: "base64" })
//   files.write("notes/today.md", text)   -> undefined
//   files.write("logo.png", b64, { encoding: "base64", mimetype: "image/png" })
//   files.delete("notes/today.md")        -> true, or false if it was not there
//
// For content that does not change without a redeploy, an import is better
// than a read: `import policy from "./skills/refund.md"` resolves at link
// time, is cached in the prepared program and is pinned by the revision. A
// read is for content the script writes, or that changes under it.

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

  function path(value, api) {
    if (typeof value !== "string" || value === "") {
      throw new TypeError(api + ": path must be a non-empty string");
    }
    return value;
  }

  function encoding(options, api) {
    var value = options && options.encoding;
    if (value === undefined || value === null || value === "utf8") return false;
    if (value === "base64") return true;
    throw new TypeError(api + ': encoding is "utf8" or "base64"');
  }

  var host = globalThis.__hostFiles;

  var files = {
    list: function () {
      return unwrap(host.list(), "files.list");
    },

    read: function (filePath, options) {
      var api = "files.read";
      return unwrap(
        host.read(path(filePath, api), encoding(options, api)),
        api,
      );
    },

    write: function (filePath, content, options) {
      var api = "files.write";
      if (typeof content !== "string") {
        throw new TypeError(
          api +
            ': content is a string — text, or base64 with { encoding: "base64" }',
        );
      }
      var mimetype = options && options.mimetype;
      unwrap(
        host.write(
          path(filePath, api),
          content,
          encoding(options, api),
          mimetype === undefined || mimetype === null
            ? undefined
            : String(mimetype),
        ),
        api,
      );
    },

    delete: function (filePath) {
      return unwrap(
        host.delete(path(filePath, "files.delete")),
        "files.delete",
      );
    },
  };

  // Writable and configurable, unlike the other preludes' globals. `files` is
  // a name scripts use for their own variables, and a non-configurable global
  // would turn a top-level `const files = ...` into a SyntaxError before the
  // script ran a line. Replacing it takes nothing from anybody but the script
  // that did: every check is on the host side.
  Object.defineProperty(globalThis, "files", {
    value: Object.freeze(files),
    writable: true,
    enumerable: true,
    configurable: true,
  });

  try {
    delete globalThis.__hostFiles;
  } catch (e) {
    /* non-configurable in some contexts; the interface above is still what is documented */
  }
})();
