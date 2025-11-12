use axum::response::Html;
use pulldown_cmark::{Options, Parser, html};
use std::fs;
use std::path::Path;

/// Convert Markdown content to HTML with basic styling
pub fn markdown_to_html(markdown: &str, title: &str) -> String {
    let mut options = Options::empty();
    options.insert(Options::ENABLE_TABLES);
    options.insert(Options::ENABLE_FOOTNOTES);
    options.insert(Options::ENABLE_STRIKETHROUGH);
    options.insert(Options::ENABLE_TASKLISTS);
    options.insert(Options::ENABLE_HEADING_ATTRIBUTES);

    let parser = Parser::new_ext(markdown, options);
    let mut html_output = String::new();
    html::push_html(&mut html_output, parser);

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
            background: linear-gradient(135deg, #f5f7fa 0%, #c3cfe2 100%);
            padding: 2rem 0;
        }}

        .docs-container {{
            max-width: 900px;
            margin: 0 auto;
            background: rgba(255, 255, 255, 0.95);
            backdrop-filter: blur(10px);
            border-radius: var(--border-radius-lg);
            box-shadow: var(--shadow-lg);
            overflow: hidden;
        }}

        .docs-header {{
            background: var(--bg-secondary);
            padding: 1.5rem 2rem;
            border-bottom: 1px solid var(--border-color);
        }}

        .docs-nav {{
            background: rgba(255, 255, 255, 0.9);
            padding: 1rem 2rem;
            border-bottom: 1px solid var(--border-color);
            display: flex;
            gap: 1rem;
            flex-wrap: wrap;
        }}

        .docs-nav a {{
            color: var(--primary-color);
            text-decoration: none;
            font-weight: 500;
            padding: 0.5rem 1rem;
            border-radius: var(--border-radius);
            transition: var(--transition);
        }}

        .docs-nav a:hover {{
            background: var(--bg-secondary);
            color: var(--primary-color);
        }}

        .docs-content {{
            padding: 2rem;
            line-height: 1.7;
        }}

        /* Markdown content styling */
        .docs-content h1,
        .docs-content h2,
        .docs-content h3,
        .docs-content h4,
        .docs-content h5,
        .docs-content h6 {{
            color: var(--text-color);
            margin-top: 2rem;
            margin-bottom: 1rem;
            font-weight: 600;
        }}

        .docs-content h1 {{
            border-bottom: 3px solid var(--primary-color);
            padding-bottom: 0.5rem;
            margin-top: 0;
            font-size: 2.25rem;
        }}

        .docs-content h2 {{
            border-bottom: 2px solid var(--primary-color);
            padding-bottom: 0.25rem;
            font-size: 1.875rem;
        }}

        .docs-content h3 {{
            font-size: 1.5rem;
        }}

        .docs-content code {{
            background: var(--bg-secondary);
            padding: 0.125rem 0.375rem;
            border-radius: var(--border-radius-sm);
            font-family: 'Monaco', 'Menlo', 'Ubuntu Mono', monospace;
            font-size: 0.875em;
            color: var(--text-color);
        }}

        .docs-content pre {{
            background: var(--bg-secondary);
            padding: 1rem;
            border-radius: var(--border-radius);
            overflow-x: auto;
            border: 1px solid var(--border-color);
            margin: 1rem 0;
        }}

        .docs-content pre code {{
            background: none;
            padding: 0;
            font-size: 0.875em;
        }}

        .docs-content blockquote {{
            border-left: 4px solid var(--primary-color);
            padding-left: 1rem;
            margin: 1.5rem 0;
            color: var(--text-muted);
            font-style: italic;
            background: var(--bg-secondary);
            padding: 1rem 1rem 1rem 1.5rem;
            border-radius: 0 var(--border-radius) var(--border-radius) 0;
        }}

        .docs-content table {{
            border-collapse: collapse;
            width: 100%;
            margin: 1.5rem 0;
            background: var(--bg-color);
            border-radius: var(--border-radius);
            overflow: hidden;
            box-shadow: var(--shadow);
        }}

        .docs-content th,
        .docs-content td {{
            border: 1px solid var(--border-color);
            padding: 0.75rem;
            text-align: left;
        }}

        .docs-content th {{
            background: var(--bg-secondary);
            font-weight: 600;
            color: var(--text-color);
        }}

        .docs-content tr:nth-child(even) {{
            background: rgba(0, 0, 0, 0.02);
        }}

        .docs-content a {{
            color: var(--primary-color);
            text-decoration: none;
            transition: var(--transition);
        }}

        .docs-content a:hover {{
            text-decoration: underline;
            color: var(--primary-color);
        }}

        .docs-content ul,
        .docs-content ol {{
            padding-left: 1.5rem;
            margin: 1rem 0;
        }}

        .docs-content li {{
            margin: 0.5rem 0;
        }}

        .docs-content p {{
            margin-bottom: 1rem;
        }}

        /* Responsive adjustments */
        @media (max-width: 768px) {{
            .docs-container {{
                margin: 1rem;
                max-width: none;
            }}

            .docs-content {{
                padding: 1rem;
            }}

            .docs-nav {{
                padding: 0.75rem 1rem;
            }}

            .docs-nav a {{
                padding: 0.375rem 0.75rem;
                font-size: 0.875rem;
            }}
        }}
    </style>
</head>
<body>
    <div class="docs-container">
        <header class="docs-header">
            <h1>{}</h1>
        </header>
        <nav class="docs-nav">
            <a href="/engine/docs/">📚 Documentation Home</a>
            <a href="/">🏠 Home</a>
            <a href="/engine/editor">✏️ Editor</a>
            <a href="/engine/admin">👥 User Manager</a>
        </nav>
        <main class="docs-content">
            {}
        </main>
    </div>
</body>
</html>"#,
        title, title, html_output
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
