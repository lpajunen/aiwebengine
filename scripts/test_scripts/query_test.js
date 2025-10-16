// Test script for query parameter handling
function query_handler(req) {
  let queryInfo = "none";
  if (req.query && Object.keys(req.query).length > 0) {
    // req.query is now an object with parsed parameters
    let params = [];
    for (let key in req.query) {
      params.push(`${key}=${req.query[key]}`);
    }
    queryInfo = params.join(", ");
  }

  return {
    status: 200,
    body: `Path: ${req.path}, Query: ${queryInfo}`,
    contentType: "text/plain",
  };
}

// Initialization function
function init(context) {
  writeLog("Initializing query_test.js at " + new Date().toISOString());
  register("/api/query", "query_handler", "GET");
  writeLog("Query test endpoint registered");
  return { success: true };
}
