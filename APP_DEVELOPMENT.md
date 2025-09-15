# App Development Guide for aiwebengine

## Overview

aiwebengine is a lightweight web application engine that allows you to build web applications using JavaScript. The engine runs on a Rust-based server using the QuickJS JavaScript runtime, providing a simple yet powerful platform for developing web services.

Key features:

- JavaScript-based scripting for handling HTTP requests
- Built-in logging system
- Support for query parameters and form data
- Dynamic script loading and management
- RESTful API support
- Static asset serving and management

## Getting Started

### Project Structure

The engine loads JavaScript scripts from the `scripts/` directory. Core scripts are embedded at compile time, while additional scripts can be added dynamically at runtime.

### Basic Script Structure

Every script consists of:

1. Handler functions that process HTTP requests
2. Route registrations that map URLs to handlers
3. Optional logging and utility code

## Writing JavaScript Scripts

### Handler Functions

Handler functions receive a single `req` parameter containing request information and must return a response object.

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

### Request Object Structure

The `req` object contains:

- `method`: HTTP method (GET, POST, PUT, DELETE, etc.)
- `path`: Request path
- `query`: Object containing query parameters
- `form`: Object containing form data (for POST requests)

Example request object:

```javascript
{
    method: "GET",
    path: "/api/users",
    query: { id: "123", format: "json" },
    form: {} // Empty for GET requests
}
```

### Response Object Structure

Handlers must return an object with:

- `status`: HTTP status code (number)
- `body`: Response content (string)
- `contentType`: Optional MIME type (string)

```javascript
return {
    status: 200,
    body: JSON.stringify({ message: "Success" }),
    contentType: "application/json"
};
```

## Registering Routes

Use the `register()` function to map URLs to handler functions:

```javascript
register(path, handlerName, method);
```

Parameters:

- `path`: URL path (string)
- `handlerName`: Name of your handler function (string)
- `method`: HTTP method (string, defaults to "GET")

Examples:

```javascript
// Register a GET handler for /api/users
register('/api/users', 'getUsers', 'GET');

// Register a POST handler for /api/users
register('/api/users', 'createUser', 'POST');

// Register for multiple methods
register('/api/data', 'handleData', 'GET');
register('/api/data', 'handleData', 'POST');
```

## Handling Query Parameters

Query parameters are automatically parsed and available in `req.query`:

```javascript
function searchHandler(req) {
    const query = req.query.q || '';
    const limit = parseInt(req.query.limit) || 10;
    
    return {
        status: 200,
        body: `Searching for: ${query}, limit: ${limit}`,
        contentType: "text/plain"
    };
}

register('/api/search', 'searchHandler', 'GET');
```

## Handling Form Data

Form data from POST requests is automatically parsed and available in `req.form`:

```javascript
function createUserHandler(req) {
    const name = req.form.name;
    const email = req.form.email;
    
    if (!name || !email) {
        return {
            status: 400,
            body: "Name and email are required",
            contentType: "text/plain"
        };
    }
    
    return {
        status: 201,
        body: `User created: ${name} (${email})`,
        contentType: "text/plain"
    };
}

register('/api/users', 'createUserHandler', 'POST');
```

## Logging

Use the `writeLog()` function to write messages to the server's log:

```javascript
function myHandler(req) {
    writeLog(`Request received: ${req.method} ${req.path}`);
    
    // Your handler logic here
    
    writeLog('Request processed successfully');
    
    return {
        status: 200,
        body: "OK"
    };
}
```

### Retrieving Logs

Use `listLogs()` to retrieve recent log messages:

```javascript
function logsHandler(req) {
    const logs = listLogs();
    
    return {
        status: 200,
        body: JSON.stringify(logs),
        contentType: "application/json"
    };
}

register('/api/logs', 'logsHandler', 'GET');
```

## Script Management

### Listing Available Scripts

Use `listScripts()` to get a list of all loaded scripts:

```javascript
function scriptsHandler(req) {
    const scripts = listScripts();
    
    return {
        status: 200,
        body: JSON.stringify(scripts),
        contentType: "application/json"
    };
}

register('/api/scripts', 'scriptsHandler', 'GET');
```

## Asset Management

aiwebengine provides built-in support for serving and managing static assets (images, CSS, JavaScript files, etc.). Assets can be served directly by the server or managed programmatically through JavaScript scripts.

### Static Asset Serving

The server automatically serves static assets for GET requests. Place your asset files in the `assets/` directory, and they will be available at their corresponding URLs:

- `assets/logo.svg` → `GET /logo.svg`
- `assets/style.css` → `GET /style.css`
- `assets/app.js` → `GET /app.js`

The server automatically sets the correct MIME type based on the file extension.

### Programmatic Asset Management

Use the asset management functions to create, read, update, and delete assets programmatically:

#### Listing Assets

```javascript
function listAssetsHandler(req) {
    const assetPaths = listAssets();
    
    return {
        status: 200,
        body: JSON.stringify({ assets: assetPaths }),
        contentType: "application/json"
    };
}

register('/api/assets', 'listAssetsHandler', 'GET');
```

#### Fetching Assets

```javascript
function getAssetHandler(req) {
    // Extract asset path from URL (e.g., /api/assets/logo.svg -> /logo.svg)
    const assetPath = req.path.replace('/api/assets', '');
    
    try {
        const assetJson = fetchAsset(assetPath);
        
        if (assetJson === 'null') {
            return {
                status: 404,
                body: JSON.stringify({ error: 'Asset not found' }),
                contentType: "application/json"
            };
        }
        
        return {
            status: 200,
            body: assetJson,
            contentType: "application/json"
        };
    } catch (error) {
        return {
            status: 500,
            body: JSON.stringify({ error: error.message }),
            contentType: "application/json"
        };
    }
}

register('/api/assets/*', 'getAssetHandler', 'GET');
```

#### Creating/Updating Assets

```javascript
function createAssetHandler(req) {
    try {
        const publicPath = req.form.publicPath; // e.g., "/my-image.png"
        const mimetype = req.form.mimetype;     // e.g., "image/png"
        const contentB64 = req.form.content;    // base64-encoded content
        
        if (!publicPath || !mimetype || !contentB64) {
            return {
                status: 400,
                body: JSON.stringify({ 
                    error: 'Missing required fields: publicPath, mimetype, content' 
                }),
                contentType: "application/json"
            };
        }
        
        upsertAsset(publicPath, mimetype, contentB64);
        
        return {
            status: 201,
            body: JSON.stringify({ message: 'Asset created/updated successfully' }),
            contentType: "application/json"
        };
    } catch (error) {
        return {
            status: 500,
            body: JSON.stringify({ error: error.message }),
            contentType: "application/json"
        };
    }
}

register('/api/assets', 'createAssetHandler', 'POST');
```

#### Deleting Assets

```javascript
function deleteAssetHandler(req) {
    // Extract asset path from URL
    const assetPath = req.path.replace('/api/assets', '');
    
    try {
        const deleted = deleteAsset(assetPath);
        
        if (deleted) {
            return {
                status: 200,
                body: JSON.stringify({ message: 'Asset deleted successfully' }),
                contentType: "application/json"
            };
        } else {
            return {
                status: 404,
                body: JSON.stringify({ error: 'Asset not found' }),
                contentType: "application/json"
            };
        }
    } catch (error) {
        return {
            status: 500,
            body: JSON.stringify({ error: error.message }),
            contentType: "application/json"
        };
    }
}

register('/api/assets/*', 'deleteAssetHandler', 'DELETE');
```

### Asset API Examples

#### Upload an Image via HTML Form

```html
<!DOCTYPE html>
<html>
<body>
    <form action="/api/assets" method="POST" enctype="multipart/form-data">
        <input type="text" name="publicPath" placeholder="/my-image.jpg" required>
        <input type="text" name="mimetype" placeholder="image/jpeg" required>
        <input type="file" name="content" accept="image/*" required>
        <button type="submit">Upload Asset</button>
    </form>
</body>
</html>
```

#### Create Asset via JavaScript

```javascript
function uploadAsset(file, publicPath) {
    return new Promise((resolve, reject) => {
        const reader = new FileReader();
        reader.onload = function(e) {
            const contentB64 = e.target.result.split(',')[1]; // Remove data URL prefix
            
            // Determine MIME type
            const mimetype = file.type || 'application/octet-stream';
            
            // This would typically be sent via fetch to your asset creation endpoint
            const assetData = {
                publicPath: publicPath,
                mimetype: mimetype,
                content: contentB64
            };
            
            console.log('Asset data prepared:', assetData);
            resolve(assetData);
        };
        reader.onerror = reject;
        reader.readAsDataURL(file);
    });
}

// Usage
const fileInput = document.getElementById('fileInput');
fileInput.addEventListener('change', async (e) => {
    const file = e.target.files[0];
    if (file) {
        try {
            const assetData = await uploadAsset(file, `/uploads/${file.name}`);
            // Send assetData to your server endpoint
        } catch (error) {
            console.error('Upload failed:', error);
        }
    }
});
```

## Complete Examples

### Simple API Endpoint

```javascript
function helloHandler(req) {
    const name = req.query.name || 'World';
    
    return {
        status: 200,
        body: `Hello, ${name}!`,
        contentType: "text/plain"
    };
}

register('/api/hello', 'helloHandler', 'GET');
```

### User Management API

```javascript
// In-memory user store (resets on server restart)
let users = [];
let nextId = 1;

function getUsersHandler(req) {
    writeLog('Fetching all users');
    
    return {
        status: 200,
        body: JSON.stringify(users),
        contentType: "application/json"
    };
}

function createUserHandler(req) {
    const name = req.form.name;
    const email = req.form.email;
    
    if (!name || !email) {
        return {
            status: 400,
            body: JSON.stringify({ error: "Name and email are required" }),
            contentType: "application/json"
        };
    }
    
    const user = { id: nextId++, name, email };
    users.push(user);
    
    writeLog(`Created user: ${name} (${email})`);
    
    return {
        status: 201,
        body: JSON.stringify(user),
        contentType: "application/json"
    };
}

function getUserHandler(req) {
    const id = parseInt(req.query.id);
    
    if (!id) {
        return {
            status: 400,
            body: JSON.stringify({ error: "User ID is required" }),
            contentType: "application/json"
        };
    }
    
    const user = users.find(u => u.id === id);
    
    if (!user) {
        return {
            status: 404,
            body: JSON.stringify({ error: "User not found" }),
            contentType: "application/json"
        };
    }
    
    return {
        status: 200,
        body: JSON.stringify(user),
        contentType: "application/json"
    };
}

// Register routes
register('/api/users', 'getUsersHandler', 'GET');
register('/api/users', 'createUserHandler', 'POST');
register('/api/users/single', 'getUserHandler', 'GET');
```

## Error Handling

Always handle errors gracefully in your handlers:

```javascript
function riskyHandler(req) {
    try {
        // Some operation that might fail
        const result = someRiskyOperation();
        
        return {
            status: 200,
            body: JSON.stringify({ result }),
            contentType: "application/json"
        };
    } catch (error) {
        writeLog(`Error in riskyHandler: ${error.message}`);
        
        return {
            status: 500,
            body: JSON.stringify({ error: "Internal server error" }),
            contentType: "application/json"
        };
    }
}
```

## Best Practices

1. **Validate Input**: Always validate query parameters and form data
2. **Use Appropriate Status Codes**: Return correct HTTP status codes
3. **Log Important Events**: Use `writeLog()` for debugging and monitoring
4. **Handle Errors**: Implement proper error handling in all handlers
5. **Set Content Types**: Specify appropriate MIME types for responses
6. **Keep Handlers Simple**: Break complex logic into smaller, focused functions
7. **Use Meaningful Names**: Choose descriptive names for handlers and variables

## Deployment

### Adding Scripts to the Engine

1. Place your JavaScript files in the `scripts/` directory
2. Update the `repository.rs` file to include your new scripts (for compile-time embedding)
3. Or use the dynamic script management API to load scripts at runtime

### Building and Running

```bash
# Build the project
cargo build

# Run the server
cargo run
```

The server will automatically load all scripts and register their routes on startup.

## API Reference

### Host Functions

- `register(path, handler, method)`: Register a route
- `writeLog(message)`: Write a message to the log
- `listLogs()`: Get all log messages
- `listScripts()`: Get list of loaded scripts
- `listAssets()`: Get list of available assets
- `fetchAsset(publicPath)`: Get asset data as JSON string
- `upsertAsset(publicPath, mimetype, contentB64)`: Create or update an asset
- `deleteAsset(publicPath)`: Delete an asset

### Request Object

- `req.method`: HTTP method
- `req.path`: Request path
- `req.query`: Query parameters object
- `req.form`: Form data object

### Response Object

- `status`: HTTP status code
- `body`: Response content
- `contentType`: MIME type (optional)

This guide covers the basics of developing applications on aiwebengine. Explore the example scripts in the `scripts/` directory for more advanced patterns and use cases.
