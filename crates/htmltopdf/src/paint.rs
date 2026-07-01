use crate::color::Color;

#[derive(Debug, Clone, PartialEq)]
pub enum PaintCommand {
    SetFillColor(Color),
    SetStrokeColor(Color),
    /// Set the stroke line width (PDF points) for subsequent strokes.
    SetLineWidth(f32),
    Text(TextCommand),
    StrokeRect(RectCommand),
    FillRect(RectCommand),
    StrokeLine(LineCommand),
    PushClipRect(RectCommand),
    PopClip,
    Image(ImageCommand),
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
