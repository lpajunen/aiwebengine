/// <reference path="../../assets/aiwebengine.d.ts" />

// Test script to verify GraphQL subscription schema configuration
// This is a server-side script that uses the GraphQL introspection API

function init(context) {
  // Register test route
  routeRegistry.registerRoute(
    "/test/subscription-schema",
    "testSubscriptionSchema",
    "GET",
  );
  console.log("Subscription schema test initialized at /test/subscription-schema");
}

function testSubscriptionSchema(context) {
  const introspectionQuery = `
    query IntrospectionQuery {
      __schema {
        subscriptionType {
          name
          fields {
            name
            type {
              name
            }
          }
        }
      }
    }
  `;

  try {
    // Execute GraphQL introspection query
    const resultJson = graphQLRegistry.executeGraphQL(introspectionQuery, "{}");
    const result = JSON.parse(resultJson);

    let output = "GraphQL Subscription Schema Test\n";
    output += "=================================\n\n";

    if (result.errors) {
      output += "❌ GraphQL Errors:\n";
      result.errors.forEach((error) => {
        output += `  - ${error.message}\n`;
      });
      return ResponseBuilder.text(output);
    }

    if (result.data && result.data.__schema && result.data.__schema.subscriptionType) {
      output += "✅ GraphQL subscription type is configured!\n\n";
      output += `Subscription type name: ${result.data.__schema.subscriptionType.name}\n\n`;
      output += "Available subscription fields:\n";
      result.data.__schema.subscriptionType.fields.forEach((field) => {
        output += `  - ${field.name}: ${field.type.name}\n`;
      });
    } else {
      output += "❌ GraphQL subscription type is NOT configured\n";
    }

    return ResponseBuilder.text(output);
  } catch (error) {
    return ResponseBuilder.error(500, "Request failed: " + error.toString());
  }
}
