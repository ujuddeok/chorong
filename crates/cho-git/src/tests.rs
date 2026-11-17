#![allow(clippy::disallowed_methods)] // tests read their own fixtures
use super::*;
use std::process::Command;

fn r(s: &str) -> Option<String> {
    repo_ref_for_remote(s).map(|r| r.to_string())
}

#[test]
fn recognises_github_and_bitbucket_remotes_in_every_url_shape() {
    for url in [
        "git@github.com:acme/api.git",
        "https://github.com/acme/api.git",
        "https://github.com/acme/api",
        "https://someone@github.com/acme/api.git",
        "ssh://git@github.com/acme/api.git",
        "git://github.com/acme/api",
    ] {
        assert_eq!(r(url).as_deref(), Some("github:acme/api"), "{url}");
    }
    assert_eq!(
        r("git@bitbucket.org:team/web.git").as_deref(),
        Some("bitbucket:team/web")
    );
    assert_eq!(
        r("https://team@bitbucket.org/team/web.git").as_deref(),
        Some("bitbucket:team/web")
    );
    assert_eq!(
        r("https://git.example.com/scm/PROJ/svc.git").as_deref(),
        Some("bitbucket-dc:git.example.com/PROJ/svc")
    );
    assert_eq!(
        r("https://git.example.com/bitbucket/scm/PROJ/svc.git").as_deref(),
        Some("bitbucket-dc:git.example.com/PROJ/svc")
    );
    assert_eq!(
        r("ssh://git@git.example.com:7999/proj/svc.git").as_deref(),
        Some("bitbucket-dc:git.example.com/proj/svc")
    );
}

#[test]
fn unknown_hosts_and_local_paths_are_not_guessed() {
    for url in [
        "https://gitlab.com/acme/api.git",
        "git@gitlab.com:acme/api.git",
        "/srv/git/api.git",
        "file:///srv/git/api.git",
        "../api",
        "C:/repos/api",
        "https://github.com/acme",
        "ssh://git@git.example.com/proj/svc.git",
    ] {
        assert_eq!(r(url), None, "{url}");
    }
}

#[test]
fn local_references_abbreviate_the_home_directory() {
    let home = Path::new("/home/someone");
    assert_eq!(
        local_ref(Path::new("/home/someone/code/chorong"), Some(home)).to_string(),
        "local:~/code/chorong"
    );
    assert_eq!(local_ref(home, Some(home)).to_string(), "local:~");
    assert_eq!(
        local_ref(Path::new("/srv/chorong"), Some(home)).to_string(),
        "local:/srv/chorong"
    );
}

#[test]
fn parses_worktree_lists() {
    let text = "worktree /w/main\nHEAD 1111111111111111111111111111111111111111\nbranch refs/heads/main\n\nworktree /w/feature\nHEAD 2222222222222222222222222222222222222222\ndetached\n\n";
    let w = parse_worktrees(text);
    assert_eq!(w.len(), 2);
    assert_eq!(w[0].branch.as_deref(), Some("main"));
    assert_eq!(w[1].branch, None);
    assert_eq!(w[1].head.as_deref().map(short).as_deref(), Some("2222222"));
}

#[test]
fn prefers_a_worktree_at_the_rev_then_on_the_branch_then_the_first() {
    let c = |p: &str, h: &str, b: Option<&str>| Candidate {
        path: PathBuf::from(p),
        head: Some(h.into()),
        branch: b.map(str::to_string),
    };
    let all = [
        c("/a", "aaaaaaa1", Some("main")),
        c("/b", "bbbbbbb2", Some("feature")),
        c("/c", "ccccccc3", None),
    ];
    let pick = |rev, branch| {
        choose_worktree(&all, rev, branch)
            .map(|c| c.path.display().to_string())
            .unwrap()
    };
    assert_eq!(pick(Some("ccccccc"), Some("feature")), "/c");
    assert_eq!(pick(Some("0000000"), Some("feature")), "/b");
    assert_eq!(pick(Some("0000000"), None), "/a");
    assert_eq!(pick(None, None), "/a");
    assert!(choose_worktree(&[], None, None).is_none());
}

#[test]
fn parses_head_side_hunks() {
    let diff = "diff --git a/a.rs b/a.rs\n--- a/a.rs\n+++ b/a.rs\n@@ -3,2 +3,4 @@ fn x\n@@ -10 +12 @@\n@@ -20,3 +22,0 @@\ndiff --git a/b.rs b/b.rs\n--- a/b.rs\n+++ /dev/null\n@@ -1,5 +0,0 @@\n";
    let (hunks, removed) = parse_diff(diff);
    let spans: Vec<_> = hunks
        .iter()
        .map(|h| (h.path.as_str(), h.start_line, h.end_line))
        .collect();
    assert_eq!(spans, [("a.rs", 3, 6), ("a.rs", 12, 12), ("a.rs", 22, 23)]);
    assert_eq!(removed, ["b.rs"]);
}

/// Found by the `git_diff` fuzz target: git ends the `---` / `+++` lines of names with spaces
/// in a tab, and the path came back with it.
#[test]
fn diff_paths_with_spaces_lose_the_trailing_tab() {
    let diff = "diff --git a/d e/f g.txt b/d e/f g.txt\n--- a/d e/f g.txt\t\n+++ b/d e/f g.txt\t\n@@ -1 +1 @@\n-x\n+y\n";
    let (hunks, _) = parse_diff(diff);
    assert_eq!(hunks.len(), 1);
    assert_eq!(hunks[0].path, "d e/f g.txt");
}

/// Found by the `git_diff` fuzz target: names git quotes (non-ASCII under the default
/// `core.quotePath`, quotes, tabs, backslashes) were dropped, with their hunks and deletions.
#[test]
fn quoted_diff_paths_are_unquoted() {
    let diff = "--- \"a/caf\\303\\251 \\\"q\\\" \\\\ \\t.txt\"\n+++ \"b/caf\\303\\251 \\\"q\\\" \\\\ \\t.txt\"\n@@ -1 +1 @@\n-x\n+y\n--- \"a/g\\303\\251.txt\"\n+++ /dev/null\n@@ -1 +0,0 @@\n-x\n";
    let (hunks, removed) = parse_diff(diff);
    assert_eq!(hunks.len(), 1);
    assert_eq!(hunks[0].path, "caf\u{e9} \"q\" \\ \t.txt");
    assert_eq!(removed, ["g\u{e9}.txt"]);
}

/// Found by the `git_diff` fuzz target: a removed line `-- x` (shown as `--- x`) or an added
/// line `++ y` (`+++ y`) was read as a file header, which lost every later hunk of the file.
#[test]
fn hunk_body_lines_that_look_like_headers_are_skipped() {
    let diff =
        "--- a/f.sql\n+++ b/f.sql\n@@ -2,2 +2 @@\n-two\n--- x\n+++ y\n@@ -9 +8,2 @@\n-z\n+a\n+b\n";
    let (hunks, removed) = parse_diff(diff);
    let spans: Vec<_> = hunks
        .iter()
        .map(|h| (h.path.as_str(), h.start_line, h.end_line))
        .collect();
    assert_eq!(spans, [("f.sql", 2, 2), ("f.sql", 8, 9)]);
    assert!(removed.is_empty());
}

use cho_core::test_repo::git as sh;

#[test]
fn reads_a_file_at_an_older_commit_without_touching_the_checkout() {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    sh(d, &["init", "-q", "-b", "main"]);
    std::fs::create_dir(d.join("src")).unwrap();
    std::fs::write(d.join("src/a.rs"), "fn old() {}\n").unwrap();
    sh(d, &["add", "."]);
    sh(d, &["commit", "-q", "-m", "one"]);
    let first = rev_parse(d, "HEAD").unwrap();
    std::fs::write(d.join("src/a.rs"), "fn new() {}\n").unwrap();
    sh(d, &["commit", "-q", "-am", "two"]);
    sh(
        d,
        &["remote", "add", "origin", "git@github.com:acme/demo.git"],
    );

    assert_eq!(
        read_file_at(d, &short(&first), "src/a.rs").unwrap(),
        b"fn old() {}\n"
    );
    assert!(read_file_at(d, &first, "src/missing.rs").is_err());
    assert!(read_file_at(d, "nope", "src/a.rs").is_err());
    // The checkout is untouched.
    assert_eq!(
        std::fs::read_to_string(d.join("src/a.rs")).unwrap(),
        "fn new() {}\n"
    );
    let i = info(d);
    assert_eq!(i.branch.as_deref(), Some("main"));
    assert!(!i.dirty);
    assert_eq!(repo_identity(d).to_string(), "github:acme/demo");

    let repo = RepoRef::parse("github:acme/demo").unwrap();
    let found = candidates_for(&repo, &[d.to_path_buf()]);
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].branch.as_deref(), Some("main"));
    assert!(candidates_for(&RepoRef::parse("github:acme/other").unwrap(), &[d.into()]).is_empty());
}

#[test]
fn reading_git_state_never_rewrites_the_users_index() {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    sh(d, &["init", "-q", "-b", "main"]);
    std::fs::write(d.join("a.rs"), "fn a() {}\n").unwrap();
    sh(d, &["add", "."]);
    sh(d, &["commit", "-q", "-m", "one"]);
    // Same content, newer mtime: the index's stat cache is stale, so a plain `git status`
    // would refresh it and rewrite `.git/index`.
    let file = std::fs::File::options()
        .write(true)
        .open(d.join("a.rs"))
        .unwrap();
    let later = std::time::SystemTime::now() + std::time::Duration::from_secs(5);
    file.set_modified(later).unwrap();
    drop(file);
    let before = std::fs::read(d.join(".git/index")).unwrap();
    let info = info(d);
    assert!(!info.dirty, "same content is not a change: {info:?}");
    assert_eq!(std::fs::read(d.join(".git/index")).unwrap(), before);
    assert!(!d.join(".git/index.lock").exists());
}

/// The command is copied from a banner and pasted into a shell; `rev` and `branch` come from
/// links, so a name with shell syntax must reach git as one word and run nothing.
#[test]
fn switch_commands_quote_names_from_links() {
    assert_eq!(
        switch_command("abc1234", Some("feature/x-1")).as_deref(),
        Some("git switch feature/x-1")
    );
    assert_eq!(
        switch_command("abc1234", None).as_deref(),
        Some("git switch --detach abc1234")
    );
    assert_eq!(
        switch_command("abc1234", Some("x;touch $(id)`y`")).as_deref(),
        Some("git switch 'x;touch $(id)`y`'")
    );
    assert_eq!(
        switch_command("abc1234", Some("it's")).as_deref(),
        Some(r"git switch 'it'\''s'")
    );
    // A leading `-` would be read as an option; an empty name means nothing to switch to.
    for bad in ["-x", "--detach", ""] {
        assert_eq!(switch_command("abc1234", Some(bad)), None, "{bad:?}");
        assert_eq!(switch_command(bad, None), None, "{bad:?}");
    }
}

/// The quoting holds in a real shell: the quoted word reaches the program unchanged and
/// nothing else runs.
#[cfg(unix)]
#[test]
fn quoted_switch_commands_run_nothing_in_a_shell() {
    let dir = tempfile::tempdir().unwrap();
    let marker = dir.path().join("ran");
    let name = format!(
        "x;touch {m};$(touch {m})`touch {m}`'q",
        m = marker.display()
    );
    let cmd = switch_command("abc1234", Some(&name)).unwrap();
    // Replace `git switch` with `printf %s` to see the single argument the shell passes.
    let shown = cmd.replacen("git switch", "printf %s", 1);
    let out = Command::new("sh").arg("-c").arg(&shown).output().unwrap();
    assert!(out.status.success());
    assert_eq!(String::from_utf8(out.stdout).unwrap(), name);
    assert!(!marker.exists(), "the shell ran part of the name");
}

#[test]
fn blames_the_shown_text_from_a_commit_and_marks_what_is_not_committed() {
    if Command::new("git").arg("--version").output().is_err() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    sh(d, &["init", "-q", "-b", "main"]);
    std::fs::create_dir(d.join("src")).unwrap();
    std::fs::write(d.join("src/a.rs"), "one\ntwo\n").unwrap();
    sh(d, &["add", "."]);
    sh(d, &["commit", "-q", "-m", "first"]);
    let first = rev_parse(d, "HEAD").unwrap();
    std::fs::write(d.join("src/a.rs"), "one\ntwo\nthree\n").unwrap();
    sh(d, &["commit", "-q", "-am", "second"]);
    let src = d.join("src");
    let head = blame::resolve_commit(&src, None).unwrap();
    assert_eq!(head, rev_parse(d, "HEAD").unwrap());
    assert_eq!(
        blame::resolve_commit(&src, Some(&short(&first))).unwrap(),
        first
    );

    // The shown text: line 2 edited and a line added, not committed.
    let shown = b"one\nTWO\nthree\nfour\n";
    let b = blame::blame(&src, &head, Path::new("a.rs"), shown).unwrap();
    let ids: Vec<_> = (0..4).map(|l| b.commit_of(l).unwrap().id.clone()).collect();
    assert_eq!(ids[0], first);
    assert!(b.commit_of(1).unwrap().uncommitted());
    assert_eq!(ids[2], head);
    assert!(b.commit_of(3).unwrap().uncommitted());
    assert_eq!(b.commit_of(0).unwrap().summary, "first");
    assert_eq!(b.commit_of(0).unwrap().author, "Chorong Test");
    assert!(
        !b.commit_of(0).unwrap().boundary,
        "--root: the first commit is no boundary"
    );

    // At the first commit, its own text: everything is that commit's.
    let old = blame::blame(&src, &first, Path::new("a.rs"), b"one\ntwo\n").unwrap();
    assert_eq!(old.lines, [0, 0]);
    assert_eq!(old.commits[0].id, first);

    // A rev that looks like an option, a missing path, and a folder outside any repository
    // are refused with git's reason, never run.
    assert!(blame::resolve_commit(&src, Some("--output=x")).is_err());
    assert!(!src.join("x").exists());
    assert!(blame::blame(&src, "--output=x", Path::new("a.rs"), b"").is_err());
    let missing = blame::blame(&src, &head, Path::new("new.rs"), b"x\n").unwrap_err();
    assert!(missing.contains("new.rs"), "{missing}");
    let outside = tempfile::tempdir().unwrap();
    assert_eq!(
        blame::resolve_commit(outside.path(), None).unwrap_err(),
        "not in a git repository"
    );
}
