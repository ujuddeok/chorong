//! The `git` command, for the network work and the writes gix can't do yet: a blobless bare
//! clone, fetches, and adding, moving and removing linked worktrees. Every command here runs
//! inside Chorong's cache (checked before it runs) with prompts turned off.

use crate::{Error, Result};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// The oldest git Chorong drives: `worktree add` in a partial clone, `--filter` on clone and
/// `fetch` with the promisor remote remembered all work from here.
pub const MIN_GIT: (u32, u32) = (2, 36);

/// How to run git, and the credential helper to fall back on. Every command is built by
/// `cho_core::git::own_repo_command` (these are repositories Chorong created in its cache).
#[derive(Debug, Clone, Default)]
pub struct Git {
    /// `cho credential`, as a program path; used only after the user's own setup failed and
    /// Chorong holds a token for the host.
    pub credential_helper: Option<PathBuf>,
}

/// What a git command printed when it failed.
#[derive(Debug, Clone, Default)]
pub struct GitFailure {
    pub args: String,
    pub stderr: String,
    /// It was stopped by a [`Cancel`](crate::Cancel).
    pub cancelled: bool,
}

impl GitFailure {
    /// The failure looks like a missing or refused credential, so retrying with a token can
    /// help.
    pub fn is_auth(&self) -> bool {
        let s = self.stderr.to_ascii_lowercase();
        [
            "authentication failed",
            "could not read username",
            "could not read password",
            "terminal prompts disabled",
            "permission denied",
            // git and curl's phrasing for HTTP 401 and 403, not the bare digits (which a ref
            // such as `refs/pull/401/head` contains).
            "returned error: 401",
            "returned error: 403",
            "http 401",
            "http 403",
            "403 forbidden",
            "repository not found",
            "invalid username or password",
            "access denied",
        ]
        .iter()
        .any(|p| s.contains(p))
    }

    /// The remote, or the ref asked for, doesn't exist.
    pub fn is_not_found(&self) -> bool {
        let s = self.stderr.to_ascii_lowercase();
        [
            "couldn't find remote ref",
            "does not appear to be a git repository",
            "no such remote ref",
            "not our ref",
        ]
        .iter()
        .any(|p| s.contains(p))
    }

    /// The [`Error`] this failure is, for a command talking to `host`: a short message
    /// naming the host and the cause; the git command and its stderr go in `detail`.
    pub fn to_error(&self, host: &str) -> Error {
        use crate::ErrorKind;
        let (kind, message) = if self.cancelled {
            (ErrorKind::Cancelled, "Cancelled".to_string())
        } else if self.is_offline() {
            (
                ErrorKind::Offline,
                format!("Can't reach {host} ({})", self.offline_cause()),
            )
        } else if self.is_auth() {
            (
                ErrorKind::Auth {
                    host: host.to_string(),
                },
                format!("{host} refused access"),
            )
        } else if self.is_not_found() {
            (ErrorKind::NotFound, format!("Not found on {host}"))
        } else {
            (
                ErrorKind::Other,
                format!("`git {}` failed: {}", self.subcommand(), self.first_line()),
            )
        };
        Error::new(kind, message).with_detail(self.to_string() + "\n" + &self.stderr_tail())
    }

    /// The git subcommand (`clone`, `fetch`, …), for a short message.
    fn subcommand(&self) -> &str {
        self.args
            .split_whitespace()
            .find(|a| !a.starts_with('-'))
            .unwrap_or("git")
    }

    /// The first `fatal:`/`error:` line of stderr (else the first line), shortened.
    fn first_line(&self) -> String {
        let lines: Vec<&str> = self
            .stderr
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .collect();
        let line = lines
            .iter()
            .find(|l| l.starts_with("fatal:") || l.starts_with("error:"))
            .or(lines.first())
            .copied()
            .unwrap_or("no message");
        let line = line
            .trim_start_matches("fatal:")
            .trim_start_matches("error:")
            .trim();
        let mut s: String = line.chars().take(120).collect();
        if line.chars().count() > 120 {
            s.push('…');
        }
        s
    }

    /// The last lines of stderr, for `detail`.
    fn stderr_tail(&self) -> String {
        let lines: Vec<&str> = self.stderr.lines().collect();
        lines[lines.len().saturating_sub(20)..].join("\n")
    }

    fn offline_cause(&self) -> &'static str {
        let s = self.stderr.to_ascii_lowercase();
        // curl says "Couldn't connect to server" for a refused connection.
        if s.contains("connection refused") || s.contains("couldn't connect to server") {
            "connection refused"
        } else if s.contains("could not resolve host") || s.contains("name resolution") {
            "host not found"
        } else if s.contains("timed out") {
            "timed out"
        } else if s.contains("unreachable") {
            "network unreachable"
        } else {
            "no connection"
        }
    }

    /// The failure looks like no network or an unreachable host.
    pub fn is_offline(&self) -> bool {
        let s = self.stderr.to_ascii_lowercase();
        [
            "could not resolve host",
            "network is unreachable",
            "connection timed out",
            "connection refused",
            "failed to connect",
            "temporary failure in name resolution",
        ]
        .iter()
        .any(|p| s.contains(p))
    }
}

impl std::fmt::Display for GitFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let first = self
            .stderr
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .take(3)
            .collect::<Vec<_>>()
            .join("; ");
        if self.cancelled {
            return write!(f, "`git {}` was cancelled", self.args);
        }
        write!(f, "`git {}` failed: {first}", self.args)
    }
}

impl Git {
    /// The one place cho-sources builds a git command: `cho_core::git::own_repo_command`
    /// (hooks, fsmonitor, optional locks and prompts off) in `dir`, or the current folder for
    /// commands that use no repository. Every `dir` is inside Chorong's cache.
    fn command(&self, dir: Option<&Path>) -> Command {
        let mut c = cho_core::git::own_repo_command(dir.unwrap_or(Path::new(".")));
        // Chorong reads code: Git LFS files stay pointer files rather than downloading content.
        c.env("GIT_LFS_SKIP_SMUDGE", "1")
            .env("GCM_INTERACTIVE", "never")
            .env("LC_ALL", "C")
            .stdin(Stdio::null());
        c
    }

    /// The full command for `args`; with `use_token`, Chorong's helper replaces the credential
    /// helpers (an empty `credential.helper` resets git's list, so only Chorong's remains).
    fn build(&self, dir: Option<&Path>, args: &[&str], use_token: bool) -> Command {
        let mut c = self.command(dir);
        if use_token && let Some(helper) = &self.credential_helper {
            c.args(["-c", "credential.helper="]).arg("-c").arg(format!(
                "credential.helper=!{} credential",
                shell_quote(helper)
            ));
        }
        c.args(args);
        c
    }

    /// Runs git and returns stdout. `dir` is passed as `-C`.
    pub fn run(
        &self,
        dir: Option<&Path>,
        args: &[&str],
    ) -> std::result::Result<String, GitFailure> {
        self.run_with(dir, args, false)
    }

    /// Like [`run`](Self::run); with `use_token`, replaces the credential helpers with Chorong's
    /// own for this one command (`-c credential.helper= -c credential.helper=!'cho'
    /// credential`), after the user's setup has already failed. The token itself is never on
    /// the command line: `cho credential` reads it from the keychain or environment.
    pub fn run_with(
        &self,
        dir: Option<&Path>,
        args: &[&str],
        use_token: bool,
    ) -> std::result::Result<String, GitFailure> {
        let mut c = self.build(dir, args, use_token);
        let fail = |stderr: String| GitFailure {
            args: args.join(" "),
            stderr,
            ..GitFailure::default()
        };
        let out = c
            .output()
            .map_err(|e| fail(format!("cannot run git: {e}")))?;
        if out.status.success() {
            Ok(String::from_utf8_lossy(&out.stdout).trim_end().to_string())
        } else {
            Err(fail(String::from_utf8_lossy(&out.stderr).into_owned()))
        }
    }

    /// Like [`run_with`](Self::run_with), reporting git's progress lines (pass `--progress`
    /// in `args` for clone and fetch) and stopping git and its children when `cancel` is set.
    pub fn run_watched(
        &self,
        dir: Option<&Path>,
        args: &[&str],
        use_token: bool,
        on_progress: &dyn Fn(crate::GitProgress),
        cancel: &crate::Cancel,
    ) -> std::result::Result<String, GitFailure> {
        use std::io::Read;
        let fail = |stderr: String, cancelled: bool| GitFailure {
            args: args.join(" "),
            stderr,
            cancelled,
        };
        if cancel.is_cancelled() {
            return Err(fail(String::new(), true));
        }
        let mut c = self.build(dir, args, use_token);
        c.stdout(Stdio::piped()).stderr(Stdio::piped());
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            c.process_group(0);
        }
        let mut child = c
            .spawn()
            .map_err(|e| fail(format!("cannot run git: {e}"), false))?;
        let mut stdout = child.stdout.take().expect("piped");
        let mut stderr = child.stderr.take().expect("piped");
        let out_thread = std::thread::spawn(move || {
            let mut b = Vec::new();
            let _ = stdout.read_to_end(&mut b);
            b
        });
        let (tx, rx) = std::sync::mpsc::channel::<String>();
        let err_thread = std::thread::spawn(move || {
            // Progress lines end in `\r`; the rest in `\n`.
            let (mut all, mut piece, mut buf) = (String::new(), Vec::new(), [0u8; 4096]);
            while let Ok(n) = stderr.read(&mut buf) {
                if n == 0 {
                    break;
                }
                for &b in &buf[..n] {
                    if b == b'\r' || b == b'\n' {
                        let line = String::from_utf8_lossy(&piece).into_owned();
                        piece.clear();
                        if !line.trim().is_empty() {
                            if b == b'\n' && crate::progress::parse_progress(&line).is_none() {
                                all.push_str(&line);
                                all.push('\n');
                            }
                            let _ = tx.send(line);
                        }
                    } else {
                        piece.push(b);
                    }
                }
            }
            all.push_str(&String::from_utf8_lossy(&piece));
            all
        });
        let drain = |rx: &std::sync::mpsc::Receiver<String>| {
            while let Ok(line) = rx.try_recv() {
                if let Some(p) = crate::progress::parse_progress(&line) {
                    on_progress(p);
                }
            }
        };
        let status = loop {
            drain(&rx);
            if cancel.is_cancelled() {
                crate::progress::stop_group(&mut child);
                let _ = out_thread.join();
                let _ = err_thread.join();
                return Err(fail(String::new(), true));
            }
            match child.try_wait() {
                Ok(Some(st)) => break st,
                Ok(None) => std::thread::sleep(std::time::Duration::from_millis(20)),
                Err(e) => return Err(fail(format!("waiting for git: {e}"), false)),
            }
        };
        let out = out_thread.join().unwrap_or_default();
        let err = err_thread.join().unwrap_or_default();
        drain(&rx);
        if status.success() {
            Ok(String::from_utf8_lossy(&out).trim_end().to_string())
        } else {
            Err(fail(err, false))
        }
    }

    /// Runs git and returns stdout unchanged (file contents).
    pub fn run_bytes(
        &self,
        dir: Option<&Path>,
        args: &[&str],
    ) -> std::result::Result<Vec<u8>, GitFailure> {
        let out = self
            .build(dir, args, false)
            .output()
            .map_err(|e| GitFailure {
                args: args.join(" "),
                stderr: format!("cannot run git: {e}"),
                ..GitFailure::default()
            })?;
        if out.status.success() {
            Ok(out.stdout)
        } else {
            Err(GitFailure {
                args: args.join(" "),
                stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
                ..GitFailure::default()
            })
        }
    }

    /// The installed version, or an error that says what to install.
    pub fn check_version(&self) -> Result<(u32, u32, u32)> {
        let text = self.run(None, &["--version"]).map_err(|e| {
            Error::msg(format!(
                "git is needed for remote sources and could not be run ({e}); install git {}.{} or newer",
                MIN_GIT.0, MIN_GIT.1
            ))
        })?;
        let v = parse_version(&text)
            .ok_or_else(|| Error::msg(format!("cannot read the git version from `{text}`")))?;
        if (v.0, v.1) < MIN_GIT {
            return Err(Error::unsupported(format!(
                "git {}.{}.{} is too old for remote sources; Chorong needs {}.{} or newer",
                v.0, v.1, v.2, MIN_GIT.0, MIN_GIT.1
            )));
        }
        Ok(v)
    }
}

/// `git version 2.43.0` (and vendor suffixes such as `2.39.3 (Apple Git-146)`).
pub fn parse_version(text: &str) -> Option<(u32, u32, u32)> {
    let v = text.trim().strip_prefix("git version ")?;
    let v = v.split_whitespace().next()?;
    let mut it = v.split('.').map(|p| {
        p.chars()
            .take_while(char::is_ascii_digit)
            .collect::<String>()
            .parse::<u32>()
    });
    let major = it.next()?.ok()?;
    let minor = it.next()?.ok()?;
    let patch = it.next().and_then(|p| p.ok()).unwrap_or(0);
    Some((major, minor, patch))
}

/// Single-quoted for the shell git runs `!` helpers with.
fn shell_quote(p: &Path) -> String {
    format!("'{}'", p.to_string_lossy().replace('\'', r"'\''"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions() {
        assert_eq!(parse_version("git version 2.43.0"), Some((2, 43, 0)));
        assert_eq!(
            parse_version("git version 2.39.3 (Apple Git-146)"),
            Some((2, 39, 3))
        );
        assert_eq!(
            parse_version("git version 2.45.1.windows.1"),
            Some((2, 45, 1))
        );
        assert_eq!(parse_version("git version 2.36"), Some((2, 36, 0)));
        assert_eq!(parse_version("nonsense"), None);
    }

    #[test]
    fn quoting() {
        assert_eq!(shell_quote(Path::new("/a b/chorong")), "'/a b/chorong'");
        assert_eq!(shell_quote(Path::new("/it's")), r"'/it'\''s'");
    }

    /// A credential helper script that answers with `password=<name>`.
    fn helper(dir: &Path, name: &str) -> PathBuf {
        let p = dir.join(name);
        std::fs::write(
            &p,
            format!("#!/bin/sh\necho username=u\necho password={name}\n"),
        )
        .unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        p
    }

    /// What `git credential fill` answers through the builder, with a global helper set.
    fn fill(git: &Git, dir: &Path, global: &Path, use_token: bool) -> String {
        use std::io::Write;
        let mut c = git.build(Some(dir), &["credential", "fill"], use_token);
        c.env("GIT_CONFIG_GLOBAL", global)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped());
        let mut child = c.spawn().unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(b"protocol=https\nhost=example.com\n\n")
            .unwrap();
        String::from_utf8_lossy(&child.wait_with_output().unwrap().stdout).into_owned()
    }

    /// The user's global helper is consulted through the builder; with the token fallback,
    /// Chorong's helper is the only one left (an empty value resets git's list).
    #[test]
    fn global_helper_then_cho_helper() {
        let dir = tempfile::tempdir().unwrap();
        let global_helper = helper(dir.path(), "global");
        let cho = helper(dir.path(), "cho");
        let global = dir.path().join("gitconfig");
        std::fs::write(
            &global,
            format!("[credential]\n\thelper = !'{}'\n", global_helper.display()),
        )
        .unwrap();
        let git = Git {
            credential_helper: Some(cho),
        };
        assert!(fill(&git, dir.path(), &global, false).contains("password=global"));
        let with_token = fill(&git, dir.path(), &global, true);
        assert!(with_token.contains("password=cho"), "{with_token}");
    }

    #[test]
    fn classifies_failures() {
        let f = |s: &str| GitFailure {
            args: "fetch".into(),
            stderr: s.into(),
            ..GitFailure::default()
        };
        assert!(
            f("fatal: could not read Username for 'https://github.com': terminal prompts disabled")
                .is_auth()
        );
        assert!(f("fatal: unable to access 'https://x/': Could not resolve host: x").is_offline());
        assert!(!f("fatal: couldn't find remote ref refs/heads/nope").is_auth());
        // HTTP refusals as git and curl word them.
        for s in [
            "fatal: unable to access 'https://h/a.git/': The requested URL returned error: 403",
            "error: RPC failed; HTTP 401 curl 22 The requested URL returned error: 401",
            "remote: 403 Forbidden",
        ] {
            assert!(f(s).is_auth(), "{s}");
        }
        // A pull request numbered like a status code is not an auth failure.
        let pr = f("fatal: couldn't find remote ref refs/pull/401/head");
        assert!(!pr.is_auth() && pr.is_not_found());
        assert_eq!(pr.to_error("h").kind, crate::ErrorKind::NotFound);
        assert!(!f("fatal: couldn't find remote ref refs/pull-requests/403/from").is_auth());
    }
}
