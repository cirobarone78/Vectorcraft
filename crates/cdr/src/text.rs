//! CorelDRAW text records: fonts (`font`), the style table (`stlt`) and the text itself (`txsm`).
//!
//! Layouts follow libcdr's `CDRParser.cpp` (`readFont`, `readStlt`, `readTxsm5`, `readTxsm6` and
//! the version 7–15 body of `readTxsm`); runs are assembled as `CDRStylesCollector::collectText`
//! does, styles inherited as `CDRParserState::getRecursedStyle` does. Versions 16 and later (X6+)
//! keep text in `readTxsm16`'s layout with JSON styles ([`decode_txsm16`]); versions 5 and 6 keep
//! styles in `styd` records ([`decode_styd`]).
//!
//! Sizes and indents are in inches, as the geometry records' coordinates are. Eight-bit text is
//! decoded with the Windows code page its font's character set names (`encoding_rs`); text in a
//! symbol font (character set 2) maps byte `b` to U+F000 + `b`, the private-use range symbol fonts
//! map their glyphs to, so the font shows its own pictures.

use std::collections::BTreeMap;

use crate::Error;
use crate::geometry::CorelColor;
use crate::riff::CdrVersion;

/// Most styles, fonts, paragraphs or runs read from one record.
const MAX_LIST: usize = 1 << 16;
/// Most characters read from one paragraph.
const MAX_CHARS: usize = 1 << 20;
/// Deepest style inheritance followed (libcdr follows any depth, also round a cycle).
const MAX_STYLE_DEPTH: usize = 64;

/// The Windows character set ids CorelDRAW stores with fonts and runs.
pub mod charset {
    pub const ANSI: u16 = 0x00;
    pub const SYMBOL: u16 = 0x02;
    pub const SHIFT_JIS: u16 = 0x80;
    pub const HANGUL: u16 = 0x81;
    pub const GB2312: u16 = 0x86;
    pub const BIG5: u16 = 0x88;
    pub const GREEK: u16 = 0xa1;
    pub const TURKISH: u16 = 0xa2;
    pub const VIETNAMESE: u16 = 0xa3;
    pub const HEBREW: u16 = 0xb1;
    pub const ARABIC: u16 = 0xb2;
    pub const BALTIC: u16 = 0xba;
    pub const RUSSIAN: u16 = 0xcc;
    pub const THAI: u16 = 0xde;
    pub const EAST_EUROPE: u16 = 0xee;
}

/// Decode eight-bit text in character set `cs` (libcdr's `appendCharacters`; an unknown set is
/// Windows-1252, as there).
pub fn decode_8bit(bytes: &[u8], cs: u16) -> String {
    use encoding_rs as e;
    let enc = match cs {
        charset::SYMBOL => return bytes.iter().map(|&b| char::from_u32(0xF000 + u32::from(b)).unwrap_or(' ')).collect(),
        charset::SHIFT_JIS => e::SHIFT_JIS,
        charset::HANGUL => e::EUC_KR,
        charset::GB2312 => e::GBK,
        charset::BIG5 => e::BIG5,
        charset::GREEK => e::WINDOWS_1253,
        charset::TURKISH => e::WINDOWS_1254,
        charset::VIETNAMESE => e::WINDOWS_1258,
        charset::HEBREW => e::WINDOWS_1255,
        charset::ARABIC => e::WINDOWS_1256,
        charset::BALTIC => e::WINDOWS_1257,
        charset::RUSSIAN => e::WINDOWS_1251,
        charset::THAI => e::WINDOWS_874,
        charset::EAST_EUROPE => e::WINDOWS_1250,
        _ => e::WINDOWS_1252,
    };
    enc.decode_without_bom_handling(bytes).0.into_owned()
}

/// Decode UTF-16LE text, dropping what isn't a character (libcdr keeps only Unicode characters).
pub fn decode_utf16(bytes: &[u8]) -> String {
    let units = bytes.chunks_exact(2).map(|b| u16::from_le_bytes([b[0], b[1]]));
    char::decode_utf16(units).filter_map(Result::ok).collect()
}

struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
    what: &'static str,
    wide: bool,
}

impl<'a> Reader<'a> {
    fn new(data: &'a [u8], what: &'static str, version: CdrVersion) -> Self {
        Self { data, pos: 0, what, wide: version >= CdrVersion(600) }
    }
    fn short(&self) -> Error {
        Error::Record { record: self.what, message: format!("ends at byte {} of {}", self.pos, self.data.len()) }
    }
    fn take(&mut self, n: usize) -> Result<&'a [u8], Error> {
        let end = self.pos.checked_add(n).ok_or_else(|| self.short())?;
        let b = self.data.get(self.pos..end).ok_or_else(|| self.short())?;
        self.pos = end;
        Ok(b)
    }
    fn skip(&mut self, n: usize) -> Result<(), Error> {
        self.take(n).map(|_| ())
    }
    fn u8(&mut self) -> Result<u8, Error> {
        Ok(self.take(1)?[0])
    }
    fn u16(&mut self) -> Result<u16, Error> {
        let b = self.take(2)?;
        Ok(u16::from_le_bytes([b[0], b[1]]))
    }
    fn u32(&mut self) -> Result<u32, Error> {
        let b = self.take(4)?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }
    fn u64(&mut self) -> Result<u64, Error> {
        let b = self.take(8)?;
        let mut a = [0; 8];
        a.copy_from_slice(b);
        Ok(u64::from_le_bytes(a))
    }
    fn count(&mut self) -> Result<usize, Error> {
        Ok((self.u32()? as usize).min(MAX_LIST))
    }
    fn coord(&mut self) -> Result<f64, Error> {
        if self.wide { Ok(f64::from(self.u32()? as i32) / 254_000.0) } else { Ok(f64::from(self.u16()? as i16) / 1000.0) }
    }
    fn remaining(&self) -> usize {
        self.data.len().saturating_sub(self.pos)
    }
}

/// A `font` record's font.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Font {
    pub name: String,
    pub charset: u16,
}

/// Decode a `font` record: its id and font.
pub fn decode_font(data: &[u8], version: CdrVersion) -> Result<(u16, Font), Error> {
    let mut r = Reader::new(data, "font", version);
    let id = r.u16()?;
    let charset = r.u16()?;
    r.skip(14)?;
    let rest = r.data.get(r.pos..).unwrap_or_default();
    let name = if version >= CdrVersion(1200) {
        let len = rest.chunks_exact(2).position(|c| c == [0, 0]).unwrap_or(rest.len() / 2);
        decode_utf16(rest.get(..len * 2).unwrap_or_default())
    } else {
        let len = rest.iter().position(|&b| b == 0).unwrap_or(rest.len());
        decode_8bit(rest.get(..len).unwrap_or_default(), charset)
    };
    let (name, charset) = name_script(name, charset);
    Ok((id, Font { name, charset }))
}

/// A font name's script suffix (libcdr's `processNameForEncoding`), when no character set is set.
fn name_script(mut name: String, mut charset: u16) -> (String, u16) {
    if charset == 0 {
        // Windows names a font's other scripts with a suffix ("Arial CE", "Times New Roman Cyr").
        const SUFFIXES: [(&str, u16); 11] = [
            (" CE", charset::EAST_EUROPE),
            (" Cyrillic", charset::RUSSIAN),
            (" Cyr", charset::RUSSIAN),
            (" CYR", charset::RUSSIAN),
            (" Baltic", charset::BALTIC),
            (" Greek", charset::GREEK),
            (" Tur", charset::TURKISH),
            (" TUR", charset::TURKISH),
            (" Hebrew", charset::HEBREW),
            (" Arabic", charset::ARABIC),
            (" Thai", charset::THAI),
        ];
        if let Some((base, cs)) = SUFFIXES.iter().find_map(|(s, cs)| name.strip_suffix(s).filter(|b| !b.is_empty()).map(|b| (b.to_owned(), *cs))) {
            name = base;
            charset = cs;
        } else if name.starts_with("GOST") {
            charset = charset::RUSSIAN;
        }
    }
    (name, charset)
}

/// What fills characters.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Ink {
    /// A `fild` record.
    Record(u32),
    /// A colour (version 16+ styles give it directly).
    Color(CorelColor),
    /// Not filled.
    None,
}

/// What outlines characters.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Pen {
    /// An `outl` record.
    Record(u32),
    /// A line `width` inches wide (version 16+ styles).
    Line { width: f64, color: CorelColor },
}

/// Character and paragraph attributes; `None` where a style leaves them to its parent.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TextStyle {
    pub font: Option<String>,
    pub charset: Option<u16>,
    /// Font size in inches.
    pub size: Option<f64>,
    /// 0 none, 1 left, 2 centre, 3 right, 4 justify, 5 force justify.
    pub align: Option<u32>,
    pub fill: Option<Ink>,
    pub outline: Option<Pen>,
    /// Left, first-line and right indents in inches.
    pub indents: Option<(f64, f64, f64)>,
    /// Space added between characters, as a fraction of the width of a space.
    pub char_spacing: Option<f64>,
    /// Line spacing as a fraction of the default (1 = the font's own).
    pub line_spacing: Option<f64>,
    /// A character's own rotation, degrees counter-clockwise.
    pub rotation: Option<f64>,
    /// A character's vertical shift, ems (up).
    pub rise: Option<f64>,
    /// A character's horizontal shift, ems (right).
    pub shift: Option<f64>,
}

impl TextStyle {
    /// Take what `over` sets (libcdr's `CDRStyle::overrideStyle`: the font and its character
    /// set go together).
    pub fn apply(&mut self, over: &TextStyle) {
        if over.font.is_some() || over.charset.is_some() {
            self.font = over.font.clone().or_else(|| self.font.clone());
            self.charset = over.charset.or(self.charset);
        }
        fn set<T: Clone>(a: &mut Option<T>, b: &Option<T>) {
            if b.is_some() {
                a.clone_from(b);
            }
        }
        set(&mut self.size, &over.size);
        set(&mut self.align, &over.align);
        set(&mut self.fill, &over.fill);
        set(&mut self.outline, &over.outline);
        set(&mut self.indents, &over.indents);
        set(&mut self.char_spacing, &over.char_spacing);
        set(&mut self.line_spacing, &over.line_spacing);
        set(&mut self.rotation, &over.rotation);
        set(&mut self.rise, &over.rise);
        set(&mut self.shift, &over.shift);
    }
}

/// The style table: each style and its parent.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Styles {
    pub styles: BTreeMap<u32, (u32, TextStyle)>,
}

impl Styles {
    /// Style `id` with what it inherits: its ancestors first, then itself.
    pub fn resolve(&self, id: u32) -> TextStyle {
        let mut chain = Vec::new();
        let mut at = id;
        while let Some((parent, s)) = self.styles.get(&at) {
            chain.push(s);
            if *parent == 0 || chain.len() >= MAX_STYLE_DEPTH {
                break;
            }
            at = *parent;
        }
        let mut out = TextStyle::default();
        for s in chain.into_iter().rev() {
            out.apply(s);
        }
        out
    }

    /// Decode an `stlt` record (version 7 and later), with `fonts` from the `font` records.
    pub fn decode(data: &[u8], version: CdrVersion, fonts: &BTreeMap<u16, Font>) -> Result<Self, Error> {
        let mut r = Reader::new(data, "stlt", version);
        let x3 = version >= CdrVersion(1300);
        let records = r.count()?;
        if records == 0 {
            return Ok(Self::default());
        }
        let id_map = |r: &mut Reader<'_>, extra: usize| -> Result<BTreeMap<u32, u32>, Error> {
            let n = r.count()?;
            let mut m = BTreeMap::new();
            for _ in 0..n {
                let id = r.u32()?;
                r.skip(4)?;
                m.insert(id, r.u32()?);
                r.skip(extra)?;
            }
            Ok(m)
        };
        let fills = id_map(&mut r, if x3 { 48 } else { 0 })?;
        let outlines = id_map(&mut r, 0)?;
        let pad = if version < CdrVersion(1000) { 12 } else { 20 };
        let mut font_recs = BTreeMap::new();
        for _ in 0..r.count()? {
            let id = r.u32()?;
            r.skip(pad)?;
            let (font, cs) = (r.u16()?, r.u16()?);
            r.skip(8)?;
            let size = r.coord()?;
            r.skip(pad)?;
            font_recs.insert(id, (font, cs, size));
        }
        let aligns = id_map(&mut r, 0)?;
        let skip_n = |r: &mut Reader<'_>, size: usize| -> Result<(), Error> {
            let n = r.count()?;
            r.skip(n.saturating_mul(size))
        };
        skip_n(&mut r, 52)?; // intervals
        skip_n(&mut r, 152)?; // set 5s
        skip_n(&mut r, 784)?; // tabs
        let bullets = r.count()?;
        for _ in 0..bullets {
            if r.remaining() < 16 {
                break;
            }
            r.skip(40)?;
            if version > CdrVersion(1300) {
                r.skip(4)?;
            }
            if x3 {
                let more = r.u32()? != 0;
                r.skip(if more { 68 } else { 12 })?;
            } else {
                r.skip(20)?;
                if version >= CdrVersion(1000) {
                    r.skip(8)?;
                }
                if r.u32()? != 0 {
                    r.skip(8)?;
                }
                r.skip(8)?;
            }
        }
        let mut indents = BTreeMap::new();
        for _ in 0..r.count()? {
            let id = r.u32()?;
            r.skip(12)?;
            let (right, first, left) = (r.coord()?, r.coord()?, r.coord()?);
            indents.insert(id, (left, first, right));
        }
        skip_n(&mut r, if x3 { 36 } else { 32 })?; // hyphenation
        skip_n(&mut r, 28)?; // drop caps
        let set11 = version > CdrVersion(800);
        if set11 {
            skip_n(&mut r, 12)?;
        }
        let mut out = Self::default();
        for _ in 0..records {
            if r.remaining() < 32 {
                break;
            }
            let num = r.u32()?;
            let id = r.u32()?;
            let parent = r.u32()?;
            r.skip(8)?;
            let name_len = r.u32()? as usize;
            r.skip(if version >= CdrVersion(1200) { name_len.saturating_mul(2) } else { name_len })?;
            let (fill, outline) = (r.u32()?, r.u32()?);
            let (mut font_rec, mut align, mut indent) = (0, 0, 0);
            if num > 1 {
                font_rec = r.u32()?;
                align = r.u32()?;
                r.skip(8)?; // interval, set 5
                if set11 {
                    r.skip(4)?;
                }
            }
            if num > 2 {
                r.skip(8)?; // tab, bullet
                indent = r.u32()?;
                r.skip(8)?; // hyphenation, drop cap
            }
            let mut s = TextStyle::default();
            if let Some(&(font, cs, size)) = font_recs.get(&font_rec).filter(|_| font_rec != 0) {
                if let Some(f) = fonts.get(&font) {
                    s.font = Some(f.name.clone());
                    s.charset = Some(f.charset);
                }
                if cs != 0 {
                    s.charset = Some(cs);
                }
                s.size = Some(size).filter(|v| v.abs() > 1e-9);
            }
            s.align = aligns.get(&align).filter(|_| align != 0).copied();
            s.indents = indents.get(&indent).filter(|_| indent != 0).copied();
            s.fill = fills.get(&fill).filter(|_| fill != 0).map(|&id| Ink::Record(id));
            s.outline = outlines.get(&outline).filter(|_| outline != 0).map(|&id| Pen::Record(id));
            out.styles.insert(id, (parent, s));
        }
        Ok(out)
    }
}

/// A run's characters: two-byte (UTF-16) ones are decoded as read; eight-bit ones need the
/// character set of the run's resolved style ([`Run::decoded`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Chars {
    Unicode(String),
    Bytes(Vec<u8>),
}

/// A run of characters in one style.
#[derive(Debug, Clone, PartialEq)]
pub struct Run {
    pub chars: Chars,
    /// What the run sets over its paragraph's style.
    pub style: TextStyle,
}

impl Run {
    /// The run's text, eight-bit characters decoded in the character set of `resolved`, the run's
    /// style over its paragraph's.
    pub fn decoded(&self, resolved: &TextStyle) -> String {
        match &self.chars {
            Chars::Unicode(s) => s.clone(),
            Chars::Bytes(b) => decode_8bit(b, resolved.charset.unwrap_or(charset::ANSI)),
        }
    }
}

/// A paragraph of a text object.
#[derive(Debug, Clone, PartialEq)]
pub struct Paragraph {
    /// Its `stlt` style.
    pub style: u32,
    pub runs: Vec<Run>,
}

/// Decode a `txsm` record (version 5 and later): the text id (an object's `spnd`) and its
/// paragraphs. `fonts` resolves the font ids runs name (version 16+ styles name fonts themselves).
pub fn decode_txsm(data: &[u8], version: CdrVersion, fonts: &BTreeMap<u16, Font>) -> Result<(u32, Vec<Paragraph>), Error> {
    if version < CdrVersion(500) {
        return Err(Error::Record { record: "txsm", message: format!("version {} text is not read", version.major()) });
    }
    if version >= CdrVersion(1600) {
        return decode_txsm16(data, version);
    }
    let mut r = Reader::new(data, "txsm", version);
    if version < CdrVersion(600) {
        return txsm5(&mut r, fonts);
    }
    if version < CdrVersion(700) {
        return txsm6(&mut r, fonts);
    }
    let v = |n: u32| version >= CdrVersion(n);
    let frame_flag = r.u32()?;
    r.skip(0x20)?;
    if v(1500) {
        r.skip(1)?;
    }
    if !v(701) && r.u32()? == 1 {
        r.skip(32)?; // text on a path
    }
    let mut text_id = 0;
    for _ in 0..r.count()? {
        text_id = r.u32()?;
        r.skip(48)?;
        if v(701) {
            if r.u32()? == 1 {
                // var1, [orientation, var2,] var3, offset, var4, distance, var5, mirror v/h, [var6, var7]
                r.skip(4 + if v(1201) { 8 } else { 0 } + 28 + if v(1500) { 8 } else { 0 })?;
            } else if v(1500) {
                r.skip(8)?;
            }
        }
        if frame_flag == 0 {
            r.skip(if v(1500) {
                40
            } else if v(1400) {
                36
            } else if v(801) {
                34
            } else if v(800) {
                32
            } else {
                36
            })?;
        } else if v(1500) {
            r.skip(4)?;
        }
    }
    let mut paras = Vec::new();
    for _ in 0..r.count()? {
        let style = r.u32()?;
        r.skip(1)?;
        // Artistic text whose characters were moved one by one (rotated or shifted with the Shape
        // tool) carries a 64-byte block here: a count, a value, a transform and 8 more bytes.
        // Read from a user's version 14 file; libcdr skips only the flag.
        if v(1201) && frame_flag != 0 && r.u8()? != 0 {
            r.skip(64)?;
        }
        let mut overrides = BTreeMap::new();
        for i in 0..r.count()? {
            r.skip(2)?;
            let fl2 = r.u8()?;
            let fl3 = if v(800) { r.u8()? } else { 0 };
            let mut s = TextStyle::default();
            if fl2 & 0x01 != 0 {
                font_override(&mut s, r.u16()?, r.u16()?, fonts);
            }
            if fl2 & 0x02 != 0 {
                r.skip(4)?; // weight and slant (not decoded by libcdr either)
            }
            if fl2 & 0x04 != 0 {
                s.size = Some(r.coord()?);
            }
            // A character's rotation (millionths of a degree) and its horizontal and vertical
            // shift (percent of the size). Measured against a user's version 14 file and its PDF.
            if fl2 & 0x08 != 0 {
                s.rotation = Some(f64::from(r.u32()? as i32) / 1e6).filter(|a| a.is_finite() && *a != 0.0);
            }
            if fl2 & 0x10 != 0 {
                s.shift = Some(f64::from(r.u32()? as i32) / 100.0).filter(|x| *x != 0.0);
            }
            if fl2 & 0x20 != 0 {
                s.rise = Some(f64::from(r.u32()? as i32) / 100.0).filter(|y| *y != 0.0);
            }
            if fl2 & 0x40 != 0 {
                s.fill = Some(Ink::Record(r.u32()?));
                if v(1300) {
                    r.skip(48)?;
                }
            }
            if fl2 & 0x80 != 0 {
                s.outline = Some(Pen::Record(r.u32()?));
            }
            if fl3 & 0x08 != 0 {
                if v(1300) {
                    let n = r.u32()? as usize;
                    r.skip(n.saturating_mul(2))?;
                } else {
                    r.skip(4)?;
                }
            }
            if fl3 & 0x20 != 0 && r.data.get(r.pos).is_some_and(|&b| b != 0) {
                r.skip(4 + if v(1500) { 48 } else { 0 })?;
            }
            overrides.insert(2 * i as u32, s);
        }
        let char_size = if v(1200) { 8 } else { 4 };
        let n = (r.u32()? as usize).min(r.remaining() / char_size).min(MAX_CHARS);
        let mut descs = Vec::with_capacity(n);
        for _ in 0..n {
            let d = if v(1200) { (r.u64()? & 0xffff_ffff) as u32 } else { r.u32()? };
            descs.push(((d >> 16) | (d & 1)) as u8);
        }
        let bytes = if v(1200) { r.u32()? as usize } else { n };
        let text = r.take(bytes)?;
        r.skip(1)?;
        paras.push(Paragraph { style, runs: runs(text, &descs, &overrides) });
    }
    Ok((text_id, paras))
}

/// Most bytes of one version 16+ JSON style read.
const MAX_JSON: usize = 1 << 20;

/// A version 16+ colour string (libcdr's `parseColourString`): `MODEL,PALETTE,v1,…,UUID`, maybe
/// followed by `,~,NAME,N,FALLBACK,~,…`. A fallback colour, when there is one, is used instead
/// (it is the process colour a palette colour stands for).
pub fn parse_color(s: &str) -> Option<CorelColor> {
    parse_color_depth(s, 4)
}

fn parse_color_depth(s: &str, depth: u8) -> Option<CorelColor> {
    let parts: Vec<&str> = s.split(',').map(str::trim).collect();
    let is_uuid = |t: &str| t.len() == 36 && t.chars().filter(|&c| c == '-').count() == 4 && t.chars().all(|c| c == '-' || c.is_ascii_hexdigit());
    let uuid = parts.iter().position(|t| is_uuid(t))?;
    if depth > 0 && parts.get(uuid + 1) == Some(&"~") {
        // ~, name, count, then fallback colours separated by ",~,".
        let rest = parts.get(uuid + 4..).unwrap_or_default().join(",");
        if let Some(c) = rest
            .split(",~,")
            .map(|f| f.trim_end_matches(",~").trim_end_matches(','))
            .find(|f| !f.is_empty())
            .and_then(|f| parse_color_depth(f, depth - 1))
        {
            return Some(c);
        }
    }
    let model = match *parts.first()? {
        "CMYK" => 2,
        "CMYK255" => 3,
        "RGB255" => 5,
        "HSB" => 6,
        "HLS" => 7,
        "GRAY255" => 9,
        "YIQ255" => 11,
        "LAB" => 12,
        "PANTONEHX" => 14,
        "LAB255" => 18,
        "REGCOLOR" => 20,
        "SPOT" => 25,
        _ => 0,
    };
    let palette = match *parts.get(1)? {
        "USER" => 5,
        "FOCTONE" => 8,
        "DUPONT" => 9,
        "TOYO" => 10,
        "DIC" => 11,
        "PAN_HEX" => 12,
        "HKS" => 0x17,
        "HKS_K" => 0x1a,
        "HKS_N" => 0x1b,
        "HKS_Z" => 0x1c,
        "HKS_E" => 0x1d,
        _ => 0,
    };
    let v: Vec<u32> = parts.get(2..uuid)?.iter().map(|t| t.parse().ok()).collect::<Option<_>>()?;
    let b = |i: usize| v.get(i).copied().unwrap_or(0);
    let value = match v.len() {
        5.. => b(0) | b(1) << 8 | b(2) << 16 | b(3) << 24,
        4 => match model {
            5 => b(2) | b(1) << 8 | b(0) << 16,
            6 | 7 => b(0) | b(1) << 16 | b(2) << 24,
            _ => b(0) | b(1) << 8 | b(2) << 16,
        },
        2 | 3 if matches!(model, 25 | 14) => b(1) << 16 | b(0),
        1.. => b(0),
        0 => return None,
    };
    Some(CorelColor { model, palette, value })
}

/// A JSON style value: CorelDRAW X7 writes numbers as strings (`"size": "17300"`), the synthetic
/// files as numbers; either is read.
fn num(v: Option<&serde_json::Value>) -> Option<f64> {
    let v = v?;
    let n = v.as_f64().or_else(|| v.as_str().and_then(|s| s.trim().parse::<f64>().ok()))?;
    n.is_finite().then_some(n)
}

/// A non-negative whole JSON style value (see [`num`]).
fn whole<T: TryFrom<u64>>(v: Option<&serde_json::Value>) -> Option<T> {
    num(v).filter(|n| *n >= 0.0 && n.fract() == 0.0 && *n <= u64::MAX as f64).and_then(|n| T::try_from(n as u64).ok())
}

/// A version 16+ JSON style (libcdr's `_readX6StyleString`): font, character set, size, fill,
/// outline and justification, and the paragraph's character and line spacing (millionths:
/// `interCharSpacing` of a space's width, `interLineSpacing` of the default). Anything unreadable
/// is left unset.
pub fn style_from_json(json: &str) -> TextStyle {
    let mut s = TextStyle::default();
    let Ok(v) = serde_json::from_str::<serde_json::Value>(json) else { return s };
    if let Some(c) = v.get("character") {
        let latin = c.get("latin");
        let font = latin.and_then(|l| l.get("font")).and_then(|f| f.as_str());
        let cs: Option<u16> = whole(latin.and_then(|l| l.get("charset")));
        if let Some(f) = font {
            let (name, charset) = name_script(f.to_owned(), cs.unwrap_or(0));
            s.font = Some(name);
            s.charset = Some(charset);
        } else if let Some(cs) = cs.filter(|&c| c != 0) {
            s.charset = Some(cs);
        }
        s.size = whole::<u64>(latin.and_then(|l| l.get("size"))).map(|v| v as f64 / 254_000.0);
        if let Some(o) = c.get("outline") {
            let width = whole::<u64>(o.get("width")).map_or(0.0, |w| w as f64 / 254_000.0);
            if let Some(color) = o.get("color").and_then(|c| c.as_str()).and_then(parse_color) {
                s.outline = Some(Pen::Line { width, color });
            }
        }
        if let Some(f) = c.get("fill") {
            let kind: Option<u64> = whole(f.get("type"));
            let color = f.get("primaryColor").and_then(|c| c.as_str()).and_then(parse_color);
            s.fill = match (kind, color) {
                (Some(0), _) => Some(Ink::None),
                (_, Some(c)) => Some(Ink::Color(c)),
                _ => None,
            };
        }
    }
    if let Some(p) = v.get("paragraph") {
        s.align = whole(p.get("justify"));
        s.char_spacing = num(p.get("interCharSpacing")).map(|v| (v / 1e6).clamp(-1.0, 20.0));
        s.line_spacing = num(p.get("interLineSpacing")).map(|v| v / 1e6).filter(|v| *v > 0.0).map(|v| v.min(20.0));
    }
    s
}

/// A version 16+ style string of `len` units: UTF-16 before version 17, UTF-8 from it on.
fn json_at(r: &mut Reader<'_>, len: usize, version: CdrVersion) -> Result<TextStyle, Error> {
    let utf8 = version >= CdrVersion(1700);
    let bytes = if utf8 { len } else { len.saturating_mul(2) };
    let bytes = bytes.min(r.remaining()).min(MAX_JSON);
    let raw = r.take(if utf8 { bytes } else { bytes & !1 })?;
    let text = if utf8 { String::from_utf8_lossy(raw).into_owned() } else { decode_utf16(raw) };
    Ok(style_from_json(&text))
}

/// Decode a version 16+ `txsm` record (libcdr's `readTxsm16`): the text id and its paragraphs,
/// each run's style its paragraph's JSON style with the run's own over it.
pub fn decode_txsm16(data: &[u8], version: CdrVersion) -> Result<(u32, Vec<Paragraph>), Error> {
    let mut r = Reader::new(data, "txsm", version);
    let frame_flag = r.u32()?;
    r.skip(37)?;
    let mut text_id = 0;
    for _ in 0..r.count()? {
        text_id = r.u32()?;
        r.skip(48)?;
        if r.u32()? == 1 {
            r.skip(48)?; // text on a path
        } else {
            r.skip(8)?;
        }
        if frame_flag == 0 {
            r.skip(16)?;
            let n = r.u32()? as usize;
            r.skip(if version > CdrVersion(1600) { n } else { n.saturating_mul(2) })?;
        }
    }
    let mut paras = Vec::new();
    for _ in 0..r.count()? {
        let style = r.u32()?;
        r.skip(if frame_flag != 0 { 2 } else { 1 })?;
        let n = r.u32()? as usize;
        let default = json_at(&mut r, n, version)?;
        let mut overrides = BTreeMap::new();
        let records = r.count()?;
        for i in 0..records {
            if r.remaining() < 17 {
                break;
            }
            let mut s = default.clone();
            r.skip(2)?;
            let (f1, f2) = (r.u16()?, r.u16()?);
            if f2 & 0x04 != 0 {
                let n = r.u32()? as usize;
                r.skip(n.saturating_mul(2))?; // encoding name
            }
            if f1 != 0 || f2 & 0x04 != 0 {
                let n = r.u32()? as usize;
                s.apply(&json_at(&mut r, n, version)?);
            }
            overrides.insert(2 * i as u32, s);
        }
        let n = (r.u32()? as usize).min(r.remaining() / 8).min(MAX_CHARS);
        let mut descs = Vec::with_capacity(n);
        for _ in 0..n {
            let d = (r.u64()? & 0xffff_ffff) as u32;
            descs.push(((d >> 16) | (d & 1)) as u8);
        }
        let bytes = r.u32()? as usize;
        let text = r.take(bytes)?;
        r.skip(1)?;
        paras.push(Paragraph { style, runs: runs(text, &descs, &overrides) });
    }
    Ok((text_id, paras))
}

/// Decode a version 5–6 `styd` record (libcdr's `readStyd`): the style id, its parent and its
/// font, size, alignment, fill and outline.
pub fn decode_styd(data: &[u8], version: CdrVersion, fonts: &BTreeMap<u16, Font>) -> Result<(u32, u32, TextStyle), Error> {
    let mut r = Reader::new(data, "styd", version);
    let id = u32::from(r.u16()?);
    let start = r.pos;
    let unsigned = |r: &mut Reader<'_>| if r.wide { r.u32() } else { r.u16().map(u32::from) };
    let _length = unsigned(&mut r)?;
    let count = (unsigned(&mut r)? as usize).min(MAX_LIST);
    let (args_at, types_at) = (unsigned(&mut r)? as usize, unsigned(&mut r)? as usize);
    let parent = unsigned(&mut r)?;
    let seek = |r: &mut Reader<'_>, at: usize| -> Result<(), Error> {
        let to = start.saturating_add(at);
        if to > r.data.len() {
            return Err(r.short());
        }
        r.pos = to;
        Ok(())
    };
    seek(&mut r, args_at)?;
    let mut offsets = Vec::with_capacity(count);
    for _ in 0..count {
        offsets.push(unsigned(&mut r)? as usize);
    }
    seek(&mut r, types_at)?;
    let mut types = Vec::with_capacity(count);
    for _ in 0..count {
        types.push(unsigned(&mut r)?);
    }
    types.reverse();
    let mut s = TextStyle::default();
    for (&at, &ty) in offsets.iter().zip(&types) {
        seek(&mut r, at)?;
        match ty {
            0xcd => s.fill = Some(Ink::Record(r.u32()?)),
            0xd2 => s.outline = Some(Pen::Record(r.u32()?)),
            0xdc => {
                if r.wide {
                    r.skip(4)?;
                }
                let short = |r: &mut Reader<'_>| if r.wide { r.u16() } else { r.u8().map(u16::from) };
                let (font, cs) = (short(&mut r)?, short(&mut r)?);
                font_override(&mut s, font, cs, fonts);
                if r.wide {
                    r.skip(8)?;
                }
                s.size = Some(r.coord()?).filter(|v| v.abs() > 1e-9);
            }
            0xe1 => s.align = Some(unsigned(&mut r)?),
            _ => {}
        }
    }
    Ok((id, parent, s))
}

fn font_override(s: &mut TextStyle, font: u16, cs: u16, fonts: &BTreeMap<u16, Font>) {
    if let Some(f) = fonts.get(&font) {
        s.font = Some(f.name.clone());
        s.charset = Some(f.charset);
    }
    if cs != 0 {
        s.charset = Some(cs);
    }
}

fn txsm6(r: &mut Reader<'_>, fonts: &BTreeMap<u16, Font>) -> Result<(u32, Vec<Paragraph>), Error> {
    let frame_flag = r.u32()?;
    r.skip(0x18)?;
    if r.u32()? == 1 {
        r.skip(32)?;
    }
    let mut text_id = 0;
    for _ in 0..r.count()? {
        text_id = r.u32()?;
        r.skip(48 + if frame_flag == 0 { 8 } else { 0 })?;
    }
    let mut paras = Vec::new();
    for _ in 0..r.count()? {
        let style = r.u32()?;
        let mut overrides = BTreeMap::new();
        for i in 0..r.count()? {
            let mut s = TextStyle::default();
            let flag = r.u8()?;
            r.skip(3)?;
            if flag & 0x01 != 0 {
                font_override(&mut s, r.u16()?, r.u16()?, fonts);
            } else {
                r.skip(4)?;
            }
            r.skip(4)?;
            if flag & 0x04 != 0 {
                s.size = Some(r.coord()?);
            } else {
                r.skip(4)?;
            }
            r.skip(44)?;
            if flag & 0x10 != 0 {
                s.fill = Some(Ink::Record(r.u32()?));
            }
            if flag & 0x20 != 0 {
                s.outline = Some(Pen::Record(r.u32()?));
            }
            overrides.insert(2 * i as u32, s);
        }
        let n = r.u32()? as usize;
        r.skip(4)?;
        let n = n.min(r.remaining() / 12).min(MAX_CHARS);
        let (mut text, mut descs) = (Vec::with_capacity(n), Vec::with_capacity(n));
        for _ in 0..n {
            text.push(r.u8()?);
            r.skip(5)?;
            descs.push(r.u8()? << 1);
            r.skip(5)?;
        }
        paras.push(Paragraph { style, runs: runs(&text, &descs, &overrides) });
    }
    Ok((text_id, paras))
}

fn txsm5(r: &mut Reader<'_>, fonts: &BTreeMap<u16, Font>) -> Result<(u32, Vec<Paragraph>), Error> {
    r.skip(2)?;
    let mut text_id = 0;
    for _ in 0..usize::from(r.u16()?) {
        text_id = u32::from(r.u16()?);
        r.skip(2)?;
    }
    let mut paras = Vec::new();
    for _ in 0..usize::from(r.u16()?) {
        let style = u32::from(r.u16()?);
        let mut overrides = BTreeMap::new();
        for i in 0..usize::from(r.u16()?) {
            let mut s = TextStyle::default();
            let flag = r.u8()?;
            r.skip(1)?;
            if flag & 0x01 != 0 {
                let (font, cs) = (r.u8()?, r.u8()?);
                font_override(&mut s, u16::from(font), u16::from(cs), fonts);
            } else {
                r.skip(2)?;
            }
            r.skip(6)?;
            if flag & 0x04 != 0 {
                s.size = Some(r.coord()?);
            } else {
                r.skip(2)?;
            }
            r.skip(2)?;
            let fill = r.u32()?;
            let outline = r.u32()?;
            s.fill = Some(Ink::Record(fill)).filter(|_| flag & 0x10 != 0);
            s.outline = Some(Pen::Record(outline)).filter(|_| flag & 0x20 != 0);
            r.skip(14)?;
            overrides.insert(2 * i as u32, s);
        }
        let n = usize::from(r.u16()?).min(r.remaining() / 8);
        let (mut text, mut descs) = (Vec::with_capacity(n), Vec::with_capacity(n));
        for _ in 0..n {
            r.skip(4)?;
            text.push(r.u8()?);
            r.skip(1)?;
            descs.push(((r.u16()? >> 3) & 0xff) as u8);
        }
        paras.push(Paragraph { style, runs: runs(&text, &descs, &overrides) });
    }
    Ok((text_id, paras))
}

/// Split a paragraph's bytes into runs where the character descriptor changes. A descriptor's
/// low bit says the character is two bytes of UTF-16; the rest names the run's style override.
fn runs(text: &[u8], descs: &[u8], overrides: &BTreeMap<u32, TextStyle>) -> Vec<Run> {
    let mut out: Vec<(u8, Vec<u8>)> = Vec::new();
    let mut at = 0;
    for &d in descs {
        let width = if d & 1 != 0 { 2 } else { 1 };
        let Some(bytes) = text.get(at..at + width) else { break };
        at += width;
        match out.last_mut() {
            Some((last, buf)) if *last == d => buf.extend_from_slice(bytes),
            _ => out.push((d, bytes.to_vec())),
        }
    }
    out.into_iter()
        .map(|(d, bytes)| Run {
            chars: if d & 1 != 0 { Chars::Unicode(decode_utf16(&bytes)) } else { Chars::Bytes(bytes) },
            style: overrides.get(&u32::from(d & 0xfe)).cloned().unwrap_or_default(),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::synth;

    const V9: CdrVersion = CdrVersion(900);

    fn le(v: &[u32]) -> Vec<u8> {
        v.iter().flat_map(|x| x.to_le_bytes()).collect()
    }

    fn arial() -> BTreeMap<u16, Font> {
        BTreeMap::from([(1, Font { name: "Arial".into(), charset: 0 })])
    }

    #[test]
    fn eight_bit_text_follows_the_character_set() {
        assert_eq!(decode_8bit(&[0xF6], charset::ANSI), "ö");
        assert_eq!(decode_8bit(&[0xC0], charset::RUSSIAN), "А");
        assert_eq!(decode_8bit(&[0x8A], charset::EAST_EUROPE), "Š");
        assert_eq!(decode_8bit(&[0x82, 0xA0], charset::SHIFT_JIS), "あ");
        assert_eq!(decode_8bit(&[0x41], charset::SYMBOL), "\u{F041}");
        assert_eq!(decode_8bit(&[0x80], 0x77), "€", "unknown sets read as Windows-1252");
        assert_eq!(decode_utf16(&[0x41, 0, 0x00, 0xD8, 0x42, 0]), "AB", "a lone surrogate is dropped");
    }

    #[test]
    fn font_names_carry_their_script() {
        let (id, f) = decode_font(&synth::font(3, 0, "Arial CE"), V9).unwrap();
        assert_eq!((id, f.name.as_str(), f.charset), (3, "Arial", charset::EAST_EUROPE));
        assert_eq!(decode_font(&synth::font(4, 0, "GOST type A"), V9).unwrap().1.charset, charset::RUSSIAN);
        assert_eq!(decode_font(&synth::font(5, charset::GREEK, "Arial Tur"), V9).unwrap().1.name, "Arial Tur", "a set charset keeps the name");
        // Version 12+: UTF-16 names.
        let mut v = 6u16.to_le_bytes().to_vec();
        v.extend_from_slice(&[0; 16]);
        v.extend("Ω font".encode_utf16().flat_map(|u| u.to_le_bytes()));
        v.extend_from_slice(&[0, 0]);
        assert_eq!(decode_font(&v, CdrVersion(1200)).unwrap().1.name, "Ω font");
        assert!(decode_font(&[1, 0], V9).is_err());
    }

    #[test]
    fn styles_inherit_from_their_parents() {
        let s = Styles::decode(&synth::stlt(0.5, 2, 9), V9, &arial()).unwrap();
        let parent = s.resolve(10);
        assert_eq!((parent.font.as_deref(), parent.size, parent.align, parent.fill), (Some("Arial"), Some(0.5), Some(2), Some(Ink::Record(9))));
        let child = s.resolve(11);
        assert_eq!((child.font.as_deref(), child.size, child.align, child.fill), (Some("Arial"), Some(0.5), Some(1), Some(Ink::Record(9))));
        assert_eq!(s.resolve(99), TextStyle::default());
        // A cycle ends.
        let mut c = Styles::default();
        c.styles.insert(1, (2, TextStyle { size: Some(1.0), ..TextStyle::default() }));
        c.styles.insert(2, (1, TextStyle { align: Some(3), ..TextStyle::default() }));
        assert_eq!(c.resolve(1).size, Some(1.0));
        assert_eq!(Styles::decode(&le(&[0]), V9, &arial()).unwrap(), Styles::default());
    }

    #[test]
    fn version_9_text_runs_and_overrides() {
        let (id, paras) = decode_txsm(&synth::txsm(7, 10, &[("Hi ", None), ("wörld", Some(0.25))]), V9, &arial()).unwrap();
        assert_eq!((id, paras.len(), paras[0].style), (7, 1, 10));
        let runs = &paras[0].runs;
        assert_eq!(runs.len(), 2);
        assert_eq!(runs[0].decoded(&TextStyle::default()), "Hi ");
        assert_eq!((runs[1].decoded(&TextStyle::default()), runs[1].style.size), ("wörld".to_string(), Some(0.25)));
        // The same bytes in a Cyrillic style read as Cyrillic.
        let cyr = TextStyle { charset: Some(charset::RUSSIAN), ..TextStyle::default() };
        assert_eq!(runs[1].decoded(&cyr), "w\u{0446}rld");
    }

    #[test]
    fn version_12_text_is_utf16() {
        let text = "Ω×A";
        let n = u32::try_from(text.encode_utf16().count()).unwrap();
        let mut v = le(&[1]);
        v.extend_from_slice(&[0; 0x20]);
        v.extend_from_slice(&le(&[1, 42]));
        v.extend_from_slice(&[0; 48]);
        v.extend_from_slice(&le(&[0, 1, 5]));
        v.push(0);
        v.extend_from_slice(&le(&[1]));
        v.extend_from_slice(&[0, 0, 0, 0]);
        v.extend_from_slice(&le(&[n]));
        for _ in 0..n {
            v.extend_from_slice(&1u64.to_le_bytes());
        }
        v.extend_from_slice(&le(&[2 * n]));
        v.extend(text.encode_utf16().flat_map(|u| u.to_le_bytes()));
        v.push(0);
        let (id, paras) = decode_txsm(&v, CdrVersion(1200), &arial()).unwrap();
        assert_eq!((id, paras[0].style), (42, 5));
        assert_eq!(paras[0].runs[0].chars, Chars::Unicode(text.into()));
    }

    #[test]
    fn versions_5_and_6_text() {
        // Version 6: a 12-byte record a character.
        let mut v6 = le(&[1]);
        v6.extend_from_slice(&[0; 0x18]);
        v6.extend_from_slice(&le(&[0, 1, 3]));
        v6.extend_from_slice(&[0; 48]);
        v6.extend_from_slice(&le(&[1, 20, 1]));
        v6.extend_from_slice(&[0x04, 0, 0, 0]);
        v6.extend_from_slice(&[0; 8]);
        v6.extend_from_slice(&127_000i32.to_le_bytes());
        v6.extend_from_slice(&[0; 44]);
        v6.extend_from_slice(&le(&[2, 0]));
        for c in b"ok" {
            v6.push(*c);
            v6.extend_from_slice(&[0; 11]);
        }
        let (id, p) = decode_txsm(&v6, CdrVersion(600), &arial()).unwrap();
        assert_eq!((id, p[0].style, p[0].runs[0].style.size), (3, 20, Some(0.5)));
        assert_eq!(p[0].runs[0].decoded(&TextStyle::default()), "ok");
        // Version 5: 16-bit counts and 8 bytes a character.
        let mut v5 = vec![0, 0, 1, 0, 4, 0, 0, 0, 1, 0, 21, 0, 1, 0];
        v5.extend_from_slice(&[0x04, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
        v5.extend_from_slice(&500i16.to_le_bytes());
        v5.extend_from_slice(&[0; 2 + 8 + 14]);
        v5.extend_from_slice(&2u16.to_le_bytes());
        for c in b"go" {
            v5.extend_from_slice(&[0; 4]);
            v5.push(*c);
            v5.extend_from_slice(&[0; 3]);
        }
        let (id, p) = decode_txsm(&v5, CdrVersion(500), &arial()).unwrap();
        assert_eq!((id, p[0].style, p[0].runs[0].style.size), (4, 21, Some(0.5)));
        assert_eq!(p[0].runs[0].decoded(&TextStyle::default()), "go");
    }

    #[test]
    fn colour_strings() {
        let u = "1a2b3c4d-0000-1111-2222-333344445555";
        assert_eq!(parse_color(&format!("RGB255,USER,255,128,0,100,{u}")), Some(CorelColor { model: 5, palette: 5, value: 0x00ff8000 }));
        assert_eq!(
            parse_color(&format!("CMYK,USER,0,100,100,0,100,{u}")),
            Some(CorelColor { model: 2, palette: 5, value: u32::from_le_bytes([0, 100, 100, 0]) })
        );
        assert_eq!(parse_color(&format!("GRAY255,USER,200,100,{u}")).map(|c| (c.model, c.value)), Some((9, 200)));
        // A palette colour with a process fallback reads as the fallback.
        let spot = format!("SPOT,PAN_HEX,12,100,{u},~,PANTONE X,1,RGB255,USER,0,0,255,100,{u},~,");
        assert_eq!(parse_color(&spot), Some(CorelColor { model: 5, palette: 5, value: 0x0000_00ff }));
        assert_eq!(parse_color("RGB255,USER,1,2,3"), None, "no id");
        assert_eq!(parse_color(""), None);
    }

    #[test]
    fn version_14_characters_moved_one_by_one() {
        // Artistic text (frame flag set), one frame, one paragraph whose flag byte announces the
        // 64-byte block, then a run style and a per-character style: rotation, shift and rise.
        let le = |v: u32| v.to_le_bytes().to_vec();
        let mut d = le(0xffff_ffff);
        d.extend_from_slice(&[0; 0x20]);
        d.extend(le(1)); // frames
        d.extend(le(7)); // text id
        d.extend_from_slice(&[0; 48]);
        d.extend(le(0)); // not on a path
        d.extend(le(1)); // paragraphs
        d.extend(le(3)); // style
        d.extend_from_slice(&[0, 1]);
        d.extend_from_slice(&[0; 64]);
        d.extend(le(2)); // styles
        d.extend_from_slice(&[1, 0, 0, 0]); // the run's own: nothing set
        d.extend_from_slice(&[1, 0, 0x38, 0]);
        d.extend(le((-18_000_000i32) as u32));
        d.extend(le(181));
        d.extend(le((-72i32) as u32));
        d.extend(le(2)); // characters, 8 bytes each: the second uses style 1 (key 2)
        d.extend_from_slice(&[0; 8]);
        d.extend_from_slice(&(2u64 << 16).to_le_bytes());
        d.extend(le(2));
        d.extend_from_slice(b"TA\0");
        let (id, paras) = decode_txsm(&d, CdrVersion(1400), &BTreeMap::new()).unwrap();
        assert_eq!((id, paras.len()), (7, 1));
        let moved = paras[0].runs.iter().find(|r| r.style.rotation.is_some()).unwrap();
        assert_eq!((moved.style.rotation, moved.style.shift, moved.style.rise), (Some(-18.0), Some(1.81), Some(-0.72)));
        assert_eq!(moved.decoded(&TextStyle::default()), "A");
        // Without its block the record is short: an error, not a panic.
        assert!(decode_txsm(&d[..150], CdrVersion(1400), &BTreeMap::new()).is_err());
    }

    #[test]
    fn json_styles() {
        let u = "1a2b3c4d-0000-1111-2222-333344445555";
        let s = style_from_json(&format!(
            r#"{{"character":{{"latin":{{"font":"Arial CE","size":127000}},"fill":{{"type":1,"primaryColor":"RGB255,USER,255,0,0,100,{u}"}},"outline":{{"width":2540,"color":"RGB255,USER,0,0,255,100,{u}"}}}},"paragraph":{{"justify":2}}}}"#
        ));
        assert_eq!((s.font.as_deref(), s.charset, s.size, s.align), (Some("Arial"), Some(charset::EAST_EUROPE), Some(0.5), Some(2)));
        assert_eq!(s.fill, Some(Ink::Color(CorelColor { model: 5, palette: 5, value: 0x00ff0000 })));
        assert!(matches!(s.outline, Some(Pen::Line { width, .. }) if (width - 0.01).abs() < 1e-12));
        assert_eq!(style_from_json(r#"{"character":{"fill":{"type":0}}}"#).fill, Some(Ink::None));
        // CorelDRAW X7 writes the numbers as strings; spacing is in millionths.
        let x7 = style_from_json(
            r#"{"character":{"latin":{"font":"Arial","size":"127000","charset":"0"},"fill":{"type":"0"}},"paragraph":{"justify":"2","interCharSpacing":"250000","interLineSpacing":"1500000"}}"#,
        );
        assert_eq!((x7.size, x7.align, x7.fill), (Some(0.5), Some(2), Some(Ink::None)));
        assert_eq!((x7.char_spacing, x7.line_spacing), (Some(0.25), Some(1.5)));
        let bad = style_from_json(r#"{"character":{"latin":{"size":"-3"}},"paragraph":{"justify":"x","interLineSpacing":"0"}}"#);
        assert_eq!((bad.size, bad.align, bad.line_spacing), (None, None, None));
        assert_eq!(style_from_json("not json"), TextStyle::default());
        assert_eq!(style_from_json(&"[".repeat(10_000)), TextStyle::default());
    }

    #[test]
    fn version_16_and_17_text_with_json_styles() {
        for version in [CdrVersion(1600), CdrVersion(1700), CdrVersion(2100)] {
            let rec = synth::txsm16(
                version,
                4,
                r#"{"character":{"latin":{"font":"Verdana","size":254000}}}"#,
                r#"{"character":{"latin":{"size":127000}}}"#,
                "Ünïcode",
            );
            let (id, paras) = decode_txsm(&rec, version, &BTreeMap::new()).unwrap();
            assert_eq!((id, paras[0].style), (77, 4), "{version:?}");
            let run = &paras[0].runs[0];
            assert_eq!(run.chars, Chars::Unicode("Ünïcode".into()));
            // The paragraph's JSON style with the run's own over it.
            assert_eq!((run.style.font.as_deref(), run.style.size), (Some("Verdana"), Some(0.5)));
            for n in 0..rec.len() {
                let _ = decode_txsm(&rec[..n], version, &BTreeMap::new());
            }
        }
    }

    #[test]
    fn version_5_and_6_styd_styles() {
        // Version 6: id, then a 32-bit argument table: fill, font (id, set, size), alignment.
        let mut v6 = 12u16.to_le_bytes().to_vec();
        let body = |wide: bool| {
            let n = |x: u32| if wide { x.to_le_bytes().to_vec() } else { u16::try_from(x).unwrap().to_le_bytes().to_vec() };
            let w = if wide { 4 } else { 2 };
            let (args_at, types_at) = (5 * w, 8 * w);
            let fill_at = 11 * w;
            let font_at = fill_at + 4;
            let font_len = if wide { 4 + 2 + 2 + 8 + 4 } else { 1 + 1 + 2 };
            let align_at = font_at + font_len;
            let mut b = Vec::new();
            for x in [align_at as u32 + w as u32, 3, args_at as u32, types_at as u32, 9] {
                b.extend(n(x));
            }
            for x in [fill_at, font_at, align_at] {
                b.extend(n(x as u32));
            }
            for x in [0xe1u32, 0xdc, 0xcd] {
                b.extend(n(x));
            }
            b.extend(le(&[5]));
            if wide {
                b.extend_from_slice(&[0; 4]);
                b.extend_from_slice(&1u16.to_le_bytes());
                b.extend_from_slice(&0u16.to_le_bytes());
                b.extend_from_slice(&[0; 8]);
                b.extend_from_slice(&127_000i32.to_le_bytes());
            } else {
                b.extend_from_slice(&[1, 0]);
                b.extend_from_slice(&500i16.to_le_bytes());
            }
            b.extend(n(3));
            b
        };
        v6.extend(body(true));
        let (id, parent, s) = decode_styd(&v6, CdrVersion(600), &arial()).unwrap();
        assert_eq!((id, parent), (12, 9));
        assert_eq!((s.font.as_deref(), s.size, s.align, s.fill), (Some("Arial"), Some(0.5), Some(3), Some(Ink::Record(5))));
        let mut v5 = 13u16.to_le_bytes().to_vec();
        v5.extend(body(false));
        let (id, _, s) = decode_styd(&v5, CdrVersion(500), &arial()).unwrap();
        assert_eq!((id, s.font.as_deref(), s.size, s.align), (13, Some("Arial"), Some(0.5), Some(3)));
        for n in 0..v6.len() {
            let _ = decode_styd(&v6[..n], CdrVersion(600), &arial());
        }
    }

    #[test]
    fn unread_versions_and_damage_are_errors() {
        assert!(decode_txsm(&[0; 64], CdrVersion(400), &arial()).is_err());
        let rec = synth::txsm(1, 1, &[("abc", Some(0.2))]);
        for n in 0..rec.len() {
            let _ = decode_txsm(&rec[..n], V9, &arial());
        }
        let st = synth::stlt(0.2, 1, 1);
        for n in 0..st.len() {
            let _ = Styles::decode(&st[..n], V9, &arial());
        }
    }
}
