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
    <style>
        body {{
            font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, sans-serif;
            line-height: 1.6;
            color: #333;
            max-width: 800px;
            margin: 0 auto;
            padding: 20px;
            background-color: #f8f9fa;
        }}
        .container {{
            background: white;
            padding: 30px;
            border-radius: 8px;
            box-shadow: 0 2px 10px rgba(0,0,0,0.1);
        }}
        h1, h2, h3, h4, h5, h6 {{
            color: #2c3e50;
            margin-top: 1.5em;
            margin-bottom: 0.5em;
        }}
        h1 {{
            border-bottom: 2px solid #3498db;
            padding-bottom: 10px;
            margin-top: 0;
        }}
        code {{
            background: #f1f3f4;
            padding: 2px 6px;
            border-radius: 3px;
            font-family: 'Monaco', 'Menlo', 'Ubuntu Mono', monospace;
            font-size: 0.9em;
        }}
        pre {{
            background: #f8f9fa;
            padding: 15px;
            border-radius: 5px;
            overflow-x: auto;
            border: 1px solid #e9ecef;
        }}
        pre code {{
            background: none;
            padding: 0;
        }}
        blockquote {{
            border-left: 4px solid #3498db;
            padding-left: 15px;
            margin: 15px 0;
            color: #555;
            font-style: italic;
        }}
        table {{
            border-collapse: collapse;
            width: 100%;
            margin: 15px 0;
        }}
        th, td {{
            border: 1px solid #ddd;
            padding: 8px 12px;
            text-align: left;
        }}
        th {{
            background-color: #f8f9fa;
            font-weight: bold;
        }}
        tr:nth-child(even) {{
            background-color: #f9f9f9;
        }}
        a {{
            color: #3498db;
            text-decoration: none;
        }}
        a:hover {{
            text-decoration: underline;
        }}
        .nav {{
            margin-bottom: 20px;
            padding: 10px;
            background: #e3f2fd;
            border-radius: 5px;
        }}
        .nav a {{
            margin-right: 15px;
            font-weight: bold;
        }}
        ul, ol {{
            padding-left: 20px;
        }}
        li {{
            margin: 5px 0;
        }}
    </style>
</head>
<body>
    <div class="container">
        <div class="nav">
            <a href="/docs/">📚 Documentation Home</a>
            <a href="/">🏠 Home</a>
        </div>
        {}
    </div>
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
