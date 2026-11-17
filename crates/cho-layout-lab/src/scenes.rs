//! Starting arrangements, built only through `cho-layout` operations, so the screenshots show
//! states the model really reaches.

use anyhow::Result;
use cho_layout::{Edge, PaneContent, PaneId, PaneKind, Pin, PinPlace, Target, Workspace};

pub const PIN_SYMBOLS: [(&str, &str); 4] = [
    ("SourceBuffer::utf16_to_byte", "method"),
    ("LineCol", "struct"),
    ("SourceBuffer", "struct"),
    ("Outline::entries", "method"),
];

pub fn pin(n: usize) -> Pin {
    let (symbol, kind) = PIN_SYMBOLS[n % PIN_SYMBOLS.len()];
    Pin {
        symbol: symbol.into(),
        symbol_kind: kind.into(),
        path: "crates/cho-core/src/buffer.rs".into(),
        byte_offset: 4_212 + n * 97,
        commit: "04e63f8".into(),
        content_hash: None,
    }
}

fn find(ws: &Workspace, kind: PaneKind) -> PaneId {
    ws.panes()
        .iter()
        .find(|(_, c)| c.kind() == kind)
        .map(|(id, _)| *id)
        .expect("scene pane exists")
}

/// The default arrangement of layout.md, with a tour, two editor groups and two bottom tabs.
fn base() -> Result<Workspace> {
    let mut ws = Workspace::with_defaults();
    let tree = find(&ws, PaneKind::FileTree);
    let tree_group = ws.group_of(tree).expect("tree placed");
    ws.open(
        PaneContent::Tour {
            investigation: "reading-loop".into(),
            title: "Tour: the reading loop".into(),
        },
        Some(Target::Beside {
            group: tree_group,
            edge: Edge::Top,
        }),
    )?;
    let buffer = ws.open(
        PaneContent::Editor {
            path: "crates/cho-core/src/buffer.rs".into(),
            caret: 4_212,
        },
        None,
    )?;
    ws.open(
        PaneContent::Editor {
            path: "crates/cho-core/src/line.rs".into(),
            caret: 0,
        },
        None,
    )?;
    ws.activate(buffer)?;
    let right = ws.split(buffer, Edge::Right)?;
    ws.set_editor_position(right, "app/src/view.rs", 0)?;
    ws.open(
        PaneContent::CallStack {
            investigation: "utf16-flow".into(),
            title: "Call stack".into(),
        },
        None,
    )?;
    ws.open(
        PaneContent::Graph {
            query: "LineCol".into(),
            title: "Graph".into(),
        },
        None,
    )?;
    ws.activate(buffer)?;
    Ok(ws)
}

/// 200 panes: the default scene plus 50-tab editor strips, many pins and tool panes. For
/// frame-time checks at a stress size, not a realistic layout.
fn stress(ws: &mut Workspace) -> Result<()> {
    let editors: Vec<PaneId> = ws
        .panes()
        .iter()
        .filter(|(_, c)| c.kind() == PaneKind::Editor)
        .map(|(id, _)| *id)
        .collect();
    for (n, e) in editors.iter().enumerate() {
        ws.activate(*e)?;
        for i in 0..48 {
            ws.open(
                PaneContent::Editor {
                    path: format!("crates/stress/src/m{n}/file{i}.rs"),
                    caret: 0,
                },
                None,
            )?;
        }
    }
    let mut i = 0;
    while ws.panes().len() < 200 {
        match i % 3 {
            0 => ws.pin_context(pin(i), PinPlace::BottomBar).map(|_| ())?,
            1 => ws.pin_context(pin(i), PinPlace::Here).map(|_| ())?,
            _ => ws
                .open(
                    PaneContent::Placeholder {
                        name: format!("Tool {i}"),
                    },
                    Some(Target::Dock(cho_layout::DockId::Left)),
                )
                .map(|_| ())?,
        }
        i += 1;
    }
    ws.activate(editors[0])?;
    Ok(())
}

/// A starting arrangement (`--scene`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum)]
pub enum Scene {
    Default,
    Pinned,
    Minimised,
    Focus,
    Tray,
    Stress,
    SplitOutline,
    FocusPages,
}

pub fn build(scene: Scene) -> Result<Workspace> {
    let mut ws = base()?;
    match scene {
        Scene::Default => {}
        Scene::Pinned => {
            ws.pin_context(pin(0), PinPlace::Here)?;
            ws.pin_context(pin(1), PinPlace::BottomBar)?;
        }
        Scene::Minimised => {
            ws.toggle_minimize(find(&ws, PaneKind::Outline))?;
            ws.toggle_minimize(find(&ws, PaneKind::Tour))?;
        }
        Scene::Focus => {
            let stack = find(&ws, PaneKind::CallStack);
            ws.maximize(stack, 0)?;
            let g = ws.group_of(stack).expect("on screen");
            let buffer = ws.focused().expect("buffer focused before");
            let buffer = if ws.kind(buffer) == Some(PaneKind::Editor) {
                buffer
            } else {
                find(&ws, PaneKind::Editor)
            };
            ws.move_pane(
                buffer,
                Target::Beside {
                    group: g,
                    edge: Edge::Top,
                },
            )?;
            let bg = ws.group_of(buffer).expect("on screen");
            ws.move_pane(
                find(&ws, PaneKind::Context),
                Target::Beside {
                    group: bg,
                    edge: Edge::Right,
                },
            )?;
            ws.activate(buffer)?;
        }
        Scene::Tray => {
            let buffer = find(&ws, PaneKind::Editor);
            ws.maximize(buffer, 0)?;
        }
        Scene::Stress => stress(&mut ws)?,
        Scene::SplitOutline => {
            // The owner's example: one editor group with its tabs, and an Outline column
            // beside it in the centre.
            let view = ws
                .panes()
                .iter()
                .find(|(_, c)| matches!(c, PaneContent::Editor { path, .. } if path == "app/src/view.rs"))
                .map(|(id, _)| *id)
                .expect("view.rs is open");
            ws.close(view)?;
            ws.move_pane(find(&ws, PaneKind::Outline), Target::EditorArea)?;
        }
        Scene::FocusPages => {
            let buffer = find(&ws, PaneKind::Editor);
            ws.maximize(buffer, 0)?;
            ws.new_page(None)?;
            ws.place_on_empty_page(find(&ws, PaneKind::CallStack))?;
            let g = ws
                .group_of(find(&ws, PaneKind::CallStack))
                .expect("on page");
            ws.move_pane(
                find(&ws, PaneKind::Context),
                Target::Beside {
                    group: g,
                    edge: Edge::Right,
                },
            )?;
            ws.new_page(None)?;
            ws.place_on_empty_page(find(&ws, PaneKind::Graph))?;
            ws.go_to_page(1)?;
        }
    }
    ws.check_invariants().map_err(anyhow::Error::msg)?;
    Ok(ws)
}

#[cfg(test)]
mod tests {
    use clap::ValueEnum;

    #[test]
    fn every_scene_builds() {
        for s in super::Scene::value_variants() {
            super::build(*s).unwrap();
        }
    }
}
