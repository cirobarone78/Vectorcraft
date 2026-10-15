//! Dimension pointer workflow: pick measurement points, then place the annotation.
use crate::{Action, Cursor, Mods, Overlay, PointerEvent, PointerKind, Tool, ToolContext, ToolKey};
use serde_json::{Value, json};
use vectorcraft_doc::dimension::{DimensionKind, DimensionOptions, DimensionSpec};
use vectorcraft_geom::kurbo::{ParamCurveNearest, PathSeg};
use vectorcraft_geom::{Affine, Point, Vec2};

#[derive(Default)]
pub struct DimensionTool {
    kind: DimensionKind,
    axis: String,
    options: DimensionOptions,
    points: Vec<Point>,
    hover: Point,
    pressed: Option<Point>,
    direct: bool,
    snap: Vec<Overlay>,
}

impl DimensionTool {
    fn constrained(&self, a: Point, p: Point) -> Point {
        if self.kind != DimensionKind::Linear {
            return p;
        }
        match self.axis.as_str() {
            "horizontal" => Point::new(p.x, a.y),
            "vertical" => Point::new(a.x, p.y),
            _ => p,
        }
    }
    fn spec(&self) -> DimensionSpec {
        DimensionSpec {
            kind: self.kind,
            points: self.points.clone(),
            placement: self.hover,
            options: self.options.clone(),
            transform: Affine::IDENTITY,
        }
    }
    fn count(&self) -> usize {
        if self.kind == DimensionKind::Angular { 3 } else { 2 }
    }
    fn reset(&mut self) {
        self.points.clear();
        self.pressed = None;
        self.direct = false;
        self.snap.clear();
    }

    fn pick(&self, cx: &ToolContext, p: Point) -> Option<Vec<Point>> {
        let mut found = None;
        let mut distance = cx.tol(7.0);
        cx.doc.walk(|n| {
            if !cx.doc.is_editable(n.id) || n.extras().dimension.is_some() {
                return;
            }
            if cx.doc.ancestry(n.id).is_some_and(|ids| ids.iter().any(|id| cx.doc.node(*id).is_some_and(|p| p.extras().dimension.is_some()))) {
                return;
            }
            if !n.geometric_bounds().is_some_and(|b| b.inflate(distance, distance).contains(p)) {
                return;
            }
            if matches!(self.kind, DimensionKind::Radius | DimensionKind::Diameter) {
                if let Some(vectorcraft_doc::LiveShape::Ellipse { w, h, pie, xf }) = n.live_shape() {
                    let [a, b, c, d, _, _] = xf.as_coeffs();
                    let (sx, sy) = (a.hypot(b), c.hypot(d));
                    if (w - h).abs() > w.abs() * 1e-6
                        || (sx - sy).abs() > sx * 1e-6
                        || (a * c + b * d).abs() > sx * sy * 1e-6
                        || (pie.1 - pie.0).abs() < 1e-6
                    {
                        return;
                    }
                    let center = *xf * Point::new(w * 0.5, h * 0.5);
                    let radius = w.abs() * sx * 0.5;
                    let delta = p - center;
                    let dist = (delta.hypot() - radius).abs();
                    if radius > 1e-6 && delta.hypot() > 1e-6 && dist < distance {
                        distance = dist;
                        found = Some(vec![center, center + delta / delta.hypot() * radius]);
                    }
                }
            } else if self.kind == DimensionKind::Linear && matches!(self.axis.as_str(), "horizontal" | "vertical") {
                if let Some(path) = n.path_data() {
                    let dist = path.to_bezpath().segments().map(|s| s.nearest(p, 0.1).distance_sq.sqrt()).fold(f64::INFINITY, f64::min);
                    if dist < distance
                        && let Some(b) = n.geometric_bounds()
                    {
                        distance = dist;
                        found = Some(if self.axis == "horizontal" {
                            vec![Point::new(b.x0, b.y0), Point::new(b.x1, b.y0)]
                        } else {
                            vec![Point::new(b.x1, b.y0), Point::new(b.x1, b.y1)]
                        });
                    }
                }
            } else if self.kind == DimensionKind::Angular {
                if let Some(path) = n.path_data() {
                    for sub in &path.subpaths {
                        let count = sub.anchors.len();
                        for (i, anchor) in sub.anchors.iter().enumerate() {
                            let dist = (anchor.p - p).hypot();
                            if dist >= distance || count < 3 || (!sub.closed && (i == 0 || i + 1 == count)) {
                                continue;
                            }
                            let previous = if i == 0 { count - 1 } else { i - 1 };
                            let next = if i + 1 == count { 0 } else { i + 1 };
                            let (Some(before), Some(after)) = (sub.anchors.get(previous), sub.anchors.get(next)) else { continue };
                            let ray1 = if anchor.h_in != anchor.p { anchor.h_in } else { before.p };
                            let ray2 = if anchor.h_out != anchor.p { anchor.h_out } else { after.p };
                            distance = dist;
                            found = Some(vec![anchor.p, ray1, ray2]);
                        }
                    }
                }
            } else if self.kind == DimensionKind::Linear
                && let Some(path) = n.path_data()
            {
                for segment in path.to_bezpath().segments() {
                    if let PathSeg::Line(line) = segment {
                        let near = line.nearest(p, 0.1);
                        let dist = near.distance_sq.sqrt();
                        if dist < distance && (line.p1 - line.p0).hypot() > 1e-6 {
                            distance = dist;
                            found = Some(vec![line.p0, line.p1]);
                        }
                    }
                }
            }
        });
        found
    }
}

impl Tool for DimensionTool {
    fn id(&self) -> &'static str {
        "dimension"
    }
    fn busy(&self) -> bool {
        !self.points.is_empty()
    }
    fn pointer(&mut self, cx: &ToolContext, ev: &PointerEvent) -> Vec<Action> {
        let (mut p, overlay) = crate::guides::snap_draw(cx, ev.pos, &[]);
        if self.points.len() == 1
            && let Some(a) = self.points.first()
        {
            p = self.constrained(*a, p);
        }
        if ev.mods.shift
            && !(self.kind == DimensionKind::Linear && matches!(self.axis.as_str(), "horizontal" | "vertical"))
            && self.points.len() == 1
            && let Some(a) = self.points.first()
        {
            p = *a + vectorcraft_geom::constrain_angle(p - *a, 45.0);
        }
        self.hover = p;
        self.snap = overlay;
        match ev.kind {
            PointerKind::Down => {
                self.pressed = Some(ev.pos);
                if self.points.is_empty()
                    && let Some(points) = self.pick(cx, ev.pos)
                {
                    self.points = points;
                    self.direct = true;
                }
            }
            PointerKind::Up => {
                let pressed = self.pressed.take();
                let dragged = pressed.is_some_and(|start| (ev.pos - start).hypot() > cx.tol(3.0));
                if self.points.is_empty() && dragged && self.kind != DimensionKind::Angular {
                    if let Some(start) = pressed {
                        let a = crate::guides::snap_draw(cx, start, &[]).0;
                        self.points = vec![a, self.constrained(a, p)];
                    }
                    return vec![];
                }
                if self.direct && !dragged {
                    self.direct = false;
                    return vec![];
                }
                if self.points.len() == self.count() {
                    let points: Vec<_> = self.points.iter().map(|p| json!([p.x, p.y])).collect();
                    let params = json!({"kind":self.kind,"points":points,"placement":[p.x,p.y],"options":self.options});
                    self.reset();
                    return vec![Action::Exec("dimension.create".into(), params)];
                }
                if self.points.last().is_none_or(|a| (*a - p).hypot() > 1e-6) {
                    self.points.push(p);
                }
            }
            _ => {}
        }
        vec![]
    }
    fn key(&mut self, _cx: &ToolContext, key: ToolKey, _mods: Mods) -> Vec<Action> {
        match key {
            ToolKey::Escape => self.reset(),
            ToolKey::Backspace => {
                self.points.pop();
            }
            _ => {}
        }
        vec![]
    }
    fn deactivate(&mut self, _cx: &ToolContext) -> Vec<Action> {
        self.reset();
        vec![]
    }
    fn overlays(&self, cx: &ToolContext) -> Vec<Overlay> {
        let mut out = self.snap.clone();
        let color = self.options.line_color;
        if self.points.len() == self.count() {
            if let Ok(drawing) = self.spec().drawing() {
                for stroke in drawing.strokes {
                    for pair in stroke.points.windows(2) {
                        if let (Some(a), Some(b)) = (pair.first(), pair.get(1)) {
                            out.push(Overlay::Line { a: *a, b: *b, color, dashed: stroke.extension });
                        }
                    }
                }
                out.push(Overlay::Measure { p: drawing.at, text: drawing.text });
            }
        } else {
            if let Some(a) = self.points.last() {
                out.push(Overlay::Line { a: *a, b: self.hover, color, dashed: true });
            }
            let text = if self.kind == DimensionKind::Angular {
                match self.points.len() {
                    0 => "Pick vertex",
                    1 => "Pick first ray",
                    _ => "Pick second ray",
                }
            } else if self.points.is_empty() {
                "Pick first point or drag an edge"
            } else {
                "Pick second point"
            };
            out.push(Overlay::Measure { p: self.hover + Vec2::new(cx.tol(12.0), cx.tol(12.0)), text: text.into() });
        }
        out
    }
    fn cursor(&self, _cx: &ToolContext, _p: Point, _mods: Mods) -> Cursor {
        Cursor::Crosshair
    }
    fn options(&self) -> Value {
        json!({"kind":self.kind,"axis":if self.axis.is_empty(){"aligned"}else{&self.axis},"style":self.options})
    }
    fn set_option(&mut self, key: &str, value: &Value) {
        match key {
            "axis" => {
                if let Some(axis) = value.as_str().filter(|a| matches!(*a, "aligned" | "horizontal" | "vertical")) {
                    self.axis = axis.into();
                    self.reset();
                }
            }
            "kind" => {
                if let Ok(kind) = serde_json::from_value(value.clone()) {
                    self.kind = kind;
                    self.reset();
                }
            }
            "style" => {
                if let Ok(style) = serde_json::from_value::<DimensionOptions>(value.clone())
                    && style.validate().is_ok()
                {
                    self.options = style;
                }
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::*;
    use vectorcraft_doc::Selection;

    fn click(t: &mut DimensionTool, cx: &ToolContext, x: f64, y: f64) -> Vec<Action> {
        t.pointer(cx, &PointerEvent::new(PointerKind::Down, x, y));
        t.pointer(cx, &PointerEvent::new(PointerKind::Up, x, y))
    }

    #[test]
    fn dimension_point_picks_preview_and_commit_once_then_escape_cancels() {
        let (doc, _) = doc_with_rect();
        let sel = Selection::default();
        let paint = paint();
        let mut cx = cx(&doc, &sel, &paint);
        cx.smart_guides = false;
        let mut tool = DimensionTool::default();
        assert!(click(&mut tool, &cx, 300.0, 300.0).is_empty());
        assert!(click(&mut tool, &cx, 372.0, 300.0).is_empty());
        tool.pointer(&cx, &PointerEvent::new(PointerKind::Move, 340.0, 270.0));
        assert!(tool.overlays(&cx).iter().any(|o| matches!(o,Overlay::Measure{text,..} if text=="25.4 mm")));
        let actions = click(&mut tool, &cx, 340.0, 270.0);
        assert!(matches!(actions.as_slice(),[Action::Exec(id,p)] if id=="dimension.create" && p["points"]==json!([[300.0,300.0],[372.0,300.0]])));
        assert!(!tool.busy());
        click(&mut tool, &cx, 400.0, 400.0);
        tool.key(&cx, ToolKey::Escape, Mods::default());
        assert!(!tool.busy());
    }

    #[test]
    fn dimension_drag_in_empty_space_picks_two_points_and_needs_label_placement() {
        let (doc, _) = doc_with_rect();
        let sel = Selection::default();
        let paint = paint();
        let mut cx = cx(&doc, &sel, &paint);
        cx.smart_guides = false;
        let mut tool = DimensionTool::default();
        tool.pointer(&cx, &PointerEvent::new(PointerKind::Down, 300.0, 300.0));
        assert!(tool.pointer(&cx, &PointerEvent::new(PointerKind::Up, 372.0, 300.0)).is_empty());
        assert_eq!(tool.points.len(), 2);
        assert!(matches!(click(&mut tool,&cx,330.0,260.0).as_slice(),[Action::Exec(id,_)] if id=="dimension.create"));
    }

    #[test]
    fn dimension_angular_picks_vertex_two_rays_and_label() {
        let (doc, _) = doc_with_rect();
        let sel = Selection::default();
        let paint = paint();
        let mut cx = cx(&doc, &sel, &paint);
        cx.smart_guides = false;
        let mut tool = DimensionTool::default();
        tool.set_option("kind", &json!("angular"));
        for (x, y) in [(300.0, 300.0), (372.0, 300.0), (300.0, 372.0)] {
            assert!(click(&mut tool, &cx, x, y).is_empty());
        }
        let action = click(&mut tool, &cx, 330.0, 330.0);
        assert!(matches!(action.as_slice(),[Action::Exec(id,p)] if id=="dimension.create" && p["kind"]=="angular"));
    }
}
