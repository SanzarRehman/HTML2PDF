# ADR 0002: DOM-Based Pipeline and Foundation Dependencies

## Status

Accepted (2026-06-29). Supersedes the parsing/styling approach used in the
minimal vertical slice. Builds on ADR 0001 (display-list rendering), which
remains in force.

## Context

The minimal vertical slice rendered a real 1.8 MB PhpSpreadsheet fixture with
~12 MB peak memory and ~27 ms/PDF at 32 workers. It proved the product thesis:
high concurrency and low RAM without a browser subprocess.

It did so with a deliberately narrow implementation:

- A hand-rolled character scanner that produced a flat `Vec<Block>` instead of a
  DOM tree.
- CSS "matching" via `input.find(selector)` substring search over raw HTML
  text, which is O(rules x document) and matches text inside comments,
  attribute values, and `@media` blocks.
- Text width estimated as a single `font_size * 0.52` constant.

These were correct choices to validate the thesis cheaply, and they are wrong
choices to grow on. The product goal is full CSS + JavaScript. None of the three
can carry that:

- A flat block list cannot represent nesting, inheritance, or stacking.
- Substring CSS is not a cascade and gives wrong answers on real stylesheets.
- A constant glyph width makes line breaking, column fit, and pagination guesses
  that cannot be validated against a browser.

The hard constraints that any replacement must preserve are the product's
differentiators: low RAM per render, render independence (every render is
`Send`, no global mutable state, linear core scaling), and the display-list
boundary from ADR 0001.

## Decision

Build the real engine spine, foundation-first, behind stable boundaries so the
concurrency and memory properties are never lost.

### Pipeline

```text
Input HTML
  -> html5ever            spec-compliant tokenizer + tree builder
  -> arena DOM            Vec<Node>, children by index (cache-friendly, low RAM)
  -> CSS (cssparser)      real stylesheet model
  -> cascade + computed   selector matching, specificity, inheritance
  -> box tree             from computed display
  -> layout               block/inline/table with real font metrics
  -> fragmentation        pagination
  -> display list         backend-neutral paint commands (ADR 0001)
  -> PDF writer           streaming, FlateDecode-compressed
```

### Dependency choices

- **HTML parsing: `html5ever`, with a custom arena `TreeSink`.** We first
  validated the pipeline by parsing into `markup5ever_rcdom` and lowering into
  our arena, then replaced that with a `TreeSink` implemented directly against
  the `Vec` arena (`crates/htmltopdf/src/dom.rs`). This removes the `Rc`/
  `RefCell` reference tree entirely and avoids holding two copies of the
  document at once, cutting peak parse-time RAM (~180 MB saved at 16 concurrent
  renders of the 22k-cell fixture). The `Handle` is a plain index; `elem_name`
  returns an owned `ElemName` that borrows from itself, which is what frees the
  handle from having to be a reference into the interior-mutable arena.
  `markup5ever_rcdom` is retained only as a dev-dependency, where a parity test
  asserts the custom sink builds the same tree as the reference implementation.
  Rationale: html5ever gives spec-correct parsing of malformed, nested, and
  entity-laden real-world HTML; the arena gives a compact, cache-friendly,
  `Send` structure that fits the low-RAM goal.

- **CSS parsing: `cssparser`** now, with **`selectors`** added later for full
  combinator/pseudo-class matching. Rationale: `cssparser` is the Servo/Stylo
  tokenizer and is the correct base for a real cascade. The `selectors` crate
  requires implementing its `Element` trait against our DOM, which we add once
  the arena DOM and computed-style model are in place.

- **Font metrics: standard-14 Helvetica AFM width table** now. The PDF backend
  emits base-14 Helvetica, whose per-glyph advance widths are a fixed, published
  table. Using those widths makes measurement exact for the font we actually
  render, with zero dependencies, fully deterministic, and zero per-render
  allocation. `ttf-parser` + `fontdb` are introduced together with font
  embedding/subsetting, so measured metrics always correspond to an embedded
  font. We explicitly do not add `ttf-parser`/`fontdb` before embedding, to
  avoid measuring against fonts we do not embed.

- **JavaScript: deferred to its own milestone**, behind a trait, prototyped with
  QuickJS and Boa. The default server path runs a bounded pre-layout execution
  stage, then freezes the DOM for style/layout (per PLAN.md section 8).

## Consequences

Benefits:

- The DOM becomes the single source of truth, so CSS cascade, inheritance,
  stacking, and JS DOM mutation all have a real place to attach.
- Measurement is correct for the rendered font, unblocking honest visual
  comparison against Chromium.
- The arena DOM keeps memory low and renders independent, preserving the
  concurrency thesis.

Costs:

- The table extraction path and the substring CSS lookup are now legacy and must
  be migrated onto the DOM/cascade incrementally. During migration both paths
  coexist behind the `parse()` entry point.
- More internal structure (separate `dom` and `font` modules now; `css`,
  `style`, and `box`/layout-tree modules to follow).

## Migration order

1. Real font metrics (`font.rs`); remove `0.52` width guesses. **Done.**
2. `html5ever` arena DOM (`dom.rs`); route generic block extraction through it.
   **Done.**
3. Route table row/cell extraction through the DOM. **Done.**
4. Custom arena `TreeSink` to drop the RcDom intermediate. **Done.**
5. Replace the hand-rolled CSS tokenizer with `cssparser`; source `<style>` CSS
   from the DOM. The cascade model, selector model, and value parsing are
   reused, so output is unchanged while comments, strings/`url()`, nested
   blocks, and `@media` are now handled correctly. **Done.** (`@page`/column
   geometry still uses a substring scan; folding it in is a follow-up.)
6. Computed-style model with inheritance: a top-down DOM pass resolves each
   node's style, inheriting color/font/text-align/white-space/wrapping from the
   parent and taking border/padding/background/overflow/vertical-align from the
   node's own cascade. Table cells read the precomputed style. **Done.**
7. Box generation from computed `display`: flow content (non-table documents) is
   generated by a display-driven walk that skips `display: none` subtrees and
   attaches each block's computed style, which layout renders (font size, color,
   text alignment). **Partially done.** Remaining: a fully nested block/inline
   box tree consumed directly by layout — today flow boxes flatten to a block
   list and the table path keeps its specialized layout.
8. Font embedding/subsetting with `ttf-parser`/`fontdb`.
9. Bounded pre-layout JavaScript stage.
