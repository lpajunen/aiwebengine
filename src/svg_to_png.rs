//! `convert.svg_to_png`: rasterising an SVG a script holds.
//!
//! The SVG is untrusted — often model-written, and steerable by whatever the
//! model read — and what it costs to render is not knowable from its size: a
//! few kilobytes of filters, masks or translucent layers can run for a minute
//! and allocate gigabytes, and resvg cannot be interrupted once started. So
//! the render happens in a child process the engine can kill:
//!
//! - **The parent** ([`svg_to_png`]) makes the cheap refusals, waits for one
//!   of a few render slots, and runs this same binary with [`CHILD_FLAG`]
//!   under a deadline — [`RENDER_TIMEOUT`], shortened to what is left of the
//!   execution budget. Past it the child is killed.
//! - **The child** ([`child_main`]) caps its own CPU time (and, on Linux, its
//!   address space) before reading anything, so it cannot outlive a parent
//!   that died, then renders and writes the PNG to stdout.
//!
//! What it renders from is nothing but the SVG's own text:
//!
//! - **No outside resources.** Every `<image>` href resolves to nothing, a
//!   `data:` URI included, so there is no path to the filesystem or the
//!   network and no image decoder sees hostile bytes.
//! - **Bundled fonts only.** The fonts under `assets/fonts/` are the whole
//!   font database. Every family, generic or named, falls back to Noto Sans,
//!   so text always renders and renders the same on every host.

use base64::Engine;
use resvg::{tiny_skia, usvg};
use serde::{Deserialize, Serialize};
use std::io::{Read, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant};

/// The argument that makes the engine binary a renderer instead of a server.
/// `main` checks for it before anything else runs.
pub const CHILD_FLAG: &str = "--render-svg-to-png";

/// Largest SVG accepted, in bytes.
pub const MAX_SVG_BYTES: usize = 1_000_000;

/// Longest side of the output, in pixels.
pub const MAX_PNG_EDGE: u32 = 4096;

/// Most pixels in the output (2048 × 2048).
pub const MAX_PNG_PIXELS: u64 = 2048 * 2048;

/// Most nodes in the parsed drawing, counting clip paths, masks and patterns.
pub const MAX_SVG_NODES: usize = 100_000;

/// Longest a render may take, before the execution budget shortens it.
pub const RENDER_TIMEOUT: Duration = Duration::from_secs(5);

/// The child's address space, on Linux. Past it an allocation fails and the
/// child aborts, which the parent reports as running out of memory.
#[cfg(target_os = "linux")]
const CHILD_MEMORY_BYTES: u64 = 1024 * 1024 * 1024;

/// Most bytes read back from the child: a 2048 × 2048 PNG of noise is about
/// 16 MB, so this is never the limit an honest render meets.
const MAX_PNG_BYTES: u64 = 32 * 1024 * 1024;

/// Most bytes of the child's stderr kept for the error message.
const MAX_MESSAGE_BYTES: u64 = 4096;

/// Longest `background` accepted; a CSS color is far shorter. It is what
/// keeps the options line the child reads inside [`MAX_OPTIONS_BYTES`].
const MAX_BACKGROUND_CHARS: usize = 64;

/// Room the child gives the options line ahead of the SVG.
const MAX_OPTIONS_BYTES: u64 = 1024;

/// The child's exit code for a refusal, whose message is on stderr. Any
/// other failure is a crash.
const REFUSED: i32 = 2;

const SANS: &str = "Noto Sans";
const MONO: &str = "Noto Sans Mono";

/// What a script may ask of the render. Unknown keys are refused, so a typo
/// is not silently a default.
#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SvgToPngOptions {
    /// Output width in pixels. Alone, the height follows the drawing's aspect.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub width: Option<u32>,
    /// Output height in pixels. Alone, the width follows the drawing's aspect.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub height: Option<u32>,
    /// A CSS color painted under the drawing. Without one the PNG is
    /// transparent, which some consumers (Telegram's photo pipeline among
    /// them) flatten to black.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub background: Option<String>,
}

// ---------------------------------------------------------------------------
// The parent
// ---------------------------------------------------------------------------

static RENDERER: OnceLock<PathBuf> = OnceLock::new();

/// Run renders with `path` rather than the current executable.
///
/// For tests, whose executable is a test harness rather than the engine:
/// they point this at `CARGO_BIN_EXE_aiwebengine`. Only the first call has
/// any effect.
pub fn use_renderer(path: PathBuf) {
    let _ = RENDERER.set(path);
}

fn renderer() -> Result<PathBuf, String> {
    match RENDERER.get() {
        Some(path) => Ok(path.clone()),
        None => std::env::current_exe().map_err(|e| format!("cannot find the renderer: {}", e)),
    }
}

/// Renders at once: half the cores, so rendering cannot take every one of
/// them from the requests around it.
fn max_concurrent_renders() -> usize {
    std::thread::available_parallelism()
        .map(|n| n.get() / 2)
        .unwrap_or(1)
        .max(1)
}

struct Slots {
    busy: Mutex<usize>,
    freed: Condvar,
}

fn slots() -> &'static Slots {
    static SLOTS: OnceLock<Slots> = OnceLock::new();
    SLOTS.get_or_init(|| Slots {
        busy: Mutex::new(0),
        freed: Condvar::new(),
    })
}

/// A render slot, given back when dropped.
struct Slot;

impl Drop for Slot {
    fn drop(&mut self) {
        let slots = slots();
        let mut busy = slots.busy.lock().unwrap_or_else(|e| e.into_inner());
        *busy = busy.saturating_sub(1);
        slots.freed.notify_one();
    }
}

fn acquire_slot(deadline: Instant) -> Result<Slot, String> {
    let slots = slots();
    let limit = max_concurrent_renders();
    let mut busy = slots.busy.lock().unwrap_or_else(|e| e.into_inner());
    while *busy >= limit {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Err(format!(
                "every render slot ({}) stayed busy until the deadline; try again",
                limit
            ));
        }
        busy = slots
            .freed
            .wait_timeout(busy, left)
            .unwrap_or_else(|e| e.into_inner())
            .0;
    }
    *busy += 1;
    Ok(Slot)
}

/// Render `svg` to a PNG, answered as base64 — the form `fetch` takes as
/// `bodyBase64` or a `form` part's `base64`.
pub fn svg_to_png(svg: &str, options: &SvgToPngOptions) -> Result<String, String> {
    check(svg, options)?;
    let timeout = crate::database::within_host_budget(RENDER_TIMEOUT);
    let deadline = Instant::now() + timeout;
    let _slot = acquire_slot(deadline)?;
    let png = run_child(svg, options, deadline, timeout)?;
    Ok(base64::engine::general_purpose::STANDARD.encode(png))
}

fn run_child(
    svg: &str,
    options: &SvgToPngOptions,
    deadline: Instant,
    timeout: Duration,
) -> Result<Vec<u8>, String> {
    let header =
        serde_json::to_string(options).map_err(|e| format!("cannot pass the options on: {}", e))?;
    let mut input = Vec::with_capacity(header.len() + 1 + svg.len());
    input.extend_from_slice(header.as_bytes());
    input.push(b'\n');
    input.extend_from_slice(svg.as_bytes());

    // Nothing of the engine's environment — database URLs, keys — reaches
    // the child; it needs none of it.
    let mut child = Command::new(renderer()?)
        .arg(CHILD_FLAG)
        .env_clear()
        .current_dir("/")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("cannot start the renderer: {}", e))?;

    // Each pipe on a thread of its own: a PNG larger than the pipe buffer
    // would otherwise block the child while this side waits for it to exit.
    let stdin = child.stdin.take();
    let writer = std::thread::spawn(move || {
        if let Some(mut stdin) = stdin {
            let _ = stdin.write_all(&input);
        }
    });
    let stdout = child.stdout.take();
    let reader = std::thread::spawn(move || {
        let mut png = Vec::new();
        if let Some(stdout) = stdout {
            let _ = stdout.take(MAX_PNG_BYTES + 1).read_to_end(&mut png);
        }
        png
    });
    let stderr = child.stderr.take();
    let messages = std::thread::spawn(move || {
        let mut message = Vec::new();
        if let Some(stderr) = stderr {
            let _ = stderr.take(MAX_MESSAGE_BYTES).read_to_end(&mut message);
        }
        String::from_utf8_lossy(&message).trim().to_string()
    });

    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!(
                    "rendering took longer than {} ms and was stopped. Filters, masks, many \
                     translucent groups and fine dash patterns are what make an SVG slow",
                    timeout.as_millis()
                ));
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(2)),
            Err(e) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("lost track of the renderer: {}", e));
            }
        }
    };

    let _ = writer.join();
    let png = reader.join().unwrap_or_default();
    let message = messages.join().unwrap_or_default();

    if status.code() == Some(REFUSED) {
        return Err(message);
    }
    if !status.success() {
        return Err(
            "the renderer stopped without an answer; the SVG most likely needs more memory \
             than a render is given"
                .to_string(),
        );
    }
    if png.len() as u64 > MAX_PNG_BYTES {
        return Err(format!("the PNG would be over {} bytes", MAX_PNG_BYTES));
    }
    if !png.starts_with(b"\x89PNG\r\n\x1a\n") {
        return Err("the renderer answered with something that is not a PNG".to_string());
    }
    Ok(png)
}

// ---------------------------------------------------------------------------
// The child
// ---------------------------------------------------------------------------

/// The renderer process: options as one JSON line on stdin, then the SVG;
/// the PNG on stdout. Answers the exit code.
pub fn child_main() -> i32 {
    limit_resources();

    let mut input = Vec::new();
    let read = std::io::stdin()
        .lock()
        .take(MAX_SVG_BYTES as u64 + MAX_OPTIONS_BYTES)
        .read_to_end(&mut input);
    let answer = read
        .map_err(|e| format!("cannot read the SVG: {}", e))
        .and_then(|_| {
            let split = input
                .iter()
                .position(|b| *b == b'\n')
                .ok_or_else(|| "no options line".to_string())?;
            let options: SvgToPngOptions =
                serde_json::from_slice(&input[..split]).map_err(|e| format!("options: {}", e))?;
            let svg = std::str::from_utf8(&input[split + 1..])
                .map_err(|_| "the SVG is not UTF-8".to_string())?;
            render(svg, &options)
        });

    match answer {
        Ok(png) => match std::io::stdout().lock().write_all(&png) {
            Ok(()) => 0,
            Err(_) => 1,
        },
        Err(message) => {
            eprintln!("{}", message);
            REFUSED
        }
    }
}

/// CPU time a little past the timeout, so a child whose parent died stops on
/// its own; and on Linux, a ceiling on its memory.
fn limit_resources() {
    // A failed `setrlimit` leaves the limit as it was, which the parent's
    // deadline still bounds, so its answer is not checked.
    #[cfg(unix)]
    {
        let cpu = rlimit(RENDER_TIMEOUT.as_secs() + 1);
        // SAFETY: `setrlimit` reads the struct it is given and nothing else.
        unsafe {
            libc::setrlimit(libc::RLIMIT_CPU, &cpu);
        }
    }
    #[cfg(target_os = "linux")]
    {
        let memory = rlimit(CHILD_MEMORY_BYTES);
        // SAFETY: as above.
        unsafe {
            libc::setrlimit(libc::RLIMIT_AS, &memory);
        }
    }
}

#[cfg(unix)]
fn rlimit(value: u64) -> libc::rlimit {
    libc::rlimit {
        rlim_cur: value as libc::rlim_t,
        rlim_max: value as libc::rlim_t,
    }
}

// ---------------------------------------------------------------------------
// Rendering
// ---------------------------------------------------------------------------

fn fontdb() -> Arc<usvg::fontdb::Database> {
    static FONTS: OnceLock<Arc<usvg::fontdb::Database>> = OnceLock::new();
    FONTS
        .get_or_init(|| {
            let mut db = usvg::fontdb::Database::new();
            db.load_font_data(include_bytes!("../assets/fonts/NotoSans-Regular.ttf").to_vec());
            db.load_font_data(include_bytes!("../assets/fonts/NotoSans-Bold.ttf").to_vec());
            db.load_font_data(include_bytes!("../assets/fonts/NotoSansMono-Regular.ttf").to_vec());
            // usvg's selector ends every query with `serif`, so pointing that
            // at the sans face is what makes an unknown family fall back.
            db.set_serif_family(SANS);
            db.set_sans_serif_family(SANS);
            db.set_cursive_family(SANS);
            db.set_fantasy_family(SANS);
            db.set_monospace_family(MONO);
            Arc::new(db)
        })
        .clone()
}

fn background(options: &SvgToPngOptions) -> Result<Option<tiny_skia::Color>, String> {
    options
        .background
        .as_deref()
        .map(|css| {
            if css.len() > MAX_BACKGROUND_CHARS {
                return Err(format!(
                    "background is {} characters; a CSS color is at most {}",
                    css.len(),
                    MAX_BACKGROUND_CHARS
                ));
            }
            css.parse::<svgtypes::Color>()
                .map(|c| tiny_skia::Color::from_rgba8(c.red, c.green, c.blue, c.alpha))
                .map_err(|_| format!("background {:?} is not a CSS color", css))
        })
        .transpose()
}

/// What can be refused without parsing, so neither half spends more on it.
fn check(svg: &str, options: &SvgToPngOptions) -> Result<(), String> {
    if svg.trim().is_empty() {
        return Err("the SVG is empty".to_string());
    }
    if svg.len() > MAX_SVG_BYTES {
        return Err(format!(
            "the SVG is {} bytes; at most {} are accepted",
            svg.len(),
            MAX_SVG_BYTES
        ));
    }
    if options.width == Some(0) || options.height == Some(0) {
        return Err("width and height must be at least 1".to_string());
    }
    background(options).map(|_| ())
}

fn count_nodes(group: &usvg::Group) -> usize {
    group
        .children()
        .iter()
        .map(|node| {
            1 + match node {
                usvg::Node::Group(g) => count_nodes(g),
                usvg::Node::Text(t) => count_nodes(t.flattened()),
                usvg::Node::Path(_) | usvg::Node::Image(_) => 0,
            }
        })
        .sum()
}

/// The output size: what was asked for, else the drawing's own.
fn output_size(drawing: usvg::Size, options: &SvgToPngOptions) -> Result<(u32, u32), String> {
    let aspect = drawing.width() / drawing.height();
    let (width, height) = match (options.width, options.height) {
        (Some(w), Some(h)) => (w as f32, h as f32),
        (Some(w), None) => (w as f32, w as f32 / aspect),
        (None, Some(h)) => (h as f32 * aspect, h as f32),
        (None, None) => (drawing.width(), drawing.height()),
    };
    let (width, height) = (width.round().max(1.0), height.round().max(1.0));
    if !width.is_finite()
        || !height.is_finite()
        || width > MAX_PNG_EDGE as f32
        || height > MAX_PNG_EDGE as f32
    {
        return Err(format!(
            "output would be {}×{} px; each side may be at most {} px — pass a smaller width or height",
            width, height, MAX_PNG_EDGE
        ));
    }
    let (width, height) = (width as u32, height as u32);
    if u64::from(width) * u64::from(height) > MAX_PNG_PIXELS {
        return Err(format!(
            "output would be {}×{} px; at most {} pixels in all — pass a smaller width or height",
            width, height, MAX_PNG_PIXELS
        ));
    }
    Ok((width, height))
}

/// Render in this process. Only the child calls it outside tests.
fn render(svg: &str, options: &SvgToPngOptions) -> Result<Vec<u8>, String> {
    check(svg, options)?;
    let background = background(options)?;

    let parse_options = usvg::Options {
        resources_dir: None,
        font_family: SANS.to_string(),
        image_href_resolver: usvg::ImageHrefResolver {
            resolve_data: Box::new(|_, _, _| None),
            resolve_string: Box::new(|_, _| None),
        },
        fontdb: fontdb(),
        ..usvg::Options::default()
    };
    let tree = usvg::Tree::from_str(svg, &parse_options)
        .map_err(|e| format!("not a renderable SVG: {}", e))?;

    let nodes = count_nodes(tree.root())
        + tree
            .clip_paths()
            .iter()
            .map(|c| count_nodes(c.root()))
            .sum::<usize>()
        + tree
            .masks()
            .iter()
            .map(|m| count_nodes(m.root()))
            .sum::<usize>()
        + tree
            .patterns()
            .iter()
            .map(|p| count_nodes(p.root()))
            .sum::<usize>();
    if nodes > MAX_SVG_NODES {
        return Err(format!(
            "the drawing has {} elements once expanded; at most {} are rendered",
            nodes, MAX_SVG_NODES
        ));
    }

    let drawing = tree.size();
    let (width, height) = output_size(drawing, options)?;
    let mut pixmap = tiny_skia::Pixmap::new(width, height)
        .ok_or_else(|| format!("cannot allocate a {}×{} image", width, height))?;
    if let Some(color) = background {
        pixmap.fill(color);
    }

    // Scale to fit, centred: with only one side given this is exact; with
    // both, the drawing keeps its aspect and the rest is background.
    let scale = (width as f32 / drawing.width()).min(height as f32 / drawing.height());
    let dx = (width as f32 - drawing.width() * scale) / 2.0;
    let dy = (height as f32 - drawing.height() * scale) / 2.0;
    let transform = tiny_skia::Transform::from_row(scale, 0.0, 0.0, scale, dx, dy);
    resvg::render(&tree, transform, &mut pixmap.as_mut());

    pixmap
        .encode_png()
        .map_err(|e| format!("PNG encoding failed: {}", e))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dimensions(png: &[u8]) -> (u32, u32) {
        let w = u32::from_be_bytes([png[16], png[17], png[18], png[19]]);
        let h = u32::from_be_bytes([png[20], png[21], png[22], png[23]]);
        (w, h)
    }

    fn pixmap(png: &[u8]) -> tiny_skia::Pixmap {
        tiny_skia::Pixmap::decode_png(png).expect("decodes")
    }

    const SQUARE: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" width="40" height="20"><rect width="40" height="20" fill="red"/></svg>"#;

    #[test]
    fn renders_at_the_drawings_own_size() {
        let png = render(SQUARE, &SvgToPngOptions::default()).expect("renders");
        assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n");
        assert_eq!(dimensions(&png), (40, 20));
        let px = pixmap(&png).pixel(5, 5).expect("pixel");
        assert_eq!(
            (px.red(), px.green(), px.blue(), px.alpha()),
            (255, 0, 0, 255)
        );
    }

    #[test]
    fn one_side_keeps_the_aspect() {
        let options = SvgToPngOptions {
            width: Some(400),
            ..Default::default()
        };
        let png = render(SQUARE, &options).expect("renders");
        assert_eq!(dimensions(&png), (400, 200));
    }

    #[test]
    fn both_sides_fit_and_centre_over_the_background() {
        let options = SvgToPngOptions {
            width: Some(40),
            height: Some(40),
            background: Some("white".to_string()),
        };
        let png = render(SQUARE, &options).expect("renders");
        assert_eq!(dimensions(&png), (40, 40));
        let image = pixmap(&png);
        let top = image.pixel(20, 2).expect("pixel");
        assert_eq!((top.red(), top.green(), top.blue()), (255, 255, 255));
        let middle = image.pixel(20, 20).expect("pixel");
        assert_eq!((middle.red(), middle.green(), middle.blue()), (255, 0, 0));
    }

    #[test]
    fn text_renders_with_the_bundled_fonts_whatever_family_is_named() {
        for family in ["Helvetica", "serif", "monospace", "NoSuchFont"] {
            let svg = format!(
                r#"<svg xmlns="http://www.w3.org/2000/svg" width="200" height="60"><text x="10" y="40" font-size="32" font-family="{}">Hello</text></svg>"#,
                family
            );
            let png = render(&svg, &SvgToPngOptions::default()).expect("renders");
            let inked = pixmap(&png)
                .pixels()
                .iter()
                .filter(|p| p.alpha() > 0)
                .count();
            assert!(inked > 100, "{} drew {} pixels", family, inked);
        }
    }

    #[test]
    fn images_are_never_loaded() {
        for href in [
            "file:///etc/passwd",
            "/etc/hosts",
            "https://example.com/a.png",
            // A 1×1 opaque PNG: even a data URI is not decoded.
            "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8DwHwAFBQIAX8jx0gAAAABJRU5ErkJggg==",
        ] {
            let svg = format!(
                r#"<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink" width="10" height="10"><image width="10" height="10" xlink:href="{}"/></svg>"#,
                href
            );
            let png = render(&svg, &SvgToPngOptions::default()).expect("renders");
            assert!(
                pixmap(&png).pixels().iter().all(|p| p.alpha() == 0),
                "{} was drawn",
                href
            );
        }
    }

    #[test]
    fn refuses_what_is_not_an_svg() {
        assert!(render("", &SvgToPngOptions::default()).is_err());
        let err = render("<html></html>", &SvgToPngOptions::default()).unwrap_err();
        assert!(err.contains("not a renderable SVG"), "{}", err);
    }

    #[test]
    fn refuses_oversized_input_and_output() {
        let big = format!(
            "<svg xmlns=\"http://www.w3.org/2000/svg\">{}</svg>",
            " ".repeat(MAX_SVG_BYTES)
        );
        assert!(
            render(&big, &SvgToPngOptions::default())
                .unwrap_err()
                .contains("bytes")
        );

        let wide = SvgToPngOptions {
            width: Some(MAX_PNG_EDGE + 1),
            ..Default::default()
        };
        assert!(render(SQUARE, &wide).unwrap_err().contains("each side"));

        let square = SvgToPngOptions {
            width: Some(4000),
            height: Some(4000),
            ..Default::default()
        };
        assert!(render(SQUARE, &square).unwrap_err().contains("pixels"));

        let huge = r#"<svg xmlns="http://www.w3.org/2000/svg" width="100000" height="10"/>"#;
        assert!(
            render(huge, &SvgToPngOptions::default())
                .unwrap_err()
                .contains("each side")
        );
    }

    #[test]
    fn refuses_a_drawing_that_expands_past_the_node_limit() {
        // Each level `<use>`s the one before it ten times: five levels is a
        // hundred thousand rectangles from a few hundred bytes. (Six reaches
        // usvg's own million-node ceiling during parsing.)
        let mut svg = String::from(
            r#"<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink" width="10" height="10"><defs><rect id="l0" width="1" height="1"/>"#,
        );
        for level in 1..=5 {
            svg.push_str(&format!("<g id=\"l{}\">", level));
            for _ in 0..10 {
                svg.push_str(&format!("<use xlink:href=\"#l{}\"/>", level - 1));
            }
            svg.push_str("</g>");
        }
        svg.push_str("</defs><use xlink:href=\"#l5\"/></svg>");
        let err = render(&svg, &SvgToPngOptions::default()).unwrap_err();
        assert!(err.contains("elements"), "{}", err);
    }

    #[test]
    fn refuses_a_background_that_is_not_a_color() {
        let options = SvgToPngOptions {
            background: Some("not-a-color".to_string()),
            ..Default::default()
        };
        assert!(render(SQUARE, &options).unwrap_err().contains("CSS color"));
    }

    #[test]
    fn refuses_unknown_options() {
        let parsed: Result<SvgToPngOptions, _> = serde_json::from_str(r#"{"widht": 10}"#);
        assert!(parsed.is_err());
    }
}
