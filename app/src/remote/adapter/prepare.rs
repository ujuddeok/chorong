//! "Prepare and index" through the backend: assess a worktree, store an approval, run the
//! recipe.

use super::{Cache, Cancel, real_sources, require_cache};
use crate::remote::prepare::{Access, FileChange, Summary};
use cho_core::RepoRef;
use cho_sources::prepare::ApprovalState;
use cho_sources::prepare::ops::{Permission, PrepareView};
use cho_sources::{ErrorKind, PrepareStage};
use std::path::{Path, PathBuf};

/// What the backend says about a freshly opened worktree.
#[derive(Debug, Clone)]
pub struct Assessment {
    pub access: Access,
    /// The recipe and approval view, or why there is none. Kept to run and to approve.
    pub view: Result<PrepareView, String>,
}

impl Assessment {
    /// The facts the decision and the panel need.
    pub fn summary(&self) -> Result<Summary, String> {
        let v = self.view.as_ref().map_err(Clone::clone)?;
        Ok(summary_of(v))
    }
}

fn summary_of(v: &PrepareView) -> Summary {
    let source = v.effective.source.describe(&v.commit);
    let (files, reapproval, recipe_changed) = match &v.approval {
        ApprovalState::Changed { recipe, triggers } => (
            triggers
                .iter()
                .map(|t| (t.path.clone(), FileChange::from(t.change)))
                .collect(),
            true,
            *recipe,
        ),
        // A first approval covers every trigger file there is.
        _ => (
            v.triggers
                .keys()
                .map(|p| (p.clone(), FileChange::New))
                .collect(),
            false,
            false,
        ),
    };
    Summary {
        repo: v.repo.clone(),
        commands: v
            .effective
            .recipe
            .steps
            .iter()
            .map(|s| s.display())
            .collect(),
        source,
        notes: v.effective.notes.clone(),
        files,
        reapproval,
        recipe_changed,
    }
}

/// Asks the backend whether `folder` (the opened worktree) may be prepared and indexed, and
/// reads its recipe, trigger files and approval state (`ops::assess`, which works both out
/// at once). Blocks (git): background executor only.
pub fn assess(folder: &Path) -> Assessment {
    let (permission, view) = cho_sources::prepare::ops::assess(folder);
    assessment(permission, view)
}

#[cfg(test)]
fn assess_in(
    cache: Cache,
    projects: Result<&cho_sources::Projects, String>,
    folder: &Path,
) -> Assessment {
    let (permission, view) = cho_sources::prepare::ops::assess_with(&cache, projects, folder);
    assessment(permission, view)
}

fn assessment(permission: Permission, view: Result<PrepareView, String>) -> Assessment {
    let access = match permission {
        Permission::Local => Access::Local,
        Permission::CacheApproved { .. } => Access::Approved,
        Permission::CacheNeedsApproval { reason, .. } => Access::NeedsApproval { reason },
    };
    Assessment { access, view }
}

/// Stores the project's approval of the recipe in `view` (what `cho prepare approve` does).
/// The caller assesses again afterwards.
pub fn approve(view: &PrepareView) -> Result<(), String> {
    let repo = RepoRef::parse(&view.repo).map_err(|e| e.to_string())?;
    cho_sources::prepare::ops::save_approval(&repo, view).map_err(|e| e.to_string())
}

/// How a prepare run ended.
#[derive(Debug, Clone)]
pub enum RunEnd {
    /// It ran (or the last run already covered it: `ran` false). `failure` is set when it
    /// ended badly; `log_tail` is the end of its log.
    Done {
        ran: bool,
        result: cho_sources::prepare::run::PrepareResult,
        message: Option<String>,
        log: PathBuf,
        log_tail: String,
    },
    /// The reader cancelled: nothing is shown and nothing was recorded.
    Cancelled,
    /// Prepare could not run: the short sentence and the backend's detail.
    Error(String, Option<String>),
}

/// Runs the approved recipe in `folder` with `Sources::run_prepare_with`. `progress` is told
/// when each step starts. Blocks, possibly for minutes: background executor only.
pub fn run_prepare(
    folder: &Path,
    view: &PrepareView,
    force: bool,
    progress: &dyn Fn(PrepareStage),
    cancel: &Cancel,
) -> RunEnd {
    let cache = match require_cache() {
        Ok(cache) => cache,
        Err(e) => return RunEnd::Error(e, None),
    };
    run_prepare_in(cache, folder, view, force, progress, cancel)
}

fn run_prepare_in(
    cache: Cache,
    folder: &Path,
    view: &PrepareView,
    force: bool,
    progress: &dyn Fn(PrepareStage),
    cancel: &Cancel,
) -> RunEnd {
    let Some((repo, slug)) = cache.worktree_owner(folder) else {
        return RunEnd::Error("this folder is not one of Chorong's worktrees".into(), None);
    };
    let sources = real_sources(cache, None);
    // Prepare removes host tokens from this environment itself.
    let env: Vec<_> = std::env::vars_os().collect();
    match sources.run_prepare_with(&repo, view, &slug, &env, force, progress, cancel) {
        Ok(out) => RunEnd::Done {
            ran: out.ran,
            result: out.record.result,
            message: out.record.message.clone(),
            log: out.record.log.clone(),
            log_tail: log_tail(&out.record.log),
        },
        Err(e) if e.kind == ErrorKind::Cancelled => RunEnd::Cancelled,
        Err(e) => RunEnd::Error(e.message, e.detail),
    }
}

/// The last few kilobytes of Chorong's own prepare log (it holds the project's output).
fn log_tail(log: &Path) -> String {
    use std::io::{Read, Seek, SeekFrom};
    const BYTES: u64 = 6000;
    let Ok(mut f) = std::fs::File::open(log) else {
        return String::new();
    };
    let len = f.metadata().map(|m| m.len()).unwrap_or(0);
    if f.seek(SeekFrom::Start(len.saturating_sub(BYTES))).is_err() {
        return String::new();
    }
    let mut buf = Vec::new();
    let _ = f.take(BYTES).read_to_end(&mut buf);
    String::from_utf8_lossy(&buf).into_owned()
}

#[cfg(test)]
mod tests {
    use super::super::open::open_in;
    use super::super::test_support::*;
    use super::*;
    use cho_core::test_repo::git;
    use cho_sources::{Projects, TargetKind};

    /// A remote whose `main` ships a one-step recipe that writes a file.
    fn recipe_remote(dir: &Path) -> String {
        let work = dir.join("work");
        let bare = dir.join("remote.git");
        std::fs::create_dir_all(work.join(".chorong")).unwrap();
        git(&work, &["init", "-q", "-b", "main"]);
        std::fs::write(work.join("a.txt"), "one\n").unwrap();
        std::fs::write(
            work.join(".chorong/prepare.json"),
            r#"{"steps": [{"argv": ["sh", "-c", "echo prepared > made.txt"]}]}"#,
        )
        .unwrap();
        git(&work, &["add", "-A"]);
        git(&work, &["commit", "-q", "-m", "one"]);
        git(
            dir,
            &["init", "-q", "--bare", "-b", "main", bare.to_str().unwrap()],
        );
        git(&work, &["push", "-q", bare.to_str().unwrap(), "main"]);
        format!("file://{}", bare.display())
    }

    #[test]
    fn a_new_recipe_asks_then_an_approved_one_runs_through_the_backend() {
        use crate::remote::prepare::{Access, Decision, decide};
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        let url = recipe_remote(dir);
        let cache = Cache::at(dir.join("cache"));
        let mut projects = Projects::open(dir.join("projects.json")).unwrap();
        let req = request(&url, TargetKind::Default);
        let done = open_in(
            cache.clone(),
            &mut projects,
            backend(),
            &req,
            &|_| {},
            &Cancel::new(),
        )
        .unwrap();

        // Never approved: the panel asks, listing the recipe's command exactly.
        let a = assess_in(cache.clone(), Ok(&projects), &done.worktree);
        assert!(matches!(a.access, Access::NeedsApproval { .. }), "{a:?}");
        let summary = a.summary().unwrap();
        assert_eq!(summary.commands, ["sh -c 'echo prepared > made.txt'"]);
        assert!(
            summary
                .files
                .iter()
                .any(|(p, _)| p == ".chorong/prepare.json"),
            "{:?}",
            summary.files
        );
        let ask = decide(
            true,
            &a.access,
            a.summary().as_ref().map_err(String::as_str),
        );
        assert!(matches!(ask, Decision::Ask(_)), "{ask:?}");
        // Unchecked: nothing, whatever the backend says.
        assert_eq!(
            decide(
                false,
                &a.access,
                a.summary().as_ref().map_err(String::as_str)
            ),
            Decision::Nothing
        );
        // Running without approval is refused by the backend.
        let view = a.view.clone().unwrap();
        let refused = run_prepare_in(
            cache.clone(),
            &done.worktree,
            &view,
            false,
            &|_| {},
            &Cancel::new(),
        );
        assert!(matches!(refused, RunEnd::Error(..)), "{refused:?}");
        assert!(!done.worktree.join("made.txt").exists());

        // Approve (the backend function `cho prepare approve` uses), assess again, run.
        let repo = req.repo.clone();
        cho_sources::prepare::ops::approve_view(&mut projects, &repo, &view).unwrap();
        let a = assess_in(cache.clone(), Ok(&projects), &done.worktree);
        assert_eq!(a.access, Access::Approved, "{a:?}");
        let go = decide(
            true,
            &a.access,
            a.summary().as_ref().map_err(String::as_str),
        );
        assert_eq!(go, Decision::Run);
        let seen = std::cell::RefCell::new(Vec::new());
        let end = run_prepare_in(
            cache.clone(),
            &done.worktree,
            a.view.as_ref().unwrap(),
            false,
            &|s| seen.borrow_mut().push(s),
            &Cancel::new(),
        );
        match end {
            RunEnd::Done { ran, result, .. } => {
                assert!(ran);
                assert!(!result.is_failure(), "{result:?}");
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(seen.borrow().len(), 1);
        assert_eq!((seen.borrow()[0].step, seen.borrow()[0].of), (1, 1));
        assert!(done.worktree.join("made.txt").is_file());

        // A second run is covered: nothing runs. Cancelled before it starts: silent.
        let end = run_prepare_in(
            cache.clone(),
            &done.worktree,
            a.view.as_ref().unwrap(),
            false,
            &|_| {},
            &Cancel::new(),
        );
        assert!(matches!(end, RunEnd::Done { ran: false, .. }), "{end:?}");
        let cancel = Cancel::new();
        cancel.cancel();
        let end = run_prepare_in(
            cache,
            &done.worktree,
            a.view.as_ref().unwrap(),
            true,
            &|_| {},
            &cancel,
        );
        assert!(matches!(end, RunEnd::Cancelled), "{end:?}");
    }

    #[test]
    fn a_folder_outside_the_cache_is_not_this_features_business() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        let projects = Projects::open(dir.join("projects.json")).unwrap();
        let a = assess_in(Cache::at(dir.join("cache")), Ok(&projects), dir);
        assert_eq!(a.access, crate::remote::prepare::Access::Local);
        assert!(a.view.is_err());
    }
}
