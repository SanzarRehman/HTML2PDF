//! The flow box tree: a nested block/inline model for non-table documents.
//!
//! This is the structural step the flat `Vec<Block>` model could not represent
//! (ADR 0002 step 8). Box generation (`html::build_flow`) lowers the DOM into a
//! tree of block boxes whose leaves are runs of styled inline text; layout
//! (`layout::layout_flow`) then walks that tree recursively, so nesting,
//! indentation, and per-run color/size finally survive into the PDF.
//!
//! Blocks stack vertically and establish a containing block (a left indent and a
//! width); inline content is collected into line boxes that the layout wraps to
//! the containing width. A block with both inline text and child blocks keeps
//! them interleaved as `BoxChild`s, which is how anonymous block boxes behave.

use crate::color::Color;
use crate::html::{
    AlignItems, BlockKind, Clear, FlexDirection, FloatDir, GridTrack, JustifyContent,
    PositionKind, TableCell, TextAlign,
};

/// The root of a non-table document: a sequence of top-level boxes. The root
/// itself contributes no spacing of its own — only its children do.
#[derive(Debug, Clone, PartialEq)]
pub struct FlowRoot {
    pub children: Vec<BoxChild>,
}

/// One child in a block's content: either a nested block box or a run of inline
/// content (a line box, wrapped to the containing width at layout time).
#[derive(Debug, Clone, PartialEq)]
pub enum BoxChild {
    Block(BlockBox),
    /// Inline content. Each `InlineRun` carries its own style; the layout
    /// collapses whitespace across runs and wraps them into visual lines. A hard
    /// break (`<br>`) splits content into separate `Line` children.
    Line(Vec<InlineRun>),
    /// A block-level `<img>`. Resolved after parsing by `html::resolve_images`.
    Image(ImageBox),
    /// A `<table>` embedded in flow content, laid out in document order alongside
    /// surrounding headings/paragraphs (rather than the mutually-exclusive
    /// spreadsheet path). Rows are pre-collected from the `<table>` subtree.
    Table(TableBox),
}

/// A table living inside the flow tree. `rows` are the collected `<tr>`s (with
/// their header/body/footer kind); `columns` are declared `<col>` widths (empty
/// → fully automatic sizing); `row_height` mirrors the document-level table row
/// height. Column widths and geometry are resolved at layout time.
#[derive(Debug, Clone, PartialEq)]
pub struct TableBox {
    pub rows: Vec<TableRow>,
    pub columns: Vec<f32>,
    pub row_height: Option<f32>,
}

/// One row of a [`TableBox`]: its section kind and its cells.
#[derive(Debug, Clone, PartialEq)]
pub struct TableRow {
    pub kind: BlockKind,
    pub cells: Vec<TableCell>,
}

/// A block-level image box. Before image resolution it carries the source, any
/// presentational HTML `width`/`height` attributes (CSS pixels), and any CSS
/// `width`/`height` from the cascade (already resolved to points); resolution
/// fills in `image_index` (into the document's image table) and the laid-out
/// point `width`/`height`. CSS dimensions take precedence over the HTML
/// attributes, matching browser behavior. An unresolved or failed image keeps
/// `image_index == None` and is not painted.
#[derive(Debug, Clone, PartialEq)]
pub struct ImageBox {
    pub src: String,
    /// Presentational `width`/`height` attributes, in CSS pixels.
    pub attr_width: Option<f32>,
    pub attr_height: Option<f32>,
    /// Cascaded CSS `width`/`height`, already resolved to points; a
    /// percentage width resolves against the containing block at layout time.
    pub css_width: Option<f32>,
    pub css_width_percent: Option<f32>,
    pub css_height: Option<f32>,
    /// CSS `max-width` (points / percent), clamped at layout time.
    pub max_width: Option<f32>,
    pub max_width_percent: Option<f32>,
    pub image_index: Option<usize>,
    pub width: f32,
    pub height: f32,
    /// CSS `float` on the image: text wraps around it.
    pub float_dir: Option<FloatDir>,
}

/// A block-level box. `kind` drives the default font size (and the default
/// margins, when CSS sets none); `margin` and `padding` are the resolved CSS box
/// edges (list/blockquote nesting folds into `margin.left`); `align` applies to
/// the inline content it contains.
#[derive(Debug, Clone, PartialEq)]
pub struct BlockBox {
    pub kind: BlockKind,
    pub margin: Edges,
    pub padding: Edges,
    pub align: TextAlign,
    /// A non-white background color to paint behind the block's border box.
    pub background: Option<Color>,
    /// Resolved border sides (per-side width/style/color); `None` = no border.
    /// The side widths are already folded into `padding`, so the border box is
    /// `content + padding` and borders consume layout space.
    pub border: Option<crate::html::BorderEdges>,
    /// `border-radius` (uniform corner radius, points; 0 = square). Applied to
    /// the background fill and — when the border is uniform — the border
    /// stroke; content is not clipped to the rounded shape.
    pub border_radius: f32,
    /// `box-sizing: border-box`: the declared CSS width/height include padding
    /// and borders instead of adding to them.
    pub border_box: bool,
    /// `Some` when this block is a `display: flex` container: its block children
    /// are laid out as flex items instead of stacking vertically.
    pub flex: Option<FlexContainer>,
    /// Flex *item* properties, used when this block is a child of a flex
    /// container (`flex-grow`; `flex-basis` in points, `None` = auto/content).
    pub flex_grow: f32,
    pub flex_basis: Option<f32>,
    /// `Some` when this block is a `display: grid` container: its children are
    /// placed into columns row-major instead of stacking vertically.
    pub grid: Option<GridContainer>,
    /// `grid-column: span N` when this block is a grid item (1 = one track).
    pub grid_span: usize,
    /// Line-based `grid-column: start / end` (1-based; negative from the end),
    /// resolved against the track count at layout time.
    pub grid_col_start: Option<i32>,
    pub grid_col_end: Option<i32>,
    /// CSS `float`: the block is taken out of normal flow and placed at the
    /// left/right edge; following line boxes shorten around it.
    pub float_dir: Option<FloatDir>,
    /// CSS `clear`: drop below active floats on the given side(s) first.
    pub clear: Option<Clear>,
    /// Cascaded CSS `width` (points), honored for floated blocks (otherwise a
    /// float is shrink-to-fit), positioned boxes, and in-flow blocks. A
    /// percentage width resolves against the containing block at layout time.
    pub css_width: Option<f32>,
    pub css_width_percent: Option<f32>,
    /// CSS `max-width` (points / percent), clamping the used width.
    pub max_width: Option<f32>,
    pub max_width_percent: Option<f32>,
    /// CSS `height` (points, content-box). Treated as a *minimum* box height:
    /// the block extends to it when its content is shorter (content taller
    /// than the height overflows visibly, as in CSS `overflow: visible`).
    pub css_height: Option<f32>,
    /// `margin-left: auto` + `margin-right: auto` + a width = centered.
    pub center: bool,
    /// CSS `line-height` (inherited): overrides the default leading of this
    /// block's line boxes (`None` = UA default, `font × 1.35`).
    pub line_height: Option<crate::html::LineHeight>,
    /// Base paragraph direction (UAX #9 base level) for this block's inline
    /// content: `true` = right-to-left (`dir="rtl"` / `direction: rtl`).
    pub rtl: bool,
    /// CSS `position` (static when `None`) with its box offsets in points.
    pub position: Option<PositionKind>,
    /// CSS `z-index` (`None` = `auto` = 0): paint order among positioned boxes.
    pub z_index: Option<i32>,
    pub offset_top: Option<f32>,
    pub offset_right: Option<f32>,
    pub offset_bottom: Option<f32>,
    pub offset_left: Option<f32>,
    /// The element's HTML `id`, kept as a destination for `#fragment` links.
    pub anchor: Option<String>,
    pub children: Vec<BoxChild>,
}

/// Flex container parameters resolved from the cascade.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FlexContainer {
    pub direction: FlexDirection,
    pub justify: JustifyContent,
    pub align: AlignItems,
    pub gap: f32,
    /// `flex-wrap: wrap` — items break onto additional flex lines.
    pub wrap: bool,
}

/// Grid container parameters resolved from the cascade. An empty `columns`
/// list behaves as a single `auto` column.
#[derive(Debug, Clone, PartialEq)]
pub struct GridContainer {
    pub columns: Vec<GridTrack>,
    pub column_gap: f32,
    pub row_gap: f32,
}

/// The four CSS box edges, in points.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Edges {
    pub top: f32,
    pub right: f32,
    pub bottom: f32,
    pub left: f32,
}

/// A contiguous run of inline text sharing one computed style. Text is stored
/// verbatim (including its whitespace); the layout collapses runs of whitespace
/// when it wraps. `bold` renders as faux-bold (fill+stroke) in the PDF, since a
/// single (regular) font face is embedded.
#[derive(Debug, Clone, PartialEq)]
pub struct InlineRun {
    pub text: String,
    pub font_size: f32,
    pub bold: bool,
    /// Interned font-spec index into `Document::font_specs` (0 = default).
    pub font: u16,
    /// Interned link target: `Document::links[link - 1]` (0 = not a link).
    pub link: u16,
    /// An inline `<img>` flowing with the text (empty `text` in that case):
    /// it occupies its resolved width on the line and sits on the baseline.
    /// `None` for ordinary text runs.
    pub image: Option<Box<ImageBox>>,
    /// `text-decoration: underline` (also `<u>`/`<ins>`), stroked below the baseline.
    pub underline: bool,
    /// `text-decoration: line-through` (also `<s>`/`<strike>`/`<del>`).
    pub line_through: bool,
    pub color: Color,
}

impl FlowRoot {
    /// True when the tree carries no visible content at all (e.g. a document that
    /// is only whitespace or `display:none`). Used to treat such input as empty.
    pub fn has_text(&self) -> bool {
        children_have_text(&self.children)
    }

    /// True when the tree has any non-table content (text, images) — i.e. the
    /// document is *not* a bare table. A pure-table document falls back to the
    /// dedicated spreadsheet layout path; a mixed one is laid out as flow with the
    /// table embedded in document order.
    pub fn has_nontable_content(&self) -> bool {
        children_have_nontable(&self.children)
    }
}

fn children_have_text(children: &[BoxChild]) -> bool {
    children.iter().any(|child| match child {
        BoxChild::Block(block) => children_have_text(&block.children),
        BoxChild::Line(runs) => runs
            .iter()
            .any(|run| run.image.is_some() || !run.text.trim().is_empty()),
        // An image is visible content in its own right. This is evaluated both
        // before image resolution (to keep an image-only document's flow tree)
        // and after, so it counts regardless of whether `image_index` is set yet.
        BoxChild::Image(_) => true,
        BoxChild::Table(table) => table.rows.iter().any(|row| !row.cells.is_empty()),
    })
}

fn children_have_nontable(children: &[BoxChild]) -> bool {
    children.iter().any(|child| match child {
        BoxChild::Block(block) => children_have_nontable(&block.children),
        BoxChild::Line(runs) => runs
            .iter()
            .any(|run| run.image.is_some() || !run.text.trim().is_empty()),
        BoxChild::Image(_) => true,
        BoxChild::Table(_) => false,
    })
}
