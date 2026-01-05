# aiwebengine User Documentation

## Overview

**aiwebengine** is a lightweight web application engine built in Rust that enables developers to create dynamic web content using JavaScript scripts. It provides a simple yet powerful platform for building web applications by writing JavaScript handlers that process HTTP requests and generate responses.

The core concept is that you create JavaScript scripts that define how your web application behaves. These scripts contain handler functions that receive HTTP requests and return responses, allowing you to build APIs, serve HTML pages, handle forms, and more—all using familiar JavaScript syntax.

## Key Concepts

- **Scripts**: JavaScript files that contain your application logic
- **Handlers**: Functions that process HTTP requests and return responses
- **Routes**: URL paths mapped to specific handlers
- **Assets**: Static files (images, CSS, etc.) served by the engine

## Quick Start

1. **Install and run the server**:

   ```bash
   git clone https://github.com/lpajunen/aiwebengine.git
   cd aiwebengine
   cargo build --release
   cargo run
   ```

2. **Create a simple script** (e.g., `scripts/hello.js`):

   ```javascript
   function helloHandler(req) {
     return {
       status: 200,
       body: `Hello, ${req.query.name || "World"}!`,
       contentType: "text/plain",
     };
   }

   register("/hello", "helloHandler", "GET");
   ```

3. **Access your endpoint**: Visit `http://localhost:3000/hello?name=User`

## Quick Start: Real-Time Streaming

Want to add real-time features? Here's how to create a live notification system:

1. **Create a streaming script** (e.g., `scripts/notifications.js`):

   ```javascript
   // Register a stream endpoint for real-time notifications
   registerWebStream("/notifications");

   // Page that displays notifications in real-time
   function notificationPage(req) {
     return {
       status: 200,
       body: `
           <!DOCTYPE html>
           <html>
           <head><title>Live Notifications</title></head>
           <body>
               <h1>Live Notifications</h1>
               <button onclick="sendTest()">Send Test Notification</button>
               <div id="messages"></div>
               <script>
                   const eventSource = new EventSource('/notifications');
                   eventSource.onmessage = function(event) {
                       const data = JSON.parse(event.data);
                       document.getElementById('messages').innerHTML += 
                           '<p><strong>' + data.type + ':</strong> ' + data.message + '</p>';
                   };
                   function sendTest() {
                       fetch('/send-notification', { method: 'POST' });
                   }
               </script>
           </body>
           </html>`,
       contentType: "text/html",
     };
   }

   // Handler to send notifications to all connected clients
   function sendNotification(req) {
     sendStreamMessage({
       type: "info",
       message: "Hello from the server! " + new Date().toLocaleTimeString(),
     });
     return { status: 200, body: "Notification sent!" };
   }

   register("/live-notifications", "notificationPage", "GET");
   register("/send-notification", "sendNotification", "POST");
   ```

2. **Access your streaming app**: Visit `http://localhost:3000/live-notifications`
3. **Click "Send Test Notification"** to see real-time updates in action!

## Documentation Sections

- **[Local Development](engine-administrators/local-development.md)**: Setting up your development environment
- **[Remote Development](engine-administrators/remote-development.md)**: Using the built-in web editor for script management
- **[JavaScript APIs](solution-developers/javascript-apis.md)**: Complete reference for available JavaScript functions and objects
- **[Real-Time Streaming](solution-developers/streaming.md)**: Guide to building live, interactive applications with Server-Sent Events
- **[Examples](solution-developers/examples.md)**: Sample scripts demonstrating common patterns and use cases

## Getting Help

- Check the [examples](solution-developers/examples.md) for common patterns
- Review the [JavaScript APIs](solution-developers/javascript-apis.md) for available functions
- Learn about [real-time streaming](solution-developers/streaming.md) for interactive features
- See [local development](engine-administrators/local-development.md) for development workflows
- Use the [remote editor](engine-administrators/remote-development.md) for quick prototyping

## Project Status

⚠️ **Work in Progress**: aiwebengine is actively developed. Core features are stable, but some advanced features are still evolving.
