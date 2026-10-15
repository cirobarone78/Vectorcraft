//! Editable dimension annotations. Geometry is generated as ordinary vector art by the engine.
use serde::{Deserialize, Serialize};
use vectorcraft_geom::{Affine, Point, Vec2};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum DimensionKind {
    #[default]
    Linear,
    Angular,
    Radius,
    Diameter,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct DimensionOptions {
    pub unit: crate::Unit,
    pub precision: u8,
    pub hide_unit: bool,
    /// Real-world length per drawing length: 100 is a 1:100 drawing.
    pub scale: f64,
    pub line_width: f64,
    pub line_type: String,
    pub line_color: [u8; 3],
    pub arrow: String,
    pub arrow_scale: f64,
    pub extension_width: f64,
    pub extension_type: String,
    pub extension_color: [u8; 3],
    pub extension_gap: f64,
    pub extension_overrun: f64,
    pub hide_extensions: bool,
    pub font_family: String,
    pub font_style: String,
    pub font_size: f64,
    pub text_color: [u8; 3],
    pub text_position: String,
    pub text_align: String,
    pub text_offset: f64,
}

impl Default for DimensionOptions {
    fn default() -> Self {
        Self {
            unit: crate::Unit::Millimeters,
            precision: 1,
            hide_unit: false,
            scale: 1.0,
            line_width: 1.0,
            line_type: "solid".into(),
            line_color: [112, 64, 152],
            arrow: "closed".into(),
            arrow_scale: 100.0,
            extension_width: 0.5,
            extension_type: "solid".into(),
            extension_color: [112, 64, 152],
            extension_gap: 5.0,
            extension_overrun: 9.0,
            hide_extensions: false,
            font_family: "sans-serif".into(),
            font_style: "Regular".into(),
            font_size: 12.0,
            text_color: [112, 64, 152],
            text_position: "above".into(),
            text_align: "center".into(),
            text_offset: 6.0,
        }
    }
}

impl DimensionOptions {
    pub fn validate(&self) -> Result<(), String> {
        for (name, value, min, max) in [
            ("scale", self.scale, 0.000001, 1e6),
            ("lineWidth", self.line_width, 0.05, 100.0),
            ("extensionWidth", self.extension_width, 0.05, 100.0),
            ("arrowScale", self.arrow_scale, 1.0, 1000.0),
            ("extensionGap", self.extension_gap, 0.0, 10000.0),
            ("extensionOverrun", self.extension_overrun, 0.0, 10000.0),
            ("fontSize", self.font_size, 1.0, 1000.0),
            ("textOffset", self.text_offset, 0.0, 10000.0),
        ] {
            if !value.is_finite() || !(min..=max).contains(&value) {
                return Err(format!("{name} must be finite and within {min}..{max}"));
            }
        }
        if self.precision > 6 {
            return Err("precision must be 0..6".into());
        }
        for (name, value, allowed) in [
            ("lineType", self.line_type.as_str(), &["solid", "dashed", "dotted"][..]),
            ("extensionType", self.extension_type.as_str(), &["solid", "dashed", "dotted"][..]),
            ("arrow", self.arrow.as_str(), &["closed", "open", "tick", "none"][..]),
            ("textPosition", self.text_position.as_str(), &["above", "inline", "below"][..]),
            ("textAlign", self.text_align.as_str(), &["start", "center", "end"][..]),
        ] {
            if !allowed.contains(&value) {
                return Err(format!("invalid {name}"));
            }
        }
        if self.font_family.trim().is_empty() || self.font_family.len() > 256 || self.font_style.len() > 128 {
            return Err("invalid dimension font name".into());
        }
        Ok(())
    }

    pub fn label(&self, measured: f64, angular: bool, prefix: &str) -> String {
        if !angular && self.unit == crate::Unit::FeetInches {
            let factor = 10f64.powi(i32::from(self.precision));
            let inches = (measured * self.scale / 72.0 * factor).round() / factor;
            let feet = (inches / 12.0).floor();
            let rest = inches - feet * 12.0;
            return format!(
                "{prefix}{feet:.0}{foot} {rest:.precision$}{inch}",
                foot = if self.hide_unit { "" } else { "′" },
                inch = if self.hide_unit { "" } else { "″" },
                precision = usize::from(self.precision)
            );
        }
        let value = if angular { measured } else { self.unit.from_pt(measured * self.scale) };
        let value = if value.abs() < 0.5 * 10f64.powi(-i32::from(self.precision)) { 0.0 } else { value };
        let suffix = if self.hide_unit {
            ""
        } else if angular {
            "°"
        } else {
            self.unit.suffix()
        };
        format!(
            "{prefix}{value:.precision$}{space}{suffix}",
            precision = usize::from(self.precision),
            space = if suffix.is_empty() || angular { "" } else { " " }
        )
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DimensionSpec {
    pub kind: DimensionKind,
    /// Linear: endpoints; angular: vertex, first ray, second ray; radial: centre, rim.
    #[serde(deserialize_with = "read_points")]
    pub points: Vec<Point>,
    pub placement: Point,
    pub options: DimensionOptions,
    #[serde(default)]
    pub transform: Affine,
}

impl DimensionSpec {
    pub fn validate(&self) -> Result<(), String> {
        self.options.validate()?;
        let count = if self.kind == DimensionKind::Angular { 3 } else { 2 };
        if self.points.len() != count {
            return Err(format!("dimension needs {count} points"));
        }
        if self
            .points
            .iter()
            .chain(std::iter::once(&self.placement))
            .any(|p| !p.x.is_finite() || !p.y.is_finite() || p.x.abs() > 1e8 || p.y.abs() > 1e8)
        {
            return Err("dimension coordinates must be finite and within ±100000000".into());
        }
        if self.transform.as_coeffs().iter().any(|n| !n.is_finite() || n.abs() > 1e12) {
            return Err("invalid dimension transform".into());
        }
        let a = *self.points.first().ok_or("missing dimension point")?;
        if self.points.iter().skip(1).any(|p| (*p - a).hypot() < 1e-6) {
            return Err("dimension points must be distinct".into());
        }
        Ok(())
    }

    /// Shared by the pointer preview and the persisted vector artwork.
    pub fn drawing(&self) -> Result<DimensionDrawing, String> {
        self.validate()?;
        let a = *self.points.first().ok_or("missing first point")?;
        let b = *self.points.get(1).ok_or("missing second point")?;
        let o = &self.options;
        let mut out = DimensionDrawing { strokes: vec![], text: String::new(), at: self.placement, angle: 0.0 };
        let direction = (b - a) / (b - a).hypot();
        let normal = Vec2::new(-direction.y, direction.x);
        match self.kind {
            DimensionKind::Linear => {
                let offset = (self.placement - a).dot(normal);
                let (p, q) = (a + normal * offset, b + normal * offset);
                let side = if offset < 0.0 { -1.0 } else { 1.0 };
                if !o.hide_extensions {
                    for (source, end) in [(a, p), (b, q)] {
                        if offset.abs() > o.extension_gap {
                            out.line(source + normal * (side * o.extension_gap), end + normal * (side * o.extension_overrun), true);
                        }
                    }
                }
                out.text = o.label((b - a).hypot(), false, "");
                let fraction = match o.text_align.as_str() {
                    "start" => 0.0,
                    "end" => 1.0,
                    _ => 0.5,
                };
                out.at = p + (q - p) * fraction;
                let upright = if direction.x < 0.0 { -direction } else { direction };
                let above = Vec2::new(upright.y, -upright.x);
                // Outside/inside follow the measured object, including a dimension below it.
                // The persisted names remain above/below for compatibility with early files.
                if o.text_position == "inline" {
                    out.at -= above * (o.font_size * 0.3);
                } else {
                    let away = normal * side * if o.text_position == "below" { -1.0 } else { 1.0 };
                    let baseline = if away.dot(above) > 0.0 { o.font_size * 0.3 } else { o.font_size };
                    out.at += away * (o.text_offset + baseline);
                }
                out.angle = upright.y.atan2(upright.x);
                if o.text_position == "inline" {
                    let half = (out.text.chars().count() as f64 * o.font_size * 0.32 + 3.0).min((q - p).hypot() * 0.45);
                    let mid = p + (q - p) * fraction;
                    if fraction > 0.0 {
                        out.line(p, mid - direction * half, false);
                    }
                    if fraction < 1.0 {
                        out.line(mid + direction * half, q, false);
                    }
                } else {
                    out.line(p, q, false);
                }
                out.arrow(p, direction, o);
                out.arrow(q, -direction, o);
            }
            DimensionKind::Radius | DimensionKind::Diameter => {
                let radius = (b - a).hypot();
                let p = if self.kind == DimensionKind::Diameter { a - direction * radius } else { a };
                out.line(p, b, false);
                out.arrow(b, -direction, o);
                if self.kind == DimensionKind::Diameter {
                    out.arrow(p, direction, o);
                }
                // A leader lets the label be placed freely outside a small circle.
                out.line(b, self.placement, false);
                out.at = self.placement
                    + Vec2::new(
                        0.0,
                        match o.text_position.as_str() {
                            "above" => -o.text_offset,
                            "below" => o.text_offset + o.font_size,
                            _ => 0.0,
                        },
                    );
                out.text = o.label(
                    radius * if self.kind == DimensionKind::Diameter { 2.0 } else { 1.0 },
                    false,
                    if self.kind == DimensionKind::Diameter { "Ø " } else { "R " },
                );
            }
            DimensionKind::Angular => {
                let c = *self.points.get(2).ok_or("missing second ray")?;
                let start = direction.y.atan2(direction.x);
                let ray = c - a;
                let delta = (ray.y.atan2(ray.x) - start).rem_euclid(std::f64::consts::TAU);
                if delta < 1e-8 || (delta - std::f64::consts::TAU).abs() < 1e-8 {
                    return Err("angular dimension needs two different ray directions".into());
                }
                let at = self.placement - a;
                let phase = (at.y.atan2(at.x) - start).rem_euclid(std::f64::consts::TAU);
                let sweep = if phase <= delta { delta } else { delta - std::f64::consts::TAU };
                let radius = at.hypot().max(o.font_size * 2.0);
                let radial = |angle: f64| Vec2::new(angle.cos(), angle.sin());
                let mut points = vec![];
                // A bounded polyline with <= 2-degree segments, independent of image input sizes.
                let count = (sweep.abs().to_degrees() / 2.0).ceil().clamp(1.0, 180.0) as u32;
                for i in 0..=count {
                    points.push(a + radial(start + sweep * f64::from(i) / f64::from(count)) * radius);
                }
                out.strokes.push(DimensionStroke { points, extension: false, fill: false });
                if !o.hide_extensions {
                    for ray in [direction, (c - a) / (c - a).hypot()] {
                        out.line(a + ray * o.extension_gap, a + ray * (radius + o.extension_overrun), true);
                    }
                }
                let sign = sweep.signum();
                for (angle, turn) in [(start, sign), (start + sweep, -sign)] {
                    let r = radial(angle);
                    out.arrow(a + r * radius, Vec2::new(-r.y, r.x) * turn, o);
                }
                let text_radius = radius
                    + match o.text_position.as_str() {
                        "below" => -o.text_offset - o.font_size,
                        "inline" => 0.0,
                        _ => o.text_offset + o.font_size,
                    };
                let fraction = match o.text_align.as_str() {
                    "start" => 0.1,
                    "end" => 0.9,
                    _ => 0.5,
                };
                out.at = a + radial(start + sweep * fraction) * text_radius;
                out.text = o.label(sweep.abs().to_degrees(), true, "");
            }
        }
        Ok(out)
    }
}

pub struct DimensionStroke {
    pub points: Vec<Point>,
    pub extension: bool,
    pub fill: bool,
}
pub struct DimensionDrawing {
    pub strokes: Vec<DimensionStroke>,
    pub text: String,
    pub at: Point,
    pub angle: f64,
}
impl DimensionDrawing {
    fn line(&mut self, a: Point, b: Point, extension: bool) {
        self.strokes.push(DimensionStroke { points: vec![a, b], extension, fill: false });
    }
    fn arrow(&mut self, tip: Point, direction: Vec2, o: &DimensionOptions) {
        let size = (5.0 + o.line_width * 2.0) * o.arrow_scale / 100.0;
        let side = Vec2::new(-direction.y, direction.x);
        let rear = tip + direction * size;
        match o.arrow.as_str() {
            "closed" => self.strokes.push(DimensionStroke {
                points: vec![tip, rear + side * size * 0.35, rear - side * size * 0.35],
                extension: false,
                fill: true,
            }),
            "open" => {
                self.line(tip, rear + side * size * 0.35, false);
                self.line(tip, rear - side * size * 0.35, false);
            }
            "tick" => self.line(tip + (direction + side) * size * 0.4, tip - (direction + side) * size * 0.4, false),
            _ => {}
        }
    }
}

fn read_points<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Vec<Point>, D::Error> {
    struct Points;
    impl<'de> serde::de::Visitor<'de> for Points {
        type Value = Vec<Point>;
        fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("at most three dimension points")
        }
        fn visit_seq<A: serde::de::SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
            let mut points = Vec::with_capacity(3);
            while let Some(p) = seq.next_element()? {
                if points.len() == 3 {
                    return Err(serde::de::Error::custom("too many dimension points"));
                }
                points.push(p);
            }
            Ok(points)
        }
    }
    d.deserialize_seq(Points)
}
