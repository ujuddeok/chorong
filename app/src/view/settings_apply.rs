//! Settings that change while the viewer runs: the settings files are polled about once a
//! second, and a changed feature starts or stops its work at once (no restart).

use super::*;
use cho_settings::ChangeSet;

const POLL: Duration = Duration::from_secs(1);

impl Chorong {
    /// Polls the settings files for edits made elsewhere (`cho settings`, an editor).
    pub(super) fn watch_settings(&mut self, cx: &mut Context<Self>) {
        // A debugging aid for measuring the features: the cost report, once a second.
        let dump = std::env::var_os("CHORONG_METRICS_DUMP").map(std::path::PathBuf::from);
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(POLL).await;
                let alive = this.update(cx, |this, cx| {
                    if let Some(changes) = this.features.reload() {
                        this.settings_changed(&changes, cx);
                    }
                    if let Some(path) = &dump {
                        std::fs::write(path, this.features.report()).ok();
                    }
                });
                if alive.is_err() {
                    break;
                }
            }
        })
        .detach();
    }

    /// Installs the fonts the settings choose (falling back to the bundled IBM Plex) as the
    /// global [`cho_fonts::Fonts`] that panes, the status bar and the settings page read.
    pub(super) fn apply_fonts(features: &Features, cx: &mut App) -> cho_fonts::Fonts {
        let s = features.settings();
        cho_fonts::install(
            cx,
            Some(s.appearance_ui_font.as_str()),
            Some(s.appearance_code_font.as_str()),
        )
    }

    /// Starts or stops the work of every feature whose effective value changed.
    pub(super) fn settings_changed(&mut self, changes: &ChangeSet, cx: &mut Context<Self>) {
        let changed = |f: Feature| changes.contains(f.key());
        if changed(Feature::FollowHover) && !self.features.on(Feature::FollowHover) {
            // Back to the caret, and no dwell timer left to fire.
            self.hover.left();
        }
        if changed(Feature::FollowHover)
            || changed(Feature::FollowCaret)
            || changed(Feature::References)
            || changes.contains("context.hover_delay_ms")
        {
            self.requery_context(cx);
        }
        if changes.contains("appearance.ui_font") || changes.contains("appearance.code_font") {
            let fonts = Self::apply_fonts(&self.features, cx);
            // Pane headers and tabs too (the shell keeps its own copy).
            self.shell
                .update(cx, |shell, cx| shell.set_fonts(fonts.clone(), cx));
            if self.cli_font.is_none() {
                self.font = fonts.mono;
            }
        }
        if changed(Feature::Blame) {
            // Off hides the column; on (with Toggle Blame on) blames the open files.
            self.dirty_all = true;
            self.ensure_blame(cx);
        }
        if changed(Feature::CallKinds) {
            // Off clears the kinds; on classifies the open file.
            self.refresh_call_kinds(cx);
        }
        if changed(Feature::AutoRefresh) {
            self.maybe_keep_fresh(cx);
        }
        if changed(Feature::FrameOverlay)
            || changed(Feature::LayoutAutosave)
            || changed(Feature::FrameLog)
        {
            let (overlay, autosave) = (
                self.features.on(Feature::FrameOverlay),
                self.features.on(Feature::LayoutAutosave),
            );
            let log = Self::frame_log_path(&self.features);
            self.shell.update(cx, |shell, cx| {
                shell.apply_options(overlay, autosave, log, cx)
            });
        }
        cx.notify();
    }
}
