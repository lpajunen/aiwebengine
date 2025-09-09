// Example QuickJS script to register routes
// register('/hello', (req) => ({ status: 200, body: 'Hello from JS!' }));

const routes = new Map();

function register(path, handler) {
  routes.set(path, handler);
}

function handle(path, req) {
  const h = routes.get(path);
  if (!h) return { status: 404, body: 'Not found' };
  return h(req);
}

// export helpers
// register a sample route
register('/hello', (req) => ({ status: 200, body: `Hello from JS! method=${req.method}` }));

globalThis.register = register;
globalThis.handle = handle;
