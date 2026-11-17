//! `cho-layout-ui`: the GPUI chrome for `cho-layout`, shared by the viewer and the layout
//! lab.
//!
//! - [`shell`]: the [`shell::Shell`] view (focus bar, docks, editor area, focus pages, tray,
//!   menus, history list, frame-stats overlay) plus [`shell::bind_actions`], the status-bar
//!   pieces and the [`shell::PaneBodies`] trait through which the host supplies pane contents.
//!   The host owns the window frame: its root tracks focus, carries the key context, binds the
//!   actions and draws the status bar.
//! - [`actions`]: every named action (namespace `cho`), including those that carry data.
//! - [`keys`]: formatting of the keymap's bindings for menus and hints.
//! - [`widgets`]: small shared pieces (kind icons, verification dots and chips).
//! - [`colors`], [`fonts`]: theme-resolved colours and installed-font choice.
//! - [`field`], [`fuzzy`]: query editing and fuzzy ranking, shared with the viewer's pickers.
//! - [`assets`]: bundled Lucide icons.
//! - [`perf`]: frame timing and the off-thread layout saver.

pub mod actions;
pub mod assets;
pub mod colors;
pub mod field;
pub mod fonts;
pub mod fuzzy;
pub mod keys;
pub mod perf;
pub mod shell;
pub mod widgets;
