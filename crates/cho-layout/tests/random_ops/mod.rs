//! Random layout operations, shared by the property tests (`tests/property.rs`, seeded) and
//! the `layout_ops` fuzz target (`fuzz/`, driven by fuzz bytes).

#![allow(dead_code)]

use cho_layout::*;

/// Where the operations' choices come from.
pub trait Choices {
    /// A number below `n` (0 when `n` is 0).
    fn below(&mut self, n: usize) -> usize;
    /// A number in `0.0..1.0`.
    fn f32(&mut self) -> f32;
    fn pick<T: Copy>(&mut self, xs: &[T]) -> Option<T> {
        (!xs.is_empty()).then(|| xs[self.below(xs.len())])
    }
}

pub const FILES: [&str; 5] = [
    "crates/cho-core/src/buffer.rs",
    "crates/cho-core/src/line.rs",
    "app/src/view.rs",
    "README.md",
    "src/é/naïve.rs",
];

pub fn groups_on_screen(ws: &Workspace) -> Vec<NodeId> {
    match ws.focus() {
        Some(f) => f
            .root()
            .map(|r| r.groups().iter().map(|g| g.id).collect())
            .unwrap_or_default(),
        None => Region::ALL
            .iter()
            .filter_map(|r| ws.arrangement().root(*r).as_ref())
            .flat_map(|n| n.groups().into_iter().map(|g| g.id).collect::<Vec<_>>())
            .collect(),
    }
}

pub fn splits(node: &Node, out: &mut Vec<(NodeId, usize)>) {
    if let Node::Split(s) = node {
        out.push((s.id, s.children.len()));
        s.children.iter().for_each(|c| splits(&c.node, out));
    }
}

pub fn splits_on_screen(ws: &Workspace) -> Vec<(NodeId, usize)> {
    let mut out = Vec::new();
    match ws.focus() {
        Some(f) => {
            if let Some(r) = f.root() {
                splits(r, &mut out)
            }
        }
        None => Region::ALL
            .iter()
            .filter_map(|r| ws.arrangement().root(*r).as_ref())
            .for_each(|n| splits(n, &mut out)),
    }
    out
}

pub fn random_target(rng: &mut impl Choices, ws: &Workspace) -> Target {
    let groups = groups_on_screen(ws);
    match rng.below(6) {
        0 => Target::Dock(DockId::ALL[rng.below(3)]),
        1 => Target::EditorArea,
        2 => Target::Tray,
        3 | 4 => match rng.pick(&groups) {
            Some(group) => Target::Beside {
                group,
                edge: Edge::ALL[rng.below(4)],
            },
            None => Target::EditorArea,
        },
        _ => match rng.pick(&groups) {
            Some(group) => Target::Tabs {
                group,
                index: rng.below(5),
            },
            None => Target::Tray,
        },
    }
}

/// One random operation. `structural_only` leaves out operations that create or close panes,
/// for the restore-exactness property.
pub fn step(
    rng: &mut impl Choices,
    ws: &mut Workspace,
    now: u64,
    structural_only: bool,
) -> Result<()> {
    let panes: Vec<PaneId> = ws.panes().keys().copied().collect();
    let pane = rng.pick(&panes).unwrap_or(PaneId(999_999));
    // Structural operations rearrange panes without creating or closing any; with them, a
    // focus layout must restore exactly. 16.. are focus-page operations.
    const STRUCTURAL: [usize; 15] = [0, 1, 2, 3, 4, 5, 6, 7, 8, 16, 17, 18, 19, 20, 21];
    let op = if structural_only {
        STRUCTURAL[rng.below(STRUCTURAL.len())]
    } else {
        rng.below(31)
    };
    let pages = ws.focus().map_or(1, |f| f.pages.len());
    match op {
        0 | 1 => {
            let t = random_target(rng, ws);
            ws.move_pane(pane, t)
        }
        2 => {
            let groups = groups_on_screen(ws);
            let g = rng.pick(&groups).unwrap_or(NodeId(999_999));
            let t = random_target(rng, ws);
            ws.move_group(g, t)
        }
        3 => {
            let s = splits_on_screen(ws);
            match rng.pick(&s) {
                Some((id, n)) => ws
                    .resize_split(id, rng.below(n), rng.f32() - 0.5)
                    .map(|_| ()),
                None => Err(LayoutError::NoOp),
            }
        }
        4 => ws.toggle_minimize(pane).map(|_| ()),
        5 => ws.activate(pane),
        6 => match ws.content(pane) {
            Some(PaneContent::Editor { path, .. }) => {
                let path = path.clone();
                ws.set_editor_position(pane, &path, rng.below(10_000))
            }
            _ => Err(LayoutError::NotAnEditor),
        },
        7 => ws
            .resize_dock(DockId::ALL[rng.below(3)], rng.f32() * 800.0)
            .map(|_| ()),
        8 => ws.toggle_dock(DockId::ALL[rng.below(3)]).map(|_| ()),
        9 => {
            let target = random_target(rng, ws);
            let target = if rng.below(2) == 0 {
                None
            } else {
                Some(target)
            };
            ws.open(
                PaneContent::Editor {
                    path: FILES[rng.below(FILES.len())].into(),
                    caret: rng.below(500),
                },
                target,
            )
            .map(|_| ())
        }
        10 => ws
            .open(
                PaneContent::CallStack {
                    investigation: format!("inv-{}", rng.below(3)),
                    title: "Call stack".into(),
                },
                None,
            )
            .map(|_| ()),
        11 => ws
            .pin_context(
                Pin {
                    symbol: format!("sym{}", rng.below(4)),
                    symbol_kind: "function".into(),
                    path: FILES[0].into(),
                    byte_offset: rng.below(1000),
                    commit: "abc1234".into(),
                    content_hash: None,
                },
                if rng.below(2) == 0 {
                    PinPlace::Here
                } else {
                    PinPlace::BottomBar
                },
            )
            .map(|_| ()),
        12 => ws.close(pane),
        13 => ws.split(pane, Edge::ALL[rng.below(4)]).map(|_| ()),
        14 => {
            if ws.in_focus() {
                ws.restore(now).map(|_| ())
            } else {
                ws.maximize(pane, now)
            }
        }
        15 => {
            let n = ws.history().len().max(1);
            ws.reopen_focus(rng.below(n), now, |s| !matches!(s, PaneSource::Search))
                .map(|_| ())
        }
        16 => ws.new_page(None).map(|_| ()),
        17 => ws.go_to_page(rng.below(pages + 1)),
        18 => {
            if rng.below(2) == 0 {
                ws.next_page()
            } else {
                ws.prev_page()
            }
        }
        19 => ws.move_pane_to_page(pane, rng.below(pages + 1)),
        20 => ws.move_page(rng.below(pages), rng.below(pages)),
        21 => {
            // Closing the last page ends focus; structural runs keep at least one.
            if pages > 1 || !structural_only {
                ws.close_page(rng.below(pages), now)
            } else {
                ws.place_on_empty_page(pane)
            }
        }
        22 => ws.place_on_empty_page(pane),
        23 => {
            let rects = ws.rects(1200.0, 800.0);
            ws.move_pane_dir(pane, Direction::ALL[rng.below(4)], &rects)
                .map(|_| ())
        }
        24 => {
            let rects = ws.rects(1200.0, 800.0);
            let groups = groups_on_screen(ws);
            match rng.pick(&groups) {
                Some(g) => ws
                    .move_group_dir(g, Direction::ALL[rng.below(4)], &rects)
                    .map(|_| ()),
                None => Err(LayoutError::NoOp),
            }
        }
        25 => {
            let rects = ws.rects(1200.0, 800.0);
            ws.focus_dir(Direction::ALL[rng.below(4)], &rects)
                .map(|_| ())
        }
        26 => {
            let step = if rng.below(2) == 0 {
                ResizeStep::NORMAL
            } else {
                ResizeStep::FINE
            };
            ws.resize_pane(pane, Direction::ALL[rng.below(4)], step)
                .map(|_| ())
        }
        27 => ws.equalize_around(pane),
        28 => ws.undo(),
        29 => ws.redo(),
        _ => ws.toggle_all_docks().map(|_| ()),
    }
}
