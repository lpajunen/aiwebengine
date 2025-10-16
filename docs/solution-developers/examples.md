# Examples

This page showcases practical examples of aiwebengine scripts that demonstrate common patterns and use cases. These examples show how to build web applications using JavaScript handlers, from simple APIs to interactive forms.

## Getting Started with Examples

### Running Examples Locally

1. **Start the server**:

   ```bash
   cargo run
   ```

2. **Upload scripts via the editor**:
   - Open `http://localhost:3000/editor`
   - Create a new script
   - Copy-paste example code
   - Save and test

3. **Or use the deployer tool**:

   ```bash
   cargo run --bin deployer \
     --uri "https://example.com/blog" \
     --file "scripts/example_scripts/blog.js"
   ```

## Available Examples

### Blog Script (`blog.js`)

**Endpoint**: `/blog`  
**Method**: GET  
**Description**: A sample blog post showcasing aiwebengine capabilities with modern HTML/CSS styling.

**Features**:

- HTML templating with embedded CSS
- Feature showcase with code examples
- Responsive design
- Clean, modern UI

**Try it**: Visit `http://localhost:3000/blog` after uploading.

**Code Highlights**:

```javascript
function blogHandler(req) {
  const html = `
    <!DOCTYPE html>
    <html>
    <head>
        <title>aiwebengine Blog</title>
        <style>
        body { font-family: Arial, sans-serif; max-width: 800px; margin: 0 auto; padding: 20px; }
        .header { background: linear-gradient(135deg, #667eea 0%, #764ba2 100%); color: white; padding: 40px; border-radius: 10px; margin-bottom: 30px; }
        .feature { background: #f8f9fa; padding: 20px; margin: 20px 0; border-left: 4px solid #667eea; }
        </style>
    </head>
    <body>
        <div class="header">
            <h1>🚀 aiwebengine</h1>
            <p>Lightweight web application engine</p>
        </div>
        <!-- More content -->
    </body>
    </html>`;

  return {
    status: 200,
    body: html,
    contentType: "text/html",
  };
}

register("/blog", "blogHandler", "GET");
```

### Feedback Form (`feedback.js`)

**Endpoints**: `/feedback` (GET), `/feedback` (POST)  
**Description**: A complete feedback form with rating system and submission handling.

**Features**:

- GET handler for displaying the form
- POST handler for processing submissions
- Form validation and data processing
- Success confirmation page
- Logging of submitted data

**Try it**:

- Visit `http://localhost:3000/feedback` to see the form
- Submit feedback to test POST handling

**Code Highlights**:

```javascript
function feedbackFormHandler(req) {
  const form = `
    <form method="POST" action="/feedback">
        <label>Name: <input type="text" name="name" required></label><br>
        <label>Email: <input type="email" name="email" required></label><br>
        <label>Rating: 
            <select name="rating">
                <option value="5">⭐⭐⭐⭐⭐ Excellent</option>
                <option value="4">⭐⭐⭐⭐ Very Good</option>
                <option value="3">⭐⭐⭐ Average</option>
                <option value="2">⭐ Poor</option>
                <option value="1">⭐ Very Poor</option>
            </select>
        </label><br>
        <label>Comments: <textarea name="comments"></textarea></label><br>
        <button type="submit">Submit Feedback</button>
    </form>`;

  return {
    status: 200,
    body: form,
    contentType: "text/html",
  };
}

function feedbackSubmitHandler(req) {
  // Process form data
  const name = req.form.name;
  const email = req.form.email;
  const rating = req.form.rating;
  const comments = req.form.comments;

  writeLog(`Feedback received from ${name} (${email}): ${rating} stars`);

  const response = `
    <h1>Thank you for your feedback!</h1>
    <p>Name: ${name}</p>
    <p>Email: ${email}</p>
    <p>Rating: ${rating} stars</p>
    <p>Comments: ${comments}</p>
    <a href="/feedback">Submit another response</a>`;

  return {
    status: 200,
    body: response,
    contentType: "text/html",
  };
}

register("/feedback", "feedbackFormHandler", "GET");
register("/feedback", "feedbackSubmitHandler", "POST");
```

## Script Development Patterns

### Basic API Endpoint

```javascript
function apiHandler(req) {
  const data = {
    message: "Hello from aiwebengine!",
    timestamp: new Date().toISOString(),
    path: req.path,
    query: req.query,
  };

  return {
    status: 200,
    body: JSON.stringify(data),
    contentType: "application/json",
  };
}

register("/api/hello", "apiHandler", "GET");
```

### Dynamic Content with Query Parameters

```javascript
function greetHandler(req) {
  const name = req.query.name || "World";
  const greeting = `Hello, ${name}!`;

  return {
    status: 200,
    body: greeting,
    contentType: "text/plain",
  };
}

register("/greet", "greetHandler", "GET");
```

### Form Processing

```javascript
function contactHandler(req) {
  if (req.method === "GET") {
    return {
      status: 200,
      body: '<form method="POST"><input name="message"><button>Send</button></form>',
      contentType: "text/html",
    };
  } else {
    const message = req.form.message;
    writeLog(`Message received: ${message}`);

    return {
      status: 200,
      body: `Message "${message}" received!`,
      contentType: "text/plain",
    };
  }
}

register("/contact", "contactHandler", "GET");
register("/contact", "contactHandler", "POST");
```

## Streaming Examples

### Real-Time Notifications

**Description**: Demonstrates server-sent events for push notifications.

```javascript
// Register a notifications stream
registerWebStream("/notifications");

// Handler to display the notification page
function notificationPage(req) {
  const html = `
    <!DOCTYPE html>
    <html>
    <head>
        <title>Real-Time Notifications</title>
        <style>
            body { font-family: Arial, sans-serif; margin: 40px; }
            .notification { 
                background: #e3f2fd; 
                border-left: 4px solid #2196f3; 
                padding: 15px; 
                margin: 10px 0; 
                border-radius: 4px; 
            }
            .controls { margin: 20px 0; }
            button { 
                background: #2196f3; 
                color: white; 
                border: none; 
                padding: 10px 20px; 
                border-radius: 4px; 
                cursor: pointer; 
            }
            button:hover { background: #1976d2; }
        </style>
    </head>
    <body>
        <h1>Real-Time Notifications</h1>
        <div class="controls">
            <button onclick="sendTestNotification()">Send Test Notification</button>
        </div>
        <div id="notifications"></div>
        
        <script>
            const eventSource = new EventSource('/notifications');
            const container = document.getElementById('notifications');
            
            eventSource.onmessage = function(event) {
                const data = JSON.parse(event.data);
                const div = document.createElement('div');
                div.className = 'notification';
                div.innerHTML = \`
                    <strong>\${data.type.toUpperCase()}</strong><br>
                    \${data.message}<br>
                    <small>\${new Date(data.timestamp).toLocaleString()}</small>
                \`;
                container.insertBefore(div, container.firstChild);
            };
            
            function sendTestNotification() {
                fetch('/send-notification', { method: 'POST' });
            }
        </script>
    </body>
    </html>`;

  return { status: 200, body: html, contentType: "text/html" };
}

// Handler to send notifications
function sendNotification(req) {
  sendStreamMessage({
    type: "info",
    message: "This is a test notification from the server!",
    timestamp: new Date().toISOString(),
  });

  return { status: 200, body: "Notification sent" };
}

register("/notifications-demo", "notificationPage", "GET");
register("/send-notification", "sendNotification", "POST");
```

### Live Chat System

**Description**: A complete chat system with real-time messaging.

```javascript
// Register a chat stream
registerWebStream("/chat");

// Chat page handler
function chatPage(req) {
  const html = `
    <!DOCTYPE html>
    <html>
    <head>
        <title>Live Chat</title>
        <style>
            body { font-family: Arial, sans-serif; margin: 0; padding: 20px; }
            #chat-container { 
                max-width: 600px; 
                margin: 0 auto; 
                border: 1px solid #ddd; 
                border-radius: 8px; 
                overflow: hidden; 
            }
            #messages { 
                height: 400px; 
                overflow-y: scroll; 
                padding: 20px; 
                background: #f9f9f9; 
            }
            .message { 
                margin: 10px 0; 
                padding: 8px 12px; 
                background: white; 
                border-radius: 6px; 
                box-shadow: 0 1px 3px rgba(0,0,0,0.1); 
            }
            .message .user { 
                font-weight: bold; 
                color: #2196f3; 
            }
            .message .time { 
                font-size: 0.8em; 
                color: #666; 
                float: right; 
            }
            #chat-form { 
                padding: 20px; 
                background: white; 
                display: flex; 
                gap: 10px; 
            }
            #user-input, #message-input { 
                padding: 10px; 
                border: 1px solid #ddd; 
                border-radius: 4px; 
            }
            #user-input { width: 150px; }
            #message-input { flex: 1; }
            button { 
                background: #4caf50; 
                color: white; 
                border: none; 
                padding: 10px 20px; 
                border-radius: 4px; 
                cursor: pointer; 
            }
        </style>
    </head>
    <body>
        <div id="chat-container">
            <div id="messages"></div>
            <form id="chat-form">
                <input type="text" id="user-input" placeholder="Your name" required>
                <input type="text" id="message-input" placeholder="Type a message..." required>
                <button type="submit">Send</button>
            </form>
        </div>
        
        <script>
            const eventSource = new EventSource('/chat');
            const messages = document.getElementById('messages');
            const form = document.getElementById('chat-form');
            const userInput = document.getElementById('user-input');
            const messageInput = document.getElementById('message-input');
            
            eventSource.onmessage = function(event) {
                const data = JSON.parse(event.data);
                if (data.type === 'chat_message') {
                    addMessage(data.user, data.message, data.timestamp);
                }
            };
            
            function addMessage(user, message, timestamp) {
                const div = document.createElement('div');
                div.className = 'message';
                div.innerHTML = \`
                    <span class="time">\${new Date(timestamp).toLocaleTimeString()}</span>
                    <div class="user">\${user}:</div>
                    <div>\${message}</div>
                \`;
                messages.appendChild(div);
                messages.scrollTop = messages.scrollHeight;
            }
            
            form.addEventListener('submit', function(e) {
                e.preventDefault();
                const formData = new FormData();
                formData.append('user', userInput.value);
                formData.append('message', messageInput.value);
                
                fetch('/chat/send', {
                    method: 'POST',
                    body: formData
                });
                
                messageInput.value = '';
            });
        </script>
    </body>
    </html>`;

  return { status: 200, body: html, contentType: "text/html" };
}

// Send message handler
function sendChatMessage(req) {
  const { user, message } = req.form;

  if (!user || !message) {
    return { status: 400, body: "Missing user or message" };
  }

  sendStreamMessage({
    type: "chat_message",
    user: user,
    message: message,
    timestamp: new Date().toISOString(),
  });

  return { status: 200, body: "Message sent" };
}

register("/chat-demo", "chatPage", "GET");
register("/chat/send", "sendChatMessage", "POST");
```

### System Status Dashboard

**Description**: Real-time system status updates using streams.

```javascript
// Register a status stream
registerWebStream("/status");

// Status dashboard page
function statusDashboard(req) {
  const html = `
    <!DOCTYPE html>
    <html>
    <head>
        <title>System Status Dashboard</title>
        <style>
            body { 
                font-family: 'Segoe UI', sans-serif; 
                margin: 0; 
                padding: 20px; 
                background: #f5f5f5; 
            }
            .dashboard { 
                max-width: 800px; 
                margin: 0 auto; 
            }
            .status-card { 
                background: white; 
                border-radius: 8px; 
                padding: 20px; 
                margin: 15px 0; 
                box-shadow: 0 2px 10px rgba(0,0,0,0.1); 
                display: flex; 
                align-items: center; 
                justify-content: space-between; 
            }
            .status-indicator { 
                width: 12px; 
                height: 12px; 
                border-radius: 50%; 
                margin-right: 10px; 
            }
            .status-online { background: #4caf50; }
            .status-warning { background: #ff9800; }
            .status-offline { background: #f44336; }
            .last-update { 
                font-size: 0.9em; 
                color: #666; 
            }
            h1 { color: #333; text-align: center; }
            button { 
                background: #2196f3; 
                color: white; 
                border: none; 
                padding: 8px 16px; 
                border-radius: 4px; 
                cursor: pointer; 
            }
        </style>
    </head>
    <body>
        <div class="dashboard">
            <h1>System Status Dashboard</h1>
            <button onclick="triggerUpdate()">Trigger Status Update</button>
            <div id="status-container"></div>
        </div>
        
        <script>
            const eventSource = new EventSource('/status');
            const container = document.getElementById('status-container');
            const services = {};
            
            eventSource.onmessage = function(event) {
                const data = JSON.parse(event.data);
                if (data.type === 'status_update') {
                    updateServiceStatus(data.service, data.status, data.timestamp);
                }
            };
            
            function updateServiceStatus(service, status, timestamp) {
                const statusClass = \`status-\${status}\`;
                const html = \`
                    <div class="status-card">
                        <div style="display: flex; align-items: center;">
                            <div class="status-indicator \${statusClass}"></div>
                            <strong>\${service}</strong>
                        </div>
                        <div>
                            <div>\${status.toUpperCase()}</div>
                            <div class="last-update">Updated: \${new Date(timestamp).toLocaleString()}</div>
                        </div>
                    </div>
                \`;
                
                if (services[service]) {
                    services[service].innerHTML = html;
                } else {
                    const div = document.createElement('div');
                    div.innerHTML = html;
                    services[service] = div;
                    container.appendChild(div);
                }
            }
            
            function triggerUpdate() {
                fetch('/trigger-status', { method: 'POST' });
            }
        </script>
    </body>
    </html>`;

  return { status: 200, body: html, contentType: "text/html" };
}

// Trigger status updates
function triggerStatusUpdate(req) {
  const services = ["Database", "API Server", "Cache", "Message Queue"];
  const statuses = ["online", "warning", "offline"];

  services.forEach((service) => {
    const status = statuses[Math.floor(Math.random() * statuses.length)];
    sendStreamMessage({
      type: "status_update",
      service: service,
      status: status,
      timestamp: new Date().toISOString(),
    });
  });

  return { status: 200, body: "Status updates sent" };
}

register("/status-dashboard", "statusDashboard", "GET");
register("/trigger-status", "triggerStatusUpdate", "POST");
```

## Best Practices from Examples

1. **Separate concerns**: Use different handlers for different HTTP methods
2. **Validate input**: Check required parameters before processing
3. **Provide feedback**: Use logging to track script execution
4. **Handle errors gracefully**: Return appropriate status codes
5. **Use semantic HTML**: Structure your HTML properly for accessibility
6. **Style consistently**: Include CSS for better user experience

## Creating Your Own Examples

1. **Start simple**: Begin with basic text responses
2. **Add interactivity**: Include forms and dynamic content
3. **Test thoroughly**: Use the editor to test different scenarios
4. **Add logging**: Include `writeLog()` calls for debugging
5. **Document your scripts**: Add comments explaining complex logic

## Next Steps

- Learn the [JavaScript APIs](../javascript-apis.md) for available functions
- Set up [local development](../local-development.md) with the deployer
- Try the [remote editor](../remote-development.md) for quick prototyping
