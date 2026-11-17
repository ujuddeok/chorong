#![no_main]
//! Every file Chorong saves and loads back (apart from layouts: see `layout_persist`). They can
//! be stale, truncated, from another version or hand-edited. The first byte picks the file:
//!
//! - 0 `state.json` beside each bare clone (`RepoState`)
//! - 1 `projects.json` (`Projects`)
//! - 2 `recent.json` (`recent::list`, `recent::remember`)
//! - 3 `meta.json` in the index cache (`Meta`, `IndexCache::projects`)
//! - 4 `settings.json` (`SettingsStore`)
//! - 5 `repos.json` (`SettingsStore`, per-repository overrides)
//!
//! Any text loads without panicking. It either errors cleanly or gives a value that respects
//! the module's own rules, whose own save loads back to an equal value, and saving that again
//! gives the same bytes.
use libfuzzer_sys::fuzz_target;
use cho_semantic::cache::{GitState, IndexCache, status_of};
use cho_semantic::{IndexStatus, Meta};
use cho_settings::{Scope, SettingsStore, def, defs};
use cho_sources::projects::Projects;
use cho_sources::state::{RepoState, WorktreeState};
use cho_sources::{Project, recent};
use std::path::{Path, PathBuf};

fn scratch(name: &str) -> PathBuf {
    let d = std::env::temp_dir()
        .join(format!("cho-fuzz-persist-{}", std::process::id()))
        .join(name);
    std::fs::create_dir_all(&d).expect("scratch dir");
    d
}

fn read(p: &Path) -> String {
    #[allow(clippy::disallowed_methods)]
    std::fs::read_to_string(p).expect("read back")
}

fn repo_state(text: &str) {
    let dir = scratch("state");
    let (p1, p2, p3) = (dir.join("a.json"), dir.join("b.json"), dir.join("c.json"));
    std::fs::write(&p1, text).unwrap();
    let s = RepoState::load(&p1);

    for slug in s.worktrees.keys() {
        assert!(
            !slug.is_empty() && !slug.contains(['/', '\\', '\0']) && !slug.starts_with('.'),
            "worktree slug {slug:?} is not a folder name"
        );
    }
    let top = s.worktrees.values().map(|w| w.seq).max().unwrap_or(0);
    assert!(
        s.seq >= top,
        "state seq {} behind a worktree's {top}",
        s.seq
    );

    s.save(&p2).unwrap();
    let again = RepoState::load(&p2);
    assert_eq!(again, s, "save/load changed the state");
    again.save(&p3).unwrap();
    assert_eq!(read(&p2), read(&p3), "save is not stable");

    // Eviction never picks `keep`, a default branch or an unknown slug, and takes as many as
    // it can (but only as many as needed).
    let keep = s
        .worktrees
        .keys()
        .next()
        .cloned()
        .unwrap_or_else(|| "x".into());
    for cap in 0..4 {
        let ev = s.evictions(&keep, cap);
        for e in &ev {
            let w = s.worktrees.get(e).expect("evicts a known worktree");
            assert!(!w.default_branch && *e != keep, "evicted {e}");
        }
        let mut sorted = ev.clone();
        sorted.dedup();
        assert_eq!(sorted.len(), ev.len(), "evicts a worktree twice");
        let others = s.worktrees.keys().filter(|k| **k != keep).count();
        let removable = s
            .worktrees
            .iter()
            .filter(|(k, w)| **k != keep && !w.default_branch)
            .count();
        assert_eq!(
            ev.len(),
            (others + 1).saturating_sub(cap).min(removable),
            "cap {cap}"
        );
    }

    // A newly opened worktree is the most recent.
    let mut t = s.clone();
    t.touch("zz-new", WorktreeState::default());
    if top < u64::MAX {
        let seq = t.worktrees["zz-new"].seq;
        assert!(
            t.worktrees
                .iter()
                .all(|(k, w)| k == "zz-new" || w.seq < seq)
        );
    }
}

fn projects(text: &str) {
    let dir = scratch("projects");
    let p = dir.join("projects.json");
    std::fs::write(&p, text).unwrap();
    let Ok(mut pr) = Projects::open(&p) else {
        return;
    };
    let listed: Vec<(cho_core::RepoRef, Project)> =
        pr.list().into_iter().map(|(r, p)| (r, p.clone())).collect();
    for (repo, proj) in &listed {
        assert!(!matches!(repo, cho_core::RepoRef::Local { .. }), "{repo}");
        assert_eq!(
            pr.get(repo),
            Some(proj),
            "`{repo}` is listed but not found by its identity"
        );
        // What `insert` insists on, a loaded project meets too.
        if let Some(u) = &proj.url {
            cho_core::git::check_remote_url(u).unwrap_or_else(|e| panic!("{repo}: url: {e}"));
        }
        if let Some(r) = &proj.recipe {
            r.validate()
                .unwrap_or_else(|e| panic!("{repo}: recipe: {e}"));
        }
        assert!(proj.worktree_cap >= 1, "{repo}: worktree cap 0");
    }
    // Saving (insert writes the whole file) and loading back.
    for (repo, proj) in &listed {
        assert!(
            !pr.insert(repo, proj.clone()).expect("re-insert"),
            "{repo} was new"
        );
    }
    let extra = cho_core::RepoRef::parse("github:fuzz/extra").unwrap();
    pr.insert(&extra, Project::default()).expect("insert");
    let saved = read(&p);
    let back = Projects::open(&p).expect("own save loads");
    let mut want = listed.clone();
    want.push((extra.clone(), Project::default()));
    want.sort_by_key(|(r, _)| r.to_string());
    let got: Vec<_> = back
        .list()
        .into_iter()
        .map(|(r, p)| (r, p.clone()))
        .collect();
    assert_eq!(got, want, "save/load changed the projects");
    let mut back = back;
    back.insert(&extra, Project::default()).unwrap();
    assert_eq!(read(&p), saved, "save is not stable");
    assert!(back.remove(&extra).unwrap());
}

fn recent_list(text: &str, tag: u8) {
    let dir = scratch("recent");
    // Safety: the fuzz target is single-threaded.
    unsafe { std::env::set_var("CHORONG_CONFIG_DIR", &dir) };
    let file = dir.join("recent.json");
    std::fs::write(&file, text).unwrap();
    let before = recent::list();
    let mut uniq = before.clone();
    uniq.dedup();
    let distinct: std::collections::HashSet<_> = before.iter().collect();
    assert_eq!(
        distinct.len(),
        before.len(),
        "a folder is listed twice: {uniq:?}"
    );
    let folder = dir.join(format!("f{}", tag % 3));
    std::fs::create_dir_all(&folder).unwrap();
    recent::remember(&folder).expect("remember");
    let key = folder.canonicalize().unwrap();
    let after = recent::list();
    assert_eq!(after.first(), Some(&key), "the folder is not first");
    assert!(after.len() <= 40, "{} remembered", after.len());
    let mut rest: Vec<PathBuf> = before.iter().filter(|p| **p != key).cloned().collect();
    rest.truncate(39);
    assert_eq!(
        &after[1..],
        &rest[..],
        "the others changed order or went missing"
    );
    let saved = read(&file);
    recent::remember(&folder).expect("remember again");
    assert_eq!(read(&file), saved, "remember is not stable");
    assert_eq!(recent::list(), after);
}

fn meta(text: &str, through_cache: bool) {
    let dir = scratch("meta");
    if through_cache {
        // The cache loader itself: `<base>/<repo id>/<project>/meta.json`.
        let root = dir.canonicalize().unwrap();
        let cache = IndexCache::at(dir.join("indexes"));
        let proj = cache.repo_dir(&root).join("p");
        std::fs::create_dir_all(&proj).unwrap();
        std::fs::write(proj.join("meta.json"), text).unwrap();
        let entries = cache.projects(&root);
        let parsed = serde_json::from_str::<Meta>(text);
        assert_eq!(entries.len(), usize::from(parsed.is_ok()));
        if let (Some(e), Ok(m)) = (entries.first(), &parsed) {
            assert_eq!(&e.meta, m);
            assert!(!e.has_index);
            // Failed runs are failed, whatever HEAD says.
            assert_eq!(e.status == IndexStatus::Failed, !m.success());
        }
        return;
    }
    let Ok(m) = serde_json::from_str::<Meta>(text) else {
        return;
    };
    let proj = dir.join("p");
    std::fs::create_dir_all(&proj).unwrap();
    IndexCache::write_meta(&proj, &m).expect("write_meta");
    let saved = read(&proj.join("meta.json"));
    let back: Meta = serde_json::from_str(&saved).expect("own save loads");
    assert_eq!(back, m, "save/load changed the record");
    IndexCache::write_meta(&proj, &back).unwrap();
    assert_eq!(read(&proj.join("meta.json")), saved, "save is not stable");
    // Status against any repository state never panics and follows the record.
    let states = [
        GitState {
            head: None,
            status_hash: None,
        },
        GitState {
            head: m.git_head.clone(),
            status_hash: m.git_status_hash.clone(),
        },
        GitState {
            head: Some("x".into()),
            status_hash: Some("y".into()),
        },
    ];
    for now in &states {
        let st = status_of(&m, now);
        assert_eq!(st == IndexStatus::Failed, !m.success());
        if st == IndexStatus::Current {
            assert!(m.success() && m.prepare.is_none() && m.git_head == now.head);
        }
    }
}

/// Settings and per-repository overrides.
fn settings(text: &str, repos: bool) {
    let dir = scratch(if repos { "repos" } else { "settings" });
    let repo = dir.join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    let canon = repo.canonicalize().unwrap();
    let (file, other) = if repos {
        ("repos.json", "settings.json")
    } else {
        ("settings.json", "repos.json")
    };
    let _ = std::fs::remove_file(dir.join(other));
    // In a repos file, U+0001 stands for this repository's key.
    let q = cho_settings::jsonc::quote(&canon.to_string_lossy());
    let text = text.replace('\u{1}', &q[1..q.len() - 1]);
    std::fs::write(dir.join(file), &text).unwrap();

    let mut s = SettingsStore::open(Some(dir.clone()));
    s.set_active_repo(Some(&repo)).unwrap();
    // Every effective value is allowed for its setting.
    for (d, e) in s.effective().iter() {
        d.kind
            .check(&e.value)
            .unwrap_or_else(|why| panic!("{}: {why}", d.key));
    }
    for d in defs() {
        for v in [s.global_value(d.key), s.repo_value(d.key)]
            .into_iter()
            .flatten()
        {
            d.kind
                .check(&v)
                .unwrap_or_else(|why| panic!("{} layer: {why}", d.key));
        }
    }
    if s.file_status(if repos {
        cho_settings::FileKind::Repos
    } else {
        cho_settings::FileKind::Settings
    })
    .as_str()
        != "ok"
    {
        return; // a broken file is left alone: covered by `settings_jsonc`
    }

    // Writing every explicit value back changes none of them, and doing it twice changes
    // nothing more.
    let scope = if repos {
        Scope::Repo(&repo)
    } else {
        Scope::Global
    };
    let get = |s: &SettingsStore, k: &str| {
        if repos {
            s.repo_value(k)
        } else {
            s.global_value(k)
        }
    };
    let set: Vec<_> = defs()
        .iter()
        .filter_map(|d| get(&s, d.key).map(|v| (d.key, v)))
        .collect();
    for (k, v) in &set {
        s.set(scope, k, v.clone())
            .unwrap_or_else(|e| panic!("set {k}: {e}\n{text:?}"));
    }
    let once = read(&dir.join(file));
    for (k, v) in &set {
        s.set(scope, k, v.clone()).unwrap();
    }
    assert_eq!(read(&dir.join(file)), once, "saving again changed the file");
    let mut back = SettingsStore::open(Some(dir.clone()));
    back.set_active_repo(Some(&repo)).unwrap();
    for d in defs() {
        assert_eq!(
            get(&back, d.key),
            get(&s, d.key),
            "{} after reload\n{text:?}\n{once:?}",
            d.key
        );
        if get(&s, d.key).is_some() {
            assert!(def(d.key).is_some());
        }
    }
    assert_eq!(back.effective().lightweight(), s.effective().lightweight());
}

fuzz_target!(|data: &[u8]| {
    let Some((&which, rest)) = data.split_first() else {
        return;
    };
    let Ok(text) = std::str::from_utf8(rest) else {
        return;
    };
    match which % 6 {
        0 => repo_state(text),
        1 => projects(text),
        2 => recent_list(text, which / 6),
        3 => meta(text, which / 6 % 16 == 0),
        4 => settings(text, false),
        _ => settings(text, true),
    }
});
