//! `cho prepare`: the commands a remote worktree needs before indexing, their approval and
//! running them; and the prepare-then-index step after `cho open … --prepare`. See
//! `docs/public/sources.md` ("Prepare").

use crate::sources_cmd::{ago, open_target, repo_arg, sources};
use crate::{IndexArgs, Output};
use anyhow::{Result, anyhow, bail};
use cho_core::RepoRef;
use cho_sources::prepare::ops::{PrepareOutcome, PrepareView, partial_note};
use cho_sources::prepare::{ApprovalState, Recipe, Step};
use cho_sources::{Opened, Project, Projects};
use clap::Subcommand;
use serde_json::{Value, json};
use std::io::{IsTerminal, Write};
use std::path::PathBuf;

#[derive(Subcommand)]
pub enum PrepareCmd {
    /// The recipe that applies (and where it comes from), the approval state, the trigger
    /// files that changed, and the last run. TARGET: a branch, `#PR`, commit or URL (default:
    /// the default branch).
    Show {
        repo: String,
        target: Option<String>,
    },
    /// Approve the recipe shown by `show` for this project: covers prepare and indexing
    /// until the recipe or a trigger file changes.
    Approve {
        repo: String,
        target: Option<String>,
    },
    /// Set the project's own recipe: one step after `--`, or several from `--file`.
    Set {
        repo: String,
        /// A JSON recipe: `{"steps": [{"argv": [...], "cwd": ".", "timeout_s": 600}]}`.
        #[arg(long, conflicts_with = "argv")]
        file: Option<PathBuf>,
        /// Folder (in the repository) the step runs in.
        #[arg(long, default_value = ".")]
        cwd: String,
        /// Seconds before the step is stopped.
        #[arg(long, default_value_t = cho_sources::prepare::DEFAULT_STEP_TIMEOUT)]
        timeout: u64,
        /// The command, run with no shell (write `sh -c '…'` for one).
        #[arg(last = true)]
        argv: Vec<String>,
    },
    /// Run the approved recipe in TARGET's worktree (it is fetched and placed first).
    Run {
        repo: String,
        target: Option<String>,
        /// Run even if the last run covers this recipe, these files and this toolchain.
        #[arg(long)]
        force: bool,
    },
    /// Removed: Chorong no longer prepares or indexes on its own. Kept so that running it says
    /// so instead of "unrecognized subcommand".
    #[command(hide = true)]
    Auto {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        rest: Vec<String>,
    },
    /// Remove the project's recipe and approval.
    Clear { repo: String },
}

/// Opens TARGET of REPO (default branch when none).
fn open_for(repo: &str, target: Option<&str>) -> Result<(RepoRef, Opened, Project)> {
    let (r, url) = repo_arg(repo)?;
    let mut t = cho_sources::parse_target(target.unwrap_or(""))?;
    if t.repo.as_ref().is_some_and(|tr| *tr != r) {
        bail!("the target names another repository than {r}");
    }
    t.repo = None;
    let (opened, project) = open_target(&r, url, t, None)?;
    Ok((r, opened, project))
}

pub fn run(cmd: PrepareCmd) -> Result<Output> {
    match cmd {
        PrepareCmd::Show { repo, target } => {
            let (r, opened, project) = open_for(&repo, target.as_deref())?;
            let view = sources()?.prepare_view(&r, &project, &opened)?;
            Ok(Output::ok(view_text(&view), serde_json::to_value(&view)?))
        }
        PrepareCmd::Approve { repo, target } => {
            let (r, opened, project) = open_for(&repo, target.as_deref())?;
            let view = sources()?.prepare_view(&r, &project, &opened)?;
            save_approval(&r, &view)?;
            Ok(Output::ok(
                format!(
                    "{}approved prepare and indexing for {r} ({} step{}, {} trigger file{})",
                    recipe_text(&view),
                    view.effective.recipe.steps.len(),
                    plural(view.effective.recipe.steps.len()),
                    view.triggers.len(),
                    plural(view.triggers.len()),
                ),
                json!({ "repo": r.to_string(), "approved": true, "recipe_hash": view.recipe_hash, "triggers_hash": view.triggers_hash }),
            ))
        }
        PrepareCmd::Set {
            repo,
            file,
            cwd,
            timeout,
            argv,
        } => {
            let (r, _) = repo_arg(&repo)?;
            let recipe = match file {
                Some(f) => Recipe::parse(
                    &cho_core::fs::read_source_text(&f)
                        .map_err(|e| anyhow!("cannot read {}: {e}", f.display()))?,
                )?,
                None if argv.is_empty() => bail!("give the command after `--`, or --file"),
                None => Recipe {
                    steps: vec![Step {
                        argv,
                        cwd,
                        timeout_s: timeout,
                    }],
                },
            };
            recipe.validate()?;
            let mut projects = Projects::open_default()?;
            let mut p = projects.get(&r).cloned().unwrap_or_default();
            let approved_before = p.approval.is_some();
            p.recipe = Some(recipe.clone());
            p.approval = None;
            projects.insert(&r, p)?;
            let mut text = format!("set the prepare recipe for {r}:\n{}", steps_text(&recipe));
            if approved_before {
                text.push_str(
                    "the previous approval no longer applies; review and approve it again\n",
                );
            }
            Ok(Output::ok(
                text,
                json!({ "repo": r.to_string(), "recipe": recipe, "recipe_hash": recipe.hash() }),
            ))
        }
        PrepareCmd::Run {
            repo,
            target,
            force,
        } => {
            let (r, opened, project) = open_for(&repo, target.as_deref())?;
            let view = sources()?.prepare_view(&r, &project, &opened)?;
            let (text, json, _) = prepare_only(&r, &opened, &view, force)?;
            Ok(Output::ok(text, json))
        }
        PrepareCmd::Auto { .. } => bail!("{AUTO_REMOVED}"),
        PrepareCmd::Clear { repo } => {
            let (r, _) = repo_arg(&repo)?;
            let mut projects = Projects::open_default()?;
            let mut p = projects
                .get(&r)
                .cloned()
                .ok_or_else(|| anyhow!("{r} is not a project"))?;
            p.recipe = None;
            p.approval = None;
            projects.insert(&r, p)?;
            Ok(Output::ok(
                format!("cleared the prepare recipe and approval for {r}"),
                json!({ "repo": r.to_string(), "cleared": true }),
            ))
        }
    }
}

fn plural(n: usize) -> &'static str {
    if n == 1 { "" } else { "s" }
}

/// Stores the approval for `view` (shared with the viewer: `cho_sources`).
fn save_approval(r: &RepoRef, view: &PrepareView) -> Result<()> {
    Ok(cho_sources::prepare::ops::save_approval(r, view)?)
}

fn steps_text(r: &Recipe) -> String {
    if r.is_empty() {
        return "  (no steps)\n".into();
    }
    r.steps
        .iter()
        .enumerate()
        .map(|(i, s)| {
            format!(
                "  {}. {}   (timeout {}s)\n",
                i + 1,
                s.display(),
                s.timeout_s
            )
        })
        .collect()
}

fn recipe_text(v: &PrepareView) -> String {
    let from = v.effective.source.describe(&v.commit);
    let mut s = format!("Recipe ({from}):\n{}", steps_text(&v.effective.recipe));
    for n in &v.effective.notes {
        s.push_str(&format!("note: {n}\n"));
    }
    s
}

fn approval_text(v: &PrepareView, repo: &str) -> String {
    match &v.approval {
        ApprovalState::Approved => "Approval: approved (prepare and indexing)\n".into(),
        ApprovalState::NotApproved => format!(
            "Approval: not approved; nothing from the project runs (syntax only). Approve with `cho prepare approve {repo}`\n"
        ),
        ApprovalState::Changed { recipe, triggers } => {
            let mut s = "Approval: needed again, because ".to_string();
            let mut why = vec![];
            if *recipe {
                why.push("the recipe changed".to_string());
            }
            if !triggers.is_empty() {
                why.push(format!(
                    "trigger files changed: {}",
                    triggers
                        .iter()
                        .map(|t| format!("{} ({})", t.path, t.change))
                        .collect::<Vec<_>>()
                        .join(", ")
                ));
            }
            s.push_str(&why.join("; "));
            s.push_str(&format!(". Review and run `cho prepare approve {repo}`\n"));
            s
        }
    }
}

fn view_text(v: &PrepareView) -> String {
    let mut s = format!(
        "{} at {} ({})\n",
        v.repo,
        cho_git::short(&v.commit),
        v.worktree.display()
    );
    s.push_str(&recipe_text(v));
    s.push_str(&approval_text(v, &v.repo));
    s.push_str(&format!(
        "Trigger files: {}\n",
        if v.triggers.is_empty() {
            "none".to_string()
        } else {
            v.triggers.keys().cloned().collect::<Vec<_>>().join(", ")
        }
    ));
    if let Some(r) = &v.last_run {
        s.push_str(&format!(
            "Last run: {} {} ({}); log {}\n",
            r.result.as_str(),
            ago(r.finished),
            r.message.as_deref().unwrap_or("no errors"),
            r.log.display()
        ));
    }
    s
}

/// Runs prepare (if approved) and reports it.
fn prepare_only(
    r: &RepoRef,
    opened: &Opened,
    view: &PrepareView,
    force: bool,
) -> Result<(String, Value, PrepareOutcome)> {
    let env: Vec<_> = std::env::vars_os().collect();
    let out = sources()?.run_prepare(r, view, &opened.slug, &env, force)?;
    let rec = &out.record;
    let text = if !out.ran {
        format!(
            "prepare is up to date (last run {}: {}); --force runs it again\n",
            ago(rec.finished),
            rec.result.as_str()
        )
    } else {
        format!(
            "{} in {:.1}s{}; log {}\n",
            rec.result.describe(),
            rec.duration_ms as f64 / 1000.0,
            rec.message
                .as_deref()
                .map(|m| format!(": {m}"))
                .unwrap_or_default(),
            rec.log.display()
        )
    };
    let json = serde_json::to_value(&out)?;
    Ok((text, json, out))
}

/// What `cho prepare auto` says now.
const AUTO_REMOVED: &str = "`cho prepare auto` was removed: Chorong no longer prepares or indexes \
     a project on its own. Prepare and index when you ask: `cho open REPO --prepare`, \
     `cho prepare run REPO` then `cho index`, or the viewer's banner";

/// After `cho open`: prepare and index only when asked (`--prepare`). Needs an approval;
/// without one nothing runs and the text says what to approve.
pub fn after_open(
    r: &RepoRef,
    project: &Project,
    opened: &Opened,
    asked: bool,
) -> Result<Option<Output>> {
    if !asked {
        return Ok(None);
    }
    let mut view = sources()?.prepare_view(r, project, opened)?;
    let mut text = String::new();
    if !view.approval.is_approved() {
        let tty = std::io::stdin().is_terminal() && std::io::stderr().is_terminal();
        if tty {
            eprint!(
                "{}{}",
                recipe_text(&view),
                approval_text(&view, &r.to_string())
            );
            eprint!("Prepare and index {r} with this recipe? This runs the project's code. [y/N] ");
            let _ = std::io::stderr().flush();
            let mut answer = String::new();
            std::io::stdin().read_line(&mut answer)?;
            if matches!(answer.trim(), "y" | "Y" | "yes") {
                save_approval(r, &view)?;
                let project = Projects::open_default()?
                    .get(r)
                    .cloned()
                    .unwrap_or_default();
                view = sources()?.prepare_view(r, &project, opened)?;
            }
        }
        if !view.approval.is_approved() {
            text.push_str(&recipe_text(&view));
            text.push_str(&approval_text(&view, &r.to_string()));
            text.push_str("Nothing was prepared or indexed.\n");
            return Ok(Some(Output::ok(
                text,
                json!({ "approved": false, "view": view }),
            )));
        }
    }
    let (prep_text, prep_json, out) = prepare_only(r, opened, &view, false)?;
    text.push_str(&prep_text);
    let index = crate::index_cmd::index_build_with(
        &IndexArgs {
            root: opened.worktree.clone(),
            only: None,
            force: false,
            timeout: 900,
        },
        partial_note(&out.record),
    )?;
    text.push_str(&index.text);
    Ok(Some(Output::ok(
        text,
        json!({ "approved": true, "run": prep_json, "index": index.json }),
    )))
}

/// For `cho index` on a folder in Chorong's cache: the prepare note to record (a failed
/// prepare makes the index partial), or an error when the project's approval doesn't cover
/// the commit. Decided by `index_permission`, the same check the viewer uses. `None` for any
/// other folder, and when the last prepare didn't fail.
pub fn gate_cache_worktree(root: &std::path::Path) -> Result<Option<cho_semantic::PrepareNote>> {
    use cho_sources::Permission;
    match cho_sources::prepare::ops::assess(root) {
        (Permission::Local, _) => Ok(None),
        (Permission::CacheNeedsApproval { reason, .. }, _) => bail!(
            "{} is Chorong's worktree of a remote project; approve it first: {reason}",
            root.display()
        ),
        (Permission::CacheApproved { .. }, view) => Ok(view
            .ok()
            .and_then(|v| v.last_run)
            .as_ref()
            .and_then(partial_note)),
    }
}
