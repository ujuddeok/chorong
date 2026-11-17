//! Helpers shared by the integration tests.

use cho_semantic::index::{Occurrence, RawRange};

/// A single-line occurrence of `symbol` on `line`, columns `col..end`; a definition when `def`.
pub fn occ(line: u32, col: u32, end: u32, symbol: &str, def: bool) -> Occurrence {
    Occurrence {
        range: RawRange {
            start_line: line,
            start_col: col,
            end_line: line,
            end_col: end,
        },
        symbol: symbol.into(),
        roles: def as i32,
    }
}
