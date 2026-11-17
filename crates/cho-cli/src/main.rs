//! `cho`: the headless command-line interface.
//!
//! Every command can emit JSON (`--json`) so agents and scripts can drive Chorong.
//! It never needs a display or GPU; `cho open` hands off to the `chorong-app`
//! viewer. See `docs/public/cli.md`.

mod changes_cmd;
mod editor_cmd;
mod file_cmd;
mod formats_cmd;
mod graph_cmd;
mod index_cmd;
mod investigation_cmd;
mod link_cmd;
mod nav_cmd;
mod open_cmd;
mod prepare_cmd;
mod search_cmd;
mod settings_cmd;
mod skill;
mod sources_cmd;

use anyhow::{Context, Result, anyhow};
use cho_core::SourceBuffer;
use cho_semantic::{IndexCache, Semantic};
use cho_syntax::Language;
use clap::{Args, Parser, Subcommand};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

#[derive(Parser)]
#[command(
    name = "cho",
    version,
    about = "Chorong, a read-only code viewer. Headless commands for people and agents."
)]
struct Cli {
    /// Print machine-readable JSON instead of text.
    #[arg(long, global = true)]
    json: bool,
    /// On an error, also print its full detail (the git command, its output, paths).
    #[arg(long, short = 'v', global = true)]
    verbose: bool,
    #[command(subcommand)]
    command: Cmd,
}

#[derive(Subcommand)]
enum SettingsCmd {
    /// Print the effective value of one setting.
    Get { key: String },
    /// Set a setting in `settings.json`, or in this repository's override with `--repo`.
    Set {
        key: String,
        #[arg(allow_hyphen_values = true)]
        value: String,
    },
    /// Remove a setting from `settings.json` (or the repository override), so the next layer applies.
    Unset { key: String },
}

#[derive(Subcommand)]
enum Cmd {
    /// List the declarations in a file, with qualified names and locations.
    Outline { file: PathBuf },
    /// Find a declaration by (optionally qualified) name: `Queue::push`, `Svc.run`.
    Find { file: PathBuf, symbol: String },
    /// Print a file, or part of it, with line numbers.
    Show {
        file: PathBuf,
        /// One-based inclusive line range, `START:END`, `START:` or `:END`.
        #[arg(long)]
        lines: Option<String>,
    },
    /// Print syntax highlight spans (byte ranges and theme keys).
    Highlight { file: PathBuf },
    /// Work with investigation files (call stacks, tours, diagrams, PR overlays).
    #[command(subcommand)]
    Investigation(investigation_cmd::InvCmd),
    /// Parse or build `chorong://` links.
    #[command(subcommand)]
    Link(LinkCmd),
    /// Open a location in another editor.
    #[command(subcommand)]
    Editor(EditorCmd),
    /// Show or change Chorong's own settings (never a repository's files).
    Settings {
        #[command(subcommand)]
        action: Option<SettingsCmd>,
        /// Resolve for (and, with `set`/`unset`, write the override of) this repository.
        #[arg(long, global = true, value_name = "DIR")]
        repo: Option<PathBuf>,
    },
    /// Print the web URL of a file on GitHub or Bitbucket.
    WebUrl {
        /// Repository, e.g. `github:owner/name`.
        #[arg(long)]
        repo: String,
        #[arg(long)]
        rev: String,
        #[arg(long)]
        path: String,
        #[arg(long)]
        line: Option<usize>,
    },
    /// Validate a Zed theme file and list the themes in it.
    Theme { file: PathBuf },
    /// List the file formats agents can write for Chorong.
    Formats,
    /// Print the JSON Schema for a format (`investigation`).
    Schema { format: String },
    /// Print an agent skill that teaches how to write a format, or install it.
    Skill {
        format: String,
        /// Write `<DIR>/chorong-investigations/SKILL.md` (e.g. `.claude/skills` or `.cursor/skills`).
        #[arg(long)]
        install: Option<PathBuf>,
        /// Overwrite an existing skill file.
        #[arg(long)]
        force: bool,
    },
    /// Build or refresh the semantic (SCIP) index, or show its status.
    Index {
        #[command(subcommand)]
        sub: Option<IndexSub>,
        #[command(flatten)]
        args: IndexArgs,
    },
    /// Go to the definition of the symbol at FILE LINE:COL (one-based, column in UTF-8 bytes).
    Definition {
        file: PathBuf,
        position: String,
        #[arg(long, default_value = ".")]
        root: PathBuf,
    },
    /// List every reference to the symbol at FILE LINE:COL, definitions first.
    References {
        file: PathBuf,
        position: String,
        #[arg(long, default_value = ".")]
        root: PathBuf,
    },
    /// What the index records about the symbol at FILE LINE:COL: signature, docs, definition.
    Hover {
        file: PathBuf,
        position: String,
        #[arg(long, default_value = ".")]
        root: PathBuf,
        /// Also give each definition's full declaration (byte range, lines, and its text).
        #[arg(long)]
        peek: bool,
    },
    /// Search symbol definitions by display name (case-insensitive substring).
    Symbols {
        query: String,
        #[arg(long, default_value = ".")]
        root: PathBuf,
        /// Maximum number of results.
        #[arg(long, default_value_t = 50)]
        limit: usize,
    },
    /// Search file contents (ripgrep's engine; respects .gitignore). Exit 1 if nothing matches.
    Search {
        pattern: String,
        #[arg(long, default_value = ".")]
        root: PathBuf,
        /// Treat PATTERN as a regular expression instead of a literal string.
        #[arg(long)]
        regex: bool,
        /// Match case exactly (default: ignore case).
        #[arg(long)]
        case_sensitive: bool,
        /// Include files matching GLOB; prefix with `!` to exclude. Repeatable.
        #[arg(long = "glob", value_name = "GLOB")]
        globs: Vec<String>,
        /// Maximum number of matching lines.
        #[arg(long, default_value_t = cho_search::DEFAULT_MAX_RESULTS)]
        limit: usize,
    },
    /// Changed files and declarations between two commits (`--base REV` or `--merge-base TARGET`).
    Changes(changes_cmd::ChangesArgs),
    /// Reference graph: who uses a symbol and what it uses (needs `cho index`). `--json`, `--dot` or `--html OUT`.
    Graph(graph_cmd::GraphArgs),
    /// Open a file, folder, `chorong://` link or investigation in the Chorong viewer; or a remote
    /// repository (`REPO --branch X | --pr N | --rev SHA`, or a URL): fetch it into Chorong's
    /// cache and print the worktree, Before and After.
    Open {
        /// One or more: several files open in a focus layout, the first on the page and the
        /// rest in the tray. `-` reads paths from standard input, one per line
        /// (`git diff --name-only | cho -`).
        #[arg(required = true, num_args = 1..)]
        targets: Vec<String>,
        /// A name for the window's title, to tell it apart from other Chorong windows (always a
        /// new window).
        #[arg(long)]
        name: Option<String>,
        /// Open a local folder or file comparing with this branch, tag or commit: Before is
        /// the merge base of HEAD and it, as the viewer's Compare Against… (always a new
        /// window; `--view` is accepted and changes nothing).
        #[arg(long, value_name = "REF")]
        compare: Option<String>,
        /// Print the command instead of running it.
        #[arg(long)]
        dry_run: bool,
        #[command(flatten)]
        remote: sources_cmd::OpenRemote,
    },
    /// Remote repositories Chorong remembers: list, add, remove.
    #[command(subcommand)]
    Project(sources_cmd::ProjectCmd),
    /// A pull request's details from the host (number, title, author, state, branches, head).
    Pr { repo: String, number: u64 },
    /// Host tokens in the OS keychain: add, status, remove.
    #[command(subcommand)]
    Auth(sources_cmd::AuthCmd),
    /// The repository cache: disk use, and clearing it.
    #[command(subcommand)]
    Cache(sources_cmd::CacheCmd),
    /// Prepare remote worktrees before indexing: show, approve, set, run, auto, clear.
    #[command(subcommand)]
    Prepare(prepare_cmd::PrepareCmd),
    /// Git credential helper (git runs this; not for people).
    #[command(hide = true)]
    Credential { operation: String },
}

#[derive(Args)]
struct IndexArgs {
    /// Repository root.
    #[arg(long, default_value = ".")]
    root: PathBuf,
    /// Only index this language: rust, typescript or python.
    #[arg(long)]
    only: Option<String>,
    /// Rebuild even if the index is current.
    #[arg(long)]
    force: bool,
    /// Seconds before an indexer is stopped.
    #[arg(long, default_value_t = 900)]
    timeout: u64,
}

#[derive(Subcommand)]
enum IndexSub {
    /// Show what is indexed and how fresh it is.
    Status {
        #[arg(long, default_value = ".")]
        root: PathBuf,
    },
}

#[derive(Subcommand)]
enum LinkCmd {
    /// Parse a `chorong://` link and print its parts.
    Parse { url: String },
    /// Build a `chorong://open` link.
    Build(BuildLink),
}

#[derive(Args)]
struct BuildLink {
    #[arg(long)]
    repo: String,
    #[arg(long)]
    rev: Option<String>,
    #[arg(long)]
    path: Option<String>,
    #[arg(long)]
    symbol: Option<String>,
    #[arg(long)]
    line: Option<usize>,
    /// Branch checked out when the link was made.
    #[arg(long)]
    branch: Option<String>,
    /// The working tree had uncommitted changes.
    #[arg(long)]
    dirty: bool,
}

#[derive(Subcommand)]
enum EditorCmd {
    /// List the editors Chorong knows and whether each is installed.
    List,
    /// Open a file (and optional position) in an editor.
    Open {
        file: PathBuf,
        #[arg(long)]
        line: Option<usize>,
        #[arg(long)]
        column: Option<usize>,
        /// Program name: zed, code, cursor, subl, xed, rustrover, webstorm, pycharm, idea.
        #[arg(long, default_value = "zed")]
        editor: String,
        /// Also open this project folder (for editors that take one).
        #[arg(long)]
        project: Option<PathBuf>,
        /// Print the command instead of running it.
        #[arg(long)]
        dry_run: bool,
    },
}

/// Text and JSON forms of a command's result, plus whether it succeeded.
struct Output {
    text: String,
    json: Value,
    ok: bool,
}

impl Output {
    fn ok(text: impl Into<String>, json: Value) -> Self {
        Self {
            text: text.into(),
            json,
            ok: true,
        }
    }
}

/// `cho PATH` is shorthand for `cho open PATH`. If the first word that is not a flag is not a
/// subcommand but names something that exists, `open` is inserted before it. Subcommand names
/// always win, so a folder called `search` is reached as `cho ./search`.
fn expand_shorthand(
    mut args: Vec<std::ffi::OsString>,
    is_subcommand: impl Fn(&str) -> bool,
    exists: impl Fn(&std::ffi::OsStr) -> bool,
) -> Vec<std::ffi::OsString> {
    // `--name NAME` belongs to `open`: its value is not the path, and `open` goes before it.
    let mut after_name = false;
    let mut name_at = None;
    let first = args.iter().enumerate().skip(1).find(|(i, a)| {
        let skip = std::mem::replace(&mut after_name, *a == "--name");
        if *a == "--name" && name_at.is_none() {
            name_at = Some(*i);
        }
        !skip && (*a == "-" || !a.to_string_lossy().starts_with('-'))
    });
    if let Some((i, word)) = first
        && !word.to_str().is_some_and(is_subcommand)
        && exists(word)
    {
        args.insert(name_at.unwrap_or(i), "open".into());
    }
    args
}

fn main() -> ExitCode {
    let args = expand_shorthand(
        std::env::args_os().collect(),
        |w| {
            use clap::CommandFactory;
            let cmd = Cli::command();
            w == "help" || cmd.find_subcommand(w).is_some()
        },
        // `-` reads paths from standard input (`git diff --name-only | cho -`).
        |w| w == "-" || std::path::Path::new(w).exists(),
    );
    let cli = Cli::parse_from(args);
    match run(cli.command) {
        Ok(out) => {
            if cli.json {
                println!("{}", serde_json::to_string_pretty(&out.json).expect("json"));
            } else if !out.text.is_empty() {
                println!("{}", out.text.trim_end());
            }
            if out.ok {
                ExitCode::SUCCESS
            } else {
                ExitCode::from(1)
            }
        }
        Err(e) => {
            // A remote-sources error has a short message, a kind and the full detail.
            let src = e.downcast_ref::<cho_sources::Error>();
            let detail = src.and_then(|s| s.detail.clone());
            if cli.json {
                let mut j = json!({ "error": format!("{e:#}") });
                if let Some(s) = src {
                    j["kind"] = json!(error_kind(&s.kind));
                    j["detail"] = json!(detail);
                }
                println!("{j}");
            } else {
                eprintln!("cho: {e:#}");
                match (&detail, cli.verbose) {
                    (Some(d), true) => eprintln!("{d}"),
                    (Some(_), false) => eprintln!("(run with --verbose for details)"),
                    _ => {}
                }
            }
            ExitCode::from(2)
        }
    }
}

/// A remote-sources error kind as a word for JSON (`offline`, `auth`, …).
fn error_kind(k: &cho_sources::ErrorKind) -> &'static str {
    use cho_sources::ErrorKind as K;
    match k {
        K::Offline => "offline",
        K::Auth { .. } => "auth",
        K::NotFound | K::CommitNotFound { .. } => "not-found",
        K::Ambiguous { .. } => "ambiguous",
        K::LockBusy => "lock-busy",
        K::Unsupported => "unsupported",
        K::Cancelled => "cancelled",
        K::Other => "other",
    }
}

fn read_source(file: &Path) -> Result<(SourceBuffer, Option<Language>)> {
    let bytes = cho_core::fs::read_source(file)
        .with_context(|| format!("cannot read {}", file.display()))?;
    let buffer = SourceBuffer::from_bytes(bytes).with_context(|| file.display().to_string())?;
    Ok((buffer, Language::from_path(file)))
}

/// Reads repository file `rel` with the confined reader (`read_source_in`): it is resolved
/// again at read time and refused if it now leaves `root`, so a path swapped for a symbolic link
/// after the caller's checks can't read outside the repository. Errors name `shown`, as
/// [`read_source`]'s do.
fn read_source_under(root: &Path, rel: &Path, shown: &Path) -> Result<SourceBuffer> {
    let bytes = cho_core::fs::read_source_in(root, rel)
        .with_context(|| format!("cannot read {}", shown.display()))?;
    SourceBuffer::from_bytes(bytes).with_context(|| shown.display().to_string())
}

fn run(cmd: Cmd) -> Result<Output> {
    match cmd {
        Cmd::Outline { file } => file_cmd::outline_cmd(file),
        Cmd::Find { file, symbol } => file_cmd::find(file, symbol),
        Cmd::Show { file, lines } => file_cmd::show(file, lines),
        Cmd::Highlight { file } => file_cmd::highlight(file),
        Cmd::Investigation(cmd) => investigation_cmd::run(cmd),
        Cmd::Link(LinkCmd::Parse { url }) => link_cmd::parse(url),
        Cmd::Link(LinkCmd::Build(b)) => link_cmd::build(b),
        Cmd::Editor(EditorCmd::List) => editor_cmd::list(),
        Cmd::Editor(EditorCmd::Open {
            file,
            line,
            column,
            editor,
            project,
            dry_run,
        }) => editor_cmd::open(file, line, column, editor, project, dry_run),
        Cmd::WebUrl {
            repo,
            rev,
            path,
            line,
        } => link_cmd::web(repo, rev, path, line),
        Cmd::Theme { file } => formats_cmd::theme(file),
        Cmd::Formats => formats_cmd::formats(),
        Cmd::Schema { format } => formats_cmd::schema(format),
        Cmd::Skill {
            format,
            install,
            force,
        } => formats_cmd::skill_cmd(format, install, force),
        Cmd::Index {
            sub: Some(IndexSub::Status { root }),
            ..
        } => index_cmd::index_status(&root),
        Cmd::Index { sub: None, args } => index_cmd::index_build(&args),
        Cmd::Definition {
            file,
            position,
            root,
        } => nav_cmd::navigate(&file, &position, &root, nav_cmd::Nav::Definitions),
        Cmd::References {
            file,
            position,
            root,
        } => nav_cmd::navigate(&file, &position, &root, nav_cmd::Nav::References),
        Cmd::Hover {
            file,
            position,
            root,
            peek,
        } => nav_cmd::hover(&file, &position, &root, peek),
        Cmd::Symbols { query, root, limit } => nav_cmd::symbols(query, root, limit),
        Cmd::Settings { action, repo } => settings_cmd::run(action, repo),
        Cmd::Search {
            pattern,
            root,
            regex,
            case_sensitive,
            globs,
            limit,
        } => search_cmd::run(search_cmd::SearchArgs {
            pattern,
            root,
            regex,
            case_sensitive,
            globs,
            limit,
        }),
        Cmd::Changes(args) => changes_cmd::run(args),
        Cmd::Graph(args) => graph_cmd::run(args),
        Cmd::Open {
            targets,
            name,
            compare,
            dry_run,
            remote,
        } => open_cmd::run(targets, name, compare, dry_run, remote),
        Cmd::Project(c) => sources_cmd::project(c),
        Cmd::Pr { repo, number } => sources_cmd::pr(&repo, number),
        Cmd::Auth(c) => sources_cmd::auth(c),
        Cmd::Cache(c) => sources_cmd::cache_cmd(c),
        Cmd::Prepare(c) => prepare_cmd::run(c),
        Cmd::Credential { operation } => {
            sources_cmd::credential(&operation)?;
            Ok(Output::ok("", json!(null)))
        }
    }
}

fn index_cache() -> Result<IndexCache> {
    IndexCache::default_location()
        .ok_or_else(|| anyhow!("cannot find a cache directory (HOME is not set)"))
}

/// The cached index for `root`, if there is one.
fn open_index(root: &Path) -> Result<Option<Semantic>> {
    Ok(Semantic::open(&index_cache()?, root)?)
}

fn require_index(root: &Path) -> Result<Semantic> {
    open_index(root)?.ok_or_else(|| {
        anyhow!(
            "no semantic index for {}; run `cho index --root {}`",
            root.display(),
            root.display()
        )
    })
}

#[cfg(test)]
mod shorthand_tests {
    use super::*;
    use clap::CommandFactory;
    use std::ffi::OsString;

    fn expand(args: &[&str], existing: &[&str]) -> Vec<String> {
        let args = args.iter().map(OsString::from).collect();
        expand_shorthand(
            args,
            |w| w == "help" || Cli::command().find_subcommand(w).is_some(),
            |w| existing.iter().any(|e| w == *e),
        )
        .into_iter()
        .map(|a| a.to_string_lossy().into_owned())
        .collect()
    }

    #[test]
    fn a_path_means_open() {
        assert_eq!(expand(&["cho", "."], &["."]), ["cho", "open", "."]);
        assert_eq!(
            expand(
                &["cho", "--json", "src/lib.rs", "--dry-run"],
                &["src/lib.rs"]
            ),
            ["cho", "--json", "open", "src/lib.rs", "--dry-run"]
        );
    }

    #[test]
    fn several_paths_and_stdin_mean_open() {
        assert_eq!(
            expand(&["cho", "a.rs", "b.rs"], &["a.rs", "b.rs"]),
            ["cho", "open", "a.rs", "b.rs"]
        );
        assert_eq!(expand(&["cho", "-"], &["-"]), ["cho", "open", "-"]);
        // The name is the flag's value, not the first path.
        assert_eq!(
            expand(&["cho", "--name", "PR 412", "a.rs"], &["a.rs"]),
            ["cho", "open", "--name", "PR 412", "a.rs"]
        );
    }

    #[test]
    fn subcommand_names_win_over_folders() {
        assert_eq!(
            expand(&["cho", "search", "foo"], &["search"]),
            ["cho", "search", "foo"]
        );
        assert_eq!(
            expand(&["cho", "./search"], &["./search"]),
            ["cho", "open", "./search"]
        );
    }

    #[test]
    fn unknown_words_and_flags_are_left_to_clap() {
        assert_eq!(expand(&["cho", "nope"], &[]), ["cho", "nope"]);
        assert_eq!(expand(&["cho", "--version"], &[]), ["cho", "--version"]);
        assert_eq!(expand(&["chorong"], &[]), ["chorong"]);
    }
}

#[cfg(test)]
mod confined_read_tests {
    use super::*;

    /// `resolve_position` reads the repository file with the confined reader: a path that is
    /// (or has become) a symbolic link out of the root is refused, with the usual message.
    #[test]
    fn a_repository_file_is_read_only_inside_the_root() {
        let dir = std::env::temp_dir().join(format!("cho-cli-confined-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("repo/src")).unwrap();
        let base = dir.canonicalize().unwrap();
        let root = base.join("repo");
        std::fs::write(base.join("secret.rs"), "fn secret() {}\n").unwrap();
        std::fs::write(root.join("src/a.rs"), "fn a() {}\n").unwrap();
        let ok = read_source_under(&root, Path::new("src/a.rs"), &root.join("src/a.rs")).unwrap();
        assert_eq!(ok.text(), "fn a() {}\n");
        // Swapped for a link that leaves the root (after resolve_position's own checks).
        std::fs::remove_file(root.join("src/a.rs")).unwrap();
        std::os::unix::fs::symlink(base.join("secret.rs"), root.join("src/a.rs")).unwrap();
        let shown = root.join("src/a.rs");
        let err = read_source_under(&root, Path::new("src/a.rs"), &shown).unwrap_err();
        assert_eq!(err.to_string(), format!("cannot read {}", shown.display()));
        // The unconfined reader it replaced follows the link.
        assert!(read_source(&shown).is_ok());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
