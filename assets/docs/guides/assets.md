# Asset Management Guide

Learn how to work with static files like images, CSS, JavaScript, and other assets in aiwebengine.

## Overview

Assets are static files that your scripts can serve to clients. They can include:

- **Images** - PNG, JPEG, GIF, SVG
- **Stylesheets** - CSS files
- **Scripts** - Client-side JavaScript
- **Documents** - PDF, text files
- **Fonts** - WOFF, TTF files
- **Any other static content**

## How Assets Work

### Automatic Serving

aiwebengine automatically serves files from the `assets/` directory:

```text
assets/logo.png       → http://yourserver.com/logo.png
assets/style.css      → http://yourserver.com/style.css
assets/app.js         → http://yourserver.com/app.js
assets/docs/guide.pdf → http://yourserver.com/docs/guide.pdf
```

The server automatically:

- Sets correct MIME types based on file extensions
- Handles HTTP GET requests for assets
- Serves files efficiently

### Directory Structure

Organize assets in subdirectories:

```text
assets/
├── css/
│   ├── main.css
│   └── theme.css
├── js/
│   ├── app.js
│   └── utils.js
├── images/
│   ├── logo.png
│   └── banner.jpg
├── fonts/
│   └── custom.woff2
└── docs/
    └── manual.pdf
```

## Managing Assets

### Method 1: Web Editor (Easiest)

**Upload assets via the editor:**

1. Open `http://localhost:8080/editor`
2. Click "Assets" in sidebar
3. Click "Upload Assets"
4. Select files from your computer
5. Assets are immediately available

**View assets:**

- Browse in the Assets section
- Preview images directly
- Download or delete as needed

**Example: Upload a stylesheet**

1. Create `style.css` locally:

   ```css
   body {
     font-family: Arial, sans-serif;
     max-width: 800px;
     margin: 0 auto;
     padding: 20px;
   }
   ```

2. Upload via editor
3. Use in your scripts:
   ```javascript
   function pageHandler(req) {
     return {
       status: 200,
       body: `
         <!DOCTYPE html>
         <html>
         <head>
           <link rel="stylesheet" href="/css/style.css">
         </head>
         <body>
           <h1>Styled Page</h1>
         </body>
         </html>
       `,
       contentType: "text/html",
     };
   }
   ```

### Method 2: Direct File Placement

**Copy files to the assets directory:**

```bash
# Copy a single file
cp logo.png /path/to/aiwebengine/assets/

# Copy directory structure
cp -r public/* /path/to/aiwebengine/assets/

# Using rsync
rsync -av local-assets/ server:/path/to/aiwebengine/assets/
```

Files are immediately available at their URLs.

### Method 3: API-Based (Programmatic)

**Use the Asset Management API:**

```javascript
// List available assets
const assetsJson = assetStorage.listAssets();
const assets = JSON.parse(assetsJson);
console.log("Available assets:", assets);

// Access asset metadata
assets.forEach((asset) => {
  console.log(`${asset.name}: ${asset.size} bytes, ${asset.mimetype}`);
});

// Fetch asset data
const assetData = assetStorage.fetchAsset("/logo.png");
const asset = JSON.parse(assetData);
console.log(asset.mimetype); // "image/png"
console.log(asset.contentB64); // Base64 encoded content

// Create or update asset
assetStorage.upsertAsset(
  "/new-image.png", // Public path
  "image/png", // MIME type
  base64EncodedContent, // Base64 string
);

// Delete asset
const deleted = assetStorage.deleteAsset("/old-image.png");
console.log(deleted); // true if deleted
```

**Example: Upload from form**

```javascript
function uploadHandler(req) {
  const publicPath = req.form.path; // "/uploads/file.jpg"
  const mimetype = req.form.mimetype; // "image/jpeg"
  const contentB64 = req.form.content; // Base64 string

  try {
    assetStorage.upsertAsset(publicPath, mimetype, contentB64);
    console.log(`Asset uploaded: ${publicPath}`);

    return {
      status: 201,
      body: JSON.stringify({
        message: "Asset uploaded",
        url: publicPath,
      }),
      contentType: "application/json",
    };
  } catch (error) {
    console.error(`Upload failed: ${error.message}`);
    return {
      status: 500,
      body: JSON.stringify({ error: "Upload failed" }),
      contentType: "application/json",
    };
  }
}

routeRegistry.registerRoute("/upload-asset", "uploadHandler", "POST");
```

## Using Assets in Scripts

### Linking Stylesheets

```javascript
function styledPageHandler(req) {
  const html = `
    <!DOCTYPE html>
    <html>
    <head>
      <title>My Page</title>
      <!-- Link to CSS assets -->
      <link rel="stylesheet" href="/css/main.css">
      <link rel="stylesheet" href="/css/theme.css">
    </head>
    <body>
      <h1>Styled Content</h1>
    </body>
    </html>
  `;

  return {
    status: 200,
    body: html,
    contentType: "text/html",
  };
}
```

### Loading JavaScript

```javascript
function appPageHandler(req) {
  const html = `
    <!DOCTYPE html>
    <html>
    <head>
      <title>App</title>
    </head>
    <body>
      <div id="app"></div>
      
      <!-- Load JavaScript assets -->
      <script src="/js/utils.js"></script>
      <script src="/js/app.js"></script>
    </body>
    </html>
  `;

  return {
    status: 200,
    body: html,
    contentType: "text/html",
  };
}
```

### Embedding Images

```javascript
function galleryHandler(req) {
  const html = `
    <!DOCTYPE html>
    <html>
    <body>
      <h1>Gallery</h1>
      
      <!-- Reference image assets -->
      <img src="/images/logo.png" alt="Logo">
      <img src="/images/banner.jpg" alt="Banner">
      <img src="/images/icon.svg" alt="Icon">
      
      <!-- Background images via CSS -->
      <div style="
        background-image: url('/images/background.jpg');
        height: 300px;
      ">
        Content
      </div>
    </body>
    </html>
  `;

  return {
    status: 200,
    body: html,
    contentType: "text/html",
  };
}
```

### Referencing Documents

```javascript
function resourcesHandler(req) {
  const html = `
    <!DOCTYPE html>
    <html>
    <body>
      <h1>Resources</h1>
      
      <!-- Links to documents -->
      <ul>
        <li><a href="/docs/manual.pdf">User Manual (PDF)</a></li>
        <li><a href="/docs/guide.txt">Quick Guide (Text)</a></li>
        <li><a href="/downloads/template.zip">Template (ZIP)</a></li>
      </ul>
    </body>
    </html>
  `;

  return {
    status: 200,
    body: html,
    contentType: "text/html",
  };
}
```

## Asset Organization Patterns

### By Type

```text
assets/
├── css/
├── js/
├── images/
├── fonts/
└── docs/
```

**Pros:** Clear separation, easy to find
**Cons:** Mixed purposes in same category

### By Feature

```text
assets/
├── home/
│   ├── hero.jpg
│   ├── home.css
│   └── home.js
├── dashboard/
│   ├── dashboard.css
│   └── dashboard.js
└── shared/
    ├── common.css
    └── logo.png
```

**Pros:** Related assets together
**Cons:** Harder to find all CSS files

### Hybrid Approach

```text
assets/
├── global/
│   ├── css/
│   ├── js/
│   └── images/
├── features/
│   ├── blog/
│   ├── shop/
│   └── auth/
└── vendor/
    ├── bootstrap.css
    └── jquery.js
```

**Pros:** Best of both worlds
**Cons:** More complex structure

## Dynamic Asset Management

### Asset Gallery Script

```javascript
function assetGalleryHandler(req) {
  // Get all assets with metadata
  const assetsJson = assetStorage.listAssets();
  const assets = JSON.parse(assetsJson);

  // Filter for images
  const images = assets.filter((asset) => {
    return asset.mimetype.startsWith(\"image/\");
  });

  // Build HTML gallery
  const imageCards = images
    .map((asset) => {
      return `
      <div class=\"image-card\">
        <img src=\"/${asset.name}\" alt=\"${asset.name}\">
        <p>${asset.name} (${Math.round(asset.size / 1024)}KB)</p>
      </div>
    `;
    })
        <img src="${path}" alt="${path}">
        <p>${path}</p>
      </div>
    `;
    })
    .join("");

  const html = `
    <!DOCTYPE html>
    <html>
    <head>
      <title>Asset Gallery</title>
      <style>
        .gallery { display: grid; grid-template-columns: repeat(auto-fill, minmax(200px, 1fr)); gap: 20px; }
        .image-card { border: 1px solid #ddd; padding: 10px; }
        .image-card img { max-width: 100%; height: auto; }
      </style>
    </head>
    <body>
      <h1>Asset Gallery</h1>
      <div class="gallery">
        ${imageCards}
      </div>
    </body>
    </html>
  `;

  return {
    status: 200,
    body: html,
    contentType: "text/html",
  };
}

routeRegistry.registerRoute("/assets-gallery", "assetGalleryHandler", "GET");
```

### Asset Upload Form

```javascript
function uploadFormHandler(req) {
  const html = `
    <!DOCTYPE html>
    <html>
    <head>
      <title>Upload Asset</title>
    </head>
    <body>
      <h1>Upload Asset</h1>
      <form id="uploadForm">
        <label>
          File Path (e.g., /images/photo.jpg):
          <input type="text" id="path" required>
        </label><br>
        
        <label>
          File:
          <input type="file" id="file" required>
        </label><br>
        
        <button type="submit">Upload</button>
      </form>
      
      <div id="result"></div>
      
      <script>
        document.getElementById('uploadForm').addEventListener('submit', async (e) => {
          e.preventDefault();
          
          const path = document.getElementById('path').value;
          const file = document.getElementById('file').files[0];
          
          // Read file as base64
          const reader = new FileReader();
          reader.onload = async function(e) {
            const base64 = e.target.result.split(',')[1];
            
            // Send to server
            const response = await fetch('/api/upload-asset', {
              method: 'POST',
              headers: { 'Content-Type': 'application/json' },
              body: JSON.stringify({
                path: path,
                mimetype: file.type,
                content: base64
              })
            });
            
            const result = await response.json();
            document.getElementById('result').innerHTML = 
              response.ok 
                ? '<p style="color: green;">Upload successful!</p>' 
                : '<p style="color: red;">Upload failed: ' + result.error + '</p>';
          };
          reader.readAsDataURL(file);
        });
      </script>
    </body>
    </html>
  `;

  return {
    status: 200,
    body: html,
    contentType: "text/html",
  };
}

routeRegistry.registerRoute("/upload-form", "uploadFormHandler", "GET");
```

## Asset API Reference

### `assetStorage.listAssets()`

Returns JSON string with metadata for all assets.

```javascript
const assetsJson = assetStorage.listAssets();
const assets = JSON.parse(assetsJson);
// [
//   {
//     "name": "logo.png",
//     "size": 1024,
//     "mimetype": "image/png",
//     "createdAt": 1699564800000,
//     "updatedAt": 1699564800000
//   },
//   { ... }
// ]
```

### `assetStorage.fetchAsset(publicPath)`

Returns JSON string with asset data.

```javascript
const assetJson = assetStorage.fetchAsset("/logo.png");
const asset = JSON.parse(assetJson);

// Returns:
// {
//   "publicPath": "/logo.png",
//   "mimetype": "image/png",
//   "contentB64": "iVBORw0KGgoAAAANS..."
// }
```

Returns `"null"` if asset not found.

### `assetStorage.upsertAsset(publicPath, mimetype, contentB64)`

Creates or updates an asset.

```javascript
assetStorage.upsertAsset(
  "/images/new.png",
  "image/png",
  "iVBORw0KGgoAAAANS...", // Base64 encoded
);
```

**Parameters:**

- `publicPath` - URL path (e.g., `/images/photo.jpg`)
- `mimetype` - MIME type (e.g., `image/jpeg`)
- `contentB64` - Base64 encoded file content

### `assetStorage.deleteAsset(publicPath)`

Deletes an asset. Returns `true` if deleted, `false` if not found.

```javascript
const deleted = assetStorage.deleteAsset("/old-image.png");
if (deleted) {
  console.log("Asset deleted");
} else {
  console.log("Asset not found");
}
```

## MIME Types Reference

Common MIME types for assets:

### Images

```javascript
"image/jpeg"; // .jpg, .jpeg
"image/png"; // .png
"image/gif"; // .gif
"image/svg+xml"; // .svg
"image/webp"; // .webp
"image/x-icon"; // .ico
```

### Stylesheets & Scripts

```javascript
"text/css"; // .css
"application/javascript"; // .js
"application/json"; // .json
```

### Documents

```javascript
"application/pdf"; // .pdf
"text/plain"; // .txt
"text/html"; // .html
"text/markdown"; // .md
```

### Fonts

```javascript
"font/woff"; // .woff
"font/woff2"; // .woff2
"font/ttf"; // .ttf
"font/otf"; // .otf
```

### Archives

```javascript
"application/zip"; // .zip
"application/gzip"; // .gz
"application/x-tar"; // .tar
```

## Best Practices

### 1. Organize Consistently

Choose an organization pattern and stick to it:

```text
assets/
├── css/
├── js/
└── images/
```

### 2. Use Descriptive Names

**Good:**

- `header-logo.png`
- `main-stylesheet.css`
- `user-profile-default.jpg`

**Bad:**

- `img1.png`
- `style.css`
- `pic.jpg`

### 3. Optimize Assets

- Compress images before uploading
- Minify CSS and JavaScript
- Use appropriate formats (WebP for images, WOFF2 for fonts)

### 4. Version Assets

Include versions in filenames for cache busting:

```text
app.v1.js → app.v2.js
style-2024-01.css
```

Or use query parameters:

```html
<script src="/app.js?v=2"></script>
```

### 5. Clean Up Unused Assets

Regularly remove assets that are no longer referenced:

```javascript
function cleanupHandler(req) {
  const assets = assetStorage.listAssets();
  const unusedAssets = findUnusedAssets(assets);

  unusedAssets.forEach((asset) => {
    assetStorage.deleteAsset(asset);
    console.log(`Deleted unused asset: ${asset}`);
  });

  return {
    status: 200,
    body: JSON.stringify({
      deleted: unusedAssets.length,
    }),
    contentType: "application/json",
  };
}
```

## Common Patterns

### Responsive Images

```javascript
function responsiveImageHandler(req) {
  const html = `
    <!DOCTYPE html>
    <html>
    <body>
      <!-- Responsive image with srcset -->
      <img 
        src="/images/photo-800.jpg"
        srcset="
          /images/photo-400.jpg 400w,
          /images/photo-800.jpg 800w,
          /images/photo-1200.jpg 1200w
        "
        sizes="(max-width: 600px) 400px, 800px"
        alt="Photo"
      >
    </body>
    </html>
  `;

  return { status: 200, body: html, contentType: "text/html" };
}
```

### CSS Themes

```javascript
function themedPageHandler(req) {
  const theme = req.query.theme || "light";

  const html = `
    <!DOCTYPE html>
    <html>
    <head>
      <link rel="stylesheet" href="/css/base.css">
      <link rel="stylesheet" href="/css/theme-${theme}.css">
    </head>
    <body>
      <h1>Themed Page</h1>
    </body>
    </html>
  `;

  return { status: 200, body: html, contentType: "text/html" };
}
```

### Progressive Web App (PWA)

```javascript
function pwaManifestHandler(req) {
  const manifest = {
    name: "My App",
    short_name: "App",
    icons: [
      { src: "/images/icon-192.png", sizes: "192x192", type: "image/png" },
      { src: "/images/icon-512.png", sizes: "512x512", type: "image/png" },
    ],
    start_url: "/",
    display: "standalone",
    theme_color: "#ffffff",
    background_color: "#ffffff",
  };

  return {
    status: 200,
    body: JSON.stringify(manifest),
    contentType: "application/json",
  };
}

routeRegistry.registerRoute("/manifest.json", "pwaManifestHandler", "GET");
```

## Troubleshooting

### Asset Not Loading

**Check:**

- File exists in `assets/` directory
- Path is correct (case-sensitive)
- MIME type is correct
- No typos in URL

### Images Not Displaying

**Check:**

- Image format is supported
- File is not corrupted
- Path starts with `/`
- Browser console for errors

### CSS Not Applied

**Check:**

- `<link>` tag in `<head>`
- Correct `href` path
- CSS syntax is valid
- Browser cache (try hard refresh: Ctrl+F5)

### JavaScript Not Running

**Check:**

- `<script>` tag placement (before closing `</body>` or with `defer`)
- Console for JavaScript errors
- Correct `src` path

## Next Steps

- **[Script Development](scripts.md)** - Learn to create dynamic content
- **[Logging Guide](logging.md)** - Debug asset issues
- **[Examples](../examples/index.md)** - See asset usage in practice
- **[API Reference](../reference/javascript-apis.md)** - Complete API docs

## Quick Reference

```javascript
// List all assets with metadata
const assetsJson = assetStorage.listAssets();
const assets = JSON.parse(assetsJson);
// Each asset has: name, size, mimetype, createdAt, updatedAt

// Get asset data
const assetJson = assetStorage.fetchAsset("/logo.png");
const asset = JSON.parse(assetJson);

// Create/update asset
assetStorage.upsertAsset("/new.png", "image/png", base64Content);

// Delete asset
assetStorage.deleteAsset("/old.png");
```
