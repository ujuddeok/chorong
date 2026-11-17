#![no_main]
//! Saved layout files: they live in Chorong's cache but can be stale, from another version, or
//! hand-edited. Any text loads (never panics) to a workspace that satisfies the invariants,
//! and what it saves loads back without warnings to the same layout.
use libfuzzer_sys::fuzz_target;
use cho_layout::Workspace;

fuzz_target!(|s: &str| {
    let loaded = Workspace::from_json(s);
    let ws = loaded.workspace;
    if let Err(e) = ws.check_invariants() {
        panic!("loaded layout breaks invariants: {e}\nwarnings: {:?}", loaded.warnings);
    }
    let json = ws.to_json();
    let again = Workspace::from_json(&json);
    assert!(again.warnings.is_empty(), "own save warns: {:?}", again.warnings);
    again.workspace.check_invariants().expect("invariants after reload");
    assert!(again.workspace.arrangement() == ws.arrangement(), "arrangement changed");
    assert!(again.workspace.panes() == ws.panes(), "panes changed");
    assert_eq!(again.workspace.to_json(), json, "save is not stable");
});
