use std::fmt;
use std::io::Write;

use flate2::{write::ZlibEncoder, Compression};

use crate::image::{DecodedImage, ImageFilter};
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

pub fn write_pdf(
    pages: &[Page],
    images: &[DecodedImage],
    options: &RenderOptions,
) -> Result<Vec<u8>, PdfError> {
    let page_count = pages.len();
    let embedded = options.font.embedding();
    // An embedded font is written as a Type0/Identity-H composite, which needs
    // four extra objects after the pages: the descendant CIDFont, the
    // FontDescriptor, the FontFile2 program, and the ToUnicode CMap.
    let base_object_count = 3 + (page_count * 2);
    let font_extra = if embedded.is_some() { 4 } else { 0 };

    // Each embedded image is one XObject, plus one more for its soft mask when
    // it carries alpha. Assign object ids after the font block.
    let mut next_id = base_object_count + font_extra + 1;
    let image_plans: Vec<ImagePlan> = images
        .iter()
        .enumerate()
        .map(|(index, image)| {
            let smask_id = image.smask.as_ref().map(|_| {
                let id = next_id;
                next_id += 1;
                id
            });
            let object_id = next_id;
            next_id += 1;
            ImagePlan {
                name: format!("Im{index}"),
                object_id,
                smask_id,
            }
        })
        .collect();
    let object_count = next_id - 1;

    if object_count > u16::MAX as usize {
        return Err(PdfError::TooManyObjects);
    }

    let catalog_id = 1;
    let pages_id = 2;
    let font_id = 3;
    let first_page_id = 4;
    let descendant_id = base_object_count + 1;
    let descriptor_id = base_object_count + 2;
    let fontfile_id = base_object_count + 3;
    let tounicode_id = base_object_count + 4;

    // Every page declares all image XObjects in its resources (unused ones are
    // harmless), so a `/ImN Do` operator resolves regardless of page.
    let xobject_resources = if image_plans.is_empty() {
        String::new()
    } else {
        let entries = image_plans
            .iter()
            .map(|plan| format!("/{} {} 0 R", plan.name, plan.object_id))
            .collect::<Vec<_>>()
            .join(" ");
        format!(" /XObject << {entries} >>")
    };

    // For an embedded font, shape every unique text run this document paints
    // (cache-assisted — layout already shaped them for measurement) to resolve
    // the used glyph ids, their natural widths, and the Unicode mapping.
    let cid = embedded.map(|font| {
        let used: std::collections::BTreeSet<&str> = pages
            .iter()
            .flat_map(|page| page.commands.iter())
            .filter_map(|command| match command {
                PaintCommand::Text(text) => Some(text.text.as_str()),
                _ => None,
            })
            .collect();
        font.shaped_cid_layout(used.into_iter())
    });

    // Subset the embedded program to the used glyphs when possible (retain-GIDs,
    // so the maps above stay valid); fall back to the full program. A subset font
    // gets a `ABCDEF+` name prefix, as PDF readers expect.
    let used_gids: Option<std::collections::BTreeSet<u16>> = cid
        .as_ref()
        .map(|cid| cid.widths.iter().map(|(gid, _)| *gid).collect());
    let font_program: Option<Vec<u8>> = match (embedded, used_gids.as_ref()) {
        (Some(font), Some(gids)) => font.subset(gids),
        _ => None,
    };
    let base_font_name: Option<String> = embedded.map(|font| {
        match (&font_program, used_gids.as_ref()) {
            (Some(_), Some(gids)) => format!("{}+{}", subset_tag(gids), font.postscript_name),
            _ => font.postscript_name.clone(),
        }
    });

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

    // The font object (id 3, referenced as /F1 by every page). With no embedded
    // font it is the standard-14 Helvetica; with one it is a Type0 composite
    // whose descendant CIDFont is written after the pages.
    match embedded {
        None => writer.object(
            font_id,
            "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>",
        ),
        Some(_) => {
            writer.object(
                font_id,
                &format!(
                    concat!(
                        "<< /Type /Font /Subtype /Type0 /BaseFont /{name} ",
                        "/Encoding /Identity-H /DescendantFonts [{desc} 0 R] ",
                        "/ToUnicode {tu} 0 R >>"
                    ),
                    name = base_font_name.as_deref().unwrap_or(""),
                    desc = descendant_id,
                    tu = tounicode_id
                ),
            );
        }
    }

    for (index, page) in pages.iter().enumerate() {
        let page_id = first_page_id + (index * 2);
        let content_id = page_id + 1;
        let content = page_content(page, embedded);

        writer.object(
            page_id,
            &format!(
                concat!(
                    "<< /Type /Page /Parent 2 0 R ",
                    "/MediaBox [0 0 {:.2} {:.2}] ",
                    "/Resources << /Font << /F1 3 0 R >>{} >> ",
                    "/Contents {} 0 R >>"
                ),
                options.page_size.width,
                options.page_size.height,
                xobject_resources,
                content_id
            ),
        );

        writer.stream_object(content_id, content.as_bytes())?;
    }

    // The embedded font's descendant CIDFont, descriptor, program, and the
    // ToUnicode CMap, after the pages.
    if let (Some(font), Some(cid)) = (embedded, cid.as_ref()) {
        let name = base_font_name.as_deref().unwrap_or(&font.postscript_name);
        let w_array = cid
            .widths
            .iter()
            .map(|(gid, width)| format!("{gid} [{width}]"))
            .collect::<Vec<_>>()
            .join(" ");
        writer.object(
            descendant_id,
            &format!(
                concat!(
                    "<< /Type /Font /Subtype /CIDFontType2 /BaseFont /{name} ",
                    "/CIDSystemInfo << /Registry (Adobe) /Ordering (Identity) /Supplement 0 >> ",
                    "/FontDescriptor {desc} 0 R /CIDToGIDMap /Identity /DW 1000 ",
                    "/W [{w}] >>"
                ),
                name = name,
                desc = descriptor_id,
                w = w_array
            ),
        );

        writer.object(
            descriptor_id,
            &format!(
                concat!(
                    "<< /Type /FontDescriptor /FontName /{} /Flags {} ",
                    "/FontBBox [{} {} {} {}] /ItalicAngle {:.2} ",
                    "/Ascent {} /Descent {} /CapHeight {} /StemV {} ",
                    "/FontFile2 {} 0 R >>"
                ),
                name,
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
        // Retain-GIDs keeps the /W, /ToUnicode, and Identity map valid against the
        // (possibly subset) program.
        let program = font_program.as_deref().unwrap_or(font.data());
        writer.font_file_object(fontfile_id, program)?;
        writer.stream_object(tounicode_id, to_unicode_cmap(cid).as_bytes())?;
    }

    // Image XObjects (and their soft masks). JPEG data passes through as
    // `DCTDecode`; decoded samples are `FlateDecode`-compressed here.
    for (image, plan) in images.iter().zip(&image_plans) {
        if let (Some(smask_id), Some(alpha)) = (plan.smask_id, image.smask.as_ref()) {
            let dict = format!(
                "/Type /XObject /Subtype /Image /Width {} /Height {} \
                 /ColorSpace /DeviceGray /BitsPerComponent 8 /Filter /FlateDecode",
                image.width, image.height
            );
            writer.stream_with_dict(smask_id, &dict, &compress_stream(alpha)?);
        }

        let mut dict = format!(
            "/Type /XObject /Subtype /Image /Width {} /Height {} \
             /ColorSpace /{} /BitsPerComponent {} /Filter /{}",
            image.width,
            image.height,
            image.color_space.pdf_name(),
            image.bits_per_component,
            image.filter.pdf_name(),
        );
        if let Some(smask_id) = plan.smask_id {
            dict.push_str(&format!(" /SMask {smask_id} 0 R"));
        }

        let body = match image.filter {
            ImageFilter::Dct => image.data.clone(),
            ImageFilter::Flate => compress_stream(&image.data)?,
        };
        writer.stream_with_dict(plan.object_id, &dict, &body);
    }

    writer.finish(catalog_id, object_count);
    Ok(writer.into_bytes())
}

/// Object ids assigned to one embedded image and its optional soft mask.
struct ImagePlan {
    name: String,
    object_id: usize,
    smask_id: Option<usize>,
}

/// A deterministic six-uppercase-letter subset tag (the `ABCDEF+` prefix PDF
/// readers use to recognize a subset font), derived from the used glyph ids.
fn subset_tag(used_gids: &std::collections::BTreeSet<u16>) -> String {
    // FNV-1a over the sorted glyph ids, then base-26 into six letters.
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for gid in used_gids {
        hash ^= u64::from(*gid);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    let mut tag = String::with_capacity(6);
    for _ in 0..6 {
        tag.push((b'A' + (hash % 26) as u8) as char);
        hash /= 26;
    }
    tag
}

/// Build a `/ToUnicode` CMap mapping glyph ids back to Unicode, so text in a
/// Type0/Identity-H font stays selectable and searchable.
fn to_unicode_cmap(cid: &crate::font::CidLayout) -> String {
    let mut cmap = String::from(
        "/CIDInit /ProcSet findresource begin\n\
         12 dict begin\n\
         begincmap\n\
         /CIDSystemInfo << /Registry (Adobe) /Ordering (UCS) /Supplement 0 >> def\n\
         /CMapName /Adobe-Identity-UCS def\n\
         /CMapType 2 def\n\
         1 begincodespacerange\n<0000> <FFFF>\nendcodespacerange\n",
    );

    // `beginbfchar` blocks are limited to 100 entries each by the spec. A
    // destination may be several characters (a ligature glyph maps to them all).
    let entries: Vec<(u16, &String)> = cid.gid_to_unicode.iter().map(|(&g, c)| (g, c)).collect();
    for chunk in entries.chunks(100) {
        cmap.push_str(&format!("{} beginbfchar\n", chunk.len()));
        for (gid, chars) in chunk {
            let dest: String = chars.chars().map(utf16_be_hex).collect();
            cmap.push_str(&format!("<{:04X}> <{}>\n", gid, dest));
        }
        cmap.push_str("endbfchar\n");
    }

    cmap.push_str("endcmap\nCMapName currentdict /CMap defineresource pop\nend\nend\n");
    cmap
}

/// Hex-encode a character as UTF-16BE (a surrogate pair for astral scalars),
/// as required by `/ToUnicode` destination strings.
fn utf16_be_hex(ch: char) -> String {
    let mut buf = [0u16; 2];
    ch.encode_utf16(&mut buf)
        .iter()
        .map(|unit| format!("{unit:04X}"))
        .collect()
}

fn compress_stream(stream: &[u8]) -> Result<Vec<u8>, PdfError> {
    let mut encoder = ZlibEncoder::new(Vec::new(), Compression::fast());
    encoder.write_all(stream).map_err(PdfError::Compression)?;
    encoder.finish().map_err(PdfError::Compression)
}

fn page_content(page: &Page, embedded: Option<&crate::font::TrueTypeFont>) -> String {
    let mut content = String::new();
    // Track the current fill color so faux-bold text can stroke in the same color.
    let mut fill = (0.0f32, 0.0f32, 0.0f32);

    for command in &page.commands {
        match command {
            PaintCommand::SetFillColor(color) => {
                fill = (color.r, color.g, color.b);
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
            PaintCommand::SetLineWidth(width) => {
                content.push_str(&format!("{width:.3} w\n"));
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
            PaintCommand::Image(image) => {
                // Image space is the unit square; the CTM scales/positions it so
                // the image fills the box with lower-left corner (x, y).
                content.push_str("q\n");
                content.push_str(&format!(
                    "{:.2} 0 0 {:.2} {:.2} {:.2} cm\n",
                    image.width, image.height, image.x, image.y
                ));
                content.push_str(&format!("/Im{} Do\n", image.image_index));
                content.push_str("Q\n");
            }
            PaintCommand::Text(text) => {
                content.push_str("BT\n");
                content.push_str(&format!("/F1 {:.2} Tf\n", text.font_size));
                if text.bold {
                    // Faux-bold: fill + stroke the glyphs in the same color with a
                    // thin outline, since only a regular font face is embedded.
                    content.push_str(&format!("{:.4} {:.4} {:.4} RG\n", fill.0, fill.1, fill.2));
                    content.push_str(&format!("{:.3} w 2 Tr\n", text.font_size * 0.03));
                }
                content.push_str(&format!("{:.2} {:.2} Td\n", text.x, text.y));
                match embedded {
                    // Embedded Type0/Identity-H font: write the *shaped* glyph
                    // ids (ligatures substituted) as a TJ array whose numeric
                    // adjustments reproduce kerning: the viewer advances each
                    // glyph by its natural /W width, so the correction is
                    // (natural - shaped) in 1/1000 em.
                    Some(font) => {
                        let run = font.shape(&text.text);
                        content.push_str("[<");
                        for glyph in &run.glyphs {
                            content.push_str(&format!("{:04X}", glyph.gid));
                            let adjust =
                                ((glyph.natural_em - glyph.advance_em) * 1000.0).round() as i32;
                            if adjust != 0 {
                                content.push_str(&format!("> {adjust} <"));
                            }
                        }
                        content.push_str(">] TJ\n");
                    }
                    // Standard-14 Helvetica: a WinAnsi literal string.
                    None => {
                        content.push_str(&format!("({}) Tj\n", escape_text(&text.text)));
                    }
                }
                if text.bold {
                    content.push_str("0 Tr\n"); // restore normal fill render mode
                }
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
            // Printable ASCII is written literally.
            _ if ch.is_ascii_graphic() || ch == ' ' => output.push(ch),
            // Other characters are emitted as their WinAnsi byte via an octal
            // escape (the font is declared `/WinAnsiEncoding`), so Latin-1 text
            // and CP1252 specials (bullet, dashes, curly quotes, accents) render
            // instead of becoming `?`.
            _ => match crate::font::char_to_winansi(ch) {
                Some(byte) => output.push_str(&format!("\\{byte:03o}")),
                None => output.push('?'),
            },
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

    /// A stream object with a caller-supplied dictionary body (which must not
    /// include `/Length`, appended here). `body` is written verbatim, so the
    /// caller controls any filtering.
    fn stream_with_dict(&mut self, id: usize, dict: &str, body: &[u8]) {
        self.start_object(id);
        self.bytes.extend_from_slice(
            format!("<< {dict} /Length {} >>\nstream\n", body.len()).as_bytes(),
        );
        self.bytes.extend_from_slice(body);
        self.bytes.extend_from_slice(b"\nendstream\nendobj\n");
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

    use super::{escape_text, page_content, to_unicode_cmap, utf16_be_hex, write_pdf};

    #[test]
    fn utf16_be_hex_encodes_bmp_and_astral() {
        assert_eq!(utf16_be_hex('A'), "0041");
        assert_eq!(utf16_be_hex('世'), "4E16");
        // Astral scalar -> UTF-16 surrogate pair.
        assert_eq!(utf16_be_hex('\u{10348}'), "D800DF48");
    }

    #[test]
    fn to_unicode_cmap_maps_glyphs_back_to_text() {
        let mut gid_to_unicode = std::collections::BTreeMap::new();
        gid_to_unicode.insert(36u16, "A".to_string());
        gid_to_unicode.insert(0x4E16u16, "世".to_string());
        let cid = crate::font::CidLayout {
            widths: Vec::new(),
            gid_to_unicode,
        };

        let cmap = to_unicode_cmap(&cid);
        assert!(cmap.contains("/CMapType 2"));
        assert!(cmap.contains("2 beginbfchar"));
        assert!(cmap.contains("<0024> <0041>")); // gid 36 -> 'A'
        assert!(cmap.contains("<4E16> <4E16>")); // gid 0x4E16 -> '世'
        assert!(cmap.contains("endcmap"));
    }

    #[test]
    fn embeds_type0_font_when_a_face_is_available() {
        // Use any system TrueType we can find; skip cleanly if none resolves
        // (keeps the test deterministic on machines without that font).
        let candidates = [
            "/System/Library/Fonts/Supplemental/Arial.ttf",
            "/Library/Fonts/Arial.ttf",
        ];
        let Some(path) = candidates.iter().find(|p| std::path::Path::new(p).is_file()) else {
            return;
        };

        let options = RenderOptions::default()
            .with_font(&crate::font::FontSource::Path((*path).into()))
            .expect("load font");
        let mut page = Page::new();
        page.push_colored_line(
            Line {
                text: "Hi".to_string(),
                x: 48.0,
                y: 700.0,
                font_size: 12.0,
                leading: 16.0,
            },
            Color::BLACK, false,
        );

        let pdf = write_pdf(&[page], &[], &options).expect("render");
        let text = String::from_utf8_lossy(&pdf);
        assert!(text.contains("/Subtype /Type0"));
        assert!(text.contains("/Encoding /Identity-H"));
        assert!(text.contains("/Subtype /CIDFontType2"));
        assert!(text.contains("/CIDToGIDMap /Identity"));
        assert!(text.contains("/ToUnicode"));
    }

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
            Color::BLACK, false,
        );

        assert_eq!(escape_text("A (test) \\ value"), "A \\(test\\) \\\\ value");

        let pdf = write_pdf(&[page], &[], &RenderOptions::default()).expect("pdf should render");
        assert!(pdf
            .windows(b"/Filter /FlateDecode".len())
            .any(|window| window == b"/Filter /FlateDecode"));
    }

    #[test]
    fn encodes_winansi_specials_as_octal() {
        // Bullet (0x95), en dash (0x96), right curly quote (0x92), é (0xE9 Latin-1).
        assert_eq!(escape_text("\u{2022}"), "\\225");
        assert_eq!(escape_text("\u{2013}"), "\\226");
        assert_eq!(escape_text("\u{2019}"), "\\222");
        assert_eq!(escape_text("é"), "\\351");
        // A character outside WinAnsi still degrades to `?`.
        assert_eq!(escape_text("\u{4E2D}"), "?");
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
                bold: false,
            }));
        page.commands.push(PaintCommand::PopClip);

        let content = page_content(&page, None);

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

        let content = page_content(&page, None);

        assert!(content.contains("1.0000 0.0000 0.0000 rg\n"));
        assert!(content.contains("0.0000 0.0000 1.0000 RG\n"));
    }
}
