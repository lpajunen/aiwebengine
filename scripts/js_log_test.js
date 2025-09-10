// JS test script: registers /js-log-test and uses writeLog

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

// register a route that writes a log entry when called
register('/js-log-test', (req) => {
  writeLog('js-log-test-called');
  return { status: 200, body: 'logged' };
});

// register a route that returns the current logs using listLogs()
register('/js-list', (req) => {
  try {
    const logs = listLogs();
    return { status: 200, body: JSON.stringify(logs) };
  } catch (e) {
    return { status: 500, body: String(e) };
  }
});
