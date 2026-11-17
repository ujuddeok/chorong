//! A repository whose config names a program for every key git documents as running one
//! (in `.git/config`, in a file it includes, and in `config.worktree`) must run none of them
//! through `cho_core::git::command`, for every git command Chorong runs on a reader's folder.
//! The commands and arguments below mirror the call sites in `cho-git` (`git`, `info`,
//! `rev_parse`, `identity_remote`, `worktrees`, `diff_hunks`), `cho-semantic`'s
//! `GitState::of` and `app/src/graph.rs` (`branch_base`).
#![cfg(unix)]
#![allow(clippy::disallowed_methods)] // the test reads its own fixture's config

use std::path::{Path, PathBuf};

// The crate's `test-support` module, compiled into this test (an integration test cannot turn
// on its own crate's feature).
#[path = "../src/test_repo.rs"]
#[allow(dead_code)] // not every helper is used here
mod test_repo;

/// Plain git (not `cho_core::git::command`), hermetic: whether `git args` succeeded.
fn plain(dir: &Path, args: &[&str]) -> bool {
    test_repo::output(dir, args).status.success()
}

fn script(path: &Path, body: &str) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::write(path, format!("#!/bin/sh\n{body}\n")).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

/// Every config key that git documents as naming a program or shell command (git 2.43), with
/// drivers in mixed case and with `=` in the name, which `git -c key=value` cannot express.
/// Each is set to the marker script with its own name as the argument.
const PROGRAM_KEYS: &[&str] = &[
    "core.fsmonitor",
    "core.gitProxy",
    "core.sshCommand",
    "core.alternateRefsCommand",
    "core.askPass",
    "core.editor",
    "core.pager",
    "pager.status",
    "pager.diff",
    "pager.log",
    "pager.worktree",
    "pager.remote",
    "pager.config",
    "pager.rev-parse",
    "pager.merge-base",
    "pager.symbolic-ref",
    "remote.origin.uploadpack",
    "remote.origin.receivepack",
    "remote.origin.vcs",
    "uploadpack.packObjectsHook",
    "gc.recentObjectsHook",
    "gpg.program",
    "gpg.openpgp.program",
    "gpg.ssh.program",
    "gpg.x509.program",
    "gpg.minTrustLevel.program",
    "gpg.ssh.defaultKeyCommand",
    "credential.helper",
    "credential.https://example.invalid.helper",
    "diff.external",
    "diff.evil.command",
    "diff.evil.textconv",
    "diff.Mixed.textconv",
    "diff.a=b.textconv",
    "merge.evil.driver",
    "merge.Mixed.driver",
    "filter.evil.clean",
    "filter.evil.smudge",
    "filter.evil.process",
    "filter.Mixed.clean",
    "filter.Mixed.process",
    "filter.a=b.clean",
    "filter.a=b.process",
    "sequence.editor",
    "trailer.t.cmd",
    "trailer.t.command",
    "mergetool.t.cmd",
    "mergetool.t.path",
    "difftool.t.cmd",
    "difftool.t.path",
    "browser.t.cmd",
    "browser.t.path",
    "man.t.cmd",
    "man.t.path",
    "web.browser",
    "instaweb.browser",
    "sendemail.smtpServer",
    "ssh.variant",
    "core.hooksPath",
];

fn set_keys(out: &mut String, keys: &[&str], mark: &Path, tag: &str) {
    use std::fmt::Write;
    for key in keys {
        // Write as a `[section "subsection"] variable = value` entry.
        let parts: Vec<&str> = key.split('.').collect();
        // `credential.https://example.invalid.helper` has dots in the subsection; the
        // variable is always the last dotted part and the section the first.
        let (section, var) = (parts[0], parts[parts.len() - 1]);
        let sub = key[section.len()..key.len() - var.len()].trim_matches('.');
        let name = format!("{tag}-{key}").replace(['/', ':', '='], "_");
        let value = if *key == "core.hooksPath" {
            // Hooks are covered separately; keep this key pointing at the hostile hooks.
            mark.join("hooks").display().to_string()
        } else {
            format!("{} {name}", mark.join("mark.sh").display())
        };
        if sub.is_empty() {
            writeln!(out, "[{section}]\n\t{var} = {value}").unwrap();
        } else {
            writeln!(out, "[{section} \"{sub}\"]\n\t{var} = {value}").unwrap();
        }
    }
}

fn ran(marks: &Path) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(marks)
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.starts_with("ran-"))
        .collect();
    v.sort();
    v
}

fn clear(marks: &Path) {
    for n in ran(marks) {
        std::fs::remove_file(marks.join(n)).ok();
    }
}

#[test]
fn a_hostile_repository_config_runs_no_program_through_command() {
    let root: PathBuf =
        std::env::temp_dir().join(format!("cho-git-hostile-{}", std::process::id()));
    std::fs::remove_dir_all(&root).ok();
    let dir = root.join("repo");
    let marks = root.join("marks");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::create_dir_all(marks.join("hooks")).unwrap();
    assert!(plain(&dir, &["init", "-q", "-b", "main"]));
    // `mark.sh NAME` records that something ran.
    script(
        &marks.join("mark.sh"),
        &format!("touch '{}/ran-'\"$1\"\ncat", marks.display()),
    );
    for hook in [
        "post-index-change",
        "reference-transaction",
        "post-checkout",
        "pre-commit",
        "post-commit",
        "fsmonitor-watchman",
        "pre-auto-gc",
        "post-rewrite",
    ] {
        script(
            &marks.join("hooks").join(hook),
            &format!("touch '{}/ran-hook-{hook}'", marks.display()),
        );
    }

    // History: two commits on main, a feature branch, a remote called origin.
    std::fs::write(dir.join("a"), "x\n").unwrap();
    for f in ["evil", "Mixed", "a=b"] {
        std::fs::write(dir.join(format!("f-{f}")), "x\n").unwrap();
    }
    std::fs::write(
        dir.join(".gitattributes"),
        "f-evil filter=evil diff=evil merge=evil\n\
         f-Mixed filter=Mixed diff=Mixed merge=Mixed\n\
         f-a=b filter=a=b diff=a=b\n",
    )
    .unwrap();
    assert!(plain(&dir, &["add", "."]));
    assert!(plain(&dir, &["commit", "-q", "-m", "one"]));
    assert!(plain(&dir, &["switch", "-q", "-c", "feat"]));
    std::fs::write(dir.join("a"), "x\ny\n").unwrap();
    assert!(plain(&dir, &["commit", "-q", "-am", "two"]));
    assert!(plain(&dir, &["switch", "-q", "main"]));
    assert!(plain(
        &dir,
        &[
            "remote",
            "add",
            "origin",
            &root.join("nowhere").display().to_string()
        ]
    ));
    assert!(plain(
        &dir,
        &[
            "worktree",
            "add",
            "-q",
            &root.join("wt").display().to_string(),
            "feat"
        ]
    ));

    // Hostile config in three places: .git/config, an included file, config.worktree.
    let mut local = String::new();
    set_keys(&mut local, PROGRAM_KEYS, &marks, "local");
    local.push_str("[extensions]\n\tworktreeConfig = true\n");
    local.push_str(&format!(
        "[include]\n\tpath = {}\n",
        root.join("included.cfg").display()
    ));
    local.push_str(&format!(
        "[includeIf \"gitdir:{}/\"]\n\tpath = {}\n",
        dir.join(".git").display(),
        root.join("included-if.cfg").display()
    ));
    local.push_str(&format!(
        "[includeIf \"gitdir:{}\"]\n\tpath = {}\n",
        dir.join(".git").display(),
        root.join("included-if.cfg").display()
    ));
    let mut included = String::new();
    set_keys(&mut included, PROGRAM_KEYS, &marks, "included");
    let mut included_if = String::new();
    set_keys(&mut included_if, PROGRAM_KEYS, &marks, "includeif");
    let mut worktree = String::new();
    set_keys(&mut worktree, PROGRAM_KEYS, &marks, "worktree");
    let git_dir = dir.join(".git");
    let existing = std::fs::read_to_string(git_dir.join("config")).unwrap();
    // `extensions.worktreeConfig` needs repositoryformatversion 1 to be read back.
    std::fs::write(git_dir.join("config"), format!("{existing}{local}")).unwrap();
    assert!(plain(
        &dir,
        &["config", "core.repositoryformatversion", "1"]
    ));
    std::fs::write(root.join("included.cfg"), included).unwrap();
    std::fs::write(root.join("included-if.cfg"), included_if).unwrap();
    std::fs::write(git_dir.join("config.worktree"), worktree).unwrap();
    // Make `status` do work: a modified tracked file of each driver, and an untracked file.
    // Same size and mtime-in-the-same-second as the index entry, so git must hash the content
    // (running the clean filter) rather than trust the size.
    for f in ["a", "f-evil", "f-Mixed", "f-a=b"] {
        std::fs::write(dir.join(f), "z\n").unwrap();
    }
    std::fs::write(dir.join("untracked"), "u\n").unwrap();
    // The check below is meaningful only if the config is really in effect.
    let listing = test_repo::git(&dir, &["config", "--list", "--show-scope"]);
    for scope in [
        "local\tcore.fsmonitor",
        "worktree\tcore.fsmonitor",
        "local\tfilter.Mixed.clean",
    ] {
        assert!(
            listing.contains(scope),
            "fixture lost `{scope}`:\n{listing}"
        );
    }

    // Every git invocation Chorong makes on a reader's folder.
    let diff_args = [
        "diff",
        "-U0",
        "--no-color",
        "--no-ext-diff",
        "--no-textconv",
        "--ignore-submodules=all",
        "main..feat",
    ];
    let rev = "main^{commit}";
    let invocations: Vec<Vec<&str>> = vec![
        vec!["rev-parse", "HEAD"],
        vec!["symbolic-ref", "--quiet", "--short", "HEAD"],
        vec!["status", "--porcelain", "--ignore-submodules=all"],
        vec!["rev-parse", "--verify", "--quiet", rev],
        vec!["remote"],
        vec!["remote", "get-url", "origin"],
        vec!["worktree", "list", "--porcelain"],
        diff_args.to_vec(),
        vec!["rev-parse", "--show-toplevel"],
        vec![
            "symbolic-ref",
            "--quiet",
            "--short",
            "refs/remotes/origin/HEAD",
        ],
        vec!["merge-base", "main", "HEAD"],
    ];
    for cwd in [dir.clone(), root.join("wt")] {
        for args in &invocations {
            clear(&marks);
            let out = cho_core::git::command(&cwd).args(args).output().unwrap();
            let stderr = String::from_utf8_lossy(&out.stderr);
            assert!(
                !stderr.contains("fatal: bad config"),
                "git {args:?} in {cwd:?}: {stderr}"
            );
            assert_eq!(
                ran(&marks),
                Vec::<String>::new(),
                "git {args:?} in {cwd:?} ran programs from the repository's config"
            );
        }
    }

    // Control: plain git runs at least the fsmonitor and the filters, so the fixture bites.
    clear(&marks);
    plain(&dir, &["status", "--porcelain"]);
    plain(&dir, &["diff", "HEAD"]);
    assert!(
        !ran(&marks).is_empty(),
        "plain git ran nothing: the hostile fixture is not hostile"
    );
    std::fs::remove_dir_all(&root).ok();
}
