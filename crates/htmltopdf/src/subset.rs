//! A minimal "retain-GIDs" TrueType subsetter.
//!
//! Given a font and the set of glyph ids a document actually uses, rebuild the
//! `glyf` and `loca` tables so only the used glyphs (plus the components of any
//! composite glyph, transitively, and `.notdef`) carry outline data; every other
//! table is copied verbatim. Glyph ids are **not** renumbered, so the cmap, the
//! PDF `/W` widths, the `/ToUnicode` CMap, and `/CIDToGIDMap /Identity` all stay
//! valid against the subset (this is why it composes with the Type0/Identity-H
//! embedding in ADR 0005).
//!
//! This only handles `glyf`-based TrueType outlines. CFF/OpenType-CFF fonts have
//! no `glyf` table and return `None`, so the caller embeds the full program.
//! `ttf-parser` reads the source tables (resolving the right face inside a
//! collection), so `.ttc` inputs work too — the output is always a standalone
//! single-font sfnt.

use std::collections::BTreeSet;

use ttf_parser::Tag;

const HEAD: Tag = Tag::from_bytes(b"head");
const LOCA: Tag = Tag::from_bytes(b"loca");
const GLYF: Tag = Tag::from_bytes(b"glyf");
const DSIG: Tag = Tag::from_bytes(b"DSIG");

/// Subset `font_data` (face `index`) to `used_gids`, returning a new standalone
/// sfnt, or `None` if the font cannot be subset (no `glyf`, or a structural
/// problem) — in which case the caller should embed the full font.
pub fn subset(font_data: &[u8], index: u32, used_gids: &BTreeSet<u16>) -> Option<Vec<u8>> {
    let face = ttf_parser::Face::parse(font_data, index).ok()?;
    let num_glyphs = face.number_of_glyphs();
    if num_glyphs == 0 {
        return None;
    }

    let raw = face.raw_face();
    let head = raw.table(HEAD)?;
    let loca = raw.table(LOCA)?;
    let glyf = raw.table(GLYF)?;
    if head.len() < 54 {
        return None;
    }

    // Old loca: numGlyphs + 1 offsets, short (×2) or long format.
    let long_loca = be_i16(head, 50)? != 0;
    let count = num_glyphs as usize + 1;
    let mut old_offsets = Vec::with_capacity(count);
    if long_loca {
        if loca.len() < count * 4 {
            return None;
        }
        for i in 0..count {
            old_offsets.push(be_u32(loca, i * 4)?);
        }
    } else {
        if loca.len() < count * 2 {
            return None;
        }
        for i in 0..count {
            old_offsets.push(u32::from(be_u16(loca, i * 2)?) * 2);
        }
    }

    // Closure of kept glyphs: the used ids, `.notdef`, and (transitively) the
    // components referenced by any composite glyph.
    let mut keep: BTreeSet<u16> = BTreeSet::new();
    keep.insert(0);
    for &gid in used_gids {
        if gid < num_glyphs {
            keep.insert(gid);
        }
    }
    let mut stack: Vec<u16> = keep.iter().copied().collect();
    while let Some(gid) = stack.pop() {
        let glyph = glyph_bytes(glyf, &old_offsets, gid)?;
        for component in composite_components(glyph) {
            if component < num_glyphs && keep.insert(component) {
                stack.push(component);
            }
        }
    }

    // Rebuild glyf (kept glyphs only) and a long-format loca.
    let mut new_glyf: Vec<u8> = Vec::new();
    let mut new_loca: Vec<u32> = Vec::with_capacity(count);
    for gid in 0..num_glyphs {
        new_loca.push(new_glyf.len() as u32);
        if keep.contains(&gid) {
            let glyph = glyph_bytes(glyf, &old_offsets, gid)?;
            new_glyf.extend_from_slice(glyph);
            while new_glyf.len() % 4 != 0 {
                new_glyf.push(0);
            }
        }
    }
    new_loca.push(new_glyf.len() as u32);

    let mut new_loca_bytes = Vec::with_capacity(new_loca.len() * 4);
    for offset in &new_loca {
        new_loca_bytes.extend_from_slice(&offset.to_be_bytes());
    }

    // head with indexToLocFormat = 1 (long) and checkSumAdjustment zeroed.
    let mut new_head = head.to_vec();
    new_head[50] = 0;
    new_head[51] = 1;
    new_head[8..12].copy_from_slice(&[0, 0, 0, 0]);

    // Collect the output tables: every source table, with glyf/loca/head replaced
    // and the digital signature (now invalid) dropped.
    let mut tables: Vec<([u8; 4], Vec<u8>)> = Vec::new();
    for record in raw.table_records {
        let tag = record.tag;
        if tag == DSIG {
            continue;
        }
        let bytes = if tag == GLYF {
            new_glyf.clone()
        } else if tag == LOCA {
            new_loca_bytes.clone()
        } else if tag == HEAD {
            new_head.clone()
        } else {
            raw.table(tag)?.to_vec()
        };
        tables.push((tag.to_bytes(), bytes));
    }
    if tables.is_empty() {
        return None;
    }
    tables.sort_by(|a, b| a.0.cmp(&b.0));

    Some(assemble(&tables))
}

/// The raw bytes of glyph `gid` from the source `glyf` table.
fn glyph_bytes<'a>(glyf: &'a [u8], offsets: &[u32], gid: u16) -> Option<&'a [u8]> {
    let start = *offsets.get(gid as usize)? as usize;
    let end = *offsets.get(gid as usize + 1)? as usize;
    if end <= start {
        return Some(&[]);
    }
    glyf.get(start..end)
}

/// The component glyph ids referenced by a composite glyph (empty for simple or
/// empty glyphs).
fn composite_components(glyph: &[u8]) -> Vec<u16> {
    let mut components = Vec::new();
    if glyph.len() < 10 || be_i16(glyph, 0).unwrap_or(0) >= 0 {
        return components; // simple glyph or no contours
    }

    const ARG_1_AND_2_ARE_WORDS: u16 = 0x0001;
    const WE_HAVE_A_SCALE: u16 = 0x0008;
    const MORE_COMPONENTS: u16 = 0x0020;
    const WE_HAVE_AN_X_AND_Y_SCALE: u16 = 0x0040;
    const WE_HAVE_A_TWO_BY_TWO: u16 = 0x0080;

    let mut pos = 10;
    loop {
        let (Some(flags), Some(component)) = (be_u16(glyph, pos), be_u16(glyph, pos + 2)) else {
            break;
        };
        components.push(component);
        pos += 4;
        pos += if flags & ARG_1_AND_2_ARE_WORDS != 0 { 4 } else { 2 };
        if flags & WE_HAVE_A_SCALE != 0 {
            pos += 2;
        } else if flags & WE_HAVE_AN_X_AND_Y_SCALE != 0 {
            pos += 4;
        } else if flags & WE_HAVE_A_TWO_BY_TWO != 0 {
            pos += 8;
        }
        if flags & MORE_COMPONENTS == 0 {
            break;
        }
    }
    components
}

/// Assemble a single-font sfnt from `(tag, data)` tables (already sorted by tag),
/// computing the table directory, per-table checksums, and the head checksum
/// adjustment.
fn assemble(tables: &[([u8; 4], Vec<u8>)]) -> Vec<u8> {
    let num_tables = tables.len() as u16;
    let (search_range, entry_selector, range_shift) = search_params(num_tables);

    let mut out = Vec::new();
    out.extend_from_slice(&0x0001_0000u32.to_be_bytes()); // TrueType outlines
    out.extend_from_slice(&num_tables.to_be_bytes());
    out.extend_from_slice(&search_range.to_be_bytes());
    out.extend_from_slice(&entry_selector.to_be_bytes());
    out.extend_from_slice(&range_shift.to_be_bytes());

    // Table data begins after the header and directory.
    let mut data_offset = 12 + 16 * tables.len();
    let mut head_file_offset = None;
    for (tag, bytes) in tables {
        if tag == b"head" {
            head_file_offset = Some(data_offset);
        }
        out.extend_from_slice(tag);
        out.extend_from_slice(&checksum(bytes).to_be_bytes());
        out.extend_from_slice(&(data_offset as u32).to_be_bytes());
        out.extend_from_slice(&(bytes.len() as u32).to_be_bytes());
        data_offset += align4(bytes.len());
    }

    for (_, bytes) in tables {
        out.extend_from_slice(bytes);
        for _ in 0..(align4(bytes.len()) - bytes.len()) {
            out.push(0);
        }
    }

    // Patch head.checkSumAdjustment = 0xB1B0AFBA - checksum(whole file).
    if let Some(head_offset) = head_file_offset {
        let adjustment = 0xB1B0_AFBAu32.wrapping_sub(checksum(&out));
        out[head_offset + 8..head_offset + 12].copy_from_slice(&adjustment.to_be_bytes());
    }

    out
}

fn search_params(num_tables: u16) -> (u16, u16, u16) {
    let mut entry_selector = 0u16;
    let mut largest_pow2 = 1u16;
    while largest_pow2 * 2 <= num_tables {
        largest_pow2 *= 2;
        entry_selector += 1;
    }
    let search_range = largest_pow2 * 16;
    let range_shift = num_tables * 16 - search_range;
    (search_range, entry_selector, range_shift)
}

/// sfnt table checksum: the sum of the data interpreted as big-endian u32 words,
/// zero-padded to a multiple of four bytes.
fn checksum(data: &[u8]) -> u32 {
    let mut sum = 0u32;
    let mut i = 0;
    while i < data.len() {
        let mut word = 0u32;
        for j in 0..4 {
            word = (word << 8) | u32::from(data.get(i + j).copied().unwrap_or(0));
        }
        sum = sum.wrapping_add(word);
        i += 4;
    }
    sum
}

fn align4(n: usize) -> usize {
    (n + 3) & !3
}

fn be_i16(data: &[u8], offset: usize) -> Option<i16> {
    Some(i16::from_be_bytes([
        *data.get(offset)?,
        *data.get(offset + 1)?,
    ]))
}

fn be_u16(data: &[u8], offset: usize) -> Option<u16> {
    Some(u16::from_be_bytes([
        *data.get(offset)?,
        *data.get(offset + 1)?,
    ]))
}

fn be_u32(data: &[u8], offset: usize) -> Option<u32> {
    Some(u32::from_be_bytes([
        *data.get(offset)?,
        *data.get(offset + 1)?,
        *data.get(offset + 2)?,
        *data.get(offset + 3)?,
    ]))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn load_arial() -> Option<Vec<u8>> {
        let candidates = [
            "/System/Library/Fonts/Supplemental/Arial.ttf",
            "/Library/Fonts/Arial.ttf",
        ];
        candidates
            .iter()
            .find(|p| std::path::Path::new(p).is_file())
            .and_then(|p| std::fs::read(p).ok())
    }

    #[test]
    fn subset_is_smaller_and_keeps_used_glyphs() {
        let Some(data) = load_arial() else {
            return; // no system font available; skip
        };
        let face = ttf_parser::Face::parse(&data, 0).expect("parse");

        // Keep the glyphs for "Hi".
        let used: BTreeSet<u16> = "Hi"
            .chars()
            .filter_map(|c| face.glyph_index(c).map(|g| g.0))
            .collect();
        assert!(!used.is_empty());

        let subset = subset(&data, 0, &used).expect("subset");
        assert!(subset.len() < data.len(), "subset should shrink the font");

        // The subset must re-parse, keep outlines for the used glyphs, and drop
        // outlines for an unused glyph.
        let sub_face = ttf_parser::Face::parse(&subset, 0).expect("subset parses");
        assert_eq!(sub_face.number_of_glyphs(), face.number_of_glyphs());
        for gid in &used {
            assert!(
                sub_face.glyph_bounding_box(ttf_parser::GlyphId(*gid)).is_some(),
                "kept glyph {gid} must still have an outline"
            );
        }
        let unused = (1..face.number_of_glyphs()).find(|g| !used.contains(g));
        if let Some(gid) = unused {
            assert!(
                sub_face.glyph_bounding_box(ttf_parser::GlyphId(gid)).is_none(),
                "dropped glyph {gid} should have no outline"
            );
        }
    }
}
