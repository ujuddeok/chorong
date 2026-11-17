//! Property-style tests: long random sequences of operations from fixed seeds. After every
//! step the invariants hold, a failed operation changes nothing, the same seed gives the same
//! workspace, save/load round-trips, and restoring a focus layout returns the exact prior
//! arrangement apart from the editor's active tab.

mod random_ops;

use cho_layout::*;
use random_ops::*;

/// xorshift64*: tiny, deterministic, no dependency.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }
}

impl Choices for Rng {
    fn below(&mut self, n: usize) -> usize {
        if n == 0 {
            0
        } else {
            (self.next() % n as u64) as usize
        }
    }
    fn f32(&mut self) -> f32 {
        (self.next() % 10_000) as f32 / 10_000.0
    }
}

fn run(seed: u64, steps: usize) -> Workspace {
    let mut rng = Rng(seed | 1);
    let mut ws = Workspace::with_defaults();
    let mut succeeded = 0;
    for i in 0..steps {
        let before = ws.clone();
        let result = step(&mut rng, &mut ws, i as u64 * 7, false);
        succeeded += usize::from(result.is_ok());
        if let Err(e) = ws.check_invariants() {
            panic!("seed {seed} step {i}: invariant broken after {result:?}: {e}\n{ws:#?}");
        }
        if result.is_err() {
            assert_eq!(
                ws, before,
                "seed {seed} step {i}: failed op {result:?} changed state"
            );
            // Workspace equality leaves undo out; a failed op must not record either.
            let undo = |w: &Workspace| (w.undo_stack().len(), w.undo_stack().can_redo());
            assert_eq!(
                undo(&ws),
                undo(&before),
                "seed {seed} step {i}: failed op {result:?} changed undo"
            );
        }
    }
    assert!(
        succeeded * 3 > steps,
        "seed {seed}: only {succeeded}/{steps} operations applied"
    );
    ws
}

#[test]
fn invariants_hold_and_failures_change_nothing() {
    for seed in 1u64..=60 {
        run(seed.wrapping_mul(0x9e37_79b9), 400);
    }
}

#[test]
fn same_seed_same_workspace() {
    for seed in [3, 77, 1234] {
        assert_eq!(run(seed, 300), run(seed, 300));
        assert_eq!(run(seed, 300).to_json(), run(seed, 300).to_json());
    }
}

#[test]
fn save_and_load_round_trip() {
    for seed in 1u64..=40 {
        let mut ws = run(seed * 31, 250);
        let json = ws.to_json();
        let loaded = Workspace::from_json(&json);
        assert!(
            loaded.warnings.is_empty(),
            "seed {seed}: {:?}",
            loaded.warnings
        );
        // A focus layout is temporary and not saved: compare with it ended without recording.
        if ws.in_focus() {
            let history = ws.history().to_vec();
            ws.restore(0).unwrap();
            assert_eq!(loaded.workspace.history(), &history[..]);
            assert_eq!(
                loaded.workspace.arrangement(),
                ws.arrangement(),
                "seed {seed}"
            );
            continue;
        }
        assert_eq!(
            loaded.workspace.arrangement(),
            ws.arrangement(),
            "seed {seed}"
        );
        assert_eq!(loaded.workspace.panes(), ws.panes());
        assert_eq!(
            loaded.workspace.to_json(),
            Workspace::from_json(&loaded.workspace.to_json())
                .workspace
                .to_json()
        );
    }
}

#[test]
fn restore_returns_the_exact_prior_layout_apart_from_the_editor_position() {
    for seed in 1u64..=80 {
        let mut rng = Rng(seed.wrapping_mul(0x517c_c1b7_2722_0a95) | 1);
        let mut ws = Workspace::with_defaults();
        for i in 0..60 {
            let _ = step(&mut rng, &mut ws, i, false);
        }
        if ws.in_focus() {
            ws.restore(0).unwrap();
        }
        let panes: Vec<PaneId> = ws.panes().keys().copied().collect();
        let Some(p) = rng.pick(&panes) else { continue };
        let before = ws.arrangement().clone();
        ws.maximize(p, 100).unwrap();
        for i in 0..80 {
            let _ = step(&mut rng, &mut ws, 100 + i, true);
            ws.check_invariants().unwrap();
            assert_eq!(
                ws.arrangement(),
                &before,
                "seed {seed}: focus changed the arrangement"
            );
        }
        let last_editor = ws.focus().unwrap().last_editor;
        ws.restore(500).unwrap();
        ws.check_invariants().unwrap();
        let mut expected = before.clone();
        if let Some(e) = last_editor {
            let (_, g) = expected.locate(e).unwrap();
            let group = expected.editor.as_mut().unwrap().group_mut(g).unwrap();
            group.active = group.tabs.iter().position(|t| *t == e).unwrap();
        }
        assert_eq!(ws.arrangement(), &expected, "seed {seed}");
    }
}

#[test]
fn validate_agrees_with_move_pane() {
    for seed in 1u64..=30 {
        let mut rng = Rng(seed.wrapping_mul(0x2545_f491) | 1);
        let mut ws = Workspace::with_defaults();
        for i in 0..200 {
            let _ = step(&mut rng, &mut ws, i, false);
            let panes: Vec<PaneId> = ws.panes().keys().copied().collect();
            let Some(p) = rng.pick(&panes) else { continue };
            let t = random_target(&mut rng, &ws);
            let fast = ws.valid_targets(p);
            let mut slow: Vec<Target> = DockId::ALL.iter().map(|d| Target::Dock(*d)).collect();
            slow.push(Target::EditorArea);
            for g in groups_on_screen(&ws) {
                for edge in Edge::ALL {
                    slow.push(Target::Beside { group: g, edge });
                }
                slow.push(Target::Tabs {
                    group: g,
                    index: usize::MAX,
                });
            }
            slow.extend(Edge::ALL.into_iter().map(Target::AreaEdge));
            slow.push(Target::Tray);
            slow.retain(|t| ws.validate(p, *t).is_ok());
            let visible = |t: &Target| match t {
                Target::Beside { group, .. } | Target::Tabs { group, .. } => {
                    ws.focus().is_some()
                        || ws
                            .arrangement()
                            .region_of_node(*group)
                            .is_some_and(|r| match r {
                                Region::Dock(d) => ws.arrangement().dock(d).visible,
                                Region::Editor => true,
                            })
                }
                _ => true,
            };
            slow.retain(visible);
            assert_eq!(fast, slow, "seed {seed} step {i}: valid_targets({p:?})");
            let said = ws.validate(p, t).is_ok();
            let did = ws.clone().move_pane(p, t).is_ok();
            assert_eq!(said, did, "seed {seed} step {i}: {p:?} {t:?}");
            // Along a whole edge of the page or code area: every edge, every step.
            for t in Edge::ALL.map(Target::AreaEdge) {
                let said = ws.validate(p, t).is_ok();
                let mut moved = ws.clone();
                let did = moved.move_pane(p, t).is_ok();
                assert_eq!(said, did, "seed {seed} step {i}: {p:?} {t:?}");
                moved.check_invariants().unwrap();
            }
        }
    }
}

#[test]
fn undoing_everything_returns_the_starting_layout() {
    for seed in 1u64..=40 {
        let mut rng = Rng(seed.wrapping_mul(0x9e37_79b9_7f4a_7c15) | 1);
        let mut ws = Workspace::with_defaults();
        for i in 0..30 {
            let _ = step(&mut rng, &mut ws, i, false);
        }
        if ws.in_focus() {
            ws.restore(0).unwrap();
        }
        ws.clear_undo();
        let start = (ws.arrangement().clone(), ws.focus().cloned());
        let panes_before: Vec<PaneId> = ws.panes().keys().copied().collect();
        for i in 0..60 {
            // Structural moves, closes and keyboard operations; nothing opens panes.
            let _ = match rng.below(3) {
                0 => step(&mut rng, &mut ws, 100 + i, true),
                1 => {
                    let rects = ws.rects(1200.0, 800.0);
                    let panes: Vec<PaneId> = ws.panes().keys().copied().collect();
                    match rng.pick(&panes) {
                        Some(p) => ws
                            .move_pane_dir(p, Direction::ALL[rng.below(4)], &rects)
                            .map(|_| ()),
                        None => Ok(()),
                    }
                }
                _ => {
                    let panes: Vec<PaneId> = ws.panes().keys().copied().collect();
                    match rng.pick(&panes) {
                        Some(p) if rng.below(4) == 0 => ws.close(p),
                        Some(p) => ws
                            .resize_pane(p, Direction::ALL[rng.below(4)], ResizeStep::NORMAL)
                            .map(|_| ()),
                        None => Ok(()),
                    }
                }
            };
            ws.check_invariants().unwrap();
        }
        while ws.undo().is_ok() {
            ws.check_invariants().unwrap();
        }
        assert_eq!(
            ws.panes().keys().copied().collect::<Vec<_>>(),
            panes_before,
            "seed {seed}"
        );
        assert_eq!(
            (ws.arrangement().clone(), ws.focus().cloned()),
            start,
            "seed {seed}"
        );
    }
}
