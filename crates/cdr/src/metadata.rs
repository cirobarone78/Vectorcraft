//! Typed views of `metadata/metadata.xml` and `metadata/textinfo.xml`.
//!
//! Field names are the ones CorelDRAW writes. The page size and clip rectangle are in CorelDRAW's
//! document unit, a tenth of a micrometre: the X4 sample's Letter page reads `2159000 × 2794000`
//! beside `PageDimensions` "8.5 x 11 in". Every scalar is also kept as text in
//! [`Metadata::fields`] for callers that need something not typed here.

use std::collections::BTreeMap;

use crate::xmp::{self, RawRun};
use crate::{Error, Limits};

/// Document units per millimetre (one unit is 0.1 µm).
const UNITS_PER_MM: f64 = 10_000.0;

/// Page setup recorded in `metadata.xml`.
#[derive(Debug, Clone, PartialEq)]
pub struct PageInfo {
    /// Width in document units (0.1 µm).
    pub width: i64,
    /// Height in document units (0.1 µm).
    pub height: i64,
    /// Preset name, such as "Letter".
    pub size_name: Option<String>,
    /// CorelDRAW's own description, such as "8.5 x 11 in".
    pub dimensions: Option<String>,
}

impl PageInfo {
    pub fn width_mm(&self) -> f64 {
        self.width as f64 / UNITS_PER_MM
    }

    pub fn height_mm(&self) -> f64 {
        self.height as f64 / UNITS_PER_MM
    }

    /// Width in PostScript points (1/72 in).
    pub fn width_pt(&self) -> f64 {
        self.width_mm() * 72.0 / 25.4
    }

    /// Height in PostScript points (1/72 in).
    pub fn height_pt(&self) -> f64 {
        self.height_mm() * 72.0 / 25.4
    }
}

/// Document properties from `metadata/metadata.xml`. Empty fields read as `None`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Metadata {
    pub title: Option<String>,
    pub subject: Option<String>,
    pub author: Option<String>,
    pub last_author: Option<String>,
    pub keywords: Option<String>,
    pub notes: Option<String>,
    pub copyright: Option<String>,
    /// Saving application, such as "CorelDRAW X4".
    pub product_name: Option<String>,
    /// Saving application version times 100 (1400 for X4).
    pub app_version: Option<u32>,
    pub core_version: Option<u32>,
    pub build_number: Option<u32>,
    pub revision: Option<u32>,
    pub doc_id: Option<String>,
    /// Windows locale id, such as 1033 for en-US.
    pub language: Option<u32>,
    pub page_count: Option<u32>,
    pub layer_count: Option<u32>,
    pub page: Option<PageInfo>,
    /// Horizontal and vertical resolution in dpi.
    pub resolution: Option<(u32, u32)>,
    /// `[left, top, right, bottom]` of the drawing's extent, in document units.
    pub clip_rect: Option<[i64; 4]>,
    pub fonts_used: Vec<String>,
    pub styles: Vec<String>,
    /// Object counts by kind from `Objects` (`Total`, `Curve`, `Rect`, `Text`, …), including nested
    /// ones as `Dimension/Linar`.
    pub object_counts: BTreeMap<String, u64>,
    /// Every scalar field by path, such as `Fills/Uniform` or `TextInfo/Words`.
    pub fields: BTreeMap<String, String>,
    /// Every list by path, such as `Fills/Colors`.
    pub lists: BTreeMap<String, Vec<String>>,
}

fn text(fields: &BTreeMap<String, String>, key: &str) -> Option<String> {
    fields.get(key).filter(|v| !v.is_empty()).cloned()
}

fn number<T: std::str::FromStr>(fields: &BTreeMap<String, String>, key: &str) -> Option<T> {
    fields.get(key).and_then(|v| v.parse().ok())
}

impl Metadata {
    pub fn parse(xml: &str, entry: &str, limits: &Limits) -> Result<Self, Error> {
        let x = xmp::parse(xml, entry, limits)?;
        let f = &x.scalars;
        let page = match (number::<i64>(f, "PageWidth"), number::<i64>(f, "PageHeight")) {
            (Some(width), Some(height)) => {
                Some(PageInfo { width, height, size_name: text(f, "PageSizeName"), dimensions: text(f, "PageDimensions") })
            }
            _ => None,
        };
        let resolution = number(f, "ResolutionX").zip(number(f, "ResolutionY"));
        let clip_rect = x.attrs.get("ClipRect").and_then(|attrs| {
            let get = |k: &str| attrs.iter().find(|(n, _)| n == k).and_then(|(_, v)| v.parse::<i64>().ok());
            Some([get("left")?, get("top")?, get("right")?, get("bottom")?])
        });
        let object_counts = f.iter().filter_map(|(k, v)| Some((k.strip_prefix("Objects/")?.to_owned(), v.parse::<u64>().ok()?))).collect();
        Ok(Self {
            title: text(f, "Title"),
            subject: text(f, "Subject"),
            author: text(f, "Author"),
            last_author: text(f, "LastAuthor"),
            keywords: text(f, "Keywords"),
            notes: text(f, "Notes"),
            copyright: text(f, "Copyright"),
            product_name: text(f, "ProductName"),
            app_version: number(f, "AppVersion"),
            core_version: number(f, "CoreVersion"),
            build_number: number(f, "BuildNumber"),
            revision: number(f, "Revision"),
            doc_id: text(f, "DocID"),
            language: number(f, "Language"),
            page_count: number(f, "NumPages"),
            layer_count: number(f, "NumLayers"),
            page,
            resolution,
            clip_rect,
            fonts_used: x.lists.get("FontsUsed").cloned().unwrap_or_default(),
            styles: x.lists.get("Styles").cloned().unwrap_or_default(),
            object_counts,
            fields: x.scalars,
            lists: x.lists,
        })
    }
}

/// How a text run ends, from its `break` attribute.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Break {
    /// `para`: the run ends a paragraph.
    Paragraph,
    /// `line`: the run ends a line inside a paragraph.
    Line,
    /// No `break` attribute.
    None,
    /// Any other value, kept as written.
    Other(String),
}

/// One run of plain text from `textinfo.xml`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextRun {
    pub text: String,
    /// Windows locale id of the run's language.
    pub lang: Option<u32>,
    pub brk: Break,
}

/// The plain text of one text object, as runs.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TextStream {
    pub runs: Vec<TextRun>,
}

impl TextStream {
    /// The runs joined by newlines.
    pub fn text(&self) -> String {
        self.runs.iter().map(|r| r.text.as_str()).collect::<Vec<_>>().join("\n")
    }
}

/// Names and plain text from `metadata/textinfo.xml`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TextInfo {
    /// Layer names in document order, each listed once for the whole document.
    pub layer_names: Vec<String>,
    /// Page names; CorelDRAW leaves the list empty for unnamed pages.
    pub page_names: Vec<String>,
    /// Names of named objects (imported bitmaps carry their file name).
    pub object_names: Vec<String>,
    /// One stream per text object, in document order.
    pub text_streams: Vec<TextStream>,
}

fn run(raw: RawRun) -> TextRun {
    let attr = |k: &str| raw.attrs.iter().find(|(n, _)| n == k).map(|(_, v)| v.as_str());
    let brk = match attr("break") {
        Some("para") => Break::Paragraph,
        Some("line") => Break::Line,
        Some(other) => Break::Other(other.to_owned()),
        None => Break::None,
    };
    let lang = attr("lang").and_then(|v| v.parse().ok());
    TextRun { lang, brk, text: raw.text }
}

impl TextInfo {
    pub fn parse(xml: &str, entry: &str, limits: &Limits) -> Result<Self, Error> {
        let mut x = xmp::parse(xml, entry, limits)?;
        let mut list = |k: &str| x.lists.remove(k).unwrap_or_default();
        Ok(Self {
            layer_names: list("LayerNames"),
            page_names: list("PageNames"),
            object_names: list("ObjectNames"),
            text_streams: x.text_streams.into_iter().map(|s| TextStream { runs: s.runs.into_iter().map(run).collect() }).collect(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn page_units_convert() {
        let p = PageInfo { width: 2_159_000, height: 2_794_000, size_name: None, dimensions: None };
        assert!((p.width_pt() - 612.0).abs() < 1e-9 && (p.height_pt() - 792.0).abs() < 1e-9);
    }

    #[test]
    fn partial_metadata_leaves_fields_absent() {
        let m = Metadata::parse(
            "<rdf:Description><PageWidth>10</PageWidth><ResolutionX>x</ResolutionX><ClipRect left=\"1\"/></rdf:Description>",
            "m",
            &Limits::default(),
        )
        .unwrap();
        assert_eq!((m.page, m.resolution, m.clip_rect, m.app_version), (None, None, None, None));
        assert_eq!(m.fields.get("PageWidth").map(String::as_str), Some("10"));
    }

    #[test]
    fn break_values() {
        let t = TextInfo::parse(
            r#"<rdf:Description><TextData><TextStream><TextRun>a</TextRun><TextRun break="col" lang="x">b</TextRun></TextStream></TextData></rdf:Description>"#,
            "t",
            &Limits::default(),
        )
        .unwrap();
        let runs = &t.text_streams[0].runs;
        assert_eq!((runs[0].brk.clone(), runs[1].brk.clone()), (Break::None, Break::Other("col".into())));
        assert_eq!(runs[1].lang, None);
    }
}
