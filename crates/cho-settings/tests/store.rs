//! The store against real files in a temporary directory.
#![allow(clippy::disallowed_methods)] // tests read their own fixtures

use cho_settings::*;
use std::path::{Path, PathBuf};

struct Dir(PathBuf);

impl Dir {
    fn new(name: &str) -> Self {
        let p =
            std::env::temp_dir().join(format!("cho-settings-test-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        Dir(p)
    }
    fn cfg(&self) -> PathBuf {
        self.0.join("cfg")
    }
    fn repo(&self, name: &str) -> PathBuf {
        let p = self.0.join(name);
        std::fs::create_dir_all(&p).unwrap();
        p
    }
    fn write(&self, file: &str, text: &str) {
        std::fs::create_dir_all(self.cfg()).unwrap();
        std::fs::write(self.cfg().join(file), text).unwrap();
    }
    fn read(&self, file: &str) -> String {
        std::fs::read_to_string(self.cfg().join(file)).unwrap()
    }
    fn open(&self) -> SettingsStore {
        SettingsStore::open(Some(self.cfg()))
    }
}

impl Drop for Dir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

const T: Value = Value::Bool(true);
const F: Value = Value::Bool(false);

fn src(s: &SettingsStore, key: &str) -> Source {
    s.effective().entry(key).unwrap().source
}

#[test]
fn no_files_means_defaults() {
    let d = Dir::new("defaults");
    let s = d.open();
    assert_eq!(s.effective().settings(), &Settings::default());
    assert_eq!(s.file_status(FileKind::Settings), &FileStatus::Missing);
    assert!(s.diagnostics().is_empty());
    assert_eq!(s.version(), 0);
}

#[test]
fn reads_jsonc_with_comments_and_trailing_commas() {
    let d = Dir::new("jsonc");
    d.write(
        "settings.json",
        "// Chorong settings\n{\n  /* off for now */ \"hover.card\": false, // slow disk\n  \"search.max_results\": 250,\n}\n",
    );
    let s = d.open();
    assert!(!s.effective().settings().hover_card);
    assert_eq!(s.effective().settings().search_max_results, 250);
    assert_eq!(src(&s, "hover.card"), Source::Global);
    assert_eq!(src(&s, "hover.peek"), Source::Default);
    assert!(s.diagnostics().is_empty());
}

#[test]
fn unknown_keys_are_reported_and_preserved_through_a_save() {
    let d = Dir::new("unknown");
    let text = "{\n  \"future.thing\": 1, // from a newer Chorong\n  \"context\": { \"follow_hover\": false },\n  \"hover.card\": true\n}\n";
    d.write("settings.json", text);
    let mut s = d.open();
    let unknown: Vec<_> = s.unknown_keys().into_iter().filter_map(|u| u.key).collect();
    assert_eq!(unknown, ["future.thing", "context"]);
    assert!(s.unknown_keys()[1].message.contains("dotted keys"));
    assert!(
        s.effective().settings().context_follow_hover,
        "nested form is not applied"
    );

    s.set(Scope::Global, "hover.card", F).unwrap();
    let after = d.read("settings.json");
    assert_eq!(
        after,
        text.replace("\"hover.card\": true", "\"hover.card\": false")
    );
    assert_eq!(s.unknown_keys().len(), 2);
}

/// `index.auto_build` and `prepare.auto` were retired (manual start): a file that still has
/// them is told so, keeps them, and they change nothing; setting one says why it is gone.
#[test]
fn retired_keys_are_reported_as_retired_and_kept() {
    let d = Dir::new("retired");
    let text = "{\"index.auto_build\": true, \"prepare.auto\": true, \"hover.card\": true}";
    d.write("settings.json", text);
    let mut s = d.open();
    let unknown = s.unknown_keys();
    assert_eq!(unknown.len(), 2, "{unknown:?}");
    for u in &unknown {
        assert!(u.message.contains("was retired"), "{}", u.message);
    }
    assert!(!s.effective().settings().index_auto_refresh);
    let e = s.set(Scope::Global, "index.auto_build", T).unwrap_err();
    assert!(e.to_string().contains("was retired"), "{e}");
    s.set(Scope::Global, "hover.card", F).unwrap();
    assert!(d.read("settings.json").contains("\"prepare.auto\": true"));
}

#[test]
fn invalid_values_fall_back_one_layer_and_are_reported() {
    let d = Dir::new("invalid");
    d.write(
        "settings.json",
        "{\"hover.card\": \"no\", \"search.max_results\": 5, \"index.parallel_projects\": -1, \"hover.peek\": false}",
    );
    let s = d.open();
    let bad: Vec<_> = s
        .diagnostics()
        .iter()
        .filter(|x| x.kind == DiagnosticKind::InvalidValue)
        .filter_map(|x| x.key.clone())
        .collect();
    assert_eq!(
        bad,
        [
            "hover.card",
            "search.max_results",
            "index.parallel_projects"
        ]
    );
    assert!(s.effective().settings().hover_card);
    assert_eq!(s.effective().settings().search_max_results, 5000);
    assert!(
        !s.effective().settings().hover_peek,
        "valid keys still apply"
    );
}

#[test]
fn malformed_file_is_reported_and_last_good_settings_stay() {
    let d = Dir::new("malformed");
    d.write("settings.json", "{\"hover.card\": false}");
    let mut s = d.open();
    assert!(!s.effective().settings().hover_card);

    d.write("settings.json", "{\"hover.card\": tru");
    let change = s.reload_if_changed().expect("file changed");
    assert!(change.keys.is_empty() && change.diagnostics_changed);
    assert!(
        !s.effective().settings().hover_card,
        "last good settings in force"
    );
    assert!(
        matches!(s.file_status(FileKind::Settings), FileStatus::Malformed(m) if m.contains("line 1"))
    );
    let diags = s.diagnostics();
    assert_eq!(diags.len(), 1);
    assert_eq!(diags[0].kind, DiagnosticKind::Malformed);

    // Saving refuses to clobber the broken file.
    let err = s.set(Scope::Global, "hover.peek", F).unwrap_err();
    assert!(matches!(err, SaveError::Malformed(_)));
    assert_eq!(d.read("settings.json"), "{\"hover.card\": tru");

    // Fixing the file recovers, and the report clears.
    d.write(
        "settings.json",
        "{\"hover.card\": true, \"hover.peek\": false}",
    );
    let change = s.reload_if_changed().unwrap();
    assert!(change.contains("hover.card") && change.contains("hover.peek"));
    assert!(s.diagnostics().is_empty());
    assert_eq!(s.file_status(FileKind::Settings), &FileStatus::Ok);
}

#[test]
fn malformed_at_startup_falls_back_to_defaults() {
    let d = Dir::new("malformed-start");
    d.write("settings.json", "not json at all");
    let s = d.open();
    assert_eq!(s.effective().settings(), &Settings::default());
    assert_eq!(s.diagnostics()[0].kind, DiagnosticKind::Malformed);
    let d2 = Dir::new("not-object");
    d2.write("settings.json", "[1, 2]");
    assert_eq!(d2.open().diagnostics()[0].kind, DiagnosticKind::Malformed);
}

#[test]
fn save_changes_only_the_changed_key() {
    let d = Dir::new("save");
    let text = "// my settings\n{\n    \"hover.card\": false,   // distracting\n\n    // keep peeks\n    \"hover.peek\": true\n}\n";
    d.write("settings.json", text);
    let mut s = d.open();

    let c = s.set(Scope::Global, "hover.peek", F).unwrap();
    assert_eq!(c.keys, ["hover.peek"]);
    assert_eq!(c.version, 1);
    assert_eq!(
        d.read("settings.json"),
        text.replace("\"hover.peek\": true", "\"hover.peek\": false")
    );

    // A new key goes at the end, in the file's own indentation, comments intact.
    s.set(Scope::Global, "search.max_results", Value::UInt(900))
        .unwrap();
    let after = d.read("settings.json");
    assert!(
        after.contains("    \"search.max_results\": 900\n}"),
        "{after}"
    );
    assert!(
        after.contains("// distracting")
            && after.contains("// keep peeks")
            && after.starts_with("// my settings")
    );
    assert_eq!(s.effective().settings().search_max_results, 900);

    // Removing takes the key back to its default.
    let c = s.unset(Scope::Global, "search.max_results").unwrap();
    assert_eq!(c.keys, ["search.max_results"]);
    assert_eq!(s.effective().settings().search_max_results, 5000);
    assert!(!d.read("settings.json").contains("search.max_results"));
    assert!(jsonc::parse(&d.read("settings.json")).is_ok());
}

#[test]
fn saving_creates_directory_and_file() {
    let d = Dir::new("create");
    let mut s = d.open();
    assert!(!d.cfg().exists());
    s.set(Scope::Global, "graph.enabled", F).unwrap();
    assert_eq!(
        d.read("settings.json"),
        "{\n  \"graph.enabled\": false\n}\n"
    );
    assert!(!d.open().effective().settings().graph_enabled);
}

#[test]
fn setting_the_same_value_writes_nothing_and_changes_nothing() {
    let d = Dir::new("noop");
    d.write("settings.json", "{\"hover.card\":false}");
    let mut s = d.open();
    let c = s.set(Scope::Global, "hover.card", F).unwrap();
    assert!(c.is_empty() && c.version == 0);
    assert_eq!(d.read("settings.json"), "{\"hover.card\":false}");
}

#[test]
fn bad_saves_are_refused() {
    let d = Dir::new("refuse");
    let mut s = d.open();
    assert_eq!(
        s.set(Scope::Global, "nope", T),
        Err(SaveError::UnknownKey("nope".into()))
    );
    assert!(matches!(
        s.set(Scope::Global, "hover.card", Value::UInt(1)),
        Err(SaveError::InvalidValue(_))
    ));
    assert!(matches!(
        s.set(Scope::Global, "search.max_results", Value::UInt(1)),
        Err(SaveError::InvalidValue(_))
    ));
    assert!(!d.cfg().exists(), "nothing written");
    assert_eq!(
        SettingsStore::in_memory().set(Scope::Global, "hover.card", F),
        Err(SaveError::NoConfigDir)
    );
}

#[test]
fn repo_overrides_sit_between_global_and_lightweight() {
    let d = Dir::new("repo");
    let (a, b) = (d.repo("a"), d.repo("b"));
    d.write(
        "settings.json",
        "{\"index.auto_refresh\": false, \"hover.card\": false}",
    );
    let mut s = d.open();

    s.set_active_repo(Some(&a)).unwrap();
    let c = s.set(Scope::Repo(&a), "index.auto_refresh", T).unwrap();
    assert_eq!(c.keys, ["index.auto_refresh"]);
    assert_eq!(src(&s, "index.auto_refresh"), Source::Repo);
    assert_eq!(src(&s, "hover.card"), Source::Global);

    // Another repository is unaffected.
    let c = s.set_active_repo(Some(&b)).unwrap();
    assert_eq!(c.keys, ["index.auto_refresh"]);
    assert_eq!(src(&s, "index.auto_refresh"), Source::Global);
    // And no repository at all.
    s.set_active_repo(None).unwrap();
    assert!(!s.effective().settings().index_auto_refresh);

    // Stored in the config dir, keyed by canonical path, never in the repository.
    let stored = d.read("repos.json");
    assert!(stored.contains(&repo_key(&a).unwrap()), "{stored}");
    assert_eq!(std::fs::read_dir(&a).unwrap().count(), 0);
    assert_eq!(
        d.read("settings.json"),
        "{\"index.auto_refresh\": false, \"hover.card\": false}"
    );

    // Survives a reopen.
    let mut s2 = d.open();
    s2.set_active_repo(Some(&a)).unwrap();
    assert_eq!(src(&s2, "index.auto_refresh"), Source::Repo);

    // Unset removes the override and tidies the empty entry.
    s2.unset(Scope::Repo(&a), "index.auto_refresh").unwrap();
    assert_eq!(src(&s2, "index.auto_refresh"), Source::Global);
    assert!(!d.read("repos.json").contains(&repo_key(&a).unwrap()));
}

#[test]
fn repo_paths_are_canonical_so_aliases_agree() {
    let d = Dir::new("canon");
    let a = d.repo("a");
    let alias = a.join("..").join("a");
    assert_eq!(repo_key(&a).unwrap(), repo_key(&alias).unwrap());
    let mut s = d.open();
    s.set(Scope::Repo(&alias), "hover.peek", F).unwrap();
    s.set_active_repo(Some(&a)).unwrap();
    assert_eq!(src(&s, "hover.peek"), Source::Repo);
    assert!(matches!(
        s.set_active_repo(Some(Path::new("/definitely/not/here"))),
        Err(SaveError::BadRepo(_))
    ));
    assert!(matches!(
        s.set(
            Scope::Repo(Path::new("/definitely/not/here")),
            "hover.peek",
            F
        ),
        Err(SaveError::BadRepo(_))
    ));
}

#[test]
fn repos_file_problems_are_reported() {
    let d = Dir::new("repos-diag");
    d.write(
        "repos.json",
        "{\"note\": 1, \"repos\": {\"/r\": {\"hover.card\": false, \"zzz\": 1, \"hover.peek\": 3}, \"/s\": 4}}",
    );
    let s = d.open();
    let d = s.diagnostics();
    assert!(
        d.iter()
            .any(|x| x.kind == DiagnosticKind::UnknownKey && x.key.as_deref() == Some("note"))
    );
    assert!(d.iter().any(|x| x.kind == DiagnosticKind::UnknownKey
        && x.key.as_deref() == Some("zzz")
        && x.repo.as_deref() == Some("/r")));
    assert!(
        d.iter()
            .any(|x| x.kind == DiagnosticKind::InvalidValue
                && x.key.as_deref() == Some("hover.peek"))
    );
    assert!(
        d.iter()
            .any(|x| x.kind == DiagnosticKind::InvalidValue && x.repo.as_deref() == Some("/s"))
    );
    assert!(d.iter().all(|x| x.file == FileKind::Repos));

    // A malformed repos.json keeps the last good overrides.
    let dd = Dir::new("repos-malformed");
    let r = dd.repo("r");
    let mut s = dd.open();
    s.set(Scope::Repo(&r), "hover.card", F).unwrap();
    s.set_active_repo(Some(&r)).unwrap();
    dd.write("repos.json", "{ broken");
    s.reload_if_changed().unwrap();
    assert!(!s.effective().settings().hover_card);
    assert!(matches!(
        s.file_status(FileKind::Repos),
        FileStatus::Malformed(_)
    ));
}

#[test]
fn lightweight_round_trip_through_the_file() {
    let d = Dir::new("lightweight");
    d.write(
        "settings.json",
        "{\"hover.peek\": false, \"graph.enabled\": true}",
    );
    let mut s = d.open();
    let before = s.effective().clone();

    let c = s.set(Scope::Global, "lightweight", T).unwrap();
    assert!(s.effective().lightweight());
    assert!(
        c.contains("lightweight")
            && c.contains("changes.auto_refresh")
            && c.contains("graph.enabled")
    );
    assert!(!c.contains("hover.peek"), "was already off");
    assert!(!c.contains("search.max_results"));
    for (def, e) in s.effective().iter().filter(|(d, _)| d.lightweight_off) {
        assert_eq!(e.value, def.off_value(), "{}", def.key);
        assert_eq!(e.source, Source::Lightweight);
    }
    // Individual values are still in the file, untouched.
    assert!(d.read("settings.json").contains("\"graph.enabled\": true"));

    let c = s.set(Scope::Global, "lightweight", F).unwrap();
    assert!(c.contains("lightweight"));
    assert_eq!(s.effective().settings(), before.settings());
    // Every source is back to what it was, except that `lightweight` is now explicitly false.
    for ((d, now), (_, was)) in s.effective().iter().zip(before.iter()) {
        if d.key != "lightweight" {
            assert_eq!(
                (now.value.clone(), now.source),
                (was.value.clone(), was.source),
                "{}",
                d.key
            );
        }
    }
}

#[test]
fn version_counts_effective_changes_only() {
    let d = Dir::new("version");
    let mut s = d.open();
    assert_eq!(s.version(), 0);
    s.set(Scope::Global, "hover.card", F).unwrap();
    assert_eq!(s.version(), 1);
    s.set(Scope::Global, "hover.card", F).unwrap();
    assert_eq!(s.version(), 1);
    // A repo override that matches the global value changes nothing visible.
    let r = d.repo("r");
    s.set_active_repo(Some(&r)).unwrap();
    let c = s.set(Scope::Repo(&r), "hover.card", F).unwrap();
    assert!(c.keys.is_empty());
    assert_eq!(s.version(), 1);
    s.set(Scope::Repo(&r), "hover.card", T).unwrap();
    assert_eq!(s.version(), 2);
}

#[test]
fn external_edits_are_picked_up_and_own_saves_are_not_reported_twice() {
    let d = Dir::new("watch");
    d.write("settings.json", "{\"hover.card\": false}");
    let mut s = d.open();
    assert_eq!(s.reload_if_changed(), None, "nothing changed");

    s.set(Scope::Global, "hover.peek", F).unwrap();
    assert_eq!(
        s.reload_if_changed(),
        None,
        "our own write is already accounted for"
    );

    // An external edit (different length so the fingerprint differs even on coarse clocks).
    d.write(
        "settings.json",
        "{\"hover.card\": true, \"highlight.exact\": false}",
    );
    let c = s.reload_if_changed().unwrap();
    assert_eq!(c.keys, ["hover.card", "hover.peek", "highlight.exact"]);
    assert_eq!(s.version(), 2, "one bump per change set");
    assert_eq!(s.reload_if_changed(), None);

    // Deleting the file returns to defaults.
    std::fs::remove_file(d.cfg().join("settings.json")).unwrap();
    let c = s.reload_if_changed().unwrap();
    assert!(c.contains("highlight.exact"));
    assert_eq!(s.effective().settings(), &Settings::default());
}

#[test]
fn every_effective_value_has_an_explanation() {
    let d = Dir::new("explain");
    d.write(
        "settings.json",
        "{\"lightweight\": true, \"hover.card\": false}",
    );
    let s = d.open();
    for (def, e) in s.effective().iter() {
        let text = e.explanation();
        assert!(text.contains(&e.value.to_string()), "{}: {text}", def.key);
    }
    assert!(
        s.effective()
            .entry("hover.card")
            .unwrap()
            .explanation()
            .contains("global")
    );
    assert!(
        s.effective()
            .entry("search.max_results")
            .unwrap()
            .explanation()
            .contains("default")
    );
}

#[test]
fn text_setting_round_trips_and_a_number_is_invalid() {
    let d = Dir::new("font-text");
    let mut s = d.open();
    s.set(Scope::Global, "appearance.ui_font", Value::text("Menlo"))
        .unwrap();
    assert!(
        d.read("settings.json")
            .contains("\"appearance.ui_font\": \"Menlo\"")
    );
    let s = d.open();
    assert_eq!(
        s.effective().entry("appearance.ui_font").unwrap().value,
        Value::text("Menlo")
    );

    d.write("settings.json", "{\"appearance.ui_font\": 12}");
    let s = d.open();
    assert!(s.diagnostics().iter().any(|x| {
        x.kind == DiagnosticKind::InvalidValue && x.key.as_deref() == Some("appearance.ui_font")
    }));
}
