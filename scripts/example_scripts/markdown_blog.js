// Markdown Blog Example
// Demonstrates using convert.markdown_to_html() to render blog posts

function init(context) {
  // Register routes for the blog
  routeRegistry.registerRoute("/blog", "blogRouter", "GET");
  routeRegistry.registerRoute("/blog/*", "blogRouter", "GET");
  routeRegistry.registerRoute("/blog/admin/create", "createPost", "POST");

  // Store some example blog posts in markdown
  const examplePosts = {
    welcome: `# Welcome to My Blog

This is my **first blog post** using the new markdown conversion feature!

## Features

- Easy to write in markdown
- Automatically converted to HTML
- Supports code blocks
- Tables and more!

### Code Example

\`\`\`javascript
function hello() {
  return "Hello from markdown!";
}
\`\`\`

[Learn more about markdown](https://www.markdownguide.org/)`,

    "markdown-guide": `# Markdown Guide

Learn how to use markdown in your blog posts.

## Basic Formatting

- **Bold text**: Use \`**bold**\` or \`__bold__\`
- *Italic text*: Use \`*italic*\` or \`_italic_\`
- ~~Strikethrough~~: Use \`~~text~~\`

## Lists

### Unordered Lists

- Item 1
- Item 2
  - Nested item
  - Another nested item
- Item 3

### Ordered Lists

1. First item
2. Second item
3. Third item

## Code

Inline code: \`const x = 42;\`

Block code:

\`\`\`javascript
function fibonacci(n) {
  if (n <= 1) return n;
  return fibonacci(n - 1) + fibonacci(n - 2);
}
\`\`\`

## Tables

| Feature | Supported | Notes |
|---------|-----------|-------|
| Headers | ✓ | H1-H6 |
| Lists | ✓ | Ordered and unordered |
| Code | ✓ | Inline and blocks |
| Tables | ✓ | With alignment |

## Links and Images

[Link text](https://example.com)

![Alt text](https://via.placeholder.com/150)

## Blockquotes

> This is a blockquote
> It can span multiple lines`,
  };

  // Store example posts
  for (const slug in examplePosts) {
    sharedStorage.setItem(`blog:${slug}`, examplePosts[slug]);
  }

  console.log("Blog initialized with example posts");
}

function blogRouter(context) {
  const req = context.request;
  
  // Check if this is the blog list page or a specific post
  // Path will be exactly "/blog" for list, or "/blog/something" for a post
  const pathParts = req.path.split("/").filter((p) => p !== "");
  
  // If path is just "/blog", show the list
  if (pathParts.length === 1 && pathParts[0] === "blog") {
    return listPosts(context);
  }
  
  // If path is "/blog/slug", show the specific post
  if (pathParts.length === 2 && pathParts[0] === "blog") {
    return showPost(context, pathParts[1]);
  }
  
  // Unknown path
  return {
    status: 404,
    body: "Not found",
    contentType: "text/plain; charset=UTF-8",
  };
}

function listPosts(context) {
  const req = context.request;

  // In a real blog, you'd list all posts from storage
  // For this example, we'll show the hardcoded ones

  const html = `<!DOCTYPE html>
<html lang="en">
<head>
  <meta charset="UTF-8">
  <meta name="viewport" content="width=device-width, initial-scale=1.0">
  <title>My Blog</title>
  <link rel="stylesheet" href="/engine.css">
  <style>
    .container { max-width: 800px; margin: 2rem auto; padding: 2rem; }
    .post-list { list-style: none; padding: 0; }
    .post-list li { margin: 1rem 0; }
    .post-list a {
      display: block;
      padding: 1rem;
      background: #f5f5f5;
      border-radius: 4px;
      text-decoration: none;
      color: #333;
      transition: background 0.2s;
    }
    .post-list a:hover { background: #e0e0e0; }
  </style>
</head>
<body>
  <div class="container">
    <h1>My Blog</h1>
    <ul class="post-list">
      <li><a href="/blog/welcome">Welcome to My Blog</a></li>
      <li><a href="/blog/markdown-guide">Markdown Guide</a></li>
    </ul>
  </div>
</body>
</html>`;

  return {
    status: 200,
    body: html,
    contentType: "text/html; charset=UTF-8",
  };
}

function showPost(context, slug) {
  const req = context.request;

  // Load markdown from storage
  const markdown = sharedStorage.getItem(`blog:${slug}`);

  if (!markdown) {
    return {
      status: 404,
      body: `<!DOCTYPE html>
<html>
<head><title>Not Found</title></head>
<body>
  <h1>Blog post not found</h1>
  <p>The post "${slug}" does not exist.</p>
  <a href="/blog">← Back to blog</a>
</body>
</html>`,
      contentType: "text/html; charset=UTF-8",
    };
  }

  // Convert markdown to HTML
  const content = convert.markdown_to_html(markdown);

  if (content.startsWith("Error:")) {
    console.error(`Failed to convert blog post ${slug}: ${content}`);
    return {
      status: 500,
      body: `Error rendering blog post: ${content}`,
      contentType: "text/plain; charset=UTF-8",
    };
  }

  // Wrap in blog template
  const html = `<!DOCTYPE html>
<html lang="en">
<head>
  <meta charset="UTF-8">
  <meta name="viewport" content="width=device-width, initial-scale=1.0">
  <title>Blog - ${slug}</title>
  <link rel="stylesheet" href="/engine.css">
  <style>
    .blog-container {
      max-width: 800px;
      margin: 2rem auto;
      padding: 2rem;
      background: white;
      border-radius: 8px;
      box-shadow: 0 2px 4px rgba(0,0,0,0.1);
    }
    .blog-nav {
      margin-bottom: 2rem;
      padding-bottom: 1rem;
      border-bottom: 1px solid #e0e0e0;
    }
    .blog-nav a {
      color: #0066cc;
      text-decoration: none;
    }
    .blog-content h1 {
      color: #333;
      margin-top: 0;
    }
    .blog-content h2 {
      color: #555;
      border-bottom: 2px solid #0066cc;
      padding-bottom: 0.5rem;
    }
    .blog-content code {
      background: #f4f4f4;
      padding: 2px 6px;
      border-radius: 3px;
      font-family: 'Monaco', 'Courier New', monospace;
    }
    .blog-content pre {
      background: #f4f4f4;
      padding: 1rem;
      border-radius: 4px;
      overflow-x: auto;
    }
    .blog-content pre code {
      background: none;
      padding: 0;
    }
    .blog-content table {
      width: 100%;
      border-collapse: collapse;
      margin: 1rem 0;
    }
    .blog-content th,
    .blog-content td {
      border: 1px solid #ddd;
      padding: 0.75rem;
      text-align: left;
    }
    .blog-content th {
      background: #f4f4f4;
      font-weight: bold;
    }
    .blog-content blockquote {
      border-left: 4px solid #0066cc;
      padding-left: 1rem;
      margin: 1rem 0;
      color: #666;
      font-style: italic;
    }
  </style>
</head>
<body>
  <div class="blog-container">
    <div class="blog-nav">
      <a href="/blog">← Back to all posts</a>
    </div>
    <div class="blog-content">
      ${content}
    </div>
  </div>
</body>
</html>`;

  return {
    status: 200,
    body: html,
    contentType: "text/html; charset=UTF-8",
  };
}

function createPost(context) {
  const req = context.request;

  const slug = req.form.slug || "";
  const markdown = req.form.content || "";

  if (!slug || !markdown) {
    return {
      status: 400,
      body: "Missing slug or content",
      contentType: "text/plain; charset=UTF-8",
    };
  }

  // Validate markdown size (10KB limit for this example)
  if (markdown.length > 10000) {
    return {
      status: 400,
      body: "Blog post too long (max 10KB)",
      contentType: "text/plain; charset=UTF-8",
    };
  }

  // Test conversion before storing
  const testHtml = convert.markdown_to_html(markdown);
  if (testHtml.startsWith("Error:")) {
    return {
      status: 400,
      body: `Invalid markdown: ${testHtml}`,
      contentType: "text/plain; charset=UTF-8",
    };
  }

  // Store the markdown
  sharedStorage.setItem(`blog:${slug}`, markdown);

  console.log(`Blog post created: ${slug}`);

  return {
    status: 201,
    body: JSON.stringify({
      success: true,
      message: "Blog post created",
      slug: slug,
      url: `/blog/${slug}`,
    }),
    contentType: "application/json",
  };
}
