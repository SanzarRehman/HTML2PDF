# htmltopdf — Plain-English Overview

A friendly map of what this project is, how it's built, and what each piece does.
For the deep version see [PLAN.md](PLAN.md) (the grand vision),
[IMPLEMENTATION.md](IMPLEMENTATION.md) (the checklist + benchmarks), and
[docs/adr/](docs/adr/) (decision records).

---

## 1. What are we building?

A program that turns **HTML + CSS into a PDF**, written in **Rust**.

The whole point is to be different from the usual options:

| Tool | Problem we avoid |
| --- | --- |
| Chromium / Puppeteer | Huge RAM, launches a whole browser per job |
| iText | Limited HTML/CSS, hard to use across many CPU cores |
| wkhtmltopdf | Abandoned, built on an obsolete browser engine |

**Our three goals:**

1. **Low RAM** — tens of MB per document, not hundreds.
2. **High concurrency** — one process renders many PDFs in parallel across all
   CPU cores, no browser subprocess.
3. **Full CSS + JS (eventually)** — grow real browser-style support honestly.

Today it renders large spreadsheet-style tables and basic flow documents
(headings, paragraphs, lists). It is **not** a full browser yet — see the status
section.

---

## 2. The pipeline (architecture diagram)

Think of it as an assembly line. HTML goes in one end, PDF bytes come out the
other. Each stage hands a clean data structure to the next.

```
            ┌─────────────────────────────────────────────────────────────┐
  HTML text │                                                             │
  ───────►  │   html5ever  ──►  Arena DOM   ──►  cssparser  ──►  Cascade   │
            │  (parse HTML)    (tree of      (parse <style>)   + Inherit   │
            │                   nodes)                         (computed   │
            │                                                   styles)    │
            └─────────────────────────────────────────────────────────────┘
                                                                    │
                                                                    ▼
            ┌─────────────────────────────────────────────────────────────┐
            │   Box generation   ──►   Layout      ──►   Display list      │
            │  (what to draw:        (where things    (backend-neutral     │
            │   tables / blocks,      go on each       paint commands:     │
            │   skip display:none)    page; wrap       text, rects,        │
            │                         text; paginate)  fills, clips)       │
            └─────────────────────────────────────────────────────────────┘
                                                                    │
                                                                    ▼
                                          PDF writer  ──►  compressed PDF bytes
                                         (flate2 zip)        ───────────►  📄
```

Plain words for each stage:

1. **Parse HTML** → build a tree of nodes (the DOM), exactly like a browser.
2. **Parse CSS** → read every `<style>` block into real CSS rules.
3. **Cascade + inherit** → figure out each element's final style (which rule
   wins, what's inherited from parents like color/font-size).
4. **Box generation** → decide what boxes exist (table rows/cells, or
   paragraphs/headings), dropping anything `display:none`.
5. **Layout** → place boxes on pages, wrap text to the right width, break across
   pages, repeat table headers.
6. **Display list** → record neutral "draw this here" commands (not PDF-specific).
7. **PDF writer** → turn those commands into compressed PDF bytes.

The **display list** in the middle is deliberate: layout never writes raw PDF.
That means we could add a PNG or debug-JSON backend later without touching
layout. (See [ADR 0001](docs/adr/0001-display-list-rendering-architecture.md).)

---

## 3. What library does what?

We keep dependencies tiny on purpose. Here is **everything** we pull in:

| Library | Used for | Where |
| --- | --- | --- |
| **html5ever** | Spec-correct HTML parsing (the same engine family as Servo). Handles malformed/nested tags, entities (`&amp;`), implied `<tbody>`, etc. | `dom.rs` |
| **cssparser** | Spec-correct CSS tokenizing (also from Servo/Stylo). Handles comments, strings, `url()`, `@media`, `!important`. | `html.rs` |
| **flate2** | Zip-compresses the PDF page content streams (`/FlateDecode`). | `pdf.rs` |
| **markup5ever_rcdom** | *Test only.* A reference DOM we compare our own DOM against, to prove ours is correct. Not in the shipped binary. | `dom.rs` tests |

That's it. No web framework, no async runtime, no browser. Everything else
(layout, pagination, fonts, PDF structure, color) is **our own code**.

---

## 4. "Are we using regex to match CSS?"  → No.

**There is no regex anywhere in the project.** (Verified: zero `regex` usage.)

CSS matching works the proper way browsers do it:

- `cssparser` turns CSS text into a stream of **tokens** (`ident`, `.`, `:`,
  whitespace, `{`, etc.).
- We read those tokens into **selectors** (a tag name + class names) and
  **declarations** (property + value).
- To style an element we **index rules by tag and class** and check matches by
  comparing strings — e.g. selector `td.amount` matches an element whose tag is
  `td` and whose class list contains `amount`. This respects specificity,
  source order, and `!important`, like a real cascade.

So pattern matching is **structured token parsing + indexed lookup**, not regex
and not "search the raw text for a string."

> **One honest exception (being removed):** page geometry — the `@page` margins,
> the spreadsheet row height, and column widths — is still read with a small
> substring scan (`find_css_rule` in `html.rs`). It only looks for a few fixed
> selectors and is on the list to fold into the real cssparser path. The actual
> **style cascade** for content does **not** use it.

---

## 5. Where things live (file map)

```
crates/
  htmltopdf/            ← the engine library
    src/
      lib.rs       Public API: Engine::render_html(html, options) -> PDF bytes
      dom.rs       HTML parsing into our compact "arena" DOM (custom html5ever sink)
      html.rs      CSS parsing, the cascade, computed styles + inheritance,
                   and turning the DOM into boxes (tables + flow content)
      font.rs      Text measurement (real Helvetica character widths)
      layout.rs    Placing boxes on pages, text wrapping, pagination, tables
      paint.rs     The display list (neutral draw commands)
      pdf.rs       Writing the actual PDF file + compression
      color.rs     Color type/helpers
  htmltopdf-cli/        ← the command-line tool
    src/main.rs    `htmltopdf input.html output.pdf` (+ benchmark commands)
```

Two design choices worth knowing:

- **Arena DOM:** the document tree is one flat `Vec` of nodes referenced by
  index (not pointers/`Rc`). It's compact, cache-friendly, and cheap to throw
  across threads — which is what keeps RAM low and concurrency high.
- **Custom html5ever sink:** we let html5ever do the hard parsing but build
  *our* arena directly, so we never hold a second heavyweight tree in memory.

---

## 6. How one conversion runs

```
htmltopdf reg-2-9-1.html out.pdf
        │
        ├─ read the HTML file
        ├─ Engine::render_html(html, default options)
        │     ├─ html5ever  → arena DOM
        │     ├─ cssparser  → stylesheet
        │     ├─ compute every node's style (cascade + inheritance)
        │     ├─ build boxes (table rows/cells, or flow blocks; skip display:none)
        │     ├─ layout onto pages
        │     ├─ display list
        │     └─ PDF writer → bytes
        └─ write out.pdf
```

Each conversion is fully independent, so running 16 of them at once just uses 16
cores. (Measured: ~20 ms per PDF at 16 workers, ~50 MB RAM, for the 1.8 MB /
22k-cell test spreadsheet — far below a browser for the same document.)

---

## 7. What works today vs. what's next

**Works now**

- Real HTML parsing (handles messy real-world markup).
- Real CSS parsing + cascade: selectors (tag/class), specificity, source order,
  `!important`, comments/strings, `@media`, multiple `<style>` blocks.
- Inheritance (color, font-size, text-align, etc. flow from parents).
- `display: none`.
- Tables: rows, cells, colspans, `<thead>/<tbody>/<tfoot>`, repeated headers,
  per-cell styles, borders, backgrounds, alignment, text wrapping/clipping.
- Flow content: headings/paragraphs/lists with computed font-size, color, align.
- Pagination, landscape, page margins, PDF compression, selectable text.

**Not yet (the honest list)**

- A fully nested block/inline **box-tree layout** (flow boxes currently flatten
  to a simple list; tables use specialized layout).
- **Font embedding** / non-Helvetica fonts (we measure with real Helvetica
  metrics but only embed the standard font).
- **Images, SVG, flexbox, grid, absolute positioning.**
- **JavaScript** (planned as a controlled pre-layout stage, later).
- Folding page-geometry parsing into cssparser (the substring scan above).

The guiding rule (from PLAN.md): build real, spec-based behavior step by step,
and don't claim support for something until it's actually implemented and tested.

---

## 8. The build order we're following

Foundation first, so features attach to something solid. Done ✓ / next ▶:

```
✓ Real font metrics
✓ Real DOM (html5ever, arena)
✓ Tables read from the DOM
✓ Custom DOM builder (low RAM)
✓ Real CSS parsing (cssparser)
✓ Computed styles + inheritance
✓ Box generation from `display` (flow content + display:none)
▶ Full nested box-tree layout
· Font embedding
· JavaScript (pre-layout)
```

Every step keeps the test suite green and the test spreadsheet rendering
byte-for-byte identical, so we always know we didn't break anything.
