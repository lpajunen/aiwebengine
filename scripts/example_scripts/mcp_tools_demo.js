// MCP (Model Context Protocol) Tools Demo
// This script demonstrates how to register and use MCP tools

// MCP tool handler for getting current time
function getCurrentTimeHandler(context) {
  const args = context.args || {};
  const timezone = args.timezone || "UTC";

  const now = new Date();
  const timeString = now.toLocaleString("en-US", { timeZone: timezone });

  return JSON.stringify({
    timestamp: now.toISOString(),
    timezone: timezone,
    formatted: timeString,
    unix: Math.floor(now.getTime() / 1000),
  });
}

// MCP tool handler for calculating simple math operations
function calculateHandler(context) {
  const args = context.args || {};
  const operation = args.operation;
  const a = parseFloat(args.a);
  const b = parseFloat(args.b);

  if (isNaN(a) || isNaN(b)) {
    return JSON.stringify({
      error: "Invalid numbers provided",
    });
  }

  let result;
  switch (operation) {
    case "add":
      result = a + b;
      break;
    case "subtract":
      result = a - b;
      break;
    case "multiply":
      result = a * b;
      break;
    case "divide":
      if (b === 0) {
        return JSON.stringify({
          error: "Cannot divide by zero",
        });
      }
      result = a / b;
      break;
    default:
      return JSON.stringify({
        error: "Unknown operation: " + operation,
      });
  }

  return JSON.stringify({
    operation: operation,
    a: a,
    b: b,
    result: result,
  });
}

// MCP tool handler for fetching weather information (simulated)
function getWeatherHandler(context) {
  const args = context.args || {};
  const location = args.location || "Unknown";

  // Simulate weather data (in real implementation, you'd call a weather API)
  const conditions = ["Sunny", "Cloudy", "Rainy", "Snowy"];
  const randomCondition =
    conditions[Math.floor(Math.random() * conditions.length)];
  const temperature = Math.floor(Math.random() * 30) + 10; // 10-40°C

  return JSON.stringify({
    location: location,
    condition: randomCondition,
    temperature: temperature,
    unit: "celsius",
    timestamp: new Date().toISOString(),
  });
}

// MCP tool handler for generating a random ID
function generateIdHandler(context) {
  const args = context.args || {};
  const prefix = args.prefix || "id";
  const length = args.length || 8;

  const chars =
    "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";
  let randomPart = "";
  for (let i = 0; i < length; i++) {
    randomPart += chars.charAt(Math.floor(Math.random() * chars.length));
  }

  return JSON.stringify({
    id: prefix + "-" + randomPart,
    timestamp: Date.now(),
  });
}

// Initialization function - called when script is loaded or updated
function init(context) {
  console.log("Initializing MCP tools demo at " + new Date().toISOString());

  // Register MCP tool: getCurrentTime
  const currentTimeSchema = JSON.stringify({
    type: "object",
    properties: {
      timezone: {
        type: "string",
        description:
          "IANA timezone (e.g., 'America/New_York', 'Europe/London')",
        default: "UTC",
      },
    },
  });

  mcpRegistry.registerTool(
    "getCurrentTime",
    "Get the current date and time in a specified timezone",
    currentTimeSchema,
    "getCurrentTimeHandler",
  );

  // Register MCP tool: calculate
  const calculateSchema = JSON.stringify({
    type: "object",
    properties: {
      operation: {
        type: "string",
        enum: ["add", "subtract", "multiply", "divide"],
        description: "Mathematical operation to perform",
      },
      a: {
        type: "number",
        description: "First operand",
      },
      b: {
        type: "number",
        description: "Second operand",
      },
    },
    required: ["operation", "a", "b"],
  });

  mcpRegistry.registerTool(
    "calculate",
    "Perform basic mathematical calculations",
    calculateSchema,
    "calculateHandler",
  );

  // Register MCP tool: getWeather
  const weatherSchema = JSON.stringify({
    type: "object",
    properties: {
      location: {
        type: "string",
        description: "City name or location",
      },
    },
    required: ["location"],
  });

  mcpRegistry.registerTool(
    "getWeather",
    "Get current weather information for a location (simulated data)",
    weatherSchema,
    "getWeatherHandler",
  );

  // Register MCP tool: generateId
  const generateIdSchema = JSON.stringify({
    type: "object",
    properties: {
      prefix: {
        type: "string",
        description: "Prefix for the generated ID",
        default: "id",
      },
      length: {
        type: "number",
        description: "Length of the random part",
        default: 8,
        minimum: 4,
        maximum: 32,
      },
    },
  });

  mcpRegistry.registerTool(
    "generateId",
    "Generate a random unique identifier with optional prefix",
    generateIdSchema,
    "generateIdHandler",
  );

  console.log("MCP tools demo script initialized successfully");
  console.log(
    "Registered 4 MCP tools: getCurrentTime, calculate, getWeather, generateId",
  );

  return {
    success: true,
    message: "MCP tools demo initialized",
    tools: ["getCurrentTime", "calculate", "getWeather", "generateId"],
  };
}
