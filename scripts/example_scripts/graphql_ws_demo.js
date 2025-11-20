// GraphQL WebSocket Subscription Example
// This script demonstrates GraphQL subscriptions using WebSocket (graphql-transport-ws protocol)

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
    const user = req.auth.currentUser();
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

function triggerMessageHandler(context) {
  const req = context.request || {};
  const messageBody = req.body;
  const message =
    (typeof messageBody === "string" && messageBody.trim()) ||
    req.form?.message ||
    "Hello from HTTP trigger!";

  broadcastLiveMessage(message);

  return {
    status: 200,
    body: JSON.stringify({ success: true, message: "Message broadcasted" }),
    contentType: "application/json",
  };
}

function wsSubscriptionDemoPage(context) {
  return {
    status: 200,
    body: `
        <!DOCTYPE html>
        <html lang="en">
        <head>
            <meta charset="UTF-8">
            <meta name="viewport" content="width=device-width, initial-scale=1.0">
            <title>GraphQL WebSocket Demo - aiwebengine</title>
            <link rel="stylesheet" href="/engine.css">
            <link rel="icon" type="image/x-icon" href="/favicon.ico">
            <style>
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

                .status-connecting {
                    background: var(--warning-bg);
                    color: var(--warning-color);
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

                .instructions ol, .instructions ul {
                    margin: 1rem 0 0 0;
                    padding-left: 1.5rem;
                }

                .instructions li {
                    margin-bottom: 0.5rem;
                    color: var(--text-muted);
                }

                code {
                    background: rgba(0, 0, 0, 0.1);
                    padding: 0.2rem 0.4rem;
                    border-radius: 3px;
                    font-family: 'Courier New', monospace;
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
                        <h1>🔌 GraphQL WebSocket Demo</h1>
                        <p>This demonstrates GraphQL subscriptions using WebSocket (graphql-transport-ws protocol)</p>
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
                        <h3>Live Messages (GraphQL Subscription via WebSocket)</h3>
                        <div id="status" class="status-indicator status-connecting">Connecting to WebSocket...</div>
                        <div class="messages-container" id="messages"></div>
                    </div>

                    <div class="instructions">
                        <h3>📋 About This Demo</h3>
                        <ol>
                            <li>This page uses the <strong>graphql-transport-ws</strong> protocol over WebSocket</li>
                            <li>WebSocket connections support multiple concurrent subscriptions per connection</li>
                            <li>The connection includes automatic ping/pong keep-alive (30 second interval)</li>
                            <li>Messages sent by either method will appear in real-time via the subscription</li>
                            <li>Open multiple browser tabs to see multi-client broadcasting</li>
                        </ol>

                        <h3>🔧 Technical Details</h3>
                        <ul>
                            <li><strong>Endpoint:</strong> <code>ws://localhost:3000/graphql/ws</code></li>
                            <li><strong>Protocol:</strong> graphql-transport-ws</li>
                            <li><strong>Max Subscriptions:</strong> 20 per connection</li>
                            <li><strong>Keep-Alive:</strong> 30 second ping interval</li>
                            <li><strong>Authentication:</strong> Supported via connection_init payload</li>
                        </ul>

                        <h3>🌐 Also Available</h3>
                        <p>For SSE-based subscriptions, visit <a href="/subscription-demo">/subscription-demo</a></p>
                    </div>
                </div>
            </div>

            <script>
                let ws = null;
                let subscriptionId = null;
                let messageCount = 0;

                // Simple implementation of graphql-transport-ws protocol
                function connectWebSocket() {
                    const protocol = window.location.protocol === 'https:' ? 'wss:' : 'ws:';
                    const wsUrl = \`\${protocol}//\${window.location.host}/graphql/ws\`;
                    
                    updateStatus('Connecting to WebSocket...', 'connecting');
                    
                    ws = new WebSocket(wsUrl, 'graphql-transport-ws');

                    ws.onopen = () => {
                        console.log('WebSocket connected');
                        
                        // Send connection_init message
                        ws.send(JSON.stringify({
                            type: 'connection_init',
                            payload: {}
                        }));
                    };

                    ws.onmessage = (event) => {
                        const message = JSON.parse(event.data);
                        console.log('Received:', message);

                        switch (message.type) {
                            case 'connection_ack':
                                console.log('Connection acknowledged');
                                updateStatus('Connected to WebSocket ✓', 'connected');
                                subscribeToMessages();
                                break;

                            case 'next':
                                if (message.id === subscriptionId && message.payload?.data?.liveMessages) {
                                    displayMessage(message.payload.data.liveMessages);
                                }
                                break;

                            case 'error':
                                console.error('Subscription error:', message.payload);
                                updateStatus('Subscription error: ' + JSON.stringify(message.payload), 'error');
                                break;

                            case 'complete':
                                console.log('Subscription completed:', message.id);
                                if (message.id === subscriptionId) {
                                    updateStatus('Subscription completed', 'error');
                                }
                                break;

                            case 'ping':
                                // Respond to server ping with pong
                                ws.send(JSON.stringify({ type: 'pong' }));
                                break;

                            case 'pong':
                                console.log('Received pong');
                                break;
                        }
                    };

                    ws.onerror = (error) => {
                        console.error('WebSocket error:', error);
                        updateStatus('WebSocket error', 'error');
                    };

                    ws.onclose = () => {
                        console.log('WebSocket closed');
                        updateStatus('WebSocket disconnected', 'error');
                        
                        // Attempt to reconnect after 3 seconds
                        setTimeout(() => {
                            if (ws.readyState === WebSocket.CLOSED) {
                                console.log('Attempting to reconnect...');
                                connectWebSocket();
                            }
                        }, 3000);
                    };
                }

                function subscribeToMessages() {
                    subscriptionId = 'sub-' + Math.random().toString(36).substr(2, 9);
                    
                    const subscribeMessage = {
                        id: subscriptionId,
                        type: 'subscribe',
                        payload: {
                            query: 'subscription { liveMessages }'
                        }
                    };

                    ws.send(JSON.stringify(subscribeMessage));
                    console.log('Sent subscribe message:', subscribeMessage);
                }

                function updateStatus(text, state) {
                    const statusEl = document.getElementById('status');
                    statusEl.textContent = text;
                    statusEl.className = 'status-indicator status-' + state;
                }

                function displayMessage(message) {
                    const messagesDiv = document.getElementById('messages');
                    const messageEl = document.createElement('div');
                    messageEl.className = 'message';

                    let messageData;
                    try {
                        if (typeof message === 'string') {
                            messageData = JSON.parse(message);
                        } else if (typeof message === 'object' && message !== null) {
                            messageData = message;
                        } else {
                            throw new Error('Invalid message format');
                        }
                        
                        if (messageData.type === 'join') {
                            messageEl.className = 'message join-message';
                            messageEl.innerHTML = \`
                                <em>[\${messageData.timestamp}] \${messageData.text}</em>
                            \`;
                        } else {
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

                    fetch('/trigger-message-ws', {
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

                // Connect WebSocket when page loads
                connectWebSocket();
            </script>
        </body>
        </html>`,
    contentType: "text/html",
  };
}

// Initialization function - called when script is loaded or updated
function init(context) {
  try {
    console.log(
      `Initializing graphql_ws_demo.js script at ${new Date().toISOString()}`,
    );

    // Register a GraphQL subscription (reuses the same subscription from SSE demo)
    graphQLRegistry.registerSubscription(
      "liveMessages",
      "type Subscription { liveMessages: String }",
      "liveMessagesResolver",
    );

    // Register a GraphQL mutation to trigger the subscription
    graphQLRegistry.registerMutation(
      "sendMessage",
      "type Mutation { sendMessage(text: String!): String }",
      "sendMessageResolver",
    );

    // Register HTTP endpoints for testing
    routeRegistry.registerRoute(
      "/trigger-message-ws",
      "triggerMessageHandler",
      "POST",
    );

    // WebSocket demo page
    routeRegistry.registerRoute("/ws-demo", "wsSubscriptionDemoPage", "GET");

    console.log("GraphQL WebSocket example script initialized successfully");

    return {
      success: true,
      message: "GraphQL WebSocket example script initialized successfully",
      registeredEndpoints: 2,
      registeredGraphQLOperations: 2,
    };
  } catch (error) {
    console.error(
      `GraphQL WebSocket example script initialization failed: ${error.message}`,
    );
    throw error;
  }
}
