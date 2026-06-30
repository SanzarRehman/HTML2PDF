# htmltopdf

> World's fastest, leanest parallel HTML-to-PDF engine for serious server workloads.

[![Rust](https://img.shields.io/badge/Rust-1.86%2B-orange.svg)](https://www.rust-lang.org/)
[![License](https://img.shields.io/badge/license-MIT-blue.svg)](#license)
[![Status](https://img.shields.io/badge/status-experimental-yellow.svg)](#project-status)

`htmltopdf` is a Rust HTML-to-PDF engine designed for high concurrency, low RAM,
low CPU overhead, and browser-grade rendering fidelity over time. The core idea
is simple: render many documents in parallel inside one process, without
launching Chromium, Puppeteer, or a browser subprocess per job.

The project is built around a real rendering pipeline: HTML parsing, compact DOM,
CSS parsing, cascade, box generation, layout, display-list painting, and a
streaming compressed PDF writer.

```text
HTML -> html5ever -> arena DOM -> cssparser -> cascade
     -> box tree -> layout -> display list -> compressed PDF
```

## Why htmltopdf?

- **Fast by design**: independent render jobs scale across CPU cores.
- **Minimal RAM**: compact arena-based DOM, index-based data, and no browser
  renderer process per conversion.
- **Parallel-first**: CLI benchmarks and the HTTP server are built around
  worker-level parallelism.
- **Real HTML parser**: uses `html5ever`, not ad hoc tag scanning.
- **Real CSS parser**: uses `cssparser` for stylesheet tokenization and cascade
  support.
- **Selectable compressed PDFs**: generated text stays searchable/selectable.
- **Unicode font support**: optional TrueType/OpenType embedding with Type0 /
  Identity-H PDFs, ToUnicode maps, and TrueType glyph subsetting when possible.
- **Small dependency surface**: no async runtime, no browser, no web framework.

## Project Status

This is an early engine, not a complete browser. The long-term goal is full CSS
and controlled JavaScript support with much lower memory cost than
Chromium-based renderers.

Works today:

- HTML parsing through `html5ever`.
- CSS parsing and cascade for supported selector/declaration subsets.
- Tag/class selectors, specificity, source order, inheritance, and `!important`.
- Basic flow documents: headings, paragraphs, lists, inline runs, blockquotes.
- Tables: rows, cells, colspans, headers/footers, borders, backgrounds,
  alignment, wrapping, clipping, and repeated table headers.
- CSS colors, font sizes, bold text, text alignment, margins, padding, block
  backgrounds, and basic borders.
- Pagination, page margins, landscape pages, compressed PDF streams.
- Built-in Helvetica metrics and optional embedded TrueType/OpenType fonts.
- Font subsetting for `glyf`-based TrueType fonts, with full-font fallback for
  formats that cannot be subset yet.
- CLI, Rust library API, and lightweight HTTP API.

Not complete yet:

- JavaScript execution.
- Images, SVG, canvas, flexbox, grid, floats, and absolute positioning.
- Full browser text shaping and baseline handling.
- Exact non-Latin layout metrics for every script.
- Complete CSS selector/property coverage.
- Full visual compatibility with Chromium.

See [OVERVIEW.md](OVERVIEW.md), [IMPLEMENTATION.md](IMPLEMENTATION.md), and
[PLAN.md](PLAN.md) for the deeper roadmap and benchmark history.

## Quick Start

### Requirements

- Rust 1.86 or newer
- Cargo

### Build

```bash
cargo build --release
```

### Convert HTML to PDF

```bash
cargo run --release -p htmltopdf-cli -- examples/invoice.html out/invoice.pdf
```

Embed a font by file path or installed system family name:

```bash
cargo run --release -p htmltopdf-cli -- --font Georgia examples/invoice.html out/invoice.pdf
cargo run --release -p htmltopdf-cli -- --font /path/to/font.ttf input.html output.pdf
```

## CLI

```bash
htmltopdf [--font <path|family>] <input.html> <output.pdf>
htmltopdf bench <input.html> <output-dir> [runs]
htmltopdf bench-concurrent <input.html> <output-dir> <workers> <runs-per-worker>
```

Examples:

```bash
cargo run --release -p htmltopdf-cli -- reg-2-9-1.html out/report.pdf
cargo run --release -p htmltopdf-cli -- bench reg-2-9-1.html out/bench 10
cargo run --release -p htmltopdf-cli -- bench-concurrent reg-2-9-1.html out/bench 16 4
```

## HTTP Server

Start the server:

```bash
cargo run --release -p htmltopdf-server
```

By default it binds to `127.0.0.1:8080`. You can override the address and worker
count:

```bash
HTMLTOPDF_WORKERS=24 cargo run --release -p htmltopdf-server -- 0.0.0.0:9000
```

Endpoints:

| Method | Path | Description |
| --- | --- | --- |
| `POST` | `/render` | Request body is HTML, response is `application/pdf` |
| `GET` | `/health` | Liveness check |
| `GET` | `/` | Usage text |

Render with `curl`:

```bash
curl -X POST http://127.0.0.1:8080/render \
  -H 'Content-Type: text/html' \
  --data-binary @examples/invoice.html \
  -o invoice.pdf
```

Render with options:

```bash
curl -X POST 'http://127.0.0.1:8080/render?landscape=true&margin=36&font=Georgia' \
  --data-binary @examples/invoice.html \
  -o invoice.pdf
```

Supported query parameters:

| Parameter | Example | Description |
| --- | --- | --- |
| `landscape` | `true` | Force A4 landscape output |
| `margin` | `36` | Set all page margins in PDF points |
| `font` | `Georgia` | Embed a font by family name or file path |

Load-test the API:

```bash
cargo run --release -p htmltopdf-server -- 127.0.0.1:8123
scripts/api-convert.sh -c 16 -n 64
scripts/api-convert.sh -c 8 -n 32 -q 'landscape=true&font=Georgia'
```

## Rust API

```rust
use htmltopdf::{Engine, FontSource, RenderOptions};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let html = "<h1>Invoice</h1><p>Hello from Rust.</p>";

    let options = RenderOptions::default()
        .with_font(&FontSource::Family("Georgia".to_string()))?;

    let pdf = Engine::new().render_html(html, options)?;
    std::fs::write("invoice.pdf", pdf)?;

    Ok(())
}
```

## Architecture

The workspace contains three crates:

```text
crates/htmltopdf          Core rendering engine
crates/htmltopdf-cli      Command-line interface and benchmark commands
crates/htmltopdf-server   Lightweight thread-pooled HTTP API
```

Important engine modules:

| File | Responsibility |
| --- | --- |
| `dom.rs` | `html5ever` integration and compact arena DOM |
| `html.rs` | CSS parsing, cascade, computed styles, document extraction |
| `box_tree.rs` | Nested flow box tree |
| `layout.rs` | Pagination, text wrapping, tables, and page layout |
| `paint.rs` | Backend-neutral display-list commands |
| `pdf.rs` | PDF writer, compression, embedded fonts, ToUnicode maps |
| `font.rs` | Font loading, metrics, encoding, and system font lookup |

The display-list boundary is intentional. Layout produces neutral paint
commands; the PDF backend consumes them. That keeps the engine extensible for
future rendering targets and makes layout independent from raw PDF syntax.

## Performance

The current benchmark fixture is `reg-2-9-1.html`, a real-world 1.8 MB
spreadsheet-like HTML file with roughly 22k table cells.

Early project measurements from [IMPLEMENTATION.md](IMPLEMENTATION.md):

| Scenario | Result |
| --- | --- |
| Single render, early table-aware layout | about 0.15s, about 20.6 MB peak RSS |
| Wrapped table layout | about 189 ms average over 5 runs |
| CSS page margins and row height | about 195 ms average over 5 runs |
| Parsed CSS cell styles | about 218 ms average over 5 runs |
| 16-worker benchmark | about 23-25 ms average wall time per PDF in earlier runs |

These numbers are development baselines, not a final performance guarantee.
Every major rendering feature should be benchmarked against fixed fixtures so
speed and memory stay visible as fidelity improves.

## Roadmap

- Broaden CSS selectors and properties.
- Add image support.
- Add SVG support.
- Broaden font subsetting and non-Latin text measurement.
- Add visual comparison tests against browser output.
- Add bounded pre-layout JavaScript through a runtime abstraction.
- Add more CSS layout modes, including absolute positioning, flexbox, and grid.
- Harden the HTTP server for production deployment patterns.

## Contributing

Contributions are welcome. The project is still young, so the best
contributions are focused, measured, and tied to tests.

### Good first contributions

- Add small HTML/CSS regression fixtures.
- Improve docs and examples.
- Add focused CSS property support with tests.
- Add PDF writer tests for new paint operators.
- Improve benchmark scripts and fixture metadata.

### Development workflow

1. Fork the repository and create a feature branch.
2. Keep changes scoped to one behavior or subsystem.
3. Add or update tests for the behavior you changed.
4. Run formatting and tests:

```bash
cargo fmt
cargo test
```

5. For rendering changes, generate a sample PDF:

```bash
cargo run --release -p htmltopdf-cli -- examples/invoice.html out/invoice.pdf
```

6. For performance-sensitive changes, run a benchmark:

```bash
cargo run --release -p htmltopdf-cli -- bench reg-2-9-1.html out/bench 5
cargo run --release -p htmltopdf-cli -- bench-concurrent reg-2-9-1.html out/bench 16 3
```

7. Update [IMPLEMENTATION.md](IMPLEMENTATION.md) when the change completes a
   checklist item or changes benchmark expectations.

### Pull request guidelines

- Explain the problem and the chosen approach.
- Include before/after notes for rendering behavior when useful.
- Include benchmark numbers when touching layout, parsing, PDF writing, or
  concurrency-sensitive code.
- Do not claim full browser compatibility for partial support.
- Prefer structured parsers and existing engine boundaries over one-off string
  scans.

## Design Principles

- Low RAM per render.
- Parallel rendering with no shared global mutable state.
- Real parser and cascade foundations before broad feature claims.
- Browser-compatible behavior over time, implemented honestly in layers.
- Deterministic server behavior with explicit limits for expensive features.

## License

This project is licensed under the MIT license.
