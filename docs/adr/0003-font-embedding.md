# ADR 0003: Font Embedding

## Status

Accepted (2026-06-29). Builds on ADR 0002 step 8. The simple
`/TrueType`+`/WinAnsiEncoding` embedding described here is **superseded by ADR
0005** (Type0/Identity-H composite + ToUnicode); the metrics and `/FontFile2`
program described here are reused unchanged.

## Context

Until now every PDF referenced the standard-14 `Helvetica` and embedded no font.
That is small and fast but limits output to one built-in typeface and to
Helvetica's metrics. "Real" output — invoices/reports in a chosen brand font —
needs the engine to embed an actual font and measure text with that font's
metrics so layout and rendering agree.

Two hard requirements shaped the design:

- **Determinism.** Output must not silently depend on whatever font a machine
  happens to have. So the font is an explicit input, not auto-discovered.
- **No regression.** The existing Helvetica path (and the byte-identical test
  fixture) must be untouched when no font is requested.

## Decision

Font use is **opt-in**. `RenderOptions` carries an `Arc<Font>` that defaults to
the built-in Helvetica. A caller selects a font via
`RenderOptions::with_font(FontSource)`, where `FontSource` is either a file path
or a system family name. The CLI exposes this as `--font <path|family>`.

- **Parsing/metrics: `ttf-parser`.** At load we parse the face once and
  precompute: per-character advances for the WinAnsi range (used for both layout
  measurement and the PDF `/Widths`), `units_per_em`, and the `FontDescriptor`
  metrics (ascent/descent/cap-height/bbox/italic-angle/flags), all scaled to
  PDF's 1000-unit em. The raw bytes are kept for embedding. No `ttf_parser::Face`
  is held past load, so the font is `Send` and cheap to share across render
  threads via `Arc`.
- **Family resolution: `fontdb`.** A family name is resolved against the system
  font database to raw face bytes + index.
- **Embedding: simple TrueType.** The PDF emits `/Subtype /TrueType` with
  `/Encoding /WinAnsiEncoding`, a `/Widths` array, a `/FontDescriptor`, and the
  font program as a compressed `/FontFile2` (with `/Length1`). Text is written as
  WinAnsi/ASCII bytes (unchanged from the Helvetica path), so it stays
  selectable and extractable.
- **Measurement uses the active font.** Layout's width/wrap/truncate helpers take
  the active `&Font`, so line breaking and column fitting match the embedded
  font's real glyph widths.

## Consequences

- Non-Helvetica fonts now render and embed; text remains selectable
  (verified with `pdffonts` → `emb yes` and `pdftotext`).
- Default output (no font) is byte-identical; all existing tests pass.
- Per-render font load: a file path is cheap (read + parse); a *family name*
  calls `fontdb::load_system_fonts()` per load, which is expensive. For
  high-volume rendering, build `RenderOptions` once and clone it per document —
  the `Arc<Font>` is shared, so the font is parsed only once.

## Known limitations / follow-ups

- **No subsetting yet** — the full font program is embedded in every PDF
  (`pdffonts` shows `sub no`). Subsetting to used glyphs is the next step to cut
  output size.
- **WinAnsi/Latin only** — text is written as WinAnsi bytes; non-Latin
  (e.g. CJK) needs a `Type0`/CID composite font with `Identity-H` and a
  `ToUnicode` CMap. That's the path to full Unicode + shaping later.
- **No `ToUnicode` CMap** — extraction currently relies on WinAnsi encoding,
  which works for Latin; a `ToUnicode` map would make extraction robust for the
  remapped 0x80–0x9F range.
