//! The `test_cdr` harness end to end: each container takes its own route to a document that
//! validates, the same RIFF stream gives the same document either way, and bad input fails cleanly.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use vectorcraft_doc::NodeKind;

const IN: i32 = 254_000;

fn chunk(id: &[u8; 4], data: &[u8]) -> Vec<u8> {
    let mut v = id.to_vec();
    v.extend_from_slice(&u32::try_from(data.len()).unwrap().to_le_bytes());
    v.extend_from_slice(data);
    if data.len() % 2 == 1 {
        v.push(0);
    }
    v
}

fn list(id: &[u8; 4], list_type: &[u8; 4], children: &[Vec<u8>]) -> Vec<u8> {
    let mut body = list_type.to_vec();
    children.iter().for_each(|c| body.extend_from_slice(c));
    chunk(id, &body)
}

fn le(v: &[u32]) -> Vec<u8> {
    v.iter().flat_map(|x| x.to_le_bytes()).collect()
}

/// A version 6+ `loda` with (type, payload) arguments.
fn loda(chunk_type: u32, args: &[(u32, Vec<u8>)]) -> Vec<u8> {
    let n = args.len() as u32;
    let mut at = 20 + 8 * n;
    let mut offsets = Vec::new();
    for (_, p) in args {
        offsets.push(at);
        at += p.len() as u32;
    }
    let mut v = le(&[at, n, 20, 20 + 4 * n, chunk_type]);
    v.extend_from_slice(&le(&offsets));
    args.iter().rev().for_each(|(t, _)| v.extend_from_slice(&t.to_le_bytes()));
    args.iter().for_each(|(_, p)| v.extend_from_slice(p));
    v
}

/// A version 9 drawing: a 4 × 3 in page holding a red, 1 pt blue-outlined triangle and a
/// rectangle (counted, not translated).
fn legacy_riff() -> Vec<u8> {
    let mut red = le(&[1]);
    red.extend_from_slice(&1u16.to_le_bytes());
    red.extend_from_slice(&[0, 0, 5, 0, 0, 0, 0, 0, 0, 0, 0, 0, 255, 0]);
    let mut outl = le(&[2]);
    for x in [0x02u16, 1, 1, 0] {
        outl.extend_from_slice(&x.to_le_bytes());
    }
    outl.extend_from_slice(&((IN / 72) as u32).to_le_bytes());
    outl.extend_from_slice(&100u16.to_le_bytes());
    outl.extend_from_slice(&[0; 6 + 52]);
    outl.extend_from_slice(&[5, 0, 0, 0, 0, 0, 0, 0, 255, 0, 0, 0]);
    outl.extend_from_slice(&[0; 16 + 2 + 22 + 8]);
    let mut tri = 3u16.to_le_bytes().to_vec();
    tri.extend_from_slice(&[0, 0]);
    for (x, y) in [(0, 0), (IN, 0), (0, IN)] {
        tri.extend_from_slice(&x.to_le_bytes());
        tri.extend_from_slice(&y.to_le_bytes());
    }
    tri.extend_from_slice(&[0x00, 0x40, 0x48]);
    let mut mcfg = vec![0; 4];
    mcfg.extend_from_slice(&(4 * IN).to_le_bytes());
    mcfg.extend_from_slice(&(3 * IN).to_le_bytes());
    let obj = list(b"LIST", b"obj ", &[chunk(b"loda", &loda(0x03, &[(0x14, le(&[1])), (0x0a, le(&[2])), (0x1e, tri)]))]);
    let rect = list(b"LIST", b"obj ", &[chunk(b"loda", &loda(0x01, &[]))]);
    list(
        b"RIFF",
        b"CDR9",
        &[
            list(
                b"LIST",
                b"doc ",
                &[chunk(b"mcfg", &mcfg), list(b"LIST", b"filt", &[chunk(b"fild", &red)]), list(b"LIST", b"otlt", &[chunk(b"outl", &outl)])],
            ),
            list(b"LIST", b"page", &[chunk(b"flgs", &le(&[0x0098_0000]))]),
            list(b"LIST", b"page", &[chunk(b"flgs", &[0; 4]), list(b"LIST", b"layr", &[obj, rect])]),
        ],
    )
}

fn package(riff: &[u8]) -> Vec<u8> {
    let meta = r#"<x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
<rdf:Description rdf:about=""><Title>Triangle</Title><ProductName>Test</ProductName>
<PageWidth>1016000</PageWidth><PageHeight>762000</PageHeight></rdf:Description></rdf:RDF></x:xmpmeta>"#;
    let mut w = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let opts = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    for (name, data) in [("content/riffData.cdr", riff), ("metadata/metadata.xml", meta.as_bytes())] {
        w.start_file(name, opts).unwrap();
        w.write_all(data).unwrap();
    }
    w.finish().unwrap().into_inner()
}

fn dir() -> PathBuf {
    let d = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("test_cdr");
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn write(name: &str, bytes: &[u8]) -> PathBuf {
    let p = dir().join(name);
    std::fs::write(&p, bytes).unwrap();
    p
}

fn run(args: &[&std::ffi::OsStr]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_test_cdr")).args(args).output().unwrap()
}

fn run_file(path: &Path) -> (Output, String) {
    let out = run(&[path.as_os_str()]);
    let err = String::from_utf8_lossy(&out.stderr).into_owned();
    (out, err)
}

#[test]
fn legacy_riff_takes_the_riff_route() {
    let (out, err) = run_file(&write("legacy.cdr", &legacy_riff()));
    assert!(out.status.success(), "{err}");
    assert!(err.contains("route: riff (CorelDRAW 9)"), "{err}");
    assert!(err.contains("pages: 1, paths: 1") && err.contains("1 Rectangle object(s) not translated yet"), "{err}");
    assert!(err.contains("ok: round trip is lossless") && err.contains("valid: VectorCraft document"), "{err}");

    let doc = vectorcraft_format::load(&out.stdout).unwrap();
    assert_eq!(doc.artboards.len(), 1);
    assert_eq!(doc.artboards[0].rect, vectorcraft_geom::Rect::new(0.0, 0.0, 288.0, 216.0));
    let layer = &doc.layers[0];
    let [path] = layer.children().unwrap().as_slice() else { panic!() };
    let NodeKind::Path { path: p, rule, .. } = &path.kind else { panic!() };
    assert_eq!(*rule, vectorcraft_geom::FillRule::EvenOdd);
    // (0,0) in at the centre of a 4 × 3 in page is (144, 108) pt; (1,0) in is 72 pt right; (0,1) in is up.
    let pts: Vec<_> = p.subpaths[0].anchors.iter().map(|a| (a.p.x, a.p.y)).collect();
    assert_eq!(pts, [(144.0, 108.0), (216.0, 108.0), (144.0, 36.0)]);
    assert!(p.subpaths[0].closed);
    let ap = &path.appearance;
    assert_eq!(ap.fill_paint(), vectorcraft_color::Paint::solid(vectorcraft_color::Color::rgb8(255, 0, 0)));
    let stroke = ap.stroke().unwrap();
    assert!((stroke.width - 1.0).abs() < 1e-3);
    assert_eq!(stroke.cap, vectorcraft_doc::appearance::LineCap::Round);
}

#[test]
fn zip_package_takes_the_package_route_to_the_same_document() {
    let riff = legacy_riff();
    let (legacy, _) = run_file(&write("same-legacy.cdr", &riff));
    let (out, err) = run_file(&write("modern.cdr", &package(&riff)));
    assert!(out.status.success(), "{err}");
    assert!(err.contains("route: package (content/riffData.cdr, X4/X5 riffData)"), "{err}");
    assert!(err.contains("saved by: Test"), "{err}");
    let modern = vectorcraft_format::load(&out.stdout).unwrap();
    let old = vectorcraft_format::load(&legacy.stdout).unwrap();
    // Same art and artboards; only the package's metadata adds the title.
    assert_eq!(modern.title, "Triangle");
    assert_eq!(modern.artboards, old.artboards);
    assert_eq!(serde_json::to_value(&modern.layers).unwrap(), serde_json::to_value(&old.layers).unwrap());
}

#[test]
fn writes_to_a_file_with_o() {
    let out_path = dir().join("out.vectorcraft");
    let _ = std::fs::remove_file(&out_path);
    let input = write("o.cdr", &legacy_riff());
    let out = run(&[input.as_os_str(), "-o".as_ref(), out_path.as_os_str(), "--compact".as_ref()]);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert!(out.stdout.is_empty());
    let saved = std::fs::read(&out_path).unwrap();
    assert!(!saved.contains(&b'\n'), "--compact writes one line");
    assert_eq!(vectorcraft_format::load(&saved).unwrap().artboards.len(), 1);
}

#[test]
fn refuses_what_it_cannot_route() {
    for (name, bytes, says) in [
        ("waldo.cdr", &b"WL\x01\x02 old"[..], "CorelDRAW 1"),
        ("wave.cdr", b"RIFF\x04\x00\x00\x00WAVE", "WAVE"),
        ("text.cdr", b"hello", "not a CorelDRAW"),
        ("empty-zip.cdr", b"PK\x05\x06\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0", "not a CorelDRAW"),
    ] {
        let (out, err) = run_file(&write(name, bytes));
        assert_eq!(out.status.code(), Some(1), "{name}: {err}");
        assert!(err.contains(says), "{name}: {err}");
        assert!(out.stdout.is_empty());
    }
    let (out, err) = run_file(&dir().join("missing.cdr"));
    assert_eq!(out.status.code(), Some(1), "{err}");
}

#[test]
fn usage_errors() {
    for args in [vec![], vec!["--bogus"], vec!["a.cdr", "b.cdr"], vec!["a.cdr", "-o"]] {
        let out = run(&args.iter().map(std::ffi::OsStr::new).collect::<Vec<_>>());
        assert_eq!(out.status.code(), Some(2), "{args:?}");
        assert!(String::from_utf8_lossy(&out.stderr).contains("usage: test_cdr"));
    }
}

/// The user's CorelDRAW X4 sample, when present (see sample_file.rs).
#[test]
fn x4_sample_validates() {
    let path = std::env::var_os("CDR_SAMPLE")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../libcdr/2019placement.cdr"));
    if !path.exists() {
        eprintln!("skipping: no CorelDRAW sample at {}", path.display());
        return;
    }
    let (out, err) = run_file(&path);
    assert!(out.status.success(), "{err}");
    assert!(err.contains("route: package (content/riffData.cdr, X4/X5 riffData)"), "{err}");
    assert!(err.contains("version: 1400") && err.contains("pages: 1, paths: 9, text: 7"), "{err}");
    let doc = vectorcraft_format::load(&out.stdout).unwrap();
    assert_eq!(doc.artboards[0].rect, vectorcraft_geom::Rect::new(0.0, 0.0, 612.0, 792.0));
    assert_eq!(doc.color_mode, vectorcraft_doc::ColorMode::Cmyk);
    assert_eq!(doc.layers[0].name.as_deref(), Some("Layer 1"));
    assert_eq!(doc.title, "2019placement", "untitled files are named after the file");
}
