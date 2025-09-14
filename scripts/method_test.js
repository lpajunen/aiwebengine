// Test script demonstrating different HTTP methods
function get_handler(req) {
    return { status: 200, body: `GET request to ${req.path}` };
}

function post_handler(req) {
    return { status: 201, body: `POST request to ${req.path} with method ${req.method}` };
}

function put_handler(req) {
    return { status: 200, body: `PUT request to ${req.path}` };
}

function delete_handler(req) {
    return { status: 204, body: '' };
}

// Register handlers for different methods on the same path
register('/api/test', 'get_handler', 'GET');
register('/api/test', 'post_handler', 'POST');
register('/api/test', 'put_handler', 'PUT');
register('/api/test', 'delete_handler', 'DELETE');