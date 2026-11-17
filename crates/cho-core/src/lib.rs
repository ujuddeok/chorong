//! Core types shared by every Chorong crate.
//!
//! The one rule this crate enforces: **positions are UTF-8 byte offsets**.
//! [`SourceBuffer`] is the only place that converts them to lines, columns or
//! UTF-16 code units, which UI toolkits and some indexers use.

pub mod buffer;
pub mod build_env;
pub mod editor;
pub mod fs;
pub mod git;
pub mod jsonc;
pub mod link;
pub mod npm_version;
#[cfg(any(test, feature = "test-support"))]
pub mod test_repo;
pub mod words;

pub use buffer::{ColumnUnit, LineCol, SourceBuffer, SourceError, is_binary};
pub use editor::{Editor, EditorLocation, Launch, web_url};
pub use link::{ChorongLink, LinkError, LinkSide, RepoRef};
