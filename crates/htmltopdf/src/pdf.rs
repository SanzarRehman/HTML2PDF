use std::fmt;
use std::io::Write;

use flate2::{write::ZlibEncoder, Compression};

use crate::layout::{Page, RenderOptions};
use crate::paint::PaintCommand;

#[derive(Debug)]
pub enum PdfError {
    TooManyObjects,
    Compression(std::io::Error),
}

impl fmt::Display for PdfError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TooManyObjects => write!(f, "too many PDF objects"),
            Self::Compression(error) => write!(f, "failed to compress PDF stream: {error}"),
        }
    }
}

impl std::error::Error for PdfError {}

pub fn write_pdf(pages: &[Page], options: &RenderOptions) -> Result<Vec<u8>, PdfError> {
    let page_count = pages.len();
    let embedded = options.font.embedding();
    // An embedded TrueType font needs two extra objects: a FontDescriptor and
    // the FontFile2 stream.
    let base_object_count = 3 + (page_count * 2);
    let object_count = base_object_count + if embedded.is_some() { 2 } else { 0 };

    if object_count > u16::MAX as usize {
        return Err(PdfError::TooManyObjects);
    }

    let catalog_id = 1;
    let pages_id = 2;
    let font_id = 3;
    let first_page_id = 4;
    let descriptor_id = base_object_count + 1;
    let fontfile_id = base_object_count + 2;

    let mut writer = PdfWriter::new();
    writer.write_header();

    writer.object(catalog_id, "<< /Type /Catalog /Pages 2 0 R >>");

    let kids = (0..page_count)
        .map(|index| format!("{} 0 R", first_page_id + (index * 2)))
        .collect::<Vec<_>>()
        .join(" ");
    writer.object(
        pages_id,
        &format!("<< /Type /Pages /Kids [{kids}] /Count {page_count} >>"),
    );

    // The font object (id 3, referenced as /F1 by every page).
    match embedded {
        None => writer.object(
            font_id,
            "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>",
        ),
        Some(font) => {
            let widths = font
                .widths
                .iter()
                .map(|w| w.to_string())
                .collect::<Vec<_>>()
                .join(" ");
            writer.object(
                font_id,
                &format!(
                    concat!(
                        "<< /Type /Font /Subtype /TrueType /BaseFont /{} ",
                        "/FirstChar {} /LastChar {} /Widths [{}] ",
                        "/FontDescriptor {} 0 R /Encoding /WinAnsiEncoding >>"
                    ),
                    font.postscript_name,
                    font.first_char,
                    font.last_char,
                    widths,
                    descriptor_id
                ),
            );
        }
    }

    for (index, page) in pages.iter().enumerate() {
        let page_id = first_page_id + (index * 2);
        let content_id = page_id + 1;
        let content = page_content(page);

        writer.object(
            page_id,
            &format!(
                concat!(
                    "<< /Type /Page /Parent 2 0 R ",
                    "/MediaBox [0 0 {:.2} {:.2}] ",
                    "/Resources << /Font << /F1 3 0 R >> >> ",
                    "/Contents {} 0 R >>"
                ),
                options.page_size.width, options.page_size.height, content_id
            ),
        );

        writer.stream_object(content_id, content.as_bytes())?;
    }

    // The embedded font's descriptor and program, after the pages.
    if let Some(font) = embedded {
        writer.object(
            descriptor_id,
            &format!(
                concat!(
                    "<< /Type /FontDescriptor /FontName /{} /Flags {} ",
                    "/FontBBox [{} {} {} {}] /ItalicAngle {:.2} ",
                    "/Ascent {} /Descent {} /CapHeight {} /StemV {} ",
                    "/FontFile2 {} 0 R >>"
                ),
                font.postscript_name,
                font.flags,
                font.bbox[0],
                font.bbox[1],
                font.bbox[2],
                font.bbox[3],
                font.italic_angle,
                font.ascent,
                font.descent,
                font.cap_height,
                font.stem_v,
                fontfile_id
            ),
        );
        writer.font_file_object(fontfile_id, &font.data)?;
    }

    writer.finish(catalog_id, object_count);
    Ok(writer.into_bytes())
}

fn compress_stream(stream: &[u8]) -> Result<Vec<u8>, PdfError> {
    let mut encoder = ZlibEncoder::new(Vec::new(), Compression::fast());
    encoder.write_all(stream).map_err(PdfError::Compression)?;
    encoder.finish().map_err(PdfError::Compression)
}

fn page_content(page: &Page) -> String {
    let mut content = String::new();

    for command in &page.commands {
        match command {
            PaintCommand::SetFillColor(color) => {
                content.push_str(&format!(
                    "{:.4} {:.4} {:.4} rg\n",
                    color.r, color.g, color.b
                ));
            }
            PaintCommand::SetStrokeColor(color) => {
                content.push_str(&format!(
                    "{:.4} {:.4} {:.4} RG\n",
                    color.r, color.g, color.b
                ));
            }
            PaintCommand::StrokeRect(rect) => {
                content.push_str(&format!(
                    "{:.2} {:.2} {:.2} {:.2} re S\n",
                    rect.x, rect.y, rect.width, rect.height
                ));
            }
            PaintCommand::FillRect(rect) => {
                content.push_str(&format!(
                    "{:.2} {:.2} {:.2} {:.2} re f\n",
                    rect.x, rect.y, rect.width, rect.height
                ));
            }
            PaintCommand::StrokeLine(line) => {
                content.push_str(&format!(
                    "{:.2} {:.2} m {:.2} {:.2} l S\n",
                    line.x1, line.y1, line.x2, line.y2
                ));
            }
            PaintCommand::PushClipRect(rect) => {
                content.push_str("q\n");
                content.push_str(&format!(
                    "{:.2} {:.2} {:.2} {:.2} re W n\n",
                    rect.x, rect.y, rect.width, rect.height
                ));
            }
            PaintCommand::PopClip => {
                content.push_str("Q\n");
            }
            PaintCommand::Text(text) => {
                content.push_str("BT\n");
                content.push_str(&format!("/F1 {:.2} Tf\n", text.font_size));
                content.push_str(&format!("{:.2} {:.2} Td\n", text.x, text.y));
                content.push_str(&format!("({}) Tj\n", escape_text(&text.text)));
                content.push_str("ET\n");
            }
        }
    }

    content
}

fn escape_text(text: &str) -> String {
    let mut output = String::with_capacity(text.len());

    for ch in text.chars() {
        match ch {
            '(' => output.push_str("\\("),
            ')' => output.push_str("\\)"),
            '\\' => output.push_str("\\\\"),
            '\r' => output.push_str("\\r"),
            '\n' => output.push_str("\\n"),
            '\t' => output.push_str("\\t"),
            _ if ch.is_ascii() => output.push(ch),
            _ => output.push('?'),
        }
    }

    output
}

struct PdfWriter {
    bytes: Vec<u8>,
    offsets: Vec<usize>,
}

impl PdfWriter {
    fn new() -> Self {
        Self {
            bytes: Vec::new(),
            offsets: vec![0],
        }
    }

    fn write_header(&mut self) {
        self.bytes.extend_from_slice(b"%PDF-1.7\n");
    }

    fn object(&mut self, id: usize, body: &str) {
        self.start_object(id);
        self.bytes.extend_from_slice(body.as_bytes());
        self.bytes.extend_from_slice(b"\nendobj\n");
    }

    fn stream_object(&mut self, id: usize, stream: &[u8]) -> Result<(), PdfError> {
        let compressed = compress_stream(stream)?;
        self.start_object(id);
        self.bytes.extend_from_slice(
            format!(
                "<< /Length {} /Filter /FlateDecode >>\nstream\n",
                compressed.len()
            )
            .as_bytes(),
        );
        self.bytes.extend_from_slice(&compressed);
        self.bytes.extend_from_slice(b"endstream\nendobj\n");
        Ok(())
    }

    /// A compressed font program stream. `/Length1` is the uncompressed length,
    /// which PDF requires for embedded TrueType (`FontFile2`) programs.
    fn font_file_object(&mut self, id: usize, font_data: &[u8]) -> Result<(), PdfError> {
        let compressed = compress_stream(font_data)?;
        self.start_object(id);
        self.bytes.extend_from_slice(
            format!(
                "<< /Length {} /Length1 {} /Filter /FlateDecode >>\nstream\n",
                compressed.len(),
                font_data.len()
            )
            .as_bytes(),
        );
        self.bytes.extend_from_slice(&compressed);
        self.bytes.extend_from_slice(b"endstream\nendobj\n");
        Ok(())
    }

    fn finish(&mut self, root_id: usize, object_count: usize) {
        let xref_offset = self.bytes.len();
        self.bytes
            .extend_from_slice(format!("xref\n0 {}\n", object_count + 1).as_bytes());
        self.bytes.extend_from_slice(b"0000000000 65535 f \n");

        for offset in self.offsets.iter().skip(1) {
            self.bytes
                .extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
        }

        self.bytes.extend_from_slice(
            format!(
                concat!(
                    "trailer\n",
                    "<< /Size {} /Root {} 0 R >>\n",
                    "startxref\n",
                    "{}\n",
                    "%%EOF\n"
                ),
                object_count + 1,
                root_id,
                xref_offset
            )
            .as_bytes(),
        );
    }

    fn into_bytes(self) -> Vec<u8> {
        self.bytes
    }

    fn start_object(&mut self, id: usize) {
        while self.offsets.len() <= id {
            self.offsets.push(0);
        }

        self.offsets[id] = self.bytes.len();
        self.bytes
            .extend_from_slice(format!("{id} 0 obj\n").as_bytes());
    }
}

#[cfg(test)]
mod tests {
    use crate::color::Color;
    use crate::layout::{Line, Page, RenderOptions};
    use crate::paint::{PaintCommand, RectCommand};

    use super::{escape_text, page_content, write_pdf};

    #[test]
    fn escapes_pdf_text() {
        let mut page = Page::new();
        page.push_colored_line(
            Line {
                text: "A (test) \\ value".to_string(),
                x: 48.0,
                y: 700.0,
                font_size: 12.0,
                leading: 16.0,
            },
            Color::BLACK,
        );

        assert_eq!(escape_text("A (test) \\ value"), "A \\(test\\) \\\\ value");

        let pdf = write_pdf(&[page], &RenderOptions::default()).expect("pdf should render");
        assert!(pdf
            .windows(b"/Filter /FlateDecode".len())
            .any(|window| window == b"/Filter /FlateDecode"));
    }

    #[test]
    fn writes_clip_scope_operators() {
        let mut page = Page::new();
        page.commands.push(PaintCommand::PushClipRect(RectCommand {
            x: 10.0,
            y: 20.0,
            width: 30.0,
            height: 40.0,
        }));
        page.commands
            .push(PaintCommand::Text(crate::paint::TextCommand {
                text: "inside".to_string(),
                x: 12.0,
                y: 45.0,
                font_size: 10.0,
            }));
        page.commands.push(PaintCommand::PopClip);

        let content = page_content(&page);

        assert!(content.contains("q\n10.00 20.00 30.00 40.00 re W n\n"));
        assert!(content.contains("Q\n"));
    }

    #[test]
    fn writes_pdf_color_operators() {
        let mut page = Page::new();
        page.commands
            .push(PaintCommand::SetFillColor(Color::from_rgb_u8(255, 0, 0)));
        page.commands
            .push(PaintCommand::SetStrokeColor(Color::from_rgb_u8(0, 0, 255)));

        let content = page_content(&page);

        assert!(content.contains("1.0000 0.0000 0.0000 rg\n"));
        assert!(content.contains("0.0000 0.0000 1.0000 RG\n"));
    }
}
