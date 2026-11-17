//! Tree-sitter languages and syntax highlighting.
//!
//! Highlights are byte ranges tagged with a capture. Each capture maps to a
//! Zed theme key ([`HighlightSpan::theme_key`]); theme lookup in `cho-theme`
//! falls back from `function.method` to `function`.

mod analysis;
mod highlight;
mod language;
mod outline;
mod refkinds;

pub use analysis::{Analysis, analyze};
pub use highlight::{
    CAPTURES, HighlightError, HighlightSpan, Highlighter, capture_names, highlight_shared,
};
pub use language::Language;
pub use outline::{Symbol, SymbolKind, SymbolMatch, find_symbol, outline};
pub use refkinds::{RefKind, RefToken, classify_references};
