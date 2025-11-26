// Real-Time Chat Application
// Demonstrates GraphQL subscriptions with filtered messaging, persistent storage, and authentication

// ============================================
// Storage Layer - Helper Functions
// ============================================

function loadChannels() {
  try {
    const data = sharedStorage.getItem("chat:channels");
    return data ? JSON.parse(data) : [];
  } catch (error) {
    console.error("Error loading channels: " + error);
    return [];
  }
}

function saveChannels(channels) {
  try {
    sharedStorage.setItem("chat:channels", JSON.stringify(channels));
    return true;
  } catch (error) {
    console.error("Error saving channels: " + error);
    return false;
  }
}

function loadMessages(channelId, limit) {
  try {
    const key = "chat:messages:" + channelId;
    const data = sharedStorage.getItem(key);
    const messages = data ? JSON.parse(data) : [];

    // Return last N messages
    if (limit && messages.length > limit) {
      return messages.slice(-limit);
    }
    return messages;
  } catch (error) {
    console.error(
      "Error loading messages for channel " + channelId + ": " + error,
    );
    return [];
  }
}

function saveMessage(channelId, message) {
  try {
    const key = "chat:messages:" + channelId;
    const messages = loadMessages(channelId);
    messages.push(message);

    // Keep only last 1000 messages per channel to prevent unbounded growth
    const trimmedMessages =
      messages.length > 1000 ? messages.slice(-1000) : messages;

    sharedStorage.setItem(key, JSON.stringify(trimmedMessages));
    return true;
  } catch (error) {
    console.error(
      "Error saving message to channel " + channelId + ": " + error,
    );
    return false;
  }
}

// ============================================
// GraphQL Query Resolvers
// ============================================

function channelsResolver(context) {
  const req = context.request || {};
  const args = context.args || {};
  try {
    // Require authentication
    req.auth.requireAuth();

    const channels = loadChannels();
    return channels;
  } catch (error) {
    console.error("Error in channelsResolver: " + error);
    throw new Error("Failed to load channels: " + error.message);
  }
}

function messagesResolver(context) {
  const req = context.request || {};
  const args = context.args || {};
  try {
    // Require authentication
    req.auth.requireAuth();

    const channelId = args.channelId;
    const limit = args.limit || 50;

    if (!channelId) {
      throw new Error("channelId is required");
    }

    const messages = loadMessages(channelId, limit);
    return messages;
  } catch (error) {
    console.error("Error in messagesResolver: " + error);
    throw new Error("Failed to load messages: " + error.message);
  }
}

function currentUserResolver(context) {
  const req = context.request || {};
  const args = context.args || {};
  try {
    const user = req.auth.requireAuth();
    return {
      id: user.id,
      name: user.name || user.email,
      email: user.email,
    };
  } catch (error) {
    console.error("Error in currentUserResolver: " + error);
    throw new Error("Authentication required: " + error.message);
  }
}

// ============================================
// GraphQL Mutation Resolvers
// ============================================

function createChannelResolver(context) {
  const req = context.request || {};
  const args = context.args || {};
  try {
    // Require authentication
    const user = req.auth.requireAuth();

    const name = args.name;
    const isPrivate = args.isPrivate || false;

    if (!name || name.trim().length === 0) {
      throw new Error("Channel name is required");
    }

    if (name.length > 50) {
      throw new Error("Channel name must be 50 characters or less");
    }

    const channels = loadChannels();

    // Check if channel with same name exists
    const existing = channels.find(function (ch) {
      return ch.name.toLowerCase() === name.toLowerCase();
    });

    if (existing) {
      throw new Error("Channel with this name already exists");
    }

    // Create new channel
    const channelId =
      "channel_" + Date.now() + "_" + Math.random().toString(36).substr(2, 9);
    const newChannel = {
      id: channelId,
      name: name,
      isPrivate: isPrivate,
      createdBy: user.name || user.email,
      createdAt: new Date().toISOString(),
    };

    channels.push(newChannel);
    saveChannels(channels);

    console.log(
      "Channel created: " + name + " by " + (user.name || user.email),
    );

    return newChannel;
  } catch (error) {
    console.error("Error in createChannelResolver: " + error);
    throw new Error("Failed to create channel: " + error.message);
  }
}

function sendMessageResolver(context) {
  const req = context.request || {};
  const args = context.args || {};
  try {
    // Require authentication
    const user = req.auth.requireAuth();

    const channelId = args.channelId;
    const text = args.text;

    if (!channelId) {
      throw new Error("channelId is required");
    }

    if (!text || text.trim().length === 0) {
      throw new Error("Message text is required");
    }

    if (text.length > 2000) {
      throw new Error("Message must be 2000 characters or less");
    }

    // Verify channel exists
    const channels = loadChannels();
    const channel = channels.find(function (ch) {
      return ch.id === channelId;
    });

    if (!channel) {
      throw new Error("Channel not found");
    }

    // Create message
    const message = {
      id: "msg_" + Date.now() + "_" + Math.random().toString(36).substr(2, 9),
      sender: user.name || user.email,
      text: text,
      timestamp: new Date().toISOString(),
      type: "user_message",
    };

    // Save to storage
    saveMessage(channelId, message);

    // Broadcast to subscribers of this channel
    // Send the message object directly (not as JSON string)
    // The GraphQL subscription will wrap it in the response format
    const broadcastData = JSON.stringify(message);
    const filterCriteria = JSON.stringify({ channelId: channelId });

    graphQLRegistry.sendSubscriptionMessageFiltered(
      "chatUpdates",
      broadcastData,
      filterCriteria,
    );

    console.log(
      "Message sent to channel " + channelId + " by " + message.sender,
    );

    return message;
  } catch (error) {
    console.error("Error in sendMessageResolver: " + error);
    throw new Error("Failed to send message: " + error.message);
  }
}

// ============================================
// GraphQL Subscription Resolver
// ============================================

function chatUpdatesResolver(context) {
  try {
    const req = context.request || {};
    const args = context.args || {};
    const queryParams = req.query || {};
    const channelId = args.channelId || queryParams.channelId;

    if (!channelId) {
      // Silent return - this is likely a schema introspection call or connection setup
      // Only log if there are other query params (indicating it might be an error)
      if (Object.keys(queryParams).length > 0) {
        console.error(
          "channelId not found in req.query:",
          JSON.stringify(queryParams),
        );
      }
      return {};
    }

    // Check authentication manually (req.auth.requireAuth() not available in stream customization context)
    if (!req.auth || !req.auth.isAuthenticated) {
      console.error("Authentication check failed for channel subscription");
      throw new Error("Authentication required");
    }

    const user = {
      id: req.auth.userId,
      name: req.auth.name,
      email: req.auth.email,
    };

    console.log(
      "User " +
        (user.name || user.email) +
        " subscribed to channel: " +
        channelId,
    );

    // Return an object with string values for filtering
    // This will be converted to HashMap<String, String> and stored as connection metadata
    // sendSubscriptionMessageFiltered will match against these key-value pairs
    var filterCriteria = {};
    filterCriteria.channelId = String(channelId);
    return filterCriteria;
  } catch (error) {
    console.error("Error in chatUpdatesResolver: " + error);
    // Don't throw error - just return empty filter criteria
    // This allows the subscription to continue but won't receive filtered messages
    return {};
  }
}

// ============================================
// HTTP Handler - Chat Interface
// ============================================

function chatInterfaceHandler(context) {
  try {
    const req = context.request || {};
    // Require authentication
    const user = req.auth.requireAuth();

    const html = `<!DOCTYPE html>
<html lang="en">
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <title>Real-Time Chat</title>
    <link rel="stylesheet" href="/engine.css">
    <style>
        body {
            margin: 0;
            padding: 0;
            font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, sans-serif;
            background: linear-gradient(135deg, #667eea 0%, #764ba2 100%);
            min-height: 100vh;
            display: flex;
            align-items: center;
            justify-content: center;
        }
        
        .chat-container {
            width: 90%;
            max-width: 1200px;
            height: 80vh;
            background: white;
            border-radius: 12px;
            box-shadow: 0 10px 40px rgba(0,0,0,0.2);
            display: flex;
            flex-direction: column;
            overflow: hidden;
        }
        
        .chat-header {
            background: #667eea;
            color: white;
            padding: 1rem 1.5rem;
            display: flex;
            align-items: center;
            justify-content: space-between;
        }
        
        .chat-header h1 {
            margin: 0;
            font-size: 1.5rem;
        }
        
        .user-info {
            font-size: 0.9rem;
            opacity: 0.9;
        }
        
        .chat-body {
            flex: 1;
            display: flex;
            overflow: hidden;
        }
        
        .channels-sidebar {
            width: 250px;
            background: #f7f7f7;
            border-right: 1px solid #ddd;
            display: flex;
            flex-direction: column;
        }
        
        .channels-header {
            padding: 1rem;
            font-weight: 600;
            border-bottom: 1px solid #ddd;
        }
        
        .channels-list {
            flex: 1;
            overflow-y: auto;
        }
        
        .channel-item {
            padding: 0.75rem 1rem;
            cursor: pointer;
            border-bottom: 1px solid #eee;
            transition: background 0.2s;
        }
        
        .channel-item:hover {
            background: #e8e8e8;
        }
        
        .channel-item.active {
            background: #667eea;
            color: white;
        }
        
        .channel-actions {
            padding: 1rem;
            border-top: 1px solid #ddd;
        }
        
        .messages-area {
            flex: 1;
            display: flex;
            flex-direction: column;
        }
        
        .messages-header {
            padding: 1rem 1.5rem;
            border-bottom: 1px solid #ddd;
            font-weight: 600;
            background: #fafafa;
        }
        
        .messages-container {
            flex: 1;
            overflow-y: auto;
            padding: 1.5rem;
            background: #f9f9f9;
        }
        
        .message {
            margin-bottom: 1rem;
            padding: 0.75rem 1rem;
            background: white;
            border-radius: 8px;
            box-shadow: 0 1px 3px rgba(0,0,0,0.1);
        }
        
        .message.system {
            background: #fff3cd;
            border-left: 3px solid #ffc107;
            font-style: italic;
        }
        
        .message-sender {
            font-weight: 600;
            color: #667eea;
            margin-bottom: 0.25rem;
        }
        
        .message-text {
            margin: 0.25rem 0;
            line-height: 1.5;
        }
        
        .message-timestamp {
            font-size: 0.75rem;
            color: #888;
        }
        
        .message-input-area {
            padding: 1rem 1.5rem;
            border-top: 1px solid #ddd;
            background: white;
        }
        
        .message-form {
            display: flex;
            gap: 0.5rem;
        }
        
        .message-input {
            flex: 1;
            padding: 0.75rem;
            border: 1px solid #ddd;
            border-radius: 6px;
            font-size: 1rem;
        }
        
        .btn {
            padding: 0.75rem 1.5rem;
            border: none;
            border-radius: 6px;
            font-size: 1rem;
            cursor: pointer;
            transition: background 0.2s;
        }
        
        .btn-primary {
            background: #667eea;
            color: white;
        }
        
        .btn-primary:hover {
            background: #5568d3;
        }
        
        .btn-secondary {
            background: #6c757d;
            color: white;
        }
        
        .btn-secondary:hover {
            background: #5a6268;
        }
        
        .status {
            font-size: 0.85rem;
            padding: 0.5rem 1rem;
            background: #d4edda;
            color: #155724;
            border-bottom: 1px solid #c3e6cb;
        }
        
        .status.disconnected {
            background: #f8d7da;
            color: #721c24;
            border-bottom: 1px solid #f5c6cb;
        }
        
        .loading {
            text-align: center;
            padding: 2rem;
            color: #888;
        }
    </style>
</head>
<body>
    <div class="chat-container">
        <div class="chat-header">
            <h1>💬 Real-Time Chat</h1>
            <div class="user-info">Logged in as: ${user.name || user.email}</div>
        </div>
        
        <div id="status" class="status">Connecting...</div>
        
        <div class="chat-body">
            <div class="channels-sidebar">
                <div class="channels-header">Channels</div>
                <div id="channels-list" class="channels-list">
                    <div class="loading">Loading channels...</div>
                </div>
                <div class="channel-actions">
                    <button class="btn btn-secondary" onclick="createNewChannel()">+ New Channel</button>
                </div>
            </div>
            
            <div class="messages-area">
                <div id="messages-header" class="messages-header">Select a channel</div>
                <div id="messages-container" class="messages-container">
                    <div class="loading">No channel selected</div>
                </div>
                <div class="message-input-area">
                    <form id="message-form" class="message-form" onsubmit="sendMessage(event)">
                        <input 
                            type="text" 
                            id="message-input" 
                            class="message-input" 
                            placeholder="Type a message..." 
                            disabled
                            maxlength="2000"
                        />
                        <button type="submit" class="btn btn-primary" disabled>Send</button>
                    </form>
                </div>
            </div>
        </div>
    </div>

    <script>
        let currentChannel = null;
        let channels = [];
        
        // Load channels on startup
        async function loadChannels() {
            try {
                const response = await fetch('/graphql', {
                  method: 'POST',
                  headers: { 'Content-Type': 'application/json' },
                  body: JSON.stringify({
                    query:
                      "query { channels { id name isPrivate createdBy createdAt } }",
                  }),
                });
                
                const result = await response.json();
                
                if (result.data && result.data.channels) {
                    channels = result.data.channels;
                    renderChannels();
                    
                    // Auto-select system channel
                    if (channels.length > 0) {
                        const systemChannel = channels.find(ch => ch.id === 'system');
                        if (systemChannel) {
                            selectChannel(systemChannel.id);
                        }
                    }
                }
            } catch (error) {
                console.error('Error loading channels:', error);
                updateStatus('Failed to load channels', true);
            }
        }
        
        function renderChannels() {
            const list = document.getElementById('channels-list');
            list.innerHTML = channels.map(ch => 
                \`<div class="channel-item \${currentChannel && currentChannel.id === ch.id ? 'active' : ''}" 
                     onclick="selectChannel('\${ch.id}')">
                    \${ch.name}
                </div>\`
            ).join('');
        }
        
        async function selectChannel(channelId) {
            // Close existing subscription using abort controller
            if (currentSubscriptionController) {
                currentSubscriptionController.abort();
                currentSubscriptionController = null;
            }
            
            const channel = channels.find(ch => ch.id === channelId);
            if (!channel) return;
            
            currentChannel = channel;
            renderChannels();
            
            // Update header
            document.getElementById('messages-header').textContent = channel.name;
            
            // Enable message input
            document.getElementById('message-input').disabled = false;
            document.querySelector('#message-form button').disabled = false;
            
            // Load message history
            await loadMessages(channelId);
            
            // Subscribe to real-time updates
            subscribeToChannel(channelId);
        }
        
        async function loadMessages(channelId) {
            try {
                const response = await fetch('/graphql', {
                  method: 'POST',
                  headers: { 'Content-Type': 'application/json' },
                  body: JSON.stringify({
                    query:
                      "query($channelId: String!, $limit: Int) { messages(channelId: $channelId, limit: $limit) { id sender text timestamp type } }",
                    variables: { channelId: channelId, limit: 50 },
                  }),
                });
                
                const result = await response.json();
                
                if (result.data && result.data.messages) {
                    const messages = result.data.messages;
                    renderMessages(messages);
                }
            } catch (error) {
                console.error('Error loading messages:', error);
            }
        }
        
        function renderMessages(messages) {
            const container = document.getElementById('messages-container');
            
            if (messages.length === 0) {
                container.innerHTML = '<div class="loading">No messages yet. Start the conversation!</div>';
                return;
            }
            
            container.innerHTML = messages.map(msg => {
                const date = new Date(msg.timestamp);
                const time = date.toLocaleTimeString();
                const isSystem = msg.type === 'system_message';
                
                return \`<div class="message \${isSystem ? 'system' : ''}">
                    <div class="message-sender">\${msg.sender}</div>
                    <div class="message-text">\${escapeHtml(msg.text)}</div>
                    <div class="message-timestamp">\${time}</div>
                </div>\`;
            }).join('');
            
            // Scroll to bottom
            container.scrollTop = container.scrollHeight;
        }
        
        let currentSubscriptionController = null;
        
        function subscribeToChannel(channelId) {
            // Cancel any existing subscription
            if (currentSubscriptionController) {
                currentSubscriptionController.abort();
                currentSubscriptionController = null;
            }
            
            updateStatus('Connecting to ' + currentChannel.name + '...');
            
            // Create abort controller for this subscription
            currentSubscriptionController = new AbortController();
            
            // Subscribe via GraphQL SSE endpoint using explicit GraphQL args
            const subscriptionQuery = {
              query:
                "subscription ($channelId: String!) { chatUpdates(channelId: $channelId) { id sender text timestamp type } }",
              variables: { channelId },
            };
            
            fetch('/graphql/sse?query=' + encodeURIComponent(subscriptionQuery.query) + '&variables=' + encodeURIComponent(JSON.stringify(subscriptionQuery.variables)), {
                method: 'GET',
                headers: { 
                    'Accept': 'text/event-stream',
                    'Cache-Control': 'no-cache'
                },
                signal: currentSubscriptionController.signal,
                // Safari compatibility: disable keepalive
                keepalive: false
            })
            .then(response => {
                if (!response.ok) {
                    console.error('Subscription failed with status:', response.status, response.statusText);
                    return response.text().then(text => {
                        console.error('Response body:', text);
                        throw new Error('Subscription failed: ' + response.status + ' ' + response.statusText);
                    });
                }
                
                updateStatus('Connected to ' + currentChannel.name);
                
                // Safari compatibility check
                if (!response.body || !response.body.getReader) {
                    console.error('Browser does not support ReadableStream');
                    throw new Error('Browser does not support streaming responses');
                }
                
                const reader = response.body.getReader();
                const decoder = new TextDecoder();
                let buffer = '';
                
                function readStream() {
                    reader.read().then(({ done, value }) => {
                        if (done) {
                            console.log('Subscription stream ended normally');
                            updateStatus('Disconnected from ' + currentChannel.name, true);
                            
                            // Only reconnect if still on same channel and not manually cancelled
                            setTimeout(() => {
                                if (currentChannel && currentChannel.id === channelId && currentSubscriptionController) {
                                    console.log('Auto-reconnecting to', currentChannel.name);
                                    subscribeToChannel(channelId);
                                }
                            }, 2000);
                            return;
                        }
                        
                        // Append to buffer and process complete lines
                        buffer += decoder.decode(value, { stream: true });
                        const lines = buffer.split('\\n');
                        
                        // Keep the last incomplete line in buffer
                        buffer = lines.pop() || '';
                        
                        lines.forEach(line => {
                            if (line.startsWith('data: ')) {
                                try {
                                    const data = JSON.parse(line.substring(6));
                                    
                                    // Handle GraphQL response format
                                    if (data.data && data.data.chatUpdates) {
                                        // chatUpdates might be a JSON string or an object
                                        let message = data.data.chatUpdates;
                                        
                                        // If it's a string, parse it
                                        if (typeof message === 'string') {
                                            message = JSON.parse(message);
                                        }
                                        
                                        // Only add message if it has content (not empty object)
                                        if (message && message.id) {
                                            addMessage(message);
                                        }
                                    } else if (data.errors && data.errors.length > 0) {
                                        console.error('GraphQL subscription error:', JSON.stringify(data.errors, null, 2));
                                    } else if (data.data) {
                                        // Subscription connected successfully (initial response may have null data)
                                        console.log('Subscription response:', data);
                                    }
                                } catch (error) {
                                    console.error('Error parsing SSE message:', error, 'Line:', line);
                                }
                            }
                        });
                        
                        readStream();
                    }).catch(error => {
                        // Ignore AbortError - this is expected when we cancel subscriptions
                        if (error.name === 'AbortError') {
                            console.log('Subscription cancelled');
                            return;
                        }
                        
                        console.error('Stream read error:', error);
                        console.error('Browser:', navigator.userAgent);
                        updateStatus('Connection interrupted', true);
                        
                        // Only reconnect if still on same channel
                        setTimeout(() => {
                            if (currentChannel && currentChannel.id === channelId) {
                                console.log('Reconnecting after error...');
                                subscribeToChannel(channelId);
                            }
                        }, 2000);
                    });
                }
                
                readStream();
            })
            .catch(error => {
                // Ignore AbortError - this is expected when we cancel subscriptions
                if (error.name === 'AbortError') {
                    console.log('Subscription fetch cancelled');
                    return;
                }
                
                console.error('Subscription error:', error);
                console.error('Browser:', navigator.userAgent);
                updateStatus('Failed to connect to ' + currentChannel.name, true);
                
                // Only reconnect if still on same channel
                setTimeout(() => {
                    if (currentChannel && currentChannel.id === channelId) {
                        console.log('Reconnecting after connection failure...');
                        subscribeToChannel(channelId);
                    }
                }, 2000);
            });
        }
        
        function addMessage(message) {
            const container = document.getElementById('messages-container');
            
            // Remove "no messages" placeholder if present
            if (container.querySelector('.loading')) {
                container.innerHTML = '';
            }
            
            const date = new Date(message.timestamp);
            const time = date.toLocaleTimeString();
            const isSystem = message.type === 'system_message';
            
            const messageEl = document.createElement('div');
            messageEl.className = 'message' + (isSystem ? ' system' : '');
            messageEl.innerHTML = \`
                <div class="message-sender">\${message.sender}</div>
                <div class="message-text">\${escapeHtml(message.text)}</div>
                <div class="message-timestamp">\${time}</div>
            \`;
            
            container.appendChild(messageEl);
            container.scrollTop = container.scrollHeight;
        }
        
        async function sendMessage(event) {
            event.preventDefault();
            
            const input = document.getElementById('message-input');
            const text = input.value.trim();
            
            if (!text || !currentChannel) return;
            
            try {
                const response = await fetch('/graphql', {
                  method: 'POST',
                  headers: { 'Content-Type': 'application/json' },
                  body: JSON.stringify({
                    query:
                      "mutation($channelId: String!, $text: String!) { sendMessage(channelId: $channelId, text: $text) { id sender text timestamp type } }",
                    variables: { channelId: currentChannel.id, text: text },
                  }),
                });
                
                const result = await response.json();
                
                if (result.errors) {
                    alert('Error sending message: ' + result.errors[0].message);
                } else {
                    input.value = '';
                }
            } catch (error) {
                console.error('Error sending message:', error);
                alert('Failed to send message');
            }
        }
        
        async function createNewChannel() {
            const name = prompt('Enter channel name:');
            if (!name) return;
            
            try {
                const response = await fetch('/graphql', {
                  method: 'POST',
                  headers: { 'Content-Type': 'application/json' },
                  body: JSON.stringify({
                    query:
                      "mutation($name: String!, $isPrivate: Boolean) { createChannel(name: $name, isPrivate: $isPrivate) { id name isPrivate createdBy createdAt } }",
                    variables: { name: name, isPrivate: false },
                  }),
                });
                
                const result = await response.json();
                
                if (result.errors) {
                    alert('Error creating channel: ' + result.errors[0].message);
                } else {
                    // Reload channels
                    await loadChannels();
                    
                    // Select the new channel
                    const newChannel = result.data.createChannel;
                    selectChannel(newChannel.id);
                }
            } catch (error) {
                console.error('Error creating channel:', error);
                alert('Failed to create channel');
            }
        }
        
        function updateStatus(message, isError) {
            const status = document.getElementById('status');
            status.textContent = message;
            status.className = 'status' + (isError ? ' disconnected' : '');
        }
        
        function escapeHtml(text) {
            const div = document.createElement('div');
            div.textContent = text;
            return div.innerHTML;
        }
        
        // Initialize
        loadChannels();
    </script>
</body>
</html>`;

    return {
      status: 200,
      body: html,
      contentType: "text/html; charset=UTF-8",
    };
  } catch (error) {
    // User not authenticated, redirect to login
    const req = context.request || {};
    const currentPath = encodeURIComponent(req.path || "/chat");
    const loginUrl = "/auth/login?redirect=" + currentPath;

    return {
      status: 302,
      headers: {
        Location: loginUrl,
      },
      body: "",
      contentType: "text/plain; charset=UTF-8",
    };
  }
}

// ============================================
// Initialization
// ============================================

function init(context) {
  console.log("Initializing chat_app.js at " + new Date().toISOString());

  try {
    // Initialize system channel if it doesn't exist
    let channels = loadChannels();
    const systemChannelExists = channels.some(function (ch) {
      return ch.id === "system";
    });

    if (!systemChannelExists) {
      const systemChannel = {
        id: "system",
        name: "System Announcements",
        isPrivate: false,
        createdBy: "System",
        createdAt: new Date().toISOString(),
      };

      channels.push(systemChannel);
      saveChannels(channels);

      console.log("System channel created");
    }

    // Register GraphQL queries
    graphQLRegistry.registerQuery(
      "channels",
      "type Channel { id: String!, name: String!, isPrivate: Boolean!, createdBy: String!, createdAt: String! } type Query { channels: [Channel!]! }",
      "channelsResolver",
    );

    graphQLRegistry.registerQuery(
      "messages",
      "type Message { id: String!, sender: String!, text: String!, timestamp: String!, type: String! } type Query { messages(channelId: String!, limit: Int): [Message!]! }",
      "messagesResolver",
    );

    graphQLRegistry.registerQuery(
      "currentUser",
      "type User { id: String!, name: String!, email: String! } type Query { currentUser: User! }",
      "currentUserResolver",
    );

    // Register GraphQL mutations
    graphQLRegistry.registerMutation(
      "createChannel",
      "type Mutation { createChannel(name: String!, isPrivate: Boolean): Channel! }",
      "createChannelResolver",
    );

    graphQLRegistry.registerMutation(
      "sendMessage",
      "type Mutation { sendMessage(channelId: String!, text: String!): Message! }",
      "sendMessageResolver",
    );

    // Register GraphQL subscription with explicit channelId argument
    graphQLRegistry.registerSubscription(
      "chatUpdates",
      "type Subscription { chatUpdates(channelId: String!): Message }",
      "chatUpdatesResolver",
    );

    // Register HTTP route for chat interface
    routeRegistry.registerRoute("/chat", "chatInterfaceHandler", "GET");

    console.log("Chat application initialized successfully");
    console.log("Access the chat at /chat (authentication required)");

    return {
      success: true,
      message: "Chat application initialized",
      endpoints: ["/chat"],
      graphqlOperations: {
        queries: ["channels", "messages", "currentUser"],
        mutations: ["createChannel", "sendMessage"],
        subscriptions: ["chatUpdates"],
      },
    };
  } catch (error) {
    console.error("Error initializing chat application: " + error);
    throw error;
  }
}
