use crate::color::Color;

#[derive(Debug, Clone, PartialEq)]
pub enum PaintCommand {
    SetFillColor(Color),
    SetStrokeColor(Color),
    Text(TextCommand),
    StrokeRect(RectCommand),
    FillRect(RectCommand),
    StrokeLine(LineCommand),
    PushClipRect(RectCommand),
    PopClip,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TextCommand {
    pub text: String,
    pub x: f32,
    pub y: f32,
    pub font_size: f32,
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
