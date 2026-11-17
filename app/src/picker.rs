//! Fuzzy ranking for the file picker: the shell's ranker ([`cho_layout_ui::fuzzy`]), set up
//! for paths.

pub use cho_layout_ui::fuzzy::{Match, Ranker};

pub const MAX_RESULTS: usize = 50;
