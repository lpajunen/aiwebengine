// Markdown Blog Example
// Demonstrates using convert.markdown_to_html() to render blog posts

function init(context) {
  // Register routes for the blog
  routeRegistry.registerRoute("/blog", "blogRouter", "GET");
  routeRegistry.registerRoute("/blog/*", "blogRouter", "GET");
  routeRegistry.registerRoute("/blog/admin/create", "createPost", "POST");
  routeRegistry.registerRoute("/blog/admin/update", "updatePost", "POST");
  routeRegistry.registerRoute("/blog/admin/delete", "deletePost", "POST");
  routeRegistry.registerRoute("/blog/new", "newPostForm", "GET");

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

  // Store example posts and initialize index
  const postSlugs = [];
  for (const slug in examplePosts) {
    sharedStorage.setItem("blog:" + slug, examplePosts[slug]);
    postSlugs.push(slug);
  }

  // Store the index of all post slugs
  sharedStorage.setItem("blog:index", JSON.stringify(postSlugs));

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

  // If path is "/blog/new", show the create post form
  if (
    pathParts.length === 2 &&
    pathParts[0] === "blog" &&
    pathParts[1] === "new"
  ) {
    return newPostForm(context);
  }

  // If path is "/blog/slug/edit", show the edit post form
  if (
    pathParts.length === 3 &&
    pathParts[0] === "blog" &&
    pathParts[2] === "edit"
  ) {
    return editPostForm(context, pathParts[1]);
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

  // Get all blog posts from storage using the index
  const posts = [];

  try {
    // Get the index of all post slugs
    const indexJson = sharedStorage.getItem("blog:index");
    let postSlugs = [];

    if (indexJson) {
      postSlugs = JSON.parse(indexJson);
    }

    // Get all posts from the index
    postSlugs.forEach((slug) => {
      const content = sharedStorage.getItem("blog:" + slug);
      if (content) {
        // Extract title from first header
        const titleMatch = content.match(/^# (.+)$/m);
        const title = titleMatch ? titleMatch[1] : slug;
        posts.push({ slug: slug, title: title });
      }
    });
  } catch (error) {
    console.log("Error loading posts: " + error);
  }

  // Generate HTML for posts list
  let postsHtml = "";
  if (posts.length === 0) {
    postsHtml =
      '<p>No blog posts yet. <a href="/blog/new">Create your first post</a></p>';
  } else {
    postsHtml = posts
      .map(
        (post) =>
          `<li class="post-item">
        <a href="/blog/${post.slug}" class="post-link">${post.title}</a>
        <a href="/blog/${post.slug}/edit" class="edit-btn" title="Edit post">✏️</a>
      </li>`,
      )
      .join("");
  }

  const html = `<!DOCTYPE html>
<html lang="en">
<head>
  <meta charset="UTF-8">
  <meta name="viewport" content="width=device-width, initial-scale=1.0">
  <title>My Blog</title>
  <link rel="stylesheet" href="/engine.css">
  <style>
    .container { max-width: 800px; margin: 2rem auto; padding: 2rem; }
    .blog-header { display: flex; justify-content: space-between; align-items: center; margin-bottom: 2rem; }
    .blog-header h1 { margin: 0; }
    .create-post-btn {
      background: #0066cc;
      color: white;
      padding: 0.75rem 1.5rem;
      text-decoration: none;
      border-radius: 4px;
      font-weight: bold;
      transition: background 0.2s;
    }
    .create-post-btn:hover { background: #0052a3; }
    .post-list { list-style: none; padding: 0; }
    .post-list li { margin: 1rem 0; }
    .post-item {
      display: flex;
      align-items: center;
      background: #f5f5f5;
      border-radius: 4px;
      overflow: hidden;
    }
    .post-link {
      flex: 1;
      padding: 1rem;
      text-decoration: none;
      color: #333;
      transition: background 0.2s;
    }
    .post-link:hover { background: #e0e0e0; }
    .edit-btn {
      padding: 1rem;
      background: #f8f9fa;
      color: #666;
      text-decoration: none;
      font-size: 1.2rem;
      transition: background 0.2s;
    }
    .edit-btn:hover {
      background: #e9ecef;
      color: #333;
    }
  </style>
</head>
<body>
  <div class="container">
    <div class="blog-header">
      <h1>My Blog</h1>
      <a href="/blog/new" class="create-post-btn">+ New Post</a>
    </div>
    <ul class="post-list">
      ${postsHtml}
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
      display: flex;
      justify-content: space-between;
      align-items: center;
    }
    .blog-nav a {
      color: #0066cc;
      text-decoration: none;
    }
    .blog-nav .nav-left a:first-child {
      margin-right: 1rem;
    }
    .edit-btn {
      background: #ffc107;
      color: #212529;
      padding: 0.5rem 1rem;
      text-decoration: none;
      border-radius: 4px;
      font-weight: bold;
      font-size: 0.9rem;
      transition: background 0.2s;
    }
    .edit-btn:hover { background: #e0a800; }
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
      <div class="nav-left">
        <a href="/blog">← Back to all posts</a>
      </div>
      <a href="/blog/${slug}/edit" class="edit-btn">Edit Post</a>
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

function newPostForm(context) {
  const req = context.request;

  const html = `<!DOCTYPE html>
<html lang="en">
<head>
  <meta charset="UTF-8">
  <meta name="viewport" content="width=device-width, initial-scale=1.0">
  <title>Create New Blog Post</title>
  <link rel="stylesheet" href="/engine.css">
  <style>
    .container {
      max-width: 900px;
      margin: 2rem auto;
      padding: 2rem;
      background: white;
      border-radius: 8px;
      box-shadow: 0 2px 4px rgba(0,0,0,0.1);
    }
    .form-header {
      display: flex;
      justify-content: space-between;
      align-items: center;
      margin-bottom: 2rem;
      padding-bottom: 1rem;
      border-bottom: 1px solid #e0e0e0;
    }
    .form-header h1 { margin: 0; }
    .back-link {
      color: #0066cc;
      text-decoration: none;
    }
    .back-link:hover { text-decoration: underline; }
    .form-group {
      margin-bottom: 1.5rem;
    }
    .form-group label {
      display: block;
      margin-bottom: 0.5rem;
      font-weight: bold;
      color: #333;
    }
    .form-group input,
    .form-group textarea {
      width: 100%;
      padding: 0.75rem;
      border: 1px solid #ddd;
      border-radius: 4px;
      font-family: inherit;
      font-size: 1rem;
    }
    .form-group input:focus,
    .form-group textarea:focus {
      outline: none;
      border-color: #0066cc;
      box-shadow: 0 0 0 2px rgba(0,102,204,0.2);
    }
    .form-group textarea {
      min-height: 400px;
      resize: vertical;
      font-family: 'Monaco', 'Courier New', monospace;
      line-height: 1.4;
    }
    .form-actions {
      display: flex;
      gap: 1rem;
      margin-top: 2rem;
    }
    .btn {
      padding: 0.75rem 1.5rem;
      border: none;
      border-radius: 4px;
      font-size: 1rem;
      font-weight: bold;
      cursor: pointer;
      text-decoration: none;
      display: inline-block;
      transition: background 0.2s;
    }
    .btn-primary {
      background: #0066cc;
      color: white;
    }
    .btn-primary:hover { background: #0052a3; }
    .btn-secondary {
      background: #6c757d;
      color: white;
    }
    .btn-secondary:hover { background: #545b62; }
    .preview-section {
      margin-top: 2rem;
      padding-top: 2rem;
      border-top: 1px solid #e0e0e0;
    }
    .preview-toggle {
      background: #f8f9fa;
      border: 1px solid #dee2e6;
      padding: 1rem;
      border-radius: 4px;
      margin-bottom: 1rem;
    }
    .preview-content {
      background: #f8f9fa;
      border: 1px solid #dee2e6;
      padding: 1rem;
      border-radius: 4px;
      min-height: 200px;
    }
    .help-text {
      font-size: 0.9rem;
      color: #666;
      margin-top: 0.5rem;
    }
    .markdown-help {
      background: #e7f3ff;
      border: 1px solid #b3d9ff;
      padding: 1rem;
      border-radius: 4px;
      margin-bottom: 1rem;
    }
    .markdown-help h4 {
      margin-top: 0;
      color: #0066cc;
    }
    .markdown-examples {
      display: grid;
      grid-template-columns: repeat(auto-fit, minmax(250px, 1fr));
      gap: 1rem;
      margin-top: 1rem;
    }
    .example {
      background: white;
      padding: 0.75rem;
      border-radius: 4px;
      border: 1px solid #dee2e6;
    }
    .example code {
      background: #f4f4f4;
      padding: 2px 4px;
      border-radius: 2px;
      font-family: 'Monaco', 'Courier New', monospace;
    }
  </style>
</head>
<body>
  <div class="container">
    <div class="form-header">
      <h1>Create New Blog Post</h1>
      <a href="/blog" class="back-link">← Back to Blog</a>
    </div>

    <form id="blog-form" action="/blog/admin/create" method="POST">
      <div class="form-group">
        <label for="slug">Post Slug (URL identifier)</label>
        <input type="text" id="slug" name="slug" required
               placeholder="my-awesome-post"
               pattern="[a-z0-9-]+"
               title="Only lowercase letters, numbers, and hyphens allowed">
        <div class="help-text">This will be part of the URL: /blog/your-slug-here</div>
      </div>

      <div class="form-group">
        <label for="content">Content (Markdown)</label>
        <textarea id="content" name="content" required
                  placeholder="# Your Blog Post Title

Write your blog post content here using Markdown formatting.

## Section Header

- List item 1
- List item 2

\`\`\`javascript
// Code blocks are supported
function hello() {
  return 'Hello, World!';
}
\`\`\`

[Links](https://example.com) and **bold text** work too!"></textarea>
      </div>

      <div class="markdown-help">
        <h4>Markdown Quick Reference</h4>
        <div class="markdown-examples">
          <div class="example">
            <strong>Headers:</strong><br>
            <code># H1</code><br>
            <code>## H2</code><br>
            <code>### H3</code>
          </div>
          <div class="example">
            <strong>Text:</strong><br>
            <code>**bold**</code><br>
            <code>*italic*</code><br>
            <code>~~strikethrough~~</code>
          </div>
          <div class="example">
            <strong>Lists:</strong><br>
            <code>- item</code><br>
            <code>1. numbered</code>
          </div>
          <div class="example">
            <strong>Code:</strong><br>
            <code>\`inline\`</code><br>
            <code>\`\`\`block\`\`\`</code>
          </div>
          <div class="example">
            <strong>Links:</strong><br>
            <code>[text](url)</code>
          </div>
          <div class="example">
            <strong>Tables:</strong><br>
            <code>| A | B |</code><br>
            <code>|---|---|</code>
          </div>
        </div>
      </div>

      <div class="preview-section">
        <button type="button" id="preview-btn" class="btn btn-secondary">Toggle Preview</button>
        <div id="preview-container" style="display: none;">
          <h3>Preview</h3>
          <div id="preview-content" class="preview-content">
            Preview will appear here...
          </div>
        </div>
      </div>

      <div class="form-actions">
        <button type="submit" class="btn btn-primary">Create Post</button>
        <a href="/blog" class="btn btn-secondary">Cancel</a>
      </div>
    </form>
  </div>

  <script>
    // Auto-generate slug from title (if there's a title in the content)
    document.getElementById('content').addEventListener('input', function() {
      const content = this.value;
      const titleMatch = content.match(/^# (.+)$/m);
      if (titleMatch && !document.getElementById('slug').value) {
        const slug = titleMatch[1]
          .toLowerCase()
          .replace(/[^a-z0-9\\s-]/g, '')
          .replace(/\\s+/g, '-')
          .replace(/-+/g, '-')
          .trim();
        document.getElementById('slug').value = slug;
      }
    });

    // Preview functionality
    document.getElementById('preview-btn').addEventListener('click', function() {
      const previewContainer = document.getElementById('preview-container');
      const isVisible = previewContainer.style.display !== 'none';

      if (isVisible) {
        previewContainer.style.display = 'none';
        this.textContent = 'Toggle Preview';
      } else {
        updatePreview();
        previewContainer.style.display = 'block';
        this.textContent = 'Hide Preview';
      }
    });

    function updatePreview() {
      const content = document.getElementById('content').value;
      const previewContent = document.getElementById('preview-content');

      if (!content.trim()) {
        previewContent.innerHTML = '<em>Preview will appear here...</em>';
        return;
      }

      // Simple client-side markdown preview (basic conversion)
      let html = content
        // Headers
        .replace(/^### (.*$)/gim, '<h3>$1</h3>')
        .replace(/^## (.*$)/gim, '<h2>$1</h2>')
        .replace(/^# (.*$)/gim, '<h1>$1</h1>')
        // Bold
        .replace(/\\*\\*(.*?)\\*\\*/g, '<strong>$1</strong>')
        // Italic
        .replace(/\\*(.*?)\\*/g, '<em>$1</em>')
        // Lists
        .replace(/^\\* (.*$)/gim, '<li>$1</li>')
        .replace(/^\\d+\\. (.*$)/gim, '<li>$1</li>')
        // Line breaks
        .replace(/\\n/g, '<br>');

      // Wrap lists
      html = html.replace(/(<li>.*<\/li>(\\s*<li>.*<\/li>)*)/g, '<ul>$1</ul>');

      previewContent.innerHTML = html;
    }

    // Update preview on content change
    let previewTimeout;
    document.getElementById('content').addEventListener('input', function() {
      clearTimeout(previewTimeout);
      previewTimeout = setTimeout(updatePreview, 500);
    });

    // Form validation
    document.getElementById('blog-form').addEventListener('submit', function(e) {
      const slug = document.getElementById('slug').value;
      const content = document.getElementById('content').value;

      if (!slug || !content) {
        e.preventDefault();
        alert('Please fill in both slug and content fields.');
        return;
      }

      if (!/^[a-z0-9-]+$/.test(slug)) {
        e.preventDefault();
        alert('Slug can only contain lowercase letters, numbers, and hyphens.');
        return;
      }
    });
  </script>
</body>
</html>`;

  return {
    status: 200,
    body: html,
    contentType: "text/html; charset=UTF-8",
  };
}

function editPostForm(context, slug) {
  const req = context.request;

  // Load existing post content
  const existingContent = sharedStorage.getItem("blog:" + slug);

  if (!existingContent) {
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

  const html = `<!DOCTYPE html>
<html lang="en">
<head>
  <meta charset="UTF-8">
  <meta name="viewport" content="width=device-width, initial-scale=1.0">
  <title>Edit Blog Post</title>
  <link rel="stylesheet" href="/engine.css">
  <style>
    .container {
      max-width: 900px;
      margin: 2rem auto;
      padding: 2rem;
      background: white;
      border-radius: 8px;
      box-shadow: 0 2px 4px rgba(0,0,0,0.1);
    }
    .form-header {
      display: flex;
      justify-content: space-between;
      align-items: center;
      margin-bottom: 2rem;
      padding-bottom: 1rem;
      border-bottom: 1px solid #e0e0e0;
    }
    .form-header h1 { margin: 0; }
    .back-link {
      color: #0066cc;
      text-decoration: none;
    }
    .back-link:hover { text-decoration: underline; }
    .form-group {
      margin-bottom: 1.5rem;
    }
    .form-group label {
      display: block;
      margin-bottom: 0.5rem;
      font-weight: bold;
      color: #333;
    }
    .form-group input,
    .form-group textarea {
      width: 100%;
      padding: 0.75rem;
      border: 1px solid #ddd;
      border-radius: 4px;
      font-family: inherit;
      font-size: 1rem;
    }
    .form-group input:focus,
    .form-group textarea:focus {
      outline: none;
      border-color: #0066cc;
      box-shadow: 0 0 0 2px rgba(0,102,204,0.2);
    }
    .form-group textarea {
      min-height: 400px;
      resize: vertical;
      font-family: 'Monaco', 'Courier New', monospace;
      line-height: 1.4;
    }
    .form-actions {
      display: flex;
      gap: 1rem;
      margin-top: 2rem;
    }
    .btn {
      padding: 0.75rem 1.5rem;
      border: none;
      border-radius: 4px;
      font-size: 1rem;
      font-weight: bold;
      cursor: pointer;
      text-decoration: none;
      display: inline-block;
      transition: background 0.2s;
    }
    .btn-primary {
      background: #0066cc;
      color: white;
    }
    .btn-primary:hover { background: #0052a3; }
    .btn-secondary {
      background: #6c757d;
      color: white;
    }
    .btn-secondary:hover { background: #545b62; }
    .btn-danger {
      background: #dc3545;
      color: white;
    }
    .btn-danger:hover { background: #c82333; }
    .preview-section {
      margin-top: 2rem;
      padding-top: 2rem;
      border-top: 1px solid #e0e0e0;
    }
    .preview-toggle {
      background: #f8f9fa;
      border: 1px solid #dee2e6;
      padding: 1rem;
      border-radius: 4px;
      margin-bottom: 1rem;
    }
    .preview-content {
      background: #f8f9fa;
      border: 1px solid #dee2e6;
      padding: 1rem;
      border-radius: 4px;
      min-height: 200px;
    }
    .help-text {
      font-size: 0.9rem;
      color: #666;
      margin-top: 0.5rem;
    }
    .markdown-help {
      background: #e7f3ff;
      border: 1px solid #b3d9ff;
      padding: 1rem;
      border-radius: 4px;
      margin-bottom: 1rem;
    }
    .markdown-help h4 {
      margin-top: 0;
      color: #0066cc;
    }
    .markdown-examples {
      display: grid;
      grid-template-columns: repeat(auto-fit, minmax(250px, 1fr));
      gap: 1rem;
      margin-top: 1rem;
    }
    .example {
      background: white;
      padding: 0.75rem;
      border-radius: 4px;
      border: 1px solid #dee2e6;
    }
    .example code {
      background: #f4f4f4;
      padding: 2px 4px;
      border-radius: 2px;
      font-family: 'Monaco', 'Courier New', monospace;
    }
  </style>
</head>
<body>
  <div class="container">
    <div class="form-header">
      <h1>Edit Blog Post</h1>
      <div>
        <a href="/blog/${slug}" class="back-link">View Post</a>
        <span style="margin: 0 0.5rem;">|</span>
        <a href="/blog" class="back-link">Back to Blog</a>
      </div>
    </div>

    <form id="blog-form" action="/blog/admin/update" method="POST">
      <input type="hidden" name="originalSlug" value="${slug}">
      
      <div class="form-group">
        <label for="slug">Post Slug (URL identifier)</label>
        <input type="text" id="slug" name="slug" required
               value="${slug}"
               pattern="[a-z0-9-]+"
               title="Only lowercase letters, numbers, and hyphens allowed">
        <div class="help-text">This will be part of the URL: /blog/your-slug-here</div>
      </div>

      <div class="form-group">
        <label for="content">Content (Markdown)</label>
        <textarea id="content" name="content" required>${existingContent.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;").replace(/"/g, "&quot;").replace(/'/g, "&#39;")}</textarea>
      </div>

      <div class="markdown-help">
        <h4>Markdown Quick Reference</h4>
        <div class="markdown-examples">
          <div class="example">
            <strong>Headers:</strong><br>
            <code># H1</code><br>
            <code>## H2</code><br>
            <code>### H3</code>
          </div>
          <div class="example">
            <strong>Text:</strong><br>
            <code>**bold**</code><br>
            <code>*italic*</code><br>
            <code>~~strikethrough~~</code>
          </div>
          <div class="example">
            <strong>Lists:</strong><br>
            <code>- item</code><br>
            <code>1. numbered</code>
          </div>
          <div class="example">
            <strong>Code:</strong><br>
            <code>\`inline\`</code><br>
            <code>\`\`\`block\`\`\`</code>
          </div>
          <div class="example">
            <strong>Links:</strong><br>
            <code>[text](url)</code>
          </div>
          <div class="example">
            <strong>Tables:</strong><br>
            <code>| A | B |</code><br>
            <code>|---|---|</code>
          </div>
        </div>
      </div>

      <div class="preview-section">
        <button type="button" id="preview-btn" class="btn btn-secondary">Toggle Preview</button>
        <div id="preview-container" style="display: none;">
          <h3>Preview</h3>
          <div id="preview-content" class="preview-content">
            Preview will appear here...
          </div>
        </div>
      </div>

      <div class="form-actions">
        <button type="submit" class="btn btn-primary">Update Post</button>
        <a href="/blog/${slug}" class="btn btn-secondary">Cancel</a>
        <button type="button" id="delete-btn" class="btn btn-danger">Delete Post</button>
      </div>
    </form>
  </div>

  <script>
    // Auto-generate slug from title (if there's a title in the content)
    document.getElementById('content').addEventListener('input', function() {
      const content = this.value;
      const titleMatch = content.match(/^# (.+)$/m);
      if (titleMatch && !document.getElementById('slug').value) {
        const slug = titleMatch[1]
          .toLowerCase()
          .replace(/[^a-z0-9\\s-]/g, '')
          .replace(/\\s+/g, '-')
          .replace(/-+/g, '-')
          .trim();
        document.getElementById('slug').value = slug;
      }
    });

    // Preview functionality
    document.getElementById('preview-btn').addEventListener('click', function() {
      const previewContainer = document.getElementById('preview-container');
      const isVisible = previewContainer.style.display !== 'none';

      if (isVisible) {
        previewContainer.style.display = 'none';
        this.textContent = 'Toggle Preview';
      } else {
        updatePreview();
        previewContainer.style.display = 'block';
        this.textContent = 'Hide Preview';
      }
    });

    function updatePreview() {
      const content = document.getElementById('content').value;
      const previewContent = document.getElementById('preview-content');

      if (!content.trim()) {
        previewContent.innerHTML = '<em>Preview will appear here...</em>';
        return;
      }

      // Simple client-side markdown preview (basic conversion)
      let html = content
        // Headers
        .replace(/^### (.*$)/gim, '<h3>$1</h3>')
        .replace(/^## (.*$)/gim, '<h2>$1</h2>')
        .replace(/^# (.*$)/gim, '<h1>$1</h1>')
        // Bold
        .replace(/\\*\\*(.*?)\\*\\*/g, '<strong>$1</strong>')
        // Italic
        .replace(/\\*(.*?)\\*/g, '<em>$1</em>')
        // Lists
        .replace(/^\\* (.*$)/gim, '<li>$1</li>')
        .replace(/^\\d+\\. (.*$)/gim, '<li>$1</li>')
        // Line breaks
        .replace(/\\n/g, '<br>');

      // Wrap lists
      html = html.replace(/(<li>.*<\/li>(\\s*<li>.*<\/li>)*)/g, '<ul>$1</ul>');

      previewContent.innerHTML = html;
    }

    // Update preview on content change
    let previewTimeout;
    document.getElementById('content').addEventListener('input', function() {
      clearTimeout(previewTimeout);
      previewTimeout = setTimeout(updatePreview, 500);
    });

    // Form validation
    document.getElementById('blog-form').addEventListener('submit', function(e) {
      const slug = document.getElementById('slug').value;
      const content = document.getElementById('content').value;

      if (!slug || !content) {
        e.preventDefault();
        alert('Please fill in both slug and content fields.');
        return;
      }

      if (!/^[a-z0-9-]+$/.test(slug)) {
        e.preventDefault();
        alert('Slug can only contain lowercase letters, numbers, and hyphens.');
        return;
      }
    });

    // Delete functionality
    document.getElementById('delete-btn').addEventListener('click', function() {
      if (confirm('Are you sure you want to delete this blog post? This action cannot be undone.')) {
        const form = document.createElement('form');
        form.method = 'POST';
        form.action = '/blog/admin/delete';
        
        const slugInput = document.createElement('input');
        slugInput.type = 'hidden';
        slugInput.name = 'slug';
        slugInput.value = '${slug}';
        form.appendChild(slugInput);
        
        document.body.appendChild(form);
        form.submit();
      }
    });
  </script>
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

  // Validate slug format
  if (!/^[a-z0-9-]+$/.test(slug)) {
    return {
      status: 400,
      body: "Invalid slug format. Only lowercase letters, numbers, and hyphens allowed.",
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
      body: "Invalid markdown: " + testHtml,
      contentType: "text/plain; charset=UTF-8",
    };
  }

  // Store the markdown
  sharedStorage.setItem("blog:" + slug, markdown);

  // Update the index
  try {
    const indexJson = sharedStorage.getItem("blog:index");
    let postSlugs = [];
    if (indexJson) {
      postSlugs = JSON.parse(indexJson);
    }
    if (!postSlugs.includes(slug)) {
      postSlugs.push(slug);
      sharedStorage.setItem("blog:index", JSON.stringify(postSlugs));
    }
  } catch (error) {
    console.log("Error updating index: " + error);
  }

  console.log("Blog post created: " + slug);

  // Redirect back to the blog list with success message
  const html = `<!DOCTYPE html>
<html>
<head>
  <meta http-equiv="refresh" content="2;url=/blog">
  <title>Post Created</title>
  <style>
    body { font-family: Arial, sans-serif; text-align: center; padding: 2rem; }
    .success { color: #28a745; font-size: 1.2rem; }
  </style>
</head>
<body>
  <h1 class="success">✓ Blog post created successfully!</h1>
  <p>Redirecting to blog list...</p>
  <p><a href="/blog">Click here if not redirected</a></p>
</body>
</html>`;

  return {
    status: 201,
    body: html,
    contentType: "application/json",
  };
}

function updatePost(context) {
  const req = context.request;

  const originalSlug = req.form.originalSlug || "";
  const newSlug = req.form.slug || "";
  const markdown = req.form.content || "";

  if (!originalSlug || !newSlug || !markdown) {
    return {
      status: 400,
      body: "Missing originalSlug, slug, or content",
      contentType: "text/plain; charset=UTF-8",
    };
  }

  // Validate slug format
  if (!/^[a-z0-9-]+$/.test(newSlug)) {
    return {
      status: 400,
      body: "Invalid slug format. Only lowercase letters, numbers, and hyphens allowed.",
      contentType: "text/plain; charset=UTF-8",
    };
  }

  // Check if original post exists
  const existingContent = sharedStorage.getItem("blog:" + originalSlug);
  if (!existingContent) {
    return {
      status: 404,
      body: "Original post not found",
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
      body: "Invalid markdown: " + testHtml,
      contentType: "text/plain; charset=UTF-8",
    };
  }

  try {
    // Get current index
    const indexJson = sharedStorage.getItem("blog:index");
    let postSlugs = [];
    if (indexJson) {
      postSlugs = JSON.parse(indexJson);
    }

    // If slug changed, update the index
    if (originalSlug !== newSlug) {
      // Remove old slug and add new one
      const oldIndex = postSlugs.indexOf(originalSlug);
      if (oldIndex > -1) {
        postSlugs.splice(oldIndex, 1);
      }
      if (!postSlugs.includes(newSlug)) {
        postSlugs.push(newSlug);
      }

      // Delete old post
      sharedStorage.setItem("blog:" + originalSlug, null);
    }

    // Store the updated markdown
    sharedStorage.setItem("blog:" + newSlug, markdown);

    // Update the index
    sharedStorage.setItem("blog:index", JSON.stringify(postSlugs));

    console.log("Blog post updated: " + originalSlug + " -> " + newSlug);

    // Redirect back to the updated post
    const html = `<!DOCTYPE html>
<html>
<head>
  <meta http-equiv="refresh" content="2;url=/blog/${newSlug}">
  <title>Post Updated</title>
  <style>
    body { font-family: Arial, sans-serif; text-align: center; padding: 2rem; }
    .success { color: #28a745; font-size: 1.2rem; }
  </style>
</head>
<body>
  <h1 class="success">✓ Blog post updated successfully!</h1>
  <p>Redirecting to updated post...</p>
  <p><a href="/blog/${newSlug}">Click here if not redirected</a></p>
</body>
</html>`;

    return {
      status: 200,
      body: html,
      contentType: "text/html; charset=UTF-8",
    };
  } catch (error) {
    console.log("Error updating post: " + error);
    return {
      status: 500,
      body: "Error updating post: " + error,
      contentType: "text/plain; charset=UTF-8",
    };
  }
}

function deletePost(context) {
  const req = context.request;

  const slug = req.form.slug || "";

  if (!slug) {
    return {
      status: 400,
      body: "Missing slug",
      contentType: "text/plain; charset=UTF-8",
    };
  }

  // Check if post exists
  const existingContent = sharedStorage.getItem("blog:" + slug);
  if (!existingContent) {
    return {
      status: 404,
      body: "Post not found",
      contentType: "text/plain; charset=UTF-8",
    };
  }

  try {
    // Get current index
    const indexJson = sharedStorage.getItem("blog:index");
    let postSlugs = [];
    if (indexJson) {
      postSlugs = JSON.parse(indexJson);
    }

    // Remove from index
    const slugIndex = postSlugs.indexOf(slug);
    if (slugIndex > -1) {
      postSlugs.splice(slugIndex, 1);
    }

    // Delete the post
    sharedStorage.setItem("blog:" + slug, null);

    // Update the index
    sharedStorage.setItem("blog:index", JSON.stringify(postSlugs));

    console.log("Blog post deleted: " + slug);

    // Redirect back to the blog list
    const html = `<!DOCTYPE html>
<html>
<head>
  <meta http-equiv="refresh" content="2;url=/blog">
  <title>Post Deleted</title>
  <style>
    body { font-family: Arial, sans-serif; text-align: center; padding: 2rem; }
    .success { color: #dc3545; font-size: 1.2rem; }
  </style>
</head>
<body>
  <h1 class="success">✓ Blog post deleted successfully!</h1>
  <p>Redirecting to blog list...</p>
  <p><a href="/blog">Click here if not redirected</a></p>
</body>
</html>`;

    return {
      status: 200,
      body: html,
      contentType: "text/html; charset=UTF-8",
    };
  } catch (error) {
    console.log("Error deleting post: " + error);
    return {
      status: 500,
      body: "Error deleting post: " + error,
      contentType: "text/plain; charset=UTF-8",
    };
  }
}
