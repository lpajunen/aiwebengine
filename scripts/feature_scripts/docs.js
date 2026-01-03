/// <reference path="../../assets/aiwebengine-priv.d.ts" />

/**
 * Documentation Feature Script
 * Serves markdown documentation from assets/docs as HTML at /engine/docs routes
 */

function getRequest(context) {
  return (context && context.request) || {};
}

/**
 * Decode base64 string to UTF-8 text
 */
function decodeBase64(base64) {
  const chars =
    "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
  let buffer = [];
  let bits = 0;
  let value = 0;

  for (let i = 0; i < base64.length; i++) {
    const char = base64[i];
    if (char === "=" || char === "\n" || char === "\r" || char === " ")
      continue;

    const charIndex = chars.indexOf(char);
    if (charIndex === -1) {
      throw new Error("Invalid base64 character: " + char);
    }

    value = (value << 6) | charIndex;
    bits += 6;

    if (bits >= 8) {
      bits -= 8;
      buffer.push((value >> bits) & 0xff);
      value &= (1 << bits) - 1;
    }
  }

  // Convert bytes to UTF-8 string
  let result = "";
  let i = 0;
  while (i < buffer.length) {
    const byte1 = buffer[i++];

    // Single-byte character (0xxxxxxx)
    if (byte1 < 0x80) {
      result += String.fromCharCode(byte1);
    }
    // Two-byte character (110xxxxx 10xxxxxx)
    else if (byte1 >= 0xc0 && byte1 < 0xe0 && i < buffer.length) {
      const byte2 = buffer[i++];
      const codePoint = ((byte1 & 0x1f) << 6) | (byte2 & 0x3f);
      result += String.fromCharCode(codePoint);
    }
    // Three-byte character (1110xxxx 10xxxxxx 10xxxxxx)
    else if (byte1 >= 0xe0 && byte1 < 0xf0 && i + 1 < buffer.length) {
      const byte2 = buffer[i++];
      const byte3 = buffer[i++];
      const codePoint =
        ((byte1 & 0x0f) << 12) | ((byte2 & 0x3f) << 6) | (byte3 & 0x3f);
      result += String.fromCharCode(codePoint);
    }
    // Four-byte character (11110xxx 10xxxxxx 10xxxxxx 10xxxxxx) - for emojis
    else if (byte1 >= 0xf0 && byte1 < 0xf8 && i + 2 < buffer.length) {
      const byte2 = buffer[i++];
      const byte3 = buffer[i++];
      const byte4 = buffer[i++];
      let codePoint =
        ((byte1 & 0x07) << 18) |
        ((byte2 & 0x3f) << 12) |
        ((byte3 & 0x3f) << 6) |
        (byte4 & 0x3f);
      // Convert to UTF-16 surrogate pair for characters above U+FFFF
      if (codePoint > 0xffff) {
        codePoint -= 0x10000;
        result += String.fromCharCode(0xd800 + (codePoint >> 10));
        result += String.fromCharCode(0xdc00 + (codePoint & 0x3ff));
      } else {
        result += String.fromCharCode(codePoint);
      }
    } else {
      // Invalid UTF-8 sequence, skip byte
      console.error("[docs.js] Invalid UTF-8 byte: " + byte1.toString(16));
    }
  }

  return result;
}

/**
 * Extract title from markdown by finding first H1 heading
 */
function extractTitle(markdown) {
  const lines = markdown.split("\n");
  for (let i = 0; i < lines.length; i++) {
    const line = lines[i].trim();
    if (line.startsWith("# ")) {
      return line.substring(2).trim();
    }
  }
  return "Documentation";
}

/**
 * Map URL path to asset name
 * /engine/docs/ -> docs/README.md
 * /engine/docs/guides/scripts -> docs/guides/scripts.md
 * /engine/docs/guides/scripts/ -> docs/guides/scripts.md
 */
function mapPathToAssetName(docPath) {
  // Remove trailing slash if present
  if (docPath.endsWith("/")) {
    docPath = docPath.substring(0, docPath.length - 1);
  }

  // Empty path means README
  if (docPath === "" || docPath === "/") {
    return "docs/README.md";
  }

  // Remove leading slash if present
  if (docPath.startsWith("/")) {
    docPath = docPath.substring(1);
  }

  // If path already has .md extension, use as-is
  if (docPath.endsWith(".md")) {
    return "docs/" + docPath;
  }

  // Otherwise add .md extension
  return "docs/" + docPath + ".md";
}

/**
 * Wrap HTML content in styled template
 */
function wrapInTemplate(htmlContent, title) {
  return `<!DOCTYPE html>
<html lang="en">
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <title>${title}</title>
    <link rel="stylesheet" href="/engine.css">
    <link rel="icon" type="image/x-icon" href="/favicon.ico">
    <style>
        /* Documentation-specific overrides */
        body {
            background-color: #1e1e1e;
            padding: 0;
            margin: 0;
            height: 100vh;
            overflow: hidden;
        }

        .docs-container {
            display: flex;
            flex-direction: column;
            height: 100vh;
            background: #1e1e1e;
            overflow: hidden;
        }

        /* Unified header styles inherited from editor.css */
        .unified-header {
            display: flex;
            justify-content: space-between;
            align-items: center;
            padding: 10px 20px;
            background-color: #252526;
            border-bottom: 1px solid #3e3e42;
            min-height: 50px;
            flex-shrink: 0;
        }

        .unified-header h1 {
            font-size: 18px;
            font-weight: 600;
            color: #cccccc;
            margin: 0;
            line-height: 1;
        }

        .unified-nav {
            display: flex;
            gap: 5px;
        }

        .unified-nav a {
            color: #999999;
            text-decoration: none;
            font-size: 12px;
            padding: 6px 12px;
            border-radius: 4px;
            transition: all 0.2s ease;
            display: flex;
            align-items: center;
            gap: 4px;
        }

        .unified-nav a:hover {
            background-color: #37373d;
            color: #007acc;
        }

        .unified-nav a.active {
            background-color: #007acc;
            color: white;
            font-weight: 600;
            border-left: 3px solid #007acc;
        }

        .docs-content {
            flex: 1;
            overflow-y: auto;
            padding: 2rem;
            line-height: 1.7;
            max-width: 900px;
            margin: 0 auto;
            width: 100%;
            background: #252526;
        }

        /* Markdown content styling */
        .docs-content h1,
        .docs-content h2,
        .docs-content h3,
        .docs-content h4,
        .docs-content h5,
        .docs-content h6 {
            color: #ffffff;
            margin-top: 2rem;
            margin-bottom: 1rem;
            font-weight: 600;
        }

        .docs-content h1 {
            border-bottom: 3px solid #007acc;
            padding-bottom: 0.5rem;
            margin-top: 0;
            font-size: 2.25rem;
        }

        .docs-content h2 {
            border-bottom: 2px solid #007acc;
            padding-bottom: 0.25rem;
            font-size: 1.875rem;
        }

        .docs-content h3 {
            font-size: 1.5rem;
        }

        .docs-content code {
            background: #2d2d30;
            padding: 0.125rem 0.375rem;
            border-radius: 4px;
            font-family: 'Monaco', 'Menlo', 'Ubuntu Mono', monospace;
            font-size: 0.875em;
            color: #d4d4d4;
        }

        .docs-content pre {
            background: #1e1e1e;
            padding: 1rem;
            border-radius: 4px;
            overflow-x: auto;
            border: 1px solid #3e3e42;
            margin: 1rem 0;
        }

        .docs-content pre code {
            background: none;
            padding: 0;
            font-size: 0.875em;
            color: #d4d4d4;
        }

        .docs-content blockquote {
            border-left: 4px solid #007acc;
            padding-left: 1rem;
            margin: 1.5rem 0;
            color: #999999;
            font-style: italic;
            background: #2d2d30;
            padding: 1rem 1rem 1rem 1.5rem;
            border-radius: 0 4px 4px 0;
        }

        .docs-content table {
            border-collapse: collapse;
            width: 100%;
            margin: 1.5rem 0;
            background: #1e1e1e;
            border-radius: 4px;
            overflow: hidden;
        }

        .docs-content th,
        .docs-content td {
            border: 1px solid #3e3e42;
            padding: 0.75rem;
            text-align: left;
            color: #cccccc;
        }

        .docs-content th {
            background: #2d2d30;
            font-weight: 600;
            color: #ffffff;
        }

        .docs-content tr:nth-child(even) {
            background: rgba(255, 255, 255, 0.03);
        }

        .docs-content a {
            color: #007acc;
            text-decoration: none;
            transition: all 0.2s ease;
        }

        .docs-content a:hover {
            text-decoration: underline;
            color: #4daafc;
        }

        .docs-content ul,
        .docs-content ol {
            padding-left: 1.5rem;
            margin: 1rem 0;
            color: #cccccc;
        }

        .docs-content li {
            margin: 0.5rem 0;
        }

        .docs-content p {
            margin-bottom: 1rem;
            color: #cccccc;
        }

        /* Error page styling */
        .error-container {
            text-align: center;
            padding: 3rem 2rem;
        }

        .error-container h1 {
            font-size: 3rem;
            color: #e74c3c;
            margin-bottom: 1rem;
        }

        .error-container p {
            font-size: 1.125rem;
            color: #cccccc;
            margin-bottom: 2rem;
        }

        .error-container a {
            display: inline-block;
            padding: 0.75rem 1.5rem;
            background: #007acc;
            color: white;
            text-decoration: none;
            border-radius: 4px;
            transition: background 0.2s ease;
        }

        .error-container a:hover {
            background: #005a9e;
        }

        /* Responsive adjustments */
        @media (max-width: 768px) {
            .docs-content {
                padding: 1rem;
            }

            .unified-nav {
                gap: 3px;
            }

            .unified-nav a {
                padding: 4px 8px;
                font-size: 11px;
            }
        }
    </style>
</head>
<body>
    <div class="docs-container">
        <header class="unified-header">
            <div class="header-left">
                <h1>aiwebengine</h1>
            </div>
            <nav class="unified-nav">
                <a href="/engine/docs" title="Documentation">📚 Documentation</a>
                <a href="/engine/editor" title="Code Editor">✏️ Editor</a>
                <a href="/engine/graphql" title="GraphQL API">🔗 GraphiQL</a>
                <a href="/engine/swagger" title="REST API">📖 Swagger</a>
            </nav>
        </header>
        <main class="docs-content">
            ${htmlContent}
        </main>
    </div>
    <script>
        (function() {
            const path = window.location.pathname;
            document.querySelectorAll('.unified-nav a').forEach(function(link) {
                const linkPath = new URL(link.href, window.location.origin).pathname;
                if (path.startsWith(linkPath)) {
                    link.classList.add('active');
                }
            });
        })();
    </script>
</body>
</html>`;
}

/**
 * Render 404 error page
 */
function render404Page() {
  const errorContent = `
    <div class="error-container">
        <h1>404</h1>
        <p>Documentation page not found</p>
        <a href="/engine/docs/">← Back to Documentation</a>
    </div>
  `;
  return wrapInTemplate(errorContent, "Page Not Found");
}

/**
 * Render 500 error page
 */
function render500Page(errorMessage) {
  const errorContent = `
    <div class="error-container">
        <h1>500</h1>
        <p>Error rendering documentation</p>
        <p style="font-size: 0.875rem; color: #e74c3c;">${errorMessage}</p>
        <a href="/engine/docs/">← Back to Documentation</a>
    </div>
  `;
  return wrapInTemplate(errorContent, "Server Error");
}

/**
 * Handle redirect from /engine/docs to /engine/docs/
 */
function handleDocsRedirect(context) {
  return {
    status: 301,
    headers: {
      Location: "/engine/docs/",
    },
    body: "",
    contentType: "text/html; charset=UTF-8",
  };
}

/**
 * Main documentation request handler
 */
function handleDocsRequest(context) {
  const req = getRequest(context);
  const path = req.path || "/engine/docs/";

  // Extract the doc path (remove /engine/docs prefix)
  let docPath = path.substring("/engine/docs".length);

  // Map path to asset name
  const assetName = mapPathToAssetName(docPath);

  console.log(
    "[docs.js] Requesting documentation: path=" +
      path +
      ", assetName=" +
      assetName,
  );

  // Fetch markdown from asset storage
  const assetContent = assetStorage.fetchAsset(assetName);

  if (
    assetContent === "null" ||
    assetContent === null ||
    assetContent.startsWith("Asset '") ||
    assetContent.startsWith("Error:")
  ) {
    console.log("[docs.js] Documentation not found: " + assetName);
    return {
      status: 404,
      body: render404Page(),
      contentType: "text/html; charset=UTF-8",
    };
  }

  try {
    // Decode base64 content (fetchAsset returns base64-encoded string directly)
    const markdown = decodeBase64(assetContent);

    // Convert markdown to HTML
    const htmlContent = convert.markdown_to_html(markdown);

    if (htmlContent.startsWith("Error:")) {
      console.error("[docs.js] Markdown conversion failed: " + htmlContent);
      return {
        status: 500,
        body: render500Page(htmlContent),
        contentType: "text/html; charset=UTF-8",
      };
    }

    // Extract title from markdown
    const title = extractTitle(markdown);

    // Wrap in template
    const fullPage = wrapInTemplate(htmlContent, title);

    return {
      status: 200,
      body: fullPage,
      contentType: "text/html; charset=UTF-8",
    };
  } catch (error) {
    console.error(
      "[docs.js] Error processing documentation: " + error.toString(),
    );
    return {
      status: 500,
      body: render500Page(error.toString()),
      contentType: "text/html; charset=UTF-8",
    };
  }
}

/**
 * Initialize documentation routes
 */
function init(context) {
  console.log(
    "[docs.js] Initializing documentation feature at " +
      new Date().toISOString(),
  );

  // Register redirect route
  routeRegistry.registerRoute("/engine/docs", "handleDocsRedirect", "GET", {
    summary: "Documentation redirect",
    description: "Redirects to /engine/docs/",
    tags: ["Documentation"],
  });

  // Register main documentation route
  routeRegistry.registerRoute("/engine/docs/", "handleDocsRequest", "GET", {
    summary: "Documentation home",
    description: "Main documentation page",
    tags: ["Documentation"],
  });

  // Register wildcard route for all doc pages
  routeRegistry.registerRoute("/engine/docs/*", "handleDocsRequest", "GET", {
    summary: "Documentation pages",
    description: "Serve documentation markdown as HTML",
    tags: ["Documentation"],
  });

  console.log("[docs.js] Documentation routes registered successfully");

  return {
    success: true,
    message: "Documentation feature initialized",
  };
}
