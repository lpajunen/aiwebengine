/// <reference path="../../assets/aiwebengine.d.ts" />

// GraphQL Subscription Example with Streaming
// This script shows how to create GraphQL subscriptions that work with Server-Sent Events

function broadcastLiveMessage(text, type) {
  const payload = {
    id: Math.random().toString(36).substr(2, 9),
    text,
    timestamp: new Date().toISOString(),
    sender: "system",
  };

  if (type) {
    payload.type = type;
  }

  graphQLRegistry.sendSubscriptionMessage(
    "liveMessages",
    JSON.stringify(payload),
  );

  return payload;
}

// The subscription resolver - called when a client subscribes
function liveMessagesResolver(context) {
  const req = context.request || {};
  console.log("Client subscribed to liveMessages from:", req.path);

  // Get user information
  let userName = "Guest";
  try {
    const user = req.auth.user;
    if (user && user.name) {
      userName = user.name;
    }
  } catch (e) {
    console.log("Auth not available, using Guest");
  }

  // Send a "user joined" message to all subscribers
  console.log(`Sending join message: ${userName} joined`);
  broadcastLiveMessage(`${userName} joined`, "join");

  // Return an empty object for stream customization (no filtering)
  // If you want to filter, return an object like: { userId: "123", topic: "general" }
  return {};
}

// The mutation resolver - triggers subscription messages
function sendMessageResolver(context) {
  const args = context.args || {};
  const message = args.text;

  if (!message || message.trim().length === 0) {
    throw new Error("text argument is required");
  }

  console.log(`Sending message to liveMessages subscribers: ${message}`);
  broadcastLiveMessage(message);
  return `Message sent: ${message}`;
}

// Optional: You can also use the lower-level API
// routeRegistry.sendStreamMessage("/graphql/subscription/liveMessages", JSON.stringify(messageData));

function triggerMessageHandler(context) {
  const req = context.request || {};
  const messageBody = req.body;
  const message =
    (typeof messageBody === "string" && messageBody.trim()) ||
    req.form?.message ||
    "Hello from HTTP trigger!";

  broadcastLiveMessage(message);

  return ResponseBuilder.json({
    success: true,
    message: "Message broadcasted",
  });
}

function subscriptionDemoPage(context) {
  return ResponseBuilder.html(`
        <!DOCTYPE html>
        <html lang="en">
        <head>
            <meta charset="UTF-8">
            <meta name="viewport" content="width=device-width, initial-scale=1.0">
            <title>GraphQL Subscription Demo - aiwebengine</title>
            <link rel="stylesheet" href="/engine.css">
            <link rel="icon" type="image/x-icon" href="/favicon.ico">
            <style>
                /* Subscription demo specific overrides */
                body {
                    background: linear-gradient(135deg, #667eea 0%, #764ba2 100%);
                    min-height: 100vh;
                    padding: 2rem 0;
                }

                .demo-container {
                    max-width: 1000px;
                    margin: 0 auto;
                    background: rgba(255, 255, 255, 0.95);
                    backdrop-filter: blur(10px);
                    border-radius: var(--border-radius-lg);
                    box-shadow: var(--shadow-lg);
                    overflow: hidden;
                }

                .demo-content {
                    padding: 3rem 2rem;
                }

                .demo-header {
                    text-align: center;
                    margin-bottom: 3rem;
                }

                .demo-header h1 {
                    color: var(--text-color);
                    margin-bottom: 0.5rem;
                }

                .demo-header p {
                    color: var(--text-muted);
                    font-size: 1.1rem;
                }

                .demo-section {
                    margin-bottom: 2.5rem;
                    padding: 1.5rem;
                    background: var(--bg-secondary);
                    border-radius: var(--border-radius);
                    border: 1px solid var(--border-color);
                }

                .demo-section h3 {
                    color: var(--text-color);
                    margin-top: 0;
                    margin-bottom: 1rem;
                    border-bottom: 2px solid var(--primary-color);
                    padding-bottom: 0.5rem;
                }

                .form-row {
                    display: flex;
                    gap: 1rem;
                    align-items: center;
                    margin-bottom: 1rem;
                    flex-wrap: wrap;
                }

                .form-row input {
                    flex: 1;
                    min-width: 200px;
                }

                .messages-container {
                    border: 1px solid var(--border-color);
                    border-radius: var(--border-radius);
                    height: 300px;
                    overflow-y: auto;
                    padding: 1rem;
                    background: var(--bg-primary);
                    margin: 1rem 0;
                }

                .message {
                    margin: 0.5rem 0;
                    padding: 0.75rem;
                    background: var(--bg-secondary);
                    border-radius: var(--border-radius);
                    border-left: 3px solid var(--primary-color);
                }

                .message strong {
                    color: var(--primary-color);
                }

                .join-message {
                    margin: 0.5rem 0;
                    padding: 0.5rem 0.75rem;
                    background: var(--info-bg);
                    border-radius: var(--border-radius);
                    border-left: 3px solid var(--info-color);
                    font-style: italic;
                    color: var(--info-color);
                }

                .status-indicator {
                    display: inline-block;
                    padding: 0.25rem 0.75rem;
                    border-radius: var(--border-radius);
                    font-size: 0.9rem;
                    font-weight: 500;
                    margin-bottom: 1rem;
                }

                .status-connected {
                    background: var(--success-bg);
                    color: var(--success-color);
                }

                .status-error {
                    background: var(--error-bg);
                    color: var(--error-color);
                }

                .instructions {
                    background: var(--info-bg);
                    border: 1px solid var(--info-border);
                    border-radius: var(--border-radius);
                    padding: 1.5rem;
                    margin-top: 2rem;
                }

                .instructions h3 {
                    color: var(--info-color);
                    margin-top: 0;
                }

                .instructions ol {
                    margin: 1rem 0 0 0;
                    padding-left: 1.5rem;
                }

                .instructions li {
                    margin-bottom: 0.5rem;
                    color: var(--text-muted);
                }

                @media (max-width: 768px) {
                    .demo-content {
                        padding: 2rem 1rem;
                    }

                    .form-row {
                        flex-direction: column;
                        align-items: stretch;
                    }

                    .form-row input {
                        width: 100%;
                    }
                }
            </style>
        </head>
        <body>
            <div class="demo-container">
                <div class="demo-content">
                    <div class="demo-header">
                        <h1>📡 GraphQL Subscription Demo</h1>
                        <p>This demonstrates GraphQL subscriptions using Server-Sent Events</p>
                    </div>

                    <div class="demo-section">
                        <h3>Send Message via GraphQL Mutation</h3>
                        <div class="form-row">
                            <input type="text" id="messageInput" placeholder="Enter your message" class="form-control" />
                            <button onclick="sendGraphQLMessage()" class="btn btn-primary">Send via GraphQL</button>
                        </div>
                    </div>

                    <div class="demo-section">
                        <h3>Send Message via HTTP</h3>
                        <div class="form-row">
                            <input type="text" id="httpMessageInput" placeholder="Enter your message" class="form-control" />
                            <button onclick="sendHttpMessage()" class="btn btn-secondary">Send via HTTP</button>
                        </div>
                    </div>

                    <div class="demo-section">
                        <h3>Live Messages (GraphQL Subscription via SSE)</h3>
                        <div id="status" class="status-indicator status-connected">Connecting to subscription...</div>
                        <div class="messages-container" id="messages"></div>
                    </div>

                    <div class="instructions">
                        <h3>📋 Instructions</h3>
                        <ol>
                            <li>The page automatically subscribes to the GraphQL subscription using SSE</li>
                            <li>Use either button to send messages</li>
                            <li>Messages will appear in real-time via the subscription</li>
                            <li>Open multiple browser tabs to see multi-client broadcasting</li>
                        </ol>
                    </div>
                </div>
            </div>

            <script>
                let messageCount = 0;

                // Subscribe to GraphQL subscription via SSE using EventSource
                function subscribeToMessages() {
                    const subscriptionQuery = {
                        query: \`subscription { liveMessages }\`
                    };

                    const eventSource = new EventSource('/graphql/sse?query=' + encodeURIComponent(subscriptionQuery.query));

                    eventSource.onopen = function(event) {
                        document.getElementById('status').textContent = 'Connected to subscription ✓';
                        document.getElementById('status').className = 'status-indicator status-connected';
                        console.log('SSE connection opened');
                    };

                    eventSource.onmessage = function(event) {
                        try {
                            const data = JSON.parse(event.data);
                            if (data.data && data.data.liveMessages) {
                                displayMessage(data.data.liveMessages);
                            }
                        } catch (e) {
                            console.log('Non-JSON data:', event.data);
                        }
                    };

                    eventSource.onerror = function(event) {
                        document.getElementById('status').textContent = 'Connection failed or lost';
                        document.getElementById('status').className = 'status-indicator status-error';
                        console.error('SSE connection error:', event);
                    };

                    // Store the EventSource instance for potential cleanup
                    window.subscriptionEventSource = eventSource;
                }

                function displayMessage(message) {
                    const messagesDiv = document.getElementById('messages');
                    const messageEl = document.createElement('div');
                    messageEl.className = 'message';

                    let messageData;
                    try {
                        // Handle both JSON string and already-parsed object
                        if (typeof message === 'string') {
                            messageData = JSON.parse(message);
                        } else if (typeof message === 'object' && message !== null) {
                            messageData = message;
                        } else {
                            throw new Error('Invalid message format');
                        }
                        
                        if (messageData.type === 'join') {
                            // Special styling for join messages
                            messageEl.className = 'message join-message';
                            messageEl.innerHTML = \`
                                <em>[\${messageData.timestamp}] \${messageData.text}</em>
                            \`;
                        } else {
                            // Regular message styling
                            messageEl.innerHTML = \`
                                <strong>#\${messageData.id}</strong> [\${messageData.timestamp}]<br>
                                \${messageData.text}
                            \`;
                        }
                    } catch (e) {
                        messageEl.textContent = \`[\${new Date().toISOString()}] \${message}\`;
                    }

                    messagesDiv.appendChild(messageEl);
                    messagesDiv.scrollTop = messagesDiv.scrollHeight;
                    messageCount++;
                }

                function sendGraphQLMessage() {
                    const input = document.getElementById('messageInput');
                    const message = input.value.trim();
                    if (!message) return;

                    const mutation = {
                        query: \`mutation { sendMessage(text: "\${message}") }\`
                    };

                    fetch('/graphql', {
                        method: 'POST',
                        headers: {
                            'Content-Type': 'application/json',
                        },
                        body: JSON.stringify(mutation)
                    })
                    .then(response => response.json())
                    .then(data => {
                        console.log('GraphQL mutation result:', data);
                        input.value = '';
                    })
                    .catch(error => {
                        console.error('GraphQL mutation error:', error);
                        alert('Failed to send message: ' + error.message);
                    });
                }

                function sendHttpMessage() {
                    const input = document.getElementById('httpMessageInput');
                    const message = input.value.trim();
                    if (!message) return;

                    fetch('/trigger-message', {
                        method: 'POST',
                        headers: {
                            'Content-Type': 'text/plain',
                        },
                        body: message
                    })
                    .then(response => response.json())
                    .then(data => {
                        console.log('HTTP trigger result:', data);
                        input.value = '';
                    })
                    .catch(error => {
                        console.error('HTTP trigger error:', error);
                        alert('Failed to send message: ' + error.message);
                    });
                }

                // Handle Enter key in input fields
                document.getElementById('messageInput').addEventListener('keypress', function(e) {
                    if (e.key === 'Enter') sendGraphQLMessage();
                });

                document.getElementById('httpMessageInput').addEventListener('keypress', function(e) {
                    if (e.key === 'Enter') sendHttpMessage();
                });

                // Start the subscription when page loads
                subscribeToMessages();
            </script>
        </body>
        </html>`);
}

// Initialization function - called when script is loaded or updated
function init(context) {
  try {
    console.log(
      `Initializing graphql_subscription_demo.js script at ${new Date().toISOString()}`,
    );
    console.log(`Init context: ${JSON.stringify(context)}`);

    // Register a GraphQL subscription (external - used by clients)
    graphQLRegistry.registerSubscription(
      "liveMessages",
      "type Subscription { liveMessages: String }",
      "liveMessagesResolver",
      "external",
    );

    // Register a GraphQL mutation to trigger the subscription (external - used by clients)
    graphQLRegistry.registerMutation(
      "sendMessage",
      "type Mutation { sendMessage(text: String!): String }",
      "sendMessageResolver",
      "external",
    );

    // Register HTTP endpoints for testing
    routeRegistry.registerRoute(
      "/trigger-message",
      "triggerMessageHandler",
      "POST",
    );

    // Test page to demonstrate subscription usage
    routeRegistry.registerRoute(
      "/subscription-demo",
      "subscriptionDemoPage",
      "GET",
    );

    console.log("GraphQL subscription example script initialized successfully");

    return {
      success: true,
      message: "GraphQL subscription example script initialized successfully",
      registeredEndpoints: 2,
      registeredGraphQLOperations: 2,
    };
  } catch (error) {
    console.error(
      `GraphQL subscription example script initialization failed: ${error.message}`,
    );
    throw error;
  }
}
