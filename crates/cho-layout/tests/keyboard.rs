//! The keyboard additions (design/layout-keyboard.md, section 4), by example.

mod common;

use cho_layout::*;
use common::*;

const W: f32 = 1200.0;
const H: f32 = 800.0;

/// Files (left) | buffer.rs, line.rs | view.rs (right half of the editor) | Context over
/// Outline (right); call stack and graph in the bottom bar.
struct F {
    ws: Workspace,
    tree: PaneId,
    context: PaneId,
    outline: PaneId,
    buffer: PaneId,
    line: PaneId,
    view: PaneId,
    stack: PaneId,
    graph: PaneId,
}

fn fixture() -> F {
    let mut ws = Workspace::with_defaults();
    let tree = find(&ws, PaneKind::FileTree);
    let context = find(&ws, PaneKind::Context);
    let outline = find(&ws, PaneKind::Outline);
    let buffer = ws.open(editor("buffer.rs"), None).unwrap();
    let line = ws.open(editor("line.rs"), None).unwrap();
    ws.activate(buffer).unwrap();
    let view = ws.split(buffer, Edge::Right).unwrap();
    ws.set_editor_position(view, "view.rs", 0).unwrap();
    let stack = ws
        .open(
            PaneContent::CallStack {
                investigation: "s".into(),
                title: "Call stack".into(),
            },
            None,
        )
        .unwrap();
    let graph = ws
        .open(
            PaneContent::Graph {
                query: "q".into(),
                title: "Graph".into(),
            },
            None,
        )
        .unwrap();
    ws.activate(buffer).unwrap();
    ws.clear_undo();
    ok(&ws);
    F {
        ws,
        tree,
        context,
        outline,
        buffer,
        line,
        view,
        stack,
        graph,
    }
}

#[test]
fn neighbour_prefers_touching_then_overlap_then_recency_then_reading_order() {
    let a = NodeId(1);
    let (b, c, d) = (NodeId(2), NodeId(3), NodeId(4));
    // a on the left; b top-right (overlaps a a lot), c bottom-right (overlaps a little).
    let rects = [
        (a, Rect::new(0., 0., 100., 100.)),
        (b, Rect::new(100., 0., 100., 80.)),
        (c, Rect::new(100., 80., 100., 20.)),
        (d, Rect::new(300., 0., 100., 100.)),
    ];
    let none = |_| usize::MAX;
    assert_eq!(
        neighbour(&rects, a, Direction::Right, |_| true, none),
        Some(b)
    );
    assert_eq!(neighbour(&rects, a, Direction::Left, |_| true, none), None);
    assert_eq!(
        neighbour(&rects, b, Direction::Down, |_| true, none),
        Some(c)
    );
    assert_eq!(
        neighbour(&rects, a, Direction::Right, |id| id != b, none),
        Some(c)
    );
    // Equal overlap: the more recently used wins, then reading order.
    let tie = [
        (a, Rect::new(0., 0., 100., 100.)),
        (b, Rect::new(100., 0., 100., 50.)),
        (c, Rect::new(100., 50., 100., 50.)),
    ];
    assert_eq!(
        neighbour(&tie, a, Direction::Right, |_| true, none),
        Some(b)
    );
    let rank = |id| if id == c { 0 } else { 5 };
    assert_eq!(
        neighbour(&tie, a, Direction::Right, |_| true, rank),
        Some(c)
    );
}

#[test]
fn neighbour_is_symmetric_for_a_two_group_split() {
    for (axis, fwd, back) in [
        (Axis::Row, Direction::Right, Direction::Left),
        (Axis::Column, Direction::Down, Direction::Up),
    ] {
        let (a, b) = (NodeId(1), NodeId(2));
        let (ra, rb) = match axis {
            Axis::Row => (
                Rect::new(0., 0., 300., 200.),
                Rect::new(305., 0., 200., 200.),
            ),
            Axis::Column => (
                Rect::new(0., 0., 300., 120.),
                Rect::new(0., 125., 300., 80.),
            ),
        };
        let rects = [(a, ra), (b, rb)];
        assert_eq!(neighbour(&rects, a, fwd, |_| true, |_| 0), Some(b));
        assert_eq!(neighbour(&rects, b, back, |_| true, |_| 0), Some(a));
    }
}

#[test]
fn focus_by_direction_crosses_regions_and_skips_hidden_docks() {
    let mut f = fixture();
    let rects = f.ws.rects(W, H);
    assert_eq!(f.ws.focus_dir(Direction::Left, &rects), Ok(f.tree));
    assert_eq!(f.ws.focus_dir(Direction::Right, &rects), Ok(f.buffer));
    assert_eq!(f.ws.focus_dir(Direction::Right, &rects), Ok(f.view));
    assert_eq!(f.ws.focus_dir(Direction::Right, &rects), Ok(f.context));
    assert_eq!(f.ws.focus_dir(Direction::Down, &rects), Ok(f.outline));
    assert_eq!(f.ws.focus_dir(Direction::Down, &rects), Ok(f.graph));
    assert_eq!(
        f.ws.focus_dir(Direction::Down, &rects),
        Err(LayoutError::NothingThatWay)
    );
    f.ws.activate(f.buffer).unwrap();
    f.ws.toggle_dock(DockId::Left).unwrap();
    let rects = f.ws.rects(W, H);
    assert_eq!(
        f.ws.focus_dir(Direction::Left, &rects),
        Err(LayoutError::NothingThatWay)
    );
    assert_eq!(f.ws.recent_panes()[0], f.buffer);
}

#[test]
fn numbers_and_labels_follow_reading_order() {
    let mut f = fixture();
    let groups = f.ws.editor_groups();
    assert_eq!(groups.len(), 2);
    let labels = f.ws.labels(&LABEL_LETTERS);
    let shown: Vec<String> = labels.iter().map(|(l, _)| l.to_string()).collect();
    // Two editor groups, then Files, Context, Outline, the bottom bar.
    assert_eq!(shown, ["1", "2", "a", "d", "f", "g"]);
    assert_eq!(labels[2].1, f.ws.group_of(f.tree).unwrap());
    assert_eq!(
        f.ws.labels(&LABEL_LETTERS),
        labels,
        "same input, same labels"
    );
    assert_eq!(f.ws.focus_editor_group(2), Ok(f.view));
    assert_eq!(f.ws.focus_editor_group(3), Err(LayoutError::NothingThatWay));
    // A hidden dock has no labels.
    f.ws.toggle_dock(DockId::Bottom).unwrap();
    assert_eq!(f.ws.labels(&LABEL_LETTERS).len(), 5);
}

#[test]
fn shift_arrows_move_a_tool_pane_step_by_step() {
    let mut f = fixture();
    // Outline (right dock) moving left enters the editor area beside view.rs.
    let t =
        f.ws.move_pane_dir(f.outline, Direction::Left, &f.ws.rects(W, H))
            .unwrap();
    let vg = f.ws.group_of(f.view).unwrap();
    assert_eq!(
        t,
        Target::Beside {
            group: vg,
            edge: Edge::Right
        }
    );
    assert_eq!(region_of(&f.ws, f.outline), Region::Editor);
    ok(&f.ws);
    // Again: it passes over the editor groups to the left dock, as a tab of Files.
    f.ws.move_pane_dir(f.outline, Direction::Left, &f.ws.rects(W, H))
        .unwrap();
    assert_eq!(region_of(&f.ws, f.outline), Region::Dock(DockId::Left));
    assert_eq!(f.ws.group_of(f.outline), f.ws.group_of(f.tree));
    // At the window edge a tool pane goes to that side's dock: down to the bottom bar.
    f.ws.move_pane_dir(f.context, Direction::Down, &f.ws.rects(W, H))
        .unwrap();
    assert_eq!(region_of(&f.ws, f.context), Region::Dock(DockId::Bottom));
    // There is no top dock: Outline, now at the top of the left dock... is under the tree's
    // group as a tab; take the call stack, which goes up into the editor area instead.
    f.ws.move_pane_dir(f.stack, Direction::Up, &f.ws.rects(W, H))
        .unwrap();
    assert_eq!(region_of(&f.ws, f.stack), Region::Editor);
    let before = f.ws.clone();
    assert_eq!(
        f.ws.move_pane_dir(f.tree, Direction::Up, &f.ws.rects(W, H)),
        Err(LayoutError::NothingThatWay),
        "nothing above, and no top dock"
    );
    assert_eq!(f.ws, before);
    ok(&f.ws);
}

#[test]
fn the_layout_mode_preview_shows_where_each_arrow_would_go() {
    let f = fixture();
    let rects = f.ws.rects(W, H);
    for p in f.ws.panes().keys().copied() {
        for whole_group in [false, true] {
            let preview = f.ws.dir_targets(p, whole_group, &rects);
            for dir in Direction::ALL {
                let mut ws = f.ws.clone();
                let moved = if whole_group {
                    let g = ws.group_of(p).unwrap();
                    ws.move_group_dir(g, dir, &rects)
                } else {
                    ws.move_pane_dir(p, dir, &rects)
                };
                let shown = preview.iter().find(|(d, _)| *d == dir).map(|(_, t)| *t);
                assert_eq!(shown, moved.ok(), "{p:?} {dir:?} group {whole_group}");
            }
        }
    }
}

#[test]
fn shift_arrows_move_an_editor_tab_between_groups_and_to_new_edge_groups() {
    let mut f = fixture();
    // line.rs moves right into view.rs's group as its last tab.
    let t =
        f.ws.move_pane_dir(f.line, Direction::Right, &f.ws.rects(W, H))
            .unwrap();
    assert!(matches!(t, Target::Tabs { .. }));
    assert_eq!(f.ws.group_of(f.line), f.ws.group_of(f.view));
    // Right again: the right dock cannot take an editor, so it splits off at the edge.
    let t =
        f.ws.move_pane_dir(f.line, Direction::Right, &f.ws.rects(W, H))
            .unwrap();
    assert!(matches!(
        t,
        Target::Beside {
            edge: Edge::Right,
            ..
        }
    ));
    assert_eq!(
        f.ws.arrangement().editor.as_ref().unwrap().groups().len(),
        3
    );
    // Alone in its group at the edge, nothing that way: unchanged, with a reason.
    let before = f.ws.clone();
    assert_eq!(
        f.ws.move_pane_dir(f.line, Direction::Right, &f.ws.rects(W, H)),
        Err(LayoutError::NothingThatWay)
    );
    assert_eq!(f.ws, before);
    ok(&f.ws);
    // Alt-arrows move a whole group: the call stack + graph strip into the right dock.
    let bottom = f.ws.group_of(f.stack).unwrap();
    f.ws.move_group_dir(bottom, Direction::Up, &f.ws.rects(W, H))
        .unwrap();
    assert_ne!(region_of(&f.ws, f.stack), Region::Dock(DockId::Bottom));
    assert_eq!(f.ws.group_of(f.stack), f.ws.group_of(f.graph));
    ok(&f.ws);
}

#[test]
fn an_editor_has_no_dock_target_and_a_tool_no_editor_strip() {
    // That every target is valid is the property test's (`validate_agrees_with_move_pane`).
    let f = fixture();
    let eg = f.ws.group_of(f.buffer).unwrap();
    assert!(
        !f.ws
            .valid_targets(f.buffer)
            .iter()
            .any(|t| matches!(t, Target::Dock(_)))
    );
    assert!(!f.ws.valid_targets(f.outline).contains(&Target::Tabs {
        group: eg,
        index: usize::MAX
    }));
}

#[test]
fn keyboard_resize_moves_the_edge_that_way() {
    let mut f = fixture();
    let Some(Node::Split(s)) = &f.ws.arrangement().editor else {
        panic!()
    };
    let split = s.id;
    let share = |ws: &Workspace| match ws.arrangement().root(Region::Editor) {
        Some(Node::Split(s)) => s.children[0].weight,
        _ => panic!(),
    };
    // buffer.rs's right edge moves right: it grows.
    assert_eq!(
        f.ws.resize_pane(f.buffer, Direction::Right, ResizeStep::NORMAL),
        Ok(0.05)
    );
    assert!((share(&f.ws) - 0.55).abs() < 1e-5);
    // view.rs's right edge is the right dock's: the dock shrinks.
    let right = f.ws.arrangement().right.size;
    assert_eq!(
        f.ws.resize_pane(f.view, Direction::Right, ResizeStep::FINE),
        Ok(-4.0)
    );
    assert_eq!(f.ws.arrangement().right.size, right - 4.0);
    // Context, at the top of its dock, has no edge upwards: its bottom edge moves up.
    let ctx_share = |ws: &Workspace| match ws.arrangement().root(Region::Dock(DockId::Right)) {
        Some(Node::Split(s)) => s.children[0].weight,
        _ => panic!(),
    };
    f.ws.resize_pane(f.context, Direction::Up, ResizeStep::FINE)
        .unwrap();
    assert!((ctx_share(&f.ws) - 0.49).abs() < 1e-5);
    // Left from buffer.rs: its left edge is the left dock, which shrinks.
    let before = f.ws.arrangement().left.size;
    assert_eq!(
        f.ws.resize_pane(f.buffer, Direction::Left, ResizeStep::NORMAL),
        Ok(-24.0)
    );
    assert_eq!(f.ws.arrangement().left.size, before - 24.0);
    // A dock pane: Files' right edge moves right, the dock grows.
    f.ws.resize_pane(f.tree, Direction::Right, ResizeStep::NORMAL)
        .unwrap();
    assert_eq!(f.ws.arrangement().left.size, before);
    // Equalise.
    f.ws.equalize(split).unwrap();
    assert_eq!(share(&f.ws), 0.5);
    f.ws.resize_pane(f.context, Direction::Down, ResizeStep::NORMAL)
        .unwrap();
    f.ws.equalize_around(f.outline).unwrap();
    ok(&f.ws);
}

#[test]
fn undo_and_redo_restore_where_panes_are_not_what_they_show() {
    let mut f = fixture();
    let start = f.ws.arrangement().clone();
    f.ws.move_pane(f.outline, Target::Dock(DockId::Left))
        .unwrap();
    f.ws.toggle_dock(DockId::Bottom).unwrap();
    let after_two = f.ws.arrangement().clone();
    assert_eq!(f.ws.undo_stack().len(), 2);
    // Reading is not recorded, and survives undo.
    f.ws.set_editor_position(f.buffer, "buffer.rs", 99).unwrap();
    f.ws.undo().unwrap();
    f.ws.undo().unwrap();
    ok(&f.ws);
    assert_eq!(f.ws.arrangement(), &start);
    assert_eq!(
        f.ws.content(f.buffer),
        Some(&PaneContent::Editor {
            path: "buffer.rs".into(),
            caret: 99
        })
    );
    assert_eq!(f.ws.undo(), Err(LayoutError::NothingToUndo));
    f.ws.redo().unwrap();
    f.ws.redo().unwrap();
    assert_eq!(f.ws.arrangement(), &after_two);
    assert_eq!(f.ws.redo(), Err(LayoutError::NothingToRedo));

    // A closed pin comes back as the same pin; a pane opened since goes to its default place.
    let pin =
        f.ws.pin_context(
            Pin {
                symbol: "LineCol".into(),
                symbol_kind: "struct".into(),
                path: "line.rs".into(),
                byte_offset: 7,
                commit: "04e63f8".into(),
                content_hash: None,
            },
            PinPlace::Here,
        )
        .unwrap();
    let pin_content = f.ws.content(pin).cloned();
    f.ws.close(pin).unwrap();
    let opened = f.ws.open(editor("new.rs"), None).unwrap();
    f.ws.undo().unwrap();
    ok(&f.ws);
    assert_eq!(f.ws.content(pin).cloned(), pin_content);
    assert_eq!(region_of(&f.ws, opened), Region::Editor);

    // A failed or empty operation records nothing; a new operation clears redo.
    let n = f.ws.undo_stack().len();
    let _ = f.ws.move_pane(f.buffer, Target::Dock(DockId::Left));
    assert_eq!(f.ws.undo_stack().len(), n);
    f.ws.undo().unwrap();
    assert!(f.ws.undo_stack().can_redo());
    f.ws.toggle_minimize(f.tree).unwrap();
    assert!(!f.ws.undo_stack().can_redo());
}

#[test]
fn a_resize_gesture_is_one_undo_step_and_focus_layouts_undo_too() {
    let mut f = fixture();
    let start = f.ws.arrangement().clone();
    for _ in 0..10 {
        f.ws.resize_pane(f.buffer, Direction::Right, ResizeStep::FINE)
            .unwrap();
    }
    for size in [300.0, 320.0, 340.0] {
        f.ws.resize_dock(DockId::Left, size).unwrap();
    }
    assert_eq!(
        f.ws.undo_stack().len(),
        2,
        "one step per split, one per dock"
    );
    f.ws.seal_undo();
    f.ws.resize_dock(DockId::Left, 360.0).unwrap();
    assert_eq!(
        f.ws.undo_stack().len(),
        3,
        "a sealed gesture starts a new step"
    );
    for _ in 0..3 {
        f.ws.undo().unwrap();
    }
    assert_eq!(f.ws.arrangement(), &start);

    // Entering and ending a focus layout are steps; nested operations record once.
    f.ws.maximize(f.stack, 0).unwrap();
    f.ws.split(f.stack, Edge::Right).unwrap_err();
    let g = f.ws.group_of(f.stack).unwrap();
    f.ws.move_pane(
        f.buffer,
        Target::Beside {
            group: g,
            edge: Edge::Top,
        },
    )
    .unwrap();
    f.ws.restore(10).unwrap();
    f.ws.undo().unwrap();
    assert!(f.ws.in_focus(), "undoing Restore reopens the focus layout");
    assert_eq!(f.ws.focus().unwrap().on_screen(), vec![f.buffer, f.stack]);
    f.ws.undo().unwrap();
    f.ws.undo().unwrap();
    assert!(!f.ws.in_focus());
    assert_eq!(f.ws.arrangement(), &start);
    ok(&f.ws);
}

#[test]
/// design/layout-keyboard.md: "Up to 100 steps". A literal, so changing the constant shows.
fn undo_keeps_at_most_the_limit() {
    let mut f = fixture();
    for _ in 0..120 {
        f.ws.toggle_minimize(f.tree).unwrap();
    }
    assert_eq!(f.ws.undo_stack().len(), 100);
    assert_eq!(UNDO_LIMIT, 100);
}

#[test]
fn toggle_all_docks_brings_back_the_same_ones() {
    let mut f = fixture();
    f.ws.toggle_dock(DockId::Right).unwrap();
    assert_eq!(f.ws.toggle_all_docks(), Ok(false));
    assert!(
        DockId::ALL
            .iter()
            .all(|d| !f.ws.arrangement().dock(*d).visible)
    );
    assert_eq!(f.ws.toggle_all_docks(), Ok(true));
    assert!(f.ws.arrangement().left.visible);
    assert!(!f.ws.arrangement().right.visible);
    assert!(f.ws.arrangement().bottom.visible);
}

#[test]
fn recent_order_is_saved_and_cleaned() {
    let mut f = fixture();
    f.ws.activate(f.outline).unwrap();
    f.ws.activate(f.view).unwrap();
    assert_eq!(&f.ws.recent_panes()[..2], &[f.view, f.outline]);
    f.ws.close(f.outline).unwrap();
    assert!(!f.ws.recent_panes().contains(&f.outline));
    let loaded = Workspace::from_json(&f.ws.to_json());
    assert_eq!(loaded.workspace.recent_panes(), f.ws.recent_panes());
}

// ----- pickers, the switcher and dock sizing ---------------------------------------------------

#[test]
fn recency_order_is_focused_first_then_last_focused_then_never_focused() {
    let mut f = fixture();
    let ws = &mut f.ws;
    ws.activate(f.outline).unwrap();
    ws.activate(f.stack).unwrap();
    ws.activate(f.line).unwrap();
    let order = ws.recency_order();
    assert_eq!(&order[..4], &[f.line, f.stack, f.outline, f.buffer]);
    // Every pane appears once, including ones never focused (the rest, in id order).
    assert_eq!(order.len(), ws.panes().len());
    let mut sorted = order.clone();
    sorted.sort();
    sorted.dedup();
    assert_eq!(sorted.len(), order.len());
    let rest = &order[4..];
    assert!(rest.windows(2).all(|w| w[0] < w[1]));
    // Focusing the same pane again changes nothing; closing one drops it.
    ws.activate(f.line).unwrap();
    assert_eq!(ws.recency_order(), order);
    ws.close(f.stack).unwrap();
    assert!(!ws.recency_order().contains(&f.stack));
}

#[test]
fn the_switcher_opens_on_the_previous_pane_and_steps_with_wrapping() {
    let mut f = fixture();
    f.ws.activate(f.outline).unwrap();
    f.ws.activate(f.graph).unwrap();
    let mut s = Switcher::open(&f.ws, false).unwrap();
    // A single Ctrl-Tab goes back to the previous pane.
    assert_eq!(s.selected(), f.outline);
    assert_eq!(s.order()[0], f.graph);
    s.step(false);
    assert_eq!(s.index(), 2);
    s.step(true);
    s.step(true);
    // Back past the current pane wraps to the least recent one.
    assert_eq!(s.index(), 0);
    s.step(true);
    assert_eq!(s.index(), s.order().len() - 1);
    // Ctrl-Shift-Tab opens on the least recent pane.
    let b = Switcher::open(&f.ws, true).unwrap();
    assert_eq!(b.index(), b.order().len() - 1);
    // Switching twice in a row alternates between the same two panes.
    let first = Switcher::open(&f.ws, false).unwrap().selected();
    f.ws.reveal(first).unwrap();
    assert_eq!(Switcher::open(&f.ws, false).unwrap().selected(), f.graph);
    // The order is a snapshot: focusing elsewhere does not reorder an open switcher.
    let snapshot = Switcher::open(&f.ws, false).unwrap();
    f.ws.activate(f.tree).unwrap();
    assert_ne!(snapshot.order(), f.ws.recency_order().as_slice());
}

#[test]
fn the_switcher_needs_two_panes() {
    let mut ws = Workspace::new();
    assert!(Switcher::open(&ws, false).is_none());
    ws.open(editor("a.rs"), None).unwrap();
    assert!(Switcher::open(&ws, false).is_none());
    ws.open(editor("b.rs"), None).unwrap();
    assert!(Switcher::open(&ws, false).is_some());
}

#[test]
fn pane_entries_list_every_pane_with_its_place() {
    let mut f = fixture();
    let ws = &mut f.ws;
    ws.toggle_dock(DockId::Right).unwrap();
    ws.toggle_minimize(f.stack).unwrap();
    let entries = ws.pane_entries();
    assert_eq!(entries.len(), ws.panes().len());
    let get = |p| entries.iter().find(|e| e.pane == p).unwrap();
    assert_eq!(get(f.tree).place_label(), "Left dock");
    assert_eq!(get(f.buffer).place_label(), "Code area");
    assert_eq!(get(f.context).place_label(), "Right dock, hidden");
    assert_eq!(get(f.stack).place_label(), "Bottom bar, minimised");
    assert_eq!(get(f.graph).place, Place::Dock(DockId::Bottom));
    assert!(get(f.buffer).focused);
    assert_eq!(get(f.outline).title, "Outline");
}

#[test]
fn reveal_shows_a_hidden_dock_unfolds_the_group_and_focuses_the_pane() {
    let mut f = fixture();
    let ws = &mut f.ws;
    ws.toggle_dock(DockId::Right).unwrap();
    ws.toggle_minimize(f.outline).unwrap();
    ws.clear_undo();
    ws.reveal(f.outline).unwrap();
    ok(ws);
    assert_eq!(ws.focused(), Some(f.outline));
    assert!(ws.arrangement().dock(DockId::Right).visible);
    let g = ws.group(ws.group_of(f.outline).unwrap()).unwrap();
    assert!(!g.minimized);
    // Revealing is one undoable layout change.
    assert_eq!(ws.undo_stack().len(), 1);
    ws.undo().unwrap();
    assert!(!ws.arrangement().dock(DockId::Right).visible);
    // A pane already on screen only takes the focus: nothing to undo.
    ws.clear_undo();
    ws.reveal(f.tree).unwrap();
    assert_eq!(ws.focused(), Some(f.tree));
    assert!(!ws.undo_stack().can_undo());
    assert!(ws.reveal(PaneId(9999)).is_err());
}

#[test]
fn reveal_in_a_focus_layout_changes_page_or_brings_the_pane_from_the_tray() {
    let mut f = fixture();
    let ws = &mut f.ws;
    ws.maximize(f.buffer, 1).unwrap();
    // Everything else is in the tray.
    assert!(ws.tray().contains(&f.outline));
    let entries = ws.pane_entries();
    let outline = entries.iter().find(|e| e.pane == f.outline).unwrap();
    assert_eq!(outline.place_label(), "Tray");
    assert_eq!(ws.tray_entries().len(), ws.tray().len());
    ws.reveal(f.outline).unwrap();
    ok(ws);
    assert_eq!(ws.focused(), Some(f.outline));
    assert!(!ws.tray().contains(&f.outline));
    // It arrived beside the pane that was focused, in the same page.
    assert_eq!(
        ws.pane_entry(f.outline).unwrap().place,
        ws.pane_entry(f.buffer).unwrap().place
    );
    // A second page: reveal switches to it.
    ws.new_page(None).unwrap();
    ws.bring_from_tray(f.context, Edge::Bottom).unwrap();
    assert_eq!(ws.pane_entry(f.context).unwrap().place, Place::Page(1));
    ws.go_to_page(0).unwrap();
    ws.reveal(f.context).unwrap();
    assert_eq!(ws.focus().unwrap().current, 1);
    assert_eq!(ws.focused(), Some(f.context));
    ok(ws);
}

#[test]
fn bring_from_tray_places_beside_the_focused_pane_and_refuses_panes_not_in_the_tray() {
    let mut f = fixture();
    let ws = &mut f.ws;
    assert_eq!(
        ws.bring_from_tray(f.outline, Edge::Right),
        Err(LayoutError::NoOp)
    );
    ws.maximize(f.buffer, 1).unwrap();
    let rects = ws.rects(W, H);
    ws.bring_from_tray(f.graph, Edge::Right).unwrap();
    ok(ws);
    let rects_after = ws.rects(W, H);
    let (_, before) = rects[0];
    let g = ws.group_of(f.graph).unwrap();
    let (_, shown) = *rects_after.iter().find(|(id, _)| *id == g).unwrap();
    assert!(
        shown.x >= before.x + before.w * 0.4,
        "graph is on the right"
    );
    assert_eq!(
        ws.bring_from_tray(f.graph, Edge::Right),
        Err(LayoutError::NoOp)
    );
    // Below puts it under the focused pane.
    ws.bring_from_tray(f.stack, Edge::Bottom).unwrap();
    ok(ws);
    assert!(!ws.tray().contains(&f.stack));
}

#[test]
fn destinations_name_every_valid_target_in_order() {
    let f = fixture();
    let ws = &f.ws;
    let dests = ws.destinations(f.outline);
    let targets: Vec<Target> = dests.iter().map(|d| d.target).collect();
    // Every valid target, less whole edges that one group already spans (the row beside it
    // does the same).
    let mut valid = ws.valid_targets(f.outline);
    valid.retain(|t| !matches!(t, Target::AreaEdge(e) if ws.area_edge_is_one_group(*e)));
    assert_eq!(targets, valid);
    let labels: Vec<&str> = dests.iter().map(|d| d.label.as_str()).collect();
    assert_eq!(
        &labels[..4],
        ["Left dock", "Right dock", "Bottom bar", "Code area"]
    );
    // The bottom bar group is named for its active tab.
    assert!(labels.iter().any(|l| l.starts_with("Tab in ")));
    assert!(labels.iter().any(|l| l.starts_with("Beside Tree (")));
    // Same-titled groups are told apart by where they are.
    let beside_editor = dests
        .iter()
        .find(|d| d.label == "Beside buffer.rs (below)")
        .expect("beside the editor");
    assert_eq!(beside_editor.place, "Code area");
    // An editor has no dock targets.
    let editor = ws.destinations(f.buffer);
    assert!(editor.iter().all(|d| !matches!(d.target, Target::Dock(_))));
    // Every destination is accepted by the model.
    for d in &dests {
        let mut copy = ws.clone();
        copy.move_pane(f.outline, d.target).unwrap();
        ok(&copy);
    }
}

#[test]
fn tray_is_a_destination_only_in_a_focus_layout() {
    let mut f = fixture();
    assert!(
        !f.ws
            .destinations(f.buffer)
            .iter()
            .any(|d| d.target == Target::Tray)
    );
    f.ws.maximize(f.buffer, 1).unwrap();
    f.ws.bring_from_tray(f.outline, Edge::Right).unwrap();
    let d = f.ws.destinations(f.outline);
    assert_eq!(d.last().map(|d| d.label.as_str()), Some("Tray"));
}

#[test]
fn dock_keys_grow_shrink_and_reset_the_focused_docks_size() {
    let mut f = fixture();
    let ws = &mut f.ws;
    // The focus is in the editor area: no dock to size.
    assert_eq!(ws.step_focused_dock(true), Err(LayoutError::NotInDock));
    ws.activate(f.outline).unwrap();
    assert_eq!(ws.focused_dock(), Ok(DockId::Right));
    let start = ws.arrangement().dock(DockId::Right).size;
    assert_eq!(ws.step_focused_dock(true), Ok(start + DOCK_STEP));
    assert_eq!(ws.step_focused_dock(false), Ok(start));
    assert_eq!(ws.step_focused_dock(false), Ok(start - DOCK_STEP));
    // Repeated steps are one undo step.
    ws.clear_undo();
    ws.step_focused_dock(true).unwrap();
    ws.step_focused_dock(true).unwrap();
    assert_eq!(ws.undo_stack().len(), 1);
    // Clamped at both ends.
    let (lo, hi) = DockId::Right.size_range();
    for _ in 0..100 {
        ws.step_focused_dock(true).unwrap();
    }
    assert_eq!(ws.arrangement().dock(DockId::Right).size, hi);
    for _ in 0..100 {
        ws.step_focused_dock(false).unwrap();
    }
    assert_eq!(ws.arrangement().dock(DockId::Right).size, lo);
    assert_eq!(ws.reset_focused_dock(), Ok(DockId::Right.default_size()));
    // Other docks follow the focus; the bottom bar sizes its height.
    ws.activate(f.graph).unwrap();
    let h = ws.arrangement().dock(DockId::Bottom).size;
    assert_eq!(ws.step_focused_dock(true), Ok(h + DOCK_STEP));
    assert_eq!(
        ws.arrangement().dock(DockId::Right).size,
        DockId::Right.default_size()
    );
    // Docks are hidden in a focus layout.
    ws.maximize(f.graph, 1).unwrap();
    assert_eq!(
        ws.step_focused_dock(true),
        Err(LayoutError::DocksHiddenInFocus)
    );
    ok(ws);
}

#[test]
fn shift_arrow_at_the_edge_spans_the_whole_code_area() {
    let mut f = fixture();
    // view.rs is alone in its group, beside buffer.rs and line.rs, with nothing below it in
    // the code area: Shift-Down puts it under both groups, across the whole width.
    let t =
        f.ws.move_pane_dir(f.view, Direction::Down, &f.ws.rects(W, H))
            .unwrap();
    assert_eq!(t, Target::AreaEdge(Edge::Bottom));
    ok(&f.ws);
    let Some(Node::Split(s)) = f.ws.arrangement().root(Region::Editor).clone() else {
        panic!("the code area is a split");
    };
    assert_eq!(s.axis, Axis::Column);
    assert!(matches!(&s.children[1].node, Node::Group(g) if g.tabs == [f.view]));
    // Again: it already spans the bottom, so there is nothing further that way.
    assert_eq!(
        f.ws.move_pane_dir(f.view, Direction::Down, &f.ws.rects(W, H)),
        Err(LayoutError::NothingThatWay)
    );
}

/// Files opened together: the first maximised, the rest in the tray (`cho a b c d`).
fn flip_fixture() -> (Workspace, [PaneId; 4]) {
    let mut ws = Workspace::with_defaults();
    let a = ws.open(editor("a.rs"), None).unwrap();
    ws.maximize(a, 1).unwrap();
    let [b, c, d] =
        ["b.rs", "c.rs", "d.rs"].map(|p| ws.open(editor(p), Some(Target::Tray)).unwrap());
    ws.clear_undo();
    ok(&ws);
    (ws, [a, b, c, d])
}

#[test]
fn tab_flips_through_the_tray_files_in_one_spot() {
    let (mut ws, [a, b, c, d]) = flip_fixture();
    let spot = ws.group_of(a).unwrap();
    for expected in [b, c, d, a, b] {
        let next = ws.tray_file(true).unwrap();
        assert_eq!(next, expected);
        ws.swap_from_tray(next).unwrap();
        ok(&ws);
        assert_eq!(ws.focused(), Some(expected));
        assert_eq!(ws.group_of(expected), Some(spot), "same spot on the page");
        assert_eq!(
            ws.focus().unwrap().on_pages().len(),
            1,
            "one file on the page"
        );
    }
    // Back the other way, and one undo per flip.
    assert_eq!(ws.tray_file(false), Some(a));
    ws.swap_from_tray(a).unwrap();
    assert_eq!(ws.focused(), Some(a));
    ws.undo().unwrap();
    assert_eq!(
        ws.focus().unwrap().on_pages(),
        [b],
        "undo puts b back on the page"
    );
    assert!(ws.tray().contains(&a));
    ok(&ws);
}

#[test]
fn tray_labels_carry_on_from_the_screen() {
    let (ws, [_, b, c, d]) = flip_fixture();
    let screen = ws.labels(&LABEL_LETTERS);
    assert_eq!(screen.first().map(|(l, _)| *l), Some(Label::Number(1)));
    let tray = ws.tray_labels(&LABEL_LETTERS, &screen);
    let files: Vec<(Label, PaneId)> = tray
        .iter()
        .copied()
        .filter(|(l, _)| matches!(l, Label::Number(_)))
        .collect();
    assert_eq!(
        files,
        [
            (Label::Number(2), b),
            (Label::Number(3), c),
            (Label::Number(4), d)
        ]
    );
    // The tool panes in the tray get letters, none already on screen.
    assert!(tray.iter().any(|(l, _)| matches!(l, Label::Letter(_))));
    assert!(!tray.iter().any(|(l, _)| screen.iter().any(|(s, _)| s == l)));
    // A pane that is not in the tray can't be swapped in.
    let (mut ws, [a, ..]) = flip_fixture();
    assert!(ws.swap_from_tray(a).is_err());
}
