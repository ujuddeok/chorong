//! The settings page: its own preferences-style window, opened by `chorong::OpenSettings`.
//!
//! One window per app (opening it again brings it forward); it closes with Esc or Cmd-W and
//! never takes over the code window. Rows come from `cho_settings::defs()`; every change is
//! written at once through the one `SettingsStore`, which edits `settings.json` in place.
//! `model.rs` holds the rows and the key handling without any GUI; `view.rs` draws them.
//!
//! The commands that open it are in `view/settings_cmds.rs`.

mod model;
mod remote_model;
mod view;

pub use model::LIGHTWEIGHT;

use crate::features::SharedMetrics;
use cho_metrics::Metrics;
use cho_settings::SettingsStore;
use cho_theme::Theme;
use gpui::{App, AppContext, Bounds, TitlebarOptions, WindowBounds, WindowOptions, px, size};
use model::{Costs, Measured};
use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
pub use view::SettingsWindow;

/// What the window reads and writes: a `SettingsStore` of its own (every viewer window polls
/// the same files and applies a saved change within a second) and the viewers' cost counters.
#[derive(Clone)]
pub struct Shared {
    pub store: Rc<RefCell<SettingsStore>>,
    pub metrics: Live,
}

/// The viewers' metrics, if a viewer window has been opened (it publishes them).
#[derive(Clone)]
pub struct Live(Option<Arc<Metrics>>);

impl Costs for Live {
    fn measured(&self, key: &str) -> Option<Measured> {
        self.0.as_ref()?.measured(key)
    }

    fn top(&self, n: usize) -> Vec<&'static str> {
        self.0.as_ref().map_or_else(Vec::new, |m| m.top(n))
    }
}

/// A store pointed at `repo`, for "this repository only".
fn open_store(repo: Option<&Path>) -> SettingsStore {
    let mut store = SettingsStore::open_default();
    store.set_active_repo(repo).ok();
    store
}

/// Flips the on/off setting `key` (Lightweight mode is `lightweight`) for the folder `repo`
/// (where a repository override is in force, that one) and saves it; every viewer applies it
/// within a second. The new state, or `None` if it could not be saved or `key` is not a
/// toggle.
pub fn toggle_setting(repo: Option<&Path>, key: &str) -> Option<bool> {
    let mut store = open_store(repo);
    let changed = model::flip(&mut store, key)?.ok()?;
    (!changed.is_empty()).then_some(())?;
    match store.effective().entry(key)?.value {
        cho_settings::Value::Bool(b) => Some(b),
        _ => None,
    }
}

/// Opens the settings window, or brings the open one forward. `root` is the folder of the code
/// window it was asked from, which "this repository only" refers to.
pub fn open(root: Option<PathBuf>, theme: Theme, cx: &mut App) {
    if let Some(existing) = cx
        .windows()
        .into_iter()
        .find_map(|w| w.downcast::<SettingsWindow>())
    {
        existing
            .update(cx, |this, window, cx| {
                window.activate_window();
                this.repo_changed(root.as_deref(), cx);
            })
            .ok();
        return;
    }
    let shared = Shared {
        store: Rc::new(RefCell::new(open_store(root.as_deref()))),
        metrics: Live(cx.try_global::<SharedMetrics>().map(|m| m.0.clone())),
    };
    let bounds = Bounds::centered(None, size(px(800.), px(740.)), cx);
    let opened = cx.open_window(
        WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            titlebar: Some(TitlebarOptions {
                title: Some("Settings \u{2014} Chorong".into()),
                ..Default::default()
            }),
            window_min_size: Some(size(px(560.), px(420.))),
            ..Default::default()
        },
        move |window, cx| {
            let view = cx.new(|cx| SettingsWindow::new(theme, shared, cx));
            let focus = view.read(cx).focus_handle();
            window.focus(&focus);
            view
        },
    );
    if let Err(e) = opened {
        eprintln!("chorong-app: cannot open the settings window: {e}");
    }
}
