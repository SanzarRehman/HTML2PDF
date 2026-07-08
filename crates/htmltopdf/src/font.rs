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
    /// Lazily-built fallback chain (system faces tried for characters this
    /// font lacks — CJK, Cyrillic, …). Built at most once per `Font`; `None`
    /// until the first uncovered character forces it. Fallback fonts have an
    /// empty chain of their own (one level deep, no recursion).
    fallbacks: Mutex<Option<Arc<Vec<Arc<Font>>>>>,
}

enum FontKind {
    Helvetica,
    TrueType(Box<TrueTypeFont>),
}

/// System families tried, in order, for characters the primary font lacks.
/// Chosen for breadth and for `glyf` outlines (subsettable): Arial Unicode MS
/// ships with macOS and covers CJK + most BMP scripts; the Noto/DejaVu names
/// cover common Linux installs. Missing families are skipped silently.
const FALLBACK_FAMILIES: &[&str] = &[
    "Arial Unicode MS",
    "Noto Sans",
    "DejaVu Sans",
    "Arial",
];

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
            fallbacks: Mutex::new(None),
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
            fallbacks: Mutex::new(None),
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

    /// Whether this face can render `c` with a real glyph. For a TrueType face
    /// the `advances` cache only spans WinAnsi, so anything else asks the
    /// face's cmap directly.
    fn covers(&self, c: char) -> bool {
        match &self.kind {
            FontKind::Helvetica => c.is_ascii() || char_to_winansi(c).is_some(),
            FontKind::TrueType(font) => {
                c.is_ascii_whitespace()
                    || font.advances.contains_key(&c)
                    || font
                        .face
                        .as_ref()
                        .is_some_and(|face| face.glyph_index(c).is_some())
            }
        }
    }

    /// The lazily-built fallback chain (may be empty). Loaded system faces are
    /// cached for the lifetime of this `Font`, so the disk/parse cost is paid
    /// once per process-shared `Arc<Font>`, and only if ever needed.
    pub fn fallback_chain(&self) -> Arc<Vec<Arc<Font>>> {
        let mut slot = self.fallbacks.lock().unwrap();
        if let Some(chain) = slot.as_ref() {
            return chain.clone();
        }
        let own_name = self.embedding().map(|f| f.postscript_name.clone());
        let mut chain: Vec<Arc<Font>> = Vec::new();
        for family in FALLBACK_FAMILIES {
            if let Ok(font) = Font::load(&FontSource::Family(family.to_string())) {
                let duplicate = font.embedding().map(|f| &f.postscript_name) == own_name.as_ref()
                    || chain.iter().any(|c| {
                        c.embedding().map(|f| &f.postscript_name)
                            == font.embedding().map(|f| &f.postscript_name)
                    });
                if !duplicate {
                    chain.push(Arc::new(font));
                }
            }
        }
        let chain = Arc::new(chain);
        *slot = Some(chain.clone());
        chain
    }

    /// Split `text` into runs by which font in the chain renders them: index
    /// `0` = this font, `i > 0` = `fallback_chain()[i - 1]`. Returns `None`
    /// when this font covers everything (the overwhelmingly common case — the
    /// caller keeps its single-font path). Whitespace stays with the run it
    /// follows so spacing doesn't flip fonts mid-phrase; characters no font
    /// covers stay with the primary (rendered as `.notdef`/`?`). Emoji never
    /// trigger a fallback (color-bitmap faces can't be embedded in our text
    /// model).
    pub fn segment_by_coverage<'a>(&self, text: &'a str) -> Option<Vec<(usize, &'a str)>> {
        // ASCII never needs a fallback (any usable primary covers it) — this
        // keeps the per-command scan in the PDF writer off the hot path.
        if text.is_ascii() || text.chars().all(|c| self.covers(c)) {
            return None;
        }
        let chain = self.fallback_chain();
        let font_for = |c: char| -> Option<usize> {
            if c.is_whitespace() || is_emoji(c) {
                return None; // neutral: inherit the surrounding run's font
            }
            if self.covers(c) {
                return Some(0);
            }
            for (i, fallback) in chain.iter().enumerate() {
                if fallback.covers(c) {
                    return Some(i + 1);
                }
            }
            Some(0) // uncovered everywhere: primary's .notdef
        };

        let mut segments: Vec<(usize, &str)> = Vec::new();
        let mut current: usize = 0;
        let mut start = 0;
        let mut assigned_leading = false;
        for (offset, c) in text.char_indices() {
            let Some(font) = font_for(c) else { continue };
            if !assigned_leading {
                // Leading neutrals join the first strong run.
                current = font;
                assigned_leading = true;
                continue;
            }
            if font != current {
                segments.push((current, &text[start..offset]));
                start = offset;
                current = font;
            }
        }
        segments.push((current, &text[start..]));
        Some(segments)
    }

    /// Resolve a chain index from [`Font::segment_by_coverage`] to its font.
    pub fn chain_font(self: &Arc<Self>, index: usize) -> Arc<Font> {
        if index == 0 {
            self.clone()
        } else {
            self.fallback_chain()[index - 1].clone()
        }
    }

    /// Measured advance width of `text` in user-space units at `font_size`.
    ///
    /// For an embedded TrueType face this is the *shaped* width (HarfBuzz via
    /// `rustybuzz`): kerning and ligatures applied, exactly what the PDF output
    /// reproduces. The built-in Helvetica keeps the per-character AFM sum.
    /// Characters this font lacks are measured with the fallback chain's face
    /// that renders them (matching PDF emission).
    pub fn text_width(&self, text: &str, font_size: f32) -> f32 {
        if !text.is_ascii() {
            if let Some(segments) = self.segment_by_coverage(text) {
                let chain = self.fallback_chain();
                return segments
                    .iter()
                    .map(|(index, segment)| {
                        let font: &Font = if *index == 0 { self } else { &chain[index - 1] };
                        font.width_covered(segment, font_size)
                    })
                    .sum();
            }
        }
        self.width_covered(text, font_size)
    }

    /// Width of text this font itself renders (no fallback consultation).
    fn width_covered(&self, text: &str, font_size: f32) -> f32 {
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
        if self.face.is_none() {
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
        }

        // Fast path: no RTL characters → shape the whole string as one LTR run.
        if !contains_rtl(text) {
            let mut run = ShapedRun {
                glyphs: Vec::new(),
                width_em: 0.0,
            };
            self.shape_segment(text, None, &mut run);
            return run;
        }

        // Bidi path (UAX #9): resolve embedding levels against an LTR base —
        // HTML's default paragraph direction; `dir`/`direction: rtl` is not
        // supported yet — then shape each visual run with its own explicit
        // direction and concatenate in visual order. Forcing the direction per
        // run matters twice over: joining forms must be computed on logical
        // text, and a whole-buffer direction guess mis-shapes mixed strings.
        let bidi = unicode_bidi::BidiInfo::new(text, Some(unicode_bidi::Level::ltr()));
        let paragraph = &bidi.paragraphs[0];
        let (levels, runs) = bidi.visual_runs(paragraph, paragraph.range.clone());
        let mut run_out = ShapedRun {
            glyphs: Vec::new(),
            width_em: 0.0,
        };
        for range in runs {
            let direction = if levels[range.start].is_rtl() {
                rustybuzz::Direction::RightToLeft
            } else {
                rustybuzz::Direction::LeftToRight
            };
            self.shape_segment(&text[range], Some(direction), &mut run_out);
        }
        run_out
    }

    /// Shape one directionally-uniform segment and append its glyphs to `out`.
    /// `direction: None` lets rustybuzz guess from content (LTR-only fast path).
    fn shape_segment(
        &self,
        segment: &str,
        direction: Option<rustybuzz::Direction>,
        out: &mut ShapedRun,
    ) {
        let face = self.face.as_ref().expect("caller checked the face exists");
        let mut buffer = rustybuzz::UnicodeBuffer::new();
        buffer.push_str(segment);
        if let Some(direction) = direction {
            buffer.guess_segment_properties();
            buffer.set_direction(direction);
        }
        let output = rustybuzz::shape(face, &[], buffer);
        let infos = output.glyph_infos();
        let positions = output.glyph_positions();

        // Cluster boundaries (byte offsets into `segment`), in logical order, so
        // a glyph's cluster maps back to the characters it covers. The first
        // glyph seen for a cluster carries the characters; the rest carry none.
        let mut boundaries: Vec<usize> = infos.iter().map(|info| info.cluster as usize).collect();
        boundaries.sort_unstable();
        boundaries.dedup();
        let cluster_end = |start: usize| -> usize {
            match boundaries.binary_search(&start) {
                Ok(i) if i + 1 < boundaries.len() => boundaries[i + 1],
                _ => segment.len(),
            }
        };

        let mut seen = std::collections::HashSet::new();
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
                segment[cluster..cluster_end(cluster)].to_string()
            } else {
                String::new()
            };
            out.width_em += advance_em;
            out.glyphs.push(ShapedGlyph {
                gid,
                advance_em,
                natural_em,
                chars,
            });
        }
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

/// The process-wide system font index, scanned once. Immutable after init, so
/// it does not compromise the no-shared-mutable-state render model — it plays
/// the same role as the OS font directory itself.
fn system_font_db() -> &'static fontdb::Database {
    static DB: std::sync::OnceLock<fontdb::Database> = std::sync::OnceLock::new();
    DB.get_or_init(|| {
        let mut db = fontdb::Database::new();
        db.load_system_fonts();
        db
    })
}

fn load_family(name: &str) -> Result<(Vec<u8>, u32), String> {
    let db = system_font_db();
    let id = db
        .query(&fontdb::Query {
            families: &[fontdb::Family::Name(name)],
            ..Default::default()
        })
        .ok_or_else(|| format!("font family '{name}' not found in system fonts"))?;
    db.with_face_data(id, |data, index| (data.to_vec(), index))
        .ok_or_else(|| "failed to load font face data".to_string())
}

/// A font requirement cascaded from CSS: an optional first `font-family` (or a
/// generic keyword; `None` = the document's default font) plus weight/style.
/// Interned per document during box-tree building; resolved once per render.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct FontSpec {
    pub family: Option<String>,
    pub bold: bool,
    pub italic: bool,
}

/// A resolved [`FontSpec`]: the face to measure/shape/embed with, and whether
/// bold still needs to be synthesized (fill+stroke) because no real bold face
/// was found for the request.
#[derive(Debug, Clone)]
pub struct ResolvedFont {
    pub font: Arc<Font>,
    pub faux_bold: bool,
}

/// Resolve a spec against the document's primary font. Named families (and the
/// CSS generic keywords) load real system faces — including real bold/italic
/// variants — through a process-wide cache, so a family is read from disk once
/// per process, not once per render. A spec with no family keeps the primary
/// font (bold stays synthesized: a path-loaded primary has no reliable family
/// to find a bold sibling in). Unresolvable families fall back the same way.
pub fn resolve_spec(primary: &Arc<Font>, spec: &FontSpec) -> ResolvedFont {
    let Some(family) = &spec.family else {
        return ResolvedFont {
            font: primary.clone(),
            faux_bold: spec.bold,
        };
    };

    type CacheKey = (String, bool, bool);
    type CacheValue = Option<(Arc<Font>, bool)>; // (face, is real bold); None = lookup failed
    static CACHE: std::sync::OnceLock<Mutex<HashMap<CacheKey, CacheValue>>> =
        std::sync::OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));

    let key = (family.to_ascii_lowercase(), spec.bold, spec.italic);
    let cached = cache.lock().unwrap().get(&key).cloned();
    let loaded = match cached {
        Some(hit) => hit,
        None => {
            let loaded = load_family_variant(family, spec.bold, spec.italic);
            cache.lock().unwrap().insert(key, loaded.clone());
            loaded
        }
    };
    match loaded {
        Some((font, real_bold)) => ResolvedFont {
            faux_bold: spec.bold && !real_bold,
            font,
        },
        None => ResolvedFont {
            font: primary.clone(),
            faux_bold: spec.bold,
        },
    }
}

/// Load the best system face for `(family, bold, italic)`. Returns the parsed
/// font and whether the matched face is genuinely bold (weight ≥ 600) when
/// bold was requested — the caller synthesizes bold otherwise. An italic
/// request that only matches an upright face is accepted upright (no faux
/// italic). Generic CSS keywords map to `fontdb`'s generic families.
fn load_family_variant(name: &str, bold: bool, italic: bool) -> Option<(Arc<Font>, bool)> {
    let family = match name.to_ascii_lowercase().as_str() {
        "serif" => fontdb::Family::Serif,
        "sans-serif" => fontdb::Family::SansSerif,
        "monospace" => fontdb::Family::Monospace,
        "cursive" => fontdb::Family::Cursive,
        "fantasy" => fontdb::Family::Fantasy,
        _ => fontdb::Family::Name(name),
    };
    let db = system_font_db();
    let id = db.query(&fontdb::Query {
        families: &[family],
        weight: if bold { fontdb::Weight::BOLD } else { fontdb::Weight::NORMAL },
        style: if italic { fontdb::Style::Italic } else { fontdb::Style::Normal },
        ..Default::default()
    })?;
    let real_bold = db
        .face(id)
        .map(|info| info.weight.0 >= 600)
        .unwrap_or(false);
    let (data, index) = db.with_face_data(id, |data, index| (data.to_vec(), index))?;
    let font = Font {
        kind: FontKind::TrueType(Box::new(TrueTypeFont::parse(data, index).ok()?)),
        fallbacks: Mutex::new(None),
    };
    Some((Arc::new(font), real_bold))
}

// --- @font-face web fonts ---------------------------------------------------

/// A parsed `@font-face` rule: the family it declares, its `src:` candidates in
/// author order, and the declared weight/style (used to pick among several
/// rules for the same family at resolve time).
#[derive(Debug, Clone, PartialEq)]
pub struct FontFaceRule {
    pub family: String,
    pub sources: Vec<FontFaceSource>,
    pub bold: bool,
    pub italic: bool,
}

/// One `src:` candidate of a `@font-face` rule.
#[derive(Debug, Clone, PartialEq)]
pub enum FontFaceSource {
    /// `local(<family>)`: resolve from the system font database by family name.
    Local(String),
    /// `url(<target>)` with its optional `format(<hint>)` (lowercased).
    Url { url: String, format: Option<String> },
}

/// A `@font-face` rule whose source bytes loaded and parsed: the face to use
/// when a [`FontSpec`] names its family. Document-scoped — never entered into
/// the process-wide system-family cache, since two documents can bind the same
/// family name to different font data.
#[derive(Debug, Clone)]
pub struct WebFont {
    family_lower: String,
    bold: bool,
    italic: bool,
    font: Arc<Font>,
}

/// Load each `@font-face` rule's first usable source. `url()` sources resolve
/// like image `src`s — `data:` URIs, file paths against `base_dir`, and
/// (policy-gated, SSRF-guarded) remote `http(s)` — and accept raw
/// TrueType/OpenType or WOFF1 bytes; WOFF2 needs a Brotli decoder and is
/// skipped, falling through to the next source in the list. `local()` resolves
/// a system family by name. A rule whose sources all fail is dropped, so its
/// family falls back to normal system lookup.
pub fn load_font_faces(
    rules: &[FontFaceRule],
    base_dir: Option<&std::path::Path>,
    remote: &crate::image::RemoteImagePolicy,
) -> Vec<WebFont> {
    let mut loaded = Vec::new();
    for rule in rules {
        if rule.family.is_empty() {
            continue;
        }
        let font = rule
            .sources
            .iter()
            .find_map(|source| load_face_source(source, base_dir, remote));
        if let Some(font) = font {
            loaded.push(WebFont {
                family_lower: rule.family.to_ascii_lowercase(),
                bold: rule.bold,
                italic: rule.italic,
                font,
            });
        }
    }
    loaded
}

fn load_face_source(
    source: &FontFaceSource,
    base_dir: Option<&std::path::Path>,
    remote: &crate::image::RemoteImagePolicy,
) -> Option<Arc<Font>> {
    match source {
        FontFaceSource::Local(name) => load_local_source(name),
        FontFaceSource::Url { url, format } => {
            // A format() hint for a container we can't read: skip without
            // fetching (WOFF2, EOT, SVG-in-font).
            if let Some(hint) = format {
                if !matches!(hint.as_str(), "truetype" | "opentype" | "woff") {
                    return None;
                }
            }
            let bytes = crate::image::load_bytes(url, base_dir, remote)?;
            let bytes = if bytes.starts_with(b"wOFF") {
                woff1_to_sfnt(&bytes)?
            } else {
                bytes
            };
            font_from_bytes(bytes, 0)
        }
    }
}

/// Resolve a `local(<name>)` source. CSS matches local() against family, full,
/// and PostScript names; `fontdb` indexes families and PostScript names, so a
/// full name like "Arial Bold" is additionally handled by stripping the
/// Bold/Italic suffix and querying the base family with that weight/style.
fn load_local_source(name: &str) -> Option<Arc<Font>> {
    let db = system_font_db();
    let id = db
        .query(&fontdb::Query {
            families: &[fontdb::Family::Name(name)],
            ..Default::default()
        })
        .or_else(|| {
            db.faces()
                .find(|face| face.post_script_name.eq_ignore_ascii_case(name))
                .map(|face| face.id)
        })
        .or_else(|| {
            let lower = name.to_ascii_lowercase();
            let (base, bold, italic) = if let Some(base) = lower.strip_suffix(" bold italic") {
                (base, true, true)
            } else if let Some(base) = lower.strip_suffix(" bold") {
                (base, true, false)
            } else if let Some(base) = lower.strip_suffix(" italic") {
                (base, false, true)
            } else {
                return None;
            };
            let base = &name[..base.len()];
            db.query(&fontdb::Query {
                families: &[fontdb::Family::Name(base)],
                weight: if bold { fontdb::Weight::BOLD } else { fontdb::Weight::NORMAL },
                style: if italic { fontdb::Style::Italic } else { fontdb::Style::Normal },
                ..Default::default()
            })
        })?;
    let (data, index) = db.with_face_data(id, |data, index| (data.to_vec(), index))?;
    font_from_bytes(data, index)
}

fn font_from_bytes(data: Vec<u8>, index: u32) -> Option<Arc<Font>> {
    let parsed = TrueTypeFont::parse(data, index).ok()?;
    Some(Arc::new(Font {
        kind: FontKind::TrueType(Box::new(parsed)),
        fallbacks: Mutex::new(None),
    }))
}

/// Resolve a spec, consulting the document's loaded `@font-face` fonts ahead of
/// the system lookup — an author-declared family shadows any same-named system
/// family. Among several rules for one family, the best weight/style match wins
/// (weight over style, first declared on a tie); bold is still synthesized when
/// only a non-bold face was declared for a bold request.
pub fn resolve_spec_with(
    primary: &Arc<Font>,
    spec: &FontSpec,
    web_fonts: &[WebFont],
) -> ResolvedFont {
    if let Some(family) = &spec.family {
        let mut best: Option<(&WebFont, u8)> = None;
        for candidate in web_fonts {
            if !candidate.family_lower.eq_ignore_ascii_case(family) {
                continue;
            }
            let score = u8::from(candidate.bold == spec.bold) * 2
                + u8::from(candidate.italic == spec.italic);
            if best.is_none_or(|(_, s)| score > s) {
                best = Some((candidate, score));
            }
        }
        if let Some((chosen, _)) = best {
            return ResolvedFont {
                font: chosen.font.clone(),
                faux_bold: spec.bold && !chosen.bold,
            };
        }
    }
    resolve_spec(primary, spec)
}

/// Convert a WOFF (version 1) container to raw sfnt (TrueType/OpenType) bytes:
/// rebuild the 16-byte-entry table directory and inflate each zlib-compressed
/// table with `flate2` (already a dependency). WOFF2 uses Brotli plus a
/// transformed `glyf` model and is out of scope here.
fn woff1_to_sfnt(data: &[u8]) -> Option<Vec<u8>> {
    fn be16(bytes: &[u8], at: usize) -> Option<u16> {
        Some(u16::from_be_bytes(bytes.get(at..at + 2)?.try_into().ok()?))
    }
    fn be32(bytes: &[u8], at: usize) -> Option<u32> {
        Some(u32::from_be_bytes(bytes.get(at..at + 4)?.try_into().ok()?))
    }

    if data.get(..4)? != b"wOFF" {
        return None;
    }
    let flavor = be32(data, 4)?;
    let num_tables = be16(data, 12)? as usize;
    // Real fonts have a few dozen tables; the cap also keeps the u16
    // searchRange arithmetic below from overflowing.
    if num_tables == 0 || num_tables > 1024 {
        return None;
    }

    struct Table {
        tag: [u8; 4],
        checksum: u32,
        data: Vec<u8>,
    }
    let mut tables = Vec::with_capacity(num_tables);
    for i in 0..num_tables {
        let at = 44 + i * 20;
        let tag: [u8; 4] = data.get(at..at + 4)?.try_into().ok()?;
        let offset = be32(data, at + 4)? as usize;
        let comp_len = be32(data, at + 8)? as usize;
        let orig_len = be32(data, at + 12)? as usize;
        let checksum = be32(data, at + 16)?;
        let raw = data.get(offset..offset.checked_add(comp_len)?)?;
        let table = if comp_len < orig_len {
            let mut out = Vec::with_capacity(orig_len);
            let mut inflater = flate2::read::ZlibDecoder::new(raw);
            std::io::Read::read_to_end(&mut inflater, &mut out).ok()?;
            if out.len() != orig_len {
                return None;
            }
            out
        } else if comp_len == orig_len {
            raw.to_vec()
        } else {
            return None;
        };
        tables.push(Table { tag, checksum, data: table });
    }

    let entry_selector = (num_tables as u32).ilog2() as u16;
    let search_range = (1u16 << entry_selector) * 16;
    let mut out = Vec::new();
    out.extend_from_slice(&flavor.to_be_bytes());
    out.extend_from_slice(&(num_tables as u16).to_be_bytes());
    out.extend_from_slice(&search_range.to_be_bytes());
    out.extend_from_slice(&entry_selector.to_be_bytes());
    out.extend_from_slice(&(num_tables as u16 * 16 - search_range).to_be_bytes());
    // WOFF directory entries keep the sfnt's tag order, so offsets are the only
    // thing to recompute; table data is 4-byte aligned as in the original.
    let mut offset = 12 + num_tables * 16;
    for table in &tables {
        out.extend_from_slice(&table.tag);
        out.extend_from_slice(&table.checksum.to_be_bytes());
        out.extend_from_slice(&(offset as u32).to_be_bytes());
        out.extend_from_slice(&(table.data.len() as u32).to_be_bytes());
        offset += (table.data.len() + 3) & !3;
    }
    for table in &tables {
        out.extend_from_slice(&table.data);
        while out.len() % 4 != 0 {
            out.push(0);
        }
    }
    Some(out)
}

/// PostScript name (name id 6), falling back to family (id 1), sanitized to a
/// valid PDF name (no spaces or delimiters).
/// Emoji and pictograph ranges: excluded from fallback (color-bitmap faces
/// can't be embedded as Type0 outlines), so they render as `.notdef` rather
/// than dragging a 10 MB emoji font into the PDF.
fn is_emoji(c: char) -> bool {
    matches!(c as u32,
        0x1F000..=0x1FAFF   // Mahjong … pictographs (incl. regional indicators)
        | 0x2600..=0x27BF   // Misc symbols, dingbats
        | 0xFE0E..=0xFE0F   // variation selectors
    )
}

/// Whether `text` contains any character from a right-to-left script (Hebrew,
/// Arabic and its extensions, Syriac, Thaana, NKo, plus the RTL presentation
/// forms and supplementary-plane RTL blocks). A cheap pre-filter so purely-LTR
/// text skips the UAX #9 machinery entirely.
pub(crate) fn contains_rtl(text: &str) -> bool {
    text.chars().any(|c| {
        matches!(c,
            '\u{0590}'..='\u{08FF}'      // Hebrew, Arabic, Syriac, Thaana, NKo, …
            | '\u{FB1D}'..='\u{FDFF}'    // Hebrew/Arabic presentation forms A
            | '\u{FE70}'..='\u{FEFF}'    // Arabic presentation forms B
            | '\u{10800}'..='\u{10FFF}'  // ancient RTL scripts (SMP)
            | '\u{1E800}'..='\u{1EFFF}') // Adlam, Mende Kikakui, Arabic math
    })
}

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
    fn woff1_converts_back_to_sfnt() {
        // A synthetic two-table sfnt wrapped into WOFF1 (first table
        // zlib-compressed, second stored raw) converts back to the expected
        // sfnt byte layout: rebuilt header/directory, inflated + aligned data.
        let table_a = vec![b'A'; 100];
        let table_b = vec![7u8; 20];
        let mut compressed = Vec::new();
        {
            use std::io::Write;
            let mut encoder = flate2::write::ZlibEncoder::new(
                &mut compressed,
                flate2::Compression::default(),
            );
            encoder.write_all(&table_a).unwrap();
            encoder.finish().unwrap();
        }
        assert!(compressed.len() < table_a.len());

        let mut woff = Vec::new();
        woff.extend_from_slice(b"wOFF");
        woff.extend_from_slice(&0x0001_0000u32.to_be_bytes()); // flavor
        woff.extend_from_slice(&0u32.to_be_bytes()); // total length (unread)
        woff.extend_from_slice(&2u16.to_be_bytes()); // numTables
        woff.extend_from_slice(&[0u8; 30]); // reserved … privLength
        assert_eq!(woff.len(), 44);
        let data_start = 44 + 2 * 20;
        woff.extend_from_slice(b"glyf");
        woff.extend_from_slice(&(data_start as u32).to_be_bytes());
        woff.extend_from_slice(&(compressed.len() as u32).to_be_bytes());
        woff.extend_from_slice(&(table_a.len() as u32).to_be_bytes());
        woff.extend_from_slice(&0xDEAD_BEEFu32.to_be_bytes());
        woff.extend_from_slice(b"loca");
        woff.extend_from_slice(&((data_start + compressed.len()) as u32).to_be_bytes());
        woff.extend_from_slice(&(table_b.len() as u32).to_be_bytes());
        woff.extend_from_slice(&(table_b.len() as u32).to_be_bytes());
        woff.extend_from_slice(&0x1234_5678u32.to_be_bytes());
        woff.extend_from_slice(&compressed);
        woff.extend_from_slice(&table_b);

        let sfnt = woff1_to_sfnt(&woff).expect("woff1 should convert");
        assert_eq!(&sfnt[0..4], &0x0001_0000u32.to_be_bytes());
        assert_eq!(u16::from_be_bytes([sfnt[4], sfnt[5]]), 2);
        // Directory entries: glyf at 12 + 2*16 = 44 (len 100), loca at 144.
        assert_eq!(&sfnt[12..16], b"glyf");
        assert_eq!(&sfnt[16..20], &0xDEAD_BEEFu32.to_be_bytes());
        assert_eq!(&sfnt[20..24], &44u32.to_be_bytes());
        assert_eq!(&sfnt[24..28], &100u32.to_be_bytes());
        assert_eq!(&sfnt[28..32], b"loca");
        assert_eq!(&sfnt[36..40], &144u32.to_be_bytes());
        assert_eq!(&sfnt[40..44], &20u32.to_be_bytes());
        assert_eq!(&sfnt[44..144], &table_a[..]);
        assert_eq!(&sfnt[144..164], &table_b[..]);
        assert!(woff1_to_sfnt(b"not a woff").is_none());
    }

    #[test]
    fn font_face_shadows_system_lookup() {
        let remote = crate::image::RemoteImagePolicy::default();
        let rules = vec![
            FontFaceRule {
                family: "BrandFont".into(),
                sources: vec![
                    // Unsupported container: skipped without touching the network.
                    FontFaceSource::Url {
                        url: "https://example.invalid/a.woff2".into(),
                        format: Some("woff2".into()),
                    },
                    FontFaceSource::Local("Arial".into()),
                ],
                bold: false,
                italic: false,
            },
            FontFaceRule {
                family: "BrandFont".into(),
                sources: vec![FontFaceSource::Local("Arial Bold".into())],
                bold: true,
                italic: false,
            },
        ];
        let web = load_font_faces(&rules, None, &remote);
        if web.is_empty() {
            return; // no Arial on this box; the macOS dev/CI machines have it
        }

        let primary = Arc::new(Font::helvetica());
        // The declared family resolves to the web font, case-insensitively.
        let regular = resolve_spec_with(
            &primary,
            &FontSpec { family: Some("brandfont".into()), bold: false, italic: false },
            &web,
        );
        assert!(!Arc::ptr_eq(&regular.font, &primary));
        assert!(!regular.faux_bold);
        let bold = resolve_spec_with(
            &primary,
            &FontSpec { family: Some("BrandFont".into()), bold: true, italic: false },
            &web,
        );
        if web.len() == 2 {
            // A real bold rule was declared and loaded: no synthesis.
            assert!(!bold.faux_bold);
            assert!(!Arc::ptr_eq(&bold.font, &regular.font));
        } else {
            // Only the regular face loaded: bold is synthesized on it.
            assert!(bold.faux_bold);
        }
        // An undeclared family still falls through to the system path.
        let other = resolve_spec_with(
            &primary,
            &FontSpec { family: Some("NoSuchFamilyZZZ".into()), bold: false, italic: false },
            &web,
        );
        assert!(Arc::ptr_eq(&other.font, &primary));
    }

    #[test]
    fn font_face_remote_url_is_fail_closed() {
        // Remote fetching is disabled by default; the rule must simply drop
        // (no panic, no network) and the family falls back to system lookup.
        let rules = vec![FontFaceRule {
            family: "RemoteFace".into(),
            sources: vec![FontFaceSource::Url {
                url: "http://127.0.0.1:1/x.ttf".into(),
                format: None,
            }],
            bold: false,
            italic: false,
        }];
        let web = load_font_faces(&rules, None, &crate::image::RemoteImagePolicy::default());
        assert!(web.is_empty());
    }

    #[test]
    fn font_face_loads_a_real_woff_container() {
        // Wrap a real system TrueType font into a WOFF1 container (each table
        // zlib-compressed) and load it through the @font-face pipeline.
        let path = "/System/Library/Fonts/Supplemental/Arial.ttf";
        let Ok(sfnt) = std::fs::read(path) else {
            return; // non-macOS box
        };
        let flavor = u32::from_be_bytes(sfnt[0..4].try_into().unwrap());
        let num = u16::from_be_bytes(sfnt[4..6].try_into().unwrap()) as usize;

        let mut dirs = Vec::new();
        let mut blobs: Vec<u8> = Vec::new();
        let data_start = 44 + 20 * num;
        for i in 0..num {
            let at = 12 + 16 * i;
            let tag = &sfnt[at..at + 4];
            let checksum = &sfnt[at + 4..at + 8];
            let off = u32::from_be_bytes(sfnt[at + 8..at + 12].try_into().unwrap()) as usize;
            let len = u32::from_be_bytes(sfnt[at + 12..at + 16].try_into().unwrap()) as usize;
            let table = &sfnt[off..off + len];
            let mut compressed = Vec::new();
            {
                use std::io::Write;
                let mut encoder = flate2::write::ZlibEncoder::new(
                    &mut compressed,
                    flate2::Compression::default(),
                );
                encoder.write_all(table).unwrap();
                encoder.finish().unwrap();
            }
            let blob = if compressed.len() < len { &compressed[..] } else { table };
            dirs.extend_from_slice(tag);
            dirs.extend_from_slice(&((data_start + blobs.len()) as u32).to_be_bytes());
            dirs.extend_from_slice(&(blob.len() as u32).to_be_bytes());
            dirs.extend_from_slice(&(len as u32).to_be_bytes());
            dirs.extend_from_slice(checksum);
            blobs.extend_from_slice(blob);
            while blobs.len() % 4 != 0 {
                blobs.push(0);
            }
        }
        let mut woff = Vec::new();
        woff.extend_from_slice(b"wOFF");
        woff.extend_from_slice(&flavor.to_be_bytes());
        woff.extend_from_slice(&((data_start + blobs.len()) as u32).to_be_bytes());
        woff.extend_from_slice(&(num as u16).to_be_bytes());
        woff.extend_from_slice(&[0u8; 30]);
        woff.extend_from_slice(&dirs);
        woff.extend_from_slice(&blobs);

        let sfnt_back = woff1_to_sfnt(&woff).expect("woff1 should convert");
        assert!(
            TrueTypeFont::parse(sfnt_back, 0).is_ok(),
            "converted sfnt should parse"
        );

        let tmp = std::env::temp_dir().join("htmltopdf-test-brand.woff");
        std::fs::write(&tmp, &woff).unwrap();
        let rules = vec![FontFaceRule {
            family: "WoffFace".into(),
            sources: vec![FontFaceSource::Url {
                url: tmp.to_string_lossy().into_owned(),
                format: Some("woff".into()),
            }],
            bold: false,
            italic: false,
        }];
        let web = load_font_faces(&rules, None, &crate::image::RemoteImagePolicy::default());
        std::fs::remove_file(&tmp).ok();
        assert_eq!(web.len(), 1, "woff url source should load");
    }

    #[test]
    fn font_face_url_loads_a_font_file() {
        let path = "/System/Library/Fonts/Supplemental/Arial.ttf";
        if !std::path::Path::new(path).exists() {
            return;
        }
        let rules = vec![FontFaceRule {
            family: "FileFace".into(),
            sources: vec![FontFaceSource::Url { url: path.into(), format: None }],
            bold: false,
            italic: false,
        }];
        let web = load_font_faces(&rules, None, &crate::image::RemoteImagePolicy::default());
        assert_eq!(web.len(), 1);
    }

    #[test]
    fn font_face_renders_with_the_declared_face() {
        if !std::path::Path::new("/System/Library/Fonts/Supplemental/Arial.ttf").exists() {
            return;
        }
        let html = r#"<style>
            @font-face { font-family: BrandFont; src: url(missing.woff2) format("woff2"), local("Arial"); }
            p { font-family: BrandFont; }
        </style><p>branded text</p>"#;
        let pdf = crate::Engine::new()
            .render_html(html, crate::RenderOptions::default())
            .unwrap();
        let raw = String::from_utf8_lossy(&pdf);
        assert!(raw.contains("ArialMT"), "web font face not embedded");
    }

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
    fn mixed_direction_text_shapes_in_visual_order() {
        let Some(font) = system_font() else { return };
        let embedded = font.embedding().expect("TrueType face");
        // Latin, a two-word Hebrew phrase, Latin again. Visual order must keep
        // the Latin in place and reverse the Hebrew segment.
        let run = embedded.shape("abc \u{05D0}\u{05D1} \u{05D2}\u{05D3} xyz");
        if run.glyphs.is_empty() {
            return; // face without Hebrew coverage: nothing to assert
        }
        let covered: String = run.glyphs.iter().map(|g| g.chars.as_str()).collect();
        // Every source character is still covered exactly once…
        let mut sorted_covered: Vec<char> = covered.chars().collect();
        let mut sorted_source: Vec<char> = "abc \u{05D0}\u{05D1} \u{05D2}\u{05D3} xyz".chars().collect();
        sorted_covered.sort_unstable();
        sorted_source.sort_unstable();
        assert_eq!(sorted_covered, sorted_source);
        // …but the Hebrew block reads right-to-left: the logically-later word
        // (גד) comes before the earlier one (אב), and each word is reversed.
        let pos = |c: char| covered.find(c).unwrap_or_else(|| panic!("missing {c}"));
        assert!(pos('\u{05D2}') < pos('\u{05D0}'), "second Hebrew word paints first: {covered:?}");
        assert!(pos('\u{05D1}') < pos('\u{05D0}'), "within a word, glyphs reverse: {covered:?}");
        // The surrounding Latin stays put.
        assert!(pos('a') < pos('\u{05D2}') && pos('\u{05D3}') < pos('x'), "{covered:?}");
    }

    #[test]
    fn segments_split_by_font_coverage() {
        let font = Font::helvetica();
        // Covered text: no segmentation at all.
        assert!(font.segment_by_coverage("plain ASCII, café — dashes").is_none());

        // Helvetica cannot cover CJK, so the string splits; the CJK segment
        // goes to a covering fallback when the system has one (index > 0),
        // else stays with the primary as .notdef.
        let segments = font
            .segment_by_coverage("abc \u{4E2D}\u{6587} def")
            .expect("CJK forces segmentation");
        let texts: Vec<&str> = segments.iter().map(|(_, s)| *s).collect();
        assert_eq!(texts, vec!["abc ", "\u{4E2D}\u{6587} ", "def"]);
        assert_eq!(segments[0].0, 0, "ASCII stays with the primary");
        assert_eq!(segments[2].0, 0, "ASCII stays with the primary");
        let has_cjk_fallback = font
            .fallback_chain()
            .iter()
            .any(|f| f.covers('\u{4E2D}'));
        if has_cjk_fallback {
            assert!(segments[1].0 > 0, "CJK goes to a fallback face");
        }

        // Chain-aware measurement: the mixed string's width is the sum of its
        // segments measured in their own fonts.
        let whole = font.text_width("abc \u{4E2D}\u{6587} def", 12.0);
        let sum: f32 = segments
            .iter()
            .map(|(index, segment)| match index {
                0 => Font::helvetica().text_width(segment, 12.0),
                i => font.fallback_chain()[i - 1].text_width(segment, 12.0),
            })
            .sum();
        assert!((whole - sum).abs() < 0.01, "whole {whole} vs sum {sum}");
    }

    #[test]
    fn resolve_spec_keeps_primary_and_finds_real_variants() {
        use super::{resolve_spec, FontSpec};
        let primary = std::sync::Arc::new(Font::helvetica());

        // No family: the primary stays, bold stays synthesized.
        let default_bold = resolve_spec(
            &primary,
            &FontSpec { family: None, bold: true, italic: false },
        );
        assert!(std::sync::Arc::ptr_eq(&default_bold.font, &primary));
        assert!(default_bold.faux_bold);

        // Unknown family: fall back to the primary, keep faux bold.
        let missing = resolve_spec(
            &primary,
            &FontSpec { family: Some("NoSuchFamily".into()), bold: true, italic: false },
        );
        assert!(std::sync::Arc::ptr_eq(&missing.font, &primary));
        assert!(missing.faux_bold);

        // A real family (system-dependent; skip without Arial): the bold and
        // italic variants resolve to real faces, killing bold synthesis.
        let arial = resolve_spec(
            &primary,
            &FontSpec { family: Some("Arial".into()), bold: false, italic: false },
        );
        let Some(regular) = arial.font.embedding() else { return };
        assert!(regular.postscript_name.contains("Arial"));

        let bold = resolve_spec(
            &primary,
            &FontSpec { family: Some("Arial".into()), bold: true, italic: false },
        );
        let bold_face = bold.font.embedding().expect("bold face resolves");
        assert!(bold_face.postscript_name.contains("Bold"), "{}", bold_face.postscript_name);
        assert!(!bold.faux_bold, "real bold face needs no synthesis");

        let italic = resolve_spec(
            &primary,
            &FontSpec { family: Some("Arial".into()), bold: false, italic: true },
        );
        let italic_face = italic.font.embedding().expect("italic face resolves");
        assert!(
            italic_face.postscript_name.contains("Italic"),
            "{}",
            italic_face.postscript_name
        );

        // The cache hands back the same face for a repeated spec.
        let again = resolve_spec(
            &primary,
            &FontSpec { family: Some("Arial".into()), bold: true, italic: false },
        );
        assert!(std::sync::Arc::ptr_eq(&again.font, &bold.font));
    }

    #[test]
    fn contains_rtl_detects_scripts() {
        use super::contains_rtl;
        assert!(!contains_rtl("plain ASCII 123"));
        assert!(!contains_rtl("café naïve"));
        assert!(contains_rtl("שלום"));
        assert!(contains_rtl("مرحبا"));
        assert!(contains_rtl("mixed מ text"));
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
