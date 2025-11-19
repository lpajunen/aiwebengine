# Conversion API Reference

The `convert` object provides functions for converting content between different formats. Currently supports Markdown to HTML conversion.

## convert.markdown_to_html()

Converts a markdown string to HTML using the same high-quality parser used by the aiwebengine documentation system.

### Syntax

```javascript
const html = convert.markdown_to_html(markdown);
```

### Parameters

- **markdown** (string): The markdown content to convert
  - Maximum size: 1MB (1,000,000 bytes)
  - Cannot be empty
  - Supports standard CommonMark syntax plus extensions

### Return Value

Returns a string containing:

- **HTML output** if conversion succeeds
- **Error message** if conversion fails (starts with "Error:")

### Supported Markdown Features

The converter supports GitHub-Flavored Markdown including:

- **Headings** (`#`, `##`, `###`, etc.)
- **Bold** (`**text**` or `__text__`)
- **Italic** (`*text*` or `_text_`)
- **Strikethrough** (`~~text~~`)
- **Code blocks** (```)
- **Inline code** (`` `code` ``)
- **Links** (`[text](url)`)
- **Images** (`![alt](url)`)
- **Lists** (ordered and unordered)
- **Tables** (with header rows and alignment)
- **Blockquotes** (`>`)
- **Task lists** (`- [ ]` and `- [x]`)
- **Footnotes**
- **Heading attributes**

### Error Handling

The function returns an error message (starting with "Error:") if:

- Markdown input is empty
- Markdown input exceeds 1MB size limit
- An unexpected parsing error occurs

### Examples

#### Basic Conversion

```javascript
function renderMarkdownPage(context) {
  const req = context.request;

  const markdown = `# Welcome

This is a **simple** example with *italic* text.

## Features

- Easy to use
- Fast rendering
- GitHub-flavored markdown
`;

  const html = convert.markdown_to_html(markdown);

  if (html.startsWith("Error:")) {
    return {
      status: 500,
      body: html,
      contentType: "text/plain; charset=UTF-8",
    };
  }

  // Wrap in HTML page template
  const fullPage = `<!DOCTYPE html>
<html>
<head>
  <title>Markdown Page</title>
  <link rel="stylesheet" href="/engine.css">
</head>
<body>
  <div class="container">
    ${html}
  </div>
</body>
</html>`;

  return {
    status: 200,
    body: fullPage,
    contentType: "text/html; charset=UTF-8",
  };
}
```

#### Blog Post from Storage

```javascript
function serveBlogPost(context) {
  const req = context.request;

  // Extract slug from path like /blog/my-post
  const slug = req.path.split("/").pop();

  // Load markdown from shared storage
  const markdown = sharedStorage.getItem(`blog:${slug}`);

  if (!markdown) {
    return {
      status: 404,
      body: "Blog post not found",
      contentType: "text/plain; charset=UTF-8",
    };
  }

  // Convert markdown to HTML
  const content = convert.markdown_to_html(markdown);

  if (content.startsWith("Error:")) {
    console.error(`Failed to convert blog post ${slug}: ${content}`);
    return {
      status: 500,
      body: "Failed to render blog post",
      contentType: "text/plain; charset=UTF-8",
    };
  }

  // Create styled blog page
  const html = `<!DOCTYPE html>
<html>
<head>
  <title>Blog - ${slug}</title>
  <link rel="stylesheet" href="/engine.css">
  <style>
    .blog-post {
      max-width: 800px;
      margin: 2rem auto;
      padding: 2rem;
      background: white;
      border-radius: 8px;
      box-shadow: 0 2px 4px rgba(0,0,0,0.1);
    }
    .blog-post h1 { color: #333; }
    .blog-post code {
      background: #f4f4f4;
      padding: 2px 6px;
      border-radius: 3px;
    }
    .blog-post pre {
      background: #f4f4f4;
      padding: 1rem;
      border-radius: 4px;
      overflow-x: auto;
    }
  </style>
</head>
<body>
  <div class="blog-post">
    ${content}
  </div>
</body>
</html>`;

  return {
    status: 200,
    body: html,
    contentType: "text/html; charset=UTF-8",
  };
}
```

#### Code Documentation with Syntax Highlighting

```javascript
function serveApiDocs(context) {
  const markdown = `# API Documentation

## Authentication

All API requests require a bearer token:

\`\`\`bash
curl -H "Authorization: Bearer YOUR_TOKEN" https://api.example.com/data
\`\`\`

## Endpoints

### GET /api/users

Returns a list of users.

**Response:**

\`\`\`json
{
  "users": [
    {"id": 1, "name": "Alice"},
    {"id": 2, "name": "Bob"}
  ]
}
\`\`\`

### POST /api/users

Creates a new user.

**Request Body:**

| Field | Type   | Required | Description |
|-------|--------|----------|-------------|
| name  | string | Yes      | User's name |
| email | string | Yes      | User's email|

`;

  const html = convert.markdown_to_html(markdown);

  const page = `<!DOCTYPE html>
<html>
<head>
  <title>API Documentation</title>
  <link rel="stylesheet" href="/engine.css">
</head>
<body>
  <nav>
    <a href="/">Home</a>
    <a href="/docs">Docs</a>
  </nav>
  <main>
    ${html}
  </main>
</body>
</html>`;

  return {
    status: 200,
    body: page,
    contentType: "text/html; charset=UTF-8",
  };
}
```

#### User-Generated Content (Safe Rendering)

```javascript
function renderUserComment(context) {
  const req = context.request;

  // Get user-submitted markdown from form data
  const userMarkdown = req.form.comment || "";

  // Validate input size
  if (userMarkdown.length > 10000) {
    // 10KB limit for user comments
    return {
      status: 400,
      body: "Comment too long (max 10KB)",
      contentType: "text/plain; charset=UTF-8",
    };
  }

  // Convert markdown to HTML
  const commentHtml = convert.markdown_to_html(userMarkdown);

  if (commentHtml.startsWith("Error:")) {
    return {
      status: 400,
      body: "Invalid markdown: " + commentHtml,
      contentType: "text/plain; charset=UTF-8",
    };
  }

  // Note: The HTML output from markdown conversion is NOT sanitized for XSS
  // For user-generated content, consider:
  // 1. Limiting allowed markdown features
  // 2. Post-processing the HTML to remove dangerous tags
  // 3. Using Content-Security-Policy headers

  return {
    status: 200,
    body: JSON.stringify({ html: commentHtml }),
    contentType: "application/json",
  };
}
```

## Performance Considerations

### Caching Converted HTML

Since markdown parsing is CPU-intensive, consider caching converted HTML for static content:

```javascript
function serveCachedDocs(context) {
  const req = context.request;
  const docId = req.query.id || "index";

  // Check cache first
  const cacheKey = `html:${docId}`;
  let html = sharedStorage.getItem(cacheKey);

  if (!html) {
    // Cache miss - load and convert markdown
    const markdown = sharedStorage.getItem(`markdown:${docId}`);

    if (!markdown) {
      return {
        status: 404,
        body: "Document not found",
        contentType: "text/plain; charset=UTF-8",
      };
    }

    html = convert.markdown_to_html(markdown);

    if (!html.startsWith("Error:")) {
      // Cache the converted HTML
      sharedStorage.setItem(cacheKey, html);
      console.info(`Cached HTML for document ${docId}`);
    }
  }

  return {
    status: 200,
    body: wrapInTemplate(html),
    contentType: "text/html; charset=UTF-8",
  };
}

function wrapInTemplate(content) {
  return `<!DOCTYPE html>
<html>
<head>
  <title>Documentation</title>
  <link rel="stylesheet" href="/engine.css">
</head>
<body>${content}</body>
</html>`;
}
```

### Invalidating Cache

```javascript
function updateDocument(context) {
  const req = context.request;
  const docId = req.form.id;
  const markdown = req.form.content;

  // Store new markdown
  sharedStorage.setItem(`markdown:${docId}`, markdown);

  // Invalidate HTML cache
  sharedStorage.removeItem(`html:${docId}`);

  return {
    status: 200,
    body: "Document updated",
    contentType: "text/plain; charset=UTF-8",
  };
}
```

## Security Notes

### XSS Prevention

The markdown converter generates HTML from markdown syntax. While markdown is generally safer than raw HTML, it can still produce potentially dangerous output:

1. **User Input**: Never trust user-provided markdown without validation
2. **Size Limits**: The converter enforces a 1MB limit, but you should set lower limits for user content
3. **Output Sanitization**: The converter does NOT sanitize HTML output. For user-generated content, consider additional sanitization
4. **CSP Headers**: Use Content-Security-Policy headers to mitigate XSS risks

### Safe Usage Pattern

```javascript
function safeUserContent(context) {
  const req = context.request;
  const userMarkdown = req.form.content || "";

  // Validate input
  if (userMarkdown.length > 5000) {
    return { status: 400, body: "Content too long" };
  }

  // Convert
  const html = convert.markdown_to_html(userMarkdown);

  if (html.startsWith("Error:")) {
    return { status: 400, body: html };
  }

  // Return with strict CSP
  return {
    status: 200,
    body: html,
    contentType: "text/html; charset=UTF-8",
    headers: {
      "Content-Security-Policy":
        "default-src 'none'; style-src 'unsafe-inline';",
    },
  };
}
```

## See Also

- [JavaScript APIs Reference](javascript-apis.md) - Complete API reference
- [Shared Storage API](javascript-apis.md#shared-storage) - For caching converted HTML
- [HTTP Response Format](javascript-apis.md#http-response-format) - Response structure
