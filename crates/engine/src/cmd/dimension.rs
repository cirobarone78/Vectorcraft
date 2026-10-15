//! Dimension objects: generated vector art with a typed, editable recipe.
use super::*;
use serde_json::{Value, json};
use std::sync::Arc;
use vectorcraft_color::{Color, Paint};
use vectorcraft_doc::appearance::{AppearanceItem, Dash};
use vectorcraft_doc::dimension::{DimensionKind, DimensionOptions, DimensionSpec};
use vectorcraft_doc::text::{CharStyle, Justify, TextObject};
use vectorcraft_doc::{Appearance, Document, Node, NodeId, NodeKind};
use vectorcraft_geom::{Affine, BezPath, PathData, Point};

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(
            "dimension.create",
            "Add Dimension",
            [],
            None,
            "{kind: linear|angular|radius|diameter, points: [[x,y],…] (linear/radial: two; angular: vertex then two rays), placement: [x,y], options?: {unit?: Points|Pixels|Millimeters|Centimeters|Inches|Picas|Feet|FeetInches|Meters|Yards, precision?: 0..6, hideUnit?, scale?: real/drawing ratio, lineWidth?, lineType?: solid|dashed|dotted, lineColor?: [r,g,b], arrow?: closed|open|tick|none, arrowScale?: %, extensionWidth?, extensionType?, extensionColor?, extensionGap?, extensionOverrun?, hideExtensions?, fontFamily?, fontStyle?, fontSize?, textColor?, textPosition?: above|inline|below, textAlign?: start|center|end, textOffset?}} create editable vector annotation on a separate Dimensions layer in one undo step → {id, layer, label}",
            has_doc,
            create
        ),
        cmd!(
            "dimension.update",
            "Edit Dimension",
            [],
            None,
            "{id?: dimension group (default selection), options: {…}} merge style settings and regenerate annotation in one undo step → {id, label}",
            has_doc,
            update
        ),
        cmd!(
            query "dimension.inspect",
            "Dimension Settings",
            [],
            None,
            "{id?: dimension group (default selection)} → {id, spec} stored editable annotation settings",
            has_doc,
            inspect
        ),
        cmd!(
            "dimension.fromSelection",
            "Dimension Selected Artwork",
            [],
            None,
            "{axis?: horizontal|vertical|radius|diameter (default horizontal), offset?: pt (24), options?: {…}} annotate selected artwork bounds; radial modes require one actual circle → {id, layer, label}",
            has_selection,
            from_selection
        ),
    ]
}

fn options(value: Option<&Value>, base: &DimensionOptions) -> Result<DimensionOptions> {
    let mut combined = serde_json::to_value(base).map_err(|e| bad("dimension", e.to_string()))?;
    if let Some(value) = value {
        let map = value.as_object().ok_or_else(|| bad("dimension", "options must be an object"))?;
        let target = combined.as_object_mut().ok_or_else(|| bad("dimension", "invalid dimension options"))?;
        target.extend(map.clone());
    }
    let out: DimensionOptions = serde_json::from_value(combined).map_err(|e| bad("dimension", e.to_string()))?;
    out.validate().map_err(|e| bad("dimension", e))?;
    Ok(out)
}

fn point(v: &Value) -> Result<Point> {
    let a = v.as_array().filter(|a| a.len() == 2).ok_or_else(|| bad("dimension", "point must be [x,y]"))?;
    let (x, y) =
        a.first().and_then(Value::as_f64).zip(a.get(1).and_then(Value::as_f64)).ok_or_else(|| bad("dimension", "point must contain two numbers"))?;
    Ok(Point::new(x, y))
}

fn create(s: &mut Session, p: &Value) -> Result<Value> {
    let kind: DimensionKind =
        serde_json::from_value(p.get("kind").cloned().unwrap_or(json!("linear"))).map_err(|e| bad("dimension.create", e.to_string()))?;
    let points = p
        .get("points")
        .and_then(Value::as_array)
        .filter(|a| a.len() <= 3)
        .ok_or_else(|| bad("dimension.create", "need 2 or 3 measurement points"))?;
    let spec = DimensionSpec {
        kind,
        points: points.iter().map(point).collect::<Result<_>>()?,
        placement: point(p.get("placement").unwrap_or(&Value::Null))?,
        options: options(p.get("options"), &DimensionOptions::default())?,
        transform: Affine::IDENTITY,
    };
    insert(s, spec)
}

/// A suitable annotation layer is reused; hidden, locked and template layers are left alone.
fn annotation_layer(d: &mut Document) -> NodeId {
    let existing = d.layers.iter().find(|n| n.extras().dimension_layer && n.visible && !n.locked && !n.is_template());
    if let Some(n) = existing {
        return n.id;
    }
    let id = d.add_layer(Some("Dimensions"));
    if let Some(n) = d.node_mut(id) {
        n.extras_mut().dimension_layer = true;
    }
    id
}

fn children(d: &mut Document, spec: &DimensionSpec) -> Result<(Vec<Arc<Node>>, String)> {
    let drawing = spec.drawing().map_err(|e| bad("dimension", e))?;
    let o = &spec.options;
    let mut children = vec![];
    for stroke in drawing.strokes {
        let Some(first) = stroke.points.first().copied() else { continue };
        let mut path = BezPath::new();
        path.move_to(first);
        for p in stroke.points.iter().skip(1) {
            path.line_to(*p);
        }
        if stroke.fill {
            path.close_path();
        }
        let (rgb, width, line_type) =
            if stroke.extension { (o.extension_color, o.extension_width, &o.extension_type) } else { (o.line_color, o.line_width, &o.line_type) };
        let paint = Paint::solid(Color::rgb8(rgb[0], rgb[1], rgb[2]));
        let mut look = if stroke.fill { Appearance::basic(paint, Paint::None, 0.0) } else { Appearance::basic(Paint::None, paint, width) };
        if !stroke.fill && line_type != "solid" {
            for item in &mut look.items {
                if let AppearanceItem::Stroke(s) = item {
                    s.dash = Some(Dash {
                        pattern: if line_type == "dotted" { vec![width, width * 2.0] } else { vec![width * 5.0, width * 3.0] },
                        ..Default::default()
                    });
                }
            }
        }
        let mut node = Node::path(d.alloc_id(), PathData::from_bezpath(&path), look);
        node.transform(spec.transform, true);
        children.push(Arc::new(node));
    }
    let rgb = o.text_color;
    // CSS generic families describe a choice of font, not a missing named font. Store the
    // resolved face in the generated text so the editor does not highlight it as substituted.
    let resolved = matches!(o.font_family.as_str(), "sans-serif" | "serif" | "monospace")
        .then(|| vectorcraft_text::FontDb::global().resolve(&o.font_family, &o.font_style))
        .flatten();
    let style = CharStyle {
        font_family: resolved.as_ref().map_or_else(|| o.font_family.clone(), |(face, _)| face.family.clone()),
        font_style: resolved.as_ref().map_or_else(|| o.font_style.clone(), |(face, _)| face.style.clone()),
        size: o.font_size,
        fill: Paint::solid(Color::rgb8(rgb[0], rgb[1], rgb[2])),
        ..Default::default()
    };
    let mut text = TextObject::point(Point::ORIGIN, &drawing.text, style);
    text.para.justify = match o.text_align.as_str() {
        "start" => Justify::Left,
        "end" => Justify::Right,
        _ => Justify::Center,
    };
    text.xf = spec.transform * Affine::translate(drawing.at.to_vec2()) * Affine::rotate(drawing.angle);
    super::typecmd::refresh_bounds(&mut text);
    children.push(Arc::new(Node::new(d.alloc_id(), NodeKind::Text(Box::new(text)))));
    Ok((children, drawing.text))
}

fn insert(s: &mut Session, spec: DimensionSpec) -> Result<Value> {
    spec.drawing().map_err(|e| bad("dimension.create", e))?;
    let (id, layer, label) = s.edit("Add Dimension", |d, sel| {
        let layer = annotation_layer(d);
        let (children, label) = children(d, &spec)?;
        let id = d.alloc_id();
        let mut node = Node::group(id, children);
        node.name = Some(format!("Dimension: {label}"));
        node.extras_mut().dimension = Some(Box::new(spec));
        d.insert(Some(layer), usize::MAX, node)?;
        sel.set([id]);
        Ok((id, layer, label))
    })?;
    Ok(json!({"id":id,"layer":layer,"label":label}))
}

fn selected(s: &Session, p: &Value) -> Result<NodeId> {
    let id = if let Some(value) = p.get("id") {
        NodeId(value.as_u64().ok_or_else(|| bad("dimension", "id must be an unsigned integer"))?)
    } else {
        let selection = &s.doc()?.selection.objects;
        if selection.len() != 1 {
            return Err(bad("dimension", "select one dimension object"));
        }
        selection.first().copied().ok_or_else(|| bad("dimension", "select a dimension object"))?
    };
    if s.doc()?.doc.node(id).is_none_or(|n| n.extras().dimension.is_none()) {
        return Err(bad("dimension", "select a dimension object"));
    }
    Ok(id)
}

fn inspect(s: &mut Session, p: &Value) -> Result<Value> {
    let id = selected(s, p)?;
    let n = s.doc()?.doc.node(id).ok_or(EngineError::NoNode(id))?;
    Ok(json!({"id":id,"spec":n.extras().dimension}))
}

fn update(s: &mut Session, p: &Value) -> Result<Value> {
    let id = selected(s, p)?;
    if !s.doc()?.doc.is_editable(id) {
        return Err(bad("dimension.update", "dimension is locked or hidden"));
    }
    let mut spec = s.doc()?.doc.node(id).and_then(|n| n.extras().dimension.as_deref()).cloned().ok_or(EngineError::NoNode(id))?;
    spec.options = options(p.get("options"), &spec.options)?;
    spec.drawing().map_err(|e| bad("dimension.update", e))?;
    let label = s.edit("Edit Dimension", |d, _| {
        let (children, label) = children(d, &spec)?;
        let n = d.node_mut(id).ok_or(EngineError::NoNode(id))?;
        n.kind = NodeKind::Group { children, clip: false };
        n.name = Some(format!("Dimension: {label}"));
        n.extras_mut().dimension = Some(Box::new(spec));
        Ok(label)
    })?;
    Ok(json!({"id":id,"label":label}))
}

fn from_selection(s: &mut Session, p: &Value) -> Result<Value> {
    let d = &s.doc()?.doc;
    let ids = s.doc()?.selection.objects.iter().copied().filter(|id| d.node(*id).is_some_and(|n| n.extras().dimension.is_none())).collect::<Vec<_>>();
    let b = d.bounds_of(&ids, false).ok_or_else(|| bad("dimension.fromSelection", "select artwork to measure"))?;
    let axis = p.get("axis").and_then(Value::as_str).unwrap_or("horizontal");
    let offset = p.get("offset").and_then(Value::as_f64).unwrap_or(24.0);
    if !offset.is_finite() || offset.abs() > 10000.0 {
        return Err(bad("dimension.fromSelection", "invalid offset"));
    }
    let (kind, points, placement) = match axis {
        "horizontal" => (DimensionKind::Linear, vec![Point::new(b.x0, b.y0), Point::new(b.x1, b.y0)], Point::new((b.x0 + b.x1) * 0.5, b.y0 - offset)),
        "vertical" => (DimensionKind::Linear, vec![Point::new(b.x1, b.y0), Point::new(b.x1, b.y1)], Point::new(b.x1 + offset, (b.y0 + b.y1) * 0.5)),
        "radius" | "diameter" => {
            if ids.len() != 1 {
                return Err(bad("dimension.fromSelection", "radial measurement requires a circle"));
            }
            let n = ids.first().and_then(|id| d.node(*id)).ok_or_else(|| bad("dimension.fromSelection", "select a circle"))?;
            let Some(vectorcraft_doc::LiveShape::Ellipse { w, h, xf, .. }) = n.live_shape() else {
                return Err(bad("dimension.fromSelection", "select a live circle"));
            };
            let [a, b, c, d, _, _] = xf.as_coeffs();
            let (sx, sy) = (a.hypot(b), c.hypot(d));
            if (w - h).abs() > w.abs() * 1e-6 || (sx - sy).abs() > sx * 1e-6 || (a * c + b * d).abs() > sx * sy * 1e-6 {
                return Err(bad("dimension.fromSelection", "radial measurement requires a circle"));
            }
            let center = *xf * Point::new(w * 0.5, h * 0.5);
            let radius = w.abs() * sx * 0.5;
            (
                if axis == "radius" { DimensionKind::Radius } else { DimensionKind::Diameter },
                vec![center, center + vectorcraft_geom::Vec2::new(radius, 0.0)],
                center + vectorcraft_geom::Vec2::new(radius + offset, -radius - offset),
            )
        }
        _ => return Err(bad("dimension.fromSelection", "unknown measurement axis")),
    };
    insert(
        s,
        DimensionSpec { kind, points, placement, options: options(p.get("options"), &DimensionOptions::default())?, transform: Affine::IDENTITY },
    )
}
