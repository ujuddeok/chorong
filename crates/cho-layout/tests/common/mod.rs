//! Helpers shared by the example tests (`ops.rs`, `keyboard.rs`).

#![allow(dead_code)]

use cho_layout::*;

/// Panics with the workspace when an invariant is broken.
pub fn ok(ws: &Workspace) {
    if let Err(e) = ws.check_invariants() {
        panic!("invariant broken: {e}\n{ws:#?}");
    }
}

pub fn editor(path: &str) -> PaneContent {
    PaneContent::Editor {
        path: path.into(),
        caret: 0,
    }
}

pub fn call_stack(id: &str) -> PaneContent {
    PaneContent::CallStack {
        investigation: id.into(),
        title: format!("Call stack {id}"),
    }
}

pub fn pin(symbol: &str) -> Pin {
    Pin {
        symbol: symbol.into(),
        symbol_kind: "function".into(),
        path: "src/lib.rs".into(),
        byte_offset: 42,
        commit: "04e63f8".into(),
        content_hash: None,
    }
}

/// The first pane of `kind`.
pub fn find(ws: &Workspace, kind: PaneKind) -> PaneId {
    *ws.panes().iter().find(|(_, c)| c.kind() == kind).unwrap().0
}

/// The region of the normal arrangement holding `pane`.
pub fn region_of(ws: &Workspace, pane: PaneId) -> Region {
    ws.arrangement().locate(pane).unwrap().0
}
