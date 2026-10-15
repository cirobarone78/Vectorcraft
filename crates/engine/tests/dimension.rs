#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]
use serde_json::{Value, json};
use vectorcraft_doc::{Document, NodeId, NodeKind};
use vectorcraft_engine::Session;

fn session() -> Session {
    let mut s = Session::new();
    s.execute("file.new", &json!({"width":600,"height":400})).unwrap();
    s
}
fn create(s: &mut Session, options: Value) -> Value {
    s.execute("dimension.create", &json!({"kind":"linear","points":[[20,50],[92,50]],"placement":[55,20],"options":options})).unwrap()
}

#[test]
fn dimension_creates_separate_layer_in_one_undo_and_preserves_source_artwork() {
    let mut s = session();
    let original = s.execute("shape.rectangle", &json!({"x":20,"y":50,"width":72,"height":40})).unwrap();
    let layers = s.doc().unwrap().doc.layers.clone();
    let result = create(&mut s, json!({}));
    assert_eq!(result["label"], "25.4 mm");
    let doc = &s.doc().unwrap().doc;
    assert_eq!(doc.layers.len(), layers.len() + 1);
    assert_eq!(doc.layers.first(), layers.first());
    let id = NodeId(result["id"].as_u64().unwrap());
    let layer = NodeId(result["layer"].as_u64().unwrap());
    assert_ne!(doc.layer_of(NodeId(original["id"].as_u64().unwrap())), Some(layer));
    assert_eq!(doc.parent_of(id), Some(layer));
    assert!(doc.node(layer).unwrap().extras().dimension_layer);
    s.execute("edit.undo", &json!({})).unwrap();
    assert_eq!(s.doc().unwrap().doc.layers, layers);
    s.execute("edit.redo", &json!({})).unwrap();
    assert!(s.doc().unwrap().doc.node(id).unwrap().extras().dimension.is_some());
}

#[test]
fn dimension_reuses_renamed_annotation_layer_but_not_same_named_source_layer() {
    let mut s = session();
    if let Some(n) = std::sync::Arc::make_mut(&mut s.doc_mut().unwrap().doc).layers.first_mut() {
        std::sync::Arc::make_mut(n).name = Some("Dimensions".into());
    }
    let first = create(&mut s, json!({}));
    let layer = NodeId(first["layer"].as_u64().unwrap());
    std::sync::Arc::make_mut(&mut s.doc_mut().unwrap().doc).node_mut(layer).unwrap().name = Some("My measurements".into());
    assert_eq!(create(&mut s, json!({}))["layer"], first["layer"]);
    std::sync::Arc::make_mut(&mut s.doc_mut().unwrap().doc).node_mut(layer).unwrap().locked = true;
    let second = create(&mut s, json!({}));
    assert_ne!(second["layer"], first["layer"]);
    let second_layer = NodeId(second["layer"].as_u64().unwrap());
    std::sync::Arc::make_mut(&mut s.doc_mut().unwrap().doc).node_mut(second_layer).unwrap().visible = false;
    assert_ne!(create(&mut s, json!({}))["layer"], second["layer"]);
}

#[test]
fn dimension_update_regenerates_vector_text_and_undo_restores_settings() {
    let mut s = session();
    let first = create(&mut s, json!({}));
    let id = NodeId(first["id"].as_u64().unwrap());
    let original = s.doc().unwrap().doc.node(id).unwrap().clone();
    let result=s.execute("dimension.update",&json!({"id":id,"options":{"unit":"Centimeters","precision":2,"scale":10,"lineType":"dashed","extensionType":"dotted","fontSize":20,"textColor":[20,100,150]}})).unwrap();
    assert_eq!(result["label"], "25.40 cm");
    let doc = &s.doc().unwrap().doc;
    let node = doc.node(id).unwrap();
    assert_eq!(node.extras().dimension.as_ref().unwrap().options.font_size, 20.0);
    assert!(node.children().unwrap().iter().any(|n| matches!(&n.kind,NodeKind::Text(t) if t.runs.first().is_some_and(|r|r.text=="25.40 cm"))));
    s.execute("edit.undo", &json!({})).unwrap();
    assert_eq!(s.doc().unwrap().doc.node(id), Some(&original));
}

#[test]
fn dimension_all_measurement_types_and_angular_scale() {
    let mut s = session();
    for (kind, points, placement, expected) in [
        ("angular", json!([[0, 0], [72, 0], [0, 72]]), json!([30, 30]), "90.0°"),
        ("angular", json!([[0, 0], [72, 0], [0, 72]]), json!([-30, -30]), "270.0°"),
        ("radius", json!([[0, 0], [72, 0]]), json!([100, 20]), "R 254.0 mm"),
        ("diameter", json!([[0, 0], [72, 0]]), json!([100, 20]), "Ø 508.0 mm"),
    ] {
        let out = s.execute("dimension.create", &json!({"kind":kind,"points":points,"placement":placement,"options":{"scale":10}})).unwrap();
        assert_eq!(out["label"], expected);
    }
}

#[test]
fn dimension_native_roundtrip_keeps_editable_spec_and_layer_marker() {
    let mut s = session();
    let made = create(&mut s, json!({"hideUnit":true,"precision":3}));
    let doc = &s.doc().unwrap().doc;
    let encoded = serde_json::to_value(doc).unwrap();
    let back: Document = serde_json::from_value(encoded).unwrap();
    let id = NodeId(made["id"].as_u64().unwrap());
    assert_eq!(back.node(id).unwrap().extras().dimension, doc.node(id).unwrap().extras().dimension);
    assert!(back.node(NodeId(made["layer"].as_u64().unwrap())).unwrap().extras().dimension_layer);
}

#[test]
fn dimension_bad_parameters_are_atomic_and_never_allocate_an_annotation_layer() {
    let mut s = session();
    let before = s.doc().unwrap().doc.layers.clone();
    for p in [
        json!({}),
        json!({"points":[[0,0],[0,0]],"placement":[0,10]}),
        json!({"points":[[0,0],[72,0]],"placement":[0,10],"options":{"scale":0}}),
        json!({"points":[[0,0],[72,0]],"placement":[0,10],"options":{"precision":255}}),
        json!({"points":[[0,0],[72,0]],"placement":[0,10],"options":{"lineType":"unknown"}}),
        json!({"points":[[0,0],[72,0]],"placement":[0,10],"options":{"lineColor":[400,0,0]}}),
    ] {
        assert!(s.execute("dimension.create", &p).is_err());
        assert_eq!(s.doc().unwrap().doc.layers, before);
    }
}

#[test]
fn dimension_selected_bounds_and_circle_validation() {
    let mut s = session();
    let shape = s.execute("shape.rectangle", &json!({"x":10,"y":10,"width":72,"height":144})).unwrap();
    assert_eq!(s.execute("dimension.fromSelection", &json!({"axis":"vertical"})).unwrap()["label"], "50.8 mm");
    s.execute("select.set", &json!({"ids":[shape["id"]]})).unwrap();
    assert!(s.execute("dimension.fromSelection", &json!({"axis":"radius"})).is_err());
    s.execute("shape.ellipse", &json!({"x":10,"y":10,"width":144,"height":144})).unwrap();
    assert_eq!(s.execute("dimension.fromSelection", &json!({"axis":"diameter"})).unwrap()["label"], "Ø 50.8 mm");
}

#[test]
fn dimension_export_is_persistent_vector_art_and_recipe_rejects_oversized_points() {
    let mut s = session();
    let made = create(&mut s, json!({}));
    let exported = s.execute("document.export", &json!({"format":"svg"})).unwrap();
    let svg = String::from_utf8(vectorcraft_format::base64_decode(exported["dataBase64"].as_str().unwrap()).unwrap()).unwrap();
    assert!(svg.contains("<path"), "{svg}");
    assert!(svg.contains("25.4 mm"), "{svg}");
    let id = NodeId(made["id"].as_u64().unwrap());
    let mut recipe = serde_json::to_value(s.doc().unwrap().doc.node(id).unwrap().extras().dimension.as_deref().unwrap()).unwrap();
    recipe["points"] = json!([[0, 0], [1, 0], [2, 0], [3, 0]]);
    assert!(serde_json::from_value::<vectorcraft_doc::dimension::DimensionSpec>(recipe).is_err());
    s.execute("select.none", &json!({})).unwrap();
    assert_eq!(s.execute("dimension.inspect", &json!({"id":id})).unwrap()["id"], made["id"]);
}

#[test]
fn dimension_edge_and_circle_drags_use_the_active_type_and_annotation_layer() {
    use vectorcraft_doc::dimension::DimensionKind;
    use vectorcraft_engine::ViewInfo;
    use vectorcraft_tools::{PointerEvent, PointerKind};
    let mut s = session();
    s.execute("shape.rectangle", &json!({"x":100,"y":100,"width":200,"height":100})).unwrap();
    let view = ViewInfo { smart_guides: false, ..Default::default() };
    s.select_tool("dimension", view).unwrap();
    for (kind, x, y) in [(PointerKind::Down, 200.0, 100.0), (PointerKind::Drag, 200.0, 60.0), (PointerKind::Up, 200.0, 60.0)] {
        s.pointer(&PointerEvent::new(kind, x, y), view).unwrap();
    }
    let linear = s.execute("dimension.inspect", &json!({})).unwrap();
    assert_eq!(linear["spec"]["kind"], "linear");
    s.execute("shape.ellipse", &json!({"x":400,"y":200,"width":100,"height":100})).unwrap();
    s.set_tool_option("kind", &json!("diameter"));
    for (kind, x, y) in [(PointerKind::Down, 500.0, 250.0), (PointerKind::Drag, 550.0, 180.0), (PointerKind::Up, 550.0, 180.0)] {
        s.pointer(&PointerEvent::new(kind, x, y), view).unwrap();
    }
    let radial = s.execute("dimension.inspect", &json!({})).unwrap();
    let doc = &s.doc().unwrap().doc;
    let node = doc.node(NodeId(radial["id"].as_u64().unwrap())).unwrap();
    let spec = node.extras().dimension.as_deref().unwrap();
    assert_eq!(spec.kind, DimensionKind::Diameter);
    assert_eq!(spec.drawing().unwrap().text, "Ø 35.3 mm");
    let layer = doc.layer_of(node.id).unwrap();
    assert!(doc.node(layer).unwrap().extras().dimension_layer);
    assert_eq!(doc.layer_of(NodeId(linear["id"].as_u64().unwrap())), Some(layer));
}

#[test]
fn dimension_rectangle_modes_measure_full_bounds_from_any_edge() {
    use vectorcraft_engine::ViewInfo;
    use vectorcraft_tools::{PointerEvent, PointerKind};
    let mut s = session();
    s.execute("shape.rectangle", &json!({"x":100,"y":100,"width":200,"height":100})).unwrap();
    let view = ViewInfo { smart_guides: false, ..Default::default() };
    s.select_tool("dimension", view).unwrap();
    assert_eq!(s.tool_options()["kind"], "linear");
    assert_eq!(s.tool_options()["axis"], "aligned");
    for (axis, start, end, expected) in
        [("horizontal", (100.0, 150.0), (200.0, 60.0), "70.6 mm"), ("vertical", (200.0, 100.0), (350.0, 150.0), "35.3 mm")]
    {
        s.set_tool_option("axis", &json!(axis));
        s.pointer(&PointerEvent::new(PointerKind::Down, start.0, start.1), view).unwrap();
        s.pointer(&PointerEvent::new(PointerKind::Drag, end.0, end.1), view).unwrap();
        s.pointer(&PointerEvent::new(PointerKind::Up, end.0, end.1), view).unwrap();
        let inspected = s.execute("dimension.inspect", &json!({})).unwrap();
        let node = s.doc().unwrap().doc.node(NodeId(inspected["id"].as_u64().unwrap())).unwrap();
        assert_eq!(node.extras().dimension.as_deref().unwrap().drawing().unwrap().text, expected);
    }
    s.select_tool("selection", view).unwrap();
    s.select_tool("dimension", view).unwrap();
    assert_eq!(s.tool_options()["axis"], "vertical");
}

#[test]
fn dimension_outside_and_inside_text_follow_the_object_side() {
    use vectorcraft_doc::dimension::{DimensionKind, DimensionOptions, DimensionSpec};
    use vectorcraft_geom::{Affine, Point};
    for y in [-30.0, 30.0] {
        let mut spec = DimensionSpec {
            kind: DimensionKind::Linear,
            points: vec![Point::ORIGIN, Point::new(100.0, 0.0)],
            placement: Point::new(50.0, y),
            options: DimensionOptions::default(),
            transform: Affine::IDENTITY,
        };
        let outside = spec.drawing().unwrap().at;
        assert!((outside.y - y) * y > 0.0);
        spec.options.text_position = "below".into();
        let inside = spec.drawing().unwrap().at;
        assert!((inside.y - y) * y < 0.0);
    }
    let mut s = session();
    let made = create(&mut s, json!({}));
    if let Some((face, _)) = vectorcraft_text::FontDb::global().resolve("sans-serif", "Regular") {
        let node = s.doc().unwrap().doc.node(NodeId(made["id"].as_u64().unwrap())).unwrap();
        let text = node.children().unwrap().iter().find_map(|n| if let NodeKind::Text(t) = &n.kind { Some(t) } else { None }).unwrap();
        assert_eq!(text.runs.first().unwrap().style.font_family, face.family);
        assert_eq!(text.runs.first().unwrap().style.font_style, face.style);
    }
}
