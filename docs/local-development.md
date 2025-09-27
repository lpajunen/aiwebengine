# Local Development

This guide covers setting up your local development environment for aiwebengine, including writing JavaScript scripts and using the deployer tool for efficient development workflows.

## Prerequisites

- Rust (latest stable version recommended)
- Basic understanding of JavaScript
- Text editor or IDE

## Project Setup

1. **Clone the repository**:

   ```bash
   git clone https://github.com/lpajunen/aiwebengine.git
   cd aiwebengine
   ```

2. **Build the project**:

   ```bash
   cargo build --release
   ```

3. **Start the server**:

   ```bash
   cargo run
   ```

The server will start on `http://localhost:3000` by default.

## Writing JavaScript Scripts

### Script Location

Place your scripts in the `scripts/` directory. The engine automatically loads scripts from this directory.

### Basic Script Structure

Every script consists of:

1. **Handler functions** that process HTTP requests
2. **Route registrations** that map URLs to handlers
3. Optional utility functions and logging

### Handler Functions

Handler functions receive a `req` object and return a response object:

```javascript
function myHandler(req) {
    // Process the request
    return {
        status: 200,
        body: "Hello World",
        contentType: "text/plain"
    };
}
```

### Request Object

The `req` object contains:

- `method`: HTTP method (GET, POST, etc.)
- `path`: Request path
- `query`: Object with query parameters
- `form`: Object with form data (for POST requests)
- `headers`: Request headers

### Response Object

Handlers must return an object with:

- `status`: HTTP status code (required)
- `body`: Response content (required)
- `contentType`: MIME type (optional, defaults to "text/plain")

### Registering Routes

Use the `register()` function to map URLs to handlers:

```javascript
register(path, handlerName, method);
```

Examples:

```javascript
// GET /api/users
register('/api/users', 'getUsersHandler', 'GET');

// POST /api/users
register('/api/users', 'createUserHandler', 'POST');
```

## Using the Deployer Tool

The deployer tool streamlines script development by automatically uploading scripts to a running server and watching for changes.

### Building the Deployer

```bash
cargo build --release --bin deployer
```

### Basic Usage

```bash
# Deploy a script once
cargo run --bin deployer --uri "https://example.com/my-script" --file "my-script.js"

# Deploy and watch for changes
cargo run --bin deployer --uri "https://example.com/my-script" --file "my-script.js" --watch
```

### Command Line Options

- `-u, --uri <URI>`: Script URI (required)
- `-f, --file <FILE>`: Path to JavaScript file (required)
- `-s, --server <SERVER>`: Server URL (default: `http://localhost:3000`)
- `-w, --watch`: Watch for file changes (default: true)

### Development Workflow

1. **Start the server** in one terminal:

   ```bash
   cargo run --bin server
   ```

2. **Start the deployer** in another terminal:

   ```bash
   cargo run --bin deployer \
     --uri "https://example.com/my-feature" \
     --file "src/my-feature.js" \
     --watch
   ```

3. **Edit your script** - changes are automatically deployed on save

4. **Test your endpoint** at `http://localhost:3000/my-feature`

### Custom Server Configuration

For servers on different ports or hosts:

```bash
cargo run --bin deployer \
  --server "http://localhost:8080" \
  --uri "https://example.com/test" \
  --file "test.js"
```

## Built-in Functions

Your scripts have access to several built-in functions:

- `register(path, handlerFunction, method)`: Register a route
- `writeLog(message)`: Write to the server log
- `registerWebStream(path)`: Register a Server-Sent Events stream endpoint
- `sendStreamMessage(data)`: Send real-time messages to connected stream clients
- `JSON.stringify(obj)`: Convert objects to JSON

## Debugging

- Use `writeLog()` to output debug information
- Check server logs for errors
- The deployer provides feedback on deployment success/failure

## Testing Streaming Endpoints

aiwebengine supports real-time streaming using Server-Sent Events (SSE). Here's how to test streaming functionality during development.

### Creating a Streaming Script

Create a script with streaming capabilities:

```javascript
// Register a stream endpoint
registerWebStream('/test-stream');

// Page to test the stream
function streamTestPage(req) {
    return {
        status: 200,
        body: `
        <!DOCTYPE html>
        <html>
        <head><title>Stream Test</title></head>
        <body>
            <h1>Stream Test</h1>
            <button onclick="sendTest()">Send Test Message</button>
            <div id="messages"></div>
            <script>
                const eventSource = new EventSource('/test-stream');
                eventSource.onmessage = function(event) {
                    const data = JSON.parse(event.data);
                    document.getElementById('messages').innerHTML += 
                        '<p>' + data.message + ' at ' + data.timestamp + '</p>';
                };
                function sendTest() {
                    fetch('/send-test', { method: 'POST' });
                }
            </script>
        </body>
        </html>`,
        contentType: "text/html"
    };
}

// Handler to send test messages
function sendTestMessage(req) {
    sendStreamMessage({
        type: 'test',
        message: 'Hello from stream!',
        timestamp: new Date().toISOString()
    });
    return { status: 200, body: 'Message sent' };
}

register('/stream-test', 'streamTestPage', 'GET');
register('/send-test', 'sendTestMessage', 'POST');
```

### Testing with Browser

1. **Deploy the streaming script**:

   ```bash
   cargo run --bin deployer --uri "test-stream" --file "stream-test.js" --watch
   ```

2. **Open the test page** in your browser:

   ```text
   http://localhost:3000/stream-test
   ```

3. **Click "Send Test Message"** to see real-time updates

### Testing with curl

You can also test streams from the command line:

```bash
# Connect to the stream (will keep connection open)
curl -N -H "Accept: text/event-stream" http://localhost:3000/test-stream

# In another terminal, trigger a message
curl -X POST http://localhost:3000/send-test
```

### Debugging Streaming Issues

Common issues and solutions:

1. **Stream not receiving messages**:
   - Check that `registerWebStream()` is called in your script
   - Verify the stream path matches your EventSource URL
   - Look for JavaScript errors in browser console

2. **Connection issues**:
   - Ensure proper `text/event-stream` content type
   - Check browser Network tab for connection status
   - Verify server is running and accessible

3. **Message format problems**:
   - Use `writeLog()` to debug message content
   - Ensure `sendStreamMessage()` receives valid objects
   - Check JSON parsing on client side

### Stream Development Tips

- **Use unique stream paths** for different features
- **Test reconnection** by stopping/starting the server
- **Monitor browser DevTools** Network tab for SSE connections
- **Use meaningful message types** for easier debugging
- **Add error handling** on both server and client sides

## Next Steps

- Check out the [examples](../examples.md) for common patterns
- Learn about [JavaScript APIs](../javascript-apis.md) for advanced features
- Try the [remote editor](../remote-development.md) for quick prototyping
