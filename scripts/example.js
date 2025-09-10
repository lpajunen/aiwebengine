// Example QuickJS script to register routes
// register('/hello', (req) => ({ status: 200, body: 'Hello from JS!' }));

// register a sample route
function example_hello(req) { return { status: 200, body: `Hello from JS! method=${req.method}` }; }
register('/hello', 'example_hello');
