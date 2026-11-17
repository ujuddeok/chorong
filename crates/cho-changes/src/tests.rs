use super::*;
use cho_core::test_repo;
use tempfile::TempDir;

/// A scratch repository built with the `git` command; each `commit` writes and removes files
/// and returns the new commit id.
struct Repo {
    dir: TempDir,
}

impl Repo {
    fn new() -> Self {
        let r = Self {
            dir: tempfile::tempdir().unwrap(),
        };
        test_repo::init(r.root());
        r
    }

    fn root(&self) -> &Path {
        self.dir.path()
    }

    fn git(&self, args: &[&str]) -> String {
        test_repo::git(self.root(), args)
    }

    fn commit(&self, write: &[(&str, &str)], remove: &[&str]) -> String {
        for (path, text) in write {
            let p = self.root().join(path);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, text).unwrap();
        }
        for path in remove {
            std::fs::remove_file(self.root().join(path)).unwrap();
        }
        test_repo::commit_all(self.root(), "c")
    }
}

fn file<'a>(c: &'a ChangeSet, path: &str) -> &'a FileChanges {
    c.files
        .iter()
        .find(|f| f.path == path)
        .unwrap_or_else(|| panic!("{path} not in {:#?}", c.files))
}

/// `(status, qualified name, kind)` for each symbol of a file.
fn symbols(f: &FileChanges) -> Vec<(&'static str, String, &'static str)> {
    f.symbols
        .iter()
        .map(|s| (s.status.as_str(), s.qualified_name.clone(), s.kind.as_str()))
        .collect()
}

const LIB: &str = "\
pub struct Queue {
    items: Vec<u32>,
}

impl Queue {
    pub fn push(&mut self, x: u32) {
        self.items.push(x);
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }
}

fn helper() -> u32 {
    1
}
";

#[test]
fn added_changed_and_removed_declarations() {
    let repo = Repo::new();
    let base = repo.commit(&[("src/lib.rs", LIB)], &[]);
    let edited = LIB
        .replace("self.items.push(x);", "self.items.push(x + 1);")
        .replace("fn helper() -> u32 {\n    1\n}\n", "")
        .replace(
            "    pub fn len(&self) -> usize {",
            "    pub fn is_empty(&self) -> bool {\n        self.items.is_empty()\n    }\n\n    pub fn len(&self) -> usize {",
        );
    let head = repo.commit(&[("src/lib.rs", &edited)], &[]);
    let c = compare(repo.root(), &base, &head).unwrap();
    assert_eq!(
        (c.base.as_str(), c.head.as_str()),
        (base.as_str(), head.as_str())
    );
    let f = file(&c, "src/lib.rs");
    assert_eq!(
        (f.status, f.level, f.language),
        (FileStatus::Modified, Level::Symbol, Some(Language::Rust))
    );
    assert_eq!(
        symbols(f),
        vec![
            ("changed", "Queue::push".into(), "method"),
            ("added", "Queue::is_empty".into(), "method"),
            ("removed", "helper".into(), "function"),
        ]
    );
    // Unchanged neighbours are not reported, and the struct is untouched.
    assert!(!f.outside_declarations);
    let push = &f.symbols[0];
    assert_eq!(push.base.len(), 1);
    assert_eq!(push.head.len(), 1);
    let h = &push.head[0];
    assert!(edited[h.range[0]..h.range[1]].starts_with("pub fn push"));
    assert_eq!((h.start_line, h.end_line), (6, 8));
    let removed = &f.symbols[2];
    assert!(removed.head.is_empty());
    let b = &removed.base[0];
    assert!(LIB[b.range[0]..b.range[1]].starts_with("fn helper"));
    assert!(b.touched);
}

#[test]
fn added_and_deleted_files_list_every_declaration() {
    let repo = Repo::new();
    let base = repo.commit(&[("a.py", "def old():\n    return 1\n")], &[]);
    let head = repo.commit(
        &[(
            "b.ts",
            "export class Svc {\n  run() {\n    return 1;\n  }\n}\n",
        )],
        &["a.py"],
    );
    let c = compare(repo.root(), &base, &head).unwrap();
    let a = file(&c, "a.py");
    assert_eq!(a.status, FileStatus::Deleted);
    assert_eq!(
        (a.path_at(Side::Base), a.path_at(Side::Head)),
        (Some("a.py"), None)
    );
    assert_eq!(symbols(a), vec![("removed", "old".into(), "function")]);
    let b = file(&c, "b.ts");
    assert_eq!(b.status, FileStatus::Added);
    assert_eq!(
        symbols(b),
        vec![
            ("added", "Svc".into(), "class"),
            ("added", "Svc.run".into(), "method"),
        ]
    );
    assert_eq!(b.hunks.len(), 1);
    assert_eq!(b.hunks[0].base.line_count, 0);
    assert_eq!(b.hunks[0].head.line_count, 5);
}

#[test]
fn renames_keep_symbols_compared_across_paths() {
    let repo = Repo::new();
    let base = repo.commit(&[("old.rs", LIB), ("same.rs", "fn a() {}\n")], &[]);
    let head = repo.commit(
        &[
            ("new.rs", &LIB.replace("    1\n", "    2\n")),
            ("moved/same.rs", "fn a() {}\n"),
        ],
        &["old.rs", "same.rs"],
    );
    let c = compare(repo.root(), &base, &head).unwrap();
    let exact = file(&c, "moved/same.rs");
    assert_eq!(
        (exact.status, exact.old_path.as_deref(), exact.similarity),
        (FileStatus::Renamed, Some("same.rs"), Some(100))
    );
    assert!(exact.symbols.is_empty() && exact.hunks.is_empty());
    let edited = file(&c, "new.rs");
    assert_eq!(edited.status, FileStatus::Renamed);
    assert_eq!(edited.old_path.as_deref(), Some("old.rs"));
    assert!(edited.similarity.unwrap() >= 50);
    assert_eq!(
        symbols(edited),
        vec![("changed", "helper".into(), "function")]
    );
    assert_eq!(c.files.len(), 2);
}

#[test]
fn a_function_moved_between_files_is_removed_from_one_and_added_to_the_other() {
    let repo = Repo::new();
    let base = repo.commit(
        &[
            (
                "a.rs",
                "fn keep() {}\n\nfn wander(x: u32) -> u32 {\n    x * 2\n}\n",
            ),
            ("b.rs", "fn other() {}\n"),
        ],
        &[],
    );
    let head = repo.commit(
        &[
            ("a.rs", "fn keep() {}\n"),
            (
                "b.rs",
                "fn other() {}\n\nfn wander(x: u32) -> u32 {\n    x * 2\n}\n",
            ),
        ],
        &[],
    );
    let c = compare(repo.root(), &base, &head).unwrap();
    assert_eq!(
        symbols(file(&c, "a.rs")),
        vec![("removed", "wander".into(), "function")]
    );
    assert_eq!(
        symbols(file(&c, "b.rs")),
        vec![("added", "wander".into(), "function")]
    );
}

#[test]
fn unsupported_languages_and_binaries_are_file_level() {
    let repo = Repo::new();
    let base = repo.commit(&[("notes.md", "# Notes\n"), ("logo.bin", "a\0b")], &[]);
    let head = repo.commit(
        &[("notes.md", "# Notes\n\nMore.\n"), ("logo.bin", "a\0c")],
        &[],
    );
    let c = compare(repo.root(), &base, &head).unwrap();
    let md = file(&c, "notes.md");
    assert_eq!(
        (md.level, md.language),
        (Level::File(FileOnly::UnsupportedLanguage), None)
    );
    assert!(md.symbols.is_empty());
    assert_eq!(md.hunks.len(), 1);
    assert_eq!(
        (md.hunks[0].head.start_line, md.hunks[0].head.line_count),
        (2, 2)
    );
    let bin = file(&c, "logo.bin");
    assert_eq!(bin.level, Level::File(FileOnly::Binary));
    assert!(bin.hunks.is_empty());
    // The JSON names the status, kind, language, level and reason as strings.
    let json = |f: &FileChanges| {
        let v = serde_json::to_value(f).unwrap();
        ["status", "kind", "language", "level", "reason"].map(|k| v.get(k).cloned())
    };
    let s = |t: &str| Some(serde_json::Value::from(t));
    assert_eq!(
        json(md),
        [
            s("modified"),
            s("file"),
            Some(serde_json::Value::Null),
            s("file"),
            s("unsupported-language")
        ]
    );
    assert_eq!(json(bin)[3..], [s("file"), s("binary")]);
}

#[test]
fn several_declarations_with_one_name_are_ambiguous() {
    let repo = Repo::new();
    let two = "struct S;\n\nimpl std::fmt::Display for S {\n    fn fmt(&self) -> u8 {\n        1\n    }\n}\n\nimpl std::fmt::Debug for S {\n    fn fmt(&self) -> u8 {\n        2\n    }\n}\n";
    let base = repo.commit(&[("s.rs", two)], &[]);
    let head = repo.commit(&[("s.rs", &two.replace("        2\n", "        3\n"))], &[]);
    let c = compare(repo.root(), &base, &head).unwrap();
    let f = file(&c, "s.rs");
    assert_eq!(symbols(f), vec![("ambiguous", "S::fmt".into(), "method")]);
    let s = &f.symbols[0];
    assert_eq!((s.base.len(), s.head.len()), (2, 2));
    assert_eq!(
        s.head.iter().map(|d| d.touched).collect::<Vec<_>>(),
        vec![false, true]
    );
}

#[test]
fn adding_a_member_does_not_change_its_container() {
    let repo = Repo::new();
    let py =
        "class Svc:\n    def a(self):\n        return 1\n\n    def c(self):\n        return 3\n";
    let rs = "mod tests {\n    #[test]\n    fn a() {}\n}\n";
    let base = repo.commit(&[("svc.py", py), ("t.rs", rs)], &[]);
    let head = repo.commit(
        &[
            (
                "svc.py",
                &py.replace(
                    "    def c",
                    "    @staticmethod\n    def b():\n        return 2\n\n    def c",
                ),
            ),
            (
                "t.rs",
                &rs.replace(
                    "    fn a() {}\n",
                    "    fn a() {}\n\n    #[test]\n    fn b() {}\n",
                ),
            ),
        ],
        &[],
    );
    let c = compare(repo.root(), &base, &head).unwrap();
    assert_eq!(
        symbols(file(&c, "svc.py")),
        vec![("added", "Svc.b".into(), "method")]
    );
    assert_eq!(
        symbols(file(&c, "t.rs")),
        vec![("added", "tests::b".into(), "method")]
    );
    let removed = compare(repo.root(), &head, &base).unwrap();
    assert_eq!(
        symbols(file(&removed, "svc.py")),
        vec![("removed", "Svc.b".into(), "method")]
    );
}

#[test]
fn imports_and_doc_comments_are_attributed_deterministically() {
    let repo = Repo::new();
    let base = repo.commit(&[("m.rs", "use a::b;\n\n/// Old.\nfn f() {}\n")], &[]);
    let head = repo.commit(&[("m.rs", "use a::c;\n\n/// New.\nfn f() {}\n")], &[]);
    let c = compare(repo.root(), &base, &head).unwrap();
    let f = file(&c, "m.rs");
    assert!(f.outside_declarations);
    assert_eq!(symbols(f), vec![("changed", "f".into(), "function")]);
}

#[test]
fn merge_base_is_computed_from_the_target() {
    let repo = Repo::new();
    let root = repo.commit(&[("a.rs", "fn a() {}\n")], &[]);
    repo.git(&["checkout", "-q", "-b", "topic"]);
    let head = repo.commit(&[("a.rs", "fn a() {}\nfn b() {}\n")], &[]);
    repo.git(&["checkout", "-q", "main"]);
    repo.commit(&[("c.rs", "fn c() {}\n")], &[]);
    let c = compare_with_merge_base(repo.root(), "main", "topic").unwrap();
    assert_eq!(
        (c.base.as_str(), c.head.as_str()),
        (root.as_str(), head.as_str())
    );
    let mb = c.merge_base.as_ref().unwrap();
    assert_eq!(
        (mb.target.as_str(), mb.candidates.clone()),
        ("main", vec![root])
    );
    // c.rs is on the target only, so it is not part of the change.
    assert_eq!(
        c.files.iter().map(|f| f.path.as_str()).collect::<Vec<_>>(),
        vec!["a.rs"]
    );
    assert!(c.notes.is_empty());
}

#[test]
fn the_checkout_is_never_read() {
    let repo = Repo::new();
    let base = repo.commit(&[("a.rs", "fn a() {}\n")], &[]);
    let head = repo.commit(&[("a.rs", "fn a() { 1; }\n")], &[]);
    std::fs::write(repo.root().join("a.rs"), "garbage that does not parse {{{").unwrap();
    std::fs::write(repo.root().join("untracked.rs"), "fn u() {}\n").unwrap();
    let c = compare(repo.root(), &base, &head).unwrap();
    assert_eq!(c.files.len(), 1);
    assert_eq!(
        symbols(&c.files[0]),
        vec![("changed", "a".into(), "function")]
    );
    assert_eq!(repo.git(&["status", "--porcelain"]).lines().count(), 2);
}

#[test]
fn output_is_deterministic_and_serialises() {
    let repo = Repo::new();
    let base = repo.commit(&[("src/lib.rs", LIB)], &[]);
    let head = repo.commit(&[("src/lib.rs", &LIB.replace("    1\n", "    2\n"))], &[]);
    let a = serde_json::to_string(&compare(repo.root(), &base, &head).unwrap()).unwrap();
    let b = serde_json::to_string(&compare(repo.root(), "HEAD~1", "HEAD").unwrap()).unwrap();
    assert_eq!(a, b);
    let v: serde_json::Value = serde_json::from_str(&a).unwrap();
    assert_eq!(v["version"], 1);
    let f = &v["files"][0];
    assert_eq!(
        [&f["status"], &f["kind"], &f["language"], &f["level"]],
        ["modified", "file", "Rust", "symbol"]
    );
    assert!(f.get("reason").is_none());
    assert_eq!(f["symbols"][0]["status"], "changed");
    assert_eq!(f["symbols"][0]["kind"], "function");
    assert!(v.get("merge_base").is_none());
}

#[test]
fn unknown_revisions_are_errors() {
    let repo = Repo::new();
    repo.commit(&[("a.rs", "fn a() {}\n")], &[]);
    assert!(
        compare(repo.root(), "nope", "HEAD")
            .unwrap_err()
            .contains("nope")
    );
}

#[test]
fn several_merge_bases_pick_one_by_a_fixed_rule_and_say_so() {
    let repo = Repo::new();
    repo.commit(&[("a.rs", "fn a() {}\n")], &[]);
    repo.git(&["checkout", "-q", "-b", "x"]);
    let x1 = repo.commit(&[("x.rs", "fn x() {}\n")], &[]);
    repo.git(&["checkout", "-q", "-b", "y", "main"]);
    let y1 = repo.commit(&[("y.rs", "fn y() {}\n")], &[]);
    repo.git(&["checkout", "-q", "x"]);
    repo.git(&["merge", "-q", "--no-ff", "-m", "m", &y1]);
    repo.git(&["checkout", "-q", "y"]);
    repo.git(&["merge", "-q", "--no-ff", "-m", "m", &x1]);
    repo.commit(&[("y.rs", "fn y() { 1; }\n")], &[]);
    let c = compare_with_merge_base(repo.root(), "x", "y").unwrap();
    let mut both = vec![x1, y1];
    both.sort();
    let mb = c.merge_base.as_ref().unwrap();
    assert_eq!(mb.candidates, both);
    // Equal commit times: the smallest id wins.
    assert_eq!(c.base, both[0]);
    assert_eq!(c.notes.len(), 1);
    assert!(c.notes[0].starts_with("2 merge bases"));
}

/// With several merge bases at different times, the most recently committed one is chosen,
/// whichever has the smaller id: each of the two is made the newer once.
#[test]
fn several_merge_bases_pick_the_most_recent() {
    for newer_is_x in [true, false] {
        let repo = Repo::new();
        repo.commit(&[("a.rs", "fn a() {}\n")], &[]);
        // A commit with its own committer date (the helper pins every other one).
        let dated = |file: &str, date: &str| {
            std::fs::write(repo.root().join(file), "fn f() {}\n").unwrap();
            repo.git(&["add", "-A"]);
            let out = test_repo::command(repo.root())
                .args(["commit", "-q", "-m", "dated"])
                .env("GIT_AUTHOR_DATE", date)
                .env("GIT_COMMITTER_DATE", date)
                .output()
                .unwrap();
            assert!(out.status.success(), "{out:?}");
            repo.git(&["rev-parse", "HEAD"])
        };
        let (late, early) = ("2026-03-01T00:00:00Z", "2026-02-01T00:00:00Z");
        repo.git(&["checkout", "-q", "-b", "x"]);
        let x1 = dated("x.rs", if newer_is_x { late } else { early });
        repo.git(&["checkout", "-q", "-b", "y", "main"]);
        let y1 = dated("y.rs", if newer_is_x { early } else { late });
        repo.git(&["checkout", "-q", "x"]);
        repo.git(&["merge", "-q", "--no-ff", "-m", "m", &y1]);
        repo.git(&["checkout", "-q", "y"]);
        repo.git(&["merge", "-q", "--no-ff", "-m", "m", &x1]);
        let c = compare_with_merge_base(repo.root(), "x", "y").unwrap();
        assert_eq!(c.merge_base.as_ref().unwrap().candidates.len(), 2);
        let want = if newer_is_x { &x1 } else { &y1 };
        assert_eq!(&c.base, want, "newer_is_x={newer_is_x}");
    }
}

fn write(repo: &Repo, path: &str, text: &str) {
    let p = repo.root().join(path);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, text).unwrap();
}

fn flags(f: &FileChanges) -> Vec<(usize, usize, bool)> {
    f.hunks
        .iter()
        .map(|h| (h.head.start_line, h.head.line_count, h.uncommitted))
        .collect()
}

#[test]
fn a_clean_working_folder_matches_the_commit_comparison() {
    let repo = Repo::new();
    let base = repo.commit(&[("src/lib.rs", LIB)], &[]);
    repo.commit(&[("src/lib.rs", &LIB.replace("x);", "x + 1);"))], &[]);
    let committed = compare(repo.root(), &base, "HEAD").unwrap();
    let mut worktree = compare_worktree(repo.root(), &base).unwrap();
    assert!(worktree.worktree);
    worktree.worktree = false;
    assert_eq!(worktree, committed);
    assert!(uncommitted_files(repo.root()).unwrap().is_empty());
}

#[test]
fn uncommitted_edits_untracked_and_deleted_files_are_flagged() {
    let repo = Repo::new();
    let base = repo.commit(
        &[
            ("src/lib.rs", LIB),
            ("src/gone.rs", "fn gone() {}\n"),
            (".gitignore", "target/\n"),
        ],
        &[],
    );
    // Committed: change `push`. Uncommitted: change `len`, add a file, delete one, and add
    // an ignored file.
    repo.commit(&[("src/lib.rs", &LIB.replace("x);", "x + 1);"))], &[]);
    let work = LIB
        .replace("x);", "x + 1);")
        .replace("self.items.len()", "self.items.len() + 0");
    write(&repo, "src/lib.rs", &work);
    write(&repo, "src/new.rs", "pub fn fresh() {}\n");
    write(&repo, "target/out.rs", "fn junk() {}\n");
    std::fs::remove_file(repo.root().join("src/gone.rs")).unwrap();

    let c = compare_worktree(repo.root(), &base).unwrap();
    assert_eq!(
        c.files.iter().map(|f| f.path.as_str()).collect::<Vec<_>>(),
        ["src/gone.rs", "src/lib.rs", "src/new.rs"],
        "ignored files are skipped"
    );
    let lib = file(&c, "src/lib.rs");
    assert!(lib.uncommitted);
    assert_eq!(flags(lib), [(7, 1, false), (11, 1, true)]);
    let by_name: Vec<_> = lib
        .symbols
        .iter()
        .map(|s| (s.qualified_name.as_str(), s.uncommitted))
        .collect();
    assert_eq!(by_name, [("Queue::push", false), ("Queue::len", true)]);
    let new = file(&c, "src/new.rs");
    assert_eq!(new.status, FileStatus::Added);
    assert_eq!(flags(new), [(1, 1, true)]);
    let gone = file(&c, "src/gone.rs");
    assert_eq!(gone.status, FileStatus::Deleted);
    assert!(gone.hunks.iter().all(|h| h.uncommitted));
    assert_eq!(
        uncommitted_files(repo.root()).unwrap(),
        ["src/gone.rs", "src/lib.rs", "src/new.rs"]
    );
}

#[test]
fn a_file_touched_without_a_change_is_not_uncommitted() {
    let repo = Repo::new();
    let base = repo.commit(&[("a.txt", "1\n")], &[]);
    write(&repo, "a.txt", "1\n");
    assert!(uncommitted_files(repo.root()).unwrap().is_empty());
    assert!(
        compare_worktree(repo.root(), &base)
            .unwrap()
            .files
            .is_empty()
    );
}

#[test]
fn one_file_can_be_asked_for_alone() {
    let repo = Repo::new();
    let base = repo.commit(&[("a.txt", "1\n2\n"), ("b.txt", "x\n")], &[]);
    repo.git(&["mv", "a.txt", "c.txt"]);
    repo.git(&["commit", "-q", "-m", "mv"]);
    write(&repo, "c.txt", "1\n2\n3\n");
    let f = worktree_file_changes(repo.root(), &base, "c.txt")
        .unwrap()
        .unwrap();
    assert_eq!(
        (f.status, f.old_path.as_deref()),
        (FileStatus::Renamed, Some("a.txt"))
    );
    assert_eq!(flags(&f), [(3, 1, true)]);
    assert!(
        worktree_file_changes(repo.root(), &base, "b.txt")
            .unwrap()
            .is_none()
    );
    let json = serde_json::to_value(&f).unwrap();
    assert_eq!(json["hunks"][0]["uncommitted"], true);
    // Committed hunks do not carry the field.
    let c = compare(repo.root(), &base, "HEAD").unwrap();
    assert!(
        serde_json::to_value(&c).unwrap()["files"][0]
            .get("uncommitted")
            .is_none()
    );
}

/// Found by the code-quality review: a working file over the source size limit failed to
/// read and was reported as deleted. It is now a file-level change with a reason.
#[test]
fn an_oversized_working_file_is_modified_not_deleted() {
    let repo = Repo::new();
    let base = repo.commit(&[("big.txt", "small\n")], &[]);
    let limit = cho_core::fs::MAX_SOURCE_BYTES as usize;
    std::fs::write(repo.root().join("big.txt"), vec![b'a'; limit + 1]).unwrap();
    let c = compare_worktree(repo.root(), &base).unwrap();
    let f = file(&c, "big.txt");
    assert_eq!(f.status, FileStatus::Modified);
    assert_eq!(f.level, Level::File(FileOnly::TooLarge));
    assert!(f.hunks.is_empty() && f.uncommitted);
    assert_eq!(uncommitted_files(repo.root()).unwrap(), ["big.txt"]);
}
