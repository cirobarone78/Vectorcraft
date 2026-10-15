//! Translation of CorelDRAW geometry and style records into VectorCraft's model: Bézier path
//! records become [`PathData`], `trfd` matrices become [`Affine`]s, `outl` outlines become
//! [`StrokeLayer`]s and `fild` solid and fountain fills become [`FillLayer`]s.
//!
//! Record layouts follow libcdr's `CDRParser.cpp` (`readLoda`, `readLineAndCurve`, `readPath`,
//! `readTrfd`, `readOutl`, `readFild`, `readColor`), version by version. How they become drawing
//! follows `CDRContentCollector.cpp` and `CommonParser::processPath`.
//!
//! # Coordinate spaces
//!
//! CorelDRAW stores geometry in inches with y pointing up and the origin at the page centre:
//! coordinates are 16-bit thousandths of an inch before version 6 and 32-bit 1/254000 inch (0.1 µm)
//! from version 6 on. VectorCraft documents are points (1/72 in) with y pointing down and the
//! origin at the page's top-left corner. A path is mapped by its object's `trfd` matrices first,
//! still in CorelDRAW space, then by [`PageSpace::to_document`]:
//! `x_pt = 72 (x + w/2)`, `y_pt = 72 (h/2 − y)` for a `w × h` inch page, as libcdr does with
//! `-offsetX` and `height - y`.
//!
//! # What is approximate
//!
//! Anything VectorCraft cannot show exactly is translated to the nearest equivalent and noted in
//! [`Shape::notes`]: conical fountains become linear and square ones radial (as libcdr does),
//! palette and spot colours (models `0x19` and `0x0e`) outside CorelDRAW's built-in palettes
//! ([`crate::palettes`]) fall back to black, and a stretched (calligraphic) nib scales the width as libcdr does. Pattern,
//! bitmap, texture and PostScript fills are not translated. Polygons, bitmaps and other record
//! kinds are counted in [`Translation::skipped`], not translated.
//!
//! Rectangles (with their corner radii) and ellipses (also arcs and pies) become paths. Artistic
//! text becomes point type anchored at its record's point, paragraph text area type in its frame;
//! the characters, fonts, sizes, fills and alignment come from [`crate::text`].

use std::collections::BTreeMap;

use vectorcraft_color::{Color, Gradient, GradientGeom, GradientKind, GradientPaint, GradientStop, Paint};
use vectorcraft_doc::appearance::{Appearance, AppearanceItem, Arrowhead, Dash, FillLayer, LineCap, LineJoin, StrokeLayer};
use vectorcraft_doc::text::{CharStyle, Justify, ParaStyle, TextKind, TextObject, TextRun};
use vectorcraft_geom::{Affine, BezPath, FillRule, PathData, Point, Rect, Vec2};

use crate::Error;
use crate::palettes::Resolved;
use crate::riff::{CdrVersion, Chunk, FourCc, List, Riff};
use crate::text::{self, Font, Ink, Paragraph, Pen, Styles, TextStyle};

/// Points per inch.
const PT_PER_IN: f64 = 72.0;
/// Most points read from one path record (each is at least 5 bytes, so data caps it too).
const MAX_POINTS: usize = 1 << 20;
/// Most gradient stops, dash entries or `loda` / `trfd` arguments read from one record.
const MAX_LIST: usize = 4096;

// ---------------------------------------------------------------------------------------------
// Bounded little-endian reader

struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
    what: &'static str,
    version: CdrVersion,
}

impl<'a> Reader<'a> {
    fn new(data: &'a [u8], what: &'static str, version: CdrVersion) -> Self {
        Self { data, pos: 0, what, version }
    }

    fn short(&self) -> Error {
        Error::Record { record: self.what, message: format!("ends at byte {} of {}", self.pos, self.data.len()) }
    }

    fn bytes<const N: usize>(&mut self) -> Result<[u8; N], Error> {
        let end = self.pos.checked_add(N).ok_or_else(|| self.short())?;
        let b = self.data.get(self.pos..end).ok_or_else(|| self.short())?;
        self.pos = end;
        let mut out = [0; N];
        out.copy_from_slice(b);
        Ok(out)
    }

    fn u8(&mut self) -> Result<u8, Error> {
        Ok(self.bytes::<1>()?[0])
    }
    fn u16(&mut self) -> Result<u16, Error> {
        Ok(u16::from_le_bytes(self.bytes()?))
    }
    fn i16(&mut self) -> Result<i16, Error> {
        Ok(i16::from_le_bytes(self.bytes()?))
    }
    fn u32(&mut self) -> Result<u32, Error> {
        Ok(u32::from_le_bytes(self.bytes()?))
    }
    fn i32(&mut self) -> Result<i32, Error> {
        Ok(i32::from_le_bytes(self.bytes()?))
    }
    fn f64(&mut self) -> Result<f64, Error> {
        let v = f64::from_le_bytes(self.bytes()?);
        if v.is_finite() { Ok(v) } else { Err(Error::Record { record: self.what, message: format!("non-finite number at byte {}", self.pos - 8) }) }
    }

    fn skip(&mut self, n: usize) -> Result<(), Error> {
        self.seek(self.pos.saturating_add(n))
    }

    fn seek(&mut self, pos: usize) -> Result<(), Error> {
        if pos > self.data.len() {
            return Err(self.short());
        }
        self.pos = pos;
        Ok(())
    }

    fn remaining(&self) -> usize {
        self.data.len().saturating_sub(self.pos)
    }

    fn wide(&self) -> bool {
        self.version >= CdrVersion(600)
    }

    /// libcdr's `readUnsigned`: 16 bits before version 6, 32 after.
    fn unsigned(&mut self) -> Result<u32, Error> {
        if self.wide() { self.u32() } else { self.u16().map(u32::from) }
    }

    /// libcdr's `readInteger`.
    fn integer(&mut self) -> Result<i32, Error> {
        if self.wide() { self.i32() } else { self.i16().map(i32::from) }
    }

    /// A coordinate in inches.
    fn coord(&mut self) -> Result<f64, Error> {
        if self.wide() { Ok(f64::from(self.i32()?) / 254_000.0) } else { Ok(f64::from(self.i16()?) / 1000.0) }
    }

    /// An angle in radians: tenths of a degree before version 6, millionths after.
    fn angle(&mut self) -> Result<f64, Error> {
        if self.wide() { Ok(f64::from(self.i32()?).to_radians() / 1_000_000.0) } else { Ok(f64::from(self.i16()?).to_radians() / 10.0) }
    }

    /// A 16.16 fixed-point number (version 4 transforms), as libcdr decodes it.
    fn fixed(&mut self) -> Result<f64, Error> {
        let v = self.u32()?;
        let int = f64::from((v >> 16) as u16 as i16);
        Ok(int + f64::from(v & 0xffff) / f64::from(0xffff_u32))
    }
}

/// X6+ records may be 16-byte stubs pointing into `content/data/*`, which this reader does not
/// follow (libcdr's `_redirectX6Chunk`).
fn redirected(data: &[u8], version: CdrVersion) -> bool {
    version >= CdrVersion(1600) && data.len() == 0x10
}

/// Where record data comes from: the RIFF stream, or for an X6+ stub (a 16-byte record of
/// `{data file, length, offset}`, libcdr's `_redirectX6Chunk`) the `content/data` file it names,
/// in `content/dataFileList.dat` order.
#[derive(Clone, Copy)]
pub struct Records<'a, 'r> {
    pub riff: &'a Riff<'r>,
    pub streams: &'a [Vec<u8>],
}

impl<'a> Records<'a, '_> {
    /// A decoded record's data, followed into its data file when it is a stub.
    pub fn data(&self, rec: &crate::riff::Record) -> Result<&'a [u8], Error> {
        let missing = |m: String| Error::Record { record: "data file", message: m };
        let d = self.riff.data(rec).ok_or_else(|| missing("record outside its stream".into()))?;
        if !redirected(d, self.riff.version) {
            return Ok(d);
        }
        let u = |i: usize| d.get(i..i + 4).map_or(0, |b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]));
        let (stream, len, offset) = (u(0), u(4) as usize, u(8) as usize);
        // libcdr reads on from the stub itself for this stream number.
        if stream == u32::MAX {
            return Ok(d.get(8..).unwrap_or_default());
        }
        let file = self
            .streams
            .get(stream as usize)
            .ok_or_else(|| missing(format!("a {} record is in data file {stream}, which the package doesn't have", rec.kind)))?;
        file.get(offset..offset.saturating_add(len)).ok_or_else(|| missing(format!("a {} record runs past the end of data file {stream}", rec.kind)))
    }
}

// ---------------------------------------------------------------------------------------------
// Colour

/// A colour as CorelDRAW stores it: a model, a palette (for palette colours) and four packed bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct CorelColor {
    pub model: u16,
    pub palette: u16,
    pub value: u32,
}

impl CorelColor {
    fn read(r: &mut Reader<'_>) -> Result<Self, Error> {
        let v = r.version;
        if v >= CdrVersion(500) {
            let mut model = r.u16()?;
            if model == 0x01 && v >= CdrVersion(1300) {
                model = 0x19;
            }
            // CorelDRAW X7 writes model 0x1e for palette 0x1e without the palette field.
            if model == 0x1e {
                return Ok(Self { model: 0x19, palette: 0x1e, value: r.u32()? });
            }
            let palette = r.u16()?;
            r.skip(4)?;
            Ok(Self { model, palette, value: r.u32()? })
        } else if v >= CdrVersion(400) {
            let model = r.u16()?;
            let [c, m, y, k] = [r.u16()?, r.u16()?, r.u16()?, r.u16()?].map(|x| u32::from(x & 0xff));
            r.skip(2)?;
            Ok(Self { model, palette: 0, value: c | m << 8 | y << 16 | k << 24 })
        } else {
            let model = u16::from(r.u8()?);
            Ok(Self { model, palette: 0, value: r.u32()? })
        }
    }

    /// The colour in its own model where VectorCraft has one (CMYK, RGB, grey, Lab), else as RGB.
    /// Palette and spot colours are looked up in CorelDRAW's built-in palettes
    /// ([`crate::palettes`]); one that isn't there is `Err` with the reason.
    pub fn to_color(&self) -> Result<Color, String> {
        let [c0, c1, c2, c3] = self.value.to_le_bytes();
        let f = |b: u8, max: f32| f32::from(b) / max;
        Ok(match self.model {
            // CMYK in percent.
            0x01 | 0x02 | 0x15 => Color::cmyk(f(c0, 100.0).min(1.0), f(c1, 100.0).min(1.0), f(c2, 100.0).min(1.0), f(c3, 100.0).min(1.0)),
            // CMYK in 0..255.
            0x03 | 0x11 => Color::cmyk(f(c0, 255.0), f(c1, 255.0), f(c2, 255.0), f(c3, 255.0)),
            0x04 => Color::cmyk(f(c0, 255.0), f(c1, 255.0), f(c2, 255.0), 0.0),
            // RGB, stored blue first.
            0x05 => Color::rgb8(c2, c1, c0),
            0x06 => hsb(u16::from_le_bytes([c0, c1]), f(c2, 255.0), f(c3, 255.0)),
            0x07 => hls(u16::from_le_bytes([c0, c1]), f(c2, 255.0), f(c3, 255.0)),
            0x08 => Color::gray(if c0 == 0 { 0.0 } else { 1.0 }),
            // Grey level: 255 is white; VectorCraft's grey is ink.
            0x09 => Color::gray(1.0 - f(c0, 255.0)),
            0x0b => yiq(c0, c1, c2),
            0x0c => Color::lab(f(c0, 255.0) * 100.0, f32::from(c1 as i8), f32::from(c2 as i8)),
            0x12 => Color::lab(f(c0, 255.0) * 100.0, f32::from(c1) - 128.0, f32::from(c2) - 128.0),
            // Registration: every plate at this tint.
            0x14 => {
                let t = f(c0, 100.0).min(1.0);
                Color::cmyk(t, t, t, t)
            }
            // Palette and spot colours: an entry of one of CorelDRAW's built-in palettes.
            0x19 | 0x0e => match crate::palettes::resolve(self.model, self.palette, self.value) {
                Some(Resolved::Cmyk([c, m, y, k])) => {
                    Color::cmyk(f(c, 100.0).min(1.0), f(m, 100.0).min(1.0), f(y, 100.0).min(1.0), f(k, 100.0).min(1.0))
                }
                Some(Resolved::Rgb([r, g, b])) => Color::rgb8(r, g, b),
                Some(Resolved::Lab([l, a, b])) => Color::lab(f(l, 255.0) * 100.0, f32::from(a) - 128.0, f32::from(b) - 128.0),
                None => {
                    return Err(format!(
                        "palette colour (model {:#x}, palette {:#x}, entry {}) is not in CorelDRAW's built-in palettes",
                        self.model,
                        self.palette,
                        self.value & 0xffff
                    ));
                }
            },
            m => return Err(format!("unknown colour model {m:#x}")),
        })
    }
}

fn hue_rgb(hue: u16) -> (f32, f32, f32) {
    let h = f32::from(hue % 360) / 60.0;
    let x = 1.0 - (h % 2.0 - 1.0).abs();
    match h as u8 {
        0 => (1.0, x, 0.0),
        1 => (x, 1.0, 0.0),
        2 => (0.0, 1.0, x),
        3 => (0.0, x, 1.0),
        4 => (x, 0.0, 1.0),
        _ => (1.0, 0.0, x),
    }
}

fn hsb(hue: u16, s: f32, v: f32) -> Color {
    let (r, g, b) = hue_rgb(hue);
    let mix = |c: f32| v * (1.0 - s + s * c);
    Color::rgb(mix(r), mix(g), mix(b))
}

fn hls(hue: u16, l: f32, s: f32) -> Color {
    let (r, g, b) = hue_rgb(hue);
    let chroma = (1.0 - (2.0 * l - 1.0).abs()) * s;
    let m = l - chroma / 2.0;
    Color::rgb(m + chroma * r, m + chroma * g, m + chroma * b)
}

/// YIQ in CorelDRAW's 0..255 encoding (100 is zero), as libcdr decodes it.
fn yiq(y: u8, i: u8, q: u8) -> Color {
    let centred = |v: u8| {
        let v = f32::from(v) - 100.0;
        if v <= 0.0 { v / 100.0 } else { v / 155.0 }
    };
    let (y, i, q) = (centred(y) * 0.5 + 0.5, centred(i) * 0.5957, centred(q) * 0.5226);
    let c = |v: f32| v.clamp(0.0, 1.0);
    Color::rgb(c(y + 0.9563 * i + 0.6210 * q), c(y - 0.2127 * i - 0.6474 * q), c(y - 1.1070 * i + 1.7046 * q))
}

fn color_or_black(c: &CorelColor, notes: &mut Vec<String>) -> Color {
    c.to_color().unwrap_or_else(|why| {
        notes.push(format!("{why}; drawn black"));
        Color::BLACK
    })
}

// ---------------------------------------------------------------------------------------------
// Transforms and page space

/// The `trfd` matrices of an object, in CorelDRAW space (inches, y up), in the order the file
/// gives them. libcdr applies them first to last.
pub fn decode_trfd(data: &[u8], version: CdrVersion) -> Result<Vec<Affine>, Error> {
    let mut r = Reader::new(data, "trfd", version);
    let _chunk_length = r.unsigned()?;
    let count = r.unsigned()? as usize;
    let start = r.unsigned()? as usize;
    r.seek(start)?;
    let mut offsets = Vec::new();
    for _ in 0..count.min(MAX_LIST) {
        offsets.push(r.unsigned()? as usize);
    }
    let div = if version < CdrVersion(600) { 1000.0 } else { 254_000.0 };
    let mut out = Vec::new();
    for off in offsets {
        r.seek(off)?;
        if version >= CdrVersion(1300) {
            r.skip(8)?;
        }
        // Type 8 is a matrix; others (0x10, envelopes and the like) are not transforms.
        if r.u16()? != 0x08 {
            continue;
        }
        let m = if version >= CdrVersion(500) {
            if version >= CdrVersion(600) {
                r.skip(6)?;
            }
            [r.f64()?, r.f64()?, r.f64()? / div, r.f64()?, r.f64()?, r.f64()? / div]
        } else {
            [r.fixed()?, r.fixed()?, f64::from(r.i32()?) / 1000.0, r.fixed()?, r.fixed()?, f64::from(r.i32()?) / 1000.0]
        };
        out.push(corel_matrix(m));
    }
    Ok(out)
}

/// libcdr's `CDRTransform(v0, v1, x0, v3, v4, y0)`: `x' = v0·x + v1·y + x0`, `y' = v3·x + v4·y + y0`.
pub fn corel_matrix([v0, v1, x0, v3, v4, y0]: [f64; 6]) -> Affine {
    Affine::new([v0, v3, v1, v4, x0, y0])
}

/// The matrices of a `trfd` record applied in order, as one transform.
pub fn compose(matrices: &[Affine]) -> Affine {
    matrices.iter().fold(Affine::IDENTITY, |acc, m| *m * acc)
}

/// A page's size in inches. CorelDRAW centres its coordinate origin on the page.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PageSpace {
    pub width_in: f64,
    pub height_in: f64,
}

impl PageSpace {
    /// From the document units `metadata.xml` uses (0.1 µm, 1/254000 in).
    pub fn from_metadata(page: &crate::PageInfo) -> Self {
        Self { width_in: page.width as f64 / 254_000.0, height_in: page.height as f64 / 254_000.0 }
    }

    /// CorelDRAW page space (inches, y up, origin at the centre) to VectorCraft document space
    /// (points, y down, origin at the top-left corner).
    pub fn to_document(&self) -> Affine {
        Affine::new([PT_PER_IN, 0.0, 0.0, -PT_PER_IN, PT_PER_IN * self.width_in / 2.0, PT_PER_IN * self.height_in / 2.0])
    }

    /// Page size in points.
    pub fn size_pt(&self) -> (f64, f64) {
        (self.width_in * PT_PER_IN, self.height_in * PT_PER_IN)
    }
}

/// Document page size from an `mcfg` record.
pub fn decode_mcfg(data: &[u8], version: CdrVersion) -> Result<PageSpace, Error> {
    let mut r = Reader::new(data, "mcfg", version);
    if version >= CdrVersion(1300) {
        r.skip(12)?;
    } else if version >= CdrVersion(900) {
        r.skip(4)?;
    } else if version >= CdrVersion(600) && version < CdrVersion(700) {
        r.skip(0x1c)?;
    }
    let (w, h) = if version < CdrVersion(400) {
        r.skip(2)?;
        let (x0, y0, x1, y1) = (r.coord()?, r.coord()?, r.coord()?, r.coord()?);
        ((x1 - x0).abs(), (y1 - y0).abs())
    } else {
        (r.coord()?, r.coord()?)
    };
    Ok(PageSpace { width_in: w, height_in: h })
}

// ---------------------------------------------------------------------------------------------
// Objects (`loda`) and Bézier paths

/// What a `loda` record draws.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShapeKind {
    Rectangle,
    Ellipse,
    /// Lines and curves: a Bézier path record.
    Curve,
    /// The version 6+ path record (chunk type 0x25).
    Path,
    ArtisticText,
    Bitmap,
    ParagraphText,
    Polygon,
    /// A B-spline; its points are in the object's `ppdt` record.
    Spline,
    Other(u32),
}

impl ShapeKind {
    fn from_chunk_type(t: u32, version: CdrVersion) -> Self {
        let modern = version >= CdrVersion(400);
        match (t, modern) {
            (0x01, true) | (0x00, false) => Self::Rectangle,
            (0x02, true) | (0x01, false) => Self::Ellipse,
            (0x03, true) | (0x02, false) => Self::Curve,
            (0x04, true) | (0x03, false) => Self::ArtisticText,
            (0x05, true) | (0x04, false) => Self::Bitmap,
            (0x06, true) | (0x05, false) => Self::ParagraphText,
            (0x25, _) => Self::Path,
            (0x14, _) => Self::Polygon,
            (0x26, _) => Self::Spline,
            (t, _) => Self::Other(t),
        }
    }

    pub fn is_bezier(self) -> bool {
        matches!(self, Self::Curve | Self::Path)
    }
}

/// The parts of a `loda` record this module uses.
#[derive(Debug, Clone, PartialEq)]
pub struct Loda {
    pub kind: ShapeKind,
    /// Offset of the coordinates argument in the record.
    pub coords: Option<usize>,
    pub fill_id: Option<u32>,
    pub outline_id: Option<u32>,
    pub style_id: Option<u32>,
    /// A page size argument (in the page's own `loda`).
    pub page: Option<PageSpace>,
    /// A polygon's rotational symmetry (argument 0x2af8).
    pub polygon: Option<PolygonXf>,
    /// The object's (or layer's) name (argument 1000).
    pub name: Option<String>,
    /// PowerClip contents: the `spnd` id of a group in the document's `clpt` list (argument 8005).
    pub clip: Option<u32>,
    /// Opacity from the object's transparency (argument 8000), 0..1.
    pub opacity: Option<f32>,
    /// A linear fading transparency (argument 8000), in object space.
    pub fade: Option<Fade>,
}

/// A transparency that fades along a line: opacity `opacity.0` at its first end to `opacity.1` at
/// its second, held beyond both.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Fade {
    pub from: Point,
    pub to: Point,
    pub opacity: (f32, f32),
}

impl Fade {
    /// The fade in another space.
    pub fn transformed(self, xf: Affine) -> Self {
        Self { from: xf * self.from, to: xf * self.to, ..self }
    }
}

/// Read a `loda` (or `lobj`) record's argument table.
pub fn decode_loda(data: &[u8], version: CdrVersion) -> Result<Loda, Error> {
    let mut r = Reader::new(data, "loda", version);
    let _chunk_length = r.unsigned()?;
    let count = (r.unsigned()? as usize).min(MAX_LIST);
    let start_args = r.unsigned()? as usize;
    let start_types = r.unsigned()? as usize;
    let kind = ShapeKind::from_chunk_type(r.unsigned()?, version);
    r.seek(start_args)?;
    let mut offsets = Vec::with_capacity(count);
    for _ in 0..count {
        offsets.push(r.unsigned()? as usize);
    }
    // The type table is stored last argument first.
    r.seek(start_types)?;
    let mut types = Vec::with_capacity(count);
    for _ in 0..count {
        types.push(r.unsigned()?);
    }
    types.reverse();
    let mut loda = Loda {
        kind,
        coords: None,
        fill_id: None,
        outline_id: None,
        style_id: None,
        page: None,
        polygon: None,
        name: None,
        clip: None,
        opacity: None,
        fade: None,
    };
    let modern = version >= CdrVersion(400);
    for (&off, &ty) in offsets.iter().zip(&types) {
        r.seek(off)?;
        match ty {
            0x1e => loda.coords = Some(off),
            // Before version 4 these hold the fill and outline inline (not translated here).
            0x14 if modern => loda.fill_id = Some(r.u32()?).filter(|&id| id != 0),
            0x0a if modern => loda.outline_id = Some(r.u32()?).filter(|&id| id != 0),
            0xc8 => loda.style_id = Some(r.unsigned()?).filter(|&id| id != 0),
            0x4aba => loda.page = Some(PageSpace { width_in: r.coord()?, height_in: r.coord()? }),
            0x2af8 => loda.polygon = Some(PolygonXf::read(&mut r)?),
            1000 => loda.name = read_name(&mut r),
            8005 => loda.clip = Some(r.u32()?).filter(|&id| id != 0),
            8000 => {
                let arg = r.data.get(off..).unwrap_or_default();
                loda.opacity = transparency(arg);
                loda.fade = fade(arg);
            }
            _ => {}
        }
    }
    Ok(loda)
}

/// A name argument: UTF-16 from version 12, else eight-bit; up to its terminating zero.
fn read_name(r: &mut Reader<'_>) -> Option<String> {
    let rest = r.data.get(r.pos..)?;
    let name = if r.version >= CdrVersion(1200) {
        let len = rest.chunks_exact(2).position(|c| c == [0, 0]).unwrap_or(rest.len() / 2).min(1024);
        crate::text::decode_utf16(rest.get(..len * 2)?)
    } else {
        let len = rest.iter().position(|&b| b == 0).unwrap_or(rest.len()).min(1024);
        crate::text::decode_8bit(rest.get(..len)?, crate::text::charset::ANSI)
    };
    Some(name).filter(|n| !n.trim().is_empty())
}

/// An object's transparency (argument 8000) as opacity. Read from the user's version 17 samples (no
/// published layout): a block tagged `0x514`, the transparency in percent as a double at byte 69
/// and, for a fading transparency, its other end at byte 77 (0 when uniform). A fade is taken at its
/// average, as VectorCraft has no gradient opacity on an object.
pub fn transparency(arg: &[u8]) -> Option<f32> {
    let tag = arg.get(0..4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))?;
    let dbl = |i: usize| arg.get(i..i + 8).map(|b| f64::from_le_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]]));
    let (start, end) = (dbl(69)?, dbl(77)?);
    let ok = |v: f64| v.is_finite() && (0.0..=100.0).contains(&v);
    if tag != 0x514 || !ok(start) || !ok(end) {
        return None;
    }
    let percent = if end > 0.0 { (start + end) / 2.0 } else { start };
    Some((1.0 - percent / 100.0) as f32)
}

/// A linear fading transparency (argument 8000), when the block places one: fountain type 1 (a
/// little-endian u32 at byte 45) and the line's two ends as four 32-bit coordinates from byte 49,
/// in object space, the double at byte 77 at the first end and the one at byte 69 at the second.
/// Read from the user's version 17 samples; a fade without ends, or of another
/// type, is left at [`transparency`]'s average.
pub fn fade(arg: &[u8]) -> Option<Fade> {
    let u32_at = |i: usize| arg.get(i..i + 4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]));
    let dbl = |i: usize| arg.get(i..i + 8).map(|b| f64::from_le_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]]));
    if u32_at(0)? != 0x514 {
        return None;
    }
    let c = |i: usize| u32_at(i).map(|v| f64::from(v as i32) / 254_000.0);
    let (from, to) = (Point::new(c(49)?, c(53)?), Point::new(c(57)?, c(61)?));
    let (start, end) = (dbl(69)?, dbl(77)?);
    let ok = |v: f64| v.is_finite() && (0.0..=100.0).contains(&v);
    if !ok(start) || !ok(end) || end == 0.0 {
        return None;
    }
    let opacity = |pct: f64| (1.0 - pct / 100.0) as f32;
    // The first percentage is the line's far end's (measured against CorelDRAW's previews).
    (u32_at(45)? == 1 && from != to).then_some(Fade { from, to, opacity: (opacity(end), opacity(start)) })
}

/// An envelope (`envd`): the outline an object's box is fitted to, in the space after the
/// object's first transform (measured on a user's file: its corner is that transform's offset,
/// and the remaining transforms take it to the object's box on the page). Read from a
/// user's version 14 file (libcdr skips the record): a `loda`-style table of two arguments, one
/// byte of node type per point and the points themselves (as a curve's), told apart by their
/// lengths. `None` for anything else.
pub fn decode_envd(data: &[u8], version: CdrVersion) -> Option<BezPath> {
    let mut r = Reader::new(data, "envd", version);
    let len = (r.unsigned().ok()? as usize).min(data.len());
    let n = r.unsigned().ok()? as usize;
    let start_args = r.unsigned().ok()? as usize;
    if n != 2 {
        return None;
    }
    r.seek(start_args).ok()?;
    let (a, b) = (r.unsigned().ok()? as usize, r.unsigned().ok()? as usize);
    let (first, second) = (a.min(b), a.max(b));
    let (first_len, second_len) = (second.checked_sub(first)?, len.checked_sub(second)?);
    let point_size = if r.wide() { 8 } else { 4 };
    // The node types are the shorter argument, one byte per point.
    let ((types_at, count), points_at) =
        if first_len * point_size <= second_len { ((first, first_len), second) } else { ((second, second_len), first) };
    let count = count.min(MAX_POINTS);
    if count < 2 {
        return None;
    }
    let types = data.get(types_at..types_at + count)?.to_vec();
    r.seek(points_at).ok()?;
    let mut points = Vec::with_capacity(count);
    for _ in 0..count {
        points.push(Point::new(r.coord().ok()?, r.coord().ok()?));
    }
    let path = CorelPath { points, types }.to_bezpath();
    (path.elements().len() > 2).then_some(path)
}

/// A fill transform (`ftil`): six doubles, its translation in 1/254000 inch (libcdr's
/// `readFtil`). `None` for anything not finite, flat or huge.
pub fn decode_ftil(data: &[u8]) -> Option<Affine> {
    let mut d = [0.0f64; 6];
    for (i, x) in d.iter_mut().enumerate() {
        let b = data.get(i * 8..i * 8 + 8)?;
        *x = f64::from_le_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]]);
    }
    // x' = v0 x + v1 y + v2, y' = v3 x + v4 y + v5.
    let [v0, v1, v2, v3, v4, v5] = d;
    let det = v0 * v4 - v1 * v3;
    (d.iter().all(|v| v.is_finite()) && det.abs() > 1e-9 && [v0, v1, v3, v4].iter().all(|v| v.abs() < 1e4))
        .then(|| Affine::new([v0, v3, v1, v4, v2 / 254_000.0, v5 / 254_000.0]))
}

/// A Bézier path record: points (inches, CorelDRAW space) and one node-type byte each.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CorelPath {
    pub points: Vec<Point>,
    pub types: Vec<u8>,
}

/// Node-type bits (libcdr's `processPath`).
pub mod node {
    /// The node closes its subpath.
    pub const CLOSED: u8 = 0x08;
    /// Smooth node.
    pub const SMOOTH: u8 = 0x10;
    /// Symmetrical node.
    pub const SYMMETRIC: u8 = 0x20;
    /// The two high bits: 00 moves, 01 draws a line, 10 ends a curve, 11 is a control point.
    pub const SEGMENT: u8 = 0xc0;
    pub const MOVE: u8 = 0x00;
    pub const LINE: u8 = 0x40;
    pub const CURVE: u8 = 0x80;
    pub const CONTROL: u8 = 0xc0;
}

impl CorelPath {
    /// Read the coordinates argument of a curve (`kind` [`ShapeKind::Curve`]) or path
    /// ([`ShapeKind::Path`]) record, starting at `offset` in the `loda` data.
    pub fn decode(data: &[u8], offset: usize, kind: ShapeKind, version: CdrVersion) -> Result<Self, Error> {
        let mut r = Reader::new(data, "path coordinates", version);
        r.seek(offset)?;
        let mut count = match kind {
            ShapeKind::Curve => {
                let n = usize::from(r.u16()?);
                r.skip(2)?;
                n
            }
            ShapeKind::Path => {
                r.skip(4)?;
                let n = usize::from(r.u16()?) + usize::from(r.u16()?);
                r.skip(16)?;
                n
            }
            other => return Err(Error::Record { record: "path coordinates", message: format!("{other:?} is not a Bézier path") }),
        };
        let point_size = if r.wide() { 9 } else { 5 };
        // As libcdr does, a count larger than the data holds is cut to what is there.
        count = count.min(r.remaining() / point_size).min(MAX_POINTS);
        let mut points = Vec::with_capacity(count);
        for _ in 0..count {
            points.push(Point::new(r.coord()?, r.coord()?));
        }
        let mut types = Vec::with_capacity(count);
        for _ in 0..count {
            types.push(r.u8()?);
        }
        Ok(Self { points, types })
    }

    /// The path in CorelDRAW space. Follows `processPath`: control points (type `11`) wait for
    /// the next curve end, which uses the first two; a curve end with fewer becomes a line. A
    /// subpath closes when its last drawn node carries [`node::CLOSED`].
    pub fn to_bezpath(&self) -> BezPath {
        let mut bp = BezPath::new();
        let mut controls: Vec<Point> = Vec::new();
        let mut open = false;
        for (&p, &t) in self.points.iter().zip(&self.types) {
            let closes = t & node::CLOSED != 0;
            match t & node::SEGMENT {
                node::MOVE => {
                    controls.clear();
                    bp.move_to(p);
                    open = true;
                    continue;
                }
                node::CONTROL => {
                    controls.push(p);
                    continue;
                }
                _ if !open => {
                    // A segment without a start point starts a subpath there instead.
                    bp.move_to(p);
                    open = true;
                }
                node::LINE => bp.line_to(p),
                _ => match controls.as_slice() {
                    [c1, c2, ..] => bp.curve_to(*c1, *c2, p),
                    _ => bp.line_to(p),
                },
            }
            controls.clear();
            if closes {
                bp.close_path();
                open = false;
            }
        }
        bp
    }

    /// The path in document space: through `object` (the composed `trfd` matrices) then `page`.
    pub fn to_path_data(&self, object: Affine, page: &PageSpace) -> PathData {
        let mut bp = self.to_bezpath();
        bp.apply_affine(page.to_document() * object);
        PathData::from_bezpath(&bp)
    }
}

/// Points per inch in a type's text space, which is in points.
const TEXT_SCALE: f64 = 1.0 / PT_PER_IN;
/// Curve flattening tolerance for arcs, in inches.
const ARC_TOLERANCE: f64 = 1e-4;
/// The cubic handle length of a quarter circle, as a fraction of its radius.
const KAPPA: f64 = 0.552_284_749_830_793_4;

/// A rectangle record's coordinates argument (libcdr's `readRectangle`): the rectangle from the
/// origin to its corner, with the corners rounded (type 0), scalloped (1) or chamfered (2).
pub fn decode_rectangle(data: &[u8], offset: usize, version: CdrVersion) -> Result<BezPath, Error> {
    let mut r = Reader::new(data, "rectangle", version);
    r.seek(offset)?;
    let doubles = version >= CdrVersion(1500);
    let coord = |r: &mut Reader<'_>| if doubles { r.f64().map(|v| v / 254_000.0) } else { r.coord() };
    let (x0, y0) = (coord(&mut r)?, coord(&mut r)?);
    let (mut sx, mut sy, mut corner) = (1.0, 1.0, 0u8);
    let radii = if !doubles {
        let r3 = r.coord()?;
        if version < CdrVersion(900) { [r3; 4] } else { [r3, r.coord()?, r.coord()?, r.coord()?] }
    } else {
        let nonzero = |v: f64| if v.abs() > 1e-9 { v } else { 1.0 };
        sx = nonzero(r.f64()?);
        sy = nonzero(r.f64()?);
        let relative = r.u8()? == 0;
        r.skip(7)?;
        let read = |r: &mut Reader<'_>| if relative { r.f64() } else { coord(r) };
        let r3 = read(&mut r)?;
        corner = r.u8()?;
        r.skip(15)?;
        let r2 = read(&mut r)?;
        r.skip(16)?;
        let r1 = read(&mut r)?;
        r.skip(16)?;
        let r0 = read(&mut r)?;
        // Relative radii are fractions of the shorter half-side.
        let k = if relative { (x0 * sx / 2.0).abs().min((y0 * sy / 2.0).abs()) } else { 1.0 };
        [r3 * k, r2 * k, r1 * k, r0 * k]
    };
    Ok(rectangle(x0, y0, radii, corner, (sx, sy)))
}

/// libcdr's rectangle outline: from the origin corner round to `(x0, y0)` and back, radii
/// `[r3, r2, r1, r0]` (by its naming), corners rounded with circular arcs where libcdr draws a
/// quadratic curve.
fn rectangle(x0: f64, y0: f64, [r3, r2, r1, r0]: [f64; 4], corner: u8, (sx, sy): (f64, f64)) -> BezPath {
    let p = |x: f64, y: f64| Point::new(x, y);
    let mut bp = BezPath::new();
    // From the point on one edge, round the corner `c`, to the point on the next edge.
    let turn = |bp: &mut BezPath, a: Point, c: Point, b: Point| match corner {
        1 => bp.quad_to(a + (b - c), b),
        2 => bp.line_to(b),
        _ => bp.curve_to(a + (c - a) * KAPPA, b + (c - b) * KAPPA, b),
    };
    bp.move_to(if r0 > 0.0 { p(0.0, -r0 / sy) } else { p(0.0, 0.0) });
    if r1 > 0.0 {
        let a = p(0.0, y0 + r1 / sy);
        bp.line_to(a);
        turn(&mut bp, a, p(0.0, y0), p(r1 / sx, y0));
    } else {
        bp.line_to(p(0.0, y0));
    }
    if r2 > 0.0 {
        let a = p(x0 - r2 / sx, y0);
        bp.line_to(a);
        turn(&mut bp, a, p(x0, y0), p(x0, y0 + r2 / sy));
    } else {
        bp.line_to(p(x0, y0));
    }
    if r3 > 0.0 {
        let a = p(x0, -r3 / sy);
        bp.line_to(a);
        turn(&mut bp, a, p(x0, 0.0), p(x0 - r3 / sx, 0.0));
    } else {
        bp.line_to(p(x0, 0.0));
    }
    if r0 > 0.0 {
        let a = p(r0 / sx, 0.0);
        bp.line_to(a);
        turn(&mut bp, a, p(0.0, 0.0), p(0.0, -r0 / sy));
    } else {
        bp.line_to(p(0.0, 0.0));
    }
    bp.close_path();
    bp
}

/// An ellipse record's coordinates argument (libcdr's `readEllipse`): the ellipse in the box from
/// the origin to `(x, y)`; with two different angles an arc from the first to the second, closed
/// through the centre for a pie.
pub fn decode_ellipse(data: &[u8], offset: usize, version: CdrVersion) -> Result<BezPath, Error> {
    let mut r = Reader::new(data, "ellipse", version);
    r.seek(offset)?;
    let (x, y) = (r.coord()?, r.coord()?);
    let (a1, a2) = (r.angle()?, r.angle()?);
    let pie = r.unsigned()? != 0;
    Ok(ellipse(x, y, a1, a2, pie))
}

fn ellipse(x: f64, y: f64, a1: f64, a2: f64, pie: bool) -> BezPath {
    let tau = std::f64::consts::TAU;
    let (cx, cy) = (x / 2.0, y / 2.0);
    let radii = Vec2::new(cx.abs(), cy.abs());
    let (a1, mut a2) = (a1.rem_euclid(tau), a2.rem_euclid(tau));
    // libcdr's parametrisation (the minus sign included): the arc runs with sweep off.
    let at = |a: f64| Point::new(cx + radii.x * a.cos(), cy - radii.y * a.sin());
    let mut bp = BezPath::new();
    let arc = |bp: &mut BezPath, from: Point, to: Point, large: bool| {
        let svg = vectorcraft_geom::kurbo::SvgArc { from, to, radii, x_rotation: 0.0, large_arc: large, sweep: false };
        match vectorcraft_geom::kurbo::Arc::from_svg_arc(&svg) {
            Some(a) => a.to_cubic_beziers(ARC_TOLERANCE, |c1, c2, p| bp.curve_to(c1, c2, p)),
            None => bp.line_to(to),
        }
    };
    if (a1 - a2).abs() > 1e-9 {
        if a2 < a1 {
            a2 += tau;
        }
        let (p0, p1) = (at(a1), at(a2));
        bp.move_to(p0);
        arc(&mut bp, p0, p1, a2 - a1 > std::f64::consts::PI);
        if pie {
            bp.line_to(Point::new(cx, cy));
            bp.close_path();
        }
    } else {
        let (p0, p1) = (at(a1), at(a2 + tau / 4.0));
        bp.move_to(p0);
        arc(&mut bp, p0, p1, false);
        arc(&mut bp, p1, p0, true);
        bp.close_path();
    }
    bp
}

/// A polygon's symmetry (libcdr's `readPolygonTransform` and `CDRPolygon`): the record's path is
/// one corner's outline, repeated `angles` times round the origin, `next` corners at a step (a
/// star when it doesn't divide `angles`), then scaled by (`rx`, `ry`) and moved to (`cx`, `cy`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PolygonXf {
    pub angles: u32,
    pub next: u32,
    pub rx: f64,
    pub ry: f64,
    pub cx: f64,
    pub cy: f64,
}

/// Most corners a polygon is expanded to (libcdr caps the elements, too).
const MAX_POLYGON_ANGLES: u32 = 1000;

impl PolygonXf {
    fn read(r: &mut Reader<'_>) -> Result<Self, Error> {
        let x3 = r.version >= CdrVersion(1300);
        if !x3 {
            r.skip(4)?;
        }
        let angles = r.u32()?;
        let mut next = r.u32()?;
        if next <= 1 {
            next = r.u32()?;
        } else {
            r.skip(4)?;
        }
        if x3 {
            r.skip(4)?;
        }
        Ok(Self { angles, next, rx: r.f64()?, ry: r.f64()?, cx: r.coord()?, cy: r.coord()? })
    }

    /// Expand one corner's `path` into the whole polygon.
    pub fn create(&self, path: &BezPath) -> BezPath {
        let n = self.angles.min(MAX_POLYGON_ANGLES);
        if n == 0 || path.elements().is_empty() {
            return path.clone();
        }
        let step = std::f64::consts::TAU / f64::from(n);
        // libcdr's CDRTransform(cos, sin, 0, -sin, cos, 0): a turn clockwise (y up) by `a`.
        let turn = |a: f64| corel_matrix([a.cos(), a.sin(), 0.0, -a.sin(), a.cos(), 0.0]);
        let mut out = path.clone();
        let mut copy = path.clone();
        let next = self.next % n.max(1);
        if next != 0 && !n.is_multiple_of(next) {
            let t = turn(f64::from(next) * step);
            for _ in 1..n {
                copy.apply_affine(t);
                append_joined(&mut out, &copy);
            }
        } else {
            let next = next.max(1);
            let (t, shift) = (turn(f64::from(next) * step), turn(step));
            for i in 0..next {
                if i > 0 {
                    copy.apply_affine(shift);
                    append_joined(&mut out, &copy);
                }
                for _ in 1..n / next {
                    copy.apply_affine(t);
                    append_joined(&mut out, &copy);
                }
                close_snapped(&mut out);
            }
        }
        close_snapped(&mut out);
        out.apply_affine(corel_matrix([self.rx, 0.0, self.cx, 0.0, self.ry, self.cy]));
        out
    }
}

/// Close the last subpath, its end moved onto its start when within the file's precision (so the
/// closing corner isn't doubled), unless it is already closed.
fn close_snapped(bp: &mut BezPath) {
    use vectorcraft_geom::PathEl;
    if matches!(bp.elements().last(), Some(PathEl::ClosePath) | None) {
        return;
    }
    let start = bp.elements().iter().rev().find_map(|e| if let PathEl::MoveTo(p) = e { Some(*p) } else { None });
    if let (Some(start), Some(last)) = (start, bp.elements_mut().last_mut()) {
        match last {
            PathEl::LineTo(p) | PathEl::QuadTo(_, p) | PathEl::CurveTo(_, _, p) if p.distance(start) < 1e-5 => *p = start,
            _ => {}
        }
    }
    bp.close_path();
}

/// Append `src` to `dst`, continuing the outline where `src` starts at `dst`'s end (libcdr's
/// collector drops such a move, which is how a polygon's corners join into one outline).
fn append_joined(dst: &mut BezPath, src: &BezPath) {
    let end = dst.elements().iter().rev().find_map(|e| match e {
        vectorcraft_geom::PathEl::MoveTo(p) | vectorcraft_geom::PathEl::LineTo(p) => Some(*p),
        vectorcraft_geom::PathEl::QuadTo(_, p) | vectorcraft_geom::PathEl::CurveTo(_, _, p) => Some(*p),
        vectorcraft_geom::PathEl::ClosePath => None,
    });
    let closed = matches!(dst.elements().last(), Some(vectorcraft_geom::PathEl::ClosePath));
    for (i, e) in src.elements().iter().enumerate() {
        match (i, e, end) {
            // Within two units of the file's precision (1/254000 in): rotated copies of stored
            // corners don't meet exactly.
            (0, vectorcraft_geom::PathEl::MoveTo(p), Some(q)) if !closed && p.distance(q) < 1e-5 => {}
            _ => dst.push(*e),
        }
    }
}

/// A `ppdt` record: a spline's points and knot flags (libcdr's `readPpdt`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SplineData {
    pub points: Vec<Point>,
    pub knots: Vec<u32>,
}

impl SplineData {
    pub fn decode(data: &[u8], version: CdrVersion) -> Result<Self, Error> {
        let mut r = Reader::new(data, "ppdt", version);
        let point_size = if r.wide() { 12 } else { 8 };
        let n = usize::from(r.u16()?);
        r.skip(4)?;
        let n = n.min(r.remaining() / point_size).min(MAX_POINTS);
        let mut points = Vec::with_capacity(n);
        for _ in 0..n {
            points.push(Point::new(r.coord()?, r.coord()?));
        }
        let mut knots = Vec::with_capacity(n);
        for _ in 0..n {
            knots.push(r.u32()?);
        }
        Ok(Self { points, knots })
    }

    /// The spline as Béziers (libcdr's `CDRSplineData::create`): a flagged point ends a piece; a
    /// piece of two points is a line, of three a quadratic curve, of more a clamped uniform cubic
    /// B-spline.
    pub fn to_bezpath(&self) -> BezPath {
        let mut bp = BezPath::new();
        let (Some(&first), true) = (self.points.first(), !self.knots.is_empty()) else { return bp };
        bp.move_to(first);
        let mut piece = vec![first];
        let flush = |bp: &mut BezPath, piece: &[Point]| match piece {
            [_, b] => bp.line_to(*b),
            [_, b, c] => bp.quad_to(*b, *c),
            p if p.len() > 3 => bspline_to(bp, p),
            _ => {}
        };
        for (i, &p) in self.points.iter().enumerate().skip(1).take(self.knots.len().saturating_sub(1)) {
            piece.push(p);
            if self.knots.get(i).is_some_and(|&k| k != 0) {
                flush(&mut bp, &piece);
                piece = vec![p];
            }
        }
        flush(&mut bp, &piece);
        bp
    }
}

/// Append the clamped uniform cubic B-spline with control points `p` (at least 4) as Bézier
/// segments, by inserting each interior knot until it has multiplicity 3 (Boehm's algorithm).
fn bspline_to(bp: &mut BezPath, p: &[Point]) {
    let n = p.len();
    // Knots 0,0,0,0,1,…,n-4,n-3,n-3,n-3,n-3 (libcdr's `CDRSplineToElement::knot`).
    let mut knots: Vec<f64> = (0..n + 4).map(|i| (i.saturating_sub(3)).min(n - 3) as f64).collect();
    let mut ctrl: Vec<Point> = p.to_vec();
    for u in 1..n - 3 {
        let u = u as f64;
        for _ in 1..3 {
            // Insert `u` once (it is in span k: knots[k] <= u < knots[k+1]).
            let Some(k) = (0..knots.len() - 1).rfind(|&i| knots[i] <= u && u < knots[i + 1]) else { break };
            let mut next = Vec::with_capacity(ctrl.len() + 1);
            for i in 0..=ctrl.len() {
                let point = if i + 3 <= k {
                    ctrl[i]
                } else if i > k {
                    ctrl[i - 1]
                } else {
                    let span = knots[i + 3] - knots[i];
                    let a = if span.abs() < 1e-12 { 0.0 } else { (u - knots[i]) / span };
                    ctrl[i - 1].lerp(ctrl[i], a)
                };
                next.push(point);
            }
            ctrl = next;
            knots.insert(k + 1, u);
        }
    }
    // Every interior knot now has multiplicity 3: control points 3i..3i+3 are segment i.
    for seg in ctrl.windows(4).step_by(3) {
        if let [_, c1, c2, end] = seg {
            bp.curve_to(*c1, *c2, *end);
        }
    }
}

/// Where a bitmap object puts its image (libcdr's `readBitmap`): image `id` in the rectangle
/// between (`x1`, `y1`) and (`x2`, `y2`), inches in object space.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BitmapPlacement {
    pub id: u32,
    pub x1: f64,
    pub y1: f64,
    pub x2: f64,
    pub y2: f64,
}

pub fn decode_bitmap_object(data: &[u8], offset: usize, version: CdrVersion) -> Result<BitmapPlacement, Error> {
    let mut r = Reader::new(data, "bitmap", version);
    r.seek(offset)?;
    if version < CdrVersion(600) {
        let (x1, y1) = (r.coord()?, r.coord()?);
        if version < CdrVersion(400) {
            r.skip(2)?;
        }
        r.skip(8)?;
        return Ok(BitmapPlacement { id: r.unsigned()?, x1, y1, x2: 0.0, y2: 0.0 });
    }
    let (x1, y1, x2, y2) = (r.coord()?, r.coord()?, r.coord()?, r.coord()?);
    r.skip(32)?;
    Ok(BitmapPlacement { id: r.unsigned()?, x1, y1, x2, y2 })
}

impl BitmapPlacement {
    /// Pixel space (`w` × `h`, y down) to object space: the image upright in its rectangle, top
    /// left at the corner with the smaller x and larger y (y is up). `None` for an empty one.
    pub fn pixels_to_object(&self, w: u32, h: u32) -> Option<Affine> {
        let (left, right) = (self.x1.min(self.x2), self.x1.max(self.x2));
        let (bottom, top) = (self.y1.min(self.y2), self.y1.max(self.y2));
        let (wi, hi) = (right - left, top - bottom);
        let ok = |v: f64| v.is_finite() && v > 1e-9;
        (ok(wi) && ok(hi) && w > 0 && h > 0)
            .then(|| Affine::translate((left, top)) * Affine::scale_non_uniform(wi / f64::from(w), -hi / f64::from(h)))
    }
}

/// Where a text record puts its type.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum TextFrame {
    /// Artistic text: point type whose first baseline starts at the object's origin, which its
    /// `trfd` places. (The record's coordinates argument is not a point: in the X4 sample its first
    /// four bytes are zero and the next four an id; libcdr reads them as one and never uses it.
    /// Each object's `bbox` reaches from the transform's origin by the font's ascent and descent.)
    Artistic,
    /// Paragraph text: area type in a frame from the origin, `width` right and `height` down.
    Paragraph { width: f64, height: f64 },
}

/// The coordinates argument of an artistic or paragraph text record (libcdr's
/// `readArtisticText`, `readParagraphText`).
pub fn decode_text_frame(data: &[u8], offset: usize, kind: ShapeKind, version: CdrVersion) -> Result<TextFrame, Error> {
    let mut r = Reader::new(data, "text", version);
    r.seek(offset)?;
    match kind {
        ShapeKind::ArtisticText => Ok(TextFrame::Artistic),
        ShapeKind::ParagraphText => {
            r.skip(4)?;
            Ok(TextFrame::Paragraph { width: r.coord()?.abs(), height: r.coord()?.abs() })
        }
        other => Err(Error::Record { record: "text", message: format!("{other:?} is not text") }),
    }
}

/// An `arrw` record (libcdr's `readArrw`, version 6+): an arrowhead's id and outline, its points
/// stored y before x.
pub fn decode_arrw(data: &[u8], version: CdrVersion) -> Result<(u32, BezPath), Error> {
    let mut r = Reader::new(data, "arrw", version);
    let id = r.u32()?;
    r.skip(4)?;
    let point_size = if r.wide() { 9 } else { 5 };
    let n = usize::from(r.u16()?);
    r.skip(4)?;
    let n = n.min(r.remaining().saturating_sub(1) / point_size).min(MAX_POINTS);
    let mut types = Vec::with_capacity(n);
    for _ in 0..n {
        types.push(r.u8()?);
    }
    r.skip(1)?;
    let mut points = Vec::with_capacity(n);
    for _ in 0..n {
        let (y, x) = (r.coord()?, r.coord()?);
        points.push(Point::new(x, y));
    }
    Ok((id, CorelPath { points, types }.to_bezpath()))
}

/// The preset VectorCraft draws for a CorelDRAW arrowhead: a filled triangle for a closed outline,
/// an open arrow for lines. (VectorCraft strokes take preset arrowheads, not arbitrary shapes.)
pub fn arrowhead_for(outline: &BezPath) -> Arrowhead {
    if outline.elements().iter().any(|e| matches!(e, vectorcraft_geom::PathEl::ClosePath)) { Arrowhead::Triangle } else { Arrowhead::ArrowOpen }
}

// ---------------------------------------------------------------------------------------------
// Outlines

/// An `outl` record.
#[derive(Debug, Clone, PartialEq)]
pub struct Outline {
    pub id: u32,
    /// Bit 0: no outline; bits 1–2: drawn (bit 2: dashed); bit 5: scales with the object.
    pub line_type: u16,
    /// 0 butt, 1 round, 2 square.
    pub cap: u16,
    /// 0 miter, 1 round, 2 bevel.
    pub join: u16,
    /// Width in inches.
    pub width_in: f64,
    /// Nib stretch, 1.0 for a round nib.
    pub stretch: f64,
    /// Nib angle in radians.
    pub angle: f64,
    pub color: CorelColor,
    /// Dash, gap, dash, gap… in multiples of the width.
    pub dashes: Vec<u16>,
    pub start_arrow: u32,
    pub end_arrow: u32,
    /// Version 13+: the ids at libcdr's offset, in case those are the arrows (see [`Self::arrows`]).
    pub alt_arrows: Option<(u32, u32)>,
}

impl Outline {
    pub const NONE: u16 = 0x01;
    pub const DASHED: u16 = 0x04;
    pub const SCALE_WITH_OBJECT: u16 = 0x20;

    /// The (start, end) arrowhead ids: the pair whose ids all name arrow records (`known`).
    pub fn arrows(&self, known: impl Fn(u32) -> bool) -> (u32, u32) {
        let names = |(a, b): (u32, u32)| (a != 0 || b != 0) && [a, b].iter().all(|&i| i == 0 || known(i));
        match self.alt_arrows {
            Some(alt) if !names((self.start_arrow, self.end_arrow)) && names(alt) => alt,
            _ => (self.start_arrow, self.end_arrow),
        }
    }

    pub fn decode(data: &[u8], version: CdrVersion) -> Result<Self, Error> {
        let mut r = Reader::new(data, "outl", version);
        let id = r.u32()?;
        if version >= CdrVersion(1300) {
            // A list of (id, length) blocks; the outline is the block with id 1.
            let mut skip = 0usize;
            loop {
                r.skip(skip)?;
                let block = r.u32()?;
                skip = r.u32()? as usize;
                if block == 1 {
                    break;
                }
            }
        }
        let line_type = r.u16()?;
        let cap = r.u16()?;
        let join = r.u16()?;
        if version >= CdrVersion(600) && version < CdrVersion(1300) {
            r.skip(2)?;
        }
        let width_in = r.coord()?;
        let stretch = f64::from(r.u16()?) / 100.0;
        if version >= CdrVersion(600) {
            r.skip(2)?;
        }
        let angle = r.angle()?;
        if version >= CdrVersion(1300) {
            r.skip(46)?;
        } else if version >= CdrVersion(600) {
            r.skip(52)?;
        }
        let color = CorelColor::read(&mut r)?;
        r.skip(if version < CdrVersion(600) { 10 } else { 16 })?;
        let n = usize::from(r.u16()?).min(r.remaining() / 2).min(MAX_LIST);
        let dash_at = r.pos;
        let mut dashes = Vec::with_capacity(n);
        for _ in 0..n {
            dashes.push(r.u16()?);
        }
        // The arrowhead ids follow the dashes: 20 bytes on before version 6, 22 after (libcdr). In
        // the X4 sample they are 24 bytes on; from X3's outline layout both are kept and the pair
        // that names arrow records is used ([`Outline::arrows`]).
        let at = |r: &mut Reader<'_>, gap: usize| -> Result<(u32, u32), Error> {
            r.seek(dash_at + gap)?;
            Ok((r.u32()?, r.u32()?))
        };
        let x3 = version >= CdrVersion(1300);
        let (start_arrow, end_arrow) = at(
            &mut r,
            if version < CdrVersion(600) {
                20
            } else if x3 {
                24
            } else {
                22
            },
        )
        .or_else(|e| if x3 { at(&mut r, 22) } else { Err(e) })?;
        let alt_arrows = if x3 { at(&mut r, 22).ok() } else { None };
        Ok(Self { id, line_type, cap, join, width_in, stretch, angle, color, dashes, start_arrow, end_arrow, alt_arrows })
    }

    /// The stroke this outline draws, or `None` for no outline. `object` is the object's
    /// transform, which scales the width when the outline scales with the object.
    pub fn to_stroke(&self, object: Affine, notes: &mut Vec<String>) -> Option<StrokeLayer> {
        if self.line_type & Self::NONE != 0 {
            return None;
        }
        let paint = Paint::solid(color_or_black(&self.color, notes));
        if self.line_type & 0x06 == 0 {
            // libcdr draws other line types as a black hairline.
            return Some(StrokeLayer::new(Paint::solid(Color::BLACK), 0.0));
        }
        let mut scale = 1.0;
        if self.line_type & Self::SCALE_WITH_OBJECT != 0 {
            let [a, b, c, d, _, _] = object.as_coeffs();
            scale = a.hypot(b).max(c.hypot(d));
        }
        if (self.stretch - 1.0).abs() > 1e-6 {
            notes.push(format!("calligraphic nib (stretch {:.0}%) drawn as a uniform width", self.stretch * 100.0));
            scale *= self.stretch;
        }
        let width = self.width_in * scale * PT_PER_IN;
        let mut stroke = StrokeLayer::new(paint, if width.is_finite() { width.max(0.0) } else { 0.0 });
        stroke.cap = match self.cap {
            1 => LineCap::Round,
            2 => LineCap::Square,
            _ => LineCap::Butt,
        };
        stroke.join = match self.join {
            1 => LineJoin::Round,
            2 => LineJoin::Bevel,
            _ => LineJoin::Miter,
        };
        if self.line_type & Self::DASHED != 0 && !self.dashes.is_empty() {
            let unit = if stroke.width > 0.0 { stroke.width } else { 1.0 };
            let pattern: Vec<f64> = self.dashes.iter().map(|&d| f64::from(d) * unit).collect();
            if pattern.iter().any(|&d| d > 0.0) {
                stroke.dash = Some(Dash { pattern, offset: 0.0, align_corners: false });
            }
        }
        Some(stroke)
    }
}

// ---------------------------------------------------------------------------------------------
// Fills

/// A fountain (gradient) fill.
#[derive(Debug, Clone, PartialEq)]
pub struct Fountain {
    /// 1 linear, 2 radial, 3 conical, 4 square.
    pub kind: u8,
    /// Edge pad in percent.
    pub edge_pad: i32,
    /// Angle in radians, counter-clockwise.
    pub angle: f64,
    /// Centre offset in percent of the half-size (radial, conical, square), y up.
    pub center: (i32, i32),
    pub mode: u8,
    /// Midpoint of a two-colour fountain, 0..1.
    pub midpoint: f64,
    /// (offset 0..1, colour).
    pub stops: Vec<(f64, CorelColor)>,
    /// Where the ramp lies on the object's box (version 17), `[cx, cy, ux, uy, vx, vy]` in units of
    /// the box's size: its centre's offset from the box centre (y up), and its axes (y down),
    /// edge pad included: a linear ramp runs along `u` from the first stop to the last, a radial
    /// one has `u` and `v` as its ellipse's diameters. Older files lay a fountain out from its
    /// angle, centre offset and pad alone.
    pub frame: Option<[f64; 6]>,
}

/// A `fild` record.
#[derive(Debug, Clone, PartialEq)]
pub enum CorelFill {
    None,
    Solid(CorelColor),
    Fountain(Fountain),
    /// A bitmap (9) or texture (11) fill: a bitmap of the file tiled over the object.
    Tiled(TiledFill),
    /// Pattern (7), full-colour (10), PostScript and others.
    Unsupported(u16),
}

/// A bitmap or texture fill: the `bmp ` record it tiles and the size of one tile.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TiledFill {
    pub bitmap: u32,
    pub width_in: f64,
    pub height_in: f64,
    /// A texture's resolution (its bitmap's pixels per inch), which sizes its tile.
    pub dpi: Option<f64>,
}

/// A bitmap tiled by fills: the pattern a tiled fill is drawn with ([`tile_name`]).
#[derive(Debug, Clone, PartialEq)]
pub struct Tile {
    pub bitmap: std::sync::Arc<crate::bitmap::Bitmap>,
    /// One tile's size in points.
    pub width_pt: f64,
    pub height_pt: f64,
}

/// The name of the pattern drawn for fill `id` (a bitmap or texture fill).
pub fn tile_name(id: u32) -> String {
    format!("CorelDRAW fill {id}")
}

impl CorelFill {
    /// Decode a `fild` record: its id and fill.
    pub fn decode(data: &[u8], version: CdrVersion) -> Result<(u32, Self), Error> {
        let x3 = version >= CdrVersion(1300);
        let mut r = Reader::new(data, "fild", version);
        let id = r.u32()?;
        if x3 {
            // The fill's own block header (8 bytes, tagged 0x514); CorelDRAW 2025 (version 29)
            // puts a 16-byte identifier block (0x960) before it.
            loop {
                let (tag, len) = (r.u32()?, r.u32()? as usize);
                if tag != 0x960 {
                    break;
                }
                r.skip(len)?;
            }
        }
        let fill = match r.u16()? {
            0 => Self::None,
            1 => {
                r.skip(if x3 { 13 } else { 2 })?;
                Self::Solid(CorelColor::read(&mut r)?)
            }
            2 => Self::Fountain(Self::fountain(&mut r)?),
            t @ (9 | 11) => Self::tiled(&mut r).map_or(Self::Unsupported(t), Self::Tiled),
            t => Self::Unsupported(t),
        };
        Ok((id, fill))
    }

    fn fountain(r: &mut Reader<'_>) -> Result<Fountain, Error> {
        let v = r.version;
        let x3 = v >= CdrVersion(1300);
        r.skip(if x3 { 8 } else { 2 })?;
        let kind = r.u8()?;
        let edge_pad = if x3 {
            r.skip(17)?;
            i32::from(r.i16()?)
        } else if v >= CdrVersion(600) {
            r.skip(19)?;
            r.i32()?
        } else {
            r.skip(11)?;
            i32::from(r.i16()?)
        };
        let angle = r.angle()?;
        let center = (r.integer()?, r.integer()?);
        if v >= CdrVersion(600) {
            r.skip(2)?;
        }
        let mode = (r.unsigned()? & 0xff) as u8;
        let midpoint = f64::from(r.u8()?) / 100.0;
        r.skip(1)?;
        let n = usize::from((r.unsigned()? & 0xffff) as u16).min(MAX_LIST);
        if x3 {
            r.skip(3)?;
        }
        let mut stops = Vec::with_capacity(n);
        for _ in 0..n {
            let color = CorelColor::read(r)?;
            if v >= CdrVersion(1500) {
                r.skip(26)?;
            } else if x3 {
                r.skip(5)?;
            }
            let offset = f64::from(r.unsigned()? & 0xffff) / 100.0;
            if x3 {
                r.skip(3)?;
            }
            stops.push((offset, color));
        }
        let frame = if v >= CdrVersion(1700) { Self::fountain_frame(r) } else { None };
        Ok(Fountain { kind, edge_pad, angle, center, mode, midpoint, stops, frame })
    }

    /// A bitmap or texture fill's bitmap and tile size, or `None` for a layout not read.
    ///
    /// Versions 13 and later: optional blocks (0x514 headers, 0x640 blocks) as libcdr skips them,
    /// then the tile's width and height at bytes 20 and 24 and the bitmap's id at byte 32,
    /// measured on a user's version 14 file whose four texture fills each name their own
    /// bitmap. Before: libcdr's layout.
    fn tiled(r: &mut Reader<'_>) -> Option<TiledFill> {
        let v = r.version;
        let size = |w: u32, h: u32| {
            let (w, h) = (f64::from(w as i32) / 254_000.0, f64::from(h as i32) / 254_000.0);
            (w > 0.0 && h > 0.0 && w < 1000.0 && h < 1000.0).then_some((w, h))
        };
        if v >= CdrVersion(1300) {
            loop {
                let at = r.pos;
                match r.u32().ok()? {
                    0x514 => {
                        r.skip(4).ok()?;
                    }
                    0x640 => {
                        let len = r.u32().ok()? as usize;
                        r.skip(len).ok()?;
                    }
                    _ => {
                        r.pos = at;
                        break;
                    }
                }
            }
            let start = r.pos;
            r.skip(20).ok()?;
            let (w, h) = size(r.u32().ok()?, r.u32().ok()?)?;
            r.pos = start + 32;
            let bitmap = r.u32().ok()?;
            // A texture's resolution at byte 48 (300 in the user's files: their 419 to 1880 pixel
            // textures then tile as CorelDRAW draws them).
            r.pos = start + 48;
            let dpi = r.u16().ok().map(f64::from).filter(|d| (30.0..=4800.0).contains(d));
            return Some(TiledFill { bitmap, width_in: w, height_in: h, dpi });
        }
        if v < CdrVersion(600) {
            return None;
        }
        r.skip(2).ok()?;
        r.unsigned().ok()?;
        let (w, h) = (r.unsigned().ok()?, r.unsigned().ok()?);
        r.skip(4).ok()?; // tile offsets
        r.skip(2).ok()?; // row or column offset
        let flags = r.u8().ok()?;
        if flags & 0x04 != 0 && v < CdrVersion(900) {
            return None; // sized relative to the object: not read
        }
        r.skip(21).ok()?;
        let bitmap = r.unsigned().ok()?;
        let (w, h) = size(w, h)?;
        Some(TiledFill { bitmap, width_in: w, height_in: h, dpi: None })
    }

    /// The placement after a version 17 fountain's stops: 3 bytes, then eight doubles, the fifth
    /// and eighth of them zero. Anything else is left to the angle and pad.
    fn fountain_frame(r: &mut Reader<'_>) -> Option<[f64; 6]> {
        r.skip(3).ok()?;
        let mut d = [0.0; 8];
        for x in &mut d {
            *x = r.f64().ok()?;
        }
        let [cx, cy, ux, uy, z0, vx, vy, z1] = d;
        let frame = [cx, cy, ux, uy, vx, vy];
        (z0 == 0.0 && z1 == 0.0 && frame.iter().all(|x| x.is_finite() && x.abs() < 1e6) && (ux != 0.0 || uy != 0.0)).then_some(frame)
    }

    /// The fill layer, or `None` for no fill. `bounds` is the object's box in document space,
    /// which a fountain is laid out on.
    pub fn to_fill(&self, bounds: Option<Rect>, notes: &mut Vec<String>) -> Option<FillLayer> {
        self.to_fill_on(bounds, None, notes)
    }

    /// [`Self::to_fill`] for an object whose own box (`local`, in object space) and transform to
    /// document space are known, which a version 17 fountain's frame is measured on.
    pub fn to_fill_on(&self, bounds: Option<Rect>, local: Option<(Rect, Affine)>, notes: &mut Vec<String>) -> Option<FillLayer> {
        match self {
            Self::None => None,
            Self::Solid(c) => Some(FillLayer::new(Paint::solid(color_or_black(c, notes)))),
            Self::Fountain(f) => f.to_paint(bounds, local, notes).map(FillLayer::new),
            Self::Tiled(_) => {
                notes.push("a bitmap or texture fill on type is not translated".into());
                None
            }
            Self::Unsupported(t) => {
                notes.push(format!("fill type {t} (pattern, full-colour or PostScript) not translated"));
                None
            }
        }
    }
}

impl Fountain {
    fn to_paint(&self, bounds: Option<Rect>, local: Option<(Rect, Affine)>, notes: &mut Vec<String>) -> Option<Paint> {
        let (first, rest) = self.stops.split_first()?;
        if rest.is_empty() {
            return Some(Paint::solid(color_or_black(&first.1, notes)));
        }
        let kind = match self.kind {
            1 => GradientKind::Linear,
            2 => GradientKind::Radial,
            3 => {
                notes.push("conical fountain drawn as linear".into());
                GradientKind::Linear
            }
            4 => {
                notes.push("square fountain drawn as radial".into());
                GradientKind::Radial
            }
            k => {
                notes.push(format!("fountain type {k} drawn as linear"));
                GradientKind::Linear
            }
        };
        // CorelDRAW's first stop is at a radial fountain's rim (libcdr hands it to ODF as the
        // border colour); VectorCraft's radial ramp starts at the centre, so radial stops mirror.
        let radial = kind == GradientKind::Radial;
        let mut stops: Vec<GradientStop> = self
            .stops
            .iter()
            .map(|(t, c)| {
                let t = t.clamp(0.0, 1.0) as f32;
                GradientStop::new(if radial { 1.0 - t } else { t }, color_or_black(c, notes))
            })
            .collect();
        stops.sort_by(|a, b| a.offset.total_cmp(&b.offset));
        if let [a, _] = stops.as_mut_slice()
            && self.midpoint > 0.0
            && self.midpoint < 1.0
        {
            let m = if radial { 1.0 - self.midpoint } else { self.midpoint } as f32;
            a.midpoint = m.clamp(0.13, 0.87);
        }
        let angle = self.angle.to_degrees();
        let mut paint = GradientPaint::new(Gradient { kind, stops });
        paint.angle = if angle.is_finite() { angle } else { 0.0 };
        if let Some(b) = bounds {
            let mut g = self.geom(kind, b, paint.angle);
            if let Some((frame, (lb, to_doc))) = self.frame.zip(local).filter(|(_, (lb, _))| lb.width() > 0.0 && lb.height() > 0.0) {
                match kind {
                    GradientKind::Linear => {
                        if let Some((a, b)) = Self::frame_ends(frame, lb) {
                            (g.start, g.end) = (to_doc * a, to_doc * b);
                        }
                    }
                    _ => {
                        // A radial's frame axes are its diameters (measured against CorelDRAW's
                        // previews: half of them gave the closest radius).
                        let [cx, cy, ux, uy, vx, vy] = frame;
                        let c = lb.center() + Vec2::new(cx * lb.width(), cy * lb.height());
                        let along = |d: Vec2| to_doc * (c + d) - to_doc * c;
                        let u = along(Vec2::new(ux * lb.width(), -uy * lb.height()) / 2.0);
                        let v = along(Vec2::new(vx * lb.width(), -vy * lb.height()) / 2.0);
                        if u.hypot() > 0.0 && v.hypot() > 0.0 && u.hypot().is_finite() && v.hypot().is_finite() {
                            g.start = to_doc * c;
                            g.end = g.start + u;
                            g.aspect = (v.hypot() / u.hypot()).clamp(1e-3, 1e3);
                            g.focal = None;
                        }
                    }
                }
            }
            paint.geom = Some(g);
        }
        Some(Paint::Gradient(Box::new(paint)))
    }

    /// A version 17 linear fountain's first and last stop on the object's own box (y up).
    fn frame_ends(f: [f64; 6], b: Rect) -> Option<(Point, Point)> {
        let [cx, cy, ux, uy, ..] = f;
        let c = b.center() + Vec2::new(cx * b.width(), cy * b.height());
        let half = Vec2::new(ux * b.width(), -uy * b.height()) / 2.0;
        (half.hypot() > 0.0).then_some((c - half, c + half))
    }

    /// Placement on the object's box: the edge pad shortens the ramp at both ends, and a radial
    /// fountain's centre moves by its offset (percent of the half-size, y up).
    fn geom(&self, kind: GradientKind, b: Rect, angle: f64) -> GradientGeom {
        let pad = (f64::from(self.edge_pad) / 100.0).clamp(0.0, 0.49);
        let mut g = GradientGeom::fit(kind, b, angle);
        if kind == GradientKind::Radial {
            // The centre moves by its offset (percent of the half-size, y up), and the ramp reaches
            // the box's farthest corner from it, so it covers the whole shape. Measured on the
            // user's version 17 samples against CorelDRAW's own previews (radial bows and badges
            // with off-centre fountains): the closest of the radius rules tried.
            let shift =
                vectorcraft_geom::Vec2::new(f64::from(self.center.0) / 100.0 * b.width() / 2.0, -f64::from(self.center.1) / 100.0 * b.height() / 2.0);
            let c = b.center() + shift;
            let r = [b.origin(), Point::new(b.x1, b.y0), Point::new(b.x0, b.y1), Point::new(b.x1, b.y1)]
                .iter()
                .map(|p| p.distance(c))
                .fold(0.0, f64::max);
            let dir = (g.end - g.start).normalize();
            let dir = if dir.x.is_finite() && dir.y.is_finite() { dir } else { vectorcraft_geom::Vec2::new(1.0, 0.0) };
            g.start = c;
            g.end = c + dir * r * (1.0 - pad);
        } else {
            let span = g.end - g.start;
            g.start += span * pad;
            g.end -= span * pad;
        }
        g
    }
}

// ---------------------------------------------------------------------------------------------
// Whole documents

/// One translated object.
#[derive(Debug, Clone, PartialEq)]
pub struct Shape {
    pub content: Content,
    /// The object's opacity (its transparency), 1 when opaque.
    pub opacity: f32,
    /// The object's name in CorelDRAW, if it has one.
    pub name: Option<String>,
    /// A fading transparency, in document space, over [`Self::opacity`].
    pub fade: Option<Fade>,
    /// An envelope the shape is fitted to (its box mapped onto this outline), document space.
    pub envelope: Option<PathData>,
    /// Fill below stroke, as CorelDRAW draws them (empty for type, whose characters carry theirs).
    pub appearance: Appearance,
    /// What was approximated or left out.
    pub notes: Vec<String>,
}

/// What a translated object is.
#[derive(Debug, Clone, PartialEq)]
pub enum Content {
    /// A path in document space (points, y down). CorelDRAW fills paths even-odd (libcdr writes
    /// `svg:fill-rule="evenodd"`).
    Path { path: PathData, rule: FillRule },
    /// Point or area type, its `xf` mapping text space to the document.
    Text(Box<TextObject>),
    /// A bitmap, `xf` mapping its pixels (0..width, 0..height, y down) to the document.
    Image { bitmap: std::sync::Arc<crate::bitmap::Bitmap>, xf: Affine },
    /// A group, bottom first. With `clip`, the first child's path clips the others (a PowerClip:
    /// the container's outline clipping its contents).
    Group { children: Vec<Shape>, clip: bool },
}

impl Shape {
    /// The path, for a path object.
    pub fn path(&self) -> Option<&PathData> {
        match &self.content {
            Content::Path { path, .. } => Some(path),
            _ => None,
        }
    }

    /// A shape holding type `t` (tests).
    #[cfg(test)]
    pub(crate) fn text_for_tests(t: TextObject) -> Self {
        Self::of(Content::Text(Box::new(t)), Vec::new())
    }

    /// The type, for a text object.
    pub fn text(&self) -> Option<&TextObject> {
        match &self.content {
            Content::Text(t) => Some(t),
            _ => None,
        }
    }

    fn of(content: Content, notes: Vec<String>) -> Self {
        Self { content, opacity: 1.0, name: None, fade: None, envelope: None, appearance: Appearance::default(), notes }
    }

    /// This shape and, for a group, everything in it, bottom first (a group before its members).
    pub fn walk(&self) -> Vec<&Shape> {
        let mut out = vec![self];
        if let Content::Group { children, .. } = &self.content {
            out.extend(children.iter().flat_map(Shape::walk));
        }
        out
    }
}

/// What a CorelDRAW layer is for (its object kind).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LayerKind {
    /// A layer of art (kind 0).
    Drawing,
    /// Guidelines (kind 12), not printed.
    Guides,
    /// The desktop around the pages (kind 17).
    Desktop,
    /// The document grid (kind 11), not printed.
    Grid,
}

impl LayerKind {
    fn from_kind(kind: ShapeKind) -> Self {
        match kind {
            ShapeKind::Other(11) => Self::Grid,
            ShapeKind::Other(12) => Self::Guides,
            ShapeKind::Other(17) => Self::Desktop,
            _ => Self::Drawing,
        }
    }

    /// Art that prints (guides and the grid don't).
    pub fn is_art(self) -> bool {
        matches!(self, Self::Drawing | Self::Desktop)
    }
}

/// One CorelDRAW layer of a page.
#[derive(Debug, Clone, PartialEq)]
pub struct LayerShapes {
    pub name: Option<String>,
    pub kind: LayerKind,
    /// Bottom first.
    pub shapes: Vec<Shape>,
}

/// The translated objects of one page, bottom first.
#[derive(Debug, Clone, PartialEq)]
pub struct PageShapes {
    /// Index of the page list in the file (CorelDRAW writes a master page before the pages).
    pub index: usize,
    /// The master page, which libcdr does not output: from version 4, the page list's first
    /// `flgs` record has a bit of `0x00ff0000` set (`CDRContentCollector::collectFlags`).
    pub master: bool,
    pub page: PageSpace,
    /// The page's layers, bottom first (objects outside any layer make an unnamed one).
    pub layers: Vec<LayerShapes>,
}

impl PageShapes {
    /// Every shape on the page's art layers, groups and their members alike, bottom first.
    pub fn shapes(&self) -> Vec<&Shape> {
        self.layers.iter().filter(|l| l.kind.is_art()).flat_map(|l| l.shapes.iter()).flat_map(Shape::walk).collect()
    }

    /// The shapes that draw something (no groups), bottom first.
    pub fn leaves(&self) -> Vec<&Shape> {
        self.shapes().into_iter().filter(|s| !matches!(s.content, Content::Group { .. })).collect()
    }
}

/// Every page's Bézier objects.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Translation {
    pub pages: Vec<PageShapes>,
    /// Objects of other kinds, by kind, that were not translated.
    pub skipped: BTreeMap<String, usize>,
    /// Records that could not be decoded (kind and reason).
    pub errors: Vec<String>,
    /// The bitmaps that bitmap and texture fills tile, by pattern name ([`tile_name`]).
    pub tiles: BTreeMap<String, Tile>,
}

/// Translate every Bézier path object of a RIFF tree. `default_page` is used until the file gives
/// a page size (an `mcfg` record or a page's own size argument).
pub fn translate(riff: &Riff<'_>, default_page: Option<PageSpace>) -> Translation {
    translate_with(riff, default_page, &[])
}

/// [`translate`] an X6+ drawing whose records may live in `content/data` files: `streams` are
/// their bytes, in `content/dataFileList.dat` order.
pub fn translate_with(riff: &Riff<'_>, default_page: Option<PageSpace>, streams: &[Vec<u8>]) -> Translation {
    let src = Records { riff, streams };
    let version = riff.version;
    let mut t = Translation::default();
    let mut fills = BTreeMap::new();
    let mut outlines = BTreeMap::new();
    let mut fonts: BTreeMap<u16, Font> = BTreeMap::new();
    let mut bitmaps = BTreeMap::new();
    let mut arrows = BTreeMap::new();
    let mut page = default_page;
    // Styles come before the objects that use them; collect them first, as libcdr's styles pass.
    for (_, c) in riff.walk() {
        let Chunk::Record(rec) = c else { continue };
        if !matches!(&rec.kind.0, b"fild" | b"fill" | b"outl" | b"mcfg" | b"bmp " | b"font" | b"arrw") {
            continue;
        }
        let data = match src.data(rec) {
            Ok(d) => d,
            Err(e) => {
                t.errors.push(e.to_string());
                continue;
            }
        };
        match &rec.kind.0 {
            b"fild" | b"fill" => match CorelFill::decode(data, version) {
                Ok((id, f)) => {
                    fills.insert(id, f);
                }
                Err(e) => t.errors.push(e.to_string()),
            },
            b"outl" => match Outline::decode(data, version) {
                Ok(o) => {
                    outlines.insert(o.id, o);
                }
                Err(e) => t.errors.push(e.to_string()),
            },
            b"mcfg" if page.is_none() => page = decode_mcfg(data, version).ok(),
            b"arrw" if version >= CdrVersion(600) => match decode_arrw(data, version) {
                Ok((id, path)) => {
                    arrows.insert(id, arrowhead_for(&path));
                }
                Err(e) => t.errors.push(e.to_string()),
            },
            b"bmp " => match crate::bitmap::decode_bmp(data, version) {
                Ok((id, b)) => {
                    bitmaps.insert(id, std::sync::Arc::new(b));
                }
                Err(e) => t.errors.push(e.to_string()),
            },
            // The first font with an id is the one used; later ones are substitutes (libcdr).
            b"font" => match text::decode_font(data, version) {
                Ok((id, f)) => {
                    fonts.entry(id).or_insert(f);
                }
                Err(e) => t.errors.push(e.to_string()),
            },
            _ => {}
        }
    }
    for (id, f) in &fills {
        if let CorelFill::Tiled(tf) = f
            && let Some(b) = bitmaps.get(&tf.bitmap)
        {
            // A texture's bitmap tiles at the resolution it was made at, when the fill gives one.
            let (w, h) = match tf.dpi {
                Some(dpi) => (f64::from(b.width) / dpi, f64::from(b.height) / dpi),
                None => (tf.width_in, tf.height_in),
            };
            t.tiles.insert(tile_name(*id), Tile { bitmap: std::sync::Arc::clone(b), width_pt: w * PT_PER_IN, height_pt: h * PT_PER_IN });
        }
    }
    // Text styles and text name fonts, which the first walk collected.
    let mut styles = Styles::default();
    let mut texts: BTreeMap<u32, Vec<Paragraph>> = BTreeMap::new();
    for (_, c) in riff.walk() {
        let Chunk::Record(rec) = c else { continue };
        if !matches!(&rec.kind.0, b"stlt" | b"txsm" | b"styd") {
            continue;
        }
        let data = match src.data(rec) {
            Ok(d) => d,
            Err(e) => {
                t.errors.push(e.to_string());
                continue;
            }
        };
        match &rec.kind.0 {
            b"stlt" if version >= CdrVersion(700) => match Styles::decode(data, version, &fonts) {
                Ok(s) => styles.styles.extend(s.styles),
                Err(e) => t.errors.push(e.to_string()),
            },
            b"styd" if version >= CdrVersion(500) && version < CdrVersion(700) => match text::decode_styd(data, version, &fonts) {
                Ok((id, parent, s)) => {
                    styles.styles.insert(id, (parent, s));
                }
                Err(e) => t.errors.push(e.to_string()),
            },
            b"txsm" if version >= CdrVersion(500) => match text::decode_txsm(data, version, &fonts) {
                Ok((id, paras)) => texts.entry(id).or_default().extend(paras),
                Err(e) => t.errors.push(e.to_string()),
            },
            _ => {}
        }
    }
    let mut ctx = Ctx {
        src,
        version,
        fills: &fills,
        outlines: &outlines,
        styles: &styles,
        texts: &texts,
        bitmaps: &bitmaps,
        arrows: &arrows,
        clips: BTreeMap::new(),
    };
    // PowerClip contents: the groups (or objects) of the document's `clpt` lists, by their `spnd`.
    let mut clips = BTreeMap::new();
    for list in riff.lists(FourCc(*b"clpt")) {
        for c in &list.children {
            let Chunk::List(l) = c else { continue };
            let id = l.children.iter().find_map(|k| match k {
                Chunk::Record(r) if r.kind.0 == *b"spnd" => ctx.src.data(r).ok().and_then(|d| Reader::new(d, "spnd", version).unsigned().ok()),
                _ => None,
            });
            let Some(id) = id else { continue };
            let mut items = Vec::new();
            ctx.items(c, &mut items, &mut Vec::new(), &mut None, &mut t);
            clips.insert(id, items);
        }
    }
    ctx.clips = clips;
    let mut index = 0;
    for chunk in &riff.chunks {
        ctx.pages(chunk, &mut page, &mut index, &mut t);
    }
    t
}

struct Ctx<'a, 'r> {
    src: Records<'a, 'r>,
    version: CdrVersion,
    fills: &'a BTreeMap<u32, CorelFill>,
    outlines: &'a BTreeMap<u32, Outline>,
    styles: &'a Styles,
    texts: &'a BTreeMap<u32, Vec<Paragraph>>,
    bitmaps: &'a BTreeMap<u32, std::sync::Arc<crate::bitmap::Bitmap>>,
    arrows: &'a BTreeMap<u32, Arrowhead>,
    clips: BTreeMap<u32, Vec<Item>>,
}

/// Raw object before page placement (the page's size may come later in its list).
#[derive(Clone)]
struct Pending {
    geometry: Geometry,
    object: Affine,
    fill: Option<u32>,
    outline: Option<u32>,
    style: Option<u32>,
    name: Option<String>,
    clip: Option<u32>,
    opacity: Option<f32>,
    fade: Option<Fade>,
    /// The fill's own transform (`ftil`), which bitmap and texture tiles follow.
    fill_xf: Option<Affine>,
    /// An envelope (`envd`): the outline the object's box is fitted to, page space.
    envelope: Option<BezPath>,
}

/// An object or a group of them, front first as CorelDRAW writes them.
#[derive(Clone)]
enum Item {
    Object(Box<Pending>),
    Group(Vec<Item>),
}

/// A layer's objects as read.
struct LayerItems {
    name: Option<String>,
    kind: LayerKind,
    items: Vec<Item>,
}

/// Deepest PowerClip nesting followed.
const MAX_CLIP_DEPTH: usize = 8;

#[derive(Clone)]
enum Geometry {
    /// A path in CorelDRAW space (inches, y up), before the object's transform.
    Path(BezPath),
    /// Type, its text found by the object's `spnd`.
    Text {
        frame: TextFrame,
        spnd: Option<u32>,
    },
    Image(BitmapPlacement),
}

impl Ctx<'_, '_> {
    fn pages(&self, chunk: &Chunk, page: &mut Option<PageSpace>, index: &mut usize, t: &mut Translation) {
        let Chunk::List(list) = chunk else { return };
        if list.list_type != FourCc::PAGE {
            for c in &list.children {
                self.pages(c, page, index, t);
            }
            return;
        }
        let mut loose = Vec::new();
        let mut layers = Vec::new();
        let mut own_size = None;
        for c in &list.children {
            self.items(c, &mut loose, &mut layers, &mut own_size, t);
        }
        if own_size.is_some() {
            *page = own_size;
        }
        if !loose.is_empty() {
            layers.push(LayerItems { name: None, kind: LayerKind::Drawing, items: loose });
        }
        let space = page.unwrap_or(PageSpace { width_in: 0.0, height_in: 0.0 });
        // CorelDRAW writes layers and objects front to back; VectorCraft stacks them bottom first.
        let layers = layers.into_iter().rev().map(|l| LayerShapes { name: l.name, kind: l.kind, shapes: self.shapes(l.items, &space, 0) }).collect();
        let flags = list.children.iter().find_map(|c| match c {
            Chunk::Record(r) if r.kind.0 == *b"flgs" => self.src.data(r).ok().and_then(|d| d.get(..4)),
            _ => None,
        });
        let master = self.version >= CdrVersion(400) && flags.is_some_and(|f| u32::from_le_bytes([f[0], f[1], f[2], f[3]]) & 0x00ff_0000 != 0);
        t.pages.push(PageShapes { index: *index, master, page: space, layers });
        *index += 1;
    }

    /// The objects under `chunk`, in file order: an object list is an object, a group list a
    /// group, a layer list a new layer (named by its own record); other lists are looked through.
    fn items(&self, chunk: &Chunk, out: &mut Vec<Item>, layers: &mut Vec<LayerItems>, own_size: &mut Option<PageSpace>, t: &mut Translation) {
        match chunk {
            // Pattern definitions and PowerClip contents, which objects use by reference.
            Chunk::List(l) if matches!(&l.list_type.0, b"vect" | b"clpt") => {}
            Chunk::List(l) if l.list_type == FourCc::OBJECT => self.object(l, out, t),
            Chunk::List(l) if l.list_type == FourCc::LAYER => {
                let own = self.own_record(l);
                let mut items = Vec::new();
                for c in &l.children {
                    self.items(c, &mut items, layers, own_size, t);
                }
                let (name, kind) = own.map_or((None, LayerKind::Drawing), |o| (o.name, LayerKind::from_kind(o.kind)));
                // Unnamed special layers (a page's own Desktop, say) take their kind's name.
                let name = name.or_else(|| match kind {
                    LayerKind::Desktop => Some("Desktop".into()),
                    LayerKind::Guides => Some("Guides".into()),
                    LayerKind::Grid => Some("Document Grid".into()),
                    LayerKind::Drawing => None,
                });
                layers.push(LayerItems { name, kind, items });
            }
            Chunk::List(l) if matches!(&l.list_type.0, b"grp " | b"lnkg") => {
                let mut items = Vec::new();
                for c in &l.children {
                    self.items(c, &mut items, layers, own_size, t);
                }
                if !items.is_empty() {
                    out.push(Item::Group(items));
                }
            }
            Chunk::List(l) => {
                for c in &l.children {
                    self.items(c, out, layers, own_size, t);
                }
            }
            Chunk::Record(r) if matches!(&r.kind.0, b"loda" | b"lobj") => {
                // A page's (or group's, or layer's) own record: it may carry the page size.
                if let Ok(Ok(l)) = self.src.data(r).map(|d| decode_loda(d, self.version)) {
                    *own_size = l.page.or(*own_size);
                }
            }
            Chunk::Record(_) => {}
        }
    }

    /// A layer's or group's own `loda` (directly in it, or in its `lgob` list).
    fn own_record(&self, l: &List) -> Option<Loda> {
        let find = |children: &[Chunk]| {
            children.iter().find_map(|c| match c {
                Chunk::Record(r) if matches!(&r.kind.0, b"loda" | b"lobj") => self.src.data(r).ok().and_then(|d| decode_loda(d, self.version).ok()),
                _ => None,
            })
        };
        find(&l.children).or_else(|| {
            l.children.iter().find_map(|c| match c {
                Chunk::List(g) if g.list_type.0 == *b"lgob" => find(&g.children),
                _ => None,
            })
        })
    }

    /// Shapes for `items` (front first), bottom first.
    fn shapes(&self, items: Vec<Item>, page: &PageSpace, depth: usize) -> Vec<Shape> {
        let mut out = Vec::new();
        for item in items.into_iter().rev() {
            match item {
                Item::Object(p) => out.extend(self.object_shapes(*p, page, depth)),
                Item::Group(children) => {
                    let children = self.shapes(children, page, depth);
                    if !children.is_empty() {
                        out.push(Shape::of(Content::Group { children, clip: false }, Vec::new()));
                    }
                }
            }
        }
        out
    }

    /// An object's shapes: itself, or for a PowerClip container its fill, its contents clipped to
    /// its outline, and its stroke over them (as CorelDRAW draws them).
    fn object_shapes(&self, p: Pending, page: &PageSpace, depth: usize) -> Vec<Shape> {
        let contents = p.clip.and_then(|id| self.clips.get(&id)).filter(|_| depth < MAX_CLIP_DEPTH).cloned();
        let mut shape = self.shape(p, page);
        let Some(contents) = contents else { return vec![shape] };
        let inside = self.shapes(contents, page, depth + 1);
        let Content::Path { path, .. } = &shape.content else {
            // Only paths clip: other containers show their contents unclipped.
            shape.notes.push("PowerClip contents of a non-path container are not clipped".into());
            let mut all = vec![shape];
            all.extend(inside);
            return all;
        };
        let clip_path = Shape::of(Content::Path { path: path.clone(), rule: FillRule::EvenOdd }, Vec::new());
        let (fills, strokes): (Vec<_>, Vec<_>) = shape.appearance.items.drain(..).partition(AppearanceItem::is_fill);
        let mut stroke = shape.clone();
        shape.appearance.items = fills;
        stroke.appearance.items = strokes;
        stroke.notes.clear();
        let mut children = vec![clip_path];
        children.extend(inside);
        let mut out = vec![shape, Shape::of(Content::Group { children, clip: true }, Vec::new())];
        if !stroke.appearance.items.is_empty() {
            out.push(stroke);
        }
        out
    }

    fn object(&self, obj: &List, out: &mut Vec<Item>, t: &mut Translation) {
        let mut loda = None;
        let mut spnd = None;
        let mut ppdt = None;
        let mut fill_xf = None;
        let mut envelope = None;
        let mut matrices = Vec::new();
        let mut stack: Vec<&Chunk> = obj.children.iter().rev().collect();
        while let Some(c) = stack.pop() {
            match c {
                Chunk::List(l) if l.list_type != FourCc::OBJECT => stack.extend(l.children.iter().rev()),
                Chunk::List(_) => {}
                Chunk::Record(r) => {
                    if !matches!(&r.kind.0, b"loda" | b"lobj" | b"spnd" | b"ppdt" | b"trfd" | b"ftil" | b"envd") {
                        continue;
                    }
                    let data = match self.src.data(r) {
                        Ok(d) => d,
                        Err(e) => {
                            t.errors.push(e.to_string());
                            continue;
                        }
                    };
                    match &r.kind.0 {
                        b"loda" | b"lobj" if loda.is_none() => loda = Some(data),
                        b"spnd" => spnd = Reader::new(data, "spnd", self.version).unsigned().ok(),
                        b"ppdt" => ppdt = Some(data),
                        b"ftil" => fill_xf = decode_ftil(data),
                        b"envd" => envelope = decode_envd(data, self.version),
                        // The last trfd wins, as in libcdr's collectTransform.
                        b"trfd" => match decode_trfd(data, self.version) {
                            Ok(m) => matrices = m,
                            Err(e) => t.errors.push(e.to_string()),
                        },
                        _ => {}
                    }
                }
            }
        }
        let Some(data) = loda else { return };
        let l = match decode_loda(data, self.version) {
            Ok(l) => l,
            Err(e) => return t.errors.push(e.to_string()),
        };
        let v = self.version;
        let geometry = match (l.kind, l.coords) {
            (ShapeKind::Curve | ShapeKind::Path, Some(at)) => CorelPath::decode(data, at, l.kind, v).map(|p| Geometry::Path(p.to_bezpath())),
            // A polygon's coordinates are laid out as a curve's: one corner, then its symmetry.
            (ShapeKind::Polygon, Some(at)) => CorelPath::decode(data, at, ShapeKind::Curve, v).map(|p| {
                let corner = p.to_bezpath();
                Geometry::Path(l.polygon.map_or(corner.clone(), |x| x.create(&corner)))
            }),
            (ShapeKind::Bitmap, Some(at)) => decode_bitmap_object(data, at, v).map(Geometry::Image),
            (ShapeKind::Spline, _) => match ppdt {
                Some(d) => SplineData::decode(d, v).map(|s| Geometry::Path(s.to_bezpath())),
                None => Err(Error::Record { record: "ppdt", message: "missing from a spline object".into() }),
            },
            (ShapeKind::Rectangle, Some(at)) => decode_rectangle(data, at, v).map(Geometry::Path),
            (ShapeKind::Ellipse, Some(at)) => decode_ellipse(data, at, v).map(Geometry::Path),
            (ShapeKind::ArtisticText | ShapeKind::ParagraphText, Some(at)) => {
                decode_text_frame(data, at, l.kind, v).map(|frame| Geometry::Text { frame, spnd })
            }
            (kind, _) => {
                *t.skipped.entry(format!("{kind:?}")).or_default() += 1;
                return;
            }
        };
        match geometry {
            Ok(geometry) => out.push(Item::Object(Box::new(Pending {
                geometry,
                object: compose(&matrices),
                fill: l.fill_id,
                outline: l.outline_id,
                style: l.style_id,
                name: l.name,
                clip: l.clip,
                opacity: l.opacity,
                fade: l.fade,
                fill_xf,
                // The envelope is drawn after the object's first transform: only the others place it.
                envelope: envelope.map(|mut e| {
                    e.apply_affine(compose(matrices.get(1..).unwrap_or_default()));
                    e
                }),
            }))),
            Err(e) => t.errors.push(e.to_string()),
        }
    }

    fn shape(&self, p: Pending, page: &PageSpace) -> Shape {
        let mut notes = Vec::new();
        let fade = p.fade.map(|f| f.transformed(page.to_document() * p.object));
        let bp = match p.geometry {
            Geometry::Path(bp) => bp,
            Geometry::Image(place) => {
                let Some(bitmap) = self.bitmaps.get(&place.id) else {
                    notes.push(format!("bitmap {} not found", place.id));
                    return Shape::of(Content::Path { path: PathData::default(), rule: FillRule::EvenOdd }, notes);
                };
                let Some(px) = place.pixels_to_object(bitmap.width, bitmap.height) else {
                    notes.push("a bitmap with an empty frame".into());
                    return Shape::of(Content::Path { path: PathData::default(), rule: FillRule::EvenOdd }, notes);
                };
                let xf = page.to_document() * p.object * px;
                return Shape {
                    opacity: p.opacity.unwrap_or(1.0),
                    name: p.name,
                    fade,
                    ..Shape::of(Content::Image { bitmap: bitmap.clone(), xf }, notes)
                };
            }
            Geometry::Text { frame, spnd } => {
                let t = self.text_object(frame, spnd, page.to_document() * p.object, p.object, &mut notes);
                // Type keeps its text under an envelope (the distorted letters aren't stored).
                let envelope = p.envelope.map(|mut e| {
                    e.apply_affine(page.to_document());
                    PathData::from_bezpath(&e)
                });
                return Shape { opacity: p.opacity.unwrap_or(1.0), name: p.name, fade, envelope, ..Shape::of(Content::Text(Box::new(t)), notes) };
            }
        };
        let path = PathData::from_bezpath(&{
            let mut bp = bp.clone();
            bp.apply_affine(page.to_document() * p.object);
            bp
        });
        let mut items = Vec::new();
        match p.fill.and_then(|id| self.fills.get(&id).map(|f| (id, f))) {
            Some((id, CorelFill::Tiled(tf))) => match (self.bitmaps.contains_key(&tf.bitmap), path.bounds()) {
                // Tiles start at the object's top-left corner, scaled and skewed by the fill's own
                // transform (y up in CorelDRAW, so its off-diagonal terms change sign).
                (true, Some(b)) => {
                    let [a, bb, c, d, _, _] = p.fill_xf.unwrap_or(Affine::IDENTITY).as_coeffs();
                    let xf = Affine::translate(b.origin().to_vec2()) * Affine::new([a, -bb, -c, d, 0.0, 0.0]);
                    items.push(AppearanceItem::Fill(FillLayer::new(Paint::Pattern { pattern: tile_name(id), xf })));
                }
                (true, None) => {}
                (false, _) => notes.push(format!("the bitmap of a bitmap or texture fill (bitmap {}) was not found", tf.bitmap)),
            },
            Some((_, f)) => {
                let local = PathData::from_bezpath(&bp).bounds().filter(|b| b.width() > 0.0 || b.height() > 0.0);
                let to_doc = page.to_document() * p.object;
                items.extend(f.to_fill_on(path.bounds(), local.map(|b| (b, to_doc)), &mut notes).map(AppearanceItem::Fill));
            }
            None if p.fill.is_some() => notes.push(format!("fill {} not found", p.fill.unwrap_or_default())),
            // Open paths are not filled, so only a closed one misses its style's fill.
            None if p.style.is_some() && path.is_closed() => notes.push("fill from a graphic style not translated".into()),
            None => {}
        }
        match p.outline.and_then(|id| self.outlines.get(&id)) {
            Some(o) => items.extend(o.to_stroke(p.object, &mut notes).map(|mut st| {
                let (start, end) = o.arrows(|id| self.arrows.contains_key(&id));
                let ends = [start, end].map(|id| (id != 0).then(|| self.arrows.get(&id).copied()));
                if ends.iter().any(|e| matches!(e, Some(None))) {
                    notes.push("an arrowhead was not found".into());
                }
                [st.start_arrow, st.end_arrow] = ends.map(Option::flatten);
                if st.start_arrow.is_some() || st.end_arrow.is_some() {
                    notes.push("arrowheads drawn as VectorCraft's nearest preset".into());
                }
                AppearanceItem::Stroke(st)
            })),
            None if p.outline.is_some() => notes.push(format!("outline {} not found", p.outline.unwrap_or_default())),
            None => {}
        }
        Shape {
            content: Content::Path { path, rule: FillRule::EvenOdd },
            opacity: p.opacity.unwrap_or(1.0),
            name: p.name,
            fade,
            // A curve's points are stored already fitted to its envelope.
            envelope: None,
            appearance: Appearance { items, ..Appearance::default() },
            notes,
        }
    }

    /// Type for a text record: its paragraphs (by `spnd`), each run's style its paragraph's `stlt`
    /// style with the run's own settings over it. `to_doc` maps the object's space to the
    /// document; `object` is the object's own transform (for outline widths that scale with it).
    fn text_object(&self, frame: TextFrame, spnd: Option<u32>, to_doc: Affine, object: Affine, notes: &mut Vec<String>) -> TextObject {
        let paras: &[Paragraph] = spnd.and_then(|id| self.texts.get(&id)).map_or(&[], Vec::as_slice);
        if paras.is_empty() {
            notes.push("the text of a text object was not found".into());
        }
        let mut runs: Vec<TextRun> = Vec::new();
        // Each run's horizontal shift, ems (characters moved one by one).
        let mut shifts: Vec<f64> = Vec::new();
        // One style per VectorCraft paragraph: each CorelDRAW paragraph's lines share its style.
        let mut para_styles = Vec::new();
        let mut no_size = false;
        for (i, p) in paras.iter().enumerate() {
            let base = self.styles.resolve(p.style);
            let ps = para_style(&base);
            if i > 0 {
                runs.push(TextRun::new("\n", self.char_style(&base, object, &mut no_size, notes)));
                shifts.push(0.0);
            }
            para_styles.push(ps.clone());
            for run in &p.runs {
                let mut s = base.clone();
                s.apply(&run.style);
                // Line breaks start a new line; VectorCraft breaks lines at paragraphs.
                let chars: String = run.decoded(&s).chars().map(|c| if matches!(c, '\r' | '\u{b}') { '\n' } else { c }).collect();
                para_styles.extend(std::iter::repeat_n(ps.clone(), chars.matches('\n').count()));
                runs.push(TextRun::new(chars, self.char_style(&s, object, &mut no_size, notes)));
                shifts.push(s.shift.unwrap_or(0.0));
            }
        }
        if no_size {
            notes.push("text without a font size set at 12 pt".into());
        }
        let (kind, origin) = match frame {
            TextFrame::Artistic => (TextKind::Point, Point::ZERO),
            TextFrame::Paragraph { width, height } => {
                let (w, h) = (width * PT_PER_IN, height * PT_PER_IN);
                let corners = [Point::ZERO, Point::new(w, 0.0), Point::new(w, h), Point::new(0.0, h)];
                (TextKind::Area { frame: PathData::single(vectorcraft_geom::SubPath::polyline(&corners, true)) }, Point::ZERO)
            }
        };
        let mut t = TextObject::point(Point::ZERO, "", CharStyle::default());
        t.kind = kind;
        // Text space is points with y down; CorelDRAW's is inches with y up.
        let lead = shift_characters(&mut runs, &shifts, notes);
        t.xf = to_doc * Affine::translate(origin.to_vec2()) * Affine::scale_non_uniform(TEXT_SCALE, -TEXT_SCALE) * Affine::translate((lead, 0.0));
        // Bitmap and texture fills on type: tiles start at the text's origin.
        let anchor = Affine::translate((t.xf * Point::ZERO).to_vec2());
        for r in &mut runs {
            if let Paint::Pattern { xf, .. } = &mut r.style.fill {
                *xf = anchor;
            }
        }
        runs.retain(|r| !r.text.is_empty());
        if !runs.is_empty() {
            t.runs = runs;
        }
        para_styles.truncate(t.plain_text().matches('\n').count() + 1);
        if let Some(first) = para_styles.first() {
            t.para = first.clone();
        }
        t.paras = para_styles;
        t.normalize_paras();
        t
    }

    fn char_style(&self, s: &TextStyle, object: Affine, no_size: &mut bool, notes: &mut Vec<String>) -> CharStyle {
        let mut c = CharStyle::default();
        if let Some(f) = s.font.as_ref().filter(|f| !f.trim().is_empty()) {
            c.font_family = f.trim().to_owned();
        }
        match s.size.map(|v| v * PT_PER_IN).filter(|v| v.is_finite() && *v > 0.0) {
            Some(pt) => c.size = pt.min(16_000.0),
            None => *no_size = true,
        }
        match s.fill {
            Some(Ink::Record(id)) => match self.fills.get(&id) {
                // A bitmap or texture fill: its pattern, placed by the text object.
                Some(CorelFill::Tiled(tf)) if self.bitmaps.contains_key(&tf.bitmap) => {
                    c.fill = Paint::Pattern { pattern: tile_name(id), xf: Affine::IDENTITY };
                }
                Some(f) => c.fill = f.to_fill(None, notes).map_or(Paint::None, |l| l.paint),
                None => {}
            },
            Some(Ink::Color(color)) => c.fill = Paint::solid(color_or_black(&color, notes)),
            Some(Ink::None) => c.fill = Paint::None,
            None => {}
        }
        let stroke = match s.outline {
            Some(Pen::Record(id)) => self.outlines.get(&id).and_then(|o| o.to_stroke(object, notes)).map(|st| (st.paint, st.width)),
            Some(Pen::Line { width, color }) => {
                Some((Paint::solid(color_or_black(&color, notes)), width * PT_PER_IN)).filter(|(_, w)| w.is_finite() && *w > 0.0)
            }
            None => None,
        };
        if let Some((paint, width)) = stroke {
            c.stroke = paint;
            c.stroke_width = width;
        }
        // Character spacing is a fraction of a space's width, taken as a quarter em (the width that
        // matched CorelDRAW's previews of the user's samples best); line spacing scales the
        // default leading (120 % of the size).
        if let Some(cs) = s.char_spacing.filter(|v| *v != 0.0) {
            c.tracking = cs * SPACE_EM * 1000.0;
        }
        if let Some(ls) = s.line_spacing.filter(|v| (*v - 1.0).abs() > 1e-6) {
            c.leading = Some(c.size * 1.2 * ls);
        }
        if let Some(a) = s.rotation {
            c.rotation = a.rem_euclid(360.0);
        }
        if let Some(rise) = s.rise {
            c.baseline_shift = (rise * c.size).clamp(-c.size * 10.0, c.size * 10.0);
        }
        c
    }
}

/// Characters moved one by one along the line: each single-character run's horizontal shift
/// (`shifts`, ems) becomes manual kerning on the character before it, and the first character's
/// is returned (points) for the text's origin. Shifts of longer runs, or after a line break, are
/// left out with a note.
fn shift_characters(runs: &mut [TextRun], shifts: &[f64], notes: &mut Vec<String>) -> f64 {
    if shifts.iter().all(|s| *s == 0.0) || shifts.len() != runs.len() {
        return 0.0;
    }
    let pts: Vec<f64> = runs.iter().zip(shifts).map(|(r, s)| s * r.style.size).collect();
    let single = |r: &TextRun| r.text.chars().count() == 1 && r.text != "\n";
    let mut kept = true;
    let mut lead = 0.0;
    for i in 0..runs.len() {
        if pts[i] == 0.0 && (i == 0 || pts[i - 1] == 0.0) {
            continue;
        }
        if !single(&runs[i]) || runs.get(..i).is_some_and(|before| before.iter().any(|r| r.text.contains('\n'))) {
            kept = false;
            continue;
        }
        match i.checked_sub(1) {
            None => lead = pts[0],
            Some(p) if single(&runs[p]) && runs[p].style.size > 0.0 => {
                let k = (pts[i] - pts[p]) / runs[p].style.size * 1000.0;
                runs[p].style.kerning = Some(runs[p].style.kerning.unwrap_or(0.0) + k);
            }
            Some(_) => kept = false,
        }
    }
    if !kept {
        notes.push("characters shifted one by one in a longer run or on a later line are drawn unshifted".into());
    }
    if lead.is_finite() { lead } else { 0.0 }
}

/// A space's width in ems, for CorelDRAW's character spacing.
const SPACE_EM: f64 = 0.25;

/// Paragraph attributes of a style: alignment and indents (CorelDRAW's first-line indent is from
/// the frame's edge, VectorCraft's from the left indent).
fn para_style(s: &TextStyle) -> ParaStyle {
    let justify = match s.align.unwrap_or(0) {
        2 => Justify::Center,
        3 => Justify::Right,
        4 => Justify::JustifyLeft,
        5 => Justify::JustifyAll,
        _ => Justify::Left,
    };
    let (left, first, right) = s
        .indents
        .filter(|(a, b, c)| a.is_finite() && b.is_finite() && c.is_finite())
        .map_or((0.0, 0.0, 0.0), |(l, f, r)| (l * PT_PER_IN, (f - l) * PT_PER_IN, r * PT_PER_IN));
    ParaStyle { justify, left_indent: left, first_line_indent: first, right_indent: right, ..ParaStyle::default() }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::synth as build;

    const V9: CdrVersion = CdrVersion(900);
    const V5: CdrVersion = CdrVersion(500);

    fn le32(v: &[i32]) -> Vec<u8> {
        v.iter().flat_map(|x| x.to_le_bytes()).collect()
    }

    /// A version 6+ `loda` with the given (type, payload) arguments.
    fn loda32(chunk_type: u32, args: &[(u32, Vec<u8>)]) -> Vec<u8> {
        let n = args.len() as u32;
        let header = 20u32;
        let offsets_at = header;
        let types_at = offsets_at + 4 * n;
        let mut payload_at = types_at + 4 * n;
        let mut offsets = Vec::new();
        let mut payload = Vec::new();
        for (_, p) in args {
            offsets.push(payload_at);
            payload.extend_from_slice(p);
            payload_at += p.len() as u32;
        }
        let mut v = Vec::new();
        for x in [payload_at, n, offsets_at, types_at, chunk_type] {
            v.extend_from_slice(&x.to_le_bytes());
        }
        offsets.iter().for_each(|o| v.extend_from_slice(&o.to_le_bytes()));
        args.iter().rev().for_each(|(t, _)| v.extend_from_slice(&t.to_le_bytes()));
        v.extend_from_slice(&payload);
        v
    }

    /// Curve coordinates (version 6+): points in 1/254000 in.
    fn curve32(points: &[(i32, i32, u8)]) -> Vec<u8> {
        let mut v = (points.len() as u16).to_le_bytes().to_vec();
        v.extend_from_slice(&[0, 0]);
        points.iter().for_each(|(x, y, _)| v.extend_from_slice(&le32(&[*x, *y])));
        points.iter().for_each(|(_, _, t)| v.push(*t));
        v
    }

    const IN: i32 = 254_000;

    #[test]
    fn page_space_maps_inches_y_up_centre_to_points_y_down_corner() {
        let page = PageSpace { width_in: 8.5, height_in: 11.0 };
        let a = page.to_document();
        assert_eq!(a * Point::new(0.0, 0.0), Point::new(306.0, 396.0));
        assert_eq!(a * Point::new(-4.25, 5.5), Point::new(0.0, 0.0));
        assert_eq!(a * Point::new(4.25, -5.5), Point::new(612.0, 792.0));
        assert_eq!(page.size_pt(), (612.0, 792.0));
    }

    #[test]
    fn corel_matrices_compose_in_file_order() {
        // Scale 2 about the origin, then move 1 in right.
        let m = [corel_matrix([2.0, 0.0, 0.0, 0.0, 2.0, 0.0]), corel_matrix([1.0, 0.0, 1.0, 0.0, 1.0, 0.0])];
        assert_eq!(compose(&m) * Point::new(1.0, 1.0), Point::new(3.0, 2.0));
        // v1 is the y→x coefficient: a 90° turn counter-clockwise (y up).
        let turn = corel_matrix([0.0, -1.0, 0.0, 1.0, 0.0, 0.0]);
        assert_eq!(turn * Point::new(1.0, 0.0), Point::new(0.0, 1.0));
    }

    #[test]
    fn trfd_versions() {
        // Version 9: header (length, count, start), one offset, then type 8, 6 bytes, 6 doubles.
        let mut v = Vec::new();
        for x in [0u32, 1, 12, 16] {
            v.extend_from_slice(&x.to_le_bytes());
        }
        v.extend_from_slice(&8u16.to_le_bytes());
        v.extend_from_slice(&[0; 6]);
        for d in [1.0f64, 0.0, 254_000.0, 0.0, -1.0, 508_000.0] {
            v.extend_from_slice(&d.to_le_bytes());
        }
        let m = decode_trfd(&v, V9).unwrap();
        assert_eq!(m.len(), 1);
        assert_eq!(m[0] * Point::new(0.0, 1.0), Point::new(1.0, 1.0));
        // Version 4: 16.16 fixed point and thousandths of an inch.
        let mut v = Vec::new();
        for x in [0u16, 1, 6, 8] {
            v.extend_from_slice(&x.to_le_bytes());
        }
        v.extend_from_slice(&8u16.to_le_bytes());
        for x in [0x0002_0000u32, 0, 1000, 0, 0x0001_0000, 0] {
            v.extend_from_slice(&x.to_le_bytes());
        }
        let m = decode_trfd(&v, CdrVersion(400)).unwrap();
        assert_eq!(m[0] * Point::new(1.0, 1.0), Point::new(3.0, 1.0));
        assert!(decode_trfd(&v[..10], CdrVersion(400)).is_err());
    }

    #[test]
    fn node_types_build_lines_curves_and_closed_subpaths() {
        let p = |x: f64, y: f64| Point::new(x, y);
        let path = CorelPath {
            points: vec![p(0.0, 0.0), p(1.0, 0.0), p(1.0, 1.0), p(0.0, 1.0), p(0.0, 0.0), p(5.0, 5.0), p(6.0, 5.0)],
            types: vec![
                node::MOVE,
                node::LINE,
                node::CONTROL,
                node::CONTROL,
                node::CURVE | node::CLOSED | node::SMOOTH,
                node::MOVE,
                node::CURVE, // a curve end without control points is a line
            ],
        };
        let bp = path.to_bezpath();
        use vectorcraft_geom::PathEl::*;
        assert_eq!(
            bp.elements(),
            [
                MoveTo(p(0.0, 0.0)),
                LineTo(p(1.0, 0.0)),
                CurveTo(p(1.0, 1.0), p(0.0, 1.0), p(0.0, 0.0)),
                ClosePath,
                MoveTo(p(5.0, 5.0)),
                LineTo(p(6.0, 5.0))
            ]
        );
        let doc = path.to_path_data(Affine::IDENTITY, &PageSpace { width_in: 2.0, height_in: 2.0 });
        assert_eq!(doc.subpaths.len(), 2);
        assert!(doc.subpaths[0].closed && !doc.subpaths[1].closed);
        // The closing anchor merged into the start: (0,0) in → (72,144) on a 2-inch page.
        assert_eq!(doc.subpaths[0].anchors.len(), 2);
        assert_eq!(doc.subpaths[0].anchors[0].p, p(72.0, 72.0));
        assert_eq!(doc.subpaths[0].anchors[0].h_in, p(72.0, 0.0));
    }

    #[test]
    fn curve_records_decode_in_both_precisions() {
        let rec = loda32(0x03, &[(0x1e, curve32(&[(0, 0, node::MOVE), (IN, 2 * IN, node::LINE)]))]);
        let l = decode_loda(&rec, V9).unwrap();
        assert_eq!(l.kind, ShapeKind::Curve);
        let path = CorelPath::decode(&rec, l.coords.unwrap(), l.kind, V9).unwrap();
        assert_eq!(path.points, [Point::new(0.0, 0.0), Point::new(1.0, 2.0)]);
        // Version 5: 16-bit header fields and thousandths of an inch.
        let mut v = Vec::new();
        for x in [0u16, 1, 10, 12, 3] {
            v.extend_from_slice(&x.to_le_bytes());
        }
        v.extend_from_slice(&14u16.to_le_bytes());
        v.extend_from_slice(&0x1eu16.to_le_bytes());
        v.extend_from_slice(&1u16.to_le_bytes());
        v.extend_from_slice(&[0, 0]);
        v.extend_from_slice(&(-500i16).to_le_bytes());
        v.extend_from_slice(&1500i16.to_le_bytes());
        v.push(node::MOVE);
        let l = decode_loda(&v, V5).unwrap();
        let path = CorelPath::decode(&v, l.coords.unwrap(), l.kind, V5).unwrap();
        assert_eq!(path.points, [Point::new(-0.5, 1.5)]);
        // A count beyond the data is cut to what is there.
        let mut short = curve32(&[(0, 0, node::MOVE)]);
        short[0] = 200;
        let rec = loda32(0x03, &[(0x1e, short)]);
        assert_eq!(CorelPath::decode(&rec, decode_loda(&rec, V9).unwrap().coords.unwrap(), ShapeKind::Curve, V9).unwrap().points.len(), 1);
    }

    #[test]
    fn colours_keep_their_model() {
        let c = |model, value| CorelColor { model, palette: 0, value }.to_color();
        assert_eq!(c(0x02, u32::from_le_bytes([100, 50, 0, 10])), Ok(Color::cmyk(1.0, 0.5, 0.0, 0.1)));
        assert_eq!(c(0x03, u32::from_le_bytes([255, 0, 0, 0])), Ok(Color::cmyk(1.0, 0.0, 0.0, 0.0)));
        assert_eq!(c(0x05, u32::from_le_bytes([0x30, 0x20, 0x10, 0])), Ok(Color::rgb8(0x10, 0x20, 0x30)));
        assert_eq!(c(0x09, 255), Ok(Color::gray(0.0)));
        assert_eq!(c(0x12, u32::from_le_bytes([255, 128, 0, 0])), Ok(Color::lab(100.0, 0.0, -128.0)));
        assert_eq!(c(0x06, u32::from_le_bytes([120, 0, 255, 255])), Ok(Color::rgb(0.0, 1.0, 0.0)));
        assert_eq!(c(0x07, u32::from_le_bytes([0, 0, 128, 255])).map(|c| matches!(c, Color::Rgb { r, .. } if r > 0.99)), Ok(true));
        assert!(c(0x19, 5).is_err() && c(0x77, 0).is_err());
        // Palette colours come from CorelDRAW's built-in palettes, tinted.
        let pal = |model, palette, value| CorelColor { model, palette, value }.to_color();
        assert_eq!(pal(0x19, 0x1e, 100 << 16 | 1), Ok(Color::rgb8(132, 118, 77)));
        assert_eq!(pal(0x19, 0x1a, 100 << 16 | 1), Ok(Color::cmyk(0.05, 0.0, 1.0, 0.0)));
        assert_eq!(pal(0x0e, 0x0c, 1), Ok(Color::lab(231.0 / 255.0 * 100.0, -1.0, 99.0)));
    }

    #[test]
    fn colour_records_by_version() {
        let mut v9 = 0x05u16.to_le_bytes().to_vec();
        v9.extend_from_slice(&[0, 0, 0, 0, 0, 0]);
        v9.extend_from_slice(&[0x30, 0x20, 0x10, 0]);
        let mut r = Reader::new(&v9, "t", V9);
        assert_eq!(CorelColor::read(&mut r).unwrap().to_color(), Ok(Color::rgb8(0x10, 0x20, 0x30)));
        // Version 13 reads model 1 as a palette colour.
        let mut v13 = 0x01u16.to_le_bytes().to_vec();
        v13.extend_from_slice(&[3, 0, 0, 0, 0, 0, 1, 0, 0, 0]);
        let mut r = Reader::new(&v13, "t", CdrVersion(1300));
        assert_eq!(CorelColor::read(&mut r).unwrap(), CorelColor { model: 0x19, palette: 3, value: 1 });
        // Version 4: four 16-bit components packed into bytes.
        let mut v4 = 0x02u16.to_le_bytes().to_vec();
        for x in [10u16, 20, 30, 40, 0] {
            v4.extend_from_slice(&x.to_le_bytes());
        }
        let mut r = Reader::new(&v4, "t", CdrVersion(400));
        assert_eq!(CorelColor::read(&mut r).unwrap().value, u32::from_le_bytes([10, 20, 30, 40]));
    }

    fn color9(model: u16, bytes: [u8; 4]) -> Vec<u8> {
        let mut v = model.to_le_bytes().to_vec();
        v.extend_from_slice(&[0; 6]);
        v.extend_from_slice(&bytes);
        v
    }

    /// A version 9 `outl` record.
    fn outl9(id: u32, line_type: u16, cap: u16, join: u16, width_in: f64, dashes: &[u16]) -> Vec<u8> {
        let mut v = id.to_le_bytes().to_vec();
        for x in [line_type, cap, join, 0] {
            v.extend_from_slice(&x.to_le_bytes());
        }
        v.extend_from_slice(&((width_in * 254_000.0) as i32).to_le_bytes());
        v.extend_from_slice(&100u16.to_le_bytes());
        v.extend_from_slice(&[0, 0]);
        v.extend_from_slice(&0i32.to_le_bytes());
        v.extend_from_slice(&[0; 52]);
        v.extend_from_slice(&color9(0x02, [0, 100, 100, 0]));
        v.extend_from_slice(&[0; 16]);
        v.extend_from_slice(&(dashes.len() as u16).to_le_bytes());
        let at = v.len();
        dashes.iter().for_each(|d| v.extend_from_slice(&d.to_le_bytes()));
        v.resize(at + 22, 0);
        v.extend_from_slice(&[0; 8]);
        v
    }

    #[test]
    fn outlines_map_caps_joins_width_and_dashes() {
        let o = Outline::decode(&outl9(7, 0x06, 1, 2, 0.5, &[3, 1]), V9).unwrap();
        assert_eq!((o.id, o.cap, o.join), (7, 1, 2));
        let mut notes = Vec::new();
        let s = o.to_stroke(Affine::scale(3.0), &mut notes).unwrap();
        assert_eq!((s.cap, s.join, s.width), (LineCap::Round, LineJoin::Bevel, 36.0));
        assert_eq!(s.paint, Paint::solid(Color::cmyk(0.0, 1.0, 1.0, 0.0)));
        assert_eq!(s.dash.as_ref().unwrap().pattern, [108.0, 36.0]);
        assert!(notes.is_empty(), "{notes:?}");
        // Scaling with the object, square caps, miter joins, no dash bit.
        let o = Outline::decode(&outl9(8, 0x02 | Outline::SCALE_WITH_OBJECT, 2, 0, 1.0 / 72.0, &[3, 1]), V9).unwrap();
        let s = o.to_stroke(Affine::scale(3.0), &mut notes).unwrap();
        assert_eq!((s.cap, s.join, s.dash.is_none()), (LineCap::Square, LineJoin::Miter, true));
        assert!((s.width - 3.0).abs() < 1e-3);
        let none = Outline::decode(&outl9(9, Outline::NONE, 0, 0, 1.0, &[]), V9).unwrap();
        assert!(none.to_stroke(Affine::IDENTITY, &mut notes).is_none());
        assert!(Outline::decode(&outl9(9, 0, 0, 0, 1.0, &[])[..40], V9).is_err());
    }

    /// A version 9 fountain `fild`.
    fn fountain9(kind: u8, pad: i32, angle_deg: i32, center: (i32, i32), mid: u8, stops: &[([u8; 4], u16)]) -> Vec<u8> {
        let mut v = 1u32.to_le_bytes().to_vec();
        v.extend_from_slice(&2u16.to_le_bytes());
        v.extend_from_slice(&[0, 0]);
        v.push(kind);
        v.extend_from_slice(&[0; 19]);
        v.extend_from_slice(&pad.to_le_bytes());
        v.extend_from_slice(&(angle_deg * 1_000_000).to_le_bytes());
        v.extend_from_slice(&le32(&[center.0, center.1]));
        v.extend_from_slice(&[0, 0]);
        v.extend_from_slice(&0u32.to_le_bytes());
        v.push(mid);
        v.push(0);
        v.extend_from_slice(&(stops.len() as u32).to_le_bytes());
        for (rgb, at) in stops {
            v.extend_from_slice(&color9(0x05, *rgb));
            v.extend_from_slice(&u32::from(*at).to_le_bytes());
        }
        v
    }

    #[test]
    fn solid_and_fountain_fills() {
        let mut solid = 4u32.to_le_bytes().to_vec();
        solid.extend_from_slice(&1u16.to_le_bytes());
        solid.extend_from_slice(&[0, 0]);
        solid.extend_from_slice(&color9(0x09, [0, 0, 0, 0]));
        let (id, f) = CorelFill::decode(&solid, V9).unwrap();
        let mut notes = Vec::new();
        assert_eq!((id, f.to_fill(None, &mut notes).map(|l| l.paint)), (4, Some(Paint::solid(Color::gray(1.0)))));

        let b = Rect::new(0.0, 0.0, 200.0, 100.0);
        let rec = fountain9(1, 10, 90, (0, 0), 30, &[([0, 0, 255, 0], 0), ([255, 0, 0, 0], 100)]);
        let (_, f) = CorelFill::decode(&rec, V9).unwrap();
        let Some(Paint::Gradient(g)) = f.to_fill(Some(b), &mut notes).map(|l| l.paint) else { panic!() };
        assert_eq!(g.gradient.kind, GradientKind::Linear);
        assert!((g.angle - 90.0).abs() < 1e-9);
        assert_eq!(g.gradient.stops[0].color, Color::rgb8(255, 0, 0));
        assert!((g.gradient.stops[0].midpoint - 0.3).abs() < 1e-6);
        // 90°: bottom (y-down 100) to top, shortened 10% at each end.
        let geom = g.geom.unwrap();
        assert!((geom.start.y - 90.0).abs() < 1e-9 && (geom.end.y - 10.0).abs() < 1e-9, "{geom:?}");

        let rec = fountain9(2, 0, 0, (50, 50), 50, &[([0, 0, 0, 0], 0), ([255, 255, 255, 0], 100)]);
        let Some(Paint::Gradient(g)) = CorelFill::decode(&rec, V9).unwrap().1.to_fill(Some(b), &mut notes).map(|l| l.paint) else { panic!() };
        assert_eq!(g.gradient.kind, GradientKind::Radial);
        // The rim colour (first stop) moves to offset 1; the centre moves right and up.
        assert_eq!((g.gradient.stops[1].offset, g.gradient.stops[1].color), (1.0, Color::rgb8(0, 0, 0)));
        assert_eq!(g.geom.unwrap().start, Point::new(150.0, 25.0));

        let rec = fountain9(3, 0, 0, (0, 0), 50, &[([0; 4], 0), ([0; 4], 100)]);
        CorelFill::decode(&rec, V9).unwrap().1.to_fill(Some(b), &mut notes);
        assert!(notes.iter().any(|n| n.contains("conical")));
        let one = fountain9(1, 0, 0, (0, 0), 50, &[([1, 2, 3, 0], 0)]);
        assert!(matches!(CorelFill::decode(&one, V9).unwrap().1.to_fill(None, &mut notes).map(|l| l.paint), Some(Paint::Solid { .. })));
    }

    #[test]
    fn translate_places_objects_on_their_page() {
        let mut fill = 4u32.to_le_bytes().to_vec();
        fill.extend_from_slice(&1u16.to_le_bytes());
        fill.extend_from_slice(&[0, 0]);
        fill.extend_from_slice(&color9(0x05, [0, 0, 255, 0]));
        let mut page_size = le32(&[2 * IN, 2 * IN]);
        page_size.truncate(8);
        let tri = curve32(&[(0, 0, node::MOVE), (IN, 0, node::LINE), (0, IN, node::LINE | node::CLOSED)]);
        let obj = |dx: i32| {
            let mut m = Vec::new();
            for x in [0u32, 1, 12, 16] {
                m.extend_from_slice(&x.to_le_bytes());
            }
            m.extend_from_slice(&8u16.to_le_bytes());
            m.extend_from_slice(&[0; 6]);
            for d in [1.0f64, 0.0, f64::from(dx), 0.0, 1.0, 0.0] {
                m.extend_from_slice(&d.to_le_bytes());
            }
            build::list(
                b"LIST",
                b"obj ",
                &[
                    build::chunk(b"loda", &loda32(0x03, &[(0x14, 4u32.to_le_bytes().to_vec()), (0x1e, tri.clone())])),
                    build::list(b"LIST", b"trfl", &[build::chunk(b"trfd", &m)]),
                ],
            )
        };
        let file = build::list(
            b"RIFF",
            b"CDR9",
            &[
                build::list(b"LIST", b"filt", &[build::chunk(b"fild", &fill)]),
                build::list(b"LIST", b"page", &[build::chunk(b"flgs", &0x0098_0000u32.to_le_bytes())]),
                build::list(
                    b"LIST",
                    b"page",
                    &[
                        build::chunk(b"flgs", &0u32.to_le_bytes()),
                        build::chunk(b"loda", &loda32(0x00, &[(0x4aba, page_size)])),
                        build::list(b"LIST", b"layr", &[obj(0), obj(-IN)]),
                    ],
                ),
                build::list(
                    b"LIST",
                    b"page",
                    &[build::list(b"LIST", b"layr", &[build::list(b"LIST", b"obj ", &[build::chunk(b"loda", &loda32(0x01, &[]))])])],
                ),
            ],
        );
        let riff = crate::riff::parse(&file, &crate::Limits::default()).unwrap();
        let t = translate(&riff, None);
        assert!(t.errors.is_empty(), "{:?}", t.errors);
        assert_eq!(t.pages.iter().map(|p| (p.index, p.master)).collect::<Vec<_>>(), [(0, true), (1, false), (2, false)]);
        let p0 = &t.pages[1];
        assert_eq!(p0.page.size_pt(), (144.0, 144.0));
        // Bottom first: the second object in the file (moved 1 in left) is the first shape.
        let b0 = p0.leaves()[0].path().unwrap().bounds().unwrap();
        let b1 = p0.leaves()[1].path().unwrap().bounds().unwrap();
        assert_eq!((b0.x0, b0.y0, b0.x1, b0.y1), (0.0, 0.0, 72.0, 72.0));
        assert_eq!((b1.x0, b1.y0, b1.x1, b1.y1), (72.0, 0.0, 144.0, 72.0));
        assert!(p0.leaves()[0].path().unwrap().subpaths[0].closed);
        assert_eq!(p0.leaves()[0].appearance.fill_paint(), Paint::solid(Color::rgb8(255, 0, 0)));
        assert!(matches!(p0.leaves()[0].content, Content::Path { rule: FillRule::EvenOdd, .. }));
        assert!(p0.leaves()[0].appearance.stroke().is_none());
        // The second page keeps the size and counts its rectangle as skipped.
        assert_eq!(t.pages[2].page.size_pt(), (144.0, 144.0));
        assert_eq!(t.skipped.get("Rectangle"), Some(&1));
    }

    /// A transparency block (argument 8000) as the version 17 samples write it: `start` and `end`
    /// percent, fading along `line` (object space, 1/254000 in) when one is given.
    fn transparency_block(start: f64, end: f64, line: Option<[i32; 4]>) -> Vec<u8> {
        let mut b = vec![0u8; 125];
        b[0..4].copy_from_slice(&0x514u32.to_le_bytes());
        b[4..8].copy_from_slice(&125u32.to_le_bytes());
        if let Some(l) = line {
            b[45..49].copy_from_slice(&1u32.to_le_bytes());
            b[49..65].copy_from_slice(&le32(&l));
        }
        b[69..77].copy_from_slice(&start.to_le_bytes());
        b[77..85].copy_from_slice(&end.to_le_bytes());
        b
    }

    #[test]
    fn texture_fills_tile_their_bitmap() {
        // Version 14: the fill's 0x514 headers, then its parameters with the tile size at bytes
        // 20 and 24 and the bitmap at byte 32.
        let mut rec = 9u32.to_le_bytes().to_vec();
        for v in [0x514u32, 0x17e] {
            rec.extend_from_slice(&v.to_le_bytes());
        }
        rec.extend_from_slice(&11u16.to_le_bytes());
        for v in [0x514u32, 0x16c, 0x514, 0x2c] {
            rec.extend_from_slice(&v.to_le_bytes());
        }
        let mut params = vec![0u8; 52];
        params[20..24].copy_from_slice(&(IN as u32).to_le_bytes());
        params[24..28].copy_from_slice(&(2 * IN as u32).to_le_bytes());
        params[32..36].copy_from_slice(&3u32.to_le_bytes());
        params[48..50].copy_from_slice(&300u16.to_le_bytes());
        rec.extend_from_slice(&params);
        let (id, fill) = CorelFill::decode(&rec, CdrVersion(1400)).unwrap();
        assert_eq!((id, fill), (9, CorelFill::Tiled(TiledFill { bitmap: 3, width_in: 1.0, height_in: 2.0, dpi: Some(300.0) })));
        // Cut short, or with an empty tile: not read, reported as the fill type.
        assert_eq!(CorelFill::decode(&rec[..40], CdrVersion(1400)).unwrap().1, CorelFill::Unsupported(11));
        let mut flat = rec.clone();
        flat[30 + 20..30 + 24].copy_from_slice(&0u32.to_le_bytes());
        assert_eq!(CorelFill::decode(&flat, CdrVersion(1400)).unwrap().1, CorelFill::Unsupported(11));
    }

    #[test]
    fn envelopes_decode_as_curves() {
        // A table of two arguments: node types (one byte per point) and the points, a closed
        // square of lines from (0, 0) to (1 in, 1 in).
        let types = [node::MOVE, node::LINE, node::LINE, node::LINE | node::CLOSED];
        let points = [(0, 0), (IN, 0), (IN, IN), (0, IN)];
        let (types_at, points_at) = (32u32, 36u32);
        let len = points_at as usize + points.len() * 8;
        let mut rec = Vec::new();
        for v in [len as u32, 2, 16, 24, 25, 0, 0, 0] {
            rec.extend_from_slice(&v.to_le_bytes());
        }
        rec[16..20].copy_from_slice(&types_at.to_le_bytes());
        rec[20..24].copy_from_slice(&points_at.to_le_bytes());
        rec.truncate(32);
        rec.extend_from_slice(&types);
        for (x, y) in points {
            rec.extend_from_slice(&le32(&[x, y]));
        }
        let e = decode_envd(&rec, CdrVersion(1400)).unwrap();
        let b = PathData::from_bezpath(&e).bounds().unwrap();
        assert_eq!((b.x0, b.y0, b.x1, b.y1), (0.0, 0.0, 1.0, 1.0));
        assert!(PathData::from_bezpath(&e).subpaths[0].closed);
        // Anything shorter or of another shape: no envelope.
        assert!(decode_envd(&rec[..40], CdrVersion(1400)).is_none());
        let mut three = rec.clone();
        three[4] = 3;
        assert!(decode_envd(&three, CdrVersion(1400)).is_none());
    }

    #[test]
    fn fill_transforms() {
        let rec: Vec<u8> = [1.0f64, 0.5, 254_000.0, 0.0, 2.0, -508_000.0].iter().flat_map(|v| v.to_le_bytes()).collect();
        // x' = x + 0.5 y + 1 in, y' = 2 y - 2 in.
        assert_eq!(decode_ftil(&rec), Some(Affine::new([1.0, 0.0, 0.5, 2.0, 1.0, -2.0])));
        assert_eq!(decode_ftil(&rec[..40]), None);
        let flat: Vec<u8> = [1.0f64, 0.0, 0.0, 0.0, 0.0, 0.0].iter().flat_map(|v| v.to_le_bytes()).collect();
        assert_eq!(decode_ftil(&flat), None);
    }

    #[test]
    fn shifted_characters_become_kerning() {
        let run = |t: &str| TextRun::new(t, CharStyle { size: 10.0, ..CharStyle::default() });
        let mut runs = vec![run("T"), run("A"), run("N")];
        let mut notes = Vec::new();
        // Shifted 1, 1.5 and 1 em: the text starts 10 pt in, T's advance grows 0.5 em, A's shrinks.
        let lead = shift_characters(&mut runs, &[1.0, 1.5, 1.0], &mut notes);
        assert_eq!(lead, 10.0);
        assert_eq!(runs.iter().map(|r| r.style.kerning).collect::<Vec<_>>(), [Some(500.0), Some(-500.0), None]);
        assert!(notes.is_empty());
        // A shifted run of several characters can't be: noted, left alone.
        let mut runs = vec![run("TA"), run("N")];
        assert_eq!(shift_characters(&mut runs, &[1.0, 0.0], &mut notes), 0.0);
        assert!(runs.iter().all(|r| r.style.kerning.is_none()) && notes.len() == 1);
    }

    #[test]
    fn version_29_fills_skip_their_identifier_block() {
        // CorelDRAW 2025: the id, a 0x960 block holding a 16-byte identifier, then the fill's own
        // 0x514 block as before.
        let mut rec = 1u32.to_le_bytes().to_vec();
        rec.extend_from_slice(&0x960u32.to_le_bytes());
        rec.extend_from_slice(&16u32.to_le_bytes());
        rec.extend_from_slice(&[0xab; 16]);
        rec.extend_from_slice(&0x514u32.to_le_bytes());
        rec.extend_from_slice(&0x4du32.to_le_bytes());
        rec.extend_from_slice(&1u16.to_le_bytes());
        rec.extend_from_slice(&[0; 13]);
        rec.extend_from_slice(&color9(0x05, [0xde, 0xc3, 0x33, 0]));
        let mut notes = Vec::new();
        let (id, fill) = CorelFill::decode(&rec, CdrVersion(2900)).unwrap();
        assert_eq!((id, fill.to_fill(None, &mut notes).map(|l| l.paint)), (1, Some(Paint::solid(Color::rgb8(0x33, 0xc3, 0xde)))));
        // A record whose blocks run out is an error, not a panic.
        assert!(CorelFill::decode(&rec[..30], CdrVersion(2900)).is_err());
    }

    #[test]
    fn transparencies_uniform_and_fading() {
        assert_eq!(transparency(&transparency_block(40.0, 0.0, None)), Some(0.6));
        // A fade is averaged for the object's opacity …
        let fading = transparency_block(0.0, 100.0, Some([0, 0, IN, 0]));
        assert_eq!(transparency(&fading), Some(0.5));
        // … and kept as a line: the first percentage belongs to the line's far end.
        assert_eq!(fade(&fading), Some(Fade { from: Point::ORIGIN, to: Point::new(1.0, 0.0), opacity: (0.0, 1.0) }));
        assert_eq!(fade(&transparency_block(40.0, 0.0, Some([0, 0, IN, 0]))), None, "uniform");
        assert_eq!(fade(&transparency_block(0.0, 100.0, None)), None, "a fade without ends");
        let mut other = fading.clone();
        other[0] = 0;
        assert_eq!((transparency(&other), fade(&other)), (None, None));
        assert_eq!(fade(&fading[..70]), None);
    }

    #[test]
    fn layers_groups_and_powerclips_keep_their_structure() {
        let mut fill = 4u32.to_le_bytes().to_vec();
        fill.extend_from_slice(&1u16.to_le_bytes());
        fill.extend_from_slice(&[0, 0]);
        fill.extend_from_slice(&color9(0x05, [0, 0, 255, 0]));
        let mut page_size = le32(&[2 * IN, 2 * IN]);
        page_size.truncate(8);
        let tri = |dx: i32| curve32(&[(dx, 0, node::MOVE), (dx + IN, 0, node::LINE), (dx, IN, node::LINE | node::CLOSED)]);
        let obj = |args: Vec<(u32, Vec<u8>)>| build::list(b"LIST", b"obj ", &[build::chunk(b"loda", &loda32(0x03, &args))]);
        let filled = |dx: i32| vec![(0x14, 4u32.to_le_bytes().to_vec()), (0x1e, tri(dx))];
        let mut container = filled(-IN);
        container.push((8005, [7u32, 1, 0, 1].iter().flat_map(|v| v.to_le_bytes()).collect()));
        container.push((8000, transparency_block(0.0, 100.0, Some([-IN, 0, 0, 0]))));
        container.push((1000, b"Frame\0".to_vec()));
        let file = build::list(
            b"RIFF",
            b"CDR9",
            &[
                build::list(b"LIST", b"filt", &[build::chunk(b"fild", &fill)]),
                // The PowerClip's contents, by `spnd` id 7.
                build::list(b"LIST", b"clpt", &[build::list(b"LIST", b"grp ", &[build::chunk(b"spnd", &7u32.to_le_bytes()), obj(filled(-IN / 2))])]),
                build::list(b"LIST", b"page", &[build::chunk(b"flgs", &0x0098_0000u32.to_le_bytes())]),
                build::list(
                    b"LIST",
                    b"page",
                    &[
                        build::chunk(b"flgs", &0u32.to_le_bytes()),
                        build::chunk(b"loda", &loda32(0x00, &[(0x4aba, page_size)])),
                        build::list(
                            b"LIST",
                            b"layr",
                            &[
                                build::chunk(b"loda", &loda32(0x00, &[(1000, b"Art\0".to_vec())])),
                                // Front first: the container, then a group of two triangles.
                                obj(container),
                                build::list(b"LIST", b"grp ", &[obj(filled(0)), obj(filled(-IN))]),
                            ],
                        ),
                        build::list(b"LIST", b"layr", &[build::chunk(b"loda", &loda32(0x11, &[]))]),
                    ],
                ),
            ],
        );
        let riff = crate::riff::parse(&file, &crate::Limits::default()).unwrap();
        let t = translate(&riff, None);
        assert!(t.errors.is_empty(), "{:?}", t.errors);
        let page = &t.pages[1];
        let layers: Vec<_> = page.layers.iter().map(|l| (l.name.as_deref(), l.kind)).collect();
        assert_eq!(layers, [(Some("Desktop"), LayerKind::Desktop), (Some("Art"), LayerKind::Drawing)], "bottom first");
        let art = &page.layers[1].shapes;
        assert_eq!(art.len(), 3, "the group, then the container's fill and its clipped contents");
        let Content::Group { children, clip: false } = &art[0].content else { panic!("{:?}", art[0].content) };
        assert_eq!(children.len(), 2);
        // The container: its fill, faded (opacity mask) with its average opacity and its name.
        assert_eq!((art[1].opacity, art[1].name.as_deref()), (0.5, Some("Frame")));
        let f = art[1].fade.unwrap();
        assert_eq!((f.from, f.to, f.opacity), (Point::new(0.0, 72.0), Point::new(72.0, 72.0), (0.0, 1.0)));
        assert!(art[1].appearance.stroke().is_none());
        // Then a clip group: the container's outline, then the `clpt` contents.
        let Content::Group { children, clip: true } = &art[2].content else { panic!("{:?}", art[2].content) };
        assert_eq!(children.len(), 2);
        assert_eq!(children[0].path().unwrap().bounds(), art[1].path().unwrap().bounds());
        assert!(children[0].appearance.items.is_empty());
        let Content::Group { children: inner, clip: false } = &children[1].content else { panic!("{:?}", children[1].content) };
        assert_eq!(inner[0].path().unwrap().bounds().map(|b| (b.x0, b.x1)), Some((36.0, 108.0)));
        assert_eq!(page.shapes().len(), 8);
        assert_eq!(page.leaves().len(), 5);
    }

    #[test]
    fn version_17_radial_fountains_are_ellipses_on_their_frame() {
        let f = Fountain {
            kind: 2,
            edge_pad: 0,
            angle: 0.0,
            center: (0, 0),
            mode: 0,
            midpoint: 0.5,
            stops: vec![(0.0, CorelColor { model: 5, palette: 0, value: 0 }), (1.0, CorelColor { model: 5, palette: 0, value: 0xff })],
            // Centred a quarter box right, as wide as the box and half as tall.
            frame: Some([0.25, 0.0, 1.0, 0.0, 0.0, 0.5]),
        };
        let local = Rect::new(-1.0, -1.0, 1.0, 1.0);
        let to_doc = PageSpace { width_in: 2.0, height_in: 2.0 }.to_document();
        let mut notes = Vec::new();
        let paint = CorelFill::Fountain(f).to_fill_on(Some(to_doc.transform_rect_bbox(local)), Some((local, to_doc)), &mut notes).map(|l| l.paint);
        let Some(Paint::Gradient(g)) = paint else { panic!("{paint:?}") };
        let geom = g.geom.unwrap();
        assert_eq!((geom.start, geom.end, geom.aspect), (Point::new(108.0, 72.0), Point::new(180.0, 72.0), 0.5));
        // CorelDRAW's first stop (black) is at the rim, its last (blue) at the centre.
        assert_eq!((g.gradient.stops[0].offset, g.gradient.stops[0].color), (0.0, Color::rgb8(0, 0, 255)));
        assert_eq!(g.gradient.stops[1].color, Color::rgb8(0, 0, 0));
    }

    #[test]
    fn version_17_fountains_lie_on_their_frame() {
        let f = Fountain {
            kind: 1,
            edge_pad: 10,
            angle: -std::f64::consts::FRAC_PI_2,
            center: (0, 0),
            mode: 0,
            midpoint: 0.5,
            stops: vec![(0.0, CorelColor { model: 5, palette: 0, value: 0 }), (1.0, CorelColor { model: 5, palette: 0, value: 0xff })],
            // Centre half a box below the box's, the ramp twice the box's height, downwards.
            frame: Some([0.0, -0.5, 0.0, 2.0, -1.0, 0.0]),
        };
        let local = Rect::new(0.0, 0.0, 1.0, 1.0);
        assert_eq!(Fountain::frame_ends(f.frame.unwrap(), local), Some((Point::new(0.5, 1.0), Point::new(0.5, -1.0))));
        let to_doc = PageSpace { width_in: 2.0, height_in: 2.0 }.to_document();
        let doc = to_doc.transform_rect_bbox(local);
        let mut notes = Vec::new();
        let paint = CorelFill::Fountain(f.clone()).to_fill_on(Some(doc), Some((local, to_doc)), &mut notes).map(|l| l.paint);
        let Some(Paint::Gradient(g)) = paint else { panic!("{paint:?}") };
        let g = g.geom.unwrap();
        assert_eq!((g.start, g.end), (Point::new(108.0, 0.0), Point::new(108.0, 144.0)), "the frame, not the pad");
        // Without the object's own box, the angle and pad lay it out.
        let paint = CorelFill::Fountain(f).to_fill(Some(doc), &mut notes).map(|l| l.paint);
        let Some(Paint::Gradient(g)) = paint else { panic!() };
        assert_eq!(g.geom.unwrap().start.y, 7.2);
    }

    #[test]
    fn rectangles_round_scallop_and_chamfer_their_corners() {
        let b = |bp: &BezPath| vectorcraft_geom::Shape::bounding_box(bp);
        // A 2 × 1 in rectangle drawn downwards (y up), radius 0.25 at each corner.
        for corner in [0, 1, 2] {
            let bp = rectangle(2.0, -1.0, [0.25; 4], corner, (1.0, 1.0));
            let r = b(&bp);
            assert!((r.x0, r.x1, r.y0, r.y1) == (0.0, 2.0, -1.0, 0.0), "{corner}: {r:?}");
            // Round and chamfered corners cut the corner point off; scalloped ones bite inwards.
            assert!(!vectorcraft_geom::Shape::contains(&bp, Point::new(0.01, -0.01)), "{corner}");
        }
        let round = rectangle(2.0, -1.0, [0.25; 4], 0, (1.0, 1.0));
        let along = round.elements().iter().filter(|e| matches!(e, vectorcraft_geom::PathEl::CurveTo(..))).count();
        assert_eq!(along, 4, "round corners are circular cubics");
        let square = rectangle(2.0, -1.0, [0.0; 4], 0, (1.0, 1.0));
        assert!((vectorcraft_geom::Shape::area(&square).abs() - 2.0).abs() < 1e-12);
        // Version 9 stores four radii; version 15 doubles, here radii relative to the half-side.
        let mut v9 = Vec::new();
        for x in [IN, -IN, 0, IN / 10, 0, 0] {
            v9.extend_from_slice(&x.to_le_bytes());
        }
        let r = b(&decode_rectangle(&v9, 0, V9).unwrap());
        assert!((r.width() - 1.0).abs() < 1e-9 && (r.height() - 1.0).abs() < 1e-9);
        let mut v15 = Vec::new();
        for d in [254_000.0f64, -127_000.0, 1.0, 1.0] {
            v15.extend_from_slice(&d.to_le_bytes());
        }
        v15.extend_from_slice(&[0; 8]);
        for r in [0.5f64, 0.5, 0.5, 0.5] {
            v15.extend_from_slice(&r.to_le_bytes());
            v15.extend_from_slice(&[0; 16]);
        }
        let rect = decode_rectangle(&v15, 0, CdrVersion(1500)).unwrap();
        // Radius 0.5 × the shorter half-side (0.25 in): the curve starts 0.125 in down the edge.
        assert!(rect.elements().iter().any(|e| matches!(e, vectorcraft_geom::PathEl::MoveTo(p) if (p.y + 0.125).abs() < 1e-9)), "{rect:?}");
        assert!(decode_rectangle(&v15[..40], 0, CdrVersion(1500)).is_err());
    }

    #[test]
    fn ellipses_arcs_and_pies() {
        let b = |bp: &BezPath| vectorcraft_geom::Shape::bounding_box(bp);
        let full = ellipse(2.0, -1.0, 0.0, 0.0, false);
        let r = b(&full);
        // Within the arc flattening tolerance (1e-4 in, under 0.01 pt).
        assert!((r.x0.abs() + (r.x1 - 2.0).abs() + (r.y0 + 1.0).abs() + r.y1.abs()) < ARC_TOLERANCE, "{r:?}");
        let area = vectorcraft_geom::Shape::area(&full).abs();
        assert!((area - std::f64::consts::PI * 0.5).abs() < 1e-3, "{area}");
        // Three quarters of a circle, closed through the centre.
        let pie = ellipse(1.0, 1.0, 0.0, 1.5 * std::f64::consts::PI, true);
        assert!((vectorcraft_geom::Shape::area(&pie).abs() - 0.75 * std::f64::consts::PI * 0.25).abs() < 1e-3);
        assert!(matches!(pie.elements().last(), Some(vectorcraft_geom::PathEl::ClosePath)));
        let arc = ellipse(1.0, 1.0, 0.0, 0.5 * std::f64::consts::PI, false);
        assert!(!matches!(arc.elements().last(), Some(vectorcraft_geom::PathEl::ClosePath)), "an arc stays open");
        let mut rec = Vec::new();
        for x in [IN, IN, 0, 90_000_000, 1] {
            rec.extend_from_slice(&x.to_le_bytes());
        }
        assert!(decode_ellipse(&rec, 0, V9).is_ok() && decode_ellipse(&rec[..12], 0, V9).is_err());
    }

    #[test]
    fn rectangles_ellipses_and_type_translate_onto_the_page() {
        let bytes = crate::synth::shapes_riff();
        let riff = crate::riff::parse(&bytes, &crate::Limits::default()).unwrap();
        let t = translate(&riff, None);
        assert!(t.errors.is_empty() && t.skipped.is_empty(), "{:?} {:?}", t.errors, t.skipped);
        let shapes = t.pages[0].leaves();
        // Bottom first: paragraph text, artistic text, pie, rectangle.
        assert_eq!(shapes.len(), 4);
        let para = shapes[0].text().unwrap();
        let art = shapes[1].text().unwrap();
        let pie = shapes[2].path().unwrap();
        let rect = shapes[3].path().unwrap();

        let r = rect.bounds().unwrap();
        assert!((r.x0.abs() + r.y0.abs() + (r.x1 - 144.0).abs() + (r.y1 - 72.0).abs()) < 1e-6, "{r:?}");
        assert_eq!(shapes[3].appearance.fill_paint(), Paint::solid(Color::rgb8(255, 0, 0)));
        let p = pie.bounds().unwrap();
        assert!((p.x0 - 144.0).abs() < 1e-6 && (p.y1 - 108.0).abs() < 1e-6, "the pie's box starts at the page centre: {p:?}");
        assert!(pie.is_closed());

        // Artistic text: point type whose baseline starts at the page centre, its style inherited
        // and overridden run by run.
        assert!(matches!(art.kind, TextKind::Point));
        assert_eq!(art.xf * Point::ZERO, Point::new(144.0, 108.0));
        assert_eq!(art.xf.as_coeffs()[..4], [1.0, 0.0, 0.0, 1.0], "upright, in points");
        assert_eq!(art.plain_text(), "Hello wörld");
        let sizes: Vec<_> = art.runs.iter().map(|r| (r.text.as_str(), (r.style.size * 100.0).round() / 100.0)).collect();
        assert_eq!(sizes, [("Hello ", 24.0), ("wörld", 12.0)]);
        assert!(art.runs.iter().all(|r| r.style.font_family == "Arial" && r.style.fill == Paint::solid(Color::rgb8(255, 0, 0))));
        assert_eq!(art.para.justify, Justify::Center);

        // Paragraph text: area type in its 1.5 × 0.5 in frame, from the object's origin.
        let TextKind::Area { frame } = &para.kind else { panic!("area type") };
        let f = frame.bounds().unwrap();
        assert!((f.width() - 108.0).abs() < 1e-6 && (f.height() - 36.0).abs() < 1e-6, "{f:?}");
        assert_eq!(para.xf * Point::ZERO, Point::new(180.0, 144.0));
        assert_eq!((para.plain_text().as_str(), para.para.justify), ("Para", Justify::Left));
        assert_eq!(para.runs[0].style.size.round(), 24.0, "inherited from the parent style");
        assert!(shapes.iter().all(|s| s.notes.is_empty()), "{:?}", shapes.iter().map(|s| &s.notes).collect::<Vec<_>>());
    }

    #[test]
    fn line_breaks_become_paragraphs_with_the_same_style() {
        let paras = vec![Paragraph {
            style: 1,
            runs: vec![crate::text::Run { chars: crate::text::Chars::Bytes(b"a\rb".to_vec()), style: TextStyle::default() }],
        }];
        let styles = Styles { styles: BTreeMap::from([(1, (0, TextStyle { align: Some(3), size: Some(0.25), ..TextStyle::default() }))]) };
        let texts = BTreeMap::from([(5, paras.clone()), (6, [paras.clone(), paras].concat())]);
        let riff_bytes = crate::synth::list(b"RIFF", b"CDR9", &[]);
        let riff = crate::riff::parse(&riff_bytes, &crate::Limits::default()).unwrap();
        let empty = BTreeMap::new();
        let no_bitmaps = BTreeMap::new();
        let ctx = Ctx {
            src: Records { riff: &riff, streams: &[] },
            version: V9,
            fills: &empty,
            outlines: &BTreeMap::new(),
            styles: &styles,
            texts: &texts,
            bitmaps: &no_bitmaps,
            arrows: &BTreeMap::new(),
            clips: BTreeMap::new(),
        };
        let mut notes = Vec::new();
        let t = ctx.text_object(TextFrame::Artistic, Some(5), Affine::IDENTITY, Affine::IDENTITY, &mut notes);
        assert_eq!((t.plain_text().as_str(), t.para.justify), ("a\nb", Justify::Right));
        let two = ctx.text_object(TextFrame::Artistic, Some(6), Affine::IDENTITY, Affine::IDENTITY, &mut notes);
        assert_eq!(two.plain_text(), "a\nb\na\nb");
        assert!(notes.is_empty());
        let none = ctx.text_object(TextFrame::Artistic, Some(9), Affine::IDENTITY, Affine::IDENTITY, &mut notes);
        assert!(none.plain_text().is_empty() && notes.iter().any(|n| n.contains("not found")));
    }

    /// A point of the clamped uniform cubic B-spline on `p` at `u` (Cox–de Boor, for reference).
    fn de_boor(p: &[Point], u: f64) -> Point {
        let n = p.len();
        let t: Vec<f64> = (0..n + 4).map(|i| (i.saturating_sub(3)).min(n - 3) as f64).collect();
        fn basis(t: &[f64], i: usize, k: usize, u: f64, last: bool) -> f64 {
            if k == 0 {
                return if (t[i] <= u && u < t[i + 1]) || (last && u == t[i + 1] && t[i] < t[i + 1] && t[i + 1] == *t.last().unwrap()) {
                    1.0
                } else {
                    0.0
                };
            }
            let a = if t[i + k] > t[i] { (u - t[i]) / (t[i + k] - t[i]) * basis(t, i, k - 1, u, last) } else { 0.0 };
            let b = if t[i + k + 1] > t[i + 1] { (t[i + k + 1] - u) / (t[i + k + 1] - t[i + 1]) * basis(t, i + 1, k - 1, u, last) } else { 0.0 };
            a + b
        }
        let last = u >= t[n];
        p.iter().enumerate().fold(Point::ZERO, |acc, (i, q)| acc + q.to_vec2() * basis(&t, i, 3, u, last))
    }

    #[test]
    fn b_splines_become_the_same_curve_in_beziers() {
        use vectorcraft_geom::ParamCurve;
        let pts: Vec<Point> = [(0.0, 0.0), (1.0, 2.0), (3.0, 3.0), (4.0, 0.0), (6.0, -1.0), (7.0, 2.0)].map(|(x, y)| Point::new(x, y)).to_vec();
        for n in 4..=pts.len() {
            let p = &pts[..n];
            let mut bp = BezPath::new();
            bp.move_to(p[0]);
            bspline_to(&mut bp, p);
            let segs: Vec<_> = bp.segments().collect();
            assert_eq!(segs.len(), n - 3);
            for (i, seg) in segs.iter().enumerate() {
                for k in 0..=4 {
                    let local = f64::from(k) / 4.0;
                    let want = de_boor(p, i as f64 + local);
                    assert!(seg.eval(local).distance(want) < 1e-9, "n={n} seg {i} t={local}: {:?} vs {want:?}", seg.eval(local));
                }
            }
        }
        // ppdt: knot flags split the spline into a line, a quadratic and a B-spline.
        let mut rec = 7u16.to_le_bytes().to_vec();
        rec.extend_from_slice(&[0; 4]);
        for (x, y) in [(0, 0), (1, 0), (2, 1), (3, 0), (4, 1), (5, 0), (6, 1)] {
            rec.extend_from_slice(&le32(&[x * IN, y * IN]));
        }
        rec.extend_from_slice(&le32(&[1, 1, 0, 1, 0, 0, 0]));
        let sp = SplineData::decode(&rec, V9).unwrap();
        use vectorcraft_geom::PathEl::*;
        let kinds: Vec<_> = sp
            .to_bezpath()
            .elements()
            .iter()
            .map(|e| match e {
                MoveTo(_) => 'M',
                LineTo(_) => 'L',
                QuadTo(..) => 'Q',
                CurveTo(..) => 'C',
                ClosePath => 'Z',
            })
            .collect();
        assert_eq!(kinds.iter().collect::<String>(), "MLQC");
        assert!(SplineData::decode(&rec[..10], V9).unwrap().points.len() <= 1);
    }

    #[test]
    fn polygons_and_stars_from_one_corner() {
        use vectorcraft_geom::PathEl;
        let step = std::f64::consts::TAU / 5.0;
        // Corner k, clockwise from the top (y up), the way libcdr's turns go.
        let corner = |k: f64| Point::new((k * step).sin(), (k * step).cos());
        let edge = |to: f64| {
            let mut bp = BezPath::new();
            bp.move_to(Point::new(0.0, 1.0));
            bp.line_to(corner(to));
            bp
        };
        // A pentagon: five joined edges, one closed outline, every corner on the circle.
        let x = PolygonXf { angles: 5, next: 1, rx: 2.0, ry: 2.0, cx: 1.0, cy: 0.0 };
        let penta = x.create(&edge(1.0));
        let moves = penta.elements().iter().filter(|e| matches!(e, PathEl::MoveTo(_))).count();
        assert_eq!(moves, 1, "{penta:?}");
        assert!(matches!(penta.elements().last(), Some(PathEl::ClosePath)));
        let ends: Vec<Point> = penta
            .elements()
            .iter()
            .filter_map(|e| match e {
                PathEl::LineTo(p) => Some(*p),
                _ => None,
            })
            .collect();
        assert_eq!(ends.len(), 5);
        assert!(ends.iter().all(|p| (p.distance(Point::new(1.0, 0.0)) - 2.0).abs() < 1e-9));
        let area = vectorcraft_geom::Shape::area(&penta).abs();
        assert!((area - 4.0 * 2.5 * step.sin()).abs() < 1e-9, "{area}");
        // A five-pointed star: edges jump two corners at a time.
        let star = PolygonXf { angles: 5, next: 2, rx: 1.0, ry: 1.0, cx: 0.0, cy: 0.0 }.create(&edge(2.0));
        assert_eq!(star.elements().iter().filter(|e| matches!(e, PathEl::LineTo(_))).count(), 5);
        assert_eq!(star.elements().iter().filter(|e| matches!(e, PathEl::MoveTo(_))).count(), 1);
        // No angles: the corner as it is.
        assert_eq!(PolygonXf { angles: 0, ..x }.create(&edge(1.0)), edge(1.0));
    }

    #[test]
    fn polygons_splines_bitmaps_and_arrowheads_translate() {
        let bytes = crate::synth::more_shapes_riff();
        let riff = crate::riff::parse(&bytes, &crate::Limits::default()).unwrap();
        let t = translate(&riff, None);
        assert!(t.errors.is_empty() && t.skipped.is_empty(), "{:?} {:?}", t.errors, t.skipped);
        let shapes = t.pages[0].leaves();
        assert_eq!(shapes.len(), 3);
        // Bottom first: bitmap, spline, pentagon.
        let Content::Image { bitmap, xf } = &shapes[0].content else { panic!("{:?}", shapes[0].content) };
        assert_eq!((bitmap.width, bitmap.height), (2, 2));
        assert_eq!(bitmap.rgba[..8], [0, 0, 255, 255, 255, 255, 255, 255], "the top row");
        assert_eq!(*xf * Point::ZERO, Point::new(108.0, 72.0), "top left of the 1 in square at the centre");
        assert_eq!(*xf * Point::new(2.0, 2.0), Point::new(180.0, 144.0));

        let spline = shapes[1].path().unwrap();
        assert!(!spline.is_closed());
        assert_eq!(spline.anchor_count(), 2, "one cubic piece");
        let stroke = shapes[1].appearance.stroke().unwrap();
        assert_eq!((stroke.start_arrow, stroke.end_arrow), (None, Some(Arrowhead::Triangle)));
        assert_eq!(shapes[1].notes, ["arrowheads drawn as VectorCraft's nearest preset"]);

        let penta = shapes[2].path().unwrap();
        assert!(penta.is_closed());
        assert_eq!(penta.anchor_count(), 5);
        let b = penta.bounds().unwrap();
        // Radius 1 in round the page centre (144, 108): the top corner 72 pt above it.
        assert!((b.y0 - 36.0).abs() < 1e-3 && (b.center().x - 144.0).abs() < 1e-3, "{b:?}");
    }

    #[test]
    fn truncated_records_are_errors_not_panics() {
        let rec = fountain9(1, 0, 0, (0, 0), 50, &[([0; 4], 0), ([0; 4], 100)]);
        for n in 0..rec.len() {
            let _ = CorelFill::decode(&rec[..n], V9);
        }
        let o = outl9(1, 0x06, 0, 0, 1.0, &[1, 1]);
        for n in 0..o.len() {
            let _ = Outline::decode(&o[..n], V9);
        }
        let l = loda32(0x03, &[(0x1e, curve32(&[(0, 0, node::MOVE), (1, 1, node::LINE)]))]);
        for n in 0..l.len() {
            if let Ok(d) = decode_loda(&l[..n], V9)
                && let Some(off) = d.coords
            {
                let _ = CorelPath::decode(&l[..n], off, d.kind, V9);
            }
        }
        assert!(redirected(&[0; 16], CdrVersion(1600)) && !redirected(&[0; 16], CdrVersion(1500)));
    }
}
