// core script: registers root handler
register('/', (req) => ({ status: 200, body: 'Core handler: OK' }));
