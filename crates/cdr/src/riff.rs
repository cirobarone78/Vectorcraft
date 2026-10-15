//! The RIFF chunk tree inside a CorelDRAW document: a bare `.cdr` file from versions 3 to 13,
//! or the `content/riffData.cdr` / `content/root.dat` stream of an X4+ package.
//!
//! The walk follows libcdr's `CDRParser::parseRecord` (`src/lib/CDRParser.cpp`), so records land
//! where libcdr would find them:
//!
//! - A chunk is a fourCC and a little-endian `u32` length, then that many data bytes. Zero bytes
//!   between chunks are padding and are skipped (CorelDRAW pads odd-length chunks).
//! - `RIFF` and `LIST` chunks carry a list type (a fourCC) and contain more chunks. The root
//!   `RIFF`'s list type is `CDR` (any case) plus a version character; see [`CdrVersion`].
//! - A `LIST` of type `cmpr` (version 7+) holds zlib data: a 24-byte header (four `u32`s, then
//!   `CPng`, `u16` 1, `u16` 4), the compressed chunks, then a compressed table of `u32` block
//!   lengths. Inside, a chunk length smaller than the table's size is an index into it, and that
//!   applies to every chunk nested below until another `cmpr`.
//! - From version 7, a `LIST` of type `stlt` is a single record (the text style table), not a list.
//!
//! CorelDRAW 1 and 2 files (`WL` signature, "Waldo") are not RIFF; [`identify`] reports them and
//! [`parse`] refuses them. Damage that libcdr would stop or skip at (a chunk running past its
//! parent, a failed inflate) becomes a [`Warning`] and the walk keeps what it has; only the
//! [`Limits`] caps are hard errors.

use std::io::Read;
use std::ops::Range;

use nom::IResult;
use nom::Parser;
use nom::bytes::complete::{tag, take};
use nom::number::complete::le_u32;

use crate::{Error, Limits};

/// A four-character code, in file order.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct FourCc(pub [u8; 4]);

impl FourCc {
    pub const RIFF: Self = Self(*b"RIFF");
    pub const LIST: Self = Self(*b"LIST");
    pub const CMPR: Self = Self(*b"cmpr");
    pub const STLT: Self = Self(*b"stlt");
    pub const PAGE: Self = Self(*b"page");
    pub const LAYER: Self = Self(*b"layr");
    pub const OBJECT: Self = Self(*b"obj ");
    pub const GROUP: Self = Self(*b"grp ");

    /// The code as text, with bytes outside printable ASCII as `?`.
    pub fn as_text(&self) -> String {
        self.0.iter().map(|&b| if b.is_ascii_graphic() || b == b' ' { char::from(b) } else { '?' }).collect()
    }

    fn is_list(self) -> bool {
        self == Self::RIFF || self == Self::LIST
    }
}

impl std::fmt::Debug for FourCc {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}", self.as_text())
    }
}

impl std::fmt::Display for FourCc {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.as_text())
    }
}

/// A CorelDRAW format version, times 100 as libcdr counts it (300 for CorelDRAW 3, 1400 for X4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct CdrVersion(pub u32);

impl CdrVersion {
    /// Version from the character after `CDR` in a list type: a space for 3, `1`–`9` for 1–9,
    /// `A`–`Z` for 10–35.
    pub fn from_char(c: u8) -> Option<Self> {
        let v = match c {
            b' ' => 3,
            b'1'..=b'9' => u32::from(c - b'0'),
            b'A'..=b'Z' => u32::from(c - b'A') + 10,
            _ => return None,
        };
        Some(Self(v * 100))
    }

    /// Version from a list type such as `CDR9` or `cdrE`.
    pub fn from_list_type(t: FourCc) -> Option<Self> {
        let [c, d, r, v] = t.0;
        if [c, d, r].eq_ignore_ascii_case(b"CDR") { Self::from_char(v) } else { None }
    }

    /// The major version, such as 14 for X4.
    pub fn major(self) -> u32 {
        self.0 / 100
    }

    /// Width of coordinates in geometry records: 16 bits before version 6, 32 bits after.
    pub fn coordinate_bits(self) -> u8 {
        if self.0 < 600 { 16 } else { 32 }
    }
}

/// What a byte stream is, from its first 12 bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileKind {
    /// A RIFF stream whose form type is `CDR` plus a version character.
    Cdr(CdrVersion),
    /// CorelDRAW 1 or 2 ("Waldo", `WL` signature): not RIFF.
    Waldo,
    /// A RIFF stream of another form type (CMX files are `CMX1`, for example).
    OtherRiff(FourCc),
}

/// Identify a stream from its header. `None` when it is neither RIFF nor Waldo.
pub fn identify(bytes: &[u8]) -> Option<FileKind> {
    if bytes.starts_with(b"WL") {
        return Some(FileKind::Waldo);
    }
    let (_, (id, _, form)) = (fourcc, le_u32, fourcc).parse(bytes).ok()?;
    if id != FourCc::RIFF {
        return None;
    }
    Some(match CdrVersion::from_list_type(form) {
        Some(v) => FileKind::Cdr(v),
        None => FileKind::OtherRiff(form),
    })
}

/// Which buffer a chunk lives in: 0 is the input, `n` is the `n`th inflated `cmpr` list.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct StreamId(pub usize);

/// A raw chunk header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Header {
    pub fourcc: FourCc,
    /// The length field as stored. Inside a `cmpr` list it may be a block-table index.
    pub raw_length: u32,
    /// The data length after resolving a block-table index.
    pub length: u32,
    pub stream: StreamId,
    /// Offset of the header in its stream.
    pub offset: usize,
}

/// How a `cmpr` list was stored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Compression {
    /// The first header field: compressed size including the 8-byte `CPng` header.
    pub compressed_size: u32,
    /// The second header field: declared size of the inflated chunks.
    pub uncompressed_size: u32,
    /// The inflated chunks.
    pub stream: StreamId,
    /// The block-length table the chunks' length fields index into.
    pub block_lengths: Vec<u32>,
}

/// A `RIFF` or `LIST` chunk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct List {
    pub header: Header,
    pub list_type: FourCc,
    pub compression: Option<Compression>,
    pub children: Vec<Chunk>,
}

/// A leaf chunk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Record {
    pub header: Header,
    /// The record's type: its fourCC, or the list type for a `LIST` read as one record (`stlt`).
    pub kind: FourCc,
    /// Where the data is in [`Header::stream`]. For a `LIST` read as a record, the data starts
    /// after the 4-byte list type, so it is 4 bytes shorter than [`Header::length`].
    pub data: Range<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Chunk {
    List(List),
    Record(Record),
}

impl Chunk {
    pub fn header(&self) -> &Header {
        match self {
            Chunk::List(l) => &l.header,
            Chunk::Record(r) => &r.header,
        }
    }

    /// The list type for a list, the record type for a record.
    pub fn kind(&self) -> FourCc {
        match self {
            Chunk::List(l) => l.list_type,
            Chunk::Record(r) => r.kind,
        }
    }

    pub fn children(&self) -> &[Chunk] {
        match self {
            Chunk::List(l) => &l.children,
            Chunk::Record(_) => &[],
        }
    }
}

/// Damage the walk stepped around.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Warning {
    pub stream: StreamId,
    pub offset: usize,
    pub message: String,
}

/// A parsed chunk tree and the buffers its records point into.
#[derive(Debug, Clone)]
pub struct Riff<'a> {
    source: &'a [u8],
    inflated: Vec<Vec<u8>>,
    pub version: CdrVersion,
    /// Top-level chunks, normally one `RIFF`.
    pub chunks: Vec<Chunk>,
    pub warnings: Vec<Warning>,
}

impl<'a> Riff<'a> {
    /// The bytes of a stream.
    pub fn stream(&self, id: StreamId) -> Option<&[u8]> {
        match id.0 {
            0 => Some(self.source),
            n => self.inflated.get(n - 1).map(Vec::as_slice),
        }
    }

    /// A record's data.
    pub fn data(&self, record: &Record) -> Option<&[u8]> {
        self.stream(record.header.stream)?.get(record.data.clone())
    }

    /// Every chunk, depth first in file order, with its depth (0 for top-level chunks).
    pub fn walk(&self) -> Walk<'_> {
        Walk { stack: self.chunks.iter().rev().map(|c| (0, c)).collect() }
    }

    /// Every record of type `kind`, in file order.
    pub fn records(&self, kind: FourCc) -> impl Iterator<Item = &Record> {
        self.walk().filter_map(move |(_, c)| match c {
            Chunk::Record(r) if r.kind == kind => Some(r),
            _ => None,
        })
    }

    /// Every list of type `list_type`, in file order.
    pub fn lists(&self, list_type: FourCc) -> impl Iterator<Item = &List> {
        self.walk().filter_map(move |(_, c)| match c {
            Chunk::List(l) if l.list_type == list_type => Some(l),
            _ => None,
        })
    }
}

/// Depth-first iterator from [`Riff::walk`].
pub struct Walk<'r> {
    stack: Vec<(usize, &'r Chunk)>,
}

impl<'r> Iterator for Walk<'r> {
    type Item = (usize, &'r Chunk);

    fn next(&mut self) -> Option<Self::Item> {
        let (depth, chunk) = self.stack.pop()?;
        self.stack.extend(chunk.children().iter().rev().map(|c| (depth + 1, c)));
        Some((depth, chunk))
    }
}

fn fourcc(i: &[u8]) -> IResult<&[u8], FourCc> {
    let (i, b) = take(4usize)(i)?;
    let mut code = [0; 4];
    code.copy_from_slice(b);
    Ok((i, FourCc(code)))
}

/// A chunk header: fourCC and length.
fn chunk_header(i: &[u8]) -> IResult<&[u8], (FourCc, u32)> {
    (fourcc, le_u32).parse(i)
}

/// The 24 bytes after a `cmpr` list type. Returns the compressed and uncompressed sizes.
fn cmpr_header(i: &[u8]) -> IResult<&[u8], (u32, u32)> {
    let (i, (compressed, uncompressed, _blocks_compressed, _blocks_uncompressed)) = (le_u32, le_u32, le_u32, le_u32).parse(i)?;
    let (i, _) = (tag(&b"CPng"[..]), tag(&[1u8, 0][..]), tag(&[4u8, 0][..])).parse(i)?;
    Ok((i, (compressed, uncompressed)))
}

/// Parse the chunk tree of a CorelDRAW RIFF stream (versions 3 and later).
pub fn parse<'a>(bytes: &'a [u8], limits: &Limits) -> Result<Riff<'a>, Error> {
    let version = match identify(bytes) {
        Some(FileKind::Cdr(v)) => v,
        Some(FileKind::Waldo) => return Err(Error::Riff("CorelDRAW 1–2 (WL) files are not RIFF and are not supported".into())),
        Some(FileKind::OtherRiff(t)) => return Err(Error::Riff(format!("RIFF form type {t} is not CorelDRAW"))),
        None => return Err(Error::Riff("not a RIFF stream".into())),
    };
    let mut w = Walker { limits, inflated: Vec::new(), inflated_total: 0, chunks: 0, version, warnings: Vec::new() };
    let chunks = w.seq(bytes, StreamId(0), 0..bytes.len(), &[], 0)?;
    Ok(Riff { source: bytes, inflated: w.inflated, version: w.version, chunks, warnings: w.warnings })
}

struct Walker<'l> {
    limits: &'l Limits,
    inflated: Vec<Vec<u8>>,
    inflated_total: u64,
    chunks: usize,
    version: CdrVersion,
    warnings: Vec<Warning>,
}

impl Walker<'_> {
    fn warn(&mut self, stream: StreamId, offset: usize, message: impl Into<String>) {
        self.warnings.push(Warning { stream, offset, message: message.into() });
    }

    /// The chunks in `buf[range]`.
    fn seq(&mut self, buf: &[u8], stream: StreamId, range: Range<usize>, blocks: &[u32], depth: usize) -> Result<Vec<Chunk>, Error> {
        if depth > self.limits.max_riff_depth {
            return Err(Error::Limit { what: "nested RIFF lists", limit: self.limits.max_riff_depth });
        }
        let end = range.end.min(buf.len());
        let mut pos = range.start;
        let mut out = Vec::new();
        loop {
            while buf.get(pos).is_some_and(|&b| b == 0) && pos < end {
                pos += 1;
            }
            let Some(rest) = buf.get(pos..end).filter(|r| !r.is_empty()) else { break };
            let Ok((_, (id, raw_length))) = chunk_header(rest) else {
                self.warn(stream, pos, format!("{} trailing bytes are too short for a chunk header", rest.len()));
                break;
            };
            self.chunks += 1;
            if self.chunks > self.limits.max_chunks {
                return Err(Error::Limit { what: "RIFF chunks", limit: self.limits.max_chunks });
            }
            let length = usize::try_from(raw_length).ok().and_then(|i| blocks.get(i)).copied().unwrap_or(raw_length);
            let header = Header { fourcc: id, raw_length, length, stream, offset: pos };
            let data_start = pos + 8;
            let declared_end = data_start.saturating_add(usize::try_from(length).unwrap_or(usize::MAX));
            let chunk_end = declared_end.min(end);
            if declared_end > end {
                self.warn(stream, pos, format!("{id} chunk of {length} bytes runs {} bytes past its parent", declared_end - end));
            }
            let chunk = if id.is_list() {
                self.list(buf, header, data_start..chunk_end, blocks, depth)?
            } else {
                Chunk::Record(Record { header, kind: id, data: data_start..chunk_end })
            };
            out.push(chunk);
            if declared_end >= end {
                break;
            }
            pos = declared_end;
        }
        Ok(out)
    }

    fn list(&mut self, buf: &[u8], header: Header, body: Range<usize>, blocks: &[u32], depth: usize) -> Result<Chunk, Error> {
        let empty = |list_type| Chunk::List(List { header, list_type, compression: None, children: Vec::new() });
        let Some(Ok((_, list_type))) = buf.get(body.clone()).map(fourcc) else {
            self.warn(header.stream, header.offset, format!("{} chunk too short for a list type", header.fourcc));
            return Ok(empty(FourCc([0; 4])));
        };
        let inner = body.start + 4..body.end;
        if let Some(v) = CdrVersion::from_list_type(list_type) {
            self.version = v;
        }
        if list_type == FourCc::STLT && self.version >= CdrVersion(700) {
            return Ok(Chunk::Record(Record { header, kind: list_type, data: inner }));
        }
        if list_type == FourCc::CMPR {
            return self.compressed(buf, header, inner, depth);
        }
        let children = self.seq(buf, header.stream, inner, blocks, depth + 1)?;
        Ok(Chunk::List(List { header, list_type, compression: None, children }))
    }

    fn compressed(&mut self, buf: &[u8], header: Header, inner: Range<usize>, depth: usize) -> Result<Chunk, Error> {
        let mut list = List { header, list_type: FourCc::CMPR, compression: None, children: Vec::new() };
        let Some(Ok((_, (compressed_size, uncompressed_size)))) = buf.get(inner.clone()).map(cmpr_header) else {
            self.warn(header.stream, header.offset, "cmpr list without a CPng header");
            return Ok(Chunk::List(list));
        };
        // libcdr inflates `compressed_size` bytes from the end of the 24-byte header. The size
        // counts the 8-byte CPng header too, so this reads 8 bytes past the zlib stream, which
        // inflate ignores, and the block table's zlib data starts exactly where the read ends.
        let data_start = inner.start + 24;
        let blocks_start = data_start.saturating_add(usize::try_from(compressed_size).unwrap_or(usize::MAX));
        let (true, Some(data), Some(table)) = (blocks_start <= inner.end, buf.get(data_start..blocks_start), buf.get(blocks_start..inner.end)) else {
            self.warn(header.stream, header.offset, format!("cmpr data of {compressed_size} bytes runs past the list"));
            return Ok(Chunk::List(list));
        };
        let (data, table) = match (self.inflate(data)?, self.inflate(table)?) {
            (Ok(d), Ok(t)) => (d, t),
            (Err(e), _) | (_, Err(e)) => {
                self.warn(header.stream, header.offset, format!("cmpr data does not inflate: {e}"));
                return Ok(Chunk::List(list));
            }
        };
        if table.len() % 4 != 0 {
            self.warn(header.stream, header.offset, format!("block table of {} bytes is not whole u32s", table.len()));
        }
        let block_lengths: Vec<u32> = table.chunks_exact(4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]])).collect();
        // Reserve the stream id before walking, so nested cmpr lists number after this one.
        self.inflated.push(Vec::new());
        let stream = StreamId(self.inflated.len());
        list.children = self.seq(&data, stream, 0..data.len(), &block_lengths, depth + 1)?;
        if let Some(slot) = self.inflated.get_mut(stream.0 - 1) {
            *slot = data;
        }
        list.compression = Some(Compression { compressed_size, uncompressed_size, stream, block_lengths });
        Ok(Chunk::List(list))
    }

    /// Inflate zlib data within what is left of the [`Limits::max_inflated_bytes`] budget.
    /// The outer error is the budget (fatal), the inner one is corrupt data (a warning).
    fn inflate(&mut self, data: &[u8]) -> Result<Result<Vec<u8>, String>, Error> {
        let limit = self.limits.max_inflated_bytes;
        let budget = limit.saturating_sub(self.inflated_total);
        let mut out = Vec::new();
        let read = flate2::read::ZlibDecoder::new(data).take(budget.saturating_add(1)).read_to_end(&mut out);
        let n = u64::try_from(out.len()).unwrap_or(u64::MAX);
        if n > budget {
            return Err(Error::TooLarge { entry: "inflated cmpr data".into(), limit });
        }
        self.inflated_total += n;
        Ok(read.map(|_| out).map_err(|e| e.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::synth::*;

    fn kinds(riff: &Riff<'_>) -> Vec<(usize, String)> {
        riff.walk().map(|(d, c)| (d, c.kind().as_text())).collect()
    }

    #[test]
    fn versions_and_kinds() {
        assert_eq!(identify(b"RIFF\0\0\0\0CDR "), Some(FileKind::Cdr(CdrVersion(300))));
        assert_eq!(identify(b"RIFF\0\0\0\0cdr9"), Some(FileKind::Cdr(CdrVersion(900))));
        assert_eq!(identify(b"RIFF\0\0\0\0CDRD"), Some(FileKind::Cdr(CdrVersion(1300))));
        assert_eq!(identify(b"RIFF\0\0\0\0CDRX"), Some(FileKind::Cdr(CdrVersion(3300))));
        assert_eq!(identify(b"RIFF\0\0\0\0CDR0"), Some(FileKind::OtherRiff(FourCc(*b"CDR0"))));
        assert_eq!(identify(b"RIFF\0\0\0\0CMX1"), Some(FileKind::OtherRiff(FourCc(*b"CMX1"))));
        assert_eq!(identify(b"WL\x01\x02"), Some(FileKind::Waldo));
        assert_eq!(identify(b"RIFF"), None);
        assert_eq!(identify(b"PK\x03\x04"), None);
        assert_eq!((CdrVersion(500).coordinate_bits(), CdrVersion(600).coordinate_bits()), (16, 32));
        assert_eq!(CdrVersion(1300).major(), 13);
    }

    #[test]
    fn legacy_tree_with_padding() {
        // A version 5 file: no compression, an odd-length record padded with a zero byte.
        let file = list(
            b"RIFF",
            b"CDR5",
            &[
                chunk(b"vrsn", &[0xf4, 0x01]),
                list(b"LIST", b"doc ", &[chunk(b"mcfg", &[1, 2, 3])]),
                list(b"LIST", b"page", &[list(b"LIST", b"layr", &[list(b"LIST", b"obj ", &[chunk(b"loda", &[9; 6])])])]),
            ],
        );
        let riff = parse(&file, &Limits::default()).unwrap();
        assert_eq!(riff.version, CdrVersion(500));
        assert!(riff.warnings.is_empty(), "{:?}", riff.warnings);
        assert_eq!(
            kinds(&riff),
            [(0, "CDR5"), (1, "vrsn"), (1, "doc "), (2, "mcfg"), (1, "page"), (2, "layr"), (3, "obj "), (4, "loda")].map(|(d, k)| (d, k.to_owned()))
        );
        let mcfg = riff.records(FourCc(*b"mcfg")).next().unwrap();
        assert_eq!(riff.data(mcfg), Some(&[1u8, 2, 3][..]));
        assert_eq!(mcfg.header.length, 3);
        assert_eq!(riff.lists(FourCc::PAGE).count(), 1);
    }

    #[test]
    fn compressed_lists_resolve_block_lengths() {
        // Inside cmpr, length fields 0 and 1 index the block table; 6 is past its end, so literal.
        let mut inner = b"trfd\x00\x00\x00\x00".to_vec();
        inner.extend_from_slice(&[7; 10]);
        inner.extend_from_slice(b"LIST\x01\x00\x00\x00obj ");
        inner.extend_from_slice(b"flgs\x06\x00\x00\x00");
        inner.extend_from_slice(&[5; 6]);
        let file = list(b"RIFF", b"CDRD", &[cmpr(&inner, &[10, 18])]);
        let riff = parse(&file, &Limits::default()).unwrap();
        assert!(riff.warnings.is_empty(), "{:?}", riff.warnings);
        assert_eq!(riff.version, CdrVersion(1300));
        let cmpr = riff.lists(FourCc::CMPR).next().unwrap();
        let c = cmpr.compression.as_ref().unwrap();
        assert_eq!((c.uncompressed_size as usize, c.stream), (inner.len(), StreamId(1)));
        assert_eq!(riff.stream(c.stream).map(<[u8]>::len), Some(inner.len()));
        let trfd = riff.records(FourCc(*b"trfd")).next().unwrap();
        assert_eq!((trfd.header.raw_length, trfd.header.length), (0, 10));
        assert_eq!(riff.data(trfd), Some(&[7u8; 10][..]));
        let obj = riff.lists(FourCc::OBJECT).next().unwrap();
        assert_eq!(obj.header.length, 18);
        let flgs = &obj.children[0];
        assert_eq!((flgs.kind(), flgs.header().length), (FourCc(*b"flgs"), 6));
    }

    #[test]
    fn stlt_list_is_a_record_from_version_7() {
        let stlt = list(b"LIST", b"stlt", &[chunk(b"xxxx", &[])]);
        let v7 = list(b"RIFF", b"CDR7", std::slice::from_ref(&stlt));
        let riff = parse(&v7, &Limits::default()).unwrap();
        let rec = riff.records(FourCc::STLT).next().unwrap();
        assert_eq!(riff.data(rec).map(<[u8]>::len), Some(8));
        let v6 = list(b"RIFF", b"CDR6", &[stlt]);
        let riff = parse(&v6, &Limits::default()).unwrap();
        assert_eq!(riff.lists(FourCc::STLT).next().unwrap().children.len(), 1);
    }

    #[test]
    fn refuses_non_cdr_streams() {
        for bytes in [&b"WL\x01\x02rest"[..], b"RIFF\x04\x00\x00\x00WAVE", b"nope"] {
            assert!(matches!(parse(bytes, &Limits::default()), Err(Error::Riff(_))));
        }
    }

    #[test]
    fn damage_becomes_warnings() {
        // A record claiming more bytes than its parent holds, then a truncated header.
        let mut file = list(b"RIFF", b"CDR8", &[chunk(b"bbox", &[1, 2, 3, 4])]);
        let at = file.len() - 8;
        file[at..at + 4].copy_from_slice(&40u32.to_le_bytes());
        let riff = parse(&file, &Limits::default()).unwrap();
        assert_eq!(riff.records(FourCc(*b"bbox")).next().unwrap().data.len(), 4);
        assert_eq!(riff.warnings.len(), 1);

        let mut file = list(b"RIFF", b"CDR8", &[]);
        file.extend_from_slice(b"abc");
        let riff = parse(&file, &Limits::default()).unwrap();
        assert!(riff.warnings[0].message.contains("too short"));

        let mut bad = cmpr(b"anything", &[]);
        let n = bad.len();
        bad[n - 3] ^= 0xff; // corrupt the block table's zlib stream
        let file = list(b"RIFF", b"CDRA", &[bad]);
        let riff = parse(&file, &Limits::default()).unwrap();
        assert!(riff.warnings.iter().any(|w| w.message.contains("inflate")), "{:?}", riff.warnings);
    }

    #[test]
    fn limits_are_hard_errors() {
        let mut nested = chunk(b"loda", &[]);
        for _ in 0..10 {
            nested = list(b"LIST", b"grp ", &[nested]);
        }
        let file = list(b"RIFF", b"CDR9", &[nested]);
        let shallow = Limits { max_riff_depth: 5, ..Limits::default() };
        assert!(matches!(parse(&file, &shallow), Err(Error::Limit { .. })));
        let few = Limits { max_chunks: 3, ..Limits::default() };
        assert!(matches!(parse(&file, &few), Err(Error::Limit { .. })));
        let bomb = list(b"RIFF", b"CDRB", &[cmpr(&vec![0; 1 << 16], &[])]);
        let small = Limits { max_inflated_bytes: 1000, ..Limits::default() };
        assert!(matches!(parse(&bomb, &small), Err(Error::TooLarge { .. })));
    }
}
