// The JavaScript half of `rateLimit` — a budget the script sizes and the
// engine keys.
//
// The bucket is named here and its owner is not: `__hostRateLimit` decides
// whose allowance is spent from the request the engine judged, so there is no
// argument a script could fill in wrongly to spend somebody else's.

(function () {
  var host = __hostRateLimit;

  var api = {
    consume: function (bucket, options) {
      if (!options || typeof options !== "object") {
        throw new Error(
          "rateLimit.consume: expects a bucket name and { limit, windowSeconds }",
        );
      }
      return JSON.parse(
        host.consume(
          JSON.stringify({
            bucket: String(bucket == null ? "" : bucket),
            limit: options.limit,
            windowSeconds: options.windowSeconds,
            per: options.per,
            cost: options.cost,
          }),
        ),
      );
    },
  };

  Object.defineProperty(globalThis, "rateLimit", {
    value: Object.freeze(api),
    writable: false,
    enumerable: true,
    configurable: false,
  });
})();
