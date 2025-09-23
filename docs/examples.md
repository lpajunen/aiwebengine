# Examples

This page showcases practical examples of aiwebengine scripts that demonstrate common patterns and use cases. These examples show how to build web applications using JavaScript handlers, from simple APIs to interactive forms.

## Getting Started with Examples

### Running Examples Locally

1. **Start the server**:

   ```bash
   cargo run
   ```

2. **Upload scripts via the editor**:
   - Open `http://localhost:3000/editor`
   - Create a new script
   - Copy-paste example code
   - Save and test

3. **Or use the deployer tool**:
   ```bash
   cargo run --bin deployer \
     --uri "https://example.com/blog" \
     --file "scripts/example_scripts/blog.js"
   ```

## Available Examples

### Blog Script (`blog.js`)

**Endpoint**: `/blog`  
**Method**: GET  
**Description**: A sample blog post showcasing aiwebengine capabilities with modern HTML/CSS styling.

**Features**:
- HTML templating with embedded CSS
- Feature showcase with code examples
- Responsive design
- Clean, modern UI

**Try it**: Visit `http://localhost:3000/blog` after uploading.

**Code Highlights**:
```javascript
function blogHandler(req) {
    const html = `
    <!DOCTYPE html>
    <html>
    <head>
        <title>aiwebengine Blog</title>
        <style>
        body { font-family: Arial, sans-serif; max-width: 800px; margin: 0 auto; padding: 20px; }
        .header { background: linear-gradient(135deg, #667eea 0%, #764ba2 100%); color: white; padding: 40px; border-radius: 10px; margin-bottom: 30px; }
        .feature { background: #f8f9fa; padding: 20px; margin: 20px 0; border-left: 4px solid #667eea; }
        </style>
    </head>
    <body>
        <div class="header">
            <h1>🚀 aiwebengine</h1>
            <p>Lightweight web application engine</p>
        </div>
        <!-- More content -->
    </body>
    </html>`;
    
    return {
        status: 200,
        body: html,
        contentType: "text/html"
    };
}

register('/blog', 'blogHandler', 'GET');
```

### Feedback Form (`feedback.js`)

**Endpoints**: `/feedback` (GET), `/feedback` (POST)  
**Description**: A complete feedback form with rating system and submission handling.

**Features**:
- GET handler for displaying the form
- POST handler for processing submissions
- Form validation and data processing
- Success confirmation page
- Logging of submitted data

**Try it**:
- Visit `http://localhost:3000/feedback` to see the form
- Submit feedback to test POST handling

**Code Highlights**:
```javascript
function feedbackFormHandler(req) {
    const form = `
    <form method="POST" action="/feedback">
        <label>Name: <input type="text" name="name" required></label><br>
        <label>Email: <input type="email" name="email" required></label><br>
        <label>Rating: 
            <select name="rating">
                <option value="5">⭐⭐⭐⭐⭐ Excellent</option>
                <option value="4">⭐⭐⭐⭐ Very Good</option>
                <option value="3">⭐⭐⭐ Average</option>
                <option value="2">⭐ Poor</option>
                <option value="1">⭐ Very Poor</option>
            </select>
        </label><br>
        <label>Comments: <textarea name="comments"></textarea></label><br>
        <button type="submit">Submit Feedback</button>
    </form>`;
    
    return {
        status: 200,
        body: form,
        contentType: "text/html"
    };
}

function feedbackSubmitHandler(req) {
    // Process form data
    const name = req.form.name;
    const email = req.form.email;
    const rating = req.form.rating;
    const comments = req.form.comments;
    
    writeLog(`Feedback received from ${name} (${email}): ${rating} stars`);
    
    const response = `
    <h1>Thank you for your feedback!</h1>
    <p>Name: ${name}</p>
    <p>Email: ${email}</p>
    <p>Rating: ${rating} stars</p>
    <p>Comments: ${comments}</p>
    <a href="/feedback">Submit another response</a>`;
    
    return {
        status: 200,
        body: response,
        contentType: "text/html"
    };
}

register('/feedback', 'feedbackFormHandler', 'GET');
register('/feedback', 'feedbackSubmitHandler', 'POST');
```

## Script Development Patterns

### Basic API Endpoint

```javascript
function apiHandler(req) {
    const data = {
        message: "Hello from aiwebengine!",
        timestamp: new Date().toISOString(),
        path: req.path,
        query: req.query
    };
    
    return {
        status: 200,
        body: JSON.stringify(data),
        contentType: "application/json"
    };
}

register('/api/hello', 'apiHandler', 'GET');
```

### Dynamic Content with Query Parameters

```javascript
function greetHandler(req) {
    const name = req.query.name || 'World';
    const greeting = `Hello, ${name}!`;
    
    return {
        status: 200,
        body: greeting,
        contentType: "text/plain"
    };
}

register('/greet', 'greetHandler', 'GET');
```

### Form Processing

```javascript
function contactHandler(req) {
    if (req.method === 'GET') {
        return {
            status: 200,
            body: '<form method="POST"><input name="message"><button>Send</button></form>',
            contentType: "text/html"
        };
    } else {
        const message = req.form.message;
        writeLog(`Message received: ${message}`);
        
        return {
            status: 200,
            body: `Message "${message}" received!`,
            contentType: "text/plain"
        };
    }
}

register('/contact', 'contactHandler', 'GET');
register('/contact', 'contactHandler', 'POST');
```

## Best Practices from Examples

1. **Separate concerns**: Use different handlers for different HTTP methods
2. **Validate input**: Check required parameters before processing
3. **Provide feedback**: Use logging to track script execution
4. **Handle errors gracefully**: Return appropriate status codes
5. **Use semantic HTML**: Structure your HTML properly for accessibility
6. **Style consistently**: Include CSS for better user experience

## Creating Your Own Examples

1. **Start simple**: Begin with basic text responses
2. **Add interactivity**: Include forms and dynamic content
3. **Test thoroughly**: Use the editor to test different scenarios
4. **Add logging**: Include `writeLog()` calls for debugging
5. **Document your scripts**: Add comments explaining complex logic

## Next Steps

- Learn the [JavaScript APIs](../javascript-apis.md) for available functions
- Set up [local development](../local-development.md) with the deployer
- Try the [remote editor](../remote-development.md) for quick prototyping
