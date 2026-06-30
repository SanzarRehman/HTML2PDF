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
use crate::html::{BlockKind, TextAlign};

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
    /// Whether to stroke a border around the block's border box.
    pub border: bool,
    pub children: Vec<BoxChild>,
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
/// when it wraps. `bold` is carried for fidelity but has no glyph effect yet,
/// because only a single (non-bold) font face is embedded today.
#[derive(Debug, Clone, PartialEq)]
pub struct InlineRun {
    pub text: String,
    pub font_size: f32,
    pub bold: bool,
    pub color: Color,
}

impl FlowRoot {
    /// True when the tree carries no visible text at all (e.g. a document that is
    /// only whitespace or `display:none`). Used to treat such input as empty.
    pub fn has_text(&self) -> bool {
        children_have_text(&self.children)
    }
}

fn children_have_text(children: &[BoxChild]) -> bool {
    children.iter().any(|child| match child {
        BoxChild::Block(block) => children_have_text(&block.children),
        BoxChild::Line(runs) => runs.iter().any(|run| !run.text.trim().is_empty()),
    })
}
