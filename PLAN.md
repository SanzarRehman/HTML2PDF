# Rust HTML-to-PDF Engine Plan

## 1. Goal

Build a fast, lightweight, embeddable HTML-to-PDF engine in Rust.

The first useful product is not a full browser replacement. The first useful
product is a reliable print-focused renderer for server-side PDFs: invoices,
reports, statements, forms, books, compliance exports, dashboards, and internal
business documents.

The long-term ambition is much larger: modern HTML, strong CSS coverage,
deterministic JavaScript execution before layout, high-quality pagination, low
memory usage, and predictable server behavior without depending on Chromium.

## 2. Why This Exists

Current options have clear gaps:

- Chromium/Puppeteer/Playwright produce strong browser-compatible PDFs, but the
  runtime is large and memory-heavy for high-volume server workloads.
- wkhtmltopdf is not a good long-term base. Its repository is archived and its
  Qt WebKit dependency is obsolete.
- iText pdfHTML is useful for controlled documents, but it is not a full
  browser-compatible HTML/CSS/JS engine.
- WeasyPrint proves that an independent HTML/CSS-to-PDF renderer can be built,
  but it intentionally avoids JavaScript and live browser behavior.
- Commercial tools such as Prince and Antenna House prove the market and
  technical feasibility, but they are closed commercial engines.

References:

- wkhtmltopdf status: https://wkhtmltopdf.org/status.html
- wkhtmltopdf archived repository: https://github.com/wkhtmltopdf/wkhtmltopdf
- iText pdfHTML support table: https://kb.itextpdf.com/itext/what-features-are-supported-or-unsupported-in-pdfh
- WeasyPrint internals: https://doc.courtbouillon.org/weasyprint/stable/going_further.html
- Prince: https://www.princexml.com/
- Antenna House Formatter: https://www.antenna.co.jp/AHF/
- Servo: https://servo.org/

## 3. Core Product Principles

1. Rust first.
   The engine should be memory-safe, embeddable, testable, and suitable for
   server environments.

2. Print-first, browser-aware.
   We should implement the parts of HTML/CSS that matter for PDF output first,
   while keeping the architecture compatible with broader browser behavior.
   Compatibility decisions should be based on HTML/CSS semantics and observable
   browser behavior, not fixture-specific visual guesses. Chromium is the main
   behavioral reference for web compatibility; CSS specifications are the
   architecture reference.

3. Deterministic by default.
   Rendering should be repeatable. Network access, filesystem access, timers,
   randomness, and JavaScript execution must be controlled by explicit options.

4. Fast cold starts.
   The engine should be usable in CLI tools, serverless jobs, workers, and
   containerized services without launching a browser process.

5. Progressive compatibility.
   We should publish a compatibility matrix and grow it honestly instead of
   claiming full CSS/JS support too early.

6. Heuristics are compatibility scaffolding, not standards.
   Temporary fixture heuristics may be used to learn, benchmark, or unblock
   early experiments, but default engine behavior should move toward parsed
   DOM, computed CSS, and spec/browser-defined layout behavior. Any heuristic
   that conflicts with Chromium/CSS semantics must be removed, gated behind an
   explicit compatibility mode, or documented as experimental.

7. Modular internals.
   HTML parsing, CSS parsing, style resolution, layout, painting, PDF writing,
   JavaScript, fonts, and image handling should be separable enough to replace
   or improve over time.

## 4. Non-Goals For Version 1

The first version should not try to support everything:

- No full browser event loop.
- No general web navigation.
- No arbitrary browser extension APIs.
- No remote resource loading unless explicitly enabled.
- No interactive PDF forms unless added as a later focused feature.
- No guarantee that arbitrary SPA applications render correctly.
- No attempt to match Chromium pixel-for-pixel.

## 5. Target Use Cases

Initial use cases:

- Invoices and receipts.
- Business reports.
- Long tables with repeated headers.
- Static charts rendered as SVG, image, or supported canvas output.
- Contracts and forms.
- Multi-page documents with headers, footers, page numbers, and bookmarks.
- Server-side PDF generation from templated HTML.

Later use cases:

- JavaScript-mutated DOM before layout.
- Chart libraries that render to SVG or canvas.
- Tailwind/Bootstrap-style layouts.
- More complete CSS Grid/Flexbox support.
- Complex multilingual documents.
- Book publishing features.

## 6. High-Level Architecture

Pipeline:

```text
Input HTML
  -> HTML parser
  -> DOM tree
  -> CSS parser
  -> stylesheet list
  -> cascade and computed styles
  -> optional JavaScript pre-layout execution
  -> box tree
  -> layout tree
  -> page fragmentation
  -> display list
  -> PDF backend
  -> output PDF
```

Architecture reality check:

- The 285 ms fixture benchmark is an early vertical-slice signal, not a mature
  engine budget. It excludes full HTML parsing, full CSS cascade, real font
  metrics, font embedding, images, JavaScript, flex/grid, floats, and broader
  browser compatibility behavior.
- Runtime must stay close to linear in document size. Selector matching,
  table sizing, pagination, and repeated headers must avoid full-document
  relayout loops, repeated backward scans, and per-cell global searches.
- Browser compatibility should be added through computed styles and layout
  algorithms, not visual patching. Chromium is the behavioral reference, but
  the engine should only claim support for the CSS features implemented and
  tested.
- JavaScript is especially risky for time and memory. The initial dynamic HTML
  model should execute scripts in a bounded pre-layout phase, then freeze the
  DOM for style/layout. Browser-like event-loop behavior is a later
  compatibility mode, not the default server rendering path.

Main crates/modules:

```text
htmltopdf
  crates/
    engine/          Core orchestration API
    html/            HTML parsing and DOM construction
    css/             CSS parsing, selector matching, cascade
    style/           Computed style model
    layout/          Block, inline, table, flex, grid, pagination
    text/            Font discovery, shaping, bidi, line breaking
    paint/           Display list and painting model
    pdf/             PDF writer backend
    js/              Optional JavaScript runtime integration
    assets/          Images, SVG, fetch/file resource policy
    cli/             Command-line interface
    server/          Optional HTTP service wrapper
```

## 7. Rust Technology Choices

Candidate libraries:

- HTML parsing: `html5ever`.
- CSS parsing: `cssparser`, `selectors`, and possibly Servo/Stylo components.
- Text shaping: HarfBuzz through Rust bindings.
- Unicode line breaking: `unicode-linebreak`.
- Bidi: `unicode-bidi`.
- Fonts: `fontdb`, `ttf-parser`, `allsorts`, or HarfBuzz/Freetype bindings.
- Images: `image`, `resvg`, `usvg`, `tiny-skia`.
- PDF writing: evaluate `pdf-writer`, `printpdf`, `lopdf`, Cairo bindings, or a
  custom writer once requirements are clear.
- JavaScript: evaluate QuickJS, Boa, and V8.
- Testing: `insta`, visual image diffs, WPT subsets, PDF validators.

Initial recommended direction:

- Use `html5ever` for HTML.
- Use Servo ecosystem crates where practical for CSS parsing/selectors.
- Start with a custom display list.
- Start with a Rust-native PDF writer if it can handle fonts/images cleanly.
  Use Cairo only if it greatly reduces early complexity.
- Delay JavaScript until static layout is credible.

## 8. JavaScript Strategy

JavaScript support should arrive in controlled stages.

Stage 1: no JavaScript.

Stage 2: deterministic pre-layout JavaScript.

- Execute scripts before layout.
- Allow DOM mutation.
- Disable timers by default.
- Disable network by default.
- Disable filesystem by default.
- Provide an execution timeout.
- Provide a memory limit where the runtime allows it.

Stage 3: browser-like APIs where needed.

- `document.querySelector`.
- DOM creation/mutation.
- Basic events only if required.
- `getComputedStyle` after style resolution.
- Limited canvas support.
- Optional `fetch` through an allowlisted resource loader.

Runtime candidates:

- QuickJS: small and embeddable, good for lightweight deterministic execution.
- Boa: Rust-native, attractive long term, but compatibility must be tested.
- V8: best compatibility, heavier integration and larger binary/runtime cost.

Recommended initial choice:

- Prototype QuickJS and Boa.
- Keep the JS layer behind a trait so V8 can be added later for compatibility
  mode if needed.

## 9. CSS Support Roadmap

Version 0.1:

- Basic selectors.
- Cascade and inheritance.
- Inline styles.
- External stylesheets from allowed sources.
- `display: block`, `inline`, `inline-block`, `none`.
- Margins, padding, borders.
- Background colors and simple background images.
- Font family, size, weight, style.
- Text alignment.
- Width/height/min/max basics.
- Basic page size and margins.

Version 0.2:

- Tables.
- Repeated table headers and footers.
- Page breaks.
- `@page`.
- Running headers/footers.
- Counters and page numbers.
- Links and bookmarks.

Version 0.3:

- Better inline layout.
- Line height correctness.
- Text shaping.
- Font fallback.
- RTL and bidi.
- Hyphenation.
- SVG images.

Version 0.4:

- Flexbox.
- Absolute and fixed positioning.
- Stacking contexts.
- Z-index.
- Overflow behavior.
- Transforms subset.

Version 0.5:

- CSS Grid subset.
- CSS custom properties.
- Media queries for print/screen.
- More complete fragmentation behavior.

Long term:

- More transforms.
- Filters where practical.
- Writing modes.
- Advanced generated content.
- Better compatibility with utility CSS frameworks.

Key specs:

- CSS Paged Media: https://www.w3.org/TR/css-page-3/
- CSS Fragmentation: https://www.w3.org/TR/css-break-3/
- Generated Content for Paged Media: https://www.w3.org/TR/css-gcpm-3/

## 10. Pagination Model

Pagination is the core product differentiator. It must be designed early, not
patched in later.

Required concepts:

- Page boxes.
- Page margin boxes.
- Fragmentainers.
- Break opportunities.
- Forced breaks.
- Avoid breaks.
- Orphans and widows.
- Repeated table headers.
- Fixed headers/footers.
- Running elements.
- Page counters.

Initial simplification:

- Implement block fragmentation first.
- Then tables.
- Then inline fragmentation improvements.
- Then flex/grid fragmentation.

## 11. PDF Backend Requirements

The PDF backend must support:

- Multi-page output.
- Vector drawing.
- Text drawing.
- Font embedding.
- Font subsetting.
- Images.
- Links.
- Outlines/bookmarks.
- Metadata.
- Compression.
- Optional PDF/A later.
- Optional tagged PDF/accessibility later.

Decision to make after prototype:

- If a Rust-native writer is sufficient, use it.
- If font/text/PDF correctness becomes expensive, consider Cairo or a focused
  lower-level PDF backend.
- Avoid tying layout logic to the PDF writer. The layout engine should emit a
  display list that can be rendered to PDF, PNG, or test snapshots.

## 12. Servo Track

Servo is the most important long-term option for broader browser compatibility.

We should not depend on Servo for the first MVP, because print/PDF output is
itself a large feature area. But we should keep a parallel research track:

- Can Servo layout produce a display list suitable for print output?
- How much of Stylo can we reuse without adopting the whole engine?
- Can we embed Servo as a compatibility mode later?
- Can our PDF backend consume Servo-like display primitives?

Decision checkpoint:

- After our static engine reaches usable 0.2 quality, spend 2-4 weeks building
  a Servo PDF proof of concept.

Possible future shape:

- `htmltopdf-core`: our print-first renderer.
- `htmltopdf-servo`: heavier compatibility backend for browser-like pages.

## 13. Performance Goals

Initial targets should be measured against Chromium, WeasyPrint, and Prince
where possible.

Early benchmark dimensions:

- Cold start latency.
- Peak RSS.
- PDF generation time.
- Pages per second.
- Output PDF size.
- Font handling time.
- Image decode time.
- Large table performance.

Suggested v0.1 targets:

- CLI cold start under 100 ms for simple local HTML.
- Simple one-page invoice under 100 ms after process start.
- Peak RSS under 100 MB for simple documents.
- No browser subprocess.

Suggested v0.3 targets:

- 100-page table/report generation under 2 seconds for controlled inputs.
- Peak RSS materially below Chromium for the same document.
- Stable output across runs.

## 14. Test Strategy

Test levels:

- Unit tests for parser adapters, cascade, lengths, colors, counters.
- Layout tests using small HTML fixtures.
- Visual regression tests by rendering PDFs to PNGs and comparing images.
- Text extraction tests to ensure searchable/selectable text.
- PDF validation tests.
- Fuzzing for parser and layout crashes.
- Compatibility fixtures against Chromium/WeasyPrint/Prince.

Fixture groups:

- Basic block layout.
- Inline text and wrapping.
- Tables.
- Pagination.
- Headers/footers.
- Fonts.
- Images.
- RTL/bidi.
- Flexbox.
- Grid.
- JavaScript DOM mutation.

Visual testing workflow:

1. Render HTML to PDF using our engine.
2. Render PDF pages to PNG.
3. Compare against approved snapshots.
4. Allow small anti-aliasing tolerances.
5. Store fixtures and snapshots in version control.

## 15. Security Model

Default mode must be safe for untrusted-ish server input, though not a complete
browser sandbox at first.

Default restrictions:

- No network.
- No filesystem reads except explicitly allowed input paths.
- No JavaScript unless enabled.
- JavaScript timeout when enabled.
- Maximum document size.
- Maximum image size.
- Maximum page count.
- Maximum CSS rules.
- Maximum recursion/layout depth.

Later hardening:

- Process isolation mode.
- WASM/plugin isolation if needed.
- Fuzzing in CI.
- Resource accounting.
- Denial-of-service regression corpus.

## 16. CLI Shape

Initial CLI:

```bash
htmltopdf input.html output.pdf
```

Useful options:

```bash
htmltopdf input.html output.pdf \
  --base-url ./ \
  --page-size A4 \
  --margin 20mm \
  --allow-file ./assets \
  --allow-network none \
  --javascript off
```

Later options:

```bash
htmltopdf input.html output.pdf \
  --javascript pre-layout \
  --timeout 5s \
  --max-pages 500 \
  --pdf-a \
  --outline \
  --dump-layout layout.json \
  --dump-display-list paint.json
```

## 17. Public API Shape

Rust API sketch:

```rust
use htmltopdf::{Engine, RenderOptions};

let engine = Engine::new();
let pdf = engine.render_html(
    include_str!("invoice.html"),
    RenderOptions::default()
        .page_size_a4()
        .base_url("./")
        .javascript(false),
)?;

std::fs::write("invoice.pdf", pdf)?;
```

Server API should be separate from the core engine. The core library should not
assume HTTP, async runtimes, or a specific deployment model.

## 18. Milestones

### Milestone 0: Repository and Baseline Research

Duration: 1-2 weeks.

Deliverables:

- Rust workspace.
- CLI skeleton.
- Initial architecture decision records.
- Benchmark corpus.
- Competitor comparison harness.
- Basic PDF writer proof of concept.

Exit criteria:

- We can run one command that renders a hardcoded PDF.
- We can run benchmark scripts against Chromium and at least one non-Chromium
  engine.

### Milestone 1: Static MVP

Duration: 6-10 weeks.

Deliverables:

- Parse HTML.
- Parse CSS.
- Build basic DOM.
- Resolve basic styles.
- Block layout.
- Inline text layout.
- Images.
- Simple pagination.
- PDF output.
- CLI.

Exit criteria:

- Render simple invoices and one-page reports.
- Text is selectable in PDF.
- Fonts are embedded.
- Visual snapshots pass in CI.

### Milestone 2: Real Documents

Duration: 8-12 weeks.

Deliverables:

- Tables.
- Page breaks.
- Repeated table headers.
- `@page`.
- Headers and footers.
- Page numbers.
- Links and outlines.
- Better font fallback.

Exit criteria:

- Render long reports with tables.
- Generate 100+ page documents without runaway memory.
- Beat Chromium memory usage materially on controlled fixtures.

### Milestone 3: Typography and International Text

Duration: 6-10 weeks.

Deliverables:

- HarfBuzz shaping.
- Bidi.
- Better line breaking.
- Hyphenation.
- Font subsetting.
- CJK and RTL fixtures.

Exit criteria:

- Correct rendering for representative English, Arabic, Hebrew, Bengali, Hindi,
  Chinese, and Japanese fixtures.
- Output remains searchable/selectable where possible.

### Milestone 4: Modern Layout

Duration: 12-20 weeks.

Deliverables:

- Flexbox.
- Absolute/fixed positioning.
- Stacking contexts.
- Z-index.
- Overflow.
- Transform subset.
- CSS variables.
- Print media queries.

Exit criteria:

- Render common Bootstrap/Tailwind document-style templates.
- No major layout instability in regression corpus.

### Milestone 5: JavaScript Pre-Layout

Duration: 8-16 weeks.

Deliverables:

- JS runtime integration.
- Minimal DOM APIs.
- Script execution before layout.
- Runtime timeout.
- Resource policy.
- DOM mutation tests.

Exit criteria:

- HTML templates that generate DOM through JS render correctly.
- Network-disabled mode is deterministic.
- JS cannot hang the renderer beyond configured timeout.

### Milestone 6: Charts and SVG/Canvas

Duration: 8-16 weeks.

Deliverables:

- Better SVG rendering.
- Canvas 2D subset or adapter.
- Chart library compatibility tests.
- Image/vector preservation where possible.

Exit criteria:

- Representative Chart.js, ECharts, or D3-generated outputs can be rendered
  through a documented path.

### Milestone 7: Compatibility and Servo R&D

Duration: ongoing, with a focused 2-4 week spike after Milestone 2.

Deliverables:

- Servo print/PDF feasibility prototype.
- WPT subset runner.
- Compatibility dashboard.
- Decision on whether Servo becomes a second backend.

Exit criteria:

- Clear decision: stay custom-only, reuse Servo components, or build a Servo
  backend.

## 19. Team Shape

Minimum serious team:

- 1 Rust systems engineer.
- 1 layout/rendering engineer.
- 1 PDF/text/font engineer.
- 1 test/infra engineer part time.

Ideal team:

- 2 Rust/layout engineers.
- 1 typography/font engineer.
- 1 JavaScript/runtime engineer.
- 1 QA/compatibility engineer.

## 20. Main Risks

1. CSS layout scope explosion.
   Mitigation: publish a compatibility matrix and build fixture-driven support.

2. Pagination complexity.
   Mitigation: treat pagination as a core layout concept from day one.

3. Text shaping and fonts.
   Mitigation: integrate HarfBuzz early and add multilingual fixtures early.

4. JavaScript API scope.
   Mitigation: only support pre-layout deterministic JS first.

5. PDF backend limitations.
   Mitigation: keep a display-list boundary so the backend can be replaced.

6. Servo uncertainty.
   Mitigation: treat Servo as a parallel R&D path, not a blocking dependency.

7. Performance regressions.
   Mitigation: benchmark every milestone against fixed documents.

8. Security/DoS.
   Mitigation: limits, timeouts, fuzzing, and safe defaults.

## 21. First 30 Days

Week 1:

- Create Rust workspace.
- Choose initial crate layout.
- Add CLI skeleton.
- Add ADR template.
- Build benchmark fixture folder.
- Add simple competitor runner for Chromium if available locally.

Week 2:

- Implement hardcoded PDF output.
- Parse HTML into a DOM.
- Parse inline styles and simple stylesheets.
- Define computed style structs.
- Create initial display list format.

Week 3:

- Implement block layout for simple documents.
- Implement text measurement placeholder.
- Render text, rectangles, borders, and backgrounds to PDF.
- Add first visual snapshot tests.

Week 4:

- Add basic pagination.
- Add image loading from allowlisted local paths.
- Add font embedding proof of concept.
- Render first invoice fixture end to end.

End-of-month demo:

- `htmltopdf examples/invoice.html out/invoice.pdf`
- PDF has real selectable text, basic styling, images, and one or more pages.
- Benchmark compares memory/time against Chromium for the same fixture.

## 22. Decision Log To Create Next

Create architecture decision records for:

- PDF backend choice.
- CSS parser and selector stack.
- Font/text stack.
- JavaScript runtime choice.
- Resource loading policy.
- Display list format.
- Servo integration strategy.

## 23. Current Recommended Next Step

Start the Rust repository with a narrow vertical slice:

1. HTML string in.
2. DOM tree.
3. Very small CSS subset.
4. Block layout.
5. Display list.
6. PDF bytes out.
7. One invoice fixture.
8. One benchmark against Chromium.

This keeps the project honest. If the first vertical slice is clean, every
later feature has a real place to attach.
