# MCP Prompts Guide

This guide explains how to create and use MCP (Model Context Protocol) prompts in the AI Web Engine.

## Table of Contents

- [Quick Start](#quick-start)
- [What are MCP Prompts?](#what-are-mcp-prompts)
- [Prompts vs Tools](#prompts-vs-tools)
- [Registering Prompts](#registering-prompts)
- [Prompt Arguments](#prompt-arguments)
- [Testing Prompts](#testing-prompts)
- [Best Practices](#best-practices)
- [Examples](#examples)

## Quick Start

```javascript
// In your script's init() function
function init(context) {
  // Register a prompt with arguments
  mcpRegistry.registerPrompt(
    "create_rest_endpoint",
    "Generate a complete REST API endpoint with handler and route",
    JSON.stringify([
      {
        name: "resourceName",
        description: "The resource name (e.g., 'users', 'products')",
        required: true,
      },
      {
        name: "method",
        description: "HTTP method (GET, POST, PUT, DELETE)",
        required: true,
      },
    ]),
    "create_rest_endpoint", // Handler function name
  );

  return { success: true };
}

// Implement the prompt handler (same name as prompt)
function create_rest_endpoint(args) {
  const { resourceName, method } = args;

  // Generate the code template
  const code = `
// ${method} handler for ${resourceName}
function handle${resourceName}${method}(req, res) {
  // TODO: Implement ${resourceName} ${method} logic
  return {
    success: true,
    data: {}
  };
}

// Register the endpoint
endpoints.register("${method} /api/${resourceName}", handle${resourceName}${method});
  `.trim();

  return {
    messages: [
      {
        role: "user",
        content: {
          type: "text",
          text: `Create a ${method} endpoint for ${resourceName}`,
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
```

## What are MCP Prompts?

MCP prompts are **reusable templates** that help AI assistants generate code or content based on user input. Unlike MCP tools which perform actions, prompts:

- Provide structured workflows for common tasks
- Help AI assistants understand what code to generate
- Define clear inputs (arguments) needed for generation
- Return messages that guide the conversation

Think of prompts as "code recipes" that AI assistants can follow to create consistent, high-quality output.

## Prompts vs Tools

| Feature     | **Tools**                                         | **Prompts**                                            |
| ----------- | ------------------------------------------------- | ------------------------------------------------------ |
| Purpose     | Execute actions                                   | Generate templates                                     |
| Returns     | Data/results                                      | Conversation messages                                  |
| When to use | Reading files, modifying data, running operations | Creating code, providing workflows, guiding generation |
| Example     | `read_file`, `write_file`, `search_files`         | `create_rest_endpoint`, `add_graphql_query`            |

**Use Tools when you need to:**

- Read or write files
- Query databases
- Perform calculations
- Execute operations

**Use Prompts when you need to:**

- Generate code templates
- Provide structured workflows
- Guide AI through complex tasks
- Offer reusable patterns

## Registering Prompts

### Basic Registration

```javascript
mcpRegistry.registerPrompt(name, description, argumentsJson, handlerFunction);
```

**Parameters:**

- `name` (string, required): Unique identifier (1-100 characters, alphanumeric + underscores)
- `description` (string, required): What the prompt generates (1-1000 characters)
- `argumentsJson` (string, required): JSON array of argument definitions
- `handlerFunction` (string, required): Name of the JavaScript function that generates messages

**Handler Function:**

- Named as specified in the registration
- Receives **context object** with mode and arguments
- In **prompt mode** (`context.mode === "prompt"`): returns messages array
- In **completion mode** (`context.mode === "completion"`): returns completion suggestions
- Has access to all standard APIs (console, sharedStorage, fetch, etc.)

**Context Object Structure:**

```javascript
// Prompt mode (prompts/get)
{
  mode: "prompt",
  arguments: { /* all provided arguments */ }
}

// Completion mode (completion/complete)
{
  mode: "completion",
  completingArgument: "argumentName",  // which argument is being completed
  partialValue: "partial text",        // what user has typed so far
  arguments: { /* previously completed arguments */ }
}
```

### Security Requirements

- Requires `ManageGraphQL` capability
- All registrations are audit-logged
- Script URI is automatically tracked
- Prompts are cleared when scripts update

## Prompt Arguments

Arguments define what information the prompt needs to generate content.

### Argument Structure

```javascript
{
  name: "argumentName",      // Required: parameter name
  description: "what it is", // Required: clear explanation
  required: true             // Required: whether it's mandatory
}
```

### Example with Multiple Arguments

```javascript
mcpRegistry.registerPrompt(
  "create_form_handler",
  "Generate an HTML form with POST handler",
  JSON.stringify([
    {
      name: "formName",
      description: "The form name (e.g., 'contact', 'registration')",
      required: true,
    },
    {
      name: "fields",
      description: "Comma-separated field names (e.g., 'name, email, message')",
      required: true,
    },
    {
      name: "submitPath",
      description: "The form submission path",
      required: false,
    },
  ]),
);
```

## Testing Prompts

### 1. List Available Prompts

```bash
curl -X POST https://softagen.com/mcp \
  -H "Content-Type: application/json" \
  -d '{
    "jsonrpc": "2.0",
    "id": 1,
    "method": "prompts/list"
  }'
```

**Response:**

```json
{
  "jsonrpc": "2.0",
  "id": 1,
  "result": {
    "prompts": [
      {
        "name": "create_rest_endpoint",
        "description": "Generate a complete REST API endpoint",
        "arguments": [
          {
            "name": "resourceName",
            "description": "The resource name",
            "required": true
          }
        ]
      }
    ]
  }
}
```

### 2. Get Prompt with Arguments

```bash
curl -X POST https://softagen.com/mcp \
  -H "Content-Type: application/json" \
  -d '{
    "jsonrpc": "2.0",
    "id": 2,
    "method": "prompts/get",
    "params": {
      "name": "create_rest_endpoint",
      "arguments": {
        "resourceName": "products",
        "method": "GET"
      }
    }
  }'
```

**Response:**

```json
{
  "jsonrpc": "2.0",
  "id": 2,
  "result": {
    "messages": [
      {
        "role": "user",
        "content": {
          "type": "text",
          "text": "Create a GET endpoint for products"
        }
      },
      {
        "role": "assistant",
        "content": {
          "type": "text",
          "text": "// GET handler for products\nfunction handleProductsGET(req, res) ..."
        }
      }
    ]
  }
}
```

### 3. Test Completions

```bash
curl -X POST https://softagen.com/mcp \
  -H "Content-Type: application/json" \
  -d '{
    "jsonrpc": "2.0",
    "id": 3,
    "method": "completion/complete",
    "params": {
      "ref": {
        "type": "ref/prompt",
        "name": "create_rest_endpoint"
      },
      "argument": {
        "name": "method",
        "value": "P"
      }
    }
  }'
```

**Response:**

```json
{
  "jsonrpc": "2.0",
  "id": 3,
  "result": {
    "completion": {
      "values": ["POST", "PUT", "PATCH"],
      "total": 3,
      "hasMore": false
    }
  }
}
```

### 4. Test via VS Code MCP Client

Configure `.vscode/mcp.json`:

```json
{
  "mcpServers": {
    "aiwebengine": {
      "url": "https://softagen.com/mcp"
    }
  }
}
```

Then in AI assistant:

1. Ask to list available prompts
2. Use a prompt with arguments
3. Review generated code

## Best Practices

### 1. Write Clear Descriptions

**Good:**

```javascript
"Generate a REST API endpoint with handler function, route registration, error handling, and JSON response formatting";
```

**Bad:**

```javascript
"Makes an endpoint";
```

### 2. Define Precise Arguments

**Good:**

```javascript
{
  name: "method",
  description: "HTTP method (GET, POST, PUT, DELETE)",
  required: true
}
```

**Bad:**

```javascript
{
  name: "type",
  description: "The type",
  required: true
}
```

### 3. Provide Examples in Descriptions

```javascript
{
  name: "fields",
  description: "Comma-separated field names (e.g., 'name, email, message')",
  required: true
}
```

### 4. Support Both Prompt and Completion Modes

```javascript
function myPromptHandler(context) {
  // Handle completion mode
  if (context.mode === "completion") {
    const { completingArgument, partialValue, arguments: args } = context;

    if (completingArgument === "method") {
      const methods = ["GET", "POST", "PUT", "DELETE"];
      const filtered = methods.filter((m) =>
        m.toLowerCase().startsWith(partialValue.toLowerCase()),
      );
      return {
        values: filtered,
        total: filtered.length,
        hasMore: false,
      };
    }

    return { values: [], total: 0, hasMore: false };
  }

  // Handle prompt mode
  const { arguments: args } = context;
  return {
    messages: [
      {
        role: "user",
        content: {
          type: "text",
          text: `User's request using ${args.param}`,
        },
      },
      {
        role: "assistant",
        content: {
          type: "text",
          text: `Generated code or explanation`,
        },
      },
    ],
  };
}
```

### 5. Validate Arguments

```javascript
function myPromptHandler(context) {
  // Skip validation in completion mode
  if (context.mode === "completion") {
    // Handle completions...
    return { values: [], total: 0, hasMore: false };
  }

  const { arguments: args } = context;

  if (!args.required_param) {
    throw new Error("required_param is missing");
  }

  const validMethods = ["GET", "POST", "PUT", "DELETE"];
  if (!validMethods.includes(args.method)) {
    throw new Error(
      `Invalid method. Must be one of: ${validMethods.join(", ")}`,
    );
  }

  // Generate content...
}
```

## Examples

### Example 1: REST Endpoint Generator

```javascript
// Register prompt
mcpRegistry.registerPrompt(
  "create_rest_endpoint",
  "Generate a REST API endpoint with handler and route registration",
  JSON.stringify([
    {
      name: "resourceName",
      description: "Resource name (e.g., 'users', 'products')",
      required: true,
    },
    {
      name: "method",
      description: "HTTP method (GET, POST, PUT, DELETE)",
      required: true,
    },
    {
      name: "path",
      description: "URL path (e.g., '/api/users')",
      required: true,
    },
  ]),
  "create_rest_endpoint", // Handler function name
);

// Handler function (same name as prompt)
function create_rest_endpoint(args) {
  const { resourceName, method, path } = args;

  const code = `
// ${method} handler for ${resourceName}
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
```

### Example 2: GraphQL Query Generator

```javascript
// Register prompt
mcpRegistry.registerPrompt(
  "add_graphql_query",
  "Generate a GraphQL query with schema and resolver",
  JSON.stringify([
    {
      name: "queryName",
      description: "Query name (e.g., 'getUser', 'listProducts')",
      required: true,
    },
    {
      name: "returnType",
      description: "Return type description",
      required: true,
    },
    {
      name: "arguments",
      description: "Query arguments (e.g., 'id: String!')",
      required: false,
    },
  ]),
  "add_graphql_query", // Handler function name
);

// Handler function (same name as prompt)
function add_graphql_query(args) {
  const { queryName, returnType, arguments: queryArgs } = args;

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
```

### Example 3: Form Handler Generator

```javascript
// Register prompt
mcpRegistry.registerPrompt(
  "create_form_handler",
  "Generate HTML form with POST handler",
  JSON.stringify([
    {
      name: "formName",
      description: "Form name (e.g., 'contact', 'registration')",
      required: true,
    },
    {
      name: "fields",
      description: "Comma-separated fields (e.g., 'name, email, message')",
      required: true,
    },
    {
      name: "submitPath",
      description: "Form submission path (e.g., '/submit_contact')",
      required: true,
    },
  ]),
  "create_form_handler", // Handler function name
);

// Handler function (same name as prompt)
function create_form_handler(args) {
  const { formName, fields, submitPath } = args;
  const fieldList = fields.split(",").map((f) => f.trim());

  const htmlFields = fieldList
    .map((field) => {
      const label = field.charAt(0).toUpperCase() + field.slice(1);
      if (field === "message") {
        return `
    <label for="${field}">${label}:</label>
    <textarea id="${field}" name="${field}" required></textarea>`;
      }
      return `
    <label for="${field}">${label}:</label>
    <input type="${field === "email" ? "email" : "text"}" id="${field}" name="${field}" required>`;
    })
    .join("\n");

  const code = `
// ${formName} form handler
function render${formName}Form() {
  return \`
    <form method="POST" action="${submitPath}">
      <h2>${formName.charAt(0).toUpperCase() + formName.slice(1)} Form</h2>
      ${htmlFields}
      <button type="submit">Submit</button>
    </form>
  \`;
}

function handle${formName}Submit(request) {
  const formData = request.formData;
  
  // Validate fields
  ${fieldList
    .map(
      (f) => `
  if (!formData.${f}) {
    return { error: "${f} is required" };
  }`,
    )
    .join("")}
  
  // TODO: Process form submission
  console.log("${formName} form submitted:", formData);
  
  return {
    success: true,
    message: "Form submitted successfully"
  };
}

// Register endpoints
endpoints.register("GET /${formName}", render${formName}Form);
endpoints.register("POST ${submitPath}", handle${formName}Submit);
  `.trim();

  return {
    messages: [
      {
        role: "user",
        content: {
          type: "text",
          text: `Create a ${formName} form with fields: ${fields}`,
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
```

## See Also

- [MCP Tools Guide](mcp-tools.md) - Learn about MCP tools for actions
- [Quick Start Guide](quick-start.md) - Get started with AI Web Engine
- [JavaScript API Reference](../reference/javascript-api.md) - Full API documentation
