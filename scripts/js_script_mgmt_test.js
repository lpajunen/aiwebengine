// JS script for testing script-management host functions

// rely on bootstrap-provided register/handle which supports handler name strings

// upsert a script from JS
try {
  upsertScript(
    'https://example.com/from_js',
    "function from_js_handler(req) { return { status: 200, body: 'from-js' }; }\nregister('/from-js', 'from_js_handler');"
  );
} catch (e) {
  // ignore errors when host function not available
}

// route that exercises getScript, listScripts, deleteScript
function js_mgmt_check(req) {
  try {
    const got = (typeof getScript === 'function') ? (getScript('https://example.com/from_js') ?? null) : null;
    const list = (typeof listScripts === 'function') ? (listScripts() ?? []) : [];
    const deleted_before = (typeof deleteScript === 'function') ? !!deleteScript('https://example.com/does-not-exist') : false;
    const deleted = (typeof deleteScript === 'function') ? !!deleteScript('https://example.com/from_js') : false;
    const after = (typeof getScript === 'function') ? (getScript('https://example.com/from_js') ?? null) : null;
    return {
      status: 200,
      body: JSON.stringify({ got, list, deleted_before, deleted, after }),
    };
  } catch (e) {
    return { status: 500, body: String(e) };
  }
}
register('/js-mgmt-check', 'js_mgmt_check');
