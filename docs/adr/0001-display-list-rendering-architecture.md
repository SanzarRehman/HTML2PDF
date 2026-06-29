# ADR 0001: Display-List Rendering Architecture

## Status

Accepted.

## Context

The project goal is not a spreadsheet-only HTML-to-PDF tool. The spreadsheet
fixture is a useful benchmark, but the engine must eventually support complex
and dynamic HTML with normal browser-style rendering concepts: CSS cascade,
layout, painting order, borders, backgrounds, clipping, transforms, images,
SVG/canvas, and JavaScript-driven DOM mutation.

Optimizing directly from table layout to PDF operators is a trap. It can make
one fixture faster while making the architecture too narrow for real HTML.

Browser engines generally separate these phases:

```text
HTML/CSS/JS
  -> DOM and CSSOM
  -> style resolution
  -> layout tree
  -> paint/display list
  -> backend renderer
```

Servo is relevant because it is a Rust browser engine with a modular,
embeddable, memory-safe, parallel architecture. Its public project description
explicitly focuses on embeddability, memory safety, modularity, and parallelism.
Chromium/WebRender-style architectures also separate layout/paint from the
final graphics backend.

References:

- Servo: https://servo.org/
- CSS Paged Media: https://www.w3.org/TR/css-page-3/
- CSS Fragmentation: https://www.w3.org/TR/css-break-3/
- Cairo PDF surfaces: https://www.cairographics.org/manual/cairo-PDF-Surfaces.html
- PDF imaging model overview: https://en.wikipedia.org/wiki/PDF#Imaging_model

## Decision

Use a display-list architecture.

Layout code must not write PDF syntax directly. Layout produces generic paint
commands. The PDF backend consumes those commands.

Initial paint commands:

- `Text`
- `StrokeRect`
- `FillRect`
- `StrokeLine`

Future paint commands:

- `Path`
- `Image`
- `Clip`
- `Transform`
- `Save`
- `Restore`
- `SetFillColor`
- `SetStrokeColor`
- `SetLineWidth`
- `DrawSvg`
- `CanvasBitmap`

The current spreadsheet-table layout is only one producer of display-list
commands. It is not the engine architecture.

## Consequences

Benefits:

- The PDF backend can support strokes, paths, images, clipping, transforms, and
  compression without changing layout algorithms.
- Complex HTML can be added incrementally through layout and paint features.
- Alternative backends become possible later: PDF, PNG snapshots, debug JSON, or
  visual diff output.
- Table-specific optimizations can be measured without becoming architectural
  constraints.

Costs:

- Slightly more internal structure now.
- Some current data is duplicated temporarily because tests still inspect layout
  lines/rectangles while PDF output consumes paint commands.
- We need a real paint-order model later, including stacking contexts and
  z-index.

## Rejected Approach

Replacing table cell borders with shared line strokes was tested and rejected
for the current PDF writer.

Results on `reg-2-9-1.html`:

- Naive shared line drawing: about 456 ms 5-run average, about 2.7 MB output.
- Hash-based shared line drawing: about 270 ms 5-run average, about 2.7 MB output.
- Current rectangle drawing: about 218 ms 5-run average, about 1.8 MB output.

Reason: without PDF stream compression or path batching, separate line commands
are larger and slower for this fixture. The result does not mean strokes are
unnecessary. It means stroke/path support belongs behind the display-list/PDF
backend layer, not as a narrow table-border rewrite.

## Next Steps

1. Add PDF stream compression.
2. Add path batching to the PDF backend.
3. Add paint-order tests.
4. Introduce a real CSS cascade module.
5. Introduce font metrics and font embedding.
6. Add visual comparison against Chromium for selected fixtures.
