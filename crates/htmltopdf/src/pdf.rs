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

    // Discover which faces actually paint text: each text command's run font
    // (per-element `font-family`/bold/italic resolution), further split by its
    // fallback chain for characters it lacks. Faces are deduplicated by
    // identity in first-use order (deterministic), so two specs resolving to
    // the same file share one /Fn resource. The primary is always F1 (a page
    // may paint no text at all, but resources still declare F1).
    let mut face_order: Vec<std::sync::Arc<crate::font::Font>> = Vec::new();
    let mut face_index: std::collections::HashMap<usize, usize> = std::collections::HashMap::new();
    let mut used_texts: Vec<std::collections::BTreeSet<&str>> = Vec::new();
    let mut slot = |font: &std::sync::Arc<crate::font::Font>,
                    face_order: &mut Vec<std::sync::Arc<crate::font::Font>>,
                    used_texts: &mut Vec<std::collections::BTreeSet<&str>>|
     -> usize {
        let key = std::sync::Arc::as_ptr(font) as usize;
        *face_index.entry(key).or_insert_with(|| {
            face_order.push(font.clone());
            used_texts.push(std::collections::BTreeSet::new());
            face_order.len() - 1
        })
    };
    slot(&options.font, &mut face_order, &mut used_texts);
    for text in pages.iter().flat_map(|page| page.commands.iter()).filter_map(|c| match c {
        PaintCommand::Text(text) => Some(text),
        _ => None,
    }) {
        let base = options.run_font(text.font);
        match base.segment_by_coverage(&text.text) {
            None => {
                let index = slot(base, &mut face_order, &mut used_texts);
                used_texts[index].insert(text.text.as_str());
            }
            Some(segments) => {
                let chain = base.fallback_chain();
                for (chain_index, segment) in segments {
                    let face = if chain_index == 0 { base } else { &chain[chain_index - 1] };
                    let index = slot(face, &mut face_order, &mut used_texts);
                    used_texts[index].insert(segment);
                }
            }
        }
    }

    // One plan per used face: resource name, object ids, and (for embedded
    // faces) the shaped CID layout, subset program, and PDF names.
    let fonts_start = 3; // catalog 1, pages tree 2
    let first_page_id = fonts_start + face_order.len();
    let extras_start = first_page_id + page_count * 2;
    let mut next_extra = extras_start;
    let mut font_plans: Vec<FontPlan> = Vec::with_capacity(face_order.len());
    for (ordinal, (font, texts)) in face_order.iter().zip(&used_texts).enumerate() {
        let font = font.clone();
        let cid = font
            .embedding()
            .map(|embedded| embedded.shaped_cid_layout(texts.iter().copied()));
        // Subset to the used glyphs when possible (retain-GIDs keeps /W,
        // /ToUnicode, and the Identity map valid); fall back to the full
        // program. A subset font gets the `ABCDEF+` name prefix readers expect.
        let used_gids: Option<std::collections::BTreeSet<u16>> = cid
            .as_ref()
            .map(|cid| cid.widths.iter().map(|(gid, _)| *gid).collect());
        let program = match (font.embedding(), used_gids.as_ref()) {
            (Some(embedded), Some(gids)) => embedded.subset(gids),
            _ => None,
        };
        let base_name = font.embedding().map(|embedded| match (&program, used_gids.as_ref()) {
            (Some(_), Some(gids)) => format!("{}+{}", subset_tag(gids), embedded.postscript_name),
            _ => embedded.postscript_name.clone(),
        });
        let ids = if font.embedding().is_some() {
            let ids = [next_extra, next_extra + 1, next_extra + 2, next_extra + 3];
            next_extra += 4;
            Some(ids)
        } else {
            None
        };
        font_plans.push(FontPlan {
            resource: format!("F{}", ordinal + 1),
            font_id: fonts_start + ordinal,
            font,
            cid,
            program,
            base_name,
            extra_ids: ids,
        });
    }

    // Each embedded image is one XObject, plus one more for its soft mask when
    // it carries alpha. Assign object ids after the font extras.
    let mut next_id = next_extra;
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

    // Interactive features. Named destinations (HTML `id` anchors) resolve
    // `#fragment` links; every laid-out link area becomes a /Link annotation
    // (URI action for external targets, an in-document /Dest for fragments);
    // headings become the /Outlines bookmark tree.
    let mut named_dests: std::collections::HashMap<&str, (usize, f32)> =
        std::collections::HashMap::new();
    for (page_index, page) in pages.iter().enumerate() {
        for anchor in &page.anchors {
            if let Some(name) = &anchor.name {
                named_dests.entry(name.as_str()).or_insert((page_index, anchor.y));
            }
        }
    }
    let page_object_id = |page_index: usize| first_page_id + page_index * 2;
    let mut annots_per_page: Vec<Vec<AnnotPlan>> = Vec::with_capacity(page_count);
    for page in pages {
        let mut annots = Vec::new();
        for area in &page.link_areas {
            let Some(target) = (area.link as usize)
                .checked_sub(1)
                .and_then(|index| options.links.get(index))
            else {
                continue;
            };
            let target = match target.strip_prefix('#') {
                Some(fragment) => match named_dests.get(fragment) {
                    // Fragment links only annotate when the anchor exists.
                    Some(&(page_index, y)) => LinkTarget::Dest {
                        page_object: page_object_id(page_index),
                        y,
                    },
                    None => continue,
                },
                None if target.is_empty() => continue,
                None => LinkTarget::Uri(target),
            };
            annots.push(AnnotPlan {
                object_id: next_id,
                rect: [area.x, area.y, area.x + area.width, area.y + area.height],
                target,
            });
            next_id += 1;
        }
        annots_per_page.push(annots);
    }

    // Outline entries in document order, with each entry's parent being the
    // closest preceding entry of a shallower level (h2 nests under h1, …).
    let entries: Vec<OutlineEntry> = pages
        .iter()
        .enumerate()
        .flat_map(|(page_index, page)| {
            page.anchors
                .iter()
                .filter(|anchor| anchor.level > 0 && !anchor.title.is_empty())
                .map(move |anchor| (page_index, anchor))
        })
        .map(|(page_index, anchor)| OutlineEntry {
            level: anchor.level,
            title: anchor.title.clone(),
            page_object: page_object_id(page_index),
            y: anchor.y,
            parent: None,
            children: Vec::new(),
        })
        .collect();
    let outline = build_outline_tree(entries);
    let outline_root_id = if outline.is_empty() {
        None
    } else {
        let id = next_id;
        next_id += 1 + outline.len();
        Some(id)
    };

    let object_count = next_id - 1;

    if object_count > u16::MAX as usize {
        return Err(PdfError::TooManyObjects);
    }

    let catalog_id = 1;
    let pages_id = 2;

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
    let font_resources = font_plans
        .iter()
        .map(|plan| format!("/{} {} 0 R", plan.resource, plan.font_id))
        .collect::<Vec<_>>()
        .join(" ");

    let mut writer = PdfWriter::new();
    writer.write_header();

    match outline_root_id {
        Some(root) => writer.object(
            catalog_id,
            &format!("<< /Type /Catalog /Pages 2 0 R /Outlines {root} 0 R >>"),
        ),
        None => writer.object(catalog_id, "<< /Type /Catalog /Pages 2 0 R >>"),
    }

    let kids = (0..page_count)
        .map(|index| format!("{} 0 R", first_page_id + (index * 2)))
        .collect::<Vec<_>>()
        .join(" ");
    writer.object(
        pages_id,
        &format!("<< /Type /Pages /Kids [{kids}] /Count {page_count} >>"),
    );

    // The base font objects (/F1, /F2, …): standard-14 Helvetica as Type1, or
    // a Type0 composite whose descendant objects are written after the pages.
    for plan in &font_plans {
        match plan.extra_ids {
            None => writer.object(
                plan.font_id,
                "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>",
            ),
            Some([descendant_id, _, _, tounicode_id]) => {
                writer.object(
                    plan.font_id,
                    &format!(
                        concat!(
                            "<< /Type /Font /Subtype /Type0 /BaseFont /{name} ",
                            "/Encoding /Identity-H /DescendantFonts [{desc} 0 R] ",
                            "/ToUnicode {tu} 0 R >>"
                        ),
                        name = plan.base_name.as_deref().unwrap_or(""),
                        desc = descendant_id,
                        tu = tounicode_id
                    ),
                );
            }
        }
    }

    for (index, page) in pages.iter().enumerate() {
        let page_id = first_page_id + (index * 2);
        let content_id = page_id + 1;
        let content = page_content(page, options, &font_plans);

        let annots = if annots_per_page[index].is_empty() {
            String::new()
        } else {
            let ids = annots_per_page[index]
                .iter()
                .map(|plan| format!("{} 0 R", plan.object_id))
                .collect::<Vec<_>>()
                .join(" ");
            format!("/Annots [{ids}] ")
        };

        writer.object(
            page_id,
            &format!(
                concat!(
                    "<< /Type /Page /Parent 2 0 R ",
                    "/MediaBox [0 0 {:.2} {:.2}] ",
                    "/Resources << /Font << {} >>{} >> ",
                    "{}/Contents {} 0 R >>"
                ),
                options.page_size.width,
                options.page_size.height,
                font_resources,
                xobject_resources,
                annots,
                content_id
            ),
        );

        writer.stream_object(content_id, content.as_bytes())?;
    }

    // Each embedded font's descendant CIDFont, descriptor, program, and
    // ToUnicode CMap, after the pages.
    for plan in &font_plans {
        let (Some(font), Some(cid), Some([descendant_id, descriptor_id, fontfile_id, tounicode_id])) =
            (plan.font.embedding(), plan.cid.as_ref(), plan.extra_ids)
        else {
            continue;
        };
        let name = plan.base_name.as_deref().unwrap_or(&font.postscript_name);
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
        let program = plan.program.as_deref().unwrap_or(font.data());
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

    // Link annotations, page by page.
    for plans in &annots_per_page {
        for plan in plans {
            let rect = format!(
                "[{:.2} {:.2} {:.2} {:.2}]",
                plan.rect[0], plan.rect[1], plan.rect[2], plan.rect[3]
            );
            let body = match &plan.target {
                LinkTarget::Uri(uri) => format!(
                    "<< /Type /Annot /Subtype /Link /Rect {rect} /Border [0 0 0] \
                     /A << /S /URI /URI ({}) >> >>",
                    escape_string_bytes(uri)
                ),
                LinkTarget::Dest { page_object, y } => format!(
                    "<< /Type /Annot /Subtype /Link /Rect {rect} /Border [0 0 0] \
                     /Dest [{page_object} 0 R /XYZ null {y:.2} null] >>"
                ),
            };
            writer.object(plan.object_id, &body);
        }
    }

    // The outline (bookmark) tree: a root plus one item per heading, all open.
    if let Some(root_id) = outline_root_id {
        let item_id = |index: usize| root_id + 1 + index;
        let top: Vec<usize> = (0..outline.len()).filter(|&i| outline[i].parent.is_none()).collect();
        writer.object(
            root_id,
            &format!(
                "<< /Type /Outlines /First {} 0 R /Last {} 0 R /Count {} >>",
                item_id(*top.first().expect("outline is non-empty")),
                item_id(*top.last().expect("outline is non-empty")),
                outline.len()
            ),
        );
        for (index, entry) in outline.iter().enumerate() {
            let siblings: Vec<usize> = (0..outline.len())
                .filter(|&i| outline[i].parent == entry.parent)
                .collect();
            let position = siblings.iter().position(|&i| i == index).expect("entry is its own sibling");
            let mut body = format!(
                "<< /Title {} /Parent {} 0 R",
                pdf_text_string(&entry.title),
                entry.parent.map(item_id).unwrap_or(root_id)
            );
            if position > 0 {
                body.push_str(&format!(" /Prev {} 0 R", item_id(siblings[position - 1])));
            }
            if position + 1 < siblings.len() {
                body.push_str(&format!(" /Next {} 0 R", item_id(siblings[position + 1])));
            }
            if let (Some(&first), Some(&last)) = (entry.children.first(), entry.children.last()) {
                body.push_str(&format!(
                    " /First {} 0 R /Last {} 0 R /Count {}",
                    item_id(first),
                    item_id(last),
                    descendant_count(&outline, index)
                ));
            }
            body.push_str(&format!(
                " /Dest [{} 0 R /XYZ null {:.2} null] >>",
                entry.page_object, entry.y
            ));
            writer.object(item_id(index), &body);
        }
    }

    writer.finish(catalog_id, object_count);
    Ok(writer.into_bytes())
}

/// One planned `/Link` annotation: its object id, rectangle (PDF page space),
/// and resolved target.
struct AnnotPlan<'a> {
    object_id: usize,
    rect: [f32; 4],
    target: LinkTarget<'a>,
}

enum LinkTarget<'a> {
    /// An external URI action.
    Uri(&'a str),
    /// An in-document destination (a resolved `#fragment`).
    Dest { page_object: usize, y: f32 },
}

/// One outline (bookmark) item: a heading with its destination, linked into
/// the tree by `build_outline_tree`.
struct OutlineEntry {
    level: u8,
    title: String,
    page_object: usize,
    y: f32,
    /// Index of the parent entry (`None` = a top-level item under the root).
    parent: Option<usize>,
    /// Indices of direct children, in document order.
    children: Vec<usize>,
}

/// Link the flat, document-ordered heading list into a tree: each entry nests
/// under the closest preceding entry with a shallower level (an `<h3>` after
/// an `<h1>` nests directly under it; a skipped level doesn't break nesting).
fn build_outline_tree(mut entries: Vec<OutlineEntry>) -> Vec<OutlineEntry> {
    let mut stack: Vec<usize> = Vec::new();
    for index in 0..entries.len() {
        while let Some(&top) = stack.last() {
            if entries[top].level >= entries[index].level {
                stack.pop();
            } else {
                break;
            }
        }
        if let Some(&parent) = stack.last() {
            entries[index].parent = Some(parent);
            entries[parent].children.push(index);
        }
        stack.push(index);
    }
    entries
}

/// Total descendants of an outline entry (its open `/Count`).
fn descendant_count(entries: &[OutlineEntry], index: usize) -> usize {
    entries[index]
        .children
        .iter()
        .map(|&child| 1 + descendant_count(entries, child))
        .sum()
}

/// Encode a human-readable string for a PDF text-string context (`/Title`):
/// ASCII goes out as an escaped literal, anything else as UTF-16BE hex with a
/// byte-order mark.
fn pdf_text_string(text: &str) -> String {
    if text.is_ascii() {
        format!("({})", escape_string_bytes(text))
    } else {
        let hex: String = std::iter::once('\u{FEFF}')
            .chain(text.chars())
            .map(utf16_be_hex)
            .collect();
        format!("<{hex}>")
    }
}

/// Escape a string for a PDF literal `(...)`, passing its UTF-8 bytes through
/// verbatim (no WinAnsi mapping — used for URIs and ASCII titles).
fn escape_string_bytes(text: &str) -> String {
    let mut output = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            '(' => output.push_str("\\("),
            ')' => output.push_str("\\)"),
            '\\' => output.push_str("\\\\"),
            '\r' => output.push_str("\\r"),
            '\n' => output.push_str("\\n"),
            _ => output.push(ch),
        }
    }
    output
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

/// One font resource in the output PDF: which chain slot it came from, its
/// `/Fn` resource name and object id, and (for embedded faces) the shaped CID
/// layout, subset program, PDF base name, and the four extra object ids
/// (descendant, descriptor, font file, ToUnicode).
struct FontPlan {
    resource: String,
    font_id: usize,
    font: std::sync::Arc<crate::font::Font>,
    cid: Option<crate::font::CidLayout>,
    program: Option<Vec<u8>>,
    base_name: Option<String>,
    extra_ids: Option<[usize; 4]>,
}

/// Append one text segment in `plan`'s font: shaped `TJ` glyph runs for an
/// embedded face (kerning as numeric adjustments), a WinAnsi literal for the
/// standard-14 fallback. The PDF text position advances across show operators,
/// so consecutive segments continue on the same baseline.
fn push_text_segment(content: &mut String, plan: &FontPlan, segment: &str) {
    match plan.font.embedding() {
        Some(font) => {
            let run = font.shape(segment);
            content.push_str("[<");
            for glyph in &run.glyphs {
                content.push_str(&format!("{:04X}", glyph.gid));
                let adjust = ((glyph.natural_em - glyph.advance_em) * 1000.0).round() as i32;
                if adjust != 0 {
                    content.push_str(&format!("> {adjust} <"));
                }
            }
            content.push_str(">] TJ\n");
        }
        None => {
            content.push_str(&format!("({}) Tj\n", escape_text(segment)));
        }
    }
}

/// Build the path operators for a rectangle with uniformly rounded corners:
/// four edge segments joined by quarter-circle Bézier arcs (the standard
/// circle approximation constant k ≈ 0.5523). `(x, y)` is the lower-left
/// corner; the caller appends the painting operator (`S` / `f`).
fn rounded_rect_path(rect: &crate::paint::RoundedRectCommand) -> String {
    let (x, y, w, h) = (rect.x, rect.y, rect.width, rect.height);
    let r = rect.radius.min(w / 2.0).min(h / 2.0).max(0.0);
    let k = 0.552_284_75 * r;
    let mut path = String::new();
    path.push_str(&format!("{:.2} {:.2} m\n", x + r, y));
    path.push_str(&format!("{:.2} {:.2} l\n", x + w - r, y));
    path.push_str(&format!(
        "{:.2} {:.2} {:.2} {:.2} {:.2} {:.2} c\n",
        x + w - r + k, y, x + w, y + r - k, x + w, y + r
    ));
    path.push_str(&format!("{:.2} {:.2} l\n", x + w, y + h - r));
    path.push_str(&format!(
        "{:.2} {:.2} {:.2} {:.2} {:.2} {:.2} c\n",
        x + w, y + h - r + k, x + w - r + k, y + h, x + w - r, y + h
    ));
    path.push_str(&format!("{:.2} {:.2} l\n", x + r, y + h));
    path.push_str(&format!(
        "{:.2} {:.2} {:.2} {:.2} {:.2} {:.2} c\n",
        x + r - k, y + h, x, y + h - r + k, x, y + h - r
    ));
    path.push_str(&format!("{:.2} {:.2} l\n", x, y + r));
    path.push_str(&format!(
        "{:.2} {:.2} {:.2} {:.2} {:.2} {:.2} c\n",
        x, y + r - k, x + r - k, y, x + r, y
    ));
    path.push_str("h\n");
    path
}

fn page_content(page: &Page, options: &RenderOptions, font_plans: &[FontPlan]) -> String {
    // Faces are deduplicated by identity when plans are built, so a linear
    // identity scan (over a handful of plans) resolves any face to its plan.
    let plan_of = |font: &std::sync::Arc<crate::font::Font>| -> &FontPlan {
        let key = std::sync::Arc::as_ptr(font) as usize;
        font_plans
            .iter()
            .find(|plan| std::sync::Arc::as_ptr(&plan.font) as usize == key)
            .unwrap_or(&font_plans[0])
    };
    // With a single font in play, skip per-command segmentation entirely.
    let single_font = font_plans.len() == 1;

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
            PaintCommand::SetDash(pattern) => match pattern {
                Some(dash) => {
                    content.push_str(&format!("[{:.2} {:.2}] 0 d\n", dash.on, dash.off));
                }
                None => content.push_str("[] 0 d\n"),
            },
            PaintCommand::StrokeRoundedRect(rect) => {
                content.push_str(&rounded_rect_path(rect));
                content.push_str("S\n");
            }
            PaintCommand::FillRoundedRect(rect) => {
                content.push_str(&rounded_rect_path(rect));
                content.push_str("f\n");
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
                if text.letter_spacing != 0.0 {
                    // `Tc` pads every shown glyph (2-byte CIDs included). It is
                    // graphics state, not text-object state, so reset after ET.
                    content.push_str(&format!("{:.3} Tc\n", text.letter_spacing));
                }
                if text.bold {
                    // Faux-bold: fill + stroke the glyphs in the same color with a
                    // thin outline, since only a regular font face is embedded.
                    content.push_str(&format!("{:.4} {:.4} {:.4} RG\n", fill.0, fill.1, fill.2));
                    content.push_str(&format!("{:.3} w 2 Tr\n", text.font_size * 0.03));
                }
                // Segment the string by font coverage (the run's own face
                // first, then its fallback chain); each segment selects its
                // /Fn resource and the text position flows across switches.
                let base = options.run_font(text.font);
                let segments = if single_font {
                    None
                } else {
                    base.segment_by_coverage(&text.text)
                };
                match segments {
                    None => {
                        let plan = plan_of(base);
                        content.push_str(&format!("/{} {:.2} Tf\n", plan.resource, text.font_size));
                        content.push_str(&format!("{:.2} {:.2} Td\n", text.x, text.y));
                        push_text_segment(&mut content, plan, &text.text);
                    }
                    Some(segments) => {
                        content.push_str(&format!("{:.2} {:.2} Td\n", text.x, text.y));
                        let chain = base.fallback_chain();
                        let mut current = usize::MAX;
                        for (chain_index, segment) in segments {
                            let face = if chain_index == 0 { base } else { &chain[chain_index - 1] };
                            let plan = plan_of(face);
                            if plan.font_id != current {
                                content.push_str(&format!(
                                    "/{} {:.2} Tf\n",
                                    plan.resource, text.font_size
                                ));
                                current = plan.font_id;
                            }
                            push_text_segment(&mut content, plan, segment);
                        }
                    }
                }
                if text.bold {
                    content.push_str("0 Tr\n"); // restore normal fill render mode
                }
                if text.letter_spacing != 0.0 {
                    content.push_str("0 Tc\n");
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

    use super::{escape_text, page_content, to_unicode_cmap, utf16_be_hex, write_pdf, FontPlan};

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
                font: 0,
                leading: 16.0,
            },
            Color::BLACK, false, 0.0,
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
    fn writes_link_annotations_and_outline() {
        use crate::layout::{AnchorMark, LinkArea};

        let mut first = Page::new();
        first.anchors.push(AnchorMark {
            name: Some("top".to_string()),
            level: 1,
            title: "Chapter One".to_string(),
            y: 780.0,
        });
        first.anchors.push(AnchorMark {
            name: None,
            level: 2,
            title: "Sección Única".to_string(), // non-ASCII → UTF-16BE title
            y: 700.0,
        });
        let mut second = Page::new();
        // One external link, one live fragment, one dead fragment (dropped).
        for (index, link) in [1u16, 2, 3].into_iter().enumerate() {
            second.link_areas.push(LinkArea {
                x: 48.0,
                y: 600.0 - 20.0 * index as f32,
                width: 90.0,
                height: 12.0,
                link,
            });
        }
        let options = RenderOptions {
            links: vec![
                "https://example.com/x".to_string(),
                "#top".to_string(),
                "#missing".to_string(),
            ],
            ..Default::default()
        };

        let pdf = write_pdf(&[first, second], &[], &options).expect("render");
        let text = String::from_utf8_lossy(&pdf);

        // Two annotations on page 2 (the dead fragment gets none).
        assert!(text.contains("/Subtype /Link"));
        assert!(text.contains("/A << /S /URI /URI (https://example.com/x) >>"));
        // Page 1 is object 3 (no fonts beyond F1 at id 3 → first page id).
        assert!(text.contains("/XYZ null 780.00 null"));
        assert_eq!(text.matches("/Subtype /Link").count(), 2);

        // The outline: root in the catalog, h2 nested under h1, UTF-16 title.
        assert!(text.contains("/Outlines"));
        assert!(text.contains("/Title (Chapter One)"));
        assert!(text.contains("/Count 1"));
        assert!(text.contains("/Title <FEFF")); // Sección Única
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
                font: 0,
                leading: 16.0,
            },
            Color::BLACK, false, 0.0,
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
                letter_spacing: 0.0,
                text: "inside".to_string(),
                x: 12.0,
                y: 45.0,
                font_size: 10.0,
                font: 0,
                bold: false,
            }));
        page.commands.push(PaintCommand::PopClip);

        let primary = std::sync::Arc::new(crate::font::Font::helvetica());
        let plans = vec![FontPlan {
            resource: "F1".to_string(),
            font_id: 3,
            font: primary.clone(),
            cid: None,
            program: None,
            base_name: None,
            extra_ids: None,
        }];
        let options = RenderOptions {
            font: primary.clone(),
            ..RenderOptions::default()
        };
        let content = page_content(&page, &options, &plans);

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

        let primary = std::sync::Arc::new(crate::font::Font::helvetica());
        let plans = vec![FontPlan {
            resource: "F1".to_string(),
            font_id: 3,
            font: primary.clone(),
            cid: None,
            program: None,
            base_name: None,
            extra_ids: None,
        }];
        let options = RenderOptions {
            font: primary.clone(),
            ..RenderOptions::default()
        };
        let content = page_content(&page, &options, &plans);

        assert!(content.contains("1.0000 0.0000 0.0000 rg\n"));
        assert!(content.contains("0.0000 0.0000 1.0000 RG\n"));
    }
}
