/**
 * Test authoring API installed into the context that runs a script's test
 * modules: `test`/`it`, `describe`, `expect`, `assert`, and the per-case hooks.
 *
 * Nothing here executes a test. `__registerTest__` is provided by the engine and
 * only collects cases; the engine runs them after the whole module has been
 * evaluated, so a test that throws cannot stop the ones below it from
 * registering. Hooks are file-scoped — one context runs one test module — and
 * apply to every case in the file no matter where they are declared.
 */
(function () {
  const suites = [];
  const beforeEachHooks = [];
  const afterEachHooks = [];

  function qualify(name) {
    return suites.length ? suites.join(" > ") + " > " + name : name;
  }

  function format(value) {
    if (typeof value === "string") return JSON.stringify(value);
    if (typeof value === "function") {
      return value.name ? "[Function " + value.name + "]" : "[Function]";
    }
    if (value === undefined) return "undefined";
    if (typeof value === "bigint") return value.toString() + "n";
    try {
      const rendered = JSON.stringify(value);
      return rendered === undefined ? String(value) : rendered;
    } catch (error) {
      return String(value);
    }
  }

  function deepEqual(a, b) {
    if (a === b) return true;
    // NaN equals itself for comparison purposes, as in every test framework.
    if (a !== a && b !== b) return true;
    if (a === null || b === null) return false;
    if (typeof a !== "object" || typeof b !== "object") return false;
    if (Array.isArray(a) !== Array.isArray(b)) return false;

    const aKeys = Object.keys(a);
    const bKeys = Object.keys(b);
    if (aKeys.length !== bKeys.length) return false;

    return aKeys.every(
      (key) =>
        Object.prototype.hasOwnProperty.call(b, key) &&
        deepEqual(a[key], b[key]),
    );
  }

  function matchers(actual, negated) {
    function claim(satisfied, description) {
      if (satisfied === negated) {
        throw new Error(
          "Expected " +
            format(actual) +
            (negated ? " not to " : " to ") +
            description,
        );
      }
    }

    const api = {
      toBe(expected) {
        claim(
          actual === expected || (actual !== actual && expected !== expected),
          "be " + format(expected),
        );
      },
      toEqual(expected) {
        claim(deepEqual(actual, expected), "equal " + format(expected));
      },
      toBeTruthy() {
        claim(!!actual, "be truthy");
      },
      toBeFalsy() {
        claim(!actual, "be falsy");
      },
      toBeNull() {
        claim(actual === null, "be null");
      },
      toBeUndefined() {
        claim(actual === undefined, "be undefined");
      },
      toBeDefined() {
        claim(actual !== undefined, "be defined");
      },
      toBeCloseTo(expected, digits) {
        const tolerance =
          Math.pow(10, -(digits === undefined ? 2 : digits)) / 2;
        claim(
          Math.abs(actual - expected) < tolerance,
          "be close to " + format(expected),
        );
      },
      toBeGreaterThan(expected) {
        claim(actual > expected, "be greater than " + format(expected));
      },
      toBeLessThan(expected) {
        claim(actual < expected, "be less than " + format(expected));
      },
      toHaveLength(expected) {
        claim(
          actual !== null && actual !== undefined && actual.length === expected,
          "have length " + format(expected),
        );
      },
      toContain(item) {
        if (typeof actual === "string") {
          claim(actual.indexOf(item) !== -1, "contain " + format(item));
          return;
        }
        if (!Array.isArray(actual)) {
          throw new Error(
            "expect(...).toContain() needs a string or array, got " +
              format(actual),
          );
        }
        claim(
          actual.some((entry) => deepEqual(entry, item)),
          "contain " + format(item),
        );
      },
      toMatch(pattern) {
        const regex = pattern instanceof RegExp ? pattern : new RegExp(pattern);
        claim(regex.test(String(actual)), "match " + String(regex));
      },
      toThrow(expected) {
        if (typeof actual !== "function") {
          throw new Error(
            "expect(fn).toThrow() needs a function, got " + format(actual),
          );
        }

        let threw = false;
        let thrown = null;
        try {
          actual();
        } catch (error) {
          threw = true;
          thrown = error;
        }

        let satisfied = threw;
        if (threw && expected !== undefined) {
          const message =
            thrown && thrown.message ? String(thrown.message) : String(thrown);
          satisfied =
            expected instanceof RegExp
              ? expected.test(message)
              : message.indexOf(String(expected)) !== -1;
        }

        claim(
          satisfied,
          expected === undefined
            ? "throw"
            : "throw an error matching " + format(String(expected)),
        );
      },
    };

    if (!negated) {
      api.not = matchers(actual, true);
    }
    return api;
  }

  function runHooks(hooks) {
    for (const hook of hooks) {
      hook();
    }
  }

  function register(name, fn) {
    if (typeof name !== "string" || name.length === 0) {
      throw new Error("test(name, fn) needs a non-empty name");
    }
    if (typeof fn !== "function") {
      throw new Error('test("' + name + '", fn) needs a function');
    }

    const qualified = qualify(name);
    __registerTest__(qualified, function () {
      runHooks(beforeEachHooks);
      try {
        const result = fn();
        if (
          result !== null &&
          result !== undefined &&
          typeof result.then === "function"
        ) {
          throw new Error(
            'Test "' +
              qualified +
              '" returned a promise. Scripts run synchronously here — host calls ' +
              "like fetch() and database queries block rather than yielding — so an " +
              "async test body would never settle. Drop the async/await.",
          );
        }
      } finally {
        runHooks(afterEachHooks);
      }
    });
  }

  globalThis.test = register;
  globalThis.it = register;

  globalThis.describe = function (name, fn) {
    suites.push(name);
    try {
      fn();
    } finally {
      suites.pop();
    }
  };

  globalThis.beforeEach = function (fn) {
    beforeEachHooks.push(fn);
  };

  globalThis.afterEach = function (fn) {
    afterEachHooks.push(fn);
  };

  globalThis.expect = function (actual) {
    return matchers(actual, false);
  };

  globalThis.assert = function (condition, message) {
    if (!condition) {
      throw new Error(message || "Assertion failed");
    }
  };
})();
