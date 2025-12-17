/// <reference path="https://your-engine.com/api/types/v0.1.0/aiwebengine.d.ts" />

// Example blog script demonstrating aiwebengine capabilities
// This script registers a /blog endpoint that serves a sample blog post

/**
 * Blog handler - serves the main blog page
 * @param {HandlerContext} context - Request context
 * @returns {HttpResponse} HTML response
 */
function blog_handler(context) {
  const html = `<!DOCTYPE html>
<html lang="en">
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <title>aiwebengine Blog - Unleashing the Power of Server-Side JavaScript</title>
    <link rel="stylesheet" href="/engine.css">
    <link rel="icon" type="image/x-icon" href="/favicon.ico">
    <style>
        /* Blog page specific overrides */
        body {
            background: linear-gradient(135deg, #667eea 0%, #764ba2 100%);
            min-height: 100vh;
            padding: 2rem 0;
        }

        .blog-container {
            max-width: 900px;
            margin: 0 auto;
            background: rgba(255, 255, 255, 0.95);
            backdrop-filter: blur(10px);
            border-radius: var(--border-radius-lg);
            box-shadow: var(--shadow-lg);
            overflow: hidden;
        }

        .blog-content {
            padding: 3rem 2rem;
        }

        .blog-header {
            text-align: center;
            margin-bottom: 3rem;
        }

        .blog-header h1 {
            color: var(--text-color);
            margin-bottom: 0.5rem;
            font-size: 2.5rem;
        }

        .blog-subtitle {
            color: var(--text-muted);
            font-style: italic;
            font-size: 1.1rem;
        }

        .feature-grid {
            display: grid;
            grid-template-columns: repeat(auto-fit, minmax(280px, 1fr));
            gap: 1.5rem;
            margin: 3rem 0;
        }

        .feature-card {
            background: var(--bg-secondary);
            padding: 1.5rem;
            border-radius: var(--border-radius);
            border-left: 4px solid var(--primary-color);
            transition: var(--transition);
        }

        .feature-card:hover {
            transform: translateY(-2px);
            box-shadow: var(--shadow);
        }

        .feature-card h3 {
            color: var(--text-color);
            margin-top: 0;
            margin-bottom: 1rem;
        }

        .feature-card p {
            color: var(--text-muted);
            margin: 0;
        }

        .code-example {
            background: var(--code-bg);
            color: var(--code-color);
            padding: 1rem;
            border-radius: var(--border-radius);
            font-family: var(--font-mono);
            margin: 1.5rem 0;
            overflow-x: auto;
            border: 1px solid var(--border-color);
        }

        .blog-cta {
            text-align: center;
            margin-top: 3rem;
            padding: 2rem;
            background: linear-gradient(135deg, #667eea 0%, #764ba2 100%);
            color: white;
            border-radius: var(--border-radius);
        }

        .blog-cta h3 {
            color: white;
            margin-bottom: 1rem;
        }

        .blog-cta p {
            margin-bottom: 0;
        }

        .blog-cta a {
            color: white;
            text-decoration: underline;
            font-weight: 500;
        }

        .blog-cta a:hover {
            text-decoration: none;
        }

        @media (max-width: 768px) {
            .blog-content {
                padding: 2rem 1rem;
            }

            .blog-header h1 {
                font-size: 2rem;
            }

            .feature-grid {
                grid-template-columns: 1fr;
                gap: 1rem;
            }
        }
    </style>
</head>
<body>
    <div class="blog-container">
        <div class="blog-content">
            <div class="blog-header">
                <h1>🚀 Unleashing the Power of Server-Side JavaScript</h1>
                <p class="blog-subtitle">How aiwebengine revolutionizes web development with embedded JavaScript execution</p>
            </div>

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
function hello_handler(context) {
    return ResponseBuilder.text("Hello, aiwebengine!");
}

routeRegistry.registerRoute('/hello', 'hello_handler', 'GET');
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

            <div class="blog-cta">
                <h3>Ready to revolutionize your web development?</h3>
                <p>Explore the <a href="/editor">built-in editor</a> to start creating your own JavaScript-powered endpoints, or check out the <a href="/feedback">feedback form</a> to share your thoughts!</p>
            </div>
        </div>
    </div>
</body>
</html>`;

  return ResponseBuilder.html(html);
}

// Initialization function - called when script is loaded or updated
function init(context) {
  try {
    console.log(`Initializing blog.js script at ${new Date().toISOString()}`);
    console.log(`Init context: ${JSON.stringify(context)}`);

    // Register the blog endpoint
    routeRegistry.registerRoute("/blog", "blog_handler", "GET");

    console.log("Blog script initialized successfully");

    return {
      success: true,
      message: "Blog script initialized successfully",
      registeredEndpoints: 1,
    };
  } catch (error) {
    console.error(`Blog script initialization failed: ${error.message}`);
    throw error;
  }
}
