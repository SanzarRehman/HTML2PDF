//! Chromium-parity harness.
//!
//! Each fixture in `tests/fixtures/{features,combined,edge-cases}/*.html` is a
//! complete HTML document. This test renders every fixture to PDF and validates
//! it against a sibling expectation JSON in `tests/fixtures/expectations/`:
//!
//!   * `must_contain_operators` — PDF content-stream operators that must appear
//!     (e.g. `BT`/`ET`/`Tj` for text, `re`/`f` for fills, `S` for strokes,
//!     `Do` for images).
//!   * `must_contain_text` — strings that must be present in the *inflated*
//!     content streams (htmltopdf FlateDecode-compresses them).
//!   * `min_size_bytes` / `max_size_bytes` / `min_pages` — coarse size/page bounds.
//!
//! The `visual_assertions` in each JSON are human-readable descriptions of what
//! the page should look like; they are checked by the raster diff against a
//! Chromium reference (see `scripts/compare-parity.sh`), not by this file.
//!
//! Run the whole suite: `cargo test --test parity_tests`
//! Print a size/render-time report: `cargo test --test parity_tests -- --ignored report`

use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::Instant;

use htmltopdf::{Engine, Paper, RenderOptions};

/// Every fixture, as `(layer, name)`. Adding a fixture is: drop the `.html` in
/// `fixtures/<layer>/`, add an expectation JSON, and list it here.
const FIXTURES: &[(&str, &str)] = &[
    ("features", "typography"),
    ("features", "text-decoration"),
    ("features", "colors"),
    ("features", "box-model"),
    ("features", "borders"),
    ("features", "lists"),
    ("features", "tables"),
    ("features", "images"),
    ("features", "flexbox"),
    ("features", "grid"),
    ("features", "floats"),
    ("features", "positioning"),
    ("features", "line-height"),
    ("features", "fixed-per-page"),
    ("features", "font-family"),
    ("features", "font-face"),
    ("features", "sizing"),
    ("features", "pct-sizing"),
    ("features", "custom-properties"),
    ("features", "calc"),
    ("features", "text-polish"),
    ("features", "generated-content"),
    ("features", "links"),
    ("features", "flex-wrap"),
    ("features", "rtl"),
    ("features", "z-index"),
    ("features", "inline-images"),
    ("features", "rich-cells"),
    ("combined", "invoice"),
    ("edge-cases", "unicode"),
    ("edge-cases", "long-table"),
    ("edge-cases", "page-breaks"),
];

fn fixtures_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

/// Render a fixture with Chromium-matching page geometry (Letter). Margins come
/// from each fixture's `@page` rule.
fn render(layer: &str, name: &str) -> Vec<u8> {
    let path = fixtures_dir().join(layer).join(format!("{name}.html"));
    let html = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("read fixture {}: {e}", path.display()));
    Engine::new()
        .render_html(&html, RenderOptions::default().with_paper(Paper::Letter))
        .unwrap_or_else(|e| panic!("render {layer}/{name}: {e}"))
}

/// Concatenate every inflated FlateDecode stream so text/operator assertions see
/// the decompressed content, plus the raw bytes (for structural markers).
fn decoded_content(pdf: &[u8]) -> String {
    let mut out = String::new();
    let needle = b"stream";
    let mut i = 0;
    while let Some(pos) = find(&pdf[i..], needle) {
        let mut start = i + pos + needle.len();
        // Skip the EOL after `stream` (\r\n or \n).
        if pdf.get(start) == Some(&b'\r') {
            start += 1;
        }
        if pdf.get(start) == Some(&b'\n') {
            start += 1;
        }
        let Some(end_rel) = find(&pdf[start..], b"endstream") else {
            break;
        };
        let raw = &pdf[start..start + end_rel];
        let mut z = flate2::read::ZlibDecoder::new(raw);
        let mut s = String::new();
        if z.read_to_string(&mut s).is_ok() {
            out.push_str(&s);
        }
        i = start + end_rel + b"endstream".len();
    }
    out
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

/// Count page objects: `/Type /Page` not followed by `s` (which is the `/Pages`
/// tree node).
fn count_pages(pdf: &[u8]) -> usize {
    let text = String::from_utf8_lossy(pdf);
    let needle = "/Type /Page";
    let mut count = 0;
    let mut start = 0;
    while let Some(pos) = text[start..].find(needle) {
        let abs = start + pos;
        let after = abs + needle.len();
        if text[after..].chars().next() != Some('s') {
            count += 1;
        }
        start = abs + 1;
    }
    count.max(1)
}

fn expectations(layer: &str, name: &str) -> serde_json::Value {
    let file = fixtures_dir()
        .join("expectations")
        .join(format!("{layer}_{name}.json"));
    let json = std::fs::read_to_string(&file)
        .unwrap_or_else(|e| panic!("read expectation {}: {e}", file.display()));
    serde_json::from_str(&json)
        .unwrap_or_else(|e| panic!("parse expectation {layer}_{name}.json: {e}"))
}

/// Assert a rendered fixture against its expectation JSON.
fn assert_fixture(layer: &str, name: &str) {
    let pdf = render(layer, name);
    assert!(
        pdf.starts_with(b"%PDF") && find(&pdf, b"%%EOF").is_some(),
        "{layer}/{name}: not a well-formed PDF"
    );

    let exp = expectations(layer, name);
    let pa = &exp["pdf_assertions"];
    let raw = String::from_utf8_lossy(&pdf);
    let content = decoded_content(&pdf);

    if let Some(ops) = pa["must_contain_operators"].as_array() {
        for op in ops.iter().filter_map(|v| v.as_str()) {
            assert!(
                content.contains(op) || raw.contains(op),
                "{layer}/{name}: missing PDF operator `{op}`"
            );
        }
    }
    if let Some(texts) = pa["must_contain_text"].as_array() {
        for t in texts.iter().filter_map(|v| v.as_str()) {
            assert!(
                content.contains(t),
                "{layer}/{name}: rendered content missing text `{t}`"
            );
        }
    }
    if let Some(min) = pa["min_size_bytes"].as_u64() {
        assert!(
            pdf.len() as u64 >= min,
            "{layer}/{name}: size {} < min {min}",
            pdf.len()
        );
    }
    if let Some(max) = pa["max_size_bytes"].as_u64() {
        assert!(
            pdf.len() as u64 <= max,
            "{layer}/{name}: size {} > max {max}",
            pdf.len()
        );
    }
    if let Some(min_pages) = pa["min_pages"].as_u64() {
        let pages = count_pages(&pdf) as u64;
        assert!(
            pages >= min_pages,
            "{layer}/{name}: {pages} page(s) < min {min_pages}"
        );
    }
}

#[test]
fn all_fixtures_meet_expectations() {
    for (layer, name) in FIXTURES {
        assert_fixture(layer, name);
    }
}

#[test]
fn every_fixture_has_an_expectation_file() {
    for (layer, name) in FIXTURES {
        let f = fixtures_dir()
            .join("expectations")
            .join(format!("{layer}_{name}.json"));
        assert!(f.exists(), "missing expectation for {layer}/{name}: {f:?}");
    }
}

/// Not run by default. Prints a markdown table of PDF size, page count, and
/// render time per fixture — the htmltopdf analogue of ironpress's parity report.
/// Run with: `cargo test --test parity_tests -- --ignored --nocapture report`
#[test]
#[ignore]
fn report() {
    println!("\n| Fixture | Pages | Size | Render |");
    println!("|---|---|---|---|");
    let mut total = std::time::Duration::ZERO;
    for (layer, name) in FIXTURES {
        // Warm once, then time a second render to avoid first-call noise.
        let _ = render(layer, name);
        let start = Instant::now();
        let pdf = render(layer, name);
        let dt = start.elapsed();
        total += dt;
        println!(
            "| {layer}/{name} | {} | {:.1} KB | {} µs |",
            count_pages(&pdf),
            pdf.len() as f64 / 1024.0,
            dt.as_micros()
        );
    }
    println!(
        "\n{} fixtures, {} µs total (~{:.0} pages/sec on this box).\n",
        FIXTURES.len(),
        total.as_micros(),
        FIXTURES.len() as f64 / total.as_secs_f64()
    );
}
