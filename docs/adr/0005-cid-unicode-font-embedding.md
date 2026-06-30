# ADR 0005: CID/Unicode Font Embedding (Type0/Identity-H)

## Status

Accepted (2026-06-30). Supersedes the simple `/TrueType` + `/WinAnsiEncoding`
embedding introduced in ADR 0003. The default standard-14 Helvetica path (no
`--font`) is unchanged.

## Context

ADR 0003 embedded a supplied font as a simple `/TrueType` font with
`/WinAnsiEncoding`: a single-byte encoding limited to the ~217 WinAnsi/CP1252
characters. Anything outside that range (CJK, Cyrillic, Greek, Arabic,
Devanagari, most symbols) could not be addressed and rendered as `?`, even when
the embedded font contained the glyphs. That is a hard ceiling for "full CSS"
fidelity: real documents are multilingual.

## Decision

Embed supplied fonts as a **PDF Type0 (composite) font** with `Identity-H`
encoding:

- The `/F1` font object becomes `/Subtype /Type0 /Encoding /Identity-H` with a
  single `DescendantFonts` entry and a `/ToUnicode` CMap.
- The descendant is a `/CIDFontType2` with `/CIDSystemInfo (Adobe) (Identity) 0`,
  `/CIDToGIDMap /Identity` (so CID == glyph id), `/DW 1000`, and a `/W` array of
  per-glyph advance widths.
- The `FontDescriptor` + compressed `/FontFile2` program are unchanged.
- Page content writes text as **2-byte glyph ids** in a hex string (`<....> Tj`)
  rather than a WinAnsi literal.
- A `/ToUnicode` CMap maps each used glyph id back to its Unicode scalar
  (UTF-16BE, surrogate pairs for astral code points) so the text stays
  selectable, copyable, and searchable.

The glyph ids, widths, and glyph→Unicode mapping are resolved at PDF-write time
for **exactly the characters the document uses** (`font::cid_layout`), by
re-parsing the already-validated face once per render.

Text *measurement* during layout still uses the cached WinAnsi-range advances
(`font::advance_em`); glyphs outside that range fall back to a default advance.
This keeps layout fast and exact for Latin text in a brand font; non-Latin
spacing is approximate (a follow-up), but the glyphs render with their true
widths from `/W`.

## Consequences

- Any Unicode text renders with an embedded font, and stays extractable via
  ToUnicode. Verified with `pdffonts` (CID TrueType / Identity-H / emb yes / uni
  yes) and `pdftotext` round-trip on Latin (including curly quotes and an em
  dash) and CJK.
- The default Helvetica path is untouched and still WinAnsi, so the ASCII table
  fixture stays byte-identical (492,740 bytes).
- The embed path adds two PDF objects versus ADR 0003 (a descendant CIDFont and
  the ToUnicode CMap).

## Follow-up (done): glyph subsetting

Retain-GIDs subsetting (`subset.rs`) now rebuilds the `glyf`/`loca` tables to
embed only the glyphs the document uses (plus `.notdef` and composite
components), copying every other table verbatim and recomputing the directory,
checksums, and `head` checksum adjustment. Glyph ids are unchanged, so the `/W`,
`/ToUnicode`, and `/CIDToGIDMap /Identity` here remain valid; the subset font
gets an `ABCDEF+` name tag. Measured: Arial 477 KB → 123 KB, an STHeiti CJK doc
33.3 MB → 0.65 MB.

## Not yet (honest limitations)

- Subsetting covers `glyf`-based TrueType only; CFF/OpenType-CFF fonts (no `glyf`)
  fall back to embedding the full program.
- Non-Latin text *measurement* uses a fallback advance, so line breaking/wrapping
  of non-WinAnsi text is approximate even though the glyph widths in the PDF are
  correct.
