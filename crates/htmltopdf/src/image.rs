//! Raster image loading for `<img>`: resolve a `src` (a `data:` URI or a file
//! path), sniff the format, and produce a [`DecodedImage`] ready for PDF
//! embedding.
//!
//! Two embedding strategies are used, both low-dependency:
//!
//! - **JPEG** is embedded verbatim through PDF's `DCTDecode` filter — the PDF
//!   reader does the decoding — so we only parse the header for dimensions and
//!   the color space. No pixel decoder, no re-encode.
//! - **PNG** is decoded here (chunk parsing, zlib inflate via `flate2`, scanline
//!   unfiltering, color-type expansion) into raw samples embedded through
//!   `FlateDecode`, with the alpha channel split out into a soft mask (`SMask`).

use std::io::Read;
use std::path::Path;

use flate2::read::ZlibDecoder;

/// A decoded image ready to embed as a PDF image XObject.
#[derive(Debug, Clone, PartialEq)]
pub struct DecodedImage {
    /// Intrinsic pixel dimensions.
    pub width: u32,
    pub height: u32,
    pub color_space: ColorSpace,
    pub bits_per_component: u8,
    pub filter: ImageFilter,
    /// The embedded bytes: raw JPEG for `Dct`, decoded samples for `Flate`.
    pub data: Vec<u8>,
    /// Optional 8-bit alpha soft mask (one gray sample per pixel), `Flate`-filtered.
    pub smask: Option<Vec<u8>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColorSpace {
    DeviceRgb,
    DeviceGray,
}

impl ColorSpace {
    pub fn pdf_name(self) -> &'static str {
        match self {
            ColorSpace::DeviceRgb => "DeviceRGB",
            ColorSpace::DeviceGray => "DeviceGray",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageFilter {
    /// Raw JPEG bytes; embedded through `/DCTDecode`.
    Dct,
    /// Decoded samples; embedded through `/FlateDecode`.
    Flate,
}

impl ImageFilter {
    pub fn pdf_name(self) -> &'static str {
        match self {
            ImageFilter::Dct => "DCTDecode",
            ImageFilter::Flate => "FlateDecode",
        }
    }
}

/// Resolve and decode an image `src`. Returns `None` on any failure (missing
/// file, unsupported format, malformed data), so a broken `<img>` is simply not
/// painted rather than aborting the whole render.
pub fn load_image(src: &str, base_dir: Option<&Path>) -> Option<DecodedImage> {
    let bytes = if let Some(rest) = src.strip_prefix("data:") {
        decode_data_uri(rest)?
    } else {
        read_file(src, base_dir)?
    };
    decode(&bytes)
}

fn read_file(src: &str, base_dir: Option<&Path>) -> Option<Vec<u8>> {
    // Ignore any URL query/fragment; only plain local paths are supported.
    let path = src.split(['?', '#']).next().unwrap_or(src);
    let path = Path::new(path);
    let resolved = if path.is_absolute() {
        path.to_path_buf()
    } else {
        base_dir?.join(path)
    };
    std::fs::read(resolved).ok()
}

/// Parse the part of a `data:` URI after `data:` — `[<media-type>][;base64],<data>`.
fn decode_data_uri(rest: &str) -> Option<Vec<u8>> {
    let comma = rest.find(',')?;
    let (meta, data) = rest.split_at(comma);
    let data = &data[1..]; // skip the comma
    if meta.rsplit(';').any(|token| token.eq_ignore_ascii_case("base64")) {
        base64_decode(data)
    } else {
        // Percent-decoding is uncommon for images; take the bytes verbatim.
        Some(data.as_bytes().to_vec())
    }
}

/// Dispatch on the file's magic bytes.
fn decode(bytes: &[u8]) -> Option<DecodedImage> {
    if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        decode_jpeg(bytes)
    } else if bytes.starts_with(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]) {
        decode_png(bytes)
    } else {
        None
    }
}

// --- JPEG -----------------------------------------------------------------

/// Read a JPEG's dimensions and component count from its start-of-frame marker,
/// then embed the file verbatim (PDF's `DCTDecode` does the decoding).
fn decode_jpeg(bytes: &[u8]) -> Option<DecodedImage> {
    let mut i = 2; // skip the SOI marker (FF D8)
    while i + 1 < bytes.len() {
        if bytes[i] != 0xFF {
            i += 1;
            continue;
        }
        // Skip any fill bytes (runs of 0xFF).
        let mut marker = bytes[i + 1];
        let mut j = i + 1;
        while marker == 0xFF && j + 1 < bytes.len() {
            j += 1;
            marker = bytes[j];
        }
        i = j;

        // Standalone markers without a length payload.
        if marker == 0xD8 || marker == 0xD9 || (0xD0..=0xD7).contains(&marker) {
            i += 1;
            continue;
        }

        let len_at = i + 1;
        if len_at + 1 >= bytes.len() {
            return None;
        }
        let seg_len = u16::from_be_bytes([bytes[len_at], bytes[len_at + 1]]) as usize;

        // SOF0..SOF15 (baseline/progressive) carry the frame geometry, excluding
        // the DHT/DAC/DQT markers that share the numeric range.
        let is_sof = (0xC0..=0xCF).contains(&marker)
            && !matches!(marker, 0xC4 | 0xC8 | 0xCC);
        if is_sof {
            // [len(2)][precision(1)][height(2)][width(2)][components(1)]
            let base = len_at + 2;
            if base + 5 >= bytes.len() {
                return None;
            }
            let height = u16::from_be_bytes([bytes[base + 1], bytes[base + 2]]) as u32;
            let width = u16::from_be_bytes([bytes[base + 3], bytes[base + 4]]) as u32;
            let components = bytes[base + 5];
            let color_space = match components {
                1 => ColorSpace::DeviceGray,
                3 => ColorSpace::DeviceRgb,
                _ => return None, // CMYK/YCCK not supported
            };
            if width == 0 || height == 0 {
                return None;
            }
            return Some(DecodedImage {
                width,
                height,
                color_space,
                bits_per_component: 8,
                filter: ImageFilter::Dct,
                data: bytes.to_vec(),
                smask: None,
            });
        }

        i = len_at + 2 + seg_len.checked_sub(2)?;
    }
    None
}

// --- PNG ------------------------------------------------------------------

struct PngHeader {
    width: u32,
    height: u32,
    bit_depth: u8,
    color_type: u8,
}

/// Decode a PNG into raw samples plus an optional alpha soft mask. Supports
/// 8- and 16-bit (16 truncated to the high byte) grayscale, RGB, palette, and
/// their alpha variants — the formats real documents actually use.
fn decode_png(bytes: &[u8]) -> Option<DecodedImage> {
    let mut header: Option<PngHeader> = None;
    let mut palette: Vec<[u8; 3]> = Vec::new();
    let mut trns: Vec<u8> = Vec::new();
    let mut idat: Vec<u8> = Vec::new();

    let mut i = 8; // skip the signature
    while i + 8 <= bytes.len() {
        let len = u32::from_be_bytes(bytes[i..i + 4].try_into().ok()?) as usize;
        let kind = &bytes[i + 4..i + 8];
        let data_start = i + 8;
        let data_end = data_start.checked_add(len)?;
        if data_end + 4 > bytes.len() {
            return None;
        }
        let chunk = &bytes[data_start..data_end];

        match kind {
            b"IHDR" => {
                if chunk.len() < 13 {
                    return None;
                }
                header = Some(PngHeader {
                    width: u32::from_be_bytes(chunk[0..4].try_into().ok()?),
                    height: u32::from_be_bytes(chunk[4..8].try_into().ok()?),
                    bit_depth: chunk[8],
                    color_type: chunk[9],
                });
                // Interlacing (chunk[12] != 0) is unsupported.
                if chunk[12] != 0 {
                    return None;
                }
            }
            b"PLTE" => {
                palette = chunk.chunks_exact(3).map(|c| [c[0], c[1], c[2]]).collect();
            }
            b"tRNS" => trns = chunk.to_vec(),
            b"IDAT" => idat.extend_from_slice(chunk),
            b"IEND" => break,
            _ => {}
        }

        i = data_end + 4; // skip the trailing CRC
    }

    let header = header?;
    if header.width == 0 || header.height == 0 {
        return None;
    }
    if !matches!(header.bit_depth, 8 | 16) {
        return None; // sub-byte depths unsupported for now
    }

    let channels = match header.color_type {
        0 => 1, // grayscale
        2 => 3, // RGB
        3 => 1, // palette index
        4 => 2, // grayscale + alpha
        6 => 4, // RGBA
        _ => return None,
    };
    let sample_bytes = (header.bit_depth / 8) as usize;
    let bpp = channels * sample_bytes; // bytes per pixel in the filtered stream
    let width = header.width as usize;
    let height = header.height as usize;
    let stride = width.checked_mul(bpp)?;

    // Inflate the concatenated IDAT stream.
    let mut inflated = Vec::new();
    ZlibDecoder::new(&idat[..])
        .read_to_end(&mut inflated)
        .ok()?;
    if inflated.len() < height.checked_mul(stride + 1)? {
        return None;
    }

    let raw = png_unfilter(&inflated, height, stride, bpp)?;

    // Expand into PDF color/data + optional 8-bit alpha soft mask. For 16-bit
    // samples we keep only the high byte (8-bit output).
    let sample = |pixel: &[u8], channel: usize| -> u8 { pixel[channel * sample_bytes] };
    let mut data: Vec<u8> = Vec::with_capacity(width * height * 3);
    let mut alpha: Vec<u8> = Vec::new();
    let color_space;

    match header.color_type {
        0 => {
            color_space = ColorSpace::DeviceGray;
            for pixel in raw.chunks_exact(bpp) {
                data.push(sample(pixel, 0));
            }
        }
        2 => {
            color_space = ColorSpace::DeviceRgb;
            for pixel in raw.chunks_exact(bpp) {
                data.push(sample(pixel, 0));
                data.push(sample(pixel, 1));
                data.push(sample(pixel, 2));
            }
        }
        3 => {
            color_space = ColorSpace::DeviceRgb;
            let has_alpha = !trns.is_empty();
            for pixel in raw.chunks_exact(bpp) {
                let index = sample(pixel, 0) as usize;
                let rgb = palette.get(index).copied().unwrap_or([0, 0, 0]);
                data.extend_from_slice(&rgb);
                if has_alpha {
                    alpha.push(trns.get(index).copied().unwrap_or(255));
                }
            }
        }
        4 => {
            color_space = ColorSpace::DeviceGray;
            for pixel in raw.chunks_exact(bpp) {
                data.push(sample(pixel, 0));
                alpha.push(sample(pixel, 1));
            }
        }
        6 => {
            color_space = ColorSpace::DeviceRgb;
            for pixel in raw.chunks_exact(bpp) {
                data.push(sample(pixel, 0));
                data.push(sample(pixel, 1));
                data.push(sample(pixel, 2));
                alpha.push(sample(pixel, 3));
            }
        }
        _ => return None,
    }

    Some(DecodedImage {
        width: header.width,
        height: header.height,
        color_space,
        bits_per_component: 8,
        filter: ImageFilter::Flate,
        data,
        smask: (!alpha.is_empty()).then_some(alpha),
    })
}

/// Reverse PNG scanline filtering in place, returning the raw sample bytes
/// (without the per-scanline filter-type byte).
fn png_unfilter(inflated: &[u8], height: usize, stride: usize, bpp: usize) -> Option<Vec<u8>> {
    let mut out = vec![0u8; height * stride];
    for row in 0..height {
        let filter = inflated[row * (stride + 1)];
        let src = &inflated[row * (stride + 1) + 1..row * (stride + 1) + 1 + stride];
        let (prev, cur) = out.split_at_mut(row * stride);
        let cur = &mut cur[..stride];
        let prev_row = if row > 0 {
            &prev[(row - 1) * stride..row * stride]
        } else {
            &[][..]
        };

        for x in 0..stride {
            let raw = src[x];
            let a = if x >= bpp { cur[x - bpp] } else { 0 };
            let b = if row > 0 { prev_row[x] } else { 0 };
            let c = if row > 0 && x >= bpp {
                prev_row[x - bpp]
            } else {
                0
            };
            let value = match filter {
                0 => raw,
                1 => raw.wrapping_add(a),
                2 => raw.wrapping_add(b),
                3 => raw.wrapping_add(((a as u16 + b as u16) / 2) as u8),
                4 => raw.wrapping_add(paeth(a, b, c)),
                _ => return None,
            };
            cur[x] = value;
        }
    }
    Some(out)
}

fn paeth(a: u8, b: u8, c: u8) -> u8 {
    let p = a as i32 + b as i32 - c as i32;
    let pa = (p - a as i32).abs();
    let pb = (p - b as i32).abs();
    let pc = (p - c as i32).abs();
    if pa <= pb && pa <= pc {
        a
    } else if pb <= pc {
        b
    } else {
        c
    }
}

// --- base64 ---------------------------------------------------------------

/// Decode standard base64 (with `+/` and optional `=` padding), ignoring ASCII
/// whitespace. Returns `None` on any invalid character.
fn base64_decode(input: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(input.len() / 4 * 3);
    let mut buffer = 0u32;
    let mut bits = 0u32;
    for byte in input.bytes() {
        let value = match byte {
            b'A'..=b'Z' => byte - b'A',
            b'a'..=b'z' => byte - b'a' + 26,
            b'0'..=b'9' => byte - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            b'=' => break,
            b' ' | b'\n' | b'\r' | b'\t' => continue,
            _ => return None,
        };
        buffer = (buffer << 6) | value as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((buffer >> bits) as u8);
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_round_trips_known_vectors() {
        assert_eq!(base64_decode("").unwrap(), b"");
        assert_eq!(base64_decode("Zg==").unwrap(), b"f");
        assert_eq!(base64_decode("Zm8=").unwrap(), b"fo");
        assert_eq!(base64_decode("Zm9v").unwrap(), b"foo");
        assert_eq!(base64_decode("Zm9vYmFy").unwrap(), b"foobar");
        // Embedded whitespace is ignored.
        assert_eq!(base64_decode("Zm9v\nYmFy").unwrap(), b"foobar");
        assert!(base64_decode("****").is_none());
    }

    #[test]
    fn parses_jpeg_dimensions_from_sof() {
        // Minimal marker stream: SOI, an APP0 segment, then an SOF0 declaring a
        // 16x8 RGB image. Enough for the header scanner; not a full JPEG.
        let mut bytes = vec![0xFF, 0xD8]; // SOI
        bytes.extend_from_slice(&[0xFF, 0xE0, 0x00, 0x04, 0x00, 0x00]); // APP0, len 4
        bytes.extend_from_slice(&[0xFF, 0xC0, 0x00, 0x11, 0x08]); // SOF0, len 17, prec 8
        bytes.extend_from_slice(&[0x00, 0x08]); // height 8
        bytes.extend_from_slice(&[0x00, 0x10]); // width 16
        bytes.push(0x03); // 3 components
        bytes.extend_from_slice(&[0u8; 9]); // component specs (ignored)

        let image = decode(&bytes).expect("jpeg header should parse");
        assert_eq!((image.width, image.height), (16, 8));
        assert_eq!(image.color_space, ColorSpace::DeviceRgb);
        assert_eq!(image.filter, ImageFilter::Dct);
    }

    /// Build a valid PNG for a small RGB(A) image so the decoder can be tested
    /// end-to-end without a fixture file or an encoder dependency.
    fn build_png(width: u32, height: u32, color_type: u8, pixels: &[u8]) -> Vec<u8> {
        use flate2::{write::ZlibEncoder, Compression};
        use std::io::Write;

        let channels = match color_type {
            0 => 1,
            2 => 3,
            6 => 4,
            _ => panic!("unsupported test color type"),
        };
        let stride = width as usize * channels;
        let mut raw = Vec::new();
        for row in 0..height as usize {
            raw.push(0); // filter: none
            raw.extend_from_slice(&pixels[row * stride..(row + 1) * stride]);
        }
        let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(&raw).unwrap();
        let idat = encoder.finish().unwrap();

        let mut ihdr = Vec::new();
        ihdr.extend_from_slice(&width.to_be_bytes());
        ihdr.extend_from_slice(&height.to_be_bytes());
        ihdr.extend_from_slice(&[8, color_type, 0, 0, 0]);

        let mut png = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
        push_chunk(&mut png, b"IHDR", &ihdr);
        push_chunk(&mut png, b"IDAT", &idat);
        push_chunk(&mut png, b"IEND", &[]);
        png
    }

    fn push_chunk(png: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
        png.extend_from_slice(&(data.len() as u32).to_be_bytes());
        png.extend_from_slice(kind);
        png.extend_from_slice(data);
        let mut crc_input = kind.to_vec();
        crc_input.extend_from_slice(data);
        png.extend_from_slice(&crc32(&crc_input).to_be_bytes());
    }

    fn crc32(data: &[u8]) -> u32 {
        let mut crc = 0xFFFF_FFFFu32;
        for &byte in data {
            crc ^= byte as u32;
            for _ in 0..8 {
                let mask = (crc & 1).wrapping_neg();
                crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
            }
        }
        !crc
    }

    #[test]
    fn decodes_rgb_png() {
        // 2x1: red, green.
        let png = build_png(2, 1, 2, &[255, 0, 0, 0, 255, 0]);
        let image = decode(&png).expect("png should decode");
        assert_eq!((image.width, image.height), (2, 1));
        assert_eq!(image.color_space, ColorSpace::DeviceRgb);
        assert_eq!(image.filter, ImageFilter::Flate);
        assert_eq!(image.data, vec![255, 0, 0, 0, 255, 0]);
        assert!(image.smask.is_none());
    }

    #[test]
    fn decodes_rgba_png_into_rgb_plus_soft_mask() {
        // 2x1 RGBA: opaque red, half-transparent green.
        let png = build_png(2, 1, 6, &[255, 0, 0, 255, 0, 255, 0, 128]);
        let image = decode(&png).expect("png should decode");
        assert_eq!(image.color_space, ColorSpace::DeviceRgb);
        assert_eq!(image.data, vec![255, 0, 0, 0, 255, 0]);
        assert_eq!(image.smask, Some(vec![255, 128]));
    }

    #[test]
    fn png_up_filter_is_reconstructed() {
        // 1x2 grayscale with an Up filter on the second row exercises unfiltering.
        use flate2::{write::ZlibEncoder, Compression};
        use std::io::Write;
        let raw = [0u8, 10, /* row0: none, value 10 */ 2, 5 /* row1: up, +5 => 15 */];
        let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(&raw).unwrap();
        let idat = encoder.finish().unwrap();
        let mut ihdr = Vec::new();
        ihdr.extend_from_slice(&1u32.to_be_bytes());
        ihdr.extend_from_slice(&2u32.to_be_bytes());
        ihdr.extend_from_slice(&[8, 0, 0, 0, 0]);
        let mut png = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
        push_chunk(&mut png, b"IHDR", &ihdr);
        push_chunk(&mut png, b"IDAT", &idat);
        push_chunk(&mut png, b"IEND", &[]);

        let image = decode(&png).expect("png should decode");
        assert_eq!(image.color_space, ColorSpace::DeviceGray);
        assert_eq!(image.data, vec![10, 15]);
    }

    #[test]
    fn unknown_formats_return_none() {
        assert!(decode(b"not an image").is_none());
        assert!(load_image("data:text/plain,hello", None).is_none());
    }

    fn base64_encode(data: &[u8]) -> String {
        const ALPHABET: &[u8; 64] =
            b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut out = String::new();
        for chunk in data.chunks(3) {
            let b = [
                chunk[0],
                *chunk.get(1).unwrap_or(&0),
                *chunk.get(2).unwrap_or(&0),
            ];
            out.push(ALPHABET[(b[0] >> 2) as usize] as char);
            out.push(ALPHABET[(((b[0] & 0x03) << 4) | (b[1] >> 4)) as usize] as char);
            out.push(if chunk.len() > 1 {
                ALPHABET[(((b[1] & 0x0f) << 2) | (b[2] >> 6)) as usize] as char
            } else {
                '='
            });
            out.push(if chunk.len() > 2 {
                ALPHABET[(b[2] & 0x3f) as usize] as char
            } else {
                '='
            });
        }
        out
    }

    #[test]
    fn base64_encode_decode_round_trip() {
        for sample in [&b"foobar"[..], &b"\x00\xff\x10\x20\x30"[..], &b"a"[..]] {
            assert_eq!(base64_decode(&base64_encode(sample)).unwrap(), sample);
        }
    }

    #[test]
    fn renders_png_data_uri_as_a_pdf_image() {
        // 2x1 RGBA (opaque red, half-transparent green) through the whole engine.
        let png = build_png(2, 1, 6, &[255, 0, 0, 255, 0, 255, 0, 128]);
        let uri = format!("data:image/png;base64,{}", base64_encode(&png));
        let html = format!("<p>before</p><img src=\"{uri}\" width=\"40\"><p>after</p>");

        let pdf = crate::Engine::new()
            .render_html(&html, crate::RenderOptions::default())
            .expect("render should succeed");
        let text = String::from_utf8_lossy(&pdf);

        assert!(text.contains("/Subtype /Image"));
        assert!(text.contains("/Width 2"));
        assert!(text.contains("/XObject"));
        // RGBA input becomes an RGB image plus a grayscale soft mask.
        assert!(text.contains("/SMask"));
        assert!(text.contains("/ColorSpace /DeviceGray"));
    }

    /// Walk the flow tree and return the first resolved `ImageBox`.
    fn first_image(document: &crate::html::Document) -> crate::box_tree::ImageBox {
        use crate::box_tree::BoxChild;
        fn find(children: &[BoxChild]) -> Option<crate::box_tree::ImageBox> {
            for child in children {
                match child {
                    BoxChild::Image(image) => return Some(image.clone()),
                    BoxChild::Block(block) => {
                        if let Some(found) = find(&block.children) {
                            return Some(found);
                        }
                    }
                    _ => {}
                }
            }
            None
        }
        find(&document.flow.as_ref().expect("flow").children).expect("an image box")
    }

    #[test]
    fn css_width_height_override_html_attributes() {
        // 2x1 opaque image. HTML says 40px; CSS says 60pt (should win), and CSS
        // also sets an explicit height so the aspect ratio is not preserved.
        let png = build_png(2, 1, 2, &[255, 0, 0, 0, 255, 0]);
        let uri = format!("data:image/png;base64,{}", base64_encode(&png));
        let html = format!(
            "<img src=\"{uri}\" width=\"40\" height=\"20\" style=\"width:60pt;height:12pt\">"
        );

        let mut document = crate::html::parse(&html);
        crate::html::resolve_images(&mut document, None);
        let image = first_image(&document);

        assert_eq!(image.width, 60.0, "CSS width should win over the attribute");
        assert_eq!(image.height, 12.0, "CSS height should win over the attribute");
    }

    #[test]
    fn css_width_only_preserves_aspect_ratio() {
        // 2x1 image with only a CSS width: height follows the 2:1 intrinsic ratio.
        let png = build_png(2, 1, 2, &[255, 0, 0, 0, 255, 0]);
        let uri = format!("data:image/png;base64,{}", base64_encode(&png));
        let html = format!("<img src=\"{uri}\" style=\"width:80pt\">");

        let mut document = crate::html::parse(&html);
        crate::html::resolve_images(&mut document, None);
        let image = first_image(&document);

        assert_eq!(image.width, 80.0);
        assert_eq!(image.height, 40.0, "height follows the 2:1 intrinsic ratio");
    }

    #[test]
    fn image_only_document_is_not_empty() {
        let png = build_png(1, 1, 2, &[10, 20, 30]);
        let uri = format!("data:image/png;base64,{}", base64_encode(&png));
        let pdf = crate::Engine::new()
            .render_html(&format!("<img src=\"{uri}\">"), crate::RenderOptions::default())
            .expect("an image-only document should render");
        assert!(pdf.starts_with(b"%PDF-1.7\n"));
    }
}
