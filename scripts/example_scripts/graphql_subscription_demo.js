// GraphQL Subscription Example with Streaming
// This script shows how to create GraphQL subscriptions that work with Server-Sent Events

// The subscription resolver - called when a client subscribes
function liveMessagesResolver() {
  writeLog("Client subscribed to liveMessages");
  return "Subscription initialized - waiting for messages...";
}

// The mutation resolver - triggers subscription messages
function sendMessageResolver(args) {
  const message = args.text;
  const timestamp = new Date().toISOString();

  // Create the message data
  const messageData = {
    id: Math.random().toString(36).substr(2, 9),
    text: message,
    timestamp: timestamp,
    sender: "system",
  };

  writeLog(`Sending message to liveMessages subscribers: ${message}`);

  // Send the message to the subscription using the convenience function
  // This will broadcast to all clients subscribed to the 'liveMessages' subscription
  sendSubscriptionMessage("liveMessages", JSON.stringify(messageData));

  return `Message sent: ${message}`;
}

// Optional: You can also use the lower-level API
// sendStreamMessageToPath("/graphql/subscription/liveMessages", JSON.stringify(messageData));

function triggerMessageHandler(req) {
  const message = req.body || "Hello from HTTP trigger!";

  // Trigger the subscription by calling the mutation resolver
  sendMessageResolver({ text: message });

  return {
    status: 200,
    body: JSON.stringify({ success: true, message: "Message broadcasted" }),
    contentType: "application/json",
  };
}

function subscriptionDemoPage(req) {
  return {
    status: 200,
    body: `
        <!DOCTYPE html>
        <html>
        <head>
            <title>GraphQL Subscription Demo</title>
            <style>
                body { font-family: Arial, sans-serif; margin: 40px; }
                .container { max-width: 800px; }
                .messages { border: 1px solid #ddd; height: 300px; overflow-y: auto; padding: 10px; margin: 20px 0; }
                .message { margin: 5px 0; padding: 5px; background: #f5f5f5; border-radius: 3px; }
                input, button { padding: 10px; margin: 5px; }
                input { width: 300px; }
            </style>
        </head>
        <body>
            <div class="container">
                <h1>GraphQL Subscription Demo</h1>
                <p>This demonstrates GraphQL subscriptions using Server-Sent Events.</p>
                
                <h3>Send Message via GraphQL Mutation</h3>
                <input type="text" id="messageInput" placeholder="Enter your message" />
                <button onclick="sendGraphQLMessage()">Send via GraphQL</button>
                
                <h3>Send Message via HTTP</h3>
                <input type="text" id="httpMessageInput" placeholder="Enter your message" />
                <button onclick="sendHttpMessage()">Send via HTTP</button>
                
                <h3>Live Messages (GraphQL Subscription via SSE)</h3>
                <div id="status">Connecting to subscription...</div>
                <div class="messages" id="messages"></div>
                
                <h3>Instructions</h3>
                <ol>
                    <li>The page automatically subscribes to the GraphQL subscription using SSE</li>
                    <li>Use either button to send messages</li>
                    <li>Messages will appear in real-time via the subscription</li>
                    <li>Open multiple browser tabs to see multi-client broadcasting</li>
                </ol>
            </div>
            
            <script>
                let messageCount = 0;
                
                // Subscribe to GraphQL subscription via SSE
                function subscribeToMessages() {
                    const subscriptionQuery = {
                        query: \`subscription { liveMessages }\`
                    };
                    
                    fetch('/graphql/sse', {
                        method: 'POST',
                        headers: {
                            'Content-Type': 'application/json',
                        },
                        body: JSON.stringify(subscriptionQuery)
                    })
                    .then(response => {
                        if (!response.ok) {
                            throw new Error('Failed to start subscription');
                        }
                        
                        const reader = response.body.getReader();
                        const decoder = new TextDecoder();
                        
                        document.getElementById('status').textContent = 'Connected to subscription ✓';
                        
                        function readStream() {
                            reader.read().then(({ done, value }) => {
                                if (done) {
                                    document.getElementById('status').textContent = 'Subscription ended';
                                    return;
                                }
                                
                                const chunk = decoder.decode(value);
                                const lines = chunk.split('\\n');
                                
                                lines.forEach(line => {
                                    if (line.startsWith('data: ')) {
                                        try {
                                            const data = JSON.parse(line.slice(6));
                                            if (data.data && data.data.liveMessages) {
                                                displayMessage(data.data.liveMessages);
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
                    })
                    .catch(error => {
                        document.getElementById('status').textContent = 'Connection failed: ' + error.message;
                        console.error('Subscription error:', error);
                    });
                }
                
                function displayMessage(message) {
                    const messagesDiv = document.getElementById('messages');
                    const messageEl = document.createElement('div');
                    messageEl.className = 'message';
                    
                    try {
                        const messageData = JSON.parse(message);
                        messageEl.innerHTML = \`
                            <strong>#\${messageData.id}</strong> [\${messageData.timestamp}]<br>
                            \${messageData.text}
                        \`;
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
        </html>`,
    contentType: "text/html",
  };
}

// Initialization function - called when script is loaded or updated
function init(context) {
  try {
    writeLog(
      `Initializing graphql_subscription_demo.js script at ${new Date().toISOString()}`,
    );
    writeLog(`Init context: ${JSON.stringify(context)}`);

    // Register a GraphQL subscription
    registerGraphQLSubscription(
      "liveMessages",
      "type Subscription { liveMessages: String }",
      "liveMessagesResolver",
    );

    // Register a GraphQL mutation to trigger the subscription
    registerGraphQLMutation(
      "sendMessage",
      "type Mutation { sendMessage(text: String!): String }",
      "sendMessageResolver",
    );

    // Register HTTP endpoints for testing
    register("/trigger-message", "triggerMessageHandler", "POST");

    // Test page to demonstrate subscription usage
    register("/subscription-demo", "subscriptionDemoPage", "GET");

    writeLog("GraphQL subscription example script initialized successfully");

    return {
      success: true,
      message: "GraphQL subscription example script initialized successfully",
      registeredEndpoints: 2,
      registeredGraphQLOperations: 2,
    };
  } catch (error) {
    writeLog(
      `GraphQL subscription example script initialization failed: ${error.message}`,
    );
    throw error;
  }
}
