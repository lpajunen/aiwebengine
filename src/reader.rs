//! `convert.parse_feed` and `convert.extract_article`: what a script needs to
//! read the web as data — a feed's items, and an article page's text.
//!
//! Both answer text, never markup. HTML inside a feed and an article's body
//! are reduced to plain text or Markdown here, so nothing either returns can
//! carry a script into a page that shows it.

use serde::{Deserialize, Serialize};

/// Input to either: what one `fetch` may return, so any response can be read.
pub const MAX_READER_INPUT_BYTES: usize = crate::http_client::MAX_RESPONSE_SIZE;

/// Elements in a page `extract_article` will score. A news article is a few
/// thousand; scoring is the slow part, so a page far past that is refused
/// instead of holding the execution slot.
pub const MAX_ARTICLE_ELEMENTS: usize = 50_000;

#[derive(Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Feed {
    pub format: &'static str,
    pub title: Option<String>,
    pub description: Option<String>,
    pub link: Option<String>,
    pub language: Option<String>,
    pub updated: Option<String>,
    pub items: Vec<FeedItem>,
}

#[derive(Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct FeedItem {
    /// The feed's own id (`guid`, Atom `id`), or one derived from the item
    /// when it has none: stable across reads, so it can key what was seen.
    pub id: String,
    pub title: Option<String>,
    pub link: Option<String>,
    pub published: Option<String>,
    pub updated: Option<String>,
    pub summary: Option<String>,
    pub content: Option<String>,
    pub authors: Vec<String>,
    pub categories: Vec<String>,
    pub image: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ParseFeedOptions {
    /// Resolves relative links, for a feed that has no `xml:base` of its own.
    pub base_url: Option<String>,
}

pub fn parse_feed(source: &str, options: &ParseFeedOptions) -> Result<Feed, String> {
    if source.trim().is_empty() {
        return Err("the feed is empty".to_string());
    }
    if source.len() > MAX_READER_INPUT_BYTES {
        return Err(format!(
            "the feed is {} bytes, over the {} byte limit",
            source.len(),
            MAX_READER_INPUT_BYTES
        ));
    }
    let feed = feed_rs::parser::Builder::new()
        .base_uri(options.base_url.as_deref())
        .build()
        .parse(source.as_bytes())
        .map_err(|e| format!("not a feed: {}", e))?;

    Ok(Feed {
        format: match feed.feed_type {
            feed_rs::model::FeedType::Atom => "atom",
            feed_rs::model::FeedType::JSON => "json",
            feed_rs::model::FeedType::RSS0 => "rss0",
            feed_rs::model::FeedType::RSS1 => "rss1",
            feed_rs::model::FeedType::RSS2 => "rss2",
        },
        title: feed.title.as_ref().and_then(text_of),
        description: feed.description.as_ref().and_then(text_of),
        link: page_link(&feed.links),
        language: feed.language,
        updated: feed.updated.map(|t| t.to_rfc3339()),
        items: feed.entries.into_iter().map(item_of).collect(),
    })
}

fn item_of(entry: feed_rs::model::Entry) -> FeedItem {
    let content = entry
        .content
        .as_ref()
        .and_then(|c| c.body.as_deref())
        .and_then(|body| non_empty(html_to_text(body)));
    // MediaRSS first, then an image enclosure, then the first `<img>` in the
    // item's own HTML. feed-rs drops a `media:thumbnail` that has no
    // `media:content` beside it, which is where the last one earns its place.
    let image = entry
        .media
        .iter()
        .flat_map(|m| m.thumbnails.iter().map(|t| t.image.uri.clone()))
        .chain(entry.media.iter().flat_map(|m| {
            m.content.iter().filter_map(|c| {
                let image = c.content_type.as_ref().is_some_and(|t| t.ty() == "image");
                image
                    .then(|| c.url.as_ref().map(|u| u.to_string()))
                    .flatten()
            })
        }))
        .next()
        .or_else(|| {
            let content = entry.content.as_ref().and_then(|c| c.body.as_deref());
            let summary = entry.summary.as_ref().map(|t| t.content.as_str());
            content.into_iter().chain(summary).find_map(first_image)
        });

    FeedItem {
        id: entry.id,
        title: entry.title.as_ref().and_then(text_of),
        link: page_link(&entry.links),
        published: entry.published.map(|t| t.to_rfc3339()),
        updated: entry.updated.map(|t| t.to_rfc3339()),
        summary: entry.summary.as_ref().and_then(text_of),
        content,
        authors: entry.authors.into_iter().filter_map(|p| p.name).collect(),
        categories: entry
            .categories
            .into_iter()
            .map(|c| c.label.unwrap_or(c.term))
            .collect(),
        image,
    }
}

/// The link a reader follows: the `alternate` one (an RSS `<link>` is one),
/// else the first that is not `self`, comments or an enclosure.
fn page_link(links: &[feed_rs::model::Link]) -> Option<String> {
    let alternate = links
        .iter()
        .find(|l| l.rel.as_deref().is_none_or(|r| r == "alternate") && l.target.is_none());
    alternate
        .or_else(|| {
            links.iter().find(|l| {
                l.target.is_none()
                    && !matches!(l.rel.as_deref(), Some("self" | "enclosure" | "replies"))
            })
        })
        .map(|l| l.href.clone())
}

/// Every text field goes through the HTML reader, whatever type it claims:
/// an RSS `<description>` says nothing about its type and is escaped HTML
/// more often than not.
fn text_of(text: &feed_rs::model::Text) -> Option<String> {
    non_empty(html_to_text(&text.content))
}

fn html_to_text(html: &str) -> String {
    let text = dom_query::Document::fragment(html).formatted_text();
    text.trim().to_string()
}

fn first_image(html: &str) -> Option<String> {
    let doc = dom_query::Document::fragment(html);
    let src = doc.select("img[src]").attr("src")?;
    let src = src.trim();
    src.starts_with("http").then(|| src.to_string())
}

fn non_empty(s: String) -> Option<String> {
    if s.is_empty() { None } else { Some(s) }
}

#[derive(Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Article {
    pub title: String,
    pub byline: Option<String>,
    pub site_name: Option<String>,
    pub language: Option<String>,
    pub published: Option<String>,
    pub excerpt: Option<String>,
    pub image: Option<String>,
    /// The article's body in `format`.
    pub text: String,
}

#[derive(Debug, Default, Deserialize, Clone, Copy, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum ArticleFormat {
    #[default]
    Markdown,
    Text,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default, deny_unknown_fields)]
pub struct ExtractArticleOptions {
    /// The page's own URL: resolves relative links and images. Absolute.
    pub url: Option<String>,
    pub format: ArticleFormat,
}

/// `Ok(None)` when the page holds no article — an index, a login wall, a
/// page built in the browser. That is an answer about the page, not a fault.
pub fn extract_article(
    html: &str,
    options: &ExtractArticleOptions,
) -> Result<Option<Article>, String> {
    if html.trim().is_empty() {
        return Err("the page is empty".to_string());
    }
    if html.len() > MAX_READER_INPUT_BYTES {
        return Err(format!(
            "the page is {} bytes, over the {} byte limit",
            html.len(),
            MAX_READER_INPUT_BYTES
        ));
    }
    let config = dom_smoothie::Config {
        max_elements_to_parse: MAX_ARTICLE_ELEMENTS,
        text_mode: match options.format {
            ArticleFormat::Markdown => dom_smoothie::TextMode::Markdown,
            ArticleFormat::Text => dom_smoothie::TextMode::Formatted,
        },
        ..Default::default()
    };
    let mut readability =
        dom_smoothie::Readability::new(html, options.url.as_deref(), Some(config)).map_err(
            |e| match e {
                dom_smoothie::ReadabilityError::BadDocumentURL => {
                    "options.url must be an absolute URL".to_string()
                }
                e => e.to_string(),
            },
        )?;
    // Before `parse`, which rewrites the document as it goes.
    if !readability.is_probably_readable() {
        return Ok(None);
    }
    let article = match readability.parse() {
        Ok(article) => article,
        Err(dom_smoothie::ReadabilityError::GrabFailed) => return Ok(None),
        Err(dom_smoothie::ReadabilityError::TooManyElements(found, max)) => {
            return Err(format!(
                "the page has {} elements, over the {} element limit",
                found, max
            ));
        }
        Err(e) => return Err(e.to_string()),
    };
    let text = article.text_content.trim().to_string();
    if text.is_empty() {
        return Ok(None);
    }
    Ok(Some(Article {
        title: article.title,
        byline: article.byline.and_then(non_empty),
        site_name: article.site_name.and_then(non_empty),
        language: article.lang.and_then(non_empty),
        published: article.published_time.and_then(non_empty),
        excerpt: article.excerpt.and_then(non_empty),
        image: article.image.and_then(non_empty),
        text,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    const RSS: &str = r#"<?xml version="1.0"?>
<rss version="2.0" xmlns:content="http://purl.org/rss/1.0/modules/content/"
     xmlns:media="http://search.yahoo.com/mrss/">
  <channel>
    <title>Example News</title>
    <link>https://news.example/</link>
    <description>All the &lt;b&gt;news&lt;/b&gt;</description>
    <language>en</language>
    <item>
      <title>You won't believe what the council decided</title>
      <link>https://news.example/2026/council</link>
      <guid isPermaLink="false">council-2026</guid>
      <pubDate>Thu, 15 Oct 2026 07:00:00 GMT</pubDate>
      <description>&lt;p&gt;The council &lt;a href="/x"&gt;voted&lt;/a&gt; on the budget.&lt;/p&gt;</description>
      <content:encoded><![CDATA[<p>First paragraph.</p><script>alert(1)</script><p>Second paragraph.</p>]]></content:encoded>
      <category>Politics</category>
      <media:content url="https://news.example/t.jpg" medium="image" type="image/jpeg"/>
    </item>
    <item>
      <title>No guid here</title>
      <link>https://news.example/2026/other</link>
      <description>&lt;img src="https://news.example/i.png"&gt; Text.</description>
    </item>
  </channel>
</rss>"#;

    #[test]
    fn rss_items_come_out_as_text() {
        let feed = parse_feed(RSS, &ParseFeedOptions::default()).unwrap();
        assert_eq!(feed.format, "rss2");
        assert_eq!(feed.title.as_deref(), Some("Example News"));
        assert_eq!(feed.description.as_deref(), Some("All the news"));
        assert_eq!(feed.link.as_deref(), Some("https://news.example/"));
        assert_eq!(feed.items.len(), 2);

        let item = &feed.items[0];
        assert_eq!(item.id, "council-2026");
        assert_eq!(
            item.link.as_deref(),
            Some("https://news.example/2026/council")
        );
        assert_eq!(item.published.as_deref(), Some("2026-10-15T07:00:00+00:00"));
        assert_eq!(
            item.summary.as_deref(),
            Some("The council voted on the budget.")
        );
        let content = item.content.as_deref().unwrap();
        assert!(content.contains("First paragraph."), "{content}");
        assert!(content.contains("Second paragraph."), "{content}");
        assert!(!content.contains('<'), "no markup survives: {content}");
        assert_eq!(item.categories, vec!["Politics".to_string()]);
        assert_eq!(item.image.as_deref(), Some("https://news.example/t.jpg"));
    }

    #[test]
    fn an_image_in_the_item_html_is_the_fallback() {
        let feed = parse_feed(RSS, &ParseFeedOptions::default()).unwrap();
        assert_eq!(
            feed.items[1].image.as_deref(),
            Some("https://news.example/i.png")
        );
        assert_eq!(feed.items[1].summary.as_deref(), Some("Text."));
    }

    #[test]
    fn an_item_without_an_id_gets_a_stable_one() {
        let first = parse_feed(RSS, &ParseFeedOptions::default()).unwrap();
        let again = parse_feed(RSS, &ParseFeedOptions::default()).unwrap();
        assert!(!first.items[1].id.is_empty());
        assert_eq!(first.items[1].id, again.items[1].id);
    }

    #[test]
    fn atom_takes_the_alternate_link_over_self() {
        let atom = r#"<?xml version="1.0" encoding="utf-8"?>
<feed xmlns="http://www.w3.org/2005/Atom">
  <title>Blog</title>
  <id>urn:blog</id>
  <updated>2026-10-15T08:00:00Z</updated>
  <link rel="self" href="https://blog.example/feed.xml"/>
  <link rel="alternate" href="https://blog.example/"/>
  <entry>
    <title type="html">A &lt;em&gt;post&lt;/em&gt;</title>
    <id>urn:post:1</id>
    <updated>2026-10-15T08:00:00Z</updated>
    <link rel="alternate" href="https://blog.example/post"/>
    <author><name>Ada</name></author>
    <summary>Short.</summary>
  </entry>
</feed>"#;
        let feed = parse_feed(atom, &ParseFeedOptions::default()).unwrap();
        assert_eq!(feed.format, "atom");
        assert_eq!(feed.link.as_deref(), Some("https://blog.example/"));
        let item = &feed.items[0];
        assert_eq!(item.title.as_deref(), Some("A post"));
        assert_eq!(item.link.as_deref(), Some("https://blog.example/post"));
        assert_eq!(item.authors, vec!["Ada".to_string()]);
        assert_eq!(item.updated.as_deref(), Some("2026-10-15T08:00:00+00:00"));
    }

    #[test]
    fn json_feed_parses() {
        let json = r#"{"version":"https://jsonfeed.org/version/1.1","title":"J",
            "items":[{"id":"1","url":"https://j.example/1","content_text":"Hi"}]}"#;
        let feed = parse_feed(json, &ParseFeedOptions::default()).unwrap();
        assert_eq!(feed.format, "json");
        assert_eq!(feed.items[0].link.as_deref(), Some("https://j.example/1"));
        assert_eq!(feed.items[0].content.as_deref(), Some("Hi"));
    }

    #[test]
    fn what_is_not_a_feed_is_refused() {
        let page = "<!doctype html><html><body>hello</body></html>";
        assert!(parse_feed(page, &ParseFeedOptions::default()).is_err());
        assert!(parse_feed("  ", &ParseFeedOptions::default()).is_err());
    }

    #[test]
    fn an_external_entity_is_not_loaded() {
        let xxe = r#"<?xml version="1.0"?>
<!DOCTYPE rss [<!ENTITY x SYSTEM "file:///etc/passwd">]>
<rss version="2.0"><channel><title>&x;</title><item><title>t</title></item></channel></rss>"#;
        if let Ok(feed) = parse_feed(xxe, &ParseFeedOptions::default()) {
            let title = feed.title.unwrap_or_default();
            assert!(!title.contains("root:"), "{title}");
        }
    }

    fn page() -> String {
        let paragraph = "The council met on Thursday and agreed a budget for the next year, \
            after a long debate about schools, roads and the library. ";
        format!(
            r#"<!doctype html><html lang="en"><head>
<title>Council agrees budget | Example News</title>
<meta property="og:site_name" content="Example News">
<meta name="author" content="Grace Hopper">
<meta property="article:published_time" content="2026-10-15T07:00:00Z">
</head><body>
<nav><a href="/">Home</a> <a href="/sport">Sport</a> <a href="/subscribe">Subscribe now!</a></nav>
<article>
  <h1>Council agrees budget</h1>
  <p>{p}{p}</p>
  <h2>What changes</h2>
  <p>{p}<a href="/budget.pdf">The full budget</a>.</p>
  <script>track()</script>
</article>
<aside class="related">You won't believe these ten tricks</aside>
<footer>Cookie settings</footer>
</body></html>"#,
            p = paragraph
        )
    }

    #[test]
    fn an_article_comes_out_without_the_page_around_it() {
        let options = ExtractArticleOptions {
            url: Some("https://news.example/2026/council".to_string()),
            format: ArticleFormat::Markdown,
        };
        let article = extract_article(&page(), &options).unwrap().unwrap();
        assert!(
            article.title.contains("Council agrees budget"),
            "{}",
            article.title
        );
        assert_eq!(article.byline.as_deref(), Some("Grace Hopper"));
        assert_eq!(article.site_name.as_deref(), Some("Example News"));
        assert_eq!(article.language.as_deref(), Some("en"));
        assert!(article.published.is_some());
        assert!(article.text.contains("agreed a budget"), "{}", article.text);
        assert!(article.text.contains("What changes"), "{}", article.text);
        assert!(
            article.text.contains("https://news.example/budget.pdf"),
            "links resolve against the url: {}",
            article.text
        );
        for chrome in ["Subscribe now", "Cookie settings", "track()"] {
            assert!(
                !article.text.contains(chrome),
                "{chrome} in {}",
                article.text
            );
        }
    }

    #[test]
    fn text_format_has_no_markdown() {
        let options = ExtractArticleOptions {
            url: None,
            format: ArticleFormat::Text,
        };
        let article = extract_article(&page(), &options).unwrap().unwrap();
        assert!(article.text.contains("What changes"));
        assert!(!article.text.contains("## "), "{}", article.text);
        assert!(!article.text.contains("]("), "{}", article.text);
    }

    #[test]
    fn a_page_without_an_article_is_none() {
        let index = "<html><body><nav><a href=/a>A</a></nav></body></html>";
        let found = extract_article(index, &ExtractArticleOptions::default()).unwrap();
        assert!(found.is_none(), "{found:?}");
    }

    #[test]
    fn a_relative_url_is_refused() {
        let options = ExtractArticleOptions {
            url: Some("/2026/council".to_string()),
            format: ArticleFormat::Markdown,
        };
        assert!(extract_article(&page(), &options).is_err());
    }

    #[test]
    fn a_page_past_the_element_limit_is_refused() {
        let huge = page().replace(
            "</article>",
            &format!(
                "{}</article>",
                "<span>x</span>".repeat(MAX_ARTICLE_ELEMENTS)
            ),
        );
        let err = extract_article(&huge, &ExtractArticleOptions::default()).unwrap_err();
        assert!(err.contains("element limit"), "{err}");
    }
}
