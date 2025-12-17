/// <reference path="../../assets/aiwebengine.d.ts" />

// Example script demonstrating the fetch() API
// This script shows how to make HTTP requests to external APIs

/**
 * Initialize fetch example routes
 * @param {HandlerContext} context - Initialization context
 * @returns {{success: boolean}} Initialization result
 */
function init(context) {
  console.log("Initializing fetch_example.js");
  routeRegistry.registerRoute("/fetch/example", "fetchExample", "GET");
  routeRegistry.registerRoute("/fetch/with-secret", "fetchWithSecret", "GET");
  routeRegistry.registerRoute("/fetch/post", "fetchPost", "POST");
  return { success: true };
}

// Example 1: Simple GET request
function fetchExample(context) {
  const req = context.request;

  // Validate query parameters using new validation helpers
  const url = validate.requireQueryParam(
    req,
    "url",
    "URL parameter is required",
  );
  if (!url.valid) {
    return Response.error(400, url.error);
  }

  console.log("Fetching data from: " + url.value);

  try {
    const responseJson = fetch(url.value);
    const response = JSON.parse(responseJson);

    if (response.ok) {
      console.log("Fetch successful! Status: " + response.status);
      return Response.json({
        message: "Fetch successful",
        data: JSON.parse(response.body),
      });
    } else {
      return Response.error(response.status, "Request failed");
    }
  } catch (error) {
    console.error("Fetch error: " + error);
    return Response.error(500, "Internal error: " + error);
  }
}

// Example 2: Using secret injection for API keys
function fetchWithSecret(context) {
  console.log("Fetching with secret injection");

  // Check if the secret exists
  if (!secretStorage.exists("example_api_key")) {
    return Response.error(
      503,
      "API key not configured. Please set 'example_api_key' in secrets configuration",
    );
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
      return Response.json({
        message: "Request with secret successful",
        headers: data.headers,
      });
    } else {
      return Response.error(response.status, "Request failed");
    }
  } catch (error) {
    console.error("Fetch error: " + error);
    return Response.error(500, "Internal error: " + error);
  }
}

// Example 3: POST request with JSON body
function fetchPost(context) {
  const req = context.request;
  console.log("Making POST request");

  // Validate required form parameters
  const name = validate.requireQueryParam(
    req,
    "name",
    "Name parameter is required",
  );
  if (!name.valid) {
    return Response.error(400, name.error);
  }

  const email = validate.requireQueryParam(
    req,
    "email",
    "Email parameter is required",
  );
  if (!email.valid) {
    return Response.error(400, email.error);
  }

  try {
    const requestData = {
      name: name.value,
      email: email.value,
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
      return Response.json({
        message: "POST successful",
        sentData: requestData,
        echo: data.json,
      });
    } else {
      return Response.error(response.status, "POST failed");
    }
  } catch (error) {
    console.error("POST error: " + error);
    return Response.error(500, "Internal error: " + error);
  }
}
