/// <reference path="../../assets/aiwebengine.d.ts" />

// Test script demonstrating different HTTP methods
function get_handler(context) {
  const req = context.request || {};
  return { status: 200, body: `GET request to ${req.path}` };
}

function post_handler(context) {
  const req = context.request || {};
  return {
    status: 201,
    body: `POST request to ${req.path} with method ${req.method}`,
  };
}

function put_handler(context) {
  const req = context.request || {};
  return { status: 200, body: `PUT request to ${req.path}` };
}

function delete_handler(context) {
  return { status: 204, body: "" };
}

// Initialization function
function init(context) {
  console.log("Initializing method_test.js at " + new Date().toISOString());
  // Register handlers for different methods on the same path
  routeRegistry.registerRoute("/api/test", "get_handler", "GET");
  routeRegistry.registerRoute("/api/test", "post_handler", "POST");
  routeRegistry.registerRoute("/api/test", "put_handler", "PUT");
  routeRegistry.registerRoute("/api/test", "delete_handler", "DELETE");
  console.log("HTTP method test endpoints registered");
  return { success: true };
}
