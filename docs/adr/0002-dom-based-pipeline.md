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

- **HTML parsing: `html5ever` + `markup5ever_rcdom`.** We parse with the proven
  tokenizer/tree-builder, then lower the resulting tree into our own arena DOM
  (`Vec<Node>` with index-based children). Downstream code never sees `Rc`/
  `RefCell`. Rationale: html5ever gives spec-correct parsing of malformed,
  nested, and entity-laden real-world HTML; the arena lowering gives us a
  compact, cache-friendly, `Send`-friendly structure that fits the low-RAM goal.
  We do not depend on RcDom as the engine's working data structure; it is only a
  transient parse target.

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
3. Route table row/cell extraction through the DOM.
4. Replace substring CSS lookup with a `cssparser` stylesheet + real cascade.
5. Computed-style model with inheritance.
6. Box tree from computed `display`; layout consumes the box tree.
7. Font embedding/subsetting with `ttf-parser`/`fontdb`.
8. Bounded pre-layout JavaScript stage.
