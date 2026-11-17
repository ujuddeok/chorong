//! Detecting projects and running the batch indexers as subprocesses.

use crate::SemanticError;
use crate::cache::{BuildResult, Failure, GitState, INDEX_FILE, IndexCache, Meta, now_unix};
use crate::progress::{Progress, Tracker};
use protobuf::CodedInputStream;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(900);
const STDERR_TAIL_BYTES: usize = 4096;
/// How often a running build reports (elapsed time and the indexer's step): a few times a second.
pub const TICK: Duration = Duration::from_millis(250);
/// Longest output line that is read for progress; the rest of a longer line is dropped.
const MAX_LINE_BYTES: usize = 4096;
/// Default Node heap limit for scip-typescript, in MB (the `index.node_memory_mb` setting).
pub const DEFAULT_NODE_MEMORY_MB: u32 = 8192;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Language {
    Rust,
    TypeScript,
    Python,
}

impl Language {
    pub const ALL: [Language; 3] = [Language::Rust, Language::TypeScript, Language::Python];

    pub fn name(self) -> &'static str {
        match self {
            Language::Rust => "rust",
            Language::TypeScript => "typescript",
            Language::Python => "python",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s.to_ascii_lowercase().as_str() {
            "rust" | "rs" => Some(Language::Rust),
            "typescript" | "ts" | "javascript" | "js" => Some(Language::TypeScript),
            "python" | "py" => Some(Language::Python),
            _ => None,
        }
    }

    /// The indexer's name: rust-analyzer is looked up on `PATH`; the other two are Chorong's own
    /// copies, run with `node` ([`crate::tools`]).
    pub fn indexer(self) -> &'static str {
        match self {
            Language::Rust => "rust-analyzer",
            Language::TypeScript => "scip-typescript",
            Language::Python => "scip-python",
        }
    }

    fn markers(self) -> &'static [&'static str] {
        match self {
            Language::Rust => &["Cargo.toml"],
            Language::TypeScript => &["tsconfig.json", "package.json"],
            Language::Python => &[
                "pyproject.toml",
                "setup.py",
                "setup.cfg",
                "requirements.txt",
            ],
        }
    }
}

/// One indexable project: a directory with a language's marker file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Project {
    pub language: Language,
    pub root: PathBuf,
    /// `root` relative to the repository root, `/`-separated; empty for the repository root.
    pub rel: String,
}

impl Project {
    /// The cache directory name for this project.
    pub fn slug(&self) -> String {
        if self.rel.is_empty() {
            self.language.name().to_string()
        } else {
            let rel: String = self
                .rel
                .chars()
                .map(|c| {
                    if c.is_ascii_alphanumeric() || c == '-' {
                        c
                    } else {
                        '_'
                    }
                })
                .collect();
            format!("{}-{rel}", self.language.name())
        }
    }
}

const SKIP_DIRS: &[&str] = &[
    "node_modules",
    "target",
    "vendor",
    "dist",
    "build",
    "venv",
    "__pycache__",
    "site-packages",
];
const MAX_DEPTH: usize = 2;

/// Finds projects at the root and up to two directories below it. A project
/// claims its subtree for its own language (Cargo workspace members are not
/// separate projects).
pub fn detect(root: &Path, only: Option<Language>) -> Vec<Project> {
    let mut out = vec![];
    walk(root, root, 0, &mut vec![], only, &mut out);
    out
}

fn walk(
    root: &Path,
    dir: &Path,
    depth: usize,
    claimed: &mut Vec<Language>,
    only: Option<Language>,
    out: &mut Vec<Project>,
) {
    let mut mine = vec![];
    for lang in Language::ALL {
        if only.is_some_and(|o| o != lang) || claimed.contains(&lang) {
            continue;
        }
        if lang.markers().iter().any(|m| dir.join(m).is_file()) {
            let rel = dir.strip_prefix(root).unwrap_or(Path::new(""));
            out.push(Project {
                language: lang,
                root: dir.to_path_buf(),
                rel: rel
                    .components()
                    .map(|c| c.as_os_str().to_string_lossy())
                    .collect::<Vec<_>>()
                    .join("/"),
            });
            mine.push(lang);
        }
    }
    claimed.extend(mine.iter().copied());
    if depth < MAX_DEPTH
        && let Ok(rd) = std::fs::read_dir(dir)
    {
        let mut subdirs: Vec<PathBuf> = rd
            .flatten()
            .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
            .filter(|e| {
                let name = e.file_name().to_string_lossy().into_owned();
                !name.starts_with('.') && !SKIP_DIRS.contains(&name.as_str())
            })
            .map(|e| e.path())
            .collect();
        subdirs.sort();
        for sub in subdirs {
            walk(root, &sub, depth + 1, claimed, only, out);
        }
    }
    claimed.truncate(claimed.len() - mine.len());
}

/// A subprocess to run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexerCommand {
    pub program: String,
    pub args: Vec<String>,
    pub cwd: PathBuf,
    pub env: Vec<(String, String)>,
}

/// Largest source file scip-typescript indexes (its own default is 1 mb).
const TS_MAX_FILE_SIZE: &str = "8mb";

/// How a Node indexer (scip-typescript, scip-python) is started: Chorong's `node` and the
/// indexer's script, both absolute ([`crate::tools`]), and for Python the project's virtual
/// environment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NodeRun {
    pub node: PathBuf,
    pub script: PathBuf,
    /// The project's `.venv`, put first on `PATH` so imports from its packages resolve.
    pub venv: Option<PathBuf>,
}

/// The command that writes `project`'s index to `out`. `scratch` is a
/// directory outside the repository for tool output such as Cargo's target
/// directory, so indexing never writes into the repository. `js` says how the Node indexers
/// start; without it their command names the indexer alone, which is not found.
pub fn command(
    project: &Project,
    out: &Path,
    scratch: &Path,
    js: Option<&NodeRun>,
) -> IndexerCommand {
    let s = |p: &Path| p.to_string_lossy().into_owned();
    match project.language {
        Language::Rust => IndexerCommand {
            program: "rust-analyzer".into(),
            args: vec!["scip".into(), s(&project.root), "--output".into(), s(out)],
            cwd: project.root.clone(),
            env: if std::env::var_os("CARGO_TARGET_DIR").is_some() {
                vec![]
            } else {
                vec![("CARGO_TARGET_DIR".into(), s(&scratch.join("cargo-target")))]
            },
        },
        Language::TypeScript | Language::Python => {
            let Some(js) = js else {
                return IndexerCommand {
                    program: project.language.indexer().into(),
                    args: vec![],
                    cwd: project.root.clone(),
                    env: vec![],
                };
            };
            let mut args = vec![s(&js.script), "index".into()];
            let node_dir = js.node.parent().unwrap_or(Path::new("/"));
            let mut env = vec![];
            if project.language == Language::TypeScript {
                // Without this, files over 1 MB (generated parsers, a type checker in one
                // file) are skipped silently and every identifier in them reads "no symbol in
                // the index".
                args.extend([
                    "--max-file-byte-size".into(),
                    TS_MAX_FILE_SIZE.into(),
                    "--output".into(),
                    s(out),
                ]);
                // A pnpm workspace usually has no root `tsconfig.json`, so a plain run fails
                // with "missing tsconfig.json". Chorong lists the packages itself, root first as
                // `pnpm ls -r` does: scip-typescript's `--pnpm-workspaces` would run `pnpm` in
                // the repository, which reads the repository's settings.
                args.extend(
                    crate::workspace::pnpm_projects(&project.root)
                        .iter()
                        .map(|p| s(p)),
                );
                env.push(("PATH".into(), crate::tools::path_with(&[node_dir])));
                // The Node heap comes from the `index.node_memory_mb` setting (applied when
                // the indexer runs; a user-set NODE_OPTIONS wins).
            } else {
                let name = project
                    .root
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| "project".into());
                args.extend(["--project-name".into(), name, "--output".into(), s(out)]);
                // scip-python starts `python3 -c …` in the project: without this, Python would
                // put the project folder first on `sys.path` and import its modules.
                env.push(("PYTHONSAFEPATH".into(), "1".into()));
                match &js.venv {
                    Some(venv) => {
                        let bin = venv.join(if cfg!(windows) { "Scripts" } else { "bin" });
                        env.push(("PATH".into(), crate::tools::path_with(&[&bin, node_dir])));
                        env.push(("VIRTUAL_ENV".into(), s(venv)));
                    }
                    None => env.push(("PATH".into(), crate::tools::path_with(&[node_dir]))),
                }
            }
            IndexerCommand {
                program: s(&js.node),
                args,
                // scip-typescript resolves type roots from its working directory.
                cwd: project.root.clone(),
                env,
            }
        }
    }
}

/// What building one project's index runs, and whether the project's own code runs: for the
/// banner that offers the build ([`build_plan`] fills `venv` from the file system).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildPlan {
    pub language: Language,
    /// The indexer, for a person: `rust-analyzer scip`, `scip-typescript (Chorong's copy)`.
    pub runs: String,
    /// The project's own code runs during the build.
    pub runs_project_code: bool,
    /// Why it does or doesn't, in a few words.
    pub why: &'static str,
}

impl BuildPlan {
    /// The plan for a `language` project, with or without a Python virtual environment. Pure.
    pub fn of(language: Language, has_venv: bool) -> Self {
        match language {
            Language::Rust => Self {
                language,
                runs: "rust-analyzer scip".into(),
                runs_project_code: true,
                why: "rust-analyzer runs the project's build scripts and procedural macros",
            },
            Language::TypeScript => Self {
                language,
                runs: "scip-typescript (Chorong's copy, with node)".into(),
                runs_project_code: false,
                why: "scip-typescript reads the project's files and runs none of them",
            },
            Language::Python if has_venv => Self {
                language,
                runs: "scip-python (Chorong's copy, with node) and the project's .venv Python"
                    .into(),
                runs_project_code: true,
                why: "starting the project's .venv Python runs its installed packages' .pth files",
            },
            Language::Python => Self {
                language,
                runs: "scip-python (Chorong's copy, with node) and python3 from PATH".into(),
                runs_project_code: false,
                why: "scip-python reads the project's files; Python starts with PYTHONSAFEPATH=1, so nothing is imported from the project",
            },
        }
    }
}

/// [`BuildPlan::of`] for `project`, looking for its `.venv` ([`crate::tools::project_venv`]).
pub fn build_plan(project: &Project) -> BuildPlan {
    let venv =
        project.language == Language::Python && crate::tools::project_venv(project).is_some();
    BuildPlan::of(project.language, venv)
}

/// Whether an indexer may see `name`: the shared build-environment allowlist
/// ([`cho_core::build_env`]), which prepare uses too.
/// Indexing runs below the viewer's priority (`nice -n 10`, which its children inherit), so a
/// build that takes every core still yields the CPU to reading: without it, opening a file
/// while the first index builds stalls. `nice` replaces itself with the program, so the pid
/// and process group are the program's. The program is looked up on the indexer's own PATH
/// first, so a missing one is still reported as such (`None`). Where `nice` is missing, or
/// off Unix, the program runs at normal priority.
fn lowered(cmd: &IndexerCommand, low_priority: bool) -> Option<Command> {
    const NICE: &str = "/usr/bin/nice";
    let program = find_program(cmd)?;
    if low_priority && cfg!(unix) && Path::new(NICE).is_file() {
        let mut c = Command::new(NICE);
        c.arg("-n").arg("10").arg(program);
        Some(c)
    } else {
        Some(Command::new(program))
    }
}

/// `cmd.program` as a path: as given when it names a path, else the first match on the PATH
/// the indexer will get (its own `PATH` setting, or the viewer's).
fn find_program(cmd: &IndexerCommand) -> Option<PathBuf> {
    let program = Path::new(&cmd.program);
    if program.components().count() > 1 {
        let p = if program.is_absolute() {
            program.to_path_buf()
        } else {
            cmd.cwd.join(program)
        };
        return p.is_file().then_some(p);
    }
    let path = cmd
        .env
        .iter()
        .rev()
        .find(|(k, _)| k == "PATH")
        .map(|(_, v)| std::ffi::OsString::from(v))
        .or_else(|| std::env::var_os("PATH"))?;
    std::env::split_paths(&path)
        .map(|dir| dir.join(program))
        .find(|p| p.is_file())
}

/// `cmd` ready to spawn, under the rules every indexer subprocess runs by: below the
/// viewer's priority ([`lowered`]), in its own directory, and with only the build-environment
/// allowlist ([`cho_core::build_env`], which prepare uses too) plus `cmd.env`. `None` when
/// the program isn't found.
fn prepared(cmd: &IndexerCommand, low_priority: bool) -> Option<Command> {
    let mut command = lowered(cmd, low_priority)?;
    command
        .args(&cmd.args)
        .current_dir(&cmd.cwd)
        .env_clear()
        .envs(
            std::env::vars_os()
                .filter(|(k, _)| k.to_str().is_some_and(cho_core::build_env::allowed)),
        )
        .envs(cmd.env.iter().map(|(k, v)| (k, v)));
    Some(command)
}

/// How a subprocess ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ended {
    /// The program could not be started (the report says why).
    NotStarted,
    /// It exited by itself. `code` is `None` when a signal ended it.
    Exited { code: Option<i32> },
    /// Stopped at the timeout.
    TimedOut,
    /// Stopped because the caller cancelled.
    Cancelled,
}

/// A finished subprocess: how it ended, the end of its stderr, and how long it ran.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunOutcome {
    pub ended: Ended,
    pub stderr_tail: String,
    pub duration: Duration,
}

impl RunOutcome {
    /// Exited by itself with status 0.
    pub fn success(&self) -> bool {
        self.ended == Ended::Exited { code: Some(0) }
    }
}

/// Asks a running build to stop. Clones share one flag, so any holder can cancel.
#[derive(Debug, Clone, Default)]
pub struct CancelToken(Arc<AtomicBool>);

impl CancelToken {
    pub fn new() -> Self {
        Self::default()
    }

    /// A token backed by `flag`, e.g. one a signal handler sets.
    pub fn from_flag(flag: Arc<AtomicBool>) -> Self {
        Self(flag)
    }

    pub fn cancel(&self) {
        self.0.store(true, Ordering::SeqCst);
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }
}

/// A report from a running build, about every [`TICK`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tick {
    pub elapsed: Duration,
    pub progress: Progress,
}

/// The end of a stream, kept for a failure report: the last [`STDERR_TAIL_BYTES`] of its lines,
/// without Node's native stack frames (`  42: 0x1004… node::…`, over a hundred lines after a
/// crash), plus the lines that say what went wrong (`FATAL ERROR: …`, `Error: …`) wherever they
/// were, so a long trace can't push the reason out.
#[derive(Default)]
struct Tail {
    lines: std::collections::VecDeque<String>,
    bytes: usize,
    reasons: Vec<String>,
}

/// How many reason lines a report keeps, so a flood of errors stays bounded.
const MAX_REASONS: usize = 4;

impl Tail {
    fn is_native_frame(line: &str) -> bool {
        let t = line.trim_start();
        let digits = t.chars().take_while(char::is_ascii_digit).count();
        digits > 0 && t[digits..].starts_with(": 0x")
    }

    fn is_reason(line: &str) -> bool {
        let t = line.trim_start();
        [
            "FATAL ERROR",
            "Fatal error",
            "Fatal JavaScript",
            "Check failed",
            "Error:",
        ]
        .iter()
        .any(|p| t.starts_with(p))
            || t.contains("heap out of memory")
    }

    fn push(&mut self, line: &str) {
        if line.trim().is_empty() || Self::is_native_frame(line) {
            return;
        }
        if Self::is_reason(line) && self.reasons.len() < MAX_REASONS {
            self.reasons.push(line.to_string());
        }
        self.bytes += line.len() + 1;
        self.lines.push_back(line.to_string());
        while self.bytes > STDERR_TAIL_BYTES && self.lines.len() > 1 {
            if let Some(old) = self.lines.pop_front() {
                self.bytes -= old.len() + 1;
            }
        }
    }

    /// The reasons that scrolled out of the tail, then the tail.
    fn finish(self) -> String {
        let mut out: Vec<String> = self
            .reasons
            .into_iter()
            .filter(|r| !self.lines.contains(r))
            .collect();
        out.extend(self.lines);
        out.join("\n")
    }
}

/// The first line of a failure report that says what went wrong (`FATAL ERROR: …`, `Error: …`),
/// for a one-line summary.
pub fn failure_reason(report: &str) -> Option<&str> {
    report.lines().find(|l| Tail::is_reason(l)).map(str::trim)
}

/// At most `max` lines of a failure report for a short display: its reason lines first, then
/// as many of its last lines as fit, with `…` between when lines were left out.
pub fn report_lines(report: &str, max: usize) -> Vec<String> {
    let all: Vec<&str> = report.lines().filter(|l| !l.trim().is_empty()).collect();
    if all.len() <= max {
        return all.into_iter().map(String::from).collect();
    }
    let reasons: Vec<&str> = all
        .iter()
        .copied()
        .filter(|l| Tail::is_reason(l))
        .take(max.saturating_sub(2).max(1))
        .collect();
    let room = max.saturating_sub(reasons.len() + usize::from(!reasons.is_empty()));
    let end: Vec<&str> = all[all.len().saturating_sub(room)..]
        .iter()
        .copied()
        .filter(|l| !reasons.contains(l))
        .collect();
    let mut out: Vec<String> = reasons.into_iter().map(String::from).collect();
    if !out.is_empty() {
        out.push("…".into());
    }
    out.extend(end.into_iter().map(String::from));
    out
}

/// Feeds each line (split on `\n` or `\r`, since progress bars redraw with `\r`) to `tracker`.
/// With `keep_tail`, returns the end of the stream too ([`Tail`]).
fn read_stream(mut stream: impl Read, tracker: &Mutex<Tracker>, keep_tail: bool) -> String {
    let mut tail = Tail::default();
    let mut line: Vec<u8> = vec![];
    let mut buf = [0u8; 8192];
    let mut feed = |line: &mut Vec<u8>| {
        if !line.is_empty() {
            let text = String::from_utf8_lossy(line);
            if let Ok(mut t) = tracker.lock() {
                t.feed(&text);
            }
            if keep_tail {
                tail.push(&text);
            }
            line.clear();
        }
    };
    while let Ok(n) = stream.read(&mut buf) {
        if n == 0 {
            break;
        }
        for &b in &buf[..n] {
            if b == b'\n' || b == b'\r' {
                feed(&mut line);
            } else if line.len() < MAX_LINE_BYTES {
                line.push(b);
            }
        }
    }
    feed(&mut line);
    tail.finish()
}

/// [`run_with`] that is never cancelled and reports nothing.
#[cfg(test)]
fn run(cmd: &IndexerCommand, timeout: Duration) -> RunOutcome {
    run_with(cmd, timeout, &CancelToken::new(), &mut |_| {})
}

/// Runs `cmd` to completion, until `timeout` or until `cancel`, keeping the last few KiB of
/// stderr. A program that can't start is a failed outcome, not an error. `on_tick` is called
/// from this thread about every [`TICK`] with the elapsed time and the step parsed from the
/// indexer's output. Cancelling stops the whole process group (`npx` and the Node it started).
pub fn run_with(
    cmd: &IndexerCommand,
    timeout: Duration,
    cancel: &CancelToken,
    on_tick: &mut dyn FnMut(&Tick),
) -> RunOutcome {
    run_inner(cmd, timeout, cancel, on_tick, false, true).0
}

/// [`run_with`] at normal priority when `low_priority` is false ([`Options::low_priority`]).
pub fn run_with_priority(
    cmd: &IndexerCommand,
    timeout: Duration,
    cancel: &CancelToken,
    on_tick: &mut dyn FnMut(&Tick),
    low_priority: bool,
) -> RunOutcome {
    run_inner(cmd, timeout, cancel, on_tick, false, low_priority).0
}

/// [`run_with`] that reports nothing and also returns the end of stdout (the last few KiB of
/// its lines), for a program whose answer is its output (`npm view`).
pub(crate) fn run_with_output(
    cmd: &IndexerCommand,
    timeout: Duration,
    cancel: &CancelToken,
) -> (RunOutcome, String) {
    run_inner(cmd, timeout, cancel, &mut |_| {}, true, true)
}

fn run_inner(
    cmd: &IndexerCommand,
    timeout: Duration,
    cancel: &CancelToken,
    on_tick: &mut dyn FnMut(&Tick),
    keep_stdout: bool,
    low_priority: bool,
) -> (RunOutcome, String) {
    let start = Instant::now();
    let failed = |msg: String| RunOutcome {
        ended: Ended::NotStarted,
        stderr_tail: msg,
        duration: start.elapsed(),
    };
    let mut command = match prepared(cmd, low_priority) {
        Some(c) => c,
        None => {
            return (
                failed(format!("cannot run `{}`: not found", cmd.program)),
                String::new(),
            );
        }
    };
    #[cfg(unix)]
    {
        // Own process group, so a timeout can stop `npx` and its children too.
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    let mut child = match command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(c) => c,
        Err(e) => {
            return (
                failed(format!("cannot run `{}`: {e}", cmd.program)),
                String::new(),
            );
        }
    };
    let tracker = Arc::new(Mutex::new(Tracker::default()));
    let stdout = child.stdout.take().expect("piped");
    let stderr = child.stderr.take().expect("piped");
    let t = tracker.clone();
    let (out_tx, out_rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        out_tx.send(read_stream(stdout, &t, keep_stdout)).ok();
    });
    let (tx, rx) = std::sync::mpsc::channel();
    let t = tracker.clone();
    std::thread::spawn(move || {
        tx.send(read_stream(stderr, &t, true)).ok();
    });
    let tick = |on_tick: &mut dyn FnMut(&Tick)| {
        let progress = tracker.lock().map(|t| t.snapshot()).unwrap_or_default();
        on_tick(&Tick {
            elapsed: start.elapsed(),
            progress,
        });
    };
    let mut next_tick = Instant::now();
    let ended = loop {
        if Instant::now() >= next_tick {
            tick(on_tick);
            next_tick = Instant::now() + TICK;
        }
        match child.try_wait() {
            Ok(Some(status)) => {
                break Ended::Exited {
                    code: status.code(),
                };
            }
            Ok(None) if cancel.is_cancelled() => {
                stop_group(&mut child);
                child.wait().ok();
                break Ended::Cancelled;
            }
            Ok(None) if start.elapsed() >= timeout => {
                kill_group(&child);
                child.kill().ok();
                child.wait().ok();
                break Ended::TimedOut;
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(25)),
            Err(_) => break Ended::Exited { code: None },
        }
    };
    tick(on_tick);
    // A grandchild that outlives its parent may hold the pipe open; don't wait for it.
    let mut stderr_tail = rx
        .recv_timeout(Duration::from_millis(500))
        .unwrap_or_default();
    match ended {
        Ended::TimedOut => {
            stderr_tail.push_str(&format!("\n[timed out after {}s]", timeout.as_secs()))
        }
        Ended::Cancelled => stderr_tail.push_str("\n[cancelled]"),
        Ended::NotStarted | Ended::Exited { .. } => {}
    }
    let stdout = if keep_stdout {
        out_rx
            .recv_timeout(Duration::from_millis(500))
            .unwrap_or_default()
    } else {
        String::new()
    };
    (
        RunOutcome {
            ended,
            stderr_tail,
            duration: start.elapsed(),
        },
        stdout,
    )
}

/// Stops the process group politely (SIGTERM), then for certain (SIGKILL) after a second.
#[cfg(unix)]
fn stop_group(child: &mut std::process::Child) {
    Command::new("kill")
        .args(["-TERM", "--", &format!("-{}", child.id())])
        .stderr(Stdio::null())
        .status()
        .ok();
    let deadline = Instant::now() + Duration::from_secs(1);
    while Instant::now() < deadline && matches!(child.try_wait(), Ok(None)) {
        std::thread::sleep(Duration::from_millis(25));
    }
    // Also reaches grandchildren that ignored SIGTERM.
    kill_group(child);
    child.kill().ok();
}

#[cfg(not(unix))]
fn stop_group(child: &mut std::process::Child) {
    child.kill().ok();
}

#[cfg(unix)]
pub(crate) fn kill_group(child: &std::process::Child) {
    Command::new("kill")
        .args(["-KILL", "--", &format!("-{}", child.id())])
        .stderr(Stdio::null())
        .status()
        .ok();
}

#[cfg(not(unix))]
pub(crate) fn kill_group(_: &std::process::Child) {}

/// The indexer's version, for `meta.json`. rust-analyzer is asked (`--version`), as the
/// program the build ran: the same lookup, environment and priority ([`prepared`]). The npm
/// indexers record theirs in the index, which is read up to its metadata only.
fn version_of(language: Language, build: &IndexerCommand, index: &Path) -> Option<String> {
    match language {
        Language::Rust => {
            let probe = IndexerCommand {
                program: language.indexer().into(),
                args: vec!["--version".into()],
                ..build.clone()
            };
            let out = prepared(&probe, true)?
                .stdin(Stdio::null())
                .stderr(Stdio::null())
                .output()
                .ok()?;
            out.status
                .success()
                .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
        }
        _ => index_tool_version(index),
    }
}

/// `metadata.tool_info.version` of a SCIP index file, read without decoding its documents:
/// the fields before the metadata (normally none) are skipped, the rest is never read.
#[allow(clippy::disallowed_methods)] // the index file the indexer just wrote
fn index_tool_version(index: &Path) -> Option<String> {
    /// `Index.metadata` is field 1.
    const METADATA: u32 = 1;
    let mut file = std::io::BufReader::new(std::fs::File::open(index).ok()?);
    let mut input = CodedInputStream::new(&mut file);
    while let Some(tag) = input.read_raw_tag_or_eof().ok()? {
        if tag >> 3 != METADATA {
            input
                .skip_field(protobuf::rt::WireType::new(tag & 7)?)
                .ok()?;
            continue;
        }
        let meta: scip::types::Metadata = input.read_message().ok()?;
        let version = meta.tool_info.as_ref()?.version.clone();
        return Some(version).filter(|v| !v.is_empty());
    }
    None
}

/// How a build is run.
#[derive(Debug, Clone)]
pub struct Options {
    pub timeout: Duration,
    pub cancel: CancelToken,
    /// Heap limit for the TypeScript indexer's Node process (the `index.node_memory_mb` setting).
    pub node_memory_mb: u32,
    /// A failed prepare that ran before this build; the index then reads `partial`.
    pub prepare: Option<crate::cache::PrepareNote>,
    /// Where `node` and `npm` are and which indexer versions run (the `tools.*` and
    /// `index.*_version` settings).
    pub tools: crate::tools::ToolConfig,
    /// Run the indexer below the viewer's priority (`nice -n 10`); false runs it at normal
    /// priority for a faster build (the `index.low_priority` setting). Default true.
    pub low_priority: bool,
}

impl Options {
    pub fn new(timeout: Duration) -> Self {
        Self {
            timeout,
            cancel: CancelToken::new(),
            node_memory_mb: DEFAULT_NODE_MEMORY_MB,
            prepare: None,
            tools: crate::tools::ToolConfig::default(),
            low_priority: true,
        }
    }
}

/// How a build ended.
#[derive(Debug, Clone)]
#[allow(clippy::large_enum_variant)] // one value per build; boxing would only add noise
pub enum Refresh {
    /// Finished, successfully or not; `meta.json` records it.
    Done(Meta),
    /// Cancelled. Nothing was written: the previous index and `meta.json` are as they were.
    Cancelled,
}

/// `NODE_OPTIONS` for the TypeScript indexer: the reader's own value with a heap limit added,
/// unless they already chose a heap size (theirs wins). `None` leaves the variable as it is.
fn node_options(existing: Option<&str>, memory_mb: u32) -> Option<String> {
    let existing = existing.unwrap_or("").trim();
    if existing.contains("--max-old-space-size") {
        return None;
    }
    let ours = format!("--max-old-space-size={memory_mb}");
    Some(if existing.is_empty() {
        ours
    } else {
        format!("{existing} {ours}")
    })
}

/// Gives a TypeScript command its heap limit. The value is passed to the child explicitly, and
/// `NODE_OPTIONS` is on the environment allowlist so a limit the reader set reaches it too.
fn apply_node_memory(cmd: &mut IndexerCommand, language: Language, memory_mb: u32) {
    if language != Language::TypeScript {
        return;
    }
    cmd.env.retain(|(k, _)| k != "NODE_OPTIONS");
    if let Some(v) = node_options(std::env::var("NODE_OPTIONS").ok().as_deref(), memory_mb) {
        cmd.env.push(("NODE_OPTIONS".into(), v));
    }
}

/// What to tell the reader when the Node heap ran out.
fn heap_hint(memory_mb: u32) -> String {
    let own = std::env::var("NODE_OPTIONS").is_ok_and(|v| v.contains("--max-old-space-size"));
    if own {
        "\nOdin: Node ran out of memory. Your NODE_OPTIONS sets --max-old-space-size, which overrides the `index.node_memory_mb` setting; raise it there.".into()
    } else {
        format!(
            "\nOdin: Node ran out of memory at {memory_mb} MB. Raise the `index.node_memory_mb` setting (cho settings set index.node_memory_mb <MB>) and build again."
        )
    }
}

/// Builds (or rebuilds) the index for one project and records `meta.json`, reporting
/// progress to `on_tick`. A failed run leaves any previous `index.scip` in place and records
/// the failure, so status reads `failed` until the next success. `opts.prepare` records a
/// failed prepare that ran before it: the index then reads `partial` (see
/// [`crate::cache::IndexStatus::Partial`]). A cancelled build removes its partial output and
/// writes nothing else, so the previous index is kept.
pub fn refresh_with(
    cache: &IndexCache,
    canonical_root: &Path,
    project: &Project,
    opts: &Options,
    on_tick: &mut dyn FnMut(&Tick),
) -> Result<Refresh, SemanticError> {
    let js = match crate::tools::Tool::for_language(project.language) {
        None => None,
        Some(tool) => match node_run(cache, project, tool, opts, on_tick) {
            Ok(js) => Some(js),
            Err(crate::tools::NotReady::Cancelled) => return Ok(Refresh::Cancelled),
            Err(crate::tools::NotReady::Failed(msg)) => {
                return refresh_inner(cache, canonical_root, project, opts, on_tick, |_, _| {
                    Err(msg)
                });
            }
        },
    };
    refresh_command(cache, canonical_root, project, opts, on_tick, |out, dir| {
        let mut cmd = command(project, out, dir, js.as_ref());
        apply_node_memory(&mut cmd, project.language, opts.node_memory_mb);
        cmd
    })
}

/// `node` and the indexer's script for a TypeScript or Python build: found, and installed or
/// checked as the settings say, never from the repository.
fn node_run(
    cache: &IndexCache,
    project: &Project,
    tool: crate::tools::Tool,
    opts: &Options,
    on_tick: &mut dyn FnMut(&Tick),
) -> Result<NodeRun, crate::tools::NotReady> {
    use crate::tools;
    let repo = tools::repo_root(project);
    let avoid = [repo.as_path(), project.root.as_path()];
    let machine = tools::Machine::current();
    let node = tools::resolve(tools::Program::Node, &opts.tools, None, &avoid, &machine)
        .map_err(tools::NotReady::Failed)?;
    let ready = tools::ensure(
        tool,
        &opts.tools,
        &node,
        cache.base(),
        &avoid,
        &machine,
        &opts.cancel,
        on_tick,
    )?;
    Ok(NodeRun {
        node,
        script: ready.script,
        venv: (tool == tools::Tool::Python)
            .then(|| tools::project_venv(project))
            .flatten(),
    })
}

/// Start of a build's temporary output name (`index.scip.partial<pid>.<n>`).
const PARTIAL_PREFIX: &str = "index.scip.partial";

/// Removes partial outputs a crashed build left behind. Only files untouched for a day go, so
/// another Chorong's running build is never disturbed.
fn remove_stale_partials(dir: &Path) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for e in rd.flatten() {
        let stale = e.file_name().to_string_lossy().starts_with(PARTIAL_PREFIX)
            && e.metadata()
                .and_then(|m| m.modified())
                .ok()
                .and_then(|t| t.elapsed().ok())
                .is_some_and(|age| age > Duration::from_secs(24 * 60 * 60));
        if stale {
            std::fs::remove_file(e.path()).ok();
        }
    }
}

fn refresh_command(
    cache: &IndexCache,
    canonical_root: &Path,
    project: &Project,
    opts: &Options,
    on_tick: &mut dyn FnMut(&Tick),
    build: impl FnOnce(&Path, &Path) -> IndexerCommand,
) -> Result<Refresh, SemanticError> {
    refresh_inner(cache, canonical_root, project, opts, on_tick, |out, dir| {
        Ok(build(out, dir))
    })
}

/// [`refresh_command`] whose command may not be available: `Err` is recorded as a failed
/// build with that message (an indexer that could not be found or installed).
fn refresh_inner(
    cache: &IndexCache,
    canonical_root: &Path,
    project: &Project,
    opts: &Options,
    on_tick: &mut dyn FnMut(&Tick),
    build: impl FnOnce(&Path, &Path) -> Result<IndexerCommand, String>,
) -> Result<Refresh, SemanticError> {
    let dir = cache.repo_dir(canonical_root).join(project.slug());
    std::fs::create_dir_all(&dir).map_err(|e| SemanticError::Io {
        path: dir.clone(),
        source: e,
    })?;
    // A name of its own per build. Two Chorong processes (or two threads) indexing one project
    // used to share `index.scip.partial`: each deleted the other's output on start and the
    // second `rename` found nothing to move. Leftovers of crashed builds are swept by age.
    static BUILD: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    remove_stale_partials(&dir);
    let partial = dir.join(format!(
        "{PARTIAL_PREFIX}{}.{}",
        std::process::id(),
        BUILD.fetch_add(1, Ordering::Relaxed)
    ));
    let git = GitState::of(canonical_root);
    let (cmd, outcome) = match build(&partial, &dir) {
        Ok(cmd) => {
            let outcome =
                run_with_priority(&cmd, opts.timeout, &opts.cancel, on_tick, opts.low_priority);
            (cmd, outcome)
        }
        Err(msg) => (
            IndexerCommand {
                program: project.language.indexer().into(),
                args: vec![],
                cwd: project.root.clone(),
                env: vec![],
            },
            RunOutcome {
                ended: Ended::NotStarted,
                stderr_tail: msg,
                duration: Duration::ZERO,
            },
        ),
    };
    let mut stderr_tail = outcome.stderr_tail;
    let result = match outcome.ended {
        Ended::Cancelled => {
            std::fs::remove_file(&partial).ok();
            return Ok(Refresh::Cancelled);
        }
        Ended::Exited { code: Some(0) } if partial.is_file() => BuildResult::Built,
        Ended::Exited { code: Some(0) } => BuildResult::Failed(Failure::NoOutput),
        Ended::Exited { code } => BuildResult::Failed(Failure::Exited { code }),
        Ended::NotStarted => BuildResult::Failed(Failure::Exited { code: None }),
        Ended::TimedOut => BuildResult::Failed(Failure::TimedOut),
    };
    if let BuildResult::Failed(failure) = result {
        if failure == Failure::NoOutput {
            stderr_tail.push_str("\nindexer exited 0 but wrote no output file");
        }
        if project.language == Language::TypeScript && stderr_tail.contains("heap out of memory") {
            stderr_tail.push_str(&heap_hint(opts.node_memory_mb));
        }
    }
    let index = dir.join(INDEX_FILE);
    if result == BuildResult::Built {
        std::fs::rename(&partial, &index).map_err(|e| SemanticError::Io {
            path: index.clone(),
            source: e,
        })?;
    } else {
        std::fs::remove_file(&partial).ok();
    }
    let meta = Meta {
        indexer: project.language.indexer().into(),
        indexer_version: if result == BuildResult::Built {
            version_of(project.language, &cmd, &index)
        } else {
            None
        },
        language: project.language,
        project_root: project.rel.clone(),
        git_head: git.head,
        git_status_hash: git.status_hash,
        created: now_unix(),
        result,
        duration_ms: outcome.duration.as_millis() as u64,
        stderr_tail,
        prepare: opts.prepare.clone(),
    };
    IndexCache::write_meta(&dir, &meta)?;
    Ok(Refresh::Done(meta))
}

#[cfg(test)]
#[allow(clippy::disallowed_methods)] // tests read their own fixtures
mod env_tests {
    use cho_core::build_env::allowed as env_allowed;

    /// End to end: every variable a real indexer process sees is allowed or set by Chorong.
    #[cfg(unix)]
    #[test]
    fn a_running_indexer_sees_only_allowed_variables() {
        let out = std::env::temp_dir().join(format!("cho-env-{}", std::process::id()));
        let cmd = super::IndexerCommand {
            program: "sh".into(),
            args: vec![
                "-c".into(),
                "env > \"$0\"".into(),
                out.display().to_string(),
            ],
            cwd: std::env::temp_dir(),
            env: vec![("CHORONG_SET_BY_CHORONG".into(), "1".into())],
        };
        assert!(super::run(&cmd, std::time::Duration::from_secs(10)).success());
        let seen = std::fs::read_to_string(&out).unwrap();
        std::fs::remove_file(&out).ok();
        let names: Vec<&str> = seen
            .lines()
            .filter_map(|l| l.split_once('='))
            .map(|(k, _)| k)
            .collect();
        assert!(names.contains(&"CHORONG_SET_BY_CHORONG"));
        for name in names {
            // `sh` itself sets PWD, SHLVL and `_`.
            assert!(
                env_allowed(name)
                    || ["CHORONG_SET_BY_CHORONG", "PWD", "OLDPWD", "SHLVL", "_"].contains(&name),
                "indexer saw {name}"
            );
        }
    }
}

#[cfg(test)]
#[allow(clippy::disallowed_methods)] // tests read their own temp files
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("cho-ix-{name}-{}", std::process::id()));
        std::fs::remove_dir_all(&d).ok();
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn detects_projects_without_nesting_same_language() {
        let d = tmp("detect");
        for f in [
            "Cargo.toml",
            "crates/a/Cargo.toml",
            "web/package.json",
            "web/node_modules/x/package.json",
            "py/pyproject.toml",
            "target/Cargo.toml",
            ".hidden/Cargo.toml",
        ] {
            let p = d.join(f);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, "").unwrap();
        }
        let found: Vec<_> = detect(&d, None)
            .into_iter()
            .map(|p| (p.language.name(), p.rel))
            .collect();
        assert_eq!(
            found,
            [
                ("rust", "".to_string()),
                ("python", "py".into()),
                ("typescript", "web".into())
            ]
        );
        let only: Vec<_> = detect(&d, Some(Language::Python))
            .into_iter()
            .map(|p| p.rel)
            .collect();
        assert_eq!(only, ["py"]);
        std::fs::remove_dir_all(d).ok();
    }

    #[test]
    fn commands_match_the_spec() {
        let p = |language, rel: &str| Project {
            language,
            root: PathBuf::from("/r").join(rel),
            rel: rel.into(),
        };
        let out = Path::new("/o/index.scip");
        let rust = command(&p(Language::Rust, ""), out, Path::new("/s"), None);
        assert_eq!(rust.program, "rust-analyzer");
        assert_eq!(rust.args, ["scip", "/r/", "--output", "/o/index.scip"]);
        let js = NodeRun {
            node: "/n/bin/node".into(),
            script: "/c/tools/scip-typescript/0.6.0/main.js".into(),
            venv: None,
        };
        let path_of = |c: &IndexerCommand| {
            c.env
                .iter()
                .find(|(k, _)| k == "PATH")
                .map(|(_, v)| v.clone())
                .unwrap()
        };
        let ts = command(
            &p(Language::TypeScript, "web"),
            out,
            Path::new("/s"),
            Some(&js),
        );
        assert_eq!(ts.program, "/n/bin/node");
        assert_eq!(
            ts.args,
            [
                "/c/tools/scip-typescript/0.6.0/main.js",
                "index",
                "--max-file-byte-size",
                "8mb",
                "--output",
                "/o/index.scip"
            ]
        );
        assert_eq!(ts.cwd, Path::new("/r/web"));
        assert!(path_of(&ts).starts_with("/n/bin:"), "{}", path_of(&ts));
        // Without Chorong's node the command names only the indexer, which is not found.
        assert_eq!(
            command(&p(Language::TypeScript, ""), out, Path::new("/s"), None).program,
            "scip-typescript"
        );
        // pnpm workspace: Chorong lists the packages, root first; no `--pnpm-workspaces`.
        let dir = std::env::temp_dir().join(format!("cho-pnpm-ws-{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        for f in [
            "packages/b/package.json",
            "packages/a/package.json",
            "packages/x.txt",
        ] {
            let f = dir.join(f);
            std::fs::create_dir_all(f.parent().unwrap()).unwrap();
            std::fs::write(f, "{}").unwrap();
        }
        std::fs::write(
            dir.join("pnpm-workspace.yaml"),
            "packages:\n  - \"packages/*\"\n",
        )
        .unwrap();
        let ws = command(
            &Project {
                language: Language::TypeScript,
                root: dir.clone(),
                rel: String::new(),
            },
            out,
            Path::new("/s"),
            Some(&js),
        );
        let d = |rel: &str| dir.join(rel).display().to_string();
        assert_eq!(
            ws.args[6..],
            [dir.display().to_string(), d("packages/a"), d("packages/b")],
            "{:?}",
            ws.args
        );
        assert!(!ws.args.iter().any(|a| a.contains("workspaces")));
        std::fs::remove_dir_all(&dir).ok();
        let py_js = NodeRun {
            script: "/c/tools/scip-python/0.6.8/index.js".into(),
            ..js.clone()
        };
        let py = command(
            &p(Language::Python, "py"),
            out,
            Path::new("/s"),
            Some(&py_js),
        );
        assert_eq!(
            py.args,
            [
                "/c/tools/scip-python/0.6.8/index.js",
                "index",
                "--project-name",
                "py",
                "--output",
                "/o/index.scip"
            ]
        );
        assert!(py.env.contains(&("PYTHONSAFEPATH".into(), "1".into())));
        assert!(path_of(&py).starts_with("/n/bin:"));
        assert!(!py.env.iter().any(|(k, _)| k == "VIRTUAL_ENV"));
        let venv = NodeRun {
            venv: Some("/r/.venv".into()),
            ..py_js
        };
        let py = command(
            &p(Language::Python, "py"),
            out,
            Path::new("/s"),
            Some(&venv),
        );
        assert!(
            path_of(&py).starts_with("/r/.venv/bin:/n/bin:"),
            "{}",
            path_of(&py)
        );
        assert!(py.env.contains(&("VIRTUAL_ENV".into(), "/r/.venv".into())));
        assert_eq!(p(Language::Python, "a/b c").slug(), "python-a_b_c");
    }

    /// End to end with a fake `node` and `npm`: a scip-typescript planted in the repository's
    /// `node_modules` (and its `.bin`) and the repository's `.npmrc` play no part. Chorong's copy
    /// is installed in its cache by an npm that runs there, and node runs that copy with the
    /// repository as its working directory.
    #[cfg(unix)]
    #[test]
    fn a_planted_indexer_and_npmrc_in_the_repository_are_not_used() {
        use crate::tools::tests::{exe, fake_npm};
        let d = tmp("planted");
        let repo = d.join("repo");
        let planted = repo.join("node_modules/@sourcegraph/scip-typescript/dist/src/main.js");
        exe(&planted, "console.log('planted')\n");
        exe(
            &repo.join("node_modules/.bin/scip-typescript"),
            "#!/bin/sh\necho planted\n",
        );
        std::fs::write(repo.join(".npmrc"), "registry=http://planted.invalid/\n").unwrap();
        std::fs::write(repo.join("package.json"), "{}").unwrap();
        std::fs::write(repo.join("tsconfig.json"), "{}").unwrap();
        let node_log = d.join("node.log");
        let node = d.join("bin/node");
        exe(
            &node,
            &format!(
                "#!/bin/sh\necho \"cwd=$PWD\" >> '{log}'\nfor a in \"$@\"; do echo \"arg=$a\" >> '{log}'; done\n\
                 while [ $# -gt 0 ]; do [ \"$1\" = --output ] && echo idx > \"$2\"; shift; done\n",
                log = node_log.display()
            ),
        );
        let npm = fake_npm(&d, "[]");
        let cache_dir = d.join("cache");
        let cache = IndexCache::at(cache_dir.clone());
        let project = Project {
            language: Language::TypeScript,
            root: repo.clone(),
            rel: String::new(),
        };
        let mut opts = Options::new(Duration::from_secs(30));
        opts.tools.node = Some(node);
        opts.tools.npm = Some(npm);
        let Refresh::Done(meta) =
            refresh_with(&cache, &repo, &project, &opts, &mut |_| {}).unwrap()
        else {
            panic!("cancelled")
        };
        assert!(meta.success(), "{}", meta.stderr_tail);
        let ran = std::fs::read_to_string(&node_log).unwrap();
        let script = cache_dir.join(
            "tools/scip-typescript/0.6.0/node_modules/@sourcegraph/scip-typescript/dist/src/main.js",
        );
        // The shell reports its folder as a real path (macOS temp folders: `/var` is
        // `/private/var`), so compare with the repository's real path.
        let real_repo = repo.canonicalize().unwrap();
        assert!(
            ran.contains(&format!("cwd={}\n", real_repo.display())),
            "{ran}"
        );
        assert!(
            ran.contains(&format!("arg={}\n", script.display())),
            "{ran}"
        );
        assert!(!ran.contains(&planted.display().to_string()), "{ran}");
        let npm_ran = std::fs::read_to_string(d.join("npm.log")).unwrap();
        assert!(!npm_ran.contains(&repo.display().to_string()), "{npm_ran}");
        assert!(
            npm_ran.contains(&format!(
                "cwd={}",
                cache_dir
                    .canonicalize()
                    .unwrap()
                    .join("tools/scip-typescript/.install-")
                    .display()
            )),
            "{npm_ran}"
        );
        std::fs::remove_dir_all(d).ok();
    }

    #[test]
    fn build_plans_say_when_project_code_runs() {
        assert!(BuildPlan::of(Language::Rust, false).runs_project_code);
        assert!(!BuildPlan::of(Language::TypeScript, false).runs_project_code);
        assert!(!BuildPlan::of(Language::TypeScript, true).runs_project_code);
        assert!(!BuildPlan::of(Language::Python, false).runs_project_code);
        let py = BuildPlan::of(Language::Python, true);
        assert!(py.runs_project_code && py.why.contains(".pth"));
        let d = tmp("plan");
        let project = Project {
            language: Language::Python,
            root: d.clone(),
            rel: String::new(),
        };
        assert!(!build_plan(&project).runs_project_code);
        std::fs::create_dir_all(d.join(".venv")).unwrap();
        std::fs::write(d.join(".venv/pyvenv.cfg"), "").unwrap();
        assert!(build_plan(&project).runs_project_code);
        std::fs::remove_dir_all(d).ok();
    }

    #[test]
    #[cfg(unix)]
    fn run_reports_failure_timeout_and_stderr_tail() {
        let ok = run(&sh("exit 0"), Duration::from_secs(10));
        assert!(ok.success());
        let bad = run(&sh("echo boom >&2; exit 3"), Duration::from_secs(10));
        assert_eq!(bad.ended, Ended::Exited { code: Some(3) });
        assert!(!bad.success() && bad.stderr_tail.contains("boom"));
        let slow = run(&sh("sleep 5"), Duration::from_millis(100));
        assert!(slow.ended == Ended::TimedOut && slow.duration < Duration::from_secs(4));
        let big = run(
            &sh(
                "i=0; while [ $i -lt 3000 ]; do echo line-$i-xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx >&2; i=$((i+1)); done",
            ),
            Duration::from_secs(10),
        );
        assert!(
            big.stderr_tail.len() <= STDERR_TAIL_BYTES + 4 && big.stderr_tail.contains("line-2999")
        );
        let missing = run(
            &IndexerCommand {
                program: "cho-no-such-program".into(),
                ..sh("")
            },
            Duration::from_secs(1),
        );
        assert_eq!(missing.ended, Ended::NotStarted);
        assert!(missing.stderr_tail.contains("cannot run"));
    }

    #[test]
    fn node_heap_is_raised_unless_the_user_chose_one() {
        assert_eq!(
            node_options(None, 4096).as_deref(),
            Some("--max-old-space-size=4096")
        );
        assert_eq!(
            node_options(Some("--no-warnings"), 8192).as_deref(),
            Some("--no-warnings --max-old-space-size=8192")
        );
        assert_eq!(node_options(Some("--max-old-space-size=1024"), 8192), None);
        let mut cmd = IndexerCommand {
            program: "npx".into(),
            args: vec![],
            cwd: PathBuf::from("/r"),
            env: vec![("NODE_OPTIONS".into(), "stale".into())],
        };
        apply_node_memory(&mut cmd, Language::Python, 1);
        assert_eq!(cmd.env.len(), 1, "only TypeScript is touched");
        assert!(heap_hint(2048).contains("index.node_memory_mb"));
    }

    #[cfg(unix)]
    fn sh(script: &str) -> IndexerCommand {
        IndexerCommand {
            program: "sh".into(),
            args: vec!["-c".into(), script.into()],
            cwd: std::env::temp_dir(),
            env: vec![],
        }
    }

    /// The indexer and what it starts run at nice 10, below the viewer.
    #[cfg(unix)]
    #[test]
    fn indexing_runs_below_the_viewer() {
        if !Path::new("/usr/bin/nice").is_file() {
            return;
        }
        let out = run(
            &sh("n=$(ps -o nice= -p $$ | tr -d ' '); echo \"nice=$n\" >&2; [ \"$n\" = 10 ]"),
            Duration::from_secs(10),
        );
        assert!(out.success(), "{}", out.stderr_tail);
        // `low_priority: false` (Options) skips nice: the child keeps the test's own niceness.
        let own = std::process::Command::new("ps")
            .args(["-o", "nice=", "-p", &std::process::id().to_string()])
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .unwrap_or_default();
        let normal = run_with_priority(
            &sh(&format!(
                "n=$(ps -o nice= -p $$ | tr -d ' '); echo \"nice=$n\" >&2; [ \"$n\" = '{own}' ]"
            )),
            Duration::from_secs(10),
            &CancelToken::new(),
            &mut |_| {},
            false,
        );
        assert!(normal.success(), "own={own} {}", normal.stderr_tail);
        assert!(Options::new(Duration::from_secs(1)).low_priority);
        // A missing program is still reported as one.
        assert_eq!(
            find_program(&IndexerCommand {
                program: "cho-no-such-program".into(),
                ..sh("")
            }),
            None
        );
    }

    /// Cancelling stops the process and its children, and reports it.
    #[cfg(unix)]
    #[test]
    fn cancel_kills_the_process_group() {
        let d = tmp("cancel-group");
        let pidfile = d.join("pid");
        let cmd = sh(&format!("sleep 60 & echo $! > {}; wait", pidfile.display()));
        let cancel = CancelToken::new();
        let c = cancel.clone();
        let stopper = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(300));
            c.cancel();
        });
        let out = run_with(&cmd, Duration::from_secs(60), &cancel, &mut |_| {});
        stopper.join().unwrap();
        assert_eq!(out.ended, Ended::Cancelled);
        assert!(out.duration < Duration::from_secs(10));
        let pid = std::fs::read_to_string(&pidfile).unwrap();
        std::thread::sleep(Duration::from_millis(200));
        // A killed process nobody has reaped yet is a zombie ("Z"): dead for our purposes.
        let alive =
            std::fs::read_to_string(format!("/proc/{}/stat", pid.trim())).is_ok_and(|stat| {
                !stat
                    .rsplit(')')
                    .next()
                    .unwrap_or("")
                    .trim_start()
                    .starts_with('Z')
            });
        assert!(!alive, "the child's child is still running");
        std::fs::remove_dir_all(d).ok();
    }

    #[cfg(unix)]
    #[test]
    fn ticks_carry_elapsed_time_and_the_parsed_step() {
        let cmd = sh(
            "echo 'indexing packages/a/tsconfig.json'; sleep 0.6; echo 'indexing packages/b/tsconfig.json' >&2; sleep 0.6",
        );
        let mut ticks: Vec<Tick> = vec![];
        let out = run_with(
            &cmd,
            Duration::from_secs(30),
            &CancelToken::new(),
            &mut |t| ticks.push(t.clone()),
        );
        assert!(out.success());
        assert!(ticks.len() >= 3 && ticks.len() < 20, "{}", ticks.len());
        assert!(ticks.windows(2).all(|w| w[0].elapsed <= w[1].elapsed));
        let last = &ticks.last().unwrap().progress;
        assert_eq!(last.step.as_deref(), Some("packages/b"));
        assert_eq!(last.n, 2);
    }

    /// The npm indexers' version comes from the index metadata, without decoding documents.
    #[test]
    fn tool_version_is_read_from_the_metadata() {
        use protobuf::Message;
        let dir = tmp("tool-version");
        let mut idx = scip::types::Index::new();
        let mut doc = scip::types::Document::new();
        doc.relative_path = "a.ts".into();
        idx.documents.push(doc);
        let path = dir.join("index.scip");
        std::fs::write(&path, idx.write_to_bytes().unwrap()).unwrap();
        assert_eq!(index_tool_version(&path), None, "no metadata");
        let mut tool = scip::types::ToolInfo::new();
        tool.name = "scip-typescript".into();
        tool.version = "0.6.0".into();
        idx.metadata.mut_or_insert_default().tool_info = Some(tool).into();
        std::fs::write(&path, idx.write_to_bytes().unwrap()).unwrap();
        assert_eq!(index_tool_version(&path).as_deref(), Some("0.6.0"));
        std::fs::write(&path, b"\xff\xff not an index").unwrap();
        assert_eq!(index_tool_version(&path), None);
        std::fs::remove_dir_all(dir).ok();
    }

    /// Partial outputs and temporary records a build left in `dir`.
    fn leftovers(dir: &Path) -> Vec<String> {
        std::fs::read_dir(dir)
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.contains("partial") || n.ends_with(".tmp"))
            .collect()
    }

    /// A cancelled rebuild leaves the earlier index and its record untouched.
    #[cfg(unix)]
    #[test]
    fn cancelled_build_keeps_the_previous_index() {
        let root = tmp("cancel-root");
        let cache_dir = tmp("cancel-cache");
        let cache = IndexCache::at(cache_dir.clone());
        let project = Project {
            language: Language::Rust,
            root: root.clone(),
            rel: String::new(),
        };
        let dir = cache.repo_dir(&root).join(project.slug());
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(INDEX_FILE), b"old index").unwrap();
        std::fs::write(dir.join("meta.json"), b"old meta").unwrap();
        let opts = Options::new(Duration::from_secs(60));
        let c = opts.cancel.clone();
        let stopper = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(300));
            c.cancel();
        });
        let result = refresh_command(&cache, &root, &project, &opts, &mut |_| {}, |out, _| {
            sh(&format!("echo half > {}; sleep 60", out.display()))
        })
        .unwrap();
        stopper.join().unwrap();
        assert!(matches!(result, Refresh::Cancelled));
        assert_eq!(std::fs::read(dir.join(INDEX_FILE)).unwrap(), b"old index");
        assert_eq!(std::fs::read(dir.join("meta.json")).unwrap(), b"old meta");
        assert!(leftovers(&dir).is_empty(), "{:?}", leftovers(&dir));
        std::fs::remove_dir_all(root).ok();
        std::fs::remove_dir_all(cache_dir).ok();
    }

    #[cfg(unix)]
    #[test]
    fn out_of_memory_failure_names_the_setting() {
        let root = tmp("oom-root");
        let cache_dir = tmp("oom-cache");
        let cache = IndexCache::at(cache_dir.clone());
        let project = Project {
            language: Language::TypeScript,
            root: root.clone(),
            rel: String::new(),
        };
        let r = refresh_command(
            &cache,
            &root,
            &project,
            &Options::new(Duration::from_secs(30)),
            &mut |_| {},
            // As Node prints it: the reason, then a native stack far longer than the tail.
            |_, _| {
                sh("echo 'FATAL ERROR: Reached heap limit Allocation failed - JavaScript heap out of memory' >&2; \
                    i=0; while [ $i -lt 150 ]; do \
                      echo \"   $i: 0x10043d65c node::builtins::BuiltinLoader::CompileAndCall(v8::Local<v8::Context>, char const*, node::Realm*) [/opt/node/bin/node]\" >&2; \
                      i=$((i+1)); done; exit 134")
            },
        )
        .unwrap();
        let Refresh::Done(meta) = r else { panic!() };
        assert!(!meta.success() && meta.stderr_tail.contains("index.node_memory_mb"));
        std::fs::remove_dir_all(root).ok();
        std::fs::remove_dir_all(cache_dir).ok();
    }

    #[test]
    fn a_crash_report_keeps_its_reason_and_drops_native_frames() {
        let mut t = Tail::default();
        t.push("Projects: 12");
        t.push("<--- Last few GCs --->");
        t.push("FATAL ERROR: Reached heap limit Allocation failed - JavaScript heap out of memory");
        for i in 0..150 {
            t.push(&format!(
                "  {i}: 0x10043d65c node::StartExecution(node::Environment*, char const*) [/opt/node/bin/node]"
            ));
        }
        for i in 0..400 {
            t.push(&format!(
                "noise line {i} that is long enough to push older lines out"
            ));
        }
        let out = t.finish();
        assert!(out.starts_with("FATAL ERROR: Reached heap limit"), "{out}");
        assert!(!out.contains(": 0x10043d65c"), "native frames are dropped");
        assert!(out.ends_with("noise line 399 that is long enough to push older lines out"));
        assert!(out.len() <= STDERR_TAIL_BYTES + 200);
        // Short displays lead with the reason.
        let lines = report_lines(&out, 4);
        assert_eq!(lines.len(), 4);
        assert!(lines[0].starts_with("FATAL ERROR") && lines[1] == "…");
        assert_eq!(
            failure_reason(&out),
            Some(
                "FATAL ERROR: Reached heap limit Allocation failed - JavaScript heap out of memory"
            )
        );
        // A short report keeps everything once, in order.
        let mut t = Tail::default();
        t.push("Error: Cannot find module 'typescript'");
        t.push("    at Module._resolveFilename (node:internal/modules/cjs/loader:1225:15)");
        assert_eq!(
            t.finish(),
            "Error: Cannot find module 'typescript'\n    at Module._resolveFilename (node:internal/modules/cjs/loader:1225:15)"
        );
    }

    /// Cancelling while the indexer floods both pipes returns promptly (the readers keep
    /// draining, so the child is never blocked on a full pipe) and keeps only a bounded tail.
    #[cfg(unix)]
    #[test]
    fn cancel_while_the_child_floods_output() {
        let cmd = sh(
            "(yes 'out-out-out-out-out-out-out-out' &) ; exec yes 'err-err-err-err-err-err-err-err' >&2",
        );
        let cancel = CancelToken::new();
        let c = cancel.clone();
        let stopper = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(300));
            c.cancel();
        });
        let out = run_with(&cmd, Duration::from_secs(60), &cancel, &mut |_| {});
        stopper.join().unwrap();
        assert_eq!(out.ended, Ended::Cancelled);
        assert!(out.duration < Duration::from_secs(5), "{:?}", out.duration);
        assert!(
            out.stderr_tail.len() <= STDERR_TAIL_BYTES + 32,
            "{}",
            out.stderr_tail.len()
        );
        assert!(out.stderr_tail.contains("err-err"));
    }

    /// A process that ignores SIGTERM (and starts a child that does too) is still stopped:
    /// `stop_group` escalates to SIGKILL after a second.
    #[cfg(unix)]
    #[test]
    fn cancel_stops_a_process_that_ignores_sigterm() {
        let d = tmp("ignore-term");
        let pids = d.join("pids");
        let cmd = sh(&format!(
            "trap '' TERM; (trap '' TERM; while :; do sleep 0.05; done) & echo $! > {p}; \
             echo ready >&2; while :; do sleep 0.05; done",
            p = pids.display()
        ));
        let cancel = CancelToken::new();
        // Cancel only once the traps are installed (400ms is ample for `sh` to get there).
        let out = run_with(&cmd, Duration::from_secs(60), &cancel, &mut |t| {
            if t.elapsed > Duration::from_millis(400) {
                cancel.cancel();
            }
        });
        assert_eq!(out.ended, Ended::Cancelled);
        assert!(out.duration < Duration::from_secs(6), "{:?}", out.duration);
        let pid = std::fs::read_to_string(&pids).unwrap();
        std::thread::sleep(Duration::from_millis(200));
        let alive = std::fs::read_to_string(format!("/proc/{}/stat", pid.trim())).is_ok_and(|s| {
            !s.rsplit(')')
                .next()
                .unwrap_or("")
                .trim_start()
                .starts_with('Z')
        });
        assert!(!alive, "the SIGTERM-ignoring grandchild survived");
        std::fs::remove_dir_all(d).ok();
    }

    /// A process that has already exited when the timeout or the cancel is noticed is a
    /// success: the exit status is read first. (`on_tick` runs before the check, so the
    /// callback holds the loop long enough for the child to exit.)
    #[cfg(unix)]
    #[test]
    fn an_exit_wins_over_a_timeout_or_cancel_that_fires_at_the_same_time() {
        for (timeout, cancel_it) in [
            (Duration::from_millis(1), false),
            (Duration::from_secs(60), true),
        ] {
            let cancel = CancelToken::new();
            let mut first = true;
            let out = run_with(&sh("exit 0"), timeout, &cancel, &mut |_| {
                if std::mem::take(&mut first) {
                    std::thread::sleep(Duration::from_millis(400));
                    if cancel_it {
                        cancel.cancel();
                    }
                }
            });
            assert_eq!(out.ended, Ended::Exited { code: Some(0) }, "{out:?}");
        }
    }

    /// Two Chorong processes (here, two threads: the paths are the same) indexing one
    /// project at once must not trample each other's partial output.
    #[cfg(unix)]
    #[test]
    fn two_builds_of_one_project_do_not_clobber_each_other() {
        let root = tmp("twice-root");
        let cache_dir = tmp("twice-cache");
        let cache = IndexCache::at(cache_dir.clone());
        let project = Project {
            language: Language::Rust,
            root: root.clone(),
            rel: String::new(),
        };
        let build = |tag: &'static str, delay_ms: u64| {
            let (cache, root, project) = (cache.clone(), root.clone(), project.clone());
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(delay_ms));
                refresh_command(
                    &cache,
                    &root,
                    &project,
                    &Options::new(Duration::from_secs(30)),
                    &mut |_| {},
                    |out, _| sh(&format!("echo {tag} > '{}'; sleep 0.8", out.display())),
                )
            })
        };
        let (a, b) = (build("first", 0), build("second", 300));
        for r in [a.join().unwrap(), b.join().unwrap()] {
            let Refresh::Done(meta) = r.expect("a concurrent build must not fail") else {
                panic!("not cancelled")
            };
            assert!(meta.success(), "{}", meta.stderr_tail);
        }
        let dir = cache.repo_dir(&root).join(project.slug());
        let index = std::fs::read_to_string(dir.join(INDEX_FILE)).unwrap();
        assert!(index == "first\n" || index == "second\n", "{index:?}");
        assert!(leftovers(&dir).is_empty(), "{:?}", leftovers(&dir));
        std::fs::remove_dir_all(root).ok();
        std::fs::remove_dir_all(cache_dir).ok();
    }

    /// The worktree vanishing while the indexer runs (a deleted checkout) ends in a recorded
    /// result, not a panic, and the cache then reads the repository as stale.
    #[cfg(unix)]
    #[test]
    fn deleting_the_worktree_during_a_build_is_recorded() {
        let root = tmp("vanish-root");
        let cache_dir = tmp("vanish-cache");
        let cache = IndexCache::at(cache_dir.clone());
        let project = Project {
            language: Language::Rust,
            root: root.clone(),
            rel: String::new(),
        };
        let r = refresh_command(
            &cache,
            &root,
            &project,
            &Options::new(Duration::from_secs(30)),
            &mut |_| {},
            |out, _| {
                let mut c = sh(&format!(
                    "rm -rf '{}'; echo idx > '{}'",
                    root.display(),
                    out.display()
                ));
                c.cwd = root.clone();
                c
            },
        )
        .unwrap();
        let Refresh::Done(meta) = r else { panic!() };
        assert!(meta.success(), "{}", meta.stderr_tail);
        assert!(!root.exists());
        let entries = cache.projects(&root);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].status, crate::cache::IndexStatus::Stale);
        std::fs::remove_dir_all(cache_dir).ok();
    }
}
