use pulldown_cmark::{Options, Parser, html};

/// Maximum size for markdown input (1MB)
const MAX_MARKDOWN_SIZE: usize = 1_000_000;

/// Convert markdown string to HTML
///
/// This function parses markdown using pulldown_cmark with the same options
/// used in the docs renderer: tables, footnotes, strikethrough, tasklists,
/// and heading attributes.
///
/// # Arguments
/// * `markdown` - The markdown string to convert
///
/// # Returns
/// * `Ok(String)` - The HTML output
/// * `Err(String)` - Error message if conversion fails
///
/// # Errors
/// * Returns error if markdown is empty
/// * Returns error if markdown exceeds 1MB size limit
pub fn convert_markdown_to_html(markdown: &str) -> Result<String, String> {
    // Validate input
    if markdown.is_empty() {
        return Err("Markdown input cannot be empty".to_string());
    }

    if markdown.len() > MAX_MARKDOWN_SIZE {
        return Err(format!(
            "Markdown input too large: {} bytes (max: {} bytes / 1MB)",
            markdown.len(),
            MAX_MARKDOWN_SIZE
        ));
    }

    // Set up parser options - same as docs.rs
    let mut options = Options::empty();
    options.insert(Options::ENABLE_TABLES);
    options.insert(Options::ENABLE_FOOTNOTES);
    options.insert(Options::ENABLE_STRIKETHROUGH);
    options.insert(Options::ENABLE_TASKLISTS);
    options.insert(Options::ENABLE_HEADING_ATTRIBUTES);

    // Parse markdown and convert to HTML
    let parser = Parser::new_ext(markdown, options);
    let mut html_output = String::new();
    html::push_html(&mut html_output, parser);

    Ok(html_output)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_convert_simple_text() {
        let markdown = "Hello, world!";
        let result = convert_markdown_to_html(markdown);
        assert!(result.is_ok());
        let html = result.unwrap();
        assert!(html.contains("<p>Hello, world!</p>"));
    }

    #[test]
    fn test_convert_heading() {
        let markdown = "# Heading 1\n## Heading 2";
        let result = convert_markdown_to_html(markdown);
        assert!(result.is_ok());
        let html = result.unwrap();
        assert!(html.contains("<h1>Heading 1</h1>"));
        assert!(html.contains("<h2>Heading 2</h2>"));
    }

    #[test]
    fn test_convert_list() {
        let markdown = "- Item 1\n- Item 2\n- Item 3";
        let result = convert_markdown_to_html(markdown);
        assert!(result.is_ok());
        let html = result.unwrap();
        assert!(html.contains("<ul>"));
        assert!(html.contains("<li>Item 1</li>"));
        assert!(html.contains("</ul>"));
    }

    #[test]
    fn test_convert_code_block() {
        let markdown = "```javascript\nconst x = 42;\n```";
        let result = convert_markdown_to_html(markdown);
        assert!(result.is_ok());
        let html = result.unwrap();
        assert!(html.contains("<pre><code"));
        assert!(html.contains("const x = 42;"));
        assert!(html.contains("</code></pre>"));
    }

    #[test]
    fn test_convert_table() {
        let markdown = "| Header 1 | Header 2 |\n|----------|----------|\n| Cell 1   | Cell 2   |";
        let result = convert_markdown_to_html(markdown);
        assert!(result.is_ok());
        let html = result.unwrap();
        assert!(html.contains("<table>"));
        assert!(html.contains("<th>Header 1</th>"));
        assert!(html.contains("<td>Cell 1</td>"));
        assert!(html.contains("</table>"));
    }

    #[test]
    fn test_convert_empty_input() {
        let result = convert_markdown_to_html("");
        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), "Markdown input cannot be empty");
    }

    #[test]
    fn test_convert_oversized_input() {
        let large_markdown = "# ".repeat(600_000); // > 1MB
        let result = convert_markdown_to_html(&large_markdown);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Markdown input too large"));
    }

    #[test]
    fn test_convert_inline_code() {
        let markdown = "This is `inline code` in text.";
        let result = convert_markdown_to_html(markdown);
        assert!(result.is_ok());
        let html = result.unwrap();
        assert!(html.contains("<code>inline code</code>"));
    }

    #[test]
    fn test_convert_bold_italic() {
        let markdown = "**bold** and *italic* text";
        let result = convert_markdown_to_html(markdown);
        assert!(result.is_ok());
        let html = result.unwrap();
        assert!(html.contains("<strong>bold</strong>"));
        assert!(html.contains("<em>italic</em>"));
    }

    #[test]
    fn test_convert_strikethrough() {
        let markdown = "~~strikethrough~~";
        let result = convert_markdown_to_html(markdown);
        assert!(result.is_ok());
        let html = result.unwrap();
        assert!(html.contains("<del>strikethrough</del>"));
    }

    #[test]
    fn test_convert_link() {
        let markdown = "[Example](https://example.com)";
        let result = convert_markdown_to_html(markdown);
        assert!(result.is_ok());
        let html = result.unwrap();
        assert!(html.contains("<a href=\"https://example.com\">Example</a>"));
    }
}
