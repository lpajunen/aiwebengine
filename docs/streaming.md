# Real-Time Streaming Guide

This guide covers aiwebengine's real-time streaming capabilities using Server-Sent Events (SSE). Learn how to build live, interactive applications that push updates to clients in real-time.

## Table of Contents

- [Overview](#overview)
- [Architecture](#architecture)
- [Quick Start](#quick-start)
- [API Reference](#api-reference)
- [Client Integration](#client-integration)
- [Use Cases](#use-cases)
- [GraphQL Subscriptions](#graphql-subscriptions)
- [Best Practices](#best-practices)
- [Troubleshooting](#troubleshooting)

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
3. **JavaScript Engine Integration**: Provides `registerWebStream()` and `sendStreamMessage()` functions
4. **SSE Server**: Handles HTTP connections and message broadcasting

### Flow Diagram

```text
JavaScript Script          Stream Registry          Connected Clients
     |                          |                         |
     | registerWebStream()      |                         |
     |------------------------->|                         |
     |                          |                         |
     |                          | <--- Client connects ---|
     |                          |                         |
     | sendStreamMessage()      |                         |
     |------------------------->|                         |
     |                          |---- Broadcast to all -->|
     |                          |---- connected clients ->|
```

### Connection Lifecycle

1. **Registration**: Script calls `registerWebStream('/path')` to create a stream endpoint
2. **Client Connection**: Browser connects using `new EventSource('/path')`
3. **Broadcasting**: Script calls `sendStreamMessage(data)` to send data to all clients
4. **Cleanup**: Connections automatically cleaned up when clients disconnect

## Quick Start

### 1. Basic Stream Setup

```javascript
// Register a stream endpoint
registerWebStream("/events");

// Handler to send events
function triggerEvent(req) {
  sendStreamMessage({
    type: "event",
    message: "Something happened!",
    timestamp: new Date().toISOString(),
  });

  return { status: 200, body: "Event sent" };
}

register("/trigger", "triggerEvent", "POST");
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

### JavaScript Functions

#### registerWebStream(path)

Registers a Server-Sent Events endpoint that clients can connect to.

**Parameters:**

- `path` (string): Stream path (must start with `/`, max 200 characters)

**Returns:** Nothing

**Throws:** Error if path is invalid or registration fails

**Example:**

```javascript
registerWebStream("/notifications");
registerWebStream("/chat/room1");
registerWebStream("/status/server1");
```

**Path Requirements:**

- Must start with `/`
- Maximum 200 characters
- Should be unique per script
- Case-sensitive

#### sendStreamMessage(data)

Broadcasts a message to all clients connected to registered streams.

**Parameters:**

- `data` (object): Data to send (will be JSON serialized)

**Returns:** Nothing

**Example:**

```javascript
sendStreamMessage({
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

### Stream Management

Streams are automatically managed by the aiwebengine:

- **Registration**: Streams persist until server restart or script reload
- **Connections**: Multiple clients can connect to the same stream
- **Broadcasting**: Messages sent to ALL connected clients on ALL registered streams
- **Cleanup**: Stale connections automatically removed

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
```

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
registerWebStream("/notifications");

function sendAlert(req) {
  const { type, message, priority } = req.form;

  sendStreamMessage({
    type: "alert",
    alertType: type,
    message: message,
    priority: priority || "normal",
    timestamp: new Date().toISOString(),
  });

  return { status: 200, body: "Alert sent" };
}

register("/send-alert", "sendAlert", "POST");
```

### 2. Real-Time Dashboard

Stream live metrics and status updates:

```javascript
registerWebStream("/dashboard");

function updateMetrics(req) {
  // Simulate gathering metrics
  const metrics = {
    type: "metrics",
    cpu: Math.random() * 100,
    memory: Math.random() * 100,
    requests: Math.floor(Math.random() * 1000),
    timestamp: new Date().toISOString(),
  };

  sendStreamMessage(metrics);
  return { status: 200, body: "Metrics updated" };
}

register("/update-metrics", "updateMetrics", "POST");
```

### 3. Chat System

Build real-time communication:

```javascript
registerWebStream("/chat");

function sendMessage(req) {
  const { user, room, message } = req.form;

  sendStreamMessage({
    type: "chat_message",
    user: user,
    room: room,
    message: message,
    timestamp: new Date().toISOString(),
  });

  return { status: 200, body: "Message sent" };
}

register("/chat/send", "sendMessage", "POST");
```

### 4. Live Data Feed

Stream continuous data updates:

```javascript
registerWebStream("/data-feed");

function broadcastData(req) {
  // Simulate real-time data
  const data = {
    type: "data_update",
    sensor_id: req.query.sensor,
    value: Math.random() * 100,
    unit: "celsius",
    location: "server_room",
    timestamp: new Date().toISOString(),
  };

  sendStreamMessage(data);
  return { status: 200, body: "Data broadcasted" };
}

register("/broadcast-data", "broadcastData", "GET");
```

## Best Practices

### Stream Design

1. **Use Descriptive Path Names**

   ```javascript
   // Good
   registerWebStream("/notifications/user123");
   registerWebStream("/chat/room/general");
   registerWebStream("/status/server/production");

   // Avoid
   registerWebStream("/stream1");
   registerWebStream("/s");
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
   sendStreamMessage({ type: 'notification', ... });
   sendStreamMessage({ type: 'update', ... });
   sendStreamMessage({ type: 'error', ... });
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
   function safeHandler(req) {
     try {
       // Your logic here
       sendStreamMessage({ type: "success", data: result });
       return { status: 200, body: "OK" };
     } catch (error) {
       writeLog("Error in handler: " + error.message);
       sendStreamMessage({
         type: "error",
         message: "Something went wrong",
         timestamp: new Date().toISOString(),
       });
       return { status: 500, body: "Error occurred" };
     }
   }
   ```

2. **Client-Side**
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
   writeLog("Registering stream...");
   registerWebStream("/my-stream");
   writeLog("Stream registered");

   // Verify message sending
   writeLog("Sending message...");
   sendStreamMessage({ type: "test", message: "Hello" });
   writeLog("Message sent");
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
   writeLog("Stream registered: /my-stream");
   writeLog("Broadcasting message: " + JSON.stringify(data));
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
registerWebStream("/notifications"); // User notifications
registerWebStream("/system-status"); // System health
registerWebStream("/chat"); // Chat messages

// Send targeted messages based on context
function handleUserAction(req) {
  // Notify about user action
  sendStreamMessage({
    type: "user_action",
    action: req.form.action,
    user: req.form.user,
  });

  // Update system status if needed
  if (req.form.action === "critical_operation") {
    sendStreamMessage({
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
registerWebStream("/external-updates");

// Webhook handler for external system notifications
function webhookHandler(req) {
  const webhookData = JSON.parse(req.body);

  // Transform external data for your stream
  sendStreamMessage({
    type: "external_update",
    source: "github",
    event: webhookData.action,
    repository: webhookData.repository.name,
    timestamp: new Date().toISOString(),
  });

  return { status: 200, body: "Webhook processed" };
}

register("/webhook/github", "webhookHandler", "POST");
```

## GraphQL Subscriptions

aiwebengine supports GraphQL subscriptions using the same SSE streaming infrastructure. When you register a GraphQL subscription, a corresponding stream path is automatically created.

### Quick Example

```javascript
// Register a GraphQL subscription
registerGraphQLSubscription(
  "liveEvents",
  "type Subscription { liveEvents: String }",
  "liveEventsResolver",
);

// Subscription resolver
function liveEventsResolver() {
  return "Live events subscription active";
}

// Send messages to subscribers
function triggerEvent() {
  sendSubscriptionMessage(
    "liveEvents",
    JSON.stringify({
      event: "user_joined",
      timestamp: new Date().toISOString(),
    }),
  );
}
```

### Client Connection

```javascript
// Connect via GraphQL subscription
const subscriptionQuery = {
  query: `subscription { liveEvents }`,
};

fetch("/graphql/sse", {
  method: "POST",
  headers: { "Content-Type": "application/json" },
  body: JSON.stringify(subscriptionQuery),
}).then((response) => {
  // Handle SSE stream...
});

// Or connect directly to the auto-registered stream path
const eventSource = new EventSource("/graphql/subscription/liveEvents");
```

For complete GraphQL subscription documentation, see [GraphQL Subscriptions Guide](graphql-subscriptions.md).

## Next Steps

- Check out the [Examples](examples.md) for complete streaming applications
- Read the [GraphQL Subscriptions Guide](graphql-subscriptions.md) for subscription-specific features
- Review the [JavaScript APIs](javascript-apis.md) for detailed API documentation
- Learn about [Local Development](local-development.md) workflows for testing streams
- Explore the streaming test scripts in the `scripts/test_scripts/` directory

---

**Note**: Streaming is a powerful feature that opens up many possibilities for interactive applications. Start simple with basic notifications and gradually build more complex real-time features as you become comfortable with the API.
