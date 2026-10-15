//! Unit tests for Corel-style pagination and multi-artboard per page support.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use serde_json::json;

use crate::Session;

fn session() -> Session {
    let mut s = Session::new();
    s.execute("file.new", &json!({ "width": 600, "height": 400, "colorMode": "rgb" })).unwrap();
    s
}

#[test]
fn page_new_and_multi_artboard_per_page() {
    let mut s = session();

    // Initial state: 1 page, 1 artboard
    let list = s.execute("page.list", &json!({})).unwrap();
    let pages = list["pages"].as_array().unwrap();
    assert_eq!(pages.len(), 1);
    assert_eq!(pages[0]["name"], "Page 1");
    assert_eq!(pages[0]["artboards"].as_array().unwrap().len(), 1);

    // Add another artboard to Page 1: 1 page can have multiple artboards!
    let ab2 = s.execute("page.addArtboard", &json!({ "page": 0, "width": 300, "height": 400, "name": "Page 1 - Artboard 2" })).unwrap();
    assert_eq!(ab2["page"], 0);

    let list2 = s.execute("page.list", &json!({})).unwrap();
    let pages2 = list2["pages"].as_array().unwrap();
    assert_eq!(pages2[0]["artboards"].as_array().unwrap().len(), 2, "Page 1 should now have 2 artboards");

    // Add a new Page (Page 2)
    let p2 = s.execute("page.new", &json!({ "name": "Page 2", "width": 500, "height": 300 })).unwrap();
    let p2_id = p2["id"].as_u64().unwrap();
    assert_eq!(p2["name"], "Page 2");

    // Add another artboard to Page 2
    s.execute("page.addArtboard", &json!({ "page": p2_id, "width": 250, "height": 300 })).unwrap();

    let list3 = s.execute("page.list", &json!({})).unwrap();
    let pages3 = list3["pages"].as_array().unwrap();
    assert_eq!(pages3.len(), 2);
    assert_eq!(pages3[0]["artboards"].as_array().unwrap().len(), 2, "Page 1 has 2 artboards");
    assert_eq!(pages3[1]["artboards"].as_array().unwrap().len(), 2, "Page 2 has 2 artboards");
}

#[test]
fn page_rename_and_reorder() {
    let mut s = session();
    s.execute("page.new", &json!({ "name": "Back Cover" })).unwrap();

    s.execute("page.rename", &json!({ "index": 0, "name": "Front Cover" })).unwrap();
    let list = s.execute("page.list", &json!({})).unwrap();
    let pages = list["pages"].as_array().unwrap();
    assert_eq!(pages[0]["name"], "Front Cover");
    assert_eq!(pages[1]["name"], "Back Cover");

    // Reorder: swap page 0 and 1
    s.execute("page.reorder", &json!({ "index": 0, "to": 1 })).unwrap();
    let list2 = s.execute("page.list", &json!({})).unwrap();
    let pages2 = list2["pages"].as_array().unwrap();
    assert_eq!(pages2[0]["name"], "Back Cover");
    assert_eq!(pages2[1]["name"], "Front Cover");
}

#[test]
fn page_duplicate_and_delete() {
    let mut s = session();
    s.execute("page.addArtboard", &json!({ "page": 0, "width": 200, "height": 200 })).unwrap();
    assert_eq!(s.doc().unwrap().doc.artboards.len(), 2);

    // Duplicate Page 1
    let dup = s.execute("page.duplicate", &json!({ "index": 0 })).unwrap();
    let _dup_id = dup["id"].as_u64().unwrap();
    assert_eq!(dup["name"], "Page 1 Copy");

    let list = s.execute("page.list", &json!({})).unwrap();
    let pages = list["pages"].as_array().unwrap();
    assert_eq!(pages.len(), 2);
    assert_eq!(pages[1]["artboards"].as_array().unwrap().len(), 2, "Duplicated page should have cloned both artboards");

    // Delete duplicated page
    s.execute("page.delete", &json!({ "index": 1 })).unwrap();
    let list2 = s.execute("page.list", &json!({})).unwrap();
    assert_eq!(list2["pages"].as_array().unwrap().len(), 1);

    // Cannot delete the only remaining page
    let err = s.execute("page.delete", &json!({ "index": 0 }));
    assert!(err.is_err(), "cannot delete the last remaining page");
}

#[test]
fn page_assign_artboard() {
    let mut s = session();
    s.execute("page.addArtboard", &json!({ "page": 0 })).unwrap();
    s.execute("page.new", &json!({ "name": "Page 2" })).unwrap();

    // Initially Page 1 has artboard 0 and 1, Page 2 has artboard 2
    let list = s.execute("page.list", &json!({})).unwrap();
    let pages = list["pages"].as_array().unwrap();
    assert_eq!(pages[0]["artboards"].as_array().unwrap().len(), 2);
    assert_eq!(pages[1]["artboards"].as_array().unwrap().len(), 1);

    // Reassign artboard 1 to Page 2 (id = 1)
    s.execute("page.assignArtboard", &json!({ "artboard_index": 1, "page": 1 })).unwrap();

    let list2 = s.execute("page.list", &json!({})).unwrap();
    let pages2 = list2["pages"].as_array().unwrap();
    assert_eq!(pages2[0]["artboards"].as_array().unwrap().len(), 1);
    assert_eq!(pages2[1]["artboards"].as_array().unwrap().len(), 2);
}
