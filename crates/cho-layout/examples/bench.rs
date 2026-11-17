//! Micro-benchmarks for layout operations at a realistic size (12 panes) and a stress size
//! (200 panes, 50 tabs in the largest strip, a full focus history).
//!
//! `cargo run --release -p cho-layout --example bench`
//!
//! With `-- --check` it also fails (exit 1) when a keyboard operation's median at the stress
//! size is over the budget of design/layout-keyboard.md (0.1 ms).
//!
//! Each operation runs on a fresh clone of the same workspace many times; the median and the
//! worst run are printed in microseconds. The clone is not timed. No dependencies, so it runs
//! anywhere the crate builds.

use cho_layout::*;
use std::hint::black_box;
use std::time::Instant;

fn editor(i: usize) -> PaneContent {
    PaneContent::Editor {
        path: format!("crates/c{}/src/file{i}.rs", i % 7),
        caret: i * 13,
    }
}

fn pin(i: usize) -> Pin {
    Pin {
        symbol: format!("Type{i}::method"),
        symbol_kind: "method".into(),
        path: "src/lib.rs".into(),
        byte_offset: i,
        commit: "04e63f8".into(),
        content_hash: None,
    }
}

fn find(ws: &Workspace, kind: PaneKind) -> PaneId {
    ws.panes()
        .iter()
        .find(|(_, c)| c.kind() == kind)
        .map(|(id, _)| *id)
        .unwrap()
}

/// About what the viewer shows on a normal day: 12 panes.
fn realistic() -> Workspace {
    let mut ws = Workspace::with_defaults();
    let tree = find(&ws, PaneKind::FileTree);
    let g = ws.group_of(tree).unwrap();
    ws.open(
        PaneContent::Tour {
            investigation: "t".into(),
            title: "Tour".into(),
        },
        Some(Target::Beside {
            group: g,
            edge: Edge::Top,
        }),
    )
    .unwrap();
    let first = ws.open(editor(0), None).unwrap();
    for i in 1..4 {
        ws.open(editor(i), None).unwrap();
    }
    ws.split(first, Edge::Right).unwrap();
    ws.open(
        PaneContent::CallStack {
            investigation: "c".into(),
            title: "Call stack".into(),
        },
        None,
    )
    .unwrap();
    ws.open(
        PaneContent::Graph {
            query: "q".into(),
            title: "Graph".into(),
        },
        None,
    )
    .unwrap();
    ws.pin_context(pin(0), PinPlace::Here).unwrap();
    ws.activate(first).unwrap();
    with_history(&mut ws, 5);
    ws
}

/// Stress: 200 panes. Three editor groups of 50 tabs, 50 tool panes and pins spread over the
/// docks, and a full focus history.
fn stress() -> Workspace {
    let mut ws = Workspace::with_defaults();
    let first = ws.open(editor(0), None).unwrap();
    for i in 1..50 {
        ws.open(editor(i), None).unwrap();
    }
    let second = ws.split(first, Edge::Right).unwrap();
    ws.activate(second).unwrap();
    for i in 50..99 {
        ws.open(editor(i), None).unwrap();
    }
    let third = ws.split(second, Edge::Bottom).unwrap();
    ws.activate(third).unwrap();
    for i in 99..147 {
        ws.open(editor(i), None).unwrap();
    }
    let mut n = ws.panes().len();
    let mut i = 0;
    while n < 200 {
        match i % 4 {
            0 => ws.pin_context(pin(i), PinPlace::BottomBar).map(|_| ()),
            1 => ws.pin_context(pin(i), PinPlace::Here).map(|_| ()),
            2 => ws
                .open(
                    PaneContent::CallStack {
                        investigation: format!("c{i}"),
                        title: "Call stack".into(),
                    },
                    None,
                )
                .map(|_| ()),
            _ => ws
                .open(
                    PaneContent::Placeholder {
                        name: format!("p{i}"),
                    },
                    Some(Target::Dock(DockId::Left)),
                )
                .map(|_| ()),
        }
        .unwrap();
        i += 1;
        n = ws.panes().len();
    }
    ws.activate(first).unwrap();
    with_history(&mut ws, HISTORY_LIMIT);
    ws.check_invariants().unwrap();
    ws
}

/// Records `n` distinct focus layouts.
fn with_history(ws: &mut Workspace, n: usize) {
    let panes: Vec<PaneId> = ws.panes().keys().copied().collect();
    for k in 0..n {
        let a = panes[k % panes.len()];
        let b = panes[(k * 7 + 3) % panes.len()];
        ws.maximize(a, k as u64).unwrap();
        if a != b {
            let g = ws.group_of(a).unwrap();
            ws.move_pane(
                b,
                Target::Beside {
                    group: g,
                    edge: Edge::Right,
                },
            )
            .unwrap();
        }
        ws.restore(k as u64 + 1).unwrap();
    }
}

/// Times `op` on fresh clones of `ws`; returns (median, worst) in microseconds.
fn time(ws: &Workspace, runs: usize, mut op: impl FnMut(&mut Workspace)) -> (f64, f64) {
    let mut samples = Vec::with_capacity(runs);
    for _ in 0..runs {
        let mut w = ws.clone();
        let t = Instant::now();
        op(&mut w);
        samples.push(t.elapsed().as_secs_f64() * 1e6);
        black_box(&w);
    }
    samples.sort_by(f64::total_cmp);
    (samples[runs / 2], samples[runs - 1])
}

/// Prints one table row; returns the median.
fn row(name: &str, ws: &Workspace, op: impl FnMut(&mut Workspace)) -> f64 {
    let (med, worst) = time(ws, 2000, op);
    println!("| {name:<44} | {med:>9.1} | {worst:>9.1} |");
    med
}

/// A row under the keyboard budget: its median goes into `out`.
fn budgeted(
    out: &mut Vec<(&'static str, f64)>,
    name: &'static str,
    ws: &Workspace,
    op: impl FnMut(&mut Workspace),
) {
    out.push((name, row(name, ws, op)));
}

/// Runs every row on `ws`; returns the keyboard operations' medians.
fn suite(label: &str, ws: &Workspace) -> Vec<(&'static str, f64)> {
    let mut keyboard = Vec::new();
    let editors: Vec<PaneId> = ws
        .panes()
        .iter()
        .filter(|(_, c)| c.kind() == PaneKind::Editor)
        .map(|(id, _)| *id)
        .collect();
    let a = editors[1];
    let other_group = ws.group_of(*editors.last().unwrap()).unwrap();
    let outline = find(ws, PaneKind::Outline);
    let tree_group = ws.group_of(find(ws, PaneKind::FileTree)).unwrap();
    let stack = find(ws, PaneKind::CallStack);
    let json = ws.to_json();
    let mut focused = ws.clone();
    focused.maximize(stack, 0).unwrap();
    let fg = focused.group_of(stack).unwrap();
    let mut paged = focused.clone();
    paged.new_page(None).unwrap();
    paged.move_pane_to_page(a, 1).unwrap();

    println!(
        "\n{label}: {} panes, {} bytes saved, {} history entries\n",
        ws.panes().len(),
        json.len(),
        ws.history().len()
    );
    println!(
        "| {:<44} | {:>9} | {:>9} |",
        "operation", "median µs", "worst µs"
    );
    println!("| {:-<44} | {:->9} | {:->9} |", "", "", "");
    row("clone (the cost of atomicity per operation)", ws, |w| {
        black_box(w.clone());
    });
    row("move a tab onto another strip", ws, |w| {
        w.move_pane(
            a,
            Target::Tabs {
                group: other_group,
                index: 3,
            },
        )
        .unwrap()
    });
    row("move a pane across docks (right → left)", ws, |w| {
        w.move_pane(
            outline,
            Target::Beside {
                group: tree_group,
                edge: Edge::Right,
            },
        )
        .unwrap()
    });
    row("drop-zone probe (clone + move, per drag move)", ws, |w| {
        let mut probe = w.clone();
        black_box(
            probe
                .move_pane(outline, Target::Dock(DockId::Bottom))
                .is_ok(),
        );
    });
    row("split an editor", ws, |w| {
        w.split(a, Edge::Right).unwrap();
    });
    row("resize a dock", ws, |w| {
        w.resize_dock(DockId::Left, 300.0).unwrap();
    });
    row("toggle a dock", ws, |w| {
        w.toggle_dock(DockId::Right).unwrap();
    });
    row("minimise", ws, |w| {
        w.toggle_minimize(outline).unwrap();
    });
    row("maximise", ws, |w| w.maximize(stack, 0).unwrap());
    row("maximise + dock two in + restore", ws, |w| {
        w.maximize(stack, 0).unwrap();
        let g = w.group_of(stack).unwrap();
        w.move_pane(
            a,
            Target::Beside {
                group: g,
                edge: Edge::Top,
            },
        )
        .unwrap();
        w.move_pane(
            outline,
            Target::Beside {
                group: g,
                edge: Edge::Right,
            },
        )
        .unwrap();
        w.restore(1).unwrap();
    });
    row("restore (from a maximised pane)", &focused, |w| {
        w.restore(1).unwrap();
    });
    row("move a tray pane into focus", &focused, |w| {
        w.move_pane(
            a,
            Target::Beside {
                group: fg,
                edge: Edge::Top,
            },
        )
        .unwrap()
    });
    row("new focus page + move a pane to it", &focused, |w| {
        w.new_page(None).unwrap();
        w.move_pane_to_page(a, 1).unwrap();
    });
    row("switch focus page", &paged, |w| {
        w.go_to_page(0).unwrap();
    });
    let rects = ws.rects(1200.0, 800.0);
    let groups: Vec<NodeId> = rects.iter().map(|(g, _)| *g).collect();
    row("neighbour (every group, 4 directions)", ws, |w| {
        for g in &groups {
            for d in Direction::ALL {
                black_box(w.neighbour(&rects, *g, d));
            }
        }
    });
    budgeted(&mut keyboard, "focus by direction", ws, |w| {
        black_box(w.focus_dir(Direction::Right, &rects).ok());
    });
    budgeted(
        &mut keyboard,
        "move a tab by direction (Shift-arrow)",
        ws,
        |w| {
            black_box(w.move_pane_dir(a, Direction::Right, &rects).ok());
        },
    );
    budgeted(
        &mut keyboard,
        "move a pane by direction across docks",
        ws,
        |w| {
            black_box(w.move_pane_dir(outline, Direction::Left, &rects).ok());
        },
    );
    budgeted(&mut keyboard, "valid targets for a pane", ws, |w| {
        black_box(w.valid_targets(outline));
    });
    budgeted(&mut keyboard, "keyboard resize one step", ws, |w| {
        black_box(w.resize_pane(a, Direction::Right, ResizeStep::NORMAL).ok());
    });
    budgeted(&mut keyboard, "labels and editor-group numbers", ws, |w| {
        black_box(w.labels(&LABEL_LETTERS));
    });
    budgeted(&mut keyboard, "undo + redo one move", ws, |w| {
        w.move_pane(outline, Target::Dock(DockId::Left)).unwrap();
        w.undo().unwrap();
        w.redo().unwrap();
    });
    let mut full_undo = ws.clone();
    for _ in 0..UNDO_LIMIT {
        full_undo.toggle_minimize(outline).unwrap();
    }
    row("a move with a full 100-step undo stack", &full_undo, |w| {
        w.move_pane(outline, Target::Dock(DockId::Left)).unwrap();
    });
    row("focus history list (20 entries max)", ws, |w| {
        black_box(w.focus_history(100, |_| true));
    });
    row("reopen focus layout", ws, |w| {
        w.reopen_focus(0, 100, |_| true).unwrap();
    });
    row("save (to_json)", ws, |w| {
        black_box(w.to_json());
    });
    row("load (from_json, with repair checks)", ws, |_| {
        black_box(Workspace::from_json(&json));
    });
    row("check_invariants (tests only)", ws, |w| {
        w.check_invariants().unwrap();
    });
    keyboard
}

/// design/layout-keyboard.md, "Performance": every keyboard operation takes under 0.1 ms at
/// 200 panes (the median, in a release build).
const KEYBOARD_BUDGET_US: f64 = 100.0;

fn main() {
    let check = std::env::args().any(|a| a == "--check");
    if cfg!(debug_assertions) {
        eprintln!("note: debug build; run with --release for real numbers");
        if check {
            eprintln!("--check needs a release build");
            std::process::exit(2);
        }
    }
    suite("Realistic", &realistic());
    let keyboard = suite("Stress", &stress());
    if check {
        let over: Vec<_> = keyboard
            .iter()
            .filter(|(_, us)| *us >= KEYBOARD_BUDGET_US)
            .collect();
        for (name, us) in &over {
            eprintln!("over budget: {name} took {us:.1} µs (budget {KEYBOARD_BUDGET_US} µs)");
        }
        if !over.is_empty() {
            std::process::exit(1);
        }
        println!("\nkeyboard budget: every operation under {KEYBOARD_BUDGET_US} µs");
    }
}
