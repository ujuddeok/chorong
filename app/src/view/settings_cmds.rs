//! The settings commands: open the settings window, and flip Lightweight mode. The window
//! itself is `crate::settings_window`; both commands act on this window's folder.

use super::*;

actions!(chorong, [OpenSettings, ToggleLightweightMode,]);

pub(super) const COMMANDS: &[Command] = &[
    c("chorong::OpenSettings", "Settings", Layer::View, 262),
    c(
        "chorong::ToggleLightweightMode",
        "Toggle Lightweight Mode",
        Layer::View,
        264,
    ),
];

pub(super) fn register(d: Div, cx: &mut Context<Chorong>) -> Div {
    let d = on!(d, cx, OpenSettings, |this, _w, cx| {
        let root = this.root().map(std::path::Path::to_path_buf);
        // Deferred: the window is being updated while its action runs.
        let theme = this.theme.clone();
        cx.defer(move |cx| crate::settings_window::open(root, theme, cx));
    });
    on!(d, cx, ToggleLightweightMode, |this, _w, cx| {
        let on = crate::settings_window::toggle_setting(
            this.root(),
            crate::settings_window::LIGHTWEIGHT,
        );
        this.notify_text(
            match on {
                Some(true) => "Lightweight mode on",
                Some(false) => "Lightweight mode off",
                None => "Could not save the setting",
            },
            cx,
        );
    })
}
