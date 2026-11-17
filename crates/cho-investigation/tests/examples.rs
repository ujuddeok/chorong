//! Every example in `docs/public/examples/` must stay valid.
#![allow(clippy::disallowed_methods)] // tests read their own fixtures

use cho_investigation::Investigation;
use std::fs;
use std::path::Path;

#[test]
fn documented_examples_are_valid() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/public/examples");
    let mut count = 0;
    for entry in fs::read_dir(&dir).expect("docs/public/examples exists") {
        let path = entry.unwrap().path();
        if path.extension().is_some_and(|e| e == "json") {
            let json = fs::read_to_string(&path).unwrap();
            if let Err(e) = Investigation::parse(&json) {
                panic!("{}: {e}", path.display());
            }
            count += 1;
        }
    }
    assert!(count >= 5, "expected example files in {}", dir.display());
}
