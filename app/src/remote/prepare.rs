//! "Prepare and index" for a freshly opened remote worktree: what to do (a pure decision from
//! the backend's permission and the recipe), what the approval panel and the failure panel say,
//! the status words, and the per-project choice kept in `remote.json`. No UI, and no I/O but
//! `remote.json` (through `crate::prefs`): the adapter gathers the facts and `view::prepare`
//! acts on the decision.

/// What the backend says about indexing this worktree (`cho_sources::Permission`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Access {
    /// Not one of Chorong's cache worktrees: the viewer's own folder trust decides, not this.
    Local,
    /// The project's approval covers the commit checked out here.
    Approved,
    /// It does not (never approved, the recipe or a trigger file changed, or not allowed).
    NeedsApproval { reason: String },
}

/// How a trigger file differs from the approved state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileChange {
    /// Not part of an earlier approval (first approval, or the file is new).
    New,
    Changed,
    Removed,
}

impl FileChange {
    pub fn label(self) -> &'static str {
        match self {
            FileChange::New => "new",
            FileChange::Changed => "changed",
            FileChange::Removed => "removed",
        }
    }
}

impl From<cho_sources::prepare::Change> for FileChange {
    fn from(c: cho_sources::prepare::Change) -> Self {
        use cho_sources::prepare::Change;
        match c {
            Change::Added => FileChange::New,
            Change::Removed => FileChange::Removed,
            Change::Changed => FileChange::Changed,
        }
    }
}

/// The recipe and approval facts the decision and the panel need, from `PrepareView`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Summary {
    /// The project name (`github:acme/api`).
    pub repo: String,
    /// Each step as a person reads it (`Step::display`), in order.
    pub commands: Vec<String>,
    /// Where the recipe comes from, in words.
    pub source: String,
    pub notes: Vec<String>,
    /// The trigger files the approval covers (or that changed since it was given).
    pub files: Vec<(String, FileChange)>,
    /// An earlier approval existed and no longer covers this commit.
    pub reapproval: bool,
    pub recipe_changed: bool,
}

/// What to do after the open.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    /// Nothing from this feature: the box was unchecked, or the folder is not a cache
    /// worktree.
    Nothing,
    /// Open read-only, syntax only, and say why.
    SyntaxOnly(String),
    /// Ask first.
    Ask(Panel),
    /// Already approved: run the recipe (it skips itself when the last run covers it), then
    /// index.
    Run,
    /// Approved and nothing to run: index now.
    IndexNow,
}

/// The decision for a finished open. `summary` is the recipe view, or why there is none.
pub fn decide(checked: bool, access: &Access, summary: Result<&Summary, &str>) -> Decision {
    if !checked {
        return Decision::Nothing;
    }
    match (access, summary) {
        (Access::Local, _) => Decision::Nothing,
        (_, Err(why)) => Decision::SyntaxOnly(format!("Not indexed: {why}")),
        (Access::Approved, Ok(s)) if s.commands.is_empty() => Decision::IndexNow,
        (Access::Approved, Ok(_)) => Decision::Run,
        (Access::NeedsApproval { .. }, Ok(s)) => Decision::Ask(Panel::new(s)),
    }
}

/// Where prepare runs and what it may touch (design/sources.md, "Prepare").
pub const WHERE_NOTE: &str = "Prepare runs these commands inside Chorong's copy of the project, in its cache folder, \
     not in any folder of yours. Package managers may use their own shared stores (pnpm's store, \
     the npm or Cargo caches). Your host tokens are not passed to them.";

/// What indexing does with the same approval.
pub const INDEX_NOTE: &str = "The approval also lets Chorong's indexers run the project's code (build scripts, \
     procedural macros). It is asked again when the recipe or one of the build files changes.";

/// The approval panel's words (with [`WHERE_NOTE`] and [`INDEX_NOTE`], which never vary).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Panel {
    pub title: String,
    pub source: String,
    /// The recipe's commands, exactly.
    pub commands: Vec<String>,
    pub notes: Vec<String>,
    /// "Build files that changed since you approved:" or "Build files this approval covers:".
    pub files_heading: Option<String>,
    /// Path and "new" / "changed" / "removed".
    pub files: Vec<(String, &'static str)>,
    pub why: Option<String>,
}

impl Panel {
    pub fn new(s: &Summary) -> Self {
        let title = if s.reapproval {
            format!("Approve prepare and index for {} again?", s.repo)
        } else {
            format!("Prepare and index {}?", s.repo)
        };
        let why = s.reapproval.then(|| {
            match (s.recipe_changed, s.files.is_empty()) {
                (true, true) => "The prepare recipe changed since you approved it.",
                (true, false) => {
                    "The prepare recipe and build files changed since you approved it."
                }
                (false, _) => "Build files changed since you approved it.",
            }
            .to_string()
        });
        let files_heading = (!s.files.is_empty()).then(|| {
            if s.reapproval {
                "Build files that changed since you approved:".to_string()
            } else {
                "Build files this approval covers:".to_string()
            }
        });
        Panel {
            title,
            source: s.source.clone(),
            commands: s.commands.clone(),
            notes: s.notes.clone(),
            files_heading,
            files: s
                .files
                .iter()
                .map(|(p, c)| (p.clone(), c.label()))
                .collect(),
            why,
        }
    }

    /// The commands block: numbered, or a sentence when the recipe is empty.
    pub fn command_lines(&self) -> Vec<String> {
        if self.commands.is_empty() {
            return vec!["No commands to run before indexing.".to_string()];
        }
        self.commands
            .iter()
            .enumerate()
            .map(|(i, c)| format!("{}. {c}", i + 1))
            .collect()
    }

    /// The primary button.
    pub fn approve_label(&self) -> &'static str {
        if self.commands.is_empty() {
            "Approve and index"
        } else {
            "Approve and prepare"
        }
    }
}

/// Where prepare is, for the status bar.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Status {
    /// Waiting for the reader's answer to the approval panel.
    Asking,
    Running {
        step: usize,
        of: usize,
        command: String,
    },
    Cancelling,
    Prepared {
        /// False when the last run already covered the recipe and nothing ran.
        ran: bool,
    },
    Failed,
    /// Syntax only: unchecked, declined, cancelled or not allowed.
    Skipped,
}

impl Status {
    /// The words for the status bar; `None` when there is nothing to say.
    pub fn text(&self) -> Option<String> {
        Some(match self {
            Status::Asking => "prepare: waiting for your approval".into(),
            Status::Running { step, of, command } => format!("preparing {step}/{of} {command}"),
            Status::Cancelling => "cancelling prepare\u{2026}".into(),
            Status::Prepared { ran: true } => "prepared".into(),
            Status::Prepared { ran: false } => "prepared (up to date)".into(),
            Status::Failed => "prepare failed".into(),
            Status::Skipped => "not indexed (prepare skipped)".into(),
        })
    }

    /// Prepare is working: the status bar offers Cancel.
    pub fn running(&self) -> bool {
        matches!(self, Status::Running { .. } | Status::Cancelling)
    }
}

/// The failure panel: the failing step and the end of its output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FailPanel {
    pub title: String,
    /// "step 2 failed (exit 1): pnpm install", from the backend's record.
    pub message: String,
    pub tail: Vec<String>,
    /// Chorong's prepare log, when there is one.
    pub log: Option<String>,
    /// The backend's detail (the command and its error output), behind "Details".
    pub detail: Option<String>,
}

/// Most lines of output shown, and the longest line.
pub const TAIL_LINES: usize = 14;
const TAIL_WIDTH: usize = 200;

/// A panel from the backend's record: `result` is its `describe()` sentence, `message` the
/// record's message, `log` the end of the log (project output: control characters are
/// dropped, lines are cut).
pub fn fail_panel(result: &str, message: Option<&str>, log: &str) -> FailPanel {
    let clean = |l: &str| -> String {
        l.chars()
            .filter(|c| !c.is_control() || *c == '\t')
            .map(|c| if c == '\t' { ' ' } else { c })
            .take(TAIL_WIDTH)
            .collect()
    };
    let lines: Vec<String> = log
        .lines()
        .map(clean)
        .filter(|l| !l.trim().is_empty())
        .collect();
    let skip = lines.len().saturating_sub(TAIL_LINES);
    FailPanel {
        title: capitalise(result),
        message: message.map(clean).unwrap_or_default(),
        tail: lines[skip..].to_vec(),
        log: None,
        detail: None,
    }
}

/// What a finished prepare run means for the window: a failure no longer stops indexing (the
/// owner's "never hard-fail" decision, as in `cho index`): the index is built at once and
/// reads `partial (prepare failed)`, and the failure is a notice that does not block.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// False `ran`: the last run already covered the recipe and nothing ran.
    Prepared { ran: bool },
    Failed {
        /// The note handed to the index build.
        note: cho_semantic::PrepareNote,
        /// The non-blocking failure notice.
        panel: FailPanel,
    },
}

impl Outcome {
    pub fn status(&self) -> Status {
        match self {
            Outcome::Prepared { ran } => Status::Prepared { ran: *ran },
            Outcome::Failed { .. } => Status::Failed,
        }
    }

    /// The status, the index build's note and the notice, as the window keeps them.
    pub fn into_parts(self) -> (Status, Option<cho_semantic::PrepareNote>, Option<FailPanel>) {
        let status = self.status();
        match self {
            Outcome::Prepared { .. } => (status, None, None),
            Outcome::Failed { note, panel } => (status, Some(note), Some(panel)),
        }
    }
}

/// The outcome of a run that ended: `result` is the backend's, `log` its log file.
pub fn after_run(
    ran: bool,
    result: cho_sources::prepare::run::PrepareResult,
    message: Option<&str>,
    log: &std::path::Path,
    log_tail: &str,
) -> Outcome {
    let Some(failure) = result.prepare_failure() else {
        return Outcome::Prepared { ran };
    };
    let mut panel = fail_panel(result.describe(), message, log_tail);
    let log = log.display().to_string();
    panel.log = Some(log.clone());
    Outcome::Failed {
        note: cho_semantic::PrepareNote {
            result: failure,
            log,
        },
        panel,
    }
}

/// The outcome when prepare could not run at all (no record, no log).
pub fn after_error(message: &str, detail: Option<String>) -> Outcome {
    Outcome::Failed {
        note: cho_semantic::PrepareNote {
            result: cho_semantic::PrepareFailure::Failed,
            log: String::new(),
        },
        panel: FailPanel {
            detail,
            ..fail_panel("prepare could not run", Some(message), "")
        },
    }
}

/// After Update moved the worktree: whether to assess prepare again. Only when the window was
/// opened with "Prepare and index" (`wanted`), nothing is running, and the head moved.
/// The backend then decides (approval panel; run again only if its record no longer covers
/// the recipe, trigger files and toolchain; else just re-index).
pub fn recheck_after_update(wanted: bool, running: bool, old_head: &str, new_head: &str) -> bool {
    wanted && !running && !new_head.is_empty() && old_head != new_head
}

fn capitalise(s: &str) -> String {
    let mut c = s.chars();
    c.next()
        .map(|f| f.to_uppercase().collect::<String>() + c.as_str())
        .unwrap_or_default()
}

// ---- remote.json ----

/// What `remote.json` keeps: the last project. The "Prepare and index" box is never
/// remembered (it starts unchecked for every open); an older file's per-project `prepare`
/// choices are ignored and dropped on the next save.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct Prefs {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_project: Option<String>,
}

const PREFS_FILE: &str = "remote.json";

impl Prefs {
    /// What `remote.json` holds (the default when it is missing or unreadable).
    pub fn load() -> Self {
        crate::prefs::file(PREFS_FILE)
            .map(|p| crate::prefs::load(&p))
            .unwrap_or_default()
    }

    /// Remembers the project last opened.
    pub fn remember(repo: &str) -> Result<(), String> {
        let mut prefs = Self::load();
        prefs.last_project = Some(repo.to_string());
        crate::prefs::save_named(PREFS_FILE, &prefs)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn summary(commands: &[&str], files: &[(&str, FileChange)], reapproval: bool) -> Summary {
        Summary {
            repo: "github:acme/api".into(),
            commands: commands.iter().map(|s| s.to_string()).collect(),
            source: "the repository's .chorong/prepare.json".into(),
            notes: vec![],
            files: files.iter().map(|(p, c)| (p.to_string(), *c)).collect(),
            reapproval,
            recipe_changed: false,
        }
    }

    fn needs() -> Access {
        Access::NeedsApproval {
            reason: "needs approval".into(),
        }
    }

    #[test]
    fn unchecked_does_nothing_and_stays_syntax_only() {
        let s = summary(&["npm ci"], &[], false);
        for access in [Access::Approved, needs(), Access::Local] {
            assert_eq!(decide(false, &access, Ok(&s)), Decision::Nothing);
        }
    }

    #[test]
    fn approved_and_unchanged_runs_or_indexes() {
        let s = summary(&["npm ci", "npm run build"], &[], false);
        assert_eq!(decide(true, &Access::Approved, Ok(&s)), Decision::Run);
        let none = summary(&[], &[], false);
        assert_eq!(
            decide(true, &Access::Approved, Ok(&none)),
            Decision::IndexNow
        );
    }

    #[test]
    fn changed_or_new_recipes_ask_and_a_failed_view_is_syntax_only() {
        let s = summary(
            &["npm ci"],
            &[("package-lock.json", FileChange::Changed)],
            true,
        );
        assert!(matches!(
            decide(true, &needs(), Ok(&s)),
            Decision::Ask(p) if p.title.contains("again")
        ));
        assert_eq!(
            decide(true, &needs(), Err("acme/api is not a project")),
            Decision::SyntaxOnly("Not indexed: acme/api is not a project".into())
        );
        assert_eq!(decide(true, &Access::Local, Err("x")), Decision::Nothing);
    }

    #[test]
    fn the_panel_lists_the_commands_and_the_changed_files() {
        let s = summary(
            &["pnpm install --frozen-lockfile", "(in web) npm run build"],
            &[
                ("pnpm-lock.yaml", FileChange::Changed),
                ("crates/m/build.rs", FileChange::New),
                ("old/package.json", FileChange::Removed),
            ],
            true,
        );
        let p = Panel::new(&s);
        assert_eq!(
            p.title,
            "Approve prepare and index for github:acme/api again?"
        );
        assert_eq!(
            p.command_lines(),
            [
                "1. pnpm install --frozen-lockfile",
                "2. (in web) npm run build"
            ]
        );
        assert_eq!(
            p.files,
            [
                ("pnpm-lock.yaml".to_string(), "changed"),
                ("crates/m/build.rs".to_string(), "new"),
                ("old/package.json".to_string(), "removed"),
            ]
        );
        assert_eq!(
            p.files_heading.as_deref(),
            Some("Build files that changed since you approved:")
        );
        assert_eq!(p.approve_label(), "Approve and prepare");
        let first = Panel::new(&summary(&[], &[], false));
        assert_eq!(first.title, "Prepare and index github:acme/api?");
        assert_eq!(
            first.command_lines(),
            ["No commands to run before indexing."]
        );
        assert_eq!(first.approve_label(), "Approve and index");
        assert_eq!(first.files_heading, None);
    }

    #[test]
    fn status_words_are_brief_and_accurate() {
        let run = Status::Running {
            step: 2,
            of: 3,
            command: "pnpm install".into(),
        };
        assert_eq!(run.text().as_deref(), Some("preparing 2/3 pnpm install"));
        assert!(run.running() && Status::Cancelling.running());
        assert_eq!(
            Status::Prepared { ran: true }.text().as_deref(),
            Some("prepared")
        );
        assert_eq!(
            Status::Skipped.text().as_deref(),
            Some("not indexed (prepare skipped)")
        );
        assert!(!Status::Skipped.running() && !Status::Failed.running());
    }

    #[test]
    fn the_failure_panel_keeps_the_end_of_the_output_without_control_characters() {
        let log: String = (1..=30).map(|i| format!("line {i}\x1b[31m\n")).collect();
        let p = fail_panel(
            "prepare failed",
            Some("step 2 failed (exit 1): npm ci"),
            &log,
        );
        assert_eq!(p.title, "Prepare failed");
        assert_eq!(p.message, "step 2 failed (exit 1): npm ci");
        assert_eq!(p.tail.len(), TAIL_LINES);
        assert_eq!(p.tail.last().unwrap(), "line 30[31m");
    }

    #[test]
    fn remote_json_keeps_the_last_project_and_drops_old_prepare_choices() {
        let parse = |t: &str| serde_json::from_str::<Prefs>(t).unwrap();
        let p = parse(r#"{"last_project":"github:a/b","prepare":{"github:a/b":true}}"#);
        assert_eq!(p.last_project.as_deref(), Some("github:a/b"));
        let text = serde_json::to_string(&p).unwrap();
        assert!(!text.contains("prepare"), "{text}");
        assert_eq!(parse(&text), p);
    }

    #[test]
    fn a_failed_prepare_still_indexes_with_a_partial_note_and_a_notice() {
        use cho_sources::prepare::run::PrepareResult as R;
        let log = std::path::Path::new("/cache/prepare/pr-1.log");
        let Outcome::Failed { note, panel: f } = after_run(
            true,
            R::Failed,
            Some("step 2 failed (exit 1): npm ci"),
            log,
            "a\nb\n",
        ) else {
            panic!("a failed run is a failure");
        };
        assert_eq!(
            (note.result.as_str(), note.log.as_str()),
            ("failed", "/cache/prepare/pr-1.log")
        );
        assert_eq!(f.log.as_deref(), Some("/cache/prepare/pr-1.log"));
        assert_eq!(f.message, "step 2 failed (exit 1): npm ci");
        assert_eq!(f.tail, ["a", "b"]);
        for r in [R::TimedOut, R::MissingTool] {
            assert_eq!(after_run(true, r, None, log, "").status(), Status::Failed);
        }
        // Success: no note, no notice.
        let ok = after_run(false, R::Ok, None, log, "");
        assert_eq!(
            ok.into_parts(),
            (Status::Prepared { ran: false }, None, None)
        );
        // Could not run at all: still indexes.
        let (status, note, panel) =
            after_error("no cache", Some("git -C x worktree list".into())).into_parts();
        assert_eq!(status, Status::Failed);
        assert!(note.is_some());
        assert_eq!(
            panel.unwrap().detail.as_deref(),
            Some("git -C x worktree list")
        );
    }

    #[test]
    fn update_asks_again_only_for_a_new_head_in_a_window_that_wanted_prepare() {
        assert!(recheck_after_update(true, false, "aaa", "bbb"));
        assert!(!recheck_after_update(false, false, "aaa", "bbb"));
        assert!(!recheck_after_update(true, true, "aaa", "bbb"));
        // Same worktree, same head: nothing.
        assert!(!recheck_after_update(true, false, "aaa", "aaa"));
        assert!(!recheck_after_update(true, false, "aaa", ""));
    }
}
