// Test script for query parameter handling
function query_handler(path, req) {
    return {
        status: 200,
        body: `Path: ${path}, Query: ${req.query || 'none'}`
    };
}

// Register handler for query test
register('/api/query', 'query_handler', 'GET');