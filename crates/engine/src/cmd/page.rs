//! Corel-style document page commands.
//! A document can have multiple pages, and 1 page can have multiple artboards.

use serde_json::{Value, json};
use vectorcraft_doc::{Artboard, Page};
use vectorcraft_geom::Rect;

use super::{CommandSpec, bad, cmd, ok};
use crate::{EngineError, Result, Session};

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!("page.new", "New Page", ["Layout", "New Page"], None, "{name?, width?, height?, add_artboard?, before?}", has_doc, page_new),
        cmd!("page.delete", "Delete Page", ["Layout", "Delete Page"], None, "{index?}", has_doc, page_delete),
        cmd!("page.rename", "Rename Page", ["Layout", "Rename Page"], None, "{index, name}", has_doc, page_rename),
        cmd!("page.duplicate", "Duplicate Page", ["Layout", "Duplicate Page"], None, "{index}", has_doc, page_duplicate),
        cmd!("page.reorder", "Reorder Page", ["Layout", "Reorder Page"], None, "{index, to}", has_doc, page_reorder),
        cmd!(
            "page.addArtboard",
            "Add Artboard to Page",
            ["Layout", "Add Artboard to Page"],
            None,
            "{page?, name?, width?, height?}",
            has_doc,
            page_add_artboard
        ),
        cmd!(
            "page.assignArtboard",
            "Assign Artboard to Page",
            ["Layout", "Assign Artboard to Page"],
            None,
            "{artboard_index, page}",
            has_doc,
            page_assign_artboard
        ),
        cmd!(query "page.list", "List Pages", ["Layout", "List Pages"], None, "{}", has_doc, page_list),
    ]
}

fn has_doc(s: &Session) -> std::result::Result<(), String> {
    if s.active().is_some() { Ok(()) } else { Err("no document open".into()) }
}

fn page_new(s: &mut Session, p: &Value) -> Result<Value> {
    s.edit("New Page", |d, _| {
        // Ensure pages is populated
        if d.pages.is_empty() {
            d.pages = vec![Page { id: 0, name: "Page 1".into() }];
        }
        let new_id = d.next_page_id();
        let page_num = d.pages.len() + 1;
        let name = p.get("name").and_then(Value::as_str).map(str::to_string).unwrap_or_else(|| format!("Page {page_num}"));

        let before = p.get("before").and_then(Value::as_u64).map(|i| i as usize);
        let insert_idx = match before {
            Some(idx) if idx < d.pages.len() => {
                d.pages.insert(idx, Page { id: new_id, name: name.clone() });
                idx
            }
            _ => {
                d.pages.push(Page { id: new_id, name: name.clone() });
                d.pages.len() - 1
            }
        };

        let add_artboard = p.get("add_artboard").and_then(Value::as_bool).unwrap_or(true);
        let mut artboard_idx = None;
        if add_artboard {
            let last_rect = d.artboards.last().map(|a| a.rect).unwrap_or(Rect::new(0.0, 0.0, 612.0, 792.0));
            let w = p.get("width").and_then(Value::as_f64).unwrap_or_else(|| last_rect.width()).max(1.0);
            let h = p.get("height").and_then(Value::as_f64).unwrap_or_else(|| last_rect.height()).max(1.0);

            let max_x = d.artboards.iter().map(|a| a.rect.x1).fold(0.0_f64, f64::max);
            let x0 = if d.artboards.is_empty() { 0.0 } else { max_x + 100.0 };
            let rect = Rect::new(x0, 0.0, x0 + w, h);
            let aid = d.next_artboard_id();
            let ab_name = format!("{name} - Artboard 1");
            d.artboards.push(Artboard { id: aid, name: ab_name, rect, show_center_mark: false, show_cross_hairs: false, page: new_id });
            artboard_idx = Some(d.artboards.len() - 1);
        }

        Ok(json!({
            "index": insert_idx,
            "id": new_id,
            "name": name,
            "artboard_index": artboard_idx,
        }))
    })
}

fn page_delete(s: &mut Session, p: &Value) -> Result<Value> {
    s.edit("Delete Page", |d, _| {
        if d.pages.is_empty() {
            d.pages = vec![Page { id: 0, name: "Page 1".into() }];
        }
        if d.pages.len() <= 1 {
            return Err(EngineError::Other("a document needs at least one page".into()));
        }
        let idx = p.get("index").and_then(Value::as_u64).map(|i| i as usize).unwrap_or(d.pages.len() - 1);
        if idx >= d.pages.len() {
            return Err(EngineError::Other("page index out of range".into()));
        }
        let page_id = d.pages.remove(idx).id;
        // Remove artboards belonging to this page
        d.artboards.retain(|a| a.page != page_id);
        // If that left no artboards, create a fallback artboard on the first page
        if d.artboards.is_empty() {
            let first_page_id = d.pages.first().map(|p| p.id).unwrap_or(0);
            d.artboards.push(Artboard {
                id: 1,
                name: "Artboard 1".into(),
                rect: Rect::new(0.0, 0.0, 612.0, 792.0),
                show_center_mark: false,
                show_cross_hairs: false,
                page: first_page_id,
            });
        }
        Ok(json!({ "deleted": page_id, "pages_remaining": d.pages.len() }))
    })
}

fn page_rename(s: &mut Session, p: &Value) -> Result<Value> {
    let name = p.get("name").and_then(Value::as_str).ok_or_else(|| bad("page.rename", "missing name"))?;
    let idx = p.get("index").and_then(Value::as_u64).ok_or_else(|| bad("page.rename", "missing index"))? as usize;
    s.edit("Rename Page", |d, _| {
        if d.pages.is_empty() {
            d.pages = vec![Page { id: 0, name: "Page 1".into() }];
        }
        let page = d.pages.get_mut(idx).ok_or_else(|| EngineError::Other("page index out of range".into()))?;
        page.name = name.to_string();
        Ok(())
    })?;
    ok()
}

fn page_duplicate(s: &mut Session, p: &Value) -> Result<Value> {
    let idx = p.get("index").and_then(Value::as_u64).ok_or_else(|| bad("page.duplicate", "missing index"))? as usize;
    s.edit("Duplicate Page", |d, _| {
        if d.pages.is_empty() {
            d.pages = vec![Page { id: 0, name: "Page 1".into() }];
        }
        let src_page = d.pages.get(idx).ok_or_else(|| EngineError::Other("page index out of range".into()))?.clone();
        let new_id = d.next_page_id();
        let new_name = format!("{} Copy", src_page.name);

        // Find artboards belonging to src_page
        let src_artboards: Vec<Artboard> = d.artboards.iter().filter(|a| a.page == src_page.id).cloned().collect();
        let max_x = d.artboards.iter().map(|a| a.rect.x1).fold(0.0_f64, f64::max);
        let min_src_x = src_artboards.iter().map(|a| a.rect.x0).fold(0.0_f64, f64::min);
        let dx = (max_x + 100.0) - min_src_x;

        for src_ab in &src_artboards {
            let aid = d.next_artboard_id();
            let rect = Rect::new(src_ab.rect.x0 + dx, src_ab.rect.y0, src_ab.rect.x1 + dx, src_ab.rect.y1);
            let name = format!("{} Copy", src_ab.name);
            d.artboards.push(Artboard {
                id: aid,
                name,
                rect,
                show_center_mark: src_ab.show_center_mark,
                show_cross_hairs: src_ab.show_cross_hairs,
                page: new_id,
            });
        }

        d.pages.push(Page { id: new_id, name: new_name.clone() });
        let new_idx = d.pages.len() - 1;

        Ok(json!({ "index": new_idx, "id": new_id, "name": new_name }))
    })
}

fn page_reorder(s: &mut Session, p: &Value) -> Result<Value> {
    let from = p.get("index").and_then(Value::as_u64).ok_or_else(|| bad("page.reorder", "missing index"))? as usize;
    let to = p.get("to").and_then(Value::as_u64).ok_or_else(|| bad("page.reorder", "missing to"))? as usize;
    s.edit("Reorder Page", |d, _| {
        if d.pages.is_empty() {
            d.pages = vec![Page { id: 0, name: "Page 1".into() }];
        }
        if from >= d.pages.len() || to >= d.pages.len() {
            return Err(EngineError::Other("page index out of range".into()));
        }
        let item = d.pages.remove(from);
        d.pages.insert(to, item);
        Ok(())
    })?;
    ok()
}

fn page_add_artboard(s: &mut Session, p: &Value) -> Result<Value> {
    s.edit("Add Artboard to Page", |d, _| {
        if d.pages.is_empty() {
            d.pages = vec![Page { id: 0, name: "Page 1".into() }];
        }
        let page_id = match p.get("page") {
            Some(Value::Number(n)) => n.as_u64().unwrap_or(0) as u32,
            _ => d.pages.first().map(|p| p.id).unwrap_or(0),
        };

        // Find existing artboards on this page
        let page_abs: Vec<&Artboard> = d.artboards.iter().filter(|a| a.page == page_id).collect();
        let last_rect = page_abs.last().map(|a| a.rect).or_else(|| d.artboards.last().map(|a| a.rect)).unwrap_or(Rect::new(0.0, 0.0, 612.0, 792.0));

        let w = p.get("width").and_then(Value::as_f64).unwrap_or_else(|| last_rect.width()).max(1.0);
        let h = p.get("height").and_then(Value::as_f64).unwrap_or_else(|| last_rect.height()).max(1.0);

        // Place to the right of the last artboard on this page, or max_x if first on page
        let x0 = if let Some(last) = page_abs.last() {
            last.rect.x1 + 30.0
        } else {
            let max_x = d.artboards.iter().map(|a| a.rect.x1).fold(0.0_f64, f64::max);
            if d.artboards.is_empty() { 0.0 } else { max_x + 100.0 }
        };

        let rect = Rect::new(x0, last_rect.y0, x0 + w, last_rect.y0 + h);
        let aid = d.next_artboard_id();
        let ab_num = page_abs.len() + 1;
        let page_name = d.pages.iter().find(|p| p.id == page_id).map(|p| p.name.as_str()).unwrap_or("Page");
        let name = p.get("name").and_then(Value::as_str).map(str::to_string).unwrap_or_else(|| format!("{page_name} - Artboard {ab_num}"));

        d.artboards.push(Artboard { id: aid, name, rect, show_center_mark: false, show_cross_hairs: false, page: page_id });

        let ab_idx = d.artboards.len() - 1;
        Ok(json!({
            "artboard_index": ab_idx,
            "artboard_id": aid,
            "page": page_id,
        }))
    })
}

fn page_assign_artboard(s: &mut Session, p: &Value) -> Result<Value> {
    let ab_idx = p.get("artboard_index").and_then(Value::as_u64).ok_or_else(|| bad("page.assignArtboard", "missing artboard_index"))? as usize;
    let page_id = p.get("page").and_then(Value::as_u64).ok_or_else(|| bad("page.assignArtboard", "missing page"))? as u32;
    s.edit("Assign Artboard to Page", |d, _| {
        let a = d.artboards.get_mut(ab_idx).ok_or_else(|| EngineError::Other("artboard index out of range".into()))?;
        a.page = page_id;
        Ok(())
    })?;
    ok()
}

fn page_list(s: &mut Session, _p: &Value) -> Result<Value> {
    let d = &s.doc()?.doc;
    let pages = d.pages();
    let mut out = Vec::new();
    for page in &pages {
        let abs: Vec<usize> = d.artboards.iter().enumerate().filter(|(_, a)| a.page == page.id).map(|(i, _)| i).collect();
        out.push(json!({
            "id": page.id,
            "name": page.name,
            "artboards": abs,
        }));
    }
    Ok(json!({ "pages": out }))
}
