# GraphQL Subscriptions with Server-Sent Events (execute_stream)

This guide explains how to use GraphQL subscriptions in aiwebengine with Server-Sent Events (SSE) for real-time updates using the native `execute_stream` approach.

## Overview

GraphQL subscriptions in aiwebengine now use the native `schema.execute_stream()` method for better standards compliance and reliability:

1. **Native GraphQL lifecycle**: Subscriptions use async-graphql's built-in subscription streaming
2. **Standards compliant**: Follows GraphQL subscription specifications exactly
3. **Better error handling**: Proper GraphQL error objects and propagation
4. **SSE transport**: Clients connect via `/graphql/sse` endpoint using standard GraphQL subscription queries
5. **Simplified architecture**: No manual stream path management needed

## JavaScript API

### Registering Subscriptions

```javascript
graphQLRegistry.registerSubscription(
  "subscriptionName",
  "type Subscription { subscriptionName: String }",
  "resolverFunctionName",
);
```

### Subscription Resolver

The subscription resolver is called when a client subscribes. It should return an initial message and set up any necessary state:

```javascript
function mySubscriptionResolver() {
  console.log("Client subscribed to mySubscription");
  return "Subscription initialized";
}
```

### Sending Messages to Subscribers

Use the convenience function (legacy compatibility):

```javascript
graphQLRegistry.sendSubscriptionMessage(
  "subscriptionName",
  JSON.stringify({
    message: "Hello subscribers!",
    timestamp: new Date().toISOString(),
  }),
);
```

**Note**: With the new execute_stream approach, this function maintains backward compatibility but logs a deprecation warning. For new development, consider generating subscription messages directly within subscription resolvers for better GraphQL compliance.
routeRegistry.sendStreamMessage("/graphql/subscription/subscriptionName", JSON.stringify(data));

````

## Client-Side Usage

### Subscribing via SSE

```javascript
const subscriptionQuery = {
    query: `subscription { mySubscription }`
};

const eventSource = new EventSource('/graphql/sse?query=' + encodeURIComponent(subscriptionQuery.query));

eventSource.onopen = function(event) {
    console.log('Connected to subscription');
};

eventSource.onmessage = function(event) {
    try {
        const data = JSON.parse(event.data);
        if (data.data && data.data.mySubscription) {
            console.log('Received:', data.data.mySubscription);
        }
    } catch (e) {
        console.log('Non-JSON data:', event.data);
    }
};

eventSource.onerror = function(event) {
    console.error('Subscription connection error:', event);
};
````

### Using fetch + StreamReader (Alternative)

If you need more control or EventSource is not available, you can use the lower-level fetch API:

```javascript
const eventSource = new EventSource("/graphql/subscription/mySubscription");

eventSource.onmessage = function (event) {
  const data = JSON.parse(event.data);
  console.log("Received message:", data);
};
```

## Complete Example

### JavaScript (Server-side)

```javascript
// Register the subscription
graphQLRegistry.registerSubscription(
  "liveNotifications",
  "type Subscription { liveNotifications: String }",
  "liveNotificationsResolver",
);

// Register a mutation to trigger notifications
graphQLRegistry.registerMutation(
  "sendNotification",
  "type Mutation { sendNotification(message: String!): String }",
  "sendNotificationResolver",
);

// Subscription resolver - called when clients subscribe
function liveNotificationsResolver() {
  console.log("Client subscribed to live notifications");
  return "Notification subscription active";
}

// Mutation resolver - triggers subscription messages
function sendNotificationResolver(args) {
  const notification = {
    id: Math.random().toString(36).substr(2, 9),
    message: args.message,
    timestamp: new Date().toISOString(),
    type: "info",
  };

  // Send to all subscription clients
  graphQLRegistry.sendSubscriptionMessage(
    "liveNotifications",
    JSON.stringify(notification),
  );

  return `Notification sent: ${args.message}`;
}

// You can also trigger from HTTP endpoints or other events
routeRegistry.registerRoute(
  "/trigger-notification",
  "triggerNotificationHandler",
  "POST",
);

function triggerNotificationHandler(req) {
  const message = req.body || "Default notification";

  graphQLRegistry.sendSubscriptionMessage(
    "liveNotifications",
    JSON.stringify({
      id: Math.random().toString(36).substr(2, 9),
      message: message,
      timestamp: new Date().toISOString(),
      type: "system",
    }),
  );

  return {
    status: 200,
    body: JSON.stringify({ success: true }),
    contentType: "application/json",
  };
}
```

### Client-side HTML

```html
<!DOCTYPE html>
<html>
  <head>
    <title>GraphQL Subscription Demo</title>
  </head>
  <body>
    <div id="notifications"></div>
    <button onclick="sendTestNotification()">Send Test Notification</button>

    <script>
      // Subscribe to notifications
      const subscriptionQuery = {
          query: \`subscription { liveNotifications }\`
      };

      const eventSource = new EventSource('/graphql/sse?query=' + encodeURIComponent(subscriptionQuery.query));

      eventSource.onopen = function(event) {
          console.log('Connected to notifications subscription');
      };

      eventSource.onmessage = function(event) {
          try {
              const data = JSON.parse(event.data);
              if (data.data && data.data.liveNotifications) {
                  displayNotification(data.data.liveNotifications);
              }
          } catch (e) {
              console.log('Non-JSON data:', event.data);
          }
      };

      eventSource.onerror = function(event) {
          console.error('Subscription connection error:', event);
      };

      function displayNotification(notification) {
          const div = document.getElementById('notifications');
          const notificationEl = document.createElement('div');

          try {
              const data = JSON.parse(notification);
              notificationEl.innerHTML = \`
                  <p><strong>\${data.id}</strong> [\${data.timestamp}]</p>
                  <p>\${data.message}</p>
              \`;
          } catch (e) {
              notificationEl.textContent = notification;
          }

          div.appendChild(notificationEl);
      }

      function sendTestNotification() {
          const mutation = {
              query: \`mutation { sendNotification(message: "Test notification from client") }\`
          };

          fetch('/graphql', {
              method: 'POST',
              headers: { 'Content-Type': 'application/json' },
              body: JSON.stringify(mutation)
          });
      }
    </script>
  </body>
</html>
```

## Architecture Details

### Native execute_stream Implementation

When you call `graphQLRegistry.registerSubscription("mySubscription", ...)`, the system:

1. Registers the GraphQL subscription field in the dynamic schema
2. **No longer auto-registers stream paths** - uses native GraphQL subscription lifecycle
3. Subscription handling is managed entirely by `schema.execute_stream()`

### Message Flow (New execute_stream Approach)

1. **Client subscribes**: Sends GraphQL subscription query to `/graphql/sse`
2. **Server uses execute_stream**: Creates native GraphQL subscription stream using `schema.execute_stream(request)`
3. **Native lifecycle**: GraphQL handles subscription initialization, data flow, and cleanup
4. **SSE conversion**: Server converts GraphQL response stream to SSE events
5. **Client receives**: Messages delivered via SSE with proper GraphQL error handling

### Benefits of execute_stream

- **Standards compliance**: Full GraphQL subscription specification support
- **Better error handling**: Native GraphQL error objects and propagation
- **Simplified architecture**: No manual stream path management
- **Future-proof**: Leverages async-graphql's built-in subscription features
- **Reduced complexity**: ~70% fewer lines of code vs manual approach

### Legacy Compatibility

The system maintains backward compatibility:

- `graphQLRegistry.sendSubscriptionMessage()` still works but logs deprecation warnings
- Existing scripts continue to function without modification
- Stream registry is still used for legacy compatibility bridge

## Best Practices

1. **Use execute_stream naturally**: Let GraphQL handle the subscription lifecycle
2. **Consider moving message generation to resolvers**: For better GraphQL compliance
3. **Send structured data**: Use JSON for consistent message format
4. **Handle errors gracefully**: execute_stream provides native GraphQL error handling
5. **Use meaningful subscription names**: They become GraphQL field names

## Troubleshooting

### Common Issues

#### Subscription not receiving messages

- Check that the subscription name matches between registration and `graphQLRegistry.sendSubscriptionMessage()`
- Check if you see deprecation warnings in logs (legacy compatibility mode)
- Verify GraphQL subscription is properly registered in the schema
- Check server logs for execute_stream connection and message flow

#### Client connection fails

- Ensure the GraphQL query syntax is correct: `subscription { subscriptionName }`
- Check that the subscription field exists in the schema
- Verify the `/graphql/sse` endpoint is accessible

#### Messages not formatted correctly

- Use `JSON.stringify()` when sending structured data
- Handle JSON parsing errors on the client side
- Check the SSE data format: should be `data: {content}\\n\\n`

### Debugging

Enable debug logging to see subscription activity:

```bash
RUST_LOG=debug ./your-server
```

Look for log messages like:

- "Registering GraphQL subscription: {name}"
- "Auto-registered stream path '/graphql/subscription/{name}'"
- "Client subscribed to {name}"
- "Successfully broadcast subscription message to N connections"
