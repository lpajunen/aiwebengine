// The JavaScript half of `convert`.
//
// `__hostConvert` answers in the envelope every preluded global uses; this
// turns that into the converted text, or a thrown error when the conversion
// failed.

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

  function text(value, api) {
    if (typeof value !== "string") {
      throw new TypeError(api + ": expects a string");
    }
    return value;
  }

  var host = globalThis.__hostConvert;

  var convert = {
    markdown_to_html: function (markdown) {
      var api = "convert.markdown_to_html";
      return unwrap(host.markdown_to_html(text(markdown, api)), api);
    },

    // render_handlebars_template(template, data) — data is an object, or the
    // JSON text of one.
    render_handlebars_template: function (template, data) {
      var api = "convert.render_handlebars_template";
      var json =
        typeof data === "string"
          ? data
          : JSON.stringify(data === undefined ? {} : data);
      return unwrap(
        host.render_handlebars_template(text(template, api), json),
        api,
      );
    },

    btoa: function (data) {
      var api = "convert.btoa";
      return unwrap(host.btoa(text(data, api)), api);
    },

    atob: function (data) {
      var api = "convert.atob";
      return unwrap(host.atob(text(data, api)), api);
    },
  };

  Object.defineProperty(globalThis, "convert", {
    value: Object.freeze(convert),
    writable: false,
    enumerable: true,
    configurable: false,
  });

  try {
    delete globalThis.__hostConvert;
  } catch (e) {
    /* non-configurable in some contexts; the interface above is still what is documented */
  }
})();
