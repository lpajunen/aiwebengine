use axum::response::Html;
use std::fs;
use std::path::Path;

/// Convert Markdown content to HTML with basic styling
pub fn markdown_to_html(markdown: &str, title: &str) -> String {
    // Use shared conversion logic to get HTML body
    let html_output = crate::conversion::convert_markdown_to_html(markdown)
        .unwrap_or_else(|e| format!("<p>Error converting markdown: {}</p>", e));

    format!(
        r#"<!DOCTYPE html>
<html lang="en">
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <title>{}</title>
    <link rel="stylesheet" href="/engine.css">
    <link rel="icon" type="image/x-icon" href="/favicon.ico">
    <style>
        /* Documentation-specific overrides */
        body {{
            background-color: #1e1e1e;
            padding: 0;
            margin: 0;
            height: 100vh;
            overflow: hidden;
        }}

        .docs-container {{
            display: flex;
            flex-direction: column;
            height: 100vh;
            background: #1e1e1e;
            overflow: hidden;
        }}

        /* Unified header styles inherited from editor.css */
        .unified-header {{
            display: flex;
            justify-content: space-between;
            align-items: center;
            padding: 10px 20px;
            background-color: #252526;
            border-bottom: 1px solid #3e3e42;
            height: 50px;
            flex-shrink: 0;
        }}

        .unified-header h1 {{
            font-size: 18px;
            font-weight: 600;
            color: #cccccc;
            margin: 0;
        }}

        .unified-nav {{
            display: flex;
            gap: 5px;
        }}

        .unified-nav a {{
            color: #999999;
            text-decoration: none;
            font-size: 12px;
            padding: 6px 12px;
            border-radius: 4px;
            transition: all 0.2s ease;
            display: flex;
            align-items: center;
            gap: 4px;
        }}

        .unified-nav a:hover {{
            background-color: #37373d;
            color: #007acc;
        }}

        .unified-nav a.active {{
            background-color: #007acc;
            color: white;
            font-weight: 600;
            border-left: 3px solid #007acc;
        }}

        .docs-content {{
            flex: 1;
            overflow-y: auto;
            padding: 2rem;
            line-height: 1.7;
            max-width: 900px;
            margin: 0 auto;
            width: 100%;
            background: #252526;
        }}

        /* Markdown content styling */
        .docs-content h1,
        .docs-content h2,
        .docs-content h3,
        .docs-content h4,
        .docs-content h5,
        .docs-content h6 {{
            color: #ffffff;
            margin-top: 2rem;
            margin-bottom: 1rem;
            font-weight: 600;
        }}

        .docs-content h1 {{
            border-bottom: 3px solid #007acc;
            padding-bottom: 0.5rem;
            margin-top: 0;
            font-size: 2.25rem;
        }}

        .docs-content h2 {{
            border-bottom: 2px solid #007acc;
            padding-bottom: 0.25rem;
            font-size: 1.875rem;
        }}

        .docs-content h3 {{
            font-size: 1.5rem;
        }}

        .docs-content code {{
            background: #2d2d30;
            padding: 0.125rem 0.375rem;
            border-radius: 4px;
            font-family: 'Monaco', 'Menlo', 'Ubuntu Mono', monospace;
            font-size: 0.875em;
            color: #d4d4d4;
        }}

        .docs-content pre {{
            background: #1e1e1e;
            padding: 1rem;
            border-radius: 4px;
            overflow-x: auto;
            border: 1px solid #3e3e42;
            margin: 1rem 0;
        }}

        .docs-content pre code {{
            background: none;
            padding: 0;
            font-size: 0.875em;
            color: #d4d4d4;
        }}

        .docs-content blockquote {{
            border-left: 4px solid #007acc;
            padding-left: 1rem;
            margin: 1.5rem 0;
            color: #999999;
            font-style: italic;
            background: #2d2d30;
            padding: 1rem 1rem 1rem 1.5rem;
            border-radius: 0 4px 4px 0;
        }}

        .docs-content table {{
            border-collapse: collapse;
            width: 100%;
            margin: 1.5rem 0;
            background: #1e1e1e;
            border-radius: 4px;
            overflow: hidden;
        }}

        .docs-content th,
        .docs-content td {{
            border: 1px solid #3e3e42;
            padding: 0.75rem;
            text-align: left;
            color: #cccccc;
        }}

        .docs-content th {{
            background: #2d2d30;
            font-weight: 600;
            color: #ffffff;
        }}

        .docs-content tr:nth-child(even) {{
            background: rgba(255, 255, 255, 0.03);
        }}

        .docs-content a {{
            color: #007acc;
            text-decoration: none;
            transition: all 0.2s ease;
        }}

        .docs-content a:hover {{
            text-decoration: underline;
            color: #4daafc;
        }}

        .docs-content ul,
        .docs-content ol {{
            padding-left: 1.5rem;
            margin: 1rem 0;
            color: #cccccc;
        }}

        .docs-content li {{
            margin: 0.5rem 0;
        }}

        .docs-content p {{
            margin-bottom: 1rem;
            color: #cccccc;
        }}

        /* Responsive adjustments */
        @media (max-width: 768px) {{
            .docs-content {{
                padding: 1rem;
            }}

            .unified-nav {{
                gap: 3px;
            }}

            .unified-nav a {{
                padding: 4px 8px;
                font-size: 11px;
            }}
        }}
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
            {}
        </main>
    </div>
    <script>
        (function() {{
            const path = window.location.pathname;
            document.querySelectorAll('.unified-nav a').forEach(function(link) {{
                const linkPath = new URL(link.href, window.location.origin).pathname;
                if (path.startsWith(linkPath)) {{
                    link.classList.add('active');
                }}
            }});
        }})();
    </script>
</body>
</html>"#,
        title, html_output
    )
}

/// Handle docs requests by converting Markdown files to HTML
pub async fn handle_docs_request(
    path: Option<axum::extract::Path<String>>,
) -> Result<Html<String>, (axum::http::StatusCode, String)> {
    // Extract the path string, defaulting to empty string if no path parameter
    let path_str = path.map(|p| p.0).unwrap_or_default();

    // If it's the root docs path, serve the main README
    let file_path = if path_str.is_empty() || path_str == "index" || path_str == "index.md" {
        "docs/solution-developers/README.md".to_string()
    } else {
        // Map the URL path to the file system path
        if path_str.ends_with(".md") {
            format!("docs/solution-developers/{}", path_str)
        } else if path_str.ends_with("/") {
            format!(
                "docs/solution-developers/{}/README.md",
                path_str.trim_end_matches('/')
            )
        } else {
            // Try both with and without .md extension
            let with_md = format!("docs/solution-developers/{}.md", path_str);
            let as_dir = format!("docs/solution-developers/{}/README.md", path_str);

            if Path::new(&with_md).exists() {
                with_md
            } else if Path::new(&as_dir).exists() {
                as_dir
            } else {
                return Err((
                    axum::http::StatusCode::NOT_FOUND,
                    format!("Documentation file not found: {}", path_str),
                ));
            }
        }
    };

    // Read the Markdown file
    let markdown_content = match fs::read_to_string(&file_path) {
        Ok(content) => content,
        Err(e) => {
            return Err((
                axum::http::StatusCode::NOT_FOUND,
                format!("Failed to read documentation file {}: {}", file_path, e),
            ));
        }
    };

    // Extract title from the first heading or use filename
    let title = if let Some(first_line) = markdown_content.lines().next() {
        if first_line.starts_with("# ") {
            first_line.trim_start_matches("# ").to_string()
        } else {
            // Use filename as fallback
            Path::new(&file_path)
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("Documentation")
                .to_string()
        }
    } else {
        "Documentation".to_string()
    };

    // Convert to HTML
    let html_content = markdown_to_html(&markdown_content, &title);

    Ok(Html(html_content))
}
