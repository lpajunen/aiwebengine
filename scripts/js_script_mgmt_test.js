// JS script for testing script-management host functions

function register(path, handler) {
  globalThis._routes = globalThis._routes || new Map();
  globalThis._routes.set(path, handler);
}

function handle(path, req) {
  const h = globalThis._routes && globalThis._routes.get(path);
  if (!h) return { status: 404, body: 'Not found' };
  return h(req);
}

globalThis.register = register;
globalThis.handle = handle;

// upsert a script from JS
try {
  upsertScript(
    'https://example.com/from_js',
    "register('/from-js', (req) => ({ status: 200, body: 'from-js' }));"
  );
} catch (e) {
  // ignore errors when host function not available
}

// route that exercises getScript, listScripts, deleteScript
register('/js-mgmt-check', (req) => {
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
});
