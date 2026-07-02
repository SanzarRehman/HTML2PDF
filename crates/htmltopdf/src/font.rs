//! Font metrics for the fonts the PDF backend actually emits.
//!
//! The PDF writer currently references the standard-14 font `Helvetica`
//! (see `pdf.rs`). The advance widths of the standard-14 fonts are a fixed,
//! published table (the Adobe Font Metrics / AFM data), so we can measure text
//! exactly for the font we render, with zero dependencies, deterministically,
//! and with no per-render allocation.
//!
//! Widths are expressed in 1/1000 of an em, which is the PDF text-space unit for
//! glyph advances. `text_width` converts to user-space units for a given font
//! size.
//!
//! When a font is supplied (a file path or a family name) the engine instead
//! measures and embeds that TrueType/OpenType font via `ttf-parser`/`fontdb`
//! (the [`Font`] type below). With no font supplied, the built-in Helvetica path
//! is used and nothing is embedded (ADR 0002 / ADR 0004).

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

/// Advance width, in 1/1000 em, for a character rendered in Helvetica.
///
/// Characters outside the WinAnsi/Latin-1 range fall back to the width of the
/// average lowercase glyph, which keeps measurement stable for unknown glyphs
/// without pretending to have metrics we do not.
pub fn helvetica_advance(c: char) -> u16 {
    // Standard-14 Helvetica AFM widths, ASCII printable range 0x20..=0x7E.
    const ASCII: [u16; 95] = [
        278, // ' ' space
        278, // !
        355, // "
        556, // #
        556, // $
        889, // %
        667, // &
        191, // '
        333, // (
        333, // )
        389, // *
        584, // +
        278, // ,
        333, // -
        278, // .
        278, // /
        556, 556, 556, 556, 556, 556, 556, 556, 556, 556, // 0-9
        278, // :
        278, // ;
        584, // <
        584, // =
        584, // >
        556, // ?
        1015, // @
        667, 667, 722, 722, 667, 611, 778, 722, 278, 500, // A-J
        667, 556, 833, 722, 778, 667, 778, 722, 667, 611, // K-T
        722, 667, 944, 667, 667, 611, // U-Z
        278, // [
        278, // backslash
        278, // ]
        469, // ^
        556, // _
        333, // `
        556, 556, 500, 556, 556, 278, 556, 556, 222, 222, // a-j
        500, 222, 833, 556, 556, 556, 556, 333, 500, 278, // k-t
        556, 500, 722, 500, 500, 500, // u-z
        334, // {
        260, // |
        334, // }
        584, // ~
    ];

    // Width of the average lowercase glyph, used as the fallback advance.
    const FALLBACK: u16 = 556;

    let code = c as u32;
    if (0x20..=0x7E).contains(&code) {
        ASCII[(code - 0x20) as usize]
    } else {
        match c {
            '\u{00A0}' => ASCII[0], // non-breaking space measures like a space
            _ => FALLBACK,
        }
    }
}

/// How to locate a font to embed.
#[derive(Debug, Clone)]
pub enum FontSource {
    /// A path to a TrueType/OpenType font file.
    Path(PathBuf),
    /// A family name resolved against the system font database (`fontdb`).
    Family(String),
}

/// A font used for both text measurement and PDF embedding.
///
/// `Helvetica` measures from the built-in AFM table and is *not* embedded (the
/// PDF references the standard-14 font). `TrueType` measures from the parsed
/// face and is embedded as a `FontFile2`.
pub struct Font {
    kind: FontKind,
}

enum FontKind {
    Helvetica,
    TrueType(Box<TrueTypeFont>),
}

/// A parsed TrueType/OpenType font: the raw bytes for embedding plus the metrics
/// the layout engine and PDF `FontDescriptor` need (scaled to PDF 1000-unit em),
/// and a cached HarfBuzz (`rustybuzz`) face for text shaping.
pub struct TrueTypeFont {
    /// Cached shaping face borrowing `data`'s heap buffer.
    ///
    /// SAFETY invariants for the `'static` lie:
    /// - declared *before* `data`, so it drops first;
    /// - `data` is private and never mutated/reallocated after `parse`;
    /// - a `Vec`'s heap buffer is stable across moves of the owning struct;
    /// - the face never escapes with the `'static` lifetime (private field,
    ///   used only inside shaping methods).
    face: Option<rustybuzz::Face<'static>>,
    /// Shaped-run cache keyed by the exact source string. `Mutex` (not
    /// `RefCell`) because `Font` is shared across render threads via `Arc`.
    shape_cache: Mutex<HashMap<String, Arc<ShapedRun>>>,
    data: Vec<u8>,
    pub index: u32,
    pub postscript_name: String,
    pub units_per_em: f32,
    advances: HashMap<char, u16>,
    default_advance: u16,
    pub ascent: i32,
    pub descent: i32,
    pub cap_height: i32,
    pub bbox: [i32; 4],
    pub italic_angle: f32,
    pub flags: u32,
    pub stem_v: i32,
}

/// One glyph of a shaped run.
#[derive(Debug, Clone)]
pub struct ShapedGlyph {
    pub gid: u16,
    /// Shaped advance (kerning applied), as a fraction of the em.
    pub advance_em: f32,
    /// The glyph's natural (`hmtx`) advance, as a fraction of the em. This is
    /// what a PDF viewer applies from `/W`, so the writer emits `TJ`
    /// adjustments of `natural - shaped` to reproduce kerning.
    pub natural_em: f32,
    /// Source characters this glyph covers (its cluster; empty for glyphs that
    /// share a cluster with a predecessor). Used for `/ToUnicode`, so ligature
    /// glyphs map back to all of their characters.
    pub chars: String,
}

/// A shaped text run: the output of HarfBuzz for one source string.
#[derive(Debug, Clone, Default)]
pub struct ShapedRun {
    pub glyphs: Vec<ShapedGlyph>,
    /// Total shaped advance as a fraction of the em.
    pub width_em: f32,
}

impl Default for Font {
    fn default() -> Self {
        Font::helvetica()
    }
}

impl std::fmt::Debug for Font {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.kind {
            FontKind::Helvetica => write!(f, "Font(Helvetica)"),
            FontKind::TrueType(t) => {
                write!(f, "Font(TrueType {:?}, {} bytes)", t.postscript_name, t.data.len())
            }
        }
    }
}

impl Font {
    pub fn helvetica() -> Font {
        Font {
            kind: FontKind::Helvetica,
        }
    }

    /// Load and parse a font from a file path or a system family name.
    pub fn load(source: &FontSource) -> Result<Font, String> {
        let (data, index) = match source {
            FontSource::Path(path) => {
                let data = std::fs::read(path)
                    .map_err(|e| format!("failed to read font {}: {e}", path.display()))?;
                (data, 0)
            }
            FontSource::Family(name) => load_family(name)?,
        };
        Ok(Font {
            kind: FontKind::TrueType(Box::new(TrueTypeFont::parse(data, index)?)),
        })
    }

    /// The embedded TrueType font, if this is not the built-in Helvetica.
    pub fn embedding(&self) -> Option<&TrueTypeFont> {
        match &self.kind {
            FontKind::TrueType(font) => Some(font),
            FontKind::Helvetica => None,
        }
    }

    /// Advance of one character as a fraction of the em.
    fn advance_em(&self, c: char) -> f32 {
        match &self.kind {
            FontKind::Helvetica => f32::from(helvetica_advance(c)) / 1000.0,
            FontKind::TrueType(font) => {
                let units = font.advances.get(&c).copied().unwrap_or(font.default_advance);
                f32::from(units) / font.units_per_em
            }
        }
    }

    /// Measured advance width of `text` in user-space units at `font_size`.
    ///
    /// For an embedded TrueType face this is the *shaped* width (HarfBuzz via
    /// `rustybuzz`): kerning and ligatures applied, exactly what the PDF output
    /// reproduces. The built-in Helvetica keeps the per-character AFM sum.
    pub fn text_width(&self, text: &str, font_size: f32) -> f32 {
        match &self.kind {
            FontKind::Helvetica => {
                font_size * text.chars().map(|c| self.advance_em(c)).sum::<f32>()
            }
            FontKind::TrueType(font) => font_size * font.shape(text).width_em,
        }
    }

    /// Largest character prefix of `text` (count) whose width fits `max_width`.
    /// Always returns at least 1 for non-empty input so callers make progress.
    pub fn fitting_char_count(&self, text: &str, max_width: f32, font_size: f32) -> usize {
        let mut used = 0.0;
        let mut count = 0;
        for c in text.chars() {
            let advance = self.advance_em(c) * font_size;
            if count > 0 && used + advance > max_width {
                break;
            }
            used += advance;
            count += 1;
        }
        count.max(usize::from(!text.is_empty()))
    }
}

/// The glyph data needed to embed a font as a PDF Type0/CIDFontType2 composite:
/// per-glyph natural advance widths (for `/W`; kerning is reproduced with `TJ`
/// adjustments at write time) and a glyph-to-Unicode map (for the `/ToUnicode`
/// CMap, so the text stays extractable/searchable — a ligature glyph maps back
/// to all of its source characters).
pub struct CidLayout {
    /// `(glyph id, natural advance in 1000-unit em)` for each used glyph,
    /// sorted by id.
    pub widths: Vec<(u16, i32)>,
    /// Glyph id → the Unicode characters it covers (for `/ToUnicode`).
    pub gid_to_unicode: std::collections::BTreeMap<u16, String>,
}

impl TrueTypeFont {
    /// The raw font program bytes (for full embedding when subsetting fails).
    pub fn data(&self) -> &[u8] {
        &self.data
    }

    /// A subset of the font program containing only `used_gids` (plus `.notdef`
    /// and composite components), or `None` if it cannot be subset (e.g. a
    /// CFF/OpenType-CFF font) — in which case the caller embeds the full program.
    pub fn subset(&self, used_gids: &std::collections::BTreeSet<u16>) -> Option<Vec<u8>> {
        crate::subset::subset(&self.data, self.index, used_gids)
    }

    /// Shape `text` with HarfBuzz (cached by the exact string). Returns an empty
    /// run (no glyphs, zero width) only if the face failed to parse at load.
    pub fn shape(&self, text: &str) -> Arc<ShapedRun> {
        if let Some(hit) = self.shape_cache.lock().unwrap().get(text) {
            return hit.clone();
        }
        let run = Arc::new(self.shape_uncached(text));
        self.shape_cache
            .lock()
            .unwrap()
            .insert(text.to_string(), run.clone());
        run
    }

    fn shape_uncached(&self, text: &str) -> ShapedRun {
        let Some(face) = &self.face else {
            // Face unavailable (should not happen: parse() validated the data).
            // Fall back to unshaped per-character advances so measurement still
            // works; PDF emission will also fall back per character.
            let width_em = text
                .chars()
                .map(|c| {
                    f32::from(self.advances.get(&c).copied().unwrap_or(self.default_advance))
                        / self.units_per_em
                })
                .sum();
            return ShapedRun {
                glyphs: Vec::new(),
                width_em,
            };
        };

        let mut buffer = rustybuzz::UnicodeBuffer::new();
        buffer.push_str(text);
        let output = rustybuzz::shape(face, &[], buffer);
        let infos = output.glyph_infos();
        let positions = output.glyph_positions();

        // Cluster boundaries (byte offsets into `text`), in logical order, so a
        // glyph's cluster maps back to the characters it covers. The first glyph
        // seen for a cluster carries the characters; the rest carry none.
        let mut boundaries: Vec<usize> = infos.iter().map(|info| info.cluster as usize).collect();
        boundaries.sort_unstable();
        boundaries.dedup();
        let cluster_end = |start: usize| -> usize {
            match boundaries.binary_search(&start) {
                Ok(i) if i + 1 < boundaries.len() => boundaries[i + 1],
                _ => text.len(),
            }
        };

        let mut seen = std::collections::HashSet::new();
        let mut glyphs = Vec::with_capacity(infos.len());
        let mut width_em = 0.0f32;
        for (info, pos) in infos.iter().zip(positions) {
            let gid = info.glyph_id as u16;
            let advance_em = pos.x_advance as f32 / self.units_per_em;
            let natural_em = self
                .face
                .as_ref()
                .and_then(|f| f.glyph_hor_advance(ttf_parser::GlyphId(gid)))
                .map(|units| f32::from(units) / self.units_per_em)
                .unwrap_or(advance_em);
            let cluster = info.cluster as usize;
            let chars = if seen.insert(cluster) {
                text[cluster..cluster_end(cluster)].to_string()
            } else {
                String::new()
            };
            width_em += advance_em;
            glyphs.push(ShapedGlyph {
                gid,
                advance_em,
                natural_em,
                chars,
            });
        }

        ShapedRun { glyphs, width_em }
    }

    /// Resolve the glyph widths and Unicode mapping for every text run the
    /// document paints, by shaping each unique string (cache-assisted; the
    /// layout pass already shaped most of them for measurement).
    pub fn shaped_cid_layout<'a>(&self, texts: impl Iterator<Item = &'a str>) -> CidLayout {
        use std::collections::BTreeMap;

        let mut widths = BTreeMap::new();
        let mut gid_to_unicode = BTreeMap::new();
        for text in texts {
            let run = self.shape(text);
            for glyph in &run.glyphs {
                widths
                    .entry(glyph.gid)
                    .or_insert_with(|| (glyph.natural_em * 1000.0).round() as i32);
                if !glyph.chars.is_empty() {
                    gid_to_unicode
                        .entry(glyph.gid)
                        .or_insert_with(|| glyph.chars.clone());
                }
            }
        }

        CidLayout {
            widths: widths.into_iter().collect(),
            gid_to_unicode,
        }
    }

    fn parse(data: Vec<u8>, index: u32) -> Result<TrueTypeFont, String> {
        let face = ttf_parser::Face::parse(&data, index)
            .map_err(|e| format!("failed to parse font: {e}"))?;

        let units_per_em = face.units_per_em() as f32;
        let to_pdf = |value: i32| (value as f32 * 1000.0 / units_per_em).round() as i32;

        // Cache WinAnsi-range advances for fast text measurement. (Glyphs outside
        // this range are resolved on demand when the font is embedded as a Type0
        // composite; see `cid_layout`.)
        let mut advances = HashMap::new();
        for code in 32u8..=255 {
            let Some(ch) = winansi_to_char(code) else {
                continue;
            };
            let units = face
                .glyph_index(ch)
                .and_then(|gid| face.glyph_hor_advance(gid))
                .unwrap_or(0);
            if units != 0 {
                advances.insert(ch, units);
            }
        }

        let default_advance = [' ', 'n', 'o']
            .iter()
            .find_map(|&c| face.glyph_index(c).and_then(|gid| face.glyph_hor_advance(gid)))
            .unwrap_or((units_per_em / 2.0) as u16);

        let bbox = face.global_bounding_box();
        let cap_height = face
            .capital_height()
            .map(i32::from)
            .unwrap_or((face.ascender() as f32 * 0.7) as i32);
        let italic_angle = face.italic_angle();

        // Nonsymbolic (bit 6 = 32); add fixed-pitch and italic bits as detected.
        let mut flags = 32u32;
        if face.is_monospaced() {
            flags |= 1;
        }
        if italic_angle != 0.0 {
            flags |= 64;
        }

        let mut font = TrueTypeFont {
            face: None,
            shape_cache: Mutex::new(HashMap::new()),
            postscript_name: postscript_name(&face),
            units_per_em,
            advances,
            default_advance,
            ascent: to_pdf(i32::from(face.ascender())),
            descent: to_pdf(i32::from(face.descender())),
            cap_height: to_pdf(cap_height),
            bbox: [
                to_pdf(i32::from(bbox.x_min)),
                to_pdf(i32::from(bbox.y_min)),
                to_pdf(i32::from(bbox.x_max)),
                to_pdf(i32::from(bbox.y_max)),
            ],
            italic_angle,
            flags,
            stem_v: 80,
            data,
            index,
        };

        // Build the cached shaping face over `font.data`'s heap buffer. See the
        // SAFETY notes on the `face` field: the buffer is stable across moves of
        // `font`, `data` is never mutated again, and `face` (declared first)
        // drops before `data`.
        let slice: &'static [u8] =
            unsafe { std::slice::from_raw_parts(font.data.as_ptr(), font.data.len()) };
        font.face = rustybuzz::Face::from_slice(slice, index);

        Ok(font)
    }
}

fn load_family(name: &str) -> Result<(Vec<u8>, u32), String> {
    let mut db = fontdb::Database::new();
    db.load_system_fonts();
    let id = db
        .query(&fontdb::Query {
            families: &[fontdb::Family::Name(name)],
            ..Default::default()
        })
        .ok_or_else(|| format!("font family '{name}' not found in system fonts"))?;
    db.with_face_data(id, |data, index| (data.to_vec(), index))
        .ok_or_else(|| "failed to load font face data".to_string())
}

/// PostScript name (name id 6), falling back to family (id 1), sanitized to a
/// valid PDF name (no spaces or delimiters).
fn postscript_name(face: &ttf_parser::Face) -> String {
    let mut postscript = None;
    let mut family = None;
    for name in face.names() {
        let Some(value) = name.to_string() else {
            continue;
        };
        match name.name_id {
            6 => postscript = Some(value),
            1 if family.is_none() => family = Some(value),
            _ => {}
        }
    }
    let raw = postscript.or(family).unwrap_or_else(|| "EmbeddedFont".to_string());
    let cleaned: String = raw
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '+')
        .collect();
    if cleaned.is_empty() {
        "EmbeddedFont".to_string()
    } else {
        cleaned
    }
}

/// Map a WinAnsi byte to its Unicode scalar (used to look up glyphs/widths).
fn winansi_to_char(code: u8) -> Option<char> {
    let mapped = match code {
        0x80 => '\u{20AC}',
        0x82 => '\u{201A}',
        0x83 => '\u{0192}',
        0x84 => '\u{201E}',
        0x85 => '\u{2026}',
        0x86 => '\u{2020}',
        0x87 => '\u{2021}',
        0x88 => '\u{02C6}',
        0x89 => '\u{2030}',
        0x8A => '\u{0160}',
        0x8B => '\u{2039}',
        0x8C => '\u{0152}',
        0x8E => '\u{017D}',
        0x91 => '\u{2018}',
        0x92 => '\u{2019}',
        0x93 => '\u{201C}',
        0x94 => '\u{201D}',
        0x95 => '\u{2022}',
        0x96 => '\u{2013}',
        0x97 => '\u{2014}',
        0x98 => '\u{02DC}',
        0x99 => '\u{2122}',
        0x9A => '\u{0161}',
        0x9B => '\u{203A}',
        0x9C => '\u{0153}',
        0x9E => '\u{017E}',
        0x9F => '\u{0178}',
        // Undefined WinAnsi code points.
        0x7F | 0x81 | 0x8D | 0x8F | 0x90 | 0x9D => return None,
        // 0x20..=0x7E ASCII and 0xA0..=0xFF Latin-1 are identity.
        _ => code as char,
    };
    Some(mapped)
}

/// Map a Unicode scalar to its WinAnsi (CP1252) byte, if one exists. The inverse
/// of [`winansi_to_char`]; used when writing PDF text strings under
/// `/WinAnsiEncoding`, so Latin-1 text and the CP1252 specials (curly quotes,
/// dashes, the bullet, the euro sign, …) survive into the PDF.
pub(crate) fn char_to_winansi(ch: char) -> Option<u8> {
    let byte = match ch {
        // ASCII and Latin-1 are identity in WinAnsi.
        '\u{20}'..='\u{7E}' | '\u{A0}'..='\u{FF}' => ch as u32 as u8,
        // CP1252 specials in the 0x80..=0x9F range.
        '\u{20AC}' => 0x80,
        '\u{201A}' => 0x82,
        '\u{0192}' => 0x83,
        '\u{201E}' => 0x84,
        '\u{2026}' => 0x85,
        '\u{2020}' => 0x86,
        '\u{2021}' => 0x87,
        '\u{02C6}' => 0x88,
        '\u{2030}' => 0x89,
        '\u{0160}' => 0x8A,
        '\u{2039}' => 0x8B,
        '\u{0152}' => 0x8C,
        '\u{017D}' => 0x8E,
        '\u{2018}' => 0x91,
        '\u{2019}' => 0x92,
        '\u{201C}' => 0x93,
        '\u{201D}' => 0x94,
        '\u{2022}' => 0x95,
        '\u{2013}' => 0x96,
        '\u{2014}' => 0x97,
        '\u{02DC}' => 0x98,
        '\u{2122}' => 0x99,
        '\u{0161}' => 0x9A,
        '\u{203A}' => 0x9B,
        '\u{0153}' => 0x9C,
        '\u{017E}' => 0x9E,
        '\u{0178}' => 0x9F,
        _ => return None,
    };
    Some(byte)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_helvetica_widths() {
        assert_eq!(helvetica_advance(' '), 278);
        assert_eq!(helvetica_advance('M'), 833);
        assert_eq!(helvetica_advance('i'), 222);
        assert_eq!(helvetica_advance('0'), 556);
    }

    #[test]
    fn unknown_glyphs_use_fallback() {
        assert_eq!(helvetica_advance('\u{4E2D}'), 556);
        assert_eq!(helvetica_advance('\u{00A0}'), 278);
    }

    #[test]
    fn text_width_scales_with_font_size() {
        let font = Font::helvetica();
        let small = font.text_width("Hello", 10.0);
        let large = font.text_width("Hello", 20.0);
        assert!((large - small * 2.0).abs() < 0.001);
    }

    #[test]
    fn narrow_glyphs_measure_less_than_wide_glyphs() {
        let font = Font::helvetica();
        assert!(font.text_width("iiii", 12.0) < font.text_width("MMMM", 12.0));
    }

    #[test]
    fn fitting_char_count_respects_width() {
        let font = Font::helvetica();
        // "MMMM" at 12pt: each M is 833/1000 * 12 = ~9.996 units.
        let one_m = font.text_width("M", 12.0);
        assert_eq!(font.fitting_char_count("MMMM", one_m * 2.5, 12.0), 2);
        assert_eq!(font.fitting_char_count("", 100.0, 12.0), 0);
        // Always at least one character of progress even in a too-narrow box.
        assert_eq!(font.fitting_char_count("M", 0.1, 12.0), 1);
    }

    #[test]
    fn helvetica_is_not_embedded() {
        assert!(Font::helvetica().embedding().is_none());
    }

    #[test]
    fn loading_a_missing_font_path_errors() {
        let result = Font::load(&FontSource::Path("/no/such/font.ttf".into()));
        assert!(result.is_err());
    }

    /// A system TrueType face for shaping tests, or `None` (test skips) on
    /// machines without one.
    fn system_font() -> Option<Font> {
        let candidates = [
            "/System/Library/Fonts/Supplemental/Arial.ttf",
            "/System/Library/Fonts/Supplemental/Times New Roman.ttf",
            "/Library/Fonts/Arial.ttf",
        ];
        let path = candidates
            .iter()
            .find(|p| std::path::Path::new(p).is_file())?;
        Font::load(&FontSource::Path(path.into())).ok()
    }

    #[test]
    fn shaped_clusters_cover_all_source_characters() {
        let Some(font) = system_font() else { return };
        let embedded = font.embedding().expect("TrueType face");
        let text = "Vault first office";
        let run = embedded.shape(text);
        assert!(!run.glyphs.is_empty());
        // Every source character appears in exactly one glyph's cluster string
        // (ligatures collapse several chars into one glyph's cluster).
        let covered: String = run.glyphs.iter().map(|g| g.chars.as_str()).collect();
        assert_eq!(covered, text);
        assert!(run.width_em > 0.0);
    }

    #[test]
    fn shaping_applies_kerning_to_measurement() {
        let Some(font) = system_font() else { return };
        let embedded = font.embedding().expect("TrueType face");
        // "AV" kerns in virtually every Latin text face; the shaped width must
        // not exceed the sum of the two natural advances, and for a kerning
        // pair it is strictly smaller.
        let run = embedded.shape("AV");
        let natural: f32 = run.glyphs.iter().map(|g| g.natural_em).sum();
        assert!(
            run.width_em <= natural + 1e-4,
            "shaped {} vs natural {natural}",
            run.width_em
        );
        // Measurement goes through the same shaping (cache hit).
        let measured = font.text_width("AV", 1000.0);
        assert!((measured - run.width_em * 1000.0).abs() < 0.01);
    }

    #[test]
    fn shaped_cid_layout_collects_gids_and_unicode() {
        let Some(font) = system_font() else { return };
        let embedded = font.embedding().expect("TrueType face");
        let cid = embedded.shaped_cid_layout(["Hello", "World"].into_iter());
        assert!(!cid.widths.is_empty());
        // Every mapped glyph resolves back to at least one character.
        assert!(cid.gid_to_unicode.values().all(|s| !s.is_empty()));
        let all: String = cid.gid_to_unicode.values().cloned().collect();
        assert!(all.contains('H') && all.contains('W'));
    }

    #[test]
    fn winansi_maps_ascii_and_latin1_and_specials() {
        assert_eq!(winansi_to_char(b'A'), Some('A'));
        assert_eq!(winansi_to_char(0xE9), Some('é')); // Latin-1 identity
        assert_eq!(winansi_to_char(0x92), Some('\u{2019}')); // right single quote
        assert_eq!(winansi_to_char(0x81), None); // undefined WinAnsi code
    }
}
