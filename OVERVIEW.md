# htmltopdf — Plain-English Overview

A friendly map of what this project is, how it's built, and what each piece does.
For the deep version see [PLAN.md](PLAN.md) (the grand vision),
[IMPLEMENTATION.md](IMPLEMENTATION.md) (the checklist + benchmarks),
[docs/COVERAGE.md](docs/COVERAGE.md) (what HTML/CSS is supported vs not), and
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
| **ttf-parser** | Reads TrueType/OpenType metrics (glyph advances, ascent/descent, bbox) for layout + the PDF font descriptor, when a font is embedded. | `font.rs` |
| **fontdb** | Resolves a font *family name* (e.g. `Georgia`) to a font file from the system database. | `font.rs` |
| **flate2** | Zip-compresses the PDF page content streams and embedded font (`/FlateDecode`); also inflates PNG `IDAT` for image decoding — one crate, reused. | `pdf.rs`, `image.rs` |
| **markup5ever_rcdom** | *Test only.* A reference DOM we compare our own DOM against, to prove ours is correct. Not in the shipped binary. | `dom.rs` tests |
| **boa_engine** | *Optional (`js` feature).* A pure-Rust JavaScript engine for the bounded pre-layout script stage. Absent from default builds. | `script.rs` |

That's it. No web framework, no async runtime, no browser. Everything else
(layout, pagination, fonts, PDF structure, color) is **our own code**. The
JavaScript engine is optional and off by default.

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
  `td` and whose class list contains `amount`. Selectors support **ids**
  (`#total`), the **universal** selector (`*`), **attribute** selectors
  (`[data-x]`, `[type=text]`, `~= |= ^= $= *=`), and **structural pseudo-classes**
  (`:first-child`, `:nth-child(2n+1)`, `:first-of-type`, `:empty`, `:root`,
  `:not(...)`). **Combinators** are matched precisely by walking the real tree
  right-to-left: `.gridlines td` (descendant), `tr > td` (child), `td + td`
  (adjacent sibling), and `.mark ~ td` (general sibling). Dynamic pseudo-classes
  (`:hover`) are dropped since they never fire in static print output;
  `::before`/`::after` rules generate styled text content. This respects specificity, source order, `!important`,
  and `@media print` (screen-only rules are excluded from the PDF), like a real
  cascade.

So pattern matching is **structured token parsing + indexed lookup**, not regex
and not "search the raw text for a string."

> Page geometry — the `@page` margins/orientation, the spreadsheet row height,
> and column widths — is also parsed with `cssparser` (a dedicated geometry pass
> over the DOM's `<style>` CSS), so there is **no substring scanning of CSS or
> HTML anywhere** in the engine.

---

## 5. Where things live (file map)

```
crates/
  htmltopdf/            ← the engine library
    src/
      lib.rs       Public API: Engine::render_html(html, options) -> PDF bytes
      dom.rs       HTML parsing into our compact "arena" DOM (custom html5ever sink)
      html.rs      CSS parsing, the cascade, computed styles + inheritance,
                   and turning the DOM into boxes (tables + the flow box tree)
      box_tree.rs  The nested block/inline box tree for non-table documents
      font.rs      Text measurement (real Helvetica widths) + WinAnsi encoding
      subset.rs    Retain-GIDs TrueType glyph subsetter for embedded fonts
      image.rs     <img> loading: data URIs, JPEG headers, in-house PNG decode
      script.rs    Pre-layout scripting seam (ScriptEngine trait; no-op default)
      layout.rs    Placing boxes on pages, text wrapping, pagination, tables,
                   and recursive flow box-tree layout
      paint.rs     The display list (neutral draw commands)
      pdf.rs       Writing the actual PDF file + compression
      color.rs     Color type/helpers
  htmltopdf-cli/        ← the command-line tool
    src/main.rs    `htmltopdf input.html output.pdf` (+ benchmark commands)
  htmltopdf-server/     ← the HTTP API (for curl / Postman)
    src/main.rs    POST /render (HTML in, PDF out), thread-pooled
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

## 7. Run it as an HTTP API (curl / Postman)

Start the server (binds `127.0.0.1:8080` by default; override with an argument
or the `HTMLTOPDF_ADDR` env var):

```bash
cargo run --release -p htmltopdf-server
# custom address, and tune the server worker-thread count:
HTMLTOPDF_WORKERS=24 cargo run --release -p htmltopdf-server -- 0.0.0.0:9000
```

Concurrency is tunable on both ends: the **server** worker count via
`HTMLTOPDF_WORKERS` (default = one per CPU core), and the **client** load via the
`-c` flag of `scripts/api-convert.sh` (below).

Endpoints:

| Method | Path | Description |
| --- | --- | --- |
| `POST` | `/render` | Request body = HTML; response = `application/pdf` |
| `GET` | `/health` | Liveness check → `ok` |
| `GET` | `/` | Usage help |

`POST /render` query options: `landscape=true`, `margin=<points>`,
`font=<path-or-family>`.

```bash
# Basic
curl -X POST http://127.0.0.1:8080/render \
  -H 'Content-Type: text/html' \
  --data-binary @examples/invoice.html -o invoice.pdf

# Landscape, 36pt margins, embed the Georgia font
curl -X POST 'http://127.0.0.1:8080/render?landscape=true&margin=36&font=Georgia' \
  --data-binary @examples/invoice.html -o invoice.pdf
```

**Batch convert / load-test:** `scripts/api-convert.sh` posts an HTML file to the
API, saves the PDF, and prints per-request latency plus a summary
(min/avg/p50/p95/max, throughput):

```bash
scripts/api-convert.sh                     # one request, saves out/reg-2-9-1-copy.pdf
scripts/api-convert.sh -c 16 -n 64         # 64 requests, 16 concurrent
scripts/api-convert.sh -c 8 -q 'font=Georgia'   # with query options
# flags: -u URL  -i input.html  -o output.pdf  -c concurrency  -n total  -q query
```

**In Postman:** method `POST`, URL `http://127.0.0.1:8080/render`, Body → `raw`
→ paste your HTML (the type can be Text or HTML). Hit Send, then **Save
Response → Save to a file** to get the PDF. Add query params under the Params
tab. Each request is handled on its own worker thread, so it scales across cores.

## 8. What works today vs. what's next

**Works now**

- Real HTML parsing (handles messy real-world markup).
- Real CSS parsing + cascade: selectors (type/**id**/class/**universal**/
  **attribute** + **descendant, child, and sibling combinators** — ` `, `>`, `+`,
  `~` + **structural pseudo-classes** like `:nth-child`, `:first-of-type`,
  `:not`), specificity, source order, `!important`, **`@media print` evaluation**,
  comments/strings, multiple `<style>` blocks.
- Inheritance (color, font-size, text-align, etc. flow from parents).
- **Custom properties** (`--x`) and `var(--x, fallback)` — cascade + inherit,
  nested/aliased variables, missing-variable fallbacks, and component-scoped
  overrides (a redefined `--x` on an ancestor recolors its subtree).
- **`calc()`** — `+ - * /`, parentheses, nested calc, unit mixing; a mixed
  `calc(100% - 20px)` resolves against the containing block at layout time and
  composes with `var()`.
- **Typography**: `text-transform` (incl. plain `th` cells), `letter-spacing`
  (PDF `Tc`, kerning preserved), `word-spacing`, `text-indent` (pt/`%`, first
  line only).
- `display: none` (tables and flow content).
- Colors: hex (3/4/6/8-digit), `rgb()`/`rgba()`/`hsl()`/`hsla()`, named colors.
- `font-weight` (bold / numeric ≥ 600) — a **real bold face** when the family
  is known (via `font-family` or a generic), synthesized fill+stroke otherwise;
  `font-style: italic` with real italic faces; **per-element `font-family`**
  (named families + generics resolve to system faces, several subset faces per
  document); `text-align` (left/center/right/justify/start/end), headings
  `h1`–`h6`.
- Tables: rows, cells, colspans, rowspans (occupancy grid; spanning cells paint
  once across their rows and split at page breaks), `<thead>/<tbody>/<tfoot>`,
  repeated headers, per-cell styles, borders, backgrounds, alignment, text
  wrapping/clipping, and
  **browser-style automatic column layout** (min/max-content widths; declared
  widths honored when they fit; over-wide tables shrink-to-fit like print output
  instead of clipping). Cell text renders at its real CSS size, and row height is
  content-driven (verified against Chrome's `--print-to-pdf`).
- Flow content: a **nested block/inline box tree** — nested blocks, list and
  blockquote indentation, list markers (`•` / `1.`), and per-run inline
  `color`/`font-size`/bold within a paragraph, all wrapped and aligned.
- **CSS box model on blocks**: `margin`/`padding` (shorthands + longhands, in
  points or **percentages** of the containing width), vertical margin collapse,
  block backgrounds (solid color, `linear-gradient()`, and `background-image:
  url()` with size/position/repeat) + real per-side `border` painted behind
  content (per page fragment), `border-radius`, `box-sizing`, `line-height`,
  `width`/`min-width`/`max-width` (points or `%`), `min-height`/`max-height`
  (points), **`margin: auto` centering**, and `overflow: hidden` clipping of a
  fixed-height box.
- **Modern layout, first pass each**: flexbox (`display: flex` — grow/basis/
  shrink, `order`, `wrap`/`wrap-reverse`, justify/align/`align-self`/
  `align-content`, gaps, row+column), grid (`display: grid` — `fr`/`auto`/
  `repeat()` tracks, spans, gaps), **`display: inline-block`** (a block box that
  flows inline on the baseline — badges/buttons/tags), **floats** with real text
  wrap and `clear`,
  and **positioning** — `relative`, `absolute` (positioned-ancestor containing
  blocks), `fixed` **repeated on every page** (headers/watermarks), and
  `z-index` ordering (positioned content paints above the flow).
- **Text shaping** (HarfBuzz via `rustybuzz`) for embedded fonts — kerning
  reproduced in the PDF, ligatures with extractable text, Arabic joining forms —
  plus **bidirectional text + RTL paragraphs** (UAX #9 visual reordering;
  `dir="rtl"`/`direction: rtl` set the base direction and right-align)
  and **font fallback chains** (CJK/Hangul/Cyrillic fall back to covering
  system faces automatically, each embedded as its own subset).
- Non-ASCII text: WinAnsi/CP1252 characters render with the built-in Helvetica;
  with an embedded font (`--font`) **any Unicode** renders (Latin, CJK, …) via a
  Type0/Identity-H composite + ToUnicode CMap, and stays selectable/searchable.
- **Clickable links + document outline** — `<a href>` becomes a `/Link`
  annotation (URI actions, `mailto:`, in-document `#fragment` jumps to `id`
  anchors) with UA styling (blue + underline; author color and
  `text-decoration: none` respected); headings (`h1`–`h6`) build the PDF
  bookmark tree, nested by level.
- Pagination, landscape, page margins, PDF compression, selectable text.
- **Font embedding** — `--font <path|family>` embeds a TrueType/OpenType font
  (real metrics via ttf-parser, family lookup via fontdb) as a Type0/Identity-H
  composite with a ToUnicode CMap, so any Unicode renders and text stays
  selectable. Glyph **subsetting** (retain-GIDs) embeds only the used glyphs
  (e.g. a CJK doc dropped from 33 MB to 0.65 MB).
- **`@font-face` web fonts** — author-declared families load ahead of system
  lookup: `url()` sources accept TrueType/OpenType/WOFF1 from `data:` URIs,
  local files, or (opt-in, SSRF-guarded) remote URLs; `local()` matches
  family/full/PostScript names; unsupported `format()` candidates (WOFF2) are
  skipped down the `src:` chain; per-family `font-weight`/`font-style` rules
  select real bold/italic variants.
- **JavaScript (opt-in)** — with the `js` build feature, a bounded pre-layout
  stage (Boa) runs inline `<script>`s against a live DOM: `getElementById`,
  `textContent`, `get/setAttribute`, `innerHTML` (get/set), `createElement`/
  `createTextNode`/`appendChild`/`removeChild`, and `document.body` — enough to
  build a whole document from script, within node/iteration budgets. Enable per
  render via `Engine::render_html_with_scripts` or the CLI `--js` flag.
- **Images** — block-level `<img>` from file paths (resolved against the input's
  directory) and `data:` URIs. JPEG embeds verbatim via `DCTDecode`; PNG is
  decoded in-house (chunk parse, `flate2` inflate, unfilter, palette/alpha) with
  its alpha channel emitted as a PDF soft mask. Sized by `width`/`height` with
  aspect-ratio preservation, scaled to fit the page, and page-broken as a unit.
  An image sharing a line with text flows **inline** on the baseline (icons
  in a sentence); standalone images stay block-level and floated ones wrap.
  **Remote `http(s)` images** are opt-in behind the `remote-images` feature: a
  fail-closed, capped, SSRF-guarded blocking fetch (see status section).

**Not yet (the honest list)**

- Border polish: `double`/`groove`/`ridge` render as solid, the stroke is
  centered on the border-box edge (not fully inside it), per-corner /
  elliptical `border-radius`, and rounded-corner *content clipping*.
- `object-fit` / `vertical-align` on images (inline images sit on the
  baseline only); remote images need the opt-in `remote-images` feature (not
  in default builds); a true font-metric baseline model (0.8 em ascent
  approximation today).
- Isolated stacking contexts (`z-index` compares globally — negative z does
  paint below the flow now); grid named lines/areas, `grid-template-rows`, and
  per-cell alignment. (Flex `flex-shrink`/`order`/`align-self`/`align-content`/
  `wrap-reverse` shipped.)
- Images / nested block layout inside table cells; tagged PDF.
- `@font-face` web fonts; emoji; `dir="auto"` and RTL table cells; `%` heights/
  margins/offsets; `calc()`/custom properties.
- Broader **JavaScript**: DOM traversal from JS, `querySelector`, events,
  timers (mid-script layout reads rejected by design — ADR 0009).
- Subsetting covers `glyf`-based TrueType; CFF/OpenType-CFF fonts embed in full.
- **SVG and canvas.**

The guiding rule (from PLAN.md): build real, spec-based behavior step by step,
and don't claim support for something until it's actually implemented and tested.

---

## 9. The build order we're following

Foundation first, so features attach to something solid. Done ✓ / next ▶:

```
✓ Real font metrics
✓ Real DOM (html5ever, arena)
✓ Tables read from the DOM
✓ Custom DOM builder (low RAM)
✓ Real CSS parsing (cssparser)
✓ Computed styles + inheritance
✓ Box generation from `display` (flow content + display:none)
✓ Font embedding (ttf-parser + fontdb, opt-in via --font)
✓ Full nested box-tree layout (box_tree.rs: nesting, lists, inline runs)
✓ CSS box model on blocks (margins/padding, margin collapse, borders/backgrounds)
✓ CID/Unicode font embedding (Type0/Identity-H + ToUnicode; any-language text)
✓ Font subsetting (retain-GIDs glyf/loca rebuild; embed only used glyphs)
✓ JavaScript pre-layout stage — first pass (Boa behind the `js` feature)
✓ Block-level `<img>` images (JPEG DCTDecode + in-house PNG decode; XObjects)
✓ Real border model (per-side width/style/color, dashed/dotted, radius, box-sizing)
▶ Broader JS DOM API (innerHTML/createElement) + heap/time limits
· CFF/OpenType-CFF subsetting; inline/floated images
```

Every step keeps the test suite green and the test spreadsheet rendering
byte-for-byte identical, so we always know we didn't break anything.
