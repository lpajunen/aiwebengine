// The JavaScript half of `database`: a script's own tables.
//
// `__hostDatabase` is the Rust call. Each of its methods answers with JSON —
// the rows, the row, or an outcome — and `{error: "..."}` when it failed. This
// turns that into the interface a script uses: values out, real exceptions on
// failure, and objects in where the host takes JSON text.
//
//   database.ensureTable("notes", { columns: [{ name: "body", type: "text" }] });
//   database.insert("notes", { body: "hello" });           -> the row, with its id
//   database.query("notes", { where: { body: "hello" }, limit: 10 });  -> rows
//   database.transaction(() => { ... });                    -> what the function returned
//
// Ten methods. The schema is described rather than built step by step —
// `ensureTable` replaced `createTable`, seven `add*Column` calls and
// `addUniqueIndex` — and the six transaction and savepoint calls are one
// `transaction(fn)` that cannot be left open.

(function () {
  function unwrap(raw, api) {
    var answer;
    try {
      answer = JSON.parse(raw);
    } catch (e) {
      throw new Error(api + ": the engine returned something unreadable");
    }
    if (answer && typeof answer === "object" && !Array.isArray(answer)) {
      if (answer.error !== undefined) {
        var error = new Error(api + ": " + answer.error);
        if (answer.elevate) error.elevate = answer.elevate;
        throw error;
      }
      // Failures throw, so a flag saying this one did not is noise.
      delete answer.success;
    }
    return answer;
  }

  function table(name, api) {
    if (typeof name !== "string" || name === "") {
      throw new TypeError(api + ": the table name must be a non-empty string");
    }
    return name;
  }

  function object(value, what, api) {
    if (value === null || typeof value !== "object" || Array.isArray(value)) {
      throw new TypeError(api + ": " + what + " must be an object");
    }
    return JSON.stringify(value);
  }

  var QUERY_KEYS = ["where", "limit", "orderBy", "order", "forUpdate"];

  var host = globalThis.__hostDatabase;

  var database = {
    // ensureTable(name, { columns: [{ name, type, nullable?, default?, references? }],
    //                     uniqueIndexes?: [["col", ...]] })
    ensureTable: function (name, schema) {
      var api = "database.ensureTable";
      return unwrap(
        host.ensureTable(table(name, api), object(schema, "the schema", api)),
        api,
      );
    },

    dropTable: function (name) {
      var api = "database.dropTable";
      return unwrap(host.dropTable(table(name, api)), api);
    },

    dropColumn: function (name, column) {
      var api = "database.dropColumn";
      return unwrap(host.dropColumn(table(name, api), String(column)), api);
    },

    // query(name, { where?, limit?, orderBy?, order?: "asc" | "desc", forUpdate? }) -> rows
    query: function (name, options) {
      var api = "database.query";
      table(name, api);
      var o = options === undefined || options === null ? {} : options;
      if (typeof o !== "object" || Array.isArray(o)) {
        throw new TypeError(api + ": the options are an object");
      }
      for (var key in o) {
        if (QUERY_KEYS.indexOf(key) === -1) {
          throw new TypeError(
            api +
              ': unknown option "' +
              key +
              '" (expected ' +
              QUERY_KEYS.join(", ") +
              ")",
          );
        }
      }
      return unwrap(
        host.query(
          name,
          o.where === undefined || o.where === null
            ? null
            : object(o.where, "where", api),
          o.limit === undefined ? null : o.limit,
          o.orderBy === undefined ? null : o.orderBy,
          o.order === undefined ? null : o.order,
          o.forUpdate ? JSON.stringify({ forUpdate: true }) : null,
        ),
        api,
      );
    },

    // insert(name, row) -> the row as stored, with its id
    insert: function (name, row) {
      var api = "database.insert";
      return unwrap(
        host.insert(table(name, api), object(row, "the row", api)),
        api,
      );
    },

    // update(name, id, changes) -> the row as stored
    update: function (name, id, changes) {
      var api = "database.update";
      return unwrap(
        host.update(table(name, api), id, object(changes, "the changes", api)),
        api,
      );
    },

    delete: function (name, id) {
      var api = "database.delete";
      return unwrap(host.delete(table(name, api), id), api);
    },

    // upsert(name, keyColumns, row) — keyColumns needs a unique index, which
    // ensureTable's uniqueIndexes provides.
    upsert: function (name, keyColumns, row) {
      var api = "database.upsert";
      var keys = typeof keyColumns === "string" ? [keyColumns] : keyColumns;
      if (!Array.isArray(keys) || keys.length === 0) {
        throw new TypeError(
          api + ": keyColumns is a column name or an array of them",
        );
      }
      return unwrap(
        host.upsert(
          table(name, api),
          JSON.stringify(keys),
          object(row, "the row", api),
        ),
        api,
      );
    },

    deleteWhere: function (name, where) {
      var api = "database.deleteWhere";
      return unwrap(
        host.deleteWhere(table(name, api), object(where, "where", api)),
        api,
      );
    },

    // transaction(fn, { timeoutMs? }) -> what fn returned
    //
    // Commits when fn returns and rolls back when it throws, so a transaction
    // cannot be left open by a forgotten call. Inside another transaction it
    // is a savepoint: rolling back the inner one keeps the outer one's work.
    // An async fn is awaited before committing.
    transaction: function (fn, options) {
      var api = "database.transaction";
      if (typeof fn !== "function") {
        throw new TypeError(api + ": takes the function to run inside it");
      }
      var timeout = options && options.timeoutMs;
      unwrap(
        timeout === undefined || timeout === null
          ? host.beginTransaction()
          : host.beginTransaction(timeout),
        api,
      );

      function rollback(error) {
        try {
          host.rollbackTransaction();
        } catch (e) {
          /* the original error is the one worth reporting */
        }
        throw error;
      }

      var result;
      try {
        result = fn();
      } catch (e) {
        rollback(e);
      }
      // A commit can be refused — a transaction past its budget, say — and
      // a refused commit leaves the transaction open, so it is rolled back
      // before the refusal is reported.
      function commit(value) {
        var answer = host.commitTransaction();
        try {
          unwrap(answer, api);
        } catch (e) {
          rollback(e);
        }
        return value;
      }
      if (result && typeof result.then === "function") {
        return result.then(commit, rollback);
      }
      return commit(result);
    },
  };

  Object.defineProperty(globalThis, "database", {
    value: Object.freeze(database),
    writable: false,
    enumerable: true,
    configurable: false,
  });

  try {
    delete globalThis.__hostDatabase;
  } catch (e) {
    /* non-configurable in some contexts; the interface above is still what is documented */
  }
})();
