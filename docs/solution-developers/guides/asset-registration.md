# Asset Registration System

## Overview

As of November 2025, aiwebengine has been refactored to use a more flexible asset registration system. Assets are now:

1. **Stored by name** in the repository (not by HTTP path)
2. **Registered to HTTP paths at runtime** using `registerPublicAsset()`
3. **Managed through JavaScript** in init() functions, similar to route registration

## Key Changes

### Before (Old System)

- Assets stored with `public_path` (e.g., `/logo.svg`)
- HTTP path was fixed in the database
- No flexibility to change paths without database updates

### After (New System)

- Assets stored with `asset_name` (e.g., `logo.svg`)
- HTTP paths registered dynamically using `registerPublicAsset(path, asset_name)`
- Same asset can be served at multiple HTTP paths
- Paths can be changed without touching the database

## Asset Functions

### registerPublicAsset(path, asset_name)

Registers an HTTP path to serve a specific asset.

**Parameters:**

- `path` (string): The HTTP path (must start with `/`, max 500 characters)
- `asset_name` (string): The name of the asset in the repository (1-255 characters, no path separators)

**Example:**

```javascript
function init(context) {
  // Register built-in assets
  registerPublicAsset("/logo.svg", "logo.svg");
  registerPublicAsset("/favicon.ico", "favicon.ico");

  // Register custom assets
  registerPublicAsset("/css/main.css", "main.css");
  registerPublicAsset("/css/theme.css", "theme.css");
  registerPublicAsset("/js/app.js", "app.js");

  // Same asset at multiple paths
  registerPublicAsset("/img/logo.svg", "logo.svg"); // Same logo at different path

  return { success: true };
}
```

### upsertAsset(asset_name, content_base64, mimetype)

Creates or updates an asset in the repository.

**Parameters:**

- `asset_name` (string): Name of the asset (e.g., `"logo.svg"`, `"app.css"`)
- `content_base64` (string): Base64-encoded content
- `mimetype` (string): MIME type (e.g., `"image/svg+xml"`, `"text/css"`)

**Example:**

```javascript
function uploadAsset(req) {
  const name = req.form.name; // "my-image.png"
  const content = req.form.content; // Base64 string
  const mimetype = req.form.mimetype; // "image/png"

  upsertAsset(name, content, mimetype);

  return {
    status: 201,
    body: JSON.stringify({ message: "Asset uploaded", name: name }),
    contentType: "application/json",
  };
}
```

### fetchAsset(asset_name)

Retrieves an asset by name from the repository.

**Parameters:**

- `asset_name` (string): Name of the asset

**Returns:**

- Base64-encoded content, or error message if not found

**Example:**

```javascript
function getAssetInfo(req) {
  const name = req.query.name; // "logo.svg"
  const content = fetchAsset(name);

  if (content && !content.startsWith("Asset")) {
    return {
      status: 200,
      body: content, // Base64
      contentType: "application/json",
    };
  } else {
    return {
      status: 404,
      body: "Asset not found",
      contentType: "text/plain",
    };
  }
}
```

### deleteAsset(asset_name)

Deletes an asset from the repository.

**Parameters:**

- `asset_name` (string): Name of the asset

**Returns:**

- `true` if deleted, `false` if not found

**Example:**

```javascript
function removeAsset(req) {
  const name = req.query.name;
  const deleted = deleteAsset(name);

  return {
    status: deleted ? 200 : 404,
    body: deleted ? "Deleted" : "Not found",
    contentType: "text/plain",
  };
}
```

### listAssets()

Lists all asset names in the repository.

**Returns:**

- Array of asset names (strings)

**Example:**

```javascript
function listAllAssets(req) {
  const assetNames = listAssets();

  return {
    status: 200,
    body: JSON.stringify({ assets: assetNames }),
    contentType: "application/json",
  };
}
```

## Complete Example

Here's a complete script that demonstrates the new asset system:

```javascript
// Asset demo script

function homePage(req) {
  const html = `
    <!DOCTYPE html>
    <html>
    <head>
      <title>Asset Demo</title>
      <link rel="stylesheet" href="/styles/main.css">
      <link rel="icon" href="/favicon.ico">
    </head>
    <body>
      <img src="/images/logo.svg" alt="Logo">
      <h1>Welcome</h1>
      <script src="/scripts/app.js"></script>
    </body>
    </html>
  `;

  return {
    status: 200,
    body: html,
    contentType: "text/html; charset=UTF-8",
  };
}

function init(context) {
  console.log("Initializing asset demo");

  // Register HTTP routes
  register("/", "homePage", "GET");

  // Register asset paths
  // Built-in assets
  registerPublicAsset("/favicon.ico", "favicon.ico");
  registerPublicAsset("/images/logo.svg", "logo.svg");

  // Custom assets (must be uploaded first via upsertAsset)
  registerPublicAsset("/styles/main.css", "main.css");
  registerPublicAsset("/scripts/app.js", "app.js");

  console.log("Asset paths registered");
  return { success: true };
}
```

## Migration Guide

If you have existing scripts that reference assets:

### Old Code

```javascript
// Assets were referenced by their full path
const assetPath = "/css/main.css";
// No registration needed - path was in database
```

### New Code

```javascript
function init(context) {
  // Register the asset path in init()
  registerPublicAsset("/css/main.css", "main.css");

  return { success: true };
}
```

## Built-in Assets

The following assets are provided by the system and should be registered in your init() function:

- `logo.svg` - aiwebengine logo
- `favicon.ico` - Favicon
- `editor.css` - Editor styles
- `editor.js` - Editor JavaScript
- `engine.css` - Engine styles

**Recommended registration (in core.js or your main script):**

```javascript
function init(context) {
  registerPublicAsset("/logo.svg", "logo.svg");
  registerPublicAsset("/favicon.ico", "favicon.ico");
  registerPublicAsset("/editor.css", "editor.css");
  registerPublicAsset("/editor.js", "editor.js");
  registerPublicAsset("/engine.css", "engine.css");

  // ... rest of your init code
}
```

## Best Practices

1. **Register in init()**: Always call `registerPublicAsset()` in your script's `init()` function
2. **Use descriptive names**: Asset names should be descriptive (e.g., `logo.svg`, `main.css`)
3. **Organize paths**: Use logical HTTP paths (e.g., `/css/`, `/js/`, `/images/`)
4. **One asset, multiple paths**: You can serve the same asset at multiple HTTP paths
5. **Asset names vs paths**: Keep asset names simple (no slashes), use paths for organization

## Error Handling

The `registerPublicAsset()` function will return an error message if:

- Path doesn't start with `/`
- Path is too long (>500 characters)
- Asset name is empty or too long (>255 characters)
- Asset name contains path separators (`/`, `\`, or `..`)
- User lacks WriteAssets capability

**Example with error checking:**

```javascript
function init(context) {
  const result = registerPublicAsset("/logo.svg", "logo.svg");

  // Result is a string, check for success
  if (result.includes("registered")) {
    console.log("Asset registered successfully");
  } else {
    console.error("Failed to register asset: " + result);
  }

  return { success: true };
}
```

## Database Schema

The assets table structure:

```sql
CREATE TABLE assets (
    asset_name TEXT PRIMARY KEY,        -- Asset identifier
    mimetype TEXT NOT NULL,              -- MIME type
    content BYTEA NOT NULL,              -- Binary content
    created_at TIMESTAMPTZ NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL
);
```

HTTP path mappings are maintained in-memory and registered via JavaScript.
