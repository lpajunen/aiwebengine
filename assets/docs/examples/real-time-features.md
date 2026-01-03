# Real-Time Features Examples

Learn how to build real-time applications with aiwebengine using WebSocket streams, Server-Sent Events (SSE), and live data updates.

## Server-Sent Events (SSE) Basics

SSE allows servers to push updates to clients over HTTP. Perfect for one-way real-time updates.

### Simple Notification System

```javascript
function init() {
  // Register a WebSocket stream
  routeRegistry.registerStreamRoute("/notifications");

  // Page to display notifications
  routeRegistry.registerRoute(
    "GET",
    "/notifications-demo",
    showNotificationsPage,
  );

  // Endpoint to send a notification
  routeRegistry.registerRoute("POST", "/send-notification", sendNotification);
}

function showNotificationsPage(request) {
  const html = `
    <!DOCTYPE html>
    <html>
    <head>
      <title>Real-Time Notifications</title>
      <style>
        body {
          font-family: Arial, sans-serif;
          max-width: 800px;
          margin: 50px auto;
          padding: 20px;
          background: #f5f5f5;
        }
        h1 {
          color: #333;
        }
        .controls {
          background: white;
          padding: 20px;
          border-radius: 8px;
          margin-bottom: 20px;
          box-shadow: 0 2px 4px rgba(0,0,0,0.1);
        }
        button {
          background: #007bff;
          color: white;
          border: none;
          padding: 10px 20px;
          border-radius: 4px;
          cursor: pointer;
          margin-right: 10px;
        }
        button:hover {
          background: #0056b3;
        }
        #notifications {
          min-height: 200px;
        }
        .notification {
          background: white;
          padding: 15px;
          margin: 10px 0;
          border-radius: 4px;
          border-left: 4px solid #007bff;
          box-shadow: 0 2px 4px rgba(0,0,0,0.1);
          animation: slideIn 0.3s ease-out;
        }
        .notification.info {
          border-left-color: #17a2b8;
        }
        .notification.success {
          border-left-color: #28a745;
        }
        .notification.warning {
          border-left-color: #ffc107;
        }
        .notification.error {
          border-left-color: #dc3545;
        }
        .notification-type {
          font-weight: bold;
          text-transform: uppercase;
          font-size: 0.85em;
          margin-bottom: 5px;
        }
        .notification-time {
          font-size: 0.85em;
          color: #666;
          margin-top: 5px;
        }
        .status {
          padding: 10px;
          background: #d4edda;
          border: 1px solid #c3e6cb;
          border-radius: 4px;
          margin-bottom: 20px;
        }
        @keyframes slideIn {
          from {
            transform: translateX(-100%);
            opacity: 0;
          }
          to {
            transform: translateX(0);
            opacity: 1;
          }
        }
      </style>
    </head>
    <body>
      <h1>📬 Real-Time Notifications</h1>
      
      <div id="status" class="status">
        <strong>Status:</strong> <span id="connectionStatus">Connecting...</span>
      </div>
      
      <div class="controls">
        <button onclick="sendTestNotification('info')">Send Info</button>
        <button onclick="sendTestNotification('success')">Send Success</button>
        <button onclick="sendTestNotification('warning')">Send Warning</button>
        <button onclick="sendTestNotification('error')">Send Error</button>
        <button onclick="clearNotifications()">Clear All</button>
      </div>
      
      <div id="notifications"></div>
      
      <script>
        const notificationsContainer = document.getElementById('notifications');
        const statusElement = document.getElementById('connectionStatus');
        
        // Connect to SSE stream
        const eventSource = new EventSource('/notifications');
        
        eventSource.onopen = function() {
          statusElement.textContent = 'Connected ✓';
          statusElement.style.color = '#28a745';
        };
        
        eventSource.onerror = function() {
          statusElement.textContent = 'Disconnected ✗';
          statusElement.style.color = '#dc3545';
        };
        
        eventSource.onmessage = function(event) {
          const data = JSON.parse(event.data);
          addNotification(data);
        };
        
        function addNotification(data) {
          const div = document.createElement('div');
          div.className = \`notification \${data.type}\`;
          div.innerHTML = \`
            <div class="notification-type">\${data.type}</div>
            <div>\${data.message}</div>
            <div class="notification-time">\${new Date(data.timestamp).toLocaleString()}</div>
          \`;
          
          notificationsContainer.insertBefore(div, notificationsContainer.firstChild);
          
          // Keep only last 10 notifications
          while (notificationsContainer.children.length > 10) {
            notificationsContainer.removeChild(notificationsContainer.lastChild);
          }
        }
        
        function sendTestNotification(type) {
          fetch('/send-notification', {
            method: 'POST',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify({ type: type })
          });
        }
        
        function clearNotifications() {
          notificationsContainer.innerHTML = '';
        }
      </script>
    </body>
    </html>
  `;

  return {
    status: 200,
    headers: { "Content-Type": "text/html" },
    body: html,
  };
}

function sendNotification(request) {
  const data = JSON.parse(request.body || "{}");
  const type = data.type || "info";

  const messages = {
    info: "This is an informational message",
    success: "Operation completed successfully!",
    warning: "Please review this warning",
    error: "An error occurred that needs attention",
  };

  const notification = {
    type: type,
    message: messages[type] || "Test notification",
    timestamp: new Date().toISOString(),
  };

  sendStreamMessage(notification);

  console.log("Notification sent", notification);

  return {
    status: 200,
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ success: true }),
  };
}

init();
```

## Live Chat Application

A complete real-time chat system:

```javascript
function init() {
  routeRegistry.registerStreamRoute("/chat");
  routeRegistry.registerRoute("GET", "/chat", showChatPage);
  routeRegistry.registerRoute("POST", "/chat/send", sendMessage);
  routeRegistry.registerRoute("POST", "/chat/typing", sendTypingIndicator);
}

function showChatPage(request) {
  const html = `
    <!DOCTYPE html>
    <html>
    <head>
      <title>Live Chat</title>
      <style>
        * {
          margin: 0;
          padding: 0;
          box-sizing: border-box;
        }
        body {
          font-family: 'Segoe UI', Tahoma, Geneva, Verdana, sans-serif;
          height: 100vh;
          display: flex;
          flex-direction: column;
          background: #f0f2f5;
        }
        .header {
          background: #075e54;
          color: white;
          padding: 15px 20px;
          box-shadow: 0 2px 4px rgba(0,0,0,0.1);
        }
        .header h1 {
          font-size: 1.2em;
        }
        .chat-container {
          flex: 1;
          display: flex;
          flex-direction: column;
          max-width: 1200px;
          width: 100%;
          margin: 0 auto;
          background: white;
        }
        .messages {
          flex: 1;
          overflow-y: auto;
          padding: 20px;
          background: #e5ddd5;
        }
        .message {
          margin: 10px 0;
          display: flex;
          animation: fadeIn 0.3s;
        }
        .message-content {
          max-width: 60%;
          padding: 8px 12px;
          border-radius: 8px;
          word-wrap: break-word;
        }
        .message.own {
          justify-content: flex-end;
        }
        .message.own .message-content {
          background: #dcf8c6;
        }
        .message.other .message-content {
          background: white;
        }
        .message-user {
          font-weight: bold;
          color: #075e54;
          font-size: 0.85em;
          margin-bottom: 3px;
        }
        .message-time {
          font-size: 0.75em;
          color: #667;
          margin-top: 3px;
          text-align: right;
        }
        .typing-indicator {
          padding: 10px 20px;
          font-style: italic;
          color: #666;
          min-height: 30px;
          background: #f0f2f5;
        }
        .input-area {
          display: flex;
          padding: 15px;
          background: #f0f2f5;
          border-top: 1px solid #ddd;
        }
        #username {
          width: 150px;
          padding: 10px;
          border: 1px solid #ddd;
          border-radius: 20px;
          margin-right: 10px;
        }
        #messageInput {
          flex: 1;
          padding: 10px 15px;
          border: 1px solid #ddd;
          border-radius: 20px;
          margin-right: 10px;
        }
        button {
          background: #075e54;
          color: white;
          border: none;
          padding: 10px 25px;
          border-radius: 20px;
          cursor: pointer;
          font-weight: bold;
        }
        button:hover {
          background: #064e47;
        }
        button:disabled {
          background: #ccc;
          cursor: not-allowed;
        }
        @keyframes fadeIn {
          from { opacity: 0; transform: translateY(10px); }
          to { opacity: 1; transform: translateY(0); }
        }
        .system-message {
          text-align: center;
          color: #666;
          font-size: 0.85em;
          margin: 15px 0;
          font-style: italic;
        }
      </style>
    </head>
    <body>
      <div class="header">
        <h1>💬 Live Chat Room</h1>
      </div>
      
      <div class="chat-container">
        <div class="messages" id="messages"></div>
        
        <div class="typing-indicator" id="typingIndicator"></div>
        
        <div class="input-area">
          <input type="text" id="username" placeholder="Your name" value="User">
          <input type="text" id="messageInput" placeholder="Type a message..." autocomplete="off">
          <button id="sendBtn">Send</button>
        </div>
      </div>
      
      <script>
        const messagesContainer = document.getElementById('messages');
        const messageInput = document.getElementById('messageInput');
        const usernameInput = document.getElementById('username');
        const sendBtn = document.getElementById('sendBtn');
        const typingIndicator = document.getElementById('typingIndicator');
        
        let typingTimeout;
        const typingUsers = new Set();
        
        // Connect to chat stream
        const eventSource = new EventSource('/chat');
        
        eventSource.onmessage = function(event) {
          const data = JSON.parse(event.data);
          
          if (data.type === 'message') {
            addMessage(data.user, data.message, data.timestamp, false);
          } else if (data.type === 'typing') {
            handleTyping(data.user, data.isTyping);
          } else if (data.type === 'system') {
            addSystemMessage(data.message);
          }
        };
        
        eventSource.onerror = function() {
          addSystemMessage('Connection lost. Reconnecting...');
        };
        
        // Send message
        function sendMessage() {
          const message = messageInput.value.trim();
          const username = usernameInput.value.trim() || 'Anonymous';
          
          if (!message) return;
          
          fetch('/chat/send', {
            method: 'POST',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify({ user: username, message: message })
          });
          
          // Add own message immediately
          addMessage(username, message, new Date().toISOString(), true);
          
          messageInput.value = '';
          sendTypingStatus(false);
        }
        
        sendBtn.addEventListener('click', sendMessage);
        
        messageInput.addEventListener('keypress', function(e) {
          if (e.key === 'Enter') {
            sendMessage();
          }
        });
        
        // Typing indicator
        messageInput.addEventListener('input', function() {
          sendTypingStatus(true);
          
          clearTimeout(typingTimeout);
          typingTimeout = setTimeout(() => {
            sendTypingStatus(false);
          }, 1000);
        });
        
        function sendTypingStatus(isTyping) {
          const username = usernameInput.value.trim() || 'Anonymous';
          
          fetch('/chat/typing', {
            method: 'POST',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify({ user: username, isTyping: isTyping })
          });
        }
        
        function addMessage(user, message, timestamp, isOwn) {
          const div = document.createElement('div');
          div.className = \`message \${isOwn ? 'own' : 'other'}\`;
          
          const content = document.createElement('div');
          content.className = 'message-content';
          
          if (!isOwn) {
            const userDiv = document.createElement('div');
            userDiv.className = 'message-user';
            userDiv.textContent = user;
            content.appendChild(userDiv);
          }
          
          const messageDiv = document.createElement('div');
          messageDiv.textContent = message;
          content.appendChild(messageDiv);
          
          const timeDiv = document.createElement('div');
          timeDiv.className = 'message-time';
          timeDiv.textContent = new Date(timestamp).toLocaleTimeString();
          content.appendChild(timeDiv);
          
          div.appendChild(content);
          messagesContainer.appendChild(div);
          messagesContainer.scrollTop = messagesContainer.scrollHeight;
        }
        
        function addSystemMessage(message) {
          const div = document.createElement('div');
          div.className = 'system-message';
          div.textContent = message;
          messagesContainer.appendChild(div);
          messagesContainer.scrollTop = messagesContainer.scrollHeight;
        }
        
        function handleTyping(user, isTyping) {
          if (isTyping) {
            typingUsers.add(user);
          } else {
            typingUsers.delete(user);
          }
          
          updateTypingIndicator();
        }
        
        function updateTypingIndicator() {
          if (typingUsers.size === 0) {
            typingIndicator.textContent = '';
          } else if (typingUsers.size === 1) {
            typingIndicator.textContent = \`\${Array.from(typingUsers)[0]} is typing...\`;
          } else {
            typingIndicator.textContent = \`\${typingUsers.size} people are typing...\`;
          }
        }
        
        // Welcome message
        addSystemMessage('Welcome to the chat room!');
      </script>
    </body>
    </html>
  `;

  return {
    status: 200,
    headers: { "Content-Type": "text/html" },
    body: html,
  };
}

function sendMessage(request) {
  const data = JSON.parse(request.body || "{}");

  const message = {
    type: "message",
    user: data.user || "Anonymous",
    message: data.message,
    timestamp: new Date().toISOString(),
  };

  sendStreamMessage(message);

  console.log("Chat message sent", {
    user: message.user,
    messageLength: message.message.length,
  });

  return {
    status: 200,
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ success: true }),
  };
}

function sendTypingIndicator(request) {
  const data = JSON.parse(request.body || "{}");

  const typing = {
    type: "typing",
    user: data.user,
    isTyping: data.isTyping,
  };

  sendStreamMessage(typing);

  return {
    status: 200,
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ success: true }),
  };
}

init();
```

## Live Dashboard

Real-time system monitoring dashboard:

```javascript
function init() {
  routeRegistry.registerStreamRoute("/system-stats");
  routeRegistry.registerRoute("GET", "/dashboard", showDashboard);
  routeRegistry.registerRoute("POST", "/update-stats", updateSystemStats);
}

function showDashboard(request) {
  const html = `
    <!DOCTYPE html>
    <html>
    <head>
      <title>Live Dashboard</title>
      <style>
        body {
          font-family: Arial, sans-serif;
          margin: 0;
          padding: 20px;
          background: #1a1a2e;
          color: white;
        }
        h1 {
          text-align: center;
          margin-bottom: 30px;
        }
        .dashboard {
          display: grid;
          grid-template-columns: repeat(auto-fit, minmax(300px, 1fr));
          gap: 20px;
          max-width: 1400px;
          margin: 0 auto;
        }
        .card {
          background: #16213e;
          border-radius: 12px;
          padding: 25px;
          box-shadow: 0 4px 6px rgba(0,0,0,0.3);
        }
        .card-title {
          font-size: 1.1em;
          margin-bottom: 15px;
          color: #0f9;
          display: flex;
          align-items: center;
          gap: 10px;
        }
        .metric {
          font-size: 3em;
          font-weight: bold;
          margin: 20px 0;
          text-align: center;
        }
        .metric-label {
          text-align: center;
          color: #999;
          font-size: 0.9em;
        }
        .status-indicator {
          width: 12px;
          height: 12px;
          border-radius: 50%;
          display: inline-block;
          animation: pulse 2s infinite;
        }
        .status-online {
          background: #0f9;
        }
        .status-warning {
          background: #fa0;
        }
        .status-offline {
          background: #f44;
        }
        @keyframes pulse {
          0%, 100% { opacity: 1; }
          50% { opacity: 0.5; }
        }
        .progress-bar {
          width: 100%;
          height: 30px;
          background: #0e1621;
          border-radius: 15px;
          overflow: hidden;
          margin: 15px 0;
        }
        .progress-fill {
          height: 100%;
          background: linear-gradient(90deg, #0f9 0%, #0cf 100%);
          transition: width 0.5s ease;
          display: flex;
          align-items: center;
          justify-content: center;
          font-weight: bold;
        }
        .chart {
          height: 150px;
          background: #0e1621;
          border-radius: 8px;
          margin-top: 15px;
          position: relative;
          overflow: hidden;
        }
        .chart-bar {
          position: absolute;
          bottom: 0;
          width: 8%;
          background: linear-gradient(180deg, #0cf 0%, #0f9 100%);
          transition: height 0.3s ease;
        }
        .stats-list {
          list-style: none;
          padding: 0;
        }
        .stats-list li {
          padding: 10px 0;
          border-bottom: 1px solid #0e1621;
          display: flex;
          justify-content: space-between;
        }
        .stats-list li:last-child {
          border-bottom: none;
        }
        button {
          background: #0f9;
          color: #1a1a2e;
          border: none;
          padding: 10px 20px;
          border-radius: 6px;
          cursor: pointer;
          font-weight: bold;
          margin-top: 15px;
        }
        button:hover {
          background: #0cf;
        }
      </style>
    </head>
    <body>
      <h1>📊 Live System Dashboard</h1>
      
      <div class="dashboard">
        <div class="card">
          <div class="card-title">
            <span class="status-indicator status-online"></span>
            Server Status
          </div>
          <div class="metric" id="uptime">0</div>
          <div class="metric-label">Uptime (hours)</div>
        </div>
        
        <div class="card">
          <div class="card-title">CPU Usage</div>
          <div class="metric" id="cpu">0%</div>
          <div class="progress-bar">
            <div class="progress-fill" id="cpuBar" style="width: 0%"></div>
          </div>
        </div>
        
        <div class="card">
          <div class="card-title">Memory Usage</div>
          <div class="metric" id="memory">0%</div>
          <div class="progress-bar">
            <div class="progress-fill" id="memoryBar" style="width: 0%"></div>
          </div>
        </div>
        
        <div class="card">
          <div class="card-title">Active Users</div>
          <div class="metric" id="users">0</div>
          <div class="metric-label">Connected</div>
        </div>
        
        <div class="card">
          <div class="card-title">Request Rate</div>
          <div class="metric" id="requests">0</div>
          <div class="metric-label">Requests/sec</div>
          <div class="chart" id="requestChart"></div>
        </div>
        
        <div class="card">
          <div class="card-title">System Info</div>
          <ul class="stats-list">
            <li><span>Total Requests</span><span id="totalRequests">0</span></li>
            <li><span>Error Rate</span><span id="errorRate">0%</span></li>
            <li><span>Avg Response</span><span id="avgResponse">0ms</span></li>
            <li><span>Database</span><span id="dbStatus">●</span></li>
          </ul>
          <button onclick="triggerUpdate()">Refresh Data</button>
        </div>
      </div>
      
      <script>
        const eventSource = new EventSource('/system-stats');
        const requestHistory = [];
        const maxHistory = 10;
        
        eventSource.onmessage = function(event) {
          const data = JSON.parse(event.data);
          updateDashboard(data);
        };
        
        function updateDashboard(data) {
          // Update metrics
          document.getElementById('uptime').textContent = data.uptime || 0;
          document.getElementById('cpu').textContent = data.cpu + '%';
          document.getElementById('memory').textContent = data.memory + '%';
          document.getElementById('users').textContent = data.activeUsers || 0;
          document.getElementById('requests').textContent = data.requestRate || 0;
          
          // Update progress bars
          document.getElementById('cpuBar').style.width = data.cpu + '%';
          document.getElementById('cpuBar').textContent = data.cpu + '%';
          document.getElementById('memoryBar').style.width = data.memory + '%';
          document.getElementById('memoryBar').textContent = data.memory + '%';
          
          // Update stats
          document.getElementById('totalRequests').textContent = 
            (data.totalRequests || 0).toLocaleString();
          document.getElementById('errorRate').textContent = (data.errorRate || 0) + '%';
          document.getElementById('avgResponse').textContent = (data.avgResponse || 0) + 'ms';
          
          const dbStatus = document.getElementById('dbStatus');
          dbStatus.textContent = '● ' + (data.dbConnected ? 'Connected' : 'Offline');
          dbStatus.style.color = data.dbConnected ? '#0f9' : '#f44';
          
          // Update request chart
          requestHistory.push(data.requestRate || 0);
          if (requestHistory.length > maxHistory) {
            requestHistory.shift();
          }
          updateChart();
        }
        
        function updateChart() {
          const chart = document.getElementById('requestChart');
          chart.innerHTML = '';
          
          const max = Math.max(...requestHistory, 1);
          
          requestHistory.forEach((value, index) => {
            const bar = document.createElement('div');
            bar.className = 'chart-bar';
            bar.style.left = (index * 10) + '%';
            bar.style.height = ((value / max) * 100) + '%';
            chart.appendChild(bar);
          });
        }
        
        function triggerUpdate() {
          fetch('/update-stats', { method: 'POST' });
        }
        
        // Auto-refresh every 5 seconds
        setInterval(triggerUpdate, 5000);
        
        // Initial update
        triggerUpdate();
      </script>
    </body>
    </html>
  `;

  return {
    status: 200,
    headers: { "Content-Type": "text/html" },
    body: html,
  };
}

function updateSystemStats(request) {
  // Generate mock system statistics
  const stats = {
    uptime: Math.floor(Math.random() * 100),
    cpu: Math.floor(Math.random() * 100),
    memory: Math.floor(Math.random() * 80) + 20,
    activeUsers: Math.floor(Math.random() * 50),
    requestRate: Math.floor(Math.random() * 100),
    totalRequests: Math.floor(Math.random() * 10000),
    errorRate: Math.floor(Math.random() * 5),
    avgResponse: Math.floor(Math.random() * 200) + 50,
    dbConnected: Math.random() > 0.1,
  };

  sendStreamMessage(stats);

  return {
    status: 200,
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ success: true }),
  };
}

init();
```

## Live Stock Ticker

Real-time stock price updates:

```javascript
function init() {
  routeRegistry.registerStreamRoute("/stock-prices");
  routeRegistry.registerRoute("GET", "/stocks", showStockTicker);
  routeRegistry.registerRoute("POST", "/update-prices", updatePrices);
}

function showStockTicker(request) {
  const html = `
    <!DOCTYPE html>
    <html>
    <head>
      <title>Live Stock Ticker</title>
      <style>
        body {
          font-family: 'Courier New', monospace;
          margin: 0;
          padding: 20px;
          background: #000;
          color: #0f0;
        }
        h1 {
          text-align: center;
          color: #0f0;
          text-shadow: 0 0 10px #0f0;
        }
        .ticker-container {
          max-width: 1200px;
          margin: 0 auto;
        }
        .stock {
          background: #001a00;
          border: 1px solid #0f0;
          padding: 20px;
          margin: 15px 0;
          border-radius: 4px;
          display: flex;
          justify-content: space-between;
          align-items: center;
          transition: all 0.3s;
        }
        .stock.up {
          background: #001a00;
          animation: flashGreen 0.5s;
        }
        .stock.down {
          background: #1a0000;
          animation: flashRed 0.5s;
          color: #f00;
          border-color: #f00;
        }
        @keyframes flashGreen {
          0%, 100% { background: #001a00; }
          50% { background: #003300; }
        }
        @keyframes flashRed {
          0%, 100% { background: #1a0000; }
          50% { background: #330000; }
        }
        .stock-symbol {
          font-size: 1.5em;
          font-weight: bold;
        }
        .stock-name {
          font-size: 0.9em;
          opacity: 0.7;
        }
        .stock-price {
          font-size: 2em;
          font-weight: bold;
          text-align: right;
        }
        .stock-change {
          font-size: 1.2em;
          text-align: right;
        }
        .stock-change.positive {
          color: #0f0;
        }
        .stock-change.negative {
          color: #f00;
        }
        .controls {
          text-align: center;
          margin: 30px 0;
        }
        button {
          background: #0f0;
          color: #000;
          border: none;
          padding: 10px 20px;
          font-family: 'Courier New', monospace;
          font-weight: bold;
          cursor: pointer;
          margin: 0 5px;
        }
        button:hover {
          background: #0c0;
        }
        .last-update {
          text-align: center;
          opacity: 0.5;
          margin-top: 20px;
        }
      </style>
    </head>
    <body>
      <h1>📈 LIVE STOCK TICKER 📉</h1>
      
      <div class="controls">
        <button onclick="updatePrices()">Update Prices</button>
        <button onclick="toggleAutoUpdate()">
          <span id="autoUpdateText">Start Auto-Update</span>
        </button>
      </div>
      
      <div class="ticker-container" id="stocks"></div>
      
      <div class="last-update">
        Last Update: <span id="lastUpdate">Never</span>
      </div>
      
      <script>
        const stocksContainer = document.getElementById('stocks');
        const stocks = {};
        let autoUpdateInterval = null;
        
        const eventSource = new EventSource('/stock-prices');
        
        eventSource.onmessage = function(event) {
          const data = JSON.parse(event.data);
          updateStock(data);
          document.getElementById('lastUpdate').textContent = 
            new Date().toLocaleTimeString();
        };
        
        function updateStock(data) {
          const oldPrice = stocks[data.symbol] ? stocks[data.symbol].price : data.price;
          const direction = data.price > oldPrice ? 'up' : 
                           data.price < oldPrice ? 'down' : 'same';
          
          stocks[data.symbol] = data;
          
          let stockElement = document.getElementById('stock-' + data.symbol);
          
          if (!stockElement) {
            stockElement = document.createElement('div');
            stockElement.id = 'stock-' + data.symbol;
            stockElement.className = 'stock';
            stocksContainer.appendChild(stockElement);
          }
          
          const change = data.price - oldPrice;
          const changePercent = oldPrice > 0 ? ((change / oldPrice) * 100) : 0;
          const changeClass = change >= 0 ? 'positive' : 'negative';
          const changeSign = change >= 0 ? '+' : '';
          
          stockElement.className = \`stock \${direction}\`;
          stockElement.innerHTML = \`
            <div>
              <div class="stock-symbol">\${data.symbol}</div>
              <div class="stock-name">\${data.name}</div>
            </div>
            <div>
              <div class="stock-price">$\${data.price.toFixed(2)}</div>
              <div class="stock-change \${changeClass}">
                \${changeSign}\${change.toFixed(2)} (\${changeSign}\${changePercent.toFixed(2)}%)
              </div>
            </div>
          \`;
        }
        
        function updatePrices() {
          fetch('/update-prices', { method: 'POST' });
        }
        
        function toggleAutoUpdate() {
          const button = document.getElementById('autoUpdateText');
          
          if (autoUpdateInterval) {
            clearInterval(autoUpdateInterval);
            autoUpdateInterval = null;
            button.textContent = 'Start Auto-Update';
          } else {
            autoUpdateInterval = setInterval(updatePrices, 2000);
            button.textContent = 'Stop Auto-Update';
          }
        }
      </script>
    </body>
    </html>
  `;

  return {
    status: 200,
    headers: { "Content-Type": "text/html" },
    body: html,
  };
}

function updatePrices(request) {
  const stocks = [
    { symbol: "AAPL", name: "Apple Inc." },
    { symbol: "GOOGL", name: "Alphabet Inc." },
    { symbol: "MSFT", name: "Microsoft Corp." },
    { symbol: "AMZN", name: "Amazon.com Inc." },
    { symbol: "TSLA", name: "Tesla Inc." },
  ];

  stocks.forEach((stock) => {
    // Generate realistic price movements
    const basePrice = {
      AAPL: 150,
      GOOGL: 2800,
      MSFT: 300,
      AMZN: 3200,
      TSLA: 700,
    }[stock.symbol];

    const volatility = 0.02; // 2% volatility
    const change = (Math.random() - 0.5) * 2 * volatility * basePrice;
    const price = basePrice + change;

    sendStreamMessage({
      symbol: stock.symbol,
      name: stock.name,
      price: price,
      timestamp: new Date().toISOString(),
    });
  });

  return {
    status: 200,
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ success: true }),
  };
}

init();
```

## Live Activity Feed

Real-time user activity stream:

```javascript
function init() {
  routeRegistry.registerStreamRoute("/activity");
  routeRegistry.registerRoute("GET", "/activity-feed", showActivityFeed);
  routeRegistry.registerRoute("POST", "/activity/log", logActivity);
}

function showActivityFeed(request) {
  const html = `
    <!DOCTYPE html>
    <html>
    <head>
      <title>Activity Feed</title>
      <style>
        body {
          font-family: Arial, sans-serif;
          margin: 0;
          padding: 20px;
          background: #f5f7fa;
        }
        h1 {
          text-align: center;
          color: #333;
        }
        .feed-container {
          max-width: 800px;
          margin: 0 auto;
        }
        .activity {
          background: white;
          padding: 15px 20px;
          margin: 10px 0;
          border-radius: 8px;
          box-shadow: 0 2px 4px rgba(0,0,0,0.1);
          display: flex;
          align-items: start;
          gap: 15px;
          animation: slideIn 0.3s ease-out;
        }
        @keyframes slideIn {
          from {
            transform: translateY(-20px);
            opacity: 0;
          }
          to {
            transform: translateY(0);
            opacity: 1;
          }
        }
        .activity-icon {
          width: 40px;
          height: 40px;
          border-radius: 50%;
          display: flex;
          align-items: center;
          justify-content: center;
          font-size: 1.5em;
          flex-shrink: 0;
        }
        .icon-user { background: #e3f2fd; }
        .icon-post { background: #f3e5f5; }
        .icon-comment { background: #fff3e0; }
        .icon-like { background: #ffebee; }
        .icon-share { background: #e8f5e9; }
        .activity-content {
          flex: 1;
        }
        .activity-user {
          font-weight: bold;
          color: #1976d2;
        }
        .activity-action {
          color: #666;
        }
        .activity-time {
          font-size: 0.85em;
          color: #999;
          margin-top: 5px;
        }
        .controls {
          text-align: center;
          margin: 20px 0;
        }
        button {
          background: #1976d2;
          color: white;
          border: none;
          padding: 10px 20px;
          border-radius: 4px;
          cursor: pointer;
          margin: 0 5px;
        }
        button:hover {
          background: #1565c0;
        }
      </style>
    </head>
    <body>
      <h1>🔔 Live Activity Feed</h1>
      
      <div class="controls">
        <button onclick="simulateActivity('user')">Simulate User Join</button>
        <button onclick="simulateActivity('post')">Simulate Post</button>
        <button onclick="simulateActivity('comment')">Simulate Comment</button>
        <button onclick="simulateActivity('like')">Simulate Like</button>
      </div>
      
      <div class="feed-container" id="feed"></div>
      
      <script>
        const feed = document.getElementById('feed');
        const eventSource = new EventSource('/activity');
        
        const icons = {
          user: '👤',
          post: '📝',
          comment: '💬',
          like: '❤️',
          share: '🔄'
        };
        
        eventSource.onmessage = function(event) {
          const activity = JSON.parse(event.data);
          addActivity(activity);
        };
        
        function addActivity(activity) {
          const div = document.createElement('div');
          div.className = 'activity';
          
          const iconClass = 'icon-' + activity.type;
          const icon = icons[activity.type] || '•';
          
          div.innerHTML = \`
            <div class="activity-icon \${iconClass}">\${icon}</div>
            <div class="activity-content">
              <div>
                <span class="activity-user">\${activity.user}</span>
                <span class="activity-action">\${activity.action}</span>
              </div>
              <div class="activity-time">\${timeAgo(activity.timestamp)}</div>
            </div>
          \`;
          
          feed.insertBefore(div, feed.firstChild);
          
          // Keep only last 20 activities
          while (feed.children.length > 20) {
            feed.removeChild(feed.lastChild);
          }
        }
        
        function timeAgo(timestamp) {
          const seconds = Math.floor((new Date() - new Date(timestamp)) / 1000);
          
          if (seconds < 60) return 'Just now';
          if (seconds < 3600) return Math.floor(seconds / 60) + ' minutes ago';
          if (seconds < 86400) return Math.floor(seconds / 3600) + ' hours ago';
          return Math.floor(seconds / 86400) + ' days ago';
        }
        
        function simulateActivity(type) {
          const users = ['Alice', 'Bob', 'Charlie', 'Diana', 'Eve'];
          const actions = {
            user: 'joined the platform',
            post: 'created a new post',
            comment: 'commented on a post',
            like: 'liked a post',
            share: 'shared a post'
          };
          
          fetch('/activity/log', {
            method: 'POST',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify({
              type: type,
              user: users[Math.floor(Math.random() * users.length)],
              action: actions[type]
            })
          });
        }
        
        // Simulate some initial activity
        setTimeout(() => simulateActivity('user'), 500);
        setTimeout(() => simulateActivity('post'), 1500);
        setTimeout(() => simulateActivity('comment'), 2500);
      </script>
    </body>
    </html>
  `;

  return {
    status: 200,
    headers: { "Content-Type": "text/html" },
    body: html,
  };
}

function logActivity(request) {
  const data = JSON.parse(request.body || "{}");

  const activity = {
    type: data.type,
    user: data.user,
    action: data.action,
    timestamp: new Date().toISOString(),
  };

  sendStreamMessage(activity);

  console.log("Activity logged", activity);

  return {
    status: 200,
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ success: true }),
  };
}

init();
```

## Best Practices

### 1. Keep Connections Alive

```javascript
// Server sends periodic heartbeats
setInterval(() => {
  sendStreamMessage({ type: "heartbeat", timestamp: Date.now() });
}, 30000); // Every 30 seconds
```

### 2. Handle Reconnection

```javascript
eventSource.onerror = function () {
  console.log("Connection lost, will auto-reconnect");
  // EventSource automatically reconnects
};
```

### 3. Limit Data Size

```javascript
// Send only necessary data
sendStreamMessage({
  id: userId,
  update: "status",
  value: newStatus,
  // Don't send entire user object
});
```

### 4. Use Appropriate Stream Types

```javascript
// One-way updates: Use SSE (routeRegistry.registerStreamRoute)
routeRegistry.registerStreamRoute("/notifications");

// For bidirectional: Consider GraphQL subscriptions
// See graphql-subscriptions guide
```

### 5. Clean Up Resources

```javascript
// Client-side cleanup
window.addEventListener("beforeunload", () => {
  eventSource.close();
});
```

## Common Patterns

### Broadcasting to All Clients

```javascript
function broadcastToAll(message) {
  sendStreamMessage({
    type: "broadcast",
    message: message,
    timestamp: new Date().toISOString(),
  });
}
```

### Rate Limiting Updates

```javascript
let lastUpdate = 0;
const UPDATE_INTERVAL = 1000; // 1 second

function throttledUpdate(data) {
  const now = Date.now();
  if (now - lastUpdate >= UPDATE_INTERVAL) {
    sendStreamMessage(data);
    lastUpdate = now;
  }
}
```

### Batching Updates

```javascript
let updateQueue = [];

setInterval(() => {
  if (updateQueue.length > 0) {
    sendStreamMessage({
      type: "batch",
      updates: updateQueue,
    });
    updateQueue = [];
  }
}, 500); // Batch every 500ms
```

## Next Steps

- **[Basic API Examples](basic-api.md)** - REST API patterns
- **[Forms and Data](forms-and-data.md)** - Form handling
- **[AI Integration](ai-integration.md)** - Add AI features
- **[Streaming Guide](../guides/streaming.md)** - Deep dive into streams
- **[GraphQL Subscriptions](../guides/graphql-subscriptions.md)** - GraphQL real-time

## Quick Reference

```javascript
// Register stream
routeRegistry.registerStreamRoute("/my-stream");

// Send message to all connected clients
routeRegistry.sendStreamMessage("/my-stream", { type: "update", data: value });

// Client-side connection
const eventSource = new EventSource("/my-stream");

eventSource.onmessage = function (event) {
  const data = JSON.parse(event.data);
  // Handle update
};
```

Build powerful real-time applications with aiwebengine! ⚡
