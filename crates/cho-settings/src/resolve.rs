//! Layers and resolution: defaults < global < repository override < Lightweight mode.

use crate::defs::{DEFS, SettingDef, Settings, Value, index_of};

/// Where an effective value came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    Default,
    /// `settings.json`.
    Global,
    /// This repository's override in `repos.json`.
    Repo,
    /// Forced off by Lightweight mode.
    Lightweight,
}

impl Source {
    pub fn as_str(&self) -> &'static str {
        match self {
            Source::Default => "default",
            Source::Global => "global",
            Source::Repo => "repo",
            Source::Lightweight => "lightweight",
        }
    }
}

/// Values set explicitly in one file (or one repository's entry), by setting index.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Layer(Vec<Option<Value>>);

impl Default for Layer {
    fn default() -> Self {
        Layer(vec![None; DEFS.len()])
    }
}

impl Layer {
    pub fn get(&self, key: &str) -> Option<Value> {
        self.0[index_of(key)?].clone()
    }

    pub(crate) fn put(&mut self, key: &str, v: Value) {
        if let Some(i) = index_of(key) {
            self.0[i] = Some(v);
        }
    }

    pub fn is_empty(&self) -> bool {
        self.0.iter().all(Option::is_none)
    }
}

/// One setting's effective value and the story behind it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub value: Value,
    pub source: Source,
    /// When Lightweight mode forced the value, what it would be otherwise and from where.
    pub underlying: Option<(Value, Source)>,
}

impl Entry {
    /// A sentence saying why the value is what it is.
    pub fn explanation(&self) -> String {
        match &self.underlying {
            Some((v, s)) => format!(
                "{} because Lightweight mode is on (otherwise {v}, from {})",
                self.value,
                s.as_str()
            ),
            None => format!("{} from {}", self.value, self.source.as_str()),
        }
    }
}

/// Every setting resolved, with sources.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Effective {
    settings: Settings,
    entries: Vec<Entry>,
}

impl Effective {
    /// The typed settings the viewer reads.
    pub fn settings(&self) -> &Settings {
        &self.settings
    }

    pub fn lightweight(&self) -> bool {
        self.settings.lightweight
    }

    pub fn entry(&self, key: &str) -> Option<&Entry> {
        Some(&self.entries[index_of(key)?])
    }

    /// All settings in display order.
    pub fn iter(&self) -> impl Iterator<Item = (&'static SettingDef, &Entry)> {
        DEFS.iter().zip(&self.entries)
    }

    /// Keys whose effective value differs from `other`'s.
    pub fn diff(&self, other: &Effective) -> Vec<&'static str> {
        self.iter()
            .zip(other.iter())
            .filter(|((_, a), (_, b))| a.value != b.value)
            .map(|((d, _), _)| d.key)
            .collect()
    }
}

/// Resolve `defaults < global < repo < Lightweight`.
pub fn resolve(global: &Layer, repo: Option<&Layer>) -> Effective {
    let pick = |i: usize| -> (Value, Source) {
        let mut out = (DEFS[i].default.clone(), Source::Default);
        if let Some(v) = global.0[i].clone() {
            out = (v, Source::Global);
        }
        if let Some(v) = repo.and_then(|r| r.0[i].clone()) {
            out = (v, Source::Repo);
        }
        out
    };
    let lw_index = index_of("lightweight").expect("lightweight is defined");
    let lightweight = pick(lw_index).0 == Value::Bool(true);

    let mut entries = Vec::with_capacity(DEFS.len());
    let mut settings = Settings::default();
    for (i, d) in DEFS.iter().enumerate() {
        let (value, source) = pick(i);
        let entry = if lightweight && d.lightweight_off {
            Entry {
                value: d.off_value(),
                source: Source::Lightweight,
                underlying: Some((value, source)),
            }
        } else {
            Entry {
                value,
                source,
                underlying: None,
            }
        };
        settings.set(d.key, entry.value.clone());
        entries.push(entry);
    }
    Effective { settings, entries }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn layer(pairs: &[(&str, Value)]) -> Layer {
        let mut l = Layer::default();
        for (k, v) in pairs {
            l.put(k, v.clone());
        }
        l
    }
    const T: Value = Value::Bool(true);
    const F: Value = Value::Bool(false);

    #[test]
    fn defaults_alone() {
        let e = resolve(&Layer::default(), None);
        assert_eq!(e.settings(), &Settings::default());
        assert!(e.iter().all(|(_, en)| en.source == Source::Default));
    }

    #[test]
    fn global_beats_default_repo_beats_global() {
        let g = layer(&[("hover.card", F), ("search.max_results", Value::UInt(900))]);
        let r = layer(&[("hover.card", T)]);
        let e = resolve(&g, None);
        assert_eq!(e.entry("hover.card").unwrap().source, Source::Global);
        assert!(!e.settings().hover_card);
        let e = resolve(&g, Some(&r));
        let en = e.entry("hover.card").unwrap();
        assert_eq!((en.value.clone(), en.source), (T, Source::Repo));
        // Untouched by the repo layer: still global.
        assert_eq!(
            e.entry("search.max_results").unwrap().source,
            Source::Global
        );
        assert_eq!(e.settings().search_max_results, 900);
    }

    #[test]
    fn lightweight_beats_everything_and_remembers_underneath() {
        let g = layer(&[("lightweight", T), ("hover.card", T)]);
        let r = layer(&[("index.auto_refresh", T)]);
        let e = resolve(&g, Some(&r));
        assert!(e.lightweight());
        for (d, en) in e.iter().filter(|(d, _)| d.lightweight_off) {
            assert_eq!(en.value, d.off_value(), "{}", d.key);
            assert_eq!(en.source, Source::Lightweight);
        }
        let en = e.entry("index.auto_refresh").unwrap();
        assert_eq!(en.underlying, Some((T, Source::Repo)));
        assert!(en.explanation().contains("Lightweight") && en.explanation().contains("repo"));
        // Settings not marked lightweight keep their values and sources.
        let en = e.entry("search.max_results").unwrap();
        assert_eq!(
            (en.value.clone(), en.source),
            (Value::UInt(5000), Source::Default)
        );
        // The flag itself reports where it was set.
        assert_eq!(e.entry("lightweight").unwrap().source, Source::Global);
    }

    #[test]
    fn lightweight_round_trip_restores_individual_values() {
        let before = resolve(&layer(&[("hover.peek", F), ("graph.enabled", T)]), None);
        let on = resolve(
            &layer(&[("hover.peek", F), ("graph.enabled", T), ("lightweight", T)]),
            None,
        );
        let off = resolve(
            &layer(&[("hover.peek", F), ("graph.enabled", T), ("lightweight", F)]),
            None,
        );
        assert_ne!(before, on);
        assert_eq!(before.settings(), off.settings());
        let expected: Vec<&str> = before
            .iter()
            .filter(|(d, en)| d.lightweight_off && en.value != d.off_value())
            .map(|(d, _)| d.key)
            .chain(["lightweight"])
            .collect();
        let mut got = before.diff(&on);
        got.sort_unstable();
        let mut expected = expected;
        expected.sort_unstable();
        assert_eq!(got, expected);
        assert!(
            !got.contains(&"hover.peek"),
            "already off, so it doesn't change"
        );
    }

    #[test]
    fn repo_can_turn_lightweight_on_for_one_repository() {
        let g = Layer::default();
        let r = layer(&[("lightweight", T)]);
        assert!(!resolve(&g, None).lightweight());
        let e = resolve(&g, Some(&r));
        assert!(e.lightweight() && e.entry("lightweight").unwrap().source == Source::Repo);
    }

    #[test]
    fn diff_lists_changed_keys_only() {
        let a = resolve(&Layer::default(), None);
        let b = resolve(&layer(&[("hover.card", F), ("hover.peek", T)]), None);
        assert_eq!(a.diff(&b), ["hover.card"]);
    }
}
