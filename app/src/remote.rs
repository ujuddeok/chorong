//! "Open Remote…": open a branch, pull request or commit of a remote repository in a cache
//! worktree. `adapter` is the only code that touches `cho-sources` (so a richer backend with
//! progress, cancellation and typed errors swaps in there); `form` is the form's state and its
//! hint text, with no UI and no I/O.

pub mod adapter;
pub mod failure;
pub mod form;
pub mod header;
pub mod prepare;
