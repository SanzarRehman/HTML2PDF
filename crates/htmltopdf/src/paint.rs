use crate::color::Color;

#[derive(Debug, Clone, PartialEq)]
pub enum PaintCommand {
    SetFillColor(Color),
    SetStrokeColor(Color),
    /// Set the stroke line width (PDF points) for subsequent strokes.
    SetLineWidth(f32),
    /// Set the stroke dash pattern (`Some` = dashed, `None` = back to solid).
    /// Used for `border-style: dashed`/`dotted`; every dashed stroke is paired
    /// with a `SetDash(None)` reset so later strokes stay solid.
    SetDash(Option<DashPattern>),
    Text(TextCommand),
    StrokeRect(RectCommand),
    FillRect(RectCommand),
    StrokeLine(LineCommand),
    /// Rounded rectangle (uniform corner radius), for `border-radius`.
    StrokeRoundedRect(RoundedRectCommand),
    FillRoundedRect(RoundedRectCommand),
    PushClipRect(RectCommand),
    PopClip,
    Image(ImageCommand),
}

/// An on/off stroke dash pattern in points (PDF `[on off] 0 d`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DashPattern {
    pub on: f32,
    pub off: f32,
}

/// A rectangle with a uniform corner radius (already clamped to half the
/// shorter box side by the producer).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RoundedRectCommand {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    pub radius: f32,
}

/// Draw image `image_index` (into the document's image table) into the box whose
/// lower-left corner is `(x, y)`, scaled to `width` x `height` points.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ImageCommand {
    pub image_index: usize,
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TextCommand {
    pub text: String,
    pub x: f32,
    pub y: f32,
    pub font_size: f32,
    /// Resolved font-table index (see `RenderOptions::fonts`; 0 = default).
    pub font: u16,
    /// Render with faux-bold (fill+stroke) when no bold font face is embedded.
    pub bold: bool,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RectCommand {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LineCommand {
    pub x1: f32,
    pub y1: f32,
    pub x2: f32,
    pub y2: f32,
}
