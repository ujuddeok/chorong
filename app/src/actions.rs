//! Every command the viewer has, as a named GPUI action, and the table the command palette
//! lists. Action names are stable: users' keymaps refer to them (`docs/public/keybindings.md`).

/// A command listed in the palette: the action's registered name and what the palette calls it.
/// Overlay navigation (picker and dialog keys) is deliberately not listed.
#[derive(Clone, Copy)]
pub struct Command {
    pub action: &'static str,
    pub title: &'static str,
    pub layer: Layer,
    /// Where the command sits in the palette, cheat sheet and key table: sorted by `rank`
    /// across every module, so ranks are unique (a test checks). Leave gaps to insert between.
    pub rank: u16,
}

/// Which part of the keyboard scheme a command belongs to (`docs/public/keybindings.md`).
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Layer {
    /// Cmd + key: go somewhere.
    Go,
    /// Cmd-K, then a key: change the view.
    View,
    /// Single keys in the code view: the reading loop.
    Read,
    /// Comparing with a branch: the commands that switch and link the two sides.
    Compare,
    /// Stepping through an open tour or call stack.
    Steps,
    /// A comparison's rows and variants, and a tour's branches.
    Comparison,
}

impl Layer {
    pub fn title(self) -> &'static str {
        match self {
            Layer::Go => "Go somewhere",
            Layer::View => "Change the view (key chords)",
            Layer::Read => "Read (single keys in the code view)",
            Layer::Compare => "Compare",
            Layer::Steps => "Tour and call stack",
            Layer::Comparison => "Comparison, variants and branches",
        }
    }
}

pub const fn c(action: &'static str, title: &'static str, layer: Layer, rank: u16) -> Command {
    Command {
        action,
        title,
        layer,
        rank,
    }
}

/// Registers `$action`'s handler on a div: run `$body`, then repaint.
macro_rules! on {
    ($d:expr, $cx:expr, $action:ty, |$this:ident, $window:ident, $cx2:ident| $body:expr) => {
        $d.on_action($cx.listener(move |$this, _: &$action, $window, $cx2| {
            let _ = &$window;
            $body;
            $cx2.notify();
        }))
    };
}

/// Registers the placeholder handlers for bound commands whose feature does not exist yet.
macro_rules! unavailable {
    ($d:expr, $cx:expr, $( $action:ty => $what:expr ),* $(,)?) => {{
        let d = $d;
        $( let d = $crate::actions::on!(d, $cx, $action, |this, _w, cx| this.unavailable($what, cx)); )*
        d
    }};
}

pub(crate) use {on, unavailable};
