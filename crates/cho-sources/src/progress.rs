//! Progress and cancellation for long work (opening a repository, running prepare): a
//! [`Stage`] callback, git's own `--progress` percentages, and a [`Cancel`] flag that stops
//! the running child process and everything it started.

use serde::Serialize;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// Set from any thread to stop the work it was given to. Cloning shares the flag.
#[derive(Debug, Clone, Default)]
pub struct Cancel(Arc<AtomicBool>);

impl Cancel {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn cancel(&self) {
        self.0.store(true, Ordering::SeqCst);
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }

    /// `Err(Cancelled)` once cancelled: the check between steps.
    pub fn check(&self) -> crate::Result<()> {
        if self.is_cancelled() {
            Err(crate::Error::cancelled())
        } else {
            Ok(())
        }
    }
}

/// One of git's progress lines: `Receiving objects:  45% (123/456)` → phase
/// `Receiving objects`, 45.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct GitProgress {
    pub phase: String,
    pub percent: u8,
}

/// Parses a progress line (`remote: ` prefixes allowed). `None` for anything else.
pub fn parse_progress(line: &str) -> Option<GitProgress> {
    let line = line.trim();
    let line = line.strip_prefix("remote:").map(str::trim).unwrap_or(line);
    let (phase, rest) = line.split_once(':')?;
    let rest = rest.trim_start();
    let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
    if digits.is_empty() || !rest[digits.len()..].starts_with('%') {
        return None;
    }
    let percent = digits.parse::<u32>().ok()?.min(100) as u8;
    let phase = phase.trim();
    (!phase.is_empty()).then(|| GitProgress {
        phase: phase.to_string(),
        percent,
    })
}

/// Where opening a repository is. Each stage is reported once when it starts (`progress:
/// None`) and then with git's percentages where git reports them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "stage", rename_all = "kebab-case")]
pub enum Stage {
    /// Waiting for the repository's lock (another open or clear of it).
    Lock,
    Clone {
        progress: Option<GitProgress>,
    },
    /// Listing the remote's branches.
    ListRemote,
    /// Reading the pull request's details from the host.
    PullRequest,
    Fetch {
        progress: Option<GitProgress>,
    },
    MergeBase,
    /// Fetching Before's copies of the changed files.
    Prefetch {
        progress: Option<GitProgress>,
    },
    Worktree {
        progress: Option<GitProgress>,
    },
}

/// Where a prepare run is: step `step` (one-based) of `of`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PrepareStage {
    pub step: usize,
    pub of: usize,
    pub command: String,
}

/// Stops a child and everything in its process group, then reaps it.
pub(crate) fn stop_group(child: &mut std::process::Child) {
    #[cfg(unix)]
    {
        let _ = std::process::Command::new("kill")
            .args(["-KILL", "--", &format!("-{}", child.id())])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
    }
    let _ = child.kill();
    let _ = child.wait();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_git_progress() {
        let p = |s: &str| parse_progress(s).map(|g| (g.phase, g.percent));
        assert_eq!(
            p("Receiving objects:  45% (123/456), 1.20 MiB | 2.00 MiB/s"),
            Some(("Receiving objects".into(), 45))
        );
        assert_eq!(
            p("remote: Counting objects: 100% (12/12), done."),
            Some(("Counting objects".into(), 100))
        );
        assert_eq!(
            p("Updating files:   7% (3/40)"),
            Some(("Updating files".into(), 7))
        );
        assert_eq!(p("Cloning into bare repository 'x'..."), None);
        assert_eq!(p("fatal: repository 'x' not found"), None);
        assert_eq!(p("remote: Enumerating objects: 5, done."), None);
    }

    #[test]
    fn cancel_is_shared() {
        let c = Cancel::new();
        let d = c.clone();
        assert!(c.check().is_ok());
        d.cancel();
        assert!(c.is_cancelled());
        assert_eq!(c.check().unwrap_err().kind, crate::ErrorKind::Cancelled);
    }
}
