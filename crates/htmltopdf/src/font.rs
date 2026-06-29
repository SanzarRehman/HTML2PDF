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
//! When we add font embedding/subsetting we will measure embedded fonts with
//! `ttf-parser` instead; until then, measuring against the font we actually draw
//! is the correct, faithful choice (ADR 0002).

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

/// Measured advance width of `text` in user-space units at the given font size.
pub fn text_width(text: &str, font_size: f32) -> f32 {
    let thousandths: u32 = text.chars().map(|c| u32::from(helvetica_advance(c))).sum();
    (thousandths as f32) * font_size / 1000.0
}

/// The largest character prefix of `text` whose measured width fits `max_width`,
/// returned as the number of characters. Always returns at least 1 when `text`
/// is non-empty so callers make forward progress.
pub fn fitting_char_count(text: &str, max_width: f32, font_size: f32) -> usize {
    let mut used = 0.0;
    let mut count = 0;
    for c in text.chars() {
        let advance = f32::from(helvetica_advance(c)) * font_size / 1000.0;
        if count > 0 && used + advance > max_width {
            break;
        }
        used += advance;
        count += 1;
    }
    count.max(usize::from(!text.is_empty()))
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
        let small = text_width("Hello", 10.0);
        let large = text_width("Hello", 20.0);
        assert!((large - small * 2.0).abs() < 0.001);
    }

    #[test]
    fn narrow_glyphs_measure_less_than_wide_glyphs() {
        assert!(text_width("iiii", 12.0) < text_width("MMMM", 12.0));
    }

    #[test]
    fn fitting_char_count_respects_width() {
        // "MMMM" at 12pt: each M is 833/1000 * 12 = ~9.996 units.
        let one_m = text_width("M", 12.0);
        assert_eq!(fitting_char_count("MMMM", one_m * 2.5, 12.0), 2);
        assert_eq!(fitting_char_count("", 100.0, 12.0), 0);
        // Always at least one character of progress even in a too-narrow box.
        assert_eq!(fitting_char_count("M", 0.1, 12.0), 1);
    }
}
