use crate::color::Color;
use crate::html::{
    BlockKind, Document, Overflow, OverflowWrap, PageOrientation, TableCell, TextAlign,
    VerticalAlign, WhiteSpace, WordBreak,
};
use crate::paint::{ImageCommand, PaintCommand, RectCommand, TextCommand};

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

        options
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Page {
    pub lines: Vec<Line>,
    pub rects: Vec<Rect>,
    pub commands: Vec<PaintCommand>,
}

impl Page {
    pub fn new() -> Self {
        Self {
            lines: Vec::new(),
            rects: Vec::new(),
            commands: Vec::new(),
        }
    }

    pub(crate) fn push_colored_line(&mut self, line: Line, color: Color, bold: bool) {
        self.commands.push(PaintCommand::SetFillColor(color));
        self.commands.push(PaintCommand::Text(TextCommand {
            text: line.text.clone(),
            x: line.x,
            y: line.y,
            font_size: line.font_size,
            bold,
        }));
        self.lines.push(line);
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

    layout_box_children(
        &flow.children,
        options.margin_left,
        content_width,
        TextAlign::Left,
        &mut pages,
        &mut y,
        &mut carried,
        options,
    );

    pages
}

/// Drop the carried (collapsed) margin into the page as vertical space.
fn flush_margin(y: &mut f32, carried: &mut f32) {
    *y -= *carried;
    *carried = 0.0;
}

#[allow(clippy::too_many_arguments)]
fn layout_box_children(
    children: &[crate::box_tree::BoxChild],
    x: f32,
    width: f32,
    align: TextAlign,
    pages: &mut Vec<Page>,
    y: &mut f32,
    carried: &mut f32,
    options: &RenderOptions,
) {
    use crate::box_tree::BoxChild;

    for child in children {
        match child {
            BoxChild::Block(block) => {
                layout_block_box(block, x, width, pages, y, carried, options)
            }
            BoxChild::Line(runs) => {
                // Content flushes any pending margin above it.
                flush_margin(y, carried);
                layout_line_box(runs, x, width, align, pages, y, options);
            }
            BoxChild::Image(image) => {
                flush_margin(y, carried);
                layout_image_box(image, x, width, pages, y, options);
            }
        }
    }
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

    // Scale down to the content width and to a full page's height if oversized,
    // preserving the aspect ratio.
    let page_height = options.page_size.height - options.margin_top - options.margin_bottom;
    let mut scale = 1.0_f32;
    if image.width > width {
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

fn layout_block_box(
    block: &crate::box_tree::BlockBox,
    x: f32,
    width: f32,
    pages: &mut Vec<Page>,
    y: &mut f32,
    carried: &mut f32,
    options: &RenderOptions,
) {
    // This block's top margin collapses with the margin carried from above.
    *carried = carried.max(block.margin.top);

    let decorated = block.border || block.background.is_some();
    let inner_x = x + block.margin.left + block.padding.left;
    let inner_width =
        (width - block.margin.left - block.margin.right - block.padding.left - block.padding.right)
            .max(1.0);

    // Top padding — or any border/background — is a barrier: it ends the collapse
    // and separates the block's margin from its first child's margin.
    if block.padding.top > 0.0 || decorated {
        flush_margin(y, carried);
        *y -= block.padding.top;
    }

    // Record the border box's top edge so its background/border can be painted
    // per page fragment once the content height is known.
    let start_page = pages.len() - 1;
    let start_index = pages[start_page].commands.len();
    let start_y = *y + block.padding.top;
    let box_x = x + block.margin.left;
    let box_width = (width - block.margin.left - block.margin.right).max(1.0);

    layout_box_children(
        &block.children,
        inner_x,
        inner_width,
        block.align,
        pages,
        y,
        carried,
        options,
    );

    // Bottom padding (or a border/background) likewise contains the last child's
    // margin rather than letting it collapse out of the box.
    if block.padding.bottom > 0.0 || decorated {
        flush_margin(y, carried);
        *y -= block.padding.bottom;
    }

    if decorated {
        let end_page = pages.len() - 1;
        let end_y = *y;
        paint_decorations(
            pages, options, block, start_page, start_index, start_y, end_page, end_y, box_x,
            box_width,
        );
    }

    // This block's bottom margin collapses with whatever is carried out of it.
    *carried = carried.max(block.margin.bottom);
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
        if let Some(color) = block.background {
            commands.push(PaintCommand::SetFillColor(color));
            commands.push(PaintCommand::FillRect(RectCommand {
                x,
                y: bottom,
                width,
                height,
            }));
        }
        if block.border {
            commands.push(PaintCommand::SetStrokeColor(Color::BLACK));
            commands.push(PaintCommand::StrokeRect(RectCommand {
                x,
                y: bottom,
                width,
                height,
            }));
        }

        let at = if page_index == start_page { start_index } else { 0 };
        pages[page_index].commands.splice(at..at, commands);
    }
}

/// Wrap one line box's runs to `width` and paint each visual line, honoring the
/// per-run font size and color and the block's text alignment.
fn layout_line_box(
    runs: &[crate::box_tree::InlineRun],
    x: f32,
    width: f32,
    align: TextAlign,
    pages: &mut Vec<Page>,
    y: &mut f32,
    options: &RenderOptions,
) {
    for visual in wrap_inline_runs(runs, width, &options.font) {
        let line_width: f32 = visual
            .iter()
            .map(|piece| estimate_text_width(&piece.text, piece.font_size, &options.font))
            .sum();
        // Leading follows the tallest run on the line.
        let max_font = visual
            .iter()
            .map(|piece| piece.font_size)
            .fold(0.0_f32, f32::max);
        let leading = max_font * 1.35;

        ensure_space(pages, y, options, leading);

        let mut px = match align {
            TextAlign::Left => x,
            TextAlign::Center => x + ((width - line_width) / 2.0).max(0.0),
            TextAlign::Right => x + (width - line_width).max(0.0),
        };

        let page = pages.last_mut().expect("at least one page exists");
        for piece in &visual {
            let piece_width = estimate_text_width(&piece.text, piece.font_size, &options.font);
            page.push_colored_line(
                Line {
                    text: piece.text.clone(),
                    x: px,
                    y: *y,
                    font_size: piece.font_size,
                    leading,
                },
                piece.color,
                piece.bold,
            );
            px += piece_width;
        }

        *y -= leading;
    }
}

/// A piece of a wrapped visual line: text in one style, positioned left-to-right.
struct LinePiece {
    text: String,
    font_size: f32,
    color: Color,
    bold: bool,
}

/// Wrap styled inline runs into visual lines. Whitespace is collapsed across run
/// boundaries (so `Hello <b>world</b>.` keeps a single space and no space before
/// the period), and words are placed greedily. A word wider than the whole line
/// is broken character-by-character as a last resort, so flow text never runs off
/// the page edge (a pragmatic deviation from CSS `overflow-wrap: normal`, which
/// would let it overflow — losing content is worse for paged output).
fn wrap_inline_runs(
    runs: &[crate::box_tree::InlineRun],
    max_width: f32,
    font: &crate::font::Font,
) -> Vec<Vec<LinePiece>> {
    let tokens = tokenize_runs(runs);

    let mut lines: Vec<Vec<LinePiece>> = Vec::new();
    let mut current: Vec<LinePiece> = Vec::new();
    let mut current_width = 0.0_f32;

    for token in tokens {
        let token_width: f32 = token
            .pieces
            .iter()
            .map(|piece| estimate_text_width(&piece.text, piece.font_size, font))
            .sum();

        // A token wider than the line can never fit by wrapping; start it on a
        // fresh line and break it across lines character by character. The small
        // tolerance avoids a spurious break when a word measures exactly the line
        // width (e.g. a column auto-sized to its own content) but a float ULP
        // tips the comparison.
        if token_width > max_width + WRAP_TOLERANCE {
            if !current.is_empty() {
                lines.push(std::mem::take(&mut current));
                current_width = 0.0;
            }
            break_long_token(&token.pieces, max_width, font, &mut lines, &mut current, &mut current_width);
            continue;
        }

        let space_width = if current.is_empty() {
            0.0
        } else {
            estimate_text_width(" ", token.space_font_size, font)
        };

        if !current.is_empty()
            && current_width + space_width + token_width > max_width + WRAP_TOLERANCE
        {
            lines.push(std::mem::take(&mut current));
            current_width = 0.0;
        }

        if !current.is_empty() {
            // Re-measure the separator at the (possibly new) line's leading run.
            let space_width = estimate_text_width(" ", token.space_font_size, font);
            current.push(LinePiece {
                text: " ".to_string(),
                font_size: token.space_font_size,
                color: token.pieces.first().map(|p| p.color).unwrap_or(Color::BLACK),
                bold: token.pieces.first().map(|p| p.bold).unwrap_or(false),
            });
            current_width += space_width;
        }

        for piece in token.pieces {
            current_width += estimate_text_width(&piece.text, piece.font_size, font);
            current.push(piece);
        }
    }

    if !current.is_empty() {
        lines.push(current);
    }

    lines
}

/// Break an over-long token across lines, one character at a time, preserving
/// each character's style. Appends to `current` (flushing full lines into
/// `lines`) and leaves any trailing partial line in `current` so following
/// content can continue on it.
fn break_long_token(
    pieces: &[LinePiece],
    max_width: f32,
    font: &crate::font::Font,
    lines: &mut Vec<Vec<LinePiece>>,
    current: &mut Vec<LinePiece>,
    current_width: &mut f32,
) {
    for piece in pieces {
        for ch in piece.text.chars() {
            let char_width = estimate_text_width(&ch.to_string(), piece.font_size, font);

            // Wrap before this character if the line already has content and the
            // character would overflow. A single character wider than the line is
            // still placed (on its own line) so we never loop forever.
            if !current.is_empty() && *current_width + char_width > max_width {
                lines.push(std::mem::take(current));
                *current_width = 0.0;
            }

            if let Some(last) = current.last_mut() {
                if last.font_size == piece.font_size
                    && last.color == piece.color
                    && last.bold == piece.bold
                {
                    last.text.push(ch);
                    *current_width += char_width;
                    continue;
                }
            }
            current.push(LinePiece {
                text: ch.to_string(),
                font_size: piece.font_size,
                color: piece.color,
                bold: piece.bold,
            });
            *current_width += char_width;
        }
    }
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
                if last.font_size == run.font_size
                    && last.color == run.color
                    && last.bold == run.bold
                {
                    last.text.push(ch);
                    continue;
                }
            }
            word.push(LinePiece {
                text: ch.to_string(),
                font_size: run.font_size,
                color: run.color,
                bold: run.bold,
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
    let planned_cells = plan_table_cells(cells, table_geometry, options.table_row_height, &options.font);
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
    let planned_cells = plan_table_cells(cells, table_geometry, options.table_row_height, &options.font);
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
            page.push_rect(Rect {
                x,
                y: *y - row_height,
                width: planned.width,
                height: row_height,
                stroke: true,
            });
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

        for (line_index, line) in planned.lines.iter().enumerate() {
            let text_width = estimate_text_width(line, planned.font_size, &options.font);
            let text_x = match planned.source.style.align.unwrap_or(TextAlign::Left) {
                TextAlign::Left => x + planned.padding_left,
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
                    leading: planned.leading,
                },
                text_color,
                planned.source.style.bold,
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
    let text_height = (planned.lines.len().max(1) as f32 * planned.leading).max(0.0);

    (content_height - text_height).max(0.0)
}

fn plan_table_cells<'a>(
    cells: &'a [TableCell],
    table_geometry: &TableGeometry,
    base_row_height: f32,
    font: &crate::font::Font,
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
        let leading = font_size * 1.18;
        let padding_left = cell.style.padding_left.unwrap_or(DEFAULT_CELL_PADDING) * paint_scale;
        let padding_right = cell.style.padding_right.unwrap_or(DEFAULT_CELL_PADDING) * paint_scale;
        let padding_top = cell.style.padding_top.unwrap_or(DEFAULT_CELL_PADDING) * paint_scale;
        let padding_bottom = cell.style.padding_bottom.unwrap_or(DEFAULT_CELL_PADDING) * paint_scale;
        let white_space = cell.style.white_space.unwrap_or(WhiteSpace::Normal);
        let break_long_tokens = should_break_long_tokens(cell);
        let lines = wrap_cell_text(
            &cell.text,
            width - padding_left - padding_right,
            font_size,
            if white_space == WhiteSpace::NoWrap {
                1
            } else {
                3
            },
            white_space,
            break_long_tokens,
            font,
        );
        let line_count = lines.len().max(1);
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
            font_size,
            leading,
            height,
            padding_left,
            padding_right,
            padding_top,
            padding_bottom,
            clip_content,
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
    font_size: f32,
    leading: f32,
    height: f32,
    padding_left: f32,
    padding_right: f32,
    padding_top: f32,
    padding_bottom: f32,
    clip_content: bool,
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

/// Default table-cell font size (pt) when the cascade sets none — the browser
/// default of ~11pt rather than a shrink-to-fit fudge.
const DEFAULT_CELL_FONT_SIZE: f32 = 11.0;
/// Default table-cell padding (pt) when the cascade sets none (≈ the 1px UA
/// default). Real spreadsheet exports set padding explicitly.
const DEFAULT_CELL_PADDING: f32 = 1.0;

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
fn table_geometry(document: &Document, content_width: f32, font: &crate::font::Font) -> TableGeometry {
    let column_count = document
        .blocks
        .iter()
        .filter(|block| is_table_row_kind(block.kind))
        .map(|block| block.cells.iter().map(|cell| cell.colspan.max(1)).sum::<usize>())
        .max()
        .unwrap_or(1)
        .max(1);

    let mut min_content = vec![0.0f32; column_count];
    let mut max_content = vec![0.0f32; column_count];
    // Cells spanning multiple columns constrain the spanned columns' totals.
    let mut spans: Vec<(usize, usize, f32, f32)> = Vec::new();

    for block in document.blocks.iter().filter(|b| is_table_row_kind(b.kind)) {
        let mut col = 0;
        for cell in &block.cells {
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
    let declared = &document.table_columns;
    let declared_total: f32 = declared.iter().sum();
    let use_declared = declared.len() == column_count
        && declared_total > 0.0
        && declared_total <= content_width
        && declared
            .iter()
            .zip(&min_content)
            .all(|(d, m)| *d + 0.5 >= *m);
    let upper: Vec<f32> = if use_declared {
        declared.clone()
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

    use super::{estimate_text_width, layout_document, table_geometry, PageSize, RenderOptions};

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
                    border: false,
                    children: vec![BoxChild::Line(vec![InlineRun {
                        text: format!("Paragraph {index}"),
                        font_size: 11.0,
                        bold: false,
                        color: Color::BLACK,
                    }])],
                })
            })
            .collect();
        let document = Document {
            page_style: crate::html::PageStyle::default(),
            table_style: crate::html::TableStyle::default(),
            table_columns: Vec::new(),
            images: Vec::new(),
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
            flow: Some(FlowRoot {
                children: vec![BoxChild::Block(BlockBox {
                    kind: BlockKind::Paragraph,
                    margin: Edges::default(),
                    padding: Edges::default(),
                    align: crate::html::TextAlign::Left,
                    background: None,
                    border: false,
                    children: vec![BoxChild::Line(vec![InlineRun {
                        text: long,
                        font_size: 12.0,
                        bold: false,
                        color: Color::BLACK,
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
                border: false,
                children: vec![BoxChild::Line(vec![InlineRun {
                    text: text.to_string(),
                    font_size: 10.0,
                    bold: false,
                    color: Color::BLACK,
                }])],
            })
        };
        let document = Document {
            page_style: crate::html::PageStyle::default(),
            table_style: crate::html::TableStyle::default(),
            table_columns: Vec::new(),
            images: Vec::new(),
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
    fn paints_block_background_behind_text() {
        use crate::box_tree::{BlockBox, BoxChild, Edges, FlowRoot, InlineRun};

        let document = Document {
            page_style: crate::html::PageStyle::default(),
            table_style: crate::html::TableStyle::default(),
            table_columns: Vec::new(),
            images: Vec::new(),
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
                    border: true,
                    children: vec![BoxChild::Line(vec![InlineRun {
                        text: "boxed".to_string(),
                        font_size: 11.0,
                        bold: false,
                        color: Color::BLACK,
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
            blocks: vec![Block {
                kind: BlockKind::TableRow,
                style: Default::default(),
                text: String::new(),
                cells: vec![
                    crate::html::TableCell {
                        text: "SL".to_string(),
                        colspan: 1,
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
            blocks: vec![Block {
                kind: BlockKind::TableRow,
                style: Default::default(),
                text: String::new(),
                cells: vec![crate::html::TableCell {
                    text: "Warning".to_string(),
                    colspan: 1,
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
            blocks: vec![Block {
                kind: BlockKind::TableRow,
                style: Default::default(),
                text: String::new(),
                cells: vec![
                    crate::html::TableCell {
                        text: "Top".to_string(),
                        colspan: 1,
                        style: crate::html::CellStyle {
                            vertical_align: Some(crate::html::VerticalAlign::Top),
                            ..Default::default()
                        },
                    },
                    crate::html::TableCell {
                        text: "Middle".to_string(),
                        colspan: 1,
                        style: crate::html::CellStyle {
                            vertical_align: Some(crate::html::VerticalAlign::Middle),
                            ..Default::default()
                        },
                    },
                    crate::html::TableCell {
                        text: "Bottom".to_string(),
                        colspan: 1,
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
            flow: None,
            blocks: vec![Block {
                kind: BlockKind::TableRow,
                style: Default::default(),
                text: String::new(),
                cells: vec![crate::html::TableCell {
                    text: "A".to_string(),
                    colspan: 1,
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
            blocks: vec![Block {
                kind: BlockKind::TableRow,
                style: Default::default(),
                text: String::new(),
                cells: vec![crate::html::TableCell {
                    text: "1000055403@example.com".to_string(),
                    colspan: 1,
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
            blocks: vec![Block {
                kind: BlockKind::TableRow,
                style: Default::default(),
                text: String::new(),
                cells: vec![crate::html::TableCell {
                    text: "1000055403@example.com".to_string(),
                    colspan: 1,
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
            blocks: vec![Block {
                kind: BlockKind::TableRow,
                style: Default::default(),
                text: String::new(),
                cells: vec![crate::html::TableCell {
                    text: long_word.clone(),
                    colspan: 1,
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
            blocks: vec![Block {
                kind: BlockKind::TableRow,
                style: Default::default(),
                text: String::new(),
                cells: vec![
                    crate::html::TableCell {
                        text: "A".to_string(),
                        colspan: 1,
                        style: crate::html::CellStyle {
                            font_size: Some(11.0),
                            ..Default::default()
                        },
                    },
                    crate::html::TableCell {
                        text: "B".to_string(),
                        colspan: 1,
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
            blocks: vec![Block {
                kind: BlockKind::TableRow,
                style: Default::default(),
                text: String::new(),
                cells: vec![crate::html::TableCell {
                    text: "Report title".to_string(),
                    colspan: 2,
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
                style: crate::html::CellStyle {
                    border: Some(true),
                    bold: true,
                    ..Default::default()
                },
            },
            crate::html::TableCell {
                text: "Name".to_string(),
                colspan: 1,
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
                        style: crate::html::CellStyle {
                            border: Some(true),
                            ..Default::default()
                        },
                    },
                    crate::html::TableCell {
                        text: format!("Student {index}"),
                        colspan: 1,
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
            },
        );

        assert!(pages.len() > 1);
        assert!(pages
            .iter()
            .skip(1)
            .all(|page| page.lines.iter().any(|line| line.text == "Name")));
    }
}
