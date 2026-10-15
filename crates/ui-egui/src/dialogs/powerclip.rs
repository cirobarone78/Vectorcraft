//! PowerClip Options dialog: 300px x 200px dialog configuring default positioning and fit mode.

use serde_json::{Value, json};

use super::DialogSpec;
use crate::state::Dialog;
use crate::{VectorcraftApp, widgets};

/// The dialog kind of PowerClip Options.
pub const KIND: &str = "powerclipOptions";

pub(super) const SPEC: DialogSpec =
    DialogSpec { heading: |_| tl!("PowerClip Options").into(), body, confirm, min_width: 256.0, max_width: Some(256.0), ..DialogSpec::FORM };

/// Prompt with the PowerClip dialog if not suppressed, or place directly using saved preferences.
pub fn prompt_or_place(app: &mut VectorcraftApp, container: Option<u64>, contents: Vec<u64>) -> Result<Value, String> {
    let (suppress, auto_center, fit_mode) = {
        let prefs = &app.ui.powerclip_options;
        let suppress = prefs.get("suppress_dialog").and_then(Value::as_bool).unwrap_or(false);
        let auto_center = prefs.get("auto_center").or_else(|| prefs.get("center")).and_then(Value::as_bool).unwrap_or(true);
        let fit_mode = prefs.get("fit_mode").and_then(Value::as_str).unwrap_or("none").to_string();
        (suppress, auto_center, fit_mode)
    };

    if !suppress {
        let mut fields = json!({
            "center": auto_center,
            "auto_center": auto_center,
            "fit_mode": fit_mode,
            "dont_show_again": false,
            "pending_place": true,
        });
        if let Some(c) = container {
            fields["container"] = json!(c);
        }
        if !contents.is_empty() {
            fields["contents"] = json!(contents);
        }
        app.ui.dialog = Some(Dialog::new(KIND, fields));
        Ok(Value::Null)
    } else {
        apply_placement(app, container, contents, auto_center, &fit_mode)
    }
}

/// Helper to execute PowerClip placement or switch to targeting tool if container is needed.
fn apply_placement(app: &mut VectorcraftApp, container: Option<u64>, contents: Vec<u64>, center: bool, fit_mode: &str) -> Result<Value, String> {
    let fit_param = if fit_mode == "none" { Value::Null } else { json!(fit_mode) };

    // 1. Explicit container provided (e.g. from PowerClip tool or dialog fields)
    if let Some(c) = container {
        let mut params = json!({
            "container": c,
            "center": center,
            "fit": fit_param,
        });
        if !contents.is_empty() {
            params["contents"] = json!(contents);
        }
        return app.run("object.powerclip.placeInside", params);
    }

    // 2. Check current selection: if 2+ objects selected, place inside topmost
    let sel = app.session.active().map(|st| st.selection.objects.clone()).unwrap_or_default();
    if sel.len() >= 2 {
        let mut params = json!({
            "center": center,
            "fit": fit_param,
        });
        if !contents.is_empty() {
            params["contents"] = json!(contents);
        }
        return app.run("object.powerclip.placeInside", params);
    }

    // 3. If only 1 object selected, and there is exactly 1 other root object on canvas: use it as container!
    let other_roots = app
        .session
        .active()
        .map(|st| {
            let mut objs = Vec::new();
            for l in &st.doc.layers {
                if let Some(ch) = l.children() {
                    for c in ch {
                        if !st.selection.contains(c.id) {
                            objs.push(c.id);
                        }
                    }
                }
            }
            objs
        })
        .unwrap_or_default();

    if other_roots.len() == 1 {
        let target = other_roots[0];
        let sel_contents = if !contents.is_empty() {
            contents
        } else {
            app.session.active().map(|st| st.selection.objects.iter().map(|n| n.0).collect::<Vec<_>>()).unwrap_or_default()
        };
        let params = json!({
            "container": target.0,
            "contents": sel_contents,
            "center": center,
            "fit": fit_param,
        });
        return app.run("object.powerclip.placeInside", params);
    }

    // 4. Otherwise, switch to PowerClip tool and prompt user to click container frame
    app.select_tool("powerclip");
    app.status(tl!("Click a container frame to place inside"));
    Ok(Value::Null)
}

/// Open the PowerClip Options dialog (Object > PowerClip > PowerClip Options…).
pub fn open_options(app: &mut VectorcraftApp) -> Result<Value, String> {
    let prefs = &app.ui.powerclip_options;
    let auto_center = prefs.get("auto_center").or_else(|| prefs.get("center")).and_then(Value::as_bool).unwrap_or(true);
    let fit_mode = prefs.get("fit_mode").and_then(Value::as_str).unwrap_or("none");
    let suppress = prefs.get("suppress_dialog").and_then(Value::as_bool).unwrap_or(false);

    let fields = json!({
        "center": auto_center,
        "auto_center": auto_center,
        "fit_mode": fit_mode,
        "dont_show_again": suppress,
        "pending_place": false,
    });
    app.ui.dialog = Some(Dialog::new(KIND, fields));
    Ok(Value::Null)
}

fn body(_app: &mut VectorcraftApp, ui: &mut egui::Ui, d: &mut Dialog) -> bool {
    let center = d.fields.get("center").or_else(|| d.fields.get("auto_center")).and_then(Value::as_bool).unwrap_or(true);
    let fit_mode_val = d.str("fit_mode");
    let is_fill = fit_mode_val == "fillFrame";
    let is_fit = fit_mode_val == "fitProportional";
    let is_stretch = fit_mode_val == "stretch";
    let dont_show = d.fields.get("dont_show_again").and_then(Value::as_bool).unwrap_or(false);

    ui.spacing_mut().item_spacing.y = 5.0;

    // 1. Center
    if widgets::check(ui, tl!("Center"), center, true) {
        d.fields.insert("center".into(), json!(!center));
        d.fields.insert("auto_center".into(), json!(!center));
    }

    // 2. Fill (Fill frame proportionally)
    if widgets::check(ui, tl!("Fill"), is_fill, true) {
        let new_mode = if is_fill { "none" } else { "fillFrame" };
        d.fields.insert("fit_mode".into(), json!(new_mode));
    }

    // 3. Fit (Fit content proportionally)
    if widgets::check(ui, tl!("Fit"), is_fit, true) {
        let new_mode = if is_fit { "none" } else { "fitProportional" };
        d.fields.insert("fit_mode".into(), json!(new_mode));
    }

    // 4. Stretch (Stretch to fill)
    if widgets::check(ui, tl!("Stretch"), is_stretch, true) {
        let new_mode = if is_stretch { "none" } else { "stretch" };
        d.fields.insert("fit_mode".into(), json!(new_mode));
    }

    ui.add_space(4.0);
    ui.separator();
    ui.add_space(4.0);

    // 5. Don't show again
    if widgets::check(ui, tl!("Don't show again"), dont_show, true) {
        d.fields.insert("dont_show_again".into(), json!(!dont_show));
    }

    ui.set_min_height(88.0);
    false
}

fn confirm(app: &mut VectorcraftApp, d: &Dialog) -> Result<Value, String> {
    let center = d.fields.get("center").or_else(|| d.fields.get("auto_center")).and_then(Value::as_bool).unwrap_or(true);
    let fit_mode_val = d.str("fit_mode");
    let fit_mode = if fit_mode_val.is_empty() { "none" } else { fit_mode_val.as_str() };
    let dont_show = d.fields.get("dont_show_again").and_then(Value::as_bool).unwrap_or(false);

    // Persist user preferences in UiState
    app.ui.powerclip_options = json!({
        "auto_center": center,
        "center": center,
        "fit_mode": fit_mode,
        "suppress_dialog": dont_show,
    });

    let pending_place = d.fields.get("pending_place").and_then(Value::as_bool).unwrap_or(false);
    let container = d.fields.get("container").and_then(Value::as_u64);
    let contents =
        d.fields.get("contents").and_then(Value::as_array).map(|arr| arr.iter().filter_map(Value::as_u64).collect::<Vec<_>>()).unwrap_or_default();

    app.ui.dialog = None;

    if pending_place {
        apply_placement(app, container, contents, center, fit_mode)?;
    }

    Ok(Value::Null)
}
