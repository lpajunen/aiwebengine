# Web Editor Guide

The aiwebengine web editor is a comprehensive browser-based development environment for creating, editing, testing, and deploying scripts and assets.

## Overview

The editor provides:

- **Monaco Editor** - VS Code's editor engine with syntax highlighting
- **Script Management** - Create, edit, delete, and organize scripts
- **Asset Manager** - Upload and manage static files (CSS, JS, images)
- **Log Viewer** - Real-time log monitoring and filtering
- **AI Assistant** - Integrated AI for code generation and refactoring
- **Live Testing** - Test endpoints immediately after saving
- **Dark Theme** - Modern, eye-friendly interface

## Accessing the Editor

### Local Development

Start your aiwebengine server:

```bash
cargo run
```

Open in browser:

```
http://localhost:8080/editor
```

### Remote Server

If aiwebengine is running on a remote server:

```
https://your-domain.com/editor
```

**Note:** Ensure the editor feature script is loaded and the route is registered.

## Editor Interface

### Main Layout

```text
┌─────────────────────────────────────────────────────────────┐
│  aiwebengine Editor               [Logs] [Test] [Settings]  │
├──────────┬──────────────────────────────────────────────────┤
│          │                                                   │
│ Scripts  │                                                   │
│  📁 api/ │                                                   │
│   users  │         Monaco Code Editor                       │
│   posts  │                                                   │
│  📁 pages│         • Syntax highlighting                    │
│   home   │         • Auto-completion                        │
│   about  │         • Error detection                        │
│          │         • Find & replace                         │
│ Assets   │         • Multi-cursor editing                   │
│  📁 css/ │                                                   │
│  📁 js/  │                                                   │
│  📁 img/ │                                                   │
│          │                                                   │
│          │    [New Script] [Save] [Delete] [Format]         │
├──────────┴──────────────────────────────────────────────────┤
│                                                               │
│  🤖 AI Assistant                                              │
│  > Create a REST API for blog posts...                       │
│                                   [Ask AI] [Clear]            │
└───────────────────────────────────────────────────────────────┘
```

### Key Components

1. **Sidebar (Left)**
   - Script tree with folder organization
   - Asset browser with type filtering
   - Quick navigation

2. **Editor Pane (Center)**
   - Monaco editor with JavaScript mode
   - Line numbers and error indicators
   - Minimap for navigation
   - Status bar with cursor position

3. **Action Bar**
   - New Script button
   - Save button (Cmd+S / Ctrl+S)
   - Delete button
   - Format Code button
   - Test API button

4. **AI Assistant Panel (Bottom)**
   - Natural language input
   - Code generation
   - Code editing with diff preview
   - Explanation mode

5. **Tabs (Top)**
   - Scripts tab (default)
   - Logs tab
   - Test API tab
   - Settings tab (if available)

## Script Management

### Creating a New Script

#### Method 1: Manual Creation

1. Click **"New Script"** button
2. Enter script path:
   - `api/users.js` - For API endpoints
   - `pages/home.js` - For page handlers
   - `features/chat.js` - For features
3. Start coding in the editor
4. Click **"Save"** or press Cmd+S / Ctrl+S

#### Method 2: AI Generation

1. Type in AI Assistant panel:
   ```
   Create a REST API for managing todo items with CRUD operations
   ```
2. Click **"Ask AI"**
3. Review generated code in preview
4. Click **"Preview & Create"**
5. Review the diff
6. Click **"Apply Changes"**

The script is created and immediately available.

### Editing Scripts

#### Opening a Script

- Click script name in sidebar
- Script loads in editor
- Recent scripts appear at top

#### Editing

The Monaco editor provides:

**Syntax Highlighting**

- JavaScript keywords in color
- Strings, numbers, comments differentiated
- Error highlighting in red

**Auto-completion**

- Type to see suggestions
- Tab to accept
- Works for JavaScript built-ins

**Error Detection**

- Real-time syntax checking
- Red squiggly lines for errors
- Hover for error details

**Find and Replace**

- Cmd+F / Ctrl+F to find
- Cmd+H / Ctrl+H to replace
- Regex support
- Case sensitive toggle

**Multi-cursor Editing**

- Alt+Click to add cursors
- Edit multiple lines simultaneously
- Cmd+D / Ctrl+D to select next occurrence

**Code Formatting**

- Click "Format" button
- Or Shift+Alt+F / Shift+Option+F
- Auto-indents and cleans code

#### Saving Changes

**Manual Save:**

- Click "Save" button
- Or Cmd+S / Ctrl+S
- Green indicator shows success

**Auto-reload:**

- Server automatically reloads script
- Routes re-register
- Changes take effect immediately
- No server restart needed

### Deleting Scripts

1. Select script in sidebar
2. Click **"Delete"** button
3. Confirm deletion in dialog
4. Script and routes removed immediately

**Warning:** Deletion is permanent. No undo.

### Organizing Scripts

**Recommended structure:**

```
scripts/
├── api/
│   ├── users.js
│   ├── posts.js
│   └── auth.js
├── pages/
│   ├── home.js
│   ├── about.js
│   └── contact.js
└── features/
    ├── chat.js
    └── notifications.js
```

**Naming conventions:**

- Use descriptive names
- Use folders for organization
- Lowercase with hyphens: `user-profile.js`
- Group related functionality

## Asset Management

### Uploading Assets

#### Single File Upload

1. Click **"Assets"** in sidebar
2. Click **"Upload Assets"** button
3. Select file(s) from computer
4. Files upload immediately
5. Available at their URL path

#### Drag and Drop (if supported)

1. Drag files from computer
2. Drop on asset area
3. Automatic upload

#### Supported File Types

- **Images:** PNG, JPEG, GIF, SVG, WebP, ICO
- **Styles:** CSS
- **Scripts:** JavaScript
- **Documents:** PDF, TXT, MD
- **Fonts:** WOFF, WOFF2, TTF, OTF
- **Archives:** ZIP, GZ
- **Any other file type**

### Viewing Assets

**Asset Browser:**

- Organized by folder
- Type icons (image, CSS, JS, etc.)
- Image previews
- File size shown
- Upload date

**Actions per asset:**

- 👁️ Preview (images)
- ⬇️ Download
- 🗑️ Delete

### Organizing Assets

**Create folder structure:**

- Upload with paths: `css/main.css`
- Upload with paths: `images/logo.png`
- Folders created automatically

**Recommended structure:**

```
assets/
├── css/
│   ├── main.css
│   └── theme.css
├── js/
│   ├── app.js
│   └── utils.js
└── images/
    ├── logo.png
    └── banner.jpg
```

### Using Uploaded Assets

Assets are immediately accessible:

```javascript
function homeHandler(req) {
  const html = `
    <!DOCTYPE html>
    <html>
    <head>
      <link rel="stylesheet" href="/css/main.css">
      <script src="/js/app.js"></script>
    </head>
    <body>
      <img src="/images/logo.png" alt="Logo">
      <h1>Welcome</h1>
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

## Log Viewing

### Accessing Logs

**Click "Logs" tab** at the top of editor.

### Log Features

**Real-time Updates:**

- Logs refresh every 5 seconds
- New entries appear at top
- Auto-scroll to latest

**Filtering:**

- Filter by script (dropdown)
- Search logs (text input)
- Filter by level (if tagged)

**Log Display:**

- Timestamp for each entry
- Script URI shown
- Color-coded by level (if tagged)
- Copy log entries

### Using Logs

**Viewing specific script logs:**

1. Select script from dropdown
2. Only that script's logs shown
3. Click "All Scripts" to see everything

**Searching logs:**

1. Type search term
2. Matching entries highlighted
3. Use for debugging specific issues

**Jump to latest:**

Click "Jump to latest" to scroll the view to the newest log entry.

### Log Patterns in Output

```
[2024-10-24 10:30:15] api/users.js: User created: john@example.com
[2024-10-24 10:30:16] api/users.js: [ERROR] Email already exists
[2024-10-24 10:30:17] pages/home.js: Page rendered for user 123
```

## AI Assistant

### Overview

The AI Assistant understands:

- All aiwebengine JavaScript APIs
- Your current script content
- All scripts in workspace
- Best practices

### Using the AI Assistant

#### Basic Usage

1. **Type your request** in AI input field
2. **Click "Ask AI"** or press Enter
3. **Wait for response** (few seconds)
4. **Review the result**

#### Response Types

**1. Explanation**

Ask for understanding:

```
Explain what this script does
How does streaming work?
What is the fetch() function?
```

**2. Code Generation**

Create new scripts:

```
Create a REST API for blog posts with CRUD operations
Create a contact form with validation
Build a chat system using Server-Sent Events
```

Click **"Preview & Create"** to review before applying.

**3. Code Editing**

Modify existing code:

```
Add error handling to all functions
Add input validation
Refactor to use async/await
Add logging to track requests
```

Review in **side-by-side diff** before applying.

**4. Debugging**

Fix issues:

```
This script returns 500 errors, please fix
Why isn't this form handler working?
Help me debug the authentication logic
```

### Effective AI Prompts

**Be specific:**

✅ Good:

```
Create a user registration API that validates email,
checks for duplicates, hashes passwords, and returns a JWT token
```

❌ Bad:

```
Create a registration API
```

**Provide context:**

✅ Good:

```
Add authentication to this API using the built-in auth functions.
Require login for all endpoints except /api/public
```

❌ Bad:

```
Add authentication
```

**Describe outcome:**

✅ Good:

```
Refactor this script to:
- Use helper functions for validation
- Add JSDoc comments
- Return consistent JSON error responses
- Add request logging
```

❌ Bad:

```
Make this better
```

### AI Workflow Example

**1. Generate initial code:**

```
Create a blog API with list and create endpoints
```

**2. Review and apply:**

- Check generated code
- Click "Apply Changes"
- Test the endpoints

**3. Iterate with improvements:**

```
Add pagination to the list endpoint
```

**4. Add more features:**

```
Add a search endpoint with query parameter
```

**5. Enhance quality:**

```
Add error handling and input validation
```

## Testing Scripts

### Quick Test

1. Click **"Test API"** button or tab
2. Enter endpoint path: `/api/users`
3. Select HTTP method: GET, POST, PUT, DELETE
4. Add parameters (query or form data)
5. Click **"Send Request"**
6. View response:
   - Status code
   - Headers
   - Body
   - Time taken

### Testing Workflow

**After creating a script:**

1. **Save the script** (Cmd+S / Ctrl+S)
2. **Click "Test API"** tab
3. **Enter your endpoint** (e.g., `/api/hello`)
4. **Click "Send Request"**
5. **Verify response**
6. **Check logs** for any errors

**For forms (POST):**

1. Select **POST** method
2. Add form fields:
   - name: `John`
   - email: `john@example.com`
3. Click **"Send Request"**
4. Verify form processing

### Browser Testing

Open a new tab and navigate to your endpoint:

```
http://localhost:8080/api/users
http://localhost:8080/hello?name=World
http://localhost:8080/
```

**Use DevTools:**

- F12 to open
- Network tab to see requests
- Console for JavaScript errors
- Inspect response data

## Keyboard Shortcuts

### Editor Shortcuts

| Action                 | Windows/Linux  | macOS             |
| ---------------------- | -------------- | ----------------- |
| Save script            | Ctrl+S         | Cmd+S             |
| Find                   | Ctrl+F         | Cmd+F             |
| Replace                | Ctrl+H         | Cmd+H             |
| Find next              | F3             | Cmd+G             |
| Select next occurrence | Ctrl+D         | Cmd+D             |
| Select all occurrences | Ctrl+Shift+L   | Cmd+Shift+L       |
| Comment line           | Ctrl+/         | Cmd+/             |
| Undo                   | Ctrl+Z         | Cmd+Z             |
| Redo                   | Ctrl+Y         | Cmd+Shift+Z       |
| Format code            | Shift+Alt+F    | Shift+Option+F    |
| Go to line             | Ctrl+G         | Cmd+G             |
| Multi-cursor           | Alt+Click      | Option+Click      |
| Column select          | Shift+Alt+Drag | Shift+Option+Drag |

### Navigation Shortcuts

| Action             | Shortcut          |
| ------------------ | ----------------- |
| Focus editor       | Click editor area |
| Focus sidebar      | Click sidebar     |
| Focus AI assistant | Click AI input    |
| Switch to Logs     | Click Logs tab    |
| Switch to Test     | Click Test tab    |

## Advanced Features

### Code Snippets

Type and press Tab:

```javascript
// Type: handler
function myHandler(req) {
  return {
    status: 200,
    body: "",
    contentType: "text/plain; charset=UTF-8",
  };
}

// Type: init
function init() {
  routeRegistry.registerRoute("/path", "handlerName", "GET");
}

init();
```

### Multiple Cursors

**Add cursor:** Alt+Click (Option+Click on Mac)

**Edit multiple lines:**

1. Alt+Click to place cursors
2. Type simultaneously on all lines
3. Press Escape to return to single cursor

**Select all occurrences:**

1. Select a word
2. Ctrl+Shift+L (Cmd+Shift+L on Mac)
3. All occurrences selected
4. Edit simultaneously

### Find and Replace with Regex

1. Open Find: Ctrl+F / Cmd+F
2. Click regex button `.*`
3. Enter pattern: `function (\w+)Handler`
4. Replace with: `function handle$1`
5. Replace All

## Best Practices

### 1. Save Frequently

- Use Cmd+S / Ctrl+S after changes
- Editor auto-saves on blur (optional)
- Save before testing

### 2. Use Folders

Organize scripts logically:

```
api/ - API endpoints
pages/ - Web pages
features/ - Feature modules
utils/ - Helper scripts
```

### 3. Test Immediately

- Save → Test workflow
- Check logs after each test
- Fix errors before moving on

### 4. Leverage AI

- Generate boilerplate with AI
- Ask AI to add features
- Use AI for refactoring
- Get AI explanations

### 5. Keep Assets Organized

- Use folders: `css/`, `js/`, `images/`
- Descriptive filenames
- Delete unused assets

### 6. Review Before Applying AI Changes

- Always check diff preview
- Understand what AI changed
- Test after applying
- Iterate if needed

### 7. Use Logs for Debugging

- Add `console.log()` liberally
- Check logs after requests
- Filter by script when debugging
- Remove excessive logging later

## Troubleshooting

### Editor Won't Load

**Check:**

- Server is running (`cargo run`)
- Editor script is loaded
- Navigate to correct URL
- Check browser console for errors

**Solutions:**

- Restart server
- Clear browser cache
- Try different browser
- Check server logs

### Can't Save Script

**Check:**

- Valid JavaScript syntax
- No reserved filenames
- Server has write permissions
- Network connection stable

**Solutions:**

- Fix syntax errors (red squiggles)
- Choose different filename
- Check server logs
- Refresh editor

### Scripts Not Appearing

**Check:**

- Script was saved successfully
- Refresh sidebar
- Check scripts directory on server

**Solutions:**

- Click refresh in sidebar
- Reload browser page
- Verify file exists on server

### AI Not Responding

**Check:**

- AI service configured on server
- Network connection
- No errors in browser console

**Solutions:**

- Wait (AI can take a few seconds)
- Refresh page
- Try simpler prompt
- Check server configuration

### Assets Not Uploading

**Check:**

- File size limits
- Supported file types
- Server has write permissions
- Network connection

**Solutions:**

- Compress large files
- Check server settings
- Try smaller file first
- Check server logs

### Logs Not Showing

**Check:**

- Script has `console.log()` calls
- Script has been executed
- Correct script selected in filter
- Logs tab selected

**Solutions:**

- Add `console.log()` to script
- Execute script (make a request)
- Select correct script
- Click "Jump to latest" (scrolls view to newest entry)

## Editor API Endpoints

For programmatic access:

### Scripts

```bash
# List all scripts
GET /api/scripts

# Get script content
GET /api/scripts/api/users.js

# Create/update script
POST /api/scripts/api/users.js
Content-Type: application/json
{"content": "function handler(req) {...}"}

# Delete script
DELETE /api/scripts/api/users.js
```

### Assets

```bash
# List all assets
GET /api/assets

# Get asset data
GET /api/assets/logo.png

# Upload asset
POST /api/assets
Content-Type: application/json
{"path": "/logo.png", "mimetype": "image/png", "content": "base64..."}

# Delete asset
DELETE /api/assets/logo.png
```

### Logs

```bash
# Get all logs
GET /api/logs

# Get logs for specific script
GET /script_logs?uri=/api/users
```

## Tips and Tricks

### 1. Quick Script Creation

Use AI to generate complete scripts:

```
Create a simple hello world API at /hello
```

Then customize as needed.

### 2. Bulk Editing

Use Find & Replace (Ctrl+H / Cmd+H) to:

- Rename functions across file
- Update API paths
- Change variable names
- Fix common patterns

### 3. Code Navigation

- Ctrl+G / Cmd+G to jump to line
- Ctrl+F / Cmd+F to find text
- Minimap for quick scrolling

### 4. Asset Preview

Click asset in sidebar to preview:

- Images show inline
- Text files can be viewed
- Download any asset

### 5. Split Testing

- Open editor in one tab
- Open endpoint in another tab
- Edit → Save → Refresh test tab

### 6. Log Filtering

Add tags to logs:

```javascript
console.log("[INFO] User logged in");
console.log("[ERROR] Database connection failed");
console.log("[DEBUG] Processing 100 items");
```

Then search for `[ERROR]` to find errors.

## Next Steps

- **[Deployer Tool](deployer.md)** - CLI deployment workflow
- **[External Tools](external-tools.md)** - VS Code, Git integration
- **[Script Development](../guides/scripts.md)** - Deep dive into scripting
- **[AI Development](../guides/ai-development.md)** - Master AI assistance

## Quick Reference

### Common Tasks

```text
Create script → New Script → Enter name → Code → Save
Upload asset → Assets → Upload → Select file
View logs → Logs tab → Select script → Read
Test API → Test tab → Enter endpoint → Send
Use AI → Type request → Ask AI → Review → Apply
```

### Essential Shortcuts

```text
Save: Cmd+S / Ctrl+S
Find: Cmd+F / Ctrl+F
Format: Shift+Alt+F / Shift+Option+F
Multi-cursor: Alt+Click / Option+Click
```

The web editor is your complete development environment for aiwebengine. Master it to build applications efficiently! 🚀
