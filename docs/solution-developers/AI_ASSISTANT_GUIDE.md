# AI Assistant Script Management Guide

## Overview

The aiwebengine editor now features an advanced AI assistant that can help you create, edit, and manage scripts with intelligent suggestions and a built-in diff preview system.

## Features

### 1. **Context-Aware AI Assistance**
The AI assistant understands:
- All available aiwebengine JavaScript APIs
- Your current script and its content
- All scripts in your workspace
- Best practices for script development

### 2. **Structured Response Types**
The AI can perform four types of actions:

#### Explanation
Ask the AI to explain scripts, APIs, or concepts.

**Example Prompts:**
- "Explain what this script does"
- "How does the fetch API work?"
- "What's the purpose of the init function?"

#### Create Script
The AI can generate complete, working scripts from scratch.

**Example Prompts:**
- "Create a hello world API"
- "Create a REST API for managing blog posts"
- "Create a script that fetches weather data from an external API"

**Features:**
- Complete script with proper structure
- Error handling included
- init() function with route registration
- Preview before creating

#### Edit Script
The AI can modify existing scripts with surgical precision.

**Example Prompts:**
- "Add error handling to the current script"
- "Add logging to all functions"
- "Refactor this to use async/await"
- "Add input validation"

**Features:**
- Side-by-side diff preview
- See exactly what changes before applying
- Original and modified code comparison

#### Delete Script
The AI can suggest script deletions with explanations.

**Example Prompts:**
- "Should I delete this script?"
- "This script is obsolete, can you delete it?"

**Features:**
- Confirmation dialog with explanation
- Safe deletion process

## How to Use

### Basic Workflow

1. **Open the Editor**
   - Navigate to `/editor` in your browser

2. **Select a Script (Optional)**
   - Click any script in the sidebar to provide context to the AI

3. **Ask the AI**
   - Type your request in the AI Assistant panel at the bottom
   - The AI has full context of your current script

4. **Review Suggestions**
   - For create/edit actions, click "Preview & Create" or "Preview Changes"
   - A diff modal will show exactly what will change

5. **Apply or Reject**
   - Review the changes in the side-by-side diff viewer
   - Click "Apply Changes" to accept
   - Click "Reject" to cancel

### Example Use Cases

#### Creating a New API

**Prompt:**
```
Create a REST API for a todo list with endpoints to create, list, and delete todos
```

**Result:**
- AI generates complete script with all CRUD operations
- Includes proper error handling
- Uses JSON responses
- Has init() function with route registration
- You can preview the code before creating

#### Improving an Existing Script

**Prompt:**
```
Add try-catch error handling to all functions and log errors
```

**Result:**
- AI analyzes your current script
- Adds error handling where needed
- Shows diff with original vs modified code
- You can see exactly what changes before applying

#### Understanding Code

**Prompt:**
```
Explain what this script does and how it works
```

**Result:**
- AI provides detailed explanation
- Describes the purpose and functionality
- Explains each major component

## System Prompt & API Knowledge

The AI assistant is pre-configured with complete knowledge of:

### Available APIs
- `register(path, handlerName, method)` - Register HTTP routes
- `writeLog(message)` - Server logging
- `fetch(url, options)` - HTTP requests with secret injection
- `registerWebStream(path)` - SSE stream endpoints
- `sendStreamMessage(data)` - Broadcast to streams
- `getSecret(key)` - Retrieve secrets
- `listScripts()` - List all scripts
- `getScript(uri)` - Get script content

### Response Format
The AI always responds in structured JSON:
```json
{
  "type": "explanation|create_script|edit_script|delete_script",
  "message": "Human-readable explanation",
  "script_name": "name.js",
  "code": "complete script code",
  "original_code": "original code (for edits)"
}
```

### Script Structure
The AI knows the required pattern:
```javascript
// Script description

function handlerName(req) {
  try {
    // Logic here
    return {
      status: 200,
      body: 'response',
      contentType: 'text/plain'
    };
  } catch (error) {
    writeLog('Error: ' + error);
    return { status: 500, body: 'Internal error' };
  }
}

function init(context) {
  writeLog('Initializing script');
  register('/path', 'handlerName', 'GET');
  return { success: true };
}
```

## Advanced Features

### Context Injection

The AI automatically receives:
```javascript
{
  prompt: "your request",
  currentScript: "blog.js",
  currentScriptContent: "full script code..."
}
```

This allows it to:
- Understand what script you're working on
- Make specific suggestions based on your code
- Provide relevant examples

### Diff Preview

The Monaco diff editor provides:
- **Side-by-side comparison**
- **Syntax highlighting**
- **Line-by-line changes**
- **Color-coded additions/deletions**

### Safety Features

1. **Preview Before Apply** - Never applies changes without your confirmation
2. **Validation** - AI response validation and error handling
3. **Rollback** - Can reject changes at any time
4. **Confirmation** - Delete actions require explicit confirmation

## Tips for Best Results

### Be Specific
❌ "Make it better"
✅ "Add error handling and input validation to the POST endpoint"

### Provide Context
❌ "Fix the bug"
✅ "The fetch request in handleAPI is failing, add timeout and retry logic"

### Use the Current Script Context
✅ Open the script you want to modify
✅ Then ask the AI to make changes
✅ The AI will see your code and make targeted suggestions

### Iterate
- Start with broad requests
- Review the suggestion
- Ask for refinements
- Apply when satisfied

## Keyboard Shortcuts

- **Enter** - Submit prompt (Shift+Enter for new line)
- **Escape** - Close diff modal (when focused)

## Technical Architecture

### Backend Enhancement
- System prompt with full API documentation
- Context injection (current script, all scripts)
- Structured JSON response parsing
- Enhanced error handling

### Frontend Features
- Monaco diff editor integration
- Modal-based preview system
- Action button rendering
- Real-time diff visualization

### CSS Styling
- Dark theme modal
- Responsive diff viewer
- Action type badges
- Smooth transitions

## Troubleshooting

### AI Not Responding
- Check that `SECRET_ANTHROPIC_API_KEY` is configured
- Check server logs for API errors

### Changes Not Applying
- Ensure you clicked "Apply Changes" in the diff modal
- Check browser console for errors
- Verify the script name is valid

### Diff Not Showing
- Ensure Monaco editor loaded successfully
- Check for JavaScript errors in console
- Try refreshing the page

## Configuration

The AI uses Claude 3 Haiku by default:
```javascript
model: "claude-3-haiku-20240307",
max_tokens: 4096
```

You can modify this in `scripts/feature_scripts/editor.js` if needed.

## Examples

### Example 1: Create Authentication API

**Prompt:**
```
Create a script that handles user authentication with login and logout endpoints
```

**AI Creates:**
```javascript
// auth.js - User authentication script

function handleLogin(req) {
  try {
    const { username, password } = req.form;
    
    if (!username || !password) {
      return {
        status: 400,
        body: JSON.stringify({ error: 'Missing credentials' }),
        contentType: 'application/json'
      };
    }
    
    // Authentication logic here
    writeLog('Login attempt for: ' + username);
    
    return {
      status: 200,
      body: JSON.stringify({ success: true, token: 'abc123' }),
      contentType: 'application/json'
    };
  } catch (error) {
    writeLog('Login error: ' + error);
    return { status: 500, body: 'Internal error' };
  }
}

function init(context) {
  writeLog('Initializing auth script');
  register('/api/login', 'handleLogin', 'POST');
  return { success: true };
}
```

### Example 2: Add Rate Limiting

**Prompt:**
```
Add rate limiting to the current API endpoint
```

**AI Modifies:**
Shows diff with rate limiting logic added, tracking request counts, and returning 429 when limit exceeded.

### Example 3: Explain Complex Code

**Prompt:**
```
Explain how the WebSocket streaming works in this script
```

**AI Explains:**
Provides detailed explanation of the streaming mechanism, how clients connect, how messages are broadcasted, and best practices.

## Summary

The AI assistant transforms script development in aiwebengine by:
- ✅ Understanding your platform's specific APIs
- ✅ Generating working, production-ready code
- ✅ Providing safe, reviewable changes
- ✅ Offering intelligent explanations
- ✅ Accelerating development workflow

Start using it today to supercharge your aiwebengine development!
