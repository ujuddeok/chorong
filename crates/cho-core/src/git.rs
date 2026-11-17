//! Running `git` safely in a folder Chorong did not create.
//!
//! A repository's own config (`.git/config`, plus anything it includes) can name programs
//! that git runs during ordinary read-only commands: `core.fsmonitor` on `git status`,
//! `filter.<name>.clean`/`process` when hashing changed files, `diff.<name>.textconv` on
//! `git diff`, and hooks such as `post-index-change` from `.git/hooks`. A folder unpacked
//! from an archive or opened from a shared drive keeps that config, so opening it in Chorong
//! must not run git with it as written.
//!
//! [`command`] neutralises every such setting the repository defines (local and worktree
//! scope only; the user's own global and system config, such as git-lfs, is left alone),
//! turns off hooks and optional locks, and refuses to discover embedded bare repositories.
//! Callers that hash or diff files should also pass `--ignore-submodules=all` (a submodule
//! has its own config) and, for `diff`, `--no-ext-diff --no-textconv`. Use it for every git
//! command run in a folder the reader opened, read or write.
//!
//! [`own_repo_command`] is for repositories Chorong created itself (clones in its own cache):
//! their config is Chorong's, so it is honoured (credential helpers, bare clones), but hooks,
//! fsmonitor, optional locks and prompts are still off. Never use it on a reader's folder.
//!
//! Both refuse the `ext::` transport (it runs a shell command named in the URL), allow
//! `file://` only for commands Chorong runs itself (not for submodules), and never recurse into
//! submodules, whatever the user's or repository's config says. A URL that came from
//! anywhere but the reader (PR data, an API, a link) must pass [`check_remote_url`] and
//! follow `--` on the command line, so it cannot be read as an option such as
//! `--upload-pack=<program>`.

use std::path::Path;
use std::process::Command;

/// Config keys (lower-case, final dotted part) that make git run a program. Matching on the
/// last part covers per-driver keys such as `filter.<name>.clean`. Audited against git 2.43's
/// `git-config(1)`; the end-to-end test in `tests/git_hostile_config.rs` sets every one.
const RUNS_A_PROGRAM: &[&str] = &[
    "alternaterefscommand", // core.alternateRefsCommand: run for each ref advertisement
    "askpass",
    "browser", // web.browser, instaweb.browser
    "clean",
    "cmd", // trailer.<t>.cmd, and <tool>.<name>.cmd for difftool, mergetool, browser, man
    "command",
    "defaultkeycommand", // gpg.ssh.defaultKeyCommand
    "driver",            // merge.<name>.driver
    "editor",
    "external",
    "fsmonitor",
    "gitproxy", // core.gitProxy: runs for git:// URLs
    "helper",
    "hookspath",
    "packobjectshook", // uploadpack.packObjectsHook
    "pager",
    "process",
    "program",
    "receivepack",       // remote.<name>.receivepack
    "recentobjectshook", // gc.recentObjectsHook
    "smudge",
    "sshcommand",
    "textconv",
    "uploadpack", // remote.<name>.uploadpack
];

/// Sections whose every variable names a program: `pager.<command>` (its value is the pager
/// to run for that git command).
const PROGRAM_SECTIONS: &[&str] = &["pager"];

/// Sections whose `<tool>.path` variable is the tool's program.
const TOOL_SECTIONS: &[&str] = &["browser", "difftool", "man", "mergetool"];

/// A `git -C root` command that will not run programs named by the repository's own config.
pub fn command(root: &Path) -> Command {
    let mut cmd = base(root);
    cmd.args(["-c", "safe.bareRepository=explicit"]);
    // Keys `git -c key=value` cannot express go in the environment instead (see below).
    let mut env_keys: Vec<(String, &str)> = vec![];
    for key in repo_program_keys(root) {
        let value = match key.to_ascii_lowercase().as_str() {
            "core.fsmonitor" => "false",
            "core.hookspath" => "/dev/null",
            _ => "",
        };
        // `-c` splits `name=value` at the first `=`, so a subsection containing `=`
        // (`[filter "a=b"]`) was never neutralised: `-c filter.a=b.process=` sets `filter.a`
        // instead and the driver ran. `GIT_CONFIG_COUNT`/`KEY_n`/`VALUE_n` (git 2.31+) take the
        // key verbatim. A newline cannot be in a subsection, so nothing else needs this.
        if key.contains('=') {
            env_keys.push((key, value));
        } else {
            cmd.arg("-c").arg(format!("{key}={value}"));
        }
    }
    if !env_keys.is_empty() {
        // Entries after any the caller's environment already carries, so ours win.
        let first: usize = std::env::var("GIT_CONFIG_COUNT")
            .ok()
            .and_then(|n| n.parse().ok())
            .unwrap_or(0);
        for (i, (key, value)) in env_keys.iter().enumerate() {
            cmd.env(format!("GIT_CONFIG_KEY_{}", first + i), key)
                .env(format!("GIT_CONFIG_VALUE_{}", first + i), value);
        }
        cmd.env("GIT_CONFIG_COUNT", (first + env_keys.len()).to_string());
    }
    cmd
}

/// A `git -C root` command for a repository Chorong created (see the module docs).
pub fn own_repo_command(root: &Path) -> Command {
    base(root)
}

/// The fixed part: hooks off, no fsmonitor, no optional index writes, no prompts or pager.
/// `-C root` comes last, so callers' `-c` options may follow it.
fn base(root: &Path) -> Command {
    let mut cmd = Command::new("git");
    cmd.env("GIT_OPTIONAL_LOCKS", "0")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_PAGER", "cat")
        .args(["-c", "core.fsmonitor=false"])
        .args(["-c", "core.hooksPath=/dev/null"])
        .args(["-c", "protocol.ext.allow=never"])
        .args(["-c", "protocol.file.allow=user"])
        .args(["-c", "submodule.recurse=false"])
        .arg("-C")
        .arg(root);
    cmd
}

/// Transports a remote URL may use. Everything else, including `ext::` and other
/// `<helper>::<address>` forms, is refused.
const ALLOWED_SCHEMES: &[&str] = &["https", "http", "ssh", "git", "git+ssh", "ssh+git", "file"];

/// Checks a remote URL that did not come from the reader before Chorong passes it to git:
/// a URL with an allowed scheme, an scp-style `user@host:path`, or an absolute local path.
/// Refuses anything git could read as an option (a leading `-`, or a host starting with
/// `-`, which ssh would read as an option), remote-helper forms such as `ext::`, and
/// control characters.
pub fn check_remote_url(url: &str) -> Result<(), String> {
    let refuse = |why: &str| {
        Err(format!(
            "refusing remote URL `{}`: {why}",
            url.escape_debug()
        ))
    };
    if url.is_empty() {
        return refuse("empty");
    }
    if url.starts_with('-') {
        return refuse("starts with `-`, so git would read it as an option");
    }
    if url.chars().any(char::is_control) {
        return refuse("contains control characters");
    }
    if let Some((scheme, rest)) = url.split_once("://") {
        if !ALLOWED_SCHEMES.contains(&scheme.to_ascii_lowercase().as_str()) {
            return refuse("transport not allowed");
        }
        let authority = rest.split('/').next().unwrap_or("");
        let host = authority.rsplit('@').next().unwrap_or("");
        // git strips the brackets of `[host]` before handing the host to ssh.
        if host.trim_start_matches('[').starts_with('-') {
            return refuse("host starts with `-`");
        }
        return Ok(());
    }
    let before_slash = url.split('/').next().unwrap_or("");
    if before_slash.contains("::") {
        return refuse("remote-helper transports (such as `ext::`) are not allowed");
    }
    if url.starts_with('/') {
        return Ok(()); // a local path
    }
    // scp-style `[user@]host:path`
    match before_slash.split_once(':') {
        Some((authority, _)) if !authority.is_empty() => {
            let host = authority.rsplit('@').next().unwrap_or("");
            let host = host.trim_start_matches('[');
            if host.is_empty() || host.starts_with('-') {
                refuse("bad host")
            } else {
                Ok(())
            }
        }
        _ => refuse("not a URL, `host:path` or absolute path"),
    }
}

/// Keys set in the repository's own config (local or worktree scope, includes followed) that
/// make git run a program. Reading config runs nothing.
fn repo_program_keys(root: &Path) -> Vec<String> {
    let Ok(out) = base(root)
        .args(["config", "--list", "--show-scope", "--null"])
        .output()
    else {
        return vec![];
    };
    program_keys(&out.stdout)
}

/// Whether the config key (`section[.subsection].variable`) names a program git would run.
fn runs_a_program(key: &str) -> bool {
    let (Some((section, _)), Some((_, var))) = (key.split_once('.'), key.rsplit_once('.')) else {
        return false;
    };
    let (section, var) = (section.to_ascii_lowercase(), var.to_ascii_lowercase());
    PROGRAM_SECTIONS.contains(&section.as_str())
        || RUNS_A_PROGRAM.contains(&var.as_str())
        || (var == "path" && TOOL_SECTIONS.contains(&section.as_str()))
}

/// Parses `git config --list --show-scope --null` output: `scope\0key\nvalue\0` per entry.
fn program_keys(listing: &[u8]) -> Vec<String> {
    let text = String::from_utf8_lossy(listing);
    let fields: Vec<&str> = text.split('\0').collect();
    // Section and variable names are case-insensitive but the subsection is not: keep the
    // key exactly as `git config --list` prints it (section and variable already lower-case).
    // Lower-casing all of it named the wrong driver for `[filter "Evil"]`, which kept running.
    let mut keys: Vec<String> = fields
        .chunks(2)
        .filter(|c| c.len() == 2 && matches!(c[0], "local" | "worktree"))
        .map(|c| c[1].split('\n').next().unwrap_or("").to_string())
        .filter(|k| runs_a_program(k))
        .collect();
    keys.sort();
    keys.dedup();
    keys
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remote_urls_that_git_could_misread_are_refused() {
        for ok in [
            "https://github.com/acme/api.git",
            "ssh://git@bitbucket.example.com:7999/plat/core.git",
            "git@github.com:acme/api.git",
            "file:///srv/mirror/api.git",
            "/srv/mirror/api.git",
        ] {
            assert!(check_remote_url(ok).is_ok(), "{ok}");
        }
        for bad in [
            "",
            "--upload-pack=touch /tmp/x",
            "-oProxyCommand=evil",
            "ext::sh -c touch% /tmp/x",
            "fd::3",
            "EXT://x",
            "ssh://-oProxyCommand=evil/x",
            "ssh://user@-evil/x",
            // Found by the `source_url` fuzz target: git removes the brackets.
            "ssh://[-oProxyCommand=evil]/x",
            "ssh://git@[-oProxyCommand=evil]:22/x",
            "[-oProxyCommand=evil]:x",
            "git@[-evil]:x",
            "-evil:path",
            "https://example.com/a\nb",
            "relative/path",
        ] {
            assert!(check_remote_url(bad).is_err(), "{bad:?}");
        }
    }

    /// Even when the user's global config allows `ext::`, Chorong's git refuses it.
    #[test]
    fn the_ext_transport_never_runs() {
        let dir = std::env::temp_dir().join(format!("cho-git-ext-{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        let global = dir.join("gitconfig");
        std::fs::write(&global, "[protocol \"ext\"]\n\tallow = always\n").unwrap();
        let marker = dir.join("RAN");
        let url = format!("ext::sh -c touch% {}", marker.display());
        let run = |mut cmd: Command| {
            cmd.env("GIT_CONFIG_GLOBAL", &global)
                .args(["ls-remote", "--", &url])
                .output()
                .ok()
        };
        if run(own_repo_command(&dir)).is_none() {
            return; // no git here
        }
        assert!(!marker.exists(), "own_repo_command ran an ext:: URL");
        run(command(&dir));
        assert!(!marker.exists(), "command ran an ext:: URL");
        // Control: plain git with that global config does run it.
        let mut plain = Command::new("git");
        plain.arg("-C").arg(&dir);
        run(plain);
        assert!(marker.exists(), "plain git should have run the ext:: URL");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn only_repository_scoped_program_keys_are_neutralised() {
        let listing = b"system\0filter.lfs.clean\ngit-lfs clean -- %f\0\
local\0core.fsmonitor\ntouch /tmp/x\0\
local\0filter.Evil.process\nsh -c evil\0\
local\0filter.a=b.clean\nsh -c evil\0\
local\0pager.status\nevil\0\
local\0merge.m.driver\nevil\0\
local\0core.alternaterefscommand\nevil\0\
local\0remote.origin.uploadpack\nevil\0\
local\0mergetool.t.path\nevil\0\
local\0include.path\n/x\0\
local\0diff.evil.textconv\nevil\0\
local\0user.name\nme\0\
worktree\0core.hookspath\n/tmp/hooks\0\
global\0core.pager\nless\0";
        assert_eq!(
            program_keys(listing),
            [
                "core.alternaterefscommand",
                "core.fsmonitor",
                "core.hookspath",
                "diff.evil.textconv",
                // The subsection keeps its case; `=` in it is kept too.
                "filter.Evil.process",
                "filter.a=b.clean",
                "merge.m.driver",
                "mergetool.t.path",
                "pager.status",
                "remote.origin.uploadpack",
            ]
        );
    }

    /// End to end: a repository whose config names programs for fsmonitor, a filter driver,
    /// a textconv driver and a hook runs none of them through `command`.
    #[test]
    fn a_hostile_repository_config_runs_nothing() {
        let dir = std::env::temp_dir().join(format!("cho-git-safe-{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        let plain = |args: &[&str]| crate::test_repo::output(&dir, args).status.success();
        assert!(plain(&["init", "-q"]));
        std::fs::write(dir.join("a"), "x\n").unwrap();
        std::fs::write(dir.join(".gitattributes"), "* filter=evil diff=evil\n").unwrap();
        assert!(plain(&["add", "."]));
        assert!(plain(&["commit", "-q", "-m", "one"]));
        let marker = dir.join("RAN");
        let touch = format!("touch '{}'", marker.display());
        for (k, v) in [
            ("core.fsmonitor", format!("{touch}; false")),
            ("filter.evil.clean", format!("{touch}; cat")),
            ("filter.evil.process", touch.clone()),
            ("diff.evil.textconv", format!("{touch}; cat")),
        ] {
            assert!(plain(&["config", k, &v]));
        }
        let hook = dir.join(".git/hooks/post-index-change");
        std::fs::write(&hook, format!("#!/bin/sh\n{touch}\n")).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        std::fs::write(dir.join("a"), "x\ny\n").unwrap();

        let status = command(&dir)
            .args(["status", "--porcelain", "--ignore-submodules=all"])
            .output()
            .unwrap();
        assert!(status.status.success());
        assert_eq!(String::from_utf8_lossy(&status.stdout).trim(), "M a");
        let diff = command(&dir)
            .args(["diff", "--no-ext-diff", "--no-textconv", "HEAD"])
            .output()
            .unwrap();
        assert!(String::from_utf8_lossy(&diff.stdout).contains("+y"));
        assert!(
            !marker.exists(),
            "a program from the repository's config ran"
        );
        // Control: plain git does run them, so the check above means something.
        plain(&["status", "--porcelain"]);
        assert!(
            marker.exists(),
            "plain `git status` should have run the fsmonitor"
        );
        std::fs::remove_dir_all(&dir).ok();
    }
}
