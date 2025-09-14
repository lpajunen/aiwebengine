// Test script for query parameter handling
function query_handler(req) {
    return {
        status: 200,
        body: `Path: ${req.path}, Query: ${req.query || 'none'}`
    };
}

// Register handler for query test
register('/api/query', 'query_handler', 'GET');