//! The layout shell's named actions, in the viewer and the lab. The first nine are layout.md's
//! stable names; the rest are what the pane menu (…) offers, so that every mouse action has a
//! named, remappable action too.

use gpui::actions;

actions!(
    chorong,
    [
        // design/layout.md, "Keys".
        ToggleMaximizePane,
        RestoreLayout,
        MinimizePane,
        PinContextHere,
        PinContextToBottomBar,
        ReopenFocusLayout,
        ToggleLeftDock,
        ToggleRightDock,
        ToggleBottomBar,
        // The pane menu.
        MovePaneToLeftDock,
        MovePaneToRightDock,
        MovePaneToBottomBar,
        SplitPaneRight,
        SplitPaneDown,
        SendPaneToTray,
        ClosePane,
        MovePaneToEditorArea,
        // Focus pages. Keys are provisional (keymap/lab.json).
        NextFocusPage,
        PrevFocusPage,
        NewFocusPage,
        CloseFocusPage,
        // Asks the host for a file picker that adds files to the current focus page: Tab in
        // Bring from tray…, or New Focus Page with an empty tray. The shell does not handle it.
        AddFileToPage,
        MovePaneToNextFocusPage,
        // Keyboard layout (design/layout-keyboard.md).
        EnterLayoutMode,
        ExitLayoutMode,
        EnterResizeMode,
        ExitResizeMode,
        FocusPaneLeft,
        FocusPaneRight,
        FocusPaneUp,
        FocusPaneDown,
        MovePaneLeft,
        MovePaneRight,
        MovePaneUp,
        MovePaneDown,
        MoveGroupLeft,
        MoveGroupRight,
        MoveGroupUp,
        MoveGroupDown,
        // In a focus layout: Tab / Shift-Tab flip the focused spot through the tray's files.
        NextTrayFile,
        PrevTrayFile,
        SplitPaneLeft,
        SplitPaneUp,
        EqualizePanes,
        UndoLayout,
        RedoLayout,
        ToggleAllDocks,
        // Pickers and the switcher (design/layout-keyboard.md, "Pickers", "By recency").
        FocusPane,
        MovePaneTo,
        BringFromTray,
        OpenSwitcher,
        OpenSwitcherReverse,
        // Dock sizes: Ctrl-Alt-=, Ctrl-Alt--, Ctrl-Alt-0 (Zed's keys).
        GrowDock,
        ShrinkDock,
        ResetDockSize,
        // Inside an open picker or the switcher (key contexts `PanePicker` and `PaneSwitcher`).
        PaneListPrev,
        PaneListNext,
        PaneListConfirm,
        // The second way to confirm: Bring from tray… places the pane below, not to the right.
        PaneListConfirmAlt,
        // Tab: in Bring from tray…, switches to the host's file picker (`AddFileToPage`).
        PaneListTab,
        PaneListClose,
        // Lab instrumentation.
        ToggleFrameStats,
    ]
);

/// Focuses editor group N, counted from 1 (Cmd-1…9; `1`–`9` in Layout mode).
#[derive(Clone, PartialEq, Debug, serde::Deserialize, schemars::JsonSchema, gpui::Action)]
#[action(namespace = chorong)]
pub struct FocusEditorGroup(pub usize);

/// Focuses the pane whose jump label is this letter (Layout mode).
#[derive(Clone, PartialEq, Debug, serde::Deserialize, schemars::JsonSchema, gpui::Action)]
#[action(namespace = chorong)]
pub struct FocusLabel(pub String);

#[derive(Clone, Copy, PartialEq, Eq, Debug, serde::Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Dir {
    Left,
    Right,
    Up,
    Down,
}

impl From<Dir> for cho_layout::Direction {
    fn from(d: Dir) -> Self {
        match d {
            Dir::Left => cho_layout::Direction::Left,
            Dir::Right => cho_layout::Direction::Right,
            Dir::Up => cho_layout::Direction::Up,
            Dir::Down => cho_layout::Direction::Down,
        }
    }
}

/// One resize step that way (the resize sub-mode): `["chorong::ResizePane", {"direction":
/// "left", "fine": true}]`.
#[derive(Clone, PartialEq, Debug, serde::Deserialize, schemars::JsonSchema, gpui::Action)]
#[action(namespace = chorong)]
pub struct ResizePane {
    pub direction: Dir,
    #[serde(default)]
    pub fine: bool,
}

/// Shows focus page N, counted from 1 as the focus bar shows them. In a keymap:
/// `["chorong::GoToFocusPage", 2]`, the same shape as the planned `chorong::FocusEditorGroup`.
#[derive(Clone, PartialEq, Debug, serde::Deserialize, schemars::JsonSchema, gpui::Action)]
#[action(namespace = chorong)]
pub struct GoToFocusPage(pub usize);
