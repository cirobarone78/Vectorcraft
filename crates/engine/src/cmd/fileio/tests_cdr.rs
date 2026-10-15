//! CorelDRAW `.cdr` files through every way in: detection by content and by name, File › Open
//! (`document.open`), Place, New from Template, and the format listing. Both containers (X4+ ZIP
//! packages and version 3–13 RIFF files) reach the same document. Inputs are synthetic
//! (`cdr::synth`), not files saved by CorelDRAW.

use super::*;
use serde_json::json;
use vectorcraft_doc::Document;

fn riff() -> Vec<u8> {
    cdr::synth::triangle_riff()
}

fn package() -> Vec<u8> {
    cdr::synth::package(&riff(), "Triangle")
}

fn b64(name: &str, bytes: &[u8]) -> Value {
    json!({"name": name, "dataBase64": vectorcraft_format::base64_encode(bytes)})
}

fn paths(doc: &Document) -> usize {
    doc.layers.iter().flat_map(|l| l.children().into_iter().flatten()).filter(|n| matches!(n.kind, vectorcraft_doc::NodeKind::Path { .. })).count()
}

#[test]
fn detected_by_content_whatever_the_name() {
    for (name, bytes) in [("x.cdr", riff()), ("x.cdr", package()), ("renamed.bin", riff()), ("renamed.zip", package()), ("noext", package())] {
        assert_eq!(detect(name, &bytes).map(|f| f.id), Some("cdr"), "{name}");
    }
    // Other ZIP archives and RIFF files keep their own formats (or none).
    let docx = cdr::synth::zip(&[("word/document.xml", b"<w/>")]);
    assert_eq!(detect("letter.docx", &docx).map(|f| f.id), None);
    let webp = image::RgbaImage::new(1, 1);
    let mut wb = Vec::new();
    webp.write_to(&mut std::io::Cursor::new(&mut wb), image::ImageFormat::WebP).unwrap();
    assert_eq!(detect("x.webp", &wb).map(|f| f.id), Some("webp"));
    // By name alone (bytes that say nothing), so Open reports the reader's own reason.
    assert_eq!(detect("old.cdr", b"WL\x01\x02").map(|f| f.id), Some("cdr"));
}

#[test]
fn listed_as_a_readable_document_format() {
    let f = format("cdr").unwrap();
    assert_eq!((f.label, f.mime, f.read, f.write, f.raster), ("CorelDRAW Drawing", "application/vnd.corel-draw", true, false, false));
    assert!(OPEN_EXTS.contains(&"cdr") && PLACE_EXTS.contains(&"cdr"));
    assert!(open_filters().any(|(label, exts)| label == "CorelDRAW Drawing" && exts == ["cdr"]));
    assert!(place_filters().any(|(label, exts)| label == "CorelDRAW Drawing" && exts == ["cdr"]));
    assert_eq!(CDR_MIME_ALIASES, ["application/x-cdr", "image/x-cdr"]);
    let mut s = Session::new();
    let listed = s.execute("document.formats", &json!({})).unwrap();
    let cdr = listed["formats"].as_array().unwrap().iter().find(|f| f["id"] == "cdr").unwrap();
    assert_eq!((cdr["extensions"].clone(), cdr["mime"].clone()), (json!(["cdr"]), json!("application/vnd.corel-draw")));
}

#[test]
fn both_containers_open_to_the_same_art() {
    let legacy = load("legacy.cdr", &riff()).unwrap();
    let modern = load("modern.cdr", &package()).unwrap();
    for l in [&legacy, &modern] {
        assert_eq!(l.format.id, "cdr");
        assert_eq!(l.doc.artboards.len(), 1);
        assert_eq!(l.doc.artboards[0].rect, vectorcraft_geom::Rect::new(0.0, 0.0, 288.0, 216.0));
        assert_eq!(paths(&l.doc), 1);
        assert!(l.warnings.iter().any(|w| w.contains("Rectangle")), "{:?}", l.warnings);
    }
    assert_eq!(legacy.doc.layers, modern.doc.layers);
    // Imports are named after the file.
    assert_eq!((legacy.doc.title.as_str(), modern.doc.title.as_str()), ("legacy.cdr", "modern.cdr"));
}

#[test]
fn document_open_makes_it_the_active_document_without_a_save_path() {
    let mut s = Session::new();
    let r = s.execute("document.open", &b64("drawing.cdr", &package())).unwrap();
    assert_eq!(r["format"], "cdr");
    assert!(r["warnings"].as_array().unwrap().iter().any(|w| w.as_str().unwrap().contains("Rectangle")));
    let st = s.doc().unwrap();
    assert_eq!(paths(&st.doc), 1);
    // CorelDRAW files are read, never written: Save asks for a new file.
    assert!(st.path.is_none());
    for format in ["svg", "pdf", "png"] {
        assert!(s.execute("document.export", &json!({"format": format})).is_ok(), "{format}");
    }
}

#[test]
fn placed_and_opened_as_a_template() {
    let mut s = Session::new();
    s.execute("file.new", &json!({"width": 400, "height": 400})).unwrap();
    s.execute("file.place", &b64("drawing.cdr", &riff())).unwrap();
    assert!(s.doc().unwrap().doc.layers.iter().any(|l| !l.children().unwrap().is_empty()));
    s.execute("file.newFromTemplate", &b64("drawing.cdr", &package())).unwrap();
    assert_eq!(paths(&s.doc().unwrap().doc), 1);
}

#[test]
fn unreadable_corel_files_say_why_and_keep_the_active_document() {
    let mut s = Session::new();
    s.execute("file.new", &json!({"width": 100, "height": 100})).unwrap();
    let before = s.documents().len();
    let e = s.execute("document.open", &b64("ancient.cdr", b"WL\x01\x02 rest")).unwrap_err().to_string();
    assert!(e.contains("ancient.cdr") && e.contains("CorelDRAW 1"), "{e}");
    let mut broken = package();
    broken.truncate(broken.len() / 2);
    assert!(s.execute("document.open", &b64("broken.cdr", &broken)).is_err());
    assert_eq!(s.documents().len(), before);
}

#[test]
fn rectangles_ellipses_and_type_open_as_editable_objects() {
    let mut s = Session::new();
    let r = s.execute("document.open", &b64("shapes.cdr", &cdr::synth::shapes_riff())).unwrap();
    assert!(r["warnings"].as_array().unwrap().is_empty(), "{r}");
    let doc = &s.doc().unwrap().doc;
    let kids = doc.layers[0].children().unwrap();
    let kinds: Vec<_> = kids.iter().map(|n| n.kind_label()).collect();
    assert_eq!(kinds, ["Type", "Type", "Path", "Path"], "bottom first: paragraph text, artistic text, pie, rectangle");
    let vectorcraft_doc::NodeKind::Text(t) = &kids[1].kind else { panic!() };
    assert_eq!(t.plain_text(), "Hello wörld");
    // Opened type has its layout (bounds) like any other type.
    assert!(t.cached_bounds.is_some());
    for format in ["svg", "pdf"] {
        assert!(s.execute("document.export", &json!({"format": format})).is_ok(), "{format}");
    }
}

#[test]
fn polygons_splines_bitmaps_and_x6_packages_open() {
    let mut s = Session::new();
    s.execute("document.open", &b64("more.cdr", &cdr::synth::more_shapes_riff())).unwrap();
    let doc = &s.doc().unwrap().doc;
    let kids = doc.layers[0].children().unwrap();
    assert_eq!(kids.len(), 3);
    let vectorcraft_doc::NodeKind::Image(img) = &kids[0].kind else { panic!("{:?}", kids[0].kind) };
    assert_eq!((img.width, img.height), (2, 2));
    assert_eq!(doc.images.get(&img.key).map(|b| b.mime.as_str()), Some("image/png"));
    assert!(kids[1..].iter().all(|n| matches!(n.kind, vectorcraft_doc::NodeKind::Path { .. })));
    for format in ["svg", "pdf", "png"] {
        assert!(s.execute("document.export", &json!({"format": format})).is_ok(), "{format}");
    }
    let r = s.execute("document.open", &b64("x6.cdr", &cdr::synth::x6_package())).unwrap();
    assert_eq!(r["format"], "cdr");
    let kids = s.doc().unwrap().doc.layers[0].children().unwrap().clone();
    assert!(kids.iter().any(|n| matches!(&n.kind, vectorcraft_doc::NodeKind::Text(t) if t.plain_text() == "Ünïcode")));
}

#[test]
fn a_package_carries_its_cmyk_profile() {
    use vectorcraft_color::cms;
    let xml = |name: &str| format!(r#"<Color><ColorContext><ColorProfile id="Cmyk">{name}</ColorProfile></ColorContext></Color>"#);
    let icc = cms::icc_bytes(cms::GENERIC_CMYK).unwrap();
    let ok = xml("CorelDRAW Test Press");
    let file = cdr::synth::package_with(&riff(), "t", &[("color/color.xml", ok.as_bytes()), ("color/profiles/cmyk/press.icc", &icc)]);
    let l = load("press.cdr", &file).unwrap();
    assert_eq!(l.doc.color_profiles.cmyk.as_deref(), Some("CorelDRAW Test Press"));
    assert_eq!(cms::profile("CorelDRAW Test Press").map(|p| p.kind), Some(cms::ProfileKind::Cmyk));
    // A profile that can't be used: said, and the document keeps the working space.
    let bad = xml("CorelDRAW Broken Press");
    let file = cdr::synth::package_with(&riff(), "t", &[("color/color.xml", bad.as_bytes()), ("color/profiles/cmyk/broken.icc", b"not a profile")]);
    let l = load("broken.cdr", &file).unwrap();
    assert_eq!(l.doc.color_profiles.cmyk, None);
    assert!(l.warnings.iter().any(|w| w.contains("CorelDRAW Broken Press")), "{:?}", l.warnings);
}

#[test]
fn enveloped_type_fits_its_ink_to_the_envelope() {
    use vectorcraft_doc::live::{EnvelopeKind, EnvelopeOptions};
    use vectorcraft_doc::text::{CharStyle, TextObject};
    use vectorcraft_doc::{Node, NodeId, NodeKind};
    use vectorcraft_geom::{Point, Rect};
    let t = TextObject::point(Point::new(0.0, 100.0), "Moloko", CharStyle { size: 48.0, ..CharStyle::default() });
    let outline = vectorcraft_geom::shapes::rectangle(Rect::new(0.0, 0.0, 200.0, 50.0));
    let kind = NodeKind::Envelope {
        content: vec![std::sync::Arc::new(Node::new(NodeId(2), NodeKind::Text(Box::new(t))))],
        kind: Box::new(EnvelopeKind::TopObject { path: outline.clone() }),
        fidelity: vectorcraft_doc::live::default_fidelity(),
        editing: false,
        options: EnvelopeOptions::NEW,
        frame: Box::new(vectorcraft_geom::Affine::IDENTITY),
    };
    let mut n = Node::new(NodeId(1), kind);
    super::coreldraw::fit_envelopes_to_ink(&mut n);
    let NodeKind::Envelope { kind, .. } = &n.kind else { panic!() };
    let EnvelopeKind::TopObject { path } = kind.as_ref() else { panic!() };
    let (before, after) = (outline.bounds().unwrap(), path.bounds().unwrap());
    // The layout box holds the letters' ink with room around it: the outline grows to match.
    assert!(after.contains_rect(before) && after.height() > before.height(), "{before:?} → {after:?}");
}
