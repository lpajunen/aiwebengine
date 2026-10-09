//! `convert.svg_to_png` through its child process: the engine binary run as
//! a renderer, killed at the deadline.

mod common;

use aiwebengine::js_engine::execute_script_secure;
use aiwebengine::security::{Principal, UserContext};
use aiwebengine::svg_to_png::{SvgToPngOptions, svg_to_png};
use base64::Engine;
use common::setup_env;
use std::time::{Duration, Instant};

const DIAGRAM: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" width="300" height="100">
  <rect x="10" y="10" width="120" height="80" rx="8" fill="#e8f0fe" stroke="#1a73e8"/>
  <text x="70" y="55" text-anchor="middle" font-family="Helvetica" font-size="16">Engine</text>
  <path d="M130 50 H170" stroke="#333" stroke-dasharray="4 2"/>
  <rect x="170" y="10" width="120" height="80" rx="8" fill="#fef7e0" stroke="#f9ab00"/>
</svg>"##;

/// A blur over a large region, many times: tens of seconds in-process.
fn slow_svg() -> String {
    let mut svg = String::from(
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="2048" height="2048"><defs><filter id="b" x="-50%" y="-50%" width="200%" height="200%"><feMorphology radius="20"/></filter></defs>"#,
    );
    for i in 0..20 {
        svg.push_str(&format!(
            r#"<g filter="url(#b)"><rect x="{}" width="2048" height="2048" fill="red"/></g>"#,
            i
        ));
    }
    svg.push_str("</svg>");
    svg
}

/// `setup_env`, and one render outside any script budget. macOS checks a
/// binary the first time it runs, which under a loaded test run takes longer
/// than a script's budget; the server never sees this, being that binary
/// already.
async fn ready() {
    setup_env().await;
    static WARM: std::sync::OnceLock<()> = std::sync::OnceLock::new();
    WARM.get_or_init(|| {
        let _ = svg_to_png(DIAGRAM, &SvgToPngOptions::default());
    });
}

fn png_size(base64: &str) -> (u32, u32) {
    let png = base64::engine::general_purpose::STANDARD
        .decode(base64)
        .expect("base64");
    assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n");
    let w = u32::from_be_bytes([png[16], png[17], png[18], png[19]]);
    let h = u32::from_be_bytes([png[20], png[21], png[22], png[23]]);
    (w, h)
}

fn anyone() -> Principal {
    Principal::Caller(UserContext {
        user_id: Some("user".to_string()),
        is_authenticated: true,
        capabilities: Default::default(),
        attenuated: false,
        network_scope: None,
    })
}

#[tokio::test(flavor = "multi_thread")]
async fn renders_in_a_child_process() {
    ready().await;
    let options = SvgToPngOptions {
        width: Some(600),
        background: Some("white".to_string()),
        ..Default::default()
    };
    let png = svg_to_png(DIAGRAM, &options).expect("renders");
    assert_eq!(png_size(&png), (600, 200));
}

#[tokio::test(flavor = "multi_thread")]
async fn the_childs_refusal_is_the_message() {
    ready().await;
    let err = svg_to_png("<html/>", &SvgToPngOptions::default()).unwrap_err();
    assert!(err.contains("not a renderable SVG"), "{}", err);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_slow_render_is_killed_at_the_execution_budget() {
    ready().await;
    let svg = slow_svg();
    let started = Instant::now();
    let err = {
        let _budget =
            aiwebengine::database::bound_host_calls(Instant::now() + Duration::from_millis(300));
        svg_to_png(&svg, &SvgToPngOptions::default()).unwrap_err()
    };
    assert!(err.contains("longer than"), "{}", err);
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "took {:?}",
        started.elapsed()
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn any_script_may_call_it() {
    ready().await;
    let script = format!(
        r##"
        const png = convert.svg_to_png({svg:?}, {{ height: 50, background: "#fff" }});
        if (typeof png !== "string" || !png.startsWith("iVBORw0KGgo")) {{
            throw new Error("expected base64 PNG, got " + String(png).slice(0, 40));
        }}
        "##,
        svg = DIAGRAM
    );
    let result = execute_script_secure("test://svg-to-png", &script, anyone());
    assert!(result.success, "{:?}", result.error);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_script_gets_errors_it_can_catch() {
    ready().await;
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
        expect(() => convert.svg_to_png("<svg", {}), "Error", "not a renderable SVG");
        expect(() => convert.svg_to_png("<svg/>", { widht: 10 }), "TypeError", "widht");
        expect(() => convert.svg_to_png("<svg/>", "big"), "TypeError", "options must be an object");
        expect(() => convert.svg_to_png(42), "TypeError", "expects a string");
        expect(() => convert.svg_to_png("<svg/>", { background: "nope" }), "Error", "CSS color");
    "#;
    let result = execute_script_secure("test://svg-to-png-errors", script, anyone());
    assert!(result.success, "{:?}", result.error);
}
