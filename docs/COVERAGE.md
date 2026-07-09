# Feature Coverage

What htmltopdf renders today, and what it doesn't yet. Legend: ✅ supported ·
🟡 partial · ❌ not yet. This is a living support matrix; the authoritative task
list is [../IMPLEMENTATION.md](../IMPLEMENTATION.md), and the parity fixtures in
`crates/htmltopdf/tests/fixtures/` exercise most of the ✅/🟡 rows.

> **Not a browser (yet).** The goal is low-RAM, highly-concurrent, browser-free
> HTML→PDF. Layout covers block/inline flow, tables, and first-pass flexbox,
> grid, floats, and positioning — deep browser features (stacking contexts,
> `%` lengths, `calc()`, SVG) are still missing, so layouts leaning on those
> will not match a browser yet.

## HTML elements

| Element(s) | Status | Notes |
|---|---|---|
| `h1`–`h6`, `p`, `div`, `section`, `article`, `main`, `header`, `footer`, `nav`, `aside`, `address`, `figure`, `figcaption`, `pre` | ✅ | Block flow with margins/padding/borders. |
| `ul`, `ol`, `li`, `dl`, `dt`, `dd`, `blockquote` | ✅ | Bullets/numbers; nesting indents. |
| `b`, `strong`, `i`, `em`, `cite`, `var`, `dfn` | ✅ | Real bold/italic faces when the family is known (named `font-family` or a generic); faux-bold fill+stroke only when no bold face resolves. |
| `u`, `ins`, `s`, `strike`, `del` | ✅ | Underline / line-through decoration. |
| `span`, inline text | ✅ | Per-run color/size/weight/decoration. |
| `br` | ✅ | Hard line break. |
| `table`, `thead`, `tbody`, `tfoot`, `tr`, `td`, `th`, `col` | ✅ | Automatic column sizing, header repeat across pages, renders alongside surrounding flow content. **Rich cell content**: a cell with inline markup carries styled runs — mixed bold/italic/color/size segments, clickable `<a href>` links, underline/strike, and RTL cells (`dir`/`direction` set the base level, reorder per UAX #9, right-align by default). Plain text-only cells keep the fast single-style path. Column sizing still measures the flattened text (a heavily-bold cell can be slightly under-measured); no images/`<br>`/nested blocks *as blocks* inside cells. Cells honor per-side borders with width/color (`th { border-bottom: 2px solid }` paints just that edge); uniform black keeps the fast rect path. |
| `colspan` | ✅ | |
| `rowspan` | ❌ | Ignored. |
| `caption`, nested tables | ❌ | |
| `img` | 🟡 | Inline when it shares a line with text (baseline-aligned, wraps like a word, clickable inside `<a>`); block path for standalone/floated images (see Images). |
| `a` | 🟡 | Clickable `/Link` annotations: external URIs and `mailto:` as URI actions, `#fragment` as in-document jumps to `id` anchors (dead fragments annotate nothing). UA style applied (blue + underline; author `color` and `text-decoration: none` win). Adjacent words merge into one clickable rect per line. Links inside table cells work (rich cells carry styled runs). |
| `form`, `fieldset`, `input`, `select`, `textarea`, `button` | ❌ | Containers may render text; no form controls. |
| `svg`, `canvas`, `video`, `audio`, `iframe`, `object` | ❌ | |
| `script`, `style`, `head`, `title` | ✅ | Consumed (CSS parsed; scripts only with `--js`), not rendered. |

## CSS selectors & at-rules

| Feature | Status | Notes |
|---|---|---|
| Type, `.class`, `#id`, `*` | ✅ | |
| Attribute `[a]`, `[a=v]`, `~=`, `\|=`, `^=`, `$=`, `*=` | ✅ | |
| Descendant, child `>`, sibling `+` / `~` | ✅ | |
| `:nth-child`, `:first/last-child`, `:*-of-type`, `:empty`, `:root`, `:not()` | ✅ | |
| `:hover`, `:active`, `:focus` (dynamic pseudo-classes) | — | Out of scope by design: the target is static print, so these can never fire. Selectors using them are dropped, never over-applied. |
| `:link`/`:visited`, `::before`/`::after` + `content` | ❌ | Generated content does matter for print; queued. |
| Specificity + source-order cascade, `!important` | ✅ | |
| `@media print` / `screen` | ✅ | Screen-only rules excluded. |
| `@page` margins + orientation | ✅ | `size: landscape`, `margin`. |
| `@font-face` | 🟡 | Author families shadow system lookup. `src:` fallback chain honored: `url()` loads TrueType/OpenType/**WOFF1** from `data:` URIs and local files (remote `http(s)` behind the same opt-in `RemoteImagePolicy` as images); `local()` matches family, PostScript, and "Family Bold/Italic" full names; unsupported `format()` hints (WOFF2/EOT/SVG) skipped without fetching. Per-family `font-weight`/`font-style` descriptors select real bold/italic variants; missing variants synthesize bold. No WOFF2 (needs Brotli), `unicode-range`, `font-display`, or variable-font selection; fonts load per render (no cache). |
| `@supports`, `@keyframes`, `@import` | ❌ | |

## CSS properties

| Property | Status | Notes |
|---|---|---|
| `color` | ✅ | hex, `rgb()/rgba()`, `hsl()/hsla()`, named, `transparent` (alpha ignored). |
| `background-color` / `background` | 🟡 | Solid color only (no images/gradients). |
| `font-size` | ✅ | px/pt/in/cm/mm. |
| `font-weight` | 🟡 | Numeric ≥600 = bold. Real bold face when the family is known; faux-bold (fill+stroke) otherwise (e.g. the default font with no `font-family`). |
| `text-align` | ✅ | left/center/right/justify (justified lines stretch inter-word spaces; the paragraph's last line stays ragged; cells treat justify as left). |
| `vertical-align` | ✅ | top/middle/bottom/baseline (table cells). |
| `text-decoration` | 🟡 | `underline`, `line-through`, `none`; no `overline`/color/style; can't cancel an ancestor's. |
| `margin` (+ longhands, shorthand) | ✅ | Vertical margins collapse. Lengths and `%` (of the containing width, all four sides). |
| `padding` (+ longhands, shorthand) | ✅ | Lengths and `%` (of the containing width, all four sides; `padding: 5% 10px` mixes units per side). |
| `border` (+ per-side, `border-width/style/color`, all 12 longhands) | ✅ | Per-side width (incl. `thin/medium/thick`), style, and color; `solid`/`dashed`/`dotted` real (double/groove/ridge/inset/outset render solid); color defaults to `currentColor`. Borders consume layout space; backgrounds cover the border box. Uniform borders stroke one rect (fast path); mixed sides stroke per-edge segments; fragmented blocks repeat left/right edges, top/bottom only on first/last page. Stroke is centered on the border-box edge (CSS puts the band fully inside). Legacy lenience: a width with no style still paints solid (`border: 1px` gridlines). |
| `width` / `height` | 🟡 | `width` honored on `img`, table `col`, floats, positioned boxes, and in-flow blocks. `height` on `img` and on blocks — treated as a **minimum** box height (short content extends the box; taller content overflows visibly unless `overflow: hidden` clips it; the extension never crosses a page break). An empty div with a background + explicit size still paints (background-layer pattern). `%` height not yet (indefinite containing height in flow). |
| `min-width` / `min-height` / `max-height` | 🟡 | `min-width` (pt / `%` of containing width) clamps the used width up, winning over `max-width`. `min-height` (pt) extends the box down like `height`. `max-height` (pt) caps the box **with `overflow: hidden`** (clipped on the start page). `%` on the height family not yet. |
| `white-space` | 🟡 | `normal` / `nowrap`. |
| `overflow` | 🟡 | `visible` / `hidden`. On a block with a definite height (`height`/`max-height`), `hidden`/`clip` caps and clips content to the border box on its start page (a `re W n` clip). Multi-page clipped boxes and horizontal-only clipping not yet. |
| `overflow-wrap`, `word-break` | ✅ | |
| `display` | 🟡 | `none` and `table-*-group`; no `flex`/`grid`/`inline-block`. |
| `line-height` | 🟡 | Unitless number, `%`, and absolute lengths, on flow blocks and table cells; inherits; extra leading split as half-leading (Chrome-like). Defaults stay `font×1.35` flow / `×1.18` cells. No `normal` *override* of an inherited value, block-level only (no per-inline-run line-height). |
| `font-family` | 🟡 | Per-element: named families resolve to real system faces (embedded + subset, several per document); generics (`serif`/`sans-serif`/`monospace`/…) map to system defaults; `pre`/`code`/`kbd`/`samp` default to monospace. First usable family in the stack wins (no per-character walk down the stack — the fallback chain handles missing glyphs). Unknown families fall back to the document font. |
| `font-style: italic` | 🟡 | Real italic faces when the family is known (`<i>`/`<em>` + `font-style`); no synthetic slant otherwise. | |
| `text-transform` | ✅ | `uppercase`/`lowercase`/`capitalize`/`none` (inherited; explicit `none` overrides an ancestor). Applied as text is collected — measurement and column sizing see the transformed string; works in flow, rich cells, and plain table cells; `capitalize` tracks word boundaries across style runs. |
| `letter-spacing` | 🟡 | Lengths incl. negative (`normal` = explicit 0). Affects wrapping/measurement and is reproduced in the PDF via the `Tc` char-spacing state (kerning preserved). Flow text and rich table cells; not plain single-style cells. |
| `word-spacing` | 🟡 | Lengths (`normal` = 0): widens inter-word spaces (composes with `text-align: justify`). Flow text and rich table cells. |
| `text-indent` | 🟡 | Points or `%` of the containing width (inherited); indents only the block's first line (negative = hanging indent into the padding). Applies to the block's own first line box, not one inherited into a nested block's first line. |
| `display: flex` (+ `flex`, `flex-grow`, `flex-basis`, `flex-wrap`, `flex-flow`, `justify-content`, `align-items`, `gap`, `flex-direction`) | 🟡 | Row: grow/basis sizing, justify-content, **align-items** (center/end via measure pass), **`flex-wrap: wrap`** (greedy lines by base size; justify/align apply per line; `gap` doubles as cross-axis line gap), inline (`span`) children promoted to items, anonymous text items. **Column**: vertical stack with `gap` (no height grow/justify). No explicit `flex-shrink`/`order`, `align-self`, `align-content`, `wrap-reverse`, or cross-page rows. |
| `display: grid` (+ `grid-template-columns`, `gap`/`row-gap`/`column-gap`, `grid-column`) | 🟡 | Tracks: fixed lengths, `fr`, `auto`, `repeat(N, …)`, **`minmax(min, max)`** (pt/auto floors; pt/fr/auto ceilings; fr floors pinned iteratively). Placement: row-major auto-placement, `span N`, and **line-based `grid-column: A / B`** incl. negative lines (`1 / -1` = full row; an explicit start pins the column, wrapping to the next row if the cursor passed it). No named lines/areas, `grid-template-rows`, dense packing, collision-aware placement, or cell alignment. |
| `float: left/right` + `clear` | 🟡 | Floated blocks (shrink-to-fit or CSS `width`) and floated images; line boxes shorten around the exclusion bands (interval-accurate for stacked floats) and re-widen below; a word that can't fit beside a float drops below it instead of breaking. Floats never split across pages (page break retires them). No `clear` on inline content, no float stacking overflow to a new band row, no margins between float and wrapped text beyond the float's own box. |
| `position: relative/absolute/fixed` (+ `top`/`right`/`bottom`/`left`, `z-index`) | 🟡 | Relative = visual offset with flow preserved. Absolute = out of flow; `left`/`right`/`top` resolve against the nearest **positioned ancestor's** containing block (else the page content box), `bottom` against the page. **`fixed` repeats on every page** (headers/footers/watermarks). Positioned boxes paint ordered by `z-index` (integer; `auto`=0): non-negative z above in-flow content, **negative z below it** (the `z-index: -1` background layer; nested negative-z descendants paint below their positioned ancestor's content too). Absolute boxes don't paginate (content past the page bottom is dropped). z compares globally (no isolated per-context stacking from `opacity`/`transform`), no `sticky`. **`%` offsets**: left/right resolve against the containing width (relative + absolute); top/bottom against the containing height only when it's the page (a positioned ancestor's height isn't tracked). |
| `width` on in-flow blocks | 🟡 | Content-box width (points or `%`), `min-width`/`max-width` (points or `%`), and **`margin: auto` horizontal centering**. |
| `columns` (multi-col), `flex-shrink` (explicit), `order` | ❌ | |
| `border-radius` | 🟡 | Single uniform radius on blocks: rounds the background fill and (uniform) border stroke via Bézier paths. No per-corner/elliptical radii, no content clipping to the rounded shape, not on table cells. |
| `box-sizing` | ✅ | `border-box`/`content-box` on in-flow blocks, floats, and positioned boxes (width and min-height). |
| `transform`, `opacity`, `box-shadow`, `filter` | ❌ | |
| `max-width` (pt / `%`) | 🟡 | On blocks and images (`max-width: 100%` works). |
| `object-fit` | ❌ | |
| Custom properties (`--var`, `var()`) | 🟡 | Custom properties cascade and inherit; `var(--name, fallback)` resolves (nested/aliased vars, component-scoped overrides, fallbacks). Values referencing a var are resolved in the top-down pass, so scoped redefinitions recolor subtrees. No `@property`, no `var()` in selectors, and `!important` custom properties don't override a normal one. |
| `calc()` | 🟡 | `+ - * /`, parentheses, nested `calc()`, and unit mixing (px/pt/in/cm/mm/%). Resolves to a point + percent pair, so a mixed `calc(100% - 20px)` resolves against the containing block at layout — on `width`/`min-width`/`max-width`, padding, margin, and positioned-box offsets. Point-only calc also works for `height`/`min-height`/`max-height`. `*`/`/` require a unitless operand (per spec). No math functions (`min()`/`max()`/`clamp()`), no unit types beyond length/percent, and calc percentages on the height family aren't resolved (indefinite containing height). |
| `%` lengths | 🟡 | `width`/`min-width`/`max-width`, padding, and margin on blocks (of the containing width); box offsets on positioned boxes; images. Not yet on `height`/`min-height`/`max-height` (indefinite containing height in flow). |

## Images

| Feature | Status | Notes |
|---|---|---|
| PNG (8-bit, non-interlaced), JPEG | ✅ | In-house PNG decode; JPEG embedded via DCTDecode. |
| `data:` URIs, local file paths | ✅ | |
| Block-level placement, CSS `width`/`height` (aspect-preserving) | ✅ | |
| PNG alpha → `/SMask` | ✅ | |
| Inline/floated images | 🟡 | **Inline**: an `<img>` with text on its line flows in place — bottom on the baseline, the line box grows to the image, over-wide images scale to the line, linked images clickable. Standalone images stay block-level; **floats** wrap text around the image. No `vertical-align` variants (baseline only), no descender-aware baseline. |
| Remote `http(s)` URLs | 🟡 | Opt-in behind the `remote-images` cargo feature *and* a per-render flag (`RemoteImagePolicy { enabled: true }`); **fail-closed** otherwise. Byte + timeout caps; blocks loopback/private/link-local/CGNAT hosts (SSRF guard); no redirects. Not in default builds (keeps the base engine free of a TLS/networking stack). No DNS-rebinding pin, no redirect following, no on-disk cache. |
| CSS `%` width / `max-width` (incl. `max-width:100%`) | ✅ | Percent of the containing block; `%` may scale up, `max-width` clamps. |
| `object-fit` | ❌ | |
| Sub-byte / interlaced / 16-bit PNG; GIF, WebP, SVG, BMP | ❌ | |
| `srcset` / `<picture>` | ❌ | |

## Fonts & text

| Feature | Status | Notes |
|---|---|---|
| Base-14 standard PDF fonts (default) | ✅ | WinAnsi text, selectable. AFM per-char metrics (no shaping — no face to shape with). |
| Embedded TrueType faces (`--font`, `font-family`, fallback) | ✅ | Type0/Identity-H, per-face glyph subsetting, ToUnicode; several faces per document. |
| **Text shaping (HarfBuzz via `rustybuzz`)** for embedded fonts | ✅ | Kerning (measured *and* reproduced in PDF via `TJ` adjustments), ligatures (GSUB; ToUnicode maps a ligature glyph back to all its chars), Arabic joining forms with correct in-run RTL order. Shaped-run cache keyed by string. |
| Real glyph metrics + line breaking | ✅ | via `ttf-parser`/`fontdb`; widths are shaped widths when a face is embedded. |
| Bidi reordering + RTL base (UAX #9) | 🟡 | Line pieces reorder visually against the paragraph's base level; shaping itemizes each string into directional runs (joining computed on logical text, glyphs emitted visually). **`dir="rtl"` / `direction: rtl`** set the base direction (inherited; block-level), flipping the base level to RTL and right-aligning by default; an explicit `text-align` overrides. Works for embedded fonts (base-14 has no RTL glyphs; the fallback chain supplies them). RTL base works inside table cells too (`dir` on the cell / CSS `direction`; right-aligned by default). No `dir="auto"`, bracket mirroring, or inline `<bdi>`/`<bdo>` embeddings; an ancestor's `dir` *attribute* doesn't reach cells (CSS `direction` inherits fine). |
| **Font fallback chain** (CJK, Hangul, Cyrillic, …) | 🟡 | Characters the primary font lacks fall back to system faces (Arial Unicode MS / Noto Sans / DejaVu Sans, first that covers), each embedded as its own subset Type0 resource — works from the base-14 default *and* from an embedded `--font`. Measurement is chain-aware. Emoji excluded (color faces can't embed as outlines). Chain is fixed, not configurable; char-level line breaking still measures with the primary. |
| Bold/italic faces, multiple families per document | ✅ | Resolved per element via `fontdb` (weight/style queries), each face embedded as its own subset resource; process-wide face cache. |
| `@font-face` web fonts | 🟡 | Declared families resolve ahead of system lookup and embed/subset/shape like any other face (see the at-rules table for `src:` details). |

## JavaScript (opt-in: `--js` / `js` feature)

| Feature | Status | Notes |
|---|---|---|
| Inline `<script>`, bounded pre-layout run (Boa) | ✅ | Loop-iteration limit. |
| `document.getElementById`, `textContent`, `get/setAttribute`, `console.log` | ✅ | |
| `innerHTML` (get/set) | ✅ | Structural mutation via fragment parse + graft (ADR 0008). Node budget enforced. |
| `createElement`, `createTextNode`, `appendChild`, `removeChild`, `document.body` | ✅ | Detached-node creation, attach/move/reparent with cycle guard, detach (ADR 0009). Created nodes get normal CSS cascade. |
| `insertBefore`, `cloneNode`, `querySelector`, `parentNode`/`children` traversal | ❌ | |
| Events, timers, `fetch`, layout reads (`getBoundingClientRect`) | ❌ | Layout reads rejected by design — see ADR 0009. |

## PDF output

| Feature | Status | Notes |
|---|---|---|
| PDF 1.7, streaming, FlateDecode compression | ✅ | |
| Image XObjects, per-page backgrounds/borders | ✅ | |
| Multi-page pagination, repeated table headers | ✅ | |
| Configurable page size (A4/Letter, portrait/landscape), margins | ✅ | `--paper`, `@page`. |
| Link annotations (`/Annots`) | 🟡 | URI actions + in-document `/Dest` (`#fragment` → `id` anchor); one merged rect per link per line, including inside table cells. No `PageMode /UseOutlines`, no `<a name>` anchors. |
| Document outline (`/Outlines`) | ✅ | Built from `h1`–`h6` in document order; deeper levels nest under the closest shallower heading; non-ASCII titles as UTF-16BE. |
| Headers/footers, tagged PDF, encryption | ❌ | |
