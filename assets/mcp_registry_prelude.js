// The JavaScript half of `mcpRegistry`.
//
// `__hostMcpRegistry` answers in the envelope every preluded global uses. The
// three registrations take a name and a spec object, as
// `routeRegistry.registerRoute` does:
//
//   mcpRegistry.registerTool("sum", { description, inputSchema, handler });
//   mcpRegistry.registerPrompt("review", { description, arguments, handler });
//   mcpRegistry.registerResource("docs://handbook", { file, name, description, mimeType });
//
// A mistake in the call throws. A refusal — a call made outside `init()`, a
// resource file outside `resources/` — is the value `{ ok: false, reason }`.

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

  // The spec, checked for shape: an object holding only known keys. The old
  // positional form — a string where the spec goes — is named in the error,
  // because every script written before the change makes exactly that call.
  function spec(name, value, keys, api, oldForm) {
    if (typeof name !== "string" || name === "") {
      throw new TypeError(
        api + ": the first argument must be a non-empty string",
      );
    }
    if (typeof value === "string") {
      throw new TypeError(
        api +
          "(" +
          oldForm +
          ") is now " +
          api +
          "(name, { " +
          keys.join(", ") +
          " })",
      );
    }
    if (value === null || typeof value !== "object" || Array.isArray(value)) {
      throw new TypeError(
        api + ": the second argument is { " + keys.join(", ") + " }",
      );
    }
    for (var k in value) {
      if (keys.indexOf(k) === -1) {
        throw new TypeError(
          api + ': unknown key "' + k + '" (expected ' + keys.join(", ") + ")",
        );
      }
    }
    return value;
  }

  function text(value, what, api) {
    if (typeof value !== "string" || value === "") {
      throw new TypeError(api + ": " + what + " must be a non-empty string");
    }
    return value;
  }

  var host = globalThis.__hostMcpRegistry;

  var mcpRegistry = {
    // registerTool(name, { description, inputSchema, handler })
    registerTool: function (name, toolSpec) {
      var api = "mcpRegistry.registerTool";
      var s = spec(
        name,
        toolSpec,
        ["description", "inputSchema", "handler"],
        api,
        "name, description, inputSchemaJson, handler",
      );
      var schema =
        s.inputSchema === undefined
          ? { type: "object", properties: {} }
          : s.inputSchema;
      if (
        schema === null ||
        typeof schema !== "object" ||
        Array.isArray(schema)
      ) {
        throw new TypeError(api + ": inputSchema is a JSON Schema object");
      }
      return unwrap(
        host.registerTool(
          name,
          text(s.description, "description", api),
          JSON.stringify(schema),
          text(s.handler, "handler", api),
        ),
        api,
      );
    },

    // registerPrompt(name, { description, arguments?, handler })
    // arguments: [{ name, description?, required? }]
    registerPrompt: function (name, promptSpec) {
      var api = "mcpRegistry.registerPrompt";
      var s = spec(
        name,
        promptSpec,
        ["description", "arguments", "handler"],
        api,
        "name, description, argumentsJson, handler",
      );
      var args = s.arguments === undefined ? [] : s.arguments;
      if (!Array.isArray(args)) {
        throw new TypeError(
          api + ": arguments is an array of { name, description, required }",
        );
      }
      return unwrap(
        host.registerPrompt(
          name,
          text(s.description, "description", api),
          JSON.stringify(args),
          text(s.handler, "handler", api),
        ),
        api,
      );
    },

    // registerResource(uri, { file, name?, description?, mimeType? })
    // `file` must be under `resources/` in the script's tree.
    registerResource: function (uri, resourceSpec) {
      var api = "mcpRegistry.registerResource";
      var s = spec(
        uri,
        resourceSpec,
        ["file", "name", "description", "mimeType"],
        api,
        "uri, assetName, metadata",
      );
      var metadata = {};
      if (s.name !== undefined) metadata.name = s.name;
      if (s.description !== undefined) metadata.description = s.description;
      if (s.mimeType !== undefined) metadata.mimeType = s.mimeType;
      return unwrap(
        host.registerResource(uri, text(s.file, "file", api), metadata),
        api,
      );
    },
  };

  Object.defineProperty(globalThis, "mcpRegistry", {
    value: Object.freeze(mcpRegistry),
    writable: false,
    enumerable: true,
    configurable: false,
  });

  try {
    delete globalThis.__hostMcpRegistry;
  } catch (e) {
    /* non-configurable in some contexts; the interface above is still what is documented */
  }
})();
