# ADR 0004: Nested Flow Box Tree

## Status

Accepted (2026-06-29). Builds on ADR 0002 (DOM-based pipeline) step 7 and ADR
0001 (display-list rendering), both of which remain in force.

## Context

Flow content (non-table documents: headings, paragraphs, lists, generic blocks)
was generated as a flat `Vec<Block>` — one block per heading/paragraph leaf,
each carrying a single computed style and a single string of text. That flat
model could not represent:

- **Nesting.** A `<div>` containing a `<p>` and trailing text, a `<blockquote>`,
  or a nested list collapsed into a flat sequence with no containing-block width
  or indentation.
- **Mixed inline styling.** A paragraph with `<strong>bold</strong>` or a
  `<span style="color:red">` flattened to one style for the whole block, so the
  inline color/size/weight changes were lost.
- **List structure.** List items had no markers and no indentation.

The table path already had its own specialized layout and is out of scope here;
tables and flow content are mutually exclusive.

## Decision

Lower flow content into a **nested box tree** (`box_tree.rs`) and lay it out
recursively, mirroring the CSS block/inline model at a first-pass level:

- `FlowRoot { children: Vec<BoxChild> }` is the document root (it contributes no
  spacing of its own).
- `BoxChild` is either a `Block(BlockBox)` or a `Line(Vec<InlineRun>)` (a line
  box of inline content). A block with both inline text and child blocks keeps
  them interleaved as separate `BoxChild`s — i.e. anonymous block boxes.
- `BlockBox { kind, indent, align, children }` establishes a containing block:
  `kind` drives default font size and vertical spacing (the heading/paragraph
  metrics), `indent` is its left offset (own `padding-left` plus a fixed step per
  list/blockquote nesting level, which accumulates as they nest), and `align`
  applies to its inline content.
- `InlineRun { text, font_size, bold, color }` is a contiguous run of inline text
  sharing one computed style.

Box generation (`html::build_flow`) walks the DOM threading an inline style
context: block-level tags open a child `BlockBox`; inline tags fold their
computed style into the context for the runs they contain (`<b>`/`<strong>`
force bold even without a UA stylesheet); `display:none` subtrees are skipped;
`<li>` gets a bullet or `1.`-style marker. Layout (`layout::layout_flow`) walks
the tree recursively, wrapping each line box's runs to the containing width with
whitespace collapsed across run boundaries, and painting each run with its own
font size and color at its advancing x.

Separately, the PDF text writer now **WinAnsi-encodes** non-ASCII characters via
octal escapes (`font::char_to_winansi`) instead of replacing them with `?`, so
the bullet, en/em dashes, curly quotes, the euro sign, and Latin-1 accents
render (the font is already declared `/WinAnsiEncoding`).

## Consequences

- Flow documents finally honor nesting, indentation, list markers, and per-run
  inline color/size — validated by tests and a rendered sample.
- The table path is untouched: `Document.flow` is `Some` only when there are no
  table rows, so the spreadsheet fixture's output is **byte-identical**
  (492,740 bytes). The fixture is pure ASCII, so the WinAnsi change does not
  affect it either.
- The flat `FlowBuilder`/`visit_flow`/`blocks_from_dom` path is removed; `Block`
  now carries only table rows.

## Follow-up (done): CSS box model on blocks

`margin`/`padding` (1-to-4-value shorthands and longhands) are parsed into
computed style and resolved onto each `BlockBox`'s `Edges`. Layout collapses
vertical margins via a threaded "carried" margin (adjacent sibling and
parent/first-last-child margins collapse to their maximum; content, padding, and
border/background edges flush it). Block `background-color` (non-white) and
`border` paint one rectangle per page the block spans, inserted before the
content already emitted on that page so they sit behind text and nested boxes
stack correctly. The ASCII table fixture stays byte-identical.

## Not yet (honest limitations)

- Borders are a uniform 1pt box: no per-side width/style/color, no rounded
  corners; `margin: auto` centering and `box-sizing` are unhandled.
- Over-long words are broken character-by-character as a last resort so text
  stays on the page; honoring `overflow-wrap`/`word-break` for explicit or
  earlier breaks (and `white-space: nowrap`/`pre`) in the flow path is a
  follow-up.
- `bold` is tracked per run but has no glyph effect until a bold face is
  embedded (only one font face is embedded today).
- No inline images, no true baseline model (line leading follows the tallest run
  on the line).
