//! PowerClip tests: placing contents inside containers preserving container appearance,
//! extracting contents, centering contents, and editing isolation.

use serde_json::json;
use vectorcraft_geom::{Affine, Point};
use vectorcraft_render::{RenderOptions, Renderer};

use super::*;

fn rect(s: &mut Session, x: f64, y: f64, w: f64, h: f64, fill: &str) -> NodeId {
    let id = NodeId(s.execute("shape.rectangle", &json!({"x": x, "y": y, "width": w, "height": h})).unwrap()["id"].as_u64().unwrap());
    s.execute("paint.setFill", &json!({"color": fill, "ids": [id.0]})).unwrap();
    id
}

#[test]
fn powerclip_preserves_container_fill_and_stroke() {
    let mut s = Session::new();
    s.execute("file.new", &json!({"width": 200, "height": 200})).unwrap();

    // Content: blue rect at 40..100
    let content = rect(&mut s, 40.0, 50.0, 60.0, 100.0, "#0000ff");
    // Container: red fill with green stroke at 50..150
    let container = rect(&mut s, 50.0, 50.0, 100.0, 100.0, "#ff0000");
    s.execute("paint.setStroke", &json!({"color": "#00ff00", "ids": [container.0]})).unwrap();
    s.execute("stroke.set", &json!({"weight": 10, "ids": [container.0]})).unwrap();

    // Select content and container, run powerclip.placeInside
    s.execute("select.set", &json!({"ids": [content.0, container.0]})).unwrap();
    let res = s.execute("object.powerclip.placeInside", &json!({})).unwrap();
    let gid = NodeId(res["id"].as_u64().unwrap());

    let doc = &s.doc().unwrap().doc;
    let group = doc.node(gid).unwrap();
    assert!(group.clips(), "PowerClip group must clip");

    // Container is child 0: MUST PRESERVE fill and stroke (unlike Illustrator clipping mask)
    let c0 = doc.node(container).unwrap();
    assert!(c0.clip_paint().fill.is_some(), "PowerClip container must retain its fill");
    assert!(c0.clip_paint().stroke.is_some(), "PowerClip container must retain its stroke");

    // Render test: container fill shows behind clipped content, content is clipped, stroke is on top
    let opts = RenderOptions { background: Some([255, 255, 255, 255]), ..Default::default() };
    let img = Renderer::new().render(doc, 200, 200, Affine::IDENTITY, &opts);
    let pixel = |x, y| {
        let p = img.pixel(x, y);
        [p[0], p[1], p[2]]
    };

    // Outside the container
    assert_eq!(pixel(30, 100), [255, 255, 255]);
    // The clipped content (blue) inside container
    assert_eq!(pixel(70, 100), [0, 0, 255]);
    // The container's fill (red) where content is not drawn
    assert_eq!(pixel(120, 100), [255, 0, 0]);
    // The container's green stroke
    assert_eq!(pixel(45, 100), [0, 255, 0]);
}

#[test]
fn powerclip_extract_restores_container_and_contents() {
    let mut s = Session::new();
    s.execute("file.new", &json!({"width": 200, "height": 200})).unwrap();

    let content = rect(&mut s, 40.0, 50.0, 60.0, 100.0, "#0000ff");
    let container = rect(&mut s, 50.0, 50.0, 100.0, 100.0, "#ff0000");

    s.execute("select.set", &json!({"ids": [content.0, container.0]})).unwrap();
    let gid = NodeId(s.execute("object.powerclip.placeInside", &json!({})).unwrap()["id"].as_u64().unwrap());

    // Extract contents
    s.execute("select.set", &json!({"ids": [gid.0]})).unwrap();
    let ext = s.execute("object.powerclip.extract", &json!({})).unwrap();
    assert_eq!(ext["count"].as_u64().unwrap(), 2);

    let doc = &s.doc().unwrap().doc;
    assert!(doc.node(gid).is_none(), "PowerClip group must be removed after extraction");
    let c0 = doc.node(container).unwrap();
    assert!(!matches!(c0.kind, NodeKind::Path { clipping: true, .. }), "Container must no longer be a clipping path");
    assert!(doc.node(content).is_some(), "Content must be restored on canvas");
}

#[test]
fn powerclip_place_inside_with_explicit_container_param() {
    let mut s = Session::new();
    s.execute("file.new", &json!({"width": 200, "height": 200})).unwrap();

    let content = rect(&mut s, 10.0, 10.0, 20.0, 20.0, "#0000ff");
    let container = rect(&mut s, 50.0, 50.0, 80.0, 80.0, "#00ff00");

    // Only content is selected, container passed in params
    s.execute("select.set", &json!({"ids": [content.0]})).unwrap();
    let res = s.execute("object.powerclip.placeInside", &json!({"container": container.0})).unwrap();
    let gid = NodeId(res["id"].as_u64().unwrap());

    let doc = &s.doc().unwrap().doc;
    let group = doc.node(gid).unwrap();
    assert!(group.clips());
}

#[test]
fn powerclip_center_centers_contents_in_frame() {
    let mut s = Session::new();
    s.execute("file.new", &json!({"width": 200, "height": 200})).unwrap();

    // Content at (10, 10, 20, 20) -> center is (20, 20)
    let content = rect(&mut s, 10.0, 10.0, 20.0, 20.0, "#0000ff");
    // Container at (0, 0, 100, 100) -> center is (50, 50)
    let container = rect(&mut s, 0.0, 0.0, 100.0, 100.0, "#ff0000");

    s.execute("select.set", &json!({"ids": [content.0, container.0]})).unwrap();
    let gid = NodeId(s.execute("object.powerclip.placeInside", &json!({"container": container.0})).unwrap()["id"].as_u64().unwrap());

    s.execute("select.set", &json!({"ids": [gid.0]})).unwrap();
    s.execute("object.powerclip.center", &json!({})).unwrap();

    let doc = &s.doc().unwrap().doc;
    let b = doc.node(content).unwrap().visual_bounds().unwrap();
    // After centering to (50, 50), content of size 20x20 with 1 pt stroke has visual bounds 39.5..60.5
    assert!((b.x0 - 39.5).abs() < 1e-4, "Expected x0 ~ 39.5, got {}", b.x0);
    assert!((b.y0 - 39.5).abs() < 1e-4, "Expected y0 ~ 39.5, got {}", b.y0);
}

#[test]
fn powerclip_edit_and_finish_isolation() {
    let mut s = Session::new();
    s.execute("file.new", &json!({"width": 200, "height": 200})).unwrap();

    let content = rect(&mut s, 10.0, 10.0, 20.0, 20.0, "#0000ff");
    let container = rect(&mut s, 50.0, 50.0, 80.0, 80.0, "#00ff00");

    s.execute("select.set", &json!({"ids": [content.0, container.0]})).unwrap();
    let gid = NodeId(s.execute("object.powerclip.placeInside", &json!({})).unwrap()["id"].as_u64().unwrap());

    // Enter PowerClip isolation
    s.execute("select.set", &json!({"ids": [gid.0]})).unwrap();
    s.execute("object.powerclip.editContents", &json!({})).unwrap();
    assert_eq!(s.doc().unwrap().isolation, Some(gid));

    // Finish editing
    s.execute("object.powerclip.finishEdit", &json!({})).unwrap();
    assert_eq!(s.doc().unwrap().isolation, None);
}

#[test]
fn powerclip_fit_modes() {
    let mut s = Session::new();
    s.execute("file.new", &json!({"width": 300, "height": 300})).unwrap();

    // Content: 20x20 at origin
    let content = rect(&mut s, 0.0, 0.0, 20.0, 20.0, "#0000ff");
    // Container: 100x50 at origin
    let container = rect(&mut s, 0.0, 0.0, 100.0, 50.0, "#ff0000");

    s.execute("select.set", &json!({"ids": [content.0, container.0]})).unwrap();
    let gid = NodeId(s.execute("object.powerclip.placeInside", &json!({"container": container.0})).unwrap()["id"].as_u64().unwrap());

    // 1. Fit Proportional
    s.execute("select.set", &json!({"ids": [gid.0]})).unwrap();
    s.execute("object.powerclip.fit", &json!({"mode": "fitProportional"})).unwrap();
    let doc = &s.doc().unwrap().doc;
    let b = doc.node(content).unwrap().visual_bounds().unwrap();
    // Container is 100x50, content 20x20. Min ratio is 50/20 = 2.5.
    // Content width and height scaled to ~50. Centered at (50, 25): y bounds ~ 0..50, x bounds ~ 25..75.
    assert!((b.height() - 50.0).abs() < 2.0, "Expected height ~ 50, got {}", b.height());
    assert!((b.y0 - 0.0).abs() < 2.0, "Expected y0 ~ 0, got {}", b.y0);

    // 2. Stretch to Fill
    s.execute("object.powerclip.fit", &json!({"mode": "stretch"})).unwrap();
    let doc = &s.doc().unwrap().doc;
    let b = doc.node(content).unwrap().visual_bounds().unwrap();
    assert!((b.width() - 100.0).abs() < 2.0, "Expected width ~ 100, got {}", b.width());
    assert!((b.height() - 50.0).abs() < 2.0, "Expected height ~ 50, got {}", b.height());
}

#[test]
fn powerclip_select_contents() {
    let mut s = Session::new();
    s.execute("file.new", &json!({"width": 200, "height": 200})).unwrap();

    let c1 = rect(&mut s, 10.0, 10.0, 20.0, 20.0, "#0000ff");
    let c2 = rect(&mut s, 30.0, 30.0, 20.0, 20.0, "#ffff00");
    let container = rect(&mut s, 0.0, 0.0, 100.0, 100.0, "#ff0000");

    s.execute("select.set", &json!({"ids": [c1.0, c2.0, container.0]})).unwrap();
    let gid = NodeId(s.execute("object.powerclip.placeInside", &json!({"container": container.0})).unwrap()["id"].as_u64().unwrap());

    s.execute("select.set", &json!({"ids": [gid.0]})).unwrap();
    let res = s.execute("object.powerclip.selectContents", &json!({})).unwrap();
    assert_eq!(res["count"].as_u64().unwrap(), 2);

    let sel = &s.doc().unwrap().selection;
    assert!(sel.contains(c1), "Selection must contain content 1");
    assert!(sel.contains(c2), "Selection must contain content 2");
    assert!(!sel.contains(gid), "Selection must not contain group");
    assert!(!sel.contains(container), "Selection must not contain container frame");
}

#[test]
fn powerclip_full_workflow_roundtrip() {
    let mut s = Session::new();
    s.execute("file.new", &json!({"width": 200, "height": 200})).unwrap();

    let content = rect(&mut s, 10.0, 10.0, 30.0, 30.0, "#0000ff");
    let container = rect(&mut s, 0.0, 0.0, 100.0, 100.0, "#ff0000");

    // 1. Place Inside
    s.execute("select.set", &json!({"ids": [content.0, container.0]})).unwrap();
    let res = s.execute("object.powerclip.placeInside", &json!({"container": container.0})).unwrap();
    let gid = NodeId(res["id"].as_u64().unwrap());
    assert_eq!(s.doc().unwrap().doc.node(gid).unwrap().name.as_deref(), Some("PowerClip"));

    // 2. Edit Contents (Enter isolation)
    s.execute("select.set", &json!({"ids": [gid.0]})).unwrap();
    s.execute("object.powerclip.editContents", &json!({})).unwrap();
    assert_eq!(s.doc().unwrap().isolation, Some(gid));

    // 3. Move content inside isolation mode
    s.execute("select.set", &json!({"ids": [content.0]})).unwrap();
    s.execute("object.move", &json!({"dx": 15.0, "dy": 15.0})).unwrap();

    // 4. Finish Editing
    s.execute("object.powerclip.finishEdit", &json!({})).unwrap();
    assert_eq!(s.doc().unwrap().isolation, None);

    // 5. Extract Contents
    s.execute("select.set", &json!({"ids": [gid.0]})).unwrap();
    s.execute("object.powerclip.extract", &json!({})).unwrap();
    assert!(s.doc().unwrap().doc.node(gid).is_none(), "PowerClip group removed after extract");

    // 6. Undo/Redo verification
    s.execute("edit.undo", &json!({})).unwrap();
    assert!(s.doc().unwrap().doc.node(gid).is_some(), "PowerClip group restored after undo");
    s.execute("edit.redo", &json!({})).unwrap();
    assert!(s.doc().unwrap().doc.node(gid).is_none(), "PowerClip group removed again after redo");
}

#[test]
fn powerclip_add_object_to_existing_powerclip() {
    let mut s = Session::new();
    s.execute("file.new", &json!({"width": 300, "height": 300})).unwrap();

    let c1 = rect(&mut s, 10.0, 10.0, 20.0, 20.0, "#0000ff");
    let container = rect(&mut s, 0.0, 0.0, 100.0, 100.0, "#ff0000");

    // 1. Initial PowerClip
    s.execute("select.set", &json!({"ids": [c1.0, container.0]})).unwrap();
    let res = s.execute("object.powerclip.placeInside", &json!({"container": container.0})).unwrap();
    let gid = NodeId(res["id"].as_u64().unwrap());

    // Record c1 bounds after first placement
    let c1_b_before = s.doc().unwrap().doc.node(c1).unwrap().visual_bounds().unwrap();

    // 2. Add second object by targeting the PowerClip or clicking its child
    let c2 = rect(&mut s, 40.0, 40.0, 15.0, 15.0, "#ffff00");
    s.execute("object.powerclip.placeInside", &json!({"container": c1.0, "contents": [c2.0]})).unwrap();

    let doc = &s.doc().unwrap().doc;
    let group_node = doc.node(gid).unwrap();
    let children = group_node.children().unwrap();
    // Container + 2 contents = 3 children
    assert_eq!(children.len(), 3);
    assert_eq!(children[0].id, container);
    assert_eq!(children[1].id, c1);
    assert_eq!(children[2].id, c2);

    // Existing c1 position should NOT be altered by adding c2
    let c1_b_after = doc.node(c1).unwrap().visual_bounds().unwrap();
    assert!((c1_b_before.x0 - c1_b_after.x0).abs() < 1e-4);
    assert!((c1_b_before.y0 - c1_b_after.y0).abs() < 1e-4);
}

#[test]
fn powerclip_change_color_modifies_only_container_shape() {
    let mut s = Session::new();
    s.execute("file.new", &json!({"width": 200, "height": 200})).unwrap();

    let content = rect(&mut s, 10.0, 10.0, 20.0, 20.0, "#0000ff");
    let container = rect(&mut s, 0.0, 0.0, 100.0, 100.0, "#ff0000");

    s.execute("select.set", &json!({"ids": [content.0, container.0]})).unwrap();
    let res = s.execute("object.powerclip.placeInside", &json!({"container": container.0})).unwrap();
    let gid = NodeId(res["id"].as_u64().unwrap());

    // Select the PowerClip group and change fill color to yellow
    s.execute("select.set", &json!({"ids": [gid.0]})).unwrap();
    s.execute("paint.setFill", &json!({"color": "#ffff00"})).unwrap();

    let doc = &s.doc().unwrap().doc;
    let opts = RenderOptions { background: Some([255, 255, 255, 255]), ..Default::default() };
    let img = Renderer::new().render(doc, 200, 200, Affine::IDENTITY, &opts);
    let pixel = |x, y| {
        let p = img.pixel(x, y);
        [p[0], p[1], p[2]]
    };

    // Container's fill became yellow where content is not drawn
    assert_eq!(pixel(50, 50), [255, 255, 0]);
    // Content shape remains blue!
    assert_eq!(pixel(20, 20), [0, 0, 255]);
}

#[test]
fn powerclip_edit_contents_renders_container_as_blue_wireframe() {
    let mut s = Session::new();
    s.execute("file.new", &json!({"width": 200, "height": 200})).unwrap();

    let content = rect(&mut s, 40.0, 40.0, 20.0, 20.0, "#00ff00");
    let container = rect(&mut s, 0.0, 0.0, 100.0, 100.0, "#ff0000");

    s.execute("select.set", &json!({"ids": [content.0, container.0]})).unwrap();
    let res = s.execute("object.powerclip.placeInside", &json!({"container": container.0})).unwrap();
    let gid = NodeId(res["id"].as_u64().unwrap());

    // Enter isolation mode
    s.execute("select.set", &json!({"ids": [gid.0]})).unwrap();
    s.execute("object.powerclip.editContents", &json!({})).unwrap();

    let doc = &s.doc().unwrap().doc;
    let iso = s.doc().unwrap().isolation;
    assert_eq!(iso, Some(gid));

    // When rendering in isolation edit mode (unclipped_isolation = Some(gid)):
    // 1. The container does NOT paint its solid red fill at (20, 20) -> background white
    // 2. The boundary of the container is rendered as a blue wireframe
    // 3. The content is visible and unclipped
    let opts = RenderOptions { background: Some([255, 255, 255, 255]), unclipped_isolation: iso, ..Default::default() };
    let img = Renderer::new().render(doc, 200, 200, Affine::IDENTITY, &opts);
    let pixel = |x, y| {
        let p = img.pixel(x, y);
        [p[0], p[1], p[2]]
    };

    // (20, 20) is inside container bounds, but container solid fill is replaced with wireframe
    assert_eq!(pixel(20, 20), [255, 255, 255], "Container solid fill is not rendered in edit mode");
    // (50, 50) has content (green)
    assert_eq!(pixel(50, 50), [0, 255, 0], "Content is rendered in edit mode");
    // At boundary edge of container (x=0..100, y=0), pixel contains blue wireframe tint
    let edge_pixel = pixel(50, 0);
    assert!(edge_pixel[2] > 200, "Boundary edge must be rendered as blue wireframe: {:?}", edge_pixel);

    // Finish editing
    s.execute("object.powerclip.finishEdit", &json!({})).unwrap();
    assert_eq!(s.doc().unwrap().isolation, None);

    // Normal rendering after finish edit restores container's red fill
    let normal_opts = RenderOptions { background: Some([255, 255, 255, 255]), ..Default::default() };
    let normal_img = Renderer::new().render(&s.doc().unwrap().doc, 200, 200, Affine::IDENTITY, &normal_opts);
    let normal_pixel = |x, y| {
        let p = normal_img.pixel(x, y);
        [p[0], p[1], p[2]]
    };
    assert_eq!(normal_pixel(20, 20), [255, 0, 0], "Container fill restored after exit edit mode");
}

#[test]
fn powerclip_edit_contents_can_select_and_drag_content_outside_container() {
    let mut s = Session::new();
    s.execute("file.new", &json!({"width": 300, "height": 300})).unwrap();

    // Container at (0, 0, 100, 100)
    let container = rect(&mut s, 0.0, 0.0, 100.0, 100.0, "#ff0000");
    // Content at (150, 150, 50, 50) - completely outside the container
    let content = rect(&mut s, 150.0, 150.0, 50.0, 50.0, "#0000ff");

    // Place inside container
    s.execute("select.set", &json!({"ids": [content.0, container.0]})).unwrap();
    let res = s.execute("object.powerclip.placeInside", &json!({"container": container.0})).unwrap();
    let gid = NodeId(res["id"].as_u64().unwrap());

    // Enter isolation mode
    s.execute("select.set", &json!({"ids": [gid.0]})).unwrap();
    s.execute("object.powerclip.editContents", &json!({})).unwrap();

    // With isolation mode active:
    // Hit test at (175, 175) which is inside content (outside container)
    let doc = &s.doc().unwrap().doc;
    let hit_opt = vectorcraft_doc::hit::HitOptions {
        tol: 3.0,
        outline: false,
        path_only: false,
        unclipped: s.doc().unwrap().isolation,
        ..Default::default()
    };
    let hit = vectorcraft_doc::hit::hit_test(doc, Point::new(175.0, 175.0), hit_opt);
    assert!(hit.is_some(), "Must hit content positioned outside container during edit mode");
    assert_eq!(hit.unwrap().top_object(s.doc().unwrap().isolation), content);

    // Now select content and transform/move it
    s.execute("select.set", &json!({"ids": [content.0]})).unwrap();
    s.execute("object.move", &json!({"dx": 20.0, "dy": 20.0})).unwrap();

    let doc = &s.doc().unwrap().doc;
    let content_b = doc.node(content).unwrap().geometric_bounds().unwrap();
    assert!((content_b.x0 - 170.0).abs() < 1e-4, "Content must have moved by 20px horizontally: x0={}", content_b.x0);
    assert!((content_b.y0 - 170.0).abs() < 1e-4, "Content must have moved by 20px vertically: y0={}", content_b.y0);
}

#[test]
fn powerclip_edit_contents_dims_outside_objects_and_restricts_hit_test() {
    let mut s = Session::new();
    s.execute("file.new", &json!({"width": 200, "height": 200})).unwrap();

    // Outside object: pure black rect at (10, 10, 30, 30)
    let _outside = rect(&mut s, 10.0, 10.0, 30.0, 30.0, "#000000");

    // PowerClip container: red rect at (50, 50, 100, 100)
    let container = rect(&mut s, 50.0, 50.0, 100.0, 100.0, "#ff0000");
    // PowerClip content: green rect at (70, 70, 40, 40)
    let content = rect(&mut s, 70.0, 70.0, 40.0, 40.0, "#00ff00");

    // Create PowerClip
    s.execute("select.set", &json!({"ids": [content.0, container.0]})).unwrap();
    let res = s.execute("object.powerclip.placeInside", &json!({"container": container.0})).unwrap();
    let gid = NodeId(res["id"].as_u64().unwrap());

    // Enter PowerClip edit mode
    s.execute("select.set", &json!({"ids": [gid.0]})).unwrap();
    s.execute("object.powerclip.editContents", &json!({})).unwrap();

    let iso_id = s.doc().unwrap().isolation;
    assert_eq!(iso_id, Some(gid));

    // Render with isolation active
    let opts = RenderOptions { background: Some([255, 255, 255, 255]), isolated: iso_id, unclipped_isolation: iso_id, ..Default::default() };
    let img = Renderer::new().render(&s.doc().unwrap().doc, 200, 200, Affine::IDENTITY, &opts);
    let pixel = |x, y| {
        let p = img.pixel(x, y);
        [p[0], p[1], p[2]]
    };

    // 1. Outside object (black #000000 on white #ffffff):
    // Dimmed to 50% opacity by ISOLATION_DIM: 0.5 * 0 + 0.5 * 255 = ~128
    let outside_px = pixel(20, 20);
    assert!(
        (120..=135).contains(&outside_px[0]) && outside_px[0] == outside_px[1] && outside_px[1] == outside_px[2],
        "Outside black object must be dimmed to 50% opacity on white (~128, got {:?})",
        outside_px
    );

    // 2. Inside content (green #00ff00) must NOT be dimmed (pure green):
    assert_eq!(pixel(80, 80), [0, 255, 0], "Inside content must have 100% full opacity");

    // 3. Container wrapper outline is drawn as active blue wireframe:
    // Container border sits at x=50, y=50..150
    let wire_px = pixel(50, 60);
    assert!(wire_px[2] > 200, "Container border should be active blue wireframe, got: {:?}", wire_px);

    // 4. Hit test on outside object must NOT hit during isolation mode:
    let hit_opt = vectorcraft_doc::hit::HitOptions {
        tol: 3.0,
        outline: false,
        path_only: false,
        unclipped: iso_id,
        ..Default::default()
    };
    let hit_outside = vectorcraft_doc::hit::hit_test(&s.doc().unwrap().doc, Point::new(20.0, 20.0), hit_opt);
    assert!(hit_outside.is_none(), "Outside object must not be hittable during PowerClip edit mode");

    // 5. Hit test on inside content MUST hit:
    let hit_inside = vectorcraft_doc::hit::hit_test(&s.doc().unwrap().doc, Point::new(80.0, 80.0), hit_opt);
    assert!(hit_inside.is_some(), "Inside content must be hittable during PowerClip edit mode");
    assert_eq!(hit_inside.unwrap().leaf, content);

    // 6. Finish editing PowerClip:
    s.execute("object.powerclip.finishEdit", &json!({})).unwrap();
    assert_eq!(s.doc().unwrap().isolation, None);

    // Outside object returns to 100% opacity (solid black):
    let normal_opts = RenderOptions { background: Some([255, 255, 255, 255]), ..Default::default() };
    let normal_img = Renderer::new().render(&s.doc().unwrap().doc, 200, 200, Affine::IDENTITY, &normal_opts);
    let normal_px = normal_img.pixel(20, 20);
    assert_eq!([normal_px[0], normal_px[1], normal_px[2]], [0, 0, 0], "Outside object must return to solid black after exiting edit mode");
}

#[test]
fn powerclip_edit_contents_drawn_on_top_of_overlapping_outside_objects() {
    let mut s = Session::new();
    s.execute("file.new", &json!({"width": 200, "height": 200})).unwrap();

    // 1. Create PowerClip container and content (red container, green content) at 50..110
    let container = rect(&mut s, 50.0, 50.0, 60.0, 60.0, "#ff0000");
    let content = rect(&mut s, 50.0, 50.0, 60.0, 60.0, "#00ff00");
    s.execute("select.set", &json!({"ids": [content.0, container.0]})).unwrap();
    let res = s.execute("object.powerclip.placeInside", &json!({"container": container.0})).unwrap();
    let gid = NodeId(res["id"].as_u64().unwrap());

    // 2. Create an outside object positioned AFTER the PowerClip in document order (higher in z-order)
    // at (90..150, 90..150), overlapping with the PowerClip content at (90..110, 90..110)
    // Solid black #000000
    let _outside_above = rect(&mut s, 90.0, 90.0, 60.0, 60.0, "#000000");

    // 3. Enter PowerClip edit mode
    s.execute("select.set", &json!({"ids": [gid.0]})).unwrap();
    s.execute("object.powerclip.editContents", &json!({})).unwrap();

    let iso_id = s.doc().unwrap().isolation;
    assert_eq!(iso_id, Some(gid));

    let opts = RenderOptions { background: Some([255, 255, 255, 255]), isolated: iso_id, unclipped_isolation: iso_id, ..Default::default() };
    let img = Renderer::new().render(&s.doc().unwrap().doc, 200, 200, Affine::IDENTITY, &opts);
    let pixel = |x, y| {
        let p = img.pixel(x, y);
        [p[0], p[1], p[2]]
    };

    // At (100, 100), which is in the overlapping region (inside container 50..110 and outside object 90..150):
    // Under isolation mode (#833), outside art above the isolated container is dimmed to 50% opacity.
    // Green content [0, 255, 0] under 50% black produces green dimmed with black (~128 green):
    let overlap_px = pixel(100, 100);
    assert!(
        overlap_px[0] == 0 && (120..=135).contains(&overlap_px[1]) && overlap_px[2] == 0,
        "Overlapping outside object must be dimmed to 50% over PowerClip content, got {:?}",
        overlap_px
    );

    // At (130, 130), which is outside the PowerClip container (ends at 110) but inside the outside object (90..150):
    // The outside object should be rendered dimmed (~128 on white background):
    let out_px = pixel(130, 130);
    assert!((120..=135).contains(&out_px[0]), "Outside object in non-overlapping area must be dimmed: {:?}", out_px);

    // 4. Finish edit mode
    s.execute("object.powerclip.finishEdit", &json!({})).unwrap();

    // In normal mode, outside object is above the container in z-order, so at (100, 100) it is black [0, 0, 0]:
    let normal_opts = RenderOptions { background: Some([255, 255, 255, 255]), ..Default::default() };
    let normal_img = Renderer::new().render(&s.doc().unwrap().doc, 200, 200, Affine::IDENTITY, &normal_opts);
    let normal_px = normal_img.pixel(100, 100);
    assert_eq!([normal_px[0], normal_px[1], normal_px[2]], [0, 0, 0], "Normal z-order restored after exiting edit mode");
}
