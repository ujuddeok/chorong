#![no_main]
//! Layout operations chosen by the fuzz bytes (the same operations as cho-layout's
//! property tests): after every step the invariants hold and a failed operation changes
//! nothing; replaying the bytes gives the same workspace; the result saves and loads back.
use libfuzzer_sys::fuzz_target;
use cho_layout::Workspace;

#[path = "../../crates/cho-layout/tests/random_ops/mod.rs"]
mod random_ops;
use random_ops::{Choices, step};

/// Choices read from the input, 0 once it runs out.
struct Bytes<'a>(&'a [u8]);

impl Bytes<'_> {
    fn byte(&mut self) -> u8 {
        let Some((&b, rest)) = self.0.split_first() else { return 0 };
        self.0 = rest;
        b
    }
}

impl Choices for Bytes<'_> {
    fn below(&mut self, n: usize) -> usize {
        if n == 0 {
            return 0;
        }
        let v = u16::from_le_bytes([self.byte(), self.byte()]);
        v as usize % n
    }
    fn f32(&mut self) -> f32 {
        f32::from(self.byte()) / 256.0
    }
}

fn run(data: &[u8]) -> Workspace {
    let mut bytes = Bytes(data);
    let mut ws = Workspace::with_defaults();
    let mut i = 0u64;
    while !bytes.0.is_empty() && i < 150 {
        let before = ws.clone();
        let result = step(&mut bytes, &mut ws, i * 7, false);
        if let Err(e) = ws.check_invariants() {
            panic!("step {i}: invariant broken after {result:?}: {e}");
        }
        if result.is_err() {
            assert!(ws == before, "step {i}: failed op {result:?} changed state");
        }
        i += 1;
    }
    ws
}

fuzz_target!(|data: &[u8]| {
    let mut ws = run(data);
    assert!(run(data) == ws, "replay differs");
    if ws.in_focus() {
        ws.restore(0).expect("restore ends focus");
        ws.check_invariants().expect("invariants after restore");
    }
    let json = ws.to_json();
    let loaded = Workspace::from_json(&json);
    assert!(loaded.warnings.is_empty(), "{:?}", loaded.warnings);
    assert!(loaded.workspace.arrangement() == ws.arrangement(), "arrangement lost in save");
    assert!(loaded.workspace.panes() == ws.panes(), "panes lost in save");
    loaded.workspace.check_invariants().expect("loaded layout invariants");
});
