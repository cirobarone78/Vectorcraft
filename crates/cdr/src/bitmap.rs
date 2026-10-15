//! Bitmaps: `bmp ` records (the pixels) and bitmap objects (where they go).
//!
//! Layouts follow libcdr: `CDRParser::readBmp` and `CommonParser::readRImage` for the record,
//! `CDRStylesCollector::collectBmp` for the pixel formats (1-bit black and white, 8-bit grey,
//! palettes, 24 and 32 bits a pixel) and `CDRParserState::getBMPColor` for their colour models.
//! Rows are stored bottom first (libcdr writes them as is into a bottom-up BMP). Before version 5
//! the record holds a whole BMP file.

use std::collections::HashMap;

use crate::Error;
use crate::geometry::CorelColor;
use crate::riff::CdrVersion;

/// Most pixels a bitmap may have (as `vectorcraft_doc::MAX_RASTER_ALLOC` bytes of RGBA).
pub const MAX_PIXELS: u64 = vectorcraft_doc::MAX_RASTER_ALLOC / 4;

/// Decoded pixels, top row first, four bytes (RGBA) a pixel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bitmap {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

fn short(at: usize, len: usize) -> Error {
    Error::Record { record: "bmp", message: format!("ends at byte {at} of {len}") }
}

fn u32_at(d: &[u8], at: usize) -> Result<u32, Error> {
    d.get(at..at + 4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]])).ok_or_else(|| short(at, d.len()))
}

/// Decode a `bmp ` record: its image id and pixels.
pub fn decode_bmp(data: &[u8], version: CdrVersion) -> Result<(u32, Bitmap), Error> {
    let wide = version >= CdrVersion(600);
    let (id, mut at) = if wide {
        (u32_at(data, 0)?, 4)
    } else {
        let b = data.get(0..2).ok_or_else(|| short(0, data.len()))?;
        (u32::from(u16::from_le_bytes([b[0], b[1]])), 2)
    };
    if version < CdrVersion(500) {
        return Ok((id, from_bmp_file(data.get(at..).unwrap_or_default())?));
    }
    at += if version < CdrVersion(600) {
        14
    } else if version < CdrVersion(700) {
        46
    } else {
        50
    };
    let model = u32_at(data, at)?;
    let width = u32_at(data, at + 8)?;
    let height = u32_at(data, at + 12)?.max(1);
    let bpp = u32_at(data, at + 20)?;
    let size = u32_at(data, at + 28)? as usize;
    at += 64;
    let mut palette = Vec::new();
    // Model 99 (8-bit, no palette) is how version 17+ files store masks and greyscale images.
    if bpp < 24 && model != 5 && model != 6 && model != 99 {
        let n = data.get(at + 2..at + 4).map(|b| usize::from(u16::from_le_bytes([b[0], b[1]]))).ok_or_else(|| short(at, data.len()))?;
        at += 4;
        let n = n.min(data.len().saturating_sub(at) / 3);
        for i in 0..n {
            if let Some(&[b, g, r]) = data.get(at + 3 * i..at + 3 * i + 3).and_then(|s| <&[u8; 3]>::try_from(s).ok()) {
                palette.push(u32::from(b) | u32::from(g) << 8 | u32::from(r) << 16);
            }
        }
        at += 3 * n;
    }
    let pixels = data.get(at..at.saturating_add(size)).ok_or_else(|| short(at, data.len()))?;
    let mut bitmap = decode_pixels(model, width, height, bpp, &palette, pixels)?;
    // Version 17+ (read from the user's samples): an alpha channel may follow the pixels, an image
    // record of its own (`RI`, model 99, 8 bits a pixel, the same size) whose data starts 78 bytes
    // in; 0 is transparent and 255 opaque.
    if let Some(alpha) = data.get(at.saturating_add(size)..).and_then(|rest| alpha_block(rest, width, height)) {
        for (px, a) in bitmap.rgba.chunks_exact_mut(4).zip(alpha) {
            px[3] = a;
        }
    }
    Ok((id, bitmap))
}

/// An alpha channel after an image's pixels: top row first, `width` × `height` bytes.
fn alpha_block(rest: &[u8], width: u32, height: u32) -> Option<Vec<u8>> {
    if rest.get(0..2) != Some(b"RI") {
        return None;
    }
    let (model, w, h, bpp) = (u32_at(rest, 14).ok()?, u32_at(rest, 22).ok()?, u32_at(rest, 26).ok()?, u32_at(rest, 34).ok()?);
    let (line, size) = (u32_at(rest, 38).ok()? as usize, u32_at(rest, 42).ok()? as usize);
    if model != 99 || bpp != 8 || w != width || h != height || line < w as usize {
        return None;
    }
    let data = rest.get(78..78usize.checked_add(size)?)?;
    let (w, h) = (w as usize, h as usize);
    let mut out = vec![255u8; w * h];
    // Rows bottom first, as the image's.
    for j in 0..h {
        let row = data.get(j * line..j * line + w)?;
        out.get_mut((h - 1 - j) * w..(h - j) * w)?.copy_from_slice(row);
    }
    Some(out)
}

fn from_bmp_file(file: &[u8]) -> Result<Bitmap, Error> {
    let mut reader = image::ImageReader::with_format(std::io::Cursor::new(file), image::ImageFormat::Bmp);
    let mut limits = image::Limits::default();
    limits.max_alloc = Some(vectorcraft_doc::MAX_RASTER_ALLOC);
    reader.limits(limits);
    let img = reader.decode().map_err(|e| Error::Record { record: "bmp", message: e.to_string() })?.to_rgba8();
    Ok(Bitmap { width: img.width(), height: img.height(), rgba: img.into_raw() })
}

/// libcdr's `getBMPColor`: a bitmap's colour model as a CorelDRAW colour model (`None`: the value
/// is already 0xRRGGBB).
fn color_model(bitmap_model: u32) -> Option<u16> {
    Some(match bitmap_model {
        1 | 10 => 0x05,
        2 => 0x04,
        3 => 0x03,
        4 => 0x06,
        5 => 0x09,
        6 => 0x08,
        7 => 0x07,
        11 => 0x12,
        8 | 9 => return None,
        _ => 0x00,
    })
}

/// Pixels of `bpp` bits in bitmap colour model `model`, rows bottom first and padded alike.
pub fn decode_pixels(model: u32, width: u32, height: u32, bpp: u32, palette: &[u32], data: &[u8]) -> Result<Bitmap, Error> {
    let too_big = || Error::Record { record: "bmp", message: format!("{width} × {height} pixels is too large") };
    if width == 0 || u64::from(width) * u64::from(height) > MAX_PIXELS {
        return Err(too_big());
    }
    let (w, h) = (width as usize, height as usize);
    let line = data.len() / h;
    let cdr_model = color_model(model);
    let mut cache: HashMap<u32, [u8; 4]> = HashMap::new();
    let mut rgba = |value: u32| -> [u8; 4] {
        *cache.entry(value).or_insert_with(|| match cdr_model {
            None => [(value >> 16) as u8, (value >> 8) as u8, value as u8, 255],
            Some(m) => CorelColor { model: m, palette: 0, value }.to_color().map_or([0, 0, 0, 255], |c| c.to_rgba8(1.0)),
        })
    };
    let mut out = vec![0u8; w * h * 4];
    for j in 0..h {
        let row = data.get(j * line..(j + 1) * line).unwrap_or_default();
        let dest = (h - 1 - j) * w * 4;
        let mut put = |k: usize, px: [u8; 4]| {
            if let Some(d) = out.get_mut(dest + 4 * k..dest + 4 * k + 4) {
                d.copy_from_slice(&px);
            }
        };
        match (model, bpp) {
            (6, _) => {
                for k in 0..w {
                    let bit = row.get(k / 8).is_some_and(|b| b & (0x80 >> (k % 8)) != 0);
                    put(k, if bit { [255, 255, 255, 255] } else { [0, 0, 0, 255] });
                }
            }
            (5, _) => row.iter().take(w).enumerate().for_each(|(k, &c)| put(k, rgba(u32::from(c)))),
            // An 8-bit mask or grey image (version 17+): 0 black, 255 white.
            (99, 8) => row.iter().take(w).enumerate().for_each(|(k, &c)| put(k, [c, c, c, 255])),
            _ if !palette.is_empty() => {
                for (k, &c) in row.iter().take(w).enumerate() {
                    let entry = palette.get(usize::from(c)).or(palette.last()).copied().unwrap_or(0);
                    put(k, rgba(entry));
                }
            }
            (_, 24) => {
                for (k, p) in row.chunks_exact(3).take(w).enumerate() {
                    put(k, rgba(u32::from(p[0]) | u32::from(p[1]) << 8 | u32::from(p[2]) << 16));
                }
            }
            (_, 32) => {
                for (k, p) in row.chunks_exact(4).take(w).enumerate() {
                    put(k, rgba(u32::from_le_bytes([p[0], p[1], p[2], p[3]])));
                }
            }
            _ => return Err(Error::Record { record: "bmp", message: format!("{bpp}-bit pixels in colour model {model} are not read") }),
        }
    }
    Ok(Bitmap { width, height, rgba: out })
}

impl Bitmap {
    /// The pixels as a PNG file.
    pub fn to_png(&self) -> Result<Vec<u8>, Error> {
        let mut out = Vec::new();
        image::write_buffer_with_format(
            &mut std::io::Cursor::new(&mut out),
            &self.rgba,
            self.width,
            self.height,
            image::ExtendedColorType::Rgba8,
            image::ImageFormat::Png,
        )
        .map_err(|e| Error::Record { record: "bmp", message: e.to_string() })?;
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pixel_formats_read_bottom_row_first() {
        // 2 × 2, 24-bit RGB (stored B, G, R), bottom row first: red, green / blue, white.
        let data = [0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 255];
        let b = decode_pixels(1, 2, 2, 24, &[], &data).unwrap();
        assert_eq!(b.rgba[..8], [0, 0, 255, 255, 255, 255, 255, 255], "the top row is the last stored");
        assert_eq!(b.rgba[8..], [255, 0, 0, 255, 0, 255, 0, 255]);
        // 1-bit: set bits are white.
        let bw = decode_pixels(6, 3, 1, 1, &[], &[0b1010_0000]).unwrap();
        assert_eq!(bw.rgba, [255, 255, 255, 255, 0, 0, 0, 255, 255, 255, 255, 255]);
        // 8-bit grey (bitmap model 5 is CorelDRAW's grey, 255 white).
        let g = decode_pixels(5, 2, 1, 8, &[], &[0, 255]).unwrap();
        assert_eq!(g.rgba, [0, 0, 0, 255, 255, 255, 255, 255]);
        // A palette, an index past its end taking the last entry.
        let p = decode_pixels(1, 2, 1, 8, &[0x0000ff, 0x00ff00], &[0, 9]).unwrap();
        assert_eq!(p.rgba, [0, 0, 255, 255, 0, 255, 0, 255]);
        assert!(decode_pixels(1, 0, 1, 24, &[], &[]).is_err());
        assert!(decode_pixels(1, u32::MAX, u32::MAX, 24, &[], &[]).is_err());
        assert!(decode_pixels(1, 1, 1, 16, &[], &[0, 0]).is_err());
        let mask = decode_pixels(99, 2, 1, 8, &[], &[0, 255, 0, 0]).unwrap();
        assert_eq!(mask.rgba, [0, 0, 0, 255, 255, 255, 255, 255]);
    }

    #[test]
    fn records_by_version_and_png() {
        // Version 9: id, 50 bytes, the image header (model, size, bpp, data size), then the data.
        let mut rec = 7u32.to_le_bytes().to_vec();
        rec.extend_from_slice(&[0; 50]);
        for v in [1u32, 0, 1, 1, 0, 24, 0, 4] {
            rec.extend_from_slice(&v.to_le_bytes());
        }
        rec.extend_from_slice(&[0; 32]);
        rec.extend_from_slice(&[10, 20, 30, 0]);
        let (id, b) = decode_bmp(&rec, CdrVersion(900)).unwrap();
        assert_eq!((id, b.width, b.height, &b.rgba[..]), (7, 1, 1, &[30, 20, 10, 255][..]));
        let png = b.to_png().unwrap();
        assert!(png.starts_with(b"\x89PNG"));
        for n in 0..rec.len() {
            let _ = decode_bmp(&rec[..n], CdrVersion(900));
        }
        // Version 17: an alpha block after the pixels (an image record of model 99).
        let mut with_alpha = rec.clone();
        with_alpha.extend_from_slice(b"RI");
        with_alpha.extend_from_slice(&[0; 12]);
        for (at, v) in [(14usize, 99u32), (22, 1), (26, 1), (34, 8), (38, 4), (42, 4)] {
            with_alpha.resize(with_alpha.len().max(rec.len() + at + 4), 0);
            with_alpha[rec.len() + at..rec.len() + at + 4].copy_from_slice(&v.to_le_bytes());
        }
        with_alpha.resize(rec.len() + 78, 0);
        with_alpha.extend_from_slice(&[128, 0, 0, 0]);
        let (_, b) = decode_bmp(&with_alpha, CdrVersion(1700)).unwrap();
        assert_eq!(b.rgba, [30, 20, 10, 128]);
        // Version 4: a whole BMP file after a 16-bit id.
        let mut file = Vec::new();
        image::RgbaImage::from_pixel(2, 1, image::Rgba([1, 2, 3, 255]))
            .write_to(&mut std::io::Cursor::new(&mut file), image::ImageFormat::Bmp)
            .unwrap();
        let mut v4 = 3u16.to_le_bytes().to_vec();
        v4.extend_from_slice(&file);
        let (id, b) = decode_bmp(&v4, CdrVersion(400)).unwrap();
        assert_eq!((id, b.width, &b.rgba[..4]), (3, 2, &[1, 2, 3, 255][..]));
    }
}
