use crate::color::Color;
use crate::html::{
    BlockKind, Document, Overflow, OverflowWrap, PageOrientation, TableCell, TextAlign,
    VerticalAlign, WhiteSpace, WordBreak,
};
use crate::paint::{
    DashPattern, ImageCommand, LineCommand, PaintCommand, RectCommand, RoundedRectCommand,
    TextCommand,
};

#[derive(Debug, Clone, Copy)]
pub struct PageSize {
    pub width: f32,
    pub height: f32,
}

impl PageSize {
    pub const A4: Self = Self {
        width: 595.0,
        height: 842.0,
    };

    pub const A4_LANDSCAPE: Self = Self {
        width: 842.0,
        height: 595.0,
    };

    pub const LETTER: Self = Self {
        width: 612.0,
        height: 792.0,
    };

    pub const LETTER_LANDSCAPE: Self = Self {
        width: 792.0,
        height: 612.0,
    };
}

/// The base paper size to render on (before applying the document's orientation).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Paper {
    #[default]
    A4,
    Letter,
}

impl Paper {
    fn portrait(self) -> PageSize {
        match self {
            Paper::A4 => PageSize::A4,
            Paper::Letter => PageSize::LETTER,
        }
    }

    fn landscape(self) -> PageSize {
        match self {
            Paper::A4 => PageSize::A4_LANDSCAPE,
            Paper::Letter => PageSize::LETTER_LANDSCAPE,
        }
    }
}

#[derive(Debug, Clone)]
pub struct RenderOptions {
    pub page_size: PageSize,
    pub margin: f32,
    pub margin_top: f32,
    pub margin_right: f32,
    pub margin_bottom: f32,
    pub margin_left: f32,
    pub table_row_height: f32,
    /// The font used for measurement and (if a TrueType) embedding. Defaults to
    /// the built-in Helvetica (not embedded).
    pub font: std::sync::Arc<crate::font::Font>,
    /// Base directory for resolving relative `<img src>` file paths. `None`
    /// disables file-path images (`data:` URIs still work).
    pub base_dir: Option<std::path::PathBuf>,
    /// Base paper size; the document's orientation picks portrait/landscape.
    pub paper: Paper,
    /// Faces resolved from the document's interned font specs — indexed by
    /// `InlineRun::font` / `TableCell::font`. Empty (index 0 → `font`) when
    /// the document never selects a family; filled per render by
    /// [`RenderOptions::with_document_hints`].
    pub fonts: Vec<crate::font::ResolvedFont>,
    /// The document's interned link targets (`<a href>` values), indexed by
    /// `LinkArea::link - 1`; filled per render by `with_document_hints`.
    pub links: Vec<String>,
    /// Whether/how remote `http(s)` `<img>` URLs may be fetched. Fail-closed by
    /// default (disabled); see [`crate::image::RemoteImagePolicy`].
    pub remote_images: crate::image::RemoteImagePolicy,
}

impl Default for RenderOptions {
    fn default() -> Self {
        Self {
            page_size: PageSize::A4,
            margin: 48.0,
            margin_top: 48.0,
            margin_right: 48.0,
            margin_bottom: 48.0,
            margin_left: 48.0,
            // No fixed row-height floor: rows are sized from their content
            // (line box + padding), like a browser. A CSS-declared row height
            // (e.g. Excel exports) overrides this via `with_document_hints`.
            table_row_height: 0.0,
            font: std::sync::Arc::new(crate::font::Font::helvetica()),
            base_dir: None,
            paper: Paper::A4,
            fonts: Vec::new(),
            links: Vec::new(),
            remote_images: crate::image::RemoteImagePolicy::default(),
        }
    }
}

impl RenderOptions {
    /// Load and use `source` as the document font (measured and embedded).
    pub fn with_font(mut self, source: &crate::font::FontSource) -> Result<Self, String> {
        self.font = std::sync::Arc::new(crate::font::Font::load(source)?);
        Ok(self)
    }

    /// Set the base directory used to resolve relative `<img src>` file paths
    /// (typically the input HTML file's directory).
    pub fn with_base_dir(mut self, base_dir: impl Into<std::path::PathBuf>) -> Self {
        self.base_dir = Some(base_dir.into());
        self
    }

    /// Enable (or configure) remote `http(s)` image fetching. Off by default;
    /// passing a policy with `enabled = true` opts in. Actual network access
    /// additionally requires the crate's `remote-images` feature — without it,
    /// enabling has no effect (stays fail-closed).
    pub fn with_remote_images(mut self, policy: crate::image::RemoteImagePolicy) -> Self {
        self.remote_images = policy;
        self
    }

    /// Choose the base paper size (A4 or Letter).
    pub fn with_paper(mut self, paper: Paper) -> Self {
        self.paper = paper;
        self.page_size = paper.portrait();
        self
    }

    pub fn with_document_hints(&self, document: &Document) -> Self {
        let mut options = self.clone();

        options.page_size = if document.page_style.orientation == PageOrientation::Landscape {
            options.paper.landscape()
        } else {
            options.paper.portrait()
        };

        options.margin_top = document.page_style.margin_top.unwrap_or(options.margin);
        options.margin_right = document.page_style.margin_right.unwrap_or(options.margin);
        options.margin_bottom = document.page_style.margin_bottom.unwrap_or(options.margin);
        options.margin_left = document.page_style.margin_left.unwrap_or(options.margin);
        options.table_row_height = document
            .table_style
            .row_height
            .unwrap_or(options.table_row_height);

        // Load any `@font-face` fonts first — an author-declared family shadows
        // a same-named system family. `url()` sources resolve like image srcs
        // (data: / file via base_dir / policy-gated remote); a rule whose
        // sources all fail falls back to system lookup.
        let web_fonts = crate::font::load_font_faces(
            &document.font_faces,
            self.base_dir.as_deref(),
            &self.remote_images,
        );
        // Resolve the document's interned font specs to concrete faces (spec 0
        // is the default and always resolves to the primary font unchanged).
        options.fonts = document
            .font_specs
            .iter()
            .map(|spec| crate::font::resolve_spec_with(&options.font, spec, &web_fonts))
            .collect();
        options.links = document.links.clone();

        options
    }

    /// The face for an interned font-spec index (0 or out-of-range = primary).
    pub(crate) fn run_font(&self, index: u16) -> &std::sync::Arc<crate::font::Font> {
        self.fonts
            .get(index as usize)
            .map(|resolved| &resolved.font)
            .unwrap_or(&self.font)
    }

    /// Whether a run with this font index still needs synthesized bold.
    /// `run_bold` is the cascaded bold flag — the fallback answer when the
    /// document carries no resolved font table (tests, hand-built box trees).
    pub(crate) fn run_faux_bold(&self, index: u16, run_bold: bool) -> bool {
        self.fonts
            .get(index as usize)
            .map(|resolved| resolved.faux_bold)
            .unwrap_or(run_bold)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Page {
    pub lines: Vec<Line>,
    pub rects: Vec<Rect>,
    pub commands: Vec<PaintCommand>,
    /// Clickable regions from `<a href>` text, turned into `/Annots` links.
    pub link_areas: Vec<LinkArea>,
    /// Destinations recorded on this page: headings (for the PDF outline) and
    /// elements with an HTML `id` (for `#fragment` links).
    pub anchors: Vec<AnchorMark>,
}

/// The clickable rectangle of one laid-out link piece, in PDF page space.
/// `link` is a 1-based index into the document's interned targets
/// (`RenderOptions::links`).
#[derive(Debug, Clone, PartialEq)]
pub struct LinkArea {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    pub link: u16,
}

/// A named or heading destination: where a block landed on its page. `level`
/// is 1–6 for `<h1>`–`<h6>` (outline entries), 0 for a plain `id` anchor.
#[derive(Debug, Clone, PartialEq)]
pub struct AnchorMark {
    pub name: Option<String>,
    pub level: u8,
    pub title: String,
    pub y: f32,
}

impl Page {
    pub fn new() -> Self {
        Self {
            lines: Vec::new(),
            rects: Vec::new(),
            commands: Vec::new(),
            link_areas: Vec::new(),
            anchors: Vec::new(),
        }
    }

    pub(crate) fn push_colored_line(&mut self, line: Line, color: Color, bold: bool) {
        self.commands.push(PaintCommand::SetFillColor(color));
        self.commands.push(PaintCommand::Text(TextCommand {
            text: line.text.clone(),
            x: line.x,
            y: line.y,
            font_size: line.font_size,
            font: line.font,
            bold,
        }));
        self.lines.push(line);
    }

    /// Stroke `text-decoration` lines for a text run drawn with its baseline at
    /// `(x, y)` and horizontal extent `width`. Underlines sit just below the
    /// baseline; line-through crosses near the x-height midline.
    pub(crate) fn push_text_decoration(
        &mut self,
        x: f32,
        y: f32,
        width: f32,
        font_size: f32,
        color: Color,
        underline: bool,
        line_through: bool,
    ) {
        if (!underline && !line_through) || width <= 0.0 {
            return;
        }
        self.commands.push(PaintCommand::SetStrokeColor(color));
        self.commands
            .push(PaintCommand::SetLineWidth((font_size * 0.06).max(0.4)));
        if underline {
            let uy = y - font_size * 0.12;
            self.commands.push(PaintCommand::StrokeLine(LineCommand {
                x1: x,
                y1: uy,
                x2: x + width,
                y2: uy,
            }));
        }
        if line_through {
            let ly = y + font_size * 0.28;
            self.commands.push(PaintCommand::StrokeLine(LineCommand {
                x1: x,
                y1: ly,
                x2: x + width,
                y2: ly,
            }));
        }
    }

    pub(crate) fn push_rect(&mut self, rect: Rect) {
        if rect.stroke {
            self.commands
                .push(PaintCommand::SetStrokeColor(Color::BLACK));
            self.commands.push(PaintCommand::StrokeRect(RectCommand {
                x: rect.x,
                y: rect.y,
                width: rect.width,
                height: rect.height,
            }));
        } else {
            self.commands.push(PaintCommand::SetFillColor(Color::BLACK));
            self.commands.push(PaintCommand::FillRect(RectCommand {
                x: rect.x,
                y: rect.y,
                width: rect.width,
                height: rect.height,
            }));
        }
        self.rects.push(rect);
    }

    pub(crate) fn push_colored_fill_rect(&mut self, rect: Rect, color: Color) {
        self.commands.push(PaintCommand::SetFillColor(color));
        self.commands.push(PaintCommand::FillRect(RectCommand {
            x: rect.x,
            y: rect.y,
            width: rect.width,
            height: rect.height,
        }));
        self.rects.push(rect);
    }

    pub(crate) fn push_clip_rect(&mut self, rect: Rect) {
        self.commands.push(PaintCommand::PushClipRect(RectCommand {
            x: rect.x,
            y: rect.y,
            width: rect.width,
            height: rect.height,
        }));
    }

    pub(crate) fn pop_clip(&mut self) {
        self.commands.push(PaintCommand::PopClip);
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Line {
    pub text: String,
    pub x: f32,
    pub y: f32,
    pub font_size: f32,
    /// Interned font-spec index (see `Document::font_specs`; 0 = default).
    pub font: u16,
    pub leading: f32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    pub stroke: bool,
}

pub fn layout_document(document: &Document, options: &RenderOptions) -> Vec<Page> {
    if let Some(flow) = &document.flow {
        return layout_flow(flow, options);
    }

    let mut pages = vec![Page::new()];
    let mut y = options.page_size.height - options.margin_top;
    let content_width = options.page_size.width - options.margin_left - options.margin_right;
    let table_geometry = table_geometry(document, content_width, &options.font);
    let mut repeated_table_header: Option<Vec<TableCell>> = None;

    for block in &document.blocks {
        if repeated_table_header.is_none() && block.kind == BlockKind::TableHeaderRow {
            repeated_table_header = Some(block.cells.clone());
        }
        let header = if block.kind == BlockKind::TableHeaderRow {
            None
        } else {
            repeated_table_header.as_deref()
        };

        layout_table_row(
            block.cells.as_slice(),
            &table_geometry,
            &mut pages,
            &mut y,
            options,
            header,
        );
    }

    pages
}

/// A positioned (absolute/fixed) box captured out of flow. Its paint commands
/// are appended *after* the page's in-flow content — positioned boxes paint
/// above normal flow, as in CSS — ordered by `z` (stable, so equal z keeps
/// encounter order).
struct Overlay {
    z: i32,
    /// Page the overlay belongs to; `None` = every page (`position: fixed`).
    page: Option<usize>,
    commands: Vec<PaintCommand>,
    lines: Vec<Line>,
    rects: Vec<Rect>,
    links: Vec<LinkArea>,
}

/// The containing block established by the nearest positioned ancestor, for
/// resolving an absolute descendant's `left`/`right`/`top` offsets. `None`
/// means the page content box. (`bottom` always resolves against the page:
/// the ancestor's height isn't known until its own layout finishes.)
#[derive(Clone, Copy)]
struct ContainingBlock {
    x: f32,
    top: f32,
    width: f32,
}

/// Merge captured overlays into the finished pages, sorted by `z-index`.
/// Page-bound overlays land on their page; `fixed` overlays are stamped onto
/// every page (headers/footers/watermarks).
///
/// Paint order follows CSS: overlays with a **negative** `z-index` go *below*
/// the in-flow content (prepended, most-negative deepest — the classic
/// `z-index: -1` decorative background), everything else paints above it in
/// ascending z order.
fn apply_overlays(pages: &mut [Page], mut overlays: Vec<Overlay>) {
    overlays.sort_by_key(|overlay| overlay.z);

    // Collect the below-flow layer per page first, so several negative
    // overlays keep their ascending z order after the single prepend.
    let mut under: Vec<Vec<PaintCommand>> = vec![Vec::new(); pages.len()];
    for overlay in &overlays {
        if overlay.z >= 0 {
            continue;
        }
        match overlay.page {
            Some(index) => under[index.min(pages.len() - 1)].extend(overlay.commands.iter().cloned()),
            None => {
                for buffer in &mut under {
                    buffer.extend(overlay.commands.iter().cloned());
                }
            }
        }
    }
    for (page, buffer) in pages.iter_mut().zip(under) {
        if !buffer.is_empty() {
            page.commands.splice(0..0, buffer);
        }
    }

    for overlay in overlays {
        match overlay.page {
            Some(index) => {
                let page = &mut pages[index.min(pages.len() - 1)];
                if overlay.z >= 0 {
                    page.commands.extend(overlay.commands);
                }
                page.lines.extend(overlay.lines);
                page.rects.extend(overlay.rects);
                page.link_areas.extend(overlay.links);
            }
            None => {
                for page in pages.iter_mut() {
                    if overlay.z >= 0 {
                        page.commands.extend(overlay.commands.iter().cloned());
                    }
                    page.lines.extend(overlay.lines.iter().cloned());
                    page.rects.extend(overlay.rects.iter().cloned());
                    page.link_areas.extend(overlay.links.iter().cloned());
                }
            }
        }
    }
}

/// Lay out the flow box tree by walking it recursively. Each block establishes a
/// containing block (an x offset and a width); its inline content is wrapped to
/// that width, and nested blocks indent by their margin/padding. The root
/// contributes no spacing of its own.
///
/// Vertical margins collapse: a "carried" margin is threaded through the walk and
/// adjacent margins (sibling-to-sibling and parent-to-child) collapse to their
/// maximum, flushed only when real content or a padding edge intervenes.
fn layout_flow(flow: &crate::box_tree::FlowRoot, options: &RenderOptions) -> Vec<Page> {
    let mut pages = vec![Page::new()];
    let mut y = options.page_size.height - options.margin_top;
    let content_width = options.page_size.width - options.margin_left - options.margin_right;
    let mut carried = 0.0;
    let mut floats: Vec<FloatBand> = Vec::new();
    let mut overlays: Vec<Overlay> = Vec::new();

    layout_box_children(
        &flow.children,
        options.margin_left,
        content_width,
        TextAlign::Left,
        None,
        false,
        &mut pages,
        &mut y,
        &mut carried,
        &mut floats,
        &mut overlays,
        None,
        options,
    );

    // Positioned boxes paint above the flow, in z-index order; `fixed` ones
    // repeat on every page.
    apply_overlays(&mut pages, overlays);

    pages
}

/// Drop the carried (collapsed) margin into the page as vertical space.
fn flush_margin(y: &mut f32, carried: &mut f32) {
    *y -= *carried;
    *carried = 0.0;
}

#[allow(clippy::too_many_arguments)]
#[allow(clippy::too_many_arguments)]
fn layout_box_children(
    children: &[crate::box_tree::BoxChild],
    x: f32,
    width: f32,
    align: TextAlign,
    line_height: Option<crate::html::LineHeight>,
    base_rtl: bool,
    pages: &mut Vec<Page>,
    y: &mut f32,
    carried: &mut f32,
    floats: &mut Vec<FloatBand>,
    overlays: &mut Vec<Overlay>,
    containing: Option<ContainingBlock>,
    options: &RenderOptions,
) {
    use crate::box_tree::BoxChild;

    for child in children {
        match child {
            BoxChild::Block(block)
                if matches!(
                    block.position,
                    Some(crate::html::PositionKind::Absolute)
                        | Some(crate::html::PositionKind::Fixed)
                ) =>
            {
                // Out of flow: does not move the cursor and does not take part
                // in margin collapsing. The static fallback position is the
                // current cursor (with any pending margin applied visually).
                // The box is laid out into a scratch page and captured as an
                // overlay, painted above the flow after layout completes.
                let fixed = block.position == Some(crate::html::PositionKind::Fixed);
                let mut scratch = vec![Page::new()];
                let mut nested: Vec<Overlay> = Vec::new();
                layout_absolute_box(
                    block,
                    x,
                    *y - *carried,
                    &mut scratch,
                    &mut nested,
                    // `fixed` positions against the page even inside a
                    // positioned ancestor (its containing block is the
                    // viewport in CSS terms).
                    if fixed { None } else { containing },
                    options,
                );
                let mut captured = scratch.swap_remove(0);
                // Nested positioned descendants paint above their ancestor in
                // z order; nested `fixed` boxes keep their per-page repeat.
                nested.sort_by_key(|overlay| overlay.z);
                let mut under: Vec<PaintCommand> = Vec::new();
                for inner in nested {
                    if inner.page.is_none() {
                        overlays.push(inner);
                    } else {
                        // Within this positioned box's own stacking context: a
                        // negative-z descendant paints below the box's content.
                        if inner.z < 0 {
                            under.extend(inner.commands);
                        } else {
                            captured.commands.extend(inner.commands);
                        }
                        captured.lines.extend(inner.lines);
                        captured.rects.extend(inner.rects);
                        captured.link_areas.extend(inner.links);
                    }
                }
                if !under.is_empty() {
                    captured.commands.splice(0..0, under);
                }
                overlays.push(Overlay {
                    z: block.z_index.unwrap_or(0),
                    page: if fixed { None } else { Some(pages.len() - 1) },
                    commands: captured.commands,
                    lines: captured.lines,
                    rects: captured.rects,
                    links: captured.link_areas,
                });
            }
            BoxChild::Block(block) if block.float_dir.is_some() => {
                // A floated block leaves normal flow: content above must be
                // flushed, but `y` does not advance past it.
                flush_margin(y, carried);
                layout_float(block, x, width, pages, y, floats, overlays, containing, options);
            }
            BoxChild::Block(block)
                if block.position == Some(crate::html::PositionKind::Relative) =>
            {
                // Visual offset only: lay out at the shifted position, then put
                // the flow cursor back where an unshifted box would have left it.
                // `%` offsets resolve against the containing block: width for
                // left/right; top/bottom `%` need a definite CB height (absent
                // in flow) and are ignored here.
                let off_left = block
                    .offset_left
                    .or(block.offset_percent.left.map(|p| p / 100.0 * width));
                let off_right = block
                    .offset_right
                    .or(block.offset_percent.right.map(|p| p / 100.0 * width));
                let dx = off_left.or(off_right.map(|r| -r)).unwrap_or(0.0);
                let dy = block
                    .offset_top
                    .or(block.offset_bottom.map(|b| -b))
                    .unwrap_or(0.0);
                let start = *y;
                let mut shifted = start - dy;
                layout_block_box(
                    block, x + dx, width, pages, &mut shifted, carried, floats, overlays,
                    containing, options,
                );
                *y = shifted + dy;
            }
            BoxChild::Block(block) => layout_block_box(
                block, x, width, pages, y, carried, floats, overlays, containing, options,
            ),
            BoxChild::Line(runs) => {
                // Content flushes any pending margin above it.
                flush_margin(y, carried);
                layout_line_box(runs, x, width, align, line_height, base_rtl, pages, y, floats, options);
            }
            BoxChild::Image(image) if image.float_dir.is_some() => {
                flush_margin(y, carried);
                layout_float_image(image, x, width, pages, y, floats, options);
            }
            BoxChild::Image(image) => {
                flush_margin(y, carried);
                layout_image_box(image, x, width, pages, y, options);
            }
            BoxChild::Table(table) => {
                layout_table_box(table, width, pages, y, carried, options);
            }
        }
    }
}

/// Place a floated block: shrink-to-fit width (or the CSS `width`), positioned
/// at the left or right edge beside any floats already active, painted at the
/// current `y` without advancing it, and registered as a [`FloatBand`] so the
/// following line boxes shorten around it. A float never splits across pages.
fn layout_float(
    block: &crate::box_tree::BlockBox,
    x: f32,
    width: f32,
    pages: &mut Vec<Page>,
    y: &mut f32,
    floats: &mut Vec<FloatBand>,
    overlays: &mut Vec<Overlay>,
    containing: Option<ContainingBlock>,
    options: &RenderOptions,
) {
    let is_left = block.float_dir == Some(crate::html::FloatDir::Left);

    // Shrink-to-fit: CSS width (points or percent) if declared, else
    // max-content + own edges, clamped to the containing width. With
    // `border-box` sizing the declared width already includes the padding.
    let float_width = block
        .css_width
        .or(block.css_width_percent.map(|pct| pct / 100.0 * width))
        .map(|w| {
            if block.border_box {
                w.max(block.padding.left + block.padding.right)
            } else {
                w + block.padding.left + block.padding.right
            }
        })
        .unwrap_or_else(|| {
            measure_max_content(&block.children, options)
                + block.padding.left
                + block.padding.right
                + block.margin.left
                + block.margin.right
        })
        .clamp(1.0, width);

    let item = FlexItem::Block(block);
    let height = item.measure_height(float_width, options);
    if !has_space(*y, options, height) {
        push_page(pages, y, options);
        floats.clear();
    }

    let (band_x, band_width) = float_band_at(floats, *y, x, width);
    let float_x = if is_left {
        band_x
    } else {
        band_x + band_width - float_width
    };

    let top = *y;
    let mut float_y = top;
    let mut carried = 0.0;
    let mut inner_floats: Vec<FloatBand> = Vec::new();
    layout_block_box(
        block,
        float_x,
        float_width,
        pages,
        &mut float_y,
        &mut carried,
        &mut inner_floats,
        overlays,
        containing,
        options,
    );

    floats.push(FloatBand {
        left: is_left,
        x0: float_x,
        x1: float_x + float_width,
        top,
        bottom: top - height,
    });
}

/// Place an absolutely positioned block against the page content box: CSS
/// `left`/`right`/`top`/`bottom` offsets pick the edges (falling back to the
/// in-flow cursor position), width is the CSS width or shrink-to-fit, and the
/// flow cursor is not advanced. `left`/`right`/`top` resolve against the
/// nearest positioned ancestor's containing block when there is one (else the
/// page content box); `bottom` always resolves against the page, since the
/// ancestor's height isn't known mid-layout. The caller captures the scratch
/// page this renders into as an [`Overlay`], so the box paints above the flow
/// in z-index order; content past the page bottom is dropped (absolute boxes
/// do not paginate).
fn layout_absolute_box(
    block: &crate::box_tree::BlockBox,
    static_x: f32,
    cursor_y: f32,
    pages: &mut Vec<Page>,
    overlays: &mut Vec<Overlay>,
    containing: Option<ContainingBlock>,
    options: &RenderOptions,
) {
    let page_content_width = options.page_size.width - options.margin_left - options.margin_right;
    let page_top = options.page_size.height - options.margin_top;
    let page_bottom = options.margin_bottom;
    let (content_x, content_width, top_edge) = match containing {
        Some(cb) => (cb.x, cb.width.max(1.0), cb.top),
        None => (options.margin_left, page_content_width, page_top),
    };

    let width = block
        .css_width
        .or(block.css_width_percent.map(|pct| pct / 100.0 * content_width))
        .map(|w| {
            if block.border_box {
                w.max(block.padding.left + block.padding.right)
            } else {
                w + block.padding.left + block.padding.right
            }
        })
        .unwrap_or_else(|| {
            measure_max_content(&block.children, options)
                + block.padding.left
                + block.padding.right
                + block.margin.left
                + block.margin.right
        })
        .clamp(1.0, content_width);
    let item = FlexItem::Block(block);
    let height = item.measure_height(width, options);

    // `%` offsets resolve against the containing block: width for left/right;
    // height for top/bottom. The positioned-ancestor CB height isn't tracked,
    // so top/bottom `%` are only resolved when the CB is the page.
    let pct_w = |p: f32| p / 100.0 * content_width;
    let cb_height = containing.is_none().then_some(top_edge - page_bottom);
    let off_left = block.offset_left.or(block.offset_percent.left.map(pct_w));
    let off_right = block.offset_right.or(block.offset_percent.right.map(pct_w));
    let off_top = block
        .offset_top
        .or(block.offset_percent.top.zip(cb_height).map(|(p, h)| p / 100.0 * h));
    let off_bottom = block
        .offset_bottom
        .or(block.offset_percent.bottom.zip(cb_height).map(|(p, h)| p / 100.0 * h));

    let x = if let Some(left) = off_left {
        content_x + left
    } else if let Some(right) = off_right {
        content_x + content_width - right - width
    } else {
        static_x
    };
    let top = if let Some(offset) = off_top {
        top_edge - offset
    } else if let Some(offset) = off_bottom {
        page_bottom + offset + height
    } else {
        cursor_y
    };

    let mut box_y = top;
    let mut carried = 0.0;
    let mut inner_floats: Vec<FloatBand> = Vec::new();
    layout_block_box(
        block,
        x,
        width,
        pages,
        &mut box_y,
        &mut carried,
        &mut inner_floats,
        overlays,
        containing,
        options,
    );
}

/// Place a floated image at the flow edge and register its exclusion band.
fn layout_float_image(
    image: &crate::box_tree::ImageBox,
    x: f32,
    width: f32,
    pages: &mut Vec<Page>,
    y: &mut f32,
    floats: &mut Vec<FloatBand>,
    options: &RenderOptions,
) {
    let Some(image_index) = image.image_index else {
        return;
    };
    if image.width <= 0.0 || image.height <= 0.0 {
        return;
    }
    let mut scale = match image.css_width_percent {
        Some(pct) => (pct / 100.0 * width) / image.width,
        None => (width / image.width).min(1.0),
    };
    if let Some(max_w) = image
        .max_width
        .or(image.max_width_percent.map(|pct| pct / 100.0 * width))
    {
        scale = scale.min(max_w / image.width);
    }
    let (w, h) = (image.width * scale, image.height * scale);

    if !has_space(*y, options, h) {
        push_page(pages, y, options);
        floats.clear();
    }
    let (band_x, band_width) = float_band_at(floats, *y, x, width);
    let is_left = image.float_dir == Some(crate::html::FloatDir::Left);
    let ix = if is_left { band_x } else { band_x + band_width - w };

    let page = pages.last_mut().expect("at least one page exists");
    page.commands.push(PaintCommand::Image(ImageCommand {
        image_index,
        x: ix,
        y: *y - h,
        width: w,
        height: h,
    }));

    floats.push(FloatBand {
        left: is_left,
        x0: ix,
        x1: ix + w,
        top: *y,
        bottom: *y - h,
    });
}

/// Lay out a flow-embedded table in document order, sharing the page/`y` cursor
/// with the surrounding flow content. Geometry is resolved from the table's own
/// rows and declared columns; header rows repeat across page breaks. Cells are
/// painted at the left margin (like the spreadsheet path), so this does not yet
/// honor a left indent from an enclosing block.
fn layout_table_box(
    table: &crate::box_tree::TableBox,
    width: f32,
    pages: &mut Vec<Page>,
    y: &mut f32,
    carried: &mut f32,
    options: &RenderOptions,
) {
    // The table's top margin collapses with the margin carried from above; its
    // bottom margin is left as the new carried value. Both give surrounding flow
    // text room to clear the table's edges (a table row has no line leading of
    // its own, unlike a paragraph).
    *carried = carried.max(TABLE_FLOW_MARGIN);
    flush_margin(y, carried);

    let rows: Vec<&[TableCell]> = table.rows.iter().map(|row| row.cells.as_slice()).collect();
    let geometry = table_geometry_cells(&rows, &table.columns, width, &options.font);

    // The row-height floor comes from the table's own CSS row height.
    let mut opts = options.clone();
    opts.table_row_height = table.row_height.unwrap_or(0.0);

    let mut repeated_header: Option<Vec<TableCell>> = None;
    for row in &table.rows {
        if repeated_header.is_none() && row.kind == BlockKind::TableHeaderRow {
            repeated_header = Some(row.cells.clone());
        }
        let header = if row.kind == BlockKind::TableHeaderRow {
            None
        } else {
            repeated_header.as_deref()
        };
        layout_table_row(&row.cells, &geometry, pages, y, &opts, header);
    }

    *carried = TABLE_FLOW_MARGIN;
}

/// First-pass flexbox (row): lay out a flex container's block children as
/// horizontal flex items across `inner_width`, sharing one top edge. Item main
/// sizes come from `flex-basis` (or declared width, or content max-content),
/// then `flex-grow` distributes free space and a uniform shrink absorbs overflow.
/// `justify-content` distributes any leftover space; `gap` separates items. Items
/// are top-aligned (cross-axis alignment beyond the top is not yet applied), and
/// a flex row is assumed to fit on the current page.
///
/// `flex-direction: column` and non-block (inline/text) flex items fall back to
/// normal vertical block layout.
fn layout_flex_box(
    block: &crate::box_tree::BlockBox,
    flex: &crate::box_tree::FlexContainer,
    inner_x: f32,
    inner_width: f32,
    pages: &mut Vec<Page>,
    y: &mut f32,
    overlays: &mut Vec<Overlay>,
    containing: Option<ContainingBlock>,
    options: &RenderOptions,
) {
    use crate::box_tree::BoxChild;
    use crate::html::FlexDirection;

    // Flex items: block children are items; contiguous inline content (a `Line`)
    // becomes an anonymous item. Images/tables inside a flex row are still
    // skipped (rare; documented).
    let items: Vec<FlexItem> = block
        .children
        .iter()
        .filter_map(|child| match child {
            BoxChild::Block(b) => Some(FlexItem::Block(b)),
            BoxChild::Line(runs) => Some(FlexItem::Line(runs)),
            _ => None,
        })
        .collect();
    if items.is_empty() {
        return;
    }

    let gap = flex.gap;

    // Column direction: items stack vertically, separated by `gap`. Main-axis
    // (height) grow/basis and justify-content are not applied in this pass.
    if flex.direction == FlexDirection::Column {
        for (index, item) in items.iter().enumerate() {
            if index > 0 {
                *y -= gap;
            }
            let mut carried = 0.0;
            item.layout(inner_x, inner_width, pages, y, &mut carried, overlays, containing, options);
        }
        return;
    }

    // `flex-wrap: wrap` partitions the items into flex lines greedily by base
    // size; without it every item shares the single line (shrinking to fit).
    let lines: Vec<std::ops::Range<usize>> = if flex.wrap {
        let mut lines = Vec::new();
        let mut start = 0usize;
        let mut used = 0.0f32;
        for (index, item) in items.iter().enumerate() {
            let base = item.basis(options).clamp(0.0, inner_width);
            let lead_gap = if index > start { gap } else { 0.0 };
            if index > start && used + lead_gap + base > inner_width + WRAP_TOLERANCE {
                lines.push(start..index);
                start = index;
                used = base;
            } else {
                used += lead_gap + base;
            }
        }
        lines.push(start..items.len());
        lines
    } else {
        vec![0..items.len()]
    };

    for (line_index, range) in lines.iter().enumerate() {
        if line_index > 0 {
            *y -= gap; // cross-axis gap between flex lines
        }
        layout_flex_line(
            &items[range.clone()],
            flex,
            inner_x,
            inner_width,
            pages,
            y,
            overlays,
            containing,
            options,
        );
    }
}

/// Lay out one flex line (the whole container when not wrapping): distribute
/// the main axis by flex-grow / proportional shrink, apply `justify-content`
/// and `align-items` within the line, and advance `y` past its tallest item.
#[allow(clippy::too_many_arguments)]
fn layout_flex_line(
    items: &[FlexItem],
    flex: &crate::box_tree::FlexContainer,
    inner_x: f32,
    inner_width: f32,
    pages: &mut Vec<Page>,
    y: &mut f32,
    overlays: &mut Vec<Overlay>,
    containing: Option<ContainingBlock>,
    options: &RenderOptions,
) {
    use crate::html::AlignItems;

    let gap = flex.gap;
    let total_gap = gap * (items.len() as f32 - 1.0).max(0.0);
    let avail = (inner_width - total_gap).max(0.0);

    // Base main size per item: flex-basis, else content max-content, clamped to
    // the line's available width.
    let bases: Vec<f32> = items
        .iter()
        .map(|item| item.basis(options).clamp(0.0, avail))
        .collect();

    let total_base: f32 = bases.iter().sum();
    let total_grow: f32 = items.iter().map(FlexItem::grow).sum();
    let free = avail - total_base;

    let widths: Vec<f32> = if free > 0.0 && total_grow > 0.0 {
        // Distribute free space by flex-grow.
        bases
            .iter()
            .zip(items)
            .map(|(base, item)| base + free * (item.grow() / total_grow))
            .collect()
    } else if free < 0.0 && total_base > 0.0 {
        // Overflow: shrink every item proportionally to its base so the line fits.
        let scale = avail / total_base;
        bases.iter().map(|base| base * scale).collect()
    } else {
        bases.clone()
    };

    // justify-content distributes any leftover main-axis space.
    let used: f32 = widths.iter().sum::<f32>() + total_gap;
    let slack = (inner_width - used).max(0.0);
    let n = items.len() as f32;
    let (mut cursor, between) = justify_offsets(flex.justify, slack, gap, n);
    cursor += inner_x;

    // Measure pass: lay each item out into scratch pages to learn its height, so
    // align-items can offset shorter items against the tallest one.
    let heights: Vec<f32> = items
        .iter()
        .zip(&widths)
        .map(|(item, width)| item.measure_height(*width, options))
        .collect();
    let row_height = heights.iter().fold(0.0_f32, |a, &b| a.max(b));

    // Cross-axis alignment factor: how much of the leftover height goes above
    // the item. `stretch` behaves as `flex-start` (items are not inflated).
    let align_factor = match flex.align {
        AlignItems::Stretch | AlignItems::FlexStart => 0.0,
        AlignItems::Center => 0.5,
        AlignItems::FlexEnd => 1.0,
    };

    let top = *y;
    let mut lowest = *y;
    for ((item, width), height) in items.iter().zip(&widths).zip(&heights) {
        let mut item_y = top - (row_height - height) * align_factor;
        let mut carried = 0.0;
        item.layout(cursor, *width, pages, &mut item_y, &mut carried, overlays, containing, options);
        lowest = lowest.min(item_y);
        cursor += width + between;
    }

    *y = lowest.min(top - row_height);
}

/// One flex item: a block child, or an anonymous item wrapping contiguous
/// inline content.
enum FlexItem<'a> {
    Block(&'a crate::box_tree::BlockBox),
    Line(&'a [crate::box_tree::InlineRun]),
}

impl FlexItem<'_> {
    fn grow(&self) -> f32 {
        match self {
            FlexItem::Block(b) => b.flex_grow,
            FlexItem::Line(_) => 0.0,
        }
    }

    /// Base main size: `flex-basis` when declared, else the content's
    /// max-content width plus the item's own horizontal padding and margins
    /// (the outer main size, so padded pills don't collapse to zero content).
    fn basis(&self, options: &RenderOptions) -> f32 {
        match self {
            FlexItem::Block(b) => b.flex_basis.unwrap_or_else(|| {
                measure_max_content(&b.children, options)
                    + b.padding.left
                    + b.padding.right
                    + b.margin.left
                    + b.margin.right
            }),
            FlexItem::Line(runs) => runs
                .iter()
                .map(|run| match run.image.as_deref() {
                    Some(image) => image.width,
                    None => {
                        estimate_text_width(&run.text, run.font_size, options.run_font(run.font))
                    }
                })
                .sum(),
        }
    }

    fn layout(
        &self,
        x: f32,
        width: f32,
        pages: &mut Vec<Page>,
        y: &mut f32,
        carried: &mut f32,
        overlays: &mut Vec<Overlay>,
        containing: Option<ContainingBlock>,
        options: &RenderOptions,
    ) {
        // A flex/grid item establishes its own flow: floats do not escape it.
        let mut floats: Vec<FloatBand> = Vec::new();
        match self {
            FlexItem::Block(b) => layout_block_box(
                b, x, width, pages, y, carried, &mut floats, overlays, containing, options,
            ),
            FlexItem::Line(runs) => {
                layout_line_box(runs, x, width, TextAlign::Left, None, false, pages, y, &mut floats, options)
            }
        }
    }

    /// Dry-run the item into scratch pages to learn the height it will consume
    /// at `width`. Cheap (a flex item is a small subtree) and exact, since it
    /// runs the same layout code as the paint pass. Positioned descendants are
    /// captured into a throwaway list — out-of-flow boxes contribute no height.
    fn measure_height(&self, width: f32, options: &RenderOptions) -> f32 {
        let mut scratch = vec![Page::new()];
        let start = options.page_size.height - options.margin_top;
        let mut item_y = start;
        let mut carried = 0.0;
        let mut overlays: Vec<Overlay> = Vec::new();
        self.layout(
            0.0, width, &mut scratch, &mut item_y, &mut carried, &mut overlays, None, options,
        );
        start - item_y
    }
}

/// First-pass CSS grid: place the container's children into the column tracks
/// of `grid-template-columns`, row-major (auto-placement), honoring
/// `grid-column: span N`, `gap`, `fr` fractions, fixed lengths, and `auto`
/// (content-sized) tracks. Rows are sized to their tallest item (via the same
/// measure pass flex uses) and may break to a new page between rows.
///
/// Not yet handled: line-based placement (`grid-column: 1 / 3`), named
/// lines/areas, `minmax()`, dense packing, and `align`/`justify` of items
/// within their cells (items are top-left in their cell).
fn layout_grid_box(
    block: &crate::box_tree::BlockBox,
    grid: &crate::box_tree::GridContainer,
    inner_x: f32,
    inner_width: f32,
    pages: &mut Vec<Page>,
    y: &mut f32,
    overlays: &mut Vec<Overlay>,
    containing: Option<ContainingBlock>,
    options: &RenderOptions,
) {
    use crate::box_tree::BoxChild;
    use crate::html::GridTrack;

    use crate::html::{MaxTrack, MinTrack};

    // Grid items with their column span and any line-based `grid-column`
    // placement. Anonymous inline content spans 1, auto-placed.
    let items: Vec<(FlexItem, usize, Option<i32>, Option<i32>)> = block
        .children
        .iter()
        .filter_map(|child| match child {
            BoxChild::Block(b) => {
                Some((FlexItem::Block(b), b.grid_span.max(1), b.grid_col_start, b.grid_col_end))
            }
            BoxChild::Line(runs) => Some((FlexItem::Line(runs), 1, None, None)),
            _ => None,
        })
        .collect();
    if items.is_empty() {
        return;
    }

    let tracks: Vec<GridTrack> = if grid.columns.is_empty() {
        vec![GridTrack::Auto]
    } else {
        grid.columns.clone()
    };
    let track_count = tracks.len();

    // A 1-based grid line (negative = from the end) as a 0-based line index,
    // clamped to the explicit grid (`-1` → line `track_count`, the last).
    let resolve_line = |line: i32| -> usize {
        let lines = track_count as i32 + 1;
        let zero_based = if line < 0 { lines + line } else { line - 1 };
        zero_based.clamp(0, track_count as i32) as usize
    };

    // Placement, row-major: auto-placed items take the next `span` tracks,
    // wrapping to a fresh row when they don't fit; an explicit `grid-column`
    // start pins the column (moving to the next row if the cursor is already
    // past it), and an end line overrides the span (`1 / -1` = full row).
    struct Placed {
        item: usize,
        row: usize,
        col: usize,
        span: usize,
    }
    let mut placements = Vec::with_capacity(items.len());
    let (mut row, mut col) = (0usize, 0usize);
    for (index, (_, span, col_start, col_end)) in items.iter().enumerate() {
        match col_start.map(|line| resolve_line(line).min(track_count - 1)) {
            Some(start) => {
                let span = match col_end {
                    Some(end) => resolve_line(*end).saturating_sub(start).max(1),
                    None => *span,
                }
                .min(track_count - start);
                if col > start {
                    row += 1;
                }
                placements.push(Placed { item: index, row, col: start, span });
                col = start + span;
            }
            None => {
                let span = (*span).min(track_count);
                if col + span > track_count {
                    row += 1;
                    col = 0;
                }
                placements.push(Placed { item: index, row, col, span });
                col += span;
            }
        }
        if col >= track_count {
            row += 1;
            col = 0;
        }
    }
    let row_count = placements.iter().map(|p| p.row).max().unwrap_or(0) + 1;

    // Track widths: fixed lengths as declared; `auto` sized to the widest
    // single-span item placed in that track; `fr` shares of what remains.
    let total_column_gap = grid.column_gap * (track_count as f32 - 1.0).max(0.0);
    let avail = (inner_width - total_column_gap).max(0.0);

    let mut auto_size = vec![0.0f32; track_count];
    for placed in &placements {
        let content_sized = matches!(
            tracks[placed.col],
            GridTrack::Auto
                | GridTrack::MinMax(MinTrack::Auto, _)
                | GridTrack::MinMax(_, MaxTrack::Auto)
        );
        if placed.span == 1 && content_sized {
            let basis = items[placed.item].0.basis(options).min(avail);
            auto_size[placed.col] = auto_size[placed.col].max(basis);
        }
    }

    // A minmax() floor: explicit points, or the content size for `auto`.
    let min_of = |track: &GridTrack, index: usize| -> f32 {
        match track {
            GridTrack::MinMax(MinTrack::Pt(min), _) => *min,
            GridTrack::MinMax(MinTrack::Auto, _) => auto_size[index],
            _ => 0.0,
        }
    };
    // Tracks with a definite (non-fr) size; `None` = participates in the fr
    // distribution (plain `fr`, or `minmax(min, fr)` with its floor).
    let fixed: Vec<Option<f32>> = tracks
        .iter()
        .enumerate()
        .map(|(index, track)| match track {
            GridTrack::Pt(width) => Some(*width),
            GridTrack::Auto => Some(auto_size[index]),
            GridTrack::Fr(_) => None,
            GridTrack::MinMax(_, MaxTrack::Pt(max)) => Some(max.max(min_of(track, index))),
            GridTrack::MinMax(_, MaxTrack::Auto) => {
                Some(auto_size[index].max(min_of(track, index)))
            }
            GridTrack::MinMax(_, MaxTrack::Fr(_)) => None,
        })
        .collect();
    let fr_weight = |track: &GridTrack| match track {
        GridTrack::Fr(weight) => *weight,
        GridTrack::MinMax(_, MaxTrack::Fr(weight)) => *weight,
        _ => 0.0,
    };
    let non_fr: f32 = fixed.iter().flatten().sum();

    // Distribute the remainder among fr tracks; a `minmax(min, fr)` track whose
    // floor exceeds its share is pinned at the floor and leaves the pool
    // (iterate until stable, so the rest re-share what's left).
    let mut widths: Vec<f32> = vec![0.0; track_count];
    let mut pinned = vec![false; track_count];
    loop {
        let pool: f32 = (0..track_count)
            .filter(|&index| fixed[index].is_none() && !pinned[index])
            .map(|index| fr_weight(&tracks[index]))
            .sum();
        let pinned_sum: f32 = (0..track_count)
            .filter(|&index| fixed[index].is_none() && pinned[index])
            .map(|index| widths[index])
            .sum();
        let remaining = (avail - non_fr - pinned_sum).max(0.0);
        let mut changed = false;
        for index in 0..track_count {
            if fixed[index].is_some() || pinned[index] {
                continue;
            }
            let share = if pool > 0.0 {
                remaining * fr_weight(&tracks[index]) / pool
            } else {
                0.0
            };
            let floor = min_of(&tracks[index], index);
            if floor > share + 0.01 {
                widths[index] = floor;
                pinned[index] = true;
                changed = true;
            } else {
                widths[index] = share;
            }
        }
        if !changed {
            break;
        }
    }
    for (index, width) in fixed.iter().enumerate() {
        if let Some(width) = width {
            widths[index] = *width;
        }
    }

    // Over-wide fixed/auto tracks: shrink everything proportionally to fit.
    let used: f32 = widths.iter().sum();
    if used > avail && used > 0.0 {
        let scale = avail / used;
        for width in &mut widths {
            *width *= scale;
        }
    }

    // Column left edges.
    let mut lefts = Vec::with_capacity(track_count);
    let mut cursor = inner_x;
    for width in &widths {
        lefts.push(cursor);
        cursor += width + grid.column_gap;
    }
    // Width of a cell spanning `span` tracks from `col` (includes crossed gaps).
    let cell_width = |col: usize, span: usize| -> f32 {
        widths[col..col + span].iter().sum::<f32>()
            + grid.column_gap * (span as f32 - 1.0).max(0.0)
    };

    // Lay rows out top-down; each row is as tall as its tallest item and may
    // move to a fresh page as a unit (a single row is not split).
    for row in 0..row_count {
        let row_placements: Vec<&Placed> =
            placements.iter().filter(|p| p.row == row).collect();
        if row_placements.is_empty() {
            continue;
        }

        let row_height = row_placements
            .iter()
            .map(|p| {
                items[p.item]
                    .0
                    .measure_height(cell_width(p.col, p.span), options)
            })
            .fold(0.0f32, f32::max);

        if !has_space(*y, options, row_height) {
            push_page(pages, y, options);
        }

        let top = *y;
        for placed in row_placements {
            let mut item_y = top;
            let mut carried = 0.0;
            items[placed.item].0.layout(
                lefts[placed.col],
                cell_width(placed.col, placed.span),
                pages,
                &mut item_y,
                &mut carried,
                overlays,
                containing,
                options,
            );
        }
        *y = top - row_height;
        if row + 1 < row_count {
            *y -= grid.row_gap;
        }
    }
}

/// Return `(leading offset, gap between items)` for a `justify-content` value,
/// given the leftover `slack`, the base `gap`, and item count `n`.
fn justify_offsets(
    justify: crate::html::JustifyContent,
    slack: f32,
    gap: f32,
    n: f32,
) -> (f32, f32) {
    use crate::html::JustifyContent::*;
    match justify {
        FlexStart => (0.0, gap),
        FlexEnd => (slack, gap),
        Center => (slack / 2.0, gap),
        SpaceBetween if n > 1.0 => (0.0, gap + slack / (n - 1.0)),
        SpaceBetween => (0.0, gap),
        SpaceAround => (slack / n / 2.0, gap + slack / n),
        SpaceEvenly => (slack / (n + 1.0), gap + slack / (n + 1.0)),
    }
}

/// Approximate the max-content main size of a block's flow children: the widest
/// natural (unwrapped) line, plus its own horizontal padding. Nested blocks
/// recurse. Used as the default `flex-basis` when none is declared.
fn measure_max_content(children: &[crate::box_tree::BoxChild], options: &RenderOptions) -> f32 {
    use crate::box_tree::BoxChild;
    let mut widest = 0.0_f32;
    for child in children {
        let w = match child {
            BoxChild::Line(runs) => runs
                .iter()
                .map(|run| match run.image.as_deref() {
                    Some(image) => image.width,
                    None => {
                        estimate_text_width(&run.text, run.font_size, options.run_font(run.font))
                    }
                })
                .sum(),
            BoxChild::Block(b) => {
                measure_max_content(&b.children, options)
                    + b.padding.left
                    + b.padding.right
                    + b.margin.left
                    + b.margin.right
            }
            BoxChild::Image(img) => img.width,
            BoxChild::Table(_) => 0.0,
        };
        widest = widest.max(w);
    }
    widest
}

/// Place a resolved block-level image: scale it to fit the content box if
/// necessary, page-break if it does not fit the remaining space, then emit an
/// image paint command with its lower-left corner at the current pen position.
fn layout_image_box(
    image: &crate::box_tree::ImageBox,
    x: f32,
    width: f32,
    pages: &mut Vec<Page>,
    y: &mut f32,
    options: &RenderOptions,
) {
    let Some(image_index) = image.image_index else {
        return; // unresolved / failed to load: nothing to paint
    };
    if image.width <= 0.0 || image.height <= 0.0 {
        return;
    }

    // A percentage width sizes the image against the containing block (this
    // may scale *up*); `max-width` (points or percent) clamps it. Otherwise
    // scale down to the content width — and to a full page's height — if
    // oversized, preserving the aspect ratio.
    let page_height = options.page_size.height - options.margin_top - options.margin_bottom;
    let mut scale = match image.css_width_percent {
        Some(pct) => (pct / 100.0 * width) / image.width,
        None => 1.0_f32,
    };
    let max_w = image
        .max_width
        .or(image.max_width_percent.map(|pct| pct / 100.0 * width));
    if let Some(max_w) = max_w {
        scale = scale.min(max_w / image.width);
    }
    if image.width * scale > width {
        scale = scale.min(width / image.width);
    }
    if image.height * scale > page_height {
        scale = scale.min(page_height / image.height);
    }
    let draw_width = image.width * scale;
    let draw_height = image.height * scale;

    // Move to a fresh page if the image does not fit the remaining space.
    ensure_space(pages, y, options, draw_height);

    let page = pages.last_mut().expect("at least one page");
    page.commands.push(PaintCommand::Image(ImageCommand {
        image_index,
        x,
        y: *y - draw_height,
        width: draw_width,
        height: draw_height,
    }));
    *y -= draw_height;
}

/// Fold `%` edges (of the containing block's width) into the point edges. A
/// side is set in at most one of the two; the point side already carries any
/// folded border width, so the sum is the used edge.
fn resolve_edges(
    base: &crate::box_tree::Edges,
    pct: &crate::box_tree::EdgesPercent,
    cb_width: f32,
) -> crate::box_tree::Edges {
    let add = |b: f32, p: Option<f32>| b + p.map_or(0.0, |p| p / 100.0 * cb_width);
    crate::box_tree::Edges {
        top: add(base.top, pct.top),
        right: add(base.right, pct.right),
        bottom: add(base.bottom, pct.bottom),
        left: add(base.left, pct.left),
    }
}

#[allow(clippy::too_many_arguments)]
fn layout_block_box(
    block: &crate::box_tree::BlockBox,
    x: f32,
    width: f32,
    pages: &mut Vec<Page>,
    y: &mut f32,
    carried: &mut f32,
    floats: &mut Vec<FloatBand>,
    overlays: &mut Vec<Overlay>,
    containing: Option<ContainingBlock>,
    options: &RenderOptions,
) {
    // Resolve `%` padding/margin against the containing block's width (the
    // incoming `width`, before any own-width narrowing below). The point sides
    // already include folded border widths; a `%` side had a 0 point value, so
    // adding the resolved percentage reconstructs the used edge.
    let cb_width = width;
    let padding = resolve_edges(&block.padding, &block.padding_percent, cb_width);
    let margin = resolve_edges(&block.margin, &block.margin_percent, cb_width);

    // An explicit CSS `width` (content-box; points or a percentage of the
    // containing block) narrows the block to `width + padding + margins`,
    // clamped to the containing width; `min-width`/`max-width` clamp further
    // (min wins over max). With `margin: auto` on both sides, the leftover
    // space centers the box. With `box-sizing: border-box`, the declared width
    // already covers padding and borders, so only the margins add on top.
    let css_extra = if block.border_box {
        margin.left + margin.right
    } else {
        padding.left + padding.right + margin.left + margin.right
    };
    let css_width = block
        .css_width
        .or(block.css_width_percent.map(|pct| pct / 100.0 * width));
    let max_outer = block
        .max_width
        .or(block.max_width_percent.map(|pct| pct / 100.0 * width))
        .map(|max| max + css_extra);
    let min_outer = block
        .min_width
        .or(block.min_width_percent.map(|pct| pct / 100.0 * width))
        .map(|min| min + css_extra);
    let outer = match css_width {
        Some(css) => (css + css_extra).min(width),
        None => width,
    };
    let outer = match max_outer {
        Some(max) => outer.min(max),
        None => outer,
    };
    // `min-width` wins over `max-width`; clamp to the containing width so the
    // box never draws off the page.
    let outer = match min_outer {
        Some(min) => outer.max(min).min(width),
        None => outer,
    };
    let x = if block.center && outer < width {
        x + (width - outer) / 2.0
    } else {
        x
    };
    let width = outer;

    // `clear` drops the block below the matching active floats first.
    if let Some(clear) = block.clear {
        let below = below_next_float(floats, *y - *carried, Some(clear));
        if below < *y - *carried {
            flush_margin(y, carried);
            *y = below;
        }
    }

    // This block's top margin collapses with the margin carried from above.
    *carried = carried.max(margin.top);

    let decorated = block.border.is_some() || block.background.is_some();
    let inner_x = x + margin.left + padding.left;
    let inner_width =
        (width - margin.left - margin.right - padding.left - padding.right).max(1.0);

    // Top padding — or any border/background — is a barrier: it ends the collapse
    // and separates the block's margin from its first child's margin.
    if padding.top > 0.0 || decorated {
        flush_margin(y, carried);
        *y -= padding.top;
    }

    // Record the border box's top edge so its background/border can be painted
    // per page fragment once the content height is known.
    let start_page = pages.len() - 1;
    let start_index = pages[start_page].commands.len();
    let start_y = *y + padding.top;
    let box_x = x + margin.left;
    let box_width = (width - margin.left - margin.right).max(1.0);

    // A positioned block (relative/absolute/fixed) establishes the containing
    // block that its absolutely-positioned descendants resolve offsets against.
    let child_containing = if block.position.is_some() {
        Some(ContainingBlock {
            x: inner_x,
            top: *y - *carried,
            width: inner_width,
        })
    } else {
        containing
    };

    if let Some(flex) = &block.flex {
        // A flex container lays out its block children along the main axis
        // instead of stacking them. Content above must be flushed first.
        flush_margin(y, carried);
        layout_flex_box(
            block, flex, inner_x, inner_width, pages, y, overlays, child_containing, options,
        );
    } else if let Some(grid) = &block.grid {
        // A grid container places its children into column tracks, row-major.
        flush_margin(y, carried);
        layout_grid_box(
            block, grid, inner_x, inner_width, pages, y, overlays, child_containing, options,
        );
    } else {
        layout_box_children(
            &block.children,
            inner_x,
            inner_width,
            block.align,
            block.line_height,
            block.rtl,
            pages,
            y,
            carried,
            floats,
            overlays,
            child_containing,
            options,
        );
    }

    // Bottom padding (or a border/background) likewise contains the last child's
    // margin rather than letting it collapse out of the box.
    if padding.bottom > 0.0 || decorated {
        flush_margin(y, carried);
        *y -= padding.bottom;
    }

    // Definite box height. `height`/`min-height` extend the border box down to
    // at least that height when the content is shorter; taller content
    // overflows visibly (`overflow: visible`) — unless `overflow: hidden`,
    // where `height`/`max-height` also cap the box and its content is clipped.
    // No pagination of the extension: a fragment never grows past its page.
    let pad_v = padding.top + padding.bottom;
    // border-box: the declared height already includes padding+borders (folded
    // into `padding`); content-box adds them.
    let to_outer = |h: f32| {
        if block.border_box {
            h.max(pad_v)
        } else {
            h + pad_v
        }
    };
    let min_h = [block.css_height, block.min_height]
        .into_iter()
        .flatten()
        .fold(None, |acc: Option<f32>, h| Some(acc.map_or(h, |a| a.max(h))));
    if let Some(height) = min_h {
        flush_margin(y, carried);
        let target = start_y - to_outer(height);
        if target < *y && pages.len() - 1 == start_page {
            *y = target;
        }
    }
    // `overflow: hidden` + a definite height caps the box on its start page and
    // clips content to the border box. Multi-page clipped boxes are not handled
    // (the clip would need to re-establish per page), so this only fires when
    // the content stayed on the start page.
    if block.overflow_hidden {
        if let Some(cap) = block.max_height.or(block.css_height) {
            flush_margin(y, carried);
            let target = start_y - to_outer(cap);
            if pages.len() - 1 == start_page && *y < target {
                *y = target;
                let clip = RectCommand {
                    x: box_x,
                    y: target,
                    width: box_width,
                    height: to_outer(cap),
                };
                pages[start_page]
                    .commands
                    .insert(start_index, PaintCommand::PushClipRect(clip));
                pages[start_page].commands.push(PaintCommand::PopClip);
            }
        }
    }

    if decorated {
        let end_page = pages.len() - 1;
        let end_y = *y;
        paint_decorations(
            pages, options, block, start_page, start_index, start_y, end_page, end_y, box_x,
            box_width,
        );
    }

    // Record this block as a destination: headings feed the PDF outline, an
    // HTML `id` serves `#fragment` links. If the block's first content moved to
    // a fresh page (nothing painted where it started), anchor there instead.
    let heading_level = heading_level(block.kind);
    if heading_level > 0 || block.anchor.is_some() {
        let (anchor_page, anchor_y) =
            if pages.len() - 1 > start_page && pages[start_page].commands.len() == start_index {
                (start_page + 1, options.page_size.height - options.margin_top)
            } else {
                (start_page, start_y)
            };
        let title = if heading_level > 0 { first_line_text(&block.children) } else { String::new() };
        pages[anchor_page].anchors.push(AnchorMark {
            name: block.anchor.clone(),
            level: heading_level,
            title,
            y: anchor_y,
        });
    }

    // This block's bottom margin collapses with whatever is carried out of it.
    *carried = carried.max(block.margin.bottom);
}

/// The outline level of a block: 1–6 for headings, 0 otherwise.
fn heading_level(kind: crate::html::BlockKind) -> u8 {
    match kind {
        crate::html::BlockKind::Heading1 => 1,
        crate::html::BlockKind::Heading2 => 2,
        crate::html::BlockKind::Heading3 => 3,
        crate::html::BlockKind::Heading4 => 4,
        crate::html::BlockKind::Heading5 => 5,
        crate::html::BlockKind::Heading6 => 6,
        _ => 0,
    }
}

/// A block's leading inline text (its first line box), whitespace-collapsed
/// and capped — used as the outline title for headings.
fn first_line_text(children: &[crate::box_tree::BoxChild]) -> String {
    for child in children {
        if let crate::box_tree::BoxChild::Line(runs) = child {
            let joined: String = runs.iter().map(|run| run.text.as_str()).collect();
            let mut title = String::new();
            for word in joined.split_whitespace() {
                if !title.is_empty() {
                    title.push(' ');
                }
                title.push_str(word);
                if title.len() > 200 {
                    break;
                }
            }
            if !title.is_empty() {
                return title;
            }
        }
    }
    String::new()
}

/// Paint a decorated block's background and border, one rectangle per page the
/// block spans. Each rectangle is inserted *before* the content already emitted
/// on that page (at the recorded command index for the start page, at the front
/// for continuation pages), so decorations paint behind text and nested boxes
/// stack correctly (ancestors behind descendants).
#[allow(clippy::too_many_arguments)]
fn paint_decorations(
    pages: &mut [Page],
    options: &RenderOptions,
    block: &crate::box_tree::BlockBox,
    start_page: usize,
    start_index: usize,
    start_y: f32,
    end_page: usize,
    end_y: f32,
    x: f32,
    width: f32,
) {
    let page_top = options.page_size.height - options.margin_top;
    let page_bottom = options.margin_bottom;

    for page_index in start_page..=end_page {
        let top = if page_index == start_page { start_y } else { page_top };
        let bottom = if page_index == end_page { end_y } else { page_bottom };
        let height = top - bottom;
        if height <= 0.0 {
            continue;
        }

        let mut commands = Vec::new();
        let radius = block.border_radius.min(width / 2.0).min(height / 2.0).max(0.0);
        if let Some(color) = block.background {
            commands.push(PaintCommand::SetFillColor(color));
            if radius > 0.0 {
                commands.push(PaintCommand::FillRoundedRect(RoundedRectCommand {
                    x,
                    y: bottom,
                    width,
                    height,
                    radius,
                }));
            } else {
                commands.push(PaintCommand::FillRect(RectCommand {
                    x,
                    y: bottom,
                    width,
                    height,
                }));
            }
        }
        if let Some(edges) = &block.border {
            paint_border(
                &mut commands,
                edges,
                radius,
                x,
                bottom,
                width,
                height,
                page_index == start_page,
                page_index == end_page,
            );
        }

        let at = if page_index == start_page { start_index } else { 0 };
        pages[page_index].commands.splice(at..at, commands);
    }
}

/// Stroke a block's border along its border-box rectangle. A uniform border
/// (all four sides equal) strokes one rectangle — rounded when a radius is
/// set; mixed sides stroke individual edge segments (radius is ignored for
/// them). On a fragmented block, the top edge paints only on the first page
/// fragment and the bottom edge only on the last (CSS `box-decoration-break:
/// slice`); left/right repeat on every fragment. Dashed/dotted styles emit a
/// dash pattern and always reset it, so later strokes stay solid.
#[allow(clippy::too_many_arguments)]
fn paint_border(
    commands: &mut Vec<PaintCommand>,
    edges: &crate::html::BorderEdges,
    radius: f32,
    x: f32,
    bottom: f32,
    width: f32,
    height: f32,
    first_fragment: bool,
    last_fragment: bool,
) {
    let dash_for = border_dash;

    if let Some(side) = edges.uniform() {
        commands.push(PaintCommand::SetStrokeColor(side.color));
        commands.push(PaintCommand::SetLineWidth(side.width));
        let dash = dash_for(side);
        if let Some(pattern) = dash {
            commands.push(PaintCommand::SetDash(Some(pattern)));
        }
        let rect = RectCommand { x, y: bottom, width, height };
        if radius > 0.0 {
            commands.push(PaintCommand::StrokeRoundedRect(RoundedRectCommand {
                x,
                y: bottom,
                width,
                height,
                radius,
            }));
        } else {
            commands.push(PaintCommand::StrokeRect(rect));
        }
        if dash.is_some() {
            commands.push(PaintCommand::SetDash(None));
        }
        return;
    }

    let top_y = bottom + height;
    let sides = [
        (edges.top, first_fragment, (x, top_y, x + width, top_y)),
        (edges.bottom, last_fragment, (x, bottom, x + width, bottom)),
        (edges.left, true, (x, bottom, x, top_y)),
        (edges.right, true, (x + width, bottom, x + width, top_y)),
    ];
    for (side, active, (x1, y1, x2, y2)) in sides {
        let Some(side) = side else { continue };
        if !active {
            continue;
        }
        commands.push(PaintCommand::SetStrokeColor(side.color));
        commands.push(PaintCommand::SetLineWidth(side.width));
        let dash = dash_for(side);
        if let Some(pattern) = dash {
            commands.push(PaintCommand::SetDash(Some(pattern)));
        }
        commands.push(PaintCommand::StrokeLine(LineCommand { x1, y1, x2, y2 }));
        if dash.is_some() {
            commands.push(PaintCommand::SetDash(None));
        }
    }
}

/// The dash pattern for a border side's style: dashed = 3×width on/off,
/// dotted = width on/off (square dots). Solid strokes return `None`.
fn border_dash(side: crate::html::BorderSide) -> Option<DashPattern> {
    match side.style {
        crate::html::BorderStyle::Dashed => Some(DashPattern {
            on: side.width * 3.0,
            off: side.width * 3.0,
        }),
        crate::html::BorderStyle::Dotted => Some(DashPattern {
            on: side.width,
            off: side.width,
        }),
        _ => None,
    }
}

/// An active float exclusion band: a floated box occupying the vertical range
/// `[bottom, top]` (page coordinates, y downward-decreasing) at the left or
/// right edge of the flow, `width` points wide. Line boxes that overlap the
/// band shorten to the space beside it.
pub(crate) struct FloatBand {
    left: bool,
    /// The horizontal interval the float actually occupies (page coordinates),
    /// so stacked floats (side by side) exclude the right span at any `y`.
    x0: f32,
    x1: f32,
    top: f32,
    bottom: f32,
}

/// The horizontal segment available for a line starting at `y` inside the
/// content box `[x, x + width]`, after subtracting active float bands: text
/// sits right of every active left float and left of every active right float.
fn float_band_at(floats: &[FloatBand], y: f32, x: f32, width: f32) -> (f32, f32) {
    let mut left_edge = x;
    let mut right_edge = x + width;
    for band in floats {
        if band.top >= y - 0.5 && band.bottom < y - 0.5 {
            if band.left {
                left_edge = left_edge.max(band.x1);
            } else {
                right_edge = right_edge.min(band.x0);
            }
        }
    }
    (left_edge, (right_edge - left_edge).max(1.0))
}

/// The y just below the nearest active float bottom at `y` (for dropping a line
/// or a cleared block past floats). Returns `y` unchanged if none are active.
fn below_next_float(floats: &[FloatBand], y: f32, side: Option<crate::html::Clear>) -> f32 {
    use crate::html::Clear;
    let mut target = y;
    for band in floats {
        let matches = match side {
            None | Some(Clear::Both) => true,
            Some(Clear::Left) => band.left,
            Some(Clear::Right) => !band.left,
        };
        if matches && band.top >= y - 0.5 && band.bottom < y - 0.5 {
            target = target.min(band.bottom);
        }
    }
    target
}

/// Wrap one line box's runs and paint each visual line, honoring the per-run
/// font size and color and the block's text alignment. Lines are built one at a
/// time so each can shorten around the float bands active at its own `y`.
fn layout_line_box(
    runs: &[crate::box_tree::InlineRun],
    x: f32,
    width: f32,
    align: TextAlign,
    line_height: Option<crate::html::LineHeight>,
    base_rtl: bool,
    pages: &mut Vec<Page>,
    y: &mut f32,
    floats: &mut Vec<FloatBand>,
    options: &RenderOptions,
) {
    let mut breaker = LineBreaker::new(runs);
    while !breaker.is_done() {
        let (band_x, band_width) = float_band_at(floats, *y, x, width);

        // A word that does not fit beside a float but would fit at full width
        // drops below the float instead of being broken mid-word.
        if band_width + WRAP_TOLERANCE < width {
            if let Some(token_width) = breaker.peek_token_width(options) {
                if token_width > band_width + WRAP_TOLERANCE
                    && token_width <= width + WRAP_TOLERANCE
                {
                    let below = below_next_float(floats, *y, None);
                    if below < *y {
                        *y = below;
                        continue;
                    }
                }
            }
        }

        let visual = breaker.next_line(band_width, options);
        if visual.is_empty() {
            break;
        }
        // UAX #9: put the line's pieces in visual order when it mixes
        // directions (each piece's own glyph order is handled by shaping).
        let visual = reorder_pieces_bidi(visual, base_rtl);

        let line_width: f32 = visual.iter().map(|piece| piece.advance(options)).sum();
        // Leading follows the tallest run on the line; an explicit `line-height`
        // overrides the UA default (×1.35). An inline image grows the line box
        // when it rises higher above the baseline than the tallest text ascent
        // (the image's bottom sits on the baseline, like a browser's default
        // `vertical-align: baseline`).
        let max_font = visual
            .iter()
            .map(|piece| piece.font_size)
            .fold(0.0_f32, f32::max);
        let max_image = visual
            .iter()
            .filter_map(|piece| piece.image.map(|(_, _, height)| height))
            .fold(0.0_f32, f32::max);
        let text_ascent = max_font * 0.8;
        let image_rise = (max_image - text_ascent).max(0.0);
        let leading = resolve_leading(line_height, max_font, FLOW_LEADING_FACTOR) + image_rise;
        // When line-height exceeds the default line box, distribute the extra as
        // half-leading (glyphs sit mid-line, as browsers do). When it's smaller
        // (or unset) the baseline stays where the default box puts it.
        let half_leading =
            ((leading - image_rise - max_font * FLOW_LEADING_FACTOR) / 2.0).max(0.0);

        // A page break retires the previous page's floats.
        if !has_space(*y, options, leading) {
            push_page(pages, y, options);
            floats.clear();
        }

        // Justification stretches the spaces of every line except the
        // paragraph's last (and lines with no spaces to stretch).
        let justify_bonus = if align == TextAlign::Justify && !breaker.is_done() {
            let spaces = visual.iter().filter(|piece| piece.text == " ").count();
            if spaces > 0 {
                ((band_width - line_width) / spaces as f32).max(0.0)
            } else {
                0.0
            }
        } else {
            0.0
        };

        let mut px = match align {
            TextAlign::Left | TextAlign::Justify => band_x,
            TextAlign::Center => band_x + ((band_width - line_width) / 2.0).max(0.0),
            TextAlign::Right => band_x + (band_width - line_width).max(0.0),
        };

        // Drop the baseline below the line's top edge by the tallest run's
        // ascent (~0.8 em) — or by the tallest inline image, whichever rises
        // higher — so ascenders stay inside the line box instead of
        // overlapping the border/padding of the box above.
        let baseline = *y - half_leading - text_ascent.max(max_image);

        let page = pages.last_mut().expect("at least one page exists");
        for piece in &visual {
            // An inline image paints with its bottom on the baseline and
            // advances by its width; a linked image is clickable over its box.
            if let Some((image_index, image_w, image_h)) = piece.image {
                page.commands.push(PaintCommand::Image(ImageCommand {
                    image_index,
                    x: px,
                    y: baseline,
                    width: image_w,
                    height: image_h,
                }));
                if piece.link != 0 && image_w > 0.0 {
                    page.link_areas.push(LinkArea {
                        x: px,
                        y: baseline,
                        width: image_w,
                        height: image_h,
                        link: piece.link,
                    });
                }
                px += image_w;
                continue;
            }
            let mut piece_width =
                estimate_text_width(&piece.text, piece.font_size, options.run_font(piece.font));
            if piece.text == " " {
                piece_width += justify_bonus;
            }
            page.push_colored_line(
                Line {
                    text: piece.text.clone(),
                    x: px,
                    y: baseline,
                    font_size: piece.font_size,
                    font: piece.font,
                    leading,
                },
                piece.color,
                // Synthesize bold only when the resolved face isn't truly bold.
                options.run_faux_bold(piece.font, piece.bold),
            );
            page.push_text_decoration(
                px,
                baseline,
                piece_width,
                piece.font_size,
                piece.color,
                piece.underline,
                piece.line_through,
            );
            if piece.link != 0 && piece_width > 0.0 {
                // Clickable rect around the glyphs: descent below the baseline
                // to roughly the cap height above it. Abutting pieces of the
                // same link (words and the spaces between them) merge into one
                // rectangle per line.
                let area = LinkArea {
                    x: px,
                    y: baseline - piece.font_size * 0.25,
                    width: piece_width,
                    height: piece.font_size * 1.1,
                    link: piece.link,
                };
                match page.link_areas.last_mut() {
                    Some(last)
                        if last.link == area.link
                            && last.y == area.y
                            && (last.x + last.width - area.x).abs() < 0.05 =>
                    {
                        last.width += area.width;
                    }
                    _ => page.link_areas.push(area),
                }
            }
            px += piece_width;
        }

        *y -= leading;
    }
}

/// A piece of a wrapped visual line: text in one style, positioned left-to-right.
struct LinePiece {
    text: String,
    font_size: f32,
    /// Interned font-spec index (see `Document::font_specs`).
    font: u16,
    color: Color,
    bold: bool,
    underline: bool,
    line_through: bool,
    /// Interned link target (see `Document::links`; 0 = not a link).
    link: u16,
    /// An inline image: `(image_index, width, height)` in points. The piece
    /// occupies `width` on the line and sits with its bottom on the baseline.
    image: Option<(usize, f32, f32)>,
}

impl LinePiece {
    /// The horizontal space this piece occupies: its image width, or the
    /// measured text width.
    fn advance(&self, options: &RenderOptions) -> f32 {
        match self.image {
            Some((_, width, _)) => width,
            None => estimate_text_width(&self.text, self.font_size, options.run_font(self.font)),
        }
    }
}

/// Reorder one visual line's pieces per UAX #9 so mixed LTR/RTL text reads
/// correctly: embedding levels are resolved against the paragraph's base
/// direction (`base_rtl` → base level 1, from `dir="rtl"`/`direction: rtl`),
/// and the pieces inside each right-to-left run are emitted in reverse order.
/// Glyph order *within* a piece is the shaper's job (rustybuzz emits RTL
/// segments visually), so only whole pieces move here; a piece is assigned to
/// the run containing its first byte (word tokens rarely straddle a direction
/// change). A purely-LTR line with an LTR base returns unchanged.
fn reorder_pieces_bidi(pieces: Vec<LinePiece>, base_rtl: bool) -> Vec<LinePiece> {
    // With an LTR base, a line free of RTL characters never reorders. With an
    // RTL base the same is true (a run of LTR words stays in logical order —
    // only its alignment differs), so the fast path applies to both.
    if !pieces
        .iter()
        .any(|piece| crate::font::contains_rtl(&piece.text))
    {
        return pieces;
    }

    let base = if base_rtl {
        unicode_bidi::Level::rtl()
    } else {
        unicode_bidi::Level::ltr()
    };
    let line_text: String = pieces.iter().map(|piece| piece.text.as_str()).collect();
    let bidi = unicode_bidi::BidiInfo::new(&line_text, Some(base));
    let paragraph = &bidi.paragraphs[0];
    let (levels, runs) = bidi.visual_runs(paragraph, paragraph.range.clone());

    // Byte offset of each piece's start within the concatenated line text.
    let mut starts = Vec::with_capacity(pieces.len());
    let mut offset = 0;
    for piece in &pieces {
        starts.push(offset);
        offset += piece.text.len();
    }

    let mut slots: Vec<Option<LinePiece>> = pieces.into_iter().map(Some).collect();
    let mut out = Vec::with_capacity(slots.len());
    for range in runs {
        let mut members: Vec<usize> = (0..slots.len())
            .filter(|&i| slots[i].is_some() && range.contains(&starts[i]))
            .collect();
        if levels[range.start].is_rtl() {
            members.reverse();
        }
        for index in members {
            out.push(slots[index].take().expect("member selected once"));
        }
    }
    // Anything not claimed by a run (defensive: shouldn't happen) keeps its
    // logical position at the end.
    out.extend(slots.into_iter().flatten());
    out
}

/// Line-at-a-time greedy breaker over the tokenized inline runs. Each call to
/// [`next_line`](Self::next_line) may use a different maximum width, which is
/// what lets lines shorten around float bands. Whitespace collapses across run
/// boundaries; a word wider than the whole line is broken character-by-character
/// as a last resort (a pragmatic deviation from CSS `overflow-wrap: normal` —
/// losing content off the page edge is worse for paged output).
struct LineBreaker {
    tokens: std::collections::VecDeque<Token>,
}

impl LineBreaker {
    fn new(runs: &[crate::box_tree::InlineRun]) -> Self {
        Self {
            tokens: tokenize_runs(runs).into(),
        }
    }

    fn is_done(&self) -> bool {
        self.tokens.is_empty()
    }

    /// Width of the next unplaced token, if any.
    fn peek_token_width(&self, options: &RenderOptions) -> Option<f32> {
        self.tokens.front().map(|token| {
            token.pieces.iter().map(|piece| piece.advance(options)).sum()
        })
    }

    /// Build the next visual line, at most `max_width` wide. Returns an empty
    /// vec only when all tokens are consumed.
    fn next_line(&mut self, max_width: f32, options: &RenderOptions) -> Vec<LinePiece> {
        let mut current: Vec<LinePiece> = Vec::new();
        let mut current_width = 0.0_f32;

        while let Some(token) = self.tokens.front() {
            let token_width: f32 = token.pieces.iter().map(|piece| piece.advance(options)).sum();

            // A token wider than the line can never fit by wrapping: fill this
            // line character-by-character (starting on an empty line) and queue
            // the remainder as the next token. An over-wide *image* token can't
            // split: it takes the whole line, scaled down to fit.
            if token_width > max_width + WRAP_TOLERANCE {
                if !current.is_empty() {
                    break;
                }
                let token = self.tokens.pop_front().expect("front token exists");
                if let Some((_, width, height)) =
                    token.pieces.first().and_then(|piece| piece.image)
                {
                    let mut piece = token.pieces.into_iter().next().expect("image piece");
                    let scale = (max_width / width).min(1.0).max(0.0);
                    piece.image = Some((piece.image.expect("is image").0, width * scale, height * scale));
                    current.push(piece);
                    break;
                }
                let leftover =
                    fill_from_long_token(&token.pieces, max_width, options, &mut current, &mut current_width);
                if !leftover.is_empty() {
                    self.tokens.push_front(Token {
                        pieces: leftover,
                        space_font_size: 0.0,
                    });
                }
                break;
            }

            let space_width = if current.is_empty() {
                0.0
            } else {
                estimate_text_width(" ", token.space_font_size, options.run_font(token.pieces.first().map(|p| p.font).unwrap_or(0)))
            };
            if !current.is_empty()
                && current_width + space_width + token_width > max_width + WRAP_TOLERANCE
            {
                break;
            }

            let token = self.tokens.pop_front().expect("front token exists");
            if !current.is_empty() {
                let lead = token.pieces.first();
                let prev = current.last();
                // A space carries a decoration (or link) only when the words on
                // *both* sides share it, so a run of decorated words gets a
                // continuous underline/strike without bleeding one space past
                // either end of the run.
                let both = |get: fn(&LinePiece) -> bool| {
                    prev.map(get).unwrap_or(false) && lead.map(get).unwrap_or(false)
                };
                let underline = both(|p| p.underline);
                let line_through = both(|p| p.line_through);
                let link = match (prev, lead) {
                    (Some(a), Some(b)) if a.link == b.link => a.link,
                    _ => 0,
                };
                current.push(LinePiece {
                    text: " ".to_string(),
                    font_size: token.space_font_size,
                    font: lead.map(|p| p.font).unwrap_or(0),
                    color: lead.map(|p| p.color).unwrap_or(Color::BLACK),
                    bold: lead.map(|p| p.bold).unwrap_or(false),
                    underline,
                    line_through,
                    link,
                    image: None,
                });
                current_width += space_width;
            }
            for piece in token.pieces {
                current_width += piece.advance(options);
                current.push(piece);
            }
        }

        current
    }
}

/// Fill `current` character-by-character from an over-long token until
/// `max_width`, returning the unplaced remainder (style preserved). A single
/// character wider than the line is still placed so callers always progress.
fn fill_from_long_token(
    pieces: &[LinePiece],
    max_width: f32,
    options: &RenderOptions,
    current: &mut Vec<LinePiece>,
    current_width: &mut f32,
) -> Vec<LinePiece> {
    let mut leftover: Vec<LinePiece> = Vec::new();
    let mut full = false;

    let push_merged = |list: &mut Vec<LinePiece>, piece: &LinePiece, ch: char| {
        if let Some(last) = list.last_mut() {
            if last.image.is_none()
                && last.font_size == piece.font_size
                && last.font == piece.font
                && last.color == piece.color
                && last.bold == piece.bold
                && last.underline == piece.underline
                && last.line_through == piece.line_through
                && last.link == piece.link
            {
                last.text.push(ch);
                return;
            }
        }
        list.push(LinePiece {
            text: ch.to_string(),
            font_size: piece.font_size,
            font: piece.font,
            color: piece.color,
            bold: piece.bold,
            underline: piece.underline,
            line_through: piece.line_through,
            link: piece.link,
            image: None,
        });
    };

    for piece in pieces {
        for ch in piece.text.chars() {
            if full {
                push_merged(&mut leftover, piece, ch);
                continue;
            }
            let char_width = estimate_text_width(&ch.to_string(), piece.font_size, options.run_font(piece.font));
            if !current.is_empty() && *current_width + char_width > max_width {
                full = true;
                push_merged(&mut leftover, piece, ch);
                continue;
            }
            push_merged(current, piece, ch);
            *current_width += char_width;
        }
    }

    leftover
}

/// A whitespace-delimited token built from the inline run stream. A token may
/// span several styles (when an inline style change falls inside a word).
struct Token {
    pieces: Vec<LinePiece>,
    /// Font size of the whitespace that preceded this token (for space width).
    space_font_size: f32,
}

/// Split styled runs into whitespace-delimited tokens, collapsing runs of
/// whitespace to single separators and dropping leading/trailing whitespace.
fn tokenize_runs(runs: &[crate::box_tree::InlineRun]) -> Vec<Token> {
    let mut tokens: Vec<Token> = Vec::new();
    let mut word: Vec<LinePiece> = Vec::new();
    // Font size of the most recent whitespace seen since the last token started.
    let mut pending_space: Option<f32> = None;
    let mut seen_token = false;

    let finish_word = |word: &mut Vec<LinePiece>,
                       tokens: &mut Vec<Token>,
                       pending_space: &mut Option<f32>| {
        if word.is_empty() {
            return;
        }
        tokens.push(Token {
            pieces: std::mem::take(word),
            space_font_size: pending_space.take().unwrap_or(0.0),
        });
    };

    for run in runs {
        // An inline image is a word of its own: it can wrap to the next line
        // but never merges with or splits across text pieces. Unresolved
        // images (failed load) contribute nothing.
        if let Some(image) = run.image.as_deref() {
            if let Some(index) = image.image_index {
                if image.width > 0.0 && image.height > 0.0 {
                    finish_word(&mut word, &mut tokens, &mut pending_space);
                    tokens.push(Token {
                        pieces: vec![LinePiece {
                            text: String::new(),
                            font_size: run.font_size,
                            font: run.font,
                            color: run.color,
                            bold: false,
                            underline: false,
                            line_through: false,
                            link: run.link,
                            image: Some((index, image.width, image.height)),
                        }],
                        space_font_size: pending_space.take().unwrap_or(0.0),
                    });
                    seen_token = true;
                }
            }
            continue;
        }
        for ch in run.text.chars() {
            if ch.is_whitespace() {
                finish_word(&mut word, &mut tokens, &mut pending_space);
                if seen_token {
                    pending_space = Some(run.font_size);
                }
                continue;
            }

            seen_token = true;
            // Extend the current word, merging into the last piece if the style
            // matches so adjacent same-style characters stay in one piece.
            if let Some(last) = word.last_mut() {
                if last.image.is_none()
                    && last.font_size == run.font_size
                    && last.font == run.font
                    && last.color == run.color
                    && last.bold == run.bold
                    && last.underline == run.underline
                    && last.line_through == run.line_through
                    && last.link == run.link
                {
                    last.text.push(ch);
                    continue;
                }
            }
            word.push(LinePiece {
                text: ch.to_string(),
                font_size: run.font_size,
                font: run.font,
                color: run.color,
                bold: run.bold,
                underline: run.underline,
                line_through: run.line_through,
                link: run.link,
                image: None,
            });
        }
    }
    finish_word(&mut word, &mut tokens, &mut pending_space);

    tokens
}

fn ensure_space(pages: &mut Vec<Page>, y: &mut f32, options: &RenderOptions, needed: f32) -> bool {
    if has_space(*y, options, needed) {
        return false;
    }

    push_page(pages, y, options);
    true
}

fn has_space(y: f32, options: &RenderOptions, needed: f32) -> bool {
    y - needed >= options.margin_bottom
}

fn push_page(pages: &mut Vec<Page>, y: &mut f32, options: &RenderOptions) {
    pages.push(Page::new());
    *y = options.page_size.height - options.margin_top;
}

fn layout_table_row(
    cells: &[TableCell],
    table_geometry: &TableGeometry,
    pages: &mut Vec<Page>,
    y: &mut f32,
    options: &RenderOptions,
    repeated_header: Option<&[TableCell]>,
) {
    let planned_cells = plan_table_cells(cells, table_geometry, options.table_row_height, options);
    let row_height = planned_cells
        .iter()
        .map(|cell| cell.height)
        .fold(options.table_row_height * table_geometry.paint_scale, f32::max);

    if !has_space(*y, options, row_height) {
        push_page(pages, y, options);

        if let Some(header_cells) = repeated_header {
            render_repeated_table_header(header_cells, table_geometry, pages, y, options);
        }
    }

    render_planned_table_row(planned_cells.as_slice(), pages, y, options, row_height);
    *y -= row_height;
}

fn render_repeated_table_header(
    cells: &[TableCell],
    table_geometry: &TableGeometry,
    pages: &mut Vec<Page>,
    y: &mut f32,
    options: &RenderOptions,
) {
    let planned_cells = plan_table_cells(cells, table_geometry, options.table_row_height, options);
    let row_height = planned_cells
        .iter()
        .map(|cell| cell.height)
        .fold(options.table_row_height * table_geometry.paint_scale, f32::max);

    if !has_space(*y, options, row_height) {
        push_page(pages, y, options);
    }

    render_planned_table_row(planned_cells.as_slice(), pages, y, options, row_height);
    *y -= row_height;
}

fn render_planned_table_row(
    planned_cells: &[PlannedCell<'_>],
    pages: &mut [Page],
    y: &mut f32,
    options: &RenderOptions,
    row_height: f32,
) {
    let page = pages.last_mut().expect("at least one page exists");
    let mut x = options.margin_left;

    for planned in planned_cells {
        let vertical_offset = vertical_text_offset(planned, row_height);

        if let Some(background_color) = planned.source.style.background_color {
            if background_color != Color::WHITE {
                page.push_colored_fill_rect(
                    Rect {
                        x,
                        y: *y - row_height,
                        width: planned.width,
                        height: row_height,
                        stroke: false,
                    },
                    background_color,
                );
            }
        }

        if planned.source.style.border == Some(true) {
            let rect = Rect {
                x,
                y: *y - row_height,
                width: planned.width,
                height: row_height,
                stroke: true,
            };
            match crate::html::resolved_borders(&planned.source.style) {
                Some(edges) => {
                    paint_cell_border(page, &edges, planned.paint_scale, rect);
                }
                // Summary on but no resolvable sides (legacy inputs that only
                // flipped the flag): the historic uniform 1px black rect.
                None => {
                    page.commands.push(PaintCommand::SetLineWidth(
                        DEFAULT_BORDER_WIDTH * planned.paint_scale,
                    ));
                    page.push_rect(rect);
                }
            }
        }

        if planned.clip_content {
            page.push_clip_rect(Rect {
                x: x + planned.padding_left,
                y: *y - row_height + planned.padding_bottom,
                width: (planned.width - planned.padding_left - planned.padding_right).max(0.0),
                height: (row_height - planned.padding_top - planned.padding_bottom).max(0.0),
                stroke: false,
            });
        }

        // A cell's default alignment follows its base direction (rtl → right),
        // like flow blocks; an explicit `text-align` wins.
        let default_align = if planned.source.style.direction == Some(true) {
            TextAlign::Right
        } else {
            TextAlign::Left
        };

        // Rich cells: paint per piece — each with its own face, size, color,
        // decoration, faux-bold decision, and clickable link rect.
        for (line_index, pieces) in planned.piece_lines.iter().enumerate() {
            let line_width: f32 = pieces.iter().map(|piece| piece.advance(options)).sum();
            let start_x = match planned.source.style.align.unwrap_or(default_align) {
                TextAlign::Left | TextAlign::Justify => x + planned.padding_left,
                TextAlign::Center => {
                    x + ((planned.width - line_width) / 2.0).max(planned.padding_left)
                }
                TextAlign::Right => {
                    x + (planned.width - line_width - planned.padding_right)
                        .max(planned.padding_left)
                }
            };
            let text_y = *y
                - planned.padding_top
                - vertical_offset
                - planned.font_size
                - (line_index as f32 * planned.leading);

            let mut px = start_x;
            for piece in pieces {
                let piece_width = piece.advance(options);
                if !piece.text.is_empty() {
                    page.push_colored_line(
                        Line {
                            text: piece.text.clone(),
                            x: px,
                            y: text_y,
                            font_size: piece.font_size,
                            font: piece.font,
                            leading: planned.leading,
                        },
                        piece.color,
                        options.run_faux_bold(piece.font, piece.bold),
                    );
                    page.push_text_decoration(
                        px,
                        text_y,
                        piece_width,
                        piece.font_size,
                        piece.color,
                        piece.underline,
                        piece.line_through,
                    );
                }
                if piece.link != 0 && piece_width > 0.0 {
                    let area = LinkArea {
                        x: px,
                        y: text_y - piece.font_size * 0.25,
                        width: piece_width,
                        height: piece.font_size * 1.1,
                        link: piece.link,
                    };
                    match page.link_areas.last_mut() {
                        Some(last)
                            if last.link == area.link
                                && last.y == area.y
                                && (last.x + last.width - area.x).abs() < 0.05 =>
                        {
                            last.width += area.width;
                        }
                        _ => page.link_areas.push(area),
                    }
                }
                px += piece_width;
            }
        }

        for (line_index, line) in planned.lines.iter().enumerate() {
            let text_width =
                estimate_text_width(line, planned.font_size, options.run_font(planned.source.font));
            let text_x = match planned.source.style.align.unwrap_or(default_align) {
                // Table cells don't justify; treat it as left.
                TextAlign::Left | TextAlign::Justify => x + planned.padding_left,
                TextAlign::Center => {
                    x + ((planned.width - text_width) / 2.0).max(planned.padding_left)
                }
                TextAlign::Right => {
                    x + (planned.width - text_width - planned.padding_right)
                        .max(planned.padding_left)
                }
            };
            let text_y = *y
                - planned.padding_top
                - vertical_offset
                - planned.font_size
                - (line_index as f32 * planned.leading);
            let text_color = planned.source.style.color.unwrap_or(Color::BLACK);

            page.push_colored_line(
                Line {
                    text: line.clone(),
                    x: text_x,
                    y: text_y,
                    font_size: planned.font_size,
                    font: planned.source.font,
                    leading: planned.leading,
                },
                text_color,
                options.run_faux_bold(planned.source.font, planned.source.style.bold),
            );
            page.push_text_decoration(
                text_x,
                text_y,
                text_width,
                planned.font_size,
                text_color,
                planned.source.style.underline,
                planned.source.style.line_through,
            );
        }

        if planned.clip_content {
            page.pop_clip();
        }

        x += planned.width;
    }
}

fn vertical_text_offset(planned: &PlannedCell<'_>, row_height: f32) -> f32 {
    let align = planned
        .source
        .style
        .vertical_align
        .unwrap_or(VerticalAlign::Top);

    match align {
        VerticalAlign::Top | VerticalAlign::Baseline => 0.0,
        VerticalAlign::Middle => available_vertical_slack(planned, row_height) / 2.0,
        VerticalAlign::Bottom => available_vertical_slack(planned, row_height),
    }
}

fn available_vertical_slack(planned: &PlannedCell<'_>, row_height: f32) -> f32 {
    let content_height = (row_height - planned.padding_top - planned.padding_bottom).max(0.0);
    let line_count = planned.lines.len().max(planned.piece_lines.len()).max(1);
    let text_height = (line_count as f32 * planned.leading).max(0.0);

    (content_height - text_height).max(0.0)
}

fn plan_table_cells<'a>(
    cells: &'a [TableCell],
    table_geometry: &TableGeometry,
    base_row_height: f32,
    options: &RenderOptions,
) -> Vec<PlannedCell<'a>> {
    let mut planned = Vec::with_capacity(cells.len());
    let mut column_index = 0;

    for cell in cells {
        let colspan = cell.colspan.max(1);
        let width = cell_width(&table_geometry.columns, column_index, colspan);
        // Font/padding scale down only in the last-resort branch of
        // `table_geometry` (min-content still overflows); normally this is 1.0.
        let paint_scale = table_geometry.paint_scale;
        let font_size = cell_font_size(cell) * paint_scale;
        // Cell leading honors CSS `line-height`; absolute lengths shrink with
        // the table's paint scale, like the font itself.
        let leading = match cell.style.line_height {
            Some(crate::html::LineHeight::Length(points)) => points * paint_scale,
            other => resolve_leading(other, font_size, CELL_LEADING_FACTOR),
        };
        let padding_left = cell.style.padding_left.unwrap_or(DEFAULT_CELL_PADDING) * paint_scale;
        let padding_right = cell.style.padding_right.unwrap_or(DEFAULT_CELL_PADDING) * paint_scale;
        let padding_top = cell.style.padding_top.unwrap_or(DEFAULT_CELL_PADDING) * paint_scale;
        let padding_bottom = cell.style.padding_bottom.unwrap_or(DEFAULT_CELL_PADDING) * paint_scale;
        let white_space = cell.style.white_space.unwrap_or(WhiteSpace::Normal);
        let break_long_tokens = should_break_long_tokens(cell);
        let max_lines = if white_space == WhiteSpace::NoWrap { 1 } else { 3 };
        let content_width = width - padding_left - padding_right;

        // Rich cells (inline markup) wrap their styled runs through the shared
        // line breaker; plain cells keep the fast single-string path.
        let (lines, piece_lines, leading) = if cell.runs.is_empty() {
            let lines = wrap_cell_text(
                &cell.text,
                content_width,
                font_size,
                max_lines,
                white_space,
                break_long_tokens,
                options.run_font(cell.font),
            );
            (lines, Vec::new(), leading)
        } else {
            let piece_lines = wrap_cell_runs(
                &cell.runs,
                content_width,
                paint_scale,
                max_lines,
                cell.style.direction.unwrap_or(false),
                options,
            );
            // Leading follows the tallest run in the cell (mixed sizes), still
            // honoring an explicit CSS line-height.
            let max_piece_font = piece_lines
                .iter()
                .flatten()
                .map(|piece| piece.font_size)
                .fold(font_size, f32::max);
            let leading = match cell.style.line_height {
                Some(crate::html::LineHeight::Length(points)) => points * paint_scale,
                other => resolve_leading(other, max_piece_font, CELL_LEADING_FACTOR),
            };
            (Vec::new(), piece_lines, leading)
        };
        let line_count = lines.len().max(piece_lines.len()).max(1);
        // A CSS-declared row height is a floor, but it shrinks with the table's
        // shrink-to-fit scale (as a browser's print scaling does) so rows don't
        // stay tall while the text is scaled down.
        let height = ((line_count as f32 * leading) + padding_top + padding_bottom)
            .max(base_row_height * table_geometry.paint_scale);
        let clip_content = cell.style.overflow.unwrap_or(Overflow::Hidden) == Overflow::Hidden;

        planned.push(PlannedCell {
            source: cell,
            width,
            lines,
            piece_lines,
            font_size,
            leading,
            height,
            padding_left,
            padding_right,
            padding_top,
            padding_bottom,
            clip_content,
            paint_scale: table_geometry.paint_scale,
        });

        column_index += colspan;
    }

    planned
}

fn is_table_row_kind(kind: BlockKind) -> bool {
    matches!(
        kind,
        BlockKind::TableHeaderRow | BlockKind::TableRow | BlockKind::TableFooterRow
    )
}

struct PlannedCell<'a> {
    source: &'a TableCell,
    width: f32,
    lines: Vec<String>,
    /// Rich-cell content: wrapped piece lines (per-run fonts/colors/links),
    /// already in visual (bidi) order. Empty for plain single-style cells,
    /// which paint from `lines`.
    piece_lines: Vec<Vec<LinePiece>>,
    font_size: f32,
    leading: f32,
    height: f32,
    padding_left: f32,
    padding_right: f32,
    padding_top: f32,
    padding_bottom: f32,
    clip_content: bool,
    /// The table's shrink-to-fit scale (border widths scale with it, like the
    /// font and padding do).
    paint_scale: f32,
}

/// Stroke a table cell's border. The uniform solid-black case reproduces the
/// historic command sequence exactly (line width + `push_rect`), keeping
/// plain spreadsheet output byte-identical; colored/dashed/per-side borders
/// paint their own strokes, always leaving solid black stroke state behind.
fn paint_cell_border(page: &mut Page, edges: &crate::html::BorderEdges, scale: f32, rect: Rect) {
    let scaled_dash = |side: crate::html::BorderSide| {
        border_dash(side).map(|dash| DashPattern {
            on: dash.on * scale,
            off: dash.off * scale,
        })
    };

    if let Some(side) = edges.uniform() {
        let dash = scaled_dash(side);
        page.commands
            .push(PaintCommand::SetLineWidth(side.width * scale));
        if let Some(pattern) = dash {
            page.commands.push(PaintCommand::SetDash(Some(pattern)));
        }
        if side.color == Color::BLACK {
            page.push_rect(rect);
        } else {
            page.commands.push(PaintCommand::SetStrokeColor(side.color));
            page.commands.push(PaintCommand::StrokeRect(RectCommand {
                x: rect.x,
                y: rect.y,
                width: rect.width,
                height: rect.height,
            }));
            page.rects.push(rect);
            page.commands.push(PaintCommand::SetStrokeColor(Color::BLACK));
        }
        if dash.is_some() {
            page.commands.push(PaintCommand::SetDash(None));
        }
        return;
    }

    let (x0, y0) = (rect.x, rect.y);
    let (x1, y1) = (rect.x + rect.width, rect.y + rect.height);
    let sides = [
        (edges.top, (x0, y1, x1, y1)),
        (edges.bottom, (x0, y0, x1, y0)),
        (edges.left, (x0, y0, x0, y1)),
        (edges.right, (x1, y0, x1, y1)),
    ];
    let mut recolored = false;
    for (side, (ax, ay, bx, by)) in sides {
        let Some(side) = side else { continue };
        page.commands.push(PaintCommand::SetStrokeColor(side.color));
        recolored |= side.color != Color::BLACK;
        page.commands
            .push(PaintCommand::SetLineWidth(side.width * scale));
        let dash = scaled_dash(side);
        if let Some(pattern) = dash {
            page.commands.push(PaintCommand::SetDash(Some(pattern)));
        }
        page.commands.push(PaintCommand::StrokeLine(LineCommand {
            x1: ax,
            y1: ay,
            x2: bx,
            y2: by,
        }));
        if dash.is_some() {
            page.commands.push(PaintCommand::SetDash(None));
        }
    }
    if recolored {
        page.commands.push(PaintCommand::SetStrokeColor(Color::BLACK));
    }
}

#[derive(Debug, Clone, PartialEq)]
struct TableGeometry {
    columns: Vec<f32>,
    /// Uniform font/padding scale applied when the table is wider than the page
    /// even at min-content (shrink-to-fit, like a browser's print path); `1.0`
    /// otherwise.
    paint_scale: f32,
}

/// Slack (pt) allowed before wrapping, so text that measures exactly the line
/// width (an auto-sized column fitting its own content) is not broken by a
/// floating-point rounding error.
const WRAP_TOLERANCE: f32 = 0.25;

/// UA-default leading factors (multiples of the font size) when no CSS
/// `line-height` applies: flow line boxes and table-cell lines.
const FLOW_LEADING_FACTOR: f32 = 1.35;
const CELL_LEADING_FACTOR: f32 = 1.18;

/// The distance between successive baselines: an explicit CSS `line-height`
/// (a number scales the font in use, a length is absolute), else the UA
/// default `font × default_factor`.
fn resolve_leading(
    line_height: Option<crate::html::LineHeight>,
    font_size: f32,
    default_factor: f32,
) -> f32 {
    match line_height {
        Some(crate::html::LineHeight::Number(n)) => font_size * n,
        Some(crate::html::LineHeight::Length(points)) => points,
        None => font_size * default_factor,
    }
}

/// Default table-cell font size (pt) when the cascade sets none — the browser
/// default of ~11pt rather than a shrink-to-fit fudge.
const DEFAULT_CELL_FONT_SIZE: f32 = 11.0;

/// Default border stroke width (pt) — a 1px CSS border at 96 dpi. Kept thin so
/// gridlines look like a browser's, not a heavy 1pt default.
const DEFAULT_BORDER_WIDTH: f32 = 0.75;
/// Default table-cell padding (pt) when the cascade sets none (≈ the 1px UA
/// default). Real spreadsheet exports set padding explicitly.
const DEFAULT_CELL_PADDING: f32 = 1.0;

/// Vertical margin (pt) above and below a flow-embedded table, so surrounding
/// paragraphs clear its edges (table rows carry no line leading of their own).
/// Collapses with adjacent block margins like any CSS vertical margin.
const TABLE_FLOW_MARGIN: f32 = 10.0;

fn cell_font_size(cell: &TableCell) -> f32 {
    cell.style.font_size.unwrap_or(DEFAULT_CELL_FONT_SIZE)
}

fn cell_padding_x(cell: &TableCell) -> f32 {
    cell.style.padding_left.unwrap_or(DEFAULT_CELL_PADDING)
        + cell.style.padding_right.unwrap_or(DEFAULT_CELL_PADDING)
}

/// The min-content width of `text`: the widest run that cannot be broken. Normally
/// that is the widest whitespace-separated word, but when the cell allows breaking
/// inside words (`overflow-wrap`/`word-break`) it drops to the widest single
/// character, so the column can be narrow and the text wraps rather than forcing a
/// font downscale.
fn min_content_width(text: &str, font_size: f32, font: &crate::font::Font, breakable: bool) -> f32 {
    if breakable {
        text.chars()
            .filter(|ch| !ch.is_whitespace())
            .map(|ch| estimate_text_width(&ch.to_string(), font_size, font))
            .fold(0.0, f32::max)
    } else {
        text.split_whitespace()
            .map(|word| estimate_text_width(word, font_size, font))
            .fold(0.0, f32::max)
    }
}

/// Compute table column widths the way a browser's automatic table layout does,
/// rather than force-fitting oversized declared widths by shrinking the font.
///
/// Each column gets a min-content width (its widest word) and a max-content width
/// (its widest cell on one line), both including padding. Declared `<col>` widths
/// are honored only when they collectively fit; otherwise columns are sized to
/// content. The chosen widths are then distributed into the available width:
/// content fits → use it as-is at full font size; too wide but min-content fits →
/// shrink wide columns toward their min-content (text wraps, font stays); even
/// min-content overflows → only then scale the font down (`paint_scale`).
/// Document-path wrapper: build the row cell-slices from `document.blocks` and
/// delegate to [`table_geometry_cells`].
fn table_geometry(document: &Document, content_width: f32, font: &crate::font::Font) -> TableGeometry {
    let rows: Vec<&[TableCell]> = document
        .blocks
        .iter()
        .filter(|block| is_table_row_kind(block.kind))
        .map(|block| block.cells.as_slice())
        .collect();
    table_geometry_cells(&rows, &document.table_columns, content_width, font)
}

/// Compute table column widths (browser-style automatic table layout) from a set
/// of rows (each a slice of cells) and any declared column widths. Shared by the
/// spreadsheet `blocks` path and flow-embedded `Table` boxes.
fn table_geometry_cells(
    rows: &[&[TableCell]],
    declared: &[f32],
    content_width: f32,
    font: &crate::font::Font,
) -> TableGeometry {
    let column_count = rows
        .iter()
        .map(|cells| cells.iter().map(|cell| cell.colspan.max(1)).sum::<usize>())
        .max()
        .unwrap_or(1)
        .max(1);

    let mut min_content = vec![0.0f32; column_count];
    let mut max_content = vec![0.0f32; column_count];
    // Cells spanning multiple columns constrain the spanned columns' totals.
    let mut spans: Vec<(usize, usize, f32, f32)> = Vec::new();

    for cells in rows {
        let mut col = 0;
        for cell in cells.iter() {
            if col >= column_count {
                break;
            }
            let span = cell.colspan.max(1);
            let end = (col + span).min(column_count);
            let font_size = cell_font_size(cell);
            let padding = cell_padding_x(cell);
            let max_w = estimate_text_width(&cell.text, font_size, font) + padding;
            let min_w =
                min_content_width(&cell.text, font_size, font, should_break_long_tokens(cell))
                    + padding;
            if end - col == 1 {
                max_content[col] = max_content[col].max(max_w);
                min_content[col] = min_content[col].max(min_w);
            } else {
                spans.push((col, end - col, min_w, max_w));
            }
            col = end;
        }
    }

    // Grow spanned columns so each spanning cell's content fits across them.
    let grow = |widths: &mut [f32], start: usize, span: usize, need: f32| {
        let current: f32 = widths[start..start + span].iter().sum();
        if current < need {
            let add = (need - current) / span as f32;
            widths[start..start + span].iter_mut().for_each(|w| *w += add);
        }
    };
    for (start, span, min_w, max_w) in spans {
        grow(&mut min_content, start, span, min_w);
        grow(&mut max_content, start, span, max_w);
    }
    for col in 0..column_count {
        max_content[col] = max_content[col].max(min_content[col]);
    }

    // Prefer declared widths only when they fit and respect every column's
    // min-content; otherwise size to content (matching a browser's auto layout).
    let declared_total: f32 = declared.iter().sum();
    let use_declared = declared.len() == column_count
        && declared_total > 0.0
        && declared_total <= content_width
        && declared
            .iter()
            .zip(&min_content)
            .all(|(d, m)| *d + 0.5 >= *m);
    let upper: Vec<f32> = if use_declared {
        declared.to_vec()
    } else {
        max_content.clone()
    };

    let total_upper: f32 = upper.iter().sum();
    let total_min: f32 = min_content.iter().sum();

    if total_upper <= 0.0 {
        // No measurable content: fall back to equal division.
        let even = content_width / column_count as f32;
        return TableGeometry {
            columns: vec![even; column_count],
            paint_scale: 1.0,
        };
    }

    if total_upper <= content_width {
        // Everything fits at its natural width; keep the font at its CSS size.
        TableGeometry {
            columns: upper,
            paint_scale: 1.0,
        }
    } else if total_min <= content_width {
        // Too wide, but shrinking multi-word columns toward their longest word
        // makes it fit. Text wraps between words; font stays at its CSS size.
        let flex = total_upper - total_min;
        let t = if flex > 0.0 {
            ((content_width - total_min) / flex).clamp(0.0, 1.0)
        } else {
            0.0
        };
        let columns = upper
            .iter()
            .zip(&min_content)
            .map(|(u, m)| m + (u - m) * t)
            .collect();
        TableGeometry {
            columns,
            paint_scale: 1.0,
        }
    } else {
        // Wider than the page even with every column at its longest word: scale
        // columns and font down uniformly to fit (shrink-to-fit), the way a
        // browser's print path fits an over-wide table onto the page. The scale
        // is available/min-content, so text stays as large as possible while
        // every column keeps its content on one line.
        let scale = content_width / total_min;
        TableGeometry {
            columns: min_content.iter().map(|m| m * scale).collect(),
            paint_scale: scale,
        }
    }
}

fn cell_width(columns: &[f32], start: usize, colspan: usize) -> f32 {
    let width = columns
        .iter()
        .skip(start)
        .take(colspan)
        .copied()
        .sum::<f32>();

    if width > 0.0 {
        width
    } else {
        48.0 * colspan as f32
    }
}

fn wrap_text_with_mode(
    text: &str,
    max_width: f32,
    font_size: f32,
    white_space: WhiteSpace,
    break_long_tokens: bool,
    font: &crate::font::Font,
) -> Vec<String> {
    let mut lines = Vec::new();
    let mut current = String::new();
    // Track the measured width of `current` so line fitting stays O(n) overall
    // instead of re-measuring the whole line for every word. The fonts we use
    // have no kerning, so advances are additive and this is exact.
    let mut current_width = 0.0f32;
    let space_width = estimate_text_width(" ", font_size, font);

    for word in text.split_whitespace() {
        if white_space == WhiteSpace::NoWrap {
            append_word_preserving_no_wrap(word, &mut current);
            continue;
        }

        let word_width = estimate_text_width(word, font_size, font);

        if break_long_tokens && word_width > max_width + WRAP_TOLERANCE {
            if !current.is_empty() {
                lines.push(std::mem::take(&mut current));
                current_width = 0.0;
            }

            lines.extend(split_long_word(word, max_width, font_size, font));
            continue;
        }

        if !current.is_empty()
            && current_width + space_width + word_width > max_width + WRAP_TOLERANCE
        {
            lines.push(std::mem::take(&mut current));
            current_width = 0.0;
        }

        if current.is_empty() {
            current.push_str(word);
            current_width = word_width;
        } else {
            current.push(' ');
            current.push_str(word);
            current_width += space_width + word_width;
        }
    }

    if !current.is_empty() {
        lines.push(current);
    }

    lines
}

fn append_word_preserving_no_wrap(word: &str, current: &mut String) {
    if !current.is_empty() {
        current.push(' ');
    }
    current.push_str(word);
}

fn split_long_word(
    word: &str,
    max_width: f32,
    font_size: f32,
    font: &crate::font::Font,
) -> Vec<String> {
    let mut lines = Vec::new();
    let mut rest = word;

    while !rest.is_empty() {
        let count = font.fitting_char_count(rest, max_width, font_size);
        let split_at = rest
            .char_indices()
            .nth(count)
            .map(|(index, _)| index)
            .unwrap_or(rest.len());
        lines.push(rest[..split_at].to_string());
        rest = &rest[split_at..];
    }

    lines
}

/// Wrap a rich cell's styled runs into at most `max_lines` piece lines using
/// the shared line breaker (per-run fonts/sizes measured correctly), scaling
/// every run's font by the table's `paint_scale` first. Lines come back in
/// visual (bidi) order. Content past the line cap is dropped (the cell clips,
/// like the plain-text path).
fn wrap_cell_runs(
    runs: &[crate::box_tree::InlineRun],
    content_width: f32,
    paint_scale: f32,
    max_lines: usize,
    base_rtl: bool,
    options: &RenderOptions,
) -> Vec<Vec<LinePiece>> {
    let scaled: Vec<crate::box_tree::InlineRun>;
    let runs = if (paint_scale - 1.0).abs() < f32::EPSILON {
        runs
    } else {
        scaled = runs
            .iter()
            .map(|run| {
                let mut run = run.clone();
                run.font_size *= paint_scale;
                run
            })
            .collect();
        &scaled
    };

    let mut breaker = LineBreaker::new(runs);
    let mut lines = Vec::new();
    while !breaker.is_done() && lines.len() < max_lines {
        // `white-space: nowrap` (max_lines == 1) never wraps: give the single
        // line unbounded width and let the cell's clip rect cut it off.
        let line_width = if max_lines == 1 { f32::MAX } else { content_width.max(1.0) };
        let visual = breaker.next_line(line_width, options);
        if visual.is_empty() {
            break;
        }
        lines.push(reorder_pieces_bidi(visual, base_rtl));
    }
    lines
}

fn wrap_cell_text(
    text: &str,
    max_width: f32,
    font_size: f32,
    max_lines: usize,
    white_space: WhiteSpace,
    break_long_tokens: bool,
    font: &crate::font::Font,
) -> Vec<String> {
    if text.is_empty() {
        return Vec::new();
    }

    let mut lines =
        wrap_text_with_mode(text, max_width, font_size, white_space, break_long_tokens, font);

    if lines.len() > max_lines {
        lines.truncate(max_lines);
        if let Some(last) = lines.last_mut() {
            *last = truncate_to_width(last, max_width, font_size, font);
        }
    }

    lines
}

fn should_break_long_tokens(cell: &TableCell) -> bool {
    if cell.style.white_space.unwrap_or(WhiteSpace::Normal) == WhiteSpace::NoWrap {
        return false;
    }

    matches!(
        cell.style.overflow_wrap,
        Some(OverflowWrap::Anywhere | OverflowWrap::BreakWord)
    ) || matches!(cell.style.word_break, Some(WordBreak::BreakAll))
}

fn estimate_text_width(text: &str, font_size: f32, font: &crate::font::Font) -> f32 {
    font.text_width(text, font_size)
}

fn truncate_to_width(
    text: &str,
    max_width: f32,
    font_size: f32,
    font: &crate::font::Font,
) -> String {
    if estimate_text_width(text, font_size, font) <= max_width {
        return text.to_string();
    }

    let ellipsis = "...";
    let ellipsis_width = estimate_text_width(ellipsis, font_size, font);
    let budget = (max_width - ellipsis_width).max(0.0);
    let keep = font.fitting_char_count(text, budget, font_size);

    // `fitting_char_count` returns at least 1 for non-empty input; if even one
    // glyph plus the ellipsis overflows a tiny box we still keep one glyph so
    // output stays non-empty and deterministic.
    let prefix: String = text.chars().take(keep).collect();
    format!("{prefix}{ellipsis}")
}

pub(crate) fn font_size_for(kind: BlockKind) -> f32 {
    match kind {
        BlockKind::Heading1 => 24.0,
        BlockKind::Heading2 => 18.0,
        BlockKind::Heading3 => 14.0,
        BlockKind::Heading4 => 12.0,
        BlockKind::Heading5 => 10.5,
        BlockKind::Heading6 => 9.0,
        BlockKind::Paragraph
        | BlockKind::TableHeaderRow
        | BlockKind::TableRow
        | BlockKind::TableFooterRow => 11.0,
    }
}

pub(crate) fn spacing_before(kind: BlockKind) -> f32 {
    match kind {
        BlockKind::Heading1 => 0.0,
        BlockKind::Heading2 => 10.0,
        BlockKind::Heading3 | BlockKind::Heading4 | BlockKind::Heading5 | BlockKind::Heading6 => 8.0,
        BlockKind::Paragraph
        | BlockKind::TableHeaderRow
        | BlockKind::TableRow
        | BlockKind::TableFooterRow => 6.0,
    }
}

pub(crate) fn spacing_after(kind: BlockKind) -> f32 {
    match kind {
        BlockKind::Heading1 => 12.0,
        BlockKind::Heading2 => 8.0,
        BlockKind::Heading3 | BlockKind::Heading4 | BlockKind::Heading5 | BlockKind::Heading6 => 6.0,
        BlockKind::Paragraph
        | BlockKind::TableHeaderRow
        | BlockKind::TableRow
        | BlockKind::TableFooterRow => 4.0,
    }
}

#[cfg(test)]
mod tests {
    use crate::color::Color;
    use crate::html::{Block, BlockKind, Document};
    use crate::paint::PaintCommand;

    /// The historic default border for hand-built test boxes: uniform 1px
    /// solid black.
    const TEST_BORDER_SIDE: crate::html::BorderSide = crate::html::BorderSide {
        width: super::DEFAULT_BORDER_WIDTH,
        style: crate::html::BorderStyle::Solid,
        color: Color::BLACK,
    };

    use super::{
        estimate_text_width, justify_offsets, layout_document, table_geometry, PageSize,
        RenderOptions,
    };

    #[test]
    fn min_width_widens_a_narrow_background_box() {
        // `width: 100pt` would make the card 100pt wide, but `min-width: 300pt`
        // wins and widens it — the painted background fill spans ~300pt.
        let document = crate::html::parse(
            "<style>.card { width: 100pt; min-width: 300pt; background: #eee; }</style>\
             <div class=\"card\">hi</div>",
        );
        let pages = layout_document(&document, &RenderOptions::default());
        let fill = pages[0]
            .commands
            .iter()
            .find_map(|c| match c {
                PaintCommand::FillRect(r) => Some(r),
                _ => None,
            })
            .expect("background fill");
        assert!(
            (fill.width - 300.0).abs() < 1.0,
            "min-width should widen the box to ~300pt, got {}",
            fill.width
        );
    }

    #[test]
    fn percent_padding_resolves_against_containing_width() {
        // `padding-left: 10%` of the ~499pt A4 content width ≈ 49.9pt, so the
        // paragraph's text starts about that far in from the content edge.
        let document = crate::html::parse(
            "<style>.p { padding-left: 10%; }</style><p class=\"p\">indented</p>",
        );
        let options = RenderOptions::default();
        let pages = layout_document(&document, &options);
        let line = pages[0]
            .lines
            .iter()
            .find(|l| l.text == "indented")
            .expect("text line");
        let content_left = options.margin_left;
        let indent = line.x - content_left;
        let expected = 0.10 * (options.page_size.width - options.margin_left - options.margin_right);
        assert!(
            (indent - expected).abs() < 2.0,
            "expected ~{expected}pt percent indent, got {indent}"
        );
    }

    #[test]
    fn overflow_hidden_clips_a_fixed_height_box() {
        // A short fixed-height box with `overflow: hidden` and lots of content
        // caps its height and wraps the start-page content in a clip rect.
        let document = crate::html::parse(
            "<style>.clip { height: 30pt; overflow: hidden; background: #eee; }</style>\
             <div class=\"clip\"><p>line one</p><p>line two</p><p>line three</p>\
             <p>line four</p><p>line five</p></div>",
        );
        let pages = layout_document(&document, &RenderOptions::default());
        let clips = pages[0]
            .commands
            .iter()
            .filter(|c| matches!(c, PaintCommand::PushClipRect(_)))
            .count();
        let pops = pages[0]
            .commands
            .iter()
            .filter(|c| matches!(c, PaintCommand::PopClip))
            .count();
        assert!(clips >= 1, "overflow:hidden should push a clip rect");
        assert_eq!(clips, pops, "every clip is popped");
    }

    #[test]
    fn flex_wrap_breaks_items_onto_new_lines() {
        let document = crate::html::parse(
            "<style>.row { display: flex; flex-wrap: wrap; gap: 10pt; } \
                    .card { flex-basis: 200pt; }</style>\
             <div class=\"row\">\
               <div class=\"card\">one</div><div class=\"card\">two</div>\
               <div class=\"card\">three</div><div class=\"card\">four</div>\
             </div>",
        );
        // A4 content width ~499pt fits two 200pt cards + one gap per line, so
        // four cards make two flex lines: two distinct baselines, and the
        // third card starts back at the left content edge.
        let options = RenderOptions::default();
        let pages = layout_document(&document, &options);
        let lines = &pages[0].lines;
        assert_eq!(lines.len(), 4);
        let mut ys: Vec<f32> = lines.iter().map(|l| l.y).collect();
        ys.dedup();
        assert_eq!(ys.len(), 2, "two flex lines: {ys:?}");
        let three = lines.iter().find(|l| l.text == "three").unwrap();
        let one = lines.iter().find(|l| l.text == "one").unwrap();
        assert_eq!(three.x, one.x, "second line restarts at the left edge");
        assert!(three.y < one.y);
    }

    #[test]
    fn grid_line_placement_and_minmax_size_tracks() {
        let document = crate::html::parse(
            "<style>.g { display: grid; grid-template-columns: 100pt minmax(150pt, 1fr) 1fr; } \
                    .full { grid-column: 1 / -1; } .b { grid-column: 2; }</style>\
             <div class=\"g\">\
               <div class=\"full\">header</div>\
               <div class=\"b\">second</div>\
               <div>third</div>\
             </div>",
        );
        let options = RenderOptions::default();
        let pages = layout_document(&document, &options);
        let find = |t: &str| pages[0].lines.iter().find(|l| l.text == t).unwrap();

        // A4 content: 595.28 - 96 = 499.28pt. Track 1 = 100, remainder 399.28
        // split between minmax(150,1fr) and 1fr = ~199.6 each (the floor does
        // not bind), so column 2 starts at 48 + 100 = 148.
        let header = find("header");
        let second = find("second");
        let third = find("third");
        assert_eq!(header.x, 48.0, "full-row item starts at track 1");
        assert!((second.x - 148.0).abs() < 0.5, "explicit column 2: {}", second.x);
        assert!(third.x > second.x + 150.0, "auto-placed after the pinned item");
        assert!(second.y < header.y, "explicit placement moved below the full-row header");
        assert_eq!(second.y, third.y, "second and third share a row");
    }

    #[test]
    fn justify_stretches_all_lines_but_the_last() {
        let words = "alpha beta gamma delta ".repeat(12);
        let document = crate::html::parse(&format!(
            "<p style=\"text-align: justify\">{words}end</p>"
        ));
        let options = RenderOptions::default();
        let pages = layout_document(&document, &options);
        let lines = &pages[0].lines;

        // Right edge of each painted piece; group by baseline.
        let mut rows: std::collections::BTreeMap<i64, f32> = std::collections::BTreeMap::new();
        for line in lines {
            let right = line.x
                + estimate_text_width(&line.text, line.font_size, options.run_font(line.font));
            let key = (line.y * 100.0) as i64;
            let entry = rows.entry(key).or_insert(right);
            *entry = entry.max(right);
        }
        let mut rights: Vec<f32> = rows.into_iter().rev().map(|(_, r)| r).collect();
        assert!(rights.len() >= 3, "need several lines: {}", rights.len());
        let last = rights.pop().unwrap();
        let margin_right_edge = options.page_size.width - options.margin_right;
        for right in &rights {
            assert!(
                (right - margin_right_edge).abs() < 1.0,
                "justified line should end flush at {margin_right_edge}, got {right}"
            );
        }
        assert!(last < margin_right_edge - 20.0, "last line stays ragged: {last}");
    }

    #[test]
    fn rich_cells_paint_per_run_styles_and_links() {
        let document = crate::html::parse(
            "<table><tr>\
               <td>plain</td>\
               <td>pay <b>now</b> via <a href=\"https://x.test/pay\">this link</a></td>\
             </tr></table>",
        );
        let options = RenderOptions::default().with_document_hints(&document);
        let pages = layout_document(&document, &options);

        // Both cells' words paint; the bold word carries the bold flag through
        // the faux-bold decision (default font: no real bold face).
        let texts: Vec<&str> = pages[0]
            .commands
            .iter()
            .filter_map(|c| match c {
                PaintCommand::Text(t) => Some(t.text.as_str()),
                _ => None,
            })
            .collect();
        assert!(texts.contains(&"plain"));
        assert!(texts.contains(&"now"));
        let bold_cmd = pages[0]
            .commands
            .iter()
            .find_map(|c| match c {
                PaintCommand::Text(t) if t.text == "now" => Some(t),
                _ => None,
            })
            .unwrap();
        assert!(bold_cmd.bold, "run-level bold survives into the cell paint");

        // The linked words produce a clickable area pointing at the target.
        assert_eq!(options.links, vec!["https://x.test/pay".to_string()]);
        assert!(
            !pages[0].link_areas.is_empty(),
            "cell link produced a clickable area"
        );
        assert!(pages[0].link_areas.iter().all(|a| a.link == 1));
    }

    #[test]
    fn rtl_cells_right_align_their_content() {
        let document = crate::html::parse(
            "<table><tr>\
               <td dir=\"rtl\"><b>\u{05E9}\u{05DC}\u{05D5}\u{05DD}</b></td>\
               <td><b>ltr</b></td>\
             </tr><tr>\
               <td>a much longer plain cell that widens the first column</td>\
               <td>x</td>\
             </tr></table>",
        );
        let options = RenderOptions::default().with_document_hints(&document);
        let pages = layout_document(&document, &options);
        let rtl = pages[0].lines.iter().find(|l| l.text.contains('\u{05E9}')).unwrap();
        let ltr = pages[0].lines.iter().find(|l| l.text == "ltr").unwrap();
        // Column 1 spans half the table; the RTL cell's text must hug the
        // column's right edge, far past where the LTR cell's text starts
        // relative to its own column.
        let rtl_offset_from_left = rtl.x - options.margin_left;
        assert!(
            rtl_offset_from_left > 20.0,
            "rtl cell content right-aligned (offset {rtl_offset_from_left})"
        );
        assert!(ltr.x > rtl.x, "second column starts after the first");
    }

    #[test]
    fn inline_images_sit_on_the_baseline_and_grow_the_line() {
        use crate::box_tree::{BoxChild, FlowRoot, ImageBox, InlineRun};

        let run = |text: &str| InlineRun {
            text: text.to_string(),
            font_size: 12.0,
            bold: false,
            font: 0,
            link: 0,
            underline: false,
            line_through: false,
            color: Color::BLACK,
            image: None,
        };
        let image_run = |w: f32, h: f32| InlineRun {
            image: Some(Box::new(ImageBox {
                src: String::new(),
                attr_width: None,
                attr_height: None,
                css_width: None,
                css_width_percent: None,
                css_height: None,
                max_width: None,
                max_width_percent: None,
                image_index: Some(0),
                width: w,
                height: h,
                float_dir: None,
            })),
            ..run("")
        };

        let document = Document {
            page_style: crate::html::PageStyle::default(),
            table_style: crate::html::TableStyle::default(),
            table_columns: Vec::new(),
            images: Vec::new(),
            font_specs: Vec::new(),
            font_faces: Vec::new(),
            links: Vec::new(),
            flow: Some(FlowRoot {
                children: vec![BoxChild::Line(vec![
                    run("before "),
                    image_run(30.0, 40.0),
                    run(" after"),
                ])],
            }),
            blocks: Vec::new(),
        };
        let options = RenderOptions::default();
        let pages = layout_document(&document, &options);

        let image = pages[0]
            .commands
            .iter()
            .find_map(|c| match c {
                PaintCommand::Image(image) => Some(*image),
                _ => None,
            })
            .expect("inline image painted");
        let before = pages[0].lines.iter().find(|l| l.text == "before").unwrap();
        let after = pages[0].lines.iter().find(|l| l.text == "after").unwrap();

        // Bottom of the image sits on the shared baseline; text continues
        // after it on the same line.
        assert_eq!(image.y, before.y, "image bottom on the text baseline");
        assert_eq!(before.y, after.y, "one line");
        assert!(image.x > before.x);
        assert!(after.x >= image.x + image.width - 0.5);
        assert_eq!(image.width, 30.0);

        // The 40pt image rises above the 12pt text ascent, so the line's top
        // must clear it: baseline sits at least 40pt below the content top.
        let top = options.page_size.height - options.margin_top;
        assert!(
            top - before.y >= 40.0 - 0.5,
            "line box grew to the image height (drop = {})",
            top - before.y
        );
    }

    #[test]
    fn oversized_inline_images_scale_to_the_line() {
        use crate::box_tree::{BoxChild, FlowRoot, ImageBox, InlineRun};
        let image_run = InlineRun {
            text: String::new(),
            font_size: 12.0,
            bold: false,
            font: 0,
            link: 0,
            underline: false,
            line_through: false,
            color: Color::BLACK,
            image: Some(Box::new(ImageBox {
                src: String::new(),
                attr_width: None,
                attr_height: None,
                css_width: None,
                css_width_percent: None,
                css_height: None,
                max_width: None,
                max_width_percent: None,
                image_index: Some(0),
                width: 900.0,
                height: 300.0,
                float_dir: None,
            })),
        };
        let document = Document {
            page_style: crate::html::PageStyle::default(),
            table_style: crate::html::TableStyle::default(),
            table_columns: Vec::new(),
            images: Vec::new(),
            font_specs: Vec::new(),
            font_faces: Vec::new(),
            links: Vec::new(),
            flow: Some(FlowRoot {
                children: vec![BoxChild::Line(vec![image_run])],
            }),
            blocks: Vec::new(),
        };
        let options = RenderOptions::default();
        let pages = layout_document(&document, &options);
        let image = pages[0]
            .commands
            .iter()
            .find_map(|c| match c {
                PaintCommand::Image(image) => Some(*image),
                _ => None,
            })
            .expect("image painted");
        let content_width = options.page_size.width - options.margin_left - options.margin_right;
        assert!(image.width <= content_width + 0.5, "scaled to fit: {}", image.width);
        // Aspect ratio preserved by the same scale.
        assert!((image.height / image.width - 300.0 / 900.0).abs() < 0.01);
    }

    #[test]
    fn negative_z_index_paints_below_flow_content() {
        // The classic pattern: an empty, sized, absolutely-positioned div with
        // z-index: -1 painting a band *behind* the flow text, while a positive
        // z-index badge paints above it.
        let document = crate::html::parse(
            "<style>.hero { position: relative; } \
                    .bg { position: absolute; z-index: -1; top: 0; left: 0; \
                          width: 300pt; height: 80pt; background: #ffd54a; } \
                    .badge { position: absolute; z-index: 5; top: 0; right: 10pt; \
                             background: #c62828; margin: 0; }</style>\
             <div class=\"hero\"><div class=\"bg\"></div>\
             <p class=\"badge\">TOP</p><h1>Headline</h1></div>",
        );
        let pages = layout_document(&document, &RenderOptions::default());
        let commands = &pages[0].commands;

        let first_text = commands
            .iter()
            .position(|c| matches!(c, PaintCommand::Text(_)))
            .expect("has text");
        let fill_rects: Vec<usize> = commands
            .iter()
            .enumerate()
            .filter(|(_, c)| matches!(c, PaintCommand::FillRect(_)))
            .map(|(i, _)| i)
            .collect();
        // The band (z: -1) paints before any text; the badge (z: 5) after.
        assert!(
            fill_rects.iter().any(|&i| i < first_text),
            "a background rect must precede the first text (below the flow)"
        );
        assert!(
            fill_rects.iter().any(|&i| i > first_text),
            "the positive-z badge rect must follow the flow text (above it)"
        );
    }

    #[test]
    fn paints_per_side_dashed_and_rounded_borders() {
        let document = crate::html::parse(
            r##"
            <style>
            .dash { border: 2px dashed #b00; padding: 4px; }
            .rule { border-bottom: 3px solid #00f; }
            .round { border: 2px solid black; border-radius: 6px;
                     background-color: #dde; padding: 4px; }
            </style>
            <div class="dash">dashed box</div>
            <div class="rule">ruled paragraph</div>
            <div class="round">rounded card</div>
            "##,
        );
        let pages = layout_document(&document, &RenderOptions::default());
        let commands = &pages[0].commands;

        // Dashed: a dash pattern is set and always reset back to solid.
        let dash_on = commands
            .iter()
            .any(|c| matches!(c, PaintCommand::SetDash(Some(_))));
        let dash_off = commands
            .iter()
            .any(|c| matches!(c, PaintCommand::SetDash(None)));
        assert!(dash_on && dash_off, "dash set + reset expected");

        // The single-side rule paints a line, in its declared blue.
        assert!(commands
            .iter()
            .any(|c| matches!(c, PaintCommand::StrokeLine(_))));
        assert!(commands.iter().any(|c| matches!(
            c,
            PaintCommand::SetStrokeColor(color) if color.b > 0.9 && color.r < 0.1
        )));

        // border-radius: both the background and the border take the rounded
        // path commands.
        assert!(commands
            .iter()
            .any(|c| matches!(c, PaintCommand::FillRoundedRect(r) if r.radius > 0.0)));
        assert!(commands
            .iter()
            .any(|c| matches!(c, PaintCommand::StrokeRoundedRect(r) if r.radius > 0.0)));
    }

    #[test]
    fn border_box_sizing_narrows_the_declared_width() {
        // Same declared width; the border-box one absorbs padding + border
        // into it, so its stroked border box is exactly the declared 90pt
        // (120px) while the content-box one adds 8px padding and 3px border
        // per side (90 + 2*6 + 2*2.25 = 106.5).
        let document = crate::html::parse(
            r#"
            <div style="width: 120px; padding: 8px; border: 3px solid black">a</div>
            <div style="width: 120px; padding: 8px; border: 3px solid black; box-sizing: border-box">b</div>
            "#,
        );
        let pages = layout_document(&document, &RenderOptions::default());
        let mut widths: Vec<f32> = pages[0]
            .commands
            .iter()
            .filter_map(|c| match c {
                PaintCommand::StrokeRect(rect) => Some(rect.width),
                _ => None,
            })
            .collect();
        widths.sort_by(|a, b| a.partial_cmp(b).unwrap());
        assert_eq!(widths.len(), 2, "two bordered boxes");
        assert!((widths[0] - 90.0).abs() < 0.01, "border-box: {}", widths[0]);
        assert!((widths[1] - 106.5).abs() < 0.01, "content-box: {}", widths[1]);
    }

    #[test]
    fn cell_border_bottom_paints_a_line_not_a_rect() {
        let document = crate::html::parse(
            "<style>td { border-bottom: 1pt solid #cccccc; }</style>\
             <table><tr><td>alpha</td><td>beta</td></tr></table>",
        );
        let pages = layout_document(&document, &RenderOptions::default());
        let commands = &pages[0].commands;
        let lines = commands
            .iter()
            .filter(|c| matches!(c, PaintCommand::StrokeLine(_)))
            .count();
        let rects = commands
            .iter()
            .filter(|c| matches!(c, PaintCommand::StrokeRect(_)))
            .count();
        assert_eq!(lines, 2, "one bottom edge per cell");
        assert_eq!(rects, 0, "no full border rectangles");
        // The declared #ccc stroke color is used, and solid black is restored
        // afterwards so later strokes are unaffected.
        assert!(commands.iter().any(|c| matches!(
            c,
            PaintCommand::SetStrokeColor(color) if (color.r - 0.8).abs() < 0.01
        )));
        assert!(matches!(
            commands
                .iter()
                .filter(|c| matches!(c, PaintCommand::SetStrokeColor(_)))
                .last(),
            Some(PaintCommand::SetStrokeColor(color)) if *color == Color::BLACK
        ));
    }

    #[test]
    fn css_height_extends_short_blocks() {
        // A 100pt-high box with one short line: the following paragraph must
        // start below the full declared height, not right under the line.
        let document = crate::html::parse(
            "<div style=\"height: 100pt; background: #eee; margin: 0\">short</div>\
             <p style=\"margin: 0\">after</p>",
        );
        let pages = layout_document(&document, &RenderOptions::default());
        let short = pages[0].lines.iter().find(|l| l.text == "short").unwrap();
        let after = pages[0].lines.iter().find(|l| l.text == "after").unwrap();
        assert!(
            short.y - after.y >= 90.0,
            "gap should span the declared height, got {}",
            short.y - after.y
        );
    }

    #[test]
    fn links_produce_merged_areas_and_headings_produce_anchors() {
        let document = crate::html::parse(
            "<h1 id=\"top\">The Report Title</h1>\
             <p>visit <a href=\"https://x.test/docs\">the docs pages</a> today</p>\
             <h2>Details</h2>",
        );
        let options = RenderOptions::default().with_document_hints(&document);
        assert_eq!(options.links, vec!["https://x.test/docs".to_string()]);

        let pages = layout_document(&document, &options);
        // The three linked words (and the spaces between them) merge into one
        // clickable rectangle on one line.
        assert_eq!(pages[0].link_areas.len(), 1);
        let area = &pages[0].link_areas[0];
        assert_eq!(area.link, 1);
        assert!(area.width > 0.0 && area.height > 0.0);

        // Both headings are anchored, in document order, with their text as the
        // outline title; the h1 also carries its HTML id.
        let anchors = &pages[0].anchors;
        assert_eq!(anchors.len(), 2);
        assert_eq!(anchors[0].level, 1);
        assert_eq!(anchors[0].title, "The Report Title");
        assert_eq!(anchors[0].name.as_deref(), Some("top"));
        assert_eq!(anchors[1].level, 2);
        assert_eq!(anchors[1].title, "Details");
        assert!(anchors[0].y > anchors[1].y, "anchors descend down the page");
    }

    #[test]
    fn absolute_boxes_leave_flow_and_relative_preserves_it() {
        let document = crate::html::parse(
            "<style>.a { position:absolute; top:100pt; left:50pt; width:80pt; } \
                    .r { position:relative; left:30pt; }</style>\
             <p>first</p>\
             <div class=\"a\">stamp</div>\
             <p class=\"r\">nudged</p>\
             <p>after</p>",
        );
        let options = RenderOptions::default();
        let pages = layout_document(&document, &options);
        let lines = &pages[0].lines;
        let find = |t: &str| lines.iter().find(|l| l.text.contains(t)).unwrap();

        // Absolute: placed from the page top by its offsets, not at the cursor.
        let stamp = find("stamp");
        assert!((stamp.x - (options.margin_left + 50.0)).abs() < 1.0, "x {}", stamp.x);
        let page_top = options.page_size.height - options.margin_top;
        assert!(stamp.y < page_top - 100.0 && stamp.y > page_top - 130.0, "y {}", stamp.y);

        // Relative: shifted right, but "after" flows as if it had not moved —
        // the vertical gap between "first" and "after" matches two unshifted
        // paragraph advances regardless of the nudge.
        let nudged = find("nudged");
        assert!((nudged.x - (options.margin_left + 30.0)).abs() < 1.0);
        let first = find("first");
        let after = find("after");
        let gap_first_nudgedless = first.y - after.y;
        assert!(gap_first_nudgedless > 0.0, "after must sit below first");
    }

    #[test]
    fn bidi_reorders_rtl_pieces_within_a_line() {
        use super::{reorder_pieces_bidi, LinePiece};
        let piece = |text: &str| LinePiece {
            text: text.to_string(),
            font_size: 10.0,
            font: 0,
            color: Color::BLACK,
            bold: false,
            underline: false,
            line_through: false,
            link: 0,
            image: None,
        };
        // Logical: abc · אבג · space · דהו · xyz. The two Hebrew words and the
        // space between them form one RTL run, so they swap; Latin stays put.
        let pieces = vec![
            piece("abc "),
            piece("\u{05D0}\u{05D1}\u{05D2}"),
            piece(" "),
            piece("\u{05D3}\u{05D4}\u{05D5}"),
            piece(" xyz"),
        ];
        let visual: Vec<String> = reorder_pieces_bidi(pieces, false)
            .into_iter()
            .map(|p| p.text)
            .collect();
        assert_eq!(
            visual,
            vec!["abc ", "\u{05D3}\u{05D4}\u{05D5}", " ", "\u{05D0}\u{05D1}\u{05D2}", " xyz"]
        );

        // A purely-LTR line is untouched.
        let pieces = vec![piece("one "), piece("two")];
        let visual: Vec<String> = reorder_pieces_bidi(pieces, false).into_iter().map(|p| p.text).collect();
        assert_eq!(visual, vec!["one ", "two"]);

        // With an RTL base, two Hebrew words swap so the first word sits on the
        // right; an LTR word embedded between them keeps its own reading order.
        let pieces = vec![
            piece("\u{05D0}\u{05D1}"),
            piece(" World "),
            piece("\u{05D2}\u{05D3}"),
        ];
        let visual: Vec<String> =
            reorder_pieces_bidi(pieces, true).into_iter().map(|p| p.text).collect();
        assert_eq!(visual, vec!["\u{05D2}\u{05D3}", " World ", "\u{05D0}\u{05D1}"]);
    }

    #[test]
    fn rtl_paragraph_reorders_and_right_aligns() {
        // Two Hebrew words in a base-RTL paragraph: the first logical word must
        // paint furthest right, and the line must be right-aligned (its right
        // edge near the content edge, not the left margin).
        let document = crate::html::parse(
            "<p dir=\"rtl\">\u{05E9}\u{05DC}\u{05D5}\u{05DD} \u{05E2}\u{05D5}\u{05DC}\u{05DD}</p>",
        );
        let options = RenderOptions::default();
        let pages = layout_document(&document, &options);
        let first = pages[0].lines.iter().find(|l| l.text.contains('\u{05E9}')).unwrap();
        let second = pages[0].lines.iter().find(|l| l.text.contains('\u{05E2}')).unwrap();
        assert!(
            first.x > second.x,
            "first logical word sits to the right of the second"
        );
        // Right-aligned: the rightmost glyphs land well past the page midline.
        let mid = options.page_size.width / 2.0;
        assert!(first.x > mid, "rtl line is right-aligned (x={})", first.x);
    }

    #[test]
    fn fixed_boxes_repeat_on_every_page() {
        let mut html = String::from(
            "<style>.w { position: fixed; top: 10pt; right: 10pt; }</style>\
             <div class=\"w\">WATERMARK</div>",
        );
        for i in 0..120 {
            html.push_str(&format!("<p>filler paragraph number {i}</p>"));
        }
        let document = crate::html::parse(&html);
        let pages = layout_document(&document, &RenderOptions::default());

        assert!(pages.len() >= 2, "need a multi-page document, got {}", pages.len());
        for (index, page) in pages.iter().enumerate() {
            let stamped = page.lines.iter().any(|l| l.text.contains("WATERMARK"));
            assert!(stamped, "page {index} is missing the fixed watermark");
        }
        // And it paints at the same spot on every page.
        let ys: Vec<f32> = pages
            .iter()
            .map(|p| p.lines.iter().find(|l| l.text.contains("WATERMARK")).unwrap().y)
            .collect();
        assert!(ys.windows(2).all(|w| (w[0] - w[1]).abs() < 0.01), "{ys:?}");
    }

    #[test]
    fn z_index_orders_overlapping_positioned_boxes() {
        // `.low` comes later in the document but has the smaller z-index, so it
        // must paint first (underneath).
        let document = crate::html::parse(
            "<style>\
             .high { position: absolute; top: 100pt; left: 50pt; z-index: 5; }\
             .low  { position: absolute; top: 100pt; left: 50pt; z-index: 1; }\
             </style>\
             <p>flow</p>\
             <div class=\"high\">TOP</div>\
             <div class=\"low\">UNDER</div>",
        );
        let pages = layout_document(&document, &RenderOptions::default());
        let page = &pages[0];

        let index_of = |needle: &str| {
            page.commands
                .iter()
                .position(|c| matches!(c, PaintCommand::Text(t) if t.text.contains(needle)))
                .unwrap_or_else(|| panic!("missing {needle}"))
        };
        assert!(index_of("UNDER") < index_of("TOP"), "z-index must reorder painting");
        // Positioned content paints above in-flow content regardless of source order.
        assert!(index_of("flow") < index_of("UNDER"));
    }

    #[test]
    fn percent_width_max_width_and_margin_auto_centering() {
        let document = crate::html::parse(
            "<style>\
             .half { width: 50%; margin: 0 auto; }\
             .capped { max-width: 100pt; }\
             .pct { width: 25%; }\
             </style>\
             <div class=\"half\">centered</div>\
             <div class=\"pct\">quarter</div>\
             <div class=\"capped\">a capped div whose text is far wider than one \
             hundred points so it must wrap into several lines</div>",
        );
        let options = RenderOptions::default();
        let pages = layout_document(&document, &options);
        let lines = &pages[0].lines;
        let find = |t: &str| lines.iter().find(|l| l.text.contains(t)).unwrap();

        let content = options.page_size.width - options.margin_left - options.margin_right; // 499
        // width: 50% + margin auto → the box starts at the centering offset.
        let centered = find("centered");
        let expected_x = options.margin_left + (content - content * 0.5) / 2.0;
        assert!(
            (centered.x - expected_x).abs() < 1.0,
            "centered x {} vs expected {expected_x}",
            centered.x
        );
        // width: 25% without auto margins stays left-aligned.
        let quarter = find("quarter");
        assert!((quarter.x - options.margin_left).abs() < 1.0, "x {}", quarter.x);

        // max-width: 100pt wraps the long text: every capped line stays inside
        // 100pt, and there are several of them.
        let capped: Vec<_> = lines
            .iter()
            .filter(|l| {
                l.text.contains("capped")
                    || l.text.contains("hundred")
                    || l.text.contains("wrap")
            })
            .collect();
        assert!(capped.len() >= 2, "capped div must wrap");
        for line in &capped {
            let text_width = estimate_text_width(&line.text, line.font_size, &options.font);
            assert!(
                text_width <= 101.0,
                "line `{}` is {text_width}pt wide",
                line.text
            );
        }
    }

    #[test]
    fn absolute_resolves_against_positioned_ancestor() {
        let document = crate::html::parse(
            "<style>\
             .card { position: relative; margin-top: 100pt; margin-left: 60pt; }\
             .badge { position: absolute; top: 0; left: 0; margin: 0; }\
             </style>\
             <div class=\"card\"><p>card body</p><div class=\"badge\">BADGE</div></div>",
        );
        let options = RenderOptions::default();
        let pages = layout_document(&document, &options);
        let lines = &pages[0].lines;
        let find = |t: &str| lines.iter().find(|l| l.text.contains(t)).unwrap();

        let badge = find("BADGE");
        let body = find("card");
        // left:0 → the card's content edge, not the page margin.
        assert!((badge.x - (options.margin_left + 60.0)).abs() < 1.0, "x {}", badge.x);
        // top:0 → the card's top edge: the badge overlays the card's first line,
        // far below the page top it would sit at without the positioned ancestor.
        assert!((badge.y - body.y).abs() < 2.0, "badge y {} vs body y {}", badge.y, body.y);
    }

    #[test]
    fn float_bands_narrow_and_release_lines() {
        use super::{below_next_float, float_band_at, FloatBand};
        let floats = vec![
            FloatBand { left: true, x0: 48.0, x1: 148.0, top: 700.0, bottom: 600.0 },
            FloatBand { left: false, x0: 400.0, x1: 500.0, top: 700.0, bottom: 650.0 },
        ];
        // Inside both bands: text sits between the left float's right edge and
        // the right float's left edge.
        assert_eq!(float_band_at(&floats, 690.0, 48.0, 452.0), (148.0, 252.0));
        // Below the right float, only the left band still narrows the line.
        assert_eq!(float_band_at(&floats, 640.0, 48.0, 452.0), (148.0, 352.0));
        // Below both, the full width is back.
        assert_eq!(float_band_at(&floats, 590.0, 48.0, 452.0), (48.0, 452.0));
        // Clearing drops just below the matching float's bottom.
        assert_eq!(below_next_float(&floats, 690.0, Some(crate::html::Clear::Left)), 600.0);
        assert_eq!(below_next_float(&floats, 690.0, Some(crate::html::Clear::Right)), 650.0);
        assert_eq!(below_next_float(&floats, 690.0, Some(crate::html::Clear::Both)), 600.0);
        // Nothing active: unchanged.
        assert_eq!(below_next_float(&floats, 550.0, Some(crate::html::Clear::Both)), 550.0);
    }

    #[test]
    fn floated_block_narrows_following_text_lines() {
        let document = crate::html::parse(
            "<style>.f { float: left; width: 100pt; } </style>\
             <div class=\"f\">float</div>\
             <p>alpha beta gamma delta epsilon zeta eta theta iota kappa lambda mu \
             nu xi omicron pi rho sigma tau upsilon phi chi psi omega and yet more \
             words to make absolutely sure the paragraph wraps well past the float \
             so the last lines return to the left margin after it ends.</p>",
        );
        let options = RenderOptions::default();
        let pages = layout_document(&document, &options);
        let xs: Vec<f32> = pages[0].lines.iter().map(|l| l.x).collect();
        let margin = options.margin_left;
        // Some lines are pushed right of the float; later ones return to the margin.
        assert!(xs.iter().any(|&x| x > margin + 90.0), "no narrowed lines: {xs:?}");
        assert!(xs.iter().any(|&x| (x - margin).abs() < 0.5), "no full-width lines: {xs:?}");
    }

    #[test]
    fn justify_offsets_distribute_slack() {
        use crate::html::JustifyContent::*;
        // 3 items, 30pt slack, 10pt base gap.
        assert_eq!(justify_offsets(FlexStart, 30.0, 10.0, 3.0), (0.0, 10.0));
        assert_eq!(justify_offsets(FlexEnd, 30.0, 10.0, 3.0), (30.0, 10.0));
        assert_eq!(justify_offsets(Center, 30.0, 10.0, 3.0), (15.0, 10.0));
        // space-between: no leading, slack spread across the 2 gaps (+15 each).
        assert_eq!(justify_offsets(SpaceBetween, 30.0, 10.0, 3.0), (0.0, 25.0));
        // space-around: half-unit lead, full unit between (slack/3 = 10).
        assert_eq!(justify_offsets(SpaceAround, 30.0, 10.0, 3.0), (5.0, 20.0));
        // space-evenly: equal lead and between (slack/4 = 7.5).
        assert_eq!(justify_offsets(SpaceEvenly, 30.0, 10.0, 3.0), (7.5, 17.5));
    }

    #[test]
    fn creates_multiple_pages_for_long_documents() {
        use crate::box_tree::{BlockBox, BoxChild, FlowRoot, InlineRun};

        let children = (0..200)
            .map(|index| {
                BoxChild::Block(BlockBox {
                    kind: BlockKind::Paragraph,
                    margin: crate::box_tree::Edges::default(),
                    padding: crate::box_tree::Edges::default(),
                    align: crate::html::TextAlign::Left,
                    background: None,
                    border: None,
                    border_radius: 0.0,
                    border_box: false,
                    flex: None,
                    flex_grow: 0.0,
                    flex_basis: None,
                    grid: None,
                    grid_span: 1,
                    grid_col_start: None,
                    grid_col_end: None,
                    float_dir: None,
                    clear: None,
                    css_width: None,
                    css_width_percent: None,
                    max_width: None,
                    max_width_percent: None,
                    css_height: None,
                    min_width: None,
                    min_width_percent: None,
                    min_height: None,
                    max_height: None,
                    padding_percent: Default::default(),
                    margin_percent: Default::default(),
                    overflow_hidden: false,
                    center: false,
                    line_height: None,
                    rtl: false,
                    position: None,
                    z_index: None,
                    offset_top: None,
                    offset_right: None,
                    offset_bottom: None,
                    offset_left: None,
                    offset_percent: Default::default(),
                    anchor: None,
                    children: vec![BoxChild::Line(vec![InlineRun {
                        text: format!("Paragraph {index}"),
                        font_size: 11.0,
                        bold: false,
                        font: 0,
                        link: 0,
                        underline: false,
                        line_through: false,
                        color: Color::BLACK,
                        image: None,
                    }])],
                })
            })
            .collect();
        let document = Document {
            page_style: crate::html::PageStyle::default(),
            table_style: crate::html::TableStyle::default(),
            table_columns: Vec::new(),
            images: Vec::new(),
           font_specs: Vec::new(),
           font_faces: Vec::new(),
           links: Vec::new(),
            flow: Some(FlowRoot { children }),
            blocks: Vec::new(),
        };

        let pages = layout_document(&document, &RenderOptions::default());

        assert!(pages.len() > 1);
    }

    #[test]
    fn breaks_over_long_words_to_stay_on_page() {
        use crate::box_tree::{BlockBox, BoxChild, Edges, FlowRoot, InlineRun};

        // A single unbroken token far wider than the content width.
        let long = "M".repeat(400);
        let document = Document {
            page_style: crate::html::PageStyle::default(),
            table_style: crate::html::TableStyle::default(),
            table_columns: Vec::new(),
            images: Vec::new(),
            font_specs: Vec::new(),
            font_faces: Vec::new(),
            links: Vec::new(),
            flow: Some(FlowRoot {
                children: vec![BoxChild::Block(BlockBox {
                    kind: BlockKind::Paragraph,
                    margin: Edges::default(),
                    padding: Edges::default(),
                    align: crate::html::TextAlign::Left,
                    background: None,
                    border: None,
                    border_radius: 0.0,
                    border_box: false,
                    flex: None,
                    flex_grow: 0.0,
                    flex_basis: None,
                    grid: None,
                    grid_span: 1,
                    grid_col_start: None,
                    grid_col_end: None,
                    float_dir: None,
                    clear: None,
                    css_width: None,
                    css_width_percent: None,
                    max_width: None,
                    max_width_percent: None,
                    css_height: None,
                    min_width: None,
                    min_width_percent: None,
                    min_height: None,
                    max_height: None,
                    padding_percent: Default::default(),
                    margin_percent: Default::default(),
                    overflow_hidden: false,
                    center: false,
                    line_height: None,
                    rtl: false,
                    position: None,
                    z_index: None,
                    offset_top: None,
                    offset_right: None,
                    offset_bottom: None,
                    offset_left: None,
                    offset_percent: Default::default(),
                    anchor: None,
                    children: vec![BoxChild::Line(vec![InlineRun {
                        text: long,
                        font_size: 12.0,
                        bold: false,
                        font: 0,
                        link: 0,
                        underline: false,
                        line_through: false,
                        color: Color::BLACK,
                        image: None,
                    }])],
                })],
            }),
            blocks: Vec::new(),
        };

        let options = RenderOptions::default();
        let pages = layout_document(&document, &options);
        let content_width =
            options.page_size.width - options.margin_left - options.margin_right;

        let line_count: usize = pages.iter().map(|page| page.lines.len()).sum();
        assert!(line_count > 1, "the long word must break across lines");
        for page in &pages {
            for line in &page.lines {
                let width = estimate_text_width(&line.text, line.font_size, &options.font);
                assert!(
                    width <= content_width + 0.01,
                    "each broken line must fit the content width"
                );
            }
        }
    }

    #[test]
    fn collapses_adjacent_block_margins() {
        use crate::box_tree::{BlockBox, BoxChild, Edges, FlowRoot, InlineRun};

        let para = |text: &str| {
            BoxChild::Block(BlockBox {
                kind: BlockKind::Paragraph,
                margin: Edges {
                    top: 20.0,
                    right: 0.0,
                    bottom: 20.0,
                    left: 0.0,
                },
                padding: Edges::default(),
                align: crate::html::TextAlign::Left,
                background: None,
                border: None,
                border_radius: 0.0,
                border_box: false,
                flex: None,
                flex_grow: 0.0,
                flex_basis: None,
                grid: None,
                grid_span: 1,
                grid_col_start: None,
                grid_col_end: None,
                float_dir: None,
                clear: None,
                css_width: None,
                css_width_percent: None,
                max_width: None,
                max_width_percent: None,
                css_height: None,
                min_width: None,
                min_width_percent: None,
                min_height: None,
                max_height: None,
                padding_percent: Default::default(),
                margin_percent: Default::default(),
                overflow_hidden: false,
                center: false,
                line_height: None,
                rtl: false,
                position: None,
                z_index: None,
                offset_top: None,
                offset_right: None,
                offset_bottom: None,
                offset_left: None,
                offset_percent: Default::default(),
                anchor: None,
                children: vec![BoxChild::Line(vec![InlineRun {
                    text: text.to_string(),
                    font_size: 10.0,
                    bold: false,
                    font: 0,
                    link: 0,
                    underline: false,
                    line_through: false,
                    color: Color::BLACK,
                    image: None,
                }])],
            })
        };
        let document = Document {
            page_style: crate::html::PageStyle::default(),
            table_style: crate::html::TableStyle::default(),
            table_columns: Vec::new(),
            images: Vec::new(),
           font_specs: Vec::new(),
           font_faces: Vec::new(),
           links: Vec::new(),
            flow: Some(FlowRoot {
                children: vec![para("first"), para("second")],
            }),
            blocks: Vec::new(),
        };

        let pages = layout_document(&document, &RenderOptions::default());
        let lines = &pages[0].lines;
        assert_eq!(lines.len(), 2);

        let leading = 10.0 * 1.35;
        let gap = lines[0].y - lines[1].y;
        // Collapsed: gap = leading + max(20, 20) = leading + 20, NOT leading + 40.
        assert!(
            (gap - (leading + 20.0)).abs() < 0.01,
            "expected collapsed gap {}, got {gap}",
            leading + 20.0
        );
    }

    #[test]
    fn line_height_controls_line_spacing_and_half_leading() {
        use crate::box_tree::{BlockBox, BoxChild, Edges, FlowRoot, InlineRun};
        use crate::html::LineHeight;

        let render = |line_height: Option<LineHeight>| {
            let line = |text: &str| {
                BoxChild::Line(vec![InlineRun {
                    text: text.to_string(),
                    font_size: 10.0,
                    bold: false,
                    font: 0,
                    link: 0,
                    underline: false,
                    line_through: false,
                    color: Color::BLACK,
                    image: None,
                }])
            };
            let document = Document {
                page_style: crate::html::PageStyle::default(),
                table_style: crate::html::TableStyle::default(),
                table_columns: Vec::new(),
                images: Vec::new(),
                font_specs: Vec::new(),
                font_faces: Vec::new(),
                links: Vec::new(),
                flow: Some(FlowRoot {
                    children: vec![BoxChild::Block(BlockBox {
                        kind: BlockKind::Paragraph,
                        margin: Edges::default(),
                        padding: Edges::default(),
                        align: crate::html::TextAlign::Left,
                        background: None,
                        border: None,
                        border_radius: 0.0,
                        border_box: false,
                        flex: None,
                        flex_grow: 0.0,
                        flex_basis: None,
                        grid: None,
                        grid_span: 1,
                        grid_col_start: None,
                        grid_col_end: None,
                        float_dir: None,
                        clear: None,
                        css_width: None,
                        css_width_percent: None,
                        max_width: None,
                        max_width_percent: None,
                        css_height: None,
                        min_width: None,
                        min_width_percent: None,
                        min_height: None,
                        max_height: None,
                        padding_percent: Default::default(),
                        margin_percent: Default::default(),
                        overflow_hidden: false,
                        center: false,
                        line_height,
                        rtl: false,
                        position: None,
                        z_index: None,
                        offset_top: None,
                        offset_right: None,
                        offset_bottom: None,
                        offset_left: None,
                        offset_percent: Default::default(),
                        anchor: None,
                        children: vec![line("one"), line("two")],
                    })],
                }),
                blocks: Vec::new(),
            };
            layout_document(&document, &RenderOptions::default())
        };

        let default = render(None);
        let doubled = render(Some(LineHeight::Number(2.0)));
        let fixed = render(Some(LineHeight::Length(30.0)));

        let gap = |pages: &[super::Page]| pages[0].lines[0].y - pages[0].lines[1].y;
        assert!((gap(&default) - 13.5).abs() < 0.01, "default = 10 × 1.35");
        assert!((gap(&doubled) - 20.0).abs() < 0.01, "number scales the font");
        assert!((gap(&fixed) - 30.0).abs() < 0.01, "length is absolute");

        // Extra leading is split around the glyphs: with line-height 2.0 the
        // first baseline sits (20 − 13.5)/2 = 3.25pt lower than by default.
        let shift = default[0].lines[0].y - doubled[0].lines[0].y;
        assert!((shift - 3.25).abs() < 0.01, "half-leading shift, got {shift}");
    }

    #[test]
    fn paints_block_background_behind_text() {
        use crate::box_tree::{BlockBox, BoxChild, Edges, FlowRoot, InlineRun};

        let document = Document {
            page_style: crate::html::PageStyle::default(),
            table_style: crate::html::TableStyle::default(),
            table_columns: Vec::new(),
            images: Vec::new(),
            font_specs: Vec::new(),
            font_faces: Vec::new(),
            links: Vec::new(),
            flow: Some(FlowRoot {
                children: vec![BoxChild::Block(BlockBox {
                    kind: BlockKind::Paragraph,
                    margin: Edges::default(),
                    padding: Edges {
                        top: 6.0,
                        right: 6.0,
                        bottom: 6.0,
                        left: 6.0,
                    },
                    align: crate::html::TextAlign::Left,
                    background: Some(Color::from_rgb_u8(255, 0, 0)),
                    border: Some(crate::html::BorderEdges {
                        top: Some(TEST_BORDER_SIDE),
                        right: Some(TEST_BORDER_SIDE),
                        bottom: Some(TEST_BORDER_SIDE),
                        left: Some(TEST_BORDER_SIDE),
                    }),
                    border_radius: 0.0,
                    border_box: false,
                    flex: None,
                    flex_grow: 0.0,
                    flex_basis: None,
                    grid: None,
                    grid_span: 1,
                    grid_col_start: None,
                    grid_col_end: None,
                    float_dir: None,
                    clear: None,
                    css_width: None,
                    css_width_percent: None,
                    max_width: None,
                    max_width_percent: None,
                    css_height: None,
                    min_width: None,
                    min_width_percent: None,
                    min_height: None,
                    max_height: None,
                    padding_percent: Default::default(),
                    margin_percent: Default::default(),
                    overflow_hidden: false,
                    center: false,
                    line_height: None,
                    rtl: false,
                    position: None,
                    z_index: None,
                    offset_top: None,
                    offset_right: None,
                    offset_bottom: None,
                    offset_left: None,
                    offset_percent: Default::default(),
                    anchor: None,
                    children: vec![BoxChild::Line(vec![InlineRun {
                        text: "boxed".to_string(),
                        font_size: 11.0,
                        bold: false,
                        font: 0,
                        link: 0,
                        underline: false,
                        line_through: false,
                        color: Color::BLACK,
                        image: None,
                    }])],
                })],
            }),
            blocks: Vec::new(),
        };

        let pages = layout_document(&document, &RenderOptions::default());
        let commands = &pages[0].commands;

        // Background fill and border stroke both present...
        let fill = commands
            .iter()
            .position(|c| matches!(c, PaintCommand::FillRect(_)))
            .expect("background fill present");
        let stroke = commands
            .iter()
            .position(|c| matches!(c, PaintCommand::StrokeRect(_)))
            .expect("border stroke present");
        let text = commands
            .iter()
            .position(|c| matches!(c, PaintCommand::Text(_)))
            .expect("text present");

        // ...and both are painted before the text (i.e. behind it).
        assert!(fill < text, "background must paint behind text");
        assert!(stroke < text, "border must paint before text");
    }

    #[test]
    fn lays_out_table_rows_with_rects() {
        let document = Document {
            page_style: crate::html::PageStyle {
                orientation: crate::html::PageOrientation::Landscape,
                ..Default::default()
            },
            table_style: crate::html::TableStyle::default(),
            flow: None,
            table_columns: vec![30.0, 70.0],
            images: Vec::new(),
            font_specs: Vec::new(),
            font_faces: Vec::new(),
            links: Vec::new(),
            blocks: vec![Block {
                kind: BlockKind::TableRow,
                style: Default::default(),
                text: String::new(),
                cells: vec![
                    crate::html::TableCell {
                        text: "SL".to_string(),
                        colspan: 1,
                        font: 0,
                        runs: Vec::new(),
                        style: crate::html::CellStyle {
                            border: Some(true),
                            bold: true,
                            align: Some(crate::html::TextAlign::Center),
                            ..Default::default()
                        },
                    },
                    crate::html::TableCell {
                        text: "Name".to_string(),
                        colspan: 1,
                        font: 0,
                        runs: Vec::new(),
                        style: crate::html::CellStyle {
                            border: Some(true),
                            bold: true,
                            align: Some(crate::html::TextAlign::Left),
                            ..Default::default()
                        },
                    },
                ],
            }],
        };

        let pages = layout_document(
            &document,
            &RenderOptions::default().with_document_hints(&document),
        );

        assert_eq!(pages[0].lines.len(), 2);
        assert_eq!(pages[0].rects.len(), 2);
    }

    #[test]
    fn paints_table_cell_background_and_text_color() {
        let document = Document {
            page_style: crate::html::PageStyle::default(),
            table_style: crate::html::TableStyle::default(),
            flow: None,
            table_columns: vec![100.0],
            images: Vec::new(),
            font_specs: Vec::new(),
            font_faces: Vec::new(),
            links: Vec::new(),
            blocks: vec![Block {
                kind: BlockKind::TableRow,
                style: Default::default(),
                text: String::new(),
                cells: vec![crate::html::TableCell {
                    text: "Warning".to_string(),
                    colspan: 1,
                    font: 0,
                    runs: Vec::new(),
                    style: crate::html::CellStyle {
                        color: Some(Color::from_rgb_u8(0, 0, 255)),
                        background_color: Some(Color::from_rgb_u8(255, 0, 0)),
                        ..Default::default()
                    },
                }],
            }],
        };

        let pages = layout_document(&document, &RenderOptions::default());

        assert!(pages[0]
            .commands
            .contains(&PaintCommand::SetFillColor(Color::from_rgb_u8(255, 0, 0))));
        assert!(pages[0]
            .commands
            .contains(&PaintCommand::SetFillColor(Color::from_rgb_u8(0, 0, 255))));
    }

    #[test]
    fn applies_table_cell_vertical_alignment() {
        let document = Document {
            page_style: crate::html::PageStyle::default(),
            table_style: crate::html::TableStyle {
                row_height: Some(60.0),
            },
            flow: None,
            table_columns: vec![100.0, 100.0, 100.0],
            images: Vec::new(),
            font_specs: Vec::new(),
            font_faces: Vec::new(),
            links: Vec::new(),
            blocks: vec![Block {
                kind: BlockKind::TableRow,
                style: Default::default(),
                text: String::new(),
                cells: vec![
                    crate::html::TableCell {
                        text: "Top".to_string(),
                        colspan: 1,
                        font: 0,
                        runs: Vec::new(),
                        style: crate::html::CellStyle {
                            vertical_align: Some(crate::html::VerticalAlign::Top),
                            ..Default::default()
                        },
                    },
                    crate::html::TableCell {
                        text: "Middle".to_string(),
                        colspan: 1,
                        font: 0,
                        runs: Vec::new(),
                        style: crate::html::CellStyle {
                            vertical_align: Some(crate::html::VerticalAlign::Middle),
                            ..Default::default()
                        },
                    },
                    crate::html::TableCell {
                        text: "Bottom".to_string(),
                        colspan: 1,
                        font: 0,
                        runs: Vec::new(),
                        style: crate::html::CellStyle {
                            vertical_align: Some(crate::html::VerticalAlign::Bottom),
                            ..Default::default()
                        },
                    },
                ],
            }],
        };

        let pages = layout_document(
            &document,
            &RenderOptions::default().with_document_hints(&document),
        );
        let top_y = pages[0].lines[0].y;
        let middle_y = pages[0].lines[1].y;
        let bottom_y = pages[0].lines[2].y;

        assert!(top_y > middle_y);
        assert!(middle_y > bottom_y);
    }

    #[test]
    fn wraps_table_cell_text_and_grows_row_height() {
        let document = Document {
            page_style: crate::html::PageStyle::default(),
            table_style: crate::html::TableStyle::default(),
            flow: None,
            table_columns: Vec::new(),
            images: Vec::new(),
            font_specs: Vec::new(),
            font_faces: Vec::new(),
            links: Vec::new(),
            blocks: vec![Block {
                kind: BlockKind::TableRow,
                style: Default::default(),
                text: String::new(),
                // Long enough that its single-line width exceeds the page, so
                // auto layout must shrink the column and wrap it (min-content of
                // each individual word still fits).
                cells: vec![crate::html::TableCell {
                    text: "This single table cell contains far more words than could \
                           possibly fit on one line within the available page width so \
                           the automatic table layout has to wrap it across several lines"
                        .to_string(),
                    colspan: 1,
                    font: 0,
                    runs: Vec::new(),
                    style: crate::html::CellStyle {
                        border: Some(true),
                        bold: false,
                        align: Some(crate::html::TextAlign::Left),
                        ..Default::default()
                    },
                }],
            }],
        };

        let pages = layout_document(&document, &RenderOptions::default());

        assert!(pages[0].lines.len() > 1);
        assert!(pages[0].rects[0].height > 18.0);
    }

    #[test]
    fn uses_page_margins_and_css_row_height() {
        let document = Document {
            page_style: crate::html::PageStyle {
                margin_top: Some(54.0),
                margin_right: Some(18.0),
                margin_bottom: Some(54.0),
                margin_left: Some(18.0),
                ..Default::default()
            },
            table_style: crate::html::TableStyle {
                row_height: Some(15.0),
            },
            table_columns: vec![20.0, 200.0],
            images: Vec::new(),
            font_specs: Vec::new(),
            font_faces: Vec::new(),
            links: Vec::new(),
            flow: None,
            blocks: vec![Block {
                kind: BlockKind::TableRow,
                style: Default::default(),
                text: String::new(),
                cells: vec![crate::html::TableCell {
                    text: "A".to_string(),
                    colspan: 1,
                    font: 0,
                    runs: Vec::new(),
                    style: crate::html::CellStyle {
                        border: Some(true),
                        bold: false,
                        align: Some(crate::html::TextAlign::Left),
                        ..Default::default()
                    },
                }],
            }],
        };
        let options = RenderOptions::default().with_document_hints(&document);
        let pages = layout_document(&document, &options);

        assert_eq!(options.margin_top, 54.0);
        assert_eq!(options.margin_left, 18.0);
        assert_eq!(options.table_row_height, 15.0);
        assert_eq!(pages[0].rects[0].x, 18.0);
        assert!(pages[0].rects[0].height >= 15.0);
    }

    #[test]
    fn shrinks_to_fit_a_table_wider_than_the_page() {
        // A long unbreakable token cannot fit even at min-content, so the whole
        // table (columns + font) is scaled down to fit — shrink-to-fit, matching
        // a browser's print path, rather than clipping the data.
        let document = Document {
            page_style: crate::html::PageStyle::default(),
            table_style: crate::html::TableStyle::default(),
            flow: None,
            table_columns: Vec::new(),
            images: Vec::new(),
            font_specs: Vec::new(),
            font_faces: Vec::new(),
            links: Vec::new(),
            blocks: vec![Block {
                kind: BlockKind::TableRow,
                style: Default::default(),
                text: String::new(),
                cells: vec![crate::html::TableCell {
                    text: "1000055403@example.com".to_string(),
                    colspan: 1,
                    font: 0,
                    runs: Vec::new(),
                    style: crate::html::CellStyle {
                        border: Some(true),
                        font_size: Some(10.0),
                        ..Default::default()
                    },
                }],
            }],
        };
        let options = RenderOptions {
            page_size: PageSize {
                width: 80.0,
                height: 200.0,
            },
            margin: 10.0,
            margin_top: 10.0,
            margin_right: 10.0,
            margin_bottom: 10.0,
            margin_left: 10.0,
            table_row_height: 0.0,
            font: std::sync::Arc::new(crate::font::Font::helvetica()),
            base_dir: None,
            paper: crate::layout::Paper::A4,
            fonts: Vec::new(),
            links: Vec::new(),
            remote_images: crate::image::RemoteImagePolicy::default(),
        };

        let pages = layout_document(&document, &options);

        // Scaled down onto a single line (font < CSS size) fitting the column box.
        assert_eq!(pages[0].lines.len(), 1);
        assert!(pages[0].lines[0].font_size < 10.0);
        assert!(
            estimate_text_width(
                &pages[0].lines[0].text,
                pages[0].lines[0].font_size,
                &crate::font::Font::helvetica()
            ) <= pages[0].rects[0].width + 0.5
        );
    }

    #[test]
    fn honors_explicit_long_token_breaking() {
        let document = Document {
            page_style: crate::html::PageStyle::default(),
            table_style: crate::html::TableStyle::default(),
            flow: None,
            table_columns: vec![60.0],
            images: Vec::new(),
            font_specs: Vec::new(),
            font_faces: Vec::new(),
            links: Vec::new(),
            blocks: vec![Block {
                kind: BlockKind::TableRow,
                style: Default::default(),
                text: String::new(),
                cells: vec![crate::html::TableCell {
                    text: "1000055403@example.com".to_string(),
                    colspan: 1,
                    font: 0,
                    runs: Vec::new(),
                    style: crate::html::CellStyle {
                        border: Some(true),
                        font_size: Some(10.0),
                        overflow_wrap: Some(crate::html::OverflowWrap::Anywhere),
                        ..Default::default()
                    },
                }],
            }],
        };
        let options = RenderOptions {
            page_size: PageSize {
                width: 80.0,
                height: 200.0,
            },
            margin: 10.0,
            margin_top: 10.0,
            margin_right: 10.0,
            margin_bottom: 10.0,
            margin_left: 10.0,
            table_row_height: 18.0,
            font: std::sync::Arc::new(crate::font::Font::helvetica()),
            base_dir: None,
            paper: crate::layout::Paper::A4,
            fonts: Vec::new(),
            links: Vec::new(),
            remote_images: crate::image::RemoteImagePolicy::default(),
        };

        let pages = layout_document(&document, &options);
        let text_area_width = pages[0].rects[0].width - 4.0;

        assert!(pages[0].lines.len() > 1);
        assert!(pages[0]
            .lines
            .iter()
            .all(|line| estimate_text_width(&line.text, line.font_size, &crate::font::Font::helvetica()) <= text_area_width + 0.1));
        assert!(pages[0]
            .commands
            .iter()
            .any(|command| matches!(command, PaintCommand::PushClipRect(_))));
        assert!(pages[0]
            .commands
            .iter()
            .any(|command| matches!(command, PaintCommand::PopClip)));
    }

    #[test]
    fn scales_font_only_when_min_content_overflows_the_page() {
        // A single unbreakable word wider than the page: even min-content cannot
        // fit, so the table (columns + font) is scaled down uniformly to fit
        // (shrink-to-fit). Content that fits is not scaled (other tests).
        let long_word = "W".repeat(60);
        let document = Document {
            page_style: crate::html::PageStyle::default(),
            table_style: crate::html::TableStyle::default(),
            flow: None,
            table_columns: Vec::new(),
            images: Vec::new(),
            font_specs: Vec::new(),
            font_faces: Vec::new(),
            links: Vec::new(),
            blocks: vec![Block {
                kind: BlockKind::TableRow,
                style: Default::default(),
                text: String::new(),
                cells: vec![crate::html::TableCell {
                    text: long_word.clone(),
                    colspan: 1,
                    font: 0,
                    runs: Vec::new(),
                    style: crate::html::CellStyle {
                        font_size: Some(10.0),
                        ..Default::default()
                    },
                }],
            }],
        };
        let font = crate::font::Font::helvetica();
        let content_width = 200.0;
        let geometry = table_geometry(&document, content_width, &font);

        assert!(geometry.paint_scale < 1.0);
        assert!((geometry.columns.iter().sum::<f32>() - content_width).abs() < 0.5);

        let pages = layout_document(
            &document,
            &RenderOptions {
                page_size: PageSize {
                    width: 220.0,
                    height: 200.0,
                },
                margin: 10.0,
                margin_top: 10.0,
                margin_right: 10.0,
                margin_bottom: 10.0,
                margin_left: 10.0,
                table_row_height: 0.0,
                font: std::sync::Arc::new(font),
                base_dir: None,
                paper: crate::layout::Paper::A4,
                fonts: Vec::new(),
                links: Vec::new(),
                remote_images: crate::image::RemoteImagePolicy::default(),
            },
        );
        // The painted font is the CSS size times the shrink-to-fit scale.
        assert!((pages[0].lines[0].font_size - 10.0 * geometry.paint_scale).abs() < 0.01);
    }

    #[test]
    fn keeps_font_size_when_content_fits_the_page() {
        // Declared columns far wider than the page, but sparse content: auto
        // layout sizes columns to content and keeps the font at its CSS size,
        // rather than force-fitting the declared widths and shrinking text.
        let document = Document {
            page_style: crate::html::PageStyle::default(),
            table_style: crate::html::TableStyle::default(),
            flow: None,
            table_columns: vec![400.0, 400.0],
            images: Vec::new(),
            font_specs: Vec::new(),
            font_faces: Vec::new(),
            links: Vec::new(),
            blocks: vec![Block {
                kind: BlockKind::TableRow,
                style: Default::default(),
                text: String::new(),
                cells: vec![
                    crate::html::TableCell {
                        text: "A".to_string(),
                        colspan: 1,
                        font: 0,
                        runs: Vec::new(),
                        style: crate::html::CellStyle {
                            font_size: Some(11.0),
                            ..Default::default()
                        },
                    },
                    crate::html::TableCell {
                        text: "B".to_string(),
                        colspan: 1,
                        font: 0,
                        runs: Vec::new(),
                        style: crate::html::CellStyle {
                            font_size: Some(11.0),
                            ..Default::default()
                        },
                    },
                ],
            }],
        };
        let font = crate::font::Font::helvetica();
        let geometry = table_geometry(&document, 500.0, &font);
        assert_eq!(geometry.paint_scale, 1.0);
        assert!(geometry.columns.iter().sum::<f32>() < 500.0);
    }

    #[test]
    fn does_not_shrink_full_span_unbordered_caption_cells() {
        let document = Document {
            page_style: crate::html::PageStyle::default(),
            table_style: crate::html::TableStyle::default(),
            flow: None,
            table_columns: vec![100.0, 300.0],
            images: Vec::new(),
            font_specs: Vec::new(),
            font_faces: Vec::new(),
            links: Vec::new(),
            blocks: vec![Block {
                kind: BlockKind::TableRow,
                style: Default::default(),
                text: String::new(),
                cells: vec![crate::html::TableCell {
                    text: "Report title".to_string(),
                    colspan: 2,
                    font: 0,
                    runs: Vec::new(),
                    style: crate::html::CellStyle {
                        font_size: Some(12.0),
                        padding_left: Some(4.0),
                        border: None,
                        ..Default::default()
                    },
                }],
            }],
        };
        let pages = layout_document(
            &document,
            &RenderOptions {
                page_size: PageSize {
                    width: 220.0,
                    height: 200.0,
                },
                margin: 10.0,
                margin_top: 10.0,
                margin_right: 10.0,
                margin_bottom: 10.0,
                margin_left: 10.0,
                table_row_height: 18.0,
                font: std::sync::Arc::new(crate::font::Font::helvetica()),
                base_dir: None,
                paper: crate::layout::Paper::A4,
                fonts: Vec::new(),
                links: Vec::new(),
                remote_images: crate::image::RemoteImagePolicy::default(),
            },
        );

        assert_eq!(pages[0].lines[0].font_size, 12.0);
        assert_eq!(pages[0].lines[0].x, 14.0);
    }

    #[test]
    fn repeats_semantic_table_header_after_page_breaks() {
        let header_cells = vec![
            crate::html::TableCell {
                text: "SL".to_string(),
                colspan: 1,
                font: 0,
                runs: Vec::new(),
                style: crate::html::CellStyle {
                    border: Some(true),
                    bold: true,
                    ..Default::default()
                },
            },
            crate::html::TableCell {
                text: "Name".to_string(),
                colspan: 1,
                font: 0,
                runs: Vec::new(),
                style: crate::html::CellStyle {
                    border: Some(true),
                    bold: true,
                    ..Default::default()
                },
            },
        ];
        let mut blocks = vec![Block {
            kind: BlockKind::TableHeaderRow,
            style: Default::default(),
            text: String::new(),
            cells: header_cells,
        }];

        for index in 0..12 {
            blocks.push(Block {
                kind: BlockKind::TableRow,
                style: Default::default(),
                text: String::new(),
                cells: vec![
                    crate::html::TableCell {
                        text: index.to_string(),
                        colspan: 1,
                        font: 0,
                        runs: Vec::new(),
                        style: crate::html::CellStyle {
                            border: Some(true),
                            ..Default::default()
                        },
                    },
                    crate::html::TableCell {
                        text: format!("Student {index}"),
                        colspan: 1,
                        font: 0,
                        runs: Vec::new(),
                        style: crate::html::CellStyle {
                            border: Some(true),
                            ..Default::default()
                        },
                    },
                ],
            });
        }

        let document = Document {
            page_style: crate::html::PageStyle::default(),
            table_style: crate::html::TableStyle::default(),
            flow: None,
            table_columns: vec![20.0, 80.0],
            images: Vec::new(),
           font_specs: Vec::new(),
           font_faces: Vec::new(),
           links: Vec::new(),
            blocks,
        };
        let pages = layout_document(
            &document,
            &RenderOptions {
                page_size: PageSize {
                    width: 140.0,
                    height: 100.0,
                },
                margin: 10.0,
                margin_top: 10.0,
                margin_right: 10.0,
                margin_bottom: 10.0,
                margin_left: 10.0,
                table_row_height: 18.0,
                font: std::sync::Arc::new(crate::font::Font::helvetica()),
                base_dir: None,
                paper: crate::layout::Paper::A4,
                fonts: Vec::new(),
                links: Vec::new(),
                remote_images: crate::image::RemoteImagePolicy::default(),
            },
        );

        assert!(pages.len() > 1);
        assert!(pages
            .iter()
            .skip(1)
            .all(|page| page.lines.iter().any(|line| line.text == "Name")));
    }
}
