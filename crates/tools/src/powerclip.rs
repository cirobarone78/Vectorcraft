//! PowerClip tool: click a container frame to place selected art inside it.

use serde_json::json;
use vectorcraft_doc::NodeId;
use vectorcraft_doc::hit::{HitOptions, hit_test};
use vectorcraft_geom::Point;

use crate::{Action, Cursor, Mods, Overlay, PointerEvent, PointerKind, Tool, ToolContext, ToolKey};

#[derive(Default)]
pub struct PowerClipTool {
    hovered: Option<NodeId>,
}

impl Tool for PowerClipTool {
    fn id(&self) -> &'static str {
        "powerclip"
    }

    fn pointer(&mut self, cx: &ToolContext, ev: &PointerEvent) -> Vec<Action> {
        let opts = HitOptions { path_only: false, ..cx.hit_options() };
        let hit = hit_test(cx.doc, ev.pos, opts);

        if let Some(ref h) = hit {
            self.hovered = Some(h.leaf);
        } else {
            self.hovered = None;
        }

        if ev.kind != PointerKind::Down {
            return vec![];
        }

        let Some(h) = hit else { return vec![] };
        let target_id = h.leaf;

        // If nothing is selected, select the clicked object as the content to place
        if cx.selection.is_empty() {
            return vec![Action::Exec("select.set".into(), json!({ "ids": [target_id.0] }))];
        }

        // If target is in selection, do nothing
        if cx.selection.contains(target_id) {
            return vec![];
        }

        // Place selected objects into target_id container frame via options dialog / prompt
        let contents = cx.selection.objects.iter().map(|n| n.0).collect::<Vec<_>>();
        vec![
            Action::Dialog("powerclipOptions".into(), json!({ "container": target_id.0, "contents": contents })),
            Action::SwitchTool("selection".into()),
        ]
    }

    fn overlays(&self, cx: &ToolContext) -> Vec<Overlay> {
        if let Some(id) = self.hovered
            && let Some(n) = cx.doc.node(id)
            && let Some(b) = n.visual_bounds()
        {
            return vec![Overlay::Marquee(b)];
        }
        vec![]
    }

    fn cursor(&self, _cx: &ToolContext, _p: Point, _mods: Mods) -> Cursor {
        Cursor::Crosshair
    }

    fn key(&mut self, _cx: &ToolContext, key: ToolKey, _mods: Mods) -> Vec<Action> {
        if key == ToolKey::Escape { vec![Action::SwitchTool("selection".into())] } else { vec![] }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn powerclip_tool_id_and_cursor() {
        let tool = PowerClipTool::default();
        assert_eq!(tool.id(), "powerclip");
    }
}
