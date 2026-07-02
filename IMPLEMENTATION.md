# Implementation Checklist

This document is the working checklist for the Rust implementation. When a task
is completed, update its checkbox in this file.

> **What's covered vs not:** see the support matrix in
> [docs/COVERAGE.md](docs/COVERAGE.md) for a per-element / per-property
> supported · partial · not-yet table.

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
      carry computed style).
- [x] Build the nested flow box tree (block/inline) and lay it out recursively:
      nested blocks, list/blockquote indentation, list markers, and per-run
      inline color/font-size; WinAnsi-encode non-ASCII text in the PDF writer.

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
- Update: `@page` margins/orientation, spreadsheet column widths, and the table
  row height are now parsed from the DOM's `<style>` CSS with `cssparser` (a
  dedicated geometry pass), replacing the `find_css_rule` substring scan over raw
  HTML. Output is byte-identical on the fixture.

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

Nested flow box tree (`box_tree.rs`):

- Non-table documents now lower to a nested tree of block boxes whose leaves are
  runs of styled inline text (`html::build_flow`), and layout walks that tree
  recursively (`layout::layout_flow`) instead of rendering a flat `Vec<Block>`.
- New behavior, covered by tests: blocks nest (a `<div>` containing a `<p>` and
  trailing text keeps them as interleaved anonymous blocks); `<ul>`/`<ol>` and
  `<blockquote>` indent their contents (indent accumulates as they nest); list
  items carry a bullet / `1.`-style marker; inline `<strong>`/`<b>` mark runs
  bold; and inline `color`/`font-size` (e.g. a `<span>`) apply per run inside a
  paragraph. Whitespace is collapsed across run boundaries during wrapping, and
  alignment is honored per block.
- PDF text writer now WinAnsi-encodes non-ASCII characters via octal escapes
  (`font::char_to_winansi`) instead of replacing them with `?`, so the bullet,
  en/em dashes, curly quotes, the euro sign, and Latin-1 accents render. The
  fixture is pure ASCII, so its output stays **byte-identical** (492,740 bytes).
- The table path is untouched and remains byte-identical; tables and flow content
  are mutually exclusive (`Document.flow` is `Some` only when there are no table
  rows).
- Remaining for flow layout: inline images and a true baseline model. Bold is
  tracked per run but has no glyph effect until a bold face is embedded.
- Over-long words now break character-by-character as a last resort so flow text
  never runs off the page (a pragmatic deviation from CSS `overflow-wrap:
  normal`; honoring the property for earlier/explicit breaks is a follow-up).

CSS box model on flow blocks:

- `margin` and `padding` are parsed (1-to-4-value shorthands and longhands) into
  computed style and resolved onto each block's `Edges`. List/blockquote nesting
  folds into `margin.left`; when CSS sets no margin, the per-kind heading/
  paragraph spacing is used as the default.
- Vertical margins collapse: layout threads a "carried" margin and collapses
  adjacent margins (sibling-to-sibling and parent-to-first/last-child) to their
  maximum, flushing only when content, padding, or a border/background edge
  intervenes (verified: two 20pt-margin paragraphs sit 20pt apart, not 40pt).
- Block `background-color` (non-white) and `border` paint as one rectangle per
  page the block spans, inserted *before* the content already emitted on that
  page so they sit behind text and nested boxes stack correctly (ancestors
  behind descendants). Verified on a card/callout sample: 2 background fills and
  1 border stroke precede all 141 text operations.
- The table path is unchanged and the ASCII fixture stays **byte-identical**
  (492,740 bytes).
- Remaining: borders are a uniform 1pt box (no per-side width/style/color or
  rounded corners), `margin: auto` centering, and `box-sizing` are not handled.

CID/Unicode font embedding (Type0/Identity-H):

- Embedded fonts (`--font <path|family>`) are now written as a PDF Type0
  composite with a `CIDFontType2` descendant, `Identity-H` encoding,
  `/CIDToGIDMap /Identity`, a per-glyph `/W` width array, and a `/ToUnicode`
  CMap. Text is emitted as 2-byte glyph ids; the glyph ids, widths, and
  glyph→Unicode mapping are resolved for exactly the characters the document
  uses (`font::cid_layout`).
- Effect: **any Unicode text renders** with an embedded font (previously
  non-WinAnsi characters became `?`), and the text stays selectable/searchable
  via ToUnicode. Verified with `pdffonts` (CID TrueType / Identity-H / emb yes /
  uni yes) and `pdftotext` round-trip on Latin (incl. curly quotes/em dash) and
  CJK (`你好世界` / `这是中文测试`).
- The default (no `--font`) standard-14 Helvetica path is unchanged and still
  WinAnsi, so the ASCII fixture stays **byte-identical** (492,740 bytes).
Glyph subsetting (retain-GIDs, `subset.rs`):

- The embedded font program is now subset to the glyphs the document uses (plus
  `.notdef` and composite components, transitively): the `glyf`/`loca` tables are
  rebuilt and every other table copied verbatim, recomputing the table directory,
  per-table checksums, and the `head` checksum adjustment. Glyph ids are **not**
  renumbered (retain-GIDs), so the `/W`, `/ToUnicode`, and `/CIDToGIDMap
  /Identity` from the Type0 setup stay valid; the subset font gets an `ABCDEF+`
  name tag.
- Only `glyf`-based TrueType is subset; CFF/OpenType-CFF (no `glyf`) falls back to
  full embedding. `.ttc` inputs work (the output is a standalone single-font
  sfnt).
- Effect (measured): embedding Arial for a short doc went 477 KB → 123 KB; an
  STHeiti CJK doc went 33.3 MB → 0.65 MB. Verified the subset re-parses, kept
  glyphs retain outlines and dropped ones do not (unit test), `pdffonts` reports
  `sub yes` / `uni yes`, and `pdftotext` still round-trips Latin and CJK.
- The default Helvetica path is untouched, so the ASCII fixture is byte-identical.

Bounded pre-layout JavaScript (first pass, `js` feature):

- `BoaScriptEngine` (Boa, behind the optional `js` cargo feature) runs a
  document's inline `<script>`s after the DOM is built and before styling/layout
  (ADR 0006), mutating the DOM. Exposed via `Engine::render_html_with_scripts`
  and the CLI `--js` flag; default builds have no JS engine and are byte-identical.
- DOM API: `document.getElementById`, element `textContent` (get/set),
  `getAttribute`/`setAttribute`, and `console.log`. Runaway loops are stopped by
  Boa's loop-iteration limit (from `ScriptLimits.max_ticks`); `set_text_content`
  respects `max_new_nodes`. Each run is isolated (fresh `Context`) and takes the
  DOM back out of the shared cell afterward.
- Verified: unit tests execute real scripts (arithmetic, array iteration,
  attribute round-trips, the loop limit), and end-to-end the CLI turns a template
  (`PLACEHOLDER`/`0`) into `Invoice #1024` / `Total: $35` only with `--js`.
- Build note: Boa 0.20's tree pulls `time`/`regress` versions that require rustc
  1.88; `Cargo.lock` pins `time` 0.3.41 and `regress` 0.10.3 so the `js` feature
  builds on the workspace's rustc 1.86.
- Follow-ups: broader DOM (`innerHTML`, `createElement`, traversal), heap and
  wall-time limit enforcement, and choosing Boa vs QuickJS as the default engine.

Cascade fidelity: descendant selectors, overridable border, @media print:

- **Descendant combinators** are now matched: a selector like `.gridlines td`
  requires an ancestor with class `gridlines`, instead of collapsing to a bare
  `td` that matched every cell. `SimpleSelector` carries ancestor `Compound`s;
  the cascade collects each element's ancestor tags/classes (`AncestorSet`) and
  matches them (presence-based, exact for descendant combinators). The style
  cache key gains a small signature of only the ancestor tokens used as selector
  qualifiers, so it stays effective.
- **`border` is now a cascaded `Option<bool>`**, so a more specific `border:
  none` overrides a broader `border` rule (previously a one-way latch).
- **`@media print` is evaluated**: screen-only `@media screen { … }` rules no
  longer leak into the PDF (which is a print target); `print`/`all`/unqualified
  apply.
- Effect on the `reg-2-9-1` fixture: the borderless title/header block
  (`td.style4/5/9 { border: none }`, which a browser leaves unboxed but the old
  bare-`td` gridlines rule boxed) is now correctly unboxed — border strokes drop
  22166 → 22144, the data grid (`td.style8 { border: 1px !important }`) stays
  boxed, matching Chrome. Output is intentionally **no longer byte-identical**
  (492,740 → 492,721): this is a fidelity fix, not a refactor.

Child and sibling combinators (2026-07-01):

- Selectors now carry a `context: Vec<(Combinator, Compound)>` (nearest-first)
  instead of a flat ancestor list. `Combinator` covers `Descendant`, `Child`
  (`>`), `NextSibling` (`+`), and `SubsequentSibling` (`~`).
- `SimpleSelector::matches` walks the real tree right-to-left with a single
  leftward cursor: `Child`/`Descendant` step through element parents,
  `NextSibling`/`SubsequentSibling` through preceding element siblings. Because
  the ancestor chain is linear, one cursor is exact for arbitrary combinator
  chains (e.g. `.a b > c`), not just single combinators — `>`/`+`/`~` are no
  longer approximated as descendant.
- The parser tracks the pending combinator kind (`CompoundBuilder.pending`);
  whitespace implies descendant only when no explicit combinator is pending, so
  `A > B` (tokenized `A` WS `>` WS `B`) keeps `>`.
- Cache correctness: `structural_signature` replaces `AncestorSet`. Descendant
  only (the fixture's case) keeps the cheap unordered ancestor-token set for max
  sharing; when any `>`/`+`/`~` exists it switches to an ordered per-level
  fingerprint (plus per-level preceding-sibling tokens when sibling combinators
  are present), which is exact because a combinator walk can only reach ancestors
  and their preceding siblings. Restricted to tokens that actually appear as
  qualifiers, so keys stay tiny.
- The `reg-2-9-1` fixture uses only descendant selectors, so it hits the fast
  path and output stays byte-identical (492,721). Tests: 86 default / 91 with
  `js`, adding child/adjacent-sibling/general-sibling coverage.

Id, universal, attribute, and pseudo-class selectors (2026-07-01):

- `Compound` now carries `tag`, `id`, `classes`, `attrs` (`AttrSelector` with
  the `= ~= |= ^= $= *=` operators plus presence), `pseudos` (`PseudoClass`),
  and a `universal` (`*`) flag. `SimpleSelector` holds a `subject: Compound`.
- Structural pseudo-classes are matched against the DOM: `:first-child`,
  `:last-child`, `:only-child`, `:nth-child`/`:nth-last-child` (full `An+B`,
  incl. `odd`/`even`), the four `-of-type` variants, `:empty`, `:root`, and
  `:not(<compound-list>)`. Dynamic pseudo-classes (`:hover`, …) and pseudo-
  elements (`::before`) are unsupported and drop the whole selector, so they
  never over-apply to the static print render.
- Indexing gained an `id_rules` map and a `universal_rules` bucket for subjects
  with no tag/id/class (attribute-only, pseudo-only, or `*`). Specificity now
  counts ids/attributes/pseudo-classes correctly, with `:not()` contributing its
  most specific argument.
- Cache correctness: a `needs_precise_match` flag is set when any selector
  (subject or context) uses an id, attribute, or pseudo-class — things the cheap
  shared cache key cannot represent (they depend on per-element attributes or
  sibling position). When set, `element_own` keys the style cache per element
  (`@{node_id}`), which is exact; otherwise it keeps the shared tag/class/sig
  key. The `reg-2-9-1` fixture uses none of these, so it stays on the fast path
  and output is byte-identical (492,721). Tests: 94 default / 99 with `js`.

Block-level `<img>` images — JPEG + PNG (2026-07-01):

- New `image.rs` loads an `<img src>` from a `data:` URI (base64 or literal) or a
  file path (resolved against `RenderOptions.base_dir`, set by the CLI to the
  input file's directory), sniffs the format by magic bytes, and produces a
  `DecodedImage` for PDF embedding.
- **JPEG** is embedded verbatim through PDF's `DCTDecode` filter — no pixel
  decoder — after scanning the marker stream for the SOFn frame size and
  component count (1 → DeviceGray, 3 → DeviceRGB).
- **PNG** is decoded in-house with zero new dependencies: chunk parsing
  (IHDR/PLTE/tRNS/IDAT/IEND), `flate2` inflate of the IDAT stream (the same crate
  already used to compress PDF streams), scanline unfiltering (None/Sub/Up/
  Average/Paeth), and color-type expansion for grayscale/RGB/palette/gray+alpha/
  RGBA at 8 or 16 bit depth. Alpha is split into a separate 8-bit soft mask
  (`/SMask`); palette `tRNS` becomes a per-pixel mask. Interlaced and sub-byte
  depths are unsupported (the image is skipped).
- Pipeline wiring: `box_tree::BoxChild::Image(ImageBox)`; `html::build_flow`
  emits an unresolved `ImageBox` (src + `width`/`height` hints); a post-parse
  `html::resolve_images` pass loads/measures each and fills `Document.images`;
  `layout` scales to fit the content box, page-breaks the image as a unit, and
  emits `PaintCommand::Image`; `pdf` writes image (and soft-mask) XObjects,
  lists them in every page's `/Resources /XObject`, and paints them with
  `q w 0 0 h x y cm /ImN Do Q`. Sizing uses `width`/`height` (CSS px → pt at
  96 dpi) preserving aspect ratio, else the intrinsic size.
- Verified end-to-end with `pdfimages`: a file-path PNG round-trips to a 3x2 RGB
  image XObject whose extracted pixels match the source exactly. The
  `reg-2-9-1` fixture has no images, so its output stays byte-identical
  (492,721). Tests: 103 default / 108 with `js` (base64, JPEG header, PNG
  RGB/RGBA/Up-filter decode, and a full data-URI render).

Table fidelity: content-based column layout + real font sizes (2026-07-01):

- Verified against Chrome (`--headless --print-to-pdf`) on `reg-2-9-1.html`.
  Root cause of the poor match: the old `table_geometry` scaled the declared
  column widths (summing to ~1879pt) down to the page and scaled the *font* by
  the same factor (~0.40) — so 10–11pt cell text rendered at ~4.4pt, with a fixed
  18pt row-height floor inflating spacing.
- Replaced it with a browser-style **automatic table layout**: each column gets a
  min-content (widest word, or widest char when `overflow-wrap`/`word-break`
  allow it) and a max-content (widest single line) width, including padding, with
  colspan cells distributed across the columns they span. Declared `<col>` widths
  are honored only when they collectively fit and respect min-content; otherwise
  columns are sized to content. Distribution: content fits → natural widths at the
  **full CSS font size**; too wide but min-content fits → shrink wide columns
  toward their longest word (wrap, font unchanged); wider than the page even at
  min-content → uniform **shrink-to-fit** (columns + font), matching a browser's
  print scaling rather than clipping data.
- Row height is now content-driven (`line-height ≈ font*1.18 + padding`) with no
  fixed floor; a CSS-declared row height still acts as a minimum. Cell font-size
  default is 11pt (was a 7/8.5pt fudge) and padding default ~1px, so the cascade's
  real `font-size`/`padding` drive layout. Added a small wrap tolerance so text
  that measures exactly the column width is not broken by a float rounding error.
- Result vs Chrome on the fixture: data text ~7.8pt (Chrome ~7.8pt; was ~4.4pt);
  a table that fits renders at full 11pt with row pitch 15.98pt (Chrome 16.5pt);
  page count 46 (Chrome 32 — the residual gap is Chrome shrinking ~10% more and
  Letter-vs-A4 geometry). Matching the intended **Calibri** needs the font
  embedded (`--font Carlito`, the metric-compatible free clone, or `--font Arial`
  to match Chrome's macOS fallback); the built-in Helvetica is close but wider.

Font size, shrink-to-fit, and faux-bold (2026-07-01, follow-up):

- Confirmed via Chrome's content stream that Chrome renders this fixture with a
  page-level **shrink-to-fit** (data text ≈ 7pt on Letter landscape, not the
  literal 10pt) because the 16-column table is far wider than the page. Our
  `table_geometry` shrink-to-fit branch (font+columns scaled by
  `available/min-content`) matches this; on A4 landscape the text is ~11% larger
  than Chrome's Letter output purely because the page is wider (806 vs 734pt
  content) — the same methodology, fit to a wider page.
- **Faux-bold**: `font-weight:bold` (and `<th>`/bold cells) now render visibly
  bold. `TextCommand`/`LinePiece` carry a `bold` flag (threaded from the cascade
  and inline runs); the PDF writer draws bold glyphs with text render mode 2
  (fill+stroke) at ~3% line width in the fill color — no second font face needed.
  Header rows, the title, and label rows in the fixture now match Chrome's bold.
- To match Chrome's actual output use `--font` (Chrome falls back to **Arial** on
  macOS since Calibri is absent): `--font /System/Library/Fonts/Supplemental/Arial.ttf`
  reproduces Chrome's glyph widths; `--font Carlito` gives the intended Calibri.

Row-height scaling + `--paper`; near-1:1 with Chrome (2026-07-01, follow-up):

- A CSS-declared table row height (the fixture's 20px = 15pt) was applied as a
  fixed floor that was **not** scaled by the table's shrink-to-fit factor, so
  rows stayed 15pt tall while the font/columns shrank to ~0.73 — inflating row
  height ~1.4x and the page count. Fixed: the row-height floor is multiplied by
  `paint_scale` (browser print scaling shrinks rows too). Fixture page count
  dropped 46 → 37 on A4.
- Added `PageSize::LETTER`/`LETTER_LANDSCAPE`, a `Paper` enum, and a CLI
  `--paper a4|letter` flag (Chrome's default paper is Letter). On Letter with
  `--font Arial`, the fixture is now **near-1:1 with Chrome**: 33 pages vs 32,
  row pitch 10.9pt vs 10.5pt, data font/email width within ~4%, matching bold
  headers, columns, and per-page row counts.
- **Gridline weight**: cell/box borders were stroked at the PDF default 1.0pt
  (never setting a line width), so gridlines looked heavier/darker than a
  browser's. Added a `SetLineWidth` paint command; borders now stroke at a 1px
  (0.75pt) width scaled by `paint_scale`, matching Chromium's lighter gridlines.
- **Perf fix (O(n²) → O(n))**: `div + div` in the fixture's CSS set
  `has_sibling_combinator`, which made `structural_signature` scan every
  ancestor's preceding siblings — for each of 22k cells it walked the 1088-row
  `<tbody>`, so the cascade took ~4.3s. The signature now scans only the
  *subject's* own preceding siblings; ancestor-level sibling combinators
  (`.x + .y .z`, rare) fall back to the per-element cache key. Full-fixture
  render dropped **4.3s → 0.35s** (~48 MB RSS), vs Chrome's ~1.7s / ~840 MB —
  see the README comparison table.

Important limitation:

- This is now a fast spreadsheet-table PDF with basic images, close to Chrome for
  this class of document, but still not a fully faithful general browser render.
  It still needs richer layout (inline/floated images, flex/grid), broader CSS
  values, a real bold font face (faux-bold is an approximation), and general
  visual validation before it can replace Chromium. Selector coverage omits
  namespaces and `:link`-style pseudo-classes; images are block-level only.

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
- [x] Migrate `@page` / column-width geometry off `find_css_rule` substring scan
      (now parsed from the DOM's CSS with `cssparser`; fixture byte-identical).
- [x] Add inheritance and computed-style model over the DOM.
- [x] Generate flow-content boxes from computed `display` (display:none honored,
      generic blocks carry computed style). Layout renders them with computed
      font-size/color/text-align.
- [x] Full nested box tree with block/inline layout (`box_tree.rs`): non-table
      documents lower to a nested tree of block boxes whose leaves are styled
      inline runs, laid out recursively (indentation, lists, per-run color/size).
- [x] CSS box model on flow blocks: `margin`/`padding` (shorthands + longhands),
      vertical margin collapse, and per-page-fragment block backgrounds and
      borders painted behind content.
- [x] Add font embedding with real metrics (`ttf-parser`/`fontdb`), opt-in via
      `--font <path|family>`, as a Type0/Identity-H composite with a ToUnicode
      CMap (any Unicode renders + stays selectable) and **retain-GIDs glyph
      subsetting** (`subset.rs`) so only used glyphs are embedded.
- [x] Add an HTTP API crate (`htmltopdf-server`, `tiny_http`): `POST /render`
      (HTML → PDF), thread-pooled; query options `landscape`/`margin`/`font`.
- [x] Add bounded pre-layout JavaScript stage (Boa behind the `js` feature).
      First pass: `BoaScriptEngine` runs inline scripts against a minimal
      `document` DOM API (`getElementById`, `textContent` get/set,
      `get/setAttribute`, `console.log`) with a loop-iteration limit, mutating the
      DOM before styling/layout. Opt-in via `Engine::render_html_with_scripts` and
      the CLI `--js` flag; default builds are unchanged. Broader DOM APIs,
      `innerHTML`/`createElement`, and heap/wall-time enforcement are follow-ups.

### Next up (prioritized)

The current front of the queue (rough value order). Details for each are in the
feature list below and in [docs/COVERAGE.md](docs/COVERAGE.md).

- [x] **Per-element `font-family` + real bold/italic faces**: the cascade
      carries `font-family` (first usable name in the stack, generics kept)
      and `font-style`; the flow builder interns every distinct
      `(family, bold, italic)` requirement into `Document::font_specs`
      (`FontInterner`; spec 0 = default) and runs/cells store a `u16` spec
      index; a post-pass interns table-cell fonts wherever cells live.
      `with_document_hints` resolves specs once per render through a
      process-wide face cache (`resolve_spec` + shared `fontdb` index scanned
      once per process): named families and generics load real system faces —
      including real **bold**/**italic** variants via weight/style queries —
      and each distinct face embeds as its own subset Type0 resource
      (deduplicated by identity). Layout measures every run with its own face
      (`RenderOptions::run_font`), and faux-bold survives only where no real
      bold face resolves (`run_faux_bold`). UA defaults: `pre`/`code`/`kbd`/
      `samp` → monospace, `<i>`/`<em>`/`cite`/`var`/`dfn`/`address` → italic.
      Verified: a Georgia/Arial/Courier document embeds 8 real subset faces
      with **zero** faux-bold strokes; the 22k-cell Excel fixture (which asks
      for Arial/Calibri) now renders in real Arial + Arial-Bold by default —
      matching what the Chromium comparison previously needed `--font Arial`
      for — at the same cost as that flag (~0.62 s / 90 MB vs 0.64 s / 95 MB).
      **Not yet done:** walking the rest of the `font-family` stack per
      character (the fallback chain covers missing glyphs instead),
      `@font-face` (web fonts from CSS), synthetic italic slant when no italic
      face exists, `font-weight` granularity beyond bold/regular (300/600/900
      map to the nearest), and bold synthesis for fallback-chain faces.
- [x] **`%` lengths + sizing keywords (first pass)**: `width` and `max-width`
      accept percentages, resolved against the containing block at layout time
      (parse time can't know it) — on in-flow blocks, floats, absolute boxes,
      and images (`width: 120%` may scale an image up; `max-width: 100%`
      clamps). `margin: auto` on both sides centers a width-constrained block
      (`margin: 0 auto` shorthand detected in the expanded 1-to-4 slots).
      Verified geometry: a 60%+auto card centers to the half-point exactly;
      caps clamp border boxes to the point. `features/sizing` parity fixture
      (19 total). **Not yet done:** `%` heights/margins/padding/offsets,
      `min-width`, `max-height`, `height` on blocks, and `auto` margins
      combined with a single `auto` side (off-center distribution).
- [ ] **Link annotations + outline**: `<a href>` → PDF `/Annots` link
      rectangles (external URLs and `#anchor` → internal GoTo), and a document
      outline (bookmarks) from `h1`–`h3`. Pure pdf.rs additions; the display
      list needs a link-region command.
- [ ] **Finish flexbox** *(phase 2 shipped — see below)*: remaining gaps are
      `flex-wrap`, explicit `flex-shrink`/`order`, `align-self`, column-direction
      main-axis sizing (height grow/justify), and flex rows spanning a page break.
- [ ] **Grid, phase 2**: line-based placement (`grid-column: 1 / 3`),
      `minmax()`, `grid-template-rows`, cell alignment.
- [ ] **`text-align: justify`** — the LineBreaker already builds lines one at
      a time; justification is distributing the slack into word-space TJ
      adjustments (flow) once a line is known not to be the paragraph's last.
- [ ] **`dir="rtl"` / `direction: rtl`**: RTL base paragraphs (bidi core is
      in; this is plumbing the base level + right alignment default).
- [ ] **Remote `http(s)` images** (opt-in flag, size/time caps, server-safe
      fail-closed default) and inline/floated image follow-ups.
- [ ] **Live-DOM surface on demand**: `insertBefore`, `cloneNode`,
      `querySelector(All)`, JS-side `parentNode`/`children` traversal.
- [ ] **Stacking contexts**: negative `z-index` painting below flow content,
      per-context z comparison (currently global, positioned always above).
- [x] **CSS grid, first pass** (`display: grid`): `grid-template-columns` with
      fixed lengths / `fr` / `auto` / `repeat(N, …)`, row-major auto-placement,
      `grid-column: span N`, and separate row/column gaps. Auto tracks size to
      their widest single-span item; `fr` shares the remainder; over-wide track
      sets shrink proportionally. Rows size to their tallest item (measure pass)
      and page-break between rows. Note: adding grid required dropping `Copy`
      from `CellStyle` (track lists are `Vec`s) — verified no wall-time change
      and +5 MB peak RSS on the 22k-cell fixture.
      **Not yet done:** line-based placement (`grid-column: 1 / 3`), named
      lines/areas, `minmax()`, `grid-template-rows`, dense packing, and
      `align`/`justify` of items within cells.
- [x] **`float` / `clear`, first pass**: floated blocks (shrink-to-fit or CSS
      `width`) and floated images placed at the flow edges; the line breaker was
      reworked to build lines one at a time so each shortens to the float bands
      active at its own `y` (interval-accurate for stacked floats) and re-widens
      below; words that cannot fit beside a float drop below it; `clear:
      left/right/both` drops a block past the matching floats; a page break
      retires the page's floats. **Not yet done:** floats that overflow into a
      second band row when they don't fit side by side, `float: none`
      overriding an earlier float in the cascade, and relative/absolute
      positioning (still open, next line).
- [x] **`position`, first pass**: `relative` (visual offset via a shifted
      layout cursor; flow position fully preserved) and `absolute`
      (out-of-flow against the page content box: `top`/`right`/`bottom`/`left`
      offsets with in-flow-cursor fallback, shrink-to-fit or CSS width, no
      cursor advance, excluded from margin collapsing). In-flow blocks now also
      honor a content-box CSS `width` (left-aligned). **Not yet done:**
      positioned-ancestor containing blocks, `fixed` repeated on every page
      (currently absolute on its page), `z-index`/stacking (paint order is
      encounter order), `%` offsets, `margin: auto`, and `sticky`.
- [x] **Text shaping via `rustybuzz` (HarfBuzz)** for embedded fonts: layout
      measures *shaped* widths (kerning + ligatures) and the PDF writer emits the
      shaped glyph ids as `TJ` arrays whose numeric adjustments reproduce kerning
      (`/W` carries natural advances). Ligature glyphs map back to all their
      source characters in `/ToUnicode` (text stays extractable); Arabic gets
      joining forms and correct in-run RTL order. The `rustybuzz::Face` is cached
      per font (self-referential over the font bytes, dropped first), and shaped
      runs are cached by string behind a `Mutex` (fonts are shared across render
      threads via `Arc`). Cost on the 22k-cell fixture with a font: +0.03 s /
      +3 MB vs unshaped; the default base-14 path is unchanged (no face to shape).
      **Not yet done:** bidi paragraph reordering (UAX #9) for mixed LTR/RTL,
      font-fallback chains (CJK/emoji), `fitting_char_count`/char-level breaking
      still uses unshaped advances, and shaping-aware `letter-spacing`.
- [x] **Extend live-DOM JS** (ADR 0009): `document.createElement`/`createTextNode`
      push detached arena nodes (drawn from the `max_new_nodes` budget; `null`
      past the cap); `appendChild` attaches — and, on an attached node, *moves*
      (reparents) — with a cycle guard that refuses illegal moves by returning
      `null` instead of throwing; `removeChild` detaches (orphans stay in the
      arena, dropped wholesale with the render); `document.body` exposed as the
      attachment point. Script-created nodes go through the normal cascade, so
      stylesheet classes apply. A document whose only content is script-built
      renders end to end. Mid-script layout reads (`getBoundingClientRect`)
      were considered and **rejected** — they would make layout re-entrant and
      break the one-pass cost model; ADR 0009 records the reasoning and the
      cheaper `measureText` escape hatch if demand appears. **Not yet done:**
      `insertBefore`, `cloneNode`, `querySelector(All)`, JS-side tree traversal
      (`parentNode`/`children`), events, timers.
- [x] **`position: fixed` per page, positioned-ancestor containing blocks,
      `z-index`**: positioned (absolute/fixed) boxes are now laid out into a
      scratch page and captured as *overlays*, appended after each page's
      in-flow content — so they paint **above** the flow (as CSS specifies)
      sorted by `z-index` (stable; `auto` = 0), and a `fixed` box's overlay is
      stamped onto **every** page (print headers/footers/watermarks). Absolute
      descendants resolve `left`/`right`/`top` against the nearest positioned
      ancestor's content box (`bottom` still resolves against the page — the
      ancestor's height isn't known mid-layout); `fixed` always positions
      against the page. Absolute boxes no longer spill onto a real next page:
      content past the page bottom is dropped (they don't paginate). Guarded by
      a multi-page `features/fixed-per-page` parity fixture (17 total).
      **Not yet done:** negative `z-index` painting *below* in-flow content
      (overlays always sit above the flow), `bottom` against a positioned
      ancestor, `%` offsets, `sticky`, and stacking-context isolation
      (z-indexes compare globally, not per stacking context).
- [x] **Bidi paragraph reordering (UAX #9)** via `unicode-bidi` (Servo's
      implementation), two layers deep: (1) `TrueTypeFont::shape` itemizes a
      mixed-direction string into visual runs and shapes each with an explicit
      direction — joining forms are computed on logical text, glyphs emitted in
      visual order, so a single string like `Total: ١٢٣` is right inside table
      cells too; (2) `layout_line_box` reorders each wrapped line's word pieces
      into visual order (`reorder_pieces_bidi`), reversing the pieces inside
      each RTL run. Both layers resolve levels against an **LTR base** (HTML's
      default) and skip cleanly via a cheap RTL-range char scan, so LTR-only
      documents take the exact old path (verified: no perf change on the
      22k-cell fixture). Verified visually with Arial: Arabic phrase embedded
      in an English sentence, Hebrew inline, and a full Arabic paragraph all
      place words right-to-left, and `pdftotext` recovers the logical text.
      **Not yet done:** `dir="rtl"` / `direction: rtl` (RTL base paragraphs —
      currently rendered against an LTR base and left-aligned, matching
      Chrome's dir-less default), bracket mirroring, and pieces that straddle
      a direction boundary move whole (assigned by first byte).
- [x] **Font fallback chains**: characters the primary font lacks (CJK,
      Hangul, Cyrillic under a Latin `--font`, …) are rendered by the first
      covering face from a system chain (Arial Unicode MS → Noto Sans → DejaVu
      Sans → Arial), loaded lazily and cached per `Font`. `Font` is now
      chain-aware end to end: `segment_by_coverage` splits a string into
      per-font runs (whitespace inherits its run; ASCII binds to the primary;
      unfixable chars stay primary `.notdef`; emoji never trigger fallback),
      and `text_width` measures each segment with its owning face — so layout
      code needed **zero** changes. The PDF writer generalized from one font
      object to N `FontPlan`s (`/F1…/Fn`): each used face gets its own
      resources, per-face glyph subsetting, `/W` widths, and ToUnicode CMap;
      text commands re-segment at emission and switch fonts with `Tf` between
      show operators (the text matrix carries the position). Coverage asks the
      face's cmap directly (the WinAnsi `advances` cache was the original bug
      — CJK read as "uncovered" even in Arial Unicode). Verified: a
      default-font (Helvetica) render of Chinese/Japanese/Korean/Russian
      embeds one subset Arial Unicode face (172 KB PDF), and an Arial-primary
      render keeps Cyrillic in Arial while CJK goes to the fallback; both
      extract correctly with `pdftotext`. ASCII fast path keeps the 22k-cell
      fixture at identical wall/RSS. **Not yet done:** configurable chain
      (CSS `font-family` / an options list), fallback-aware
      `fitting_char_count` (char-level breaking measures with the primary),
      per-fallback bold synthesis, and emoji (deliberately excluded).
- [x] **`line-height`**: unitless numbers, percentages (both = font-size
      multiples), and absolute lengths, applied to flow line boxes (per block,
      inherited through the cascade) and table-cell leading (absolute lengths
      scale with the table's shrink-to-fit paint scale, like the font). Leading
      beyond the default line box is distributed as half-leading, so glyphs sit
      mid-line like a browser; when unset, output is byte-identical to before
      (`font×1.35` flow / `×1.18` cells). Guarded by a `features/line-height`
      parity fixture (16 total). **Not yet done:** `line-height: normal`
      *overriding* an inherited value (it currently falls through to the
      ancestor's), per-inline-run line-height (the block's value governs the
      whole line), and true font-metric line boxes (ascent+descent instead of
      the 0.8 em ascent approximation).

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
- [x] Add descendant-combinator matching (ancestor-scoped selectors like
      `.gridlines td`), overridable `border` (so `border: none` beats a broader
      rule), and `@media print` evaluation (screen-only rules excluded from PDF).
- [x] Add child (`>`) and sibling (`+`, `~`) combinator matching (exact
      right-to-left tree walk; cache signature stays exact and small).
- [x] Add id, universal, and attribute selectors and structural pseudo-classes
      (`:nth-child`, `:*-of-type`, `:empty`, `:root`, `:not`); per-element cache
      key when these are used, shared key otherwise.
- [x] Add block-level `<img>` support: JPEG (`DCTDecode`) and in-house PNG
      decode (via `flate2`, alpha as `/SMask`), from file paths and `data:` URIs,
      embedded as PDF image XObjects.
- [x] Honor cascaded CSS `width`/`height` on `<img>` (from rules and inline
      `style`), taking precedence over the presentational HTML attributes and
      preserving the intrinsic aspect ratio when only one axis is set.
- [ ] Broaden images further. **Not yet done** (explicit gaps):
  - [ ] Inline `<img>` in flow text (it currently breaks onto its own line;
        no baseline alignment with surrounding text).
  - [ ] Floated / wrapped images (`float`, text wrapping around an image).
  - [ ] `object-fit` (`contain`/`cover`/`fill`/`none`) and `object-position` —
        the box is always stretched to the resolved width/height.
  - [ ] `max-width`/`min-width`/`max-height`/`min-height` clamping (e.g. the
        common `img { max-width: 100% }`); only plain `width`/`height` are read.
  - [ ] Percentage image dimensions (resolved against the containing block).
  - [ ] Remote (`http`/`https`) image URLs — only local paths and `data:` URIs.
  - [ ] Sub-byte-depth (1/2/4-bit) and interlaced (Adam7) PNG; 16-bit PNG.
  - [ ] GIF, WebP, SVG, and BMP decoding.
  - [ ] `srcset`/responsive selection and `<picture>`.
  - [ ] Image borders/padding/background and CSS `border-radius` clipping.
- [x] Add CSS `text-decoration` (`underline`, `line-through`, and `none`) plus
      the `<u>`/`<ins>`/`<s>`/`<strike>`/`<del>` tags, propagated to inline runs
      and stroked in flow text and table cells.
      **Not yet done:** `overline`; decoration `color`/`style`/`thickness`;
      and `text-decoration: none` cannot *cancel* an ancestor's decoration
      (the flag propagates like `bold`, so it only ever turns decoration on).
- [x] First-pass **flexbox** (`display: flex`): row direction with `flex`/
      `flex-grow`/`flex-basis`, `justify-content`, and `gap`, over block-level
      flex items. Column sizing via basis → grow → uniform shrink.
- [x] **Flexbox phase 2**: `align-items` center/end (per-item height via a
      scratch-page measure pass — same code as the paint pass, so it's exact);
      inline element children (e.g. `<span>`) promoted to real flex items with
      zero default margins, and bare text as anonymous items; `flex-direction:
      column` as a vertical stack with `gap`; item basis includes the item's own
      padding/margins. Also fixed the flow first-baseline placement: baselines
      now sit ~0.8 em below the line top, so ascenders no longer overlap the
      border/padding of the box above (visible in bordered cards/pills).
      **Not yet done:** `flex-wrap`, explicit `flex-shrink`/`order`,
      `align-self`, column main-axis sizing, and flex rows spanning a page break.
- [ ] CSS grid layout.
- [ ] Move toward browser-complete computed values (more properties, shorthands,
      units); consider `:link` and namespace selectors. Text not yet supported
      includes `line-height`, `font-style: italic`, `text-transform`,
      `letter-spacing`, and `text-indent`.
- [x] Add bounded dynamic-HTML execution design before implementing JavaScript
      (ADR 0006 + the `script.rs` seam: `ScriptEngine` trait, `ScriptLimits`,
      `ScriptReport`, default `NoopScriptEngine`).
- [x] Wire in the Boa engine behind the `js` feature: inline scripts run before
      layout against a minimal `document` DOM (`getElementById`, `textContent`,
      `get/setAttribute`, `console.log`) within `ScriptLimits`. Opt-in via
      `Engine::render_html_with_scripts` / CLI `--js`.
- [x] **Live-DOM `innerHTML`** (ADR 0008): structural pre-layout mutation via
      cross-arena grafting; reflow + re-pagination come for free from the
      pre-layout model, and peak RAM stays bounded. **Not yet done:**
      `createElement`/`appendChild`/`removeChild`, and mid-script layout reads
      (`getBoundingClientRect`), which remain the hard, deferred part.
- [ ] Add explicit cell overflow modes: visible, hidden, clip, and ellipsis.
- [x] Add first-pass word-break and overflow-wrap support.
- [x] **Render flow content and tables in the same document.** *(Was: a document
      routed to **either** the flow path **or** the table/`blocks` path, so
      headings/paragraphs around a `<table>` were silently dropped.)* A `<table>`
      is now a `BoxChild::Table` in the flow tree, laid out in document order with
      surrounding headings/paragraphs (with a collapsing vertical margin so text
      clears its edges). A **bare** table still uses the dedicated spreadsheet
      `blocks` path (chosen via `FlowRoot::has_nontable_content`), preserving its
      tuned performance. Guarded by the `features/tables` and `combined/invoice`
      parity fixtures. **Not yet done:** tables do not honor a left indent from an
      enclosing block (painted at the page's left margin); nested tables and
      `<caption>` are unsupported.
- [ ] Add full CSS Text Level 3 line-breaking behavior.
- [ ] Add configurable compression levels.
- [ ] Add PDF path batching behind the display-list backend.
- [ ] Add paint-order and stacking-context tests.
- [ ] Improve table layout for exact CSS border collapse after compression/path batching.
- [ ] Implement absolute positioning subset for the `reg-2-9-1.html` fixture.
- [ ] Add page margins and configurable page sizes.
- [ ] Add font embedding.
- [ ] Add local image loading.
- [x] Add block, inline, and table layout fixtures (the parity fixture set —
      see the Parity Harness section below).
- [ ] Add visual PDF snapshot tests (raster-diff step exists in
      `scripts/compare-parity.sh`; wire it into CI once ImageMagick is available).
- [x] Add Chromium (Chromium) parity + benchmark harness (fixtures, semantic
      expectation JSON, Rust test, and the three parity scripts).

## Parity Harness (Chromium)

Inspired by ironpress's parity dashboard. Layout: `crates/htmltopdf/tests/`.

- `fixtures/{features,combined,edge-cases}/*.html` — HTML fixtures, each with an
  `@page { margin: 28.8pt }` rule so they land on the same geometry as Chromium's
  `--print-to-pdf` defaults (Letter, 0.4in).
- `fixtures/expectations/<layer>_<name>.json` — per-fixture semantic assertions
  (`must_contain_operators`, `must_contain_text`, size/page bounds) plus
  human-readable `visual_assertions` (checked by the raster diff, not the Rust test).
- `tests/parity_tests.rs` — renders every fixture, inflates the FlateDecode
  content streams, and checks the expectations. Dev-deps `serde_json` + `flate2`.
  - `cargo test --test parity_tests`
  - `cargo test --test parity_tests -- --ignored --nocapture report` → size /
    page / render-time table (~740 pages/sec locally on this fixture set).
- `scripts/render-fixtures.sh` — htmltopdf → PDFs (`--paper letter`).
- `scripts/generate-references.sh` — Chromium → 150-DPI reference PNGs
  (needs Chrome + poppler `pdftoppm`). Output is git-ignored (regenerable).
- `scripts/compare-parity.sh` — raster-diff ours vs reference with a diff-%
  threshold (needs ImageMagick `compare`/`convert`/`identify`).

Adding a fixture: drop the `.html` in a layer dir, add an expectation JSON, and
append the `(layer, name)` pair to `FIXTURES` in `parity_tests.rs`.
