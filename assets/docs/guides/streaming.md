# Real-Time Streaming Guide

This guide covers aiwebengine's real-time streaming capabilities using Server-Sent Events (SSE). Learn how to build live, interactive applications that push updates to clients in real-time.

## Table of Contents

- [Overview](#overview)
- [Architecture](#architecture)
- [Quick Start](#quick-start)
- [API Reference](#api-reference)
- [Selective Broadcasting](#selective-broadcasting)
- [Client Integration](#client-integration)
- [Use Cases](#use-cases)
- [Best Practices](#best-practices)
- [Troubleshooting](#troubleshooting)
- [Advanced Topics](#advanced-topics)
- [GraphQL Subscriptions with Selective Broadcasting](#graphql-subscriptions-with-selective-broadcasting)
- [Next Steps](#next-steps)

## Overview

aiwebengine provides built-in support for real-time streaming through Server-Sent Events (SSE). This allows you to:

- Push live updates to connected clients
- Build real-time dashboards and notifications
- Create chat systems and collaborative tools
- Stream live data updates without polling

**Key Features:**

- **Simple API**: Just two JavaScript functions to get started
- **Multi-client support**: Broadcast to multiple connected clients simultaneously
- **Automatic cleanup**: Connections are managed automatically
- **Standard protocol**: Uses SSE, compatible with EventSource API
- **No external dependencies**: Built into the core engine

## Architecture

### Components

1. **Stream Registry**: Manages registered stream paths and active connections
2. **Connection Manager**: Handles client connections and cleanup
3. **JavaScript Engine Integration**: Provides `routeRegistry.registerStreamRoute()` and `routeRegistry.sendStreamMessage()` functions
4. **SSE Server**: Handles HTTP connections and message broadcasting

### Flow Diagram

```text
JavaScript Script          Stream Registry          Connected Clients
     |                          |                         |
     | routeRegistry.           |                         |
     | registerStreamRoute()    |                         |
     |------------------------->|                         |
     |                          |                         |
     |                          | <--- Client connects ---|
     |                          |                         |
     | routeRegistry.           |                         |
     | sendStreamMessage()      |                         |
     |------------------------->|                         |
     |                          |---- Send to specific -->|
     |                          |---- stream clients ---->|
```

### Connection Lifecycle

1. **Registration**: Script calls `routeRegistry.registerStreamRoute('/path')` to create a stream endpoint
2. **Client Connection**: Browser connects using `new EventSource('/path')`
3. **Broadcasting**: Script calls `routeRegistry.sendStreamMessage('/path', data)` or `routeRegistry.sendStreamMessageFiltered('/path', data, filterJson)` to send data to clients
4. **Cleanup**: Connections automatically cleaned up when clients disconnect

## Quick Start

### 1. Basic Stream Setup

```javascript
// Register a stream endpoint
routeRegistry.registerStreamRoute("/events");

// Handler to send events
function triggerEvent(context) {
  routeRegistry.sendStreamMessage("/events", {
    type: "event",
    message: "Something happened!",
    timestamp: new Date().toISOString(),
  });

  return { status: 200, body: "Event sent" };
}

routeRegistry.registerRoute("/trigger", "triggerEvent", "POST");
```

### 2. Client-Side Connection

```html
<!DOCTYPE html>
<html>
  <head>
    <title>Stream Example</title>
  </head>
  <body>
    <div id="events"></div>

    <script>
      const eventSource = new EventSource("/events");

      eventSource.onmessage = function (event) {
        const data = JSON.parse(event.data);
        document.getElementById("events").innerHTML +=
          "<p>" + data.message + " at " + data.timestamp + "</p>";
      };

      eventSource.onerror = function (event) {
        console.error("Stream error:", event);
      };
    </script>
  </body>
</html>
```

### 3. Test the Stream

1. Load the HTML page in your browser
2. Send a POST request to `/trigger`
3. See the event appear in real-time!

## API Reference

### routeRegistry Stream Functions

#### routeRegistry.registerStreamRoute(path)

Registers a Server-Sent Events endpoint that clients can connect to.

**Parameters:**

- `path` (string): Stream path (must start with `/`, max 200 characters)

**Returns:** String describing registration result

**Throws:** Error if path is invalid or registration fails

**Example:**

```javascript
routeRegistry.registerStreamRoute("/notifications");
routeRegistry.registerStreamRoute("/chat/room1");
routeRegistry.registerStreamRoute("/status/server1");
```

**Path Requirements:**

- Must start with `/`
- Maximum 200 characters
- Should be unique per script
- Case-sensitive

#### routeRegistry.sendStreamMessage(path, data)

Sends a message to all clients connected to a specific stream path.

**Parameters:**

- `path` (string): Stream path to send to (must start with `/`)
- `data` (object): Data to send (will be JSON serialized)

**Returns:** String describing broadcast result

**Example:**

```javascript
routeRegistry.sendStreamMessage("/notifications", {
  type: "notification",
  title: "New Message",
  body: "You have a new message",
  timestamp: new Date().toISOString(),
  priority: "high",
});
```

**Message Structure Best Practices:**

- Include a `type` field to categorize messages
- Add timestamps for ordering
- Use consistent field names across your application
- Keep messages reasonably sized (< 1MB recommended)

#### routeRegistry.sendStreamMessageFiltered(path, data, filterJson) [Advanced]

Sends a message to specific clients connected to a stream path based on connection metadata filtering.

**Parameters:**

- `path` (string): Stream path to send to (must start with `/`)
- `data` (object): Data to send (will be JSON serialized)
- `filterJson` (string, optional): JSON string containing metadata filter criteria. Empty string `{}` matches all connections.

**Returns:** String describing the broadcast result including success/failure counts

**Example:**

```javascript
// Send to connections where user_id matches "user123" and room is "general"
routeRegistry.sendStreamMessageFiltered(
  "/chat",
  {
    type: "chat_message",
    message: "Hello everyone in general!",
    timestamp: new Date().toISOString(),
  },
  JSON.stringify({
    user_id: "user123",
    room: "general",
  }),
);

// Send to all connections (empty filter)
routeRegistry.sendStreamMessageFiltered(
  "/notifications",
  {
    type: "system_alert",
    message: "System maintenance starting",
  },
  "{}",
);
```

**Filter Format:**

The `filterJson` parameter should be a JSON string containing key-value pairs that must match the connection's metadata:

```javascript
// Filter examples
JSON.stringify({ user_id: "user123" }); // Match specific user
JSON.stringify({ room: "general", role: "admin" }); // Match room AND role
JSON.stringify({}); // Match all connections (empty filter)
```

**Use Cases:**

- **Chat Applications**: Send messages only to users in specific rooms or with certain permissions
- **User-Specific Notifications**: Deliver personalized content to individual users
- **Role-Based Broadcasting**: Send different messages based on user roles or groups
- **Multi-Tenant Systems**: Filter by tenant, organization, or workspace

#### graphQLRegistry.sendSubscriptionMessageFiltered(subscriptionName, data, filterJson) [Advanced]

Sends a message to specific clients subscribed to a GraphQL subscription based on connection metadata filtering.

**Parameters:**

- `subscriptionName` (string): Name of the GraphQL subscription
- `data` (string): JSON string containing the data to send to subscribers
- `filterJson` (string, optional): JSON string containing metadata filter criteria. Empty string `{}` matches all connections.

**Returns:** String describing the broadcast result including success/failure counts

**Example:**

```javascript
// Register GraphQL subscription first
graphQLRegistry.registerSubscription(
  "chatMessages",
  "type Subscription { chatMessages: String }",
  "chatMessagesResolver",
);

// Send to connections where user_id matches "user456"
graphQLRegistry.sendSubscriptionMessageFiltered(
  "chatMessages",
  JSON.stringify({
    type: "chat_message",
    message: "Private message for you!",
    timestamp: new Date().toISOString(),
  }),
  JSON.stringify({
    user_id: "user456",
  }),
);
```

**Note:** This function works with GraphQL subscriptions and uses the same metadata filtering as `sendStreamMessageToConnections`.

### Stream Management

Streams are automatically managed by the aiwebengine:

- **Registration**: Streams persist until server restart or script reload
- **Connections**: Multiple clients can connect to the same stream
- **Broadcasting**: Messages sent to clients connected to the specified stream path
- **Cleanup**: Stale connections automatically removed

## Selective Broadcasting

Selective broadcasting allows you to send messages to specific clients based on connection metadata, enabling personalized content delivery on stable endpoints without creating dynamic user-specific paths.

### How It Works

1. **Connection Metadata**: Each client connection can have associated metadata (key-value pairs)
2. **Filtering**: When broadcasting, you provide filter criteria that match against this metadata
3. **Targeted Delivery**: Only connections whose metadata matches the filter receive the message

### Benefits

- **Stable Endpoints**: Use one endpoint instead of many user-specific paths
- **Scalability**: Avoid endpoint proliferation and memory leaks
- **Personalization**: Deliver relevant content to specific users or groups
- **Security**: Filter based on user permissions, roles, or context

### Quick Example

```javascript
// Register one stream for all chat messages
routeRegistry.registerStreamRoute("/chat");

// Send personalized messages using metadata filtering
function sendPersonalMessage(context) {
  const req = context.request;
  const { targetUser, message } = req.form;

  // Send only to connections where user_id matches targetUser
  const result = sendStreamMessageToConnections(
    "/chat",
    {
      type: "personal_message",
      message: message,
      from: "system",
      timestamp: new Date().toISOString(),
    },
    JSON.stringify({
      user_id: targetUser,
    }),
  );

  return { status: 200, body: result };
}

routeRegistry.registerRoute("/chat/personal", "sendPersonalMessage", "POST");
```

### Advanced Filtering

```javascript
// Multiple filter criteria (AND logic)
sendStreamMessageToConnections(
  "/notifications",
  data,
  JSON.stringify({
    user_id: "user123",
    role: "admin",
    department: "engineering",
  }),
);

// Room-based chat filtering
sendStreamMessageToConnections(
  "/chat",
  data,
  JSON.stringify({
    room: "general",
  }),
);

// Tenant-based filtering for multi-tenant apps
sendStreamMessageToConnections(
  "/updates",
  data,
  JSON.stringify({
    tenant_id: "tenant123",
    environment: "production",
  }),
);
```

### Client-Side Metadata Setup

Connection metadata is specified when clients connect by including query parameters in the stream URL. These parameters become the connection's metadata and can be used for selective broadcasting.

**Basic Example:**

```javascript
// Connect with user-specific metadata
const eventSource = new EventSource("/chat?user_id=user123&room=general");

// Connect with role-based metadata
const adminSource = new EventSource(
  "/notifications?role=admin&department=engineering",
);

// Connect with tenant metadata for multi-tenant apps
const tenantSource = new EventSource(
  "/updates?tenant_id=tenant123&environment=production",
);
```

**Query Parameter Format:**

- All query parameters in the connection URL become metadata key-value pairs
- Values are automatically URL-decoded
- Empty values are preserved as empty strings
- Duplicate keys use the last value (standard URL behavior)

**Examples:**

```javascript
// URL: /stream?user_id=john&role=admin&department=engineering
// Metadata: { user_id: "john", role: "admin", department: "engineering" }

// URL: /chat?room=general&mute=false
// Metadata: { room: "general", mute: "false" }

// URL: /notifications?priority=high&category=system
// Metadata: { priority: "high", category: "system" }
```

**JavaScript Helper Function:**

```javascript
function connectWithMetadata(streamPath, metadata) {
  const params = new URLSearchParams(metadata);
  const url = `${streamPath}?${params.toString()}`;
  return new EventSource(url);
}

// Usage
const stream = connectWithMetadata("/chat", {
  user_id: "user123",
  room: "general",
  role: "member",
});
```

**Authentication Integration:**

For authenticated applications, you can include user information in metadata:

```javascript
// Assuming you have user context from authentication
function connectAuthenticatedStream(streamPath) {
  const user = getCurrentUser(); // Your auth function
  const metadata = {
    user_id: user.id,
    role: user.role,
    tenant_id: user.tenantId,
    // Add any other relevant metadata
  };

  return connectWithMetadata(streamPath, metadata);
}

// Usage
const notifications = connectAuthenticatedStream("/notifications");
const chat = connectAuthenticatedStream("/chat");
```

**Dynamic Metadata Updates:**

If you need to change metadata during a session, you'll need to disconnect and reconnect with new parameters:

```javascript
function switchRoom(newRoom) {
  // Close existing connection
  if (eventSource) {
    eventSource.close();
  }

  // Connect with new room metadata
  eventSource = new EventSource(
    `/chat?user_id=user123&room=${encodeURIComponent(newRoom)}`,
  );
}
```

**Best Practices for Metadata:**

1. **Keep it Simple**: Use simple key-value pairs, avoid complex nested objects
2. **Be Consistent**: Use the same metadata keys across your application
3. **URL Encode**: Always encode special characters in metadata values
4. **Security**: Don't include sensitive information in metadata (it's visible in URLs)
5. **Performance**: Prefer short, descriptive keys and values

**Troubleshooting Metadata:**

```javascript
// Debug metadata by logging connection setup
console.log("Connecting to:", urlWithMetadata);

// Verify metadata is being sent
const testConnection = new EventSource(
  "/test-metadata?debug=true&user_id=test",
);
testConnection.onmessage = (event) => {
  console.log("Connection metadata:", event.data);
};
```

### Use Cases for Selective Broadcasting

#### 1. Chat Applications

```javascript
routeRegistry.registerStreamRoute("/chat");

function sendRoomMessage(context) {
  const req = context.request;
  const { room, message, sender } = req.form;

  return sendStreamMessageToConnections(
    "/chat",
    {
      type: "room_message",
      room: room,
      message: message,
      sender: sender,
      timestamp: new Date().toISOString(),
    },
    JSON.stringify({ room: room }),
  );
}

function sendPrivateMessage(context) {
  const req = context.request;
  const { targetUser, message, sender } = req.form;

  return sendStreamMessageToConnections(
    "/chat",
    {
      type: "private_message",
      message: message,
      sender: sender,
      timestamp: new Date().toISOString(),
    },
    JSON.stringify({ user_id: targetUser }),
  );
}

routeRegistry.registerRoute("/chat/room", "sendRoomMessage", "POST");
routeRegistry.registerRoute("/chat/private", "sendPrivateMessage", "POST");
```

#### 2. Role-Based Notifications

```javascript
routeRegistry.registerStreamRoute("/notifications");

function sendAdminAlert(context) {
  const req = context.request;
  const { message, priority } = req.form;

  return sendStreamMessageToConnections(
    "/notifications",
    {
      type: "admin_alert",
      message: message,
      priority: priority,
      timestamp: new Date().toISOString(),
    },
    JSON.stringify({ role: "admin" }),
  );
}

function sendUserNotification(context) {
  const req = context.request;
  const { userId, message } = req.form;

  return sendStreamMessageToConnections(
    "/notifications",
    {
      type: "user_notification",
      message: message,
      timestamp: new Date().toISOString(),
    },
    JSON.stringify({ user_id: userId }),
  );
}

routeRegistry.registerRoute("/notify/admin", "sendAdminAlert", "POST");
routeRegistry.registerRoute("/notify/user", "sendUserNotification", "POST");
```

#### 3. Multi-Tenant Applications

```javascript
routeRegistry.registerStreamRoute("/tenant-updates");

function broadcastTenantUpdate(context) {
  const req = context.request;
  const { tenantId, message } = req.form;

  return sendStreamMessageToConnections(
    "/tenant-updates",
    {
      type: "tenant_update",
      message: message,
      timestamp: new Date().toISOString(),
    },
    JSON.stringify({ tenant_id: tenantId }),
  );
}

routeRegistry.registerRoute(
  "/tenant/broadcast",
  "broadcastTenantUpdate",
  "POST",
);
```

### Best Practices for Selective Broadcasting

1. **Filter Design**
   - Use consistent metadata keys across your application
   - Plan your filtering strategy before implementing
   - Consider performance impact of complex filters

2. **Security**
   - Validate filter criteria server-side
   - Don't expose sensitive metadata in client responses
   - Use appropriate capability checks for filtered broadcasts

3. **Performance**
   - Prefer simple filters (single key-value pairs)
   - Avoid overly complex filter combinations
   - Monitor broadcast performance with many connections

4. **Error Handling**

```javascript
function safeBroadcast(context) {
  try {
    const result = sendStreamMessageToConnections("/chat", data, filter);
    console.log("Broadcast result: " + result);
    return { status: 200, body: result };
  } catch (error) {
    console.error("Broadcast failed: " + error.message);
    return { status: 500, body: "Broadcast failed" };
  }
}
```

### Troubleshooting Selective Broadcasting

1. **Messages Not Received**
   - Check that connection metadata matches your filter exactly
   - Verify filter JSON syntax
   - Ensure connections have the expected metadata

2. **Performance Issues**
   - Monitor broadcast result strings for failure counts
   - Check server logs for filtering performance
   - Consider simplifying complex filters

3. **Debugging**

````javascript
// Log broadcast results
const result = sendStreamMessageToConnections("/test", data, filter);
console.log("Broadcast result: " + result);

// Test with empty filter to verify basic functionality
sendStreamMessageToConnections("/test", { type: "test" }, "{}");
```

## Client Integration

### EventSource API

The standard way to connect to streams from browsers:

```javascript
const eventSource = new EventSource("/your-stream-path");

// Handle messages
eventSource.onmessage = function (event) {
  const data = JSON.parse(event.data);
  console.log("Received:", data);
};

// Handle connection opened
eventSource.onopen = function (event) {
  console.log("Stream connected");
};

// Handle errors
eventSource.onerror = function (event) {
  console.error("Stream error:", event);
  // EventSource automatically attempts to reconnect
};

// Close connection when done
// eventSource.close();
````

### Advanced Client Handling

```javascript
class StreamManager {
  constructor(streamPath, options = {}) {
    this.streamPath = streamPath;
    this.options = {
      reconnectDelay: 3000,
      maxReconnectAttempts: 5,
      ...options,
    };
    this.reconnectAttempts = 0;
    this.eventSource = null;
    this.messageHandlers = new Map();
  }

  connect() {
    this.eventSource = new EventSource(this.streamPath);

    this.eventSource.onmessage = (event) => {
      const data = JSON.parse(event.data);
      this.handleMessage(data);
    };

    this.eventSource.onopen = () => {
      console.log("Stream connected");
      this.reconnectAttempts = 0;
    };

    this.eventSource.onerror = () => {
      this.handleError();
    };
  }

  handleMessage(data) {
    const handler = this.messageHandlers.get(data.type);
    if (handler) {
      handler(data);
    }
  }

  handleError() {
    if (this.reconnectAttempts < this.options.maxReconnectAttempts) {
      this.reconnectAttempts++;
      setTimeout(() => {
        console.log(`Reconnecting... (attempt ${this.reconnectAttempts})`);
        this.connect();
      }, this.options.reconnectDelay);
    }
  }

  on(messageType, handler) {
    this.messageHandlers.set(messageType, handler);
  }

  disconnect() {
    if (this.eventSource) {
      this.eventSource.close();
    }
  }
}

// Usage
const stream = new StreamManager("/notifications");
stream.on("notification", (data) => {
  showNotification(data.title, data.body);
});
stream.on("update", (data) => {
  updateUI(data);
});
stream.connect();
```

### curl Testing

You can test streams from the command line:

```bash
# Connect to a stream
curl -N -H "Accept: text/event-stream" http://localhost:3000/notifications

# In another terminal, trigger an event
curl -X POST http://localhost:3000/trigger-notification
```

## Use Cases

### 1. Live Notifications

Perfect for alerting users about important events:

```javascript
routeRegistry.registerStreamRoute("/notifications");

function sendAlert(context) {
  const req = context.request;
  const { type, message, priority } = req.form;

  routeRegistry.sendStreamMessage("/notifications", {
    type: "alert",
    alertType: type,
    message: message,
    priority: priority || "normal",
    timestamp: new Date().toISOString(),
  });

  return { status: 200, body: "Alert sent" };
}

routeRegistry.registerRoute("/send-alert", "sendAlert", "POST");
```

### 2. Real-Time Dashboard

Stream live metrics and status updates:

```javascript
routeRegistry.registerStreamRoute("/dashboard");

function updateMetrics(context) {
  // Simulate gathering metrics
  const metrics = {
    type: "metrics",
    cpu: Math.random() * 100,
    memory: Math.random() * 100,
    requests: Math.floor(Math.random() * 1000),
    timestamp: new Date().toISOString(),
  };

  routeRegistry.sendStreamMessage("/dashboard", metrics);
  return { status: 200, body: "Metrics updated" };
}

routeRegistry.registerRoute("/update-metrics", "updateMetrics", "POST");
```

### 3. Chat System

Build real-time communication:

```javascript
routeRegistry.registerStreamRoute("/chat");

function sendMessage(context) {
  const req = context.request;
  const { user, room, message } = req.form;

  routeRegistry.sendStreamMessage("/chat", {
    type: "chat_message",
    user: user,
    room: room,
    message: message,
    timestamp: new Date().toISOString(),
  });

  return { status: 200, body: "Message sent" };
}

routeRegistry.registerRoute("/chat/send", "sendMessage", "POST");
```

### 4. Live Data Feed

Stream continuous data updates:

```javascript
routeRegistry.registerStreamRoute("/data-feed");

function broadcastData(context) {
  const req = context.request;
  // Simulate real-time data
  const data = {
    type: "data_update",
    sensor_id: req.query.sensor,
    value: Math.random() * 100,
    unit: "celsius",
    location: "server_room",
    timestamp: new Date().toISOString(),
  };

  routeRegistry.sendStreamMessage("/data-feed", data);
  return { status: 200, body: "Data broadcasted" };
}

routeRegistry.registerRoute("/broadcast-data", "broadcastData", "GET");
```

## Best Practices

### Stream Design

1. **Use Descriptive Path Names**

   ```javascript
   // Good
   routeRegistry.registerStreamRoute("/notifications/user123");
   routeRegistry.registerStreamRoute("/chat/room/general");
   routeRegistry.registerStreamRoute("/status/server/production");

   // Avoid
   routeRegistry.registerStreamRoute("/stream1");
   routeRegistry.registerStreamRoute("/s");
   ```

2. **Structure Your Messages Consistently**

   ```javascript
   // Recommended message structure
   const message = {
     type: "message_type", // Required: categorize messages
     timestamp: new Date().toISOString(), // Recommended: for ordering
     id: generateId(), // Optional: for deduplication
     data: {
       /* actual payload */
     }, // Your data
   };
   ```

3. **Handle Different Message Types**

```javascript
routeRegistry.sendStreamMessage('/notifications', { type: 'notification', ... });
routeRegistry.sendStreamMessage('/updates', { type: 'update', ... });
routeRegistry.sendStreamMessage('/errors', { type: 'error', ... });
```

### Client-Side Best Practices

1. **Implement Reconnection Logic**
   - EventSource automatically reconnects, but you may want custom logic
   - Handle network failures gracefully
   - Consider exponential backoff for reconnection attempts

2. **Handle Message Types**

   ```javascript
   eventSource.onmessage = function (event) {
     const data = JSON.parse(event.data);

     switch (data.type) {
       case "notification":
         showNotification(data);
         break;
       case "update":
         updateUI(data);
         break;
       case "error":
         handleError(data);
         break;
       default:
         console.warn("Unknown message type:", data.type);
     }
   };
   ```

3. **Clean Up Connections**

```javascript
// Close connections when navigating away
window.addEventListener("beforeunload", function () {
  if (eventSource) {
    eventSource.close();
  }
});
```

### Performance Considerations

1. **Message Frequency**
   - Avoid sending too many messages per second
   - Consider batching updates for high-frequency data
   - Use throttling or debouncing when appropriate

2. **Message Size**
   - Keep messages reasonably small (< 1MB recommended)
   - Consider compression for large datasets
   - Use references instead of embedding large objects

3. **Connection Limits**
   - Browser limits concurrent SSE connections (typically 6 per domain)
   - Consider multiplexing multiple data types on one stream
   - Use appropriate stream paths to organize data

### Error Handling

1. **Server-Side**

```javascript
function safeHandler(context) {
  try {
    // Your logic here
    routeRegistry.sendStreamMessage("/notifications", {
      type: "success",
      data: result,
    });
    return { status: 200, body: "OK" };
  } catch (error) {
    console.error("Error in handler: " + error.message);
    routeRegistry.sendStreamMessage("/notifications", {
      type: "error",
      message: "Something went wrong",
      timestamp: new Date().toISOString(),
    });
    return { status: 500, body: "Error occurred" };
  }
}
```

1. **Client-Side**

```javascript
eventSource.onerror = function (event) {
  console.error("Stream error:", event);
  // Handle the error appropriately
  showErrorMessage("Connection lost. Attempting to reconnect...");
};
```

### Security Considerations

1. **Validate Stream Paths**
   - Ensure stream paths don't expose sensitive information
   - Consider using UUIDs for user-specific streams

2. **Message Content**
   - Don't send sensitive data in stream messages
   - Validate and sanitize any user input before broadcasting

3. **Rate Limiting** (Future Enhancement)
   - Consider implementing rate limiting for stream registrations
   - Monitor for abuse of the streaming endpoints

## Troubleshooting

### Common Issues

1. **Stream Not Receiving Messages**

   ```javascript
   // Check if stream is registered
   console.log("Registering stream...");
   routeRegistry.registerStreamRoute("/my-stream");
   console.log("Stream registered");

   // Verify message sending
   console.log("Sending message...");
   routeRegistry.sendStreamMessage("/my-stream", {
     type: "test",
     message: "Hello",
   });
   console.log("Message sent");
   ```

2. **Client Connection Issues**

   ```javascript
   // Add detailed error handling
   eventSource.onerror = function (event) {
     console.error("EventSource failed:", event);
     console.log("ReadyState:", eventSource.readyState);
     // 0 = CONNECTING, 1 = OPEN, 2 = CLOSED
   };
   ```

3. **Browser Connection Limits**
   - Check browser developer tools Network tab
   - Look for "Too many connections" errors
   - Consider using fewer concurrent streams

### Debugging Tips

1. **Server Logs**

   ```javascript
   console.log("Stream registered: /my-stream");
   console.log("Broadcasting message: " + JSON.stringify(data));
   ```

2. **Client Console**

   ```javascript
   console.log("EventSource state:", eventSource.readyState);
   console.log("Received message:", data);
   ```

3. **Network Inspection**
   - Use browser DevTools Network tab
   - Look for EventSource connections
   - Check for proper `text/event-stream` content type

### Performance Monitoring

Track stream performance:

```javascript
let messageCount = 0;
let connectionCount = 0;

eventSource.onopen = () => {
  connectionCount++;
  console.log("Connections:", connectionCount);
};

eventSource.onmessage = (event) => {
  messageCount++;
  if (messageCount % 100 === 0) {
    console.log("Messages received:", messageCount);
  }
};
```

## Advanced Topics

### Multiple Stream Coordination

When using multiple streams, coordinate them effectively:

```javascript
// Register different streams for different data types
routeRegistry.registerStreamRoute("/notifications"); // User notifications
routeRegistry.registerStreamRoute("/system-status"); // System health
routeRegistry.registerStreamRoute("/chat"); // Chat messages

// Send targeted messages based on context
function handleUserAction(context) {
  const req = context.request;
  // Notify about user action
  routeRegistry.sendStreamMessage("/notifications", {
    type: "user_action",
    action: req.form.action,
    user: req.form.user,
  });

  // Update system status if needed
  if (req.form.action === "critical_operation") {
    routeRegistry.sendStreamMessage("/system-status", {
      type: "system_status",
      status: "busy",
      operation: req.form.action,
    });
  }

  return { status: 200, body: "Action processed" };
}
```

### Integration with External Systems

Connect streams to external data sources:

```javascript
routeRegistry.registerStreamRoute("/external-updates");

// Webhook handler for external system notifications
function webhookHandler(context) {
  const req = context.request;
  const webhookData = JSON.parse(req.body);

  // Transform external data for your stream
  routeRegistry.sendStreamMessage("/external-updates", {
    type: "external_update",
    source: "github",
    event: webhookData.action,
    repository: webhookData.repository.name,
    timestamp: new Date().toISOString(),
  });

  return { status: 200, body: "Webhook processed" };
}

routeRegistry.registerRoute("/webhook/github", "webhookHandler", "POST");
```

### GraphQL Subscriptions with Selective Broadcasting

GraphQL subscriptions also support selective broadcasting using the same metadata mechanism. Clients specify metadata via query parameters in the GraphQL SSE endpoint URL.

**Client Connection:**

```javascript
// Connect to GraphQL subscription with metadata
const eventSource = new EventSource(
  "/graphql/sse?user_id=user123&room=general&role=member",
);

// Send GraphQL subscription request
const subscriptionQuery = {
  query: `subscription { chatMessages { id text sender } }`,
};

fetch("/graphql/sse?user_id=user123&room=general", {
  method: "POST",
  headers: { "Content-Type": "application/json" },
  body: JSON.stringify(subscriptionQuery),
}).then((response) => {
  // Handle SSE stream...
});
```

**Server-Side Broadcasting:**

```javascript
// Send to all subscribers of chatMessages
graphQLRegistry.sendSubscriptionMessage("chatMessages", data);

// Send selectively to subscribers with specific metadata
graphQLRegistry.sendSubscriptionMessageFiltered(
  "chatMessages",
  data,
  JSON.stringify({ user_id: "user123" }),
);

// Send to subscribers in a specific room
graphQLRegistry.sendSubscriptionMessageFiltered(
  "chatMessages",
  data,
  JSON.stringify({ room: "general" }),
);
```

**Key Differences from Regular Streams:**

- **URL Structure**: Metadata is specified in the `/graphql/sse` endpoint URL, not in individual stream paths
- **Subscription Name**: The system automatically determines the stream path based on the GraphQL subscription name
- **Connection Tracking**: Each GraphQL subscription connection is tracked with metadata for selective broadcasting

**Example Workflow:**

1. Client connects to `/graphql/sse?user_id=user123&room=general`
2. Client sends GraphQL subscription: `subscription { chatMessages { ... } }`
3. Server creates connection to `/graphql/subscription/chatMessages` with metadata `{user_id: "user123", room: "general"}`
4. Server can selectively broadcast using `graphQLRegistry.sendSubscriptionMessageFiltered("chatMessages", data, filter)`

**Use Cases:**

- **User-Specific Notifications**: Send notifications only to specific users
- **Room-Based Chat**: Filter messages by chat room or channel
- **Role-Based Broadcasting**: Send different content based on user roles
- **Tenant Isolation**: Filter by organization or workspace

## Next Steps

- Check out the [Examples](examples.md) for complete streaming applications
- Read the [GraphQL Subscriptions Guide](graphql-subscriptions.md) for subscription-specific features
- Review the [JavaScript APIs](javascript-apis.md) for detailed API documentation
- Learn about [Local Development](local-development.md) workflows for testing streams
- Explore the streaming test scripts in the `scripts/test_scripts/` directory

---

**Note**: Streaming is a powerful feature that opens up many possibilities for interactive applications. Start simple with basic notifications and gradually build more complex real-time features as you become comfortable with the API.
