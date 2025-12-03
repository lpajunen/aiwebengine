// MCP (Model Context Protocol) Prompts Demo
// This script demonstrates how to register MCP prompts for common development tasks

// Initialization function - called when script is loaded or updated
function init(context) {
  console.log("Initializing MCP prompts demo at " + new Date().toISOString());

  // Prompt 1: Create REST API Endpoint
  mcpRegistry.registerPrompt(
    "create_rest_endpoint",
    "Generate a complete REST API endpoint with handler function and route registration. This creates a new HTTP endpoint that can handle GET/POST/PUT/DELETE requests with proper error handling and JSON responses.",
    JSON.stringify([
      {
        name: "resourceName",
        description: "The resource name (e.g., 'users', 'products', 'orders')",
        required: true,
      },
      {
        name: "method",
        description: "HTTP method (GET, POST, PUT, DELETE)",
        required: true,
      },
      {
        name: "path",
        description: "The URL path (e.g., '/api/users', '/products/:id')",
        required: true,
      },
      {
        name: "description",
        description: "Brief description of what this endpoint does",
        required: false,
      },
    ]),
    "create_rest_endpoint", // Handler function name
  );

  // Prompt 2: Add GraphQL Query
  mcpRegistry.registerPrompt(
    "add_graphql_query",
    "Generate a GraphQL query with schema definition and resolver function. This creates a new GraphQL query that can be accessed via the /graphql endpoint with proper type definitions and data fetching logic.",
    JSON.stringify([
      {
        name: "queryName",
        description: "The GraphQL query name (e.g., 'getUser', 'listProducts')",
        required: true,
      },
      {
        name: "returnType",
        description:
          "The return type description (e.g., 'User object with id, name, email')",
        required: true,
      },
      {
        name: "arguments",
        description: "Query arguments (e.g., 'id: String!, limit: Int')",
        required: false,
      },
    ]),
    "add_graphql_query", // Handler function name
  );

  console.log("MCP prompts demo script initialized successfully");
  console.log("Registered 2 MCP prompts for common development tasks");

  return {
    success: true,
    message: "MCP prompts demo initialized",
    prompts: ["create_rest_endpoint", "add_graphql_query"],
  };
}

// Handler for create_rest_endpoint prompt
function create_rest_endpoint(args) {
  const resourceName = args.resourceName || "resource";
  const method = args.method || "GET";
  const path = args.path || `/api/${resourceName}`;
  const description = args.description || `${method} ${resourceName}`;

  const code = `
// ${description}
function handle${resourceName}${method}(request) {
  console.log("${method} ${path} called");
  
  // TODO: Implement ${resourceName} ${method} logic here
  
  return {
    success: true,
    data: []
  };
}

// Register the endpoint
endpoints.register("${method} ${path}", handle${resourceName}${method});
console.log("Registered ${method} ${path}");
  `.trim();

  return {
    messages: [
      {
        role: "user",
        content: {
          type: "text",
          text: `Create a ${method} endpoint at ${path} for ${resourceName}`,
        },
      },
      {
        role: "assistant",
        content: {
          type: "text",
          text: code,
        },
      },
    ],
  };
}

// Handler for add_graphql_query prompt
function add_graphql_query(args) {
  const queryName = args.queryName || "myQuery";
  const returnType = args.returnType || "String";
  const queryArgs = args.arguments || "";

  const argsStr = queryArgs ? `(${queryArgs})` : "";

  const code = `
// GraphQL query: ${queryName}
const ${queryName}Schema = \`
  type Query {
    ${queryName}${argsStr}: ${returnType}
  }
\`;

function ${queryName}Resolver(args, context) {
  console.log("GraphQL query ${queryName} called with:", args);
  
  // TODO: Implement query logic
  
  return {
    success: true,
    data: null
  };
}

// Register the query
graphqlRegistry.registerQuery("${queryName}", ${queryName}Schema, ${queryName}Resolver);
console.log("Registered GraphQL query: ${queryName}");
  `.trim();

  return {
    messages: [
      {
        role: "user",
        content: {
          type: "text",
          text: `Create GraphQL query ${queryName} that returns ${returnType}`,
        },
      },
      {
        role: "assistant",
        content: {
          type: "text",
          text: code,
        },
      },
    ],
  };
}
