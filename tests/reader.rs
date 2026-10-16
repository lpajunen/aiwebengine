//! `convert.parse_feed` and `convert.extract_article` as a script sees them:
//! plain objects back, `null` for a page with no article, errors it can catch.

mod common;

use aiwebengine::js_engine::execute_script_secure;
use aiwebengine::security::{Principal, UserContext};
use common::setup_env;

fn anyone() -> Principal {
    Principal::Caller(UserContext {
        user_id: Some("user".to_string()),
        is_authenticated: true,
        capabilities: Default::default(),
        attenuated: false,
        network_scope: None,
        acting: Default::default(),
    })
}

#[tokio::test(flavor = "multi_thread")]
async fn a_script_reads_a_feed_and_an_article() {
    setup_env().await;
    let script = r###"
        const rss = `<?xml version="1.0"?><rss version="2.0"><channel>
            <title>News</title><link>https://news.example/</link>
            <item><title>Ten &lt;i&gt;shocking&lt;/i&gt; facts</title>
              <link>https://news.example/a</link><guid>a</guid>
              <pubDate>Thu, 15 Oct 2026 07:00:00 GMT</pubDate></item>
        </channel></rss>`;
        const feed = convert.parse_feed(rss);
        if (feed.format !== "rss2") throw new Error("format " + feed.format);
        const item = feed.items[0];
        if (item.id !== "a") throw new Error("id " + item.id);
        if (item.title !== "Ten shocking facts") throw new Error("title " + item.title);
        if (item.published !== "2026-10-15T07:00:00+00:00") throw new Error("published " + item.published);
        if (item.content !== null) throw new Error("content " + item.content);

        const p = "The council met on Thursday and agreed a budget for the next year, " +
            "after a long debate about schools, roads and the library. ";
        const page = `<html><head><title>Budget</title></head><body>
            <nav>Subscribe now!</nav>
            <article><h1>Budget</h1><p>${p}${p}${p}</p><h2>Next</h2><p>${p}${p}</p></article>
            </body></html>`;
        const article = convert.extract_article(page, { url: "https://news.example/a" });
        if (!article) throw new Error("no article");
        if (!article.text.includes("agreed a budget")) throw new Error(article.text);
        if (article.text.includes("Subscribe")) throw new Error(article.text);
        if (!article.text.includes("## Next")) throw new Error("markdown: " + article.text);
        const plain = convert.extract_article(page, { format: "text" });
        if (plain.text.includes("## ")) throw new Error("text: " + plain.text);

        if (convert.extract_article("<html><body><a href=/x>x</a></body></html>") !== null) {
            throw new Error("an index page is not an article");
        }
    "###;
    let result = execute_script_secure("test://reader", script, anyone());
    assert!(result.success, "{:?}", result.error);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_script_gets_errors_it_can_catch() {
    setup_env().await;
    let script = r#"
        const expect = (f, name, text) => {
            try { f(); } catch (e) {
                if (e.name !== name || !e.message.includes(text)) {
                    throw new Error("wrong error: " + e.name + ": " + e.message);
                }
                return;
            }
            throw new Error("did not throw: " + text);
        };
        expect(() => convert.parse_feed("<html><body>hi</body></html>"), "Error", "not a feed");
        expect(() => convert.parse_feed(""), "Error", "empty");
        expect(() => convert.parse_feed("<rss/>", { baseURL: "x" }), "TypeError", "baseURL");
        expect(() => convert.parse_feed(42), "TypeError", "expects a string");
        expect(() => convert.extract_article("<p>x</p>", { url: "/a" }), "Error", "absolute URL");
        expect(() => convert.extract_article("<p>x</p>", { format: "html" }), "TypeError", "unknown variant `html`");
        expect(() => convert.extract_article("<p>x</p>", []), "TypeError", "options must be an object");
    "#;
    let result = execute_script_secure("test://reader-errors", script, anyone());
    assert!(result.success, "{:?}", result.error);
}
