//! The settings page without any GUI: which rows exist, what they show, and what each key
//! does. Everything writes through the one `SettingsStore` (which edits the file text in
//! place), so the page and `settings.json` are the same settings.

use cho_settings::{
    CostClass, Entry, Kind, SaveError, Scope, SettingDef, SettingsStore, Source, Value, defs,
};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// What a feature has cost in this session (a copy of the numbers the page shows).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Measured {
    pub count: u64,
    pub average: Duration,
    pub worst: Duration,
    /// Total time in the last five minutes.
    pub window_total: Duration,
    /// Background work only: CPU time and peak memory, where the platform reports them.
    pub cpu: Option<Duration>,
    pub peak_rss_bytes: Option<u64>,
}

/// Where the page reads measured cost from. `cho_metrics::Metrics` is the real one.
pub trait Costs {
    fn measured(&self, key: &str) -> Option<Measured>;
    /// Keys with the highest total time in the last five minutes, most first; only features
    /// that actually ran.
    fn top(&self, n: usize) -> Vec<&'static str>;
}

impl Costs for cho_metrics::Metrics {
    fn measured(&self, key: &str) -> Option<Measured> {
        let s = self.stats(self.id(key)?);
        Some(Measured {
            count: s.count,
            average: s.average,
            worst: s.worst,
            window_total: s.window.total,
            cpu: s.background.map(|b| b.cpu_total),
            peak_rss_bytes: s.background.map(|b| b.peak_rss_bytes),
        })
    }

    fn top(&self, n: usize) -> Vec<&'static str> {
        self.top_by_window_total(n)
            .into_iter()
            .filter(|s| s.window.total > Duration::ZERO)
            .map(|s| s.key)
            .collect()
    }
}

/// One setting as the page shows it.
#[derive(Clone, Debug)]
pub struct Row {
    pub def: &'static SettingDef,
    pub entry: Entry,
    /// There is an override for the open repository, so the row is in the repository scope.
    pub repo_scope: bool,
    pub measured: Option<Measured>,
    /// The text being typed into a number field, if this row is being edited.
    pub editing: Option<String>,
}

impl Row {
    pub fn is_lightweight_card(&self) -> bool {
        self.def.key == LIGHTWEIGHT
    }

    pub fn is_on(&self) -> bool {
        matches!(self.entry.value, Value::Bool(true))
    }

    /// The badge text: where the shown value comes from.
    pub fn source_label(&self) -> &'static str {
        source_label(self.entry.source)
    }
}

pub const LIGHTWEIGHT: &str = "lightweight";

fn source_label(s: Source) -> &'static str {
    match s {
        Source::Default => "default",
        Source::Global => "settings.json",
        Source::Repo => "this repository",
        Source::Lightweight => "Lightweight",
    }
}

pub fn class_label(c: CostClass) -> &'static str {
    match c {
        CostClass::BackgroundHeavy => "background \u{b7} heavy",
        CostClass::PerPointer => "per pointer move",
        CostClass::PerFrame => "per frame",
        CostClass::OnDemand => "on demand",
        CostClass::Negligible => "negligible",
    }
}

/// `0.3 ms`, `2.1 s`: a duration at the precision a cost needs.
pub fn fmt_duration(d: Duration) -> String {
    let us = d.as_secs_f64() * 1e6;
    if us < 1.0 {
        "<1 \u{b5}s".into()
    } else if us < 1000.0 {
        format!("{us:.0} \u{b5}s")
    } else if us < 1e6 {
        format!("{:.1} ms", us / 1e3)
    } else {
        format!("{:.1} s", us / 1e6)
    }
}

/// The "measured on this machine" line of a row.
pub fn measured_text(def: &SettingDef, m: Option<&Measured>) -> String {
    let background = def.cost_class == CostClass::BackgroundHeavy;
    let Some(m) = m.filter(|m| m.count > 0 || m.cpu.is_some()) else {
        return if background {
            "Not run yet in this session".into()
        } else {
            "Not used yet in this session".into()
        };
    };
    let mut parts = vec![
        format!("{} runs", m.count),
        format!("avg {}", fmt_duration(m.average)),
        format!("worst {}", fmt_duration(m.worst)),
    ];
    if background {
        parts.push(match m.cpu {
            Some(c) => format!("CPU {}", fmt_duration(c)),
            None => "CPU unavailable".into(),
        });
        parts.push(match m.peak_rss_bytes {
            Some(b) => format!("peak memory {}", cho_core::words::size_text(b)),
            None => "memory unavailable".into(),
        });
    }
    parts.join(" \u{b7} ")
}

/// A key press, reduced to what the page cares about.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Key {
    Up,
    Down,
    Home,
    End,
    Space,
    Enter,
    Escape,
    Backspace,
    Char(char),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    Nothing,
    /// Something on the page changed (repaint).
    Redraw,
    /// A setting changed: other windows must repaint too.
    Changed,
    Close,
}

/// What keys on the page go to.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Mode {
    /// Keys move the selection and act on the selected row.
    #[default]
    Browse,
    /// Keys go to the filter box until Enter or Esc.
    Filter,
    /// A number being typed for the selected row.
    Edit(String),
}

/// The page's state: which row is selected, the filter, a number being typed.
#[derive(Default)]
pub struct Model {
    pub filter: String,
    /// Cmd-A selected the whole filter (typing replaces it).
    pub filter_all: bool,
    /// Cmd-A selected the whole number being typed.
    pub edit_all: bool,
    pub mode: Mode,
    /// Index into the visible rows.
    pub selected: usize,
    /// The last thing that went wrong, shown under the title until the next key.
    pub notice: Option<String>,
}

/// The value a row holds itself: under Lightweight, the one Lightweight hides.
fn own_value(entry: &Entry) -> Value {
    entry
        .underlying
        .clone()
        .map_or(entry.value.clone(), |(v, _)| v)
}

/// The repository a write to `key` goes to: the active one while it has an override for `key`;
/// `None` for `settings.json`.
fn repo_scope(store: &SettingsStore, key: &str) -> Option<PathBuf> {
    store.repo_value(key)?;
    store.active_repo().map(|p| Path::new(p).to_path_buf())
}

fn scope(repo: Option<&Path>) -> Scope<'_> {
    repo.map_or(Scope::Global, Scope::Repo)
}

/// Writes `value` to `key` in the scope it is set in.
fn write(
    store: &mut SettingsStore,
    key: &str,
    value: Value,
) -> Result<cho_settings::ChangeSet, SaveError> {
    let repo = repo_scope(store, key);
    store.set(scope(repo.as_deref()), key, value)
}

/// Flips the on/off setting `key` in the scope it is set in. `None` when `key` is not a toggle.
pub fn flip(
    store: &mut SettingsStore,
    key: &str,
) -> Option<Result<cho_settings::ChangeSet, SaveError>> {
    match own_value(store.effective().entry(key)?) {
        Value::Bool(b) => Some(write(store, key, Value::Bool(!b))),
        // A choice steps to its next option, wrapping round.
        Value::Text(t) => match cho_settings::def(key)?.kind {
            Kind::Choice { options } => {
                let at = options.iter().position(|o| *o == t.as_ref()).unwrap_or(0);
                let next = options[(at + 1) % options.len()];
                Some(write(store, key, Value::text(next)))
            }
            _ => None,
        },
        Value::UInt(_) => None,
    }
}

impl Model {
    /// How many settings match the search, for the search box.
    pub fn matching(&self) -> usize {
        self.visible().len()
    }

    /// The search box was clicked: keys go to it.
    pub fn start_search(&mut self) {
        self.mode = Mode::Filter;
        self.filter_all = false;
    }

    /// Text typed while browsing starts a search with it.
    pub fn type_to_search(&mut self, text: &str) {
        self.mode = Mode::Filter;
        self.filter_all = false;
        self.filter.push_str(text);
        self.selected = 0;
    }

    /// A keystroke for the field that has the keys (the search box, or a number being
    /// typed), through the shared field editing: Cmd-A, copy, paste, word and line delete
    /// work as in every other field. `FieldEdit::None` when it is not a field key here.
    pub fn field(
        &mut self,
        k: &gpui::Keystroke,
        paste: Option<&str>,
    ) -> cho_layout_ui::field::FieldEdit {
        use cho_layout_ui::field::{FieldEdit, edit_field};
        match &mut self.mode {
            Mode::Filter => {
                let e = edit_field(k, &mut self.filter, &mut self.filter_all, paste);
                if e == FieldEdit::Text {
                    self.selected = 0;
                }
                e
            }
            Mode::Edit(text) => {
                let e = edit_field(k, text, &mut self.edit_all, paste);
                if e == FieldEdit::Text {
                    text.retain(|c| c.is_ascii_digit());
                }
                e
            }
            Mode::Browse => FieldEdit::None,
        }
    }

    /// The search box's clear button: no search, keys back on the rows.
    pub fn clear_search(&mut self) {
        self.filter.clear();
        self.mode = Mode::Browse;
        self.selected = 0;
    }

    /// The settings that match the search (key, description or cost hint), in display order
    /// (the Lightweight card first).
    fn visible(&self) -> Vec<&'static SettingDef> {
        let needle = self.filter.trim().to_lowercase();
        defs()
            .iter()
            .filter(|d| {
                needle.is_empty()
                    || d.key.to_lowercase().contains(&needle)
                    || d.description.to_lowercase().contains(&needle)
                    || d.cost_hint.to_lowercase().contains(&needle)
            })
            .collect()
    }

    /// The rows that match the filter, in display order (the Lightweight card first).
    pub fn rows(&self, store: &SettingsStore, costs: &dyn Costs) -> Vec<Row> {
        let selected = self.selected_def().map(|d| d.key);
        let editing = match &self.mode {
            Mode::Edit(text) => Some(text),
            _ => None,
        };
        self.visible()
            .into_iter()
            .filter_map(|d| {
                let entry = store.effective().entry(d.key)?.clone();
                Some(Row {
                    def: d,
                    entry,
                    repo_scope: store.repo_value(d.key).is_some(),
                    measured: costs.measured(d.key),
                    editing: editing.filter(|_| selected == Some(d.key)).cloned(),
                })
            })
            .collect()
    }

    /// The three features that have cost the most time in the last five minutes.
    pub fn costing_most(&self, store: &SettingsStore, costs: &dyn Costs) -> Vec<Row> {
        costs
            .top(3)
            .into_iter()
            .filter_map(|k| {
                let d = cho_settings::def(k)?;
                let entry = store.effective().entry(k)?.clone();
                Some(Row {
                    def: d,
                    entry,
                    repo_scope: store.repo_value(k).is_some(),
                    measured: costs.measured(k),
                    editing: None,
                })
            })
            .collect()
    }

    /// Selects row `index` and leaves the filter box or a number being typed.
    pub fn select(&mut self, index: usize) {
        self.selected = index;
        self.mode = Mode::Browse;
    }

    /// Handle one key. `store` is written through immediately.
    pub fn key(&mut self, store: &mut SettingsStore, key: Key) -> Outcome {
        self.notice = None;
        match self.mode {
            Mode::Edit(_) => return self.key_editing(store, key),
            Mode::Filter => return self.key_filtering(key),
            Mode::Browse => {}
        }
        let n = self.visible().len();
        match key {
            Key::Up => self.selected = self.selected.saturating_sub(1),
            Key::Down => self.selected = (self.selected + 1).min(n.saturating_sub(1)),
            Key::Home => self.selected = 0,
            Key::End => self.selected = n.saturating_sub(1),
            Key::Escape => {
                if self.filter.is_empty() {
                    return Outcome::Close;
                }
                self.filter.clear();
                self.selected = 0;
            }
            Key::Space | Key::Enter => return self.activate(store),
            Key::Backspace => return self.reset(store),
            Key::Char('r' | 'R') => return self.toggle_scope(store),
            Key::Char('/') => self.mode = Mode::Filter,
            Key::Char(_) => return Outcome::Nothing,
        }
        Outcome::Redraw
    }

    fn key_filtering(&mut self, key: Key) -> Outcome {
        match key {
            Key::Escape => {
                self.filter.clear();
                self.mode = Mode::Browse;
            }
            Key::Enter | Key::Up | Key::Down => {
                self.mode = Mode::Browse;
                if key == Key::Down {
                    self.selected = 1.min(self.visible().len().saturating_sub(1));
                }
            }
            Key::Backspace => {
                self.filter.pop();
            }
            Key::Space => self.filter.push(' '),
            Key::Char(c) if !c.is_control() => self.filter.push(c),
            _ => return Outcome::Nothing,
        }
        self.selected = 0;
        Outcome::Redraw
    }

    fn key_editing(&mut self, store: &mut SettingsStore, key: Key) -> Outcome {
        let Mode::Edit(text) = &mut self.mode else {
            return Outcome::Nothing;
        };
        match key {
            Key::Escape => self.mode = Mode::Browse,
            Key::Backspace => {
                text.pop();
            }
            Key::Char(c) if c.is_ascii_digit() => text.push(c),
            Key::Enter => {
                let text = text.clone();
                let Some(def) = self.selected_def() else {
                    self.mode = Mode::Browse;
                    return Outcome::Redraw;
                };
                match def.kind.parse(&text) {
                    Ok(v) => {
                        self.mode = Mode::Browse;
                        return self.finish(write(store, def.key, v));
                    }
                    Err(e) => self.notice = Some(format!("{}: {e}", def.key)),
                }
            }
            _ => return Outcome::Nothing,
        }
        Outcome::Redraw
    }

    fn selected_def(&self) -> Option<&'static SettingDef> {
        let visible = self.visible();
        visible
            .get(self.selected.min(visible.len().saturating_sub(1)))
            .copied()
    }

    /// Space / Enter on the selected row: flip a toggle, or start typing a number.
    fn activate(&mut self, store: &mut SettingsStore) -> Outcome {
        let Some(def) = self.selected_def() else {
            return Outcome::Nothing;
        };
        if let Kind::UInt { .. } = def.kind {
            let Some(entry) = store.effective().entry(def.key) else {
                return Outcome::Nothing;
            };
            self.mode = Mode::Edit(own_value(entry).to_string());
            self.edit_all = false;
            return Outcome::Redraw;
        }
        self.toggle_hidden(store, def.key)
    }

    /// Change a toggle with the pointer (the same write as Space).
    pub fn toggle(&mut self, store: &mut SettingsStore, key: &str) -> Outcome {
        let Some(i) = self.visible().iter().position(|d| d.key == key) else {
            return self.toggle_hidden(store, key);
        };
        self.selected = i;
        self.activate(store)
    }

    /// A toggle in "Costing the most" for a row the filter hides.
    fn toggle_hidden(&mut self, store: &mut SettingsStore, key: &str) -> Outcome {
        match flip(store, key) {
            Some(r) => self.finish(r),
            None => Outcome::Nothing, // Font: picker added separately
        }
    }

    /// Sets a text setting (a font family) in the selected scope, checked against its kind.
    pub fn set_text(&mut self, store: &mut SettingsStore, key: &str, text: &str) -> Outcome {
        let Some(def) = cho_settings::def(key) else {
            return Outcome::Nothing;
        };
        match def.kind.parse(text) {
            Ok(v) => self.finish(write(store, key, v)),
            Err(_) => Outcome::Nothing,
        }
    }

    fn finish(&mut self, r: Result<cho_settings::ChangeSet, SaveError>) -> Outcome {
        match r {
            Ok(c) if c.is_empty() => Outcome::Redraw,
            Ok(_) => Outcome::Changed,
            Err(e) => {
                self.notice = Some(e.to_string());
                Outcome::Redraw
            }
        }
    }

    /// Backspace: remove the setting from its scope so the next layer down applies.
    fn reset(&mut self, store: &mut SettingsStore) -> Outcome {
        let Some(def) = self.selected_def() else {
            return Outcome::Nothing;
        };
        let repo = repo_scope(store, def.key);
        self.finish(store.unset(scope(repo.as_deref()), def.key))
    }

    /// R: move the row between "everywhere" and "this repository only". Turning the
    /// repository scope on writes the row's current own value as an override; turning it
    /// off removes the override.
    fn toggle_scope(&mut self, store: &mut SettingsStore) -> Outcome {
        let Some(def) = self.selected_def() else {
            return Outcome::Nothing;
        };
        let Some(repo) = store.active_repo().map(|p| Path::new(p).to_path_buf()) else {
            self.notice = Some("Open a repository to set something for it alone".into());
            return Outcome::Redraw;
        };
        if store.repo_value(def.key).is_some() {
            return self.finish(store.unset(Scope::Repo(&repo), def.key));
        }
        let Some(entry) = store.effective().entry(def.key) else {
            return Outcome::Nothing;
        };
        let own = own_value(entry);
        self.finish(store.set(Scope::Repo(&repo), def.key, own))
    }

    /// Keep the selection inside the visible rows after the filter or the store changed.
    pub fn clamp(&mut self) {
        let n = self.visible().len();
        self.selected = self.selected.min(n.saturating_sub(1));
    }
}

#[cfg(test)]
#[allow(clippy::disallowed_methods)] // tests read their own fixtures
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[derive(Default)]
    struct FakeCosts(HashMap<&'static str, Measured>);
    impl Costs for FakeCosts {
        fn measured(&self, key: &str) -> Option<Measured> {
            self.0.get(key).copied()
        }
        fn top(&self, n: usize) -> Vec<&'static str> {
            let mut v: Vec<_> = self.0.iter().collect();
            v.sort_by_key(|(_, m)| std::cmp::Reverse(m.window_total));
            v.into_iter().take(n).map(|(k, _)| *k).collect()
        }
    }

    struct Fixture {
        _dir: tempfile::TempDir,
        repo: std::path::PathBuf,
        store: SettingsStore,
        model: Model,
        costs: FakeCosts,
    }

    fn fixture() -> Fixture {
        let dir = tempfile::tempdir().unwrap();
        let config = dir.path().join("config");
        let repo = dir.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        let mut store = SettingsStore::open(Some(config));
        store.set_active_repo(Some(&repo)).unwrap();
        Fixture {
            _dir: dir,
            repo,
            store,
            model: Model::default(),
            costs: FakeCosts::default(),
        }
    }

    fn select(f: &mut Fixture, key: &str) {
        f.model.filter.clear();
        f.model.selected = f.model.visible().iter().position(|d| d.key == key).unwrap();
    }

    fn row(f: &Fixture, key: &str) -> Row {
        let m = Model {
            filter: key.into(),
            ..Model::default()
        };
        m.rows(&f.store, &f.costs)
            .into_iter()
            .find(|r| r.def.key == key)
            .unwrap()
    }

    #[test]
    fn one_row_per_setting_with_lightweight_first() {
        let f = fixture();
        let rows = f.model.rows(&f.store, &f.costs);
        assert_eq!(rows.len(), defs().len());
        assert!(rows[0].is_lightweight_card());
        assert_eq!(rows[1].source_label(), "default");
    }

    #[test]
    fn filter_matches_key_and_description_case_insensitively() {
        let f = fixture();
        let mut m = Model {
            filter: "HOVER".into(),
            ..Model::default()
        };
        let keys: Vec<_> = m
            .rows(&f.store, &f.costs)
            .iter()
            .map(|r| r.def.key)
            .collect();
        assert!(keys.contains(&"hover.card") && keys.contains(&"context.follow_hover"));
        assert!(!keys.contains(&"graph.enabled"));
        m.filter = "background indexes".into();
        assert!(m.rows(&f.store, &f.costs).is_empty());
    }

    #[test]
    fn search_also_matches_cost_hints_and_the_box_starts_and_clears_it() {
        let f = fixture();
        let mut m = Model::default();
        m.start_search();
        assert_eq!(m.mode, Mode::Filter);
        // "indexer" is in index.auto_refresh's cost hint, not its key or description.
        m.filter = "indexer".into();
        let keys: Vec<_> = m
            .rows(&f.store, &f.costs)
            .iter()
            .map(|r| r.def.key)
            .collect();
        assert!(keys.contains(&"index.auto_refresh"), "{keys:?}");
        assert_eq!(
            m.matching(),
            keys.iter().filter(|k| **k != "lightweight").count()
        );
        m.selected = 2;
        m.clear_search();
        assert!(m.filter.is_empty() && m.mode == Mode::Browse && m.selected == 0);
    }

    #[test]
    fn typing_starts_the_filter_and_the_filter_takes_the_shared_field_keys() {
        // A key as typed (with its text, as GPUI gives it); `cmd` is Ctrl off macOS.
        let typed = |spec: &str| {
            let spec = if cfg!(target_os = "macos") {
                spec.to_string()
            } else {
                spec.replace("cmd-", "ctrl-")
            };
            let mut k = gpui::Keystroke::parse(&spec).unwrap();
            if !k.modifiers.secondary() {
                k.key_char = match k.key.as_str() {
                    "space" => Some(" ".into()),
                    one if one.chars().count() == 1 => Some(one.into()),
                    _ => None,
                };
            }
            k
        };
        let mut f = fixture();
        // Typed text while browsing starts the filter (the view sends it here).
        f.model.type_to_search("h");
        assert_eq!(f.model.mode, Mode::Filter);
        assert_eq!(f.model.filter, "h");
        // Inside the filter, r and space are text.
        f.model.field(&typed("r"), None);
        f.model.field(&typed("space"), None);
        assert_eq!(f.model.filter, "hr ");
        // Cmd-A selects it; typing replaces it; Cmd-Backspace clears it.
        f.model.field(&typed("cmd-a"), None);
        assert!(f.model.filter_all);
        f.model.field(&typed("x"), None);
        assert_eq!(f.model.filter, "x");
        f.model.field(&typed("cmd-backspace"), None);
        assert!(f.model.filter.is_empty());
        // Esc leaves the filter; a second Esc closes.
        f.model.key(&mut f.store, Key::Escape);
        assert!(f.model.mode == Mode::Browse);
        assert_eq!(f.model.key(&mut f.store, Key::Escape), Outcome::Close);
        // `/` starts an empty filter.
        f.model.key(&mut f.store, Key::Char('/'));
        assert!(f.model.mode == Mode::Filter && f.model.filter.is_empty());
    }

    #[test]
    fn arrows_move_and_stop_at_the_ends() {
        let mut f = fixture();
        f.model.key(&mut f.store, Key::Up);
        assert_eq!(f.model.selected, 0);
        f.model.key(&mut f.store, Key::Down);
        f.model.key(&mut f.store, Key::Down);
        assert_eq!(f.model.selected, 2);
        f.model.key(&mut f.store, Key::End);
        assert_eq!(f.model.selected, defs().len() - 1);
        f.model.key(&mut f.store, Key::Down);
        assert_eq!(f.model.selected, defs().len() - 1);
    }

    #[test]
    fn space_toggles_and_saves_to_settings_json() {
        let mut f = fixture();
        select(&mut f, "hover.card");
        assert_eq!(f.model.key(&mut f.store, Key::Space), Outcome::Changed);
        let r = row(&f, "hover.card");
        assert!(!r.is_on());
        assert_eq!(r.source_label(), "settings.json");
        let path = f.store.file_path(cho_settings::FileKind::Settings).unwrap();
        assert!(
            std::fs::read_to_string(path)
                .unwrap()
                .contains("\"hover.card\": false")
        );
        // And back.
        f.model.key(&mut f.store, Key::Space);
        assert!(row(&f, "hover.card").is_on());
    }

    #[test]
    fn enter_edits_a_number_and_rejects_out_of_range() {
        let mut f = fixture();
        select(&mut f, "search.max_results");
        f.model.key(&mut f.store, Key::Enter);
        assert_eq!(f.model.mode, Mode::Edit("5000".into()));
        for _ in 0..4 {
            f.model.key(&mut f.store, Key::Backspace);
        }
        f.model.key(&mut f.store, Key::Char('5'));
        // 5 is below the minimum of 100: refused, still editing, notice shown.
        f.model.key(&mut f.store, Key::Enter);
        assert!(matches!(f.model.mode, Mode::Edit(_)) && f.model.notice.is_some());
        f.model.key(&mut f.store, Key::Char('0'));
        f.model.key(&mut f.store, Key::Char('0'));
        assert_eq!(f.model.key(&mut f.store, Key::Enter), Outcome::Changed);
        assert_eq!(f.model.mode, Mode::Browse);
        assert_eq!(row(&f, "search.max_results").entry.value, Value::UInt(500));
        // Esc abandons an edit without writing.
        f.model.key(&mut f.store, Key::Enter);
        f.model.key(&mut f.store, Key::Char('9'));
        f.model.key(&mut f.store, Key::Escape);
        assert_eq!(row(&f, "search.max_results").entry.value, Value::UInt(500));
    }

    #[test]
    fn letters_are_not_typed_into_a_number() {
        let mut f = fixture();
        select(&mut f, "search.max_results");
        f.model.key(&mut f.store, Key::Enter);
        assert_eq!(f.model.key(&mut f.store, Key::Char('x')), Outcome::Nothing);
        assert_eq!(f.model.mode, Mode::Edit("5000".into()));
    }

    #[test]
    fn r_switches_scope_and_writes_to_the_repository() {
        let mut f = fixture();
        select(&mut f, "hover.card");
        f.model.key(&mut f.store, Key::Char('r'));
        let r = row(&f, "hover.card");
        assert!(r.repo_scope);
        assert_eq!(r.source_label(), "this repository");
        // A toggle now goes to the repository, leaving settings.json alone.
        f.model.key(&mut f.store, Key::Space);
        let r = row(&f, "hover.card");
        assert!(!r.is_on() && r.entry.source == Source::Repo);
        assert_eq!(f.store.global_value("hover.card"), None);
        assert_eq!(f.store.repo_value("hover.card"), Some(Value::Bool(false)));
        // R again drops the override.
        f.model.key(&mut f.store, Key::Char('R'));
        let r = row(&f, "hover.card");
        assert!(!r.repo_scope && r.is_on());
        let _ = &f.repo;
    }

    #[test]
    fn r_without_a_repository_says_so() {
        let mut f = fixture();
        f.store.set_active_repo(None).unwrap();
        select(&mut f, "hover.card");
        f.model.key(&mut f.store, Key::Char('r'));
        assert!(f.model.notice.as_deref().unwrap().contains("repository"));
        assert!(!row(&f, "hover.card").repo_scope);
    }

    #[test]
    fn backspace_resets_the_scope_the_row_is_in() {
        let mut f = fixture();
        select(&mut f, "hover.card");
        f.model.key(&mut f.store, Key::Space); // global false
        f.model.key(&mut f.store, Key::Char('r')); // repo override (false)
        f.model.key(&mut f.store, Key::Space); // repo true
        assert!(row(&f, "hover.card").is_on());
        // First Backspace removes the repository override: back to the global false.
        f.model.key(&mut f.store, Key::Backspace);
        let r = row(&f, "hover.card");
        assert!(!r.is_on() && r.entry.source == Source::Global);
        // Second removes the global value: default.
        f.model.key(&mut f.store, Key::Backspace);
        let r = row(&f, "hover.card");
        assert!(r.is_on() && r.entry.source == Source::Default);
        // Nothing left to reset: no change, no error.
        assert_eq!(f.model.key(&mut f.store, Key::Backspace), Outcome::Redraw);
        assert!(f.model.notice.is_none());
    }

    #[test]
    fn lightweight_forces_rows_off_but_a_toggle_changes_the_row_underneath() {
        let mut f = fixture();
        select(&mut f, LIGHTWEIGHT);
        assert_eq!(f.model.key(&mut f.store, Key::Space), Outcome::Changed);
        assert!(f.store.effective().lightweight());
        let r = row(&f, "hover.card");
        assert!(!r.is_on());
        assert_eq!(r.source_label(), "Lightweight");
        // Space on a forced-off row changes its own setting (true -> false), still shown off.
        select(&mut f, "hover.card");
        f.model.key(&mut f.store, Key::Space);
        assert_eq!(f.store.global_value("hover.card"), Some(Value::Bool(false)));
        // Number settings stay editable under Lightweight.
        select(&mut f, "search.max_results");
        f.model.key(&mut f.store, Key::Enter);
        assert!(matches!(f.model.mode, Mode::Edit(_)));
        f.model.key(&mut f.store, Key::Escape);
        // Turning Lightweight off restores each feature's own setting.
        select(&mut f, LIGHTWEIGHT);
        f.model.key(&mut f.store, Key::Space);
        assert!(!f.store.effective().lightweight());
        assert!(!row(&f, "hover.card").is_on());
        assert!(row(&f, "graph.enabled").is_on());
    }

    #[test]
    fn a_broken_settings_file_is_not_overwritten() {
        let mut f = fixture();
        let path = f
            .store
            .file_path(cho_settings::FileKind::Settings)
            .unwrap()
            .to_path_buf();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "{ \"hover.card\": ").unwrap();
        f.store.reload_if_changed();
        assert!(
            f.store
                .diagnostics()
                .iter()
                .any(|d| d.kind == cho_settings::DiagnosticKind::Malformed)
        );
        select(&mut f, "hover.card");
        f.model.key(&mut f.store, Key::Space);
        assert!(f.model.notice.is_some());
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "{ \"hover.card\": "
        );
    }

    #[test]
    fn costing_the_most_lists_the_top_three_with_their_values() {
        let mut f = fixture();
        let m = |ms: u64| Measured {
            count: 3,
            window_total: Duration::from_millis(ms),
            ..Default::default()
        };
        for (k, ms) in [
            ("hover.card", 5),
            ("lens.call_kinds", 40),
            ("context.references", 9),
            ("graph.enabled", 1),
        ] {
            f.costs.0.insert(k, m(ms));
        }
        let keys: Vec<_> = f
            .model
            .costing_most(&f.store, &f.costs)
            .iter()
            .map(|r| r.def.key)
            .collect();
        assert_eq!(
            keys,
            ["lens.call_kinds", "context.references", "hover.card"]
        );
        // A hidden (filtered-out) row can still be toggled from that summary.
        f.model.filter = "graph".into();
        assert_eq!(f.model.toggle(&mut f.store, "hover.card"), Outcome::Changed);
        assert!(!f.store.effective().settings().hover_card);
    }

    #[test]
    fn measured_text_reads_plainly() {
        let d = cho_settings::def("hover.card").unwrap();
        assert_eq!(measured_text(d, None), "Not used yet in this session");
        let m = Measured {
            count: 12,
            average: Duration::from_micros(300),
            worst: Duration::from_micros(2100),
            ..Default::default()
        };
        assert_eq!(
            measured_text(d, Some(&m)),
            "12 runs \u{b7} avg 300 \u{b5}s \u{b7} worst 2.1 ms"
        );
        let bg = cho_settings::def("index.auto_refresh").unwrap();
        let m = Measured {
            count: 1,
            average: Duration::from_secs(3),
            worst: Duration::from_secs(3),
            cpu: Some(Duration::from_millis(4200)),
            peak_rss_bytes: Some(310 * 1024 * 1024),
            ..Default::default()
        };
        let t = measured_text(bg, Some(&m));
        assert!(
            t.contains("CPU 4.2 s") && t.contains("peak memory 310 MB"),
            "{t}"
        );
        let t = measured_text(
            bg,
            Some(&Measured {
                count: 1,
                ..Default::default()
            }),
        );
        assert!(t.contains("CPU unavailable") && t.contains("memory unavailable"));
    }
}
