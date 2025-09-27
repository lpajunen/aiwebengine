# GraphQL Subscriptions with Server-Sent Events

This guide explains how to use GraphQL subscriptions in aiwebengine with Server-Sent Events (SSE) for real-time updates.

## Overview

GraphQL subscriptions in aiwebengine work by:

1. **Auto-registering stream paths**: When you register a GraphQL subscription, a corresponding stream path is automatically created at `/graphql/subscription/{subscriptionName}`
2. **Using existing streaming infrastructure**: Subscriptions leverage the same streaming system used for regular SSE endpoints
3. **JavaScript message sending**: Use `sendSubscriptionMessage()` to emit values to subscription clients
4. **SSE transport**: Clients connect via `/graphql/sse` endpoint using standard GraphQL subscription queries

## JavaScript API

### Registering Subscriptions

```javascript
registerGraphQLSubscription(
    "subscriptionName", 
    "type Subscription { subscriptionName: String }", 
    "resolverFunctionName"
);
```

### Subscription Resolver

The subscription resolver is called when a client subscribes. It should return an initial message and set up any necessary state:

```javascript
function mySubscriptionResolver() {
    writeLog("Client subscribed to mySubscription");
    return "Subscription initialized";
}
```

### Sending Messages to Subscribers

Use the convenience function:

```javascript
sendSubscriptionMessage("subscriptionName", JSON.stringify({
    message: "Hello subscribers!",
    timestamp: new Date().toISOString()
}));
```

Or use the lower-level API:

```javascript
sendStreamMessageToPath("/graphql/subscription/subscriptionName", JSON.stringify(data));
```

## Client-Side Usage

### Subscribing via SSE

```javascript
const subscriptionQuery = {
    query: `subscription { mySubscription }`
};

fetch('/graphql/sse', {
    method: 'POST',
    headers: {
        'Content-Type': 'application/json',
    },
    body: JSON.stringify(subscriptionQuery)
})
.then(response => {
    const reader = response.body.getReader();
    const decoder = new TextDecoder();
    
    function readStream() {
        reader.read().then(({ done, value }) => {
            if (done) return;
            
            const chunk = decoder.decode(value);
            const lines = chunk.split('\\n');
            
            lines.forEach(line => {
                if (line.startsWith('data: ')) {
                    const data = JSON.parse(line.slice(6));
                    if (data.data && data.data.mySubscription) {
                        console.log('Received:', data.data.mySubscription);
                    }
                }
            });
            
            readStream();
        });
    }
    
    readStream();
});
```

### Using EventSource (Alternative)

You can also connect directly to the auto-registered stream path:

```javascript
const eventSource = new EventSource('/graphql/subscription/mySubscription');

eventSource.onmessage = function(event) {
    const data = JSON.parse(event.data);
    console.log('Received message:', data);
};
```

## Complete Example

### JavaScript (Server-side)

```javascript
// Register the subscription
registerGraphQLSubscription(
    "liveNotifications", 
    "type Subscription { liveNotifications: String }", 
    "liveNotificationsResolver"
);

// Register a mutation to trigger notifications
registerGraphQLMutation(
    "sendNotification", 
    "type Mutation { sendNotification(message: String!): String }", 
    "sendNotificationResolver"
);

// Subscription resolver - called when clients subscribe
function liveNotificationsResolver() {
    writeLog("Client subscribed to live notifications");
    return "Notification subscription active";
}

// Mutation resolver - triggers subscription messages
function sendNotificationResolver(args) {
    const notification = {
        id: Math.random().toString(36).substr(2, 9),
        message: args.message,
        timestamp: new Date().toISOString(),
        type: "info"
    };
    
    // Send to all subscription clients
    sendSubscriptionMessage("liveNotifications", JSON.stringify(notification));
    
    return `Notification sent: ${args.message}`;
}

// You can also trigger from HTTP endpoints or other events
register('/trigger-notification', 'triggerNotificationHandler', 'POST');

function triggerNotificationHandler(req) {
    const message = req.body || "Default notification";
    
    sendSubscriptionMessage("liveNotifications", JSON.stringify({
        id: Math.random().toString(36).substr(2, 9),
        message: message,
        timestamp: new Date().toISOString(),
        type: "system"
    }));
    
    return {
        status: 200,
        body: JSON.stringify({ success: true }),
        contentType: "application/json"
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
        
        fetch('/graphql/sse', {
            method: 'POST',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify(subscriptionQuery)
        })
        .then(response => {
            const reader = response.body.getReader();
            const decoder = new TextDecoder();
            
            function readStream() {
                reader.read().then(({ done, value }) => {
                    if (done) return;
                    
                    const chunk = decoder.decode(value);
                    const lines = chunk.split('\\n');
                    
                    lines.forEach(line => {
                        if (line.startsWith('data: ')) {
                            try {
                                const data = JSON.parse(line.slice(6));
                                if (data.data && data.data.liveNotifications) {
                                    displayNotification(data.data.liveNotifications);
                                }
                            } catch (e) {
                                console.log('Non-JSON data:', line);
                            }
                        }
                    });
                    
                    readStream();
                });
            }
            
            readStream();
        });
        
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

### Stream Path Auto-Registration

When you call `registerGraphQLSubscription("mySubscription", ...)`, the system automatically:

1. Registers the GraphQL subscription field
2. Creates a corresponding stream path at `/graphql/subscription/mySubscription`
3. Makes this stream available for connections

### Message Flow

1. **Client subscribes**: Sends GraphQL subscription query to `/graphql/sse`
2. **Server initializes**: Calls the subscription resolver function
3. **Server connects**: Links the GraphQL subscription to the auto-registered stream path
4. **Messages flow**: JavaScript code uses `sendSubscriptionMessage()` to emit values
5. **Client receives**: Messages are delivered via SSE to subscribed clients

### Integration with Existing Streaming

The GraphQL subscription system is built on top of the existing streaming infrastructure, which means:

- All streaming features (connection management, cleanup, health monitoring) work with subscriptions
- You can mix GraphQL subscriptions with regular SSE endpoints
- Stream statistics and monitoring include subscription connections

## Best Practices

1. **Initialize subscriptions properly**: Use the subscription resolver to set up any necessary state
2. **Send structured data**: Use JSON for consistent message format
3. **Handle errors gracefully**: Check for connection errors when sending messages
4. **Clean up resources**: Let the system handle connection cleanup automatically
5. **Use meaningful subscription names**: They become part of the stream path structure

## Troubleshooting

### Common Issues

#### Subscription not receiving messages

- Check that the subscription name matches between registration and `sendSubscriptionMessage()`
- Verify the stream path was auto-registered: `/graphql/subscription/{name}`
- Check server logs for connection and broadcasting messages

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
