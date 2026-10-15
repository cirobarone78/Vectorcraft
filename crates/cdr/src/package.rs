//! The ZIP package CorelDRAW X4 and later save: entry lookup, size-capped reads, and where the
//! drawing and its metadata live.
//!
//! Package layout (as libcdr's `CDRDocument.cpp` opens it, and as the X4 sample in this crate's
//! test shows): the drawing is a RIFF stream at `content/riffData.cdr` (X4/X5) or
//! `content/root.dat` (X6+). From X6 on, records too large for `root.dat` live in
//! `content/data/<name>`, listed one name per line in `content/dataFileList.dat`. Metadata is XMP
//! in `metadata/metadata.xml` and `metadata/textinfo.xml`.

use std::io::{Cursor, Read};

use zip::ZipArchive;

use crate::{Error, Limits};

const RIFF_DATA: &str = "content/riffData.cdr";
const ROOT_DAT: &str = "content/root.dat";
const DATA_FILE_LIST: &str = "content/dataFileList.dat";
const DATA_DIR: &str = "content/data/";
const METADATA_XML: &str = "metadata/metadata.xml";
const TEXTINFO_XML: &str = "metadata/textinfo.xml";

/// Which RIFF root a package uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RiffKind {
    /// `content/riffData.cdr`: one self-contained RIFF stream (X4, X5).
    RiffData,
    /// `content/root.dat`: a RIFF stream whose large records point into `content/data/*` (X6+).
    Root,
}

/// The RIFF stream holding the drawing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RiffRoot {
    /// Entry name as stored in the archive.
    pub path: String,
    pub kind: RiffKind,
}

/// Where the parts of a document live in the archive. Paths are entry names as stored.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Layout {
    /// Every entry name, in archive order.
    pub entries: Vec<String>,
    pub riff: Option<RiffRoot>,
    pub metadata_xml: Option<String>,
    pub textinfo_xml: Option<String>,
    /// Names from `content/dataFileList.dat`, in order; each lives at `content/data/<name>`.
    pub data_files: Vec<String>,
}

impl Layout {
    /// Archive entry name of a file listed in [`Layout::data_files`].
    pub fn data_file_path(name: &str) -> String {
        format!("{DATA_DIR}{name}")
    }
}

/// An open `.cdr` package.
pub struct Package<'a> {
    zip: ZipArchive<Cursor<&'a [u8]>>,
    names: Vec<String>,
    limits: Limits,
}

impl<'a> Package<'a> {
    /// Open the ZIP container. Fails on anything that is not a readable ZIP archive, including
    /// pre-X4 `.cdr` files, which are bare RIFF.
    pub fn open(bytes: &'a [u8], limits: Limits) -> Result<Self, Error> {
        let zip = ZipArchive::new(Cursor::new(bytes)).map_err(|e| Error::Zip(e.to_string()))?;
        let names = zip.file_names().map(|n| n.map(|n| n.into_owned())).collect::<Result<_, _>>().map_err(|e| Error::Zip(e.to_string()))?;
        Ok(Self { zip, names, limits })
    }

    /// Entry names, in archive order.
    pub fn entries(&self) -> &[String] {
        &self.names
    }

    /// The stored name of `wanted`, matched ignoring ASCII case.
    fn find(&self, wanted: &str) -> Option<String> {
        self.names.iter().find(|n| n.eq_ignore_ascii_case(wanted)).cloned()
    }

    /// Locate the drawing, the metadata files and the X6+ data files.
    pub fn layout(&mut self) -> Result<Layout, Error> {
        let riff = match (self.find(RIFF_DATA), self.find(ROOT_DAT)) {
            (Some(path), _) => Some(RiffRoot { path, kind: RiffKind::RiffData }),
            (None, Some(path)) => Some(RiffRoot { path, kind: RiffKind::Root }),
            (None, None) => None,
        };
        let data_files = match (&riff, self.find(DATA_FILE_LIST)) {
            (Some(RiffRoot { kind: RiffKind::Root, .. }), Some(list)) => self.data_file_list(&list)?,
            _ => Vec::new(),
        };
        Ok(Layout { entries: self.names.clone(), riff, metadata_xml: self.find(METADATA_XML), textinfo_xml: self.find(TEXTINFO_XML), data_files })
    }

    /// Read a whole entry, failing if it decompresses to more than `limit` bytes. The declared
    /// size is checked first, and the read itself is capped too because the declaration can lie.
    pub fn read_capped(&mut self, name: &str, limit: u64) -> Result<Vec<u8>, Error> {
        let too_large = || Error::TooLarge { entry: name.to_owned(), limit };
        let file = self.zip.by_name(name).map_err(|e| Error::Zip(format!("{name}: {e}")))?;
        if file.size() > limit {
            return Err(too_large());
        }
        let mut out = Vec::with_capacity(usize::try_from(file.size()).unwrap_or(0));
        file.take(limit.saturating_add(1)).read_to_end(&mut out).map_err(|e| Error::Zip(format!("{name}: {e}")))?;
        if u64::try_from(out.len()).unwrap_or(u64::MAX) > limit {
            return Err(too_large());
        }
        Ok(out)
    }

    /// Read an XML entry as text, within [`Limits::max_xml_bytes`].
    pub fn read_xml(&mut self, name: &str) -> Result<String, Error> {
        let bytes = self.read_capped(name, self.limits.max_xml_bytes)?;
        String::from_utf8(bytes).map_err(|e| Error::Xml { entry: name.to_owned(), message: e.to_string() })
    }

    /// The format version in a RIFF root's header, or `None` when the header isn't CorelDRAW's.
    pub fn riff_version(&mut self, name: &str) -> Result<Option<u32>, Error> {
        let file = self.zip.by_name(name).map_err(|e| Error::Zip(format!("{name}: {e}")))?;
        let mut header = Vec::with_capacity(12);
        file.take(12).read_to_end(&mut header).map_err(|e| Error::Zip(format!("{name}: {e}")))?;
        Ok(match crate::riff::identify(&header) {
            Some(crate::riff::FileKind::Cdr(v)) => Some(v.0),
            _ => None,
        })
    }

    fn data_file_list(&mut self, name: &str) -> Result<Vec<String>, Error> {
        let bytes = self.read_capped(name, self.limits.max_xml_bytes)?;
        let mut out = Vec::new();
        // One name per line; libcdr keeps a trailing name without a newline too.
        for line in bytes.split(|&b| b == b'\n') {
            let line = String::from_utf8_lossy(line);
            let line = line.trim_end_matches('\r');
            if line.is_empty() {
                continue;
            }
            if out.len() >= self.limits.max_data_files {
                return Err(Error::Limit { what: "data files", limit: self.limits.max_data_files });
            }
            out.push(line.to_owned());
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_zip;

    #[test]
    fn lookup_ignores_case_and_prefers_riff_data() {
        let bytes = test_zip::build(&[("Content/RIFFDATA.cdr", b""), ("content/root.dat", b""), ("METADATA/metadata.xml", b"")]);
        let mut p = Package::open(&bytes, Limits::default()).unwrap();
        let layout = p.layout().unwrap();
        assert_eq!(layout.riff, Some(RiffRoot { path: "Content/RIFFDATA.cdr".into(), kind: RiffKind::RiffData }));
        assert_eq!(layout.metadata_xml.as_deref(), Some("METADATA/metadata.xml"));
        assert_eq!(layout.entries.len(), 3);
    }

    #[test]
    fn data_file_list_is_capped() {
        let bytes = test_zip::build(&[("content/root.dat", b""), ("content/dataFileList.dat", b"a\r\nb\nc")]);
        let mut p = Package::open(&bytes, Limits::default()).unwrap();
        assert_eq!(p.layout().unwrap().data_files, ["a", "b", "c"]);
        assert_eq!(Layout::data_file_path("a"), "content/data/a");
        let mut p = Package::open(&bytes, Limits { max_data_files: 2, ..Limits::default() }).unwrap();
        assert!(matches!(p.layout(), Err(Error::Limit { what: "data files", .. })));
    }

    #[test]
    fn read_capped_enforces_limit() {
        let bytes = test_zip::build(&[("x", b"12345")]);
        let mut p = Package::open(&bytes, Limits::default()).unwrap();
        assert_eq!(p.read_capped("x", 5).unwrap(), b"12345");
        assert!(matches!(p.read_capped("x", 4), Err(Error::TooLarge { .. })));
        assert!(matches!(p.read_capped("missing", 4), Err(Error::Zip(_))));
    }
}
