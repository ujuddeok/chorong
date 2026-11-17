//! Each layout operation, by example. Every test checks the invariants after each step.

mod common;

use cho_layout::*;
use common::*;

/// Files on the left; buffer.rs and line.rs in one editor group; Context and Outline on the
/// right; a call stack and a graph in the bottom bar.
struct Fixture {
    ws: Workspace,
    tree: PaneId,
    context: PaneId,
    outline: PaneId,
    buffer: PaneId,
    line: PaneId,
    stack: PaneId,
    graph: PaneId,
}

fn fixture() -> Fixture {
    let mut ws = Workspace::with_defaults();
    let tree = find(&ws, PaneKind::FileTree);
    let context = find(&ws, PaneKind::Context);
    let outline = find(&ws, PaneKind::Outline);
    let buffer = ws
        .open(editor("crates/cho-core/src/buffer.rs"), None)
        .unwrap();
    let line = ws
        .open(editor("crates/cho-core/src/line.rs"), None)
        .unwrap();
    let stack = ws.open(call_stack("buffer-flow"), None).unwrap();
    let graph = ws
        .open(
            PaneContent::Graph {
                query: "LineCol".into(),
                title: "Graph LineCol".into(),
            },
            None,
        )
        .unwrap();
    ws.activate(buffer).unwrap();
    ok(&ws);
    Fixture {
        ws,
        tree,
        context,
        outline,
        buffer,
        line,
        stack,
        graph,
    }
}

#[test]
fn default_layout_matches_layout_md() {
    let f = fixture();
    let a = f.ws.arrangement();
    assert_eq!(region_of(&f.ws, f.tree), Region::Dock(DockId::Left));
    assert_eq!(region_of(&f.ws, f.context), Region::Dock(DockId::Right));
    assert_eq!(region_of(&f.ws, f.outline), Region::Dock(DockId::Right));
    // Context and Outline are stacked, Context on top.
    let Some(Node::Split(s)) = &a.right.root else {
        panic!("right dock should be a split")
    };
    assert_eq!(s.axis, Axis::Column);
    assert_eq!(s.children[0].node.panes(), vec![f.context]);
    // Both files are tabs of one editor group; the bottom bar has both tools as tabs.
    assert_eq!(a.editor.as_ref().unwrap().panes(), vec![f.buffer, f.line]);
    assert_eq!(
        a.bottom.root.as_ref().unwrap().panes(),
        vec![f.stack, f.graph]
    );
}

#[test]
fn move_to_dock_beside_and_onto_tabs() {
    let mut f = fixture();
    f.ws.move_pane(f.outline, Target::Dock(DockId::Left))
        .unwrap();
    ok(&f.ws);
    assert_eq!(region_of(&f.ws, f.outline), Region::Dock(DockId::Left));
    // The right dock collapsed to Context alone.
    assert!(matches!(
        f.ws.arrangement().right.root,
        Some(Node::Group(_))
    ));

    let ctx_group = f.ws.group_of(f.context).unwrap();
    f.ws.move_pane(
        f.outline,
        Target::Beside {
            group: ctx_group,
            edge: Edge::Left,
        },
    )
    .unwrap();
    ok(&f.ws);
    let Some(Node::Split(s)) = &f.ws.arrangement().right.root else {
        panic!()
    };
    assert_eq!(s.axis, Axis::Row);
    assert_eq!(
        f.ws.arrangement().right.root.as_ref().unwrap().panes(),
        vec![f.outline, f.context]
    );

    f.ws.move_pane(
        f.outline,
        Target::Tabs {
            group: ctx_group,
            index: 0,
        },
    )
    .unwrap();
    ok(&f.ws);
    let g = f.ws.group(ctx_group).unwrap();
    assert_eq!(g.tabs, vec![f.outline, f.context]);
    assert_eq!(g.active_pane(), Some(f.outline));
}

#[test]
fn editors_never_go_in_docks() {
    let mut f = fixture();
    let before = f.ws.clone();
    assert_eq!(
        f.ws.move_pane(f.buffer, Target::Dock(DockId::Left)),
        Err(LayoutError::EditorOutsideEditorArea)
    );
    let tree_group = f.ws.group_of(f.tree).unwrap();
    assert_eq!(
        f.ws.move_pane(
            f.buffer,
            Target::Beside {
                group: tree_group,
                edge: Edge::Right
            }
        ),
        Err(LayoutError::EditorOutsideEditorArea)
    );
    assert_eq!(f.ws, before, "a failed operation changes nothing");
}

#[test]
fn tool_panes_share_the_editor_area_but_never_an_editor_strip() {
    let mut f = fixture();
    let eg = f.ws.group_of(f.buffer).unwrap();
    let before = f.ws.clone();
    // Not as a tab among files.
    assert_eq!(
        f.ws.move_pane(
            f.outline,
            Target::Tabs {
                group: eg,
                index: 0
            }
        ),
        Err(LayoutError::MixedKinds)
    );
    assert_eq!(f.ws, before);
    // As a column beside the editor group: "Move to editor area".
    f.ws.move_pane(f.outline, Target::EditorArea).unwrap();
    ok(&f.ws);
    let Some(Node::Split(s)) = &f.ws.arrangement().editor else {
        panic!("editor area should be split")
    };
    assert_eq!(s.axis, Axis::Row);
    assert_eq!(s.children[0].node.panes(), vec![f.buffer, f.line]);
    assert_eq!(s.children[1].node.panes(), vec![f.outline]);
    // And the other way round: no editor onto the Outline group's strip.
    let og = f.ws.group_of(f.outline).unwrap();
    assert_eq!(
        f.ws.move_pane(
            f.line,
            Target::Tabs {
                group: og,
                index: 0
            }
        ),
        Err(LayoutError::MixedKinds)
    );
    // Beside it is fine, and a tool can stack under the editor group too.
    f.ws.move_pane(
        f.line,
        Target::Beside {
            group: og,
            edge: Edge::Bottom,
        },
    )
    .unwrap();
    f.ws.move_pane(
        f.graph,
        Target::Beside {
            group: eg,
            edge: Edge::Bottom,
        },
    )
    .unwrap();
    ok(&f.ws);
    // A tool split off its strip inside the editor area stays there.
    f.ws.move_pane(
        f.stack,
        Target::Tabs {
            group: f.ws.group_of(f.graph).unwrap(),
            index: 9,
        },
    )
    .unwrap();
    f.ws.split(f.stack, Edge::Right).unwrap();
    assert_eq!(
        f.ws.arrangement().locate(f.stack).unwrap().0,
        Region::Editor
    );
    ok(&f.ws);
    // Restore round-trips the mixed tree exactly.
    let mixed = f.ws.arrangement().clone();
    f.ws.maximize(f.outline, 0).unwrap();
    f.ws.restore(1).unwrap();
    assert_eq!(f.ws.arrangement(), &mixed);
    // Closing the last editors sends the editor area's tools to their docks.
    f.ws.close(f.buffer).unwrap();
    f.ws.close(f.line).unwrap();
    ok(&f.ws);
    assert!(f.ws.arrangement().editor.is_none());
    assert_eq!(region_of(&f.ws, f.outline), Region::Dock(DockId::Right));
    assert_eq!(region_of(&f.ws, f.stack), Region::Dock(DockId::Bottom));
    // With no editor group, a tool has nothing to stand beside.
    assert_eq!(
        f.ws.move_pane(f.outline, Target::EditorArea),
        Err(LayoutError::NoEditorGroup)
    );
}

#[test]
fn split_editor_duplicates_and_close_collapses() {
    let mut f = fixture();
    let eg = f.ws.group_of(f.buffer).unwrap();
    f.ws.set_editor_position(f.buffer, "crates/cho-core/src/buffer.rs", 1234)
        .unwrap();
    let copy = f.ws.split(f.buffer, Edge::Right).unwrap();
    ok(&f.ws);
    assert_eq!(f.ws.content(copy), f.ws.content(f.buffer));
    let Some(Node::Split(s)) = &f.ws.arrangement().editor else {
        panic!()
    };
    assert_eq!(s.axis, Axis::Row);
    assert_eq!(s.children[0].node.id(), eg);
    // The docks are untouched by an editor split.
    assert_eq!(f.ws.arrangement().right, fixture().ws.arrangement().right);

    f.ws.close(copy).unwrap();
    ok(&f.ws);
    assert!(matches!(f.ws.arrangement().editor, Some(Node::Group(_))));
}

#[test]
fn split_a_lone_tool_pane_is_refused_but_a_tab_splits_off() {
    let mut f = fixture();
    assert_eq!(
        f.ws.split(f.tree, Edge::Bottom),
        Err(LayoutError::CannotSplit)
    );
    let g = f.ws.group_of(f.stack).unwrap();
    assert_eq!(f.ws.split(f.graph, Edge::Right), Ok(f.graph));
    ok(&f.ws);
    assert_ne!(f.ws.group_of(f.graph), Some(g));
    assert_eq!(region_of(&f.ws, f.graph), Region::Dock(DockId::Bottom));
}

#[test]
fn moving_a_pane_onto_its_own_lone_group_is_a_no_op() {
    let mut f = fixture();
    let g = f.ws.group_of(f.tree).unwrap();
    for edge in Edge::ALL {
        assert_eq!(
            f.ws.move_pane(f.tree, Target::Beside { group: g, edge }),
            Err(LayoutError::NoOp)
        );
    }
    assert_eq!(
        f.ws.move_group(g, Target::Tabs { group: g, index: 0 }),
        Err(LayoutError::NoOp)
    );
}

#[test]
fn reorder_tabs_within_a_strip() {
    let mut f = fixture();
    let g = f.ws.group_of(f.buffer).unwrap();
    f.ws.move_pane(f.line, Target::Tabs { group: g, index: 0 })
        .unwrap();
    assert_eq!(f.ws.group(g).unwrap().tabs, vec![f.line, f.buffer]);
    assert_eq!(f.ws.tab_drop_index(g, f.line, 2), Some(1));
    f.ws.move_pane(f.line, Target::Tabs { group: g, index: 2 })
        .unwrap();
    assert_eq!(f.ws.group(g).unwrap().tabs, vec![f.buffer, f.line]);
    ok(&f.ws);
}

#[test]
fn move_group_keeps_tabs_active_and_minimised() {
    let mut f = fixture();
    let g = f.ws.group_of(f.stack).unwrap();
    f.ws.activate(f.graph).unwrap();
    f.ws.toggle_minimize(f.graph).unwrap();
    f.ws.move_group(g, Target::Dock(DockId::Right)).unwrap();
    ok(&f.ws);
    let ng = f.ws.group_of(f.stack).unwrap();
    let group = f.ws.group(ng).unwrap();
    assert_eq!(group.tabs, vec![f.stack, f.graph]);
    assert_eq!(group.active_pane(), Some(f.graph));
    assert!(group.minimized);
    assert!(f.ws.arrangement().bottom.root.is_none());
}

#[test]
fn resize_clamps_and_dock_sizes_clamp() {
    let mut f = fixture();
    let Some(Node::Split(s)) = &f.ws.arrangement().right.root else {
        panic!()
    };
    let id = s.id;
    // The smallest share a split child keeps is 5% (a literal, so changing it shows).
    let applied = f.ws.resize_split(id, 0, 10.0).unwrap();
    assert!((applied - (0.5 - 0.05)).abs() < 1e-6, "{applied}");
    ok(&f.ws);
    assert_eq!(
        f.ws.resize_split(id, 1, 0.1),
        Err(LayoutError::UnknownSplit(id))
    );
    assert_eq!(f.ws.resize_dock(DockId::Left, 5.0), Ok(160.0));
    assert_eq!(f.ws.resize_dock(DockId::Bottom, 10_000.0), Ok(600.0));
    ok(&f.ws);
}

#[test]
fn minimise_toggles_and_docks_hide_without_losing_panes() {
    let mut f = fixture();
    assert_eq!(f.ws.toggle_minimize(f.outline), Ok(true));
    assert_eq!(f.ws.toggle_minimize(f.outline), Ok(false));
    assert_eq!(f.ws.toggle_dock(DockId::Left), Ok(false));
    assert_eq!(region_of(&f.ws, f.tree), Region::Dock(DockId::Left));
    // Moving a pane into a hidden dock shows it.
    f.ws.move_pane(f.outline, Target::Dock(DockId::Left))
        .unwrap();
    assert!(f.ws.arrangement().left.visible);
    ok(&f.ws);
}

#[test]
fn pin_here_goes_directly_under_context_newest_first() {
    let mut f = fixture();
    let a =
        f.ws.pin_context(pin("utf16_to_byte"), PinPlace::Here)
            .unwrap();
    let b = f.ws.pin_context(pin("LineCol"), PinPlace::Here).unwrap();
    ok(&f.ws);
    let right = f.ws.arrangement().right.root.as_ref().unwrap().panes();
    assert_eq!(right, vec![f.context, b, a, f.outline]);
    // Pinning does not take focus from the code.
    assert_eq!(f.ws.focused(), Some(f.buffer));
    match f.ws.content(a) {
        Some(PaneContent::PinnedContext(p)) => {
            assert_eq!(p.commit, "04e63f8");
            assert_eq!(p.byte_offset, 42);
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn pins_in_the_bottom_bar_follow_tool_tabs() {
    let mut f = fixture();
    let p =
        f.ws.pin_context(pin("LineCol"), PinPlace::BottomBar)
            .unwrap();
    let g = f.ws.group_of(f.stack).unwrap();
    // A pin dropped first lands after the tools; a tool dropped last lands before the pins.
    assert_eq!(f.ws.tab_drop_index(g, p, 0), Some(2));
    f.ws.move_pane(p, Target::Tabs { group: g, index: 0 })
        .unwrap_err();
    let s2 = f.ws.open(call_stack("second"), None).unwrap();
    assert_eq!(f.ws.group(g).unwrap().tabs, vec![f.stack, f.graph, s2, p]);
    ok(&f.ws);
    // Pins move freely between the right dock and the bottom bar.
    f.ws.move_pane(p, Target::Dock(DockId::Right)).unwrap();
    assert_eq!(region_of(&f.ws, p), Region::Dock(DockId::Right));
    ok(&f.ws);
}

#[test]
fn closing_a_pin_unpins_it() {
    let mut f = fixture();
    let p = f.ws.pin_context(pin("LineCol"), PinPlace::Here).unwrap();
    f.ws.close(p).unwrap();
    assert!(f.ws.content(p).is_none());
    assert_eq!(f.ws.close(p), Err(LayoutError::UnknownPane(p)));
    ok(&f.ws);
}

#[test]
fn maximise_hides_everything_else_in_the_tray() {
    let mut f = fixture();
    let before = f.ws.arrangement().clone();
    f.ws.maximize(f.stack, 1000).unwrap();
    ok(&f.ws);
    assert!(f.ws.in_focus());
    assert_eq!(f.ws.focus().unwrap().on_screen(), vec![f.stack]);
    assert_eq!(
        f.ws.tray(),
        vec![f.tree, f.buffer, f.line, f.context, f.outline, f.graph]
    );
    assert_eq!(
        f.ws.arrangement(),
        &before,
        "entering focus closes and moves nothing"
    );
    assert_eq!(
        f.ws.toggle_dock(DockId::Left),
        Err(LayoutError::DocksHiddenInFocus)
    );
    assert_eq!(
        f.ws.maximize(f.tree, 1000),
        Err(LayoutError::AlreadyInFocus)
    );
}

#[test]
fn dock_others_into_focus_then_restore_exactly() {
    let mut f = fixture();
    let before = f.ws.arrangement().clone();
    f.ws.maximize(f.stack, 1000).unwrap();
    let g = f.ws.group_of(f.stack).unwrap();
    f.ws.move_pane(
        f.buffer,
        Target::Beside {
            group: g,
            edge: Edge::Top,
        },
    )
    .unwrap();
    let bg = f.ws.group_of(f.buffer).unwrap();
    f.ws.move_pane(
        f.context,
        Target::Beside {
            group: bg,
            edge: Edge::Right,
        },
    )
    .unwrap();
    ok(&f.ws);
    assert_eq!(
        f.ws.focus().unwrap().on_screen(),
        vec![f.buffer, f.context, f.stack]
    );
    assert!(!f.ws.tray().contains(&f.buffer));
    // Docks and the editor area are hidden: those targets are refused.
    assert_eq!(
        f.ws.move_pane(f.tree, Target::Dock(DockId::Left)),
        Err(LayoutError::DocksHiddenInFocus)
    );
    // Rearranging in focus leaves the normal arrangement alone.
    f.ws.toggle_minimize(f.context).unwrap();
    assert_eq!(f.ws.arrangement(), &before);
    f.ws.move_pane(f.context, Target::Tray).unwrap();
    assert!(f.ws.tray().contains(&f.context));

    let rec = f.ws.restore(1600).unwrap();
    ok(&f.ws);
    assert!(!f.ws.in_focus());
    assert_eq!(f.ws.arrangement(), &before);
    assert_eq!(rec.unwrap().label(), "buffer.rs, Call stack buffer-flow");
    assert_eq!(f.ws.history().len(), 1);
}

#[test]
fn restore_keeps_the_reading_position() {
    let mut f = fixture();
    // buffer.rs is the active tab before focus; line.rs is read during focus.
    let before = f.ws.arrangement().clone();
    f.ws.maximize(f.stack, 0).unwrap();
    let g = f.ws.group_of(f.stack).unwrap();
    f.ws.move_pane(
        f.line,
        Target::Beside {
            group: g,
            edge: Edge::Top,
        },
    )
    .unwrap();
    f.ws.activate(f.line).unwrap();
    f.ws.set_editor_position(f.line, "crates/cho-core/src/line.rs", 777)
        .unwrap();
    f.ws.restore(60).unwrap();
    ok(&f.ws);
    let eg = f.ws.group_of(f.line).unwrap();
    assert_eq!(f.ws.group(eg).unwrap().active_pane(), Some(f.line));
    assert_eq!(
        f.ws.content(f.line),
        Some(&PaneContent::Editor {
            path: "crates/cho-core/src/line.rs".into(),
            caret: 777
        })
    );
    assert_eq!(f.ws.focused(), Some(f.line));
    // Apart from the active editor tab, the layout is exactly as it was.
    let mut expected = before;
    if let Some(Node::Group(g)) = &mut expected.editor {
        g.active = 1;
    }
    assert_eq!(f.ws.arrangement(), &expected);
}

#[test]
fn a_pin_made_in_focus_goes_to_its_default_place() {
    let mut f = fixture();
    f.ws.maximize(f.context, 0).unwrap();
    let p = f.ws.pin_context(pin("LineCol"), PinPlace::Here).unwrap();
    ok(&f.ws);
    // On screen under Context, since Context is on screen.
    assert_eq!(f.ws.focus().unwrap().on_screen(), vec![f.context, p]);
    f.ws.restore(10).unwrap();
    ok(&f.ws);
    assert_eq!(
        f.ws.arrangement().right.root.as_ref().unwrap().panes(),
        vec![f.context, p, f.outline]
    );

    // A pin to the bottom bar, made while Context is off screen, waits in the tray.
    f.ws.maximize(f.buffer, 20).unwrap();
    let q = f.ws.pin_context(pin("Other"), PinPlace::BottomBar).unwrap();
    assert!(f.ws.tray().contains(&q));
    f.ws.restore(30).unwrap();
    assert_eq!(region_of(&f.ws, q), Region::Dock(DockId::Bottom));
    ok(&f.ws);
}

#[test]
fn closing_in_focus_closes_everywhere_and_last_pane_ends_focus() {
    let mut f = fixture();
    let p = f.ws.pin_context(pin("LineCol"), PinPlace::Here).unwrap();
    f.ws.maximize(p, 0).unwrap();
    f.ws.close(p).unwrap();
    ok(&f.ws);
    assert!(!f.ws.in_focus());
    assert!(f.ws.arrangement().locate(p).is_none());
    assert!(f.ws.history().is_empty());
}

#[test]
fn toggle_maximise_is_cmd_k_m() {
    let mut f = fixture();
    assert_eq!(f.ws.toggle_maximize(f.buffer, 0), Ok(true));
    assert_eq!(f.ws.toggle_maximize(f.buffer, 5), Ok(false));
    assert!(!f.ws.in_focus());
    assert_eq!(f.ws.restore(5), Err(LayoutError::NotInFocus));
}

#[test]
fn focus_history_is_newest_first_deduplicated_and_capped() {
    let mut f = fixture();
    let panes = [
        f.tree, f.context, f.outline, f.buffer, f.line, f.stack, f.graph,
    ];
    let mut t = 0;
    for round in 0..30 {
        let a = panes[round % panes.len()];
        let b = panes[(round / panes.len() + round + 1) % panes.len()];
        f.ws.maximize(a, t).unwrap();
        if a != b {
            let g = f.ws.group_of(a).unwrap();
            f.ws.move_pane(
                b,
                Target::Beside {
                    group: g,
                    edge: Edge::Right,
                },
            )
            .unwrap();
        }
        t += 60;
        f.ws.restore(t).unwrap();
        ok(&f.ws);
    }
    assert_eq!(f.ws.history().len(), HISTORY_LIMIT);
    let times: Vec<u64> = f.ws.history().iter().map(|r| r.ended_at).collect();
    assert!(
        times.windows(2).all(|w| w[0] > w[1]),
        "newest first: {times:?}"
    );

    // The same layout again replaces its old entry instead of adding one.
    let first_label = f.ws.history()[0].label();
    f.ws.maximize(f.tree, t).unwrap();
    f.ws.restore(t + 1).unwrap();
    f.ws.maximize(f.tree, t + 2).unwrap();
    f.ws.restore(t + 3).unwrap();
    assert_eq!(f.ws.history()[0].label(), "Tree");
    assert_ne!(f.ws.history()[1].label(), "Tree");
    let _ = first_label;
}

#[test]
fn reopen_reports_missing_panes_and_uses_current_content() {
    let mut f = fixture();
    let p = f.ws.pin_context(pin("LineCol"), PinPlace::Here).unwrap();
    f.ws.maximize(f.stack, 0).unwrap();
    let g = f.ws.group_of(f.stack).unwrap();
    f.ws.move_pane(
        f.buffer,
        Target::Beside {
            group: g,
            edge: Edge::Top,
        },
    )
    .unwrap();
    f.ws.move_pane(
        p,
        Target::Beside {
            group: g,
            edge: Edge::Right,
        },
    )
    .unwrap();
    f.ws.restore(600).unwrap();

    // Later: the pin is closed, buffer.rs's tab is closed, and buffer.rs was read elsewhere.
    f.ws.close(p).unwrap();
    f.ws.close(f.buffer).unwrap();
    f.ws.set_editor_position(f.line, "crates/cho-core/src/line.rs", 5)
        .unwrap();

    let entries =
        f.ws.focus_history(1200, |s| matches!(s, PaneSource::Editor { .. }));
    assert_eq!(entries.len(), 1);
    assert_eq!(
        entries[0].label,
        "buffer.rs, Call stack buffer-flow, LineCol"
    );
    assert_eq!(entries[0].ago, "10 min ago");
    assert_eq!(
        entries[0].missing,
        vec![(
            0,
            PaneSource::Pin {
                symbol: "LineCol".into(),
                commit: "04e63f8".into()
            }
        )]
    );

    let report =
        f.ws.reopen_focus(0, 1200, |s| matches!(s, PaneSource::Editor { .. }))
            .unwrap();
    ok(&f.ws);
    assert_eq!(report.missing.len(), 1);
    assert_eq!(report.created.len(), 1, "buffer.rs reopens in a new tab");
    let on_screen = f.ws.focus().unwrap().on_screen();
    assert_eq!(on_screen, vec![report.created[0], f.stack]);
    assert_eq!(f.ws.focus().unwrap().maximized, f.stack);

    // If nothing can be shown, nothing changes.
    f.ws.restore(1300).unwrap();
    let mut g2 = Workspace::with_defaults();
    let only = g2.pin_context(pin("Gone"), PinPlace::Here).unwrap();
    g2.maximize(only, 0).unwrap();
    g2.restore(1).unwrap();
    g2.close(only).unwrap();
    let before = g2.clone();
    assert_eq!(
        g2.reopen_focus(0, 2, |_| true),
        Err(LayoutError::NothingToReopen)
    );
    assert_eq!(g2, before);
    assert_eq!(
        g2.reopen_focus(9, 2, |_| true),
        Err(LayoutError::NoSuchHistoryEntry(9))
    );
}

#[test]
fn ago_reads_naturally() {
    assert_eq!(ago(100, 100), "just now");
    assert_eq!(ago(700, 100), "10 min ago");
    assert_eq!(ago(3 * 3600, 0), "3 h ago");
    assert_eq!(ago(90_000, 0), "1 day ago");
    assert_eq!(ago(3 * 86_400, 0), "3 days ago");
    assert_eq!(ago(0, 100), "just now");
}

#[test]
fn focus_pages_hold_their_own_layouts_and_share_the_tray() {
    let mut f = fixture();
    let before = f.ws.arrangement().clone();
    f.ws.maximize(f.buffer, 0).unwrap();
    // Page 2 starts empty; panes come from the tray.
    assert_eq!(f.ws.new_page(None), Ok(1));
    assert!(f.ws.focus().unwrap().root().is_none());
    f.ws.place_on_empty_page(f.stack).unwrap();
    let g = f.ws.group_of(f.stack).unwrap();
    f.ws.move_pane(
        f.outline,
        Target::Beside {
            group: g,
            edge: Edge::Right,
        },
    )
    .unwrap();
    ok(&f.ws);
    assert_eq!(f.ws.focus().unwrap().on_screen(), vec![f.stack, f.outline]);
    assert!(!f.ws.tray().contains(&f.stack));
    // Page 1 still has buffer.rs alone; the reading position carries across.
    f.ws.prev_page().unwrap();
    assert_eq!(f.ws.focus().unwrap().on_screen(), vec![f.buffer]);
    assert_eq!(f.ws.focused(), Some(f.buffer));
    assert_eq!(f.ws.prev_page(), Err(LayoutError::NoOp));
    // A pane moves between pages; it is never on two.
    f.ws.move_pane_to_page(f.outline, 0).unwrap();
    ok(&f.ws);
    assert_eq!(f.ws.focus().unwrap().on_screen(), vec![f.buffer, f.outline]);
    assert_eq!(f.ws.focus().unwrap().page_of(f.outline), Some(0));
    assert_eq!(f.ws.move_pane_to_page(f.outline, 0), Err(LayoutError::NoOp));
    // Moving a pane from another page onto this one with a drop.
    let bg = f.ws.group_of(f.buffer).unwrap();
    f.ws.move_pane(
        f.stack,
        Target::Beside {
            group: bg,
            edge: Edge::Bottom,
        },
    )
    .unwrap();
    assert_eq!(f.ws.focus().unwrap().pages[1].root, None);
    ok(&f.ws);
    // A new page takes the focused pane with it (it had company on page 1).
    let moved = f.ws.focused().unwrap();
    let stays = if moved == f.buffer { f.stack } else { f.buffer };
    f.ws.new_page(None).unwrap();
    assert_eq!(f.ws.focus().unwrap().on_screen(), vec![moved]);
    // Reorder pages, keeping the same page on screen.
    f.ws.go_to_page(0).unwrap();
    f.ws.move_page(0, 2).unwrap();
    assert_eq!(f.ws.focus().unwrap().current, 2);
    assert_eq!(f.ws.go_to_page(7), Err(LayoutError::NoSuchPage(7)));
    // Closing a page sends its panes to the tray.
    f.ws.close_page(2, 5).unwrap();
    ok(&f.ws);
    assert!(f.ws.tray().contains(&stays));
    assert_eq!(f.ws.focus().unwrap().pages.len(), 2);
    // Restore ends every page and returns the exact layout.
    f.ws.restore(10).unwrap();
    ok(&f.ws);
    assert_eq!(f.ws.arrangement(), &before);
}

#[test]
fn closing_the_last_page_ends_focus_and_history_keeps_every_page() {
    let mut f = fixture();
    f.ws.maximize(f.buffer, 0).unwrap();
    f.ws.new_page(Some(PageOrigin {
        tour_step: Some("step-3".into()),
        template: Some("code-and-stack".into()),
        panes: vec![PaneSource::CallStack {
            investigation: "buffer-flow".into(),
        }],
        ..Default::default()
    }))
    .unwrap();
    f.ws.place_on_empty_page(f.stack).unwrap();
    f.ws.new_page(None).unwrap(); // left empty: not recorded
    f.ws.close_page(0, 1).unwrap();
    f.ws.close_page(0, 2).unwrap();
    assert!(f.ws.in_focus());
    f.ws.close_page(0, 3).unwrap();
    assert!(!f.ws.in_focus(), "the last page closing ends focus");
    ok(&f.ws);
    // Only the non-empty pages were recorded (the empty one and the closed buffer page not).
    // The page with an origin was closed second, so nothing is left: no record.
    assert!(f.ws.history().is_empty());

    f.ws.maximize(f.buffer, 10).unwrap();
    f.ws.new_page(Some(PageOrigin {
        tour_step: Some("step-3".into()),
        ..Default::default()
    }))
    .unwrap();
    f.ws.place_on_empty_page(f.stack).unwrap();
    f.ws.new_page(None).unwrap();
    f.ws.restore(20).unwrap();
    let rec = &f.ws.history()[0];
    assert_eq!(rec.pages.len(), 2);
    assert_eq!(rec.label(), "buffer.rs | Call stack buffer-flow");
    assert_eq!(
        rec.pages[1].origin.as_ref().unwrap().tour_step.as_deref(),
        Some("step-3")
    );

    // Reopen recreates both pages; a missing pane is reported with its page.
    f.ws.close(f.stack).unwrap();
    let report = f.ws.reopen_focus(0, 30, |_| false).unwrap();
    ok(&f.ws);
    assert_eq!(
        report.missing,
        vec![(
            1,
            PaneSource::CallStack {
                investigation: "buffer-flow".into()
            }
        )]
    );
    assert_eq!(
        f.ws.focus().unwrap().pages.len(),
        1,
        "an emptied page is dropped"
    );
    let report = {
        f.ws.restore(40).unwrap();
        f.ws.reopen_focus(1, 50, |_| true).unwrap()
    };
    ok(&f.ws);
    assert_eq!(
        report.created.len(),
        1,
        "the closed call stack is recreated"
    );
    assert_eq!(f.ws.focus().unwrap().pages.len(), 2);
    assert!(f.ws.focus().unwrap().pages[1].origin.is_some());
}

#[test]
fn a_new_page_starts_with_the_focused_pane_unless_it_is_alone() {
    let mut f = fixture();
    f.ws.maximize(f.buffer, 0).unwrap();
    // Alone on page 1: the new page starts empty, page 1 keeps buffer.rs.
    assert_eq!(f.ws.new_page(None), Ok(1));
    assert!(f.ws.focus().unwrap().root().is_none());
    assert_eq!(f.ws.focus().unwrap().page_of(f.buffer), Some(0));
    f.ws.go_to_page(0).unwrap();
    f.ws.move_pane_to_page(f.stack, 0).unwrap();
    f.ws.activate(f.stack).unwrap();
    assert_eq!(f.ws.focused(), Some(f.stack));
    // With company, the focused pane moves to the new page and keeps focus.
    assert_eq!(f.ws.new_page(None), Ok(2));
    ok(&f.ws);
    let s = f.ws.focus().unwrap();
    assert_eq!(s.current, 2);
    assert_eq!(s.on_screen(), vec![f.stack]);
    assert_eq!(s.page_of(f.buffer), Some(0));
    assert_eq!(f.ws.focused(), Some(f.stack));
    // Undo takes the page away and puts the pane back.
    f.ws.undo().unwrap();
    ok(&f.ws);
    assert_eq!(f.ws.focus().unwrap().pages.len(), 2);
    assert_eq!(f.ws.focus().unwrap().page_of(f.stack), Some(0));
}

#[test]
fn moving_a_pane_to_the_next_page_is_one_undo_step() {
    let mut f = fixture();
    f.ws.maximize(f.buffer, 0).unwrap();
    f.ws.move_pane_to_page(f.stack, 0).unwrap();
    f.ws.activate(f.stack).unwrap();
    f.ws.clear_undo();
    let before = f.ws.clone();
    // On the last page: a page is added (starting with the focused pane, this one), and the
    // first page stays on screen. One step, not a refusal for a pane already moved.
    assert_eq!(f.ws.move_pane_to_next_page(f.stack), Ok(1));
    ok(&f.ws);
    let s = f.ws.focus().unwrap();
    assert_eq!((s.current, s.pages.len()), (0, 2));
    assert_eq!(
        (s.page_of(f.stack), s.page_of(f.buffer)),
        (Some(1), Some(0))
    );
    assert_eq!(f.ws.undo_stack().len(), 1);
    f.ws.undo().unwrap();
    assert_eq!(f.ws.focus(), before.focus());
    f.ws.redo().unwrap();
    // Already there: refused, and nothing changes.
    let before = f.ws.clone();
    assert_eq!(f.ws.move_pane_to_next_page(f.stack), Err(LayoutError::NoOp));
    assert_eq!(f.ws.focus(), before.focus());
}

#[test]
fn the_last_editor_is_forgotten_when_it_leaves_every_page() {
    // Closing the page holding it, as sending it to the tray does: Restore then keeps the
    // normal arrangement's own active tab.
    let mut f = fixture();
    let active_before = f.ws.group(f.ws.group_of(f.buffer).unwrap()).unwrap().active;
    f.ws.maximize(f.stack, 0).unwrap();
    f.ws.new_page(None).unwrap();
    f.ws.move_pane_to_page(f.line, 1).unwrap();
    f.ws.go_to_page(1).unwrap();
    f.ws.activate(f.line).unwrap();
    assert_eq!(f.ws.focus().unwrap().last_editor, Some(f.line));
    f.ws.close_page(1, 0).unwrap();
    ok(&f.ws);
    assert_eq!(f.ws.focus().unwrap().last_editor, None);
    f.ws.restore(1).unwrap();
    let g = f.ws.group_of(f.line).unwrap();
    assert_eq!(f.ws.group(g).unwrap().active, active_before);
}

#[test]
fn a_tour_page_resets_to_the_panes_its_origin_declares() {
    let mut f = fixture();
    f.ws.maximize(f.buffer, 0).unwrap();
    let origin = PageOrigin {
        tour_step: Some("step-2".into()),
        template: Some("code-and-stack".into()),
        panes: vec![
            PaneSource::Editor {
                path: "crates/cho-core/src/line.rs".into(),
            },
            PaneSource::CallStack {
                investigation: "buffer-flow".into(),
            },
            PaneSource::Graph {
                query: "Gone".into(),
            },
        ],
        ..Default::default()
    };
    let i = f.ws.new_page(Some(origin)).unwrap();
    // A tour page starts empty, even with a focused pane.
    assert!(f.ws.focus().unwrap().root().is_none());
    let report = f.ws.reset_page(i, |_| false).unwrap();
    ok(&f.ws);
    assert_eq!(f.ws.focus().unwrap().on_screen(), vec![f.line, f.stack]);
    assert_eq!(
        report.missing,
        vec![(
            1,
            PaneSource::Graph {
                query: "Gone".into()
            }
        )]
    );
    // Rearranging, then resetting, puts the declared panes back side by side; extras go to
    // the tray.
    f.ws.move_pane_to_page(f.outline, i).unwrap();
    f.ws.move_pane_to_page(f.stack, 0).unwrap();
    f.ws.reset_page(i, |_| false).unwrap();
    ok(&f.ws);
    assert_eq!(f.ws.focus().unwrap().on_screen(), vec![f.line, f.stack]);
    assert!(f.ws.tray().contains(&f.outline));
    assert_eq!(f.ws.focus().unwrap().page_of(f.buffer), Some(0));
    // A declared source with no pane is opened fresh when it can be shown.
    f.ws.close(f.stack).unwrap();
    let report = f.ws.reset_page(i, |_| true).unwrap();
    ok(&f.ws);
    assert_eq!(
        report.created.len(),
        2,
        "the call stack and the unknown graph"
    );
    assert_eq!(f.ws.focus().unwrap().on_screen().len(), 3);
    // A page with no origin cannot be reset; undo returns the page as it was.
    assert_eq!(f.ws.reset_page(0, |_| true), Err(LayoutError::NoPageOrigin));
    f.ws.undo().unwrap();
    ok(&f.ws);
    assert_eq!(f.ws.focus().unwrap().on_screen(), vec![f.line]);
}

#[test]
fn a_pin_records_a_stable_content_hash() {
    assert_eq!(content_hash(b""), "cbf29ce484222325");
    assert_eq!(content_hash(b"a"), "af63dc4c8601ec8c");
    assert_ne!(content_hash(b"fn a() {}"), content_hash(b"fn a() { }"));
}

#[test]
fn close_in_focus_sends_to_the_tray_and_closes_elsewhere() {
    let mut f = fixture();
    f.ws.maximize(f.buffer, 1000).unwrap();
    let g = f.ws.group_of(f.buffer).unwrap();
    f.ws.move_pane(
        f.context,
        Target::Beside {
            group: g,
            edge: Edge::Right,
        },
    )
    .unwrap();
    // On a page: to the tray, still in the layout.
    f.ws.dismiss(f.context).unwrap();
    ok(&f.ws);
    assert!(f.ws.tray().contains(&f.context));
    assert!(f.ws.content(f.context).is_some());
    // Undo puts it back on the page.
    f.ws.undo().unwrap();
    assert!(!f.ws.tray().contains(&f.context));
    // Already in the tray: closed for good.
    f.ws.dismiss(f.outline).unwrap();
    ok(&f.ws);
    assert!(f.ws.content(f.outline).is_none());
    // Outside focus: closed.
    f.ws.restore(2000).unwrap();
    f.ws.dismiss(f.tree).unwrap();
    ok(&f.ws);
    assert!(f.ws.content(f.tree).is_none());
}

/// The pane is the last child of a column split at the top of `root`, with a third of it.
fn spans_the_bottom(root: &Node, pane: PaneId) {
    let Node::Split(s) = root else {
        panic!("not a split: {root:?}");
    };
    assert_eq!(s.axis, Axis::Column);
    let last = s.children.last().unwrap();
    assert!(matches!(&last.node, Node::Group(g) if g.tabs == [pane]));
    assert!((last.weight - AREA_EDGE_SHARE).abs() < 1e-4);
}

#[test]
fn a_pane_can_span_the_whole_bottom_of_a_page_and_of_the_code_area() {
    let mut f = fixture();
    // A focus page with buffer.rs and Context side by side; Outline goes under both.
    f.ws.maximize(f.buffer, 1000).unwrap();
    let g = f.ws.group_of(f.buffer).unwrap();
    let beside = Target::Beside {
        group: g,
        edge: Edge::Right,
    };
    f.ws.move_pane(f.context, beside).unwrap();
    // Side by side, each group already runs the whole height: a drop beside the end one is
    // the same as spanning the left or right edge, so only top and bottom are worth offering.
    assert!(f.ws.area_edge_is_one_group(Edge::Left));
    assert!(f.ws.area_edge_is_one_group(Edge::Right));
    assert!(!f.ws.area_edge_is_one_group(Edge::Bottom));
    let bottom = Target::AreaEdge(Edge::Bottom);
    assert!(f.ws.valid_targets(f.outline).contains(&bottom));
    f.ws.move_pane(f.outline, bottom).unwrap();
    ok(&f.ws);
    spans_the_bottom(f.ws.focus().unwrap().root().unwrap(), f.outline);
    // Already alone along that edge: nothing to do, and not offered.
    assert_eq!(f.ws.move_pane(f.outline, bottom), Err(LayoutError::NoOp));
    assert!(!f.ws.valid_targets(f.outline).contains(&bottom));
    f.ws.undo().unwrap();
    ok(&f.ws);

    // The normal layout: line.rs under every code group.
    f.ws.restore(2000).unwrap();
    let g = f.ws.group_of(f.buffer).unwrap();
    f.ws.move_pane(
        f.line,
        Target::Beside {
            group: g,
            edge: Edge::Right,
        },
    )
    .unwrap();
    let opened = f.ws.open(editor("src/main.rs"), Some(bottom)).unwrap();
    ok(&f.ws);
    let root = f.ws.arrangement().root(Region::Editor).clone().unwrap();
    spans_the_bottom(&root, opened);
    assert_eq!(region_of(&f.ws, opened), Region::Editor);
}

/// The shape of a page: `R[...]` for a row, `C[...]` for a column, `g` for a group.
fn shape(n: &Node) -> String {
    match n {
        Node::Group(_) => "g".into(),
        Node::Split(s) => format!(
            "{}[{}]",
            if s.axis == Axis::Row { "R" } else { "C" },
            s.children
                .iter()
                .map(|c| shape(&c.node))
                .collect::<Vec<_>>()
                .join(" ")
        ),
    }
}

#[test]
fn a_tour_page_is_laid_out_by_its_named_arrangement_and_reset_restores_it() {
    let declared = [
        PaneSource::Editor {
            path: "crates/cho-core/src/line.rs".into(),
        },
        PaneSource::Context,
        PaneSource::CallStack {
            investigation: "buffer-flow".into(),
        },
        PaneSource::Outline,
    ];
    let cases = [
        (PageArrangement::Single, 1, "g"),
        (PageArrangement::SideBySide, 2, "R[g g]"),
        (PageArrangement::Stacked, 3, "C[g g g]"),
        (PageArrangement::MainSide, 3, "R[g C[g g]]"),
        (PageArrangement::Grid, 4, "C[R[g g] R[g g]]"),
        (PageArrangement::Grid, 3, "C[R[g g] g]"),
    ];
    for (arrangement, n, want) in cases {
        let mut f = fixture();
        f.ws.maximize(f.buffer, 0).unwrap();
        let i =
            f.ws.new_page(Some(PageOrigin {
                tour_step: Some("s".into()),
                panes: declared[..n].to_vec(),
                arrangement,
                ..Default::default()
            }))
            .unwrap();
        f.ws.reset_page(i, |_| false).unwrap();
        ok(&f.ws);
        let page = f.ws.focus().unwrap().root().unwrap().clone();
        assert_eq!(shape(&page), want, "{arrangement:?} with {n}");
        assert_eq!(f.ws.focus().unwrap().on_screen()[0], f.line);
        if arrangement == PageArrangement::MainSide
            && let Node::Split(s) = &page
        {
            assert!((s.children[0].weight - MAIN_SHARE).abs() < 1e-6);
        }
        // Rearranged by the reader, then reset: the same shape again, extras to the tray.
        f.ws.move_pane_to_page(f.graph, i).unwrap();
        if n > 1 {
            f.ws.move_pane_to_page(f.line, 0).unwrap();
        }
        f.ws.reset_page(i, |_| false).unwrap();
        ok(&f.ws);
        let again = f.ws.focus().unwrap().root().unwrap();
        assert_eq!(shape(again), want, "{arrangement:?} after reset");
        assert!(f.ws.tray().contains(&f.graph));
    }
}

#[test]
fn reset_takes_a_matching_pane_from_the_tray_before_one_on_another_page() {
    let mut f = fixture();
    f.ws.maximize(f.line, 0).unwrap();
    // A second editor for the same file waits in the tray.
    let fresh =
        f.ws.open(
            PaneContent::Editor {
                path: "crates/cho-core/src/line.rs".into(),
                caret: 0,
            },
            Some(Target::Tray),
        )
        .unwrap();
    let i =
        f.ws.new_page(Some(PageOrigin {
            tour_step: Some("s".into()),
            panes: vec![PaneSource::Editor {
                path: "crates/cho-core/src/line.rs".into(),
            }],
            ..Default::default()
        }))
        .unwrap();
    f.ws.reset_page(i, |_| false).unwrap();
    ok(&f.ws);
    assert_eq!(f.ws.focus().unwrap().on_screen(), vec![fresh]);
    assert_eq!(
        f.ws.focus().unwrap().page_of(f.line),
        Some(0),
        "page 1 kept whole"
    );
}

#[test]
fn page_arrangements_round_trip_by_name_and_old_origins_default_to_side_by_side() {
    for a in PageArrangement::ALL {
        assert_eq!(PageArrangement::parse(a.name()), Some(a));
    }
    let old: PageOrigin = serde_json::from_str(r#"{"tour_step":"s","panes":[]}"#).unwrap();
    assert_eq!(old.arrangement, PageArrangement::SideBySide);
}
