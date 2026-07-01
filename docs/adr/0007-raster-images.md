# ADR 0007: Raster Images (`<img>`) via PDF Image XObjects

## Status

Accepted (2026-07-01). First pass: block-level images only. No effect on
documents without `<img>` (the fixture stays byte-identical).

## Context

The engine could render text, tables, and box decorations but had no way to put a
raster image on the page — a large gap for real documents (logos, charts,
photos). Images need three things the pipeline lacked: loading/decoding image
bytes, a place in the box tree and layout, and PDF image XObject output.

A guiding project constraint is a **small dependency surface** (no browser, no
async runtime). Pulling in a full image-codec stack (e.g. the `image` crate and
its many transitive deps) would undercut that. So the format strategy is chosen
to minimize decoding dependencies.

## Decision

Support block-level `<img>` for **JPEG** and **PNG**, from `data:` URIs and file
paths, embedded as PDF image XObjects.

- **JPEG**: embed the file **verbatim** through PDF's `DCTDecode` filter — the PDF
  reader decodes it. We only scan the marker stream for the `SOFn` frame to read
  dimensions and component count (1 → `DeviceGray`, 3 → `DeviceRGB`). No pixel
  decoder, no re-encode.
- **PNG**: decode **in-house** with no new dependency — chunk parsing, `flate2`
  inflate of the `IDAT` stream (the crate already used to *compress* PDF
  streams), scanline unfiltering (None/Sub/Up/Average/Paeth), and color-type
  expansion (grayscale, RGB, palette, gray+alpha, RGBA; 8/16-bit). The alpha
  channel becomes a separate 8-bit `/SMask` image; palette `tRNS` becomes a
  per-pixel mask. Samples embed through `/FlateDecode`.
- **Loading**: `image.rs` resolves a `data:` URI (a tiny in-house base64 decoder)
  or a file path (relative paths resolve against `RenderOptions.base_dir`, which
  the CLI sets to the input file's directory). A broken/unsupported image is
  skipped, never fatal.
- **Pipeline**: `box_tree::BoxChild::Image(ImageBox)` carries the source and
  `width`/`height` hints; a post-parse `html::resolve_images` pass loads and
  measures each image, populating `Document.images` and each box's point size and
  table index. Layout scales the image to fit the content box, page-breaks it as
  a unit, and emits `PaintCommand::Image`. `pdf.rs` writes an image (and optional
  soft-mask) XObject per image, lists them in every page's `/Resources /XObject`,
  and paints with `q  w 0 0 h x y cm  /ImN Do  Q`.
- **Sizing**: `width`/`height` attributes (CSS px → pt at the 96 dpi reference,
  1px = 0.75pt) with aspect-ratio preservation when only one is given; otherwise
  the intrinsic pixel size.

## Consequences

- Real images render with no image-codec dependency: JPEG passes through, and PNG
  reuses `flate2`. Verified with `pdfimages` (a file-path PNG round-trips to an
  RGB XObject whose extracted pixels match the source exactly).
- Only **block-level** images so far: no inline/text-flowed or floated images, no
  `object-fit`, no CSS `width`/`height` (only the HTML attributes), and no remote
  (`http`) URLs. Interlaced PNGs and sub-byte bit depths are unsupported.
- The style cache and the table path are untouched; documents without `<img>`
  produce identical output.

## Alternatives considered

- **Use the `image` crate.** Rejected for the dependency weight; `DCTDecode`
  pass-through plus a focused PNG decoder covers the common cases with only the
  `flate2` we already ship.
- **Decode JPEG ourselves.** Unnecessary and heavy — PDF natively supports
  `DCTDecode`, so the reader does it.
- **Rasterize everything to a single backdrop.** Rejected: it would break text
  selectability and inflate output; images belong as first-class XObjects.
