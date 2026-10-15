//! CorelDRAW `.cdr` import through the pure-Rust `cdr` crate: X4+ ZIP packages and bare version
//! 3–13 RIFF files, routed by their content ([`cdr::import::route`]).

use vectorcraft_doc::Document;

use super::super::*;

/// Is `bytes` a CorelDRAW file the reader routes (not just any ZIP archive)?
pub(super) fn sniff(bytes: &[u8]) -> bool {
    cdr::import::is_cdr(bytes)
}

/// A CorelDRAW file's document and what the import left out or approximated.
pub(super) fn import(bytes: &[u8]) -> Result<(Document, Vec<String>)> {
    let mut i = cdr::import::import(bytes, cdr::Limits::default()).map_err(super::err)?;
    // The package's own RGB and CMYK profiles, installed like those a native file carries; a
    // profile that can't be used leaves its space to the working space.
    let mut warnings = super::native::install_profiles(std::mem::take(&mut i.profiles));
    let p = &mut i.document.color_profiles;
    for name in [&mut p.rgb, &mut p.cmyk] {
        if name.as_deref().is_some_and(|n| vectorcraft_color::cms::profile(n).is_none()) {
            *name = None;
        }
    }
    warnings.append(&mut i.warnings);
    let mut doc = i.document;
    for layer in &mut doc.layers {
        fit_envelopes_to_ink(std::sync::Arc::make_mut(layer));
    }
    Ok((doc, warnings))
}

/// CorelDRAW fits an envelope to its type's ink (the letters' own box); VectorCraft's envelope
/// maps the type's layout box (ascent to descent, advance to advance) onto its outline. So that
/// the letters land where CorelDRAW draws them, each enveloped type's outline grows by the margin
/// between the two boxes, in proportion to the outline's size.
pub(super) fn fit_envelopes_to_ink(n: &mut vectorcraft_doc::Node) {
    use vectorcraft_doc::NodeKind;
    use vectorcraft_doc::live::EnvelopeKind;
    use vectorcraft_geom::{Affine, Shape as _};
    if let NodeKind::Envelope { content, kind, .. } = &mut n.kind
        && let [text] = content.as_slice()
        && let NodeKind::Text(t) = &text.kind
        && let EnvelopeKind::TopObject { path } = kind.as_mut()
    {
        let lay = vectorcraft_text::layout(vectorcraft_text::FontDb::global(), t);
        let ink = lay.glyphs.iter().map(|g| g.outline.bounding_box()).filter(|b| b.width() > 0.0 || b.height() > 0.0).reduce(|a, b| a.union(b));
        if let (Some(ink), Some(outline)) = (ink, path.bounds()) {
            let (ink, line) = (t.xf.transform_rect_bbox(ink), t.xf.transform_rect_bbox(lay.bounds.union(ink)));
            if ink.width() > 1e-6 && ink.height() > 1e-6 {
                let (sx, sy) = (outline.width() / ink.width(), outline.height() / ink.height());
                let grown = vectorcraft_geom::Rect::new(
                    outline.x0 - (ink.x0 - line.x0) * sx,
                    outline.y0 - (ink.y0 - line.y0) * sy,
                    outline.x1 + (line.x1 - ink.x1) * sx,
                    outline.y1 + (line.y1 - ink.y1) * sy,
                );
                let fit = Affine::translate(grown.origin().to_vec2())
                    * Affine::scale_non_uniform(grown.width() / outline.width().max(1e-9), grown.height() / outline.height().max(1e-9))
                    * Affine::translate(-outline.origin().to_vec2());
                *path = path.transformed(fit);
            }
        }
    }
    for c in n.children_mut().into_iter().flatten() {
        fit_envelopes_to_ink(std::sync::Arc::make_mut(c));
    }
}
