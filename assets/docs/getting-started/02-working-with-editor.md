# Working with the Web Editor

The aiwebengine web editor is a powerful browser-based development environment that lets you create, edit, test, and deploy scripts without leaving your browser.

## Overview

The editor provides:

- **Script Editor** with Monaco Editor (VS Code's editor engine)
- **Asset Manager** for uploading images, CSS, JavaScript files
- **Log Viewer** for monitoring and debugging
- **AI Assistant** for code generation and refactoring
- **Live Preview** for testing changes

## Accessing the Editor

Once your aiwebengine instance is running:

```text
http://localhost:8080/editor
```

Or on a remote server:

```text
https://your-domain.com/editor
```

## Editor Interface

### Layout

```text
┌─────────────────────────────────────────────────────┐
│  aiwebengine Editor                    [Logs] [Test]│
├──────────┬──────────────────────────────────────────┤
│          │                                           │
│ Scripts  │                                           │
│  └─ /    │         Monaco Code Editor                │
│  └─ api/ │                                           │
│  └─ app/ │                                           │
│          │                                           │
│ Assets   │                                           │
│  └─ css/ │                                           │
│  └─ js/  │                                           │
│          │                                           │
│          │           [New] [Save] [Delete]           │
├──────────┴──────────────────────────────────────────┤
│                                                       │
│  AI Assistant                                         │
│  > Create a REST API for blog posts...                │
│                                     [Ask AI]          │
└───────────────────────────────────────────────────────┘
```

### Key Components

1. **Sidebar** - Navigate scripts and assets
2. **Editor Pane** - Write and edit code with syntax highlighting
3. **Action Bar** - Save, delete, test scripts
4. **AI Assistant Panel** - Get AI help with your code
5. **Logs Tab** - View script execution logs
6. **Test Tab** - Test API endpoints

## Creating a New Script

### Method 1: Manual Creation

1. Click **"New Script"** button
2. Enter the script path (e.g., `api/users.js`)
3. Write your code in the editor
4. Click **"Save"**

### Method 2: Using AI Assistant

1. Type in the AI Assistant panel:

   ```text
   Create a REST API for managing todo items with CRUD operations
   ```

2. Click **"Ask AI"**
3. Review the generated code
4. Click **"Preview & Create"**
5. Review the diff
6. Click **"Apply Changes"**

### Script Naming Conventions

**Good names:**

- `api/users.js` - API endpoints
- `pages/home.js` - Page handlers
- `features/chat.js` - Feature modules
- `utils/helpers.js` - Utility functions

**Avoid:**

- Special characters: `api/users!.js` ❌
- Spaces: `my script.js` ❌
- Starting with `/`: `/api/users.js` ❌

## Editing Scripts

### Basic Editing

1. **Select a script** from the sidebar
2. **Edit** in the Monaco editor
3. **Save** your changes (Cmd+S / Ctrl+S)

The editor provides:

- **Syntax highlighting** for JavaScript
- **Auto-completion** for standard JavaScript
- **Error detection** as you type
- **Multi-cursor editing** (Alt+Click)
- **Find and replace** (Cmd+F / Ctrl+F)

### Using AI for Editing

Select a script and ask the AI to modify it:

**Example prompts:**

```text
Add error handling to all functions
```

```text
Add input validation for the form data
```

```text
Refactor this to use async/await
```

```text
Add logging to track all API calls
```

The AI will:

1. Analyze your current code
2. Generate modifications
3. Show a side-by-side diff
4. Let you apply or reject changes

### Keyboard Shortcuts

| Action       | Windows/Linux | macOS          |
| ------------ | ------------- | -------------- |
| Save         | Ctrl+S        | Cmd+S          |
| Find         | Ctrl+F        | Cmd+F          |
| Replace      | Ctrl+H        | Cmd+H          |
| Comment line | Ctrl+/        | Cmd+/          |
| Undo         | Ctrl+Z        | Cmd+Z          |
| Redo         | Ctrl+Y        | Cmd+Shift+Z    |
| Format code  | Shift+Alt+F   | Shift+Option+F |

## Managing Assets

Assets are static files like images, CSS, JavaScript, PDFs that your scripts can serve.

### Uploading Assets

1. Click **"Assets"** in the sidebar
2. Click **"Upload Assets"**
3. Select files from your computer
4. Files are uploaded and immediately available

### Asset URLs

After uploading `logo.png`, it's available at:

```text
http://localhost:8080/logo.png
```

Or in a subdirectory:

```text
/assets/images/logo.png → http://localhost:8080/assets/images/logo.png
```

### Using Assets in Scripts

**HTML example:**

```javascript
function homeHandler(req) {
  const html = `
    <!DOCTYPE html>
    <html>
    <head>
      <link rel="stylesheet" href="/style.css">
      <script src="/app.js"></script>
    </head>
    <body>
      <img src="/logo.png" alt="Logo">
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

### Deleting Assets

1. Find the asset in the Assets section
2. Click the **trash icon**
3. Confirm deletion

## Viewing Logs

Logs help you debug and monitor your scripts.

### Accessing Logs

**Method 1: Logs Tab**

1. Click **"Logs"** tab at the top
2. Select a script from the dropdown
3. Logs refresh automatically every 5 seconds
4. Click **"Refresh"** for manual update

**Method 2: Script Context**

1. Select a script in the sidebar
2. Your script's logs appear at the bottom
3. Shows only logs from that specific script

### Log Levels

aiwebengine logs capture:

- **Script initialization** - When scripts load
- **Route registration** - When routes are registered
- **Custom logs** - Your `console.log()` calls
- **Errors** - When handlers throw errors

### Example Log Output

```text
[2024-10-24 10:30:00] api/users.js initialized
[2024-10-24 10:30:01] Route registered: GET /api/users
[2024-10-24 10:30:15] Fetching user list: 25 users found
[2024-10-24 10:30:16] User created: john@example.com
[2024-10-24 10:30:20] Error in createUser: Email already exists
```

### Writing Better Logs

**Good logging practices:**

```javascript
function createUser(req) {
  const email = req.form.email;

  // Log important actions
  console.log(`Creating user: ${email}`);

  try {
    // ... create user logic
    console.log(`User created successfully: ${email}`);

    return { status: 201, body: "User created" };
  } catch (error) {
    // Log errors with context
    console.log(`Error creating user ${email}: ${error.message}`);

    return { status: 500, body: "Failed to create user" };
  }
}
```

## Testing Your Scripts

### Quick Test

1. Click **"Test API"** button
2. Enter the endpoint path (e.g., `/api/users`)
3. Select HTTP method (GET, POST, etc.)
4. Add query parameters or form data
5. Click **"Send Request"**
6. View the response

### Browser Testing

After saving a script:

1. Open a new tab
2. Navigate to your endpoint:

   ```text
   http://localhost:8080/your-endpoint
   ```

3. Test with query parameters:

   ```text
   http://localhost:8080/api/users?page=1&limit=10
   ```

### Using Browser DevTools

1. Open DevTools (F12)
2. Go to **Network** tab
3. Make requests to your endpoints
4. Inspect:
   - Request headers
   - Response status
   - Response body
   - Timing information

## AI Assistant Features

The AI assistant understands your codebase and can help with:

### Code Generation

**Prompt:**

```text
Create a REST API for blog posts with:
- GET /api/posts - list all posts
- GET /api/posts/:id - get single post
- POST /api/posts - create new post
- PUT /api/posts/:id - update post
- DELETE /api/posts/:id - delete post
```

**Result:** Complete, working script with all CRUD operations

### Code Refactoring

**Prompt:**

```text
Refactor this script to:
- Use async/await instead of callbacks
- Add JSDoc comments
- Extract validation into a separate function
```

**Result:** Improved code with side-by-side diff preview

### Debugging Help

**Prompt:**

```text
This script is returning 500 errors. Can you add better error handling?
```

**Result:** Enhanced error handling with try-catch blocks and logging

### Learning and Explanation

**Prompt:**

```text
Explain how the streaming works in this script
```

**Result:** Detailed explanation of the code

### Best AI Prompts

**Be specific:**

✅ "Create a form handler that validates email and sends a confirmation"

❌ "Create a form"

**Provide context:**

✅ "Add authentication to this API using the auth functions"

❌ "Add authentication"

**Describe the outcome:**

✅ "Convert this to return JSON instead of HTML with proper status codes"

❌ "Make it better"

## Workflow Examples

### Workflow 1: Building a New Feature

1. **Plan** - Decide what you need
2. **Ask AI** - "Create a contact form with email validation"
3. **Review** - Check the generated code
4. **Apply** - Save the script
5. **Test** - Try it in the browser
6. **View Logs** - Check for issues
7. **Iterate** - Ask AI to refine

### Workflow 2: Debugging an Issue

1. **View Logs** - Find error messages
2. **Select Script** - Open the problematic script
3. **Ask AI** - "Fix the error in line 23"
4. **Review Diff** - See proposed changes
5. **Apply** - Save the fix
6. **Test** - Verify it works
7. **Monitor Logs** - Confirm fix

### Workflow 3: Adding Assets

1. **Upload** - Add CSS/JS files
2. **Create Script** - Build page handler
3. **Reference Assets** - Link to uploaded files
4. **Test** - View in browser
5. **Adjust** - Edit CSS/JS as needed

## Tips and Best Practices

### Editor Tips

1. **Save frequently** - Changes only persist when saved
2. **Use Cmd+S / Ctrl+S** - Quick save shortcut
3. **Format code** - Use Shift+Alt+F for clean formatting
4. **Search across files** - Use find to locate code

### AI Tips

1. **Start simple** - Get basic working code first
2. **Iterate** - Refine with follow-up prompts
3. **Review diffs carefully** - Don't blindly apply changes
4. **Learn from AI code** - Study what it generates

### Testing Tips

1. **Test immediately** - After every save
2. **Check logs** - After every test
3. **Use browser DevTools** - Inspect network requests
4. **Test edge cases** - Empty params, invalid data

### Asset Tips

1. **Organize assets** - Use folders (css/, js/, images/)
2. **Optimize images** - Compress before uploading
3. **Version assets** - Use filenames like app.v2.js
4. **Clean up** - Delete unused assets

## Common Tasks

### Task: Create a New API

1. Click **"New Script"**
2. Name it `api/myapi.js`
3. Write or generate code:

   ```javascript
   function apiHandler(req) {
     return {
       status: 200,
       body: JSON.stringify({ message: "Hello API" }),
       contentType: "application/json",
     };
   }

   function init() {
     routeRegistry.registerRoute("/api/myapi", "apiHandler", "GET");
   }

   init();
   ```

4. Save and test at `/api/myapi`

### Task: Add a Web Page

1. Upload CSS to `/style.css`
2. Create script `pages/home.js`
3. Write HTML handler:

   ```javascript
   function homeHandler(req) {
     return {
       status: 200,
       body: `
         <!DOCTYPE html>
         <html>
         <head>
           <link rel="stylesheet" href="/style.css">
         </head>
         <body>
           <h1>Home Page</h1>
         </body>
         </html>
       `,
       contentType: "text/html",
     };
   }

   function init() {
     routeRegistry.registerRoute("/", "homeHandler", "GET");
   }

   init();
   ```

4. Visit `/` to see your page

### Task: Update an Existing Script

1. Select script from sidebar
2. Make your edits
3. Save (Cmd+S / Ctrl+S)
4. Test to verify changes
5. Check logs for errors

## Troubleshooting

### Script Won't Save

**Check:**

- Valid JavaScript syntax
- No reserved filenames
- Proper permissions on server

### Can't See My Logs

**Check:**

- Selected correct script in logs viewer
- Script has `console.log()` calls
- Script has been executed at least once

### Assets Not Loading

**Check:**

- File was uploaded successfully
- Correct path in URL
- No typos in filename
- Server has read permissions

### AI Not Responding

**Check:**

- AI service is configured on server
- Network connection is stable
- Try refreshing the page

## Next Steps

Now that you understand the editor:

1. **[Learn Deployment Workflows](03-deployment-workflow.md)** - Different ways to deploy scripts
2. **[Master Script Development](../guides/scripts.md)** - Deep dive into script features
3. **[Explore AI Development](../guides/ai-development.md)** - Advanced AI-assisted coding
4. **[See Examples](../examples/index.md)** - Study real-world patterns

## Quick Reference

### Editor Actions

| Action        | How To                            |
| ------------- | --------------------------------- |
| New script    | Click "New Script" button         |
| Save script   | Cmd+S / Ctrl+S or "Save" button   |
| Delete script | "Delete" button                   |
| Upload asset  | "Upload Assets" in Assets section |
| View logs     | "Logs" tab or script context      |
| Test API      | "Test API" button                 |
| AI help       | Type in AI Assistant panel        |

### AI Prompt Templates

```text
Create [feature description]
Add [enhancement] to this script
Fix [problem description]
Refactor to [improvement]
Explain [concept in the code]
Convert this to [format/pattern]
```

Happy coding! 🚀
