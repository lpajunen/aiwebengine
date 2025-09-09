// helloworld script: registers /hello
register('/hello', (req) => ({ status: 200, body: 'Hello from external script!' }));
