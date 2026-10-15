//! Dimension defaults and editing of a selected annotation, through engine commands.
use super::{DialogResult, DialogSpec};
use crate::{VectorcraftApp, state::Dialog};
use serde_json::{Value, json};
use vectorcraft_doc::{Unit, dimension::DimensionOptions};

pub const KIND: &str = "dimensionOptions";
const PRECISION_FORMATS: [&str; 7] = ["0", "0.0", "0.00", "0.000", "0.0000", "0.00000", "0.000000"];
pub(super) const SPEC: DialogSpec = DialogSpec {
    window: Some(show),
    heading: |_| tl!("Dimension Tool Options").into(),
    body,
    confirm,
    ok: Some("Save"),
    min_width: 420.0,
    max_width: Some(420.0),
    ..DialogSpec::FORM
};

pub fn open(app: &mut VectorcraftApp) -> Value {
    let selected = app.session.active().and_then(|a| {
        if a.selection.len() != 1 {
            return None;
        }
        let id = *a.selection.objects.first()?;
        let spec = a.doc.node(id)?.extras().dimension.as_deref()?;
        Some((id, spec.options.clone()))
    });
    let mut style: DimensionOptions = selected
        .as_ref()
        .map(|(_, o)| o.clone())
        .unwrap_or_else(|| serde_json::from_value(app.session.tool_options_of("dimension")["style"].clone()).unwrap_or_default());
    if matches!(style.font_family.as_str(), "sans-serif" | "serif" | "monospace")
        && let Some((face, _)) = vectorcraft_text::FontDb::global().resolve(&style.font_family, &style.font_style)
    {
        style.font_family = face.family.clone();
        style.font_style = face.style.clone();
    }
    app.ui.dialog = Some(Dialog::new(KIND, json!({"style":style,"id":selected.map(|(id,_)|id)})));
    json!({"dialog":KIND})
}

fn choose(ui: &mut egui::Ui, id: &str, value: &mut String, choices: &[(&str, &str)], width: f32) {
    let label = choices.iter().find(|(key, _)| *key == value).map(|(_, label)| *label).unwrap_or(value.as_str());
    let names: Vec<_> = choices.iter().map(|(_, label)| *label).collect();
    if let Some((key, _)) = crate::widgets::dropdown_names(ui, id, label, &names, width).and_then(|i| choices.get(i)) {
        *value = (*key).into();
    }
}

fn label(ui: &mut egui::Ui, text: &str, width: f32) {
    ui.add_sized(egui::vec2(width, 24.0), egui::Label::new(text).truncate()).on_hover_text(text);
}

fn length(ui: &mut egui::Ui, id: &str, value: &mut f64, range: std::ops::RangeInclusive<f64>, unit: Unit, width: f32) {
    if let Some(v) = crate::widgets::num_field(ui, id, Some(*value), unit, width) {
        *value = v.clamp(*range.start(), *range.end());
    }
}

fn number(ui: &mut egui::Ui, id: &str, value: &mut f64, range: std::ops::RangeInclusive<f64>, suffix: &str, width: f32) {
    if let Some(v) = crate::widgets::range_field(ui, id, *value, range, suffix, 2, width) {
        *value = v;
    }
}

fn pair(ui: &mut egui::Ui, left: impl FnOnce(&mut egui::Ui), right: impl FnOnce(&mut egui::Ui)) {
    ui.horizontal(|ui| {
        ui.allocate_ui_with_layout(egui::vec2(190.0, 24.0), egui::Layout::left_to_right(egui::Align::Center), left);
        ui.add_space(6.0);
        ui.allocate_ui_with_layout(egui::vec2(190.0, 24.0), egui::Layout::left_to_right(egui::Align::Center), right);
    });
}

/// Original code-drawn group borders and legends; no external artwork.
fn group(ui: &mut egui::Ui, title: &str, add: impl FnOnce(&mut egui::Ui)) {
    let t = crate::theme::Tokens::get(ui.ctx());
    ui.add_space(10.0);
    let frame = egui::Frame::NONE.stroke(egui::Stroke::new(1.0, t.input_border)).inner_margin(egui::Margin::symmetric(10, 10)).show(ui, |ui| {
        ui.set_min_width(396.0);
        ui.add_space(3.0);
        add(ui);
    });
    let galley = ui.painter().layout_no_wrap(title.into(), egui::FontId::proportional(12.5), t.text_dim);
    let at = frame.response.rect.min + egui::vec2(10.0, -galley.size().y * 0.5);
    ui.painter().rect_filled(egui::Rect::from_min_size(at - egui::vec2(4.0, 0.0), galley.size() + egui::vec2(8.0, 0.0)), 0.0, t.panel);
    ui.painter().galley(at, galley, t.text_dim);
}

fn check(ui: &mut egui::Ui, value: &mut bool, text: &str) {
    if crate::widgets::check(ui, text, *value, true) {
        *value = !*value;
    }
}

fn stroke_types(ui: &mut egui::Ui, id: &str, value: &mut String, color: &mut [u8; 3]) {
    let t = crate::theme::Tokens::get(ui.ctx());
    ui.push_id(id, |ui| {
        for (kind, tip, step, length) in
            [("solid", tl!("Solid"), 18.0, 18.0), ("dashed", tl!("Dashed"), 7.0, 4.0), ("dotted", tl!("Dotted"), 4.0, 1.5)]
        {
            let (r, response) = ui.allocate_exact_size(egui::vec2(26.0, 24.0), egui::Sense::click());
            ui.painter().rect_filled(r, 1.0, if *value == kind { t.tool_active } else { t.input });
            ui.painter().rect_stroke(r, 1.0, egui::Stroke::new(1.0, t.input_border), egui::StrokeKind::Inside);
            let mut x = r.left() + 4.0;
            while x < r.right() - 4.0 {
                ui.painter().line_segment(
                    [egui::pos2(x, r.center().y), egui::pos2((x + length).min(r.right() - 4.0), r.center().y)],
                    egui::Stroke::new(1.8, t.text_strong),
                );
                x += step;
            }
            response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, tip));
            if response.on_hover_text(tip).clicked() {
                *value = kind.into();
            }
        }
        ui.color_edit_button_srgb(color);
    });
}

fn arrow_picker(ui: &mut egui::Ui, value: &mut String) {
    let t = crate::theme::Tokens::get(ui.ctx());
    let field = ui.scope(|ui| {
        crate::widgets::combo(ui, "dimension-arrow", " ", 112.0, false, |ui| {
            let mut chosen = None;
            for (key, name) in [("closed", tl!("Closed")), ("open", tl!("Open")), ("tick", tl!("Tick")), ("none", tl!("None"))] {
                if ui.selectable_label(value == key, name).clicked() {
                    chosen = Some(key);
                }
            }
            chosen
        })
    });
    if let Some(key) = field.inner {
        *value = key.into();
    }
    let r = field.response.rect;
    let (a, b) = (egui::pos2(r.left() + 12.0, r.center().y), egui::pos2(r.right() - 30.0, r.center().y));
    let stroke = egui::Stroke::new(1.3, t.text_strong);
    ui.painter().line_segment([a, b], stroke);
    for (tip, sign) in [(a, 1.0), (b, -1.0)] {
        let p = tip + egui::vec2(sign * 6.0, -3.0);
        let q = tip + egui::vec2(sign * 6.0, 3.0);
        match value.as_str() {
            "closed" => {
                ui.painter().add(egui::Shape::convex_polygon(vec![tip, p, q], t.text_strong, egui::Stroke::NONE));
            }
            "open" => {
                ui.painter().line_segment([p, tip], stroke);
                ui.painter().line_segment([tip, q], stroke);
            }
            "tick" => {
                ui.painter().line_segment([tip - egui::vec2(3.0, 4.0), tip + egui::vec2(3.0, 4.0)], stroke);
            }
            _ => {}
        }
    }
}

fn text_buttons(ui: &mut egui::Ui, id: &str, value: &mut String, align: bool) {
    let t = crate::theme::Tokens::get(ui.ctx());
    let choices = if align {
        vec![("start", tl!("Start")), ("center", tl!("Center")), ("end", tl!("End"))]
    } else {
        vec![("above", tl!("Outside")), ("below", tl!("Inside"))]
    };
    ui.push_id(id, |ui| {
        for (index, (key, tip)) in choices.iter().enumerate() {
            let (r, response) = ui.allocate_exact_size(egui::vec2(24.0, 24.0), egui::Sense::click());
            ui.painter().rect_filled(r, 1.0, if value == key { t.tool_active } else { t.input });
            ui.painter().rect_stroke(r, 1.0, egui::Stroke::new(1.0, t.input_border), egui::StrokeKind::Inside);
            let x = if align { r.left() + 5.0 + index as f32 * 7.0 } else { r.center().x };
            let y = if align { r.center().y - 5.0 } else { r.top() + 3.0 + index as f32 * 12.0 };
            let stroke = egui::Stroke::new(1.2, t.text_strong);
            ui.painter().line_segment(
                [egui::pos2(r.left() + 3.0, r.center().y + 3.0), egui::pos2(r.right() - 3.0, r.center().y + 3.0)],
                egui::Stroke::new(1.0, t.text_dim),
            );
            ui.painter().line_segment([egui::pos2(x - 3.0, y), egui::pos2(x + 3.0, y)], stroke);
            ui.painter().line_segment([egui::pos2(x, y), egui::pos2(x, y + 6.0)], stroke);
            response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, *tip));
            if !align {
                response.context_menu(|ui| {
                    if ui.selectable_label(value == "inline", tl!("On Line")).clicked() {
                        *value = "inline".into();
                        ui.close();
                    }
                });
            }
            if response.on_hover_text(*tip).clicked() {
                *value = (*key).into();
            }
        }
    });
}

fn body(app: &mut VectorcraftApp, ui: &mut egui::Ui, d: &mut Dialog) -> bool {
    let mut o: DimensionOptions = serde_json::from_value(d.fields.get("style").cloned().unwrap_or(Value::Null)).unwrap_or_default();
    ui.spacing_mut().item_spacing = egui::vec2(5.0, 5.0);
    group(ui, tl!("Units & Scale"), |ui| {
        pair(
            ui,
            |ui| {
                label(ui, tl!("Units:"), 56.0);
                let units: Vec<_> = Unit::ALL.iter().map(|u| tl!(u.label())).collect();
                if let Some(unit) =
                    crate::widgets::dropdown_names(ui, "dimension-unit", tl!(o.unit.label()), &units, 120.0).and_then(|i| Unit::ALL.get(i))
                {
                    o.unit = *unit;
                }
            },
            |ui| {
                label(ui, tl!("Precision:"), 56.0);
                if let Some(i) = crate::widgets::dropdown_names(
                    ui,
                    "dimension-precision",
                    PRECISION_FORMATS.get(usize::from(o.precision)).copied().unwrap_or("0.0"),
                    &PRECISION_FORMATS,
                    108.0,
                ) {
                    o.precision = i.min(6) as u8;
                }
            },
        );
        pair(
            ui,
            |ui| {
                ui.add_space(56.0);
                check(ui, &mut o.hide_unit, tl!("Hide Units"));
            },
            |ui| {
                label(ui, tl!("Scale:"), 56.0);
                let shown = if o.scale >= 1.0 { format!("1:{}", o.scale) } else { format!("{}:1", 1.0 / o.scale) };
                if let Some(scale) = crate::widgets::combo(ui, "dimension-scale", &shown, 108.0, false, |ui| {
                    let mut out = None;
                    for scale in [0.1, 0.2, 0.5, 1.0, 2.0, 5.0, 10.0, 20.0, 50.0, 100.0, 200.0, 500.0, 1000.0] {
                        let text = if scale >= 1.0 { format!("1:{scale}") } else { format!("{}:1", 1.0 / scale) };
                        if ui.selectable_label(o.scale == scale, text).clicked() {
                            out = Some(scale);
                        }
                    }
                    ui.label(tl!("Scale (1:n):"));
                    if let Some(v) = crate::widgets::range_field(ui, "custom-scale", o.scale, 0.000001..=1e6, "", 6, 100.0) {
                        out = Some(v);
                    }
                    out
                }) {
                    o.scale = scale;
                }
            },
        );
    });
    group(ui, tl!("Dimension Lines"), |ui| {
        pair(
            ui,
            |ui| {
                label(ui, tl!("Weight:"), 56.0);
                length(ui, "dimension-weight", &mut o.line_width, 0.05..=100.0, Unit::Points, 112.0);
            },
            |ui| stroke_types(ui, "dimension-line", &mut o.line_type, &mut o.line_color),
        );
        pair(
            ui,
            |ui| {
                label(ui, tl!("Arrowheads:"), 56.0);
                arrow_picker(ui, &mut o.arrow);
            },
            |ui| {
                label(ui, tl!("Scale:"), 56.0);
                number(ui, "arrow-scale", &mut o.arrow_scale, 1.0..=1000.0, "%", 108.0);
            },
        );
    });
    group(ui, tl!("Extension Lines"), |ui| {
        pair(
            ui,
            |ui| {
                label(ui, tl!("Weight:"), 56.0);
                length(ui, "extension-weight", &mut o.extension_width, 0.05..=100.0, Unit::Points, 112.0);
            },
            |ui| stroke_types(ui, "dimension-extension", &mut o.extension_type, &mut o.extension_color),
        );
        pair(
            ui,
            |ui| {
                label(ui, tl!("Gap:"), 56.0);
                length(ui, "extension-gap", &mut o.extension_gap, 0.0..=10000.0, Unit::Points, 112.0);
            },
            |ui| {
                label(ui, tl!("Overrun:"), 72.0);
                length(ui, "extension-overrun", &mut o.extension_overrun, 0.0..=10000.0, Unit::Points, 92.0);
            },
        );
        ui.horizontal(|ui| {
            ui.add_space(196.0);
            check(ui, &mut o.hide_extensions, tl!("Hide Extension Lines"));
        });
    });
    group(ui, tl!("Dimension Text"), |ui| {
        ui.horizontal(|ui| {
            label(ui, tl!("Font Family:"), 78.0);
            let pick = crate::font_menu::font_menu(ui, "dimension-font", &o.font_family, 270.0, None, crate::font_menu::MenuLook::of(app));
            if let Some((family, style)) = crate::font_menu::picked(app, pick) {
                o.font_family = family;
                if let Some(style) = style {
                    o.font_style = style;
                }
            }
        });
        ui.horizontal(|ui| {
            label(ui, tl!("Font Style:"), 78.0);
            let styles = vectorcraft_text::FontDb::global().styles(&o.font_family);
            let names: Vec<_> = styles.iter().map(String::as_str).collect();
            if let Some(name) = crate::widgets::dropdown_names(ui, "dimension-font-style", &o.font_style, &names, 270.0).and_then(|i| names.get(i)) {
                o.font_style = (*name).into();
            }
        });
        ui.horizontal(|ui| {
            label(ui, tl!("Font Size:"), 78.0);
            let mut unit: Unit = d.fields.get("fontUnit").and_then(|v| serde_json::from_value(v.clone()).ok()).unwrap_or(Unit::Millimeters);
            length(ui, "dimension-font-size", &mut o.font_size, 1.0..=1000.0, unit, 104.0);
            let units = [Unit::Millimeters, Unit::Points, Unit::Pixels];
            let names: Vec<_> = units.iter().map(|u| tl!(u.label())).collect();
            if let Some(pick) = crate::widgets::dropdown_names(ui, "dimension-font-unit", tl!(unit.label()), &names, 104.0).and_then(|i| units.get(i))
            {
                unit = *pick;
            }
            d.fields.insert("fontUnit".into(), json!(unit));
            ui.color_edit_button_srgb(&mut o.text_color);
        });
        ui.horizontal(|ui| {
            label(ui, tl!("Position:"), 56.0);
            text_buttons(ui, "dimension-text-position", &mut o.text_position, false);
            text_buttons(ui, "dimension-text-align", &mut o.text_align, true);
            label(ui, tl!("Offset:"), 44.0);
            length(ui, "dimension-text-offset", &mut o.text_offset, 0.0..=10000.0, Unit::Points, 72.0);
        });
    });
    d.fields.insert("style".into(), json!(o));
    false
}

fn show(app: &mut VectorcraftApp, ctx: &egui::Context) {
    let Some(mut d) = app.ui.dialog.clone() else { return };
    let mut save = false;
    let mut cancel = false;
    super::modal::show(ctx, tl!("Dimension Tool Options"), egui::Id::new(("dialog", KIND)), -20.0, 16, |ui| {
        ui.set_width(420.0);
        super::modal::heading(ui, tl!("Dimension Tool Options"));
        ui.add_space(8.0);
        egui::ScrollArea::vertical().max_height((ctx.content_rect().height() - 140.0).max(160.0)).show(ui, |ui| {
            body(app, ui, &mut d);
        });
        ui.add_space(18.0);
        ui.horizontal(|ui| {
            if crate::widgets::secondary_button(ui, tl!("Reset")).clicked() {
                d.fields.insert("style".into(), json!(DimensionOptions::default()));
            }
            ui.allocate_ui_with_layout(egui::vec2(ui.available_width(), 28.0), egui::Layout::right_to_left(egui::Align::Center), |ui| {
                cancel = crate::widgets::secondary_button(ui, tl!("Cancel")).clicked();
                ui.add_space(8.0);
                save = crate::widgets::primary_button(ui, tl!("Save")).clicked();
            });
        });
    });
    app.ui.dialog = Some(d);
    if cancel || ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
        super::cancel(app);
    } else if (save || ctx.input(|i| i.key_pressed(egui::Key::Enter)))
        && let Err(e) = super::confirm(app)
    {
        app.status(e);
    }
}

fn confirm(app: &mut VectorcraftApp, d: &Dialog) -> DialogResult {
    let style = d.fields.get("style").cloned().unwrap_or(Value::Null);
    let o: DimensionOptions = serde_json::from_value(style.clone()).map_err(|e| e.to_string())?;
    o.validate()?;
    let result = if let Some(id) = d.fields.get("id").and_then(Value::as_u64) {
        app.run("dimension.update", json!({"id":id,"options":style}))?
    } else {
        app.run("tool.setOption", json!({"tool":"dimension","values":{"style":style}}))?
    };
    app.ui.dialog = None;
    Ok(result)
}

fn mode(ui: &mut egui::Ui, text: &str, selected: bool) -> bool {
    let t = crate::theme::Tokens::get(ui.ctx());
    ui.add(
        egui::Button::selectable(selected, text).fill(if selected { t.tool_active } else { t.input }).stroke(egui::Stroke::new(1.0, t.input_border)),
    )
    .clicked()
}

/// Dimension types stay reachable in Essentials even with the optional Control bar hidden.
pub(crate) fn floating_bar(app: &mut VectorcraftApp, ui: &egui::Ui, canvas: egui::Rect) {
    if app.session.tool_id() != "dimension" || app.session.active().is_none() || app.ui.dialog.is_some() {
        return;
    }
    let opts = app.session.tool_options_of("dimension");
    let kind = opts["kind"].as_str().unwrap_or("linear");
    let axis = opts["axis"].as_str().unwrap_or("aligned");
    let t = crate::theme::Tokens::get(ui.ctx());
    let width = (canvas.width() - 32.0).clamp(240.0, 520.0);
    egui::Area::new(egui::Id::new("dimension-tool-bar"))
        .order(egui::Order::Middle)
        .default_pos(egui::pos2((canvas.center().x - width * 0.5).max(canvas.left() + 8.0), canvas.top() + 16.0))
        .constrain(true)
        .show(ui.ctx(), |ui| {
            egui::Frame::NONE.fill(t.panel).stroke(egui::Stroke::new(1.0, t.input_border)).corner_radius(4).inner_margin(8).show(ui, |ui| {
                ui.set_width(width);
                ui.horizontal_wrapped(|ui| {
                    ui.label(tl!("Dimension Tool"));
                    for (value, label) in
                        [("linear", tl!("Linear")), ("angular", tl!("Angular")), ("radius", tl!("Radius")), ("diameter", tl!("Diameter"))]
                    {
                        if mode(ui, label, kind == value && (kind != "linear" || axis == "aligned")) {
                            app.run("tool.setOption", json!({"tool":"dimension","values":{"kind":value,"axis":"aligned"}})).ok();
                        }
                    }
                });
                ui.horizontal_wrapped(|ui| {
                    ui.label(tl!("Rectangle"));
                    for (value, label) in [("horizontal", tl!("Width")), ("vertical", tl!("Height"))] {
                        if mode(ui, label, kind == "linear" && axis == value) {
                            app.run("tool.setOption", json!({"tool":"dimension","values":{"kind":"linear","axis":value}})).ok();
                        }
                    }
                    if crate::widgets::secondary_button(ui, tl!("Dimension Options…")).clicked() {
                        open(app);
                    }
                });
            });
        });
}

pub fn control_bar(app: &mut VectorcraftApp, ui: &mut egui::Ui) {
    let opts = app.session.tool_options_of("dimension");
    let mut kind = opts["kind"].as_str().unwrap_or("linear").to_string();
    let before = kind.clone();
    choose(
        ui,
        "dimension-kind",
        &mut kind,
        &[("linear", tl!("Linear")), ("angular", tl!("Angular")), ("radius", tl!("Radius")), ("diameter", tl!("Diameter"))],
        125.0,
    );
    if kind != before {
        app.run("tool.setOption", json!({"tool":"dimension","values":{"kind":kind,"axis":"aligned"}})).ok();
    }
    if ui.button(tl!("Dimension Options…")).clicked() {
        open(app);
    }
    for (axis, label) in [("horizontal", "Width"), ("vertical", "Height")] {
        if ui
            .add_enabled(
                app.session
                    .active()
                    .is_some_and(|a| a.selection.objects.iter().any(|id| a.doc.node(*id).is_some_and(|n| n.extras().dimension.is_none()))),
                egui::Button::new(tl!(label)),
            )
            .clicked()
        {
            app.run("dimension.fromSelection", json!({"axis":axis,"options":opts["style"]})).ok();
        }
    }
    ui.label(tl!("Pick points, then place label; Esc cancels"));
}

#[cfg(test)]
mod tests {
    use super::*;
    use vectorcraft_engine::Session;

    fn app() -> VectorcraftApp {
        let mut app = VectorcraftApp::new(Session::new(), Default::default());
        app.run("file.new", json!({"width":600,"height":400})).unwrap();
        app
    }

    #[test]
    fn dimension_options_defaults_cancel_confirm_and_persist_across_tool_switches() {
        let mut app = app();
        app.run("tool.options", json!({"tool":"dimension"})).unwrap();
        assert_eq!(app.ui.dialog.as_ref().unwrap().kind, KIND);
        let mut d = app.ui.dialog.clone().unwrap();
        d.fields.insert("style".into(), json!(DimensionOptions { precision: 3, scale: 100.0, ..Default::default() }));
        confirm(&mut app, &d).unwrap();
        app.select_tool("rectangle");
        app.select_tool("dimension");
        assert_eq!(app.session.tool_options()["style"]["scale"], 100.0);
        assert_eq!(app.session.tool_options()["style"]["precision"], 3);
        open(&mut app);
        app.ui.dialog.as_mut().unwrap().fields.insert("style".into(), json!(DimensionOptions { font_size: 22.0, ..Default::default() }));
        super::super::cancel(&mut app);
        assert_eq!(app.session.tool_options()["style"]["fontSize"], 12.0);
    }

    #[test]
    fn dimension_options_edits_selected_annotation_and_preserves_defaults() {
        let mut app = app();
        app.run("dimension.create", json!({"kind":"linear","points":[[20,60],[92,60]],"placement":[50,30]})).unwrap();
        let before = app.session.tool_options_of("dimension");
        open(&mut app);
        let mut d = app.ui.dialog.clone().unwrap();
        assert!(d.fields["id"].is_number());
        d.fields.insert("style".into(), json!(DimensionOptions { precision: 2, hide_unit: true, ..Default::default() }));
        assert_eq!(confirm(&mut app, &d).unwrap()["label"], "25.40");
        assert_eq!(app.session.tool_options_of("dimension"), before);
        app.run("edit.undo", json!({})).unwrap();
        assert_eq!(app.run("dimension.inspect", json!({})).unwrap()["spec"]["options"]["precision"], 1);
    }

    #[test]
    fn dimension_precision_shows_decimal_format_instead_of_digit_count() {
        let mut app = app();
        app.run("dimension.create", json!({"kind":"linear","points":[[20,60],[92,60]],"placement":[50,30]})).unwrap();
        for (precision, format, expected) in [(0, "0", "25 mm"), (1, "0.0", "25.4 mm"), (2, "0.00", "25.40 mm"), (3, "0.000", "25.400 mm")] {
            open(&mut app);
            let mut d = app.ui.dialog.clone().unwrap();
            d.fields.insert("style".into(), json!(DimensionOptions { precision, ..Default::default() }));
            app.ui.dialog = Some(d.clone());
            let text = crate::tests_labels::painted_text(&mut app, |app, ui| super::super::show(app, ui.ctx()));
            assert!(text.lines().any(|line| line == format), "precision {precision} missing {format}: {text}");
            assert_eq!(confirm(&mut app, &d).unwrap()["label"], expected);
        }
    }

    #[test]
    fn dimension_options_draws_every_settings_section() {
        let mut app = app();
        open(&mut app);
        let text = crate::tests_labels::painted_text(&mut app, |app, ui| super::super::show(app, ui.ctx()));
        for label in ["Dimension Tool Options", "Units & Scale", "Dimension Lines", "Extension Lines", "Dimension Text"] {
            assert!(text.contains(label), "missing {label}: {text}");
        }
        if let Some(path) = std::env::var_os("VECTORCRAFT_DIMENSION_CAPTURE") {
            let mut ready = false;
            let mut harness = egui_kittest::Harness::builder().with_size(egui::vec2(780.0, 960.0)).wgpu().build_ui_state(
                move |ui, app: &mut VectorcraftApp| {
                    if !ready {
                        egui_extras::install_image_loaders(ui.ctx());
                        crate::theme::install_fonts(ui.ctx());
                        crate::theme::apply(ui.ctx(), crate::theme::Brightness::MediumDark);
                        ready = true;
                        return;
                    }
                    super::super::show(app, ui.ctx());
                },
                app,
            );
            harness.run_steps(4);
            harness.render().unwrap().save(path).unwrap();
        }
    }

    #[test]
    fn dimension_modes_and_rectangle_sizes_show_without_optional_bars_or_selection() {
        let mut app = app();
        app.select_tool("dimension");
        app.ui.control_bar = false;
        app.ui.task_bar = false;
        let text = crate::tests_labels::painted_text(&mut app, |app, ui| {
            floating_bar(app, ui, egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(900.0, 700.0)))
        });
        for label in ["Linear", "Angular", "Radius", "Diameter", "Rectangle", "Width", "Height", "Dimension Options…"] {
            assert!(text.contains(label), "missing {label}: {text}");
        }
        assert_eq!(app.session.tool_options()["kind"], "linear");
    }
}
