// The JavaScript half of `console`.
//
// `__writeLog` is the Rust call: it takes one already-formatted string and a
// level, and writes it to the script's log. Everything the browser console
// does to its arguments before that point — joining a variadic call, filling
// in format specifiers, turning a value into something worth reading — has to
// happen here, because the host binding only accepts a string. Handed anything
// else it throws `TypeError: Error converting from js 'object' into type
// 'string'`, which is why every value below is stringified before it crosses.
//
// Two deliberate departures from the browser, both because these lines become
// rows in Postgres rather than entries in a devtools pane:
//
//   - `clear()` does nothing. Pruning stored log lines is engine
//     administration — `DELETE /engine/script_logs` — and deliberately not a
//     script capability. Mapping it here would open that door from JavaScript.
//   - inspection is capped, by depth, by entries per level, and by the length
//     of the finished line. A browser renders lazily and can afford an
//     unbounded object graph; a write cannot.
//
// Every method answers `undefined`, as the browser's does. `__writeLog` does
// return a status string — including `"Error: …"` when the caller lacks the
// capability to write logs — but nothing reads it, and reporting it here would
// mean `console.log` had a return value no browser gives it.

(function () {
  // Deep enough for the objects people actually log — a request, a row, a
  // parsed response — without following a graph to its leaves.
  var MAX_DEPTH = 4;
  // Array elements or object keys rendered per level before eliding.
  var MAX_ENTRIES = 100;
  // Characters in one finished log line.
  var MAX_LINE = 8192;
  // Characters in any single string value inside a structure.
  var MAX_ITEM = 1024;
  // Characters of a stack trace.
  var MAX_STACK = 4096;

  var SPECIFIER = /%[sdifoOjc%]/g;

  var groupDepth = 0;
  var timers = new Map();
  var counts = new Map();

  function truncate(text, max) {
    if (text.length <= max) {
      return text;
    }
    return (
      text.slice(0, max) + "… +" + (text.length - max) + " more characters"
    );
  }

  function quote(text) {
    // `JSON.stringify` handles the escaping; it only answers `undefined` for
    // values that are not strings, and this is only ever called with one.
    var quoted = JSON.stringify(text);
    return typeof quoted === "string" ? quoted : '"' + text + '"';
  }

  function functionLabel(value) {
    return value.name
      ? "[Function: " + value.name + "]"
      : "[Function (anonymous)]";
  }

  function errorText(value) {
    var head = (value.name || "Error") + ": " + (value.message || "");
    var stack = value.stack;
    if (typeof stack !== "string" || stack.length === 0) {
      return head;
    }
    // QuickJS puts only the frames in `stack`, but an engine that prefixes the
    // message the way V8 does would otherwise have it printed twice.
    if (stack.indexOf(head) === 0) {
      return truncate(stack, MAX_STACK);
    }
    return truncate(head + "\n" + stack.replace(/\s+$/, ""), MAX_STACK);
  }

  // Renders the entries of a collection, eliding once past MAX_ENTRIES.
  function entries(items, depth, seen, render) {
    var parts = [];
    var shown = items.length < MAX_ENTRIES ? items.length : MAX_ENTRIES;
    for (var i = 0; i < shown; i++) {
      parts.push(render(items[i], depth, seen));
    }
    if (items.length > shown) {
      parts.push("… +" + (items.length - shown) + " more");
    }
    return parts;
  }

  function wrap(open, parts, close) {
    if (parts.length === 0) {
      return open + close;
    }
    return open + " " + parts.join(", ") + " " + close;
  }

  function inspect(value, depth, seen) {
    var type = typeof value;

    if (value === null) return "null";
    if (type === "undefined") return "undefined";
    if (type === "boolean") return String(value);
    if (type === "number") return Object.is(value, -0) ? "-0" : String(value);
    if (type === "bigint") return String(value) + "n";
    if (type === "symbol") return String(value);
    if (type === "string") return quote(truncate(value, MAX_ITEM));
    if (type === "function") return functionLabel(value);

    // Everything below is an object, so a cycle is possible from here on.
    if (seen.has(value)) {
      return "[Circular]";
    }

    if (value instanceof Error) return errorText(value);
    if (value instanceof RegExp) return String(value);
    if (value instanceof Date) {
      try {
        return value.toISOString();
      } catch (e) {
        return "Invalid Date";
      }
    }

    var isArray = Array.isArray(value);
    var isView = typeof ArrayBuffer === "function" && ArrayBuffer.isView(value);
    var isMap = typeof Map === "function" && value instanceof Map;
    var isSet = typeof Set === "function" && value instanceof Set;

    if (depth >= MAX_DEPTH) {
      if (isArray || isView) return "[Array]";
      if (isMap) return "[Map]";
      if (isSet) return "[Set]";
      return "[Object]";
    }

    seen.add(value);
    var rendered;

    try {
      if (isArray || isView) {
        var elements = [];
        for (var i = 0; i < value.length; i++) {
          elements.push(value[i]);
        }
        rendered = wrap("[", entries(elements, depth + 1, seen, inspect), "]");
        if (isView) {
          rendered =
            (value.constructor && value.constructor.name
              ? value.constructor.name
              : "TypedArray") +
            "(" +
            value.length +
            ") " +
            rendered;
        }
      } else if (isMap) {
        var pairs = [];
        value.forEach(function (v, k) {
          pairs.push([k, v]);
        });
        rendered =
          "Map(" +
          value.size +
          ") " +
          wrap(
            "{",
            entries(pairs, depth + 1, seen, function (pair, d, s) {
              return inspect(pair[0], d, s) + " => " + inspect(pair[1], d, s);
            }),
            "}",
          );
      } else if (isSet) {
        var members = [];
        value.forEach(function (v) {
          members.push(v);
        });
        rendered =
          "Set(" +
          value.size +
          ") " +
          wrap("{", entries(members, depth + 1, seen, inspect), "}");
      } else {
        var keys = Object.keys(value);
        var name = value.constructor && value.constructor.name;
        var prefix = name && name !== "Object" ? name + " " : "";
        // A null-prototype object has no constructor to name it.
        if (!value.constructor && Object.getPrototypeOf(value) === null) {
          prefix = "[Object: null prototype] ";
        }
        rendered =
          prefix +
          wrap(
            "{",
            entries(keys, depth + 1, seen, function (key, d, s) {
              var text;
              try {
                text = inspect(value[key], d, s);
              } catch (e) {
                // A throwing getter must not take the whole log line with it.
                text = "[Getter threw]";
              }
              return /^[A-Za-z_$][A-Za-z0-9_$]*$/.test(key)
                ? key + ": " + text
                : quote(key) + ": " + text;
            }),
            "}",
          );
      }
    } finally {
      // Removed on the way out so two siblings holding the same reference are
      // not mistaken for a cycle.
      seen.delete(value);
    }

    return rendered;
  }

  // A top-level argument: strings print bare, everything else is inspected.
  function stringifyArg(value) {
    return typeof value === "string" ? value : inspect(value, 0, new Set());
  }

  function format(args) {
    if (args.length === 0) {
      return "";
    }

    var parts = [];
    var next = 0;

    if (
      typeof args[0] === "string" &&
      args.length > 1 &&
      SPECIFIER.test(args[0])
    ) {
      // `test` on a /g/ regex leaves `lastIndex` behind it; `replace` below
      // resets it, but reset here too so the check itself cannot alternate.
      SPECIFIER.lastIndex = 0;
      var index = 1;
      parts.push(
        args[0].replace(SPECIFIER, function (spec) {
          if (spec === "%%") {
            return "%";
          }
          if (index >= args.length) {
            // Nothing left to consume: the specifier stays as written.
            return spec;
          }
          var arg = args[index++];
          switch (spec) {
            case "%s":
              return typeof arg === "string" ? arg : inspect(arg, 0, new Set());
            case "%d":
            case "%i":
              return typeof arg === "symbol" || typeof arg === "bigint"
                ? "NaN"
                : String(parseInt(arg, 10));
            case "%f":
              return typeof arg === "symbol" ? "NaN" : String(parseFloat(arg));
            case "%o":
            case "%O":
              return inspect(arg, 0, new Set());
            case "%j":
              try {
                return JSON.stringify(arg);
              } catch (e) {
                return "[Circular]";
              }
            case "%c":
              // A CSS argument styles nothing here, but it is still consumed.
              return "";
          }
          return spec;
        }),
      );
      next = index;
    }

    for (var i = next; i < args.length; i++) {
      parts.push(stringifyArg(args[i]));
    }

    return truncate(parts.join(" "), MAX_LINE);
  }

  function emit(level, message) {
    if (groupDepth > 0) {
      var indent = new Array(groupDepth + 1).join("  ");
      message = indent + message.split("\n").join("\n" + indent);
    }
    // Resolved from the global scope on each call rather than captured, the way
    // `fetch` resolves `__hostFetch`: it keeps the transport replaceable, which
    // is what lets the formatting be tested without a log store behind it.
    __writeLog(message, level);
  }

  function logger(level) {
    return function () {
      emit(level, format(Array.prototype.slice.call(arguments)));
    };
  }

  function now() {
    return typeof performance === "object" &&
      performance !== null &&
      typeof performance.now === "function"
      ? performance.now()
      : Date.now();
  }

  function elapsed(started) {
    return (now() - started).toFixed(3) + "ms";
  }

  function label(value) {
    return value === undefined ? "default" : String(value);
  }

  // ---------------------------------------------------------------------
  // console.table
  // ---------------------------------------------------------------------

  function cell(value) {
    if (value === undefined) return "";
    // Rendered at the depth cap so a nested object stays one tight token
    // rather than unfolding across the column.
    return inspect(value, MAX_DEPTH, new Set());
  }

  function renderTable(rowKeys, columns, hasValues, cells) {
    var header = ["(index)"].concat(columns);
    if (hasValues) {
      header.push("Values");
    }

    var grid = [header].concat(cells);
    var widths = header.map(function (_, column) {
      var widest = 0;
      for (var row = 0; row < grid.length; row++) {
        var text = grid[row][column] || "";
        if (text.length > widest) {
          widest = text.length;
        }
      }
      return widest;
    });

    function rule(left, join, right) {
      return (
        left +
        widths
          .map(function (width) {
            return new Array(width + 3).join("─");
          })
          .join(join) +
        right
      );
    }

    function line(cellsOfRow) {
      return (
        "│ " +
        widths
          .map(function (width, column) {
            var text = cellsOfRow[column] || "";
            return text + new Array(width - text.length + 1).join(" ");
          })
          .join(" │ ") +
        " │"
      );
    }

    var out = [rule("┌", "┬", "┐"), line(header), rule("├", "┼", "┤")];
    for (var i = 0; i < cells.length; i++) {
      out.push(line(cells[i]));
    }
    out.push(rule("└", "┴", "┘"));
    return out.join("\n");
  }

  function table(data, columns) {
    // A primitive has no table in it; the browser falls back to logging it.
    if (data === null || typeof data !== "object") {
      return format([data]);
    }

    var isArray = Array.isArray(data);
    var rowKeys = isArray
      ? data.map(function (_, i) {
          return String(i);
        })
      : Object.keys(data);

    if (rowKeys.length > MAX_ENTRIES) {
      rowKeys = rowKeys.slice(0, MAX_ENTRIES);
    }

    var rows = rowKeys.map(function (key) {
      return data[isArray ? Number(key) : key];
    });

    var found = [];
    var hasValues = false;
    rows.forEach(function (row) {
      if (row !== null && typeof row === "object") {
        Object.keys(row).forEach(function (key) {
          if (found.indexOf(key) === -1) {
            found.push(key);
          }
        });
      } else {
        hasValues = true;
      }
    });

    if (Array.isArray(columns)) {
      var wanted = columns.map(String);
      found = found.filter(function (key) {
        return wanted.indexOf(key) !== -1;
      });
    }

    var cells = rows.map(function (row, i) {
      var line = [rowKeys[i]];
      found.forEach(function (key) {
        line.push(
          row !== null && typeof row === "object" ? cell(row[key]) : "",
        );
      });
      if (hasValues) {
        line.push(row === null || typeof row !== "object" ? cell(row) : "");
      }
      return line;
    });

    return truncate(renderTable(rowKeys, found, hasValues, cells), MAX_LINE);
  }

  // ---------------------------------------------------------------------

  globalThis.console = {
    log: logger("LOG"),
    info: logger("INFO"),
    warn: logger("WARN"),
    error: logger("ERROR"),
    debug: logger("DEBUG"),

    dir: function (value) {
      emit("LOG", inspect(value, 0, new Set()));
    },

    trace: function () {
      var message = format(Array.prototype.slice.call(arguments));
      var head = message ? "Trace: " + message : "Trace";
      var stack = new Error().stack;
      emit(
        "DEBUG",
        typeof stack === "string" && stack.length
          ? truncate(head + "\n" + stack.replace(/\s+$/, ""), MAX_STACK)
          : head,
      );
    },

    assert: function (condition) {
      if (condition) {
        return;
      }
      var message = format(Array.prototype.slice.call(arguments, 1));
      emit(
        "ERROR",
        message ? "Assertion failed: " + message : "Assertion failed",
      );
    },

    table: function (data, columns) {
      emit("LOG", table(data, columns));
    },

    group: function () {
      if (arguments.length > 0) {
        emit("LOG", format(Array.prototype.slice.call(arguments)));
      }
      groupDepth++;
    },

    groupEnd: function () {
      if (groupDepth > 0) {
        groupDepth--;
      }
    },

    time: function (name) {
      var key = label(name);
      if (timers.has(key)) {
        emit("WARN", "Timer '" + key + "' already exists");
        return;
      }
      timers.set(key, now());
    },

    timeLog: function (name) {
      var key = label(name);
      if (!timers.has(key)) {
        emit("WARN", "Timer '" + key + "' does not exist");
        return;
      }
      var rest = format(Array.prototype.slice.call(arguments, 1));
      emit(
        "LOG",
        key + ": " + elapsed(timers.get(key)) + (rest ? " " + rest : ""),
      );
    },

    timeEnd: function (name) {
      var key = label(name);
      if (!timers.has(key)) {
        emit("WARN", "Timer '" + key + "' does not exist");
        return;
      }
      emit("LOG", key + ": " + elapsed(timers.get(key)));
      timers.delete(key);
    },

    count: function (name) {
      var key = label(name);
      var total = (counts.get(key) || 0) + 1;
      counts.set(key, total);
      emit("LOG", key + ": " + total);
    },

    countReset: function (name) {
      var key = label(name);
      if (!counts.has(key)) {
        emit("WARN", "Count for '" + key + "' does not exist");
        return;
      }
      counts.set(key, 0);
    },

    // Stored log lines are pruned through the engine's administration surface,
    // not from a script. Present so the call is not a ReferenceError; does
    // nothing so it cannot be mistaken for one that works.
    clear: function () {},
  };

  globalThis.console.groupCollapsed = globalThis.console.group;
})();
