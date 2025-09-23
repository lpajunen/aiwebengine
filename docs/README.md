# aiwebengine User Documentation

## Overview

**aiwebengine** is a lightweight web application engine built in Rust that enables developers to create dynamic web content using JavaScript scripts. It provides a simple yet powerful platform for building web applications by writing JavaScript handlers that process HTTP requests and generate responses.

The core concept is that you create JavaScript scripts that define how your web application behaves. These scripts contain handler functions that receive HTTP requests and return responses, allowing you to build APIs, serve HTML pages, handle forms, and more—all using familiar JavaScript syntax.

## Key Concepts

- **Scripts**: JavaScript files that contain your application logic
- **Handlers**: Functions that process HTTP requests and return responses
- **Routes**: URL paths mapped to specific handlers
- **Assets**: Static files (images, CSS, etc.) served by the engine

## Quick Start

1. **Install and run the server**:

   ```bash
   git clone https://github.com/lpajunen/aiwebengine.git
   cd aiwebengine
   cargo build --release
   cargo run
   ```

2. **Create a simple script** (e.g., `scripts/hello.js`):

   ```javascript
   function helloHandler(req) {
       return {
           status: 200,
           body: `Hello, ${req.query.name || 'World'}!`,
           contentType: "text/plain"
       };
   }

   register('/hello', 'helloHandler', 'GET');
   ```

3. **Access your endpoint**: Visit `http://localhost:3000/hello?name=User`

## Documentation Sections

- **[Local Development](local-development.md)**: Setting up your development environment and using the deployer tool
- **[Remote Development](remote-development.md)**: Using the built-in web editor for script management
- **[JavaScript APIs](javascript-apis.md)**: Complete reference for available JavaScript functions and objects
- **[Examples](examples.md)**: Sample scripts demonstrating common patterns and use cases

## Getting Help

- Check the [examples](examples.md) for common patterns
- Review the [JavaScript APIs](javascript-apis.md) for available functions
- See [local development](local-development.md) for development workflows
- Use the [remote editor](remote-development.md) for quick prototyping

## Project Status

⚠️ **Work in Progress**: aiwebengine is actively developed. Core features are stable, but some advanced features are still evolving.
