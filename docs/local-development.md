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
- `JSON.stringify(obj)`: Convert objects to JSON

## Debugging

- Use `writeLog()` to output debug information
- Check server logs for errors
- The deployer provides feedback on deployment success/failure

## Next Steps

- Check out the [examples](../examples.md) for common patterns
- Learn about [JavaScript APIs](../javascript-apis.md) for advanced features
- Try the [remote editor](../remote-development.md) for quick prototyping
