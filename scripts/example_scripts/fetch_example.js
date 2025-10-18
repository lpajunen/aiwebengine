// Example script demonstrating the fetch() API
// This script shows how to make HTTP requests to external APIs

function init(context) {
  writeLog("Initializing fetch_example.js");
  register("/fetch/example", "fetchExample", "GET");
  register("/fetch/with-secret", "fetchWithSecret", "GET");
  register("/fetch/post", "fetchPost", "POST");
  return { success: true };
}

// Example 1: Simple GET request
function fetchExample(req) {
  writeLog("Fetching data from httpbin.org");

  try {
    const responseJson = fetch("https://httpbin.org/get");
    const response = JSON.parse(responseJson);

    if (response.ok) {
      writeLog("Fetch successful! Status: " + response.status);
      return {
        status: 200,
        body: JSON.stringify({
          message: "Fetch successful",
          data: JSON.parse(response.body),
        }),
        contentType: "application/json",
      };
    } else {
      return {
        status: response.status,
        body: JSON.stringify({ error: "Request failed" }),
        contentType: "application/json",
      };
    }
  } catch (error) {
    writeLog("Fetch error: " + error);
    return {
      status: 500,
      body: JSON.stringify({ error: "Internal error: " + error }),
      contentType: "application/json",
    };
  }
}

// Example 2: Using secret injection for API keys
function fetchWithSecret(req) {
  writeLog("Fetching with secret injection");

  // Check if the secret exists
  if (!Secrets.exists("example_api_key")) {
    return {
      status: 503,
      body: JSON.stringify({
        error: "API key not configured",
        message: "Please set 'example_api_key' in secrets configuration",
      }),
      contentType: "application/json",
    };
  }

  try {
    // Use {{secret:identifier}} syntax to inject the API key
    const options = JSON.stringify({
      method: "GET",
      headers: {
        "X-API-Key": "{{secret:example_api_key}}",
        "User-Agent": "aiwebengine/fetch-example",
      },
    });

    // This would work with a real API that requires authentication
    // For demo purposes, we'll use httpbin
    const responseJson = fetch("https://httpbin.org/headers", options);
    const response = JSON.parse(responseJson);

    if (response.ok) {
      const data = JSON.parse(response.body);
      return {
        status: 200,
        body: JSON.stringify({
          message: "Request with secret successful",
          headers: data.headers,
        }),
        contentType: "application/json",
      };
    } else {
      return {
        status: response.status,
        body: JSON.stringify({ error: "Request failed" }),
        contentType: "application/json",
      };
    }
  } catch (error) {
    writeLog("Fetch error: " + error);
    return {
      status: 500,
      body: JSON.stringify({ error: "Internal error: " + error }),
      contentType: "application/json",
    };
  }
}

// Example 3: POST request with JSON body
function fetchPost(req) {
  writeLog("Making POST request");

  try {
    const requestData = {
      name: req.form.name || "Test User",
      email: req.form.email || "test@example.com",
      timestamp: new Date().toISOString(),
    };

    const options = JSON.stringify({
      method: "POST",
      headers: {
        "Content-Type": "application/json",
        Accept: "application/json",
      },
      body: JSON.stringify(requestData),
    });

    const responseJson = fetch("https://httpbin.org/post", options);
    const response = JSON.parse(responseJson);

    if (response.ok) {
      const data = JSON.parse(response.body);
      return {
        status: 200,
        body: JSON.stringify({
          message: "POST successful",
          sentData: requestData,
          echo: data.json,
        }),
        contentType: "application/json",
      };
    } else {
      return {
        status: response.status,
        body: JSON.stringify({ error: "POST failed" }),
        contentType: "application/json",
      };
    }
  } catch (error) {
    writeLog("POST error: " + error);
    return {
      status: 500,
      body: JSON.stringify({ error: "Internal error: " + error }),
      contentType: "application/json",
    };
  }
}
