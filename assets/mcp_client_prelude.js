// The JavaScript half of `McpClient`: a client for another MCP server.
//
//   const github = new McpClient("https://api.githubcopilot.com/mcp/", "GITHUB_TOKEN");
//   const tools = github.listTools();
//   const result = github.callTool("search_issues", { query: "is:open" });
//
// `__hostMcpClient` is the Rust call, three static functions passing JSON
// text — `constructor`, `_listTools`, `_callTool` — which every caller used
// to wrap in a class of its own. This is that class, once. A failure throws
// an `Error` whose message is what went wrong, rather than the binding's
// "Error converting from js ..." around it, and a JSON-RPC error from the
// server throws too; a tool result with `isError` is a result, and returned.

(function () {
  var host = globalThis.__hostMcpClient;

  function call(api, fn) {
    try {
      return fn();
    } catch (e) {
      var message = e && e.message ? String(e.message) : String(e);
      var inner =
        /^Error converting from js '[^']*' into type '[^']*': ([\s\S]*)$/.exec(
          message,
        );
      var error = new Error(
        "McpClient." + api + ": " + (inner ? inner[1] : message),
      );
      throw error;
    }
  }

  function parse(api, text) {
    try {
      return JSON.parse(text);
    } catch (e) {
      throw new Error(
        "McpClient." + api + ": the engine returned something unreadable",
      );
    }
  }

  function McpClient(serverUrl, secretIdentifier) {
    if (!(this instanceof McpClient)) {
      throw new TypeError(
        "McpClient is a class: use new McpClient(serverUrl, secretName)",
      );
    }
    if (typeof serverUrl !== "string" || typeof secretIdentifier !== "string") {
      throw new TypeError(
        "new McpClient(serverUrl, secretName) takes two strings",
      );
    }
    var data = call("constructor", function () {
      return host.constructor(serverUrl, secretIdentifier);
    });
    Object.defineProperty(this, "_data", { value: data });
    this.serverUrl = serverUrl;
  }

  // listTools() -> the server's tools: [{ name, description, inputSchema }]
  McpClient.prototype.listTools = function () {
    var data = this._data;
    var answer = parse(
      "listTools",
      call("listTools", function () {
        return host._listTools(data);
      }),
    );
    return Array.isArray(answer) ? answer : (answer && answer.tools) || [];
  };

  // callTool(name, args) -> the tool's result
  McpClient.prototype.callTool = function (name, args) {
    if (typeof name !== "string" || name === "") {
      throw new TypeError(
        "McpClient.callTool: the tool name must be a non-empty string",
      );
    }
    var data = this._data;
    var json = JSON.stringify(args === undefined ? {} : args);
    var answer = parse(
      "callTool",
      call("callTool", function () {
        return host._callTool(data, name, json);
      }),
    );
    if (answer && answer.error) {
      var rpc = answer.error;
      var error = new Error(
        "McpClient.callTool: " +
          (rpc.message || JSON.stringify(rpc)) +
          (rpc.code !== undefined ? " (" + rpc.code + ")" : ""),
      );
      error.code = rpc.code;
      throw error;
    }
    return answer;
  };

  Object.defineProperty(globalThis, "McpClient", {
    value: McpClient,
    writable: false,
    enumerable: true,
    configurable: false,
  });

  try {
    delete globalThis.__hostMcpClient;
  } catch (e) {
    /* non-configurable in some contexts; the interface above is still what is documented */
  }
})();
