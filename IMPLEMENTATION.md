# Implementation Checklist

This document is the working checklist for the Rust implementation. When a task
is completed, update its checkbox in this file.

## Product Goal (restated)

A high-concurrency, low-RAM HTML-to-PDF engine with full CSS and JavaScript
support, built so that a single process can render many documents in parallel
across cores without launching a browser.

The competitors we are deliberately differentiating from:

- Chromium/Puppeteer: faithful, but huge RAM per renderer and a browser
  subprocess per job. We want order-of-magnitude lower RAM and no subprocess.
- iText / pdfHTML: controlled documents only, not a full browser-compatible
  HTML/CSS/JS engine, and not designed for cheap multi-core fan-out.
- wkhtmltopdf: archived, obsolete Qt WebKit base.

So the three things that must hold as the engine grows:

1. Low RAM per render (index/arena data, no per-node heap churn, streaming PDF).
2. Embarrassingly parallel (each render is independent and `Send`; no global
   state; cores scale linearly).
3. Real CSS + JS fidelity over time (a real DOM, a real cascade, real font
   metrics, and a pre-layout JS stage).

## Architecture Pivot (2026-06-29)

The first sprint proved the low-RAM / high-concurrency thesis on a real
1.8 MB / 22k-cell spreadsheet fixture: ~12 MB peak footprint per render and
~27 ms/PDF at 32 workers. That signal is good and we keep it.

But the vertical slice was built on a **flat `Vec<Block>` model with a
hand-rolled char scanner and substring-search CSS**, fitted to one
PhpSpreadsheet fixture. That model cannot carry "full CSS + JS", because:

- There is no DOM tree, so nesting, inheritance, and stacking cannot be
  represented.
- CSS matching is `input.find(selector)` over raw text, which is O(rules x doc)
  and matches inside comments/attributes/`@media`. It is not a cascade.
- Text width is a single `font_size * 0.52` constant, so line breaking, column
  fit, and pagination are all guesses and cannot be validated against a browser.

Decision: stop adding features on the flat model and build the real spine,
foundation-first, in this order. Each layer is replaceable behind a boundary so
we never lose the concurrency/RAM properties.

```text
Input HTML
  -> html5ever            real, spec-compliant parsing
  -> arena DOM            Vec<Node> with index children (cache-friendly, low RAM)
  -> CSS (cssparser)      real stylesheet model
  -> cascade + computed   selector matching, specificity, inheritance
  -> box tree             from computed display
  -> layout               block/inline/table, real font metrics
  -> fragmentation        pagination
  -> display list         backend-neutral paint commands (already in place)
  -> PDF writer           streaming, compressed (already in place)
```

The display-list boundary (ADR 0001) and the streaming PDF writer stay. The
parser, DOM, CSS, cascade, and font metrics get rebuilt. See
`docs/adr/0002-dom-based-pipeline.md`.

### Crate dependency choices (locked)

- HTML parsing: `html5ever` + `markup5ever_rcdom` (parse with the proven
  tokenizer/tree-builder, then lower into our own arena DOM so downstream code
  never touches `Rc`).
- CSS parsing: `cssparser` (+ `selectors` later for full combinator matching).
- Font metrics: standard-14 Helvetica AFM width table now (deterministic, zero
  deps, matches the font we actually emit). `ttf-parser` + `fontdb` arrive with
  font embedding/subsetting, not before, so metrics always match the embedded
  font.
- JavaScript: deferred to its own milestone; QuickJS/Boa behind a trait.

## Current Sprint: Foundation Rebuild

- [x] Verify the workspace builds and all tests pass before the pivot.
- [x] Confirm html5ever/cssparser/ttf-parser/fontdb resolve and compile.
- [x] Add real font metrics: standard-14 Helvetica AFM width table (`font.rs`).
- [x] Replace `font_size * 0.52` width guesses in layout with measured widths.
- [x] Add `html5ever`-backed arena DOM (`dom.rs`) with parsing tests.
- [x] Route generic block extraction (headings/paragraphs/lists) through the DOM.
- [x] Route table row/cell extraction through the DOM (replace raw-text scan).
- [x] Skip the RcDom intermediate with a custom `html5ever` `TreeSink` (RAM).
- [x] Replace substring CSS rule lookup with a `cssparser` stylesheet model.
- [x] Build computed-style cascade over the DOM (specificity + inheritance).
- [x] Generate flow-content boxes from computed `display` (skip `display:none`,
      carry computed style). Full nested box-tree layout still to come.

## Previous Sprint: Minimal Vertical Slice

- [x] Create the Rust workspace.
- [x] Add the core engine crate.
- [x] Add the CLI crate.
- [x] Implement basic HTML text extraction.
- [x] Implement simple page layout.
- [x] Implement a minimal PDF writer.
- [x] Add an example HTML fixture.
- [x] Verify the project with `cargo test`.
- [x] Verify the CLI can generate a PDF.
- [x] Add `.gitignore` entries for generated build/output directories.
- [x] Copy real-world `reg-2-9` HTML fixture into the workspace.
- [x] Convert `reg-2-9-1.html` with the Rust CLI.
- [x] Run a 16-way concurrent conversion smoke test.
- [x] Record first time and memory baseline for the real fixture.
- [x] Add a benchmark command for repeated conversions.
- [x] Add fixture metadata for `reg-2-9-1.html`.
- [x] Detect landscape page hint from the fixture CSS.
- [x] Parse spreadsheet column widths from the fixture CSS.
- [x] Parse table rows, cells, and colspans.
- [x] Infer basic cell alignment and border styles from spreadsheet classes.
- [x] Render basic table cells and borders to PDF.
- [x] Re-run single and 16-way conversion benchmarks after table rendering.
- [x] Add a built-in concurrent benchmark command.
- [x] Wrap table cell text across multiple lines.
- [x] Grow table row height based on wrapped cell content.
- [x] Re-run single and concurrent benchmarks after wrapped row layout.
- [x] Parse `@page` margins from fixture CSS.
- [x] Parse `table.sheet0 tr` row height from fixture CSS.
- [x] Apply asymmetric page margins in layout.
- [x] Use CSS row height as the table row minimum height.
- [x] Re-run single and concurrent benchmarks after CSS page layout.
- [x] Parse spreadsheet cell style classes from CSS.
- [x] Apply parsed cell font sizes in table layout.
- [x] Apply parsed cell padding in table layout.
- [x] Re-run single and concurrent benchmarks after CSS cell style parsing.
- [x] Experiment with shared table border line drawing.
- [x] Reject shared table border line drawing after benchmark regression.
- [x] Add initial display-list paint command layer.
- [x] Route layout output through paint commands.
- [x] Route PDF backend output through paint commands.
- [x] Add ADR for display-list rendering architecture.
- [x] Add PDF stream compression with `/FlateDecode`.
- [x] Re-run single and concurrent benchmarks after PDF compression.
- [x] Add display-list clip commands for scoped painting.
- [x] Add PDF backend support for clipping paths.
- [x] Parse table-cell `overflow` and `white-space` style hints.
- [x] Parse `overflow-wrap`, `word-wrap`, and `word-break` style hints.
- [x] Clip long unspaced table-cell tokens without forcing browser-inaccurate breaks.
- [x] Break long unspaced table-cell tokens only when CSS explicitly allows it.
- [x] Clip table-cell text to its content box.
- [x] Add regression tests for clipped table-cell text and PDF clip operators.
- [x] Re-render and benchmark `reg-2-9-1.html` after cell clipping.
- [x] Add first-pass table fit-to-page paint scaling for wide declared columns.
- [x] Preserve full-span unbordered caption/title cells during table paint scaling.
- [x] Add regression tests for table paint scaling and caption-row scale policy.
- [x] Re-render and benchmark `reg-2-9-1.html` after table shrink scaling.
- [x] Add first-pass repeated table headers for semantic `TableHeaderRow` rows.
- [x] Parse `<thead>` and `<tfoot>` table section rows.
- [x] Remove appearance-based repeated-header heuristic from default behavior.
- [x] Add regression tests for semantic table section parsing and header repetition.
- [x] Re-render and benchmark `reg-2-9-1.html` after semantic header correction.
- [x] Parse inline and class-based CSS `display: table-header-group`.
- [x] Parse inline and class-based CSS `display: table-footer-group`.
- [x] Add regression tests for CSS table header/footer group row classification.
- [x] Re-render and benchmark `reg-2-9-1.html` after CSS display table-section parsing.
- [x] Add architecture note that early fixture time is not a mature performance guarantee.
- [x] Replace class-map styling with first-pass ordered selector cascade.
- [x] Add regression tests for selector specificity and source-order cascade.
- [x] Pre-index selector rules by tag/class before broadening CSS support.
- [x] Cache repeated computed cell styles for spreadsheet-like tables.
- [x] Re-render and benchmark `reg-2-9-1.html` after first-pass selector cascade.
- [x] Run high-concurrency benchmark after first-pass selector cascade.
- [x] Add first-pass `!important` cascade priority for supported CSS declarations.
- [x] Add regression tests for `!important` priority and specificity.
- [x] Re-render and benchmark `reg-2-9-1.html` after `!important` cascade support.
- [x] Add first-pass CSS text color and cell background-color parsing.
- [x] Add PDF fill/stroke color paint commands.
- [x] Paint non-white table-cell backgrounds and colored table-cell text.
- [x] Add regression tests for color parsing, display-list color commands, and PDF color operators.
- [x] Re-render and benchmark `reg-2-9-1.html` after first-pass color support.
- [x] Add first-pass CSS table-cell `vertical-align` parsing.
- [x] Apply explicit top/middle/bottom/baseline vertical alignment in table-cell layout.
- [x] Add regression tests for parsed and applied table-cell vertical alignment.
- [x] Re-render and benchmark `reg-2-9-1.html` after vertical-align support.

## Baseline: `reg-2-9-1.html`

Input:

- File: `reg-2-9-1.html`
- Size: 1,820,215 bytes
- Detected features in quick scan: inline `<style>`, `@media`, one `<table>`,
  and `position:absolute`

Initial Rust output before table-aware layout:

- Command: `target/debug/htmltopdf reg-2-9-1.html out/reg-single.pdf`
- Result: succeeds
- Output size: about 321 KB
- Single conversion wall time: about 0.08 seconds
- Peak RSS from `/usr/bin/time -l`: about 18.2 MB
- Peak memory footprint: about 13.6 MB
- 16 concurrent conversions: succeeds
- 16 concurrent wall time: about 0.164 seconds

Current Rust output after table-aware layout:

- Command: `target/debug/htmltopdf reg-2-9-1.html out/reg-table-memory.pdf`
- Result: succeeds
- Output size: about 1.7 MB
- Single conversion wall time: about 0.15 seconds
- Peak RSS from `/usr/bin/time -l`: about 20.6 MB
- Peak memory footprint: about 11.8 MB
- 5-run benchmark average: about 149 ms
- 16 concurrent conversions: succeeds
- 16 concurrent wall time: about 0.292 seconds

Current Rust output after wrapped table row layout:

- Command: `target/debug/htmltopdf reg-2-9-1.html out/reg-wrapped-memory.pdf`
- Result: succeeds
- Output size: about 1.7 MB
- Page objects: 48
- Single conversion wall time: about 0.16 seconds
- Peak RSS from `/usr/bin/time -l`: about 21.1 MB
- Peak memory footprint: about 11.8 MB
- 5-run benchmark average: about 189 ms
- Built-in 16-worker benchmark wall time: about 370 ms
- Built-in 16-worker average wall time per PDF: about 23 ms

Current Rust output after CSS page margins and row height:

- Command: `target/debug/htmltopdf reg-2-9-1.html out/reg-css-page-memory.pdf`
- Result: succeeds
- Output size: about 1.7 MB
- Page objects: 49
- Single conversion wall time: about 0.17 seconds
- Peak RSS from `/usr/bin/time -l`: about 20.9 MB
- Peak memory footprint: about 11.8 MB
- 5-run benchmark average: about 195 ms
- Built-in 16-worker benchmark wall time: about 377 ms
- Built-in 16-worker average wall time per PDF: about 24 ms

Current Rust output after parsed CSS cell styles:

- Command: `target/debug/htmltopdf reg-2-9-1.html out/reg-css-styles-memory.pdf`
- Result: succeeds
- Output size: about 1.8 MB
- Page objects: 63
- Single conversion wall time: about 0.18 seconds
- Peak RSS from `/usr/bin/time -l`: about 24.3 MB
- Peak memory footprint: about 11.9 MB
- 5-run benchmark average: about 218 ms
- Built-in 16-worker benchmark wall time: about 408 ms
- Built-in 16-worker average wall time per PDF: about 25 ms

Rejected experiment: shared table border line drawing:

- Goal: reduce duplicate borders by drawing shared horizontal/vertical lines.
- Result: not kept.
- Naive stroke dedupe: about 456 ms 5-run average, about 2.7 MB output.
- Hash-based stroke dedupe: about 270 ms 5-run average, about 2.7 MB output.
- Reason rejected: output became larger and latency was worse than the current
  rectangle path. Revisit only after PDF stream compression or path batching.

Current Rust output after PDF stream compression:

- Command: `target/debug/htmltopdf reg-2-9-1.html out/reg-compressed-memory.pdf`
- Result: succeeds
- Output size: about 287 KB
- Page objects: 63
- Single conversion wall time: about 0.25 seconds
- Peak RSS from `/usr/bin/time -l`: about 23.7 MB
- Peak memory footprint: about 11.9 MB
- 5-run benchmark average: about 248 ms
- Built-in 16-worker benchmark wall time: about 514 ms
- Built-in 16-worker average wall time per PDF: about 32 ms

Current Rust output after table-cell clipping and corrected word breaking:

- Command: `target/debug/htmltopdf reg-2-9-1.html out/reg-2-9-1-token-clip.pdf`
- Result: succeeds
- Output size: about 465 KB
- Page objects: 62
- 3-run benchmark average: about 278 ms
- Visible improvement: long emails, IDs, and other unspaced tokens no longer
  bleed outside their cell boxes, and they no longer force row-height/page-count
  expansion unless CSS explicitly allows long-token breaking.
- Remaining limitation: narrow columns now clip content horizontally because the
  engine still lacks proper CSS table min/max-content sizing and font metrics.

Current Rust output after first-pass table fit-to-page paint scaling:

- Command: `target/debug/htmltopdf reg-2-9-1.html out/reg-2-9-1-table-shrink-caption.pdf`
- Result: succeeds
- Output size: about 458 KB
- Page objects: 44
- 3-run benchmark average: about 301 ms
- Visible improvement: the dense spreadsheet grid now fits the page much more
  like a scaled wide sheet, while full-span unbordered title rows keep their
  readable declared size.
- Remaining limitation: this is a pragmatic shrink strategy, not a complete CSS
  table algorithm. It still needs real min/max-content sizing, actual font
  metrics, border-collapse, and visual comparison against Chromium.

Corrected repeated-header implementation:

- Command: `target/debug/htmltopdf reg-2-9-1.html out/reg-2-9-1-semantic-header.pdf`
- Result: succeeds
- Output size: about 458 KB
- Page objects: 44
- 3-run benchmark average: about 283 ms
- Behavior correction: repeated headers now come from semantic table header
  rows (`<thead>` / inline `display: table-header-group`) instead of visual
  guesses such as bold bordered rows.
- Fixture note: `reg-2-9-1.html` does not declare a semantic table header group,
  so the engine no longer invents repeated headers for it by default.
- Remaining limitation: stylesheet-driven `display: table-header-group` through
  class selectors still needs the real CSS cascade/computed display model.

Current Rust output after CSS table-section display parsing:

- Command: `target/debug/htmltopdf reg-2-9-1.html out/reg-2-9-1-css-display-sections.pdf`
- Result: succeeds
- Output size: about 458 KB
- Page objects: 44
- 3-run benchmark average: about 285 ms
- Behavior improvement: rows inside sections styled with inline or class-based
  `display: table-header-group` / `display: table-footer-group` are now
  classified semantically before layout.
- Fixture note: `reg-2-9-1.html` still does not declare table header-group
  semantics, so output remains unchanged.
- Remaining limitation: this parses display from a simple class map, not the
  full CSS cascade with selector specificity, inheritance, media queries, and
  computed style.

Current Rust output after first-pass ordered selector cascade:

- Command: `target/debug/htmltopdf reg-2-9-1.html out/reg-2-9-1-cached-cascade.pdf`
- Result: succeeds
- Output size: 458,868 bytes
- Page objects: 44
- Single conversion wall time from `/usr/bin/time -l`: about 0.22 seconds
- Peak RSS from `/usr/bin/time -l`: about 23.5 MB
- Peak memory footprint: about 11.9 MB
- 3-run benchmark average: about 229 ms
- Behavior improvement: cell styles and table-section display now use ordered
  selector rules with tag/class matching, source order, and selector
  specificity for the supported selector subset.
- Performance correction: selector rules are pre-indexed by tag/class and
  repeated cell opening tags use a computed-style cache, avoiding a per-cell
  full stylesheet scan.
- Current concurrency behavior:
  - 8 workers x 3 runs: 24 PDFs in about 4.48 s, about 187 ms average wall
    time per PDF.
  - 16 workers x 3 runs: 48 PDFs in about 4.42 s, about 92 ms average wall
    time per PDF.
  - 32 workers x 3 runs: 96 PDFs in about 2.57 s under `/usr/bin/time -l`,
    about 27 ms average wall time per PDF, about 397 MB peak RSS.
  - 64 workers x 2 runs: 128 PDFs in about 3.44 s under `/usr/bin/time -l`,
    about 27 ms average wall time per PDF, about 770 MB peak RSS.
- Remaining limitation: this is still a first-pass cascade. It does not yet
  implement full CSS parsing, inheritance, selector combinator matching,
  pseudo-classes, media query evaluation, or browser-complete computed values.

Current Rust output after first-pass `!important` cascade support:

- Command: `target/debug/htmltopdf reg-2-9-1.html out/reg-2-9-1-important-cascade.pdf`
- Result: succeeds
- Output size: 455,123 bytes
- Page objects: 44
- 3-run benchmark average: about 231 ms
- Behavior improvement: supported CSS declarations now carry normal and
  important layers. Important declarations beat normal declarations, and
  specificity/source order still applies among important declarations.
- Behavior correction: spreadsheet class alignment inference now fills only
  missing alignment, so parsed CSS alignment wins when present.
- Remaining limitation: `!important` currently applies only to the supported
  declaration subset, because the engine still lacks a complete CSS parser and
  complete computed-style model.

Current Rust output after first-pass CSS color support:

- Command: `target/debug/htmltopdf reg-2-9-1.html out/reg-2-9-1-color.pdf`
- Result: succeeds
- Output size: 486,682 bytes
- Page objects: 44
- 3-run benchmark average: about 243 ms
- Behavior improvement: supported `color`, `background-color`, and simple
  `background` color values are parsed. The display list now has explicit
  fill/stroke color commands, and the PDF backend writes `rg`/`RG` operators.
- Performance note: output grew because text paint now emits explicit fill color
  state and the backend has color commands available. White table-cell
  backgrounds are intentionally skipped because the PDF page is already white.
- Remaining limitation: color parsing currently supports hex colors and a small
  named-color subset. It does not yet support `rgb()`, `rgba()`, `hsl()`,
  opacity, currentColor, inherited color, or full background layers.

Current Rust output after first-pass table-cell vertical alignment:

- Command: `target/debug/htmltopdf reg-2-9-1.html out/reg-2-9-1-vertical-align.pdf`
- Result: succeeds
- Output size: 491,807 bytes
- Page objects: 44
- 3-run benchmark average: about 244 ms
- Behavior improvement: supported `vertical-align` values are parsed for table
  cells. Explicit `top`, `middle`, `bottom`, and `baseline` values affect the
  text block position within the cell content box.
- Remaining limitation: this is a table-cell text-block alignment pass, not a
  full CSS inline/table baseline algorithm. Baseline currently behaves like top
  alignment until proper table row baseline calculation exists.

Current Rust output after routing table extraction through the DOM:

- Command: `target/release/htmltopdf reg-2-9-1.html out/reg-step3.pdf`
- Result: succeeds
- Output size: 492,740 bytes
- Output is **byte-identical** to the previous (raw-text) table render, so the
  DOM migration changed structure parsing without changing any rendered output.
- Single conversion wall time: about 0.07 s user.
- Peak RSS from `/usr/bin/time -l`: about 50 MB (up from ~27 MB).
- 16-worker x 3-run concurrency: 48 PDFs, about 13 ms average wall time per PDF.
- Behavior change: `<tr>`/`<td>`/`<th>` structure, colspans, `<thead>`/
  `<tbody>`/`<tfoot>` sections, and CSS `display: table-*-group` overrides now
  come from the real html5ever DOM instead of raw-text substring scanning.
  Malformed/mis-nested table HTML now follows the HTML tree-construction spec.
- Cost noted: peak RSS rose because the transient `RcDom` (`Rc<RefCell>`) tree
  is built for all 22k cells before lowering into the arena. Still under the
  100 MB target; a custom `TreeSink` that builds the arena directly is the
  follow-up to remove the intermediate (tracked above).

Current Rust output after the custom arena `TreeSink` (no RcDom):

- Command: `target/release/htmltopdf reg-2-9-1.html out/reg-sink.pdf`
- Result: succeeds; output **byte-identical** to the RcDom-based render.
- `html5ever`'s `TreeSink` is now implemented directly against the `Vec` arena,
  so there is no `Rc`/`RefCell` reference tree and no second copy of the document
  in memory during parsing.
- Peak RSS, single render: about 44 MB (was about 50 MB with RcDom).
- Peak RSS, 16 workers x 3 runs: about 557 MB (was about 737 MB with RcDom),
  i.e. ~35 MB/worker down from ~46 MB/worker — about 180 MB saved under
  concurrency. About 13 ms average wall time per PDF (unchanged).
- Correctness is pinned by `dom.rs::matches_rcdom_reference_tree`, which asserts
  the custom sink produces the same tree as the reference RcDom across nested,
  mis-nested, table, list, and full-document samples.
- Remaining cost: the per-render arena for 22k cells (owned `String` names/text)
  is the legitimate price of a real DOM and is still far under the 100 MB target
  and far below Chromium. String interning is a possible later optimization.

Current Rust output after the `cssparser`-based stylesheet:

- Command: `target/release/htmltopdf reg-2-9-1.html out/reg-css.pdf`
- Result: succeeds; output **byte-identical** to the previous render.
- The hand-rolled CSS tokenizer (`<style>` substring scan, `find('{')` rule
  splitting, `split(';')` declaration splitting, `split(',')` selector
  splitting) is replaced by `cssparser`. `<style>` text now comes from the DOM.
- Reused unchanged: the cascade (specificity, source order, `!important`), the
  selector model (rightmost compound: tag + classes), and all value parsing
  (`apply_style_declaration`, `parse_css_color`, `parse_css_length`).
- New correctness, covered by tests: CSS comments anywhere (including inside
  selectors and declaration lists), `;`/`{` inside quoted values and `url()`,
  rules nested in `@media`, and multiple `<style>` elements.
- Peak RSS and throughput unchanged (~44 MB single, ~13 ms/PDF at 16 workers).
- Remaining: `@page` margins and column widths still use the `find_css_rule`
  substring scan; folding them into the `cssparser` stylesheet is the follow-up.

Current Rust output after the computed-style inheritance pass:

- Command: `target/release/htmltopdf reg-2-9-1.html out/reg-inherit.pdf`
- Result: succeeds; output **byte-identical** to the previous render.
- A single top-down DOM pass now computes every node's style: inheritable
  properties (color, font size, font weight, text alignment, white-space,
  wrapping) fall back to the parent's computed value; non-inheritable ones
  (border, padding, background, overflow, vertical-align) come from the node's
  own cascade only. Table cells read their precomputed style.
- Why the fixture is unchanged: its only inheritable ancestor declaration is
  `html { font-size: 11pt }`, and all 22,166 cells set their own font size via a
  style class, so the inherited value is always overridden. No ancestor sets an
  inheritable `color`/`text-align`/etc. that reaches a cell.
- Peak RSS, single render: about 49 MB (was ~44 MB) for the added per-node
  computed-style vector; throughput unchanged (~13 ms/PDF at 16 workers).
- New behavior, covered by tests: `color`/`font-size`/`text-align` inherit from
  ancestors (including through an implied `<tbody>`), an element's own value
  overrides the inherited one, and `border`/`background-color` do not inherit.

Current Rust output after display-driven flow-content boxes:

- Command: `target/release/htmltopdf reg-2-9-1.html out/reg-box.pdf`
- Result: succeeds; fixture output **byte-identical** (it uses the table path).
- Generic (non-table) documents now generate boxes from computed `display`:
  `display: none` subtrees are skipped, and each heading/paragraph block carries
  the computed style of the element that opened it. Layout renders generic blocks
  with computed font size, color, and text alignment (previously fixed 24/18/11pt,
  always black, always left-aligned).
- `Block` gained a `style` field; computed styles are now computed once in
  `parse()` and shared by the table and flow paths.
- Peak RSS ~50 MB, throughput ~13 ms/PDF at 16 workers (unchanged).
- New behavior, covered by tests: `display:none` hides flow content (class and
  inline), and flow blocks pick up `color`/`font-size`/`text-align` from their
  own rules and from ancestors.
- Remaining for the box tree: a fully nested block/inline box tree with
  box-tree-driven layout. Today flow boxes still flatten to a block list, and the
  table path keeps its specialized layout.

Expanded CSS value coverage (additive; fixture byte-identical):

- Colors: `rgb()`/`rgba()`/`hsl()`/`hsla()` (comma or space/slash syntax,
  percentage channels, alpha ignored), 4/8-digit hex, and a larger named-color
  set — on top of the existing hex and six names.
- `display: none` (tables and flow content), via a `hidden` flag computed in the
  style pass (no extra per-cell cost).
- `font-weight`: `bold`/`bolder`/numeric ≥ 600 all render bold.
- `text-align`: adds `justify` (→ left for now), `start` (→ left), `end` (→ right).
- Headings `h3`–`h6` with browser-like default sizes.

Font embedding (opt-in via `--font <path|family>`):

- `RenderOptions` carries an `Arc<Font>` (default Helvetica, not embedded).
  A file path or system family name loads a TrueType/OpenType font.
- `ttf-parser` provides metrics (advances, ascent/descent/cap-height/bbox,
  units-per-em) used by both layout measurement and the PDF; `fontdb` resolves
  family names from the system database.
- The PDF embeds a simple `/TrueType` font (`/WinAnsiEncoding`, `/Widths`,
  `/FontDescriptor`, compressed `/FontFile2`). Layout's width/wrap helpers now
  take the active font, so wrapping matches the embedded font's real metrics.
- Verified: `pdffonts` reports the font `emb yes`, `pdftotext` extracts the
  text, and default (no `--font`) output is byte-identical.
- Follow-ups: glyph subsetting (full font embedded today), and CID/`Identity-H`
  + `ToUnicode` for non-Latin/CJK.

Important limitation:

- This is now a fast spreadsheet-table PDF, but still not a fully faithful
  browser render. The fixture proves the low-memory/concurrency direction is
  viable, but the engine still needs nested box-tree layout, font subsetting,
  images, and visual validation before it can replace Chromium for documents
  like this.

## Roadmap (foundation-first, ordered)

The pivot reorders the backlog so the load-bearing layers land before more
feature polish. Items above the line are the spine; items below are features
that attach cleanly once the spine exists.

### Spine (do these first)

- [x] Replace ad hoc HTML parsing with `html5ever` (arena DOM).
- [x] Add real font metrics (Helvetica AFM); remove `0.52` width guesses.
- [x] Route table extraction through the DOM instead of raw-text scanning.
- [x] Skip the transient RcDom with a custom `TreeSink` to cut parse-time RAM.
- [x] Replace the hand-rolled CSS tokenizer with `cssparser`; source `<style>`
      CSS from the DOM. (Cascade model, selectors, and value parsing reused.)
- [ ] Migrate `@page` / column-width geometry off `find_css_rule` substring scan.
- [x] Add inheritance and computed-style model over the DOM.
- [x] Generate flow-content boxes from computed `display` (display:none honored,
      generic blocks carry computed style). Layout renders them with computed
      font-size/color/text-align.
- [ ] Full nested box tree with block/inline layout (boxes still flatten to a
      block list today).
- [x] Add font embedding with real metrics (`ttf-parser`/`fontdb`), opt-in via
      `--font <path|family>`. Subsetting + CID/Unicode still to come.
- [x] Add an HTTP API crate (`htmltopdf-server`, `tiny_http`): `POST /render`
      (HTML → PDF), thread-pooled; query options `landscape`/`margin`/`font`.
- [ ] Add bounded pre-layout JavaScript stage (QuickJS/Boa behind a trait).

### Features (attach after the spine)

- [ ] Add a real CSS parser and computed style model.
- [ ] Implement CSS table min-content and max-content column sizing.
- [x] Add first-pass table shrink strategy that balances page fitting, font scaling, and overflow.
- [ ] Replace first-pass table shrink with spec-backed CSS table sizing decisions.
- [x] Add first-pass row/header repeat support for semantic table header rows.
- [x] Replace heuristic header repetition with `<thead>` / CSS table-header-group support.
- [x] Add first-pass class/stylesheet-driven CSS `display: table-header-group`.
- [x] Replace first-pass class display parsing with first-pass selector cascade.
- [x] Keep selector matching near-linear by pre-indexing rules before broad CSS support.
- [x] Add first-pass `!important` handling for supported declarations.
- [x] Add first-pass text color and table-cell background color support.
- [x] Add first-pass table-cell vertical-align support.
- [ ] Replace first-pass selector cascade with `cssparser`/`selectors` and full computed style.
- [ ] Add bounded dynamic-HTML execution design before implementing JavaScript.
- [ ] Add explicit cell overflow modes: visible, hidden, clip, and ellipsis.
- [x] Add first-pass word-break and overflow-wrap support.
- [ ] Add full CSS Text Level 3 line-breaking behavior.
- [ ] Add configurable compression levels.
- [ ] Add PDF path batching behind the display-list backend.
- [ ] Add paint-order and stacking-context tests.
- [ ] Improve table layout for exact CSS border collapse after compression/path batching.
- [ ] Implement absolute positioning subset for the `reg-2-9-1.html` fixture.
- [ ] Add page margins and configurable page sizes.
- [ ] Add font embedding.
- [ ] Add local image loading.
- [ ] Add block, inline, and table layout fixtures.
- [ ] Add visual PDF snapshot tests.
- [ ] Add Chromium benchmark harness.
