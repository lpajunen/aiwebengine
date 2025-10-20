# How to Use the AI Assistant

## Understanding aiwebengine Scripts

**Important:** aiwebengine scripts are **server-side JavaScript** that handle HTTP requests and return responses (HTML, JSON, etc.).

### What Scripts Do

Scripts are NOT:
- ❌ Client-side browser JavaScript
- ❌ HTML files
- ❌ Static web pages

Scripts ARE:
- ✅ Server-side request handlers
- ✅ Functions that return HTML/JSON/text
- ✅ API endpoints
- ✅ Web page generators

## How to Ask for What You Want

### ✅ Correct Prompts

#### For Web Pages
```
Create a script that serves a simple web page with a welcome message
```
**Result:** Script that handles `/welcome` and returns HTML

```
Create a homepage with navigation and content sections
```
**Result:** Script that serves HTML at `/` with proper structure

```
Create a contact form page
```
**Result:** Script with GET handler (shows form) and POST handler (processes form)

#### For APIs
```
Create a REST API for managing blog posts
```
**Result:** Script with CRUD endpoints (GET, POST, PUT, DELETE)

```
Create an API that returns user data in JSON
```
**Result:** Script with `/api/users` endpoint returning JSON

```
Create a weather API that fetches data from OpenWeather
```
**Result:** Script that calls external API and returns formatted data

#### For Specific Features
```
Create a script that uploads and stores images
```
**Result:** Script with file upload handling

```
Create a real-time notification system using WebStreams
```
**Result:** Script with SSE streaming endpoints

```
Create an authentication endpoint
```
**Result:** Script with login/logout handlers

### ❌ Incorrect Prompts (and how to fix them)

| ❌ Don't say | ✅ Say instead |
|-------------|---------------|
| "Create a simple web page" | "Create a script that serves a simple web page" |
| "Make a homepage" | "Create a script for the homepage" |
| "Build a login page" | "Create a login script with a page and handler" |
| "Create HTML for..." | "Create a script that returns HTML for..." |

## Prompt Examples by Use Case

### 1. Simple Web Page

**Prompt:**
```
Create a script that serves a welcome page at /welcome with a greeting
```

**What you get:**
- Script named `welcome.js`
- Handler function that returns HTML
- Route registered at `/welcome`
- Preview button to see the changes

### 2. Multi-Page Application

**Prompt:**
```
Create a script with multiple pages: home, about, and contact
```

**What you get:**
- Script with 3 handler functions
- Routes for `/`, `/about`, `/contact`
- Each handler returns appropriate HTML

### 3. JSON API

**Prompt:**
```
Create a REST API for todo items with list, create, and delete endpoints
```

**What you get:**
- Script with 3 handlers (GET all, POST create, DELETE)
- Routes like `/api/todos`, `/api/todos/:id`
- JSON responses with proper content types

### 4. Form Handling

**Prompt:**
```
Create a feedback form that accepts POST requests and returns a thank you page
```

**What you get:**
- GET handler showing the form HTML
- POST handler processing form data
- Routes for both GET and POST

### 5. External API Integration

**Prompt:**
```
Create a script that fetches GitHub user info and displays it
```

**What you get:**
- Handler using fetch() to call GitHub API
- Route that accepts username parameter
- HTML or JSON response with user data

### 6. File Operations

**Prompt:**
```
Create an image upload endpoint that accepts files
```

**What you get:**
- POST handler accepting multipart/form-data
- File processing logic
- Response with upload status

## Understanding the Response

### When AI Suggests Creating a Script

You'll see:
1. **Action badge** - "CREATE SCRIPT"
2. **Explanation** - What the script does
3. **Script name** - e.g., `welcome.js`
4. **Preview button** - "Preview & Create"

Click "Preview & Create" to:
- See the complete code
- Review before creating
- Apply or reject

### When AI Suggests Editing

You'll see:
1. **Action badge** - "EDIT SCRIPT"  
2. **Explanation** - What changes are being made
3. **Preview button** - "Preview Changes"

Click "Preview Changes" to:
- See side-by-side diff
- Review what changes
- Apply or reject

## Tips for Better Results

### Be Specific About Routes
```
❌ Create a web page
✅ Create a script that serves a homepage at /
```

### Specify Response Type
```
❌ Create an API
✅ Create a JSON API that returns user data
```

### Mention HTTP Methods
```
❌ Create a form handler
✅ Create a form with GET (show form) and POST (process submission)
```

### Include Context
```
❌ Add validation
✅ Add input validation to the contact form POST handler
```

## Common Patterns

### Pattern 1: Static Page
**Request:** "Create an about page"

**Generated Code Structure:**
```javascript
function serveAbout(req) {
  const html = '<!DOCTYPE html>...';
  return { status: 200, body: html, contentType: 'text/html' };
}

function init(context) {
  register('/about', 'serveAbout', 'GET');
}
```

### Pattern 2: JSON API
**Request:** "Create a users API"

**Generated Code Structure:**
```javascript
function getUsers(req) {
  const users = [{id: 1, name: 'Alice'}];
  return {
    status: 200,
    body: JSON.stringify(users),
    contentType: 'application/json'
  };
}

function init(context) {
  register('/api/users', 'getUsers', 'GET');
}
```

### Pattern 3: Form Handler
**Request:** "Create a contact form"

**Generated Code Structure:**
```javascript
function showForm(req) {
  const html = '<form method="POST">...</form>';
  return { status: 200, body: html, contentType: 'text/html' };
}

function handleSubmit(req) {
  const { name, email, message } = req.form;
  // Process form data
  return { status: 200, body: 'Thank you!', contentType: 'text/html' };
}

function init(context) {
  register('/contact', 'showForm', 'GET');
  register('/contact', 'handleSubmit', 'POST');
}
```

## Editing Existing Scripts

### Select a Script First
1. Click on the script in the sidebar
2. The script loads in the editor
3. AI now has context of what you're editing

### Then Ask for Changes
```
Add error handling to all functions
Add logging for debugging
Refactor to use modern JavaScript
Add input validation
```

### The AI Will:
- Analyze your current code
- Show you the exact changes
- Let you preview before applying

## Troubleshooting

### "Response is not JSON"
- AI didn't follow format
- Will be shown as plain text
- Try rephrasing your prompt more specifically

### "Response truncated"
- Request was too complex
- Break it into smaller steps
- Ask for one feature at a time

### "No preview button"
- AI gave an explanation instead of action
- Rephrase to explicitly ask for creation/editing

### "Can't apply changes"
- Check browser console for errors
- Verify script name is valid
- Check server logs

## Best Practices

1. **Start simple, then iterate**
   - First: "Create a basic homepage"
   - Then: "Add a navigation menu"
   - Then: "Add a contact form"

2. **One feature at a time**
   - Don't ask for 10 endpoints at once
   - Build incrementally
   - Review each change

3. **Be explicit about routes**
   - Always mention the URL path
   - Specify HTTP methods (GET/POST/etc.)
   - Clear about what the endpoint does

4. **Review before applying**
   - Always preview changes
   - Check the diff carefully
   - Understand what's changing

5. **Use context effectively**
   - Select the script you want to modify
   - AI will see the code and make better suggestions
   - Can reference other scripts by name

## Summary

**Remember:** When you ask to "create a web page", you're really asking to create a **server-side script that handles requests and returns HTML**.

The AI assistant helps you write these scripts quickly and correctly, with proper error handling, routing, and best practices built in!
