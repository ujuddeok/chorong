//! Chorong's settings backend: no GUI dependency.
//!
//! - [`defs`]: every setting, with its key, default, description, cost hint and cost class
//!   (the data the settings page renders), and the typed [`Settings`] struct.
//! - [`resolve`]: `defaults < global < repository override < Lightweight mode`, with the
//!   source of every effective value.
//! - [`jsonc`]: JSON with comments, and edits that touch only the changed key (from
//!   `cho-core`, which other crates read JSONC with).
//! - [`store`]: load/save `settings.json` and `repos.json`, change notification, reload.
//!
//! See `docs/public/settings.md`.

pub mod defs;
mod dirs;
pub mod resolve;
pub mod store;

pub use cho_core::jsonc;
pub use defs::{CostClass, Kind, RETIRED, SettingDef, Settings, Value, def, defs, retired};
pub use dirs::{config_dir, resolve_config_dir};
pub use resolve::{Effective, Entry, Layer, Source, resolve};
pub use store::{
    ChangeSet, Diagnostic, DiagnosticKind, FileKind, FileStatus, SaveError, Scope, SettingsStore,
    repo_key,
};

/// Keys of every setting, for registering with `cho-metrics`.
pub fn keys() -> impl Iterator<Item = &'static str> {
    defs().iter().map(|d| d.key)
}
