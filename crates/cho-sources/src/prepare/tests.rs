#![allow(clippy::disallowed_methods)] // tests read logs and files they wrote
use super::run::{MAX_LOG_BYTES, PrepareResult, RunSpec, environment, is_secret_var, run_recipe};
use super::*;
use crate::testutil::{commit, git};
use std::ffi::OsString;

fn repo_with(files: &[(&str, &str)]) -> (tempfile::TempDir, String) {
    let dir = tempfile::tempdir().unwrap();
    git(dir.path(), &["init", "-q", "-b", "main"]);
    let c = commit(dir.path(), files, "first");
    (dir, c)
}

fn sh(script: &str) -> Step {
    Step::new(&["sh", "-c", script])
}

#[test]
fn recipes_parse_validate_and_hash() {
    let r = Recipe::parse(r#"{"steps":[{"argv":["pnpm","install"]},{"argv":["pnpm","build"],"cwd":"web","timeout_s":60}]}"#).unwrap();
    assert_eq!(r.steps[0].cwd, ".");
    assert_eq!(r.steps[0].timeout_s, DEFAULT_STEP_TIMEOUT);
    assert_eq!(r.steps[1].display(), "(in web) pnpm build");
    assert_eq!(sh("echo hi").display(), "sh -c 'echo hi'");
    assert_eq!(r.hash(), r.clone().hash());
    assert_eq!(r.hash().len(), 64);
    let mut r2 = r.clone();
    r2.steps[1].timeout_s = 61;
    assert_ne!(r.hash(), r2.hash());
    for bad in [
        r#"{"steps":[{"argv":[]}]}"#,
        r#"{"steps":[{"argv":["x"],"cwd":"../up"}]}"#,
        r#"{"steps":[{"argv":["x"],"cwd":"/abs"}]}"#,
        r#"{"steps":[{"argv":["x"],"timeout_s":0}]}"#,
        r#"{"steps":"nope"}"#,
        r#"{"steps":[{"argv":["x"],"cwd":"a\u001b[2Kb"}]}"#,
    ] {
        assert!(Recipe::parse(bad).is_err(), "{bad}");
    }
}

/// Found by the `prepare_recipe` fuzz target: a recipe argument with a terminal escape,
/// a newline or a right-to-left override was shown raw in the approval prompt, which could
/// then display a different command than the one that runs.
#[test]
fn the_displayed_command_hides_nothing() {
    let step = Step::new(&["npm", "ci\u{1b}[2K\rnpm ci", "a\nb", "x\u{202e}y", "it's\\"]);
    assert_eq!(
        step.display(),
        r"npm $'ci\x1b[2K\x0dnpm ci' $'a\x0ab' $'x\u202ey' 'it'\''s\'"
    );
    assert!(!step.display().contains(|c: char| c.is_control()));
}

#[test]
fn lockfile_suggestions() {
    let (r, note) = suggestion(&["package.json", "pnpm-lock.yaml"]).unwrap();
    assert_eq!(r.steps[0].argv, ["pnpm", "install", "--frozen-lockfile"]);
    assert!(note.is_none());
    let (r, note) = suggestion(&["package-lock.json", "yarn.lock"]).unwrap();
    assert_eq!(r.steps[0].argv, ["yarn", "install", "--immutable"]);
    assert!(note.unwrap().contains("Several lockfiles"));
    assert_eq!(
        suggestion(&["package-lock.json"]).unwrap().0.steps[0].argv,
        ["npm", "ci"]
    );
    assert!(suggestion(&["Cargo.toml"]).is_none());
    // uv: its own step, after the JavaScript install when there is one.
    let (r, note) = suggestion(&["pyproject.toml", "uv.lock"]).unwrap();
    assert_eq!(r.steps.len(), 1);
    assert_eq!(r.steps[0].argv, ["uv", "sync", "--frozen"]);
    assert!(note.is_none());
    let (r, _) = suggestion(&["pnpm-lock.yaml", "uv.lock"]).unwrap();
    let argv: Vec<_> = r.steps.iter().map(|s| s.argv[0].as_str()).collect();
    assert_eq!(argv, ["pnpm", "uv"]);
    assert!(suggestion(&["pyproject.toml", "requirements.txt"]).is_none());
}

#[test]
fn where_the_recipe_comes_from() {
    let g = crate::Git::default();
    // Lockfile only: suggested.
    let (d, c) = repo_with(&[("package.json", "{}"), ("pnpm-lock.yaml", "x")]);
    let e = effective(&g, d.path(), &c, None).unwrap();
    assert_eq!(e.source, RecipeSource::Suggested);
    // The repository's own file wins over the lockfile.
    let c2 = commit(
        d.path(),
        &[(PREPARE_FILE, r#"{"steps":[{"argv":["make","deps"]}]}"#)],
        "recipe",
    );
    let e = effective(&g, d.path(), &c2, None).unwrap();
    assert_eq!(
        (e.source, e.recipe.steps[0].argv[0].as_str()),
        (RecipeSource::Repository, "make")
    );
    // ... read from the commit, not the checkout.
    let e = effective(&g, d.path(), &c, None).unwrap();
    assert_eq!(e.source, RecipeSource::Suggested);
    // The project's recipe wins over both.
    let mine = Recipe {
        steps: vec![sh("true")],
    };
    let e = effective(&g, d.path(), &c2, Some(&mine)).unwrap();
    assert_eq!(e.source, RecipeSource::Project);
    // An invalid repository file is ignored with a note.
    let c3 = commit(d.path(), &[(PREPARE_FILE, "{oops")], "bad");
    let e = effective(&g, d.path(), &c3, None).unwrap();
    assert_eq!(e.source, RecipeSource::Suggested);
    assert!(e.notes[0].contains("is ignored"), "{:?}", e.notes);
    // So is a folder of that name.
    let (d, c) = repo_with(&[(&format!("{PREPARE_FILE}/x"), "x"), ("pnpm-lock.yaml", "x")]);
    let e = effective(&g, d.path(), &c, None).unwrap();
    assert_eq!(e.source, RecipeSource::Suggested);
    assert!(e.notes[0].contains("is not a file"), "{:?}", e.notes);
    // Nothing at all.
    let (d, c) = repo_with(&[("README", "x")]);
    let e = effective(&g, d.path(), &c, None).unwrap();
    assert_eq!((e.source, e.recipe.is_empty()), (RecipeSource::None, true));
}

#[test]
fn trigger_files() {
    let g = crate::Git::default();
    let (d, c) = repo_with(&[
        ("Cargo.toml", "[workspace]\nmembers = [\"m\", \"n\"]\n"),
        ("Cargo.lock", "x"),
        (
            "m/Cargo.toml",
            "[package]\nname = \"m\"\n[lib]\nproc-macro = true\n",
        ),
        ("n/Cargo.toml", "[package]\nname = \"n\"\n"),
        ("n/build.rs", "fn main() {}"),
        ("web/package.json", "{}"),
        ("web/src/a.ts", "x"),
        ("README", "x"),
    ]);
    let t = triggers(&g, d.path(), &c).unwrap();
    assert_eq!(
        t.keys().cloned().collect::<Vec<_>>(),
        [
            "Cargo.lock",
            "m/Cargo.toml",
            "n/build.rs",
            "web/package.json"
        ]
    );
    // The hash changes with a trigger's contents or its path.
    let mut other = t.clone();
    other.insert("Cargo.lock".into(), "0".repeat(40));
    assert_ne!(triggers_hash(&t), triggers_hash(&other));
    let mut moved = t.clone();
    let id = moved.remove("n/build.rs").unwrap();
    moved.insert("n/build2.rs".into(), id);
    assert_ne!(triggers_hash(&t), triggers_hash(&moved));
}

#[test]
fn approval_is_asked_again_when_a_trigger_changes() {
    let g = crate::Git::default();
    let (d, c) = repo_with(&[
        ("package.json", "{}"),
        ("pnpm-lock.yaml", "a"),
        ("src/x.ts", "x"),
    ]);
    let e = effective(&g, d.path(), &c, None).unwrap();
    let t = triggers(&g, d.path(), &c).unwrap();
    assert_eq!(check(None, &e, &t), ApprovalState::NotApproved);
    let a = approve(&e, &t, &c);
    assert_eq!(check(Some(&a), &e, &t), ApprovalState::Approved);

    // A source change that touches no trigger file keeps the approval.
    let c2 = commit(d.path(), &[("src/x.ts", "y")], "code");
    let t2 = triggers(&g, d.path(), &c2).unwrap();
    assert!(check(Some(&a), &e, &t2).is_approved());

    // A lockfile change and a new build.rs: asked again, naming exactly those files.
    let c3 = commit(
        d.path(),
        &[("pnpm-lock.yaml", "b"), ("crate/build.rs", "fn main(){}")],
        "deps",
    );
    let t3 = triggers(&g, d.path(), &c3).unwrap();
    assert_eq!(
        check(Some(&a), &e, &t3),
        ApprovalState::Changed {
            recipe: false,
            triggers: vec![
                TriggerChange {
                    path: "crate/build.rs".into(),
                    change: Change::Added
                },
                TriggerChange {
                    path: "pnpm-lock.yaml".into(),
                    change: Change::Changed
                },
            ]
        }
    );

    // A different recipe: asked again.
    let other = Effective {
        recipe: Recipe {
            steps: vec![sh("true")],
        },
        source: RecipeSource::Project,
        notes: vec![],
    };
    assert!(matches!(
        check(Some(&a), &other, &t),
        ApprovalState::Changed { recipe: true, .. }
    ));
}

fn run_in(
    dir: &std::path::Path,
    recipe: Recipe,
    base: &[(OsString, OsString)],
) -> (super::run::StepsOutcome, String) {
    let log = dir.join("logs").join("x.log");
    let out = run_recipe(&RunSpec {
        worktree: dir,
        recipe: &recipe,
        env: environment(base, &dir.join("target")),
        log: &log,
        progress: &|_| {},
        cancel: &crate::Cancel::new(),
    })
    .unwrap();
    (out, std::fs::read_to_string(&log).unwrap())
}

fn path_env() -> Vec<(OsString, OsString)> {
    vec![("PATH".into(), std::env::var_os("PATH").unwrap())]
}

#[test]
fn tokens_are_removed_and_cargo_target_is_set() {
    for (k, secret) in [
        ("CHORONG_GITHUB_TOKEN", true),
        ("CHORONG_BITBUCKET_EMAIL", true),
        ("GITHUB_TOKEN", true),
        ("GH_TOKEN", true),
        ("BITBUCKET_APP_PASSWORD", true),
        ("CHORONG_CACHE_DIR", false),
        ("NPM_TOKEN", false),
        ("PATH", false),
    ] {
        assert_eq!(is_secret_var(k), secret, "{k}");
    }
    let dir = tempfile::tempdir().unwrap();
    let mut env = path_env();
    for k in ["GITHUB_TOKEN", "CHORONG_GITHUB_TOKEN", "BITBUCKET_TOKEN"] {
        env.push((k.into(), "s3cr3t".into()));
    }
    env.push(("LANG".into(), "C.UTF-8".into()));
    env.push(("AWS_SECRET_ACCESS_KEY".into(), "s3cr3t".into()));
    let (out, log) = run_in(
        dir.path(),
        Recipe {
            steps: vec![sh("env")],
        },
        &env,
    );
    assert_eq!(out.result, PrepareResult::Ok);
    assert!(!log.contains("s3cr3t"), "{log}");
    assert!(log.contains("LANG=C.UTF-8"));
    assert!(log.contains(&format!(
        "CARGO_TARGET_DIR={}",
        dir.path().join("target").display()
    )));
}

#[test]
fn steps_run_in_order_and_stop_at_a_failure() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join("sub")).unwrap();
    let mut second = sh("pwd; exit 3");
    second.cwd = "sub".into();
    let (out, log) = run_in(
        dir.path(),
        Recipe {
            steps: vec![sh("echo one"), second, sh("echo never")],
        },
        &path_env(),
    );
    assert_eq!(out.result, PrepareResult::Failed);
    assert!(out.message.unwrap().contains("step 2 failed (exit 3)"));
    assert!(
        log.contains("one") && log.contains("/sub") && !log.contains("never\n"),
        "{log}"
    );
}

#[test]
fn a_timeout_stops_the_whole_step() {
    let dir = tempfile::tempdir().unwrap();
    let mut s = sh("sleep 30 & sleep 30; wait");
    s.timeout_s = 1;
    let start = std::time::Instant::now();
    let (out, _) = run_in(dir.path(), Recipe { steps: vec![s] }, &path_env());
    assert_eq!(out.result, PrepareResult::TimedOut);
    assert!(start.elapsed() < std::time::Duration::from_secs(10));
    assert!(out.message.unwrap().contains("timed out after 1s"));
}

#[test]
fn a_missing_tool_runs_nothing_and_says_which() {
    let dir = tempfile::tempdir().unwrap();
    let (out, log) = run_in(
        dir.path(),
        Recipe {
            steps: vec![
                sh("echo first > ran"),
                Step::new(&["cho-no-such-tool", "install"]),
            ],
        },
        &path_env(),
    );
    assert_eq!(out.result, PrepareResult::MissingTool);
    let msg = out.message.unwrap();
    assert!(
        msg.contains("`cho-no-such-tool` is not installed") && msg.contains("step 2"),
        "{msg}"
    );
    assert!(log.contains("not installed"));
    assert!(!dir.path().join("ran").exists());
}

/// A step's folder must resolve inside the worktree. Each case names a folder that exists
/// (so it is refused for being outside, not for being missing), and the step must not run.
#[test]
fn a_step_folder_outside_the_worktree_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let wt = dir.path().join("wt");
    let outside = dir.path().join("outside");
    std::fs::create_dir_all(wt.join("sub")).unwrap();
    std::fs::create_dir_all(&outside).unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(&outside, wt.join("link")).unwrap();
    let run = |cwd: &str| {
        let mut s = sh("touch ran");
        s.cwd = cwd.into();
        run_recipe(&RunSpec {
            worktree: &wt,
            recipe: &Recipe { steps: vec![s] },
            env: path_env(),
            log: &dir.path().join("x.log"),
            progress: &|_| {},
            cancel: &crate::Cancel::new(),
        })
    };
    let mut cases = vec!["nope", "..", "../outside", "sub/../../outside"];
    if cfg!(unix) {
        cases.push("link");
    }
    for cwd in cases {
        assert!(run(cwd).is_err(), "{cwd}");
        assert!(!outside.join("ran").exists(), "{cwd} ran outside");
        assert!(!dir.path().join("ran").exists(), "{cwd} ran outside");
    }
    // The control: a folder inside runs.
    run("sub").unwrap();
    assert!(wt.join("sub/ran").exists());
}

#[test]
fn steps_report_progress_and_cancel_stops_the_step() {
    let dir = tempfile::tempdir().unwrap();
    let seen = std::sync::Mutex::new(vec![]);
    let cancel = crate::Cancel::new();
    let c2 = cancel.clone();
    let stopper = std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(400));
        c2.cancel();
    });
    let log = dir.path().join("x.log");
    let start = std::time::Instant::now();
    let r = run_recipe(&RunSpec {
        worktree: dir.path(),
        recipe: &Recipe {
            steps: vec![
                sh("true"),
                sh("sleep 30 & sleep 30; wait"),
                sh("echo never > ran"),
            ],
        },
        env: environment(&path_env(), &dir.path().join("target")),
        log: &log,
        progress: &|p| seen.lock().unwrap().push((p.step, p.of)),
        cancel: &cancel,
    });
    stopper.join().unwrap();
    assert_eq!(r.unwrap_err().kind, crate::ErrorKind::Cancelled);
    assert!(start.elapsed() < std::time::Duration::from_secs(10));
    assert_eq!(*seen.lock().unwrap(), vec![(1, 3), (2, 3)]);
    assert!(!dir.path().join("ran").exists());
}

/// Cancelling a step that floods its output stops it and its children promptly, and leaves
/// the log holding what was written before (output goes straight to the log file, so a full
/// pipe cannot block the step or the cancel).
#[cfg(unix)]
#[test]
fn cancel_stops_a_step_that_floods_its_output() {
    let dir = tempfile::tempdir().unwrap();
    let pids = dir.path().join("pids");
    let cancel = crate::Cancel::new();
    let c2 = cancel.clone();
    let log = dir.path().join("x.log");
    let step = sh(&format!(
        // Shell loops, not `yes`: heavy output, but well under the log limit before the cancel.
        "sh -c 'echo $$ > {p}; while :; do echo flood-out; done' & \
         while :; do echo flood-err; done >&2",
        p = pids.display()
    ));
    // Cancel once the background child has written its pid and the step's output has reached
    // the log (polled, not a fixed wait, so a slow runner doesn't flake).
    let (pids2, log2) = (pids.clone(), log.clone());
    let stopper = std::thread::spawn(move || {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        let ready = || {
            std::fs::read_to_string(&pids2).is_ok_and(|p| p.ends_with('\n'))
                && std::fs::read_to_string(&log2).is_ok_and(|l| l.contains("flood-err"))
        };
        while !ready() && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        c2.cancel();
        std::time::Instant::now()
    });
    let r = run_recipe(&RunSpec {
        worktree: dir.path(),
        recipe: &Recipe { steps: vec![step] },
        env: environment(&path_env(), &dir.path().join("target")),
        log: &log,
        progress: &|_| {},
        cancel: &cancel,
    });
    let cancelled_at = stopper.join().unwrap();
    assert_eq!(r.unwrap_err().kind, crate::ErrorKind::Cancelled);
    assert!(cancelled_at.elapsed() < std::time::Duration::from_secs(5));
    let text = std::fs::read_to_string(&log).unwrap();
    assert!(text.contains("flood-err") && text.contains("cancelled during step 1"));
    let pid = std::fs::read_to_string(&pids).unwrap();
    // Gone (or a zombie awaiting its parent) within a deadline.
    let alive = || {
        std::fs::read_to_string(format!("/proc/{}/stat", pid.trim())).is_ok_and(|s| {
            !s.rsplit(')')
                .next()
                .unwrap_or("")
                .trim_start()
                .starts_with('Z')
        })
    };
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while alive() && std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    assert!(!alive(), "the step's background child survived the cancel");
}

/// A step that prints without end is stopped once the log passes `MAX_LOG_BYTES`, long
/// before its timeout, so a recipe can't fill the disk.
#[cfg(unix)]
#[test]
fn endless_output_is_stopped_at_the_log_limit() {
    let dir = tempfile::tempdir().unwrap();
    let log = dir.path().join("x.log");
    let start = std::time::Instant::now();
    let r = run_recipe(&RunSpec {
        worktree: dir.path(),
        recipe: &Recipe {
            steps: vec![sh("exec yes endless-output-endless-output")],
        },
        env: environment(&path_env(), &dir.path().join("target")),
        log: &log,
        progress: &|_| {},
        cancel: &crate::Cancel::new(),
    })
    .unwrap();
    assert_eq!(r.result, PrepareResult::Failed, "{r:?}");
    assert!(start.elapsed() < std::time::Duration::from_secs(60));
    let len = std::fs::metadata(&log).unwrap().len();
    assert!(len > MAX_LOG_BYTES && len < 4 * MAX_LOG_BYTES, "{len}");
}
