//! The viewer's optional features: which are on, and what they cost.
//!
//! One [`Features`] owns the `SettingsStore` and the `Metrics`. A call site asks
//! [`Features::on`] (so turning a feature off stops the work, not just the output) and starts
//! a timer for the same [`Feature`] while it works. Both are keyed by the setting key
//! (`docs/public/settings.md`). Lightweight mode needs nothing here: `store.effective()` already
//! resolves every toggle marked for it to off.
//!
//! [`NOT_GATED`] lists the settings that are not gated here and why; a test keeps the two
//! lists and the settings table in step, so a new setting cannot be added without deciding.
//! `Feature::enabled` reads the typed `Settings` fields (no lookup on hot paths); a test pins
//! each to its own key in the table.

use cho_metrics::{FeatureId, Kind, Metrics, Timer};
use cho_settings::{ChangeSet, Settings, SettingsStore};
use std::path::Path;
use std::sync::Arc;

/// A feature with a setting and a cost timer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Feature {
    Blame,
    ChangesAutoRefresh,
    ChangedWords,
    FollowHighlight,
    LineHighlight,
    FollowHover,
    FollowCaret,
    References,
    HighlightExact,
    CallKinds,
    AutoRefresh,
    SearchMaxResults,
    FoldBodies,
    JsonFolds,
    Graph,
    FrameOverlay,
    FrameLog,
    LayoutAutosave,
    Minimap,
    Overview,
    Overscroll,
    InvestigationsWatch,
    PickerPreview,
}

impl Feature {
    pub const ALL: [Feature; 23] = [
        Feature::Blame,
        Feature::ChangesAutoRefresh,
        Feature::ChangedWords,
        Feature::FollowHighlight,
        Feature::LineHighlight,
        Feature::FollowHover,
        Feature::FollowCaret,
        Feature::References,
        Feature::HighlightExact,
        Feature::CallKinds,
        Feature::AutoRefresh,
        Feature::SearchMaxResults,
        Feature::FoldBodies,
        Feature::JsonFolds,
        Feature::Graph,
        Feature::FrameOverlay,
        Feature::FrameLog,
        Feature::LayoutAutosave,
        Feature::Minimap,
        Feature::Overview,
        Feature::Overscroll,
        Feature::InvestigationsWatch,
        Feature::PickerPreview,
    ];

    /// The setting key.
    pub fn key(self) -> &'static str {
        match self {
            Feature::Blame => "blame.enabled",
            Feature::ChangesAutoRefresh => "changes.auto_refresh",
            Feature::ChangedWords => "compare.changed_words",
            Feature::FollowHighlight => "compare.follow_highlight",
            Feature::LineHighlight => "compare.line_highlight",
            Feature::FollowHover => "context.follow_hover",
            Feature::FollowCaret => "context.follow_caret",
            Feature::References => "context.references",
            Feature::HighlightExact => "highlight.exact",
            Feature::CallKinds => "lens.call_kinds",
            Feature::AutoRefresh => "index.auto_refresh",
            Feature::SearchMaxResults => "search.max_results",
            Feature::FoldBodies => "editor.fold_bodies",
            Feature::JsonFolds => "editor.json_folds",
            Feature::Graph => "graph.enabled",
            Feature::FrameOverlay => "debug.frame_overlay",
            Feature::FrameLog => "debug.frame_log",
            Feature::LayoutAutosave => "layout.autosave",
            Feature::Minimap => "editor.minimap",
            Feature::Overview => "editor.overview",
            Feature::Overscroll => "scroll.overscroll",
            Feature::InvestigationsWatch => "investigations.watch",
            Feature::PickerPreview => "picker.preview",
        }
    }

    /// Whether the time counts against the frame it ran in (UI thread) or not (elsewhere).
    fn kind(self) -> Kind {
        match self {
            Feature::FollowHover
            | Feature::ChangedWords
            | Feature::FollowHighlight
            | Feature::LineHighlight
            | Feature::FollowCaret
            | Feature::FoldBodies
            | Feature::FrameOverlay
            | Feature::FrameLog
            | Feature::LayoutAutosave
            | Feature::Minimap
            | Feature::Overview
            | Feature::Overscroll
            | Feature::InvestigationsWatch => Kind::Foreground,
            _ => Kind::Background,
        }
    }

    fn enabled(self, s: &Settings) -> bool {
        match self {
            Feature::Blame => s.blame_enabled,
            Feature::ChangesAutoRefresh => s.changes_auto_refresh,
            Feature::ChangedWords => s.compare_changed_words,
            Feature::FollowHighlight => s.compare_follow_highlight != "off",
            Feature::LineHighlight => s.compare_line_highlight != "off",
            Feature::FollowHover => s.context_follow_hover,
            Feature::FollowCaret => s.context_follow_caret,
            Feature::References => s.context_references,
            Feature::HighlightExact => s.highlight_exact,
            Feature::CallKinds => s.lens_call_kinds,
            Feature::AutoRefresh => s.index_auto_refresh,
            // A number, not a toggle: always on.
            Feature::SearchMaxResults => true,
            Feature::FoldBodies => s.editor_fold_bodies,
            Feature::JsonFolds => s.editor_json_folds,
            Feature::Graph => s.graph_enabled,
            Feature::FrameOverlay => s.debug_frame_overlay,
            Feature::FrameLog => s.debug_frame_log,
            Feature::LayoutAutosave => s.layout_autosave,
            Feature::Minimap => s.editor_minimap,
            Feature::Overview => s.editor_overview,
            Feature::Overscroll => s.scroll_overscroll,
            Feature::InvestigationsWatch => s.investigations_watch,
            Feature::PickerPreview => s.picker_preview,
        }
    }
}

/// Settings that are deliberately not gated at a call site, with the reason.
#[cfg(test)]
const NOT_GATED: &[(&str, &str)] = &[
    (
        "appearance.code_font",
        "a font name applied when the theme and fonts are set up",
    ),
    (
        "appearance.ui_font",
        "a font name applied when the theme and fonts are set up",
    ),
    ("lightweight", "resolved by the settings store itself"),
    (
        "editor.minimap_always",
        "read where editor.minimap is gated; it only widens where the minimap shows",
    ),
    (
        "context.hover_delay_ms",
        "a number read where the dwell timer starts",
    ),
    (
        "highlight.gutter_marks",
        "a display preference read where the gutter is drawn; it costs nothing to compute",
    ),
    ("hover.card", "the hover card is not built yet"),
    ("hover.peek", "the definition peek is not built yet"),
    (
        "index.banner",
        "a display preference read where the index banner is drawn; it does no work of its own",
    ),
    (
        "index.low_priority",
        "a choice read when an index build starts (viewer and `cho index`); it does no work of its own",
    ),
    (
        "index.node_memory_mb",
        "a number read when the TypeScript indexer starts",
    ),
    (
        "index.parallel_projects",
        "builds run one project at a time; more is not built yet",
    ),
    (
        "index.python_install",
        "a path read by cho_semantic::tools when a Python build starts",
    ),
    (
        "index.python_version",
        "a version read by cho_semantic::tools when a Python build starts",
    ),
    (
        "index.typescript_install",
        "a path read by cho_semantic::tools when a TypeScript build starts",
    ),
    (
        "index.typescript_version",
        "a version read by cho_semantic::tools when a TypeScript build starts",
    ),
    (
        "index.version_check",
        "read by cho_semantic::tools when a build starts; the check is part of the build the reader started",
    ),
    (
        "index.version_check_days",
        "a number read by cho_semantic::tools when a build starts",
    ),
    (
        "tools.node",
        "a path read when an indexer or prepare step needs node",
    ),
    ("tools.npm", "a path read when Chorong installs an indexer"),
    ("tools.uv", "a path read when a prepare step runs uv"),
];

/// The cost counters of every window, published as a GPUI global by [`Features::open_in`], for
/// the settings window to read (`cx.global::<SharedMetrics>().0.all()`, `.frames()`,
/// `.top_by_window_total(3)`). To read or change settings, open a `SettingsStore` of its own
/// (`SettingsStore::open_default()`, `set_active_repo`, `set`/`unset`, `reload_if_changed`):
/// the viewer sees a saved change within a second and applies it, so there is nothing to
/// call on the viewer.
pub struct SharedMetrics(pub Arc<Metrics>);

impl gpui::Global for SharedMetrics {}

/// The settings in force and the cost counters, shared by every call site.
pub struct Features {
    store: SettingsStore,
    metrics: Arc<Metrics>,
    ids: Vec<(Feature, FeatureId)>,
}

/// A timer source that can move into a background task.
#[derive(Clone)]
pub struct Probe {
    metrics: Arc<Metrics>,
    id: FeatureId,
}

impl Probe {
    /// Records one call when dropped.
    #[must_use = "the call is recorded when the timer is dropped"]
    pub fn start(&self) -> Timer<'_> {
        self.metrics.start(self.id)
    }

    /// Records one background job: wall time, and the thread's CPU time where reported.
    #[must_use = "the job is recorded when the span is dropped"]
    pub fn background(&self) -> cho_metrics::BackgroundSpan<'_> {
        self.metrics.background_span(self.id)
    }
}

impl Features {
    /// Reads `settings.json` from Chorong's config directory, sharing one `Metrics` between windows through [`SharedMetrics`].
    pub fn open_in(cx: &mut gpui::App) -> Self {
        let shared = cx.try_global::<SharedMetrics>().map(|m| m.0.clone());
        let feats = Self::build(SettingsStore::open_default(), shared);
        cx.set_global(SharedMetrics(feats.metrics.clone()));
        feats
    }

    #[cfg(test)]
    pub fn with_store(store: SettingsStore) -> Self {
        Self::build(store, None)
    }

    fn build(store: SettingsStore, shared: Option<Arc<Metrics>>) -> Self {
        // Every setting key is registered, so the settings page can show them all; the ones
        // without a call site simply stay at zero.
        let mut registered: Vec<(&'static str, Kind)> = Vec::new();
        for key in cho_settings::keys() {
            let kind = Feature::ALL
                .iter()
                .find(|f| f.key() == key)
                .map_or(Kind::Foreground, |f| f.kind());
            registered.push((key, kind));
        }
        let metrics = shared.unwrap_or_else(|| Arc::new(Metrics::new(&registered)));
        let ids = Feature::ALL
            .iter()
            .filter_map(|f| Some((*f, metrics.id(f.key())?)))
            .collect();
        Self {
            store,
            metrics,
            ids,
        }
    }

    /// Settings for the repository at `root` apply from now on.
    pub fn set_repo(&mut self, root: Option<&Path>) -> Option<ChangeSet> {
        self.store.set_active_repo(root).ok()
    }

    /// Picks up edits to the settings files (call about once a second).
    pub fn reload(&mut self) -> Option<ChangeSet> {
        self.store.reload_if_changed().filter(|c| !c.is_empty())
    }

    pub fn settings(&self) -> &Settings {
        self.store.effective().settings()
    }

    pub fn store(&self) -> &SettingsStore {
        &self.store
    }

    pub fn metrics(&self) -> &Arc<Metrics> {
        &self.metrics
    }

    /// Whether `f` is on right now (its own setting, and Lightweight mode).
    pub fn on(&self, f: Feature) -> bool {
        f.enabled(self.settings())
    }

    fn id(&self, f: Feature) -> FeatureId {
        self.ids
            .iter()
            .find(|(g, _)| *g == f)
            .map(|(_, id)| *id)
            .expect("every Feature is registered")
    }

    /// Times one call of `f` until the guard drops.
    #[must_use = "the call is recorded when the timer is dropped"]
    pub fn time(&self, f: Feature) -> Timer<'_> {
        self.metrics.start(self.id(f))
    }

    /// One line per feature that has run (`key calls total_us avg_us worst_us bg_cpu_us`),
    /// then the frame budget line: what `CHORONG_METRICS_DUMP=FILE` writes once a second.
    pub fn report(&self) -> String {
        let us = |d: std::time::Duration| d.as_micros();
        let mut out = String::from("key calls total_us avg_us worst_us bg_cpu_us\n");
        for s in self.metrics.all().iter().filter(|s| s.count > 0) {
            let cpu = s.background.map_or(0, |b| us(b.cpu_total));
            out.push_str(&format!(
                "{} {} {} {} {} {cpu}\n",
                s.key,
                s.count,
                us(s.total),
                us(s.average),
                us(s.worst)
            ));
        }
        let f = self.metrics.frames();
        out.push_str(&format!(
            "frames {} over_8.3ms {} over_16.7ms {} worst_us {}\n",
            f.frames,
            f.over_8_3,
            f.over_16_7,
            us(f.worst)
        ));
        out
    }

    /// A timer source for `f` while it is on, for work that checks it in a callback.
    pub fn probe_if_on(&self, f: Feature) -> Option<Probe> {
        self.on(f).then(|| self.probe(f))
    }

    /// A timer source for `f` that can move into a background task.
    pub fn probe(&self, f: Feature) -> Probe {
        Probe {
            metrics: self.metrics.clone(),
            id: self.id(f),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cho_settings::{Scope, Value};

    /// Features over a store in a fresh directory of its own (`in_memory` cannot be written
    /// to); the directory lives as long as the guard.
    fn features() -> (tempfile::TempDir, Features) {
        let dir = tempfile::tempdir().unwrap();
        let store = SettingsStore::open(Some(dir.path().to_path_buf()));
        (dir, Features::with_store(store))
    }

    fn lightweight() -> (tempfile::TempDir, Features) {
        let (dir, mut feats) = features();
        feats
            .store
            .set(Scope::Global, "lightweight", Value::Bool(true))
            .unwrap();
        (dir, feats)
    }

    /// What the settings table says about `f`: its toggle's value in force, or on for a
    /// number.
    fn in_table(feats: &Features, f: Feature) -> bool {
        match &feats.store.effective().entry(f.key()).unwrap().value {
            Value::Bool(b) => *b,
            // A choice is off at its first option.
            Value::Text(t) => match cho_settings::def(f.key()).unwrap().kind {
                cho_settings::Kind::Choice { options } => t.as_ref() != options[0],
                _ => true,
            },
            _ => true,
        }
    }

    #[test]
    fn every_setting_is_gated_or_says_why_not() {
        for d in cho_settings::defs() {
            let gated = Feature::ALL.iter().any(|f| f.key() == d.key);
            let excused = NOT_GATED.iter().any(|(k, _)| *k == d.key);
            assert!(gated || excused, "{} has no call-site gate", d.key);
            assert!(!(gated && excused), "{} is both gated and excused", d.key);
        }
        for f in Feature::ALL {
            assert!(cho_settings::def(f.key()).is_some(), "{}", f.key());
        }
        for (k, _) in NOT_GATED {
            assert!(cho_settings::def(k).is_some(), "{k}");
        }
    }

    /// `on` reads each feature's own setting: it agrees with the settings table with the
    /// toggle off, on again, and under Lightweight, so a feature wired to the wrong field
    /// fails here.
    #[test]
    fn on_agrees_with_the_settings_table() {
        for f in Feature::ALL {
            let (_dir, mut feats) = features();
            let toggle = matches!(
                cho_settings::def(f.key()).unwrap().kind,
                cho_settings::Kind::Bool
            );
            for v in [false, true] {
                if toggle {
                    feats
                        .store
                        .set(Scope::Global, f.key(), Value::Bool(v))
                        .unwrap();
                    assert_eq!(feats.on(f), v, "{} set to {v}", f.key());
                }
                assert_eq!(feats.on(f), in_table(&feats, f), "{}", f.key());
            }
        }
        let (_dir, feats) = lightweight();
        for f in Feature::ALL {
            assert_eq!(
                feats.on(f),
                in_table(&feats, f),
                "{} in Lightweight",
                f.key()
            );
        }
    }

    #[test]
    fn lightweight_turns_off_exactly_the_toggles_marked_for_it() {
        let (_dir, feats) = lightweight();
        assert!(feats.store.effective().lightweight());
        for f in Feature::ALL {
            let d = cho_settings::def(f.key()).unwrap();
            if matches!(d.kind, cho_settings::Kind::Bool) {
                assert_eq!(
                    feats.on(f),
                    !d.lightweight_off && d.default == Value::Bool(true),
                    "{}",
                    f.key()
                );
                assert!(!d.lightweight_off || !feats.on(f), "{}", f.key());
            }
        }
        // The ones the viewer's work depends on.
        for f in [
            Feature::Blame,
            Feature::ChangesAutoRefresh,
            Feature::ChangedWords,
            Feature::FollowHighlight,
            Feature::LineHighlight,
            Feature::FollowHover,
            Feature::FollowCaret,
            Feature::References,
            Feature::HighlightExact,
            Feature::CallKinds,
            Feature::AutoRefresh,
            Feature::Graph,
            Feature::FoldBodies,
            Feature::JsonFolds,
            Feature::FrameOverlay,
            Feature::FrameLog,
            Feature::LayoutAutosave,
            Feature::Minimap,
            Feature::Overview,
            Feature::InvestigationsWatch,
        ] {
            assert!(!feats.on(f), "{} stays on in Lightweight", f.key());
        }
    }

    #[test]
    fn timers_record_under_the_setting_key() {
        let feats = Features::with_store(SettingsStore::in_memory());
        for f in Feature::ALL {
            drop(feats.time(f));
            drop(feats.probe(f).start());
        }
        for f in Feature::ALL {
            let id = feats.metrics.id(f.key()).unwrap();
            assert_eq!(feats.metrics.stats(id).count, 2, "{}", f.key());
        }
    }

    #[test]
    fn a_settings_change_is_seen_without_restart() {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("settings.json");
        std::fs::write(&file, "{}").unwrap();
        let mut feats = Features::with_store(SettingsStore::open(Some(tmp.path().to_path_buf())));
        assert!(feats.on(Feature::FollowHover));
        // A different length, so the change is seen whatever the clock granularity.
        std::fs::write(&file, "{ \"context.follow_hover\": false }").unwrap();
        let changed = feats.reload().expect("the edit is noticed");
        assert!(changed.contains("context.follow_hover"));
        assert!(!feats.on(Feature::FollowHover));
    }
}
