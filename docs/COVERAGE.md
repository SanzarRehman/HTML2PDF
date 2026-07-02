# Feature Coverage

What htmltopdf renders today, and what it doesn't yet. Legend: ✅ supported ·
🟡 partial · ❌ not yet. This is a living support matrix; the authoritative task
list is [../IMPLEMENTATION.md](../IMPLEMENTATION.md), and the parity fixtures in
`crates/htmltopdf/tests/fixtures/` exercise most of the ✅/🟡 rows.

> **Not a browser (yet).** The goal is low-RAM, highly-concurrent, browser-free
> HTML→PDF. Layout is block/inline flow + automatic tables. There is no flexbox,
> grid, float, or positioning, so page layouts that depend on those will not match
> a browser.

## HTML elements

| Element(s) | Status | Notes |
|---|---|---|
| `h1`–`h6`, `p`, `div`, `section`, `article`, `main`, `header`, `footer`, `nav`, `aside`, `address`, `figure`, `figcaption`, `pre` | ✅ | Block flow with margins/padding/borders. |
| `ul`, `ol`, `li`, `dl`, `dt`, `dd`, `blockquote` | ✅ | Bullets/numbers; nesting indents. |
| `b`, `strong`, `i`, `em` | 🟡 | Bold is faux-bold (fill+stroke); italic is **not** slanted (one font face). |
| `u`, `ins`, `s`, `strike`, `del` | ✅ | Underline / line-through decoration. |
| `span`, inline text | ✅ | Per-run color/size/weight/decoration. |
| `br` | ✅ | Hard line break. |
| `table`, `thead`, `tbody`, `tfoot`, `tr`, `td`, `th`, `col` | ✅ | Automatic column sizing, header repeat across pages, **now renders alongside surrounding flow content**. |
| `colspan` | ✅ | |
| `rowspan` | ❌ | Ignored. |
| `caption`, nested tables | ❌ | |
| `img` | 🟡 | Block-level only (see Images). |
| `a` | 🟡 | Text renders; **no clickable link annotation**. |
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
| `:hover`, `:link`, `::before`/`::after`, other pseudo-elements | ❌ | |
| Specificity + source-order cascade, `!important` | ✅ | |
| `@media print` / `screen` | ✅ | Screen-only rules excluded. |
| `@page` margins + orientation | ✅ | `size: landscape`, `margin`. |
| `@font-face`, `@supports`, `@keyframes`, `@import` | ❌ | |

## CSS properties

| Property | Status | Notes |
|---|---|---|
| `color` | ✅ | hex, `rgb()/rgba()`, `hsl()/hsla()`, named, `transparent` (alpha ignored). |
| `background-color` / `background` | 🟡 | Solid color only (no images/gradients). |
| `font-size` | ✅ | px/pt/in/cm/mm. |
| `font-weight` | 🟡 | Bold via faux-bold; numeric ≥600 = bold. |
| `text-align` | ✅ | left/center/right (no `justify`). |
| `vertical-align` | ✅ | top/middle/bottom/baseline (table cells). |
| `text-decoration` | 🟡 | `underline`, `line-through`, `none`; no `overline`/color/style; can't cancel an ancestor's. |
| `margin` (+ longhands, shorthand) | ✅ | Vertical margins collapse. |
| `padding` (+ longhands, shorthand) | ✅ | |
| `border` (+ per-side) | 🟡 | On/off + width; color is always black. |
| `width` / `height` | 🟡 | Honored on `img` and table `col`; not general block sizing. |
| `white-space` | 🟡 | `normal` / `nowrap`. |
| `overflow` | 🟡 | `visible` / `hidden`. |
| `overflow-wrap`, `word-break` | ✅ | |
| `display` | 🟡 | `none` and `table-*-group`; no `flex`/`grid`/`inline-block`. |
| `line-height` | ❌ | Fixed leading (`font×1.35` flow, `×1.18` cells). |
| `font-family` | ❌ | Default base-14, or one `--font`; no per-element families. |
| `font-style: italic` | ❌ | |
| `letter-spacing`, `text-indent`, `text-transform`, `word-spacing` | ❌ | |
| `display: flex` (+ `flex`, `flex-grow`, `flex-basis`, `justify-content`, `align-items`, `gap`, `flex-direction`) | 🟡 | Row: grow/basis sizing, justify-content, **align-items** (center/end via measure pass), inline (`span`) children promoted to items, anonymous text items. **Column**: vertical stack with `gap` (no height grow/justify). No `flex-wrap`, explicit `flex-shrink`/`order`, `align-self`, or cross-page rows. |
| `display: grid` (+ `grid-template-columns`, `gap`/`row-gap`/`column-gap`, `grid-column: span N`) | 🟡 | Tracks: fixed lengths, `fr`, `auto`, `repeat(N, …)`. Row-major auto-placement; rows sized to tallest item; page-break between rows. No line-based placement (`1 / 3`), named lines/areas, `minmax()`, `grid-template-rows`, dense packing, or cell alignment. |
| `float`, `clear` | ❌ | |
| `position` (relative/absolute/fixed/sticky), `top`/`left`/`z-index` | ❌ | |
| `columns` (multi-col), `flex-wrap`, `flex-shrink` (explicit), `order` | ❌ | |
| `transform`, `opacity`, `box-shadow`, `border-radius`, `filter` | ❌ | |
| `object-fit`, `max-width`/`min-width`/`max-height`/`min-height` | ❌ | |
| `calc()`, custom properties (`--var`, `var()`) | ❌ | |
| `%` lengths | ❌ | Only absolute units. |

## Images

| Feature | Status | Notes |
|---|---|---|
| PNG (8-bit, non-interlaced), JPEG | ✅ | In-house PNG decode; JPEG embedded via DCTDecode. |
| `data:` URIs, local file paths | ✅ | |
| Block-level placement, CSS `width`/`height` (aspect-preserving) | ✅ | |
| PNG alpha → `/SMask` | ✅ | |
| Inline/floated images | ❌ | Always breaks to its own line. |
| Remote `http(s)` URLs | ❌ | |
| `object-fit`, `max-width:100%`, `%` sizes | ❌ | |
| Sub-byte / interlaced / 16-bit PNG; GIF, WebP, SVG, BMP | ❌ | |
| `srcset` / `<picture>` | ❌ | |

## Fonts & text

| Feature | Status | Notes |
|---|---|---|
| Base-14 standard PDF fonts (default) | ✅ | WinAnsi text, selectable. AFM per-char metrics (no shaping — no face to shape with). |
| Embed one TTF via `--font` | ✅ | Type0/Identity-H, glyph subsetting, ToUnicode. |
| **Text shaping (HarfBuzz via `rustybuzz`)** for embedded fonts | ✅ | Kerning (measured *and* reproduced in PDF via `TJ` adjustments), ligatures (GSUB; ToUnicode maps a ligature glyph back to all its chars), Arabic joining forms with correct in-run RTL order. Shaped-run cache keyed by string. |
| Real glyph metrics + line breaking | ✅ | via `ttf-parser`/`fontdb`; widths are shaped widths when a face is embedded. |
| Bidi paragraph reordering (mixed LTR/RTL) | ❌ | A single-script run renders correctly; mixed-direction paragraphs are not reordered (no UAX #9). |
| Bold/italic faces, multiple families, font fallback | ❌ | Faux-bold only; one face per document — no CJK/emoji fallback chain. |

## JavaScript (opt-in: `--js` / `js` feature)

| Feature | Status | Notes |
|---|---|---|
| Inline `<script>`, bounded pre-layout run (Boa) | ✅ | Loop-iteration limit. |
| `document.getElementById`, `textContent`, `get/setAttribute`, `console.log` | ✅ | |
| `innerHTML`, `createElement`, events, timers, `fetch`, layout reads | ❌ | |

## PDF output

| Feature | Status | Notes |
|---|---|---|
| PDF 1.7, streaming, FlateDecode compression | ✅ | |
| Image XObjects, per-page backgrounds/borders | ✅ | |
| Multi-page pagination, repeated table headers | ✅ | |
| Configurable page size (A4/Letter, portrait/landscape), margins | ✅ | `--paper`, `@page`. |
| Bookmarks/outline, link annotations, headers/footers, tagged PDF, encryption | ❌ | |
