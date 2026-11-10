use base64::Engine;
use handlebars::Handlebars;
use pulldown_cmark::{CowStr, Event, Options, Parser, Tag, TagEnd, html};
use serde::Deserialize;

/// Maximum size for markdown input (1MB)
pub const MAX_MARKDOWN_SIZE: usize = 1_000_000;

/// Maximum size for Handlebars template input (1MB)
pub const MAX_TEMPLATE_SIZE: usize = 1_000_000;

/// What a script may ask of `markdown_to_html`. Unknown keys are refused, so a
/// typo is not silently a default.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MarkdownOptions {
    /// Pass raw HTML, any link or image URL, and heading attributes through
    /// as written. Only for Markdown the script trusts as much as its own code.
    #[serde(default)]
    pub html: bool,
}

/// Convert markdown string to HTML
///
/// This function parses markdown using pulldown_cmark with the same options
/// used in the docs renderer: tables, footnotes, strikethrough, tasklists,
/// and heading attributes.
///
/// Unless `options.html` is set, the output is safe to put in a page whatever
/// the Markdown says: raw HTML comes out as text, a link or image whose URL
/// is not http(s), `mailto:` or relative keeps only its text, and headings
/// keep their id and classes but no other attributes.
///
/// # Errors
/// * Returns error if markdown is empty
/// * Returns error if markdown exceeds 1MB size limit
pub fn convert_markdown_to_html(
    markdown: &str,
    options: &MarkdownOptions,
) -> Result<String, String> {
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
    let mut parser_options = Options::empty();
    parser_options.insert(Options::ENABLE_TABLES);
    parser_options.insert(Options::ENABLE_FOOTNOTES);
    parser_options.insert(Options::ENABLE_STRIKETHROUGH);
    parser_options.insert(Options::ENABLE_TASKLISTS);
    parser_options.insert(Options::ENABLE_HEADING_ATTRIBUTES);

    // Parse markdown and convert to HTML
    let parser = Parser::new_ext(markdown, parser_options);
    let mut html_output = String::new();
    if options.html {
        html::push_html(&mut html_output, parser);
    } else {
        html::push_html(&mut html_output, safe_events(parser));
    }

    Ok(html_output)
}

/// The events of `parser` with everything that could run script removed.
/// An image's description may hold a link (or another image), so whether
/// each open link or image was dropped is kept on a stack, and its end goes
/// the same way as its start.
fn safe_events<'a>(parser: Parser<'a>) -> impl Iterator<Item = Event<'a>> {
    let mut dropped: Vec<bool> = Vec::new();
    parser.filter_map(move |event| match event {
        Event::Html(raw) | Event::InlineHtml(raw) => Some(Event::Text(raw)),
        Event::Start(Tag::Link { ref dest_url, .. } | Tag::Image { ref dest_url, .. }) => {
            let drop = !safe_url(dest_url);
            dropped.push(drop);
            (!drop).then_some(event)
        }
        Event::End(TagEnd::Link | TagEnd::Image) => {
            let drop = dropped.pop().unwrap_or(false);
            (!drop).then_some(event)
        }
        Event::Start(Tag::Heading {
            level, id, classes, ..
        }) => Some(Event::Start(Tag::Heading {
            level,
            id,
            classes,
            attrs: Vec::new(),
        })),
        other => Some(other),
    })
}

/// Whether a link or image URL may be followed: http(s), `mailto:`, or one
/// with no scheme (a path, a fragment, `//host`). Browsers ignore tabs and
/// newlines inside a scheme, so they are ignored here too.
fn safe_url(url: &CowStr<'_>) -> bool {
    let cleaned: String = url
        .chars()
        .filter(|c| !c.is_ascii_whitespace() && !c.is_ascii_control())
        .collect();
    match cleaned.find([':', '/', '?', '#']) {
        Some(at) if cleaned[at..].starts_with(':') => {
            let scheme = cleaned[..at].to_ascii_lowercase();
            matches!(scheme.as_str(), "http" | "https" | "mailto")
        }
        _ => true,
    }
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

/// Base64 encode a string
///
/// # Arguments
/// * `input` - The string to encode
///
/// # Returns
/// * `Ok(String)` - Base64-encoded string
/// * `Err(String)` - Error message if encoding fails
pub fn convert_btoa(input: &str) -> Result<String, String> {
    Ok(base64::engine::general_purpose::STANDARD.encode(input.as_bytes()))
}

/// Base64 decode a string
///
/// # Arguments
/// * `input` - The base64-encoded string
///
/// # Returns
/// * `Ok(String)` - Decoded UTF-8 string
/// * `Err(String)` - Error message if decoding fails
pub fn convert_atob(input: &str) -> Result<String, String> {
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(input.as_bytes())
        .map_err(|e| format!("Invalid base64 input: {}", e))?;

    String::from_utf8(decoded).map_err(|e| format!("Decoded data is not valid UTF-8: {}", e))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_convert_simple_text() {
        let markdown = "Hello, world!";
        let result = convert_markdown_to_html(markdown, &MarkdownOptions::default());
        assert!(result.is_ok());
        let html = result.unwrap();
        assert!(html.contains("<p>Hello, world!</p>"));
    }

    #[test]
    fn test_convert_heading() {
        let markdown = "# Heading 1\n## Heading 2";
        let result = convert_markdown_to_html(markdown, &MarkdownOptions::default());
        assert!(result.is_ok());
        let html = result.unwrap();
        assert!(html.contains("<h1>Heading 1</h1>"));
        assert!(html.contains("<h2>Heading 2</h2>"));
    }

    #[test]
    fn test_convert_list() {
        let markdown = "- Item 1\n- Item 2\n- Item 3";
        let result = convert_markdown_to_html(markdown, &MarkdownOptions::default());
        assert!(result.is_ok());
        let html = result.unwrap();
        assert!(html.contains("<ul>"));
        assert!(html.contains("<li>Item 1</li>"));
        assert!(html.contains("</ul>"));
    }

    #[test]
    fn test_convert_code_block() {
        let markdown = "```javascript\nconst x = 42;\n```";
        let result = convert_markdown_to_html(markdown, &MarkdownOptions::default());
        assert!(result.is_ok());
        let html = result.unwrap();
        assert!(html.contains("<pre><code"));
        assert!(html.contains("const x = 42;"));
        assert!(html.contains("</code></pre>"));
    }

    #[test]
    fn test_convert_table() {
        let markdown = "| Header 1 | Header 2 |\n|----------|----------|\n| Cell 1   | Cell 2   |";
        let result = convert_markdown_to_html(markdown, &MarkdownOptions::default());
        assert!(result.is_ok());
        let html = result.unwrap();
        assert!(html.contains("<table>"));
        assert!(html.contains("<th>Header 1</th>"));
        assert!(html.contains("<td>Cell 1</td>"));
        assert!(html.contains("</table>"));
    }

    #[test]
    fn test_convert_empty_input() {
        let result = convert_markdown_to_html("", &MarkdownOptions::default());
        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), "Markdown input cannot be empty");
    }

    #[test]
    fn test_convert_oversized_input() {
        let large_markdown = "# ".repeat(600_000); // > 1MB
        let result = convert_markdown_to_html(&large_markdown, &MarkdownOptions::default());
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Markdown input too large"));
    }

    #[test]
    fn test_convert_inline_code() {
        let markdown = "This is `inline code` in text.";
        let result = convert_markdown_to_html(markdown, &MarkdownOptions::default());
        assert!(result.is_ok());
        let html = result.unwrap();
        assert!(html.contains("<code>inline code</code>"));
    }

    #[test]
    fn test_convert_bold_italic() {
        let markdown = "**bold** and *italic* text";
        let result = convert_markdown_to_html(markdown, &MarkdownOptions::default());
        assert!(result.is_ok());
        let html = result.unwrap();
        assert!(html.contains("<strong>bold</strong>"));
        assert!(html.contains("<em>italic</em>"));
    }

    #[test]
    fn test_convert_strikethrough() {
        let markdown = "~~strikethrough~~";
        let result = convert_markdown_to_html(markdown, &MarkdownOptions::default());
        assert!(result.is_ok());
        let html = result.unwrap();
        assert!(html.contains("<del>strikethrough</del>"));
    }

    #[test]
    fn test_convert_link() {
        let markdown = "[Example](https://example.com)";
        let result = convert_markdown_to_html(markdown, &MarkdownOptions::default());
        assert!(result.is_ok());
        let html = result.unwrap();
        assert!(html.contains("<a href=\"https://example.com\">Example</a>"));
    }

    fn safe(markdown: &str) -> String {
        convert_markdown_to_html(markdown, &MarkdownOptions::default()).unwrap()
    }

    fn trusted(markdown: &str) -> String {
        convert_markdown_to_html(markdown, &MarkdownOptions { html: true }).unwrap()
    }

    #[test]
    fn test_raw_html_is_text_by_default() {
        let html = safe("<script>alert(1)</script>\n\nHi <img src=x onerror=alert(1)> there");
        assert!(!html.contains("<script"), "{html}");
        assert!(!html.contains("<img"), "{html}");
        assert!(
            html.contains("&lt;script&gt;alert(1)&lt;/script&gt;"),
            "{html}"
        );
        assert!(
            html.contains("Hi &lt;img src=x onerror=alert(1)&gt; there"),
            "{html}"
        );
    }

    #[test]
    fn test_unsafe_urls_keep_only_their_text() {
        for url in [
            "javascript:alert(1)",
            "JavaScript:alert(1)",
            "java\tscript:alert(1)",
            "data:text/html,<b>x</b>",
            "vbscript:x",
        ] {
            let html = safe(&format!("[click]({url}) and ![pic]({url})"));
            assert!(!html.contains("<a"), "{url}: {html}");
            assert!(!html.contains("<img"), "{url}: {html}");
            assert!(html.contains("click"), "{url}: {html}");
            assert!(html.contains("pic"), "{url}: {html}");
        }
        let auto = safe("<javascript:alert(1)>");
        assert!(!auto.contains("<a"), "{auto}");
    }

    #[test]
    fn test_safe_urls_are_kept() {
        for url in [
            "https://example.com/a",
            "http://example.com",
            "mailto:a@example.com",
            "/app/board.svg",
            "board.svg",
            "#section",
            "?page=2",
            "a/b:c",
        ] {
            let html = safe(&format!("[x]({url})"));
            assert!(html.contains("<a href="), "{url}: {html}");
        }
        assert!(safe("![board](/chess/board.svg)").contains("<img src=\"/chess/board.svg\""));
    }

    #[test]
    fn test_link_inside_image_description() {
        let html = safe("![a [b](javascript:x) c](javascript:y)");
        assert!(!html.contains("<a"), "{html}");
        assert!(!html.contains("<img"), "{html}");
        assert!(!html.contains("javascript"), "{html}");
        let kept = safe("![a [b](javascript:x) c](/ok.png)");
        assert!(kept.contains("<img src=\"/ok.png\""), "{kept}");
        assert!(!kept.contains("javascript"), "{kept}");
    }

    #[test]
    fn test_heading_attributes_are_dropped_but_id_and_classes_kept() {
        let html = safe("# Title {#top .wide onclick=alert(1)}");
        assert!(!html.contains("onclick"), "{html}");
        assert!(html.contains("id=\"top\""), "{html}");
        assert!(html.contains("class=\"wide\""), "{html}");
    }

    #[test]
    fn test_html_option_passes_everything_through() {
        assert!(trusted("<div class=\"x\">hi</div>").contains("<div class=\"x\">hi</div>"));
        assert!(trusted("[x](javascript:y)").contains("href=\"javascript:y\""));
        assert!(trusted("# T {onclick=z}").contains("onclick"));
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
