# AI-Assisted Development Guide

Learn how to use AI tools, especially the built-in AI assistant, to accelerate script development in aiwebengine.

## Overview

AI can help you:

- **Generate scripts** from natural language descriptions
- **Edit existing code** with targeted improvements
- **Debug issues** by analyzing error logs
- **Learn APIs** through examples and explanations
- **Refactor code** for better quality
- **Write documentation** and comments

## Using the Built-in AI Assistant

The aiwebengine editor includes an integrated AI assistant that understands your codebase and the platform APIs.

### Accessing the AI Assistant

1. Open `/editor` in your browser
2. Look for the AI Assistant panel (usually at the bottom)
3. Type your request
4. Click "Ask AI" or press Enter

### AI Assistant Features

#### 1. Code Generation

Create complete scripts from descriptions:

**Example Prompt:**

```text
Create a REST API for managing blog posts with CRUD operations
```

**What the AI provides:**

- Complete working script
- All necessary handlers
- Route registrations
- Error handling
- Input validation

**Click "Preview & Create"** to review before applying.

#### 2. Code Editing

Modify existing scripts with surgical precision:

**Example Prompts:**

```text
Add error handling to all functions

Add input validation for email fields

Refactor this to use helper functions

Add logging to track all API calls

Convert this to return JSON instead of HTML
```

**Features:**

- Side-by-side diff preview
- See exact changes before applying
- Accept or reject modifications

#### 3. Code Explanation

Understand how code works:

**Example Prompts:**

```text
Explain what this script does

How does the streaming work here?

What is the purpose of the init function?

Explain the error handling in this code
```

#### 4. Debugging Help

Get assistance fixing issues:

**Example Prompts:**

```text
This script returns 500 errors. Can you fix it?

Why isn't this form handler working?

Add better error messages to this script

Help me debug the authentication logic
```

### Writing Effective Prompts

#### Be Specific

**Good:**

```text
Create a contact form that validates email addresses, stores submissions,
and sends a confirmation email
```

**Bad:**

```text
Create a form
```

#### Provide Context

**Good:**

```text
Add authentication to this API using the built-in auth functions.
Require login for all endpoints except /api/public
```

**Bad:**

```text
Add authentication
```

#### Describe the Outcome

**Good:**

```text
Refactor this script to:
- Use async/await instead of callbacks
- Add JSDoc comments to all functions
- Extract validation into separate helper functions
- Return consistent JSON error responses
```

**Bad:**

```text
Make this better
```

#### Include Requirements

**Good:**

```text
Create a user registration API that:
- Validates email format
- Checks if email already exists
- Hashes passwords
- Returns JWT token
- Logs all registration attempts
```

**Bad:**

```text
Create a registration API
```

## Understanding aiwebengine Scripts

### Key Concept: Server-Side Handlers

aiwebengine scripts are **server-side JavaScript** that handle HTTP requests:

**Scripts ARE:**

- ✅ Server-side request handlers
- ✅ Functions that return HTML/JSON/text
- ✅ API endpoints
- ✅ Web page generators

**Scripts are NOT:**

- ❌ Client-side browser JavaScript
- ❌ Static HTML files
- ❌ Standalone web pages

### Correct Prompts for Different Scenarios

#### Web Pages

```text
Create a script that serves a welcome page at /welcome

Create a homepage with navigation, hero section, and footer

Create a contact page with a form
```

#### APIs

```text
Create a REST API for managing todo items

Create an API endpoint that returns user data as JSON

Create a search API that accepts a query parameter
```

#### Forms

```text
Create a feedback form with GET handler (show form) and POST handler (process submission)

Create a file upload handler that stores images
```

#### Real-time Features

```text
Create a real-time chat system using Server-Sent Events

Create a live notification feed using streaming

Create a dashboard that updates every 5 seconds
```

## Prompt Examples by Use Case

### 1. Simple Web Page

**Prompt:**

```text
Create a script that serves an "About Us" page with company information,
team members, and contact details
```

**Expected Result:**

- Handler function returning HTML
- Route registered at `/about`
- Properly formatted HTML with CSS

### 2. REST API

**Prompt:**

```text
Create a REST API for a product catalog with:
- GET /api/products - list all products
- GET /api/products?id=X - get single product
- POST /api/products - create new product
- PUT /api/products - update product
- DELETE /api/products - delete product
Include input validation and error handling
```

**Expected Result:**

- Multiple handler functions
- All CRUD operations
- Validation logic
- Error responses

### 3. Form Processing

**Prompt:**

```text
Create a newsletter signup form that:
- Shows form on GET /signup
- Processes submission on POST /signup
- Validates email format
- Stores email in a list
- Shows confirmation message
```

**Expected Result:**

- GET handler with form HTML
- POST handler with validation
- Data storage
- Success/error responses

### 4. External API Integration

**Prompt:**

```text
Create a weather API that:
- Accepts city name as query parameter
- Fetches data from OpenWeather API using the fetch() function
- Formats and returns weather data as JSON
- Handles API errors gracefully
```

**Expected Result:**

- Handler with `fetch()` call
- API key handling (using secrets)
- Error handling
- Data transformation

### 5. Authentication

**Prompt:**

```text
Create a login system that:
- Shows login form on GET /login
- Processes credentials on POST /login
- Uses the built-in authentication functions
- Returns session token on success
- Redirects to dashboard after login
```

**Expected Result:**

- GET and POST handlers
- Auth API usage
- Session management
- Redirect logic

## AI Development Workflows

### Workflow 1: Rapid Prototyping

1. **Describe feature** to AI

   ```text
   Create a blog homepage that lists recent posts with titles,
   excerpts, and publish dates
   ```

2. **Review generated code**
   - Check the diff preview
   - Verify it matches requirements

3. **Apply and test**
   - Click "Apply Changes"
   - Test the endpoint immediately

4. **Iterate with AI**

   ```text
   Add pagination to the blog list

   Add a search feature

   Style the blog with modern CSS
   ```

### Workflow 2: Incremental Enhancement

1. **Start with working code**
   - Have a basic script running

2. **Ask AI for specific improvements**

   ```text
   Add error handling to all database operations

   Add logging to track API usage

   Add input validation for all form fields
   ```

3. **Review changes**
   - See side-by-side diff
   - Verify only intended changes

4. **Apply iteratively**
   - Accept changes one at a time
   - Test after each change

### Workflow 3: Learning and Exploration

1. **Ask AI to explain concepts**

   ```text
   How do I implement real-time updates using streams?

   Show me an example of file upload handling

   How do I call external APIs securely?
   ```

2. **Request examples**

   ```text
   Create a simple example of Server-Sent Events

   Show me how to use the fetch() function with API keys
   ```

3. **Build on examples**

   ```text
   Expand this SSE example to broadcast chat messages

   Add error handling to this fetch example
   ```

### Workflow 4: Debugging and Fixing

1. **Identify the issue**
   - Script returns errors
   - Unexpected behavior
   - Check logs for error messages

2. **Ask AI for help**

   ```text
   This script is throwing "undefined is not a function" errors.
   Here's the error from logs: [paste error]

   This form handler isn't receiving POST data correctly

   The streaming endpoint disconnects after 30 seconds
   ```

3. **Review proposed fixes**
   - AI shows what changes would fix the issue

4. **Apply and verify**
   - Test the fix
   - Check logs to confirm resolution

## AI Prompt Templates

### Creation Templates

```text
Create a [type] that [functionality]

Create a script for [feature] with [specific requirements]

Build a [component] that handles [use case]
```

### Enhancement Templates

```text
Add [feature] to this script

Improve [aspect] in this code

Refactor [section] to use [pattern]

Optimize [operation] for better performance
```

### Debugging Templates

```text
Fix the [error type] error in this script

Debug why [expected behavior] isn't working

Add error handling for [scenario]

Improve error messages in this code
```

### Learning Templates

```text
Explain how [concept] works in aiwebengine

Show me an example of [feature]

What's the best way to [task]?

How do I implement [functionality]?
```

## Best Practices with AI

### 1. Iterate, Don't Expect Perfection

Start with a basic request, then refine:

```text
Step 1: "Create a user registration API"
Step 2: "Add email validation"
Step 3: "Add duplicate email checking"
Step 4: "Add password strength requirements"
Step 5: "Add rate limiting for registration attempts"
```

### 2. Review All AI-Generated Code

- **Always review** before applying
- **Test thoroughly** after applying
- **Understand** what the code does
- **Customize** to your specific needs

### 3. Be Specific About Technology

```text
Use the aiwebengine fetch() function to call the API

Use Server-Sent Events (routeRegistry.registerStreamRoute) for real-time updates

Use the built-in console.log() for logging

Reference the /style.css asset for styling
```

### 4. Provide Examples When Possible

```text
Create a form handler similar to the feedback example,
but for product reviews with a 1-5 star rating
```

### 5. Break Complex Requests into Steps

Instead of:

```text
Create a full e-commerce site with products, cart, checkout, and payment
```

Do this:

```text
Step 1: Create a product listing API
Step 2: Add product detail pages
Step 3: Create shopping cart functionality
Step 4: Add checkout form
Step 5: Integrate payment processing
```

## Using AI Outside the Editor

### ChatGPT / Claude / Other AI Assistants

You can use external AI tools by providing context:

**Context to provide:**

```text
I'm developing scripts for aiwebengine, which is a JavaScript-based
web application engine. Scripts are server-side handlers that:

- Receive a `req` object with method, path, query, form, headers
- Must return an object with status, body, contentType
- Use routeRegistry.registerRoute(path, handlerName, method) to map routes
- Can use console.log(message) for logging
- Can use fetch(url, options) for external API calls
- Can use routeRegistry.registerStreamRoute(path) and routeRegistry.sendStreamMessage(data) for SSE

Available functions:
- routeRegistry.registerRoute(), console.log(), listLogs(), listLogsForUri()
- fetch(), routeRegistry.registerStreamRoute(), routeRegistry.sendStreamMessage()
- assetStorage.listAssets(), assetStorage.fetchAsset(), assetStorage.upsertAsset(), assetStorage.deleteAsset()

Can you help me create a [your request]?
```

### GitHub Copilot / VS Code AI Tools

If developing locally with VS Code:

1. Create a `README.md` in your scripts directory explaining the aiwebengine APIs
2. Use Copilot with inline comments:
   ```javascript
   // Create a handler that returns a list of users as JSON
   function usersHandler(req) {
     // Copilot will suggest implementation
   }
   ```

### API-Specific AI Prompts

For external API integrations:

```text
Create an aiwebengine script that calls the Stripe API to process payments.
Use the fetch() function with {{secret:stripe_key}} for the API key.
Return JSON responses.
```

## Troubleshooting AI Assistance

### AI Generates Client-Side Code

**Problem:** AI creates browser JavaScript instead of server handlers

**Solution:** Be explicit:

```text
Create a SERVER-SIDE aiwebengine script that RETURNS HTML,
not client-side JavaScript
```

### AI Suggests Unsupported Features

**Problem:** AI uses Node.js/browser APIs not available in QuickJS

**Solution:** Specify:

```text
Use only QuickJS-compatible code. Available functions are:
routeRegistry.registerRoute(), console.log(), fetch(), JSON.parse(), JSON.stringify()
```

### AI Generates Too Much Code

**Problem:** Response is overwhelming or too complex

**Solution:** Ask for simpler version:

```text
Create a minimal working example of [feature]

Simplify this to just the essential functionality

Break this into smaller, focused functions
```

### AI Doesn't Understand Context

**Problem:** AI doesn't know about your existing code

**Solution:** Provide context:

```text
I have a script that manages users. Add a new endpoint to
delete users. Here's my current code: [paste code]
```

## Next Steps

- **[Getting Started](../getting-started/01-first-script.md)** - Create your first script
- **[Working with Editor](../getting-started/02-working-with-editor.md)** - Master the editor
- **[Script Development](scripts.md)** - Deep dive into scripting
- **[Examples](../examples/index.md)** - See AI-generated patterns
- **[API Reference](../reference/javascript-apis.md)** - Complete API documentation

## Quick Reference

### Good AI Prompts

```text
✅ Create a script that serves a blog homepage with post listings

✅ Create a REST API for managing inventory items with CRUD operations

✅ Add authentication to this API using the built-in auth functions

✅ Refactor this code to add error handling and logging

✅ Create a real-time chat using Server-Sent Events
```

### Poor AI Prompts

```text
❌ Create a website

❌ Make a form

❌ Add features

❌ Fix this

❌ Make it better
```

### AI Assistant Commands

- **Generate**: "Create a script that..."
- **Edit**: "Add [feature] to this script"
- **Explain**: "Explain how this works"
- **Debug**: "Fix the error in..."
- **Refactor**: "Improve [aspect] of this code"
