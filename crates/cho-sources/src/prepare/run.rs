//! Running a recipe in one of Chorong's own worktrees: no shell, a timeout per step (the whole
//! process group is stopped), tokens removed from the environment, one `CARGO_TARGET_DIR` per
//! project, and the output logged in the cache.

use super::{Recipe, Step};
use crate::{Error, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// How a prepare run ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PrepareResult {
    Ok,
    /// A step exited with a failure status.
    Failed,
    TimedOut,
    /// A step's program is not installed; nothing ran.
    MissingTool,
    /// There was no recipe: nothing to run.
    NoRecipe,
}

impl PrepareResult {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Failed => "failed",
            Self::TimedOut => "timed-out",
            Self::MissingTool => "missing-tool",
            Self::NoRecipe => "no-recipe",
        }
    }

    /// The index built after this is `partial`.
    pub fn is_failure(self) -> bool {
        matches!(self, Self::Failed | Self::TimedOut | Self::MissingTool)
    }
}

/// What a worktree's last prepare was run with and how it ended (kept in `state.json`).
/// Prepare runs again only when the recipe, the trigger files or the toolchain differ.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PrepareRecord {
    pub recipe_hash: String,
    pub triggers_hash: String,
    /// Program → `--version` output, for the toolchain the recipe uses (`node`, `rustc`, and
    /// `pnpm`/`npm`/`yarn` when a step runs them).
    pub toolchain: BTreeMap<String, String>,
    pub result: PrepareResult,
    /// What went wrong, for a person.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    pub log: PathBuf,
    /// Seconds since the Unix epoch.
    pub finished: u64,
    pub duration_ms: u64,
}

impl PrepareRecord {
    /// True when this record already covers the given inputs, so prepare need not run again.
    /// A failure counts too: the same recipe, files and toolchain would fail the same way
    /// (installing a missing tool changes the toolchain; `--force` runs anyway).
    pub fn covers(
        &self,
        recipe_hash: &str,
        triggers_hash: &str,
        toolchain: &BTreeMap<String, String>,
    ) -> bool {
        self.recipe_hash == recipe_hash
            && self.triggers_hash == triggers_hash
            && &self.toolchain == toolchain
    }
}

/// True for variables that hold a host token or account, which prepare never sees:
/// `CHORONG_*TOKEN*`, `CHORONG_*EMAIL*`, `GITHUB_TOKEN`, `GH_TOKEN`, `GH_ENTERPRISE_TOKEN`,
/// `GITHUB_ENTERPRISE_TOKEN` and every `BITBUCKET_*`.
pub fn is_secret_var(name: &str) -> bool {
    let n = name.to_ascii_uppercase();
    (n.starts_with("CHORONG_") && (n.contains("TOKEN") || n.contains("EMAIL")))
        || matches!(
            n.as_str(),
            "GITHUB_TOKEN" | "GH_TOKEN" | "GH_ENTERPRISE_TOKEN" | "GITHUB_ENTERPRISE_TOKEN"
        )
        || n.starts_with("BITBUCKET_")
}

/// The environment prepare runs with: from `base`, only what a build needs (the allowlist
/// indexers use, [`cho_core::build_env`]) and never a host token, plus `CARGO_TARGET_DIR`
/// and no git prompts.
pub fn environment(
    base: &[(OsString, OsString)],
    cargo_target: &Path,
) -> Vec<(OsString, OsString)> {
    let mut env: Vec<(OsString, OsString)> = base
        .iter()
        .filter(|(k, _)| {
            let k = k.to_string_lossy();
            cho_core::build_env::allowed(&k) && !is_secret_var(&k)
        })
        .filter(|(k, _)| k != "CARGO_TARGET_DIR" && k != "GIT_TERMINAL_PROMPT")
        .cloned()
        .collect();
    env.push(("CARGO_TARGET_DIR".into(), cargo_target.into()));
    env.push(("GIT_TERMINAL_PROMPT".into(), "0".into()));
    env
}

/// Where `program` would be found: a path (relative to `cwd`) or a `PATH` lookup.
pub fn find_program(program: &str, cwd: &Path, env: &[(OsString, OsString)]) -> Option<PathBuf> {
    if program.contains('/') {
        let p = cwd.join(program);
        return p.is_file().then_some(p);
    }
    let path = env
        .iter()
        .find(|(k, _)| k == "PATH")
        .map(|(_, v)| v.clone())?;
    std::env::split_paths(&path)
        .map(|d| d.join(program))
        .find(|p| p.is_file())
}

/// The toolchain versions that decide whether prepare must run again.
pub fn toolchain(
    recipe: &Recipe,
    cwd: &Path,
    env: &[(OsString, OsString)],
) -> BTreeMap<String, String> {
    let mut tools: Vec<&str> = vec!["node", "rustc"];
    for s in &recipe.steps {
        if let Some(p) = s.argv.first().map(String::as_str)
            && matches!(p, "pnpm" | "npm" | "yarn")
            && !tools.contains(&p)
        {
            tools.push(p);
        }
    }
    let mut out = BTreeMap::new();
    if recipe.is_empty() {
        return out;
    }
    for t in tools {
        if find_program(t, cwd, env).is_none() {
            continue;
        }
        let mut c = Command::new(t);
        c.arg("--version")
            .current_dir(cwd)
            .env_clear()
            .envs(env.iter().map(|(k, v)| (k, v)))
            .stdin(Stdio::null())
            .stderr(Stdio::null());
        if let Some(o) = output_within(c, Duration::from_secs(20))
            && o.status.success()
        {
            out.insert(
                t.to_string(),
                String::from_utf8_lossy(&o.stdout).trim().to_string(),
            );
        }
    }
    out
}

fn output_within(mut c: Command, limit: Duration) -> Option<std::process::Output> {
    c.stdout(Stdio::piped());
    let mut child = c.spawn().ok()?;
    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => return child.wait_with_output().ok(),
            Ok(None) if start.elapsed() < limit => std::thread::sleep(Duration::from_millis(20)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
}

/// What [`run_recipe`] needs.
pub struct RunSpec<'a> {
    pub worktree: &'a Path,
    pub recipe: &'a Recipe,
    pub env: Vec<(OsString, OsString)>,
    pub log: &'a Path,
    /// Told when each step starts.
    pub progress: &'a dyn Fn(crate::PrepareStage),
    /// Stops the running step (and its children); the run ends `Cancelled`.
    pub cancel: &'a crate::Cancel,
}

/// The outcome of running the steps (the caller adds the hashes and toolchain).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StepsOutcome {
    pub result: PrepareResult,
    pub message: Option<String>,
    pub duration_ms: u64,
}

/// The largest prepare log. Step output goes straight to it, so a step that prints without
/// end (a hostile or broken recipe) would fill the disk until its timeout; past this the step
/// is stopped and fails. Checked every 50 ms, so it can overshoot by that much output.
pub const MAX_LOG_BYTES: u64 = 64 << 20;

fn log_len(log: &Path) -> u64 {
    std::fs::metadata(log).map_or(0, |m| m.len())
}

/// Runs the recipe's steps in order, stopping at the first that fails. Every program is
/// checked first, so a missing tool runs nothing. Output goes to `log`. Cancelling returns
/// `Err` (kind `Cancelled`) and records nothing, so the next run starts over.
pub fn run_recipe(spec: &RunSpec) -> Result<StepsOutcome> {
    let start = Instant::now();
    let done = |result, message: Option<String>| StepsOutcome {
        result,
        message,
        duration_ms: start.elapsed().as_millis() as u64,
    };
    if let Some(dir) = spec.log.parent() {
        std::fs::create_dir_all(dir)
            .map_err(|e| Error::msg(format!("cannot create {}: {e}", dir.display())))?;
    }
    let mut log = std::fs::File::create(spec.log)
        .map_err(|e| Error::msg(format!("cannot write {}: {e}", spec.log.display())))?;
    use std::io::Write;
    if spec.recipe.is_empty() {
        let _ = writeln!(log, "no recipe: nothing to run");
        return Ok(done(PrepareResult::NoRecipe, None));
    }
    // Check every step's folder and program before running any.
    let mut cwds = vec![];
    for (i, s) in spec.recipe.steps.iter().enumerate() {
        let cwd = step_dir(spec.worktree, s)?;
        let program = &s.argv[0];
        if find_program(program, &cwd, &spec.env).is_none() {
            let msg = format!(
                "`{program}` is not installed (needed by step {}: {})",
                i + 1,
                s.display()
            );
            let _ = writeln!(log, "{msg}");
            return Ok(done(PrepareResult::MissingTool, Some(msg)));
        }
        cwds.push(cwd);
    }
    let of = spec.recipe.steps.len();
    for (i, (s, cwd)) in spec.recipe.steps.iter().zip(cwds).enumerate() {
        spec.cancel.check()?;
        (spec.progress)(crate::PrepareStage {
            step: i + 1,
            of,
            command: s.display(),
        });
        let _ = writeln!(log, "$ {}", s.display());
        let _ = log.flush();
        let out = log
            .try_clone()
            .map_err(|e| Error::msg(format!("log: {e}")))?;
        let err = log
            .try_clone()
            .map_err(|e| Error::msg(format!("log: {e}")))?;
        let mut c = Command::new(&s.argv[0]);
        c.args(&s.argv[1..])
            .current_dir(&cwd)
            .env_clear()
            .envs(spec.env.iter().map(|(k, v)| (k, v)))
            .stdin(Stdio::null())
            .stdout(out)
            .stderr(err);
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            c.process_group(0);
        }
        let mut child = match c.spawn() {
            Ok(c) => c,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                let msg = format!(
                    "`{}` is not installed (needed by step {})",
                    s.argv[0],
                    i + 1
                );
                let _ = writeln!(log, "{msg}");
                return Ok(done(PrepareResult::MissingTool, Some(msg)));
            }
            Err(e) => {
                let msg = format!("step {} could not start: {e}", i + 1);
                let _ = writeln!(log, "{msg}");
                return Ok(done(PrepareResult::Failed, Some(msg)));
            }
        };
        let limit = Duration::from_secs(s.timeout_s);
        let step_start = Instant::now();
        let status = loop {
            if spec.cancel.is_cancelled() {
                crate::progress::stop_group(&mut child);
                let _ = writeln!(log, "cancelled during step {}", i + 1);
                return Err(Error::cancelled());
            }
            match child.try_wait() {
                Ok(Some(st)) => break Some(st),
                Ok(None) if log_len(spec.log) > MAX_LOG_BYTES => {
                    crate::progress::stop_group(&mut child);
                    let msg = format!(
                        "step {} stopped: its output passed {} MB: {}",
                        i + 1,
                        MAX_LOG_BYTES >> 20,
                        s.display()
                    );
                    let _ = writeln!(log, "{msg}");
                    return Ok(done(PrepareResult::Failed, Some(msg)));
                }
                Ok(None) if step_start.elapsed() < limit => {
                    std::thread::sleep(Duration::from_millis(50))
                }
                Ok(None) => break None,
                Err(e) => return Err(Error::msg(format!("waiting for step {}: {e}", i + 1))),
            }
        };
        match status {
            None => {
                crate::progress::stop_group(&mut child);
                let msg = format!(
                    "step {} timed out after {}s: {}",
                    i + 1,
                    s.timeout_s,
                    s.display()
                );
                let _ = writeln!(log, "{msg}");
                return Ok(done(PrepareResult::TimedOut, Some(msg)));
            }
            Some(st) if !st.success() => {
                let msg = format!(
                    "step {} failed ({}): {}",
                    i + 1,
                    st.code()
                        .map(|c| format!("exit {c}"))
                        .unwrap_or_else(|| "killed by a signal".into()),
                    s.display()
                );
                let _ = writeln!(log, "{msg}");
                return Ok(done(PrepareResult::Failed, Some(msg)));
            }
            Some(_) => {}
        }
    }
    let _ = writeln!(log, "prepare finished");
    Ok(done(PrepareResult::Ok, None))
}

/// The step's folder: inside the worktree, existing.
fn step_dir(worktree: &Path, s: &Step) -> Result<PathBuf> {
    let dir = worktree.join(&s.cwd);
    let (canon_dir, canon_wt) = (dir.canonicalize(), worktree.canonicalize());
    match (canon_dir, canon_wt) {
        (Ok(d), Ok(w)) if d.starts_with(&w) && d.is_dir() => Ok(d),
        _ => Err(Error::msg(format!(
            "step folder `{}` is not a folder inside the worktree",
            s.cwd
        ))),
    }
}
