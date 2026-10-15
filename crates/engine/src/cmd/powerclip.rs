//! PowerClip commands (CorelDRAW-class container clipping).
//!
//! Unlike standard Illustrator clipping masks which strip the container's appearance,
//! PowerClip preserves the container's fill, stroke, effects, and opacity. The container's
//! fill renders underneath the clipped contents, and its stroke renders on top of the
//! clipped contents.

use serde_json::{Value, json};
use vectorcraft_doc::{Node, NodeId, NodeKind, Scaling};
use vectorcraft_geom::{Affine, Rect};

use super::{CommandSpec, cmd, has_doc, has_selection};
use crate::cmd::edit::selected_roots;
use crate::{EngineError, Result, Session};

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(
            "object.powerclip.placeInside",
            "Place Inside Frame",
            ["Object", "PowerClip"],
            None,
            "{container?: id, contents?: [id], center?: bool, fit?: none|fitProportional|fillProportional|stretchToFill} places selected/specified objects inside the container frame (a path, shape, compound path or text), preserving the container's fill and stroke paint → {id} of the PowerClip group",
            has_selection,
            powerclip_place_inside
        ),
        cmd!(
            "object.powerclip.extract",
            "Extract Contents",
            ["Object", "PowerClip"],
            None,
            "{id?: id} extracts all contents from selected PowerClip group(s) back onto the canvas while restoring the container frame with its paint intact → {count}",
            has_selection,
            powerclip_extract
        ),
        cmd!(
            "object.powerclip.editContents",
            "Edit PowerClip",
            ["Object", "PowerClip"],
            None,
            "{id?: id} enters isolation mode to edit the contents inside the selected PowerClip frame → {id}",
            has_selection,
            powerclip_edit_contents
        ),
        cmd!(
            "object.powerclip.finishEdit",
            "Finish Editing PowerClip",
            ["Object", "PowerClip"],
            None,
            "{} exits PowerClip isolation mode back to the main document",
            has_doc,
            super::object::exit_isolation
        ),
        cmd!(
            "object.powerclip.center",
            "Center Contents",
            ["Object", "PowerClip"],
            None,
            "{id?: id} centers the clipped contents inside the PowerClip container frame → {id}",
            has_selection,
            powerclip_center
        ),
        cmd!(
            "object.powerclip.fit",
            "Fit PowerClip Contents",
            ["Object", "PowerClip"],
            None,
            "{id?: id, mode?: \"center\"|\"fitProportional\"|\"fillFrame\"|\"stretch\", contents?: [id]} aligns or scales the clipped contents inside the PowerClip frame → {id}",
            has_selection,
            powerclip_fit
        ),
        cmd!(
            "object.powerclip.selectContents",
            "Select PowerClip Contents",
            ["Object", "PowerClip"],
            None,
            "{id?: id} selects the contents inside the selected PowerClip frame without entering isolation mode → {count}",
            has_selection,
            powerclip_select_contents
        ),
    ]
}

/// Place `contents` inside `container` without stripping the container's appearance.
fn powerclip_place_inside(s: &mut Session, p: &Value) -> Result<Value> {
    let container_param = p.get("container").and_then(Value::as_u64).map(NodeId);
    let contents_param = p.get("contents").and_then(Value::as_array).map(|arr| arr.iter().filter_map(|v| v.as_u64().map(NodeId)).collect::<Vec<_>>());

    let (container, contents) = match (container_param, contents_param) {
        (Some(c), Some(conts)) => (c, conts),
        (Some(c), None) => {
            let sel = selected_roots(s)?;
            let conts = sel.into_iter().filter(|id| *id != c).collect::<Vec<_>>();
            if conts.is_empty() {
                return Err(EngineError::Other("no objects to place inside PowerClip frame".into()));
            }
            (c, conts)
        }
        (None, _) => {
            let sel = selected_roots(s)?;
            if sel.len() < 2 {
                return Err(EngineError::Other("select both content objects and a container frame, or specify `container`".into()));
            }
            // By default, topmost selected object is the container frame
            let c = *sel.last().ok_or_else(|| EngineError::Other("nothing selected".into()))?;
            let conts = sel[..sel.len() - 1].to_vec();
            (c, conts)
        }
    };

    let gid = s.edit("PowerClip Inside", |d, sel| {
        // Resolve target container: if container or any ancestor is an existing clip group, use it
        let mut target_container = container;
        let mut cur = Some(container);
        while let Some(c) = cur {
            if let Some(n) = d.node(c)
                && n.clips()
            {
                target_container = c;
                break;
            }
            cur = d.position(c).and_then(|(par, _, _)| par);
        }

        let cn = d.node(target_container).ok_or(EngineError::NoNode(target_container))?;

        // If container is already an existing clip group:
        if cn.clips() {
            for id in &contents {
                if *id != target_container {
                    d.move_node(*id, Some(target_container), usize::MAX)?;
                }
            }
            sel.set([target_container]);
            return Ok(target_container);
        }

        // Must be a path, compound path, or text to clip
        if !matches!(cn.kind, NodeKind::Path { guide: false, .. } | NodeKind::Compound { .. } | NodeKind::Text(_)) {
            return Err(EngineError::Other("the container must be a path, shape, compound path or text".into()));
        }

        // Set clipping = true on path WITHOUT wiping appearance (PowerClip preserves paint!)
        let cn_mut = d.node_mut(container).ok_or(EngineError::NoNode(container))?;
        if let NodeKind::Path { clipping, .. } = &mut cn_mut.kind {
            *clipping = true;
        }

        let (par, idx, _) = d.position(container).ok_or(EngineError::NoNode(container))?;
        let gid = d.alloc_id();
        let mut group = Node::new(gid, NodeKind::Group { children: vec![], clip: true });
        group.name = Some("PowerClip".into());
        d.insert(par, idx + 1, group)?;
        d.move_node(container, Some(gid), 0)?;
        for id in &contents {
            if *id != container {
                d.move_node(*id, Some(gid), usize::MAX)?;
            }
        }
        sel.set([gid]);
        Ok(gid)
    })?;

    let newly_added = contents.iter().map(|id| id.0).collect::<Vec<_>>();
    if let Some(fit_mode) = p.get("fit").and_then(Value::as_str) {
        powerclip_fit(s, &json!({ "id": gid.0, "contents": newly_added, "mode": fit_mode })).ok();
    } else if p.get("center").and_then(Value::as_bool) == Some(true) {
        powerclip_fit(s, &json!({ "id": gid.0, "contents": newly_added, "mode": "center" })).ok();
    }

    Ok(json!({ "id": gid.0 }))
}

/// Extract contents from PowerClip group(s) back onto the canvas.
fn powerclip_extract(s: &mut Session, p: &Value) -> Result<Value> {
    let mut clip_groups = Vec::new();
    if let Some(target) = p.get("id").and_then(Value::as_u64).map(NodeId) {
        if s.doc()?.doc.node(target).is_some_and(|n| n.clips()) {
            clip_groups.push(target);
        }
    } else {
        for id in selected_roots(s)? {
            let mut cur = Some(id);
            while let Some(c) = cur {
                if s.doc()?.doc.node(c).is_some_and(|n| n.clips()) {
                    if !clip_groups.contains(&c) {
                        clip_groups.push(c);
                    }
                    break;
                }
                cur = s.doc()?.doc.position(c).and_then(|(par, _, _)| par);
            }
        }
    }

    if clip_groups.is_empty() {
        return Err(EngineError::Other("select a PowerClip frame to extract".into()));
    }

    let extracted = s.edit("Extract PowerClip Contents", |d, sel| {
        let mut out_ids = Vec::new();
        for gid in clip_groups {
            let Some((par, idx, _)) = d.position(gid) else { continue };
            let children = d.node(gid).and_then(|n| n.children()).map(|c| c.to_vec()).unwrap_or_default();
            if children.is_empty() {
                d.remove(gid)?;
                continue;
            }
            let container_id = children[0].id;
            // Restore container path clipping flag
            if let Some(c0) = d.node_mut(container_id)
                && let NodeKind::Path { clipping, .. } = &mut c0.kind
            {
                *clipping = false;
            }
            // Move container out to parent
            d.move_node(container_id, par, idx)?;
            out_ids.push(container_id);

            // Move contents out to parent right after container
            for (k, child) in children.iter().skip(1).enumerate() {
                d.move_node(child.id, par, idx + 1 + k)?;
                out_ids.push(child.id);
            }

            // Remove the empty group
            d.remove(gid)?;
        }
        sel.set(out_ids.iter().copied());
        Ok(out_ids.len())
    })?;

    Ok(json!({ "count": extracted }))
}

/// Enter isolation mode on the selected PowerClip frame.
fn powerclip_edit_contents(s: &mut Session, p: &Value) -> Result<Value> {
    let target = p
        .get("id")
        .and_then(Value::as_u64)
        .map(NodeId)
        .or_else(|| selected_roots(s).ok()?.into_iter().find(|id| s.doc().ok().and_then(|st| st.doc.node(*id)).is_some_and(|n| n.clips())))
        .ok_or_else(|| EngineError::Other("select a PowerClip frame to edit".into()))?;

    super::object::isolate(s, &json!({ "id": target.0 }))
}

/// Center the clipped contents within the PowerClip container frame.
fn powerclip_center(s: &mut Session, p: &Value) -> Result<Value> {
    let mut p_fit = p.clone();
    if p_fit.get("mode").is_none() {
        p_fit["mode"] = json!("center");
    }
    powerclip_fit(s, &p_fit)
}

/// Fit or position the clipped contents within the PowerClip container frame.
/// Modes: "center", "fit" / "fitProportional", "fill" / "fillFrame", "stretch".
fn powerclip_fit(s: &mut Session, p: &Value) -> Result<Value> {
    let target = p
        .get("id")
        .and_then(Value::as_u64)
        .map(NodeId)
        .or_else(|| selected_roots(s).ok()?.into_iter().find(|id| s.doc().ok().and_then(|st| st.doc.node(*id)).is_some_and(|n| n.clips())))
        .ok_or_else(|| EngineError::Other("select a PowerClip frame to fit".into()))?;

    let mode = p.get("mode").and_then(Value::as_str).unwrap_or("center");

    s.edit("Fit PowerClip Contents", |d, _| {
        let n = d.node(target).ok_or(EngineError::NoNode(target))?;
        let children = n.children().map(|c| c.to_vec()).unwrap_or_default();
        if children.len() < 2 {
            return Ok(());
        }
        let container_node = d.node(children[0].id).ok_or(EngineError::NoNode(children[0].id))?;
        let c_box = container_node.visual_bounds().ok_or_else(|| EngineError::Other("container has no bounds".into()))?;

        let target_ids: Vec<NodeId> = if let Some(conts) = p.get("contents").and_then(Value::as_array) {
            let filter_set: Vec<NodeId> = conts.iter().filter_map(|v| v.as_u64().map(NodeId)).collect();
            children[1..].iter().map(|c| c.id).filter(|id| filter_set.contains(id)).collect()
        } else {
            children[1..].iter().map(|c| c.id).collect()
        };
        if target_ids.is_empty() {
            return Ok(());
        }

        // Compute combined bounds of target contents
        let mut contents_bounds: Option<Rect> = None;
        for cid in &target_ids {
            if let Some(b) = d.node(*cid).and_then(|n| n.visual_bounds()) {
                contents_bounds = Some(match contents_bounds {
                    Some(prev) => prev.union(b),
                    None => b,
                });
            }
        }
        let Some(content_b) = contents_bounds else { return Ok(()) };

        let c_w = c_box.width();
        let c_h = c_box.height();
        let b_w = content_b.width();
        let b_h = content_b.height();

        let c_cx = c_box.x0 + c_w * 0.5;
        let c_cy = c_box.y0 + c_h * 0.5;
        let b_cx = content_b.x0 + b_w * 0.5;
        let b_cy = content_b.y0 + b_h * 0.5;

        let (scale_x, scale_y) = if b_w > 1e-6 && b_h > 1e-6 && c_w > 1e-6 && c_h > 1e-6 {
            match mode {
                "fit" | "fitProportional" => {
                    let s = (c_w / b_w).min(c_h / b_h);
                    (s, s)
                }
                "fill" | "fillFrame" => {
                    let s = (c_w / b_w).max(c_h / b_h);
                    (s, s)
                }
                "stretch" => (c_w / b_w, c_h / b_h),
                _ => (1.0, 1.0), // "center"
            }
        } else {
            (1.0, 1.0)
        };

        if !scale_x.is_finite() || !scale_y.is_finite() || scale_x <= 0.0 || scale_y <= 0.0 {
            return Ok(());
        }

        let xf = Affine::translate((c_cx, c_cy)) * Affine::scale_non_uniform(scale_x, scale_y) * Affine::translate((-b_cx, -b_cy));

        for cid in &target_ids {
            if let Some(cn) = d.node_mut(*cid) {
                cn.transform(xf, Scaling::default());
            }
        }
        Ok(())
    })?;

    Ok(json!({ "id": target.0 }))
}

/// Select the contents of the PowerClip container frame without entering isolation mode.
fn powerclip_select_contents(s: &mut Session, p: &Value) -> Result<Value> {
    let target = p
        .get("id")
        .and_then(Value::as_u64)
        .map(NodeId)
        .or_else(|| selected_roots(s).ok()?.into_iter().find(|id| s.doc().ok().and_then(|st| st.doc.node(*id)).is_some_and(|n| n.clips())))
        .ok_or_else(|| EngineError::Other("select a PowerClip frame".into()))?;

    let st = s.doc()?;
    let n = st.doc.node(target).ok_or(EngineError::NoNode(target))?;
    let children = n.children().map(|c| c.to_vec()).unwrap_or_default();
    if children.len() < 2 {
        return Ok(json!({ "count": 0 }));
    }
    let content_ids: Vec<NodeId> = children[1..].iter().map(|c| c.id).collect();
    let count = content_ids.len();
    s.select(|_, sel| sel.set(content_ids))?;
    Ok(json!({ "count": count }))
}
