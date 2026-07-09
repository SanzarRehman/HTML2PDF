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
    /// Link targets (`<a href>` values) interned while building the box tree;
    /// `InlineRun::link` is a 1-based index into this list (0 = no link).
    pub links: Vec<String>,
    /// `@font-face` rules from the stylesheet, in source order. Loaded once per
    /// render (`font::load_font_faces`) and consulted ahead of system lookup
    /// when resolving `font_specs`.
    pub font_faces: Vec<crate::font::FontFaceRule>,
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
    /// Styled inline runs for a *rich* cell — one that contains inline markup
    /// (`<b>`, `<a>`, `<span style>`, …). Empty for a plain text-only cell,
    /// which keeps the fast single-style path; when non-empty, layout wraps
    /// and paints these runs instead of `text` (`text` stays the flattened
    /// version, used for column sizing).
    pub runs: Vec<crate::box_tree::InlineRun>,
}

/// CSS `text-transform` (inherited). `None` (the variant) is an explicit
/// `text-transform: none`, which overrides an inherited transform.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextTransform {
    None,
    Uppercase,
    Lowercase,
    Capitalize,
}

/// Apply a `text-transform` to collected text. `Capitalize` uppercases the
/// first letter of each whitespace-delimited word; `at_boundary` says whether
/// the preceding character (possibly in an earlier run) was a word boundary,
/// and is updated for the caller.
pub(crate) fn apply_text_transform(
    text: &str,
    transform: TextTransform,
    at_boundary: &mut bool,
) -> String {
    match transform {
        TextTransform::None => {
            if let Some(last) = text.chars().last() {
                *at_boundary = last.is_whitespace();
            }
            text.to_string()
        }
        TextTransform::Uppercase => {
            if let Some(last) = text.chars().last() {
                *at_boundary = last.is_whitespace();
            }
            text.to_uppercase()
        }
        TextTransform::Lowercase => {
            if let Some(last) = text.chars().last() {
                *at_boundary = last.is_whitespace();
            }
            text.to_lowercase()
        }
        TextTransform::Capitalize => {
            let mut out = String::with_capacity(text.len());
            for ch in text.chars() {
                if ch.is_whitespace() {
                    *at_boundary = true;
                    out.push(ch);
                } else if *at_boundary {
                    *at_boundary = false;
                    out.extend(ch.to_uppercase());
                } else {
                    out.push(ch);
                }
            }
            out
        }
    }
}

/// Rarely-set sizing declarations, boxed on [`CellStyle`] so the common cell
/// (which sets none of these) pays a single pointer rather than a dozen
/// `Option<f32>` slots — the 22k-cell spreadsheet path stays RAM-flat, the same
/// tactic as the boxed border sides. All percentages are of the containing
/// block (width for `min-width`/padding/margin/left/right offsets; height for
/// top/bottom offsets), resolved at layout time. Padding/margin/offset arrays
/// are ordered `[top, right, bottom, left]`.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct SizingCss {
    pub min_width: Option<f32>,
    pub min_width_percent: Option<f32>,
    pub min_height: Option<f32>,
    pub max_height: Option<f32>,
    pub padding_percent: [Option<f32>; 4],
    pub margin_percent: [Option<f32>; 4],
    pub offset_percent: [Option<f32>; 4],
}

impl SizingCss {
    /// Cascade merge: a later layer's set sub-properties win, field by field, so
    /// setting only `min-width` in one rule does not wipe a `max-height` from
    /// another (mirrors the per-side border merge).
    fn merge(&mut self, other: SizingCss) {
        self.min_width = other.min_width.or(self.min_width);
        self.min_width_percent = other.min_width_percent.or(self.min_width_percent);
        self.min_height = other.min_height.or(self.min_height);
        self.max_height = other.max_height.or(self.max_height);
        for i in 0..4 {
            self.padding_percent[i] = other.padding_percent[i].or(self.padding_percent[i]);
            self.margin_percent[i] = other.margin_percent[i].or(self.margin_percent[i]);
            self.offset_percent[i] = other.offset_percent[i].or(self.offset_percent[i]);
        }
    }
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
    /// An explicit `text-decoration: none`, remembered so UA defaults that
    /// would decorate (the `<a href>` underline) know the author opted out.
    pub decoration_none: bool,
    /// Whether the box draws a border. `None` means unset (so a more specific
    /// `border: none` can override a less specific border rule in the cascade).
    /// Kept as the any-visible-border *summary* that table heuristics and fast
    /// paths read; the per-side truth lives in `border_top..border_left`.
    pub border: Option<bool>,
    /// Per-side cascaded border sub-properties + `border-radius`
    /// (non-inherited), resolved to paintable sides by [`resolved_borders`].
    /// Boxed so the ~99% of computed styles with no border declarations pay
    /// one pointer, not four inline sides — `CellStyle` is cloned per node.
    pub border_sides: Option<Box<BorderSides>>,
    /// `box-sizing`: `Some(true)` = `border-box` (declared width/height include
    /// padding and borders), `Some(false)` = explicit `content-box`.
    pub border_box: Option<bool>,
    pub overflow: Option<Overflow>,
    pub font_size: Option<f32>,
    /// First usable family from CSS `font-family` (inherited): a concrete name
    /// or a generic keyword (`serif`, `monospace`, …). `None` = document font.
    pub font_family: Option<String>,
    /// CSS `font-style` (inherited): `Some(true)` = italic/oblique,
    /// `Some(false)` = an explicit `normal` (overrides an inherited italic).
    pub italic: Option<bool>,
    /// CSS `direction` / the HTML `dir` attribute (inherited): `Some(true)` =
    /// rtl, `Some(false)` = ltr, `None` = unset (inherits; ltr at the root).
    pub direction: Option<bool>,
    /// CSS `line-height` (inherited). `None` = `normal` (UA default leading).
    pub line_height: Option<LineHeight>,
    /// CSS `width`/`height` in points. Consumed by `<img>` sizing, blocks,
    /// floats, and positioned boxes; table column/row geometry uses a separate
    /// parse. A percentage width lives in `width_percent` (resolved against
    /// the containing block at layout time).
    pub width: Option<f32>,
    pub width_percent: Option<f32>,
    pub height: Option<f32>,
    /// CSS `max-width` in points / percent of the containing block.
    pub max_width: Option<f32>,
    pub max_width_percent: Option<f32>,
    /// `margin-left: auto` / `margin-right: auto` (both set + a width =
    /// horizontal centering).
    pub margin_left_auto: bool,
    pub margin_right_auto: bool,
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
    /// `flex-wrap: wrap` (or a wrapping `flex-flow`) on a flex container.
    pub flex_wrap: Option<bool>,
    /// `display: grid` — this element establishes a grid container.
    pub display_grid: bool,
    /// `grid-template-columns` track list (`None` = single auto column).
    pub grid_template: Option<Vec<GridTrack>>,
    /// `row-gap` (or the first value of a two-value `gap`), points.
    pub row_gap: Option<f32>,
    /// `grid-column: span N` on a grid item.
    pub grid_span: Option<usize>,
    /// Line-based `grid-column: start [/ end]` (1-based; negative counts from
    /// the end, so `1 / -1` spans the full row).
    pub grid_col_start: Option<i32>,
    pub grid_col_end: Option<i32>,
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
    /// CSS `text-transform` (inherited); applied when text is collected into
    /// runs / cell text, so measurement sees the transformed string.
    pub text_transform: Option<TextTransform>,
    /// CSS `letter-spacing` (inherited): extra advance per character, points.
    /// `Some(0.0)` is an explicit `normal` (overrides an inherited spacing).
    pub letter_spacing: Option<f32>,
    /// CSS `word-spacing` (inherited): extra advance per inter-word space.
    pub word_spacing: Option<f32>,
    /// CSS `text-indent` (inherited): first-line indent in points / percent of
    /// the containing width.
    pub text_indent: Option<f32>,
    pub text_indent_percent: Option<f32>,
    /// Boxed sizing extras (`min-width`, `min/max-height`, `%` padding/margin/
    /// offsets). `None` for the overwhelmingly common cell that sets none.
    pub sizing: Option<Box<SizingCss>>,
}

impl Default for CellStyle {
    fn default() -> Self {
        Self {
            align: None,
            vertical_align: None,
            bold: false,
            underline: false,
            line_through: false,
            decoration_none: false,
            border: None,
            border_sides: None,
            border_box: None,
            overflow: None,
            font_size: None,
            font_family: None,
            italic: None,
            direction: None,
            line_height: None,
            width: None,
            width_percent: None,
            height: None,
            max_width: None,
            max_width_percent: None,
            margin_left_auto: false,
            margin_right_auto: false,
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
            flex_wrap: None,
            display_grid: false,
            grid_template: None,
            row_gap: None,
            grid_span: None,
            grid_col_start: None,
            grid_col_end: None,
            float_dir: None,
            clear: None,
            position: None,
            z_index: None,
            offset_top: None,
            offset_right: None,
            offset_bottom: None,
            offset_left: None,
            text_transform: None,
            letter_spacing: None,
            word_spacing: None,
            text_indent: None,
            text_indent_percent: None,
            sizing: None,
        }
    }
}

/// One cascaded border side's sub-properties. Each cascades independently
/// (`border-color` may arrive in a different rule than `border-style`), so all
/// three stay optional until [`resolved_borders`] runs.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct BorderSideCss {
    /// Width in points (`thin`/`medium`/`thick` already mapped).
    pub width: Option<f32>,
    pub style: Option<BorderStyle>,
    pub color: Option<Color>,
}

/// The boxed per-side border data of one style: four cascaded sides plus the
/// uniform `border-radius`. Lives behind an `Option<Box<..>>` on `CellStyle`.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct BorderSides {
    pub top: BorderSideCss,
    pub right: BorderSideCss,
    pub bottom: BorderSideCss,
    pub left: BorderSideCss,
    /// `border-radius`, single uniform corner radius in points (per-corner
    /// and elliptical radii are not parsed).
    pub radius: Option<f32>,
}

/// CSS border line styles the painter distinguishes. `double`, `groove`,
/// `ridge`, `inset`, and `outset` parse as `Solid` (a visible approximation);
/// `hidden` parses as `None`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BorderStyle {
    None,
    Solid,
    Dashed,
    Dotted,
}

/// A resolved, paintable border side (style is never `None` here).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BorderSide {
    pub width: f32,
    pub style: BorderStyle,
    pub color: Color,
}

/// The four resolved border sides of a box (`None` = that side absent).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct BorderEdges {
    pub top: Option<BorderSide>,
    pub right: Option<BorderSide>,
    pub bottom: Option<BorderSide>,
    pub left: Option<BorderSide>,
}

impl BorderEdges {
    /// The single side shared by all four edges, when they are all present and
    /// identical — the fast path that strokes one rectangle.
    pub fn uniform(&self) -> Option<BorderSide> {
        match (self.top, self.right, self.bottom, self.left) {
            (Some(t), Some(r), Some(b), Some(l)) if t == r && r == b && b == l => Some(t),
            _ => None,
        }
    }

    /// Per-side widths as box edges (absent sides are zero) — the layout space
    /// the border consumes, folded into the block's padding.
    pub fn widths(&self) -> crate::box_tree::Edges {
        let w = |side: Option<BorderSide>| side.map_or(0.0, |s| s.width);
        crate::box_tree::Edges {
            top: w(self.top),
            right: w(self.right),
            bottom: w(self.bottom),
            left: w(self.left),
        }
    }
}

/// Resolve the cascaded per-side border sub-properties to paintable sides.
/// Missing sub-properties default like CSS: width `medium` (3px), color
/// `currentColor` (the element's text color, else black) — with one legacy
/// lenience: a declared width with *no* style paints `Solid` (browsers hide
/// it, but spreadsheet exports rely on `border: 1px` drawing gridlines).
pub(crate) fn resolved_borders(style: &CellStyle) -> Option<BorderEdges> {
    let resolve = |side: BorderSideCss| -> Option<BorderSide> {
        let kind = match side.style {
            Some(BorderStyle::None) => return None,
            Some(kind) => kind,
            None if side.width.is_some() => BorderStyle::Solid,
            None => return None,
        };
        let width = side.width.unwrap_or(MEDIUM_BORDER_WIDTH);
        if width <= 0.0 {
            return None;
        }
        Some(BorderSide {
            width,
            style: kind,
            color: side.color.or(style.color).unwrap_or(Color::BLACK),
        })
    };
    let sides = style.border_sides.as_deref()?;
    let edges = BorderEdges {
        top: resolve(sides.top),
        right: resolve(sides.right),
        bottom: resolve(sides.bottom),
        left: resolve(sides.left),
    };
    (edges.top.is_some() || edges.right.is_some() || edges.bottom.is_some()
        || edges.left.is_some())
    .then_some(edges)
}

/// CSS `medium` border width: 3px at 96 dpi, in points.
const MEDIUM_BORDER_WIDTH: f32 = 2.25;

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
    /// `text-align: justify` — inter-word spaces stretch so every line but the
    /// paragraph's last fills the measure.
    Justify,
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
    /// `minmax(min, max)`: the track is at least `min` (points, or content
    /// size for `auto`) and grows toward `max` (points, an `fr` share, or
    /// content size).
    MinMax(MinTrack, MaxTrack),
}

/// The `min` component of `minmax()`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum MinTrack {
    Pt(f32),
    Auto,
}

/// The `max` component of `minmax()`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum MaxTrack {
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
    let links = std::cell::RefCell::new(LinkInterner::new());
    let env = FlowEnv {
        stylesheet: &stylesheet,
        computed: &computed,
        table_columns: &table_columns,
        row_height: table_style.row_height,
        fonts: &fonts,
        links: &links,
    };
    let flow = build_flow(&dom, &env);

    // A document with real flow content around its tables is laid out as flow
    // (headings/paragraphs and tables interleaved in document order). A bare
    // table falls back to the dedicated spreadsheet path (`blocks`), preserving
    // its fast, well-tuned layout.
    let (mut blocks, mut flow) = match flow {
        Some(root) if root.has_nontable_content() => (Vec::new(), Some(root)),
        _ => (tables_from_dom(&dom, &stylesheet, &computed, &fonts, &links), None),
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
        links: links.into_inner().into_targets(),
        font_faces: stylesheet.font_faces,
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

/// Interns `<a href>` targets seen while building the box tree. Runs store a
/// 1-based `u16` index (0 = not a link); the target list lands on the document
/// and is carried into `RenderOptions` for the PDF writer.
pub(crate) struct LinkInterner {
    targets: Vec<String>,
    map: std::collections::HashMap<String, u16>,
}

impl LinkInterner {
    fn new() -> Self {
        Self {
            targets: Vec::new(),
            map: std::collections::HashMap::new(),
        }
    }

    fn intern(&mut self, target: &str) -> u16 {
        if let Some(&index) = self.map.get(target) {
            return index;
        }
        // 1-based; saturate (drop to "no link") past the u16 range.
        if self.targets.len() >= u16::MAX as usize - 1 {
            return 0;
        }
        self.targets.push(target.to_string());
        let index = self.targets.len() as u16;
        self.map.insert(target.to_string(), index);
        index
    }

    fn into_targets(self) -> Vec<String> {
        self.targets
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
    links: &'a std::cell::RefCell<LinkInterner>,
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
        base_rtl: false,
        link: 0,
        transform: TextTransform::None,
        letter_spacing: 0.0,
        word_spacing: 0.0,
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
pub fn resolve_images(
    document: &mut Document,
    base_dir: Option<&std::path::Path>,
    remote: &crate::image::RemoteImagePolicy,
) {
    let Some(flow) = document.flow.as_mut() else {
        return;
    };
    let mut images = std::mem::take(&mut document.images);
    resolve_images_in(&mut flow.children, base_dir, remote, &mut images);
    document.images = images;
}

fn resolve_images_in(
    children: &mut [crate::box_tree::BoxChild],
    base_dir: Option<&std::path::Path>,
    remote: &crate::image::RemoteImagePolicy,
    images: &mut Vec<crate::image::DecodedImage>,
) {
    use crate::box_tree::BoxChild;
    for child in children {
        match child {
            BoxChild::Block(block) => {
                resolve_images_in(&mut block.children, base_dir, remote, images)
            }
            BoxChild::Image(image) => resolve_image_box(image, base_dir, remote, images),
            // Inline images live inside line runs; resolve them in place.
            BoxChild::Line(runs) => {
                for run in runs {
                    if let Some(image) = run.image.as_deref_mut() {
                        resolve_image_box(image, base_dir, remote, images);
                    }
                }
            }
            // Table cells carry no `<img>` content in the current model.
            BoxChild::Table(_) => {}
        }
    }
}

/// CSS pixels to PDF points at the reference 96 dpi (1px = 0.75pt).
const PX_TO_PT: f32 = 72.0 / 96.0;

fn resolve_image_box(
    image: &mut crate::box_tree::ImageBox,
    base_dir: Option<&std::path::Path>,
    remote: &crate::image::RemoteImagePolicy,
    images: &mut Vec<crate::image::DecodedImage>,
) {
    let Some(decoded) = crate::image::load_image(&image.src, base_dir, remote) else {
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
    /// Base paragraph direction (UAX #9 base level): `true` = rtl.
    base_rtl: bool,
    /// Interned link target the content sits inside (0 = none).
    link: u16,
    /// Inherited `text-transform`, applied as text is collected into runs.
    transform: TextTransform,
    /// Inherited `letter-spacing` / `word-spacing`, points (0 = none).
    letter_spacing: f32,
    word_spacing: f32,
}

/// Accumulates one block's children, buffering inline text into a pending line
/// box that is emitted whenever a block boundary (or `<br>`) is reached.
struct ChildAcc {
    children: Vec<crate::box_tree::BoxChild>,
    pending: Vec<crate::box_tree::InlineRun>,
    /// Whether the previously collected character was a word boundary — the
    /// cross-run state `text-transform: capitalize` needs (a word may split
    /// across style runs).
    word_boundary: bool,
}

impl Default for ChildAcc {
    fn default() -> Self {
        Self {
            children: Vec::new(),
            pending: Vec::new(),
            word_boundary: true,
        }
    }
}

impl ChildAcc {
    /// Append inline text under the current style, merging into the previous run
    /// when the style matches to keep the run count low. The context's
    /// `text-transform` is applied here, so measurement and painting both see
    /// the transformed string.
    fn push_text(&mut self, text: &str, ctx: &FlowCtx) {
        if text.is_empty() {
            return;
        }
        // The no-transform path (the overwhelmingly common case) borrows the
        // text; only an active transform allocates.
        let text: std::borrow::Cow<str> = if ctx.transform == TextTransform::None {
            if let Some(last) = text.chars().last() {
                self.word_boundary = last.is_whitespace();
            }
            std::borrow::Cow::Borrowed(text)
        } else {
            std::borrow::Cow::Owned(apply_text_transform(
                text,
                ctx.transform,
                &mut self.word_boundary,
            ))
        };
        if let Some(last) = self.pending.last_mut() {
            if last.image.is_none()
                && last.font_size == ctx.font_size
                && last.bold == ctx.bold
                && last.font == ctx.font
                && last.underline == ctx.underline
                && last.line_through == ctx.line_through
                && last.color == ctx.color
                && last.link == ctx.link
                && last.letter_spacing == ctx.letter_spacing
                && last.word_spacing == ctx.word_spacing
            {
                last.text.push_str(&text);
                return;
            }
        }
        self.pending.push(crate::box_tree::InlineRun {
            text: text.into_owned(),
            font_size: ctx.font_size,
            bold: ctx.bold,
            font: ctx.font,
            link: ctx.link,
            underline: ctx.underline,
            line_through: ctx.line_through,
            letter_spacing: ctx.letter_spacing,
            word_spacing: ctx.word_spacing,
            color: ctx.color,
            image: None,
        });
    }

    /// Append an inline image, flowing with the surrounding text (its run
    /// carries the context's link so a linked image stays clickable).
    fn push_image(&mut self, image: crate::box_tree::ImageBox, ctx: &FlowCtx) {
        self.word_boundary = true;
        self.pending.push(crate::box_tree::InlineRun {
            text: String::new(),
            font_size: ctx.font_size,
            bold: false,
            font: ctx.font,
            link: ctx.link,
            underline: false,
            line_through: false,
            letter_spacing: 0.0,
            word_spacing: 0.0,
            color: ctx.color,
            image: Some(Box::new(image)),
        });
    }

    /// Emit the pending inline content as a line box, if it carries any text
    /// (or an inline image, which is content in its own right).
    fn flush_line(&mut self) {
        self.word_boundary = true;
        if self
            .pending
            .iter()
            .any(|run| run.image.is_some() || !run.text.trim().is_empty())
        {
            self.children
                .push(crate::box_tree::BoxChild::Line(std::mem::take(&mut self.pending)));
        } else {
            self.pending.clear();
        }
    }
}

/// Generated-content text and context for an element's `::before`/`::after`:
/// `Some` when a pseudo rule matches and its `content` resolves to text. The
/// rule's declarations fold onto the element's context the way an inline
/// element's style would (color, weight, size, family, decoration, spacing).
fn pseudo_run(
    env: &FlowEnv,
    dom: &crate::dom::Dom,
    id: crate::dom::NodeId,
    base: &FlowCtx,
    which: PseudoElement,
) -> Option<(String, FlowCtx)> {
    let layer = env.stylesheet.pseudo_declarations(dom, id, which)?;
    let text = parse_content_value(layer.content.as_deref()?, dom, id)?;
    let style = &layer.cell;
    let bold = base.bold || style.bold;
    let italic = style.italic.unwrap_or(base.italic);
    let family = match &style.font_family {
        Some(name) => Some(env.fonts.borrow_mut().family(name)),
        None => base.family,
    };
    let font = env.fonts.borrow_mut().spec(family, bold, italic);
    let ctx = FlowCtx {
        font_size: style.font_size.unwrap_or(base.font_size),
        bold,
        italic,
        family,
        font,
        underline: base.underline || style.underline,
        line_through: base.line_through || style.line_through,
        color: style.color.unwrap_or(base.color),
        align: base.align,
        base_rtl: base.base_rtl,
        link: base.link,
        transform: style.text_transform.unwrap_or(base.transform),
        letter_spacing: style.letter_spacing.unwrap_or(base.letter_spacing),
        word_spacing: style.word_spacing.unwrap_or(base.word_spacing),
    };
    Some((text, ctx))
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
                collect_table_rows(
                    dom,
                    id,
                    TableSection::Body,
                    env.stylesheet,
                    computed,
                    env.fonts,
                    env.links,
                    &mut rows,
                );
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
                // Resolved (loaded/measured) after parsing. An image that
                // shares its line with text flows *inline* (on the baseline);
                // a standalone or floated one takes the block image path
                // (page-fitting, pagination as a unit).
                if let Some(src) = node.attr("src") {
                    if !src.is_empty() {
                        let own = &computed.style[id];
                        let image = crate::box_tree::ImageBox {
                            src: src.to_string(),
                            attr_width: node
                                .attr("width")
                                .and_then(|v| v.trim().parse::<f32>().ok()),
                            attr_height: node
                                .attr("height")
                                .and_then(|v| v.trim().parse::<f32>().ok()),
                            css_width: own.width,
                            css_width_percent: own.width_percent,
                            css_height: own.height,
                            max_width: own.max_width,
                            max_width_percent: own.max_width_percent,
                            image_index: None,
                            width: 0.0,
                            height: 0.0,
                            float_dir: own.float_dir,
                        };
                        let pending_text = acc
                            .pending
                            .iter()
                            .any(|run| run.image.is_some() || !run.text.trim().is_empty());
                        let inline = own.float_dir.is_none()
                            && (pending_text || followed_by_inline_text(dom, id));
                        if inline {
                            acc.push_image(image, &ctx);
                        } else {
                            acc.flush_line();
                            acc.children.push(crate::box_tree::BoxChild::Image(image));
                        }
                    }
                }
            } else if is_block_tag(tag) {
                acc.flush_line();
                if let Some(block) = build_block(dom, id, env, ctx, tag) {
                    acc.children.push(crate::box_tree::BoxChild::Block(block));
                }
            } else {
                // Inline element: fold its computed style into the context and
                // let its children contribute to the enclosing line — with any
                // `::before`/`::after` generated text around them.
                let child_ctx = inline_ctx(&ctx, env, dom, id, tag);
                if env.stylesheet.has_pseudo {
                    if let Some((text, pseudo_ctx)) =
                        pseudo_run(env, dom, id, &child_ctx, PseudoElement::Before)
                    {
                        acc.push_text(&text, &pseudo_ctx);
                    }
                }
                for &child in &node.children {
                    build_node(dom, child, env, child_ctx, acc);
                }
                if env.stylesheet.has_pseudo {
                    if let Some((text, pseudo_ctx)) =
                        pseudo_run(env, dom, id, &child_ctx, PseudoElement::After)
                    {
                        acc.push_text(&text, &pseudo_ctx);
                    }
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
    // Base direction: this element's own `direction`/`dir`, else inherited.
    // When an element sets a new direction and specifies no `text-align`, the
    // default alignment follows that direction (`start` edge); otherwise
    // `text-align` inherits as usual.
    let own_dir = own_direction(dom, id, own);
    let base_rtl = own_dir.unwrap_or(parent.base_rtl);
    let align = match own.align {
        Some(explicit) => explicit,
        None if own_dir.is_some() && own_dir != Some(parent.base_rtl) => {
            if base_rtl { TextAlign::Right } else { TextAlign::Left }
        }
        None => parent.align,
    };

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
    // Border widths consume layout space like padding (content sits inside
    // them), so they fold into the padding edges here; the painted background
    // and border rectangle span this same (border) box, matching how CSS
    // backgrounds extend under the border.
    let border = resolved_borders(own);
    let border_widths = border.map(|edges| edges.widths()).unwrap_or_default();
    let padding = crate::box_tree::Edges {
        top: own.padding_top.unwrap_or(0.0) + border_widths.top,
        right: own.padding_right.unwrap_or(0.0) + border_widths.right,
        bottom: own.padding_bottom.unwrap_or(0.0) + border_widths.bottom,
        left: own.padding_left.unwrap_or(0.0) + border_widths.left,
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
        base_rtl,
        link: parent.link,
        // Inherited text properties: the computed style already folded the
        // ancestors in, so `own` is authoritative (ctx fallback is belt+braces
        // for styles built outside the inheritance pass).
        transform: own.text_transform.unwrap_or(parent.transform),
        letter_spacing: own.letter_spacing.unwrap_or(parent.letter_spacing),
        word_spacing: own.word_spacing.unwrap_or(parent.word_spacing),
    };

    let mut acc = ChildAcc::default();
    if tag == "li" {
        let marker = li_marker(dom, id);
        acc.push_text(&marker, &child_ctx);
    }
    // `::before` generated content leads the block's own children.
    if env.stylesheet.has_pseudo {
        if let Some((text, pseudo_ctx)) =
            pseudo_run(env, dom, id, &child_ctx, PseudoElement::Before)
        {
            acc.push_text(&text, &pseudo_ctx);
        }
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
    // `::after` generated content trails the block's own children.
    if env.stylesheet.has_pseudo {
        if let Some((text, pseudo_ctx)) =
            pseudo_run(env, dom, id, &child_ctx, PseudoElement::After)
        {
            acc.push_text(&text, &pseudo_ctx);
        }
    }
    acc.flush_line();

    if acc.children.is_empty() {
        // An empty block is normally dropped, but a *decorated* one with an
        // explicit size still paints (the `z-index: -1` background-layer div).
        let paints_alone = (background.is_some() || border.is_some())
            && (own.width.is_some() || own.width_percent.is_some() || own.height.is_some());
        if !paints_alone {
            return None;
        }
    }
    let flex = own.display_flex.then(|| crate::box_tree::FlexContainer {
        direction: own.flex_direction.unwrap_or(FlexDirection::Row),
        justify: own.justify_content.unwrap_or(JustifyContent::FlexStart),
        align: own.align_items.unwrap_or(AlignItems::Stretch),
        gap: own.gap.unwrap_or(0.0),
        wrap: own.flex_wrap.unwrap_or(false),
    });
    let grid = own.display_grid.then(|| crate::box_tree::GridContainer {
        columns: own.grid_template.clone().unwrap_or_default(),
        column_gap: own.gap.unwrap_or(0.0),
        row_gap: own.row_gap.unwrap_or(0.0),
    });
    // Boxed sizing extras (`min-width`, `min/max-height`, `%` padding/margin/
    // offsets). Almost always `None`, so the mapping is cheap.
    let sizing = own.sizing.as_deref();
    let edges_pct = |arr: [Option<f32>; 4]| crate::box_tree::EdgesPercent {
        top: arr[0],
        right: arr[1],
        bottom: arr[2],
        left: arr[3],
    };
    Some(crate::box_tree::BlockBox {
        kind,
        margin,
        padding,
        align,
        background,
        border,
        border_radius: own
            .border_sides
            .as_deref()
            .and_then(|sides| sides.radius)
            .unwrap_or(0.0),
        border_box: own.border_box.unwrap_or(false),
        flex,
        flex_grow: own.flex_grow.unwrap_or(0.0),
        flex_basis: own.flex_basis,
        grid,
        grid_span: own.grid_span.unwrap_or(1),
        grid_col_start: own.grid_col_start,
        grid_col_end: own.grid_col_end,
        float_dir: own.float_dir,
        clear: own.clear,
        css_width: own.width,
        css_width_percent: own.width_percent,
        max_width: own.max_width,
        max_width_percent: own.max_width_percent,
        css_height: own.height,
        min_width: sizing.and_then(|s| s.min_width),
        min_width_percent: sizing.and_then(|s| s.min_width_percent),
        min_height: sizing.and_then(|s| s.min_height),
        max_height: sizing.and_then(|s| s.max_height),
        padding_percent: sizing
            .map(|s| edges_pct(s.padding_percent))
            .unwrap_or_default(),
        margin_percent: sizing
            .map(|s| edges_pct(s.margin_percent))
            .unwrap_or_default(),
        overflow_hidden: own.overflow == Some(Overflow::Hidden),
        center: own.margin_left_auto && own.margin_right_auto,
        line_height: own.line_height,
        rtl: base_rtl,
        text_indent: own.text_indent.unwrap_or(0.0),
        text_indent_percent: own.text_indent_percent,
        position: own.position,
        z_index: own.z_index,
        offset_top: own.offset_top,
        offset_right: own.offset_right,
        offset_bottom: own.offset_bottom,
        offset_left: own.offset_left,
        offset_percent: sizing
            .map(|s| edges_pct(s.offset_percent))
            .unwrap_or_default(),
        anchor: dom
            .node(id)
            .attr("id")
            .filter(|value| !value.is_empty())
            .map(str::to_string),
        children: acc.children,
    })
}

/// Fold an inline element's computed style into the surrounding context. Block
/// alignment is unaffected; `<b>`/`<strong>` force bold, `<i>`/`<em>` (and
/// citation-family tags) force italic, and `<code>`-family tags default to
/// monospace, even without a rule (there is no UA stylesheet).
fn inline_ctx(
    parent: &FlowCtx,
    env: &FlowEnv,
    dom: &crate::dom::Dom,
    id: crate::dom::NodeId,
    tag: &str,
) -> FlowCtx {
    let own = &env.computed.style[id];
    // An `<a href>` interns its target and gets the UA link style (blue,
    // underlined) unless the author overrides color / opts out of decoration.
    let href = if tag == "a" {
        dom.node(id).attr("href").filter(|value| !value.is_empty())
    } else {
        None
    };
    let link = match href {
        Some(target) => env.links.borrow_mut().intern(target),
        None => parent.link,
    };
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
        underline: parent.underline
            || own.underline
            || matches!(tag, "u" | "ins")
            || (href.is_some() && !own.decoration_none),
        line_through: parent.line_through || own.line_through || matches!(tag, "s" | "strike" | "del"),
        color: own.color.unwrap_or(if href.is_some() { LINK_COLOR } else { parent.color }),
        align: parent.align,
        // An inline `dir`/`direction` (e.g. on `<body>`, which is inline in the
        // flow builder) sets the base direction inherited by descendant blocks.
        base_rtl: own_direction(dom, id, own).unwrap_or(parent.base_rtl),
        link,
        transform: own.text_transform.unwrap_or(parent.transform),
        letter_spacing: own.letter_spacing.unwrap_or(parent.letter_spacing),
        word_spacing: own.word_spacing.unwrap_or(parent.word_spacing),
    }
}

/// The UA default link color (`#0000EE`, matching browser stylesheets).
const LINK_COLOR: Color = Color {
    r: 0.0,
    g: 0.0,
    b: 0.93,
};

/// An element's own base direction: the CSS `direction` property wins over the
/// presentational `dir` attribute (`rtl`/`ltr`; `auto` is not resolved yet).
/// `None` means the element sets no direction and inherits its parent's.
fn own_direction(
    dom: &crate::dom::Dom,
    id: crate::dom::NodeId,
    own: &CellStyle,
) -> Option<bool> {
    own.direction.or_else(|| match dom.node(id).attr("dir") {
        Some(value) if value.eq_ignore_ascii_case("rtl") => Some(true),
        Some(value) if value.eq_ignore_ascii_case("ltr") => Some(false),
        _ => None,
    })
}

/// Whether inline text follows `id` before the next block-level boundary among
/// its siblings — i.e. whether an `<img>` here would share its line with text
/// that comes after it (text *before* it is visible in the accumulator).
fn followed_by_inline_text(dom: &crate::dom::Dom, id: crate::dom::NodeId) -> bool {
    use crate::dom::NodeData;
    let Some(parent) = dom.node(id).parent else {
        return false;
    };
    let siblings = &dom.node(parent).children;
    let Some(position) = siblings.iter().position(|&child| child == id) else {
        return false;
    };
    for &sibling in &siblings[position + 1..] {
        match &dom.node(sibling).data {
            NodeData::Text(text) => {
                if !text.trim().is_empty() {
                    return true;
                }
            }
            NodeData::Element { name, .. } => {
                let tag = name.as_str();
                if is_block_tag(tag) || matches!(tag, "br" | "table" | "img") {
                    return false;
                }
                if matches!(tag, "head" | "script" | "style" | "title") {
                    continue;
                }
                if inline_subtree_has_text(dom, sibling) {
                    return true;
                }
            }
            NodeData::Document => {}
        }
    }
    false
}

/// Whether an inline element's subtree carries any non-whitespace text
/// (descending only through inline children).
fn inline_subtree_has_text(dom: &crate::dom::Dom, id: crate::dom::NodeId) -> bool {
    use crate::dom::NodeData;
    for &child in &dom.node(id).children {
        match &dom.node(child).data {
            NodeData::Text(text) => {
                if !text.trim().is_empty() {
                    return true;
                }
            }
            NodeData::Element { name, .. } => {
                let tag = name.as_str();
                if !is_block_tag(tag)
                    && !matches!(tag, "head" | "script" | "style" | "title" | "br" | "table")
                    && inline_subtree_has_text(dom, child)
                {
                    return true;
                }
            }
            NodeData::Document => {}
        }
    }
    false
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
            // `@font-face` carries no page/table geometry.
            AtRuleKind::FontFace | AtRuleKind::Other => Ok(Vec::new()),
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
    fonts: &std::cell::RefCell<FontInterner>,
    links: &std::cell::RefCell<LinkInterner>,
) -> Vec<Block> {
    let mut rows = Vec::new();
    collect_table_rows(
        dom,
        dom.root(),
        TableSection::Body,
        stylesheet,
        computed,
        fonts,
        links,
        &mut rows,
    );
    rows
}

#[allow(clippy::too_many_arguments)]
fn collect_table_rows(
    dom: &crate::dom::Dom,
    id: crate::dom::NodeId,
    section: TableSection,
    stylesheet: &Stylesheet,
    computed: &ComputedStyles,
    fonts: &std::cell::RefCell<FontInterner>,
    links: &std::cell::RefCell<LinkInterner>,
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
            let cells = cells_from_row(dom, id, stylesheet, computed, fonts, links);
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
        collect_table_rows(dom, child, child_section, stylesheet, computed, fonts, links, rows);
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
    stylesheet: &Stylesheet,
    computed: &ComputedStyles,
    fonts: &std::cell::RefCell<FontInterner>,
    links: &std::cell::RefCell<LinkInterner>,
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
        let mut text = collapse_whitespace(&text);
        // `text-transform` applies to the flattened cell text (the classic
        // `th { text-transform: uppercase }`), so column sizing measures the
        // transformed string. Descendant-specific transforms flatten to the
        // cell's own (rich cells transform per run instead).
        if let Some(transform) = computed.style[child].text_transform {
            let mut boundary = true;
            text = apply_text_transform(&text, transform, &mut boundary);
        }

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
        // Fold the cell's own `dir` attribute into the computed direction
        // (the CSS `direction` property still wins, as in the flow path).
        style.direction = own_direction(dom, child, &style);

        // A cell with inline markup gets styled runs (the rich path); a plain
        // text-only cell keeps `runs` empty — the fast single-style path,
        // byte-identical to the previous behavior.
        let runs = if cell_has_markup(dom, child) {
            collect_cell_runs(dom, child, stylesheet, computed, &style, fonts, links)
        } else {
            Vec::new()
        };

        cells.push(TableCell {
            text,
            colspan,
            style,
            font: 0, // interned in the post-pass over the finished document
            runs,
        });
    }

    cells
}

/// Whether a cell contains any element markup worth per-run styling (anything
/// beyond bare text, ignoring non-rendered and layout-neutral tags).
fn cell_has_markup(dom: &crate::dom::Dom, id: crate::dom::NodeId) -> bool {
    dom.node(id).children.iter().any(|&child| match &dom.node(child).data {
        crate::dom::NodeData::Element { name, .. } => {
            !matches!(name.as_str(), "script" | "style" | "head" | "title" | "br" | "img")
                || cell_has_markup(dom, child)
        }
        _ => false,
    })
}

/// Build a rich cell's styled inline runs: descend the cell's subtree folding
/// each element's computed style into the context (exactly like flow inline
/// content — bold/italic/color/size/family, `<a href>` link + UA styling),
/// flattening block-level descendants inline. `<img>` and `<br>` are skipped,
/// matching the flat-text collector.
fn collect_cell_runs(
    dom: &crate::dom::Dom,
    cell_id: crate::dom::NodeId,
    stylesheet: &Stylesheet,
    computed: &ComputedStyles,
    style: &CellStyle,
    fonts: &std::cell::RefCell<FontInterner>,
    links: &std::cell::RefCell<LinkInterner>,
) -> Vec<crate::box_tree::InlineRun> {
    let env = FlowEnv {
        stylesheet,
        computed,
        table_columns: &[],
        row_height: None,
        fonts,
        links,
    };
    let family = style
        .font_family
        .as_deref()
        .map(|name| env.fonts.borrow_mut().family(name));
    let bold = style.bold;
    let italic = style.italic.unwrap_or(false);
    let font = env.fonts.borrow_mut().spec(family, bold, italic);
    let ctx = FlowCtx {
        font_size: style.font_size.unwrap_or(11.0),
        bold,
        italic,
        family,
        font,
        underline: style.underline,
        line_through: style.line_through,
        color: style.color.unwrap_or(Color::BLACK),
        align: style.align.unwrap_or(TextAlign::Left),
        base_rtl: style.direction.unwrap_or(false),
        link: 0,
        transform: style.text_transform.unwrap_or(TextTransform::None),
        letter_spacing: style.letter_spacing.unwrap_or(0.0),
        word_spacing: style.word_spacing.unwrap_or(0.0),
    };
    let mut acc = ChildAcc::default();
    collect_cell_runs_into(dom, cell_id, &env, ctx, &mut acc);
    acc.pending
}

fn collect_cell_runs_into(
    dom: &crate::dom::Dom,
    id: crate::dom::NodeId,
    env: &FlowEnv,
    ctx: FlowCtx,
    acc: &mut ChildAcc,
) {
    use crate::dom::NodeData;
    for &child in &dom.node(id).children {
        match &dom.node(child).data {
            NodeData::Text(text) => acc.push_text(text, &ctx),
            NodeData::Element { name, .. } => {
                let tag = name.as_str();
                if matches!(tag, "script" | "style" | "head" | "title" | "img")
                    || env.computed.hidden[child]
                {
                    continue;
                }
                if tag == "br" {
                    continue; // parity with the flat-text collector
                }
                let child_ctx = inline_ctx(&ctx, env, dom, child, tag);
                collect_cell_runs_into(dom, child, env, child_ctx, acc);
            }
            NodeData::Document => {}
        }
    }
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
    let root_env = HashMap::new();
    compute_inherited_node(
        dom,
        dom.root(),
        CellStyle::default(),
        false,
        stylesheet,
        &mut cache,
        &root_env,
        &mut out,
    );
    out
}

#[allow(clippy::too_many_arguments)]
fn compute_inherited_node(
    dom: &crate::dom::Dom,
    id: crate::dom::NodeId,
    inherited: CellStyle,
    parent_hidden: bool,
    stylesheet: &Stylesheet,
    cache: &mut HashMap<String, (CellStyle, bool)>,
    // Inherited custom-property environment (empty unless the stylesheet uses
    // custom properties). Threaded down so `var()` resolves against ancestors.
    env: &HashMap<String, String>,
    out: &mut ComputedStyles,
) {
    let node = dom.node(id);
    let is_element = matches!(&node.data, crate::dom::NodeData::Element { .. });
    // `own_env` is materialized only on the custom-property path; otherwise the
    // parent's `env` (empty) is threaded down unchanged and the fast cached
    // `element_own` runs exactly as before.
    let (style, hidden, own_env) = if is_element {
        if stylesheet.uses_custom {
            let (own, display_none, own_env) = element_own_with_env(dom, id, stylesheet, env);
            (
                inherit_style(&inherited, &own),
                parent_hidden || display_none,
                Some(own_env),
            )
        } else {
            let (own, display_none) = element_own(dom, id, stylesheet, cache);
            (inherit_style(&inherited, &own), parent_hidden || display_none, None)
        }
    } else {
        // Text and document nodes carry no cascade of their own; they inherit
        // their parent's style and hidden state.
        (inherited, parent_hidden, None)
    };
    out.style[id] = style.clone();
    out.hidden[id] = hidden;

    let child_env = own_env.as_ref().unwrap_or(env);
    for &child in &node.children {
        compute_inherited_node(
            dom,
            child,
            style.clone(),
            hidden,
            stylesheet,
            cache,
            child_env,
            out,
        );
    }
}

/// The custom-property slow path: like [`element_own`] but not cached, because
/// the result depends on the inherited custom-property environment. Builds this
/// element's environment (inherited + own `--x` declarations), resolves every
/// deferred `var()` value against it, and returns the environment for children.
fn element_own_with_env(
    dom: &crate::dom::Dom,
    id: crate::dom::NodeId,
    stylesheet: &Stylesheet,
    env: &HashMap<String, String>,
) -> (CellStyle, bool, HashMap<String, String>) {
    let node = dom.node(id);
    let tag = node.tag().unwrap_or_default();
    let class_attr = node.attr("class").unwrap_or_default();
    let inline_style = node.attr("style").unwrap_or_default();
    let classes = class_attr.split_whitespace().collect::<Vec<_>>();

    let mut decls = stylesheet.computed_declarations(dom, id, tag, &classes);
    if !inline_style.is_empty() {
        // Inline declarations (incl. their own custom/deferred) layer on top.
        decls.merge_inline(parse_style_declarations(inline_style));
    }

    // Environment = inherited, then this element's own `--x` (normal then
    // important), each resolved against the environment built so far.
    let mut own_env = env.clone();
    for (name, raw) in decls.normal.custom.iter().chain(decls.important.custom.iter()) {
        let resolved = substitute_vars(raw, &own_env, 0);
        own_env.insert(name.clone(), resolved);
    }

    // Typed (non-var) declarations resolve as usual; then deferred (var-bearing)
    // ones resolve against the environment and apply on top, normal before
    // important.
    let mut resolved = decls.resolved();
    let mut style = std::mem::take(&mut resolved.cell);
    let mut display = resolved.display;
    for (prop, raw) in decls.normal.deferred.iter().chain(decls.important.deferred.iter()) {
        let value = substitute_vars(raw, &own_env, 0);
        let value = value.trim();
        if value.is_empty() {
            continue;
        }
        let mut layer = DeclarationLayer::default();
        apply_style_declaration(&mut layer, prop, value);
        style.merge(layer.cell);
        display = layer.display.or(display);
    }

    (style, display == Some(CssDisplay::None), own_env)
}

/// Combine a parent's computed style with an element's own cascaded style.
fn inherit_style(parent: &CellStyle, own: &CellStyle) -> CellStyle {
    CellStyle {
        // Inheritable: the element's own value wins, else the parent's.
        align: own.align.or(parent.align),
        font_size: own.font_size.or(parent.font_size),
        font_family: own.font_family.clone().or_else(|| parent.font_family.clone()),
        italic: own.italic.or(parent.italic),
        direction: own.direction.or(parent.direction),
        line_height: own.line_height.or(parent.line_height),
        color: own.color.or(parent.color),
        white_space: own.white_space.or(parent.white_space),
        overflow_wrap: own.overflow_wrap.or(parent.overflow_wrap),
        word_break: own.word_break.or(parent.word_break),
        bold: own.bold || parent.bold,
        text_transform: own.text_transform.or(parent.text_transform),
        letter_spacing: own.letter_spacing.or(parent.letter_spacing),
        word_spacing: own.word_spacing.or(parent.word_spacing),
        text_indent: own.text_indent.or(parent.text_indent),
        text_indent_percent: own.text_indent_percent.or(parent.text_indent_percent),
        // Text decoration propagates to descendant inline content (see field docs).
        underline: own.underline || parent.underline,
        line_through: own.line_through || parent.line_through,
        decoration_none: own.decoration_none,
        // Non-inheritable: the element's own value only.
        vertical_align: own.vertical_align,
        border: own.border,
        border_sides: own.border_sides.clone(),
        border_box: own.border_box,
        overflow: own.overflow,
        width: own.width,
        width_percent: own.width_percent,
        height: own.height,
        max_width: own.max_width,
        max_width_percent: own.max_width_percent,
        margin_left_auto: own.margin_left_auto,
        margin_right_auto: own.margin_right_auto,
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
        flex_wrap: own.flex_wrap,
        display_grid: own.display_grid,
        grid_template: own.grid_template.clone(),
        row_gap: own.row_gap,
        grid_span: own.grid_span,
        grid_col_start: own.grid_col_start,
        grid_col_end: own.grid_col_end,
        float_dir: own.float_dir,
        clear: own.clear,
        position: own.position,
        z_index: own.z_index,
        offset_top: own.offset_top,
        offset_right: own.offset_right,
        offset_bottom: own.offset_bottom,
        offset_left: own.offset_left,
        sizing: own.sizing.clone(),
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

/// Parse a percentage value (`"55%"` → `55.0`); `None` for anything else.
fn parse_css_percent(value: &str) -> Option<f32> {
    value
        .trim()
        .strip_suffix('%')?
        .trim()
        .parse::<f32>()
        .ok()
        .filter(|n| *n >= 0.0)
}

fn parse_css_length(value: &str) -> Option<f32> {
    // A `calc()` reducing to a pure length yields its point value; one that
    // still carries a percentage can't be represented in a points-only context.
    if starts_with_calc(value.trim()) {
        return match parse_calc(value) {
            (Some(pt), None) => Some(pt),
            _ => None,
        };
    }
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

/// Split on whitespace outside parentheses, so a `calc(...)` term (which
/// contains spaces) stays a single component in a shorthand.
fn split_ws_top_level(value: &str) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut depth = 0i32;
    let mut start = None;
    for (i, c) in value.char_indices() {
        match c {
            '(' => depth += 1,
            ')' => depth = (depth - 1).max(0),
            _ if c.is_whitespace() && depth == 0 => {
                if let Some(s) = start.take() {
                    parts.push(&value[s..i]);
                }
                continue;
            }
            _ => {}
        }
        if start.is_none() {
            start = Some(i);
        }
    }
    if let Some(s) = start {
        parts.push(&value[s..]);
    }
    parts
}

/// Parse a `margin`/`padding` shorthand into `[top, right, bottom, left]` using
/// the CSS 1-to-4 value rule. Non-length tokens (e.g. `auto`) become `None`.
fn parse_box_edges(value: &str) -> [Option<f32>; 4] {
    let parts: Vec<Option<f32>> = split_ws_top_level(value).into_iter().map(parse_css_length).collect();
    match parts.as_slice() {
        [a] => [*a, *a, *a, *a],
        [a, b] => [*a, *b, *a, *b],
        [a, b, c] => [*a, *b, *c, *b],
        [a, b, c, d, ..] => [*a, *b, *c, *d],
        [] => [None; 4],
    }
}

/// Parse a value that may be a length, a percentage, or a `calc()` expression
/// into `(points, percent)`. A plain length/percent sets one component; a
/// `calc()` mixing them (e.g. `calc(100% - 20px)`) sets both, summed at layout
/// (`points + percent% × base`). Both `None` for `auto`/invalid.
fn parse_len_or_pct(value: &str) -> (Option<f32>, Option<f32>) {
    let value = value.trim();
    if starts_with_calc(value) {
        return parse_calc(value);
    }
    match parse_css_percent(value) {
        Some(pct) => (None, Some(pct)),
        None => (parse_css_length(value), None),
    }
}

fn starts_with_calc(value: &str) -> bool {
    let v = value.trim_start();
    v.len() >= 5 && v[..5].eq_ignore_ascii_case("calc(")
}

/// A `calc()` sub-value: either a unitless number or a length with independent
/// point and percentage components (`points + percent% × base`).
#[derive(Clone, Copy)]
enum CalcVal {
    Num(f32),
    Len { pt: f32, pct: f32 },
}

impl CalcVal {
    fn add(self, rhs: CalcVal, sign: f32) -> Option<CalcVal> {
        match (self, rhs) {
            (CalcVal::Num(a), CalcVal::Num(b)) => Some(CalcVal::Num(a + sign * b)),
            (CalcVal::Len { pt: a, pct: p }, CalcVal::Len { pt: b, pct: q }) => Some(CalcVal::Len {
                pt: a + sign * b,
                pct: p + sign * q,
            }),
            // Adding a bare number to a length is invalid in CSS `calc()`.
            _ => None,
        }
    }

    fn mul(self, rhs: CalcVal) -> Option<CalcVal> {
        match (self, rhs) {
            (CalcVal::Num(a), CalcVal::Num(b)) => Some(CalcVal::Num(a * b)),
            (CalcVal::Num(n), CalcVal::Len { pt, pct }) | (CalcVal::Len { pt, pct }, CalcVal::Num(n)) => {
                Some(CalcVal::Len { pt: pt * n, pct: pct * n })
            }
            // length × length is invalid.
            _ => None,
        }
    }

    fn div(self, rhs: CalcVal) -> Option<CalcVal> {
        match (self, rhs) {
            (_, CalcVal::Num(0.0)) => None,
            (CalcVal::Num(a), CalcVal::Num(b)) => Some(CalcVal::Num(a / b)),
            (CalcVal::Len { pt, pct }, CalcVal::Num(n)) => Some(CalcVal::Len { pt: pt / n, pct: pct / n }),
            // dividing by a length is invalid.
            _ => None,
        }
    }
}

/// Evaluate a `calc()` value into `(points, percent)`: each component is `Some`
/// when non-zero (or when it is the value's only component), so a pure length,
/// a pure percent, and a mix all round-trip through the additive resolver.
fn parse_calc(value: &str) -> (Option<f32>, Option<f32>) {
    let inner = match evaluate_calc(value) {
        Some(v) => v,
        None => return (None, None),
    };
    match inner {
        // A bare number is not a valid length; ignore.
        CalcVal::Num(_) => (None, None),
        CalcVal::Len { pt, pct } => {
            let pt_c = (pt != 0.0 || pct == 0.0).then_some(pt);
            let pct_c = (pct != 0.0).then_some(pct);
            (pt_c, pct_c)
        }
    }
}

/// Strip the outer `calc(...)`, normalize nested `calc(` to `(`, and evaluate.
fn evaluate_calc(value: &str) -> Option<CalcVal> {
    let value = value.trim();
    if !starts_with_calc(value) {
        return None;
    }
    // Take the balanced body of the outermost calc().
    let after = &value[4..]; // includes the leading '('
    let body = balanced_paren_body(after)?;
    // Nested calc() acts as a parenthesized sub-expression.
    let normalized = body.replace("calc(", "(").replace("CALC(", "(");
    let tokens = tokenize_calc(&normalized)?;
    let mut parser = CalcParser { tokens: &tokens, pos: 0 };
    let result = parser.expr()?;
    (parser.pos == parser.tokens.len()).then_some(result)
}

/// Given a string starting with `(`, return the substring inside the matching
/// close paren.
fn balanced_paren_body(s: &str) -> Option<&str> {
    let bytes = s.as_bytes();
    if bytes.first() != Some(&b'(') {
        return None;
    }
    let mut depth = 0i32;
    for (i, &b) in bytes.iter().enumerate() {
        match b {
            b'(' => depth += 1,
            b')' => {
                depth -= 1;
                if depth == 0 {
                    return Some(&s[1..i]);
                }
            }
            _ => {}
        }
    }
    None
}

#[derive(Clone, Copy)]
enum CalcTok {
    Num(CalcVal),
    Plus,
    Minus,
    Mul,
    Div,
    LParen,
    RParen,
}

fn tokenize_calc(s: &str) -> Option<Vec<CalcTok>> {
    let bytes = s.as_bytes();
    let mut toks = Vec::new();
    let mut i = 0;
    while i < s.len() {
        let c = bytes[i];
        if c.is_ascii_whitespace() {
            i += 1;
            continue;
        }
        match c {
            b'+' => toks.push(CalcTok::Plus),
            b'-' => toks.push(CalcTok::Minus),
            b'*' => toks.push(CalcTok::Mul),
            b'/' => toks.push(CalcTok::Div),
            b'(' => toks.push(CalcTok::LParen),
            b')' => toks.push(CalcTok::RParen),
            _ if c.is_ascii_digit() || c == b'.' => {
                let start = i;
                while i < s.len() && (bytes[i].is_ascii_digit() || bytes[i] == b'.') {
                    i += 1;
                }
                let num: f32 = s[start..i].parse().ok()?;
                let ustart = i;
                while i < s.len() && (bytes[i].is_ascii_alphabetic() || bytes[i] == b'%') {
                    i += 1;
                }
                toks.push(CalcTok::Num(calc_unit_value(num, &s[ustart..i])?));
                continue; // already advanced past the number+unit
            }
            _ => return None,
        }
        i += 1;
    }
    Some(toks)
}

fn calc_unit_value(num: f32, unit: &str) -> Option<CalcVal> {
    Some(match unit.to_ascii_lowercase().as_str() {
        "" => CalcVal::Num(num),
        "pt" => CalcVal::Len { pt: num, pct: 0.0 },
        "px" => CalcVal::Len { pt: num * 0.75, pct: 0.0 },
        "in" => CalcVal::Len { pt: num * 72.0, pct: 0.0 },
        "cm" => CalcVal::Len { pt: num * 72.0 / 2.54, pct: 0.0 },
        "mm" => CalcVal::Len { pt: num * 72.0 / 25.4, pct: 0.0 },
        "%" => CalcVal::Len { pt: 0.0, pct: num },
        _ => return None,
    })
}

/// A recursive-descent parser over `calc()` tokens:
/// `expr = term (('+'|'-') term)*`, `term = factor (('*'|'/') factor)*`,
/// `factor = ['+'|'-'] (number | '(' expr ')')`.
struct CalcParser<'a> {
    tokens: &'a [CalcTok],
    pos: usize,
}

impl CalcParser<'_> {
    fn peek(&self) -> Option<CalcTok> {
        self.tokens.get(self.pos).copied()
    }

    fn expr(&mut self) -> Option<CalcVal> {
        let mut acc = self.term()?;
        while let Some(tok) = self.peek() {
            let sign = match tok {
                CalcTok::Plus => 1.0,
                CalcTok::Minus => -1.0,
                _ => break,
            };
            self.pos += 1;
            let rhs = self.term()?;
            acc = acc.add(rhs, sign)?;
        }
        Some(acc)
    }

    fn term(&mut self) -> Option<CalcVal> {
        let mut acc = self.factor()?;
        while let Some(tok) = self.peek() {
            match tok {
                CalcTok::Mul => {
                    self.pos += 1;
                    acc = acc.mul(self.factor()?)?;
                }
                CalcTok::Div => {
                    self.pos += 1;
                    acc = acc.div(self.factor()?)?;
                }
                _ => break,
            }
        }
        Some(acc)
    }

    fn factor(&mut self) -> Option<CalcVal> {
        match self.peek()? {
            CalcTok::Plus => {
                self.pos += 1;
                self.factor()
            }
            CalcTok::Minus => {
                self.pos += 1;
                self.factor()?.mul(CalcVal::Num(-1.0))
            }
            CalcTok::LParen => {
                self.pos += 1;
                let inner = self.expr()?;
                match self.peek()? {
                    CalcTok::RParen => {
                        self.pos += 1;
                        Some(inner)
                    }
                    _ => None,
                }
            }
            CalcTok::Num(v) => {
                self.pos += 1;
                Some(v)
            }
            _ => None,
        }
    }
}

/// A signed CSS length in points (no percent) — letter-/word-spacing values.
fn parse_offset_signed(value: &str) -> Option<f32> {
    let value = value.trim();
    match value.strip_prefix('-') {
        Some(rest) => parse_css_length(rest).map(|v| -v),
        None => parse_css_length(value),
    }
}

/// Parse a box offset (`top`/`right`/`bottom`/`left`) that may be a signed
/// length, a signed percentage, or a `calc()` expression into `(points,
/// percent)`.
fn parse_offset_lp(value: &str) -> (Option<f32>, Option<f32>) {
    let value = value.trim();
    // `calc()` handles its own internal signs.
    if starts_with_calc(value) {
        return parse_calc(value);
    }
    let (sign, body) = match value.strip_prefix('-') {
        Some(rest) => (-1.0, rest),
        None => (1.0, value),
    };
    let (pt, pct) = parse_len_or_pct(body);
    (pt.map(|v| v * sign), pct.map(|v| v * sign))
}

/// Like [`parse_box_edges`] but keeps `%` sides separate from length sides, so a
/// shorthand mixing units (`padding: 5% 10px`) resolves each correctly. Returns
/// `([points; 4], [percent; 4])`, both `[top, right, bottom, left]`.
fn parse_box_edges_lp(value: &str) -> ([Option<f32>; 4], [Option<f32>; 4]) {
    let parts: Vec<(Option<f32>, Option<f32>)> =
        split_ws_top_level(value).into_iter().map(parse_len_or_pct).collect();
    let e = match parts.as_slice() {
        [a] => [*a, *a, *a, *a],
        [a, b] => [*a, *b, *a, *b],
        [a, b, c] => [*a, *b, *c, *b],
        [a, b, c, d, ..] => [*a, *b, *c, *d],
        [] => [(None, None); 4],
    };
    let mut pt = [None; 4];
    let mut pct = [None; 4];
    for i in 0..4 {
        pt[i] = e[i].0;
        pct[i] = e[i].1;
    }
    (pt, pct)
}

/// Substitute `var(--name[, fallback])` references in a raw declaration value
/// using the element's custom-property environment. An unknown name falls back
/// to its fallback (itself resolved) or, absent one, to empty. Bounded
/// recursion guards against custom-property reference cycles.
fn substitute_vars(value: &str, env: &std::collections::HashMap<String, String>, depth: u8) -> String {
    if depth > 16 || !value.contains("var(") {
        return value.to_string();
    }
    let bytes = value.as_bytes();
    let mut out = String::new();
    let mut i = 0;
    while i < value.len() {
        let at_boundary = i == 0 || {
            let prev = bytes[i - 1];
            !prev.is_ascii_alphanumeric() && prev != b'-' && prev != b'_'
        };
        if at_boundary && value[i..].starts_with("var(") {
            // Find the matching close paren for this `var(`.
            let mut paren = 1i32;
            let mut j = i + 4;
            while j < value.len() && paren > 0 {
                match bytes[j] {
                    b'(' => paren += 1,
                    b')' => paren -= 1,
                    _ => {}
                }
                j += 1;
            }
            if paren != 0 {
                // Unbalanced: emit the rest verbatim and stop.
                out.push_str(&value[i..]);
                break;
            }
            let args = &value[i + 4..j - 1];
            let (name, fallback) = split_first_top_comma(args);
            let replacement = match env.get(name.trim()) {
                Some(v) => substitute_vars(v, env, depth + 1),
                None => fallback
                    .map(|fb| substitute_vars(fb.trim(), env, depth + 1))
                    .unwrap_or_default(),
            };
            out.push_str(&replacement);
            i = j;
        } else {
            let ch = value[i..].chars().next().unwrap();
            out.push(ch);
            i += ch.len_utf8();
        }
    }
    out
}

/// Split at the first top-level comma (outside parens and quotes): `var()`'s
/// `name, fallback` split, where the fallback may itself contain commas.
fn split_first_top_comma(s: &str) -> (&str, Option<&str>) {
    let mut depth = 0i32;
    let mut quote: Option<u8> = None;
    for (i, &b) in s.as_bytes().iter().enumerate() {
        match (quote, b) {
            (Some(q), _) if b == q => quote = None,
            (Some(_), _) => {}
            (None, b'"' | b'\'') => quote = Some(b),
            (None, b'(') => depth += 1,
            (None, b')') => depth -= 1,
            (None, b',') if depth == 0 => return (&s[..i], Some(&s[i + 1..])),
            _ => {}
        }
    }
    (s, None)
}

/// Which boxed `SizingCss` percent array a declaration writes to.
#[derive(Clone, Copy)]
enum PctGroup {
    Padding,
    Margin,
    Offset,
}

/// Write a percent value into one side of a boxed sizing array (index order
/// `[top, right, bottom, left]`), allocating the `SizingCss` box only when a
/// `%` is actually present — so plain length declarations never grow a cell's
/// footprint. A `None` on a side that has a box clears any earlier `%` in the
/// same cascade layer (last-declaration-wins).
fn set_pct_slot(cell: &mut CellStyle, group: PctGroup, index: usize, pct: Option<f32>) {
    fn arr(s: &mut SizingCss, group: PctGroup) -> &mut [Option<f32>; 4] {
        match group {
            PctGroup::Padding => &mut s.padding_percent,
            PctGroup::Margin => &mut s.margin_percent,
            PctGroup::Offset => &mut s.offset_percent,
        }
    }
    if pct.is_some() {
        arr(cell.sizing.get_or_insert_with(Default::default), group)[index] = pct;
    } else if let Some(s) = cell.sizing.as_deref_mut() {
        arr(s, group)[index] = None;
    }
}

#[derive(Debug, Default)]
struct Stylesheet {
    rules: Vec<StyleRule>,
    /// `@font-face` rules in source order, carried onto the `Document` and
    /// loaded once per render.
    font_faces: Vec<crate::font::FontFaceRule>,
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
    /// Whether any rule declares a custom property (`--x`) or references one
    /// (`var()`). When false (the overwhelming majority, incl. the spreadsheet
    /// fixtures), the cascade takes its cached fast path unchanged; when true,
    /// the top-down pass resolves the custom-property environment per element.
    uses_custom: bool,
    /// `::before`/`::after` rules, kept out of the element cascade: they style
    /// generated content, matched per element at box-build time. Rare, so they
    /// are scanned linearly, gated by `has_pseudo`.
    pseudo_rules: Vec<StyleRule>,
    has_pseudo: bool,
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
    /// `::before`/`::after` on the subject: the rule styles *generated
    /// content* of the matched element rather than the element itself. Such
    /// rules live in `Stylesheet::pseudo_rules`, never in the element cascade.
    pseudo_element: Option<PseudoElement>,
}

/// A generated-content pseudo-element (`::before` / `::after`; the legacy
/// single-colon forms parse too).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PseudoElement {
    Before,
    After,
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
    /// Custom-property declarations (`--name: value`) in cascade order (lowest
    /// priority first). Resolved to the element's custom-property environment
    /// during the top-down pass; empty for the overwhelmingly common stylesheet
    /// that declares none, so the fast cascade path is untouched.
    custom: Vec<(String, String)>,
    /// Declarations whose value contains `var()` and so cannot be parsed until
    /// the custom-property environment is known — `(property, raw value)` in
    /// cascade order. Resolved and applied in the top-down pass.
    deferred: Vec<(String, String)>,
    /// The raw `content` value — meaningful only on `::before`/`::after`
    /// rules, consumed when generated content is built.
    content: Option<String>,
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
        self.uses_custom = self.rules.iter().any(|rule| {
            let layer = |l: &DeclarationLayer| !l.custom.is_empty() || !l.deferred.is_empty();
            layer(&rule.declarations.normal) || layer(&rule.declarations.important)
        });

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

impl Stylesheet {
    /// The merged declarations of every `::before`/`::after` rule matching
    /// element `id`, in cascade order — `None` when nothing matches. Linear
    /// over the (rare) pseudo rules; callers gate on `has_pseudo`.
    fn pseudo_declarations(
        &self,
        dom: &crate::dom::Dom,
        id: crate::dom::NodeId,
        which: PseudoElement,
    ) -> Option<DeclarationLayer> {
        let mut matched: Vec<&StyleRule> = self
            .pseudo_rules
            .iter()
            .filter(|rule| {
                rule.selector.pseudo_element == Some(which) && rule.selector.matches(dom, id)
            })
            .collect();
        if matched.is_empty() {
            return None;
        }
        matched.sort_by_key(|rule| (rule.specificity, rule.order));
        let mut declarations = StyleDeclarations::default();
        for rule in matched {
            declarations.merge(rule.declarations.clone());
        }
        Some(declarations.resolved())
    }
}

/// Resolve a `content` value into the text to generate: quoted strings (with
/// `\` escapes) concatenate, `attr(name)` reads the originating element's
/// attribute, and `none`/`normal` (or anything unsupported, e.g. `counter()`)
/// generates nothing.
fn parse_content_value(
    raw: &str,
    dom: &crate::dom::Dom,
    id: crate::dom::NodeId,
) -> Option<String> {
    let raw = raw.trim();
    if raw.is_empty()
        || raw.eq_ignore_ascii_case("none")
        || raw.eq_ignore_ascii_case("normal")
    {
        return None;
    }
    let mut out = String::new();
    let mut produced = false;
    let bytes = raw.as_bytes();
    let mut i = 0;
    while i < raw.len() {
        let c = bytes[i];
        if c == b'"' || c == b'\'' {
            let quote = c;
            i += 1;
            let mut closed = false;
            while i < raw.len() {
                let b = bytes[i];
                if b == b'\\' && i + 1 < raw.len() {
                    // CSS escape: 1-6 hex digits (optionally followed by one
                    // whitespace terminator) name a code point; anything else
                    // escapes the character itself (`\"`, `\\`).
                    let hex_len = raw[i + 1..]
                        .bytes()
                        .take(6)
                        .take_while(|b| b.is_ascii_hexdigit())
                        .count();
                    if hex_len > 0 {
                        let code = u32::from_str_radix(&raw[i + 1..i + 1 + hex_len], 16).ok();
                        if let Some(ch) = code.and_then(char::from_u32) {
                            out.push(ch);
                        }
                        i += 1 + hex_len;
                        // Consume the single whitespace that terminates the escape.
                        if bytes.get(i).is_some_and(|b| b.is_ascii_whitespace()) {
                            i += 1;
                        }
                        continue;
                    }
                    out.push(bytes[i + 1] as char);
                    i += 2;
                    continue;
                }
                if b == quote {
                    i += 1;
                    closed = true;
                    break;
                }
                // Multi-byte UTF-8: copy the whole char.
                let ch = raw[i..].chars().next().expect("in-bounds char");
                out.push(ch);
                i += ch.len_utf8();
            }
            if !closed {
                return None; // unterminated string: invalid value
            }
            produced = true;
        } else if c.is_ascii_whitespace() {
            i += 1;
        } else if raw[i..].len() >= 5 && raw[i..i + 5].eq_ignore_ascii_case("attr(") {
            let close = raw[i..].find(')')? + i;
            let name = raw[i + 5..close].trim();
            if let Some(value) = dom.node(id).attr(name) {
                out.push_str(value);
            }
            produced = true;
            i = close + 1;
        } else {
            // Unsupported component (counter(), url(), open-quote, …): treat
            // the whole value as unsupported rather than emit partial text.
            return None;
        }
    }
    produced.then_some(out)
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
        // A pseudo-element counts like a type selector.
        if self.pseudo_element.is_some() {
            spec.elements += 1;
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
    fn merge(&mut self, mut other: DeclarationLayer) {
        self.cell.merge(other.cell);
        self.display = other.display.or(self.display);
        // Higher-priority (later-merged) custom/deferred entries append after,
        // so they apply last and win when the environment is built/resolved.
        self.custom.append(&mut other.custom);
        self.deferred.append(&mut other.deferred);
        self.content = other.content.or(self.content.take());
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
    let mut rule_parser = RuleParser::default();
    let mut rules = StyleSheetParser::new(&mut parser, &mut rule_parser);

    while let Some(result) = rules.next() {
        let Ok(parsed) = result else { continue };
        for (selector, declarations) in parsed {
            let rule = StyleRule {
                specificity: selector.specificity(),
                selector,
                declarations,
                order,
            };
            // Generated-content rules must not style the element itself.
            if rule.selector.pseudo_element.is_some() {
                stylesheet.pseudo_rules.push(rule);
            } else {
                stylesheet.rules.push(rule);
            }
            order += 1;
        }
    }

    stylesheet.font_faces = std::mem::take(&mut rule_parser.font_faces);
    stylesheet.has_pseudo = !stylesheet.pseudo_rules.is_empty();
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

#[derive(Default)]
struct RuleParser {
    /// `@font-face` rules collected while parsing (including inside an
    /// applicable `@media` block), in source order.
    font_faces: Vec<crate::font::FontFaceRule>,
}

enum AtRuleKind {
    /// `@media`: parse the nested block as top-level rules if the query applies
    /// to print output (the PDF target); the boolean is that decision.
    Media(bool),
    /// `@page`: parsed by the geometry parser for margins and orientation.
    Page,
    /// `@font-face`: descriptors parsed into a `FontFaceRule`.
    FontFace,
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
        } else if name.eq_ignore_ascii_case("font-face") {
            Ok(AtRuleKind::FontFace)
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
                let mut inner = RuleParser::default();
                let mut rules = StyleSheetParser::new(input, &mut inner);
                let mut collected = Vec::new();
                while let Some(result) = rules.next() {
                    if let Ok(mut parsed) = result {
                        collected.append(&mut parsed);
                    }
                }
                self.font_faces.append(&mut inner.font_faces);
                Ok(collected)
            }
            AtRuleKind::FontFace => {
                if let Some(rule) = parse_font_face_block(input) {
                    self.font_faces.push(rule);
                }
                Ok(Vec::new())
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

        // Custom-property names are case-sensitive and keep their `--` prefix;
        // ordinary property names are ASCII-lowercased.
        let is_custom = name.starts_with("--");
        let property = if is_custom {
            name.to_string()
        } else {
            name.to_ascii_lowercase()
        };
        let (value, important) = normalize_declaration_value(raw_value);
        let layer = if important {
            &mut self.declarations.important
        } else {
            &mut self.declarations.normal
        };

        if is_custom {
            // A custom property carries an arbitrary token stream; store it raw
            // for later resolution into the environment.
            layer.custom.push((property, value));
        } else if value.contains("var(") {
            // Defer any value that references a custom property: it can only be
            // parsed once the environment is known (top-down pass).
            layer.deferred.push((property, value));
        } else {
            apply_style_declaration(layer, &property, &value);
        }
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

/// Captures a block's declarations as raw `(name, value)` strings. Used for
/// `@font-face`, whose descriptor values (`src:` URLs, family names) must not
/// go through the cascade's value normalization.
struct RawDeclParser {
    entries: Vec<(String, String)>,
}

impl<'i> DeclarationParser<'i> for RawDeclParser {
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
        self.entries
            .push((name.to_ascii_lowercase(), raw_value.trim().to_string()));
        Ok(())
    }
}

impl<'i> AtRuleParser<'i> for RawDeclParser {
    type Prelude = ();
    type AtRule = ();
    type Error = ();
}

impl<'i> QualifiedRuleParser<'i> for RawDeclParser {
    type Prelude = ();
    type QualifiedRule = ();
    type Error = ();
}

impl<'i> RuleBodyItemParser<'i, (), ()> for RawDeclParser {
    fn parse_declarations(&self) -> bool {
        true
    }

    fn parse_qualified(&self) -> bool {
        false
    }
}

/// Parse a `@font-face` block into a rule: `font-family` and a non-empty `src:`
/// are required; `font-weight`/`font-style` descriptors pick this face among
/// same-family rules (bold = keyword or numeric ≥ 600, italic = italic/oblique).
fn parse_font_face_block(input: &mut Parser<'_, '_>) -> Option<crate::font::FontFaceRule> {
    let mut decl_parser = RawDeclParser { entries: Vec::new() };
    let mut items = RuleBodyParser::new(input, &mut decl_parser);
    while let Some(result) = items.next() {
        let _ = result;
    }

    let mut family = None;
    let mut sources = Vec::new();
    let mut bold = false;
    let mut italic = false;
    for (name, value) in &decl_parser.entries {
        match name.as_str() {
            "font-family" => {
                let name = strip_css_quotes(value);
                if !name.is_empty() {
                    family = Some(name.to_string());
                }
            }
            "src" => sources = parse_font_face_src(value),
            "font-weight" => {
                let first = value.split_whitespace().next().unwrap_or("");
                bold = first.eq_ignore_ascii_case("bold")
                    || first.eq_ignore_ascii_case("bolder")
                    || first.parse::<f32>().is_ok_and(|weight| weight >= 600.0);
            }
            "font-style" => {
                let value = value.to_ascii_lowercase();
                italic = value.contains("italic") || value.contains("oblique");
            }
            _ => {}
        }
    }
    let family = family?;
    if sources.is_empty() {
        return None;
    }
    Some(crate::font::FontFaceRule { family, sources, bold, italic })
}

/// Parse a `@font-face` `src:` list: comma-separated `local(<family>)` or
/// `url(<target>)`, the latter with an optional trailing `format(<hint>)`.
/// Unrecognized items are skipped (the list is a fallback chain by design).
fn parse_font_face_src(value: &str) -> Vec<crate::font::FontFaceSource> {
    let mut sources = Vec::new();
    for item in split_top_level_commas(value) {
        let item = item.trim();
        if let Some((arg, _)) = split_css_function(item, "local") {
            let name = strip_css_quotes(arg);
            if !name.is_empty() {
                sources.push(crate::font::FontFaceSource::Local(name.to_string()));
            }
        } else if let Some((arg, rest)) = split_css_function(item, "url") {
            let url = strip_css_quotes(arg).to_string();
            if url.is_empty() {
                continue;
            }
            let format = split_css_function(rest.trim_start(), "format")
                .map(|(hint, _)| strip_css_quotes(hint).to_ascii_lowercase());
            sources.push(crate::font::FontFaceSource::Url { url, format });
        }
    }
    sources
}

/// If `s` starts with `name(`, return the argument up to the first `)` and the
/// remainder after it. Sufficient for `url()`/`local()`/`format()` arguments,
/// which never contain parentheses (base64 `data:` URIs included).
fn split_css_function<'a>(s: &'a str, name: &str) -> Option<(&'a str, &'a str)> {
    if s.len() < name.len() || !s[..name.len()].eq_ignore_ascii_case(name) {
        return None;
    }
    let after = s[name.len()..].trim_start().strip_prefix('(')?;
    let close = after.find(')')?;
    Some((&after[..close], &after[close + 1..]))
}

/// Split on commas outside parentheses and quotes: a base64 `data:` URI inside
/// `url(...)` carries a comma of its own.
fn split_top_level_commas(s: &str) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut depth = 0usize;
    let mut quote: Option<char> = None;
    let mut start = 0;
    for (i, c) in s.char_indices() {
        match (quote, c) {
            (Some(q), _) if c == q => quote = None,
            (Some(_), _) => {}
            (None, '"' | '\'') => quote = Some(c),
            (None, '(') => depth += 1,
            (None, ')') => depth = depth.saturating_sub(1),
            (None, ',') if depth == 0 => {
                parts.push(&s[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    parts.push(&s[start..]);
    parts
}

/// Trim surrounding whitespace and one layer of matching CSS quotes.
fn strip_css_quotes(s: &str) -> &str {
    let s = s.trim();
    if s.len() >= 2
        && ((s.starts_with('"') && s.ends_with('"'))
            || (s.starts_with('\'') && s.ends_with('\'')))
    {
        &s[1..s.len() - 1]
    } else {
        s
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
            // `::` (a pseudo-element) arrives as two Colon tokens.
            && !(current.colon_count == 1 && matches!(token, Token::Colon))
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
    /// `::before`/`::after` seen on the compound being built. Valid only on
    /// the subject (a following combinator rejects the selector).
    pseudo_element: Option<PseudoElement>,
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
        // A pseudo-element is only valid on the subject (rightmost) compound.
        if self.pending.is_some() && self.pseudo_element.is_some() {
            self.reject();
        }
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

    /// A pseudo identifier following `:`/`::`. `before`/`after` become the
    /// selector's pseudo-element (both colon forms, per CSS legacy); other
    /// names are structural pseudo-classes (single colon only) or reject the
    /// selector (dynamic pseudo-classes, unknown pseudo-elements).
    fn finish_pseudo_ident(&mut self, name: &str) {
        let single_colon = self.colon_count == 1;
        self.colon_count = 0;
        match name.to_ascii_lowercase().as_str() {
            "before" if self.pseudo_element.is_none() => {
                self.pseudo_element = Some(PseudoElement::Before);
                return;
            }
            "after" if self.pseudo_element.is_none() => {
                self.pseudo_element = Some(PseudoElement::After);
                return;
            }
            _ => {}
        }
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
            pseudo_element: self.pseudo_element,
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
        if let Some(inner) = token
            .strip_prefix("minmax(")
            .and_then(|rest| rest.strip_suffix(')'))
        {
            let (min, max) = inner.split_once(',')?;
            let min = match min.trim() {
                m if m.eq_ignore_ascii_case("auto") || m.eq_ignore_ascii_case("min-content") => {
                    MinTrack::Auto
                }
                m => MinTrack::Pt(parse_css_length(m)?),
            };
            let max = match max.trim() {
                m if m.eq_ignore_ascii_case("auto") || m.eq_ignore_ascii_case("max-content") => {
                    MaxTrack::Auto
                }
                m => match m.strip_suffix("fr") {
                    Some(fr) => MaxTrack::Fr(fr.trim().parse::<f32>().ok()?),
                    None => MaxTrack::Pt(parse_css_length(m)?),
                },
            };
            return Some(GridTrack::MinMax(min, max));
        }
        if let Some(fr) = token.strip_suffix("fr") {
            return fr.trim().parse::<f32>().ok().map(GridTrack::Fr);
        }
        parse_css_length(token).map(GridTrack::Pt)
    }

    // Split into function-aware tokens: whitespace separates tracks, but
    // whitespace inside `repeat(...)` / `minmax(...)` stays in the token.
    fn split_tokens(value: &str) -> Vec<&str> {
        let mut out = Vec::new();
        let (mut depth, mut start) = (0usize, None::<usize>);
        for (index, ch) in value.char_indices() {
            match ch {
                '(' => depth += 1,
                ')' => depth = depth.saturating_sub(1),
                c if c.is_whitespace() && depth == 0 => {
                    if let Some(s) = start.take() {
                        out.push(&value[s..index]);
                    }
                    continue;
                }
                _ => {}
            }
            start.get_or_insert(index);
        }
        if let Some(s) = start {
            out.push(&value[s..]);
        }
        out
    }

    let mut out = Vec::new();
    for token in split_tokens(value.trim()) {
        if let Some(inner) = token
            .strip_prefix("repeat(")
            .and_then(|rest| rest.strip_suffix(')'))
        {
            if let Some((count, tracks)) = inner.split_once(',') {
                if let Ok(count) = count.trim().parse::<usize>() {
                    let unit: Vec<GridTrack> =
                        split_tokens(tracks).into_iter().filter_map(parse_token).collect();
                    for _ in 0..count.min(100) {
                        out.extend(unit.iter().copied());
                    }
                }
            }
        } else if let Some(track) = parse_token(token) {
            out.push(track);
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
        "flex-wrap" => {
            let v = value.trim().to_ascii_lowercase();
            target.cell.flex_wrap = Some(v == "wrap" || v == "wrap-reverse");
        }
        "flex-flow" => {
            // Shorthand: any order of a direction keyword and a wrap keyword.
            let v = value.to_ascii_lowercase();
            for word in v.split_whitespace() {
                match word {
                    "column" | "column-reverse" => {
                        target.cell.flex_direction = Some(FlexDirection::Column)
                    }
                    "row" | "row-reverse" => target.cell.flex_direction = Some(FlexDirection::Row),
                    "wrap" | "wrap-reverse" => target.cell.flex_wrap = Some(true),
                    "nowrap" => target.cell.flex_wrap = Some(false),
                    _ => {}
                }
            }
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
        "top" => {
            let (pt, pct) = parse_offset_lp(value);
            target.cell.offset_top = pt;
            set_pct_slot(&mut target.cell, PctGroup::Offset, 0, pct);
        }
        "right" if parse_offset_lp(value) != (None, None) => {
            let (pt, pct) = parse_offset_lp(value);
            target.cell.offset_right = pt;
            set_pct_slot(&mut target.cell, PctGroup::Offset, 1, pct);
        }
        "bottom" => {
            let (pt, pct) = parse_offset_lp(value);
            target.cell.offset_bottom = pt;
            set_pct_slot(&mut target.cell, PctGroup::Offset, 2, pct);
        }
        "left" => {
            let (pt, pct) = parse_offset_lp(value);
            target.cell.offset_left = pt;
            set_pct_slot(&mut target.cell, PctGroup::Offset, 3, pct);
        }
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
            // `span N`, `A`, `A / B`, `A / span N`, `A / -1` (negative lines
            // count from the end). Named lines and `dense` are not supported.
            let v = value.trim().to_ascii_lowercase();
            let (start, end) = match v.split_once('/') {
                Some((a, b)) => (a.trim(), Some(b.trim())),
                None => (v.as_str(), None),
            };
            if let Some(rest) = start.strip_prefix("span") {
                if let Ok(n) = rest.trim().parse::<usize>() {
                    target.cell.grid_span = Some(n.max(1));
                }
            } else if let Ok(line) = start.parse::<i32>() {
                target.cell.grid_col_start = Some(line);
            }
            if let Some(end) = end {
                if let Some(rest) = end.strip_prefix("span") {
                    if let Ok(n) = rest.trim().parse::<usize>() {
                        target.cell.grid_span = Some(n.max(1));
                    }
                } else if let Ok(line) = end.parse::<i32>() {
                    target.cell.grid_col_end = Some(line);
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
            // `start`/`end` assume left-to-right text.
            target.cell.align = match value.to_ascii_lowercase().as_str() {
                "right" | "end" => Some(TextAlign::Right),
                "center" => Some(TextAlign::Center),
                "justify" => Some(TextAlign::Justify),
                "left" | "start" => Some(TextAlign::Left),
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
                target.cell.decoration_none = true;
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
        "direction" => {
            target.cell.direction = match value.trim().to_ascii_lowercase().as_str() {
                "rtl" => Some(true),
                "ltr" => Some(false),
                _ => target.cell.direction,
            };
        }
        "line-height" => target.cell.line_height = parse_line_height(value),
        "content" => target.content = Some(value.to_string()),
        "text-transform" => {
            target.cell.text_transform = match value.trim().to_ascii_lowercase().as_str() {
                "uppercase" => Some(TextTransform::Uppercase),
                "lowercase" => Some(TextTransform::Lowercase),
                "capitalize" => Some(TextTransform::Capitalize),
                "none" => Some(TextTransform::None),
                _ => target.cell.text_transform,
            };
        }
        "letter-spacing" => {
            // `normal` is an explicit 0 so it overrides an inherited spacing;
            // negative lengths tighten tracking.
            target.cell.letter_spacing = if value.trim().eq_ignore_ascii_case("normal") {
                Some(0.0)
            } else {
                parse_offset_signed(value)
            };
        }
        "word-spacing" => {
            target.cell.word_spacing = if value.trim().eq_ignore_ascii_case("normal") {
                Some(0.0)
            } else {
                parse_offset_signed(value)
            };
        }
        "text-indent" => {
            let (pt, pct) = parse_offset_lp(value);
            target.cell.text_indent = pt;
            target.cell.text_indent_percent = pct;
        }
        "width" => {
            // `calc()` may set both a point and a percent component (summed at
            // layout); a plain length or percent sets one.
            let (pt, pct) = parse_len_or_pct(value);
            target.cell.width = pt;
            target.cell.width_percent = pct;
        }
        "max-width" => {
            let (pt, pct) = parse_len_or_pct(value);
            target.cell.max_width = pt;
            target.cell.max_width_percent = pct;
        }
        "min-width" => {
            let (pt, pct) = parse_len_or_pct(value);
            let s = target.cell.sizing.get_or_insert_with(Default::default);
            s.min_width = pt;
            s.min_width_percent = pct;
        }
        "min-height" => {
            // `%` needs a definite containing height (indefinite in flow); points
            // only for now.
            if let Some(pt) = parse_css_length(value) {
                target.cell.sizing.get_or_insert_with(Default::default).min_height = Some(pt);
            }
        }
        "max-height" => {
            if let Some(pt) = parse_css_length(value) {
                target.cell.sizing.get_or_insert_with(Default::default).max_height = Some(pt);
            }
        }
        "height" => target.cell.height = parse_css_length(value),
        "color" => target.cell.color = parse_css_color(value),
        "background-color" => target.cell.background_color = parse_css_color(value),
        "background" => target.cell.background_color = parse_css_background_color(value),
        "padding-top" => {
            let (pt, pct) = parse_len_or_pct(value);
            target.cell.padding_top = pt;
            set_pct_slot(&mut target.cell, PctGroup::Padding, 0, pct);
        }
        "padding-right" => {
            let (pt, pct) = parse_len_or_pct(value);
            target.cell.padding_right = pt;
            set_pct_slot(&mut target.cell, PctGroup::Padding, 1, pct);
        }
        "padding-bottom" => {
            let (pt, pct) = parse_len_or_pct(value);
            target.cell.padding_bottom = pt;
            set_pct_slot(&mut target.cell, PctGroup::Padding, 2, pct);
        }
        "padding-left" => {
            let (pt, pct) = parse_len_or_pct(value);
            target.cell.padding_left = pt;
            set_pct_slot(&mut target.cell, PctGroup::Padding, 3, pct);
        }
        "padding" => {
            let (pt, pct) = parse_box_edges_lp(value);
            target.cell.padding_top = pt[0];
            target.cell.padding_right = pt[1];
            target.cell.padding_bottom = pt[2];
            target.cell.padding_left = pt[3];
            for (i, &p) in pct.iter().enumerate() {
                set_pct_slot(&mut target.cell, PctGroup::Padding, i, p);
            }
        }
        "margin-left" if value.trim().eq_ignore_ascii_case("auto") => {
            target.cell.margin_left_auto = true;
        }
        "margin-left" => {
            let (pt, pct) = parse_len_or_pct(value);
            target.cell.margin_left = pt;
            set_pct_slot(&mut target.cell, PctGroup::Margin, 3, pct);
        }
        "margin-right" if value.trim().eq_ignore_ascii_case("auto") => {
            target.cell.margin_right_auto = true;
        }
        "margin-right" => {
            let (pt, pct) = parse_len_or_pct(value);
            target.cell.margin_right = pt;
            set_pct_slot(&mut target.cell, PctGroup::Margin, 1, pct);
        }
        "margin-top" => {
            let (pt, pct) = parse_len_or_pct(value);
            target.cell.margin_top = pt;
            set_pct_slot(&mut target.cell, PctGroup::Margin, 0, pct);
        }
        "margin-bottom" => {
            let (pt, pct) = parse_len_or_pct(value);
            target.cell.margin_bottom = pt;
            set_pct_slot(&mut target.cell, PctGroup::Margin, 2, pct);
        }
        "margin" => {
            let (ptv, pctv) = parse_box_edges_lp(value);
            target.cell.margin_top = ptv[0];
            target.cell.margin_right = ptv[1];
            target.cell.margin_bottom = ptv[2];
            target.cell.margin_left = ptv[3];
            for (i, &p) in pctv.iter().enumerate() {
                set_pct_slot(&mut target.cell, PctGroup::Margin, i, p);
            }
            // `margin: 0 auto` (and friends): detect `auto` in the expanded
            // right/left slots of the 1-to-4 shorthand.
            let parts: Vec<&str> = value.split_whitespace().collect();
            let (right_auto, left_auto) = match parts.as_slice() {
                [a] => (a.eq_ignore_ascii_case("auto"), a.eq_ignore_ascii_case("auto")),
                [_, b] | [_, b, _] => {
                    (b.eq_ignore_ascii_case("auto"), b.eq_ignore_ascii_case("auto"))
                }
                [_, b, _, d, ..] => (b.eq_ignore_ascii_case("auto"), d.eq_ignore_ascii_case("auto")),
                [] => (false, false),
            };
            target.cell.margin_right_auto |= right_auto;
            target.cell.margin_left_auto |= left_auto;
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
        "border" => {
            // The shorthand resets all sub-properties on every side (one
            // declaration → whole-side assignment, not a merge). The legacy
            // `border` summary flag is kept in sync for the heuristics that
            // read it (caption rows, empty decorated blocks, cell fast path).
            let side = parse_border_shorthand(value);
            let sides = border_sides_mut(&mut target.cell);
            sides.top = side;
            sides.right = side;
            sides.bottom = side;
            sides.left = side;
            target.cell.border = Some(border_side_visible(side));
        }
        "border-top" | "border-right" | "border-bottom" | "border-left" => {
            let side = parse_border_shorthand(value);
            *border_side_mut(&mut target.cell, &property[7..]) = side;
            // Matches the legacy summary latch: a per-side declaration flips
            // the whole-box summary (per-side truth lives in the side fields).
            target.cell.border = Some(border_side_visible(side));
        }
        "border-width" | "border-style" | "border-color" => {
            let values = expand_box_values(value);
            for (index, name) in ["top", "right", "bottom", "left"].iter().enumerate() {
                let Some(token) = values.get(index) else { continue };
                let side = border_side_mut(&mut target.cell, name);
                match &property[7..] {
                    "width" => side.width = parse_border_width_token(token),
                    "style" => side.style = parse_border_style_keyword(token),
                    _ => side.color = parse_css_color(token),
                }
            }
            if property == "border-style" {
                let sides = border_sides_mut(&mut target.cell);
                let any_visible = [sides.top, sides.right, sides.bottom, sides.left]
                    .iter()
                    .any(|side| border_side_visible(*side));
                target.cell.border = Some(any_visible);
            }
        }
        "border-top-width" | "border-right-width" | "border-bottom-width"
        | "border-left-width" => {
            let name = &property[7..property.len() - 6];
            border_side_mut(&mut target.cell, name).width = parse_border_width_token(value);
        }
        "border-top-style" | "border-right-style" | "border-bottom-style"
        | "border-left-style" => {
            let name = &property[7..property.len() - 6];
            let style = parse_border_style_keyword(value);
            border_side_mut(&mut target.cell, name).style = style;
            if style.is_some_and(|kind| kind != BorderStyle::None) {
                target.cell.border = Some(true);
            }
        }
        "border-top-color" | "border-right-color" | "border-bottom-color"
        | "border-left-color" => {
            let name = &property[7..property.len() - 6];
            border_side_mut(&mut target.cell, name).color = parse_css_color(value);
        }
        "border-radius" => {
            // Single uniform radius: the first length wins (per-corner and
            // elliptical `/` syntax collapse to it).
            let first = value.split(['/', ' ']).find(|token| !token.is_empty());
            if let Some(radius) = first.and_then(parse_css_length) {
                border_sides_mut(&mut target.cell).radius = Some(radius.max(0.0));
            }
        }
        "box-sizing" => {
            if value.eq_ignore_ascii_case("border-box") {
                target.cell.border_box = Some(true);
            } else if value.eq_ignore_ascii_case("content-box") {
                target.cell.border_box = Some(false);
            }
        }
        _ => {}
    }
}

/// Parse a `border`/`border-<side>` shorthand: any order of width, style, and
/// color tokens. `none`/`hidden` set the explicit `None` style.
fn parse_border_shorthand(value: &str) -> BorderSideCss {
    let mut side = BorderSideCss::default();
    for token in split_css_tokens(value) {
        if let Some(style) = parse_border_style_keyword(token) {
            side.style = Some(style);
        } else if let Some(width) = parse_border_width_token(token) {
            side.width = Some(width);
        } else if let Some(color) = parse_css_color(token) {
            side.color = Some(color);
        }
    }
    side
}

/// Whether a cascaded side would paint (mirrors [`resolved_borders`]'s rule,
/// including the width-without-style lenience).
fn border_side_visible(side: BorderSideCss) -> bool {
    match side.style {
        Some(BorderStyle::None) => false,
        Some(_) => side.width != Some(0.0),
        None => side.width.is_some_and(|width| width > 0.0),
    }
}

fn border_sides_mut(cell: &mut CellStyle) -> &mut BorderSides {
    cell.border_sides.get_or_insert_with(Default::default)
}

fn border_side_mut<'a>(cell: &'a mut CellStyle, name: &str) -> &'a mut BorderSideCss {
    let sides = border_sides_mut(cell);
    match name {
        "top" => &mut sides.top,
        "right" => &mut sides.right,
        "bottom" => &mut sides.bottom,
        _ => &mut sides.left,
    }
}

fn parse_border_style_keyword(value: &str) -> Option<BorderStyle> {
    let value = value.trim();
    Some(match value.to_ascii_lowercase().as_str() {
        "none" | "hidden" => BorderStyle::None,
        "solid" | "double" | "groove" | "ridge" | "inset" | "outset" => BorderStyle::Solid,
        "dashed" => BorderStyle::Dashed,
        "dotted" => BorderStyle::Dotted,
        _ => return None,
    })
}

/// A border width token: a length or the `thin`/`medium`/`thick` keywords
/// (1px / 3px / 5px, in points).
fn parse_border_width_token(value: &str) -> Option<f32> {
    match value.trim().to_ascii_lowercase().as_str() {
        "thin" => Some(0.75),
        "medium" => Some(2.25),
        "thick" => Some(3.75),
        other => parse_css_length(other),
    }
}

/// Split a declaration value on whitespace outside parentheses, so a color
/// like `rgb(1, 2, 3)` stays one token.
fn split_css_tokens(value: &str) -> Vec<&str> {
    let mut tokens = Vec::new();
    let mut depth = 0usize;
    let mut start = None;
    for (i, c) in value.char_indices() {
        match c {
            '(' => depth += 1,
            ')' => depth = depth.saturating_sub(1),
            _ if c.is_whitespace() && depth == 0 => {
                if let Some(s) = start.take() {
                    tokens.push(&value[s..i]);
                }
                continue;
            }
            _ => {}
        }
        if start.is_none() {
            start = Some(i);
        }
    }
    if let Some(s) = start {
        tokens.push(&value[s..]);
    }
    tokens
}

/// Expand a 1-to-4-value box property (like `border-width: 1px 2px`) into the
/// CSS top/right/bottom/left order.
fn expand_box_values(value: &str) -> Vec<String> {
    let tokens: Vec<&str> = split_css_tokens(value);
    let pick = |i: usize| -> String {
        match tokens.len() {
            0 => String::new(),
            1 => tokens[0].to_string(),
            2 => tokens[i % 2].to_string(),
            3 => tokens[[0, 1, 2, 1][i]].to_string(),
            _ => tokens[i].to_string(),
        }
    };
    (0..4).map(pick).collect()
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
        self.decoration_none |= other.decoration_none;
        self.border = other.border.or(self.border);
        // Border sides merge per sub-property: a higher-priority rule that only
        // sets `border-color` must not wipe a lower-priority `border-style`.
        self.border_sides = match (self.border_sides.take(), other.border_sides) {
            (Some(mut base), Some(over)) => {
                let side = |over: BorderSideCss, base: BorderSideCss| BorderSideCss {
                    width: over.width.or(base.width),
                    style: over.style.or(base.style),
                    color: over.color.or(base.color),
                };
                base.top = side(over.top, base.top);
                base.right = side(over.right, base.right);
                base.bottom = side(over.bottom, base.bottom);
                base.left = side(over.left, base.left);
                base.radius = over.radius.or(base.radius);
                Some(base)
            }
            (base, over) => over.or(base),
        };
        self.border_box = other.border_box.or(self.border_box);
        self.overflow = other.overflow.or(self.overflow);
        self.font_size = other.font_size.or(self.font_size);
        self.font_family = other.font_family.or(self.font_family.take());
        self.italic = other.italic.or(self.italic);
        self.direction = other.direction.or(self.direction);
        self.line_height = other.line_height.or(self.line_height);
        self.width = other.width.or(self.width);
        self.width_percent = other.width_percent.or(self.width_percent);
        self.height = other.height.or(self.height);
        self.max_width = other.max_width.or(self.max_width);
        self.max_width_percent = other.max_width_percent.or(self.max_width_percent);
        self.margin_left_auto |= other.margin_left_auto;
        self.margin_right_auto |= other.margin_right_auto;
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
        self.flex_wrap = other.flex_wrap.or(self.flex_wrap);
        self.display_grid |= other.display_grid;
        self.grid_template = other.grid_template.or(self.grid_template.take());
        self.row_gap = other.row_gap.or(self.row_gap);
        self.grid_span = other.grid_span.or(self.grid_span);
        self.grid_col_start = other.grid_col_start.or(self.grid_col_start);
        self.grid_col_end = other.grid_col_end.or(self.grid_col_end);
        self.float_dir = other.float_dir.or(self.float_dir);
        self.clear = other.clear.or(self.clear);
        self.position = other.position.or(self.position);
        self.z_index = other.z_index.or(self.z_index);
        self.offset_top = other.offset_top.or(self.offset_top);
        self.offset_right = other.offset_right.or(self.offset_right);
        self.offset_bottom = other.offset_bottom.or(self.offset_bottom);
        self.offset_left = other.offset_left.or(self.offset_left);
        self.text_transform = other.text_transform.or(self.text_transform);
        self.letter_spacing = other.letter_spacing.or(self.letter_spacing);
        self.word_spacing = other.word_spacing.or(self.word_spacing);
        self.text_indent = other.text_indent.or(self.text_indent);
        self.text_indent_percent = other.text_indent_percent.or(self.text_indent_percent);
        self.sizing = match (self.sizing.take(), other.sizing) {
            (Some(mut a), Some(b)) => {
                a.merge(*b);
                Some(a)
            }
            (a, b) => b.or(a),
        };
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
    fn anchor_hrefs_intern_links_and_get_ua_link_style() {
        let document = parse(
            "<p>go <a href=\"https://x.test/a\">there now</a> and \
             <a href=\"https://x.test/a\">again</a> or \
             <a href=\"#frag\" style=\"text-decoration: none\">quietly</a> or \
             <a href=\"https://x.test/b\" style=\"color: #ff0000\">redly</a></p>\
             <h2 id=\"frag\">Target</h2>",
        );
        // Duplicate hrefs intern once; document order is preserved.
        assert_eq!(
            document.links,
            vec!["https://x.test/a".to_string(), "#frag".to_string(), "https://x.test/b".to_string()]
        );

        let flow = document.flow.expect("flow tree");
        let blocks = flow_blocks(&flow);
        let runs = match &blocks[0].children[0] {
            BoxChild::Line(runs) => runs,
            _ => panic!("expected an inline line"),
        };
        let find = |needle: &str| runs.iter().find(|r| r.text.contains(needle)).unwrap();

        // Plain text is not a link; linked text points at the interned target
        // and gets the UA style (blue + underline).
        assert_eq!(find("go").link, 0);
        let there = find("there");
        assert_eq!(there.link, 1);
        assert!(there.underline);
        assert!(there.color.b > 0.5 && there.color.r == 0.0);
        assert_eq!(find("again").link, 1);
        // `text-decoration: none` keeps the link but drops the underline.
        let quiet = find("quietly");
        assert_eq!(quiet.link, 2);
        assert!(!quiet.underline);
        // An author color wins over the UA blue.
        let red = find("redly");
        assert_eq!(red.link, 3);
        assert!(red.color.r > 0.9 && red.color.b == 0.0);

        // The h2 keeps its id as an anchor for the #frag destination.
        let target = blocks.iter().find(|b| b.kind == BlockKind::Heading2).unwrap();
        assert_eq!(target.anchor.as_deref(), Some("frag"));
    }

    #[test]
    fn images_flow_inline_when_sharing_a_line_with_text() {
        let document = parse(
            "<p>before <img src=\"a.png\"> after</p>\
             <p><img src=\"b.png\"> leading icon</p>\
             <img src=\"alone.png\">\
             <p>text <img src=\"f.png\" style=\"float: left\"> floated</p>",
        );
        let flow = document.flow.expect("flow tree");
        let blocks = flow_blocks(&flow);

        // "before <img> after": one line, image run in the middle.
        let first_line = match &blocks[0].children[0] {
            BoxChild::Line(runs) => runs,
            other => panic!("expected a line, got {other:?}"),
        };
        assert!(first_line.iter().any(|r| r.image.is_some()));
        assert!(first_line.first().unwrap().text.contains("before"));
        assert!(first_line.last().unwrap().text.contains("after"));

        // Leading icon followed by text is inline too (lookahead).
        let second_line = match &blocks[1].children[0] {
            BoxChild::Line(runs) => runs,
            other => panic!("expected a line, got {other:?}"),
        };
        assert!(second_line.first().unwrap().image.is_some());

        // A standalone image keeps the block path; a floated one does as well.
        let block_images = count_images(&flow.children);
        assert_eq!(block_images, 2, "standalone + floated stay block-level");
    }

    fn count_images(children: &[BoxChild]) -> usize {
        children
            .iter()
            .map(|child| match child {
                BoxChild::Image(_) => 1,
                BoxChild::Block(b) => count_images(&b.children),
                _ => 0,
            })
            .sum()
    }

    #[test]
    fn cells_with_markup_get_styled_runs_and_plain_cells_stay_flat() {
        let document = parse(
            "<table><tr>\
               <td>plain text only</td>\
               <td>Total: <b>$400</b> <span style=\"color:#f00\">due</span></td>\
               <td>see <a href=\"https://x.test/inv\">the invoice</a></td>\
             </tr></table>",
        );
        let cells = &document.blocks[0].cells;

        // Plain cell: no runs (fast path), text as before.
        assert!(cells[0].runs.is_empty());
        assert_eq!(cells[0].text, "plain text only");

        // Bold + colored runs, with the flattened text still intact.
        let rich = &cells[1];
        assert_eq!(rich.text, "Total: $400 due");
        assert!(!rich.runs.is_empty());
        let bold = rich.runs.iter().find(|r| r.text.contains("$400")).unwrap();
        assert!(bold.bold);
        let due = rich.runs.iter().find(|r| r.text.contains("due")).unwrap();
        assert!(due.color.r > 0.9 && due.color.g < 0.1);

        // The linked run carries the interned target and UA link styling.
        let linked = &cells[2];
        let link_run = linked.runs.iter().find(|r| r.text.contains("invoice")).unwrap();
        assert_eq!(link_run.link, 1);
        assert!(link_run.underline);
        assert_eq!(document.links, vec!["https://x.test/inv".to_string()]);
    }

    #[test]
    fn cell_dir_attribute_sets_direction() {
        let document = parse(
            "<table><tr><td dir=\"rtl\">\u{05E9}\u{05DC}\u{05D5}\u{05DD} <b>x</b></td></tr></table>",
        );
        let cell = &document.blocks[0].cells[0];
        assert_eq!(cell.style.direction, Some(true));
        assert!(!cell.runs.is_empty());
    }

    #[test]
    fn empty_decorated_sized_blocks_are_kept() {
        // A sized background-layer div with no content must survive box
        // generation (it paints alone); a bare empty div is still dropped.
        let document = parse(
            "<div style=\"width: 200pt; height: 50pt; background: #fd4\"></div>\
             <div></div>\
             <p>text</p>",
        );
        let flow = document.flow.expect("flow tree");
        let blocks = flow_blocks(&flow);
        assert_eq!(blocks.len(), 2, "background div + paragraph, empty div dropped");
        assert!(blocks[0].background.is_some());
        assert_eq!(blocks[0].css_height, Some(50.0));
        assert!(blocks[0].children.is_empty());
    }

    #[test]
    fn dir_attribute_and_direction_css_set_base_rtl_and_default_align() {
        use super::TextAlign;
        let document = parse(
            "<p id=\"ltr\">plain</p>\
             <p id=\"attr\" dir=\"rtl\">שלום</p>\
             <p id=\"css\" style=\"direction: rtl\">שלום</p>\
             <p id=\"leftrtl\" dir=\"rtl\" style=\"text-align: left\">שלום</p>\
             <div dir=\"rtl\"><p id=\"inherit\">שלום</p></div>",
        );
        let flow = document.flow.expect("flow tree");
        let blocks = flow_blocks(&flow);
        // Only `<p>` blocks carry text; the wrapper `<div>` is block index 4.
        let ltr = &blocks[0];
        assert!(!ltr.rtl);
        assert_eq!(ltr.align, TextAlign::Left);

        let attr = &blocks[1];
        assert!(attr.rtl, "dir=rtl sets base direction");
        assert_eq!(attr.align, TextAlign::Right, "rtl default alignment is right");

        let css = &blocks[2];
        assert!(css.rtl, "direction: rtl sets base direction");
        assert_eq!(css.align, TextAlign::Right);

        // Explicit text-align wins over the direction default.
        let leftrtl = &blocks[3];
        assert!(leftrtl.rtl);
        assert_eq!(leftrtl.align, TextAlign::Left);

        // The <p> inside <div dir="rtl"> inherits the direction and right-aligns.
        let inherit = blocks.last().unwrap();
        assert!(inherit.rtl, "child inherits ancestor direction");
        assert_eq!(inherit.align, TextAlign::Right);
    }

    #[test]
    fn parses_flex_wrap_and_grid_column_lines() {
        let document = parse(
            "<style>\
             .w { display: flex; flex-wrap: wrap; }\
             .f { display: flex; flex-flow: row wrap; }\
             .g { display: grid; grid-template-columns: 1fr 1fr 1fr; }\
             .full { grid-column: 1 / -1; }\
             .mid { grid-column: 2 / 4; }\
             .sp { grid-column: span 2; }\
             </style>\
             <div class=\"w\"><span>a</span></div>\
             <div class=\"f\"><span>b</span></div>\
             <div class=\"g\">\
               <div class=\"full\">header</div>\
               <div class=\"mid\">mid</div>\
               <div class=\"sp\">spanner</div>\
             </div>",
        );
        let flow = document.flow.expect("flow tree");
        let blocks = flow_blocks(&flow);

        let wrap_boxes: Vec<bool> = blocks
            .iter()
            .filter_map(|b| b.flex.as_ref().map(|f| f.wrap))
            .collect();
        assert_eq!(wrap_boxes, vec![true, true], "flex-wrap and flex-flow both wrap");

        let full = blocks.iter().find(|b| block_text(b) == "header").unwrap();
        assert_eq!((full.grid_col_start, full.grid_col_end), (Some(1), Some(-1)));
        let mid = blocks.iter().find(|b| block_text(b) == "mid").unwrap();
        assert_eq!((mid.grid_col_start, mid.grid_col_end), (Some(2), Some(4)));
        let sp = blocks.iter().find(|b| block_text(b) == "spanner").unwrap();
        assert_eq!(sp.grid_span, 2);
        assert_eq!(sp.grid_col_start, None);
    }

    #[test]
    fn parses_grid_template_columns() {
        use super::{parse_grid_tracks, GridTrack};
        assert_eq!(
            parse_grid_tracks("120pt auto 1fr"),
            vec![GridTrack::Pt(120.0), GridTrack::Auto, GridTrack::Fr(1.0)]
        );
        assert_eq!(parse_grid_tracks("repeat(3, 1fr)"), vec![GridTrack::Fr(1.0); 3]);
        // minmax(): point/auto floors with point/fr/auto ceilings, including
        // inside repeat() (the tokenizer keeps the inner comma+space intact).
        {
            use super::{MaxTrack, MinTrack};
            assert_eq!(
                parse_grid_tracks("minmax(120pt, 1fr) minmax(auto, 200pt)"),
                vec![
                    GridTrack::MinMax(MinTrack::Pt(120.0), MaxTrack::Fr(1.0)),
                    GridTrack::MinMax(MinTrack::Auto, MaxTrack::Pt(200.0)),
                ]
            );
            assert_eq!(
                parse_grid_tracks("repeat(2, minmax(50pt, 1fr))"),
                vec![GridTrack::MinMax(MinTrack::Pt(50.0), MaxTrack::Fr(1.0)); 2]
            );
        }
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
    fn parses_per_side_borders_radius_and_box_sizing() {
        use crate::color::Color;
        use crate::html::BorderStyle;
        let document = parse(
            r#"
            <style>
            .card { border: 2px dashed rgb(255, 0, 0); border-radius: 8px; padding: 4px; }
            .rule { border-bottom: 3px solid #00f; }
            .mixed { border-style: solid; border-width: 1px 2px 3px 4px;
                     border-color: red green; }
            .boxed { box-sizing: border-box; width: 100px; border: 1px solid; }
            .off { border: 1px solid black; border-top-style: none; }
            </style>
            <div class="card">a</div>
            <div class="rule">b</div>
            <div class="mixed">c</div>
            <div class="boxed">d</div>
            <div class="off">e</div>
            "#,
        );
        let flow = document.flow.as_ref().expect("flow");
        let blocks = flow_blocks(flow);
        let by_text = |needle: &str| {
            *blocks
                .iter()
                .find(|block| block_text(block).contains(needle))
                .expect(needle)
        };

        // Shorthand: every side identical; the uniform() fast path sees it.
        let card = by_text("a");
        let edges = card.border.expect("card border");
        let side = edges.uniform().expect("uniform");
        assert_eq!(side.width, 1.5); // 2px
        assert_eq!(side.style, BorderStyle::Dashed);
        assert_eq!(side.color, Color { r: 1.0, g: 0.0, b: 0.0 });
        assert_eq!(card.border_radius, 6.0); // 8px
        // Border width consumes layout space: 4px padding + 2px border.
        assert_eq!(card.padding.left, 3.0 + 1.5);

        // A single-side declaration leaves the other sides absent.
        let rule = by_text("b");
        let edges = rule.border.expect("rule border");
        assert!(edges.top.is_none() && edges.left.is_none() && edges.right.is_none());
        let bottom = edges.bottom.expect("bottom");
        assert_eq!(bottom.width, 2.25); // 3px
        assert_eq!(bottom.color, Color { r: 0.0, g: 0.0, b: 1.0 });

        // 1-to-4 value longhands expand in top/right/bottom/left order; a
        // 2-value color list alternates.
        let mixed = by_text("c");
        let edges = mixed.border.expect("mixed border");
        let widths = [
            edges.top.unwrap().width,
            edges.right.unwrap().width,
            edges.bottom.unwrap().width,
            edges.left.unwrap().width,
        ];
        assert_eq!(widths, [0.75, 1.5, 2.25, 3.0]);
        assert_eq!(edges.top.unwrap().color, Color { r: 1.0, g: 0.0, b: 0.0 });
        assert_eq!(edges.right.unwrap().color, edges.left.unwrap().color);

        // box-sizing flows through; `border: 1px solid` with no color is black.
        let boxed = by_text("d");
        assert!(boxed.border_box);
        assert_eq!(boxed.border.unwrap().uniform().unwrap().color, Color::BLACK);

        // A later longhand can knock one side out of an earlier shorthand.
        let off = by_text("e");
        let edges = off.border.expect("off border");
        assert!(edges.top.is_none());
        assert!(edges.bottom.is_some());
    }

    #[test]
    fn parses_font_face_rules() {
        use crate::font::FontFaceSource;
        let document = parse(
            r#"
            <style>
            @font-face {
                font-family: "Brand Font";
                src: url(brand.woff2) format("WOFF2"),
                     url("data:font/ttf;base64,AAEC,tail") format(truetype),
                     local('Arial');
                font-weight: 700;
                font-style: italic;
            }
            @media print {
                @font-face { font-family: PrintFace; src: url(print.ttf); }
            }
            @media screen {
                @font-face { font-family: ScreenFace; src: url(screen.ttf); }
            }
            @font-face { font-family: NoSrc; }
            body { color: black; }
            </style>
            <p>text</p>
            "#,
        );

        let faces = &document.font_faces;
        // The screen-only and src-less rules are dropped.
        assert_eq!(faces.len(), 2, "{faces:?}");

        let brand = &faces[0];
        assert_eq!(brand.family, "Brand Font");
        assert!(brand.bold && brand.italic);
        assert_eq!(
            brand.sources,
            vec![
                FontFaceSource::Url {
                    url: "brand.woff2".into(),
                    format: Some("woff2".into()),
                },
                // The base64 comma must not split the src list.
                FontFaceSource::Url {
                    url: "data:font/ttf;base64,AAEC,tail".into(),
                    format: Some("truetype".into()),
                },
                FontFaceSource::Local("Arial".into()),
            ]
        );

        let print_face = &faces[1];
        assert_eq!(print_face.family, "PrintFace");
        assert!(!print_face.bold && !print_face.italic);
        assert_eq!(
            print_face.sources,
            vec![FontFaceSource::Url { url: "print.ttf".into(), format: None }]
        );
    }

    #[test]
    fn parses_percent_edges_and_min_max_sizing() {
        let document = parse(
            r#"
            <style>
              .card {
                padding: 5% 10px;
                margin-left: 25%;
                min-width: 40%;
                min-height: 120px;
                max-height: 200px;
              }
              .abs { position: absolute; top: 10%; left: 5%; }
              .plain { padding: 8px; margin: 4px; }
            </style>
            <div class="card">sized</div>
            <div class="abs">positioned</div>
            <div class="plain">plain</div>
            "#,
        );
        let flow = document.flow.expect("flow doc");
        let blocks = flow_blocks(&flow);

        let card = blocks
            .iter()
            .find(|b| block_text(b) == "sized")
            .expect("card block");
        // `padding: 5% 10px` → top/bottom 5% (percent), right/left 10px (points).
        assert_eq!(card.padding_percent.top, Some(5.0));
        assert_eq!(card.padding_percent.bottom, Some(5.0));
        assert_eq!(card.padding_percent.right, None);
        assert_eq!(card.padding_percent.left, None);
        assert!((card.padding.right - 7.5).abs() < 0.01, "10px → 7.5pt");
        // `margin-left: 25%` is a percent side; the point margin stays 0.
        assert_eq!(card.margin_percent.left, Some(25.0));
        assert_eq!(card.min_width_percent, Some(40.0));
        assert!((card.min_height.unwrap() - 90.0).abs() < 0.01, "120px → 90pt");
        assert!((card.max_height.unwrap() - 150.0).abs() < 0.01, "200px → 150pt");

        let abs = blocks
            .iter()
            .find(|b| block_text(b) == "positioned")
            .expect("abs block");
        assert_eq!(abs.offset_percent.top, Some(10.0));
        assert_eq!(abs.offset_percent.left, Some(5.0));

        // A plain length-only block carries no boxed sizing extras (RAM stays
        // flat for the common case).
        let plain = blocks
            .iter()
            .find(|b| block_text(b) == "plain")
            .expect("plain block");
        assert_eq!(plain.padding_percent, crate::box_tree::EdgesPercent::default());
        assert_eq!(plain.min_width, None);
        assert!((plain.padding.left - 6.0).abs() < 0.01, "8px → 6pt");
    }

    #[test]
    fn generates_before_and_after_content() {
        use crate::color::Color;
        let document = parse(
            r#"
            <style>
              .req::after { content: " *"; color: #cc0000; }
              .badge::before { content: "NEW "; }
              a::after { content: " (" attr(href) ")"; }
              h3:before { content: "\00A7 "; }
              .none::before { content: none; }
              .counterish::before { content: counter(x) ". "; }
            </style>
            <h3>section</h3>
            <p class="badge">badge</p>
            <p>a <span class="req">field</span> here</p>
            <p><a href="https://x.test">link</a></p>
            <p class="none">plain</p>
            <p class="counterish">nocounter</p>
            "#,
        );
        let flow = document.flow.expect("flow doc");
        let blocks = flow_blocks(&flow);
        let texts: Vec<String> = blocks.iter().map(|b| block_text(b)).collect();

        // Legacy single-colon :before, with a CSS hex escape whose trailing
        // space is the escape terminator (not part of the string).
        assert!(texts.iter().any(|t| t == "§section"), "{texts:?}");
        // ::before leads the block's content.
        assert!(texts.iter().any(|t| t == "NEW badge"), "{texts:?}");
        // ::after on an inline element, styled by the pseudo rule (red), and
        // the rule must NOT color the element's own text.
        let field = blocks
            .iter()
            .find(|b| block_text(b).contains("field"))
            .expect("field para");
        assert_eq!(block_text(field), "a field * here");
        let runs: Vec<&InlineRun> = field
            .children
            .iter()
            .filter_map(|c| match c {
                BoxChild::Line(runs) => Some(runs.iter()),
                _ => None,
            })
            .flatten()
            .collect();
        let star = runs.iter().find(|r| r.text.contains('*')).expect("star run");
        assert_eq!(star.color, Color::from_rgb_u8(204, 0, 0));
        let word = runs.iter().find(|r| r.text.contains("field")).expect("field run");
        assert_eq!(word.color, Color::BLACK);
        // attr() + string concatenation.
        assert!(texts.iter().any(|t| t == "link (https://x.test)"), "{texts:?}");
        // content: none and unsupported counter() generate nothing.
        assert!(texts.iter().any(|t| t == "plain"), "{texts:?}");
        assert!(texts.iter().any(|t| t == "nocounter"), "{texts:?}");
    }

    #[test]
    fn text_transform_and_spacing_reach_runs_and_blocks() {
        let document = parse(
            r#"
            <style>
              .up { text-transform: uppercase; }
              .cap { text-transform: capitalize; }
              .undo { text-transform: none; }
              .track { letter-spacing: 2pt; word-spacing: 4pt; }
              .indent { text-indent: 24pt; }
              .indentpct { text-indent: 10%; }
            </style>
            <p class="up">shout <span class="undo">quietly</span></p>
            <p class="cap">two <b>wo</b>rds</p>
            <p class="track">spaced text</p>
            <p class="indent">indented</p>
            <p class="indentpct">pct</p>
            "#,
        );
        let flow = document.flow.expect("flow doc");
        let blocks = flow_blocks(&flow);
        let by_text = |needle: &str| {
            blocks
                .iter()
                .find(|b| block_text(b).contains(needle))
                .unwrap_or_else(|| panic!("no block containing {needle}"))
        };

        // Uppercase applies; an explicit `none` on a descendant overrides it.
        assert_eq!(block_text(by_text("SHOUT")), "SHOUT quietly");
        // Capitalize uppercases word starts, and the boundary state carries
        // across a run split mid-word (`wo` bold + `rds`): only "Two Words".
        assert_eq!(block_text(by_text("Two")), "Two Words");
        // Spacing lands on the runs.
        let track = by_text("spaced");
        let run = first_run(track);
        assert_eq!(run.letter_spacing, 2.0);
        assert_eq!(run.word_spacing, 4.0);
        // text-indent lands on the block (points / percent).
        assert_eq!(by_text("indented").text_indent, 24.0);
        assert_eq!(by_text("pct").text_indent_percent, Some(10.0));
    }

    #[test]
    fn text_transform_reaches_plain_table_cells() {
        let document = parse(
            r#"
            <style>th { text-transform: uppercase; }</style>
            <table><tr><th>hello</th><td>world</td></tr></table>
            "#,
        );
        let row = &document.blocks[0];
        assert_eq!(row.cells[0].text, "HELLO");
        assert_eq!(row.cells[1].text, "world");
    }

    #[test]
    fn evaluates_calc_expressions() {
        use super::parse_calc;
        let approx = |a: Option<f32>, b: Option<f32>| match (a, b) {
            (Some(x), Some(y)) => (x - y).abs() < 0.01,
            (None, None) => true,
            _ => false,
        };
        let check = |expr: &str, pt: Option<f32>, pct: Option<f32>| {
            let (gpt, gpct) = parse_calc(expr);
            assert!(approx(gpt, pt) && approx(gpct, pct), "{expr} → ({gpt:?}, {gpct:?})");
        };
        // Pure length (15px = 11.25pt), pure percent, and mixed.
        check("calc(10px + 5px)", Some(11.25), None);
        check("calc(50% - 10%)", None, Some(40.0));
        check("calc(100% - 20px)", Some(-15.0), Some(100.0));
        // Multiplication and division by a unitless number.
        check("calc(2 * 30pt)", Some(60.0), None);
        check("calc(1in / 2)", Some(36.0), None);
        // Parentheses and precedence: (10+20)*2 = 60, not 10+40.
        check("calc((10pt + 20pt) * 2)", Some(60.0), None);
        check("calc(10pt + 20pt * 2)", Some(50.0), None);
        // Nested calc() acts as a parenthesized sub-expression.
        check("calc(calc(40pt) + 5pt)", Some(45.0), None);
        // Invalid: length × length, a bare number as a length, divide by zero.
        check("calc(10pt * 20pt)", None, None);
        check("calc(5 + 3)", None, None);
        check("calc(100% / 0)", None, None);
    }

    #[test]
    fn resolves_custom_properties_and_var() {
        use crate::color::Color;
        let document = parse(
            r#"
            <style>
              :root { --brand: #c0392b; --pad: 12pt; --gap: var(--pad); }
              .brand { color: var(--brand); padding-left: var(--pad); }
              .fallback { color: var(--missing, #008000); }
              .chain { margin-left: var(--gap); }
              .inherited { padding-left: var(--pad); }
            </style>
            <p class="brand">brand</p>
            <p class="fallback">fallback</p>
            <p class="chain">chain</p>
            <div class="brand"><p class="inherited">nested</p></div>
            "#,
        );
        let flow = document.flow.expect("flow doc");
        let blocks = flow_blocks(&flow);
        let find = |t: &str| {
            blocks
                .iter()
                .find(|b| block_text(b) == t)
                .unwrap_or_else(|| panic!("no block {t}"))
        };

        // `var(--brand)` → #c0392b (192,57,43).
        let brand = find("brand");
        assert_eq!(first_run(brand).color, Color::from_rgb_u8(192, 57, 43));
        // `padding-left: var(--pad)` → 12pt.
        assert!((brand.padding.left - 12.0).abs() < 0.01, "{}", brand.padding.left);
        // A missing var falls back to the second argument, #008000 (green).
        assert_eq!(first_run(find("fallback")).color, Color::from_rgb_u8(0, 128, 0));
        // `--gap: var(--pad)` — a custom property referencing another resolves.
        assert!((find("chain").margin.left - 12.0).abs() < 0.01);
        // Custom properties inherit: the child reads the ancestor's `--pad`.
        assert!((find("nested").padding.left - 12.0).abs() < 0.01);
    }

    #[test]
    fn percent_padding_cascade_merges_per_property() {
        // A high-priority rule setting only `min-width` must not wipe a
        // lower-priority `max-height` from another rule (boxed-sizing merge).
        let document = parse(
            r#"
            <style>
              .box { max-height: 100px; }
              div.box { min-width: 50%; }
            </style>
            <div class="box">merged</div>
            "#,
        );
        let flow = document.flow.expect("flow doc");
        let block = flow_blocks(&flow)
            .into_iter()
            .find(|b| block_text(b) == "merged")
            .expect("box block");
        assert_eq!(block.min_width_percent, Some(50.0));
        assert!((block.max_height.unwrap() - 75.0).abs() < 0.01, "both survive");
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
        assert_eq!(cells[1].style.align, Some(super::TextAlign::Justify));
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
