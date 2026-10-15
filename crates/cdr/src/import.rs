//! A `.cdr` file to a VectorCraft [`Document`], whichever container it comes in.
//!
//! [`route`] tells the two containers apart by their first bytes, and [`import`] sends each to its
//! hook:
//!
//! - **ZIP package** (CorelDRAW X4 and later, `PK\x03\x04`): [`Package`] finds the RIFF stream
//!   (`content/riffData.cdr` or `content/root.dat`) and the metadata XML, which gives the page
//!   size, title and author.
//! - **Bare RIFF** (CorelDRAW 3 to 13, `RIFF….CDR?`): the bytes are the RIFF stream; the page size
//!   comes from its `mcfg` record.
//!
//! Both then go through the same [`riff::parse`] and [`geometry::translate`], so the two routes
//! produce the same document from the same RIFF stream. CorelDRAW 1–2 files (`WL`) are refused.
//!
//! The document has one artboard per CorelDRAW page (the master page is left out, as libcdr does),
//! laid out left to right, and one layer holding every page's translated paths, type and images
//! (embedded as PNG), bottom first.

use std::collections::BTreeMap;
use std::sync::Arc;

use vectorcraft_color::{Color, Gradient, GradientGeom, GradientKind, GradientPaint, GradientStop, Paint};
use vectorcraft_doc::ColorProfiles;
use vectorcraft_doc::appearance::{Appearance, AppearanceItem, FillLayer};
use vectorcraft_doc::{Artboard, ColorMode, Document, ImageBlob, ImageObject, LayerColor, Node, NodeKind, OpacityMask};
use vectorcraft_geom::{Affine, Rect};

use crate::geometry::{self, Content, PageSpace, Translation};
use crate::riff::{self, CdrVersion, FileKind};
use crate::{Error, Layout, Limits, Metadata, Package, RiffKind};

/// Gap between artboards laid out side by side, in points.
pub const ARTBOARD_GAP: f64 = 36.0;

/// Which hook a file is handled by.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Route {
    /// A ZIP package (X4 and later).
    Package,
    /// A bare RIFF stream (versions 3 to 13).
    Riff(CdrVersion),
}

/// The hook for `bytes`, from their signature.
pub fn route(bytes: &[u8]) -> Result<Route, Error> {
    if bytes.starts_with(b"PK\x03\x04") || bytes.starts_with(b"PK\x05\x06") {
        return Ok(Route::Package);
    }
    match riff::identify(bytes) {
        Some(FileKind::Cdr(v)) => Ok(Route::Riff(v)),
        Some(FileKind::Waldo) => Err(Error::Riff("CorelDRAW 1–2 (WL) files are not supported".into())),
        Some(FileKind::OtherRiff(t)) => Err(Error::Riff(format!("RIFF form type {t} is not CorelDRAW"))),
        None => Err(Error::NotCdr),
    }
}

/// Is `bytes` a CorelDRAW file this reader routes: a bare `RIFF….CDR?` stream, or a ZIP package
/// holding `content/riffData.cdr` or `content/root.dat`? Other ZIP archives (Office documents,
/// fonts, plain archives) are not, so a caller can sniff by content before the extension.
pub fn is_cdr(bytes: &[u8]) -> bool {
    match route(bytes) {
        Ok(Route::Riff(_)) => true,
        Ok(Route::Package) => Package::open(bytes, Limits::default()).and_then(|mut p| p.layout()).is_ok_and(|l| l.riff.is_some()),
        Err(_) => false,
    }
}

/// Where the RIFF stream came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    /// The package entry it was read from, and which kind of root that is.
    Package { entry: String, kind: RiffKind },
    /// The file itself.
    Riff,
}

/// An imported document and how it was read.
#[derive(Debug, Clone)]
pub struct Imported {
    pub document: Document,
    pub route: Route,
    pub source: Source,
    /// The RIFF stream's version.
    pub version: CdrVersion,
    /// The package's metadata (ZIP route only).
    pub metadata: Option<Metadata>,
    /// Per-page translation results, including notes, skipped objects and record errors.
    pub translation: Translation,
    /// What the document leaves out or approximates, each line once.
    pub warnings: Vec<String>,
    /// The colour profiles the package carries for the document's RGB and CMYK spaces, by the
    /// names [`Document::color_profiles`] gives them (ICC files; sRGB is never listed, it is
    /// VectorCraft's own working space). The caller makes them available before using the names.
    pub profiles: BTreeMap<String, Vec<u8>>,
}

/// Import a `.cdr` file held in memory.
pub fn import(bytes: &[u8], limits: Limits) -> Result<Imported, Error> {
    let r = route(bytes)?;
    match r {
        Route::Package => {
            let doc = crate::Document::open(bytes, limits)?;
            let riff_root = doc.layout.riff.clone().ok_or(Error::NotCdr)?;
            let mut package = Package::open(bytes, limits)?;
            let stream = package.read_capped(&riff_root.path, limits.max_inflated_bytes)?;
            // X6+: records too large for root.dat live in content/data files, which stubs name by
            // their place in the list (a file that can't be read leaves its records unread).
            let mut budget = limits.max_inflated_bytes;
            let mut streams = Vec::with_capacity(doc.layout.data_files.len());
            for name in &doc.layout.data_files {
                let bytes = package.read_capped(&Layout::data_file_path(name), budget).unwrap_or_default();
                budget = budget.saturating_sub(bytes.len() as u64);
                streams.push(bytes);
            }
            let tree = riff::parse(&stream, &limits)?;
            let page = doc.metadata.as_ref().and_then(|m| m.page.as_ref()).map(PageSpace::from_metadata);
            let translation = geometry::translate_with(&tree, page, &streams);
            let layer = match doc.layer_names() {
                [one] => Some(one.clone()),
                _ => None,
            };
            let source = Source::Package { entry: riff_root.path, kind: riff_root.kind };
            let (names, profiles) = package_profiles(&mut package);
            let mut imported = assemble(r, source, tree.version, doc.metadata, translation, layer);
            imported.document.color_profiles = names;
            imported.profiles = profiles;
            Ok(imported)
        }
        Route::Riff(_) => {
            let tree = riff::parse(bytes, &limits)?;
            let translation = geometry::translate(&tree, None);
            Ok(assemble(r, Source::Riff, tree.version, None, translation, None))
        }
    }
}

/// Largest colour profile read from a package.
const MAX_PROFILE_BYTES: u64 = 4 << 20;

/// The document's RGB and CMYK profiles: their names from `color/color.xml`
/// (`<ColorProfile id="Cmyk">ISO Coated v2 (ECI)</ColorProfile>`) and their ICC files from
/// `color/profiles/rgb/` and `color/profiles/cmyk/`. sRGB, and a profile without its file, are left
/// to VectorCraft's working space.
fn package_profiles(package: &mut Package<'_>) -> (ColorProfiles, BTreeMap<String, Vec<u8>>) {
    let mut names = ColorProfiles::default();
    let mut files = BTreeMap::new();
    let Ok(xml) = package.read_xml("color/color.xml") else { return (names, files) };
    for (id, dir) in [("Rgb", "rgb"), ("Cmyk", "cmyk")] {
        let open = format!("<ColorProfile id=\"{id}\">");
        let Some(name) = xml.split_once(&open).and_then(|(_, rest)| rest.split_once("</ColorProfile>")).map(|(n, _)| n.trim().to_owned()) else {
            continue;
        };
        if name.is_empty() || name.len() > 200 || (id == "Rgb" && name.to_ascii_lowercase().starts_with("srgb")) {
            continue;
        }
        let prefix = format!("color/profiles/{dir}/");
        let Some(entry) = package.entries().iter().find(|e| e.starts_with(&prefix) && e.len() > prefix.len()).cloned() else { continue };
        let Ok(icc) = package.read_capped(&entry, MAX_PROFILE_BYTES) else { continue };
        match id {
            "Rgb" => names.rgb = Some(name.clone()),
            _ => names.cmyk = Some(name.clone()),
        }
        files.insert(name, icc);
    }
    (names, files)
}

fn assemble(
    route: Route,
    source: Source,
    version: CdrVersion,
    metadata: Option<Metadata>,
    translation: Translation,
    layer: Option<String>,
) -> Imported {
    let mut warnings: Vec<String> = Vec::new();
    let pages: Vec<_> = translation.pages.iter().filter(|p| !p.master).collect();
    let cmyk = pages
        .iter()
        .flat_map(|p| p.shapes())
        .flat_map(|s| &s.appearance.items)
        .any(|i| matches!(i.paint(), vectorcraft_color::Paint::Solid { color: vectorcraft_color::Color::Cmyk { .. }, .. }));
    let mut doc = Document::new_with_mode(1.0, 1.0, if cmyk { ColorMode::Cmyk } else { ColorMode::Rgb });
    doc.artboards.clear();
    doc.layers.clear();
    if let Some(m) = &metadata {
        if let Some(t) = &m.title {
            doc.title = t.clone();
        }
        if let Some(a) = &m.author {
            doc.metadata.author = a.clone();
        }
    }
    // VectorCraft layers by CorelDRAW layer name, bottom first; each page's layer of a name adds to
    // the same one (CorelDRAW layers span the pages).
    let mut layers: Vec<(String, Vec<Arc<Node>>)> = Vec::new();
    let fallback = layer.unwrap_or_else(|| "Layer 1".into());
    let mut x = 0.0;
    for (n, p) in pages.iter().enumerate() {
        let (w, h) = p.page.size_pt();
        let (w, h) = (finite_side(w), finite_side(h));
        doc.artboards.push(Artboard {
            id: u32::try_from(n + 1).unwrap_or(u32::MAX),
            name: format!("Page {}", n + 1),
            rect: Rect::new(x, 0.0, x + w, h),
            show_center_mark: false,
            show_cross_hairs: false,
        });
        let shift = Affine::translate((x, 0.0));
        for l in p.layers.iter().filter(|l| l.kind.is_art()) {
            let name = l.name.clone().unwrap_or_else(|| fallback.clone());
            let nodes: Vec<Arc<Node>> = l.shapes.iter().filter_map(|s| build(&mut doc, s, shift, &mut warnings)).map(Arc::new).collect();
            if nodes.is_empty() {
                continue;
            }
            match layers.iter_mut().find(|(n, _)| *n == name) {
                Some((_, children)) => children.extend(nodes),
                None => layers.push((name, nodes)),
            }
        }
        x += w + ARTBOARD_GAP;
    }
    if doc.artboards.is_empty() {
        // No page: one artboard of the document's page size (or a Letter page), so the file opens.
        let (w, h) = translation.pages.first().map_or((612.0, 792.0), |p| p.page.size_pt());
        doc.artboards.push(Artboard {
            id: 1,
            name: "Page 1".into(),
            rect: Rect::new(0.0, 0.0, finite_side(w), finite_side(h)),
            show_center_mark: false,
            show_cross_hairs: false,
        });
        warn(&mut warnings, "the file has no pages".into());
    }
    let master_shapes = translation.pages.iter().filter(|p| p.master).map(|p| p.leaves().len()).sum::<usize>();
    if master_shapes > 0 {
        warn(&mut warnings, format!("{master_shapes} object(s) on the master page left out"));
    }
    for (kind, n) in &translation.skipped {
        warn(&mut warnings, format!("{n} {kind} object(s) not translated yet"));
    }
    for e in &translation.errors {
        warn(&mut warnings, format!("unreadable record: {e}"));
    }
    if layers.is_empty() {
        layers.push((fallback, Vec::new()));
    }
    add_tile_patterns(&mut doc, &translation, &layers, &mut warnings);
    for (i, (name, children)) in layers.into_iter().enumerate() {
        let id = doc.alloc_id();
        let mut node = Node::layer(id, &name, LayerColor::Preset(u8::try_from(i % 27).unwrap_or(0)));
        if let Some(c) = node.children_mut() {
            *c = children;
        }
        doc.layers.push(Arc::new(node));
    }
    Imported { document: doc, route, source, version, metadata, translation, warnings, profiles: BTreeMap::new() }
}

/// A pattern swatch for each bitmap or texture fill the art (or its type) uses: one tile, the
/// fill's bitmap at the tile's size.
fn add_tile_patterns(doc: &mut Document, translation: &Translation, layers: &[(String, Vec<Arc<Node>>)], warnings: &mut Vec<String>) {
    fn used(n: &Node, out: &mut std::collections::BTreeSet<String>) {
        for item in &n.appearance.items {
            if let AppearanceItem::Fill(f) = item
                && let Paint::Pattern { pattern, .. } = &f.paint
            {
                out.insert(pattern.clone());
            }
        }
        if let NodeKind::Text(t) = &n.kind {
            for r in &t.runs {
                if let Paint::Pattern { pattern, .. } = &r.style.fill {
                    out.insert(pattern.clone());
                }
            }
        }
        for c in n.children().into_iter().flatten() {
            used(c, out);
        }
    }
    let mut names = std::collections::BTreeSet::new();
    for n in layers.iter().flat_map(|(_, nodes)| nodes) {
        used(n, &mut names);
    }
    for name in names {
        let Some(tile) = translation.tiles.get(&name) else { continue };
        let png = match tile.bitmap.to_png() {
            Ok(png) => png,
            Err(e) => {
                warn(warnings, format!("a bitmap could not be stored: {e}"));
                continue;
            }
        };
        let (w, h) = (tile.bitmap.width.max(1), tile.bitmap.height.max(1));
        let blob = ImageBlob::new("image/png", png);
        let key = blob.content_key();
        doc.images.insert(key.clone(), blob);
        let xf = Affine::scale_non_uniform(tile.width_pt / f64::from(w), tile.height_pt / f64::from(h));
        let image = ImageObject { key, width: w, height: h, xf, link: None, placement: Default::default() };
        let art = Node::new(doc.alloc_id(), NodeKind::Image(Box::new(image)));
        let mut def = vectorcraft_doc::pattern::PatternDef::new(&name, vec![Arc::new(art)]);
        def.tile = Rect::new(0.0, 0.0, tile.width_pt, tile.height_pt);
        doc.patterns.push(def);
    }
}

fn warn(warnings: &mut Vec<String>, w: String) {
    if !warnings.contains(&w) {
        warnings.push(w);
    }
}

/// The document node for a translated shape (`None` when it draws nothing), moved by `shift`
/// onto its artboard.
fn build(doc: &mut Document, s: &geometry::Shape, shift: Affine, warnings: &mut Vec<String>) -> Option<Node> {
    s.notes.iter().for_each(|n| warn(warnings, n.clone()));
    let id = doc.alloc_id();
    let mut node = match &s.content {
        // Nothing to draw (a bitmap that wasn't found: its note says so).
        Content::Path { path, .. } if path.is_empty() => return None,
        Content::Image { bitmap, xf } => match bitmap.to_png() {
            Ok(png) => {
                let blob = ImageBlob::new("image/png", png);
                let key = blob.content_key();
                doc.images.insert(key.clone(), blob);
                let image =
                    ImageObject { key, width: bitmap.width, height: bitmap.height, xf: shift * *xf, link: None, placement: Default::default() };
                Node::new(id, NodeKind::Image(Box::new(image)))
            }
            Err(e) => {
                warn(warnings, format!("a bitmap could not be stored: {e}"));
                return None;
            }
        },
        Content::Path { path, rule } => {
            let mut node = Node::path(id, path.transformed(shift), s.appearance.clone());
            if let NodeKind::Path { rule: r, .. } = &mut node.kind {
                *r = *rule;
            }
            // Bitmap and texture fills: their tiles move with the art.
            vectorcraft_doc::pattern::transform_pattern_paints(&mut node, shift);
            node
        }
        Content::Text(t) => {
            let mut t = t.as_ref().clone();
            t.xf = shift * t.xf;
            Node::new(id, NodeKind::Text(Box::new(t)))
        }
        Content::Group { children, clip } => {
            let kids: Vec<Arc<Node>> = children.iter().filter_map(|c| build(doc, c, shift, warnings)).map(Arc::new).collect();
            // A clip group needs its clipping path and something to clip.
            if kids.is_empty() || (*clip && kids.len() < 2) {
                return None;
            }
            let mut g = Node::group(id, kids);
            if let NodeKind::Group { clip: c, .. } = &mut g.kind {
                *c = *clip;
            }
            g
        }
    };
    node.opacity = s.opacity.clamp(0.0, 1.0);
    node.name = s.name.clone();
    // Type under an envelope: a live envelope whose shape is CorelDRAW's outline, the type inside
    // it as stored (undistorted).
    if let Some(path) = &s.envelope {
        let envelope = NodeKind::Envelope {
            content: vec![Arc::new(node)],
            kind: Box::new(vectorcraft_doc::live::EnvelopeKind::TopObject { path: path.transformed(shift) }),
            fidelity: vectorcraft_doc::live::default_fidelity(),
            editing: false,
            options: vectorcraft_doc::live::EnvelopeOptions::NEW,
            frame: Box::new(Affine::IDENTITY),
        };
        node = Node::new(doc.alloc_id(), envelope);
    }
    // A fading transparency becomes an opacity mask: a box over the object with a grey ramp,
    // instead of the fade's average opacity.
    if let Some(mask) = s.fade.and_then(|f| fade_mask(doc, f.transformed(shift), &node)) {
        node.opacity = 1.0;
        node = node.with_mask(Some(Box::new(mask)));
    }
    Some(node)
}

/// An opacity mask for `fade` over `node`: a rectangle a little larger than the node, filled
/// with a linear ramp from the fade's first opacity (as grey) to its last.
fn fade_mask(doc: &mut Document, fade: geometry::Fade, node: &Node) -> Option<OpacityMask> {
    let b = node.visual_bounds()?;
    let pad = 1.0 + 0.01 * b.width().max(b.height());
    let b = Rect::new(b.x0 - pad, b.y0 - pad, b.x1 + pad, b.y1 + pad);
    let grey = |o: f32| Color::gray(o.clamp(0.0, 1.0));
    let stops = vec![GradientStop::new(0.0, grey(fade.opacity.0)), GradientStop::new(1.0, grey(fade.opacity.1))];
    let mut paint = GradientPaint::new(Gradient { kind: GradientKind::Linear, stops });
    paint.geom = Some(GradientGeom { start: fade.from, end: fade.to, aspect: 1.0, focal: None });
    let fill = Appearance { items: vec![AppearanceItem::Fill(FillLayer::new(Paint::Gradient(Box::new(paint))))], ..Appearance::default() };
    let art = Node::path(doc.alloc_id(), vectorcraft_geom::shapes::rectangle(b), fill);
    Some(OpacityMask::new(art, true))
}

/// A page side in points VectorCraft accepts: finite and at least 1 pt.
fn finite_side(v: f64) -> f64 {
    if v.is_finite() { v.clamp(1.0, 16_383.0 * 72.0) } else { 612.0 }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::synth as build;
    use crate::test_zip;

    #[test]
    fn enveloped_type_becomes_a_live_envelope() {
        let mut doc = Document::new_with_mode(1.0, 1.0, ColorMode::Rgb);
        let t = vectorcraft_doc::text::TextObject::point(vectorcraft_geom::Point::ZERO, "Moloko", vectorcraft_doc::text::CharStyle::default());
        let outline = vectorcraft_geom::shapes::rectangle(Rect::new(0.0, 0.0, 100.0, 40.0));
        let shape = geometry::Shape { envelope: Some(outline.clone()), ..geometry::Shape::text_for_tests(t) };
        let mut warnings = Vec::new();
        let node = build(&mut doc, &shape, Affine::translate((5.0, 0.0)), &mut warnings).unwrap();
        let NodeKind::Envelope { content, kind, .. } = &node.kind else { panic!("{:?}", node.kind) };
        assert!(matches!(content[0].kind, NodeKind::Text(_)));
        let vectorcraft_doc::live::EnvelopeKind::TopObject { path } = kind.as_ref() else { panic!() };
        assert_eq!(path.bounds(), outline.transformed(Affine::translate((5.0, 0.0))).bounds(), "moved with the art");
    }

    #[test]
    fn tiled_fills_become_pattern_swatches() {
        let mut doc = Document::new_with_mode(1.0, 1.0, ColorMode::Rgb);
        let mut translation = Translation::default();
        let bitmap = std::sync::Arc::new(crate::bitmap::Bitmap { width: 2, height: 2, rgba: vec![255; 16] });
        translation.tiles.insert(geometry::tile_name(5), geometry::Tile { bitmap: bitmap.clone(), width_pt: 36.0, height_pt: 18.0 });
        translation.tiles.insert(geometry::tile_name(6), geometry::Tile { bitmap, width_pt: 9.0, height_pt: 9.0 });
        let fill = Appearance {
            items: vec![AppearanceItem::Fill(FillLayer::new(Paint::Pattern { pattern: geometry::tile_name(5), xf: Affine::IDENTITY }))],
            ..Appearance::default()
        };
        let node = Node::path(doc.alloc_id(), vectorcraft_geom::shapes::rectangle(Rect::new(0.0, 0.0, 10.0, 10.0)), fill);
        let group = Node::group(doc.alloc_id(), vec![Arc::new(node)]);
        let mut warnings = Vec::new();
        add_tile_patterns(&mut doc, &translation, &[("Layer".into(), vec![Arc::new(group)])], &mut warnings);
        // Only the pattern the art uses (also inside a group): its tile, and its bitmap at that size.
        assert_eq!(doc.patterns.len(), 1);
        let p = &doc.patterns[0];
        assert_eq!((p.name.as_str(), p.tile), ("CorelDRAW fill 5", Rect::new(0.0, 0.0, 36.0, 18.0)));
        let NodeKind::Image(img) = &p.art[0].kind else { panic!() };
        assert_eq!((img.width, img.height, img.xf), (2, 2, Affine::scale_non_uniform(18.0, 9.0)));
        assert!(doc.images.contains_key(&img.key) && warnings.is_empty());
    }

    #[test]
    fn fades_become_opacity_masks() {
        let mut doc = Document::new_with_mode(1.0, 1.0, ColorMode::Rgb);
        let path = vectorcraft_geom::shapes::rectangle(Rect::new(0.0, 0.0, 10.0, 10.0));
        let fill = Appearance { items: vec![AppearanceItem::Fill(FillLayer::new(Paint::solid(Color::BLACK)))], ..Appearance::default() };
        let fade = geometry::Fade { from: vectorcraft_geom::Point::new(0.0, 0.0), to: vectorcraft_geom::Point::new(10.0, 0.0), opacity: (1.0, 0.0) };
        let shape = geometry::Shape {
            content: Content::Path { path, rule: vectorcraft_geom::FillRule::EvenOdd },
            opacity: 0.5,
            name: None,
            fade: Some(fade),
            envelope: None,
            appearance: fill,
            notes: Vec::new(),
        };
        let mut warnings = Vec::new();
        let node = build(&mut doc, &shape, Affine::translate((5.0, 0.0)), &mut warnings).unwrap();
        // The mask replaces the fade's average opacity.
        assert_eq!(node.opacity, 1.0);
        let mask = node.extras().mask.as_deref().unwrap();
        assert!(mask.clip && !mask.invert);
        let bounds = mask.art.geometric_bounds().unwrap();
        assert!(bounds.x0 < 5.0 && bounds.x1 > 15.0, "{bounds:?}");
        let Paint::Gradient(g) = mask.art.appearance.fill_paint() else { panic!() };
        let geom = g.geom.unwrap();
        assert_eq!((geom.start.x, geom.end.x), (5.0, 15.0), "moved with the object");
        assert_eq!(g.gradient.stops.iter().map(|s| s.color).collect::<Vec<_>>(), [Color::gray(1.0), Color::gray(0.0)]);
        // A uniform transparency stays the node's opacity.
        let plain = geometry::Shape { fade: None, ..shape };
        assert_eq!(build(&mut doc, &plain, Affine::IDENTITY, &mut warnings).unwrap().opacity, 0.5);
    }

    #[test]
    fn package_colour_profiles_by_name() {
        let xml = br#"<Color><ColorContext><ColorProfile id="Rgb">sRGB IEC61966-2.1</ColorProfile><ColorProfile id="Cmyk">Press Coated</ColorProfile><ColorProfile id="Grayscale">Gray Gamma 2.2</ColorProfile></ColorContext></Color>"#;
        let riff = build::triangle_riff();
        let file = build::package_with(
            &riff,
            "t",
            &[("color/color.xml", xml), ("color/profiles/cmyk/press.icc", b"ICC"), ("color/profiles/rgb/srgb.icm", b"RGB")],
        );
        let i = import(&file, Limits::default()).unwrap();
        // sRGB is the working space already; the CMYK profile is named as the settings name it.
        assert_eq!(i.document.color_profiles, ColorProfiles { rgb: None, cmyk: Some("Press Coated".into()) });
        assert_eq!(i.profiles.into_iter().collect::<Vec<_>>(), [("Press Coated".to_owned(), b"ICC".to_vec())]);
        // A named profile without its file is left out.
        let file = build::package_with(&riff, "t", &[("color/color.xml", xml)]);
        let i = import(&file, Limits::default()).unwrap();
        assert!(i.document.color_profiles.is_empty() && i.profiles.is_empty());
        assert!(import(&build::package(&riff, "t"), Limits::default()).unwrap().profiles.is_empty());
    }

    #[test]
    fn routes_by_signature() {
        assert_eq!(route(b"PK\x03\x04rest"), Ok(Route::Package));
        assert_eq!(route(b"RIFF\0\0\0\0CDR9"), Ok(Route::Riff(CdrVersion(900))));
        assert_eq!(route(b"RIFF\0\0\0\0cdrD"), Ok(Route::Riff(CdrVersion(1300))));
        assert!(matches!(route(b"WL\x01\x02"), Err(Error::Riff(_))));
        assert!(matches!(route(b"RIFF\0\0\0\0CMX1"), Err(Error::Riff(_))));
        assert_eq!(route(b"%PDF-1.7"), Err(Error::NotCdr));
        assert_eq!(route(b""), Err(Error::NotCdr));
    }

    #[test]
    fn both_routes_reach_the_same_translation() {
        // A version 9 file with a document page size and one empty page.
        let mut mcfg = vec![0u8; 4];
        mcfg.extend_from_slice(&(2 * 254_000i32).to_le_bytes());
        mcfg.extend_from_slice(&(3 * 254_000i32).to_le_bytes());
        let riff_bytes = build::list(
            b"RIFF",
            b"CDR9",
            &[build::list(b"LIST", b"doc ", &[build::chunk(b"mcfg", &mcfg)]), build::list(b"LIST", b"page", &[build::chunk(b"flgs", &[0; 4])])],
        );
        let legacy = import(&riff_bytes, Limits::default()).unwrap();
        assert_eq!((legacy.route, &legacy.source), (Route::Riff(CdrVersion(900)), &Source::Riff));
        let zipped = test_zip::build(&[("content/riffData.cdr", &riff_bytes)]);
        let modern = import(&zipped, Limits::default()).unwrap();
        assert_eq!(modern.route, Route::Package);
        assert_eq!(modern.source, Source::Package { entry: "content/riffData.cdr".into(), kind: RiffKind::RiffData });
        assert_eq!(legacy.translation, modern.translation);
        assert_eq!(legacy.document.artboards, modern.document.artboards);
        assert_eq!(legacy.document.artboards[0].rect, Rect::new(0.0, 0.0, 144.0, 216.0));
    }

    #[test]
    fn sniffs_only_corel_containers() {
        let riff = crate::synth::triangle_riff();
        assert!(is_cdr(&riff) && is_cdr(&crate::synth::package(&riff, "t")));
        assert!(!is_cdr(&test_zip::build(&[("word/document.xml", b"<w/>")])));
        assert!(!is_cdr(b"RIFF\x04\x00\x00\x00WEBP") && !is_cdr(b"WL\x01\x02") && !is_cdr(b"PK\x03\x04 broken"));
    }

    #[test]
    fn x6_records_are_followed_into_their_data_files() {
        let i = import(&crate::synth::x6_package(), Limits::default()).unwrap();
        assert_eq!((i.version, &i.source), (CdrVersion(1600), &Source::Package { entry: "content/root.dat".into(), kind: RiffKind::Root }));
        assert!(i.translation.errors.is_empty(), "{:?}", i.translation.errors);
        let shapes = i.translation.pages[0].leaves();
        assert_eq!(shapes.len(), 2);
        let text = shapes[0].text().unwrap();
        assert_eq!(text.plain_text(), "Ünïcode");
        assert_eq!((text.runs[0].style.font_family.as_str(), text.runs[0].style.size.round()), ("Verdana", 72.0));
        assert_eq!(shapes[1].path().unwrap().anchor_count(), 3);
        assert_eq!(shapes[1].appearance.fill_paint(), vectorcraft_color::Paint::solid(vectorcraft_color::Color::rgb8(255, 0, 0)));
        // Without its data file, the stubs can't be read, and say so.
        let mut z = zip::ZipArchive::new(std::io::Cursor::new(crate::synth::x6_package())).unwrap();
        let root = std::io::Read::bytes(z.by_name("content/root.dat").unwrap()).map(Result::unwrap).collect::<Vec<u8>>();
        let without = test_zip::build(&[("content/root.dat", &root), ("content/dataFileList.dat", b"a.dat\n")]);
        let i = import(&without, Limits::default()).unwrap();
        assert!(i.translation.errors.iter().any(|e| e.contains("data file 0")), "{:?}", i.translation.errors);
        assert!(i.translation.pages[0].leaves().into_iter().all(|s| s.path().is_none()), "only the text object, without its text");
    }

    #[test]
    fn package_without_a_riff_stream_is_refused() {
        let only_meta = test_zip::build(&[("metadata/metadata.xml", b"<x/>")]);
        assert_eq!(import(&only_meta, Limits::default()).err(), Some(Error::NotCdr));
    }
}
