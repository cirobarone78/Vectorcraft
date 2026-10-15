//! Reader for CorelDRAW documents (`.cdr`) saved by CorelDRAW X4 (version 14) and later, which
//! wrap the drawing in a ZIP package.
//!
//! This first stage reads the package only: it lists the entries, finds the RIFF stream that holds
//! the drawing (`content/riffData.cdr` in X4/X5, `content/root.dat` plus `content/data/*` from X6
//! on) and reads its version from the header, then parses the two XMP files CorelDRAW writes
//! beside it: `metadata/metadata.xml` (document properties, page size, counts) and
//! `metadata/textinfo.xml` (layer, page and object names, plain text).
//!
//! [`riff`] walks the RIFF chunk tree itself, for bare `.cdr` files from versions 3 to 13 and for
//! the package's RIFF stream alike, and reports raw record headers. [`geometry`] decodes the
//! Bézier path, transform, outline and fill records and maps them into VectorCraft's paths and
//! appearances. [`import`] routes a file (ZIP package or bare RIFF) and builds a VectorCraft
//! document; the `test_cdr` binary runs it from the command line and validates the result.
//! `LIBCDR_NOTES.md` maps how the rest of the records are laid out.
//!
//! Everything read from the file is untrusted: entry sizes, XML depth and item counts are capped by
//! [`Limits`], and every failure is an [`Error`], never a panic.
#![forbid(unsafe_code)]

use std::fmt;

pub mod bitmap;
pub mod geometry;
pub mod import;
pub mod metadata;
pub mod package;
pub mod palettes;
pub mod riff;
#[cfg(any(test, feature = "synth"))]
pub mod synth;
pub mod text;
mod xmp;

pub use metadata::{Break, Metadata, PageInfo, TextInfo, TextRun, TextStream};
pub use package::{Layout, Package, RiffKind, RiffRoot};

/// Caps on what a document may make the reader allocate or walk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    /// Largest uncompressed XML or list entry read into memory.
    pub max_xml_bytes: u64,
    /// Most list items, scalar fields and text runs collected from one XML file.
    pub max_items: usize,
    /// Deepest XML element nesting accepted.
    pub max_depth: usize,
    /// Most names read from `content/dataFileList.dat`.
    pub max_data_files: usize,
    /// Most bytes inflated from all `cmpr` lists of one RIFF stream together.
    pub max_inflated_bytes: u64,
    /// Deepest RIFF list nesting accepted (libcdr allows 1024).
    pub max_riff_depth: usize,
    /// Most chunks in one RIFF stream.
    pub max_chunks: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_xml_bytes: 16 << 20,
            max_items: 100_000,
            max_depth: 64,
            max_data_files: 4096,
            max_inflated_bytes: 512 << 20,
            max_riff_depth: 256,
            max_chunks: 4_000_000,
        }
    }
}

/// Why a document could not be read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// The bytes are not a readable ZIP archive, or an entry could not be read.
    Zip(String),
    /// Neither a CorelDRAW package (a ZIP with its RIFF stream or metadata) nor a CDR RIFF stream.
    NotCdr,
    /// An entry's uncompressed size is over the limit.
    TooLarge { entry: String, limit: u64 },
    /// An XML entry is malformed.
    Xml { entry: String, message: String },
    /// A count or depth read from the file is over the limit.
    Limit { what: &'static str, limit: usize },
    /// The bytes are not a CorelDRAW RIFF stream.
    Riff(String),
    /// A record is too short or malformed for its layout.
    Record { record: &'static str, message: String },
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Zip(m) => write!(f, "CorelDRAW package: {m}"),
            Error::NotCdr => {
                write!(f, "not a CorelDRAW document: neither a package with content/riffData.cdr or content/root.dat nor a CDR RIFF stream")
            }
            Error::TooLarge { entry, limit } => write!(f, "{entry} is larger than {limit} bytes"),
            Error::Xml { entry, message } => write!(f, "{entry}: {message}"),
            Error::Limit { what, limit } => write!(f, "too many {what} (limit {limit})"),
            Error::Riff(m) => write!(f, "CorelDRAW RIFF: {m}"),
            Error::Record { record, message } => write!(f, "{record} record {message}"),
        }
    }
}

impl std::error::Error for Error {}

/// What the package tells us about a document without decoding its RIFF records.
#[derive(Debug, Clone, PartialEq)]
pub struct Document {
    /// Where the parts of the document live in the archive.
    pub layout: Layout,
    /// Format version from the RIFF header, times 100 as CorelDRAW writes it (1400 for X4).
    pub version: Option<u32>,
    /// Parsed `metadata/metadata.xml`, when present.
    pub metadata: Option<Metadata>,
    /// Parsed `metadata/textinfo.xml`, when present.
    pub text_info: Option<TextInfo>,
}

impl Document {
    /// Open a `.cdr` file held in memory.
    pub fn open(bytes: &[u8], limits: Limits) -> Result<Self, Error> {
        let mut package = Package::open(bytes, limits)?;
        let layout = package.layout()?;
        if layout.riff.is_none() && layout.metadata_xml.is_none() {
            return Err(Error::NotCdr);
        }
        let version = match &layout.riff {
            Some(riff) => package.riff_version(&riff.path)?,
            None => None,
        };
        let metadata = match &layout.metadata_xml {
            Some(name) => Some(Metadata::parse(&package.read_xml(name)?, name, &limits)?),
            None => None,
        };
        let text_info = match &layout.textinfo_xml {
            Some(name) => Some(TextInfo::parse(&package.read_xml(name)?, name, &limits)?),
            None => None,
        };
        Ok(Self { layout, version, metadata, text_info })
    }

    /// Layer names in document order, from `textinfo.xml`. CorelDRAW lists each name once for the
    /// whole document there, so per-page layer membership needs the RIFF records.
    pub fn layer_names(&self) -> &[String] {
        self.text_info.as_ref().map_or(&[], |t| t.layer_names.as_slice())
    }

    /// Layer count CorelDRAW recorded in `metadata.xml`.
    pub fn layer_count(&self) -> Option<u32> {
        self.metadata.as_ref().and_then(|m| m.layer_count)
    }

    /// Page count CorelDRAW recorded in `metadata.xml`.
    pub fn page_count(&self) -> Option<u32> {
        self.metadata.as_ref().and_then(|m| m.page_count)
    }
}

#[cfg(test)]
pub(crate) mod test_zip {
    use std::io::Write;

    /// A stored ZIP holding `entries`, built in memory for tests.
    pub fn build(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut w = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        let opts = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
        for (name, data) in entries {
            w.start_file(*name, opts).unwrap();
            w.write_all(data).unwrap();
        }
        w.finish().unwrap().into_inner()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const META: &str = r#"<?xml version="1.0" encoding="UTF-8" ?>
<x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
<rdf:Description rdf:about="" xmlns="http://namespace.corel.com/Cdr/metadata">
<FontsUsed><rdf:Bag><rdf:li>Sans</rdf:li><rdf:li>Serif &amp; Co</rdf:li></rdf:Bag></FontsUsed>
<Title>Plan</Title><Author></Author><AppVersion>1500</AppVersion><NumPages>2</NumPages><NumLayers>3</NumLayers>
<PageWidth>2100000</PageWidth><PageHeight>2970000</PageHeight><PageSizeName>A4</PageSizeName>
<Objects><Total>5</Total><Rect>2</Rect></Objects>
<ClipRect left="-10" top="20" right="30" bottom="-40" />
</rdf:Description></rdf:RDF></x:xmpmeta>"#;

    const TEXT: &str = r#"<x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
<rdf:Description rdf:about="">
<LayerNames><rdf:Bag><rdf:li>Back</rdf:li><rdf:li>Front</rdf:li></rdf:Bag></LayerNames>
<PageNames><rdf:Bag /></PageNames>
<TextData><TextStream><TextRun lang="1033" break="para">Hello </TextRun><TextRun break="line">world</TextRun></TextStream></TextData>
</rdf:Description></rdf:RDF></x:xmpmeta>"#;

    fn riff(version_byte: u8) -> Vec<u8> {
        let mut v = b"RIFF\x04\x00\x00\x00CDR".to_vec();
        v.push(version_byte);
        v
    }

    #[test]
    fn reads_layout_version_metadata_and_layers() {
        let riff = riff(b'F');
        let bytes = test_zip::build(&[
            ("content/riffData.cdr", &riff),
            ("metadata/metadata.xml", META.as_bytes()),
            ("metadata/textinfo.xml", TEXT.as_bytes()),
        ]);
        let doc = Document::open(&bytes, Limits::default()).unwrap();
        assert_eq!(doc.version, Some(1500));
        assert_eq!(doc.layout.riff.as_ref().map(|r| r.kind), Some(RiffKind::RiffData));
        let m = doc.metadata.as_ref().unwrap();
        assert_eq!(m.title.as_deref(), Some("Plan"));
        assert_eq!(m.author, None, "empty fields read as absent");
        assert_eq!(m.fonts_used, ["Sans", "Serif & Co"]);
        assert_eq!(m.object_counts.get("Total"), Some(&5));
        assert_eq!(m.clip_rect, Some([-10, 20, 30, -40]));
        let page = m.page.as_ref().unwrap();
        assert!((page.width_mm() - 210.0).abs() < 1e-9 && (page.height_mm() - 297.0).abs() < 1e-9);
        assert_eq!((doc.page_count(), doc.layer_count()), (Some(2), Some(3)));
        assert_eq!(doc.layer_names(), ["Back", "Front"]);
        let t = doc.text_info.as_ref().unwrap();
        assert!(t.page_names.is_empty());
        assert_eq!(t.text_streams[0].text(), "Hello \nworld");
        assert_eq!(t.text_streams[0].runs[0].lang, Some(1033));
        assert_eq!(t.text_streams[0].runs[1].brk, Break::Line);
    }

    #[test]
    fn x6_layout_lists_data_files() {
        let riff = riff(b'G');
        let bytes = test_zip::build(&[("content/root.dat", &riff), ("content/dataFileList.dat", b"a.dat\nb.dat\n"), ("content/data/a.dat", b"")]);
        let doc = Document::open(&bytes, Limits::default()).unwrap();
        assert_eq!(doc.version, Some(1600));
        assert_eq!(doc.layout.riff.as_ref().map(|r| r.kind), Some(RiffKind::Root));
        assert_eq!(doc.layout.data_files, ["a.dat", "b.dat"]);
        assert!(doc.metadata.is_none() && doc.layer_names().is_empty());
    }

    #[test]
    fn rejects_non_cdr_and_garbage() {
        let other = test_zip::build(&[("word/document.xml", b"<w/>")]);
        assert_eq!(Document::open(&other, Limits::default()), Err(Error::NotCdr));
        assert!(matches!(Document::open(b"RIFF\0\0\0\0CDR9", Limits::default()), Err(Error::Zip(_))));
    }

    #[test]
    fn caps_entry_size_and_items() {
        let bytes = test_zip::build(&[("metadata/metadata.xml", META.as_bytes())]);
        let small = Limits { max_xml_bytes: 64, ..Limits::default() };
        assert!(matches!(Document::open(&bytes, small), Err(Error::TooLarge { .. })));
        let few = Limits { max_items: 3, ..Limits::default() };
        assert!(matches!(Document::open(&bytes, few), Err(Error::Limit { .. })));
    }

    #[test]
    fn malformed_xml_is_an_error() {
        let bytes = test_zip::build(&[("metadata/metadata.xml", b"<a><b></a>")]);
        assert!(matches!(Document::open(&bytes, Limits::default()), Err(Error::Xml { .. })));
    }
}
