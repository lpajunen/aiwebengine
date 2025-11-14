// Test script for executeGraphQL function
// Demonstrates how to execute GraphQL queries from JavaScript

function testGraphQLHandler(req) {
  try {
    console.log("Testing executeGraphQL function");

    // Example 1: Simple query without variables
    const query1 = `
      query {
        scripts {
          uri
          chars
        }
      }
    `;

    console.log("Executing GraphQL query: " + query1.trim());
    const result1 = graphQLRegistry.executeGraphQL(query1);
    console.log("Query result: " + result1);

    // Example 2: Query with variables (if supported)
    const query2 = `
      query GetScript($uri: String!) {
        script(uri: $uri) {
          uri
          content
          contentLength
        }
      }
    `;

    const variables = JSON.stringify({
      uri: "https://example.com/core",
    });

    console.log("Executing GraphQL query with variables");
    const result2 = graphQLRegistry.executeGraphQL(query2, variables);
    console.log("Query with variables result: " + result2);

    // Return results
    const response = {
      query1: JSON.parse(result1),
      query2: JSON.parse(result2),
    };

    return {
      status: 200,
      body: JSON.stringify(response, null, 2),
      contentType: "application/json",
    };
  } catch (error) {
    console.log("Error in testGraphQLHandler: " + error.message);
    return {
      status: 500,
      body: JSON.stringify({
        error: error.message,
        stack: error.stack,
      }),
      contentType: "application/json",
    };
  }
}

function init(context) {
  console.log("Initializing GraphQL test script");
  register("/test-graphql", "testGraphQLHandler", "GET");
  console.log("GraphQL test endpoint registered at /test-graphql");
  return { success: true };
}
