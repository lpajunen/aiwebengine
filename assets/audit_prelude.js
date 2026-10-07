// The JavaScript half of `audit` — events a script records and cannot take
// back.
//
// Only what happened crosses from here. Who did it, from where and in which
// request is filled in by `__hostAudit` from the execution itself, so there is
// no argument to fill in wrongly.

(function () {
  var host = __hostAudit;

  var api = {
    record: function (action, details) {
      if (typeof action !== "string") {
        throw new Error("audit.record: the action must be a string");
      }
      return host.record(
        action,
        details === undefined || details === null
          ? ""
          : JSON.stringify(details),
      );
    },
  };

  Object.defineProperty(globalThis, "audit", {
    value: Object.freeze(api),
    writable: false,
    enumerable: true,
    configurable: false,
  });
})();
