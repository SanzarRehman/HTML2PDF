use std::collections::HashMap;

use cssparser::{
    AtRuleParser, CowRcStr, DeclarationParser, ParseError, Parser, ParserInput, ParserState,
    QualifiedRuleParser, RuleBodyItemParser, RuleBodyParser, StyleSheetParser, Token,
};

use crate::color::Color;

#[derive(Debug, Clone, PartialEq)]
pub struct Document {
    /// Table rows (the spreadsheet/table path). Empty for non-table documents.
    pub blocks: Vec<Block>,
    /// The flow box tree (headings/paragraphs/lists). `Some` only for non-table
    /// documents; the table path leaves it `None`.
    pub flow: Option<crate::box_tree::FlowRoot>,
    pub page_style: PageStyle,
    pub table_style: TableStyle,
    pub table_columns: Vec<f32>,
    /// The document's image table, indexed by `ImageBox::image_index`. Empty
    /// until [`resolve_images`] runs.
    pub images: Vec<crate::image::DecodedImage>,
    /// Font requirements interned while building the box tree, indexed by
    /// `InlineRun::font` / `TableCell::font`. Index 0 is always the default
    /// spec (document font, regular). Resolved to faces once per render.
    pub font_specs: Vec<crate::font::FontSpec>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Block {
    pub kind: BlockKind,
    pub text: String,
    pub cells: Vec<TableCell>,
    /// Computed style for flow blocks (headings/paragraphs). Table-row blocks
    /// carry styles on their cells instead and leave this at the default.
    pub style: CellStyle,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockKind {
    Heading1,
    Heading2,
    Heading3,
    Heading4,
    Heading5,
    Heading6,
    Paragraph,
    TableHeaderRow,
    TableRow,
    TableFooterRow,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TableCell {
    pub text: String,
    pub colspan: usize,
    pub style: CellStyle,
    /// Interned font-spec index into `Document::font_specs` (0 = default),
    /// assigned in a post-pass over the built document.
    pub font: u16,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CellStyle {
    pub align: Option<TextAlign>,
    pub vertical_align: Option<VerticalAlign>,
    pub bold: bool,
    /// `text-decoration: underline` / `line-through`. Not inherited in CSS, but
    /// the flag is propagated to descendant inline runs so the decoration spans
    /// them (matching how a browser paints it over inline descendants).
    pub underline: bool,
    pub line_through: bool,
    /// Whether the box draws a border. `None` means unset (so a more specific
    /// `border: none` can override a less specific border rule in the cascade).
    pub border: Option<bool>,
    pub overflow: Option<Overflow>,
    pub font_size: Option<f32>,
    /// First usable family from CSS `font-family` (inherited): a concrete name
    /// or a generic keyword (`serif`, `monospace`, …). `None` = document font.
    pub font_family: Option<String>,
    /// CSS `font-style` (inherited): `Some(true)` = italic/oblique,
    /// `Some(false)` = an explicit `normal` (overrides an inherited italic).
    pub italic: Option<bool>,
    /// CSS `line-height` (inherited). `None` = `normal` (UA default leading).
    pub line_height: Option<LineHeight>,
    /// CSS `width`/`height` in points. Currently consumed only by `<img>` sizing;
    /// table column/row geometry uses a separate parse.
    pub width: Option<f32>,
    pub height: Option<f32>,
    pub padding_left: Option<f32>,
    pub padding_right: Option<f32>,
    pub padding_top: Option<f32>,
    pub padding_bottom: Option<f32>,
    pub margin_left: Option<f32>,
    pub margin_right: Option<f32>,
    pub margin_top: Option<f32>,
    pub margin_bottom: Option<f32>,
    pub white_space: Option<WhiteSpace>,
    pub overflow_wrap: Option<OverflowWrap>,
    pub word_break: Option<WordBreak>,
    pub color: Option<Color>,
    pub background_color: Option<Color>,
    /// `display: flex` — this element establishes a flex container.
    pub display_flex: bool,
    pub flex_direction: Option<FlexDirection>,
    pub justify_content: Option<JustifyContent>,
    pub align_items: Option<AlignItems>,
    /// `gap` / `column-gap` (points) between flex items.
    pub gap: Option<f32>,
    /// Flex item properties (meaningful when the parent is a flex container).
    pub flex_grow: Option<f32>,
    /// `flex-basis` in points; `None` = `auto` (use the item's content size).
    pub flex_basis: Option<f32>,
    /// `display: grid` — this element establishes a grid container.
    pub display_grid: bool,
    /// `grid-template-columns` track list (`None` = single auto column).
    pub grid_template: Option<Vec<GridTrack>>,
    /// `row-gap` (or the first value of a two-value `gap`), points.
    pub row_gap: Option<f32>,
    /// `grid-column: span N` on a grid item.
    pub grid_span: Option<usize>,
    /// CSS `float` (left/right); `None` = not floated.
    pub float_dir: Option<FloatDir>,
    /// CSS `clear`.
    pub clear: Option<Clear>,
    /// CSS `position` (static when `None`) and its box offsets, points.
    pub position: Option<PositionKind>,
    /// CSS `z-index` (`None` = `auto`), meaningful on positioned boxes.
    pub z_index: Option<i32>,
    pub offset_top: Option<f32>,
    pub offset_right: Option<f32>,
    pub offset_bottom: Option<f32>,
    pub offset_left: Option<f32>,
}

impl Default for CellStyle {
    fn default() -> Self {
        Self {
            align: None,
            vertical_align: None,
            bold: false,
            underline: false,
            line_through: false,
            border: None,
            overflow: None,
            font_size: None,
            font_family: None,
            italic: None,
            line_height: None,
            width: None,
            height: None,
            padding_left: None,
            padding_right: None,
            padding_top: None,
            padding_bottom: None,
            margin_left: None,
            margin_right: None,
            margin_top: None,
            margin_bottom: None,
            white_space: None,
            overflow_wrap: None,
            word_break: None,
            color: None,
            background_color: None,
            display_flex: false,
            flex_direction: None,
            justify_content: None,
            align_items: None,
            gap: None,
            flex_grow: None,
            flex_basis: None,
            display_grid: false,
            grid_template: None,
            row_gap: None,
            grid_span: None,
            float_dir: None,
            clear: None,
            position: None,
            z_index: None,
            offset_top: None,
            offset_right: None,
            offset_bottom: None,
            offset_left: None,
        }
    }
}

/// A cascaded CSS `line-height` value. `normal` is represented as the absence
/// of a value (`None` in [`CellStyle::line_height`]).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum LineHeight {
    /// Unitless number or percentage: a multiple of the element's font size,
    /// re-resolved against each descendant's own font size (how CSS inherits a
    /// number — we approximate the `%`/`em` compute-then-inherit rule the same
    /// way, which matches when font sizes don't change mid-subtree).
    Number(f32),
    /// Absolute length in points.
    Length(f32),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextAlign {
    Left,
    Center,
    Right,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VerticalAlign {
    Top,
    Middle,
    Bottom,
    Baseline,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Overflow {
    Visible,
    Hidden,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WhiteSpace {
    Normal,
    NoWrap,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OverflowWrap {
    Normal,
    Anywhere,
    BreakWord,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WordBreak {
    Normal,
    BreakAll,
}

/// CSS `position` scheme (static is `None` on the style).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PositionKind {
    /// Offset visually by top/left/right/bottom; flow position is preserved.
    Relative,
    /// Out of flow, positioned against the page content box (first pass —
    /// positioned-ancestor containing blocks are not tracked yet).
    Absolute,
    /// Treated as absolute against the current page (not yet repeated on
    /// every page the way print engines repeat `fixed` content).
    Fixed,
}

/// CSS `float` direction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FloatDir {
    Left,
    Right,
}

/// CSS `clear`: which floated sides a block must drop below.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Clear {
    Left,
    Right,
    Both,
}

/// One track of a `grid-template-columns` list: a fixed length (points), a
/// fraction of the free space (`fr`), or content-sized (`auto`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum GridTrack {
    Pt(f32),
    Fr(f32),
    Auto,
}

/// Flex container main axis. First-pass flexbox supports `row` (horizontal);
/// `column` falls back to normal block stacking.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FlexDirection {
    Row,
    Column,
}

/// Main-axis distribution of free space in a flex row (`justify-content`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JustifyContent {
    FlexStart,
    FlexEnd,
    Center,
    SpaceBetween,
    SpaceAround,
    SpaceEvenly,
}

/// Cross-axis alignment of flex items (`align-items`). First pass places items at
/// the row's top; `stretch` is treated as `flex-start`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AlignItems {
    Stretch,
    FlexStart,
    Center,
    FlexEnd,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PageStyle {
    pub orientation: PageOrientation,
    pub margin_top: Option<f32>,
    pub margin_right: Option<f32>,
    pub margin_bottom: Option<f32>,
    pub margin_left: Option<f32>,
}

impl Default for PageStyle {
    fn default() -> Self {
        Self {
            orientation: PageOrientation::Portrait,
            margin_top: None,
            margin_right: None,
            margin_bottom: None,
            margin_left: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PageOrientation {
    Portrait,
    Landscape,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TableStyle {
    pub row_height: Option<f32>,
}

impl Default for TableStyle {
    fn default() -> Self {
        Self { row_height: None }
    }
}

pub fn parse(input: &str) -> Document {
    finish(crate::dom::Dom::parse(input))
}

/// Parse, but first run a pre-layout script stage that may mutate the DOM (ADR
/// 0006). With the `NoopScriptEngine` this is identical to [`parse`].
pub fn parse_scripted(
    input: &str,
    engine: &dyn crate::script::ScriptEngine,
    limits: &crate::script::ScriptLimits,
) -> Document {
    let mut dom = crate::dom::Dom::parse(input);
    let _report = engine.run(&mut dom, limits);
    finish(dom)
}

/// Build a `Document` from a (possibly script-mutated) DOM: parse the stylesheet
/// and geometry, compute styles, and generate the table rows or flow box tree.
fn finish(dom: crate::dom::Dom) -> Document {
    // Source CSS from the DOM's <style> elements and parse it once with
    // cssparser, for both the cascade and the page/table geometry.
    let css = collect_style_css(&dom);
    let (page_style, table_style, table_columns) = parse_page_geometry(&css);
    let stylesheet = parse_stylesheet(&css);
    let computed = compute_inherited_styles(&dom, &stylesheet);

    // Build the flow box tree, with any `<table>` embedded as a `Table` box.
    let fonts = std::cell::RefCell::new(FontInterner::new());
    let env = FlowEnv {
        stylesheet: &stylesheet,
        computed: &computed,
        table_columns: &table_columns,
        row_height: table_style.row_height,
        fonts: &fonts,
    };
    let flow = build_flow(&dom, &env);

    // A document with real flow content around its tables is laid out as flow
    // (headings/paragraphs and tables interleaved in document order). A bare
    // table falls back to the dedicated spreadsheet path (`blocks`), preserving
    // its fast, well-tuned layout.
    let (mut blocks, mut flow) = match flow {
        Some(root) if root.has_nontable_content() => (Vec::new(), Some(root)),
        _ => (tables_from_dom(&dom, &stylesheet, &computed), None),
    };

    // Post-pass: intern each table cell's font requirement (its computed style
    // already carries family/bold/italic), wherever the cells live.
    let mut fonts = fonts.into_inner();
    for block in &mut blocks {
        for cell in &mut block.cells {
            intern_cell_font(cell, &mut fonts);
        }
    }
    if let Some(root) = &mut flow {
        intern_table_fonts_in(&mut root.children, &mut fonts);
    }

    Document {
        blocks,
        flow,
        page_style,
        table_style,
        table_columns,
        images: Vec::new(),
        font_specs: fonts.into_specs(),
    }
}

fn intern_cell_font(cell: &mut TableCell, fonts: &mut FontInterner) {
    let family = cell
        .style
        .font_family
        .clone()
        .map(|name| fonts.family(&name));
    cell.font = fonts.spec(family, cell.style.bold, cell.style.italic.unwrap_or(false));
}

fn intern_table_fonts_in(children: &mut [crate::box_tree::BoxChild], fonts: &mut FontInterner) {
    for child in children {
        match child {
            crate::box_tree::BoxChild::Table(table) => {
                for row in &mut table.rows {
                    for cell in &mut row.cells {
                        intern_cell_font(cell, fonts);
                    }
                }
            }
            crate::box_tree::BoxChild::Block(block) => {
                intern_table_fonts_in(&mut block.children, fonts);
            }
            _ => {}
        }
    }
}

/// Interns the font requirements seen while building the box tree: family
/// names (deduplicated) and `(family, bold, italic)` specs. Runs and cells
/// store a `u16` spec index; the resolved face table is built per render.
/// Spec 0 is always the default (document font, regular weight, upright).
pub(crate) struct FontInterner {
    families: Vec<String>,
    family_map: std::collections::HashMap<String, u16>,
    specs: Vec<crate::font::FontSpec>,
    spec_map: std::collections::HashMap<(Option<u16>, bool, bool), u16>,
}

impl FontInterner {
    fn new() -> Self {
        let default = crate::font::FontSpec {
            family: None,
            bold: false,
            italic: false,
        };
        Self {
            families: Vec::new(),
            family_map: std::collections::HashMap::new(),
            specs: vec![default],
            spec_map: std::collections::HashMap::from([((None, false, false), 0)]),
        }
    }

    fn family(&mut self, name: &str) -> u16 {
        if let Some(&index) = self.family_map.get(name) {
            return index;
        }
        let index = self.families.len() as u16;
        self.families.push(name.to_string());
        self.family_map.insert(name.to_string(), index);
        index
    }

    fn spec(&mut self, family: Option<u16>, bold: bool, italic: bool) -> u16 {
        if let Some(&index) = self.spec_map.get(&(family, bold, italic)) {
            return index;
        }
        let index = self.specs.len() as u16;
        self.specs.push(crate::font::FontSpec {
            family: family.map(|f| self.families[f as usize].clone()),
            bold,
            italic,
        });
        self.spec_map.insert((family, bold, italic), index);
        index
    }

    fn into_specs(self) -> Vec<crate::font::FontSpec> {
        self.specs
    }
}

/// Shared context threaded through the flow builder: the parsed stylesheet and
/// computed styles (for table-section resolution) plus the document-level table
/// geometry that embedded `Table` boxes inherit, and the font interner.
struct FlowEnv<'a> {
    stylesheet: &'a Stylesheet,
    computed: &'a ComputedStyles,
    table_columns: &'a [f32],
    row_height: Option<f32>,
    fonts: &'a std::cell::RefCell<FontInterner>,
}

/// Lower the DOM into the flow box tree (ADR 0002 step 8) for non-table
/// documents: a nested tree of block boxes whose leaves are runs of styled
/// inline text. Block-level elements open a child box (carrying their computed
/// indent/alignment and a resolved font size); inline elements thread their
/// computed style into the runs they contain; `display: none` subtrees are
/// skipped. Returns `None` when the document carries no visible text.
fn build_flow(dom: &crate::dom::Dom, env: &FlowEnv) -> Option<crate::box_tree::FlowRoot> {
    let root_ctx = FlowCtx {
        font_size: crate::layout::font_size_for(BlockKind::Paragraph),
        bold: false,
        italic: false,
        family: None,
        font: 0, // the interner's default spec
        underline: false,
        line_through: false,
        color: Color::BLACK,
        align: TextAlign::Left,
    };
    let mut acc = ChildAcc::default();
    build_node(dom, dom.root(), env, root_ctx, &mut acc);
    acc.flush_line();

    let root = crate::box_tree::FlowRoot {
        children: acc.children,
    };
    root.has_text().then_some(root)
}

/// Load and measure every `<img>` in the flow tree, filling in each
/// `ImageBox`'s `image_index` and laid-out point size and populating
/// `document.images`. File-path sources resolve relative to `base_dir`
/// (`data:` URIs need none). Images that fail to load are left unresolved and
/// simply not painted. A no-op for table documents (which carry no flow tree).
pub fn resolve_images(document: &mut Document, base_dir: Option<&std::path::Path>) {
    let Some(flow) = document.flow.as_mut() else {
        return;
    };
    let mut images = std::mem::take(&mut document.images);
    resolve_images_in(&mut flow.children, base_dir, &mut images);
    document.images = images;
}

fn resolve_images_in(
    children: &mut [crate::box_tree::BoxChild],
    base_dir: Option<&std::path::Path>,
    images: &mut Vec<crate::image::DecodedImage>,
) {
    use crate::box_tree::BoxChild;
    for child in children {
        match child {
            BoxChild::Block(block) => resolve_images_in(&mut block.children, base_dir, images),
            BoxChild::Image(image) => resolve_image_box(image, base_dir, images),
            // Table cells carry no `<img>` content in the current model.
            BoxChild::Line(_) | BoxChild::Table(_) => {}
        }
    }
}

/// CSS pixels to PDF points at the reference 96 dpi (1px = 0.75pt).
const PX_TO_PT: f32 = 72.0 / 96.0;

fn resolve_image_box(
    image: &mut crate::box_tree::ImageBox,
    base_dir: Option<&std::path::Path>,
    images: &mut Vec<crate::image::DecodedImage>,
) {
    let Some(decoded) = crate::image::load_image(&image.src, base_dir) else {
        return;
    };
    let intrinsic_w = decoded.width as f32;
    let intrinsic_h = decoded.height as f32;
    // Resolve the box in points. CSS `width`/`height` (already in points) win
    // over the presentational HTML attributes (CSS pixels), matching browsers.
    let hint_w = image.css_width.or(image.attr_width.map(|w| w * PX_TO_PT));
    let hint_h = image.css_height.or(image.attr_height.map(|h| h * PX_TO_PT));
    // Preserve the intrinsic aspect ratio when only one dimension is given.
    let (width_pt, height_pt) = match (hint_w, hint_h) {
        (Some(w), Some(h)) => (w, h),
        (Some(w), None) if intrinsic_w > 0.0 => (w, w * intrinsic_h / intrinsic_w),
        (None, Some(h)) if intrinsic_h > 0.0 => (h * intrinsic_w / intrinsic_h, h),
        _ => (intrinsic_w * PX_TO_PT, intrinsic_h * PX_TO_PT),
    };
    image.width = width_pt;
    image.height = height_pt;
    image.image_index = Some(images.len());
    images.push(decoded);
}

/// The inline style context threaded down the tree while building flow content.
#[derive(Clone, Copy)]
struct FlowCtx {
    font_size: f32,
    bold: bool,
    /// Cascaded `font-style: italic`.
    italic: bool,
    /// Interned `font-family` (index into the interner's family list).
    family: Option<u16>,
    /// Interned `(family, bold, italic)` spec — what runs actually store.
    font: u16,
    underline: bool,
    line_through: bool,
    color: Color,
    align: TextAlign,
}

/// Accumulates one block's children, buffering inline text into a pending line
/// box that is emitted whenever a block boundary (or `<br>`) is reached.
#[derive(Default)]
struct ChildAcc {
    children: Vec<crate::box_tree::BoxChild>,
    pending: Vec<crate::box_tree::InlineRun>,
}

impl ChildAcc {
    /// Append inline text under the current style, merging into the previous run
    /// when the style matches to keep the run count low.
    fn push_text(&mut self, text: &str, ctx: &FlowCtx) {
        if text.is_empty() {
            return;
        }
        if let Some(last) = self.pending.last_mut() {
            if last.font_size == ctx.font_size
                && last.bold == ctx.bold
                && last.font == ctx.font
                && last.underline == ctx.underline
                && last.line_through == ctx.line_through
                && last.color == ctx.color
            {
                last.text.push_str(text);
                return;
            }
        }
        self.pending.push(crate::box_tree::InlineRun {
            text: text.to_string(),
            font_size: ctx.font_size,
            bold: ctx.bold,
            font: ctx.font,
            underline: ctx.underline,
            line_through: ctx.line_through,
            color: ctx.color,
        });
    }

    /// Emit the pending inline content as a line box, if it carries any text.
    fn flush_line(&mut self) {
        if self.pending.iter().any(|run| !run.text.trim().is_empty()) {
            self.children
                .push(crate::box_tree::BoxChild::Line(std::mem::take(&mut self.pending)));
        } else {
            self.pending.clear();
        }
    }
}

fn build_node(
    dom: &crate::dom::Dom,
    id: crate::dom::NodeId,
    env: &FlowEnv,
    ctx: FlowCtx,
    acc: &mut ChildAcc,
) {
    use crate::dom::NodeData;

    let computed = env.computed;
    let node = dom.node(id);
    match &node.data {
        NodeData::Text(text) => acc.push_text(text, &ctx),
        NodeData::Document => {
            for &child in &node.children {
                build_node(dom, child, env, ctx, acc);
            }
        }
        NodeData::Element { name, .. } => {
            let tag = name.as_str();
            // Non-rendered subtrees and `display: none` contribute nothing.
            if matches!(tag, "head" | "script" | "style" | "title") || computed.hidden[id] {
                return;
            }

            if tag == "br" {
                // A line break ends the current line but stays in this block.
                acc.flush_line();
            } else if tag == "table" {
                // Embed the table as a flow child, in document order. Its rows are
                // collected from this subtree; geometry is resolved at layout.
                acc.flush_line();
                let mut rows = Vec::new();
                collect_table_rows(dom, id, TableSection::Body, env.stylesheet, computed, &mut rows);
                let rows: Vec<crate::box_tree::TableRow> = rows
                    .into_iter()
                    .map(|block| crate::box_tree::TableRow {
                        kind: block.kind,
                        cells: block.cells,
                    })
                    .collect();
                if !rows.is_empty() {
                    acc.children.push(crate::box_tree::BoxChild::Table(
                        crate::box_tree::TableBox {
                            rows,
                            columns: env.table_columns.to_vec(),
                            row_height: env.row_height,
                        },
                    ));
                }
            } else if tag == "img" {
                // A block-level image. Resolved (loaded/measured) after parsing.
                if let Some(src) = node.attr("src") {
                    if !src.is_empty() {
                        acc.flush_line();
                        let own = &computed.style[id];
                        acc.children
                            .push(crate::box_tree::BoxChild::Image(crate::box_tree::ImageBox {
                                src: src.to_string(),
                                attr_width: node
                                    .attr("width")
                                    .and_then(|v| v.trim().parse::<f32>().ok()),
                                attr_height: node
                                    .attr("height")
                                    .and_then(|v| v.trim().parse::<f32>().ok()),
                                css_width: own.width,
                                css_height: own.height,
                                image_index: None,
                                width: 0.0,
                                height: 0.0,
                                float_dir: own.float_dir,
                            }));
                    }
                }
            } else if is_block_tag(tag) {
                acc.flush_line();
                if let Some(block) = build_block(dom, id, env, ctx, tag) {
                    acc.children.push(crate::box_tree::BoxChild::Block(block));
                }
            } else {
                // Inline element: fold its computed style into the context and
                // let its children contribute to the enclosing line.
                let child_ctx = inline_ctx(&ctx, env, id, tag);
                for &child in &node.children {
                    build_node(dom, child, env, child_ctx, acc);
                }
            }
        }
    }
}

/// Build a block box for a block-level element, recursing into its children.
fn build_block(
    dom: &crate::dom::Dom,
    id: crate::dom::NodeId,
    env: &FlowEnv,
    parent: FlowCtx,
    tag: &str,
) -> Option<crate::box_tree::BlockBox> {
    let computed = env.computed;
    let kind = block_kind_for(tag);
    let own = &computed.style[id];

    let font_size = own.font_size.unwrap_or(crate::layout::font_size_for(kind));
    let bold = parent.bold || own.bold || is_heading(kind) || tag == "th";
    let color = own.color.unwrap_or(parent.color);
    let align = own.align.unwrap_or(parent.align);

    // CSS margins (default to the per-kind spacing when unset) and padding. List
    // and blockquote nesting fold into `margin.left` so it accumulates as they
    // nest.
    let nesting_indent = if matches!(tag, "ul" | "ol" | "blockquote" | "dd") {
        LIST_INDENT
    } else {
        0.0
    };
    // Inline elements promoted to flex items (built via the flex-container child
    // loop) default to zero vertical margins, like a browser's UA styles.
    let inline_item = !is_block_tag(tag);
    let margin = crate::box_tree::Edges {
        top: own.margin_top.unwrap_or_else(|| {
            if inline_item { 0.0 } else { crate::layout::spacing_before(kind) }
        }),
        right: own.margin_right.unwrap_or(0.0),
        bottom: own.margin_bottom.unwrap_or_else(|| {
            if inline_item { 0.0 } else { crate::layout::spacing_after(kind) }
        }),
        left: own.margin_left.unwrap_or(0.0) + nesting_indent,
    };
    let padding = crate::box_tree::Edges {
        top: own.padding_top.unwrap_or(0.0),
        right: own.padding_right.unwrap_or(0.0),
        bottom: own.padding_bottom.unwrap_or(0.0),
        left: own.padding_left.unwrap_or(0.0),
    };
    // A white background matches the page, so it is treated as "no background".
    let background = own
        .background_color
        .filter(|color| *color != Color::WHITE);

    // Font selection: an own `font-family` overrides the inherited one; `<pre>`
    // defaults to monospace (the only block-level UA family rule we apply);
    // `<address>` is italic by UA convention.
    let family = match &own.font_family {
        Some(name) => Some(env.fonts.borrow_mut().family(name)),
        None if tag == "pre" => Some(env.fonts.borrow_mut().family("monospace")),
        None => parent.family,
    };
    let italic = own.italic.unwrap_or(parent.italic || tag == "address");
    let font = env.fonts.borrow_mut().spec(family, bold, italic);

    let child_ctx = FlowCtx {
        font_size,
        bold,
        italic,
        family,
        font,
        underline: parent.underline || own.underline,
        line_through: parent.line_through || own.line_through,
        color,
        align,
    };

    let mut acc = ChildAcc::default();
    if tag == "li" {
        let marker = li_marker(dom, id);
        acc.push_text(&marker, &child_ctx);
    }
    if own.display_flex || own.display_grid {
        // Every element child of a flex/grid container becomes an item (per the
        // flexbox/grid models), so inline elements like <span> are built as
        // blocks here instead of folding into a shared line. Bare text between
        // them still accumulates into anonymous line items.
        for &child in &dom.node(id).children {
            let child_node = dom.node(child);
            match child_node.tag() {
                Some(child_tag)
                    if !matches!(
                        child_tag,
                        "head" | "script" | "style" | "title" | "br" | "img" | "table"
                    ) && !computed.hidden[child] =>
                {
                    acc.flush_line();
                    if let Some(item) = build_block(dom, child, env, child_ctx, child_tag) {
                        acc.children.push(crate::box_tree::BoxChild::Block(item));
                    }
                }
                _ => build_node(dom, child, env, child_ctx, &mut acc),
            }
        }
    } else {
        for &child in &dom.node(id).children {
            build_node(dom, child, env, child_ctx, &mut acc);
        }
    }
    acc.flush_line();

    if acc.children.is_empty() {
        return None;
    }
    let flex = own.display_flex.then(|| crate::box_tree::FlexContainer {
        direction: own.flex_direction.unwrap_or(FlexDirection::Row),
        justify: own.justify_content.unwrap_or(JustifyContent::FlexStart),
        align: own.align_items.unwrap_or(AlignItems::Stretch),
        gap: own.gap.unwrap_or(0.0),
    });
    let grid = own.display_grid.then(|| crate::box_tree::GridContainer {
        columns: own.grid_template.clone().unwrap_or_default(),
        column_gap: own.gap.unwrap_or(0.0),
        row_gap: own.row_gap.unwrap_or(0.0),
    });
    Some(crate::box_tree::BlockBox {
        kind,
        margin,
        padding,
        align,
        background,
        border: own.border.unwrap_or(false),
        flex,
        flex_grow: own.flex_grow.unwrap_or(0.0),
        flex_basis: own.flex_basis,
        grid,
        grid_span: own.grid_span.unwrap_or(1),
        float_dir: own.float_dir,
        clear: own.clear,
        css_width: own.width,
        line_height: own.line_height,
        position: own.position,
        z_index: own.z_index,
        offset_top: own.offset_top,
        offset_right: own.offset_right,
        offset_bottom: own.offset_bottom,
        offset_left: own.offset_left,
        children: acc.children,
    })
}

/// Fold an inline element's computed style into the surrounding context. Block
/// alignment is unaffected; `<b>`/`<strong>` force bold, `<i>`/`<em>` (and
/// citation-family tags) force italic, and `<code>`-family tags default to
/// monospace, even without a rule (there is no UA stylesheet).
fn inline_ctx(parent: &FlowCtx, env: &FlowEnv, id: crate::dom::NodeId, tag: &str) -> FlowCtx {
    let own = &env.computed.style[id];
    let bold = parent.bold || own.bold || matches!(tag, "b" | "strong");
    let italic = own
        .italic
        .unwrap_or(parent.italic || matches!(tag, "i" | "em" | "cite" | "var" | "dfn"));
    let family = match &own.font_family {
        Some(name) => Some(env.fonts.borrow_mut().family(name)),
        None if matches!(tag, "code" | "tt" | "kbd" | "samp") => {
            Some(env.fonts.borrow_mut().family("monospace"))
        }
        None => parent.family,
    };
    let font = env.fonts.borrow_mut().spec(family, bold, italic);
    FlowCtx {
        font_size: own.font_size.unwrap_or(parent.font_size),
        bold,
        italic,
        family,
        font,
        underline: parent.underline || own.underline || matches!(tag, "u" | "ins"),
        line_through: parent.line_through || own.line_through || matches!(tag, "s" | "strike" | "del"),
        color: own.color.unwrap_or(parent.color),
        align: parent.align,
    }
}

/// The list marker for an `<li>`: a bullet for `<ul>` (or a bare item), or a
/// 1-based number for `<ol>`.
fn li_marker(dom: &crate::dom::Dom, id: crate::dom::NodeId) -> String {
    let parent = dom.node(id).parent;
    let ordered = parent
        .map(|p| dom.node(p).tag() == Some("ol"))
        .unwrap_or(false);

    if !ordered {
        return "\u{2022}  ".to_string();
    }

    let mut number = 1;
    if let Some(p) = parent {
        for &sibling in &dom.node(p).children {
            if sibling == id {
                break;
            }
            if dom.node(sibling).tag() == Some("li") {
                number += 1;
            }
        }
    }
    format!("{number}.  ")
}

/// Fixed left-indent step applied per list / blockquote nesting level.
const LIST_INDENT: f32 = 24.0;

fn is_heading(kind: BlockKind) -> bool {
    matches!(
        kind,
        BlockKind::Heading1
            | BlockKind::Heading2
            | BlockKind::Heading3
            | BlockKind::Heading4
            | BlockKind::Heading5
            | BlockKind::Heading6
    )
}

fn block_kind_for(tag: &str) -> BlockKind {
    match tag {
        "h1" => BlockKind::Heading1,
        "h2" => BlockKind::Heading2,
        "h3" => BlockKind::Heading3,
        "h4" => BlockKind::Heading4,
        "h5" => BlockKind::Heading5,
        "h6" => BlockKind::Heading6,
        _ => BlockKind::Paragraph,
    }
}

/// Block-level tags that open their own box. Everything else is treated as
/// inline (its text joins the enclosing line box).
fn is_block_tag(tag: &str) -> bool {
    matches!(
        tag,
        "h1" | "h2"
            | "h3"
            | "h4"
            | "h5"
            | "h6"
            | "p"
            | "div"
            | "section"
            | "article"
            | "main"
            | "header"
            | "footer"
            | "nav"
            | "aside"
            | "blockquote"
            | "figure"
            | "figcaption"
            | "address"
            | "pre"
            | "ul"
            | "ol"
            | "li"
            | "dl"
            | "dt"
            | "dd"
            | "form"
            | "fieldset"
    )
}

/// Parse page and table geometry from the stylesheet with `cssparser` (rather
/// than scanning raw HTML): `@page` margins and orientation, the spreadsheet
/// column widths (in source order), and the table row height. `@media` blocks are
/// descended into, matching the old "scan anywhere" behavior.
fn parse_page_geometry(css: &str) -> (PageStyle, TableStyle, Vec<f32>) {
    let mut input = ParserInput::new(css);
    let mut parser = Parser::new(&mut input);
    let mut geo_parser = GeometryParser;
    let mut rules = StyleSheetParser::new(&mut parser, &mut geo_parser);

    let mut page = PageStyle::default();
    let mut table = TableStyle::default();
    let mut columns = Vec::new();

    while let Some(result) = rules.next() {
        let Ok(items) = result else { continue };
        for item in items {
            match item {
                GeoItem::ColWidth(width) => columns.push(width),
                GeoItem::RowHeight(height) => {
                    table.row_height.get_or_insert(height);
                }
                GeoItem::Page {
                    margins,
                    landscape,
                } => {
                    if landscape {
                        page.orientation = PageOrientation::Landscape;
                    }
                    page.margin_top = page.margin_top.or(margins[0]);
                    page.margin_right = page.margin_right.or(margins[1]);
                    page.margin_bottom = page.margin_bottom.or(margins[2]);
                    page.margin_left = page.margin_left.or(margins[3]);
                }
            }
        }
    }

    (page, table, columns)
}

/// One piece of geometry produced by a single CSS rule.
enum GeoItem {
    /// A spreadsheet column width (`table.sheet0 col.colN { width }`).
    ColWidth(f32),
    /// A table row height (`table.sheet0 tr { height }`).
    RowHeight(f32),
    /// `@page` margins `[top, right, bottom, left]` and `size: landscape`.
    Page { margins: [Option<f32>; 4], landscape: bool },
}

/// A `cssparser` rule parser that extracts only geometry (see [`GeoItem`]).
struct GeometryParser;

impl<'i> QualifiedRuleParser<'i> for GeometryParser {
    type Prelude = Vec<SimpleSelector>;
    type QualifiedRule = Vec<GeoItem>;
    type Error = ();

    fn parse_prelude<'t>(
        &mut self,
        input: &mut Parser<'i, 't>,
    ) -> Result<Self::Prelude, ParseError<'i, ()>> {
        Ok(parse_selector_list(input))
    }

    fn parse_block<'t>(
        &mut self,
        prelude: Self::Prelude,
        _start: &ParserState,
        input: &mut Parser<'i, 't>,
    ) -> Result<Self::QualifiedRule, ParseError<'i, ()>> {
        let decls = parse_geo_declarations(input);
        let mut items = Vec::new();
        for selector in &prelude {
            match selector.subject.tag.as_deref() {
                Some("col") => {
                    if let Some(width) = decls.width {
                        items.push(GeoItem::ColWidth(width));
                    }
                }
                Some("tr") if selector.subject.classes.is_empty() => {
                    if let Some(height) = decls.height {
                        items.push(GeoItem::RowHeight(height));
                    }
                }
                _ => {}
            }
        }
        Ok(items)
    }
}

impl<'i> AtRuleParser<'i> for GeometryParser {
    type Prelude = AtRuleKind;
    type AtRule = Vec<GeoItem>;
    type Error = ();

    fn parse_prelude<'t>(
        &mut self,
        name: CowRcStr<'i>,
        input: &mut Parser<'i, 't>,
    ) -> Result<Self::Prelude, ParseError<'i, ()>> {
        let query_start = input.position();
        consume_remaining(input);
        if name.eq_ignore_ascii_case("media") {
            let query = input.slice_from(query_start);
            Ok(AtRuleKind::Media(media_applies_to_print(query)))
        } else if name.eq_ignore_ascii_case("page") {
            Ok(AtRuleKind::Page)
        } else {
            Ok(AtRuleKind::Other)
        }
    }

    fn parse_block<'t>(
        &mut self,
        prelude: Self::Prelude,
        _start: &ParserState,
        input: &mut Parser<'i, 't>,
    ) -> Result<Self::AtRule, ParseError<'i, ()>> {
        match prelude {
            AtRuleKind::Media(true) => {
                let mut inner = GeometryParser;
                let mut rules = StyleSheetParser::new(input, &mut inner);
                let mut collected = Vec::new();
                while let Some(result) = rules.next() {
                    if let Ok(mut items) = result {
                        collected.append(&mut items);
                    }
                }
                Ok(collected)
            }
            AtRuleKind::Media(false) => Ok(Vec::new()),
            AtRuleKind::Page => {
                let decls = parse_geo_declarations(input);
                Ok(vec![GeoItem::Page {
                    margins: [
                        decls.margin_top,
                        decls.margin_right,
                        decls.margin_bottom,
                        decls.margin_left,
                    ],
                    landscape: decls.landscape,
                }])
            }
            AtRuleKind::Other => Ok(Vec::new()),
        }
    }

    fn rule_without_block(&mut self, _prelude: Self::Prelude, _start: &ParserState) -> Result<Self::AtRule, ()> {
        Ok(Vec::new())
    }
}

/// The geometry declarations extracted from one rule body.
#[derive(Default)]
struct GeoDecls {
    width: Option<f32>,
    height: Option<f32>,
    margin_top: Option<f32>,
    margin_right: Option<f32>,
    margin_bottom: Option<f32>,
    margin_left: Option<f32>,
    landscape: bool,
}

fn parse_geo_declarations(input: &mut Parser<'_, '_>) -> GeoDecls {
    let mut decls = GeoDecls::default();
    let mut decl_parser = GeoDeclParser { decls: &mut decls };
    let mut items = RuleBodyParser::new(input, &mut decl_parser);
    while let Some(result) = items.next() {
        let _ = result;
    }
    decls
}

struct GeoDeclParser<'a> {
    decls: &'a mut GeoDecls,
}

impl<'i> DeclarationParser<'i> for GeoDeclParser<'_> {
    type Declaration = ();
    type Error = ();

    fn parse_value<'t>(
        &mut self,
        name: CowRcStr<'i>,
        input: &mut Parser<'i, 't>,
        _start: &ParserState,
    ) -> Result<(), ParseError<'i, ()>> {
        let value_start = input.position();
        consume_remaining(input);
        let raw_value = input.slice_from(value_start);
        let (value, _important) = normalize_declaration_value(raw_value);

        match name.to_ascii_lowercase().as_str() {
            "width" => self.decls.width = parse_css_length(&value),
            "height" => self.decls.height = parse_css_length(&value),
            "margin-top" => self.decls.margin_top = parse_css_length(&value),
            "margin-right" => self.decls.margin_right = parse_css_length(&value),
            "margin-bottom" => self.decls.margin_bottom = parse_css_length(&value),
            "margin-left" => self.decls.margin_left = parse_css_length(&value),
            "margin" => {
                let [top, right, bottom, left] = parse_box_edges(&value);
                self.decls.margin_top = top;
                self.decls.margin_right = right;
                self.decls.margin_bottom = bottom;
                self.decls.margin_left = left;
            }
            "size" => {
                if value.to_ascii_lowercase().contains("landscape") {
                    self.decls.landscape = true;
                }
            }
            _ => {}
        }
        Ok(())
    }
}

impl<'i> AtRuleParser<'i> for GeoDeclParser<'_> {
    type Prelude = ();
    type AtRule = ();
    type Error = ();
}

impl<'i> QualifiedRuleParser<'i> for GeoDeclParser<'_> {
    type Prelude = ();
    type QualifiedRule = ();
    type Error = ();
}

impl<'i> RuleBodyItemParser<'i, (), ()> for GeoDeclParser<'_> {
    fn parse_declarations(&self) -> bool {
        true
    }

    fn parse_qualified(&self) -> bool {
        false
    }
}

/// Extract table rows and cells from the real DOM.
///
/// First computes every node's inherited style in one top-down pass, then walks
/// the tree tracking the current table section (the `<thead>` / `<tbody>` /
/// `<tfoot>` ancestor, with its CSS `display` group honored) and emits one
/// `Block` per `<tr>`. Cell styles are looked up from the precomputed table so
/// each cell carries properties inherited from its ancestors (ADR 0002 step 6).
fn tables_from_dom(
    dom: &crate::dom::Dom,
    stylesheet: &Stylesheet,
    computed: &ComputedStyles,
) -> Vec<Block> {
    let mut rows = Vec::new();
    collect_table_rows(
        dom,
        dom.root(),
        TableSection::Body,
        stylesheet,
        computed,
        &mut rows,
    );
    rows
}

fn collect_table_rows(
    dom: &crate::dom::Dom,
    id: crate::dom::NodeId,
    section: TableSection,
    stylesheet: &Stylesheet,
    computed: &ComputedStyles,
    rows: &mut Vec<Block>,
) {
    // `display: none` hides the element and its whole subtree.
    if computed.hidden[id] {
        return;
    }

    let node = dom.node(id);
    let mut child_section = section;

    match node.tag() {
        Some("thead") | Some("tbody") | Some("tfoot") => {
            let default = match node.tag() {
                Some("thead") => TableSection::Header,
                Some("tfoot") => TableSection::Footer,
                _ => TableSection::Body,
            };
            // A CSS `display: table-*-group` on the section element overrides the
            // tag default (matches browser computed display).
            child_section = display_to_table_section(computed_display_for_node(dom, id, stylesheet))
                .unwrap_or(default);
        }
        Some("tr") => {
            let cells = cells_from_row(dom, id, computed);
            if !cells.is_empty() {
                rows.push(Block {
                    kind: row_kind(dom, id, section, stylesheet),
                    text: String::new(),
                    cells,
                    style: CellStyle::default(),
                });
            }
        }
        _ => {}
    }

    for &child in &node.children {
        collect_table_rows(dom, child, child_section, stylesheet, computed, rows);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TableSection {
    Header,
    Body,
    Footer,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CssDisplay {
    None,
    TableHeaderGroup,
    TableRowGroup,
    TableFooterGroup,
}

fn row_kind(
    dom: &crate::dom::Dom,
    id: crate::dom::NodeId,
    section: TableSection,
    stylesheet: &Stylesheet,
) -> BlockKind {
    // A `display: table-*-group` on the row itself overrides the inherited
    // section, matching the old open-tag behavior.
    let section =
        display_to_table_section(computed_display_for_node(dom, id, stylesheet)).unwrap_or(section);

    match section {
        TableSection::Header => BlockKind::TableHeaderRow,
        TableSection::Body => BlockKind::TableRow,
        TableSection::Footer => BlockKind::TableFooterRow,
    }
}

fn display_to_table_section(display: Option<CssDisplay>) -> Option<TableSection> {
    match display? {
        CssDisplay::TableHeaderGroup => Some(TableSection::Header),
        CssDisplay::TableRowGroup => Some(TableSection::Body),
        CssDisplay::TableFooterGroup => Some(TableSection::Footer),
        CssDisplay::None => None,
    }
}


fn computed_display_for_node(
    dom: &crate::dom::Dom,
    id: crate::dom::NodeId,
    stylesheet: &Stylesheet,
) -> Option<CssDisplay> {
    let node = dom.node(id);
    let tag = node.tag().unwrap_or_default();
    let classes = node.classes().collect::<Vec<_>>();
    let inline_style = node.attr("style").unwrap_or_default();
    let mut declarations = stylesheet.computed_declarations(dom, id, tag, &classes);

    if !inline_style.is_empty() {
        declarations.merge_inline(parse_style_declarations(inline_style));
    }

    declarations.resolved().display
}

fn cells_from_row(
    dom: &crate::dom::Dom,
    tr_id: crate::dom::NodeId,
    computed: &ComputedStyles,
) -> Vec<TableCell> {
    let mut cells = Vec::new();

    for &child in &dom.node(tr_id).children {
        let node = dom.node(child);
        if !matches!(node.tag(), Some("td") | Some("th")) {
            continue;
        }
        // Skip cells hidden by `display: none`.
        if computed.hidden[child] {
            continue;
        }

        let mut text = String::new();
        collect_text(dom, child, &mut text);
        let text = collapse_whitespace(&text);

        let colspan = node
            .attr("colspan")
            .and_then(|value| value.trim().parse::<usize>().ok())
            .unwrap_or(1)
            .max(1);

        // The cell's computed style already includes properties inherited from
        // its ancestors; layer the spreadsheet class alignment heuristic on top
        // only where neither the cascade nor inheritance set an alignment.
        let mut style = computed.style[child].clone();
        infer_cell_alignment(&mut style, &node.classes().collect::<Vec<_>>());

        cells.push(TableCell {
            text,
            colspan,
            style,
            font: 0, // interned in the post-pass over the finished document
        });
    }

    cells
}

/// Concatenate all descendant text of a node. html5ever has already decoded
/// entities, so no further decoding is required.
fn collect_text(dom: &crate::dom::Dom, id: crate::dom::NodeId, out: &mut String) {
    match &dom.node(id).data {
        crate::dom::NodeData::Text(text) => out.push_str(text),
        _ => {
            for &child in &dom.node(id).children {
                collect_text(dom, child, out);
            }
        }
    }
}

/// Compute each node's inherited style in a single top-down pass.
///
/// A node's computed style takes its inheritable properties (color, font size,
/// font weight, text alignment, white-space, and wrapping) from its parent when
/// the node itself does not set them, and its non-inheritable properties
/// (borders, padding, background, overflow, vertical alignment) from its own
/// cascade only — matching CSS inheritance.
/// Per-node computed style and a `hidden` flag (true when the node or any
/// ancestor has `display: none`), both produced in one top-down pass.
struct ComputedStyles {
    style: Vec<CellStyle>,
    hidden: Vec<bool>,
}

fn compute_inherited_styles(dom: &crate::dom::Dom, stylesheet: &Stylesheet) -> ComputedStyles {
    let mut out = ComputedStyles {
        style: vec![CellStyle::default(); dom.nodes.len()],
        hidden: vec![false; dom.nodes.len()],
    };
    let mut cache = HashMap::new();
    compute_inherited_node(
        dom,
        dom.root(),
        CellStyle::default(),
        false,
        stylesheet,
        &mut cache,
        &mut out,
    );
    out
}

fn compute_inherited_node(
    dom: &crate::dom::Dom,
    id: crate::dom::NodeId,
    inherited: CellStyle,
    parent_hidden: bool,
    stylesheet: &Stylesheet,
    cache: &mut HashMap<String, (CellStyle, bool)>,
    out: &mut ComputedStyles,
) {
    let node = dom.node(id);
    let (style, hidden) = match &node.data {
        crate::dom::NodeData::Element { .. } => {
            let (own, display_none) = element_own(dom, id, stylesheet, cache);
            (inherit_style(&inherited, &own), parent_hidden || display_none)
        }
        // Text and document nodes carry no cascade of their own; they inherit
        // their parent's style and hidden state.
        _ => (inherited, parent_hidden),
    };
    out.style[id] = style.clone();
    out.hidden[id] = hidden;

    for &child in &node.children {
        compute_inherited_node(dom, child, style.clone(), hidden, stylesheet, cache, out);
    }
}

/// Combine a parent's computed style with an element's own cascaded style.
fn inherit_style(parent: &CellStyle, own: &CellStyle) -> CellStyle {
    CellStyle {
        // Inheritable: the element's own value wins, else the parent's.
        align: own.align.or(parent.align),
        font_size: own.font_size.or(parent.font_size),
        font_family: own.font_family.clone().or_else(|| parent.font_family.clone()),
        italic: own.italic.or(parent.italic),
        line_height: own.line_height.or(parent.line_height),
        color: own.color.or(parent.color),
        white_space: own.white_space.or(parent.white_space),
        overflow_wrap: own.overflow_wrap.or(parent.overflow_wrap),
        word_break: own.word_break.or(parent.word_break),
        bold: own.bold || parent.bold,
        // Text decoration propagates to descendant inline content (see field docs).
        underline: own.underline || parent.underline,
        line_through: own.line_through || parent.line_through,
        // Non-inheritable: the element's own value only.
        vertical_align: own.vertical_align,
        border: own.border,
        overflow: own.overflow,
        width: own.width,
        height: own.height,
        padding_left: own.padding_left,
        padding_right: own.padding_right,
        padding_top: own.padding_top,
        padding_bottom: own.padding_bottom,
        margin_left: own.margin_left,
        margin_right: own.margin_right,
        margin_top: own.margin_top,
        margin_bottom: own.margin_bottom,
        background_color: own.background_color,
        // Flex/grid properties are not inherited.
        display_flex: own.display_flex,
        flex_direction: own.flex_direction,
        justify_content: own.justify_content,
        align_items: own.align_items,
        gap: own.gap,
        flex_grow: own.flex_grow,
        flex_basis: own.flex_basis,
        display_grid: own.display_grid,
        grid_template: own.grid_template.clone(),
        row_gap: own.row_gap,
        grid_span: own.grid_span,
        float_dir: own.float_dir,
        clear: own.clear,
        position: own.position,
        z_index: own.z_index,
        offset_top: own.offset_top,
        offset_right: own.offset_right,
        offset_bottom: own.offset_bottom,
        offset_left: own.offset_left,
    }
}

/// An element's own cascaded style (matched rules then inline `style`) and
/// whether its computed `display` is `none`, without inheritance or the
/// spreadsheet alignment heuristic. Cached by the element's (tag, class, inline)
/// identity, which repeats heavily in spreadsheet exports.
fn element_own(
    dom: &crate::dom::Dom,
    id: crate::dom::NodeId,
    stylesheet: &Stylesheet,
    cache: &mut HashMap<String, (CellStyle, bool)>,
) -> (CellStyle, bool) {
    let node = dom.node(id);
    let tag = node.tag().unwrap_or_default();
    let class_attr = node.attr("class").unwrap_or_default();
    let inline_style = node.attr("style").unwrap_or_default();

    // Only tree context that appears as selector qualifiers can change the
    // match, so the cache key stays small and shared across most elements.
    // Selectors using ids/attributes/pseudo-classes depend on per-element
    // identity or position the shared key cannot capture, so fall back to a
    // per-element key when the stylesheet uses any of them.
    let key = if stylesheet.needs_precise_match {
        format!("@{id}")
    } else {
        let ancestor_sig = structural_signature(dom, id, stylesheet);
        format!("{tag}|{class_attr}|{inline_style}|{ancestor_sig}")
    };
    if let Some(result) = cache.get(&key) {
        return result.clone();
    }

    let classes = class_attr.split_whitespace().collect::<Vec<_>>();
    let declarations = stylesheet.computed_declarations(dom, id, tag, &classes);
    let resolved = declarations.resolved();
    let mut style = resolved.cell;
    let mut display = resolved.display;

    if !inline_style.is_empty() {
        let inline = parse_style_declarations(inline_style);
        // Inline declarations layer on top of the resolved rule style (same
        // logic as before), and an inline `display` overrides the rule's.
        let inline_display = inline.resolved().display;
        let mut merged = StyleDeclarations::default();
        merged.normal.cell = style;
        merged.merge_inline(inline);
        style = merged.resolved().cell;
        display = inline_display.or(display);
    }

    let result = (style, display == Some(CssDisplay::None));
    cache.insert(key, result.clone());
    result
}

/// Spreadsheet exports encode numeric/centered cells with short class letters
/// (`n`/`f` → right, `b`/`e` → center). Apply that only where no alignment has
/// been set by the cascade or inheritance.
fn infer_cell_alignment(style: &mut CellStyle, classes: &[&str]) {
    if style.align.is_some() {
        return;
    }
    if classes.iter().any(|class| matches!(*class, "n" | "f")) {
        style.align = Some(TextAlign::Right);
    } else if classes.iter().any(|class| matches!(*class, "b" | "e")) {
        style.align = Some(TextAlign::Center);
    }
}

/// A CSS offset length that may be negative (`top: -4pt`), for position offsets.
fn parse_css_offset(value: &str) -> Option<f32> {
    let value = value.trim();
    if let Some(rest) = value.strip_prefix('-') {
        parse_css_length(rest).map(|v| -v)
    } else {
        parse_css_length(value)
    }
}

/// First usable family from a CSS `font-family` stack: quotes stripped,
/// generic keywords kept (resolved at render time). `inherit`/empty → `None`.
fn parse_font_family(value: &str) -> Option<String> {
    for raw in value.split(',') {
        let name = raw.trim().trim_matches('"').trim_matches('\'').trim();
        if name.is_empty() || name.eq_ignore_ascii_case("inherit") {
            continue;
        }
        return Some(name.to_string());
    }
    None
}

/// Parse a CSS `line-height` value. `normal` (and anything invalid or negative)
/// is `None`; a unitless number or `%` is a font-size multiple; a length is
/// absolute. Order matters: a bare number must NOT fall through to
/// `parse_css_length` (which reads unitless values as points).
fn parse_line_height(value: &str) -> Option<LineHeight> {
    let value = value.trim();
    if value.eq_ignore_ascii_case("normal") {
        return None;
    }
    if let Some(percent) = value.strip_suffix('%') {
        return percent
            .trim()
            .parse::<f32>()
            .ok()
            .filter(|n| *n >= 0.0)
            .map(|n| LineHeight::Number(n / 100.0));
    }
    if let Ok(number) = value.parse::<f32>() {
        return (number >= 0.0).then_some(LineHeight::Number(number));
    }
    parse_css_length(value).map(LineHeight::Length)
}

fn parse_css_length(value: &str) -> Option<f32> {
    let number = value
        .chars()
        .take_while(|ch| ch.is_ascii_digit() || *ch == '.')
        .collect::<String>()
        .parse::<f32>()
        .ok()?;
    let unit = value
        .chars()
        .skip_while(|ch| ch.is_ascii_digit() || *ch == '.')
        .collect::<String>()
        .to_ascii_lowercase();

    match unit.as_str() {
        "" | "pt" => Some(number),
        "in" => Some(number * 72.0),
        "px" => Some(number * 0.75),
        "cm" => Some(number * 72.0 / 2.54),
        "mm" => Some(number * 72.0 / 25.4),
        _ => None,
    }
}

/// Parse a `margin`/`padding` shorthand into `[top, right, bottom, left]` using
/// the CSS 1-to-4 value rule. Non-length tokens (e.g. `auto`) become `None`.
fn parse_box_edges(value: &str) -> [Option<f32>; 4] {
    let parts: Vec<Option<f32>> = value.split_whitespace().map(parse_css_length).collect();
    match parts.as_slice() {
        [a] => [*a, *a, *a, *a],
        [a, b] => [*a, *b, *a, *b],
        [a, b, c] => [*a, *b, *c, *b],
        [a, b, c, d, ..] => [*a, *b, *c, *d],
        [] => [None; 4],
    }
}

#[derive(Debug, Default)]
struct Stylesheet {
    rules: Vec<StyleRule>,
    tag_rules: HashMap<String, Vec<usize>>,
    class_rules: HashMap<String, Vec<usize>>,
    id_rules: HashMap<String, Vec<usize>>,
    /// Rules whose subject has no tag/id/class to index by (attribute-only,
    /// pseudo-only, or universal). Always considered as candidates.
    universal_rules: Vec<usize>,
    /// Tags/classes that appear as context (non-subject) requirements in some
    /// selector. Used to keep the style cache key small (only these tokens, on
    /// the relevant relatives, can change a match).
    ancestor_tag_qualifiers: std::collections::BTreeSet<String>,
    ancestor_class_qualifiers: std::collections::BTreeSet<String>,
    /// Whether any selector uses `>`/`+`/`~`. When false, only descendant
    /// combinators exist and a cheap presence-set cache key is exact.
    has_structural_combinator: bool,
    /// Whether any selector uses `+`/`~`. When true, preceding siblings can
    /// affect a match and must be reflected in the cache key.
    has_sibling_combinator: bool,
    /// Whether any selector (subject or context) uses an id, attribute, or
    /// pseudo-class. These depend on element identity/position that the cheap
    /// shared cache key cannot represent, so the style cache falls back to a
    /// per-element key when this is set.
    needs_precise_match: bool,
}

#[derive(Debug, Clone)]
struct StyleRule {
    selector: SimpleSelector,
    declarations: StyleDeclarations,
    specificity: Specificity,
    order: usize,
}

/// One compound selector: an optional type tag, an optional id, class names,
/// attribute selectors, and pseudo-classes. `universal` records an explicit `*`
/// so a compound with no other constraints is still a valid subject.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
struct Compound {
    tag: Option<String>,
    id: Option<String>,
    classes: Vec<String>,
    attrs: Vec<AttrSelector>,
    pseudos: Vec<PseudoClass>,
    universal: bool,
}

impl Compound {
    /// Whether this compound constrains anything (so it is a real selector, not
    /// an empty artifact of parsing).
    fn is_empty(&self) -> bool {
        self.tag.is_none()
            && self.id.is_none()
            && self.classes.is_empty()
            && self.attrs.is_empty()
            && self.pseudos.is_empty()
            && !self.universal
    }
}

/// An attribute selector such as `[type]`, `[type=text]`, or `[class~=x]`.
#[derive(Debug, Clone, PartialEq, Eq)]
struct AttrSelector {
    name: String,
    op: AttrOp,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum AttrOp {
    /// `[name]`
    Exists,
    /// `[name=value]`
    Equals(String),
    /// `[name~=value]` — a whitespace-separated word equals `value`.
    Includes(String),
    /// `[name|=value]` — equals `value` or begins with `value-`.
    DashMatch(String),
    /// `[name^=value]`
    Prefix(String),
    /// `[name$=value]`
    Suffix(String),
    /// `[name*=value]`
    Substring(String),
}

/// A supported pseudo-class. Structural pseudo-classes match against the DOM;
/// dynamic/interactive ones (`:hover`, `:focus`, …) are unsupported and cause the
/// whole selector to be dropped, since they never apply to a static print render.
#[derive(Debug, Clone, PartialEq, Eq)]
enum PseudoClass {
    FirstChild,
    LastChild,
    OnlyChild,
    /// `:nth-child(an+b)` — stores `(a, b)`.
    NthChild(i32, i32),
    NthLastChild(i32, i32),
    FirstOfType,
    LastOfType,
    OnlyOfType,
    NthOfType(i32, i32),
    NthLastOfType(i32, i32),
    Empty,
    Root,
    /// `:not(a, b, …)` — matches when none of the argument compounds match.
    Not(Vec<Compound>),
}

/// A CSS combinator linking a compound to the compound on its right.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Combinator {
    /// `A B`: `A` is any ancestor of the subject.
    Descendant,
    /// `A > B`: `A` is the immediate parent of the subject.
    Child,
    /// `A + B`: `A` is the immediately preceding element sibling.
    NextSibling,
    /// `A ~ B`: `A` is any preceding element sibling.
    SubsequentSibling,
}

/// A selector: the rightmost compound (the matched "subject") plus the preceding
/// compounds and the combinator linking each to the compound on its right.
/// `context` is stored nearest-first, so `context[0]` sits immediately left of
/// the subject. Matching walks the real tree right-to-left, so `>`/`+`/`~` are
/// exact rather than approximated as descendant.
#[derive(Debug, Clone, PartialEq, Eq)]
struct SimpleSelector {
    subject: Compound,
    context: Vec<(Combinator, Compound)>,
}

#[derive(Debug, Clone, Default)]
struct StyleDeclarations {
    normal: DeclarationLayer,
    important: DeclarationLayer,
}

#[derive(Debug, Clone, Default)]
struct DeclarationLayer {
    cell: CellStyle,
    display: Option<CssDisplay>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct Specificity {
    ids: usize,
    classes: usize,
    elements: usize,
}

impl Stylesheet {
    fn build_indexes(&mut self) {
        self.tag_rules.clear();
        self.class_rules.clear();
        self.id_rules.clear();
        self.universal_rules.clear();
        self.ancestor_tag_qualifiers.clear();
        self.ancestor_class_qualifiers.clear();
        self.has_structural_combinator = false;
        self.has_sibling_combinator = false;
        self.needs_precise_match = false;

        for (index, rule) in self.rules.iter().enumerate() {
            // Index by the subject (rightmost) compound. A subject with a tag,
            // id, or class is indexed by those; anything else (attribute-only,
            // pseudo-only, or universal) falls into the always-considered bucket.
            let subject = &rule.selector.subject;
            if compound_needs_precise(subject)
                || rule
                    .selector
                    .context
                    .iter()
                    .any(|(_, compound)| compound_needs_precise(compound))
            {
                self.needs_precise_match = true;
            }
            let mut indexed = false;
            if let Some(tag) = &subject.tag {
                self.tag_rules.entry(tag.clone()).or_default().push(index);
                indexed = true;
            }
            if let Some(id) = &subject.id {
                self.id_rules.entry(id.clone()).or_default().push(index);
                indexed = true;
            }
            for class in &subject.classes {
                self.class_rules
                    .entry(class.clone())
                    .or_default()
                    .push(index);
                indexed = true;
            }
            if !indexed {
                self.universal_rules.push(index);
            }

            for (position, (combinator, compound)) in rule.selector.context.iter().enumerate() {
                match combinator {
                    Combinator::Descendant => {}
                    Combinator::Child => self.has_structural_combinator = true,
                    Combinator::NextSibling | Combinator::SubsequentSibling => {
                        self.has_structural_combinator = true;
                        // A subject-adjacent sibling (`a + b`) is captured cheaply
                        // by the subject's preceding-sibling signature. A sibling
                        // deeper in the chain (`.x + .y .z`) would require scanning
                        // an ancestor's siblings, so fall back to a per-element key.
                        if position == 0 {
                            self.has_sibling_combinator = true;
                        } else {
                            self.needs_precise_match = true;
                        }
                    }
                }
                if let Some(tag) = &compound.tag {
                    self.ancestor_tag_qualifiers.insert(tag.clone());
                }
                for class in &compound.classes {
                    self.ancestor_class_qualifiers.insert(class.clone());
                }
            }
        }
    }

    fn computed_declarations(
        &self,
        dom: &crate::dom::Dom,
        id: crate::dom::NodeId,
        tag: &str,
        classes: &[&str],
    ) -> StyleDeclarations {
        let mut candidate_indexes: Vec<usize> = self.universal_rules.clone();

        if let Some(indexes) = self.tag_rules.get(tag) {
            candidate_indexes.extend(indexes.iter().copied());
        }

        for class in classes {
            if let Some(indexes) = self.class_rules.get(*class) {
                candidate_indexes.extend(indexes.iter().copied());
            }
        }

        if !self.id_rules.is_empty() {
            if let Some(element_id) = dom.node(id).attr("id") {
                if let Some(indexes) = self.id_rules.get(element_id) {
                    candidate_indexes.extend(indexes.iter().copied());
                }
            }
        }

        if candidate_indexes.is_empty() {
            return StyleDeclarations::default();
        }

        candidate_indexes.sort_unstable();
        candidate_indexes.dedup();

        let mut matched = candidate_indexes
            .into_iter()
            .map(|index| &self.rules[index])
            .filter(|rule| rule.selector.matches(dom, id))
            .collect::<Vec<_>>();

        matched.sort_by_key(|rule| (rule.specificity, rule.order));

        let mut declarations = StyleDeclarations::default();
        for rule in matched {
            declarations.merge(rule.declarations.clone());
        }

        declarations
    }
}

impl SimpleSelector {
    /// Match the selector against element `id` by walking the tree right-to-left.
    /// The subject compound must match `id`; then each `(combinator, compound)`
    /// moves a cursor to the relevant relative (parent, ancestor, or preceding
    /// sibling) and requires a match there. Because the ancestor chain is linear,
    /// a single leftward cursor is exact for every combinator, including chains.
    fn matches(&self, dom: &crate::dom::Dom, id: crate::dom::NodeId) -> bool {
        if !compound_matches_node(dom, id, &self.subject) {
            return false;
        }

        let mut cursor = id;
        for (combinator, compound) in &self.context {
            match combinator {
                Combinator::Child => {
                    let Some(parent) = element_parent(dom, cursor) else {
                        return false;
                    };
                    if !compound_matches_node(dom, parent, compound) {
                        return false;
                    }
                    cursor = parent;
                }
                Combinator::Descendant => {
                    let mut candidate = element_parent(dom, cursor);
                    loop {
                        let Some(ancestor) = candidate else {
                            return false;
                        };
                        if compound_matches_node(dom, ancestor, compound) {
                            cursor = ancestor;
                            break;
                        }
                        candidate = element_parent(dom, ancestor);
                    }
                }
                Combinator::NextSibling => {
                    let Some(prev) = prev_element_sibling(dom, cursor) else {
                        return false;
                    };
                    if !compound_matches_node(dom, prev, compound) {
                        return false;
                    }
                    cursor = prev;
                }
                Combinator::SubsequentSibling => {
                    let mut candidate = prev_element_sibling(dom, cursor);
                    loop {
                        let Some(sibling) = candidate else {
                            return false;
                        };
                        if compound_matches_node(dom, sibling, compound) {
                            cursor = sibling;
                            break;
                        }
                        candidate = prev_element_sibling(dom, sibling);
                    }
                }
            }
        }
        true
    }

    fn specificity(&self) -> Specificity {
        let mut spec = compound_specificity(&self.subject);
        for (_, compound) in &self.context {
            let part = compound_specificity(compound);
            spec.ids += part.ids;
            spec.classes += part.classes;
            spec.elements += part.elements;
        }
        spec
    }
}

/// CSS specificity of one compound: ids count as `id`; classes, attribute
/// selectors, and pseudo-classes as `class`; a type tag as `element`. `:not()`
/// contributes the specificity of its most specific argument (the `:not` itself
/// counts for nothing), per the cascade spec.
fn compound_specificity(compound: &Compound) -> Specificity {
    let mut spec = Specificity {
        ids: usize::from(compound.id.is_some()),
        classes: compound.classes.len() + compound.attrs.len(),
        elements: usize::from(compound.tag.is_some()),
    };
    for pseudo in &compound.pseudos {
        if let PseudoClass::Not(compounds) = pseudo {
            if let Some(max) = compounds.iter().map(compound_specificity).max() {
                spec.ids += max.ids;
                spec.classes += max.classes;
                spec.elements += max.elements;
            }
        } else {
            spec.classes += 1;
        }
    }
    spec
}

/// Whether a compound depends on element identity/position beyond tag/class —
/// i.e. it uses an id, attribute selector, or pseudo-class. Such selectors need
/// the per-element cache key.
fn compound_needs_precise(compound: &Compound) -> bool {
    compound.id.is_some() || !compound.attrs.is_empty() || !compound.pseudos.is_empty()
}

/// Whether `compound` matches the element node `id` (non-elements never match).
fn compound_matches_node(
    dom: &crate::dom::Dom,
    id: crate::dom::NodeId,
    compound: &Compound,
) -> bool {
    let node = dom.node(id);
    let Some(tag) = node.tag() else {
        return false;
    };
    if let Some(selector_tag) = &compound.tag {
        if selector_tag != tag {
            return false;
        }
    }
    if let Some(selector_id) = &compound.id {
        if node.attr("id") != Some(selector_id.as_str()) {
            return false;
        }
    }
    let classes = node.classes().collect::<Vec<_>>();
    if !compound
        .classes
        .iter()
        .all(|class| classes.iter().any(|candidate| candidate == class))
    {
        return false;
    }
    if !compound.attrs.iter().all(|attr| attr_matches(node, attr)) {
        return false;
    }
    compound
        .pseudos
        .iter()
        .all(|pseudo| pseudo_matches(dom, id, pseudo))
}

/// Whether an attribute selector matches an element node. Attribute names are
/// compared case-insensitively (HTML lowercases them at parse time); an empty
/// operand never matches for the substring-style operators, per the spec.
fn attr_matches(node: &crate::dom::Node, attr: &AttrSelector) -> bool {
    let Some(value) = node.attr(&attr.name) else {
        return false;
    };
    match &attr.op {
        AttrOp::Exists => true,
        AttrOp::Equals(want) => value == want,
        AttrOp::Includes(want) => !want.is_empty() && value.split_whitespace().any(|w| w == want),
        AttrOp::DashMatch(want) => value == want || value.starts_with(&format!("{want}-")),
        AttrOp::Prefix(want) => !want.is_empty() && value.starts_with(want.as_str()),
        AttrOp::Suffix(want) => !want.is_empty() && value.ends_with(want.as_str()),
        AttrOp::Substring(want) => !want.is_empty() && value.contains(want.as_str()),
    }
}

/// Whether a structural pseudo-class matches the element node `id`.
fn pseudo_matches(dom: &crate::dom::Dom, id: crate::dom::NodeId, pseudo: &PseudoClass) -> bool {
    match pseudo {
        PseudoClass::FirstChild => element_position(dom, id, None) == 1,
        PseudoClass::LastChild => element_position_from_end(dom, id, None) == 1,
        PseudoClass::OnlyChild => element_sibling_count(dom, id, None) == 1,
        PseudoClass::NthChild(a, b) => nth_matches(element_position(dom, id, None), *a, *b),
        PseudoClass::NthLastChild(a, b) => {
            nth_matches(element_position_from_end(dom, id, None), *a, *b)
        }
        PseudoClass::FirstOfType => element_position(dom, id, dom.node(id).tag()) == 1,
        PseudoClass::LastOfType => element_position_from_end(dom, id, dom.node(id).tag()) == 1,
        PseudoClass::OnlyOfType => element_sibling_count(dom, id, dom.node(id).tag()) == 1,
        PseudoClass::NthOfType(a, b) => {
            nth_matches(element_position(dom, id, dom.node(id).tag()), *a, *b)
        }
        PseudoClass::NthLastOfType(a, b) => {
            nth_matches(element_position_from_end(dom, id, dom.node(id).tag()), *a, *b)
        }
        PseudoClass::Empty => is_empty_element(dom, id),
        PseudoClass::Root => element_parent(dom, id).is_none(),
        PseudoClass::Not(compounds) => {
            compounds.iter().all(|c| !compound_matches_node(dom, id, c))
        }
    }
}

/// The 1-based position of `id` among its element siblings, optionally restricted
/// to a single tag name (for `-of-type` pseudo-classes).
fn element_position(dom: &crate::dom::Dom, id: crate::dom::NodeId, of_type: Option<&str>) -> usize {
    element_siblings(dom, id, of_type)
        .iter()
        .position(|&sibling| sibling == id)
        .map_or(0, |index| index + 1)
}

/// The 1-based position of `id` counted from the last element sibling.
fn element_position_from_end(
    dom: &crate::dom::Dom,
    id: crate::dom::NodeId,
    of_type: Option<&str>,
) -> usize {
    let siblings = element_siblings(dom, id, of_type);
    siblings
        .iter()
        .rev()
        .position(|&sibling| sibling == id)
        .map_or(0, |index| index + 1)
}

fn element_sibling_count(
    dom: &crate::dom::Dom,
    id: crate::dom::NodeId,
    of_type: Option<&str>,
) -> usize {
    element_siblings(dom, id, of_type).len()
}

/// The element siblings of `id` (including `id`), in document order. Restricted
/// to `of_type` when given. If `id` has no element parent it is its own sole
/// sibling, so `:first-child`/`:root`-style checks behave sensibly.
fn element_siblings(
    dom: &crate::dom::Dom,
    id: crate::dom::NodeId,
    of_type: Option<&str>,
) -> Vec<crate::dom::NodeId> {
    let Some(parent) = element_parent(dom, id) else {
        return vec![id];
    };
    dom.node(parent)
        .children
        .iter()
        .copied()
        .filter(|&child| match dom.node(child).tag() {
            Some(tag) => of_type.is_none_or(|want| want == tag),
            None => false,
        })
        .collect()
}

/// An element is `:empty` when it has no element children and no non-whitespace
/// text (comments are ignored).
fn is_empty_element(dom: &crate::dom::Dom, id: crate::dom::NodeId) -> bool {
    dom.node(id).children.iter().all(|&child| {
        let node = dom.node(child);
        match &node.data {
            crate::dom::NodeData::Element { .. } => false,
            crate::dom::NodeData::Text(text) => text.trim().is_empty(),
            _ => true,
        }
    })
}

/// Whether `position` (1-based) satisfies the `an+b` pattern. `position == 0`
/// means "not in the set" (e.g. a wrong-of-type element) and never matches.
fn nth_matches(position: usize, a: i32, b: i32) -> bool {
    if position == 0 {
        return false;
    }
    let position = position as i32;
    if a == 0 {
        return position == b;
    }
    // position = a*n + b for some integer n >= 0.
    let offset = position - b;
    offset % a == 0 && offset / a >= 0
}

/// The nearest ancestor of `id` that is an element node.
fn element_parent(dom: &crate::dom::Dom, id: crate::dom::NodeId) -> Option<crate::dom::NodeId> {
    let mut current = dom.node(id).parent;
    while let Some(node_id) = current {
        if dom.node(node_id).tag().is_some() {
            return Some(node_id);
        }
        current = dom.node(node_id).parent;
    }
    None
}

/// The immediately preceding element sibling of `id`, skipping text/comment nodes.
fn prev_element_sibling(
    dom: &crate::dom::Dom,
    id: crate::dom::NodeId,
) -> Option<crate::dom::NodeId> {
    let parent = dom.node(id).parent?;
    let siblings = &dom.node(parent).children;
    let position = siblings.iter().position(|&child| child == id)?;
    siblings[..position]
        .iter()
        .rev()
        .copied()
        .find(|&sibling| dom.node(sibling).tag().is_some())
}

/// A cache-key signature capturing exactly the tree context that can change a
/// selector match for element `id`, restricted to tokens that actually appear as
/// combinator qualifiers in the stylesheet. Two elements with the same subject
/// identity and the same signature always cascade identically.
///
/// - No combinator qualifiers: empty (every element shares one key).
/// - Descendant only: the unordered set of relevant ancestor tokens. Order and
///   depth cannot matter, so this maximizes cache sharing.
/// - Any `>`/`+`/`~`: an ordered, per-level fingerprint (nearest-first) of each
///   ancestor's relevant tokens, plus — when sibling combinators exist — the
///   subject's relevant preceding-sibling tokens. Ancestor-level sibling context
///   (`.x + .y .z`, a sibling combinator that is not adjacent to the subject) is
///   handled by falling back to a per-element cache key (`needs_precise_match`),
///   so this stays O(depth + subject siblings) rather than scanning every
///   ancestor's siblings (which is O(n²) on a big table body).
fn structural_signature(
    dom: &crate::dom::Dom,
    id: crate::dom::NodeId,
    stylesheet: &Stylesheet,
) -> String {
    if stylesheet.ancestor_tag_qualifiers.is_empty()
        && stylesheet.ancestor_class_qualifiers.is_empty()
    {
        return String::new();
    }

    let relevant = |node_id: crate::dom::NodeId| -> Vec<String> {
        let node = dom.node(node_id);
        let mut tokens: Vec<String> = Vec::new();
        if let Some(tag) = node.tag() {
            if stylesheet.ancestor_tag_qualifiers.contains(tag) {
                tokens.push(tag.to_string());
            }
        }
        for class in node.classes() {
            if stylesheet.ancestor_class_qualifiers.contains(class) {
                tokens.push(class.to_string());
            }
        }
        tokens.sort();
        tokens
    };

    if !stylesheet.has_structural_combinator {
        // Descendant only: an order-independent set of relevant ancestor tokens.
        let mut tokens: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
        let mut current = element_parent(dom, id);
        while let Some(node_id) = current {
            tokens.extend(relevant(node_id));
            current = element_parent(dom, node_id);
        }
        return tokens.into_iter().collect::<Vec<_>>().join(",");
    }

    // Structural combinators: an ordered per-level fingerprint of ancestors'
    // own tokens (for `>`/descendant). The subject also records its relevant
    // preceding-sibling tokens (for a subject-adjacent `+`/`~`); ancestor-level
    // sibling combinators are covered by `needs_precise_match` instead, so we
    // never scan an ancestor's siblings here.
    let mut levels: Vec<String> = Vec::new();

    let mut subject = relevant(id).join(".");
    if stylesheet.has_sibling_combinator {
        let mut siblings: Vec<String> = Vec::new();
        let mut prev = prev_element_sibling(dom, id);
        while let Some(sibling_id) = prev {
            let tokens = relevant(sibling_id);
            if !tokens.is_empty() {
                siblings.push(tokens.join("."));
            }
            prev = prev_element_sibling(dom, sibling_id);
        }
        subject = format!("{subject}+{}", siblings.join(","));
    }
    levels.push(subject);

    let mut current = element_parent(dom, id);
    while let Some(node_id) = current {
        levels.push(relevant(node_id).join("."));
        current = element_parent(dom, node_id);
    }
    levels.join("/")
}

impl StyleDeclarations {
    fn merge(&mut self, other: StyleDeclarations) {
        self.normal.merge(other.normal);
        self.important.merge(other.important);
    }

    fn merge_inline(&mut self, other: StyleDeclarations) {
        self.normal.merge(other.normal);
        self.important.merge(other.important);
    }

    fn resolved(&self) -> DeclarationLayer {
        let mut resolved = self.normal.clone();
        resolved.merge(self.important.clone());
        resolved
    }
}

impl DeclarationLayer {
    fn merge(&mut self, other: DeclarationLayer) {
        self.cell.merge(other.cell);
        self.display = other.display.or(self.display);
    }
}

/// Concatenate the text content of every `<style>` element in the document.
/// Reading from the parsed DOM is robust where the old `<style>` substring scan
/// was not (attributes on the tag, commented-out tags, etc.).
fn collect_style_css(dom: &crate::dom::Dom) -> String {
    let mut css = String::new();
    for node in &dom.nodes {
        if node.tag() == Some("style") {
            for &child in &node.children {
                if let crate::dom::NodeData::Text(text) = &dom.node(child).data {
                    css.push_str(text);
                    css.push('\n');
                }
            }
        }
    }
    css
}

/// Parse a stylesheet using `cssparser`'s tokenizer for robust rule, selector
/// list, declaration, comment, string, and `@media` handling. The cascade model
/// (specificity, source order, `!important`) and the value/selector parsers are
/// reused unchanged, so output stays identical for inputs the old hand-rolled
/// tokenizer already handled while gaining correctness on the ones it did not
/// (comments anywhere, `;`/`{` inside strings or `url()`, nested blocks).
fn parse_stylesheet(css: &str) -> Stylesheet {
    let mut stylesheet = Stylesheet::default();
    let mut order = 0;

    let mut input = ParserInput::new(css);
    let mut parser = Parser::new(&mut input);
    let mut rule_parser = RuleParser;
    let mut rules = StyleSheetParser::new(&mut parser, &mut rule_parser);

    while let Some(result) = rules.next() {
        let Ok(parsed) = result else { continue };
        for (selector, declarations) in parsed {
            stylesheet.rules.push(StyleRule {
                specificity: selector.specificity(),
                selector,
                declarations,
                order,
            });
            order += 1;
        }
    }

    stylesheet.build_indexes();
    stylesheet
}

/// Parse an inline `style="..."` declaration list with `cssparser`.
fn parse_style_declarations(declarations: &str) -> StyleDeclarations {
    let mut parsed = StyleDeclarations::default();

    let mut input = ParserInput::new(declarations);
    let mut parser = Parser::new(&mut input);
    let mut decl_parser = DeclParser {
        declarations: &mut parsed,
    };
    let mut items = RuleBodyParser::new(&mut parser, &mut decl_parser);
    while let Some(result) = items.next() {
        let _ = result;
    }

    parsed
}

/// A parsed style rule's prelude is a selector list; its block is a declaration
/// list. One comma-separated rule expands into several `(selector, decls)` pairs.
type ParsedRule = (SimpleSelector, StyleDeclarations);

struct RuleParser;

enum AtRuleKind {
    /// `@media`: parse the nested block as top-level rules if the query applies
    /// to print output (the PDF target); the boolean is that decision.
    Media(bool),
    /// `@page`: parsed by the geometry parser for margins and orientation.
    Page,
    /// Any other at-rule: ignored.
    Other,
}

/// Whether an `@media` query applies to print output (the PDF target). Screen-
/// only queries are excluded; `print`, `all`, unqualified, and feature queries
/// apply. A pragmatic first pass, not a full media-query evaluator.
fn media_applies_to_print(query: &str) -> bool {
    let query = query.to_ascii_lowercase();
    if query.contains("print") {
        true
    } else if query.contains("screen") {
        false
    } else {
        true
    }
}

impl<'i> QualifiedRuleParser<'i> for RuleParser {
    type Prelude = Vec<SimpleSelector>;
    type QualifiedRule = Vec<ParsedRule>;
    type Error = ();

    fn parse_prelude<'t>(
        &mut self,
        input: &mut Parser<'i, 't>,
    ) -> Result<Self::Prelude, ParseError<'i, ()>> {
        Ok(parse_selector_list(input))
    }

    fn parse_block<'t>(
        &mut self,
        prelude: Self::Prelude,
        _start: &ParserState,
        input: &mut Parser<'i, 't>,
    ) -> Result<Self::QualifiedRule, ParseError<'i, ()>> {
        let declarations = parse_declaration_block(input);
        Ok(prelude
            .into_iter()
            .map(|selector| (selector, declarations.clone()))
            .collect())
    }
}

impl<'i> AtRuleParser<'i> for RuleParser {
    type Prelude = AtRuleKind;
    type AtRule = Vec<ParsedRule>;
    type Error = ();

    fn parse_prelude<'t>(
        &mut self,
        name: CowRcStr<'i>,
        input: &mut Parser<'i, 't>,
    ) -> Result<Self::Prelude, ParseError<'i, ()>> {
        let query_start = input.position();
        consume_remaining(input);
        if name.eq_ignore_ascii_case("media") {
            let query = input.slice_from(query_start);
            Ok(AtRuleKind::Media(media_applies_to_print(query)))
        } else {
            Ok(AtRuleKind::Other)
        }
    }

    fn parse_block<'t>(
        &mut self,
        prelude: Self::Prelude,
        _start: &ParserState,
        input: &mut Parser<'i, 't>,
    ) -> Result<Self::AtRule, ParseError<'i, ()>> {
        match prelude {
            // Only descend into `@media` blocks whose query applies to print.
            AtRuleKind::Media(true) => {
                let mut inner = RuleParser;
                let mut rules = StyleSheetParser::new(input, &mut inner);
                let mut collected = Vec::new();
                while let Some(result) = rules.next() {
                    if let Ok(mut parsed) = result {
                        collected.append(&mut parsed);
                    }
                }
                Ok(collected)
            }
            // Screen-only `@media`, `@page` (no cascade rules; geometry is handled
            // by `parse_page_geometry`), and other at-rules contribute nothing.
            AtRuleKind::Media(false) | AtRuleKind::Page | AtRuleKind::Other => Ok(Vec::new()),
        }
    }

    fn rule_without_block(
        &mut self,
        _prelude: Self::Prelude,
        _start: &ParserState,
    ) -> Result<Self::AtRule, ()> {
        // At-rules without a block (e.g. `@import`, `@charset`) contribute no
        // style rules to the cascade.
        Ok(Vec::new())
    }
}

/// Parse a declaration list (a rule's `{ ... }` body) into a `StyleDeclarations`.
fn parse_declaration_block(input: &mut Parser<'_, '_>) -> StyleDeclarations {
    let mut declarations = StyleDeclarations::default();
    let mut decl_parser = DeclParser {
        declarations: &mut declarations,
    };
    let mut items = RuleBodyParser::new(input, &mut decl_parser);
    while let Some(result) = items.next() {
        let _ = result;
    }
    declarations
}

/// Applies each declaration into the normal/important layers, reusing the
/// existing value normalization and property mapping.
struct DeclParser<'a> {
    declarations: &'a mut StyleDeclarations,
}

impl<'i> DeclarationParser<'i> for DeclParser<'_> {
    type Declaration = ();
    type Error = ();

    fn parse_value<'t>(
        &mut self,
        name: CowRcStr<'i>,
        input: &mut Parser<'i, 't>,
        _start: &ParserState,
    ) -> Result<(), ParseError<'i, ()>> {
        let value_start = input.position();
        consume_remaining(input);
        let raw_value = input.slice_from(value_start);

        let property = name.to_ascii_lowercase();
        let (value, important) = normalize_declaration_value(raw_value);
        let layer = if important {
            &mut self.declarations.important
        } else {
            &mut self.declarations.normal
        };

        apply_style_declaration(layer, &property, &value);
        Ok(())
    }
}

// Declaration lists may, per the CSS syntax spec, contain at-rules and (in
// nesting) qualified rules. We don't support those inside a block, so reject
// them with empty implementations and only opt into declaration parsing.
impl<'i> AtRuleParser<'i> for DeclParser<'_> {
    type Prelude = ();
    type AtRule = ();
    type Error = ();
}

impl<'i> QualifiedRuleParser<'i> for DeclParser<'_> {
    type Prelude = ();
    type QualifiedRule = ();
    type Error = ();
}

impl<'i> RuleBodyItemParser<'i, (), ()> for DeclParser<'_> {
    fn parse_declarations(&self) -> bool {
        true
    }

    fn parse_qualified(&self) -> bool {
        false
    }
}

/// Consume the rest of a delimited `cssparser` parser, descending into and
/// skipping any nested `{}`/`()`/`[]`/function blocks, so a following
/// `slice_from` captures the full source text of a prelude or declaration value.
fn consume_remaining(input: &mut Parser<'_, '_>) {
    while let Ok(token) = input.next_including_whitespace().map(|token| token.clone()) {
        if matches!(
            token,
            Token::Function(_)
                | Token::ParenthesisBlock
                | Token::SquareBracketBlock
                | Token::CurlyBracketBlock
        ) {
            let _ = input.parse_nested_block(|nested| -> Result<(), ParseError<'_, ()>> {
                consume_remaining(nested);
                Ok(())
            });
        }
    }
}

/// Parse a comma-separated selector list from `cssparser` tokens into the
/// engine's `SimpleSelector` model. Tokenizing rather than string-splitting means
/// comments inside selectors are skipped and `,`/combinators inside blocks (e.g.
/// `:not(...)`, attribute selectors) do not split the list incorrectly.
///
/// Supported: type, universal (`*`), id (`#x`), class, attribute (`[a=b]` and the
/// `~= |= ^= $= *=` operators), the four combinators, and structural
/// pseudo-classes (`:first-child`, `:nth-child()`, `:*-of-type`, `:empty`,
/// `:root`, `:not()`). Dynamic pseudo-classes (`:hover`, …), pseudo-elements
/// (`::before`), and anything else unsupported drop the whole selector so it
/// never applies to the static print render (rather than over-matching).
fn parse_selector_list<'i>(input: &mut Parser<'i, '_>) -> Vec<SimpleSelector> {
    let mut selectors = Vec::new();
    let mut current = CompoundBuilder::default();

    while let Ok(token) = input.next_including_whitespace().map(|token| token.clone()) {
        // A pending `.` or `:` must be followed by an identifier / function.
        if current.expect_class && !matches!(token, Token::Ident(_)) {
            current.reject();
        }
        if current.colon_count > 0
            && !matches!(token, Token::Ident(_) | Token::Function(_))
        {
            current.reject();
        }

        match token {
            Token::Comma => {
                current.finish_into(&mut selectors);
                current = CompoundBuilder::default();
            }
            // Combinators only separate compounds when another compound actually
            // follows. Defer the reset so trailing whitespace before `{` (or
            // before a comma) does not wipe the rightmost compound. An explicit
            // `>`/`+`/`~` overrides the descendant combinator implied by nearby
            // whitespace (e.g. `A > B` tokenizes as `A`, WS, `>`, WS, `B`).
            Token::WhiteSpace(_) => current.set_descendant_pending(),
            Token::Delim('>') => current.pending = Some(Combinator::Child),
            Token::Delim('+') => current.pending = Some(Combinator::NextSibling),
            Token::Delim('~') => current.pending = Some(Combinator::SubsequentSibling),
            Token::Delim('.') => {
                current.begin_compound();
                current.expect_class = true;
            }
            Token::Ident(name) => {
                current.begin_compound();
                if current.expect_class {
                    // Class names are case-sensitive; type names are lowercased.
                    current.current.classes.push(name.to_string());
                    current.expect_class = false;
                } else if current.colon_count > 0 {
                    current.finish_pseudo_ident(&name);
                } else {
                    current.push_type(&name);
                }
            }
            Token::Colon => {
                current.begin_compound();
                current.colon_count = current.colon_count.saturating_add(1);
            }
            Token::IDHash(name) | Token::Hash(name) => {
                current.begin_compound();
                current.set_id(&name);
            }
            Token::Delim('*') => {
                current.begin_compound();
                current.set_universal();
            }
            // `[attr]` attribute selector.
            Token::SquareBracketBlock => {
                current.begin_compound();
                let parsed = input.parse_nested_block(
                    |nested| -> Result<Option<AttrSelector>, ParseError<'_, ()>> {
                        let attr = parse_attribute_selector(nested);
                        consume_remaining(nested);
                        Ok(attr)
                    },
                );
                match parsed {
                    Ok(Some(attr)) => current.current.attrs.push(attr),
                    _ => current.reject(),
                }
            }
            // A function is only valid as a functional pseudo-class (`:not(...)`,
            // `:nth-child(...)`), i.e. immediately after a single colon.
            Token::Function(name) => {
                let single_colon = current.colon_count == 1;
                let raw = input
                    .parse_nested_block(|nested| -> Result<String, ParseError<'_, ()>> {
                        let start = nested.position();
                        consume_remaining(nested);
                        Ok(nested.slice_from(start).to_string())
                    })
                    .unwrap_or_default();
                current.colon_count = 0;
                match functional_pseudo(&name, &raw) {
                    Some(pseudo) if single_colon => current.current.pseudos.push(pseudo),
                    _ => current.reject(),
                }
            }
            // Parenthesis/curly blocks are not valid selector syntax here.
            Token::ParenthesisBlock | Token::CurlyBracketBlock => {
                current.reject();
                let _ = input.parse_nested_block(|nested| -> Result<(), ParseError<'_, ()>> {
                    consume_remaining(nested);
                    Ok(())
                });
            }
            _ => current.reject(),
        }
    }

    current.finish_into(&mut selectors);
    selectors
}

/// Parse the inside of an attribute selector `[ ... ]`. Returns `None` on any
/// malformed input so the caller can drop the selector.
fn parse_attribute_selector(input: &mut Parser<'_, '_>) -> Option<AttrSelector> {
    let name = match input.next() {
        Ok(Token::Ident(n)) => n.as_ref().to_ascii_lowercase(),
        _ => return None,
    };
    let op_kind = match input.next() {
        // `[name]` — presence only.
        Err(_) => return Some(AttrSelector { name, op: AttrOp::Exists }),
        Ok(Token::Delim('=')) => 0u8,
        Ok(Token::IncludeMatch) => 1,
        Ok(Token::DashMatch) => 2,
        Ok(Token::PrefixMatch) => 3,
        Ok(Token::SuffixMatch) => 4,
        Ok(Token::SubstringMatch) => 5,
        Ok(_) => return None,
    };
    let value = match input.next() {
        Ok(Token::Ident(v)) => v.as_ref().to_string(),
        Ok(Token::QuotedString(v)) => v.as_ref().to_string(),
        _ => return None,
    };
    let op = match op_kind {
        0 => AttrOp::Equals(value),
        1 => AttrOp::Includes(value),
        2 => AttrOp::DashMatch(value),
        3 => AttrOp::Prefix(value),
        4 => AttrOp::Suffix(value),
        _ => AttrOp::Substring(value),
    };
    Some(AttrSelector { name, op })
}

/// Map a simple (non-functional) pseudo-class name to a supported variant.
fn simple_pseudo(name: &str) -> Option<PseudoClass> {
    Some(match name.to_ascii_lowercase().as_str() {
        "first-child" => PseudoClass::FirstChild,
        "last-child" => PseudoClass::LastChild,
        "only-child" => PseudoClass::OnlyChild,
        "first-of-type" => PseudoClass::FirstOfType,
        "last-of-type" => PseudoClass::LastOfType,
        "only-of-type" => PseudoClass::OnlyOfType,
        "empty" => PseudoClass::Empty,
        "root" => PseudoClass::Root,
        _ => return None,
    })
}

/// Map a functional pseudo-class (`name(raw)`) to a supported variant.
fn functional_pseudo(name: &str, raw: &str) -> Option<PseudoClass> {
    match name.to_ascii_lowercase().as_str() {
        "nth-child" => parse_an_plus_b(raw).map(|(a, b)| PseudoClass::NthChild(a, b)),
        "nth-last-child" => parse_an_plus_b(raw).map(|(a, b)| PseudoClass::NthLastChild(a, b)),
        "nth-of-type" => parse_an_plus_b(raw).map(|(a, b)| PseudoClass::NthOfType(a, b)),
        "nth-last-of-type" => {
            parse_an_plus_b(raw).map(|(a, b)| PseudoClass::NthLastOfType(a, b))
        }
        "not" => parse_not_argument(raw).map(PseudoClass::Not),
        _ => None,
    }
}

/// Parse an `<An+B>` micro-syntax (`odd`, `even`, `3`, `2n`, `2n+1`, `-n+3`, …)
/// into `(a, b)`.
fn parse_an_plus_b(raw: &str) -> Option<(i32, i32)> {
    let compact: String = raw.chars().filter(|c| !c.is_whitespace()).collect();
    let compact = compact.to_ascii_lowercase();
    match compact.as_str() {
        "" => return None,
        "odd" => return Some((2, 1)),
        "even" => return Some((2, 0)),
        _ => {}
    }
    if let Some(n_pos) = compact.find('n') {
        let a_part = &compact[..n_pos];
        let b_part = &compact[n_pos + 1..];
        let a = match a_part {
            "" | "+" => 1,
            "-" => -1,
            other => other.parse::<i32>().ok()?,
        };
        let b = if b_part.is_empty() {
            0
        } else {
            b_part.parse::<i32>().ok()?
        };
        Some((a, b))
    } else {
        Some((0, compact.parse::<i32>().ok()?))
    }
}

/// Parse a `:not(...)` argument as a list of compound selectors. Combinators
/// inside `:not()` are unsupported (returns `None` to drop the outer selector).
fn parse_not_argument(raw: &str) -> Option<Vec<Compound>> {
    let mut input = ParserInput::new(raw);
    let mut parser = Parser::new(&mut input);
    let selectors = parse_selector_list(&mut parser);
    if selectors.is_empty() {
        return None;
    }
    let mut compounds = Vec::new();
    for selector in selectors {
        if !selector.context.is_empty() {
            return None;
        }
        compounds.push(selector.subject);
    }
    Some(compounds)
}

#[derive(Default)]
struct CompoundBuilder {
    /// The compound currently being accumulated (the eventual subject).
    current: Compound,
    rejected: bool,
    expect_class: bool,
    /// Consecutive leading colons on the pending pseudo (`0` none, `1` for `:`,
    /// `2` for `::` = a pseudo-element, which is unsupported).
    colon_count: u8,
    /// The combinator seen since the current compound, if any, linking it to the
    /// compound that follows.
    pending: Option<Combinator>,
    /// Completed left-hand compounds with the combinator linking each to the
    /// compound on its right. Built farthest-first; reversed at finish.
    context: Vec<(Combinator, Compound)>,
}

impl CompoundBuilder {
    /// Whitespace implies a descendant combinator, but only when no explicit
    /// combinator is already pending (so the whitespace around `>`/`+`/`~` does
    /// not downgrade it back to descendant).
    fn set_descendant_pending(&mut self) {
        if self.pending.is_none() {
            self.pending = Some(Combinator::Descendant);
        }
    }

    /// Apply a deferred combinator: if one is pending, the tokens seen so far
    /// belonged to a *context* compound. Stash it with its combinator and begin
    /// a fresh compound for what follows (the new rightmost).
    fn begin_compound(&mut self) {
        if let Some(combinator) = self.pending.take() {
            if !self.current.is_empty() {
                self.context
                    .push((combinator, std::mem::take(&mut self.current)));
            }
            self.current = Compound::default();
            self.expect_class = false;
            self.colon_count = 0;
        }
    }

    fn reject(&mut self) {
        self.rejected = true;
    }

    /// A bare identifier in type position sets the tag; anywhere else it is
    /// malformed (a type selector must lead its compound).
    fn push_type(&mut self, name: &str) {
        if self.current.is_empty() {
            self.current.tag = Some(name.to_ascii_lowercase());
        } else {
            self.reject();
        }
    }

    /// A `#id`. Two ids in one compound is malformed.
    fn set_id(&mut self, id: &str) {
        if self.current.id.is_some() {
            self.reject();
        } else {
            self.current.id = Some(id.to_string());
        }
    }

    /// A `*` universal selector (valid only in type position).
    fn set_universal(&mut self) {
        if self.current.is_empty() {
            self.current.universal = true;
        } else {
            self.reject();
        }
    }

    /// A simple pseudo-class identifier following `:` (rejects `::` pseudo-
    /// elements and unsupported/dynamic pseudo-classes).
    fn finish_pseudo_ident(&mut self, name: &str) {
        let single_colon = self.colon_count == 1;
        self.colon_count = 0;
        match simple_pseudo(name) {
            Some(pseudo) if single_colon => self.current.pseudos.push(pseudo),
            _ => self.reject(),
        }
    }

    fn finish_into(mut self, out: &mut Vec<SimpleSelector>) {
        if self.rejected
            || self.expect_class
            || self.colon_count > 0
            || self.current.is_empty()
        {
            return;
        }
        // Stored farthest-first while parsing; matching walks nearest-first.
        self.context.reverse();
        out.push(SimpleSelector {
            subject: self.current,
            context: self.context,
        });
    }
}

fn normalize_declaration_value(value: &str) -> (String, bool) {
    let trimmed = value.trim();
    let lower = trimmed.to_ascii_lowercase();
    let important = lower.ends_with("!important");
    let value = if important {
        trimmed[..trimmed.len() - "!important".len()].trim()
    } else {
        trimmed
    };

    (value.to_string(), important)
}

/// Parse the `flex` shorthand: `none` / `auto` / `initial`, or
/// `<grow> [<shrink>] [<basis>]`. Only grow and basis are recorded (shrink
/// defaults to 1 in layout). `flex: 1` means grow 1 with a 0 basis.
fn apply_flex_shorthand(target: &mut DeclarationLayer, value: &str) {
    let v = value.trim();
    match v.to_ascii_lowercase().as_str() {
        "none" => {
            target.cell.flex_grow = Some(0.0);
            target.cell.flex_basis = Some(0.0);
            return;
        }
        "auto" => {
            target.cell.flex_grow = Some(1.0);
            target.cell.flex_basis = None;
            return;
        }
        "initial" => {
            target.cell.flex_grow = Some(0.0);
            target.cell.flex_basis = None;
            return;
        }
        _ => {}
    }
    let tokens: Vec<&str> = v.split_whitespace().collect();
    if let Some(g) = tokens.first().and_then(|t| t.parse::<f32>().ok()) {
        target.cell.flex_grow = Some(g);
    }
    let mut basis_set = false;
    for token in &tokens {
        // A length carries a unit (or `%`); a bare number is grow/shrink.
        if token.chars().any(|c| c.is_ascii_alphabetic() || c == '%') {
            if let Some(b) = parse_css_length(token) {
                target.cell.flex_basis = Some(b);
                basis_set = true;
            }
        }
    }
    if !basis_set && tokens.len() == 1 && tokens[0].parse::<f32>().is_ok() {
        target.cell.flex_basis = Some(0.0);
    }
}

/// Parse a `grid-template-columns` track list: lengths, `fr` fractions, `auto`,
/// and non-nested `repeat(N, tracks…)`. Unknown tokens (`minmax()`, named
/// lines, percentages) are skipped.
fn parse_grid_tracks(value: &str) -> Vec<GridTrack> {
    fn parse_token(token: &str) -> Option<GridTrack> {
        let token = token.trim();
        if token.eq_ignore_ascii_case("auto") {
            return Some(GridTrack::Auto);
        }
        if let Some(fr) = token.strip_suffix("fr") {
            return fr.trim().parse::<f32>().ok().map(GridTrack::Fr);
        }
        parse_css_length(token).map(GridTrack::Pt)
    }

    let mut out = Vec::new();
    let mut rest = value.trim();
    while !rest.is_empty() {
        rest = rest.trim_start();
        if let Some(after) = rest.strip_prefix("repeat(") {
            let Some(close) = after.find(')') else { break };
            let inner = &after[..close];
            if let Some((count, tracks)) = inner.split_once(',') {
                if let Ok(count) = count.trim().parse::<usize>() {
                    let unit: Vec<GridTrack> = tracks
                        .split_whitespace()
                        .filter_map(parse_token)
                        .collect();
                    for _ in 0..count.min(100) {
                        out.extend(unit.iter().copied());
                    }
                }
            }
            rest = &after[close + 1..];
        } else {
            let end = rest.find(char::is_whitespace).unwrap_or(rest.len());
            if let Some(track) = parse_token(&rest[..end]) {
                out.push(track);
            }
            rest = &rest[end..];
        }
    }
    out
}

fn apply_style_declaration(target: &mut DeclarationLayer, property: &str, value: &str) {
    match property {
        "display" if value.eq_ignore_ascii_case("none") => {
            target.display = Some(CssDisplay::None);
        }
        "display" if value.eq_ignore_ascii_case("table-header-group") => {
            target.display = Some(CssDisplay::TableHeaderGroup);
        }
        "display" if value.eq_ignore_ascii_case("table-row-group") => {
            target.display = Some(CssDisplay::TableRowGroup);
        }
        "display" if value.eq_ignore_ascii_case("table-footer-group") => {
            target.display = Some(CssDisplay::TableFooterGroup);
        }
        "display" if value.eq_ignore_ascii_case("flex") || value.eq_ignore_ascii_case("inline-flex") => {
            target.cell.display_flex = true;
        }
        "flex-direction" => {
            target.cell.flex_direction = match value.to_ascii_lowercase().as_str() {
                "column" | "column-reverse" => Some(FlexDirection::Column),
                _ => Some(FlexDirection::Row),
            };
        }
        "justify-content" => {
            target.cell.justify_content = match value.to_ascii_lowercase().as_str() {
                "flex-end" | "end" | "right" => Some(JustifyContent::FlexEnd),
                "center" => Some(JustifyContent::Center),
                "space-between" => Some(JustifyContent::SpaceBetween),
                "space-around" => Some(JustifyContent::SpaceAround),
                "space-evenly" => Some(JustifyContent::SpaceEvenly),
                _ => Some(JustifyContent::FlexStart),
            };
        }
        "align-items" => {
            target.cell.align_items = match value.to_ascii_lowercase().as_str() {
                "flex-start" | "start" => Some(AlignItems::FlexStart),
                "center" => Some(AlignItems::Center),
                "flex-end" | "end" => Some(AlignItems::FlexEnd),
                _ => Some(AlignItems::Stretch),
            };
        }
        "gap" => {
            // `gap: <row-gap> <column-gap>` — a single value sets both.
            let lengths: Vec<f32> = value
                .split_whitespace()
                .filter_map(parse_css_length)
                .collect();
            if let Some(&row) = lengths.first() {
                target.cell.row_gap = Some(row);
                target.cell.gap = Some(*lengths.get(1).unwrap_or(&row));
            }
        }
        "column-gap" => {
            if let Some(g) = parse_css_length(value) {
                target.cell.gap = Some(g);
            }
        }
        "row-gap" => {
            if let Some(g) = parse_css_length(value) {
                target.cell.row_gap = Some(g);
            }
        }
        "display" if value.eq_ignore_ascii_case("grid") || value.eq_ignore_ascii_case("inline-grid") => {
            target.cell.display_grid = true;
        }
        "grid-template-columns" => {
            let tracks = parse_grid_tracks(value);
            if !tracks.is_empty() {
                target.cell.grid_template = Some(tracks);
            }
        }
        "position" => {
            target.cell.position = match value.trim().to_ascii_lowercase().as_str() {
                "relative" => Some(PositionKind::Relative),
                "absolute" => Some(PositionKind::Absolute),
                "fixed" => Some(PositionKind::Fixed),
                _ => None, // static / sticky unsupported
            };
        }
        // `auto` (and any non-integer) stays `None`; fractional z-indexes are
        // invalid CSS and likewise ignored.
        "z-index" => target.cell.z_index = value.trim().parse::<i32>().ok(),
        "top" => target.cell.offset_top = parse_css_offset(value),
        "right" if parse_css_offset(value).is_some() => {
            target.cell.offset_right = parse_css_offset(value);
        }
        "bottom" => target.cell.offset_bottom = parse_css_offset(value),
        "left" => target.cell.offset_left = parse_css_offset(value),
        "float" => {
            target.cell.float_dir = match value.trim().to_ascii_lowercase().as_str() {
                "left" => Some(FloatDir::Left),
                "right" => Some(FloatDir::Right),
                _ => None, // `none` clears an earlier float
            };
        }
        "clear" => {
            target.cell.clear = match value.trim().to_ascii_lowercase().as_str() {
                "left" => Some(Clear::Left),
                "right" => Some(Clear::Right),
                "both" => Some(Clear::Both),
                _ => None,
            };
        }
        "grid-column" => {
            // Only the `span N` form is supported; line-based placement is not.
            let v = value.trim().to_ascii_lowercase();
            if let Some(rest) = v.strip_prefix("span") {
                if let Ok(n) = rest.trim().parse::<usize>() {
                    target.cell.grid_span = Some(n.max(1));
                }
            }
        }
        "flex-grow" => target.cell.flex_grow = value.trim().parse::<f32>().ok(),
        "flex-basis" => {
            target.cell.flex_basis = if value.eq_ignore_ascii_case("auto") {
                None
            } else {
                parse_css_length(value)
            };
        }
        "flex" => apply_flex_shorthand(target, value),
        "text-align" => {
            // `justify` maps to left until real justification exists; `start`/
            // `end` assume left-to-right text.
            target.cell.align = match value.to_ascii_lowercase().as_str() {
                "right" | "end" => Some(TextAlign::Right),
                "center" => Some(TextAlign::Center),
                "left" | "start" | "justify" => Some(TextAlign::Left),
                _ => target.cell.align,
            };
        }
        "vertical-align" if value.eq_ignore_ascii_case("top") => {
            target.cell.vertical_align = Some(VerticalAlign::Top);
        }
        "vertical-align" if value.eq_ignore_ascii_case("middle") => {
            target.cell.vertical_align = Some(VerticalAlign::Middle);
        }
        "vertical-align" if value.eq_ignore_ascii_case("bottom") => {
            target.cell.vertical_align = Some(VerticalAlign::Bottom);
        }
        "vertical-align" if value.eq_ignore_ascii_case("baseline") => {
            target.cell.vertical_align = Some(VerticalAlign::Baseline);
        }
        "font-weight" if is_bold_weight(value) => {
            target.cell.bold = true;
        }
        "text-decoration" | "text-decoration-line" => {
            // Only the `-line` component is honored; `none` clears both flags.
            let v = value.to_ascii_lowercase();
            if v.contains("none") {
                target.cell.underline = false;
                target.cell.line_through = false;
            } else {
                if v.contains("underline") {
                    target.cell.underline = true;
                }
                if v.contains("line-through") {
                    target.cell.line_through = true;
                }
            }
        }
        "font-size" => target.cell.font_size = parse_css_length(value),
        "font-family" => target.cell.font_family = parse_font_family(value),
        "font-style" => {
            target.cell.italic = match value.trim().to_ascii_lowercase().as_str() {
                "italic" | "oblique" => Some(true),
                "normal" => Some(false),
                _ => None,
            };
        }
        "line-height" => target.cell.line_height = parse_line_height(value),
        "width" => target.cell.width = parse_css_length(value),
        "height" => target.cell.height = parse_css_length(value),
        "color" => target.cell.color = parse_css_color(value),
        "background-color" => target.cell.background_color = parse_css_color(value),
        "background" => target.cell.background_color = parse_css_background_color(value),
        "padding-left" => target.cell.padding_left = parse_css_length(value),
        "padding-right" => target.cell.padding_right = parse_css_length(value),
        "padding-top" => target.cell.padding_top = parse_css_length(value),
        "padding-bottom" => target.cell.padding_bottom = parse_css_length(value),
        "padding" => {
            let [top, right, bottom, left] = parse_box_edges(value);
            target.cell.padding_top = top;
            target.cell.padding_right = right;
            target.cell.padding_bottom = bottom;
            target.cell.padding_left = left;
        }
        "margin-left" => target.cell.margin_left = parse_css_length(value),
        "margin-right" => target.cell.margin_right = parse_css_length(value),
        "margin-top" => target.cell.margin_top = parse_css_length(value),
        "margin-bottom" => target.cell.margin_bottom = parse_css_length(value),
        "margin" => {
            let [top, right, bottom, left] = parse_box_edges(value);
            target.cell.margin_top = top;
            target.cell.margin_right = right;
            target.cell.margin_bottom = bottom;
            target.cell.margin_left = left;
        }
        "overflow" if value.eq_ignore_ascii_case("visible") => {
            target.cell.overflow = Some(Overflow::Visible);
        }
        "overflow"
            if value.eq_ignore_ascii_case("hidden") || value.eq_ignore_ascii_case("clip") =>
        {
            target.cell.overflow = Some(Overflow::Hidden);
        }
        "white-space" if value.eq_ignore_ascii_case("nowrap") => {
            target.cell.white_space = Some(WhiteSpace::NoWrap);
        }
        "white-space" if value.eq_ignore_ascii_case("normal") => {
            target.cell.white_space = Some(WhiteSpace::Normal);
        }
        "overflow-wrap" | "word-wrap" if value.eq_ignore_ascii_case("normal") => {
            target.cell.overflow_wrap = Some(OverflowWrap::Normal);
        }
        "overflow-wrap" | "word-wrap" if value.eq_ignore_ascii_case("anywhere") => {
            target.cell.overflow_wrap = Some(OverflowWrap::Anywhere);
        }
        "overflow-wrap" | "word-wrap" if value.eq_ignore_ascii_case("break-word") => {
            target.cell.overflow_wrap = Some(OverflowWrap::BreakWord);
        }
        "word-break" if value.eq_ignore_ascii_case("normal") => {
            target.cell.word_break = Some(WordBreak::Normal);
        }
        "word-break" if value.eq_ignore_ascii_case("break-all") => {
            target.cell.word_break = Some(WordBreak::BreakAll);
        }
        "border" | "border-left" | "border-right" | "border-top" | "border-bottom" => {
            // `none`/`0` disable the border; anything else enables it. Recorded as
            // an explicit value so a more specific rule (e.g. `th.style0 {
            // border: none }`) can override a broad `th { border }`.
            let v = value.trim();
            target.cell.border = Some(!(v.starts_with("none") || v.starts_with('0')));
        }
        _ => {}
    }
}

/// CSS `font-weight` values that render as bold: the `bold`/`bolder` keywords or
/// a numeric weight of 600 or more.
fn is_bold_weight(value: &str) -> bool {
    let value = value.trim();
    value.eq_ignore_ascii_case("bold")
        || value.eq_ignore_ascii_case("bolder")
        || value.parse::<u32>().map(|n| n >= 600).unwrap_or(false)
}

fn parse_css_background_color(value: &str) -> Option<Color> {
    value.split_whitespace().find_map(parse_css_color)
}

fn parse_css_color(value: &str) -> Option<Color> {
    let value = value.trim().trim_matches('"').trim_matches('\'').trim();

    if value.eq_ignore_ascii_case("transparent") {
        return None;
    }

    if let Some(hex) = value.strip_prefix('#') {
        return parse_hex_color(hex);
    }

    // Functional notation: rgb()/rgba()/hsl()/hsla(), with comma or space-
    // separated components and an optional `/ alpha` (alpha is ignored since the
    // engine renders opaque colors today).
    if let Some(open) = value.find('(') {
        let stripped = value.strip_suffix(')')?;
        let function = value[..open].trim().to_ascii_lowercase();
        let args = &stripped[open + 1..];
        return match function.as_str() {
            "rgb" | "rgba" => parse_rgb_function(args),
            "hsl" | "hsla" => parse_hsl_function(args),
            _ => None,
        };
    }

    named_color(&value.to_ascii_lowercase())
}

fn parse_hex_color(hex: &str) -> Option<Color> {
    match hex.len() {
        // #rgb and #rgba (alpha ignored).
        3 | 4 => {
            let r = expand_hex_nibble(hex.as_bytes()[0])?;
            let g = expand_hex_nibble(hex.as_bytes()[1])?;
            let b = expand_hex_nibble(hex.as_bytes()[2])?;
            Some(Color::from_rgb_u8(r, g, b))
        }
        // #rrggbb and #rrggbbaa (alpha ignored).
        6 | 8 => {
            let r = u8::from_str_radix(&hex[0..2], 16).ok()?;
            let g = u8::from_str_radix(&hex[2..4], 16).ok()?;
            let b = u8::from_str_radix(&hex[4..6], 16).ok()?;
            Some(Color::from_rgb_u8(r, g, b))
        }
        _ => None,
    }
}

fn expand_hex_nibble(byte: u8) -> Option<u8> {
    let value = match byte {
        b'0'..=b'9' => byte - b'0',
        b'a'..=b'f' => byte - b'a' + 10,
        b'A'..=b'F' => byte - b'A' + 10,
        _ => return None,
    };

    Some(value * 17)
}

/// Split color-function arguments on commas, slashes (alpha separator), and
/// whitespace, so both legacy `rgb(r, g, b)` and modern `rgb(r g b / a)` work.
fn color_components(args: &str) -> Vec<&str> {
    args.split([',', '/', ' ', '\t', '\n'])
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .collect()
}

fn parse_rgb_function(args: &str) -> Option<Color> {
    let parts = color_components(args);
    if parts.len() < 3 {
        return None;
    }
    Some(Color::from_rgb_u8(
        parse_rgb_channel(parts[0])?,
        parse_rgb_channel(parts[1])?,
        parse_rgb_channel(parts[2])?,
    ))
}

/// A single rgb() channel: either 0..=255 or a percentage.
fn parse_rgb_channel(part: &str) -> Option<u8> {
    let scaled = if let Some(percent) = part.strip_suffix('%') {
        percent.trim().parse::<f32>().ok()? / 100.0 * 255.0
    } else {
        part.parse::<f32>().ok()?
    };
    Some(scaled.round().clamp(0.0, 255.0) as u8)
}

fn parse_hsl_function(args: &str) -> Option<Color> {
    let parts = color_components(args);
    if parts.len() < 3 {
        return None;
    }
    let hue = parts[0].trim_end_matches("deg").trim().parse::<f32>().ok()?;
    let saturation = parts[1].trim_end_matches('%').trim().parse::<f32>().ok()? / 100.0;
    let lightness = parts[2].trim_end_matches('%').trim().parse::<f32>().ok()? / 100.0;
    Some(hsl_to_color(hue, saturation.clamp(0.0, 1.0), lightness.clamp(0.0, 1.0)))
}

fn hsl_to_color(hue: f32, saturation: f32, lightness: f32) -> Color {
    let hue = (hue.rem_euclid(360.0)) / 360.0;
    let (r, g, b) = if saturation == 0.0 {
        (lightness, lightness, lightness)
    } else {
        let q = if lightness < 0.5 {
            lightness * (1.0 + saturation)
        } else {
            lightness + saturation - lightness * saturation
        };
        let p = 2.0 * lightness - q;
        (
            hue_to_rgb(p, q, hue + 1.0 / 3.0),
            hue_to_rgb(p, q, hue),
            hue_to_rgb(p, q, hue - 1.0 / 3.0),
        )
    };

    // Quantize through the same 0..=255 path every other color uses, so equal
    // colors compare equal regardless of how they were written.
    let to_u8 = |channel: f32| (channel.clamp(0.0, 1.0) * 255.0).round() as u8;
    Color::from_rgb_u8(to_u8(r), to_u8(g), to_u8(b))
}

fn hue_to_rgb(p: f32, q: f32, t: f32) -> f32 {
    let t = t.rem_euclid(1.0);
    if t < 1.0 / 6.0 {
        p + (q - p) * 6.0 * t
    } else if t < 1.0 / 2.0 {
        q
    } else if t < 2.0 / 3.0 {
        p + (q - p) * (2.0 / 3.0 - t) * 6.0
    } else {
        p
    }
}

/// A practical subset of CSS named colors. Keeps the original six mappings
/// exactly (black/white/red/green/blue/yellow) and adds common extras.
fn named_color(name: &str) -> Option<Color> {
    let (r, g, b) = match name {
        "black" => (0, 0, 0),
        "white" => (255, 255, 255),
        "red" => (255, 0, 0),
        "green" => (0, 128, 0),
        "blue" => (0, 0, 255),
        "yellow" => (255, 255, 0),
        "silver" => (192, 192, 192),
        "gray" | "grey" => (128, 128, 128),
        "maroon" => (128, 0, 0),
        "olive" => (128, 128, 0),
        "lime" => (0, 255, 0),
        "aqua" | "cyan" => (0, 255, 255),
        "teal" => (0, 128, 128),
        "navy" => (0, 0, 128),
        "fuchsia" | "magenta" => (255, 0, 255),
        "purple" => (128, 0, 128),
        "orange" => (255, 165, 0),
        "pink" => (255, 192, 203),
        "brown" => (165, 42, 42),
        "gold" => (255, 215, 0),
        "lightgray" | "lightgrey" => (211, 211, 211),
        "darkgray" | "darkgrey" => (169, 169, 169),
        "whitesmoke" => (245, 245, 245),
        "lightblue" => (173, 216, 230),
        "lightgreen" => (144, 238, 144),
        _ => return None,
    };
    Some(Color::from_rgb_u8(r, g, b))
}

impl CellStyle {
    fn merge(&mut self, other: CellStyle) {
        self.align = other.align.or(self.align);
        self.vertical_align = other.vertical_align.or(self.vertical_align);
        self.bold |= other.bold;
        self.underline |= other.underline;
        self.line_through |= other.line_through;
        self.border = other.border.or(self.border);
        self.overflow = other.overflow.or(self.overflow);
        self.font_size = other.font_size.or(self.font_size);
        self.font_family = other.font_family.or(self.font_family.take());
        self.italic = other.italic.or(self.italic);
        self.line_height = other.line_height.or(self.line_height);
        self.width = other.width.or(self.width);
        self.height = other.height.or(self.height);
        self.padding_left = other.padding_left.or(self.padding_left);
        self.padding_right = other.padding_right.or(self.padding_right);
        self.padding_top = other.padding_top.or(self.padding_top);
        self.padding_bottom = other.padding_bottom.or(self.padding_bottom);
        self.margin_left = other.margin_left.or(self.margin_left);
        self.margin_right = other.margin_right.or(self.margin_right);
        self.margin_top = other.margin_top.or(self.margin_top);
        self.margin_bottom = other.margin_bottom.or(self.margin_bottom);
        self.white_space = other.white_space.or(self.white_space);
        self.overflow_wrap = other.overflow_wrap.or(self.overflow_wrap);
        self.word_break = other.word_break.or(self.word_break);
        self.color = other.color.or(self.color);
        self.background_color = other.background_color.or(self.background_color);
        self.display_flex |= other.display_flex;
        self.flex_direction = other.flex_direction.or(self.flex_direction);
        self.justify_content = other.justify_content.or(self.justify_content);
        self.align_items = other.align_items.or(self.align_items);
        self.gap = other.gap.or(self.gap);
        self.flex_grow = other.flex_grow.or(self.flex_grow);
        self.flex_basis = other.flex_basis.or(self.flex_basis);
        self.display_grid |= other.display_grid;
        self.grid_template = other.grid_template.or(self.grid_template.take());
        self.row_gap = other.row_gap.or(self.row_gap);
        self.grid_span = other.grid_span.or(self.grid_span);
        self.float_dir = other.float_dir.or(self.float_dir);
        self.clear = other.clear.or(self.clear);
        self.position = other.position.or(self.position);
        self.z_index = other.z_index.or(self.z_index);
        self.offset_top = other.offset_top.or(self.offset_top);
        self.offset_right = other.offset_right.or(self.offset_right);
        self.offset_bottom = other.offset_bottom.or(self.offset_bottom);
        self.offset_left = other.offset_left.or(self.offset_left);
    }
}

fn collapse_whitespace(input: &str) -> String {
    let mut output = String::new();
    let mut last_was_space = true;

    for ch in input.chars() {
        if ch.is_whitespace() {
            if !last_was_space {
                output.push(' ');
            }
            last_was_space = true;
        } else {
            output.push(ch);
            last_was_space = false;
        }
    }

    output.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::{parse, BlockKind};
    use crate::box_tree::{BlockBox, BoxChild, FlowRoot, InlineRun};

    /// All block boxes in document order (depth-first), flattening the tree so
    /// flow-content tests can assert on leaf blocks regardless of nesting.
    fn flow_blocks(flow: &FlowRoot) -> Vec<&BlockBox> {
        fn walk<'a>(children: &'a [BoxChild], out: &mut Vec<&'a BlockBox>) {
            for child in children {
                if let BoxChild::Block(block) = child {
                    out.push(block);
                    walk(&block.children, out);
                }
            }
        }
        let mut out = Vec::new();
        walk(&flow.children, &mut out);
        out
    }

    /// The block's directly-contained inline text (its own line boxes only),
    /// with whitespace collapsed — i.e. text not inside a nested child block.
    fn block_text(block: &BlockBox) -> String {
        let mut text = String::new();
        for child in &block.children {
            if let BoxChild::Line(runs) = child {
                for run in runs {
                    text.push_str(&run.text);
                }
            }
        }
        super::collapse_whitespace(&text)
    }

    /// The first inline run of the block's first line box.
    fn first_run(block: &BlockBox) -> &InlineRun {
        block
            .children
            .iter()
            .find_map(|child| match child {
                BoxChild::Line(runs) => runs.first(),
                _ => None,
            })
            .expect("block has an inline run")
    }

    #[test]
    fn extracts_blocks() {
        let document = parse("<h1>Title</h1><p>Hello <strong>world</strong>.</p>");
        let flow = document.flow.expect("flow tree");
        let blocks = flow_blocks(&flow);

        assert_eq!(blocks.len(), 2);
        assert_eq!(blocks[0].kind, BlockKind::Heading1);
        assert_eq!(block_text(blocks[0]), "Title");
        assert_eq!(block_text(blocks[1]), "Hello world.");
    }

    #[test]
    fn bold_inline_runs_are_marked_bold() {
        let document = parse("<p>Hello <strong>world</strong>.</p>");
        let flow = document.flow.expect("flow tree");
        let blocks = flow_blocks(&flow);
        let runs = match &blocks[0].children[0] {
            BoxChild::Line(runs) => runs,
            _ => panic!("expected an inline line"),
        };

        // "Hello " is not bold; "world" (inside <strong>) is.
        let bolds: Vec<bool> = runs.iter().map(|run| run.bold).collect();
        assert_eq!(runs.iter().find(|r| r.text.contains("world")).unwrap().bold, true);
        assert!(bolds.contains(&false));
    }

    #[test]
    fn text_decoration_marks_underline_and_line_through() {
        let document = parse(
            "<p>plain <u>under</u> <s>struck</s> \
             <span style=\"text-decoration: underline line-through\">both</span></p>",
        );
        let flow = document.flow.expect("flow tree");
        let blocks = flow_blocks(&flow);
        let runs = match &blocks[0].children[0] {
            BoxChild::Line(runs) => runs,
            _ => panic!("expected an inline line"),
        };
        let find = |needle: &str| runs.iter().find(|r| r.text.contains(needle)).unwrap();

        assert!(find("plain").underline == false && find("plain").line_through == false);
        assert!(find("under").underline && !find("under").line_through);
        assert!(find("struck").line_through && !find("struck").underline);
        assert!(find("both").underline && find("both").line_through);
    }

    #[test]
    fn parses_grid_template_columns() {
        use super::{parse_grid_tracks, GridTrack};
        assert_eq!(
            parse_grid_tracks("120pt auto 1fr"),
            vec![GridTrack::Pt(120.0), GridTrack::Auto, GridTrack::Fr(1.0)]
        );
        assert_eq!(parse_grid_tracks("repeat(3, 1fr)"), vec![GridTrack::Fr(1.0); 3]);
        assert_eq!(
            parse_grid_tracks("repeat(2, 50pt 2fr) auto"),
            vec![
                GridTrack::Pt(50.0),
                GridTrack::Fr(2.0),
                GridTrack::Pt(50.0),
                GridTrack::Fr(2.0),
                GridTrack::Auto
            ]
        );
    }

    #[test]
    fn grid_container_and_span_reach_the_box_tree() {
        let document = parse(
            "<style>.g { display:grid; grid-template-columns: repeat(2, 1fr); gap: 6pt 9pt; } \
                    .w { grid-column: span 2; }</style>\
             <p>before</p>\
             <div class=\"g\"><div class=\"w\">wide</div><div>a</div><div>b</div></div>",
        );
        let flow = document.flow.expect("flow tree");
        let grid_block = flow_blocks(&flow)
            .into_iter()
            .find(|b| b.grid.is_some())
            .expect("a grid container");
        let grid = grid_block.grid.as_ref().unwrap();
        assert_eq!(grid.columns.len(), 2);
        assert_eq!(grid.row_gap, 6.0);
        assert_eq!(grid.column_gap, 9.0);
        let spans: Vec<usize> = grid_block
            .children
            .iter()
            .filter_map(|c| match c {
                BoxChild::Block(b) => Some(b.grid_span),
                _ => None,
            })
            .collect();
        assert_eq!(spans, vec![2, 1, 1]);
    }

    #[test]
    fn ignores_script_and_style_content() {
        let document =
            parse("<style>body{}</style><h1>Visible</h1><script>alert('hidden')</script>");
        let flow = document.flow.expect("flow tree");
        let blocks = flow_blocks(&flow);

        assert_eq!(blocks.len(), 1);
        assert_eq!(block_text(blocks[0]), "Visible");
    }

    #[test]
    fn parses_all_heading_levels() {
        let document = parse("<h1>a</h1><h2>b</h2><h3>c</h3><h4>d</h4><h5>e</h5><h6>f</h6>");
        let flow = document.flow.expect("flow tree");
        let kinds: Vec<BlockKind> = flow_blocks(&flow).iter().map(|b| b.kind).collect();
        assert_eq!(
            kinds,
            vec![
                BlockKind::Heading1,
                BlockKind::Heading2,
                BlockKind::Heading3,
                BlockKind::Heading4,
                BlockKind::Heading5,
                BlockKind::Heading6,
            ]
        );
    }

    #[test]
    fn nests_blocks_and_indents_lists() {
        let document = parse("<div><p>outer</p><ul><li>one</li><li>two</li></ul></div>");
        let flow = document.flow.expect("flow tree");
        let blocks = flow_blocks(&flow);

        // div > (p, ul > (li, li)). The list container is indented (its items lay
        // out inside that indent), and each item carries a bullet marker.
        assert!(
            blocks.iter().any(|b| b.margin.left >= super::LIST_INDENT),
            "the list container is indented"
        );
        let li_one = blocks
            .iter()
            .find(|b| block_text(b).contains("one"))
            .unwrap();
        assert!(block_text(li_one).starts_with('\u{2022}'), "bullet marker present");
    }

    #[test]
    fn numbers_ordered_list_items() {
        let document = parse("<ol><li>first</li><li>second</li></ol>");
        let flow = document.flow.expect("flow tree");
        let blocks = flow_blocks(&flow);
        let texts: Vec<String> = blocks
            .iter()
            .map(|b| block_text(b))
            .filter(|t| t.contains("first") || t.contains("second"))
            .collect();

        // `block_text` collapses the marker's padding (the renderer does too).
        assert_eq!(texts, vec!["1. first".to_string(), "2. second".to_string()]);
    }

    #[test]
    fn skips_display_none_flow_content() {
        let document = parse(
            r#"
            <style>.hidden { display: none; }</style>
            <p>shown</p>
            <p class="hidden">secret</p>
            <div style="display:none">also secret</div>
            "#,
        );
        let flow = document.flow.expect("flow tree");
        let texts: Vec<String> = flow_blocks(&flow).iter().map(|b| block_text(b)).collect();

        assert_eq!(texts, vec!["shown".to_string()]);
    }

    #[test]
    fn applies_computed_style_to_flow_blocks() {
        let document = parse(
            r#"
            <style>
            p.note { color: #112233; font-size: 14pt; text-align: center; }
            </style>
            <p class="note">styled</p>
            "#,
        );
        let flow = document.flow.expect("flow tree");
        let blocks = flow_blocks(&flow);
        let block = blocks[0];
        let run = first_run(block);

        assert_eq!(block.kind, BlockKind::Paragraph);
        assert_eq!(block_text(block), "styled");
        assert_eq!(block.align, super::TextAlign::Center);
        assert_eq!(run.color, crate::color::Color::from_rgb_u8(0x11, 0x22, 0x33));
        assert_eq!(run.font_size, 14.0);
    }

    #[test]
    fn parses_block_margin_and_padding_shorthands() {
        let document = parse(
            r#"<div style="margin: 10pt 20pt 30pt 40pt; padding: 5pt 6pt">x</div>"#,
        );
        let flow = document.flow.expect("flow tree");
        let block = flow_blocks(&flow)[0];

        assert_eq!(block.margin.top, 10.0);
        assert_eq!(block.margin.right, 20.0);
        assert_eq!(block.margin.bottom, 30.0);
        assert_eq!(block.margin.left, 40.0);
        // Two-value padding: top/bottom = 5, right/left = 6.
        assert_eq!(block.padding.top, 5.0);
        assert_eq!(block.padding.right, 6.0);
        assert_eq!(block.padding.bottom, 5.0);
        assert_eq!(block.padding.left, 6.0);
    }

    #[test]
    fn flow_blocks_inherit_style_from_ancestors() {
        let document = parse(
            r#"
            <style>body { color: #445566; }</style>
            <body><div><p>deep</p></div></body>
            "#,
        );
        let flow = document.flow.expect("flow tree");
        let blocks = flow_blocks(&flow);
        let deep = blocks.iter().find(|b| block_text(b) == "deep").unwrap();

        assert_eq!(
            first_run(deep).color,
            crate::color::Color::from_rgb_u8(0x44, 0x55, 0x66)
        );
    }

    #[test]
    fn parses_spreadsheet_table_rows() {
        let document = parse(
            r#"
            <style>@page page0 { size: landscape; } table.sheet0 col.col0 { width:30pt }</style>
            <table><tr><td class="style10 s" colspan="2">Student ID</td><td class="style12 n">9.00</td></tr></table>
            "#,
        );

        assert_eq!(
            document.page_style.orientation,
            super::PageOrientation::Landscape
        );
        assert_eq!(document.table_columns, vec![30.0]);
        assert_eq!(document.blocks.len(), 1);
        assert_eq!(document.blocks[0].kind, BlockKind::TableRow);
        assert_eq!(document.blocks[0].cells.len(), 2);
        assert_eq!(document.blocks[0].cells[0].text, "Student ID");
        assert_eq!(document.blocks[0].cells[0].colspan, 2);
    }

    #[test]
    fn parses_page_and_table_geometry_via_cssparser() {
        let document = parse(
            r#"
            <style>
            tr { text-align: left }
            @page page0 { margin-left: 0.25in; margin-top: 0.75in; size: landscape; }
            table.sheet0 col.col0 { width: 30pt }
            table.sheet0 col.col1 { width: 93pt }
            table.sheet0 tr { height: 15pt }
            </style>
            <table><tr><td>x</td></tr></table>
            "#,
        );

        assert_eq!(
            document.page_style.orientation,
            super::PageOrientation::Landscape
        );
        assert_eq!(document.page_style.margin_left, Some(18.0)); // 0.25in
        assert_eq!(document.page_style.margin_top, Some(54.0)); // 0.75in
        // The bare `tr { text-align }` carries no height; the row height comes
        // from `table.sheet0 tr`.
        assert_eq!(document.table_style.row_height, Some(15.0));
        // Column widths in source order.
        assert_eq!(document.table_columns, vec![30.0, 93.0]);
    }

    #[test]
    fn descendant_border_rule_is_scoped_to_its_ancestor() {
        // `.gridlines td` must NOT box a cell whose table isn't under `.gridlines`;
        // the cell's own `border: none` (more specific) wins.
        let outside = parse(
            r#"<style>.gridlines td { border: 1px solid black; }
               td.title { border: none; }</style>
               <table><tr><td class="title">x</td></tr></table>"#,
        );
        assert_ne!(outside.blocks[0].cells[0].style.border, Some(true));

        // Under a `.gridlines` ancestor, the same rule DOES apply.
        let inside = parse(
            r#"<style>.gridlines td { border: 1px solid black; }</style>
               <div class="gridlines"><table><tr><td>x</td></tr></table></div>"#,
        );
        assert_eq!(inside.blocks[0].cells[0].style.border, Some(true));
    }

    #[test]
    fn border_none_overrides_a_broader_border_rule() {
        let document = parse(
            r#"<style>th { border: 1px solid black; }
               th.plain { border: none; }</style>
               <table><tr><th class="plain">x</th></tr></table>"#,
        );
        assert_eq!(document.blocks[0].cells[0].style.border, Some(false));
    }

    #[test]
    fn screen_only_media_rules_are_ignored_for_print() {
        let document = parse(
            r#"<style>
               @media screen { td.c { color: #ff0000; } }
               @media print  { td.c { color: #0000ff; } }
               </style>
               <table><tr><td class="c">x</td></tr></table>"#,
        );
        // The print rule applies; the screen-only rule does not.
        assert_eq!(
            document.blocks[0].cells[0].style.color,
            Some(crate::color::Color::from_rgb_u8(0, 0, 255))
        );
    }

    #[test]
    fn child_combinator_requires_immediate_parent() {
        // `table > td` must not match: a cell's parent is <tr>, not <table>.
        let strict = parse(
            r#"<style>table > td { border: 1px solid black; }</style>
               <table><tr><td>x</td></tr></table>"#,
        );
        assert_ne!(strict.blocks[0].cells[0].style.border, Some(true));

        // `tr > td` matches: the cell's immediate parent is the row.
        let ok = parse(
            r#"<style>tr > td { border: 1px solid black; }</style>
               <table><tr><td>x</td></tr></table>"#,
        );
        assert_eq!(ok.blocks[0].cells[0].style.border, Some(true));
    }

    #[test]
    fn adjacent_sibling_combinator_matches_only_after_a_sibling() {
        // `td + td` boxes every cell that directly follows another cell.
        let document = parse(
            r#"<style>td + td { border: 1px solid black; }</style>
               <table><tr><td>a</td><td>b</td><td>c</td></tr></table>"#,
        );
        assert_ne!(document.blocks[0].cells[0].style.border, Some(true));
        assert_eq!(document.blocks[0].cells[1].style.border, Some(true));
        assert_eq!(document.blocks[0].cells[2].style.border, Some(true));
    }

    #[test]
    fn general_sibling_combinator_matches_all_following_siblings() {
        // `.mark ~ td` boxes cells that come after a `.mark` cell, not before.
        let document = parse(
            r#"<style>.mark ~ td { border: 1px solid black; }</style>
               <table><tr><td>a</td><td class="mark">m</td><td>c</td><td>d</td></tr></table>"#,
        );
        assert_ne!(document.blocks[0].cells[0].style.border, Some(true)); // before mark
        assert_ne!(document.blocks[0].cells[1].style.border, Some(true)); // the mark itself
        assert_eq!(document.blocks[0].cells[2].style.border, Some(true));
        assert_eq!(document.blocks[0].cells[3].style.border, Some(true));
    }

    #[test]
    fn id_selector_matches_and_outranks_class() {
        // `#hot` (id) beats `td.cell` (class + type) despite coming first.
        let document = parse(
            r#"<style>#hot { color: #ff0000; } td.cell { color: #0000ff; }</style>
               <table><tr><td class="cell" id="hot">x</td><td class="cell">y</td></tr></table>"#,
        );
        assert_eq!(
            document.blocks[0].cells[0].style.color,
            Some(crate::color::Color::from_rgb_u8(255, 0, 0))
        );
        assert_eq!(
            document.blocks[0].cells[1].style.color,
            Some(crate::color::Color::from_rgb_u8(0, 0, 255))
        );
    }

    #[test]
    fn universal_selector_matches_any_element() {
        let document = parse(
            r#"<style>* { border: 1px solid black; }</style>
               <table><tr><td>x</td></tr></table>"#,
        );
        assert_eq!(document.blocks[0].cells[0].style.border, Some(true));
    }

    #[test]
    fn attribute_selectors_match_presence_value_and_word() {
        // presence `[data-flag]`
        let presence = parse(
            r#"<style>td[data-flag] { border: 1px solid black; }</style>
               <table><tr><td data-flag="1">a</td><td>b</td></tr></table>"#,
        );
        assert_eq!(presence.blocks[0].cells[0].style.border, Some(true));
        assert_ne!(presence.blocks[0].cells[1].style.border, Some(true));

        // exact `[data-k="v"]`
        let equals = parse(
            r#"<style>td[data-k="v"] { border: 1px solid black; }</style>
               <table><tr><td data-k="v">a</td><td data-k="x">b</td></tr></table>"#,
        );
        assert_eq!(equals.blocks[0].cells[0].style.border, Some(true));
        assert_ne!(equals.blocks[0].cells[1].style.border, Some(true));

        // whitespace word `[data-tags~=hot]` and prefix `[data-id^=row-]`
        let word = parse(
            r#"<style>td[data-tags~="hot"] { border: 1px solid black; }
               td[data-id^="row-"] { border: 1px solid black; }</style>
               <table><tr><td data-tags="cold hot dry">a</td>
               <td data-tags="cold">b</td><td data-id="row-3">c</td></tr></table>"#,
        );
        assert_eq!(word.blocks[0].cells[0].style.border, Some(true));
        assert_ne!(word.blocks[0].cells[1].style.border, Some(true));
        assert_eq!(word.blocks[0].cells[2].style.border, Some(true));
    }

    #[test]
    fn nth_child_selects_by_an_plus_b() {
        // `td:nth-child(odd)` → 1st and 3rd cells (1-based among siblings).
        let document = parse(
            r#"<style>td:nth-child(odd) { border: 1px solid black; }</style>
               <table><tr><td>a</td><td>b</td><td>c</td><td>d</td></tr></table>"#,
        );
        assert_eq!(document.blocks[0].cells[0].style.border, Some(true));
        assert_ne!(document.blocks[0].cells[1].style.border, Some(true));
        assert_eq!(document.blocks[0].cells[2].style.border, Some(true));
        assert_ne!(document.blocks[0].cells[3].style.border, Some(true));
    }

    #[test]
    fn first_and_last_of_type_count_within_a_tag() {
        // A row of th, td, td: `td:first-of-type` is the first td (2nd cell).
        let document = parse(
            r#"<style>td:first-of-type { border: 1px solid black; }</style>
               <table><tr><th>h</th><td>a</td><td>b</td></tr></table>"#,
        );
        assert_ne!(document.blocks[0].cells[0].style.border, Some(true)); // th
        assert_eq!(document.blocks[0].cells[1].style.border, Some(true)); // first td
        assert_ne!(document.blocks[0].cells[2].style.border, Some(true));
    }

    #[test]
    fn not_pseudo_excludes_matching_cells() {
        let document = parse(
            r#"<style>td:not(.skip) { border: 1px solid black; }</style>
               <table><tr><td>a</td><td class="skip">b</td></tr></table>"#,
        );
        assert_eq!(document.blocks[0].cells[0].style.border, Some(true));
        assert_ne!(document.blocks[0].cells[1].style.border, Some(true));
    }

    #[test]
    fn empty_pseudo_matches_cells_without_content() {
        let document = parse(
            r#"<style>td:empty { border: 1px solid black; }</style>
               <table><tr><td></td><td>text</td></tr></table>"#,
        );
        assert_eq!(document.blocks[0].cells[0].style.border, Some(true));
        assert_ne!(document.blocks[0].cells[1].style.border, Some(true));
    }

    #[test]
    fn dynamic_pseudo_and_pseudo_element_selectors_are_dropped() {
        // `:hover` and `::before` never apply to the print render, so the rule is
        // dropped rather than applied to every `td`.
        let document = parse(
            r#"<style>td:hover { border: 1px solid black; }
               td::before { border: 1px solid black; }</style>
               <table><tr><td>a</td></tr></table>"#,
        );
        assert_ne!(document.blocks[0].cells[0].style.border, Some(true));
    }

    #[test]
    fn hides_display_none_rows_and_cells() {
        let document = parse(
            r#"
            <style>
            tr.gone { display: none; }
            td.gone { display: none; }
            </style>
            <table>
              <tr><td>a</td><td class="gone">hidden-cell</td><td>c</td></tr>
              <tr class="gone"><td>whole-row-hidden</td></tr>
              <tr><td>d</td></tr>
            </table>
            "#,
        );

        // Hidden row dropped entirely; hidden cell removed from its row.
        assert_eq!(document.blocks.len(), 2);
        let first: Vec<&str> = document.blocks[0]
            .cells
            .iter()
            .map(|c| c.text.as_str())
            .collect();
        assert_eq!(first, vec!["a", "c"]);
        assert_eq!(document.blocks[1].cells[0].text, "d");
    }

    #[test]
    fn preserves_table_header_section_rows() {
        let document = parse(
            r#"
            <table>
              <thead><tr><th>Name</th></tr></thead>
              <tbody><tr><td>Ada</td></tr></tbody>
            </table>
            "#,
        );

        assert_eq!(document.blocks.len(), 2);
        assert_eq!(document.blocks[0].kind, BlockKind::TableHeaderRow);
        assert_eq!(document.blocks[1].kind, BlockKind::TableRow);
    }

    #[test]
    fn preserves_css_table_header_group_rows() {
        let document = parse(
            r#"
            <style>.repeat { display: table-header-group; }</style>
            <table>
              <tbody class="repeat"><tr><th>Name</th></tr></tbody>
              <tbody><tr><td>Ada</td></tr></tbody>
            </table>
            "#,
        );

        assert_eq!(document.blocks.len(), 2);
        assert_eq!(document.blocks[0].kind, BlockKind::TableHeaderRow);
        assert_eq!(document.blocks[1].kind, BlockKind::TableRow);
    }

    #[test]
    fn preserves_tag_class_css_table_header_group_rows() {
        let document = parse(
            r#"
            <style>tbody.repeat { display: table-header-group; }</style>
            <table>
              <tbody class="repeat"><tr><th>Name</th></tr></tbody>
              <tbody><tr><td>Ada</td></tr></tbody>
            </table>
            "#,
        );

        assert_eq!(document.blocks.len(), 2);
        assert_eq!(document.blocks[0].kind, BlockKind::TableHeaderRow);
        assert_eq!(document.blocks[1].kind, BlockKind::TableRow);
    }

    #[test]
    fn preserves_inline_table_footer_group_rows() {
        let document = parse(
            r#"
            <table>
              <tbody><tr><td>Ada</td></tr></tbody>
              <tbody style="display: table-footer-group"><tr><td>Total</td></tr></tbody>
            </table>
            "#,
        );

        assert_eq!(document.blocks.len(), 2);
        assert_eq!(document.blocks[0].kind, BlockKind::TableRow);
        assert_eq!(document.blocks[1].kind, BlockKind::TableFooterRow);
    }

    #[test]
    fn parses_page_margins_and_row_height() {
        let document = parse(
            r#"
            <style>
            @page page0 { margin-left: 0.25in; margin-right: 0.25in; margin-top: 0.75in; margin-bottom: 0.75in; size: landscape; }
            table.sheet0 tr { height:15pt }
            </style>
            <p>Hello</p>
            "#,
        );

        assert_eq!(document.page_style.margin_left, Some(18.0));
        assert_eq!(document.page_style.margin_right, Some(18.0));
        assert_eq!(document.page_style.margin_top, Some(54.0));
        assert_eq!(document.page_style.margin_bottom, Some(54.0));
        assert_eq!(document.table_style.row_height, Some(15.0));
    }

    #[test]
    fn interns_font_specs_from_families_and_tags() {
        let document = parse(
            r#"
            <style>body { font-family: "Georgia", serif } .m { font-family: monospace }</style>
            <p>plain <b>bold</b> and <i>italic</i></p>
            <pre>preformatted</pre>
            <p class="m">mono</p>
            "#,
        );
        let specs = &document.font_specs;

        // Spec 0 is always the default (no family, regular).
        assert_eq!(specs[0].family, None);
        assert!(!specs[0].bold && !specs[0].italic);
        // The first family in the stack wins; bold/italic runs get variant specs.
        let georgia = |bold: bool, italic: bool| {
            specs.iter().any(|s| {
                s.family.as_deref() == Some("Georgia") && s.bold == bold && s.italic == italic
            })
        };
        assert!(georgia(false, false), "{specs:?}");
        assert!(georgia(true, false), "<b> inside Georgia body");
        assert!(georgia(false, true), "<i> inside Georgia body");
        // <pre> defaults to monospace; the .m class names it explicitly.
        assert!(
            specs.iter().any(|s| s.family.as_deref() == Some("monospace")),
            "{specs:?}"
        );

        // Runs actually reference distinct specs.
        fn collect_fonts(children: &[crate::box_tree::BoxChild], out: &mut Vec<u16>) {
            for child in children {
                match child {
                    crate::box_tree::BoxChild::Line(runs) => {
                        out.extend(runs.iter().map(|r| r.font))
                    }
                    crate::box_tree::BoxChild::Block(b) => collect_fonts(&b.children, out),
                    _ => {}
                }
            }
        }
        let mut fonts = Vec::new();
        collect_fonts(&document.flow.as_ref().unwrap().children, &mut fonts);
        fonts.sort_unstable();
        fonts.dedup();
        assert!(fonts.len() >= 4, "regular/bold/italic/monospace runs: {fonts:?}");
    }

    #[test]
    fn parses_line_height_values() {
        use super::{parse_line_height, LineHeight};
        assert_eq!(parse_line_height("1.5"), Some(LineHeight::Number(1.5)));
        assert_eq!(parse_line_height("150%"), Some(LineHeight::Number(1.5)));
        assert_eq!(parse_line_height("18pt"), Some(LineHeight::Length(18.0)));
        assert_eq!(parse_line_height("24px"), Some(LineHeight::Length(18.0)));
        assert_eq!(parse_line_height("normal"), None);
        assert_eq!(parse_line_height("NORMAL"), None);
        assert_eq!(parse_line_height("-2"), None, "negative is invalid");
        assert_eq!(parse_line_height("bogus"), None);
    }

    #[test]
    fn line_height_inherits_and_reaches_blocks_and_cells() {
        let document = parse(
            r#"
            <style>
            body { line-height: 1.8 }
            td { line-height: 200% }
            </style>
            <body><p>flow text</p>
            <table><tr><td>cell text</td></tr></table></body>
            "#,
        );

        // The paragraph inherits body's line-height through the cascade.
        fn find_paragraph(children: &[crate::box_tree::BoxChild]) -> Option<&crate::box_tree::BlockBox> {
            for child in children {
                if let crate::box_tree::BoxChild::Block(block) = child {
                    if matches!(block.kind, super::BlockKind::Paragraph) {
                        return Some(block);
                    }
                    if let Some(found) = find_paragraph(&block.children) {
                        return Some(found);
                    }
                }
            }
            None
        }
        let flow = document.flow.as_ref().expect("flow content present");
        let paragraph = find_paragraph(&flow.children).expect("paragraph block");
        assert_eq!(paragraph.line_height, Some(super::LineHeight::Number(1.8)));

        // The cell's own rule (200% -> 2.0) beats the inherited 1.8. The table
        // renders inside the flow, so find it there.
        fn find_table(children: &[crate::box_tree::BoxChild]) -> Option<&crate::box_tree::TableBox> {
            for child in children {
                match child {
                    crate::box_tree::BoxChild::Table(table) => return Some(table),
                    crate::box_tree::BoxChild::Block(block) => {
                        if let Some(found) = find_table(&block.children) {
                            return Some(found);
                        }
                    }
                    _ => {}
                }
            }
            None
        }
        let table = find_table(&flow.children).expect("table in flow");
        let cell = &table.rows[0].cells[0];
        assert_eq!(cell.style.line_height, Some(super::LineHeight::Number(2.0)));
    }

    #[test]
    fn parses_cell_styles_from_css_classes() {
        let document = parse(
            r#"
            <style>
            td.style10, th.style10 { text-align:center; padding-left:5px; padding-right:5px; font-weight:bold; font-size:12pt; border-bottom:1px solid #000000 !important; }
            </style>
            <table><tr><td class="style10">Student ID</td></tr></table>
            "#,
        );
        let style = document.blocks[0].cells[0].style.clone();

        assert_eq!(style.align, Some(super::TextAlign::Center));
        assert!(style.bold);
        assert_eq!(style.border, Some(true));
        assert_eq!(style.font_size, Some(12.0));
        assert_eq!(style.padding_left, Some(3.75));
        assert_eq!(style.padding_right, Some(3.75));
    }

    #[test]
    fn parses_cell_overflow_and_white_space() {
        let document = parse(
            r#"
            <style>
            td.clip { overflow:hidden; white-space:nowrap; overflow-wrap:break-word; word-break:break-all; }
            </style>
            <table><tr><td class="clip">abc@example.com</td></tr></table>
            "#,
        );
        let style = document.blocks[0].cells[0].style.clone();

        assert_eq!(style.overflow, Some(super::Overflow::Hidden));
        assert_eq!(style.white_space, Some(super::WhiteSpace::NoWrap));
        assert_eq!(style.overflow_wrap, Some(super::OverflowWrap::BreakWord));
        assert_eq!(style.word_break, Some(super::WordBreak::BreakAll));
    }

    #[test]
    fn parses_numeric_font_weight_and_align_keywords() {
        let document = parse(
            r#"
            <style>
            td.heavy { font-weight: 800; }
            td.j { text-align: justify; }
            td.e { text-align: end; }
            </style>
            <table><tr>
              <td class="heavy">x</td>
              <td class="j">y</td>
              <td class="e">z</td>
            </tr></table>
            "#,
        );
        let cells = &document.blocks[0].cells;

        assert!(cells[0].style.bold, "font-weight:800 should be bold");
        assert_eq!(cells[1].style.align, Some(super::TextAlign::Left)); // justify -> left
        assert_eq!(cells[2].style.align, Some(super::TextAlign::Right)); // end -> right
    }

    #[test]
    fn parses_rgb_hsl_and_named_colors() {
        use crate::color::Color;
        // rgb() with commas and with modern space/slash syntax.
        assert_eq!(
            super::parse_css_color("rgb(18, 52, 86)"),
            Some(Color::from_rgb_u8(18, 52, 86))
        );
        assert_eq!(
            super::parse_css_color("rgba(18 52 86 / 50%)"),
            Some(Color::from_rgb_u8(18, 52, 86))
        );
        // Percentage channels.
        assert_eq!(
            super::parse_css_color("rgb(100%, 0%, 0%)"),
            Some(Color::from_rgb_u8(255, 0, 0))
        );
        // hsl(): pure red and a gray.
        assert_eq!(
            super::parse_css_color("hsl(0, 100%, 50%)"),
            Some(Color::from_rgb_u8(255, 0, 0))
        );
        assert_eq!(
            super::parse_css_color("hsl(0, 0%, 50%)"),
            Some(Color::from_rgb_u8(128, 128, 128))
        );
        // Extended named colors and 4/8-digit hex (alpha ignored).
        assert_eq!(
            super::parse_css_color("teal"),
            Some(Color::from_rgb_u8(0, 128, 128))
        );
        assert_eq!(
            super::parse_css_color("#11223344"),
            Some(Color::from_rgb_u8(0x11, 0x22, 0x33))
        );
        // Original mappings unchanged.
        assert_eq!(super::parse_css_color("red"), Some(Color::from_rgb_u8(255, 0, 0)));
        assert_eq!(super::parse_css_color("white"), Some(Color::WHITE));
        assert_eq!(super::parse_css_color("bogus"), None);
    }

    #[test]
    fn parses_cell_text_and_background_colors() {
        let document = parse(
            r#"
            <style>
            td.notice { color:#123456; background-color:#fed; }
            </style>
            <table><tr><td class="notice">Warning</td></tr></table>
            "#,
        );
        let style = document.blocks[0].cells[0].style.clone();

        assert_eq!(
            style.color,
            Some(crate::color::Color::from_rgb_u8(0x12, 0x34, 0x56))
        );
        assert_eq!(
            style.background_color,
            Some(crate::color::Color::from_rgb_u8(0xff, 0xee, 0xdd))
        );
    }

    #[test]
    fn parses_cell_vertical_alignment() {
        let document = parse(
            r#"
            <style>
            td.middle { vertical-align:middle; }
            td.bottom { vertical-align:bottom; }
            </style>
            <table><tr><td class="middle">A</td><td class="bottom">B</td></tr></table>
            "#,
        );

        assert_eq!(
            document.blocks[0].cells[0].style.vertical_align,
            Some(super::VerticalAlign::Middle)
        );
        assert_eq!(
            document.blocks[0].cells[1].style.vertical_align,
            Some(super::VerticalAlign::Bottom)
        );
    }

    #[test]
    fn applies_css_source_order_for_same_specificity() {
        let document = parse(
            r#"
            <style>
            .amount { font-size:8pt; text-align:left; }
            .amount { font-size:10pt; text-align:right; }
            </style>
            <table><tr><td class="amount">9.00</td></tr></table>
            "#,
        );
        let style = document.blocks[0].cells[0].style.clone();

        assert_eq!(style.font_size, Some(10.0));
        assert_eq!(style.align, Some(super::TextAlign::Right));
    }

    #[test]
    fn applies_higher_specificity_over_later_lower_specificity() {
        let document = parse(
            r#"
            <style>
            td.amount { font-size:12pt; text-align:center; }
            .amount { font-size:8pt; text-align:left; }
            </style>
            <table><tr><td class="amount">9.00</td></tr></table>
            "#,
        );
        let style = document.blocks[0].cells[0].style.clone();

        assert_eq!(style.font_size, Some(12.0));
        assert_eq!(style.align, Some(super::TextAlign::Center));
    }

    #[test]
    fn applies_important_over_higher_specificity_normal_declaration() {
        let document = parse(
            r#"
            <style>
            .amount { font-size:12pt !important; text-align:right !important; }
            td.amount { font-size:8pt; text-align:left; }
            </style>
            <table><tr><td class="amount">9.00</td></tr></table>
            "#,
        );
        let style = document.blocks[0].cells[0].style.clone();

        assert_eq!(style.font_size, Some(12.0));
        assert_eq!(style.align, Some(super::TextAlign::Right));
    }

    #[test]
    fn ignores_css_comments_in_rules_and_declarations() {
        // The old hand-rolled tokenizer split on raw ';' and '{', so a comment
        // containing them would corrupt parsing. cssparser handles this.
        let document = parse(
            r#"
            <style>
            /* a stray ; and { } in a comment */
            td.amount /* comment */ {
                font-size: 10pt; /* ; ; ; */
                text-align: right;
            }
            </style>
            <table><tr><td class="amount">9.00</td></tr></table>
            "#,
        );
        let style = document.blocks[0].cells[0].style.clone();

        assert_eq!(style.font_size, Some(10.0));
        assert_eq!(style.align, Some(super::TextAlign::Right));
    }

    #[test]
    fn handles_semicolons_inside_string_values() {
        // A ';' inside a quoted value must not end the declaration early.
        let document = parse(
            r#"
            <style>
            td.q { font-family: "Weird; Font"; text-align: center; }
            </style>
            <table><tr><td class="q">x</td></tr></table>
            "#,
        );

        assert_eq!(
            document.blocks[0].cells[0].style.align,
            Some(super::TextAlign::Center)
        );
    }

    #[test]
    fn parses_rules_inside_media_blocks() {
        let document = parse(
            r#"
            <style>
            @media print {
                td.amount { text-align: right; font-size: 12pt; }
            }
            </style>
            <table><tr><td class="amount">9.00</td></tr></table>
            "#,
        );
        let style = document.blocks[0].cells[0].style.clone();

        assert_eq!(style.align, Some(super::TextAlign::Right));
        assert_eq!(style.font_size, Some(12.0));
    }

    #[test]
    fn inherits_color_and_font_size_from_ancestors() {
        let document = parse(
            r#"
            <style>
            table { color: #123456; font-size: 13pt; }
            </style>
            <table><tr><td>x</td></tr></table>
            "#,
        );
        let style = document.blocks[0].cells[0].style.clone();

        assert_eq!(
            style.color,
            Some(crate::color::Color::from_rgb_u8(0x12, 0x34, 0x56))
        );
        assert_eq!(style.font_size, Some(13.0));
    }

    #[test]
    fn own_style_overrides_inherited() {
        let document = parse(
            r#"
            <style>
            table { font-size: 13pt; }
            td.big { font-size: 20pt; }
            </style>
            <table><tr><td class="big">x</td></tr></table>
            "#,
        );

        assert_eq!(document.blocks[0].cells[0].style.font_size, Some(20.0));
    }

    #[test]
    fn inherits_text_align_through_intermediate_ancestors() {
        let document = parse(
            r#"
            <style>
            table { text-align: center; }
            </style>
            <table><tbody><tr><td>x</td></tr></tbody></table>
            "#,
        );

        assert_eq!(
            document.blocks[0].cells[0].style.align,
            Some(super::TextAlign::Center)
        );
    }

    #[test]
    fn does_not_inherit_non_inheritable_properties() {
        // border and background-color must not flow from an ancestor to a cell.
        let document = parse(
            r#"
            <style>
            table { border: 1px solid black; background-color: #abcdef; }
            </style>
            <table><tr><td>x</td></tr></table>
            "#,
        );
        let style = document.blocks[0].cells[0].style.clone();

        assert_ne!(style.border, Some(true));
        assert_eq!(style.background_color, None);
    }

    #[test]
    fn reads_style_css_from_the_dom() {
        // Two separate <style> elements both contribute to the cascade.
        let document = parse(
            r#"
            <style>td.a { text-align: right; }</style>
            <style>td.a { font-size: 9pt; }</style>
            <table><tr><td class="a">9.00</td></tr></table>
            "#,
        );
        let style = document.blocks[0].cells[0].style.clone();

        assert_eq!(style.align, Some(super::TextAlign::Right));
        assert_eq!(style.font_size, Some(9.0));
    }

    #[test]
    fn applies_specificity_among_important_declarations() {
        let document = parse(
            r#"
            <style>
            td.amount { font-size:8pt !important; text-align:center !important; }
            .amount { font-size:12pt !important; text-align:left !important; }
            </style>
            <table><tr><td class="amount">9.00</td></tr></table>
            "#,
        );
        let style = document.blocks[0].cells[0].style.clone();

        assert_eq!(style.font_size, Some(8.0));
        assert_eq!(style.align, Some(super::TextAlign::Center));
    }
}
