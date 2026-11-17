//! Saving and loading: versioned JSON, tolerant of old, new and damaged files.

use cho_layout::*;

fn sample() -> Workspace {
    let mut ws = Workspace::with_defaults();
    let e = ws
        .open(
            PaneContent::Editor {
                path: "src/lib.rs".into(),
                caret: 10,
            },
            None,
        )
        .unwrap();
    ws.pin_context(
        Pin {
            symbol: "LineCol".into(),
            symbol_kind: "struct".into(),
            path: "src/lib.rs".into(),
            byte_offset: 3,
            commit: "04e63f8".into(),
            content_hash: None,
        },
        PinPlace::BottomBar,
    )
    .unwrap();
    ws.maximize(e, 0).unwrap();
    ws.restore(60).unwrap();
    ws
}

#[test]
fn round_trips_with_version_and_history() {
    let ws = sample();
    let json = ws.to_json();
    let v: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(v["version"], FORMAT_VERSION);
    let loaded = Workspace::from_json(&json);
    assert!(loaded.warnings.is_empty(), "{:?}", loaded.warnings);
    assert_eq!(loaded.workspace, ws);
}

#[test]
fn an_open_focus_layout_is_not_saved() {
    let mut ws = sample();
    let first = *ws.panes().keys().next().unwrap();
    ws.maximize(first, 100).unwrap();
    let loaded = Workspace::from_json(&ws.to_json());
    assert!(!loaded.workspace.in_focus());
    assert_eq!(loaded.workspace.arrangement(), ws.arrangement());
}

#[test]
fn garbage_gives_the_default_layout_with_a_warning() {
    for bad in ["", "not json", "[]", "42"] {
        let loaded = Workspace::from_json(bad);
        assert_eq!(loaded.workspace, Workspace::with_defaults());
        assert_eq!(loaded.warnings.len(), 1, "{bad:?}: {:?}", loaded.warnings);
    }
}

#[test]
fn a_newer_version_with_unknown_pane_kinds_and_fields_is_read_as_far_as_possible() {
    let ws = sample();
    let mut v: serde_json::Value = serde_json::from_str(&ws.to_json()).unwrap();
    v["version"] = 7.into();
    v["something_new"] = serde_json::json!({"x": 1});
    v["arrangement"]["left"]["also_new"] = true.into();
    // A pane kind from the future, placed in the left dock.
    v["panes"]
        .as_array_mut()
        .unwrap()
        .push(serde_json::json!({"id": 500, "kind": "hologram", "beam": 3}));
    let root = v["arrangement"]["left"]["root"].clone();
    v["arrangement"]["left"]["root"] = serde_json::json!({
        "type": "split", "id": 501, "axis": "column",
        "children": [
            {"weight": 0.5, "node": root},
            {"weight": 0.5, "node": {"type": "group", "id": 502, "tabs": [500]}}
        ]
    });
    let loaded = Workspace::from_json(&v.to_string());
    let w = &loaded.warnings;
    assert!(w.iter().any(|m| m.contains("newer Chorong")), "{w:?}");
    assert!(w.iter().any(|m| m.contains("cannot read")), "{w:?}");
    assert!(w.iter().any(|m| m.contains("unknown pane 500")), "{w:?}");
    assert_eq!(loaded.workspace.arrangement(), ws.arrangement());
    loaded.workspace.check_invariants().unwrap();
}

#[test]
fn an_old_file_without_a_version_or_arrangement_places_panes_by_default() {
    let json = r#"{
        "panes": [
            {"id": 1, "kind": "file_tree"},
            {"id": 2, "kind": "editor", "path": "a.rs", "caret": 4},
            {"id": 3, "kind": "call_stack", "investigation": "x", "title": "X"}
        ]
    }"#;
    let loaded = Workspace::from_json(json);
    let ws = &loaded.workspace;
    ws.check_invariants().unwrap();
    assert!(
        loaded
            .warnings
            .iter()
            .any(|m| m.contains("no format version"))
    );
    assert!(loaded.warnings.iter().any(|m| m.contains("no place")));
    let a = ws.arrangement();
    assert_eq!(a.locate(PaneId(1)).unwrap().0, Region::Dock(DockId::Left));
    assert_eq!(a.locate(PaneId(2)).unwrap().0, Region::Editor);
    assert_eq!(a.locate(PaneId(3)).unwrap().0, Region::Dock(DockId::Bottom));
}

#[test]
fn damaged_trees_are_repaired() {
    // Pane 1 twice, an editor in a dock, a pin before a tool tab, a split of one child with a
    // bad weight, a dock far too wide, duplicate group ids and a counter that is too low.
    let json = r#"{
      "version": 1, "next_id": 2,
      "panes": [
        {"id": 1, "kind": "outline"},
        {"id": 2, "kind": "editor", "path": "a.rs", "caret": 0},
        {"id": 3, "kind": "pinned_context", "symbol": "S", "symbol_kind": "fn", "path": "a.rs", "byte_offset": 0, "commit": "c"},
        {"id": 4, "kind": "graph", "query": "q", "title": "G"}
      ],
      "arrangement": {
        "left": {"size": 9999, "root": {"type": "split", "id": 10, "axis": "row", "children": [
            {"weight": -1, "node": {"type": "group", "id": 11, "tabs": [1, 2, 1], "active": 7}}
        ]}},
        "right": {"size": 300, "root": null},
        "bottom": {"size": 200, "root": {"type": "group", "id": 11, "tabs": [3, 4]}},
        "editor": null
      }
    }"#;
    let loaded = Workspace::from_json(json);
    let ws = &loaded.workspace;
    ws.check_invariants().unwrap();
    let a = ws.arrangement();
    assert_eq!(a.left.size, 640.0);
    assert_eq!(a.left.root.as_ref().unwrap().panes(), vec![PaneId(1)]);
    assert_eq!(a.locate(PaneId(2)).unwrap().0, Region::Editor);
    assert_eq!(
        a.bottom.root.as_ref().unwrap().panes(),
        vec![PaneId(4), PaneId(3)]
    );
    assert!(loaded.warnings.len() >= 3, "{:?}", loaded.warnings);
}

#[test]
fn per_repository_files_are_stable_and_private() {
    let a = layout_file_name("/some/where/repo");
    assert_eq!(a, layout_file_name("/some/where/repo"));
    assert_ne!(a, layout_file_name("/some/where/other"));
    assert!(!a.contains("repo"));
    assert!(a.starts_with("layout-") && a.ends_with(".json"));

    let dir = std::env::temp_dir().join(format!("cho-layout-test-{}", std::process::id()));
    let ws = sample();
    let path = save(&dir, "/r", &ws).unwrap();
    assert!(path.ends_with(layout_file_name("/r")));
    let loaded = load(&dir, "/r");
    assert_eq!(loaded.workspace, ws);
    assert_eq!(load(&dir, "/missing").workspace, Workspace::with_defaults());
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn version_1_files_load_with_single_page_history() {
    let json = r#"{
      "version": 1, "next_id": 10,
      "panes": [
        {"id": 1, "kind": "outline"},
        {"id": 2, "kind": "editor", "path": "a.rs", "caret": 3}
      ],
      "arrangement": {
        "left": {"size": 260, "root": null},
        "right": {"size": 320, "root": {"type": "group", "id": 3, "tabs": [1]}},
        "bottom": {"size": 220, "root": null},
        "editor": {"type": "group", "id": 4, "tabs": [2]}
      },
      "focus_history": [
        {"layout": {"type": "group", "tabs": [{"kind": "outline"}]},
         "maximized": {"kind": "outline"}, "ended_at": 7}
      ]
    }"#;
    let loaded = Workspace::from_json(json);
    assert!(loaded.warnings.is_empty(), "{:?}", loaded.warnings);
    let h = loaded.workspace.history();
    assert_eq!(h.len(), 1);
    assert_eq!(h[0].pages.len(), 1);
    assert_eq!(h[0].label(), "Outline");
    // Saved again, it is version 2.
    let v: serde_json::Value = serde_json::from_str(&loaded.workspace.to_json()).unwrap();
    assert_eq!(v["version"], 2);
    assert!(v["focus_history"][0]["pages"].is_array());
}

#[test]
fn mixed_editor_areas_round_trip_and_bad_ones_are_repaired() {
    let mut ws = sample();
    let outline = *ws
        .panes()
        .iter()
        .find(|(_, c)| c.kind() == PaneKind::Outline)
        .unwrap()
        .0;
    ws.move_pane(outline, Target::EditorArea).unwrap();
    let loaded = Workspace::from_json(&ws.to_json());
    assert!(loaded.warnings.is_empty(), "{:?}", loaded.warnings);
    assert_eq!(loaded.workspace, ws);

    // A group mixing an editor and a tool, and an editor area with tools only.
    let json = r#"{
      "version": 2, "next_id": 20,
      "panes": [
        {"id": 1, "kind": "outline"},
        {"id": 2, "kind": "editor", "path": "a.rs", "caret": 0},
        {"id": 3, "kind": "context"}
      ],
      "arrangement": {
        "left": {"size": 260, "root": null},
        "right": {"size": 320, "root": {"type": "group", "id": 5, "tabs": [3]}},
        "bottom": {"size": 220, "root": null},
        "editor": {"type": "group", "id": 4, "tabs": [2, 1]}
      }
    }"#;
    let loaded = Workspace::from_json(json);
    loaded.workspace.check_invariants().unwrap();
    let a = loaded.workspace.arrangement();
    assert_eq!(a.editor.as_ref().unwrap().panes(), vec![PaneId(2)]);
    assert_eq!(a.locate(PaneId(1)).unwrap().0, Region::Dock(DockId::Right));

    let json = json.replace(r#""tabs": [2, 1]"#, r#""tabs": [1]"#).replace(
        r#"{"id": 2, "kind": "editor", "path": "a.rs", "caret": 0},"#,
        "",
    );
    let loaded = Workspace::from_json(&json);
    loaded.workspace.check_invariants().unwrap();
    assert!(loaded.workspace.arrangement().editor.is_none());
    assert!(
        loaded.warnings.iter().any(|w| w.contains("no code pane")),
        "{:?}",
        loaded.warnings
    );
}

#[test]
fn a_changes_pane_opens_in_the_left_dock_and_round_trips() {
    let mut ws = Workspace::with_defaults();
    let id = ws.open(PaneContent::Changes, None).unwrap();
    assert_eq!(ws.kind(id), Some(PaneKind::Changes));
    assert!(ws.check_invariants().is_ok());
    // A tool pane never joins the editors' group.
    let group = ws.group_of(id).unwrap();
    assert!(
        ws.group(group)
            .unwrap()
            .tabs
            .iter()
            .all(|p| ws.kind(*p) != Some(PaneKind::Editor))
    );
    let loaded = Workspace::from_json(&ws.to_json());
    assert!(loaded.warnings.is_empty(), "{:?}", loaded.warnings);
    assert_eq!(loaded.workspace, ws);
    assert_eq!(loaded.workspace.content(id), Some(&PaneContent::Changes));
}

#[test]
fn a_comparison_pane_opens_in_the_bottom_dock_and_round_trips() {
    let mut ws = Workspace::with_defaults();
    let content = PaneContent::Comparison {
        investigation: ".chorong/investigations/compare.json".into(),
        title: "Comparison".into(),
    };
    let id = ws.open(content.clone(), None).unwrap();
    assert_eq!(ws.kind(id), Some(PaneKind::Comparison));
    assert_eq!(
        PaneKind::Comparison.default_place(),
        DefaultPlace::Dock(DockId::Bottom)
    );
    assert_eq!(
        ws.arrangement().locate(id).unwrap().0,
        Region::Dock(DockId::Bottom)
    );
    assert!(ws.check_invariants().is_ok());
    let source = content.source();
    assert_eq!(source.kind(), PaneKind::Comparison);
    assert_eq!(
        source.to_content().map(|c| c.kind()),
        Some(PaneKind::Comparison)
    );
    let loaded = Workspace::from_json(&ws.to_json());
    assert!(loaded.warnings.is_empty(), "{:?}", loaded.warnings);
    assert_eq!(loaded.workspace, ws);
    assert_eq!(loaded.workspace.content(id), Some(&content));
}
