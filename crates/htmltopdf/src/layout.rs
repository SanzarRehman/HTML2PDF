use crate::color::Color;
use crate::html::{
    BlockKind, Document, Overflow, OverflowWrap, PageOrientation, TableCell, TextAlign,
    VerticalAlign, WhiteSpace, WordBreak,
};
use crate::paint::{PaintCommand, RectCommand, TextCommand};

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
            table_row_height: 18.0,
            font: std::sync::Arc::new(crate::font::Font::helvetica()),
        }
    }
}

impl RenderOptions {
    /// Load and use `source` as the document font (measured and embedded).
    pub fn with_font(mut self, source: &crate::font::FontSource) -> Result<Self, String> {
        self.font = std::sync::Arc::new(crate::font::Font::load(source)?);
        Ok(self)
    }

    pub fn with_document_hints(&self, document: &Document) -> Self {
        let mut options = self.clone();

        if document.page_style.orientation == PageOrientation::Landscape {
            options.page_size = PageSize::A4_LANDSCAPE;
        }

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

    pub(crate) fn push_colored_line(&mut self, line: Line, color: Color) {
        self.commands.push(PaintCommand::SetFillColor(color));
        self.commands.push(PaintCommand::Text(TextCommand {
            text: line.text.clone(),
            x: line.x,
            y: line.y,
            font_size: line.font_size,
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
    let mut pages = vec![Page::new()];
    let mut y = options.page_size.height - options.margin_top;
    let content_width = options.page_size.width - options.margin_left - options.margin_right;
    let table_geometry = table_geometry(document, content_width);
    let mut repeated_table_header: Option<Vec<TableCell>> = None;

    for block in &document.blocks {
        if is_table_row_kind(block.kind) {
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
            continue;
        }

        // Computed style overrides the per-kind defaults where CSS set a value.
        let font_size = block.style.font_size.unwrap_or(font_size_for(block.kind));
        let leading = font_size * 1.35;
        let before = spacing_before(block.kind);
        let after = spacing_after(block.kind);
        let color = block.style.color.unwrap_or(Color::BLACK);
        let align = block.style.align.unwrap_or(TextAlign::Left);

        y -= before;
        ensure_space(&mut pages, &mut y, options, leading);

        for line in wrap_text(&block.text, content_width, font_size, &options.font) {
            ensure_space(&mut pages, &mut y, options, leading);

            let text_width = estimate_text_width(&line, font_size, &options.font);
            let x = match align {
                TextAlign::Left => options.margin_left,
                TextAlign::Center => {
                    options.margin_left + ((content_width - text_width) / 2.0).max(0.0)
                }
                TextAlign::Right => {
                    options.margin_left + (content_width - text_width).max(0.0)
                }
            };

            let page = pages.last_mut().expect("at least one page exists");
            page.push_colored_line(
                Line {
                    text: line,
                    x,
                    y,
                    font_size,
                    leading,
                },
                color,
            );

            y -= leading;
        }

        y -= after;
    }

    pages
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
        .fold(options.table_row_height, f32::max);

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
        .fold(options.table_row_height, f32::max);

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

        if planned.source.style.border {
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
        let paint_scale = cell_paint_scale(cell, table_geometry);
        let font_size = cell
            .style
            .font_size
            .unwrap_or(if cell.style.bold { 8.5 } else { 7.0 })
            * paint_scale;
        let leading = font_size * 1.18;
        let padding_left = cell.style.padding_left.unwrap_or(2.0) * paint_scale;
        let padding_right = cell.style.padding_right.unwrap_or(2.0) * paint_scale;
        let padding_top = cell.style.padding_top.unwrap_or(3.0) * paint_scale;
        let padding_bottom = cell.style.padding_bottom.unwrap_or(4.0) * paint_scale;
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
        let height =
            ((line_count as f32 * leading) + padding_top + padding_bottom).max(base_row_height);
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

fn cell_paint_scale(cell: &TableCell, table_geometry: &TableGeometry) -> f32 {
    let spans_all_columns = cell.colspan >= table_geometry.columns.len();

    if spans_all_columns && !cell.style.border {
        1.0
    } else {
        table_geometry.paint_scale
    }
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
    paint_scale: f32,
}

fn table_geometry(document: &Document, content_width: f32) -> TableGeometry {
    let mut columns = document.table_columns.clone();
    let has_declared_columns = !columns.is_empty();

    if columns.is_empty() {
        let max_cells = document
            .blocks
            .iter()
            .filter(|block| is_table_row_kind(block.kind))
            .map(|block| block.cells.iter().map(|cell| cell.colspan).sum::<usize>())
            .max()
            .unwrap_or(1);
        columns = vec![content_width / max_cells as f32; max_cells];
    }

    let total = columns.iter().sum::<f32>();
    if total <= 0.0 {
        return TableGeometry {
            columns,
            paint_scale: 1.0,
        };
    }

    let column_scale = content_width / total;
    let paint_scale = if has_declared_columns {
        column_scale.min(1.0)
    } else {
        1.0
    };

    TableGeometry {
        columns: columns.iter().map(|width| width * column_scale).collect(),
        paint_scale,
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

fn wrap_text(text: &str, max_width: f32, font_size: f32, font: &crate::font::Font) -> Vec<String> {
    wrap_text_with_mode(text, max_width, font_size, WhiteSpace::Normal, false, font)
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

        if break_long_tokens && word_width > max_width {
            if !current.is_empty() {
                lines.push(std::mem::take(&mut current));
                current_width = 0.0;
            }

            lines.extend(split_long_word(word, max_width, font_size, font));
            continue;
        }

        if !current.is_empty() && current_width + space_width + word_width > max_width {
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

fn font_size_for(kind: BlockKind) -> f32 {
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

fn spacing_before(kind: BlockKind) -> f32 {
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

fn spacing_after(kind: BlockKind) -> f32 {
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
        let document = Document {
            page_style: crate::html::PageStyle::default(),
            table_style: crate::html::TableStyle::default(),
            table_columns: Vec::new(),
            blocks: (0..200)
                .map(|index| Block {
                    kind: BlockKind::Paragraph,
                    style: Default::default(),
                    text: format!("Paragraph {index}"),
                    cells: Vec::new(),
                })
                .collect(),
        };

        let pages = layout_document(&document, &RenderOptions::default());

        assert!(pages.len() > 1);
    }

    #[test]
    fn lays_out_table_rows_with_rects() {
        let document = Document {
            page_style: crate::html::PageStyle {
                orientation: crate::html::PageOrientation::Landscape,
                ..Default::default()
            },
            table_style: crate::html::TableStyle::default(),
            table_columns: vec![30.0, 70.0],
            blocks: vec![Block {
                kind: BlockKind::TableRow,
                style: Default::default(),
                text: String::new(),
                cells: vec![
                    crate::html::TableCell {
                        text: "SL".to_string(),
                        colspan: 1,
                        style: crate::html::CellStyle {
                            border: true,
                            bold: true,
                            align: Some(crate::html::TextAlign::Center),
                            ..Default::default()
                        },
                    },
                    crate::html::TableCell {
                        text: "Name".to_string(),
                        colspan: 1,
                        style: crate::html::CellStyle {
                            border: true,
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
            table_columns: vec![100.0],
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
            table_columns: vec![100.0, 100.0, 100.0],
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
            table_columns: vec![20.0, 200.0],
            blocks: vec![Block {
                kind: BlockKind::TableRow,
                style: Default::default(),
                text: String::new(),
                cells: vec![crate::html::TableCell {
                    text: "This cell has enough words to wrap into multiple lines".to_string(),
                    colspan: 1,
                    style: crate::html::CellStyle {
                        border: true,
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
            blocks: vec![Block {
                kind: BlockKind::TableRow,
                style: Default::default(),
                text: String::new(),
                cells: vec![crate::html::TableCell {
                    text: "A".to_string(),
                    colspan: 1,
                    style: crate::html::CellStyle {
                        border: true,
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
    fn clips_long_table_tokens_without_forced_breaking() {
        let document = Document {
            page_style: crate::html::PageStyle::default(),
            table_style: crate::html::TableStyle::default(),
            table_columns: vec![60.0],
            blocks: vec![Block {
                kind: BlockKind::TableRow,
                style: Default::default(),
                text: String::new(),
                cells: vec![crate::html::TableCell {
                    text: "1000055403@example.com".to_string(),
                    colspan: 1,
                    style: crate::html::CellStyle {
                        border: true,
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
            table_row_height: 18.0,
            font: std::sync::Arc::new(crate::font::Font::helvetica()),
        };

        let pages = layout_document(&document, &options);
        let text_area_width = pages[0].rects[0].width - 4.0;

        assert_eq!(pages[0].lines.len(), 1);
        assert!(
            estimate_text_width(&pages[0].lines[0].text, pages[0].lines[0].font_size, &crate::font::Font::helvetica())
                > text_area_width
        );
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
    fn honors_explicit_long_token_breaking() {
        let document = Document {
            page_style: crate::html::PageStyle::default(),
            table_style: crate::html::TableStyle::default(),
            table_columns: vec![60.0],
            blocks: vec![Block {
                kind: BlockKind::TableRow,
                style: Default::default(),
                text: String::new(),
                cells: vec![crate::html::TableCell {
                    text: "1000055403@example.com".to_string(),
                    colspan: 1,
                    style: crate::html::CellStyle {
                        border: true,
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
    fn scales_table_paint_when_declared_columns_are_wider_than_page() {
        let document = Document {
            page_style: crate::html::PageStyle::default(),
            table_style: crate::html::TableStyle::default(),
            table_columns: vec![100.0, 300.0],
            blocks: vec![Block {
                kind: BlockKind::TableRow,
                style: Default::default(),
                text: String::new(),
                cells: vec![crate::html::TableCell {
                    text: "Wide".to_string(),
                    colspan: 1,
                    style: crate::html::CellStyle {
                        font_size: Some(10.0),
                        padding_left: Some(4.0),
                        ..Default::default()
                    },
                }],
            }],
        };
        let geometry = table_geometry(&document, 200.0);
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
            },
        );

        assert_eq!(geometry.columns, vec![50.0, 150.0]);
        assert_eq!(geometry.paint_scale, 0.5);
        assert_eq!(pages[0].lines[0].font_size, 5.0);
        assert_eq!(pages[0].lines[0].x, 12.0);
    }

    #[test]
    fn does_not_shrink_full_span_unbordered_caption_cells() {
        let document = Document {
            page_style: crate::html::PageStyle::default(),
            table_style: crate::html::TableStyle::default(),
            table_columns: vec![100.0, 300.0],
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
                        border: false,
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
                    border: true,
                    bold: true,
                    ..Default::default()
                },
            },
            crate::html::TableCell {
                text: "Name".to_string(),
                colspan: 1,
                style: crate::html::CellStyle {
                    border: true,
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
                            border: true,
                            ..Default::default()
                        },
                    },
                    crate::html::TableCell {
                        text: format!("Student {index}"),
                        colspan: 1,
                        style: crate::html::CellStyle {
                            border: true,
                            ..Default::default()
                        },
                    },
                ],
            });
        }

        let document = Document {
            page_style: crate::html::PageStyle::default(),
            table_style: crate::html::TableStyle::default(),
            table_columns: vec![20.0, 80.0],
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
            },
        );

        assert!(pages.len() > 1);
        assert!(pages
            .iter()
            .skip(1)
            .all(|page| page.lines.iter().any(|line| line.text == "Name")));
    }
}
