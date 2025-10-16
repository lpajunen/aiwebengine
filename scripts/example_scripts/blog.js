// Example blog script demonstrating aiwebengine capabilities
// This script registers a /blog endpoint that serves a sample blog post

function blog_handler(req) {
  const html = `<!DOCTYPE html>
<html lang="en">
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <title>aiwebengine Blog - Unleashing the Power of Server-Side JavaScript</title>
    <style>
        body {
            font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, sans-serif;
            line-height: 1.6;
            max-width: 800px;
            margin: 0 auto;
            padding: 20px;
            background: linear-gradient(135deg, #667eea 0%, #764ba2 100%);
            min-height: 100vh;
        }
        .blog-container {
            background: white;
            border-radius: 10px;
            padding: 40px;
            box-shadow: 0 10px 30px rgba(0,0,0,0.1);
        }
        h1 {
            color: #2c3e50;
            text-align: center;
            margin-bottom: 10px;
            font-size: 2.5em;
        }
        .subtitle {
            text-align: center;
            color: #7f8c8d;
            margin-bottom: 30px;
            font-style: italic;
        }
        .feature-grid {
            display: grid;
            grid-template-columns: repeat(auto-fit, minmax(250px, 1fr));
            gap: 20px;
            margin: 30px 0;
        }
        .feature-card {
            background: #f8f9fa;
            padding: 20px;
            border-radius: 8px;
            border-left: 4px solid #3498db;
        }
        .feature-card h3 {
            color: #2c3e50;
            margin-top: 0;
        }
        .code-example {
            background: #2c3e50;
            color: #ecf0f1;
            padding: 15px;
            border-radius: 5px;
            font-family: 'Monaco', 'Menlo', monospace;
            margin: 15px 0;
            overflow-x: auto;
        }
        .cta {
            text-align: center;
            margin-top: 40px;
            padding: 20px;
            background: linear-gradient(135deg, #667eea 0%, #764ba2 100%);
            color: white;
            border-radius: 8px;
        }
        .cta a {
            color: white;
            text-decoration: none;
            font-weight: bold;
        }
    </style>
</head>
<body>
    <div class="blog-container">
        <h1>🚀 Unleashing the Power of Server-Side JavaScript</h1>
        <p class="subtitle">How aiwebengine revolutionizes web development with embedded JavaScript execution</p>

        <p>Welcome to the future of web development! <strong>aiwebengine</strong> is a groundbreaking Rust-based web server that embeds the QuickJS JavaScript engine, allowing you to write server-side logic entirely in JavaScript. This innovative approach combines the performance and safety of Rust with the flexibility and familiarity of JavaScript.</p>

        <div class="feature-grid">
            <div class="feature-card">
                <h3>⚡ Lightning Fast</h3>
                <p>Built on Rust with native performance, aiwebengine serves requests at blazing speeds while maintaining the developer-friendly JavaScript API.</p>
            </div>
            <div class="feature-card">
                <h3>🔒 Memory Safe</h3>
                <p>Rust's ownership system prevents memory leaks and buffer overflows, providing enterprise-grade security for your JavaScript applications.</p>
            </div>
            <div class="feature-card">
                <h3>📦 Hot Reload</h3>
                <p>Upload and update JavaScript code via HTTP API without restarting the server. Perfect for rapid development and deployment.</p>
            </div>
            <div class="feature-card">
                <h3>🔧 Full Control</h3>
                <p>Access to HTTP methods, query parameters, form data, and custom routing - all from your JavaScript code.</p>
            </div>
        </div>

        <h2>Getting Started</h2>
        <p>Creating a new endpoint is as simple as writing a JavaScript function and registering it:</p>

        <div class="code-example">
// Register a simple hello world endpoint
function hello_handler(req) {
    return {
        status: 200,
        body: "Hello, aiwebengine!",
        contentType: "text/plain"
    };
}

register('/hello', 'hello_handler', 'GET');
        </div>

        <h2>Advanced Features</h2>
        <p>aiwebengine supports complex server-side applications with features like:</p>
        <ul>
            <li><strong>GraphQL Integration:</strong> Built-in GraphQL support for modern API development</li>
            <li><strong>Form Handling:</strong> Automatic parsing of form data and file uploads</li>
            <li><strong>Logging:</strong> Comprehensive logging system for debugging and monitoring</li>
            <li><strong>Asset Management:</strong> Serve static files and manage web assets</li>
            <li><strong>Real-time Updates:</strong> WebSocket support for live applications</li>
        </ul>

        <div class="cta">
            <h3>Ready to revolutionize your web development?</h3>
            <p>Explore the <a href="/editor">built-in editor</a> to start creating your own JavaScript-powered endpoints, or check out the <a href="/feedback">feedback form</a> to share your thoughts!</p>
        </div>
    </div>
</body>
</html>`;

  return {
    status: 200,
    body: html,
    contentType: "text/html",
  };
}

// Register the blog endpoint
register("/blog", "blog_handler", "GET");
