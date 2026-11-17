//! `cho-layout`: the viewer's window layout as data, with no GPUI dependency.
//!
//! A [`Workspace`] holds every pane ([`PaneContent`] by [`PaneId`]), the normal
//! [`Arrangement`] (left dock, right dock, full-width bottom bar, and an editor area that is a
//! split tree of tab groups), an optional temporary focus layout, and focus history. The
//! viewer renders it and calls its operations; it never rearranges panes itself.
//!
//! Everything is deterministic: ids come from a counter, collections are ordered, and the
//! model never reads a clock (callers pass `now`). Positions inside panes (an editor's caret, a
//! pin's symbol) are UTF-8 byte offsets. See `design/layout.md`.

mod arrangement;
mod focus;
mod history;
mod invariants;
mod keyboard;
mod pane;
mod persist;
mod switcher;
mod tree;
mod undo;
mod workspace;

pub use arrangement::{Arrangement, Dock, DockId, Region};
pub use focus::{FocusPage, FocusSession, MAIN_SHARE, PageArrangement, PageOrigin};
pub use history::{
    FocusRecord, HISTORY_LIMIT, HistoryEntry, RecordChild, RecordNode, RecordPage, ago,
};
pub use keyboard::{Direction, LABEL_LETTERS, Label, Rect, ResizeStep, neighbour};
pub use pane::{DefaultPlace, PaneContent, PaneId, PaneKind, PaneSource, Pin, content_hash};
pub use persist::{FORMAT_VERSION, Loaded, layout_file_name, load, save};
pub use switcher::{DOCK_STEP, Destination, PaneEntry, Place, Switcher};
pub use tree::{Axis, Child, Edge, Group, MIN_WEIGHT, Node, NodeId, Split, normalize};
pub use undo::{UNDO_LIMIT, UndoStack};
pub use workspace::{
    AREA_EDGE_SHARE, LayoutError, PinPlace, ReopenReport, Result, Target, Workspace,
};
