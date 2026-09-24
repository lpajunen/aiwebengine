// The JavaScript half of `crypto` — the cryptography a solution should not be
// writing for itself.
//
// Thin on purpose. `__hostCrypto` does the work and throws real errors of its
// own, so all this adds is the options object: `hmacVerify` takes one because
// five positional strings is the shape nobody reads back correctly six months
// later, and a host binding takes JSON.
//
// `secretName` rather than `secret` in both places, because the whole point of
// this API is that the value never enters JavaScript. A field called `secret`
// invites somebody to pass one.

(function () {
  var host = __hostCrypto;

  var api = {
    randomUUID: function () {
      return host.randomUUID();
    },

    randomToken: function (bytes, encoding) {
      return host.randomToken(
        bytes === undefined || bytes === null ? 32 : bytes,
        encoding || "hex",
      );
    },

    constantTimeEqual: function (a, b) {
      return host.constantTimeEqual(String(a == null ? "" : a), String(b == null ? "" : b));
    },

    secretEquals: function (secretName, candidate) {
      return host.secretEquals(
        String(secretName == null ? "" : secretName),
        String(candidate == null ? "" : candidate),
      );
    },

    hmacVerify: function (options) {
      if (!options || typeof options !== "object") {
        throw new Error("crypto.hmacVerify: expects an options object");
      }
      return host.hmacVerify(JSON.stringify(options));
    },
  };

  Object.defineProperty(globalThis, "crypto", {
    value: Object.freeze(api),
    writable: false,
    enumerable: true,
    configurable: false,
  });
})();
