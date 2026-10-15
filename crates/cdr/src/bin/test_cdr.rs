//! `test_cdr FILE.cdr [-o OUT.vectorcraft] [--compact]`: import a CorelDRAW file with the pure-Rust
//! reader and print the VectorCraft document (`.vectorcraft` JSON) it becomes, after checking it.
//!
//! The document goes to standard output, or to `OUT` with `-o`. A report goes to standard error:
//! the route the file took (`route: package …` for X4+ ZIP packages, `route: riff …` for bare
//! version 3–13 RIFF files), what was translated, the warnings, and each validation check:
//!
//! - the native JSON loads back with `vectorcraft_format::load`;
//! - saving what loaded gives the same bytes (nothing lost on the round trip);
//! - there is at least one artboard, and every artboard and path coordinate is finite;
//! - the document holds one path, type or image node per translated object.
//!
//! Exit status: 0 when the document validates, 1 when the file can't be read or a check fails,
//! 2 for a usage error.

use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;

use cdr::import::{self, Imported, Route, Source};
use cdr::{Limits, RiffKind};
use vectorcraft_doc::{Document, Node, NodeKind};

const USAGE: &str = "usage: test_cdr FILE.cdr [-o OUT.vectorcraft] [--compact]";

struct Args {
    input: PathBuf,
    output: Option<PathBuf>,
    pretty: bool,
}

fn args() -> Result<Args, String> {
    let mut input = None;
    let mut output = None;
    let mut pretty = true;
    let mut it = std::env::args_os().skip(1);
    while let Some(a) = it.next() {
        match a.to_str() {
            Some("-o" | "--out") => output = Some(PathBuf::from(it.next().ok_or("-o needs a path")?)),
            Some("--compact") => pretty = false,
            Some("-h" | "--help") => return Err(String::new()),
            Some(s) if s.starts_with('-') && s.len() > 1 => return Err(format!("unknown option {s}")),
            _ if input.is_none() => input = Some(PathBuf::from(a)),
            _ => return Err("only one input file".into()),
        }
    }
    Ok(Args { input: input.ok_or("no input file")?, output, pretty })
}

fn main() -> ExitCode {
    let args = match args() {
        Ok(a) => a,
        Err(e) => {
            if !e.is_empty() {
                eprintln!("test_cdr: {e}");
            }
            eprintln!("{USAGE}");
            return ExitCode::from(2);
        }
    };
    match run(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("test_cdr: {}: {e}", args.input.display());
            ExitCode::from(1)
        }
    }
}

fn run(args: &Args) -> Result<(), String> {
    let bytes = std::fs::read(&args.input).map_err(|e| e.to_string())?;
    let mut imported = import::import(&bytes, Limits::default()).map_err(|e| e.to_string())?;
    // Untitled in the file: named after it, as an opened document is.
    if imported.metadata.as_ref().and_then(|m| m.title.as_ref()).is_none()
        && let Some(stem) = args.input.file_stem().and_then(|s| s.to_str())
    {
        imported.document.title = stem.to_owned();
    }
    report(&imported);
    let json = vectorcraft_format::save(&imported.document, args.pretty);
    let checks = validate(&imported, &json, args.pretty)?;
    for c in &checks {
        eprintln!("ok: {c}");
    }
    match &args.output {
        Some(path) => std::fs::write(path, &json).map_err(|e| format!("writing {}: {e}", path.display()))?,
        None => {
            use std::io::Write;
            let mut out = std::io::stdout().lock();
            out.write_all(&json).and_then(|()| out.write_all(b"\n")).map_err(|e| e.to_string())?;
        }
    }
    eprintln!("valid: VectorCraft document, {} bytes", json.len());
    Ok(())
}

fn report(i: &Imported) {
    let route = match (&i.route, &i.source) {
        (Route::Package, Source::Package { entry, kind }) => {
            let kind = match kind {
                RiffKind::RiffData => "X4/X5 riffData",
                RiffKind::Root => "X6+ root.dat",
            };
            format!("package ({entry}, {kind})")
        }
        (Route::Riff(v), _) => format!("riff (CorelDRAW {})", v.major()),
        (Route::Package, Source::Riff) => "package".into(),
    };
    eprintln!("route: {route}");
    eprintln!("version: {}", i.version.0);
    if let Some(p) = i.metadata.as_ref().and_then(|m| m.product_name.as_deref()) {
        eprintln!("saved by: {p}");
    }
    let pages: Vec<_> = i.translation.pages.iter().filter(|p| !p.master).collect();
    let leaves: Vec<_> = pages.iter().flat_map(|p| p.leaves()).collect();
    let text = leaves.iter().filter(|s| s.text().is_some()).count();
    let images = leaves.iter().filter(|s| matches!(s.content, cdr::geometry::Content::Image { .. })).count();
    let groups = pages.iter().flat_map(|p| p.shapes()).filter(|s| matches!(s.content, cdr::geometry::Content::Group { .. })).count();
    let layers: Vec<String> =
        pages.iter().flat_map(|p| &p.layers).filter(|l| l.kind.is_art()).map(|l| l.name.clone().unwrap_or_else(|| "(unnamed)".into())).collect();
    eprintln!("pages: {}, paths: {}, text: {text}, images: {images}, groups: {groups}", pages.len(), leaves.len() - text - images);
    eprintln!("layers: {}", layers.join(", "));
    for w in &i.warnings {
        eprintln!("warning: {w}");
    }
}

/// Check the saved JSON; the checks that passed, or the first failure.
fn validate(i: &Imported, json: &[u8], pretty: bool) -> Result<Vec<String>, String> {
    let mut ok = Vec::new();
    if json.is_empty() {
        return Err("the document did not serialize".into());
    }
    let loaded = vectorcraft_format::load(json).map_err(|e| format!("the saved JSON does not load: {e}"))?;
    ok.push("JSON loads as a VectorCraft document".to_owned());
    if vectorcraft_format::save(&loaded, pretty) != json {
        return Err("saving the loaded document gives different JSON (the round trip loses something)".into());
    }
    ok.push("round trip is lossless".to_owned());
    if loaded.artboards.is_empty() {
        return Err("no artboard".into());
    }
    let bad_board =
        loaded.artboards.iter().find(|a| ![a.rect.x0, a.rect.y0, a.rect.x1, a.rect.y1].iter().all(|v| v.is_finite()) || a.rect.area() <= 0.0);
    if let Some(a) = bad_board {
        return Err(format!("artboard {} has an invalid rectangle {:?}", a.name, a.rect));
    }
    ok.push(format!("{} artboard(s) with finite, non-empty rectangles", loaded.artboards.len()));
    let (mut objects, mut finite) = (0, true);
    count_objects(&loaded, &mut objects, &mut finite);
    if !finite {
        return Err("a path, type or image placement has a non-finite coordinate".into());
    }
    let expected: usize =
        i.translation.pages.iter().filter(|p| !p.master).flat_map(|p| &p.layers).filter(|l| l.kind.is_art()).flat_map(|l| &l.shapes).map(kept).sum();
    if objects != expected {
        return Err(format!("{objects} path, type and image node(s) in the document, {expected} translated"));
    }
    ok.push(format!("{objects} path, type and image node(s), all coordinates finite"));
    Ok(ok)
}

fn count_objects(doc: &Document, objects: &mut usize, finite: &mut bool) {
    fn walk(n: &Arc<Node>, objects: &mut usize, finite: &mut bool) {
        let ok = |p: vectorcraft_geom::Point| p.x.is_finite() && p.y.is_finite();
        match &n.kind {
            NodeKind::Path { path, .. } => {
                *objects += 1;
                *finite &= path.subpaths.iter().flat_map(|s| &s.anchors).all(|a| ok(a.p) && ok(a.h_in) && ok(a.h_out));
            }
            NodeKind::Text(t) => {
                *objects += 1;
                *finite &= t.xf.as_coeffs().iter().all(|v| v.is_finite());
            }
            NodeKind::Image(i) => {
                *objects += 1;
                *finite &= i.xf.as_coeffs().iter().all(|v| v.is_finite());
            }
            _ => {}
        }
        for c in n.children().into_iter().flatten() {
            walk(c, objects, finite);
        }
    }
    doc.layers.iter().for_each(|l| walk(l, objects, finite));
}

/// How many path, type and image nodes the import makes of a shape (as `cdr::import` keeps them:
/// empty paths and clip groups with nothing to clip are left out).
fn kept(s: &cdr::geometry::Shape) -> usize {
    use cdr::geometry::Content;
    match &s.content {
        Content::Path { path, .. } if path.is_empty() => 0,
        Content::Group { children, clip } => {
            let direct = children.iter().filter(|c| kept(c) > 0).count();
            if direct == 0 || (*clip && direct < 2) { 0 } else { children.iter().map(kept).sum() }
        }
        _ => 1,
    }
}
