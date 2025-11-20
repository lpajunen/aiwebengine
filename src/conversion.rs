use handlebars::Handlebars;
use pulldown_cmark::{Options, Parser, html};

/// Maximum size for markdown input (1MB)
const MAX_MARKDOWN_SIZE: usize = 1_000_000;

/// Maximum size for Handlebars template input (1MB)
const MAX_TEMPLATE_SIZE: usize = 1_000_000;

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

/// Render Handlebars template with data
///
/// This function compiles and renders a Handlebars template with the provided data object.
/// The data should be a JSON-serializable object that will be used to populate template variables.
///
/// # Arguments
/// * `template` - The Handlebars template string
/// * `data` - JSON string representing the data object to render with
///
/// # Returns
/// * `Ok(String)` - The rendered template output
/// * `Err(String)` - Error message if rendering fails
///
/// # Errors
/// * Returns error if template is empty
/// * Returns error if template exceeds 1MB size limit
/// * Returns error if data is not valid JSON
/// * Returns error if template compilation or rendering fails
pub fn render_handlebars_template(template: &str, data: &str) -> Result<String, String> {
    // Validate template input
    if template.is_empty() {
        return Err("Template input cannot be empty".to_string());
    }

    if template.len() > MAX_TEMPLATE_SIZE {
        return Err(format!(
            "Template input too large: {} bytes (max: {} bytes / 1MB)",
            template.len(),
            MAX_TEMPLATE_SIZE
        ));
    }

    // Parse data as JSON
    let data_value: serde_json::Value =
        serde_json::from_str(data).map_err(|e| format!("Invalid JSON data: {}", e))?;

    // Create a new Handlebars instance
    let mut handlebars = Handlebars::new();

    // Register the template (using a temporary name)
    handlebars
        .register_template_string("template", template)
        .map_err(|e| format!("Template compilation error: {}", e))?;

    // Render the template with the data
    let result = handlebars
        .render("template", &data_value)
        .map_err(|e| format!("Template rendering error: {}", e))?;

    Ok(result)
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

    #[test]
    fn test_render_handlebars_simple() {
        let template = "Hello {{name}}!";
        let data = r#"{"name": "World"}"#;
        let result = render_handlebars_template(template, data);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "Hello World!");
    }

    #[test]
    fn test_render_handlebars_complex() {
        let template = "<h1>{{title}}</h1><p>{{content}}</p><ul>{{#each items}}<li>{{this}}</li>{{/each}}</ul>";
        let data = r#"{"title": "Test Page", "content": "This is a test", "items": ["Item 1", "Item 2", "Item 3"]}"#;
        let result = render_handlebars_template(template, data);
        assert!(result.is_ok());
        let output = result.unwrap();
        assert!(output.contains("<h1>Test Page</h1>"));
        assert!(output.contains("<p>This is a test</p>"));
        assert!(output.contains("<li>Item 1</li>"));
        assert!(output.contains("<li>Item 2</li>"));
        assert!(output.contains("<li>Item 3</li>"));
    }

    #[test]
    fn test_render_handlebars_empty_template() {
        let result = render_handlebars_template("", r#"{"name": "test"}"#);
        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), "Template input cannot be empty");
    }

    #[test]
    fn test_render_handlebars_invalid_json() {
        let template = "Hello {{name}}!";
        let data = r#"{"name": "invalid json"#; // Missing closing brace
        let result = render_handlebars_template(template, data);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Invalid JSON data"));
    }

    #[test]
    fn test_render_handlebars_template_error() {
        let template = "Hello {{#if}}{{/if"; // Malformed template
        let data = r#"{"name": "World"}"#;
        let result = render_handlebars_template(template, data);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Template compilation error"));
    }
}
