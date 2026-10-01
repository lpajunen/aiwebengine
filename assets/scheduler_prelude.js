// The JavaScript half of `schedulerService`.
//
// `__hostScheduler` answers in the envelope every preluded global uses. A
// mistake in the call throws; a registration made outside startup and
// `init()` is refused as the value `{ ok: false, reason }`, as
// `routeRegistry.registerRoute`'s is, because top-level code runs on every
// execution and must not throw for being where it is.

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

  function options(value, api) {
    if (value === null || typeof value !== "object" || Array.isArray(value)) {
      throw new TypeError(api + ": takes an options object");
    }
    return value;
  }

  var host = globalThis.__hostScheduler;

  var schedulerService = {
    // registerOnce({ handler, runAt, name? }) -> { ok, jobId, name, nextRun }
    registerOnce: function (opts) {
      var api = "schedulerService.registerOnce";
      return unwrap(host.registerOnce(options(opts, api)), api);
    },

    // registerRecurring({ handler, intervalMilliseconds | intervalMinutes,
    //                     name?, startAt? }) -> { ok, jobId, name, nextRun }
    registerRecurring: function (opts) {
      var api = "schedulerService.registerRecurring";
      return unwrap(host.registerRecurring(options(opts, api)), api);
    },

    // clearAll() -> { ok, cleared }
    clearAll: function () {
      return unwrap(host.clearAll(), "schedulerService.clearAll");
    },
  };

  Object.defineProperty(globalThis, "schedulerService", {
    value: Object.freeze(schedulerService),
    writable: false,
    enumerable: true,
    configurable: false,
  });

  try {
    delete globalThis.__hostScheduler;
  } catch (e) {
    /* non-configurable in some contexts; the interface above is still what is documented */
  }
})();
