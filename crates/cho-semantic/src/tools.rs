//! Chorong's own copies of the Node indexers, and where `node`, `npm` and `uv` are.
//!
//! scip-typescript and scip-python run as `node <absolute script>` from a copy Chorong installs
//! once into `tools/` in its index cache (install scripts off, in a folder of Chorong's own), so
//! nothing in a repository (its `node_modules`, its `.npmrc`, its package manager) chooses the
//! program. The version is a setting: exact, a caret or tilde range, or `latest`; a range or
//! `latest` is checked against the registry every few days (a setting). A reader may point a
//! setting at their own install instead, and Chorong then downloads nothing.
//!
//! `node`, `npm` and `uv` come from settings. Empty, they are looked for once: the login
//! shell's `PATH`, then well-known places (Homebrew, `/usr/local/bin`, `~/.local/bin`,
//! `~/.cargo/bin`, nvm, volta, fnm), never the repository, and the result is saved to the
//! setting. A version manager's shim is skipped, since it picks the version from the
//! repository's files.

use crate::indexer::{
    CancelToken, Ended, IndexerCommand, Language, Project, Tick, run_with, run_with_output,
};
use cho_core::npm_version::{Spec, Version};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// The setting keys this module reads (defined in `cho-settings`).
pub mod keys {
    pub const NODE: &str = "tools.node";
    pub const NPM: &str = "tools.npm";
    pub const UV: &str = "tools.uv";
    pub const TYPESCRIPT_VERSION: &str = "index.typescript_version";
    pub const PYTHON_VERSION: &str = "index.python_version";
    pub const TYPESCRIPT_INSTALL: &str = "index.typescript_install";
    pub const PYTHON_INSTALL: &str = "index.python_install";
    pub const VERSION_CHECK: &str = "index.version_check";
    pub const VERSION_CHECK_DAYS: &str = "index.version_check_days";
}

/// How long the login shell may take to print its environment.
pub const SHELL_TIMEOUT: Duration = Duration::from_secs(5);
/// How long `npm install` of an indexer may take.
const INSTALL_TIMEOUT: Duration = Duration::from_secs(600);
/// How long a registry check (`npm view`) may take.
const CHECK_TIMEOUT: Duration = Duration::from_secs(60);
/// Default days between registry checks of a range or `latest`.
pub const DEFAULT_CHECK_DAYS: u32 = 7;

/// A Node indexer Chorong installs and runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Tool {
    TypeScript,
    Python,
}

impl Tool {
    pub fn for_language(language: Language) -> Option<Tool> {
        match language {
            Language::TypeScript => Some(Tool::TypeScript),
            Language::Python => Some(Tool::Python),
            Language::Rust => None,
        }
    }

    /// The program's name, as people know it.
    pub fn name(self) -> &'static str {
        match self {
            Tool::TypeScript => "scip-typescript",
            Tool::Python => "scip-python",
        }
    }

    /// The npm package.
    pub fn package(self) -> &'static str {
        match self {
            Tool::TypeScript => "@sourcegraph/scip-typescript",
            Tool::Python => "@sourcegraph/scip-python",
        }
    }

    /// The version built in: the version setting's default.
    pub fn pinned(self) -> &'static str {
        match self {
            Tool::TypeScript => "0.6.0",
            Tool::Python => "0.6.8",
        }
    }

    /// The package's program, relative to the package folder (its `bin`).
    pub fn script(self) -> &'static str {
        match self {
            Tool::TypeScript => "dist/src/main.js",
            Tool::Python => "index.js",
        }
    }

    pub fn version_key(self) -> &'static str {
        match self {
            Tool::TypeScript => keys::TYPESCRIPT_VERSION,
            Tool::Python => keys::PYTHON_VERSION,
        }
    }

    pub fn install_key(self) -> &'static str {
        match self {
            Tool::TypeScript => keys::TYPESCRIPT_INSTALL,
            Tool::Python => keys::PYTHON_INSTALL,
        }
    }
}

/// A program Chorong runs from the reader's machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Program {
    Node,
    Npm,
    Uv,
}

impl Program {
    pub fn name(self) -> &'static str {
        match self {
            Program::Node => "node",
            Program::Npm => "npm",
            Program::Uv => "uv",
        }
    }

    /// Its setting.
    pub fn key(self) -> &'static str {
        match self {
            Program::Node => keys::NODE,
            Program::Npm => keys::NPM,
            Program::Uv => keys::UV,
        }
    }
}

/// Saves a program Chorong found to its setting (`key`, absolute path). A plain function so
/// [`ToolConfig`] stays `Clone` and `Debug`; `cho_sources::tools::save_found` writes
/// `settings.json`.
pub type SaveFound = fn(&'static str, &Path);

/// Which indexer to run for one tool: a version spec, or the reader's own install.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Choice {
    /// The version setting's text (checked when the build starts).
    pub version: String,
    /// The reader's own install (package folder or script); Chorong then downloads nothing.
    pub own: Option<PathBuf>,
}

/// The tool settings a build runs with.
#[derive(Debug, Clone)]
pub struct ToolConfig {
    pub node: Option<PathBuf>,
    pub npm: Option<PathBuf>,
    pub uv: Option<PathBuf>,
    pub typescript: Choice,
    pub python: Choice,
    /// How often a range or `latest` is checked against the registry; `None` never re-checks
    /// (the installed match is used; the registry is asked only when nothing installed matches).
    pub check_every: Option<Duration>,
    /// Called with each program Chorong found because its setting was empty.
    pub save: Option<SaveFound>,
}

impl Default for ToolConfig {
    fn default() -> Self {
        Self {
            node: None,
            npm: None,
            uv: None,
            typescript: Choice {
                version: Tool::TypeScript.pinned().into(),
                own: None,
            },
            python: Choice {
                version: Tool::Python.pinned().into(),
                own: None,
            },
            check_every: Some(days(DEFAULT_CHECK_DAYS)),
            save: None,
        }
    }
}

fn days(n: u32) -> Duration {
    Duration::from_secs(u64::from(n) * 24 * 60 * 60)
}

impl ToolConfig {
    /// From settings, read through `get` (a key's value as text, `None` when unknown), so
    /// this crate needs no settings dependency.
    pub fn from_lookup(get: impl Fn(&str) -> Option<String>) -> Self {
        let d = Self::default();
        let path = |k: &str| {
            get(k)
                .map(|v| v.trim().to_string())
                .filter(|v| !v.is_empty())
                .map(PathBuf::from)
        };
        let version = |k: &str, pinned: &str| {
            get(k)
                .map(|v| v.trim().to_string())
                .filter(|v| !v.is_empty())
                .unwrap_or_else(|| pinned.to_string())
        };
        let check_on = get(keys::VERSION_CHECK).is_none_or(|v| v.trim() != "false");
        let n = get(keys::VERSION_CHECK_DAYS)
            .and_then(|v| v.trim().parse::<u32>().ok())
            .filter(|n| *n > 0)
            .unwrap_or(DEFAULT_CHECK_DAYS);
        Self {
            node: path(keys::NODE),
            npm: path(keys::NPM),
            uv: path(keys::UV),
            typescript: Choice {
                version: version(keys::TYPESCRIPT_VERSION, Tool::TypeScript.pinned()),
                own: path(keys::TYPESCRIPT_INSTALL),
            },
            python: Choice {
                version: version(keys::PYTHON_VERSION, Tool::Python.pinned()),
                own: path(keys::PYTHON_INSTALL),
            },
            check_every: check_on.then(|| days(n)),
            save: d.save,
        }
    }

    pub fn choice(&self, tool: Tool) -> &Choice {
        match tool {
            Tool::TypeScript => &self.typescript,
            Tool::Python => &self.python,
        }
    }

    pub fn setting(&self, program: Program) -> Option<&Path> {
        match program {
            Program::Node => self.node.as_deref(),
            Program::Npm => self.npm.as_deref(),
            Program::Uv => self.uv.as_deref(),
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Finding node, npm and uv

/// Where a search for a program looked and what it found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Search {
    pub program: Program,
    pub found: Option<PathBuf>,
    /// Every place looked, in order, for a person.
    pub searched: Vec<String>,
}

impl Search {
    /// The error for a program that was not found: every place searched, and the setting.
    pub fn not_found(&self) -> String {
        let mut out = format!("{} not found. Chorong looked in:\n", self.program.name());
        for s in &self.searched {
            out.push_str(&format!("  - {s}\n"));
        }
        out.push_str(&format!(
            "Install {0}, or set its path: cho settings set {1} /path/to/{0}",
            self.program.name(),
            self.program.key()
        ));
        out
    }
}

/// What the search may read from the machine; tests replace it.
#[derive(Debug, Clone)]
pub struct Machine {
    /// The reader's login shell (`$SHELL`), started as `SHELL -l -c env` to read its `PATH`.
    pub shell: Option<PathBuf>,
    pub home: Option<PathBuf>,
    /// `NVM_DIR`, `FNM_DIR`, `VOLTA_HOME` when set.
    pub nvm_dir: Option<PathBuf>,
    pub fnm_dir: Option<PathBuf>,
    pub volta_home: Option<PathBuf>,
    pub shell_timeout: Duration,
}

impl Machine {
    /// This machine, from the process environment.
    pub fn current() -> Self {
        let var = |k: &str| {
            std::env::var_os(k)
                .filter(|v| !v.is_empty())
                .map(PathBuf::from)
        };
        Self {
            shell: var("SHELL"),
            home: var("HOME"),
            nvm_dir: var("NVM_DIR"),
            fnm_dir: var("FNM_DIR"),
            volta_home: var("VOLTA_HOME"),
            shell_timeout: SHELL_TIMEOUT,
        }
    }
}

/// The login shell's `PATH`, read once per process.
static SHELL_PATH: Mutex<Option<(PathBuf, Option<String>)>> = Mutex::new(None);
/// Programs found in this process, so a second build does not search again before the
/// setting it saved is read.
static FOUND: Mutex<Vec<(Program, PathBuf)>> = Mutex::new(Vec::new());

/// `SHELL -l -c env`, in the home folder, for at most `timeout`: the `PATH` line it prints.
fn login_shell_path(shell: &Path, home: Option<&Path>, timeout: Duration) -> Option<String> {
    if let Ok(cached) = SHELL_PATH.lock()
        && let Some((s, p)) = cached.as_ref()
        && s == shell
    {
        return p.clone();
    }
    let mut cmd = std::process::Command::new(shell);
    cmd.args(["-l", "-c", "env"])
        .current_dir(home.unwrap_or(Path::new("/")))
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    let path = (|| {
        let mut child = cmd.spawn().ok()?;
        let mut stdout = child.stdout.take()?;
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            use std::io::Read;
            let mut buf = vec![];
            // A profile that prints without end is cut off at 1 MB.
            stdout.by_ref().take(1 << 20).read_to_end(&mut buf).ok();
            tx.send(buf).ok();
        });
        let start = Instant::now();
        loop {
            match child.try_wait() {
                Ok(Some(_)) => break,
                Ok(None) if start.elapsed() >= timeout => {
                    crate::indexer::kill_group(&child);
                    child.kill().ok();
                    child.wait().ok();
                    return None;
                }
                Ok(None) => std::thread::sleep(Duration::from_millis(10)),
                Err(_) => return None,
            }
        }
        let out = rx.recv_timeout(Duration::from_millis(500)).ok()?;
        String::from_utf8_lossy(&out)
            .lines()
            .rev()
            .find_map(|l| l.strip_prefix("PATH=").map(str::to_string))
    })();
    if let Ok(mut c) = SHELL_PATH.lock() {
        *c = Some((shell.to_path_buf(), path.clone()));
    }
    path
}

/// The subfolders of `dir` named like versions (`v20.11.1`, `20.11.1`), highest first.
fn version_dirs(dir: &Path) -> Vec<PathBuf> {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return vec![];
    };
    let mut found: Vec<(Version, PathBuf)> = rd
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            Some((Version::parse(&name)?, e.path()))
        })
        .collect();
    found.sort_by(|a, b| b.0.cmp(&a.0));
    found.into_iter().map(|(_, p)| p).collect()
}

/// A place to look: one folder, or the `bin` folders of every version a manager installed
/// (highest first).
enum Place {
    Dir(PathBuf),
    Versions {
        root: PathBuf,
        /// Below each version folder, e.g. `bin` or `installation/bin`.
        bin: &'static str,
    },
}

impl Place {
    fn describe(&self, home: Option<&Path>) -> String {
        let show = |p: &Path| match home.and_then(|h| p.strip_prefix(h).ok()) {
            Some(rest) => format!("~/{}", rest.display()),
            None => p.display().to_string(),
        };
        match self {
            Place::Dir(d) => show(d),
            Place::Versions { root, bin } => format!("{}/*/{bin}", show(root)),
        }
    }

    fn dirs(&self) -> Vec<PathBuf> {
        match self {
            Place::Dir(d) => vec![d.clone()],
            Place::Versions { root, bin } => version_dirs(root)
                .into_iter()
                .map(|v| v.join(bin))
                .collect(),
        }
    }
}

/// The well-known places for `program`, in order.
fn known_places(program: Program, m: &Machine) -> Vec<Place> {
    let home = m.home.as_deref();
    let h = |rel: &str| home.map(|h| h.join(rel));
    let mut out: Vec<Place> = vec![];
    let dir = |p: PathBuf| Place::Dir(p);
    match program {
        Program::Node | Program::Npm => {
            out.push(dir("/opt/homebrew/bin".into()));
            out.push(dir("/usr/local/bin".into()));
            if let Some(nvm) = m.nvm_dir.clone().or_else(|| h(".nvm")) {
                out.push(Place::Versions {
                    root: nvm.join("versions/node"),
                    bin: "bin",
                });
            }
            // Volta's own `~/.volta/bin/node` is a shim that reads the repository's
            // `package.json`; its installed images are plain Node.
            if let Some(volta) = m.volta_home.clone().or_else(|| h(".volta")) {
                out.push(Place::Versions {
                    root: volta.join("tools/image/node"),
                    bin: "bin",
                });
            }
            let mut fnm: Vec<PathBuf> = m.fnm_dir.iter().cloned().collect();
            fnm.extend(h(".local/share/fnm"));
            fnm.extend(h("Library/Application Support/fnm"));
            fnm.extend(h(".fnm"));
            fnm.dedup();
            for f in fnm {
                out.push(Place::Versions {
                    root: f.join("node-versions"),
                    bin: "installation/bin",
                });
            }
            out.extend(h(".local/bin").map(dir));
            out.push(dir("/usr/bin".into()));
        }
        Program::Uv => {
            out.extend(h(".local/bin").map(dir));
            out.extend(h(".cargo/bin").map(dir));
            out.push(dir("/opt/homebrew/bin".into()));
            out.push(dir("/usr/local/bin".into()));
            out.push(dir("/usr/bin".into()));
        }
    }
    out
}

/// Why `candidate` (an existing file) can't be used, if it can't: inside the repository, or a
/// version manager's shim (asdf, mise, pyenv, volta), which picks the version from files in
/// the folder it runs in, that is the repository.
fn rejected(program: Program, candidate: &Path, avoid: &[&Path]) -> Option<&'static str> {
    let real = candidate.canonicalize().unwrap_or(candidate.to_path_buf());
    let inside = |p: &Path| {
        avoid.iter().any(|a| {
            let a_real = a.canonicalize().unwrap_or(a.to_path_buf());
            p.starts_with(a) || p.starts_with(&a_real)
        })
    };
    if inside(candidate) || inside(&real) {
        return Some("inside the repository");
    }
    let shim_dir = |p: &Path| p.components().any(|c| c.as_os_str() == "shims");
    let name = real
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    if shim_dir(candidate) || shim_dir(&real) || name.starts_with("volta-shim") {
        return Some("a version manager's shim, which picks the version from the repository");
    }
    let plain = name.strip_suffix(".exe").unwrap_or(&name);
    if program != Program::Npm && plain != program.name() {
        return Some("not the program itself (a wrapper or shim)");
    }
    None
}

/// Looks for `program`: the login shell's `PATH` (absolute folders only), then the
/// well-known places; `near` (node's folder, for npm) first. Nothing inside `avoid` (the
/// repository) is used.
pub fn search(program: Program, near: Option<&Path>, avoid: &[&Path], m: &Machine) -> Search {
    let mut searched = vec![];
    let mut tried: Vec<PathBuf> = vec![];
    let mut found = None;
    let mut try_dir = |dir: &Path, searched_note: &mut Vec<String>| -> Option<PathBuf> {
        if !dir.is_absolute() || tried.iter().any(|t| t == dir) {
            return None;
        }
        tried.push(dir.to_path_buf());
        let c = dir.join(program.name());
        if !c.is_file() {
            return None;
        }
        match rejected(program, &c, avoid) {
            None => Some(c),
            Some(why) => {
                searched_note.push(format!("{} (skipped: {why})", c.display()));
                None
            }
        }
    };
    let mut notes = vec![];
    if let Some(dir) = near {
        searched.push(format!("{} (next to node)", dir.display()));
        found = try_dir(dir, &mut notes);
    }
    if found.is_none() {
        match &m.shell {
            Some(shell) => {
                let label = format!("the login shell's PATH ({} -l)", shell.display());
                match login_shell_path(shell, m.home.as_deref(), m.shell_timeout) {
                    Some(path) => {
                        searched.push(label);
                        found = std::env::split_paths(&path).find_map(|d| try_dir(&d, &mut notes));
                    }
                    None => searched.push(format!(
                        "{label}: it printed no PATH within {} s",
                        m.shell_timeout.as_secs()
                    )),
                }
            }
            None => searched.push("the login shell's PATH: SHELL is not set".into()),
        }
    }
    if found.is_none() {
        for place in known_places(program, m) {
            searched.push(place.describe(m.home.as_deref()));
            if let Some(c) = place.dirs().iter().find_map(|d| try_dir(d, &mut notes)) {
                found = Some(c);
                break;
            }
        }
    }
    searched.extend(notes);
    Search {
        program,
        found,
        searched,
    }
}

/// `program`'s path: its setting when set (it must exist), else one found earlier in this
/// process, else a [`search`], saved through `cfg.save`. `Err` is the message for the
/// reader: a setting that names nothing, or everything searched.
pub fn resolve(
    program: Program,
    cfg: &ToolConfig,
    near: Option<&Path>,
    avoid: &[&Path],
    m: &Machine,
) -> Result<PathBuf, String> {
    if let Some(p) = cfg.setting(program) {
        return if p.is_file() {
            Ok(p.to_path_buf())
        } else {
            Err(format!(
                "{key} is {path}, which is not a file. Fix the setting, or clear it so Chorong looks for {name} again: cho settings set {key} \"\"",
                key = program.key(),
                path = p.display(),
                name = program.name(),
            ))
        };
    }
    if let Ok(found) = FOUND.lock()
        && let Some((_, p)) = found.iter().find(|(q, p)| *q == program && p.is_file())
    {
        return Ok(p.clone());
    }
    let s = search(program, near, avoid, m);
    let Some(p) = s.found else {
        return Err(s.not_found());
    };
    if let Ok(mut found) = FOUND.lock() {
        found.retain(|(q, _)| *q != program);
        found.push((program, p.clone()));
    }
    if let Some(save) = cfg.save {
        save(program.key(), &p);
    }
    Ok(p)
}

/// Forgets the programs found in this process (tests).
#[cfg(test)]
pub(crate) fn forget_found() {
    if let Ok(mut f) = FOUND.lock() {
        f.clear();
    }
    if let Ok(mut s) = SHELL_PATH.lock() {
        *s = None;
    }
}

/// `PATH` for a program Chorong starts: `first` folders, then the viewer's own `PATH`.
pub fn path_with(first: &[&Path]) -> String {
    let rest = std::env::var_os("PATH").unwrap_or_default();
    let dirs: Vec<PathBuf> = first
        .iter()
        .map(|p| p.to_path_buf())
        .chain(std::env::split_paths(&rest))
        .collect();
    std::env::join_paths(dirs)
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|_| rest.to_string_lossy().into_owned())
}

// ---------------------------------------------------------------------------------------------
// Installing the indexers

/// An indexer ready to run with `node`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ready {
    /// The program's script, absolute.
    pub script: PathBuf,
    /// The version that will run, when known.
    pub version: Option<String>,
    /// From the reader's own install (the `index.*_install` setting).
    pub own: bool,
}

/// Why an indexer is not ready.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NotReady {
    /// What went wrong, for the reader.
    Failed(String),
    Cancelled,
}

/// `tools/` in Chorong's index cache: one folder per indexer, one folder per version below it.
pub fn tools_dir(cache_base: &Path) -> PathBuf {
    cache_base.join("tools")
}

fn tool_dir(cache_base: &Path, tool: Tool) -> PathBuf {
    tools_dir(cache_base).join(tool.name())
}

/// The script of `tool`'s installed `version`, if that version is installed completely.
fn installed_script(cache_base: &Path, tool: Tool, version: &Version) -> Option<PathBuf> {
    let p = tool_dir(cache_base, tool)
        .join(version.to_string())
        .join("node_modules")
        .join(tool.package())
        .join(tool.script());
    p.is_file().then_some(p)
}

/// Every version of `tool` installed in the cache.
fn installed_versions(cache_base: &Path, tool: Tool) -> Vec<Version> {
    let Ok(rd) = std::fs::read_dir(tool_dir(cache_base, tool)) else {
        return vec![];
    };
    rd.flatten()
        .filter_map(|e| Version::parse(&e.file_name().to_string_lossy()))
        .filter(|v| installed_script(cache_base, tool, v).is_some())
        .collect()
}

/// The last registry answer for a range or `latest` (`tools/<tool>/resolved.json`).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
struct Resolved {
    spec: String,
    version: String,
    /// Seconds since the Unix epoch.
    checked: u64,
}

const RESOLVED_FILE: &str = "resolved.json";

#[allow(clippy::disallowed_methods)] // Chorong's own cache file
fn read_resolved(cache_base: &Path, tool: Tool) -> Option<Resolved> {
    let text = std::fs::read_to_string(tool_dir(cache_base, tool).join(RESOLVED_FILE)).ok()?;
    serde_json::from_str(&text).ok()
}

fn write_resolved(cache_base: &Path, tool: Tool, r: &Resolved) {
    let dir = tool_dir(cache_base, tool);
    let tmp = dir.join(format!("{RESOLVED_FILE}.{}", std::process::id()));
    if let Ok(text) = serde_json::to_string_pretty(r)
        && std::fs::write(&tmp, text).is_ok()
    {
        std::fs::rename(&tmp, dir.join(RESOLVED_FILE)).ok();
    }
}

/// The reader's own install: a script file, a package folder, or a folder with the package in
/// its `node_modules`.
fn own_script(tool: Tool, own: &Path) -> Result<PathBuf, String> {
    let candidates = [
        own.to_path_buf(),
        own.join(tool.script()),
        own.join("node_modules")
            .join(tool.package())
            .join(tool.script()),
    ];
    candidates
        .into_iter()
        .find(|c| c.is_file())
        .ok_or_else(|| {
            format!(
                "{key} is {path}, which is not {name}'s package folder or its {script}. Fix the setting, or clear it to use Chorong's copy: cho settings set {key} \"\"",
                key = tool.install_key(),
                path = own.display(),
                name = tool.name(),
                script = tool.script(),
            )
        })
}

/// The version recorded in the `package.json` beside `script`'s package, if it names one.
#[allow(clippy::disallowed_methods)] // the reader's own install, which they named in a setting
fn own_version(tool: Tool, script: &Path) -> Option<String> {
    let depth = tool.script().split('/').count();
    let pkg = script.ancestors().nth(depth)?.join("package.json");
    let v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(pkg).ok()?).ok()?;
    v.get("version")?.as_str().map(str::to_string)
}

/// What the next build of `tool` will do, without asking the registry: for the banner.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolPlan {
    pub tool: Tool,
    /// The reader's own install, when set.
    pub own: Option<PathBuf>,
    /// The installed version that will run unless a check finds a newer match.
    pub installed: Option<String>,
    /// The registry is asked first (a range or `latest`, last checked too long ago, or
    /// nothing installed matches).
    pub checks_registry: bool,
    /// A version that will be downloaded (exact and not installed).
    pub downloads: Option<String>,
    /// The version setting is not valid; the build will fail with this message.
    pub invalid: Option<String>,
}

/// [`ToolPlan`] for `tool` under `cfg`, from the cache alone (no network, no programs run).
pub fn plan(tool: Tool, cfg: &ToolConfig, cache_base: &Path) -> ToolPlan {
    let choice = cfg.choice(tool);
    let mut p = ToolPlan {
        tool,
        own: choice.own.clone(),
        installed: None,
        checks_registry: false,
        downloads: None,
        invalid: None,
    };
    if p.own.is_some() {
        return p;
    }
    let spec = match Spec::parse(&choice.version) {
        Ok(s) => s,
        Err(e) => {
            p.invalid = Some(format!("{}: {e}", tool.version_key()));
            return p;
        }
    };
    let installed = installed_versions(cache_base, tool);
    match &spec {
        Spec::Exact(v) => {
            if installed.contains(v) {
                p.installed = Some(v.to_string());
            } else {
                p.downloads = Some(v.to_string());
            }
        }
        _ => {
            let fresh = fresh_resolution(cache_base, tool, &spec, cfg.check_every, &installed);
            match fresh {
                Some(v) => p.installed = Some(v.to_string()),
                None => {
                    p.installed = spec.best(&installed).map(|v| v.to_string());
                    p.checks_registry = cfg.check_every.is_some() || p.installed.is_none();
                }
            }
        }
    }
    p
}

/// The recorded registry answer for `spec`, when it is installed and recent enough (or
/// checks are off).
fn fresh_resolution(
    cache_base: &Path,
    tool: Tool,
    spec: &Spec,
    check_every: Option<Duration>,
    installed: &[Version],
) -> Option<Version> {
    let r = read_resolved(cache_base, tool)?;
    let v = Version::parse(&r.version)?;
    let age = crate::cache::now_unix().saturating_sub(r.checked);
    let recent = check_every.is_none_or(|every| age < every.as_secs());
    (r.spec == spec.to_string() && spec.matches(&v) && installed.contains(&v) && recent)
        .then_some(v)
}

/// The versions in `npm view … version --json` output (a string or a list of strings).
fn versions_in(output: &str) -> Vec<Version> {
    output
        .split(|c: char| !(c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '+')))
        .filter_map(Version::parse)
        .collect()
}

/// Everything `ensure` needs to run npm.
struct Npm<'a> {
    npm: &'a Path,
    node_dir: &'a Path,
    cancel: &'a CancelToken,
}

impl Npm<'_> {
    fn command(&self, args: Vec<String>, cwd: &Path) -> IndexerCommand {
        IndexerCommand {
            program: self.npm.to_string_lossy().into_owned(),
            args,
            cwd: cwd.to_path_buf(),
            // npm is a Node script: Node's own folder first, for its `#!/usr/bin/env node`.
            env: vec![("PATH".into(), path_with(&[self.node_dir]))],
        }
    }

    /// The registry's matches for `spec`, highest first is not promised.
    fn view(&self, tool: Tool, spec: &Spec, cwd: &Path) -> Result<Vec<Version>, NotReady> {
        let target = format!("{}@{spec}", tool.package());
        let cmd = self.command(
            ["view", "--json", &target, "version"]
                .map(String::from)
                .into(),
            cwd,
        );
        let (outcome, stdout) = run_with_output(&cmd, CHECK_TIMEOUT, self.cancel);
        if outcome.ended == Ended::Cancelled {
            return Err(NotReady::Cancelled);
        }
        if !outcome.success() {
            return Err(NotReady::Failed(format!(
                "`npm view {target} version` failed:\n{}",
                outcome.stderr_tail.trim()
            )));
        }
        Ok(versions_in(&stdout))
    }

    /// Installs `tool@version` into `tools/<tool>/<version>`: into a fresh folder of its own
    /// (install scripts off, no lockfile, nothing saved), run in that folder, then moved into
    /// place in one step once complete.
    fn install(
        &self,
        tool: Tool,
        version: &Version,
        cache_base: &Path,
        on_tick: &mut dyn FnMut(&Tick),
    ) -> Result<PathBuf, NotReady> {
        static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let dir = tool_dir(cache_base, tool);
        let tmp = dir.join(format!(
            ".install-{}.{}",
            std::process::id(),
            N.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&tmp)
            .map_err(|e| NotReady::Failed(format!("cannot create {}: {e}", tmp.display())))?;
        let target = format!("{}@{version}", tool.package());
        let args: Vec<String> = [
            "install",
            "--ignore-scripts",
            "--no-save",
            "--no-package-lock",
            "--no-audit",
            "--no-fund",
            "--prefix",
        ]
        .map(String::from)
        .into_iter()
        .chain([
            tmp.to_string_lossy().into_owned(),
            "--".into(),
            target.clone(),
        ])
        .collect();
        let cmd = self.command(args, &tmp);
        let outcome = run_with(&cmd, INSTALL_TIMEOUT, self.cancel, on_tick);
        let script = tmp
            .join("node_modules")
            .join(tool.package())
            .join(tool.script());
        let fail = |msg: String| {
            std::fs::remove_dir_all(&tmp).ok();
            Err(msg)
        };
        let result = match outcome.ended {
            Ended::Cancelled => {
                std::fs::remove_dir_all(&tmp).ok();
                return Err(NotReady::Cancelled);
            }
            _ if !outcome.success() => fail(format!(
                "installing {target} failed (npm install --ignore-scripts):\n{}",
                outcome.stderr_tail.trim()
            )),
            _ if !script.is_file() => {
                fail(format!("installing {target} left no {}", tool.script()))
            }
            _ => Ok(()),
        };
        result.map_err(NotReady::Failed)?;
        let dest = dir.join(version.to_string());
        if std::fs::rename(&tmp, &dest).is_err() {
            // Another build installed it first; keep theirs.
            std::fs::remove_dir_all(&tmp).ok();
        }
        installed_script(cache_base, tool, version).ok_or_else(|| {
            NotReady::Failed(format!("cannot move {target} into {}", dest.display()))
        })
    }
}

/// Removes install folders a crashed build left behind (a day old).
fn remove_stale_installs(dir: &Path) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for e in rd.flatten() {
        let stale = e.file_name().to_string_lossy().starts_with(".install-")
            && e.metadata()
                .and_then(|m| m.modified())
                .ok()
                .and_then(|t| t.elapsed().ok())
                .is_some_and(|age| age > Duration::from_secs(24 * 60 * 60));
        if stale {
            std::fs::remove_dir_all(e.path()).ok();
        }
    }
}

/// Makes `tool` ready to run: the reader's own install, or Chorong's copy of the version the
/// setting names, installed (and for a range or `latest`, checked against the registry) as
/// needed. npm is found only when something must be downloaded or checked. `on_tick` hears
/// npm's progress, with the step named.
#[allow(clippy::too_many_arguments)] // each is a separate input of the build; a struct would only rename them
pub fn ensure(
    tool: Tool,
    cfg: &ToolConfig,
    node: &Path,
    cache_base: &Path,
    avoid: &[&Path],
    machine: &Machine,
    cancel: &CancelToken,
    on_tick: &mut dyn FnMut(&Tick),
) -> Result<Ready, NotReady> {
    let choice = cfg.choice(tool);
    if let Some(own) = &choice.own {
        let script = own_script(tool, own).map_err(NotReady::Failed)?;
        return Ok(Ready {
            version: own_version(tool, &script),
            script,
            own: true,
        });
    }
    let spec = Spec::parse(&choice.version)
        .map_err(|e| NotReady::Failed(format!("{}: {e}", tool.version_key())))?;
    let dir = tool_dir(cache_base, tool);
    std::fs::create_dir_all(&dir)
        .map_err(|e| NotReady::Failed(format!("cannot create {}: {e}", dir.display())))?;
    remove_stale_installs(&dir);
    let installed = installed_versions(cache_base, tool);
    let ready = |v: &Version| {
        installed_script(cache_base, tool, v).map(|script| Ready {
            script,
            version: Some(v.to_string()),
            own: false,
        })
    };
    // Nothing to download or check: no npm needed.
    let wanted = match &spec {
        Spec::Exact(v) if installed.contains(v) => return Ok(ready(v).expect("installed")),
        Spec::Exact(v) => Some(v.clone()),
        _ => {
            if let Some(v) = fresh_resolution(cache_base, tool, &spec, cfg.check_every, &installed)
            {
                return Ok(ready(&v).expect("installed"));
            }
            if cfg.check_every.is_none()
                && let Some(v) = spec.best(&installed)
            {
                return Ok(ready(v).expect("installed"));
            }
            None
        }
    };
    let node_dir = node.parent().unwrap_or(Path::new("/"));
    let npm_path =
        resolve(Program::Npm, cfg, Some(node_dir), avoid, machine).map_err(NotReady::Failed)?;
    let npm = Npm {
        npm: &npm_path,
        node_dir,
        cancel,
    };
    let version = match wanted {
        Some(v) => v,
        None => {
            let checked = npm.view(tool, &spec, &dir);
            let best_installed = spec.best(&installed).cloned();
            match checked {
                Err(NotReady::Cancelled) => return Err(NotReady::Cancelled),
                Ok(found) if spec.best(&found).is_some() => {
                    let v = spec.best(&found).cloned().expect("checked");
                    write_resolved(
                        cache_base,
                        tool,
                        &Resolved {
                            spec: spec.to_string(),
                            version: v.to_string(),
                            checked: crate::cache::now_unix(),
                        },
                    );
                    v
                }
                // Offline, or the registry had nothing: keep using what is installed.
                Ok(_) | Err(_) if best_installed.is_some() => {
                    return Ok(ready(best_installed.as_ref().expect("some")).expect("installed"));
                }
                Ok(_) => {
                    return Err(NotReady::Failed(format!(
                        "the registry has no {} matching {spec} ({})",
                        tool.package(),
                        tool.version_key()
                    )));
                }
                Err(e) => return Err(e),
            }
        }
    };
    if let Some(r) = ready(&version) {
        return Ok(r);
    }
    let label = format!("installing {} {version}", tool.name());
    let mut tick = |t: &Tick| {
        let mut t = t.clone();
        t.progress.step = Some(label.clone());
        on_tick(&t);
    };
    let script = npm.install(tool, &version, cache_base, &mut tick)?;
    Ok(Ready {
        script,
        version: Some(version.to_string()),
        own: false,
    })
}

// ---------------------------------------------------------------------------------------------
// Python environments

/// The project's virtual environment (`.venv` with a `pyvenv.cfg`, a real folder, not a
/// link), in the project's folder or else at the repository root, as `uv sync` makes it.
pub fn project_venv(project: &Project) -> Option<PathBuf> {
    let repo = repo_root(project);
    let mut dirs = vec![project.root.join(".venv")];
    if repo != project.root {
        dirs.push(repo.join(".venv"));
    }
    dirs.into_iter().find(|d| {
        std::fs::symlink_metadata(d).is_ok_and(|m| m.is_dir()) && d.join("pyvenv.cfg").is_file()
    })
}

/// The repository root a project was found in (its `root` without its `rel`).
pub fn repo_root(project: &Project) -> PathBuf {
    let depth = project.rel.split('/').filter(|s| !s.is_empty()).count();
    project
        .root
        .ancestors()
        .nth(depth)
        .unwrap_or(&project.root)
        .to_path_buf()
}

#[cfg(test)]
#[allow(clippy::disallowed_methods)] // tests read their own temp files
pub(crate) mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("cho-tools-{name}-{}", std::process::id()));
        std::fs::remove_dir_all(&d).ok();
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[cfg(unix)]
    pub(crate) fn exe(path: &Path, body: &str) {
        use std::os::unix::fs::PermissionsExt;
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, body).unwrap();
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    fn machine(home: &Path, shell: Option<PathBuf>) -> Machine {
        Machine {
            shell,
            home: Some(home.to_path_buf()),
            nvm_dir: None,
            fnm_dir: None,
            volta_home: None,
            shell_timeout: Duration::from_secs(5),
        }
    }

    #[test]
    fn config_reads_settings_and_falls_back_to_the_pins() {
        let c = ToolConfig::from_lookup(|_| None);
        assert_eq!(c.typescript.version, "0.6.0");
        assert_eq!(c.python.version, "0.6.8");
        assert_eq!(c.check_every, Some(days(7)));
        assert!(c.node.is_none() && c.typescript.own.is_none());
        let c = ToolConfig::from_lookup(|k| {
            Some(
                match k {
                    keys::NODE => "/x/node",
                    keys::TYPESCRIPT_VERSION => "^0.6.0",
                    keys::VERSION_CHECK => "false",
                    keys::TYPESCRIPT_INSTALL => "",
                    _ => return None,
                }
                .to_string(),
            )
        });
        assert_eq!(c.node.as_deref(), Some(Path::new("/x/node")));
        assert_eq!(c.typescript.version, "^0.6.0");
        assert!(c.check_every.is_none() && c.typescript.own.is_none());
        let c = ToolConfig::from_lookup(|k| (k == keys::VERSION_CHECK_DAYS).then(|| "2".into()));
        assert_eq!(c.check_every, Some(days(2)));
    }

    /// The login shell's PATH is read (with a fake `$SHELL`), relative and repository folders
    /// are skipped, and shims are passed over for the real program.
    #[cfg(unix)]
    #[test]
    fn login_shell_path_finds_node_but_never_the_repository_or_a_shim() {
        forget_found();
        let d = tmp("shell");
        let home = d.join("home");
        let repo = d.join("repo");
        let real = d.join("opt/node/bin");
        exe(&repo.join("node_modules/.bin/node"), "#!/bin/sh\n");
        exe(&d.join("asdf/shims/node"), "#!/bin/sh\n");
        exe(&real.join("node"), "#!/bin/sh\n");
        exe(&real.join("npm"), "#!/bin/sh\n");
        let shell = d.join("fake-shell");
        exe(
            &shell,
            &format!(
                "#!/bin/sh\necho 'Welcome!'\necho 'PATH=node_modules/.bin:{}:{}:{}'\necho 'OTHER=1'\n",
                repo.join("node_modules/.bin").display(),
                d.join("asdf/shims").display(),
                real.display()
            ),
        );
        std::fs::create_dir_all(&home).unwrap();
        let m = machine(&home, Some(shell.clone()));
        let s = search(Program::Node, None, &[&repo], &m);
        assert_eq!(
            s.found.as_deref(),
            Some(real.join("node").as_path()),
            "{s:?}"
        );
        assert!(s.searched[0].contains("login shell"), "{:?}", s.searched);
        assert!(
            s.searched
                .iter()
                .any(|l| l.contains("inside the repository")),
            "{:?}",
            s.searched
        );
        assert!(
            s.searched.iter().any(|l| l.contains("shim")),
            "{:?}",
            s.searched
        );
        // npm next to the node that was found
        let n = search(Program::Npm, Some(&real), &[&repo], &m);
        assert_eq!(n.found.as_deref(), Some(real.join("npm").as_path()));
        forget_found();
        std::fs::remove_dir_all(d).ok();
    }

    /// A shell that hangs is stopped at the timeout, and the known places are searched next;
    /// nvm's highest version wins.
    #[cfg(unix)]
    #[test]
    fn a_hanging_shell_times_out_and_nvm_is_searched() {
        forget_found();
        let d = tmp("nvm");
        let home = d.join("home");
        exe(
            &home.join(".nvm/versions/node/v18.20.0/bin/node"),
            "#!/bin/sh\n",
        );
        exe(
            &home.join(".nvm/versions/node/v22.3.0/bin/node"),
            "#!/bin/sh\n",
        );
        let shell = d.join("slow-shell");
        exe(&shell, "#!/bin/sh\nsleep 30\n");
        let mut m = machine(&home, Some(shell));
        m.shell_timeout = Duration::from_millis(300);
        let start = Instant::now();
        let s = search(Program::Node, None, &[], &m);
        assert!(start.elapsed() < Duration::from_secs(5));
        // /opt/homebrew/bin and /usr/local/bin come first; on a machine with node there, that
        // is what is found, which is also right.
        let found = s.found.unwrap();
        if !found.starts_with("/opt/homebrew") && !found.starts_with("/usr/local") {
            assert_eq!(found, home.join(".nvm/versions/node/v22.3.0/bin/node"));
        }
        assert!(s.searched[0].contains("no PATH within"), "{:?}", s.searched);
        forget_found();
        std::fs::remove_dir_all(d).ok();
    }

    #[test]
    fn not_found_lists_every_place_and_the_setting() {
        let m = Machine {
            shell: None,
            home: Some("/nonexistent-home".into()),
            nvm_dir: None,
            fnm_dir: None,
            volta_home: None,
            shell_timeout: Duration::from_secs(1),
        };
        let s = Search {
            program: Program::Uv,
            found: None,
            searched: {
                let mut v = vec!["the login shell's PATH: SHELL is not set".to_string()];
                v.extend(
                    known_places(Program::Uv, &m)
                        .iter()
                        .map(|p| p.describe(m.home.as_deref())),
                );
                v
            },
        };
        let msg = s.not_found();
        for want in [
            "uv not found",
            "SHELL is not set",
            "~/.local/bin",
            "~/.cargo/bin",
            "/opt/homebrew/bin",
            "cho settings set tools.uv",
        ] {
            assert!(msg.contains(want), "{want}: {msg}");
        }
    }

    #[test]
    fn a_setting_that_names_nothing_is_an_error_not_a_search() {
        let cfg = ToolConfig {
            node: Some("/nonexistent/node".into()),
            ..ToolConfig::default()
        };
        let e = resolve(Program::Node, &cfg, None, &[], &Machine::current()).unwrap_err();
        assert!(
            e.contains("tools.node is /nonexistent/node")
                && e.contains("settings set tools.node \"\""),
            "{e}"
        );
    }

    #[test]
    fn registry_output_versions() {
        let v = versions_in("[\n  \"0.6.0\",\n  \"0.6.1\"\n]\n");
        assert_eq!(
            v.iter().map(|v| v.to_string()).collect::<Vec<_>>(),
            ["0.6.0", "0.6.1"]
        );
        assert_eq!(versions_in("\"1.2.3-rc.1\"").len(), 1);
    }

    #[test]
    fn own_install_accepts_a_folder_or_the_script() {
        let d = tmp("own");
        let pkg = d.join("lib/node_modules/@sourcegraph/scip-typescript");
        std::fs::create_dir_all(pkg.join("dist/src")).unwrap();
        std::fs::write(pkg.join("dist/src/main.js"), "").unwrap();
        std::fs::write(pkg.join("package.json"), r#"{"version":"0.9.1"}"#).unwrap();
        for own in [pkg.clone(), pkg.join("dist/src/main.js"), d.join("lib")] {
            let cfg = ToolConfig {
                typescript: Choice {
                    version: "0.6.0".into(),
                    own: Some(own.clone()),
                },
                ..ToolConfig::default()
            };
            let r = ensure(
                Tool::TypeScript,
                &cfg,
                Path::new("/no/node"),
                &d.join("cache"),
                &[],
                &Machine::current(),
                &CancelToken::new(),
                &mut |_| {},
            )
            .unwrap();
            assert_eq!(r.script, pkg.join("dist/src/main.js"), "{own:?}");
            assert_eq!(r.version.as_deref(), Some("0.9.1"));
            assert!(r.own);
        }
        assert!(!d.join("cache/tools").exists(), "nothing downloaded");
        std::fs::remove_dir_all(d).ok();
    }

    /// A fake npm stands in for the registry: it records where it ran and what it was asked,
    /// and "installs" by writing the package's script.
    #[cfg(unix)]
    pub(crate) fn fake_npm(d: &Path, versions: &str) -> PathBuf {
        let npm = d.join("bin/npm");
        let log = d.join("npm.log");
        exe(
            &npm,
            &format!(
                r#"#!/bin/sh
echo "cwd=$PWD args=$*" >> '{log}'
if [ "$1" = view ]; then echo '{versions}'; exit 0; fi
prefix=""; pkg=""
while [ $# -gt 0 ]; do
  case "$1" in --prefix) prefix="$2"; shift;; --) pkg="$2"; shift;; esac; shift
done
name="${{pkg%@*}}"; ver="${{pkg##*@}}"
if [ "$name" = "@sourcegraph/scip-typescript" ]; then s=dist/src/main.js; else s=index.js; fi
mkdir -p "$prefix/node_modules/$name/$(dirname $s)"
echo "// $ver" > "$prefix/node_modules/$name/$s"
"#,
                log = log.display()
            ),
        );
        npm
    }

    #[cfg(unix)]
    #[test]
    fn exact_versions_install_once_in_chorongs_folder() {
        let d = tmp("exact");
        let npm = fake_npm(&d, "[]");
        let base = d.join("cache");
        let cfg = ToolConfig {
            npm: Some(npm),
            ..ToolConfig::default()
        };
        let node = d.join("bin/node");
        let run = || {
            ensure(
                Tool::TypeScript,
                &cfg,
                &node,
                &base,
                &[],
                &Machine::current(),
                &CancelToken::new(),
                &mut |_| {},
            )
            .unwrap()
        };
        assert_eq!(
            plan(Tool::TypeScript, &cfg, &base).downloads.as_deref(),
            Some("0.6.0")
        );
        let r = run();
        assert_eq!(
            r.script,
            base.join("tools/scip-typescript/0.6.0/node_modules/@sourcegraph/scip-typescript/dist/src/main.js")
        );
        assert_eq!(r.version.as_deref(), Some("0.6.0"));
        let log = std::fs::read_to_string(d.join("npm.log")).unwrap();
        assert_eq!(log.lines().count(), 1, "{log}");
        assert!(
            log.contains("--ignore-scripts")
                && log.contains("-- @sourcegraph/scip-typescript@0.6.0"),
            "{log}"
        );
        assert!(
            log.contains("cwd=") && log.contains("/tools/scip-typescript/.install-"),
            "{log}"
        );
        // Second build: installed, npm not run.
        assert_eq!(run(), r);
        assert_eq!(
            std::fs::read_to_string(d.join("npm.log"))
                .unwrap()
                .lines()
                .count(),
            1
        );
        let p = plan(Tool::TypeScript, &cfg, &base);
        assert_eq!(
            (p.installed.as_deref(), p.downloads, p.checks_registry),
            (Some("0.6.0"), None, false)
        );
        std::fs::remove_dir_all(d).ok();
    }

    #[cfg(unix)]
    #[test]
    fn ranges_are_checked_then_remembered_until_the_check_is_due() {
        let d = tmp("range");
        let npm = fake_npm(&d, r#"["0.6.0", "0.6.2", "0.7.0"]"#);
        let base = d.join("cache");
        let mut cfg = ToolConfig {
            npm: Some(npm),
            ..ToolConfig::default()
        };
        cfg.typescript.version = "^0.6.0".into();
        let node = d.join("bin/node");
        let ensure_now = |cfg: &ToolConfig| {
            ensure(
                Tool::TypeScript,
                cfg,
                &node,
                &base,
                &[],
                &Machine::current(),
                &CancelToken::new(),
                &mut |_| {},
            )
            .unwrap()
        };
        assert!(plan(Tool::TypeScript, &cfg, &base).checks_registry);
        let r = ensure_now(&cfg);
        assert_eq!(r.version.as_deref(), Some("0.6.2"));
        let calls = || {
            std::fs::read_to_string(d.join("npm.log"))
                .unwrap()
                .lines()
                .count()
        };
        assert_eq!(calls(), 2, "one view, one install");
        // Checked recently: no registry call.
        assert_eq!(ensure_now(&cfg), r);
        assert_eq!(calls(), 2);
        let p = plan(Tool::TypeScript, &cfg, &base);
        assert_eq!(
            (p.installed.as_deref(), p.checks_registry),
            (Some("0.6.2"), false)
        );
        // The check is due: the registry is asked again.
        let mut stale = read_resolved(&base, Tool::TypeScript).unwrap();
        stale.checked -= 8 * 24 * 60 * 60;
        write_resolved(&base, Tool::TypeScript, &stale);
        assert!(plan(Tool::TypeScript, &cfg, &base).checks_registry);
        assert_eq!(ensure_now(&cfg), r);
        assert_eq!(calls(), 3, "a view, no install");
        // Checks off: the installed match runs without asking.
        let mut stale = read_resolved(&base, Tool::TypeScript).unwrap();
        stale.checked = 0;
        write_resolved(&base, Tool::TypeScript, &stale);
        cfg.check_every = None;
        assert_eq!(ensure_now(&cfg), r);
        assert_eq!(calls(), 3);
        std::fs::remove_dir_all(d).ok();
    }

    #[test]
    fn an_invalid_version_setting_fails_with_its_key() {
        let mut cfg = ToolConfig::default();
        cfg.python.version = ">=1".into();
        let d = tmp("invalid");
        let e = ensure(
            Tool::Python,
            &cfg,
            Path::new("/n/node"),
            &d,
            &[],
            &Machine::current(),
            &CancelToken::new(),
            &mut |_| {},
        )
        .unwrap_err();
        let NotReady::Failed(msg) = e else { panic!() };
        assert!(msg.starts_with("index.python_version:"), "{msg}");
        assert!(plan(Tool::Python, &cfg, &d).invalid.is_some());
        std::fs::remove_dir_all(d).ok();
    }

    #[test]
    fn venv_is_found_in_the_project_or_at_the_repository_root() {
        let d = tmp("venv");
        let project = Project {
            language: Language::Python,
            root: d.join("svc/api"),
            rel: "svc/api".into(),
        };
        std::fs::create_dir_all(&project.root).unwrap();
        assert_eq!(repo_root(&project), d);
        assert_eq!(project_venv(&project), None);
        std::fs::create_dir_all(d.join(".venv")).unwrap();
        assert_eq!(project_venv(&project), None, "no pyvenv.cfg");
        std::fs::write(d.join(".venv/pyvenv.cfg"), "").unwrap();
        assert_eq!(project_venv(&project), Some(d.join(".venv")));
        std::fs::create_dir_all(project.root.join(".venv")).unwrap();
        std::fs::write(project.root.join(".venv/pyvenv.cfg"), "").unwrap();
        assert_eq!(project_venv(&project), Some(project.root.join(".venv")));
        std::fs::remove_dir_all(d).ok();
    }
}
