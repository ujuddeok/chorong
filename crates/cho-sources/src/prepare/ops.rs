//! Prepare on [`Sources`]: what applies to an opened worktree, and running it.

use super::run::{self, PrepareRecord, PrepareResult, RunSpec};
use super::{ApprovalState, Effective, Triggers};
use crate::state::RepoState;
use crate::{Error, Opened, Project, Result, Sources};
use cho_core::RepoRef;
use serde::Serialize;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

/// Everything `cho prepare show` reports for one opened worktree.
#[derive(Debug, Clone, Serialize)]
pub struct PrepareView {
    pub repo: String,
    pub worktree: PathBuf,
    pub commit: String,
    pub effective: Effective,
    pub recipe_hash: String,
    pub triggers: Triggers,
    pub triggers_hash: String,
    pub approval: ApprovalState,
    /// The worktree's last prepare run, if any.
    pub last_run: Option<PrepareRecord>,
}

/// [`Sources::prepare_view_at`] with the cache and git given.
fn view_at(
    cache: &crate::Cache,
    git: &crate::Git,
    repo: &RepoRef,
    project: &Project,
    worktree: &Path,
    slug: &str,
    commit: &str,
) -> Result<PrepareView> {
    let paths = cache.repo(repo)?;
    let commit = commit.to_string();
    let effective = super::effective(git, &paths.bare, &commit, project.recipe.as_ref())?;
    let triggers = super::triggers(git, &paths.bare, &commit)?;
    let approval = super::check(project.approval.as_ref(), &effective, &triggers);
    let last_run = RepoState::load(&paths.state).prepare_record(slug);
    Ok(PrepareView {
        repo: repo.to_string(),
        worktree: worktree.to_path_buf(),
        recipe_hash: effective.recipe.hash(),
        triggers_hash: super::triggers_hash(&triggers),
        commit,
        effective,
        triggers,
        approval,
        last_run,
    })
}

/// The outcome of [`Sources::run_prepare`].
#[derive(Debug, Clone, Serialize)]
pub struct PrepareOutcome {
    /// False when the last run already covered this recipe, these trigger files and this
    /// toolchain, so nothing ran.
    pub ran: bool,
    pub record: PrepareRecord,
}

impl Sources<'_> {
    /// The recipe, trigger files and approval state for an opened worktree.
    pub fn prepare_view(
        &self,
        repo: &RepoRef,
        project: &Project,
        opened: &Opened,
    ) -> Result<PrepareView> {
        self.prepare_view_at(
            repo,
            project,
            &opened.worktree,
            &opened.slug,
            &opened.head.id,
        )
    }

    /// [`prepare_view`](Self::prepare_view) for a worktree given by folder name and commit.
    pub fn prepare_view_at(
        &self,
        repo: &RepoRef,
        project: &Project,
        worktree: &Path,
        slug: &str,
        commit: &str,
    ) -> Result<PrepareView> {
        view_at(
            &self.cache,
            &self.git,
            repo,
            project,
            worktree,
            slug,
            commit,
        )
    }

    /// The `CARGO_TARGET_DIR` shared by a project's worktrees.
    pub fn cargo_target_dir(&self, repo: &RepoRef) -> Result<PathBuf> {
        Ok(self.cache.repo(repo)?.dir.join("target"))
    }

    /// Runs the approved recipe in the opened worktree, unless the last run already covers
    /// it (`force` runs anyway). Refuses an unapproved recipe and any folder that is not one
    /// of Chorong's worktrees. `base_env` is the environment to start from (tokens are removed).
    pub fn run_prepare(
        &self,
        repo: &RepoRef,
        view: &PrepareView,
        slug: &str,
        base_env: &[(OsString, OsString)],
        force: bool,
    ) -> Result<PrepareOutcome> {
        self.run_prepare_with(
            repo,
            view,
            slug,
            base_env,
            force,
            &|_| {},
            &crate::Cancel::new(),
        )
    }

    /// [`run_prepare`](Self::run_prepare), telling `progress` when each step starts (step N
    /// of M) and stopping the running step when `cancel` is set (nothing is recorded then).
    #[allow(clippy::too_many_arguments)]
    pub fn run_prepare_with(
        &self,
        repo: &RepoRef,
        view: &PrepareView,
        slug: &str,
        base_env: &[(OsString, OsString)],
        force: bool,
        progress: &dyn Fn(crate::PrepareStage),
        cancel: &crate::Cancel,
    ) -> Result<PrepareOutcome> {
        if !view.approval.is_approved() {
            return Err(Error::msg(format!(
                "prepare for {repo} is not approved; review it with `cho prepare show {repo}` \
                 and approve it with `cho prepare approve {repo}`"
            )));
        }
        let paths = self.cache.repo(repo)?;
        let wt = paths.worktree(slug);
        self.check_own_worktree(&wt, &view.worktree)?;
        let _lock = self.cache.lock_with(&paths, self.lock_timeout, cancel)?;
        let env = run::environment(base_env, &paths.dir.join("target"));
        // `uv` and `node` from their settings (found once and saved when empty), never from
        // the worktree: a Finder or Dock launch has a minimal PATH.
        let tools = crate::tools::config(
            cho_settings::SettingsStore::open_default()
                .effective()
                .settings(),
        );
        let argv0: Vec<&str> = view
            .effective
            .recipe
            .steps
            .iter()
            .filter_map(|s| s.argv.first().map(String::as_str))
            .collect();
        let (env, tool_notes) = crate::tools::with_tool_dirs(env, &argv0, &tools, &wt);
        let toolchain = run::toolchain(&view.effective.recipe, &wt, &env);
        let mut state = RepoState::load(&paths.state);
        if !force
            && let Some(prev) = state.worktrees.get(slug).and_then(|w| w.prepare.as_ref())
            && prev.covers(&view.recipe_hash, &view.triggers_hash, &toolchain)
        {
            return Ok(PrepareOutcome {
                ran: false,
                record: prev.clone(),
            });
        }
        let log = paths.dir.join("prepare").join(format!("{slug}.log"));
        let out = run::run_recipe(&RunSpec {
            worktree: &wt,
            recipe: &view.effective.recipe,
            env,
            log: &log,
            progress,
            cancel,
        })?;
        // A missing program's message lists everywhere Chorong looked for it.
        let message = match out.message {
            Some(m) if out.result == run::PrepareResult::MissingTool && !tool_notes.is_empty() => {
                Some(format!("{m}\n{}", tool_notes.join("\n")))
            }
            m => m,
        };
        let record = PrepareRecord {
            recipe_hash: view.recipe_hash.clone(),
            triggers_hash: view.triggers_hash.clone(),
            toolchain,
            result: out.result,
            message,
            log,
            finished: crate::now_unix(),
            duration_ms: out.duration_ms,
        };
        if let Some(w) = state.worktrees.get_mut(slug) {
            w.prepare = Some(record.clone());
            state.save(&paths.state)?;
        }
        Ok(PrepareOutcome { ran: true, record })
    }

    /// Prepare runs only in a worktree folder of Chorong's cache.
    fn check_own_worktree(&self, expected: &Path, given: &Path) -> Result<()> {
        let canon = |p: &Path| p.canonicalize().ok();
        let ok = canon(expected).is_some()
            && canon(expected) == canon(given)
            && self.cache.worktree_owner(given).is_some();
        if ok {
            Ok(())
        } else {
            Err(Error::msg(format!(
                "refusing to prepare {}: Chorong prepares only its own worktrees in {}",
                given.display(),
                self.cache.root().display()
            )))
        }
    }
}

/// Stores the project's approval of `view` (what `cho prepare approve` does and the viewer's
/// "Approve and prepare" button): one approval covers prepare and indexing until the recipe or a
/// trigger file changes. A suggested recipe becomes the project's own.
pub fn approve_view(
    projects: &mut crate::Projects,
    repo: &RepoRef,
    view: &PrepareView,
) -> Result<()> {
    let mut p = projects.get(repo).cloned().unwrap_or_default();
    let mut a = super::approve(&view.effective, &view.triggers, &view.commit);
    if view.effective.source == super::RecipeSource::Suggested {
        p.recipe = Some(view.effective.recipe.clone());
        a.source = super::RecipeSource::Project;
    }
    p.approval = Some(a);
    projects.insert(repo, p).map(|_| ())
}

/// [`approve_view`] on the default projects file.
pub fn save_approval(repo: &RepoRef, view: &PrepareView) -> Result<()> {
    approve_view(&mut crate::Projects::open_default()?, repo, view)
}

/// The note an index built after this prepare records when the prepare failed (the index
/// is then partial), naming the failure and the log.
pub fn partial_note(record: &PrepareRecord) -> Option<cho_semantic::PrepareNote> {
    Some(cho_semantic::PrepareNote {
        result: record.result.prepare_failure()?,
        log: record.log.display().to_string(),
    })
}

impl PrepareResult {
    /// The failure an index built after this records (it is then `partial`); `None` when
    /// prepare did not fail.
    pub fn prepare_failure(self) -> Option<cho_semantic::PrepareFailure> {
        use cho_semantic::PrepareFailure as F;
        match self {
            PrepareResult::Failed => Some(F::Failed),
            PrepareResult::TimedOut => Some(F::TimedOut),
            PrepareResult::MissingTool => Some(F::MissingTool),
            PrepareResult::Ok | PrepareResult::NoRecipe => None,
        }
    }

    pub fn describe(self) -> &'static str {
        match self {
            PrepareResult::Ok => "prepared",
            PrepareResult::Failed => "prepare failed",
            PrepareResult::TimedOut => "prepare timed out",
            PrepareResult::MissingTool => "prepare could not start (a tool is missing)",
            PrepareResult::NoRecipe => "nothing to prepare",
        }
    }
}

/// May a folder be indexed, and by whose say-so.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Permission {
    /// Not one of Chorong's cache worktrees: the viewer's own trust (`trusted.json`) decides.
    Local,
    /// A cache worktree whose project approval covers the commit checked out there. Prepare
    /// and indexing still start only when the reader asks.
    CacheApproved { repo: String },
    /// A cache worktree its project approval doesn't cover. `reason` says how to approve;
    /// `changed` lists the trigger files that differ from an earlier approval.
    CacheNeedsApproval {
        repo: String,
        reason: String,
        recipe_changed: bool,
        changed: Vec<super::TriggerChange>,
    },
}

/// Whether `folder` may be indexed. For one of Chorong's cache worktrees only the project's
/// prepare/index approval counts (never `trusted.json`); for any other folder this says
/// [`Permission::Local`] and the project approvals never apply. The CLI and the viewer both
/// decide with this.
pub fn index_permission(folder: &Path) -> Permission {
    assess(folder).0
}

/// [`index_permission`] and the folder's [`PrepareView`] (recipe, trigger files, approval,
/// last run), worked out once: the approval decides the permission. The view is an error
/// sentence for a folder that is not a cache worktree. Blocks (git).
pub fn assess(folder: &Path) -> (Permission, std::result::Result<PrepareView, String>) {
    let Some(cache) = crate::Cache::default_location() else {
        return (
            Permission::Local,
            Err("cannot find a cache directory (HOME is not set)".into()),
        );
    };
    if cache.worktree_owner(folder).is_none() {
        return (Permission::Local, Err(NOT_A_WORKTREE.into()));
    }
    let projects = crate::Projects::open_default();
    assess_with(&cache, projects.as_ref().map_err(|e| e.to_string()), folder)
}

const NOT_A_WORKTREE: &str = "this folder is not one of Chorong's worktrees";

/// [`assess`] with the cache and the projects given.
pub fn assess_with(
    cache: &crate::Cache,
    projects: std::result::Result<&crate::Projects, String>,
    folder: &Path,
) -> (Permission, std::result::Result<PrepareView, String>) {
    let Some((repo, slug)) = cache.worktree_owner(folder) else {
        return (Permission::Local, Err(NOT_A_WORKTREE.into()));
    };
    let name = repo.to_string();
    let deny = |reason: String| {
        (
            Permission::CacheNeedsApproval {
                repo: name.clone(),
                reason: reason.clone(),
                recipe_changed: false,
                changed: vec![],
            },
            Err(reason),
        )
    };
    let project = match projects {
        Ok(p) => p.get(&repo).cloned(),
        Err(e) => return deny(e),
    };
    let Some(project) = project else {
        return deny(format!("{repo} is not a project"));
    };
    let Some(head) = cho_git::rev_parse(folder, "HEAD") else {
        return deny(format!("cannot read the commit of {}", folder.display()));
    };
    let git = crate::Git::default();
    let view = match view_at(cache, &git, &repo, &project, folder, &slug, &head) {
        Ok(v) => v,
        Err(e) => return deny(e.message),
    };
    let how = format!("`cho prepare show {repo}`, then `cho prepare approve {repo}`");
    let permission = match &view.approval {
        ApprovalState::Approved => Permission::CacheApproved { repo: name },
        ApprovalState::NotApproved => Permission::CacheNeedsApproval {
            repo: name,
            reason: format!("needs approval: indexing runs {repo}'s code; {how}"),
            recipe_changed: false,
            changed: vec![],
        },
        ApprovalState::Changed { recipe, triggers } => Permission::CacheNeedsApproval {
            repo: name,
            reason: format!(
                "needs approval again: {} changed since it was approved; {how}",
                if *recipe && !triggers.is_empty() {
                    "the prepare recipe and build files".to_string()
                } else if *recipe {
                    "the prepare recipe".to_string()
                } else {
                    triggers
                        .iter()
                        .map(|t| t.path.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                }
            ),
            recipe_changed: *recipe,
            changed: triggers.clone(),
        },
    };
    (permission, Ok(view))
}
